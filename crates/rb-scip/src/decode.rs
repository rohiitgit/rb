//! Decoding an `index.scip` (design §5.1). Ranges become 0-based `[line, byte col, line, byte
//! col]` spans like Tree-sitter's; SCIP's columns are code units in the document's position
//! encoding, which scip-java and scip-typescript leave unset while meaning UTF-16, and SCIP's
//! lines are the indexer's, which also end at terminators Tree-sitter does not count as rows
//! (contract M3b amendment 59).

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Context;
use protobuf::Message as _;
use rb_model::Span;
use scip::types::{PositionEncoding, occurrence};
use serde::{Deserialize, Serialize};

/// The `SymbolRole` bit the join reads (scip.proto); the others (import, write, read) are kept
/// in `Occurrence::roles` but have no reader.
pub const ROLE_DEFINITION: i32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ScipIndex {
    /// `Metadata.project_root` as written (a `file://` URL for both indexers).
    pub project_root: String,
    pub tool: String,
    /// One entry per distinct relative path, sorted.
    pub documents: Vec<Document>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Document {
    /// Normalised repo-relative path (`./` and duplicate separators removed).
    pub path: String,
    pub language: String,
    pub occurrences: Vec<Occurrence>,
    pub symbols: Vec<SymbolInfo>,
    /// Occurrences whose columns could not be converted (no source text): kept as code units.
    /// Both supported indexers leave the encoding unset and mean UTF-16, which is assumed then.
    pub unconverted: usize,
    /// Set when the text shows that the indexer and Tree-sitter may read the file differently
    /// (contract M3b amendment 64); the join then leaves the whole file to Tier B.
    pub rejected: Option<Rejection>,
    /// Each local symbol's name as the text reads it at its first definition occurrence (a
    /// single-line span with text; contract M3b amendment 66): scip-typescript gives locals no
    /// display name, so this is what the join's name guard compares a token with.
    pub local_names: BTreeMap<String, String>,
}

/// Why a document the index has does not own its file (contract M3b amendment 64): a sign that
/// the indexer and Tree-sitter read the file differently, so the indexer's positions may name
/// other tokens than Tree-sitter's. The decoder decides the first three and the fourth's
/// caller ([`crate::jobs::maven_source_encoding`]); the join decides the last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Rejection {
    /// The blob is not valid UTF-8, which Tree-sitter's spans and the column conversion assume.
    NotUtf8,
    /// The index carries `Document.text` and it is not the blob.
    TextDiffers,
    /// The build declares a source encoding other than UTF-8 and the blob is not ASCII.
    SourceEncoding,
    /// scip-java: the blob holds a lone `\r` or a `\u000a`/`\u000d` escape, which javac reads
    /// as a line terminator (also inside a `//` comment) and Tree-sitter-java does not.
    CommentTerminator,
    /// A global method or type definition occurrence that no Tree-sitter definition accounts for.
    DefinitionDisagreement,
}

impl Rejection {
    pub const ALL: [Rejection; 5] = [
        Rejection::NotUtf8,
        Rejection::TextDiffers,
        Rejection::SourceEncoding,
        Rejection::CommentTerminator,
        Rejection::DefinitionDisagreement,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Rejection::NotUtf8 => "not_utf8",
            Rejection::TextDiffers => "text_differs",
            Rejection::SourceEncoding => "source_encoding",
            Rejection::CommentTerminator => "comment_terminator",
            Rejection::DefinitionDisagreement => "definition_disagreement",
        }
    }
}

/// Ordered by span, then symbol, roles and enclosing range: the order of a document's
/// occurrences after decoding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Occurrence {
    pub span: Span,
    pub symbol: String,
    /// `SymbolRole` bitset.
    pub roles: i32,
    /// The enclosing definition or expression, when the indexer emitted one.
    pub enclosing: Option<Span>,
}

impl Occurrence {
    pub fn is_definition(&self) -> bool {
        self.roles & ROLE_DEFINITION != 0
    }
}

/// Ordered by symbol first: the order of a document's symbols after decoding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub symbol: String,
    /// `SymbolInformation.Kind` by name (`Class`, `Method`, …); empty when unspecified.
    pub kind: String,
    /// The symbol's name as written; read for local symbols, whose string carries no name
    /// (contract M3b amendment 54).
    pub display_name: String,
    pub relationships: Vec<Relationship>,
}

/// The two relationship flags the join and its diagnostics read; `is_type_definition` and
/// `is_definition` have no reader and are not decoded.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
pub struct Relationship {
    pub symbol: String,
    pub is_implementation: bool,
    pub is_reference: bool,
}

/// Decodes `bytes`. `source_of(path)` supplies a document's text (for converting columns to
/// bytes) when the index does not carry it.
pub fn decode(
    bytes: &[u8],
    source_of: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> anyhow::Result<ScipIndex> {
    let index = scip::types::Index::parse_from_bytes(bytes).context("parsing index.scip")?;
    let metadata = index.metadata.as_ref();
    let tool = metadata
        .and_then(|m| m.tool_info.as_ref())
        .map_or("", |t| t.name.as_str());
    let scip_java = tool == "scip-java";
    let breaks = match tool {
        "scip-java" => LineBreaks::Java,
        "scip-typescript" => LineBreaks::TypeScript,
        _ => LineBreaks::Lf,
    };
    // Duplicate documents for one path (scip-typescript #330) merge into ordered sets, which
    // drop repeats and give the decoded order without a per-item scan.
    struct Merged {
        document: Document,
        occurrences: BTreeSet<Occurrence>,
        symbols: BTreeSet<SymbolInfo>,
    }
    let mut documents: BTreeMap<String, Merged> = BTreeMap::new();
    for doc in &index.documents {
        let path = normalise(&doc.relative_path);
        let encoding = doc.position_encoding.enum_value_or_default();
        let units_per = match encoding {
            PositionEncoding::UTF8CodeUnitOffsetFromLineStart => Units::Bytes,
            PositionEncoding::UTF32CodeUnitOffsetFromLineStart => Units::Chars,
            PositionEncoding::UTF16CodeUnitOffsetFromLineStart
            | PositionEncoding::UnspecifiedPositionEncoding => {
                if scip_java {
                    Units::Javac
                } else {
                    Units::Utf16
                }
            }
        };
        let blob = source_of(&path);
        let rejected = blob
            .as_deref()
            .and_then(|blob| rejection(blob, &doc.text, scip_java));
        let text: Option<Vec<u8>> = if doc.text.is_empty() {
            blob
        } else {
            Some(doc.text.clone().into_bytes())
        };
        let lines = text.as_deref().map(|t| Lines::new(t, breaks));
        let mut unconverted = 0;
        // `None` when the line's text is unknown: the position then stays the indexer's.
        let convert = |line: i32, col: i32| -> Option<(i32, i32)> {
            match &lines {
                Some(lines) => lines.position(line, col, units_per),
                None => (units_per == Units::Bytes).then_some((line, col)),
            }
        };
        let mut occurrences = Vec::with_capacity(doc.occurrences.len());
        for occ in &doc.occurrences {
            let Some((sl, sc, el, ec)) = range_of(&occ.typed_range, &occ.range) else {
                continue;
            };
            let mut failed = false;
            let mut span_of = |(sl, sc, el, ec): (i32, i32, i32, i32)| -> Span {
                let mut at = |line: i32, col: i32| {
                    convert(line, col).unwrap_or_else(|| {
                        failed = true;
                        (line, col)
                    })
                };
                let (start, end) = (at(sl, sc), at(el, ec));
                [start.0, start.1, end.0, end.1]
            };
            let span = span_of((sl, sc, el, ec));
            let enclosing =
                enclosing_of(&occ.typed_enclosing_range, &occ.enclosing_range).map(&mut span_of);
            if failed {
                unconverted += 1;
            }
            occurrences.push(Occurrence {
                span,
                symbol: occ.symbol.clone(),
                roles: occ.symbol_roles,
                enclosing,
            });
        }
        let mut local_names = BTreeMap::new();
        if let Some(lines) = &lines {
            for o in occurrences
                .iter()
                .filter(|o| o.is_definition() && o.symbol.starts_with("local "))
            {
                if let Some(name) = lines.text_at(o.span) {
                    local_names.entry(o.symbol.clone()).or_insert(name);
                }
            }
        }
        let entry = documents.entry(path.clone()).or_insert_with(|| Merged {
            document: Document {
                path,
                language: doc.language.clone(),
                ..Document::default()
            },
            occurrences: BTreeSet::new(),
            symbols: BTreeSet::new(),
        });
        entry.occurrences.extend(occurrences);
        entry.symbols.extend(doc.symbols.iter().map(symbol_info));
        entry.document.unconverted += unconverted;
        entry.document.rejected = entry.document.rejected.or(rejected);
        for (symbol, name) in local_names {
            entry.document.local_names.entry(symbol).or_insert(name);
        }
    }
    let documents = documents
        .into_values()
        .map(|m| Document {
            occurrences: m.occurrences.into_iter().collect(),
            symbols: m.symbols.into_iter().collect(),
            ..m.document
        })
        .collect();
    Ok(ScipIndex {
        project_root: metadata.map(|m| m.project_root.clone()).unwrap_or_default(),
        tool: metadata
            .and_then(|m| m.tool_info.as_ref())
            .map(|t| format!("{} {}", t.name, t.version))
            .unwrap_or_default(),
        documents,
    })
}

/// The first sign in `blob` (and the index's `text`, when it has one) that the indexer and
/// Tree-sitter may read the file differently (contract M3b amendment 64).
fn rejection(blob: &[u8], text: &str, scip_java: bool) -> Option<Rejection> {
    if std::str::from_utf8(blob).is_err() {
        return Some(Rejection::NotUtf8);
    }
    if !text.is_empty() && text.as_bytes() != blob {
        return Some(Rejection::TextDiffers);
    }
    (scip_java && has_javac_only_line_terminator(blob)).then_some(Rejection::CommentTerminator)
}

/// A lone `\r`, or a Unicode escape `\u000a`/`\u000d` (any number of `u`, either case; JLS
/// §3.3 translates escapes before comments end), anywhere in the text: javac ends a `//`
/// comment there and Tree-sitter-java does not. A literal `\\u000a` (an escaped backslash) is
/// rejected too: the rule is kept simple, and a false rejection costs coverage only.
fn has_javac_only_line_terminator(blob: &[u8]) -> bool {
    blob.iter().enumerate().any(|(i, &b)| match b {
        b'\r' => blob.get(i + 1) != Some(&b'\n'),
        b'\\' if blob.get(i + 1) == Some(&b'u') => {
            let digits = &blob[i + 1..];
            let digits = &digits[digits.iter().take_while(|&&c| c == b'u').count()..];
            digits.len() >= 4
                && (digits[..4].eq_ignore_ascii_case(b"000a")
                    || digits[..4].eq_ignore_ascii_case(b"000d"))
        }
        _ => false,
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Units {
    Bytes,
    Utf16,
    Chars,
    /// scip-java's columns (contract M3b amendment 53): javac's `LineMap` advances a tab to the
    /// next multiple of 8 and any other character by its UTF-16 length, and scip-java then
    /// subtracts 7 for each tab that starts the line. On a line without a tab this is `Utf16`.
    Javac,
}

/// Where an indexer starts a new line (contract M3b amendment 59; measured with
/// `rb-scip-java:0.13.1` and scip-typescript 0.4.0). Tree-sitter's rows end at `\n` only.
#[derive(Clone, Copy, PartialEq, Eq)]
enum LineBreaks {
    /// `\n` only (an indexer whose rule is unknown is read as Tree-sitter reads the text).
    Lf,
    /// javac's `LineMap`: `\n`, `\r` and `\r\n` (the Java line terminators, JLS §3.4).
    Java,
    /// TypeScript's scanner (`computeLineStarts`): those and U+2028, U+2029.
    TypeScript,
}

/// A document's text indexed twice: by the indexer's lines and by Tree-sitter's rows. An
/// indexer position converts through its byte offset in the text, so a line terminator only
/// one side counts cannot shift a position onto another line.
struct Lines<'t> {
    text: &'t [u8],
    /// Each indexer line's bytes, `start..end`, without its terminator.
    lines: Vec<(usize, usize)>,
    /// The byte offset each Tree-sitter row starts at.
    rows: Vec<usize>,
}

impl<'t> Lines<'t> {
    fn new(text: &'t [u8], breaks: LineBreaks) -> Self {
        let mut lines = Vec::new();
        // TypeScript's `sys.readFile` drops a leading byte-order mark, so scip-typescript's
        // line 0 starts after it (contract M3b amendment 65). javac rejects one (no index).
        let mut start = if breaks == LineBreaks::TypeScript && text.starts_with(b"\xEF\xBB\xBF") {
            3
        } else {
            0
        };
        let mut i = start;
        while i < text.len() {
            let terminator = match (text[i], breaks) {
                (b'\n', _) => 1,
                (b'\r', LineBreaks::Java | LineBreaks::TypeScript) => {
                    if text.get(i + 1) == Some(&b'\n') {
                        2
                    } else {
                        1
                    }
                }
                // U+2028 and U+2029 are `E2 80 A8` and `E2 80 A9`.
                (0xE2, LineBreaks::TypeScript)
                    if text.get(i + 1) == Some(&0x80)
                        && matches!(text.get(i + 2), Some(0xA8 | 0xA9)) =>
                {
                    3
                }
                _ => 0,
            };
            if terminator == 0 {
                i += 1;
                continue;
            }
            lines.push((start, i));
            i += terminator;
            start = i;
        }
        lines.push((start, text.len()));
        let rows = std::iter::once(0)
            .chain(
                text.iter()
                    .enumerate()
                    .filter(|&(_, &b)| b == b'\n')
                    .map(|(i, _)| i + 1),
            )
            .collect();
        Lines { text, lines, rows }
    }

    /// The text of a converted single-line span (Tree-sitter rows, byte columns), when it is
    /// non-empty UTF-8 inside one row.
    fn text_at(&self, span: Span) -> Option<String> {
        if span[0] != span[2] || span[1] >= span[3] {
            return None;
        }
        let row = *self.rows.get(usize::try_from(span[0]).ok()?)?;
        let start = row + usize::try_from(span[1]).ok()?;
        let end = row + usize::try_from(span[3]).ok()?;
        let bytes = self.text.get(start..end)?;
        (!bytes.contains(&b'\n'))
            .then(|| std::str::from_utf8(bytes).ok().map(str::to_owned))
            .flatten()
    }

    /// Tree-sitter's `(row, byte column)` for the indexer's `(line, column)`, the column in
    /// `units`; `None` for a line the text does not have.
    fn position(&self, line: i32, col: i32, units: Units) -> Option<(i32, i32)> {
        let &(start, end) = self.lines.get(usize::try_from(line).ok()?)?;
        let offset =
            start + usize::try_from(byte_column(&self.text[start..end], col, units)).ok()?;
        // `rows[0]` is 0, so at least one row starts at or before the offset.
        let row = self.rows.partition_point(|&r| r <= offset) - 1;
        Some((
            i32::try_from(row).ok()?,
            i32::try_from(offset - self.rows[row]).ok()?,
        ))
    }
}

/// The byte offset of code unit `col` within `line`. A column past the end clamps to the line's
/// length; a column inside a multi-unit character clamps to the next character's start, and a
/// javac column inside a tab's expansion maps to the tab.
fn byte_column(line: &[u8], col: i32, units: Units) -> i32 {
    let Ok(mut target) = usize::try_from(col) else {
        return 0;
    };
    if units == Units::Javac {
        // Undo scip-java's correction of leading tabs (7 per tab) to get javac's column.
        let leading = line.iter().take_while(|&&b| b == b'\t').count();
        target = target.saturating_add(7 * leading);
    }
    let text = String::from_utf8_lossy(line);
    let mut seen = 0usize;
    for (byte, ch) in text.char_indices() {
        if seen >= target {
            return i32::try_from(byte).unwrap_or(i32::MAX);
        }
        if units == Units::Javac && ch == '\t' {
            seen = (seen / 8 + 1) * 8;
            if seen > target {
                return i32::try_from(byte).unwrap_or(i32::MAX);
            }
            continue;
        }
        seen += match units {
            Units::Utf16 | Units::Javac => ch.len_utf16(),
            Units::Chars => 1,
            Units::Bytes => ch.len_utf8(),
        };
    }
    i32::try_from(text.len()).unwrap_or(i32::MAX)
}

/// `(start line, start col, end line, end col)`: the typed range, else the deprecated form.
fn range_of(typed: &Option<occurrence::Typed_range>, raw: &[i32]) -> Option<(i32, i32, i32, i32)> {
    match typed {
        Some(occurrence::Typed_range::SingleLineRange(r)) => {
            Some((r.line, r.start_character, r.line, r.end_character))
        }
        Some(occurrence::Typed_range::MultiLineRange(r)) => {
            Some((r.start_line, r.start_character, r.end_line, r.end_character))
        }
        // The generated `oneof` enums are non-exhaustive; an unknown form falls back.
        _ => raw_range(raw),
    }
}

fn enclosing_of(
    typed: &Option<occurrence::Typed_enclosing_range>,
    raw: &[i32],
) -> Option<(i32, i32, i32, i32)> {
    match typed {
        Some(occurrence::Typed_enclosing_range::SingleLineEnclosingRange(r)) => {
            Some((r.line, r.start_character, r.line, r.end_character))
        }
        Some(occurrence::Typed_enclosing_range::MultiLineEnclosingRange(r)) => {
            Some((r.start_line, r.start_character, r.end_line, r.end_character))
        }
        _ => raw_range(raw),
    }
}

fn raw_range(raw: &[i32]) -> Option<(i32, i32, i32, i32)> {
    match *raw {
        [l, sc, ec] => Some((l, sc, l, ec)),
        [sl, sc, el, ec] => Some((sl, sc, el, ec)),
        _ => None,
    }
}

fn symbol_info(s: &scip::types::SymbolInformation) -> SymbolInfo {
    let kind = s
        .kind
        .enum_value()
        .map(|k| format!("{k:?}"))
        .unwrap_or_default();
    SymbolInfo {
        symbol: s.symbol.clone(),
        display_name: s.display_name.clone(),
        kind: if kind == "UnspecifiedKind" {
            String::new()
        } else {
            kind
        },
        relationships: s
            .relationships
            .iter()
            .map(|r| Relationship {
                symbol: r.symbol.clone(),
                is_implementation: r.is_implementation,
                is_reference: r.is_reference,
            })
            .collect(),
    }
}

/// Repo-relative path without `./`, `//` or a trailing `/`.
fn normalise(path: &str) -> String {
    let parts: Vec<&str> = path
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use scip::types::{
        Document as ScipDocument, Index, Metadata, MultiLineRange, Occurrence as ScipOccurrence,
        Relationship as ScipRelationship, SingleLineRange, SymbolInformation, ToolInfo, occurrence,
        symbol_information,
    };

    use super::*;

    fn occ(symbol: &str, roles: i32) -> ScipOccurrence {
        ScipOccurrence {
            symbol: symbol.into(),
            symbol_roles: roles,
            ..Default::default()
        }
    }

    fn encode(index: &Index) -> Vec<u8> {
        index.write_to_bytes().unwrap()
    }

    #[test]
    fn typed_and_deprecated_ranges_decode_with_utf16_columns_converted() {
        // Line 0: "🚀 Woo" ('W' is UTF-16 offset 3, byte offset 5); line 1: "ab".
        let text = "🚀 Woo\nab\n";
        let mut typed = occ("scip-java maven . . p/K#", ROLE_DEFINITION);
        typed.typed_range = Some(occurrence::Typed_range::SingleLineRange(SingleLineRange {
            line: 0,
            start_character: 3,
            end_character: 6,
            ..Default::default()
        }));
        typed.typed_enclosing_range = Some(
            occurrence::Typed_enclosing_range::MultiLineEnclosingRange(MultiLineRange {
                start_line: 0,
                start_character: 0,
                end_line: 1,
                end_character: 2,
                ..Default::default()
            }),
        );
        let mut raw3 = occ("local 1", 8);
        raw3.range = vec![1, 0, 2];
        let mut raw4 = occ("scip-typescript npm p 1.0.0 `a.ts`/f().", 0);
        raw4.range = vec![0, 3, 1, 1];
        let mut bad = occ("x", 0);
        bad.range = vec![1, 2];
        let doc = ScipDocument {
            relative_path: "./src//a.ts".into(),
            language: "typescript".into(),
            text: text.into(),
            occurrences: vec![typed, raw3, raw4, bad],
            ..Default::default()
        };
        let index = Index {
            metadata: Some(Metadata {
                project_root: "file:///sources".into(),
                tool_info: Some(ToolInfo {
                    name: "scip-java".into(),
                    version: "0.13.1".into(),
                    ..Default::default()
                })
                .into(),
                ..Default::default()
            })
            .into(),
            documents: vec![doc],
            ..Default::default()
        };
        let got = decode(&encode(&index), &|_| None).unwrap();
        assert_eq!(got.project_root, "file:///sources");
        assert_eq!(got.tool, "scip-java 0.13.1");
        assert_eq!(got.documents.len(), 1);
        let d = &got.documents[0];
        assert_eq!(d.path, "src/a.ts");
        assert_eq!(d.unconverted, 0);
        let spans: Vec<(String, Span, Option<Span>)> = d
            .occurrences
            .iter()
            .map(|o| (o.symbol.clone(), o.span, o.enclosing))
            .collect();
        assert_eq!(
            spans,
            [
                (
                    "scip-java maven . . p/K#".into(),
                    [0, 5, 0, 8],
                    Some([0, 0, 1, 2])
                ),
                (
                    "scip-typescript npm p 1.0.0 `a.ts`/f().".into(),
                    [0, 5, 1, 1],
                    None
                ),
                ("local 1".into(), [1, 0, 1, 2], None),
            ]
        );
        assert!(d.occurrences[0].is_definition());
    }

    #[test]
    fn source_text_comes_from_the_repo_when_the_index_has_none() {
        let mut o = occ("s", 0);
        o.range = vec![0, 2, 3];
        let doc = ScipDocument {
            relative_path: "é.ts".into(),
            occurrences: vec![o.clone(), o],
            ..Default::default()
        };
        let index = Index {
            documents: vec![doc.clone(), doc],
            ..Default::default()
        };
        // "é" is 1 UTF-16 unit, 2 bytes: column 2 becomes byte 3.
        let got = decode(&encode(&index), &|p| {
            (p == "é.ts").then(|| b"\xc3\xa9xy".to_vec())
        })
        .unwrap();
        assert_eq!(got.documents.len(), 1, "duplicate documents merge");
        assert_eq!(
            got.documents[0].occurrences.len(),
            1,
            "duplicate occurrences merge"
        );
        assert_eq!(got.documents[0].occurrences[0].span, [0, 3, 0, 4]);
        // Without any text the columns stay as code units and are counted.
        let got = decode(&encode(&index), &|_| None).unwrap();
        assert_eq!(got.documents[0].occurrences[0].span, [0, 2, 0, 3]);
        assert_eq!(got.documents[0].unconverted, 4);
    }

    #[test]
    fn symbol_information_and_relationships() {
        let si = SymbolInformation {
            symbol: "scip-java maven . . p/Dog#".into(),
            display_name: "Dog".into(),
            kind: symbol_information::Kind::Class.into(),
            relationships: vec![ScipRelationship {
                symbol: "scip-java maven . . p/Animal#".into(),
                is_implementation: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let index = Index {
            documents: vec![ScipDocument {
                relative_path: "p/Dog.java".into(),
                symbols: vec![si.clone()],
                ..Default::default()
            }],
            // Externally defined symbols are read from occurrences, never from here.
            external_symbols: vec![SymbolInformation {
                symbol: "scip-java maven jdk 17 java/lang/Object#".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let got = decode(&encode(&index), &|_| None).unwrap();
        let s = &got.documents[0].symbols[0];
        assert_eq!(s.kind, "Class");
        assert_eq!(s.display_name, "Dog");
        assert_eq!(s.relationships[0].symbol, "scip-java maven . . p/Animal#");
        assert!(s.relationships[0].is_implementation && !s.relationships[0].is_reference);
    }

    /// Merging duplicate documents must not compare every occurrence with every other one:
    /// 40k occurrences over two copies of one document decode in well under a second, and the
    /// result keeps one copy of each, in span order.
    #[test]
    fn decoding_forty_thousand_occurrences_is_fast() {
        let occurrences: Vec<ScipOccurrence> = (0..20_000)
            .map(|i| {
                let mut o = occ(&format!("scip-java maven . . p/S{i}#"), ROLE_DEFINITION);
                o.range = vec![i, 0, 4];
                o
            })
            .collect();
        let doc = ScipDocument {
            relative_path: "a.java".into(),
            occurrences,
            symbols: (0..2_000)
                .map(|i| SymbolInformation {
                    symbol: format!("scip-java maven . . p/S{i}#"),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        };
        let index = Index {
            documents: vec![doc.clone(), doc],
            ..Default::default()
        };
        let bytes = encode(&index);
        let started = std::time::Instant::now();
        let got = decode(&bytes, &|_| None).unwrap();
        let elapsed = started.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "decoding took {elapsed:?}"
        );
        let d = &got.documents[0];
        assert_eq!(d.occurrences.len(), 20_000);
        assert_eq!(d.symbols.len(), 2_000);
        assert!(d.occurrences.windows(2).all(|w| w[0].span < w[1].span));
        assert!(d.symbols.windows(2).all(|w| w[0].symbol < w[1].symbol));
    }

    /// Contract M3b amendment 53: scip-java reports javac's columns, which advance a tab to
    /// the next multiple of 8 and subtract 7 per tab that starts the line. The lines and raw
    /// columns are the ones measured with `rb-scip-java:0.13.1` (leading, interior, double and
    /// mixed tabs, a tab inside a string literal, non-ASCII characters before a tab); each
    /// occurrence must land on its token's bytes. A scip-typescript index with the same columns
    /// is read as UTF-16 only.
    #[test]
    fn javac_columns_expand_tabs_to_multiples_of_eight() {
        // (line text, token, its nth occurrence on the line, raw start, raw end); a token
        // `g(s` stands for the name before its `(`.
        let measured: &[(&str, &str, usize, i32, i32)] = &[
            (
                "public interface Wide extends\t\tBox<Doer>,\tYyyyyyyy {",
                "Box",
                0,
                40,
                43,
            ),
            (
                "public interface Wide extends\t\tBox<Doer>,\tYyyyyyyy {",
                "Doer",
                0,
                44,
                48,
            ),
            (
                "public interface Wide extends\t\tBox<Doer>,\tYyyyyyyy {",
                "Yyyyyyyy",
                0,
                56,
                64,
            ),
            (
                "public class Tabby implements\tNamed,\tSized {",
                "Named",
                0,
                32,
                37,
            ),
            (
                "public class Tabby implements\tNamed,\tSized {",
                "Sized",
                0,
                40,
                45,
            ),
            (
                "\tpublic String name() { return \"t\"; }\tpublic int size() { return 1; }",
                "size",
                0,
                52,
                56,
            ),
            ("\tint a;\t\tint b = 1;", "b", 0, 21, 22),
            (
                "\t\t\tvoid m1() { h(); }\tvoid m2() { m1();\t\tk(); }",
                "m1",
                0,
                8,
                10,
            ),
            (
                "\t\t\tvoid m1() { h(); }\tvoid m2() { m1();\t\tk(); }",
                "m2",
                0,
                32,
                34,
            ),
            (
                "\t\t\tvoid m1() { h(); }\tvoid m2() { m1();\t\tk(); }",
                "m1",
                1,
                39,
                41,
            ),
            (
                "\t\t\tvoid m1() { h(); }\tvoid m2() { m1();\t\tk(); }",
                "k",
                0,
                59,
                60,
            ),
            ("    \t@Override public void why() {}", "Override", 0, 9, 17),
            ("    \t@Override public void why() {}", "why", 0, 30, 33),
            ("  \tvoid g(int x, int y) {}", "g", 0, 13, 14),
            (
                "\t@Override\tpublic String toString() { return \"a\tb\" + h(); }",
                "h",
                0,
                62,
                63,
            ),
            (
                "\tvoid f() {\tg(1,\t2);\tString s = \"x\ty\";\tg(s.length(),  3); }",
                "g(s",
                0,
                57,
                58,
            ),
            (
                "\tvoid f() {\tg(1,\t2);\tString s = \"x\ty\";\tg(s.length(),  3); }",
                "length",
                0,
                61,
                67,
            ),
            (
                "\tvoid u() { String s = \"\u{e9}\t\u{1f600}\"; g(s.length(),\t1); }",
                "g(s",
                0,
                38,
                39,
            ),
            (
                "\tvoid u() { String s = \"\u{e9}\t\u{1f600}\"; g(s.length(),\t1); }",
                "length",
                0,
                42,
                48,
            ),
        ];
        let index_of = |tool: &str| {
            let text: String = measured.iter().map(|m| format!("{}\n", m.0)).collect();
            let occurrences = measured
                .iter()
                .enumerate()
                .map(|(line, &(_, _, _, start, end))| {
                    let mut o = occ(&format!("scip-java maven . . p/T#m{line}()."), 0);
                    o.typed_range =
                        Some(occurrence::Typed_range::SingleLineRange(SingleLineRange {
                            line: line as i32,
                            start_character: start,
                            end_character: end,
                            ..Default::default()
                        }));
                    o
                })
                .collect();
            Index {
                metadata: Some(Metadata {
                    tool_info: Some(ToolInfo {
                        name: tool.into(),
                        ..Default::default()
                    })
                    .into(),
                    ..Default::default()
                })
                .into(),
                documents: vec![ScipDocument {
                    relative_path: "T.java".into(),
                    text,
                    occurrences,
                    ..Default::default()
                }],
                ..Default::default()
            }
        };
        let expected: Vec<Span> = measured
            .iter()
            .enumerate()
            .map(|(line, &(text, token, nth, ..))| {
                let col = text.match_indices(token).nth(nth).unwrap().0 as i32;
                let len = token.find('(').unwrap_or(token.len()) as i32;
                [line as i32, col, line as i32, col + len]
            })
            .collect();
        let spans = |tool: &str| -> Vec<Span> {
            let got = decode(&encode(&index_of(tool)), &|_| None).unwrap();
            let mut spans: Vec<Span> = got.documents[0]
                .occurrences
                .iter()
                .map(|o| o.span)
                .collect();
            spans.sort();
            spans
        };
        assert_eq!(spans("scip-java"), expected);
        // The same columns from another indexer are UTF-16 units: no tab expansion.
        let utf16 = spans("scip-typescript");
        assert_ne!(utf16[0], expected[0]);
        assert_eq!(
            utf16[0],
            [0, 40, 0, 43],
            "Box's raw columns, read as code units"
        );
    }

    /// Decodes one document of `tool` holding `text`, with one occurrence per measured
    /// `(token, nth, raw line, raw start, raw end)`, and returns the decoded spans next to the
    /// spans Tree-sitter gives the tokens: the `nth` match of `token` in `text` (a token `b()`
    /// stands for the name before its `(`), its row counted by `\n` only and its column in bytes.
    fn decoded_and_expected(
        tool: &str,
        text: &str,
        measured: &[(&str, usize, i32, i32, i32)],
    ) -> (Vec<Span>, Vec<Span>) {
        let occurrences = measured
            .iter()
            .enumerate()
            .map(|(i, &(_, _, line, start, end))| {
                let mut o = occ(&format!("scip-java maven . . p/T#m{i}()."), 0);
                o.range = vec![line, start, end];
                o
            })
            .collect();
        let index = Index {
            metadata: Some(Metadata {
                tool_info: Some(ToolInfo {
                    name: tool.into(),
                    ..Default::default()
                })
                .into(),
                ..Default::default()
            })
            .into(),
            documents: vec![ScipDocument {
                relative_path: "T".into(),
                occurrences,
                ..Default::default()
            }],
            ..Default::default()
        };
        let got = decode(&encode(&index), &|_| Some(text.as_bytes().to_vec())).unwrap();
        let d = &got.documents[0];
        assert_eq!(d.unconverted, 0);
        // Decoded occurrences are sorted by span; the symbols give back the measured order.
        let mut decoded: Vec<(String, Span)> = d
            .occurrences
            .iter()
            .map(|o| (o.symbol.clone(), o.span))
            .collect();
        decoded.sort_by_key(|(symbol, _)| {
            symbol
                .trim_start_matches("scip-java maven . . p/T#m")
                .trim_end_matches("().")
                .parse::<usize>()
                .unwrap()
        });
        let expected = measured
            .iter()
            .map(|&(token, nth, ..)| {
                let offset = text.match_indices(token).nth(nth).unwrap().0;
                let row = text[..offset].matches('\n').count() as i32;
                let col = (offset - text[..offset].rfind('\n').map_or(0, |n| n + 1)) as i32;
                let len = token.find('(').unwrap_or(token.len()) as i32;
                [row, col, row, col + len]
            })
            .collect();
        (
            decoded.into_iter().map(|(_, span)| span).collect(),
            expected,
        )
    }

    /// Contract M3b amendment 59: javac numbers lines after `\n`, `\r` and `\r\n`, and
    /// TypeScript's scanner also after U+2028 and U+2029, while Tree-sitter's rows count `\n`
    /// only. The texts and raw ranges are the ones measured with `rb-scip-java:0.13.1` and
    /// scip-typescript 0.4.0 (`var/fix-pass6/eol`): a lone CR between members (the evaluator's
    /// `LoneCr2`), lone CRs before tab-indented lines and inside a block comment, U+2028, U+2029
    /// and U+0085 inside a Java string literal and comment (not line terminators for javac), and
    /// U+2028 inside a TypeScript string literal, U+2029 in a line comment and U+0085 in a block
    /// comment (not a terminator for TypeScript). Every occurrence must land on its token.
    #[test]
    fn indexer_lines_follow_the_indexers_line_terminators() {
        let lone_cr2 = "package t;\n\npublic class LoneCr2 {\n\tint m1() { return 0; }\r\tint m0() { return 1; }\n\tint m2() { return zeta(); }\n\tint m3() { return beta(); }\n\tint zeta() { return 2; }\n\tint beta() { return 3; }\n}\n";
        let lone_cr2_measured: &[(&str, usize, i32, i32, i32)] = &[
            ("m1", 0, 3, 5, 7),
            ("m0", 0, 4, 5, 7),
            ("m2", 0, 5, 5, 7),
            ("zeta", 0, 5, 19, 23),
            ("m3", 0, 6, 5, 7),
            ("beta", 0, 6, 19, 23),
            ("zeta", 1, 7, 5, 9),
            ("beta", 1, 8, 5, 9),
        ];
        let (got, expected) = decoded_and_expected("scip-java", lone_cr2, lone_cr2_measured);
        assert_eq!(got, expected, "LoneCr2.java");
        let lone_cr_tab = "package t;\n\npublic class LoneCrTab {\n\tint a() { return 0; }\r\t\tint b()\t{ return a(); }\r\r\tint c() { return\tb(); }\n  /* x\r y */ int d() { return c(); }\n}\n";
        let (got, expected) = decoded_and_expected(
            "scip-java",
            lone_cr_tab,
            &[
                ("a()", 0, 3, 5, 6),
                ("b()", 0, 4, 6, 7),
                ("a()", 1, 4, 19, 20),
                ("c()", 0, 6, 5, 6),
                ("b()", 1, 6, 25, 26),
                ("d()", 0, 8, 10, 11),
                ("c()", 1, 8, 23, 24),
            ],
        );
        assert_eq!(got, expected, "LoneCrTab.java");
        let java_ls = "package t;\n\npublic class Ls {\n\tString s() { return \"a\u{2028}b\u{2029}c\u{85}d\"; }\n\t// note \u{2028} here \u{2029} and \u{85}\n\tint a() { return s().length(); } int b() { return a(); }\n\tint c() { return b(); }\n}\n";
        let (got, expected) = decoded_and_expected(
            "scip-java",
            java_ls,
            &[
                ("s()", 0, 3, 8, 9),
                ("a()", 0, 5, 5, 6),
                ("s()", 1, 5, 18, 19),
                ("length", 0, 5, 22, 28),
                ("b()", 0, 5, 38, 39),
                ("a()", 1, 5, 51, 52),
                ("c()", 0, 6, 5, 6),
                ("b()", 1, 6, 18, 19),
            ],
        );
        assert_eq!(got, expected, "Ls.java");
        let crlf = "package t;\r\n\r\npublic class Crlf {\r\n\tint a() { return 0; }\r\n\tint b()\t{ return a(); }\r\n}\r\n";
        let (got, expected) = decoded_and_expected(
            "scip-java",
            crlf,
            &[
                ("a()", 0, 3, 5, 6),
                ("b()", 0, 4, 5, 6),
                ("a()", 1, 4, 18, 19),
            ],
        );
        assert_eq!(got, expected, "Crlf.java");

        let cr_ts = "export class K {\n  m1() { return 0; }\r  m0() { return 1; }\n  m2() { return this.zeta(); }\n  m3() { return this.beta(); }\n  zeta() { return 2; }\n  beta() { return 3; }\n}\n";
        let (got, expected) = decoded_and_expected(
            "scip-typescript",
            cr_ts,
            &[
                ("m1", 0, 1, 2, 4),
                ("m0", 0, 2, 2, 4),
                ("m2", 0, 3, 2, 4),
                ("zeta", 0, 3, 21, 25),
                ("m3", 0, 4, 2, 4),
                ("beta", 0, 4, 21, 25),
                ("zeta", 1, 5, 2, 6),
                ("beta", 1, 6, 2, 6),
            ],
        );
        assert_eq!(got, expected, "cr.ts");
        let ls_ts = "export class L {\n  m1() { return \"a\u{2028}b\"; }\n  m0() { return 1; }\n  m2() { return this.zeta(); }\n  // c \u{2029} d\n  m3() { return this.beta(); }\n  /* e \u{85} f */ m4() { return this.m3(); }\n  zeta() { return 2; }\n  beta() { return 3; }\n}\n";
        let ls_ts_measured: &[(&str, usize, i32, i32, i32)] = &[
            ("m1", 0, 1, 2, 4),
            ("m0", 0, 3, 2, 4),
            ("m2", 0, 4, 2, 4),
            ("zeta", 0, 4, 21, 25),
            ("d\n", 0, 6, 1, 2),
            ("m3", 0, 7, 2, 4),
            ("beta", 0, 7, 21, 25),
            ("m4", 0, 8, 14, 16),
            ("m3", 1, 8, 33, 35),
            ("zeta", 1, 9, 2, 6),
            ("beta", 1, 10, 2, 6),
        ];
        let (got, mut expected) = decoded_and_expected("scip-typescript", ls_ts, ls_ts_measured);
        // `d` ends the line comment's U+2029 line (scip-typescript reads it as a class
        // property): one byte, without the `\n` that located it.
        expected[4][3] -= 1;
        assert_eq!(got, expected, "ls.ts");

        // The rules are the indexers' own: read as scip-java's, the TypeScript file's U+2028
        // is not a line break and `m0` lands elsewhere; read by `\n` alone, the lone CR shifts
        // `m0` onto `m2` (the evaluator's finding I1).
        let (got, expected) = decoded_and_expected("scip-java", ls_ts, ls_ts_measured);
        assert_ne!(got[1], expected[1]);
        let (got, expected) = decoded_and_expected("other", lone_cr2, lone_cr2_measured);
        assert_eq!(got[1], expected[2], "\\n only: m0's position is m2's");
    }

    /// The decoded document of `tool` at `path` whose source blob is `blob` (`None`: not in the
    /// repository) and whose `Document.text` is `text`, with one occurrence at `[0, 0, 1]`.
    fn decoded_document(tool: &str, text: &str, blob: Option<&[u8]>) -> Document {
        let mut o = occ("scip-java maven . . p/T#", ROLE_DEFINITION);
        o.range = vec![0, 0, 1];
        let doc = ScipDocument {
            relative_path: "T".into(),
            text: text.into(),
            occurrences: vec![o],
            ..Default::default()
        };
        let index = Index {
            metadata: Some(Metadata {
                tool_info: Some(ToolInfo {
                    name: tool.into(),
                    ..Default::default()
                })
                .into(),
                ..Default::default()
            })
            .into(),
            // A duplicate document (scip-typescript #330) keeps the rejection.
            documents: vec![doc.clone(), doc],
            ..Default::default()
        };
        let mut got = decode(&encode(&index), &|_| blob.map(<[u8]>::to_vec)).unwrap();
        assert_eq!(got.documents.len(), 1);
        got.documents.remove(0)
    }

    /// Contract M3b amendment 64: the signs, visible in the text alone, that an indexer and
    /// Tree-sitter read a file differently. Measured with `rb-scip-java:0.13.1` and
    /// scip-typescript 0.4.0 (`var/fix-pass7/scratch`): a Latin-1 byte shifts both indexers'
    /// later columns (the evaluator's `Lat.java`, `lat.ts`); javac ends a `//` comment at a
    /// lone CR and at a `\u000a` escape, Tree-sitter-java does not (`CommentCr.java`,
    /// `UEsc.java`), while TypeScript and Tree-sitter-typescript agree on a lone CR (`cmt.ts`).
    #[test]
    fn documents_the_parsers_may_read_differently_are_rejected() {
        let latin1: &[u8] = b"/* \xe9\xe9 */ a(b())\n";
        for tool in ["scip-java", "scip-typescript"] {
            assert_eq!(
                decoded_document(tool, "", Some(latin1)).rejected,
                Some(Rejection::NotUtf8),
                "{tool}"
            );
            let utf8 = "/* \u{e9}\u{1F600} */ a(b())\r\n".as_bytes();
            assert_eq!(
                decoded_document(tool, "", Some(utf8)).rejected,
                None,
                "{tool}"
            );
            // `Document.text` (neither indexer emits it; another tool may) must be the blob.
            assert_eq!(
                decoded_document(tool, "a(b())\n", Some(b"a(c())\n")).rejected,
                Some(Rejection::TextDiffers),
                "{tool}"
            );
            assert_eq!(
                decoded_document(tool, "a(b())\n", Some(b"a(b())\n")).rejected,
                None,
                "{tool}"
            );
        }
        let comment_cr: &[u8] =
            b"class C {\n\tint good() { // note\r return a(); } int evil() {\n\t\treturn b();\n\t}\n}\n";
        let escapes: [&[u8]; 4] = [
            b"// note \\u000a return a(); }",
            b"// note \\u000A x",
            b"// note \\uu000d x",
            b"// note \\u000D x",
        ];
        assert_eq!(
            decoded_document("scip-java", "", Some(comment_cr)).rejected,
            Some(Rejection::CommentTerminator)
        );
        for text in escapes {
            assert_eq!(
                decoded_document("scip-java", "", Some(text)).rejected,
                Some(Rejection::CommentTerminator),
                "{}",
                String::from_utf8_lossy(text)
            );
        }
        assert_eq!(
            decoded_document("scip-java", "", Some(b"// \\u000b \\u00a0 \\n\r\n")).rejected,
            None
        );
        assert_eq!(
            decoded_document("scip-typescript", "", Some(comment_cr)).rejected,
            None,
            "TypeScript ends a line comment at a lone CR as Tree-sitter does"
        );
        // Without the blob nothing is compared: such a document has no Tree-sitter facts.
        assert_eq!(decoded_document("scip-java", "", None).rejected, None);
        assert_eq!(
            Rejection::ALL.map(Rejection::as_str),
            [
                "not_utf8",
                "text_differs",
                "source_encoding",
                "comment_terminator",
                "definition_disagreement"
            ]
        );
    }

    /// Contract M3b amendment 66: a local symbol's name is the text at its definition
    /// occurrence (scip-typescript gives locals no display name); a reference does not name it,
    /// and a multi-line or empty span gives no name.
    #[test]
    fn local_names_are_read_at_their_definition_occurrences() {
        let text = "function f() {\n  const loc = 1;\n  return loc;\n}\n";
        let at = |symbol: &str, roles: i32, range: [i32; 3]| {
            let mut o = occ(symbol, roles);
            o.range = range.to_vec();
            o
        };
        let doc = ScipDocument {
            relative_path: "a.ts".into(),
            occurrences: vec![
                at("local 0", ROLE_DEFINITION, [1, 8, 11]),
                at("local 0", 0, [2, 9, 12]),
                at("local 1", 0, [2, 9, 12]),
                at("local 2", ROLE_DEFINITION, [1, 8, 8]),
            ],
            ..Default::default()
        };
        let index = Index {
            documents: vec![doc],
            ..Default::default()
        };
        let got = decode(&encode(&index), &|_| Some(text.as_bytes().to_vec())).unwrap();
        assert_eq!(
            got.documents[0].local_names,
            BTreeMap::from([("local 0".to_owned(), "loc".to_owned())])
        );
    }

    /// Contract M3b amendment 65: TypeScript's `sys.readFile` drops a leading byte-order mark,
    /// so scip-typescript's line-0 columns start after it (measured on `bom.ts`: `aa` at column
    /// 16); later lines and other tools are unaffected.
    #[test]
    fn typescript_line_zero_starts_after_a_byte_order_mark() {
        let bom = "\u{FEFF}export function aa() { return bb(); } export function bb() { return 1; }\nexport function dd() { aa(); }\n";
        let measured: &[(&str, usize, i32, i32, i32)] = &[
            ("aa", 0, 0, 16, 18),
            ("bb", 0, 0, 30, 32),
            ("bb", 1, 0, 54, 56),
            ("dd", 0, 1, 16, 18),
            ("aa", 1, 1, 23, 25),
        ];
        let (got, expected) = decoded_and_expected("scip-typescript", bom, measured);
        assert_eq!(got, expected);
        let (got, expected) = decoded_and_expected("scip-java", bom, measured);
        assert_ne!(
            got[0], expected[0],
            "javac rejects a BOM; no other tool skips it"
        );
        assert_eq!(got[3], expected[3]);
    }

    #[test]
    fn javac_columns_inside_a_tab_map_to_the_tab() {
        // `a\tb`: the tab spans javac columns 1..8, `b` starts at 8.
        let line = "a\tb".as_bytes();
        assert_eq!(byte_column(line, 0, Units::Javac), 0);
        assert_eq!(byte_column(line, 1, Units::Javac), 1);
        assert_eq!(
            byte_column(line, 5, Units::Javac),
            1,
            "inside the tab's expansion"
        );
        assert_eq!(byte_column(line, 8, Units::Javac), 2);
        assert_eq!(byte_column(line, 9, Units::Javac), 3);
        assert_eq!(
            byte_column(line, 99, Units::Javac),
            3,
            "past the end clamps"
        );
        // A leading tab is corrected by scip-java (7 subtracted): `\tb` reports `b` at 1.
        assert_eq!(byte_column("\tb".as_bytes(), 1, Units::Javac), 1);
        assert_eq!(byte_column("\t\tb".as_bytes(), 2, Units::Javac), 2);
    }

    #[test]
    fn byte_columns_clamp_inside_and_past_characters() {
        let line = "🚀 Woo".as_bytes();
        assert_eq!(byte_column(line, 0, Units::Utf16), 0);
        assert_eq!(
            byte_column(line, 1, Units::Utf16),
            4,
            "inside the surrogate pair"
        );
        assert_eq!(byte_column(line, 2, Units::Utf16), 4);
        assert_eq!(byte_column(line, 3, Units::Utf16), 5);
        assert_eq!(
            byte_column(line, 99, Units::Utf16),
            i32::try_from(line.len()).unwrap()
        );
        assert_eq!(byte_column(line, 2, Units::Chars), 5);
        assert_eq!(byte_column(line, -1, Units::Utf16), 0);
    }
}
