//! Decoding an `index.scip` (design §5.1). Ranges become 0-based `[line, byte col, line, byte
//! col]` spans like Tree-sitter's; SCIP's columns are code units in the document's position
//! encoding, which scip-java and scip-typescript leave unset while meaning UTF-16.

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
    let scip_java = metadata
        .and_then(|m| m.tool_info.as_ref())
        .is_some_and(|t| t.name == "scip-java");
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
