//! Decoding an `index.scip` (design §5.1). Ranges become 0-based `[line, byte col, line, byte
//! col]` spans like Tree-sitter's; SCIP's columns are code units in the document's position
//! encoding, which scip-java and scip-typescript leave unset while meaning UTF-16.

use std::collections::BTreeMap;

use anyhow::Context;
use protobuf::Message as _;
use rb_model::Span;
use scip::types::{PositionEncoding, occurrence};
use serde::{Deserialize, Serialize};

/// `SymbolRole` bits (scip.proto).
pub const ROLE_DEFINITION: i32 = 1;
pub const ROLE_IMPORT: i32 = 2;
pub const ROLE_WRITE: i32 = 4;
pub const ROLE_READ: i32 = 8;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ScipIndex {
    /// `Metadata.project_root` as written (a `file://` URL for both indexers).
    pub project_root: String,
    pub tool: String,
    /// One entry per distinct relative path, sorted.
    pub documents: Vec<Document>,
    /// Symbols defined outside the index (never definitions here).
    pub external_symbols: Vec<SymbolInfo>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Document {
    /// Normalised repo-relative path (`./` and duplicate separators removed).
    pub path: String,
    pub language: String,
    pub occurrences: Vec<Occurrence>,
    pub symbols: Vec<SymbolInfo>,
    /// Columns were converted from UTF-16 code units because the index said nothing else. Wrong
    /// only if the indexer meant something else, which the two supported ones do not.
    pub encoding_assumed: bool,
    /// Occurrences whose columns could not be converted (no source text): kept as code units.
    pub unconverted: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    pub fn is_import(&self) -> bool {
        self.roles & ROLE_IMPORT != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SymbolInfo {
    pub symbol: String,
    /// `SymbolInformation.Kind` by name (`Class`, `Method`, …); empty when unspecified.
    pub kind: String,
    pub display_name: String,
    pub relationships: Vec<Relationship>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Relationship {
    pub symbol: String,
    pub is_implementation: bool,
    pub is_reference: bool,
    pub is_type_definition: bool,
    pub is_definition: bool,
}

/// Decodes `bytes`. `source_of(path)` supplies a document's text (for converting columns to
/// bytes) when the index does not carry it.
pub fn decode(
    bytes: &[u8],
    source_of: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> anyhow::Result<ScipIndex> {
    let index = scip::types::Index::parse_from_bytes(bytes).context("parsing index.scip")?;
    let metadata = index.metadata.as_ref();
    let mut documents: BTreeMap<String, Document> = BTreeMap::new();
    for doc in &index.documents {
        let path = normalise(&doc.relative_path);
        let encoding = doc.position_encoding.enum_value_or_default();
        let (units_per, assumed) = match encoding {
            PositionEncoding::UTF8CodeUnitOffsetFromLineStart => (Units::Bytes, false),
            PositionEncoding::UTF16CodeUnitOffsetFromLineStart => (Units::Utf16, false),
            PositionEncoding::UTF32CodeUnitOffsetFromLineStart => (Units::Chars, false),
            PositionEncoding::UnspecifiedPositionEncoding => (Units::Utf16, true),
        };
        let text: Option<Vec<u8>> = if doc.text.is_empty() {
            source_of(&path)
        } else {
            Some(doc.text.clone().into_bytes())
        };
        let lines: Option<Vec<&[u8]>> = text.as_deref().map(|t| t.split(|&b| b == b'\n').collect());
        let mut unconverted = 0;
        // `None` when the line's text is unknown: the column then stays in code units.
        let convert = |line: i32, col: i32| -> Option<i32> {
            if units_per == Units::Bytes {
                return Some(col);
            }
            let line = lines
                .as_ref()
                .and_then(|ls| usize::try_from(line).ok().and_then(|l| ls.get(l)))?;
            Some(byte_column(line, col, units_per))
        };
        let mut occurrences = Vec::with_capacity(doc.occurrences.len());
        for occ in &doc.occurrences {
            let Some((sl, sc, el, ec)) = range_of(&occ.typed_range, &occ.range) else {
                continue;
            };
            let mut failed = false;
            let mut col = |line: i32, col: i32| {
                convert(line, col).unwrap_or_else(|| {
                    failed = true;
                    col
                })
            };
            let span = [sl, col(sl, sc), el, col(el, ec)];
            let enclosing = enclosing_of(&occ.typed_enclosing_range, &occ.enclosing_range)
                .map(|(sl, sc, el, ec)| [sl, col(sl, sc), el, col(el, ec)]);
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
        let entry = documents.entry(path.clone()).or_insert_with(|| Document {
            path,
            language: doc.language.clone(),
            encoding_assumed: assumed,
            ..Document::default()
        });
        // Duplicate documents for one path (scip-typescript #330) merge, without repeats.
        for o in occurrences {
            if !entry.occurrences.contains(&o) {
                entry.occurrences.push(o);
            }
        }
        for s in doc.symbols.iter().map(symbol_info) {
            if !entry.symbols.contains(&s) {
                entry.symbols.push(s);
            }
        }
        entry.unconverted += unconverted;
    }
    for d in documents.values_mut() {
        d.occurrences
            .sort_by(|a, b| (a.span, &a.symbol, a.roles).cmp(&(b.span, &b.symbol, b.roles)));
        d.symbols.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    }
    Ok(ScipIndex {
        project_root: metadata.map(|m| m.project_root.clone()).unwrap_or_default(),
        tool: metadata
            .and_then(|m| m.tool_info.as_ref())
            .map(|t| format!("{} {}", t.name, t.version))
            .unwrap_or_default(),
        documents: documents.into_values().collect(),
        external_symbols: index.external_symbols.iter().map(symbol_info).collect(),
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Units {
    Bytes,
    Utf16,
    Chars,
}

/// The byte offset of code unit `col` within `line`. A column past the end clamps to the line's
/// length; a column inside a multi-unit character clamps to the next character's start.
fn byte_column(line: &[u8], col: i32, units: Units) -> i32 {
    let Ok(target) = usize::try_from(col) else {
        return 0;
    };
    let text = String::from_utf8_lossy(line);
    let mut seen = 0usize;
    for (byte, ch) in text.char_indices() {
        if seen >= target {
            return i32::try_from(byte).unwrap_or(i32::MAX);
        }
        seen += match units {
            Units::Utf16 => ch.len_utf16(),
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
        kind: if kind == "UnspecifiedKind" {
            String::new()
        } else {
            kind
        },
        display_name: s.display_name.clone(),
        relationships: s
            .relationships
            .iter()
            .map(|r| Relationship {
                symbol: r.symbol.clone(),
                is_implementation: r.is_implementation,
                is_reference: r.is_reference,
                is_type_definition: r.is_type_definition,
                is_definition: r.is_definition,
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
        let mut raw3 = occ("local 1", ROLE_READ);
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
        assert!(d.encoding_assumed);
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
            external_symbols: vec![SymbolInformation {
                symbol: "scip-java maven jdk 17 java/lang/Object#".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let got = decode(&encode(&index), &|_| None).unwrap();
        let s = &got.documents[0].symbols[0];
        assert_eq!((s.kind.as_str(), s.display_name.as_str()), ("Class", "Dog"));
        assert_eq!(s.relationships[0].symbol, "scip-java maven . . p/Animal#");
        assert!(s.relationships[0].is_implementation);
        assert_eq!(got.external_symbols[0].kind, "");
        assert_eq!(
            got.external_symbols[0].symbol,
            "scip-java maven jdk 17 java/lang/Object#"
        );
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
