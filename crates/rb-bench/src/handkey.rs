//! Hand keys (contract M8b, T3, T4; `bench/keys/FORMAT.md`): endpoints, handlers and table access
//! stated from the source by a labeller who never saw rb's output, against rb's dump of that
//! commit. A key gives places as file:line only, so rb's locations are compared without their
//! names (`Loc::unnamed`).

use std::collections::BTreeMap;
use std::path::Path;

use rb_model::is_test_path;

use crate::dump::Dump;
use crate::identity::{Class, EdgeId, Found, Loc, Locator, Side};
use crate::score::{Cell, Scope, score};
use crate::structural::{
    Services, endpoints, handler_bindings, normalize_path, table_maps, table_queries,
};

/// Test code is outside every key (FORMAT.md).
pub const TEST: &str = "test";

/// The bench name of a hand key scored before adjudication (T4: reported beside the final one).
pub const RAW_BENCH: &str = "handkey_raw";

pub const CLASSES: [Class; 5] = [
    Class::Endpoints,
    Class::HandlerBinding,
    Class::TableQueries,
    Class::TableOps,
    Class::TableMaps,
];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fact {
    Endpoint { method: String, path: String },
    Queries { table: String, ops: Vec<String> },
    Maps { table: String },
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct KeyLine {
    pub fact: Fact,
    pub file: String,
    pub line: u32,
}

impl KeyLine {
    fn place(&self) -> Loc {
        Loc {
            path: self.file.clone(),
            line: self.line,
            name: String::new(),
        }
    }
}

/// One key line (`ENDPOINT`, `QUERIES` or `MAPS`).
pub fn parse_line(text: &str) -> Result<KeyLine, String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let place = |w: &str| -> Result<(String, u32), String> {
        let (file, line) = w
            .rsplit_once(':')
            .ok_or_else(|| format!("{w:?} is not <file>:<line>"))?;
        let line = line
            .parse()
            .map_err(|_| format!("{line:?} is not a line number"))?;
        Ok((file.to_owned(), line))
    };
    let (fact, at) = match words.as_slice() {
        ["ENDPOINT", method, path, at] => (
            Fact::Endpoint {
                method: method.to_uppercase(),
                path: normalize_path(path),
            },
            at,
        ),
        ["QUERIES", table, ops, at] => (
            Fact::Queries {
                table: table.to_lowercase(),
                ops: ops.split(',').map(str::to_owned).collect(),
            },
            at,
        ),
        ["MAPS", table, at] => (
            Fact::Maps {
                table: table.to_lowercase(),
            },
            at,
        ),
        _ => return Err(format!("not a key line: {text:?}")),
    };
    let (file, line) = place(at)?;
    Ok(KeyLine { fact, file, line })
}

fn lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(i, l)| (i + 1, l.trim()))
        .filter(|(_, l)| !l.is_empty() && !l.starts_with('#'))
}

/// A whole `key.txt`.
pub fn parse_key(text: &str) -> Result<Vec<KeyLine>, String> {
    lines(text)
        .map(|(n, l)| parse_line(l).map_err(|e| format!("key line {n}: {e}")))
        .collect()
}

/// The effective key: `key.txt` with `adjudication.md`'s `- ADD `…`` and `- REMOVE `…`` entries
/// applied in order. A REMOVE of a line the key does not hold is an error.
pub fn adjudicate(key: Vec<KeyLine>, adjudication: &str) -> Result<Vec<KeyLine>, String> {
    let mut key = key;
    for (n, l) in adjudication.lines().enumerate() {
        let Some(rest) = l.strip_prefix("- ") else {
            continue;
        };
        let (op, rest) = match rest.split_once(' ') {
            Some((op @ ("ADD" | "REMOVE"), rest)) => (op, rest),
            _ => continue,
        };
        let entry = rest
            .strip_prefix('`')
            .and_then(|r| r.split_once('`'))
            .map(|(line, _)| line)
            .ok_or_else(|| format!("adjudication line {}: no `key line`", n + 1))?;
        let line = parse_line(entry).map_err(|e| format!("adjudication line {}: {e}", n + 1))?;
        if op == "ADD" {
            key.push(line);
        } else {
            let at = key.iter().position(|k| *k == line).ok_or_else(|| {
                format!(
                    "adjudication line {}: REMOVE of a line the key lacks",
                    n + 1
                )
            })?;
            key.remove(at);
        }
    }
    Ok(key)
}

/// Every key line names a file of the checkout and a line it has (0 for file level).
pub fn check_places(key: &[KeyLine], checkout: &Path) -> Result<(), String> {
    for k in key {
        let text = std::fs::read_to_string(checkout.join(&k.file))
            .map_err(|_| format!("{}:{}: no such file at the pin", k.file, k.line))?;
        if k.line as usize > text.lines().count() {
            return Err(format!("{}:{}: past the end of the file", k.file, k.line));
        }
    }
    Ok(())
}

/// The key's facts, by class.
fn gold(key: &[KeyLine], services: &Services) -> BTreeMap<Class, Side<()>> {
    let mut gold: BTreeMap<Class, Side<()>> =
        CLASSES.iter().map(|c| (*c, Side::default())).collect();
    let mut add = |class: Class, src: Loc, dst: Loc| {
        let side = gold.get_mut(&class).expect("every class has a side");
        side.located.insert(EdgeId { src, class, dst }, ());
    };
    for k in key {
        match &k.fact {
            Fact::Endpoint { method, path } => {
                let svc = services.of_file(&k.file).unwrap_or(".");
                let ep = Loc::endpoint(svc, method, path);
                add(Class::Endpoints, Loc::service(svc), ep.clone());
                add(Class::HandlerBinding, ep, k.place());
            }
            Fact::Queries { table, ops } => {
                add(Class::TableQueries, k.place(), Loc::table(table));
                for op in ops {
                    add(
                        Class::TableOps,
                        k.place(),
                        Loc::table(&format!("{table} {op}")),
                    );
                }
            }
            Fact::Maps { table } => add(Class::TableMaps, k.place(), Loc::table(table)),
        }
    }
    gold
}

/// rb's facts in the key's terms: places without names.
fn found(dump: &Dump) -> BTreeMap<Class, Side<Found>> {
    let locator = Locator::new(dump);
    let (queries, ops) = table_queries(dump, &locator);
    let unnamed = |side: Side<Found>, src: bool, dst: bool| {
        let mut out = Side {
            located: BTreeMap::new(),
            unmatchable: side.unmatchable,
        };
        for (id, f) in side.located {
            let id = EdgeId {
                src: if src { id.src.unnamed() } else { id.src },
                class: id.class,
                dst: if dst { id.dst.unnamed() } else { id.dst },
            };
            out.insert(id, f);
        }
        out
    };
    BTreeMap::from([
        (Class::Endpoints, endpoints(dump)),
        (
            Class::HandlerBinding,
            unnamed(handler_bindings(dump, &locator), false, true),
        ),
        (Class::TableQueries, unnamed(queries, true, false)),
        (Class::TableOps, unnamed(ops, true, false)),
        (
            Class::TableMaps,
            unnamed(table_maps(dump, &locator), true, false),
        ),
    ])
}

/// The cells of one hand key against one dump.
pub fn score_key(key: &[KeyLine], dump: &Dump) -> BTreeMap<Class, Cell> {
    let services = Services::new(dump);
    let gold = gold(key, &services);
    let found = found(dump);
    let scope = |id: &EdgeId| {
        // A place in test code: the handler (a binding's target) or the querying function.
        let place = match id.class {
            Class::HandlerBinding => Some(&id.dst),
            Class::TableQueries | Class::TableOps | Class::TableMaps => Some(&id.src),
            _ => None,
        };
        match place {
            Some(l) if is_test_path(&l.path) => Scope::Out(TEST),
            _ => Scope::In,
        }
    };
    CLASSES
        .iter()
        .map(|c| (*c, score(&gold[c], &found[c], scope)))
        .collect()
}

/// One fact as a statement in key-line form.
fn statement_text(id: &EdgeId) -> String {
    let place = |l: &Loc| format!("{}:{}", l.path, l.line);
    let endpoint = |ep: &Loc| {
        ep.name
            .split_once(' ')
            .map_or(String::new(), |(_, r)| r.to_owned())
    };
    match id.class {
        // Existence only, written alike for both sides; the handler is a statement of its own.
        Class::Endpoints => format!("ENDPOINT {} *", endpoint(&id.dst)),
        Class::HandlerBinding => format!("ENDPOINT {} {}", endpoint(&id.src), place(&id.dst)),
        Class::TableQueries => format!("QUERIES {} * {}", id.dst.name, place(&id.src)),
        Class::TableOps => {
            let (table, op) = id.dst.name.rsplit_once(' ').unwrap_or_default();
            format!("QUERIES {table} {op} {}", place(&id.src))
        }
        _ => format!("MAPS {} {}", id.dst.name, place(&id.src)),
    }
}

/// Whether an agreed fact is among the one in five shown to the adjudicator.
fn sampled(text: &str) -> bool {
    use sha2::{Digest, Sha256};
    Sha256::digest(text.as_bytes())[0] % 5 == 0
}

/// The statements an adjudicator judges (T4): every fact on which the key and rb disagree, and
/// about one in five on which they agree, each written as a key line (existence as `*`,
/// `*` where a side states no ops), deduplicated and ordered by a hash. Nothing says which
/// side made a statement, and agreements read exactly like disagreements.
pub fn statements(key: &[KeyLine], dump: &Dump) -> Vec<String> {
    use sha2::{Digest, Sha256};
    let services = Services::new(dump);
    let gold = gold(key, &services);
    let found = found(dump);
    let text = statement_text;
    let hash = |t: &str| Sha256::digest(t.as_bytes());
    let mut out: BTreeMap<Vec<u8>, String> = BTreeMap::new();
    for c in CLASSES {
        let (g, f) = (&gold[&c].located, &found[&c].located);
        for id in g.keys().chain(f.keys()) {
            let t = text(id);
            let h = hash(&t);
            let disputed = g.contains_key(id) != f.contains_key(id);
            if disputed || sampled(&t) {
                out.insert(h.to_vec(), t);
            }
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    out.into_values()
        .filter(|t| seen.insert(t.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dump::parse;

    const KEY: &str = "\
# a comment
ENDPOINT GET /api/v1/login services/auth/src/main/java/com/acme/auth/AuthController.java:24
ENDPOINT POST /internal/mfa/email services/notification/src/server.ts:7
ENDPOINT GET /nowhere services/auth/src/main/java/com/acme/auth/AuthController.java:99
QUERIES users read services/auth/src/main/java/com/acme/auth/UserDao.java:14
QUERIES audit.users insert services/auth/src/main/java/com/acme/auth/UserDao.java:18
QUERIES users update,delete services/auth/src/main/java/com/acme/auth/UserDao.java:18
";

    fn polyglot() -> Dump {
        parse(include_str!(
            "../../rb-cli/tests/golden/polyglot-monorepo/dump.jsonl"
        ))
        .unwrap()
    }

    #[test]
    fn key_lines_parse_and_bad_ones_name_their_line() {
        let k = parse_key(KEY).unwrap();
        assert_eq!(k.len(), 6);
        assert_eq!(
            k[5].fact,
            Fact::Queries {
                table: "users".into(),
                ops: vec!["update".into(), "delete".into()]
            }
        );
        let lower = parse_line("ENDPOINT get /x a.java:1").unwrap();
        assert_eq!(
            lower.fact,
            Fact::Endpoint {
                method: "GET".into(),
                path: "/x".into()
            }
        );
        let err = parse_key("ENDPOINT GET /x\nMAPS t a.java:x\n").unwrap_err();
        assert!(err.starts_with("key line 1:"), "{err}");
    }

    #[test]
    fn adjudication_adds_and_removes_only_by_entry() {
        let k = parse_key(KEY).unwrap();
        let adj = "# notes\n- REMOVE `ENDPOINT GET /nowhere services/auth/src/main/java/com/acme/auth/AuthController.java:99` — no such mapping\n- ADD `MAPS users a/User.java:5` — @Entity\nprose line\n";
        let k2 = adjudicate(k.clone(), adj).unwrap();
        assert_eq!(k2.len(), 6);
        assert!(k2.iter().any(|l| l.fact
            == Fact::Maps {
                table: "users".into()
            }));
        assert!(!k2.iter().any(|l| l.line == 99));
        let bad = "- REMOVE `MAPS nope a.java:1` — x\n";
        assert!(
            adjudicate(k, bad)
                .unwrap_err()
                .contains("REMOVE of a line the key lacks")
        );
    }

    #[test]
    fn a_key_scores_against_a_real_dump() {
        let cells = score_key(&parse_key(KEY).unwrap(), &polyglot());
        let eps = &cells[&Class::Endpoints];
        assert_eq!(
            eps.tp_total(),
            2,
            "GET /api/v1/login and POST /internal/mfa/email"
        );
        assert_eq!(eps.fn_, 1, "GET /nowhere");
        let hb = &cells[&Class::HandlerBinding];
        assert_eq!(
            hb.tp_total(),
            2,
            "a named and an anonymous handler, by file:line"
        );
        let q = &cells[&Class::TableQueries];
        assert_eq!(
            q.tp_total(),
            3,
            "emailOf→users, touch→audit.users, touch→users"
        );
        let ops = &cells[&Class::TableOps];
        assert_eq!(ops.tp_total(), 3, "read, insert, update");
        assert_eq!(ops.fn_, 1, "the key's delete, which rb lacks");
    }

    #[test]
    fn statements_cover_every_dispute_and_name_no_side() {
        let st = statements(&parse_key(KEY).unwrap(), &polyglot());
        // No statement can tell a side: existence is `*` for both, never a missing handler.
        assert!(
            st.iter().all(|s| !s.split_whitespace().any(|w| w == "?")),
            "{st:?}"
        );
        assert!(st.iter().any(|s| s == "ENDPOINT GET /nowhere *"), "{st:?}");
        // The key's GET /nowhere is disputed; so is the key's `delete` op rb lacks.
        assert!(
            st.iter().any(|s| s.starts_with("ENDPOINT GET /nowhere ")),
            "{st:?}"
        );
        assert!(st.iter().any(|s| s == "QUERIES users delete services/auth/src/main/java/com/acme/auth/UserDao.java:18"), "{st:?}");
        // rb's endpoints the key lacks are statements too, with rb's handler place.
        assert!(
            st.iter()
                .any(|s| s.starts_with("ENDPOINT POST /api/v1/login ")),
            "{st:?}"
        );
        assert!(
            st.iter().all(|s| !s.contains("rb") && !s.contains("key")),
            "{st:?}"
        );
        let again = statements(&parse_key(KEY).unwrap(), &polyglot());
        assert_eq!(st, again, "deterministic");
    }

    #[test]
    fn test_code_is_outside_every_key() {
        let rows = [
            r#"["rb-dump",2,"x"]"#,
            r#"["N","symbol","rb-heuristic . . . `src/test/java/T.java`/T#q().","q",{}]"#,
            r#"["E","symbol","rb-heuristic . . . `src/test/java/T.java`/T#q().","DEFINED_IN","file","src/test/java/T.java","treesitter",5,{},[["src/test/java/T.java",[4,0,4,1]]]]"#,
            r#"["E","symbol","rb-heuristic . . . `src/test/java/T.java`/T#q().","QUERIES","table","public.users","sql_literal",5,{"ops":["read"],"schema_assumed":true},[["src/test/java/T.java",[5,0,5,1]]]]"#,
        ];
        let d = parse(&rows.join("\n")).unwrap();
        let cells = score_key(&[], &d);
        let q = &cells[&Class::TableQueries];
        assert_eq!(q.found_out_of_scope.get(TEST), Some(&1));
        assert_eq!(q.fp_total(), 0);
    }

    #[test]
    fn statements_include_the_sampled_agreements() {
        let key = parse_key(KEY).unwrap();
        let d = polyglot();
        let st = statements(&key, &d);
        let (gold, found) = (gold(&key, &Services::new(&d)), found(&d));
        let agreed: Vec<String> = CLASSES
            .iter()
            .flat_map(|c| {
                gold[c]
                    .located
                    .keys()
                    .filter(|id| found[c].located.contains_key(*id))
            })
            .map(statement_text)
            .collect();
        assert!(agreed.len() >= 5, "{agreed:?}");
        let shown: Vec<&String> = agreed.iter().filter(|t| sampled(t)).collect();
        assert!(
            !shown.is_empty(),
            "the fixture has a sampled agreement: {agreed:?}"
        );
        assert!(
            shown.iter().all(|t| st.contains(t)),
            "every sampled agreement is shown: {shown:?} {st:?}"
        );
    }

    #[test]
    fn places_are_checked_against_the_checkout() {
        let tmp = std::env::temp_dir().join(format!("rb-bench-hk-{}", std::process::id()));
        std::fs::create_dir_all(tmp.join("a")).unwrap();
        std::fs::write(tmp.join("a/X.java"), "class X {\n}\n").unwrap();
        let ok = parse_key("MAPS x a/X.java:1\nQUERIES x read a/X.java:0\n").unwrap();
        assert_eq!(check_places(&ok, &tmp), Ok(()));
        let past = parse_key("MAPS x a/X.java:3\n").unwrap();
        assert!(
            check_places(&past, &tmp)
                .unwrap_err()
                .contains("past the end")
        );
        let missing = parse_key("MAPS x a/Y.java:1\n").unwrap();
        assert!(
            check_places(&missing, &tmp)
                .unwrap_err()
                .contains("no such file")
        );
        std::fs::remove_dir_all(tmp).unwrap();
    }
}
