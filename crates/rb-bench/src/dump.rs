//! Reads the canonical dump (`rb dump`, `rb_model::canonical_dump`) back into rb-model's row types.
//! The rendering side is `NodeRow::line` / `EdgeRow::line`; a round-trip test binds the two, so a
//! format change cannot drift past the benchmark.

use std::fmt;
use std::str::FromStr;

use rb_model::diff::{EdgeRow, NodeRow};
use rb_model::{Attrs, Certainty, EdgeKind, Evidence, NodeKind, Provenance, Span};
use serde_json::Value;

/// The only dump version this reader understands (`["rb-dump", 2, sha]`).
pub const DUMP_VERSION: i64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DumpError {
    /// The first line is not an `rb-dump` header.
    Header { found: String },
    /// A header of another version.
    Version { found: i64 },
    /// A row that does not parse; `line` is 1-based.
    Row { line: usize, message: String },
}

impl fmt::Display for DumpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DumpError::Header { found } => write!(f, "not an rb dump: first line is {found:?}"),
            DumpError::Version { found } => write!(
                f,
                "rb dump version {found} is not supported (expected {DUMP_VERSION})"
            ),
            DumpError::Row { line, message } => write!(f, "dump line {line}: {message}"),
        }
    }
}

impl std::error::Error for DumpError {}

/// A parsed dump. Diagnostics (`D` rows) are checked for length (5 fields) and not kept.
#[derive(Debug, Clone, PartialEq)]
pub struct Dump {
    pub commit: String,
    pub nodes: Vec<NodeRow>,
    pub edges: Vec<EdgeRow>,
}

pub fn parse(text: &str) -> Result<Dump, DumpError> {
    let mut lines = text.lines();
    let commit = header(lines.next().unwrap_or(""))?;
    let mut dump = Dump {
        commit,
        nodes: Vec::new(),
        edges: Vec::new(),
    };
    for (i, line) in lines.enumerate() {
        let row_error = |message: String| DumpError::Row {
            line: i + 2,
            message,
        };
        let row: Vec<Value> = serde_json::from_str(line).map_err(|e| row_error(e.to_string()))?;
        match row.first().and_then(Value::as_str) {
            Some("N") => dump.nodes.push(node(&row).map_err(row_error)?),
            Some("E") => dump.edges.push(edge(&row).map_err(row_error)?),
            Some("D") if row.len() == 5 => {}
            _ => return Err(row_error("expected an N, E or D row".to_owned())),
        }
    }
    Ok(dump)
}

fn header(line: &str) -> Result<String, DumpError> {
    let bad = || DumpError::Header {
        found: line.to_owned(),
    };
    let row: Vec<Value> = serde_json::from_str(line).map_err(|_| bad())?;
    match row.as_slice() {
        [Value::String(tag), version, Value::String(commit)] if tag == "rb-dump" => {
            match version.as_i64() {
                Some(DUMP_VERSION) => Ok(commit.clone()),
                Some(found) => Err(DumpError::Version { found }),
                None => Err(bad()),
            }
        }
        _ => Err(bad()),
    }
}

fn node(row: &[Value]) -> Result<NodeRow, String> {
    let [_, kind, key, name, attrs] = row else {
        return Err(format!("an N row has 5 fields, found {}", row.len()));
    };
    Ok(NodeRow {
        kind: parsed(kind)?,
        key: text(key)?,
        name: text(name)?,
        attrs: object(attrs)?,
    })
}

fn edge(row: &[Value]) -> Result<EdgeRow, String> {
    let [
        _,
        src_kind,
        src_key,
        kind,
        dst_kind,
        dst_key,
        provenance,
        certainty,
        attrs,
        evidence,
    ] = row
    else {
        return Err(format!("an E row has 10 fields, found {}", row.len()));
    };
    let rank = certainty
        .as_i64()
        .and_then(|r| i16::try_from(r).ok())
        .ok_or_else(|| format!("certainty is not a rank: {certainty}"))?;
    Ok(EdgeRow {
        src_kind: parsed::<NodeKind>(src_kind)?,
        src_key: text(src_key)?,
        kind: parsed::<EdgeKind>(kind)?,
        dst_kind: parsed::<NodeKind>(dst_kind)?,
        dst_key: text(dst_key)?,
        provenance: parsed::<Provenance>(provenance)?,
        certainty: Certainty::from_rank(rank).map_err(|e| e.to_string())?,
        attrs: object(attrs)?,
        evidence: evidence
            .as_array()
            .ok_or("evidence is not a list")?
            .iter()
            .map(evidence_item)
            .collect::<Result<_, _>>()?,
    })
}

fn evidence_item(v: &Value) -> Result<Evidence, String> {
    let bad = || format!("evidence is not [path, [l0, c0, l1, c1]]: {v}");
    let Some([path, span]) = v.as_array().map(Vec::as_slice) else {
        return Err(bad());
    };
    let span: Vec<i32> = span
        .as_array()
        .ok_or_else(bad)?
        .iter()
        .map(|n| n.as_i64().and_then(|n| i32::try_from(n).ok()))
        .collect::<Option<_>>()
        .ok_or_else(bad)?;
    Ok(Evidence {
        path: text(path)?,
        span: Span::try_from(span).map_err(|_| bad())?,
    })
}

fn text(v: &Value) -> Result<String, String> {
    v.as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("expected a string, found {v}"))
}

fn parsed<T: FromStr<Err = anyhow::Error>>(v: &Value) -> Result<T, String> {
    text(v)?.parse().map_err(|e: anyhow::Error| e.to_string())
}

fn object(v: &Value) -> Result<Attrs, String> {
    v.as_object()
        .cloned()
        .ok_or_else(|| format!("expected an object, found {v}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HEADER: &str = r#"["rb-dump",2,"abc"]"#;

    fn sample_edge() -> EdgeRow {
        EdgeRow {
            src_kind: NodeKind::Symbol,
            src_key: "rb-heuristic . . . `a.py`/f().".to_owned(),
            kind: EdgeKind::Calls,
            dst_kind: NodeKind::Symbol,
            dst_key: "rb-heuristic . . . `a.py`/g().".to_owned(),
            provenance: Provenance::Heuristic,
            certainty: Certainty::Ambiguous,
            attrs: json!({"candidates": 2}).as_object().cloned().unwrap(),
            evidence: vec![Evidence {
                path: "a.py".to_owned(),
                span: [3, 4, 3, 5],
            }],
        }
    }

    fn sample_node() -> NodeRow {
        NodeRow {
            kind: NodeKind::Symbol,
            key: "rb-heuristic . . . `a.py`/f().".to_owned(),
            name: "f".to_owned(),
            attrs: json!({"kind": "function"}).as_object().cloned().unwrap(),
        }
    }

    #[test]
    fn rows_round_trip_through_rb_model_rendering() {
        let (n, e) = (sample_node(), sample_edge());
        let text = format!(
            "{HEADER}\n{}\n{}\n[\"D\",\"x\",null,null,{{}}]\n",
            n.line(),
            e.line()
        );
        let d = parse(&text).unwrap();
        assert_eq!(d.commit, "abc");
        assert_eq!(d.nodes, vec![n]);
        assert_eq!(d.edges, vec![e]);
    }

    #[test]
    fn another_version_is_a_typed_error() {
        assert_eq!(
            parse(r#"["rb-dump",3,"abc"]"#),
            Err(DumpError::Version { found: 3 })
        );
        assert_eq!(
            parse(r#"["rb-dump",1,"abc"]"#),
            Err(DumpError::Version { found: 1 })
        );
    }

    #[test]
    fn a_missing_or_foreign_header_is_rejected() {
        assert!(matches!(parse(""), Err(DumpError::Header { .. })));
        assert!(matches!(
            parse(r#"["rb-diff",2,"abc"]"#),
            Err(DumpError::Header { .. })
        ));
        assert!(matches!(
            parse(r#"["rb-dump","2","abc"]"#),
            Err(DumpError::Header { .. })
        ));
    }

    #[test]
    fn bad_rows_name_their_line() {
        let bad_rank = sample_edge().line().replace(",1,{", ",9,{");
        for (row, needle) in [
            (r#"["X",1]"#.to_owned(), "expected an N, E or D row"),
            (r#"["D","x"]"#.to_owned(), "expected an N, E or D row"),
            (r#"["N","symbol","k"]"#.to_owned(), "5 fields"),
            (r#"["N","nope","k","n",{}]"#.to_owned(), "nope"),
            (bad_rank, "certainty"),
            ("not json".to_owned(), "expected"),
        ] {
            let err = parse(&format!("{HEADER}\n{}\n{row}\n", sample_node().line())).unwrap_err();
            let DumpError::Row { line, message } = err else {
                panic!("{row}: {err:?}")
            };
            assert_eq!(line, 3, "{row}");
            assert!(message.contains(needle), "{row}: {message}");
        }
    }

    #[test]
    fn every_golden_dump_round_trips() {
        let golden =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../rb-cli/tests/golden");
        let mut dumps = 0;
        for fixture in std::fs::read_dir(&golden).unwrap() {
            for name in ["dump.jsonl", "scip-dump.jsonl"] {
                let path = fixture.as_ref().unwrap().path().join(name);
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let d = parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                let rendered: Vec<String> = d
                    .nodes
                    .iter()
                    .map(NodeRow::line)
                    .chain(d.edges.iter().map(EdgeRow::line))
                    .collect();
                let original: Vec<&str> = text
                    .lines()
                    .filter(|l| l.starts_with("[\"N\"") || l.starts_with("[\"E\""))
                    .collect();
                assert_eq!(rendered, original, "{}", path.display());
                dumps += 1;
            }
        }
        assert!(dumps >= 17, "only {dumps} golden dumps found");
    }
}
