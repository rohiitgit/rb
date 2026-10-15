//! PyCG micro-benchmark (contract M8a, S5; amendment 1): each case's `callgraph.json` against
//! rb's `--no-scip` dump of that case, in PyCG's own names. PyCG names a function by its module
//! and enclosing classes and functions (`main.A.func`), and a call made at module level by the
//! module (`main`).
//!
//! Mapping an rb key to PyCG: `` rb-heuristic . . . `a/b.py`/C#f(). `` is `a.b.C.f`; a package's
//! `__init__.py` is the package. rb gives a module-level assignment (`a = A()`) its own symbol
//! (`a.`) and sources its calls there; PyCG sources them at the module. So a caller is the longest
//! chain of its key that ends in a callable, or the module (the file scope) when there is none.
//! Overloads (`f(+1)`, a redefinition) share one name and so one location.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rb_model::NodeKind;

use crate::dump::Dump;
use crate::identity::{Class, EdgeId, Found, Loc, Locator, Side, found};
use crate::score::{Cell, Scope, score};

pub mod reason {
    /// A callee in Python's builtins (`<builtin>.print`).
    pub const BUILTIN: &str = "builtin";
    /// A callee outside the case's modules.
    pub const EXTERNAL: &str = "external";
    /// A callee that is a lambda (`main.<lambda1>`): rb defines no symbol for it.
    pub const LAMBDA: &str = "lambda";
    /// A callee that is a module (its import runs it); rb models no call to a module.
    pub const MODULE: &str = "module";
    /// A PyCG function with no rb symbol of that name.
    pub const NO_RB_SYMBOL: &str = "no_rb_symbol";
}

/// The classes this key scores: calls from functions, and calls made at module level.
pub const CLASSES: [Class; 2] = [Class::Calls, Class::CallsFileScope];

/// A Python module's dotted name: `a/b.py` is `a.b`, `pkg/__init__.py` is `pkg`.
pub fn module_name(path: &str) -> Option<String> {
    let stem = path.strip_suffix(".py")?;
    let stem = stem
        .strip_suffix("/__init__")
        .or_else(|| (stem == "__init__").then_some(""))
        .unwrap_or(stem);
    Some(stem.replace('/', "."))
}

/// One step of an rb heuristic key's descriptor chain.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Type(String),
    Callable(String),
    Term(String),
}

/// The file and descriptor chain of an rb heuristic key; `None` for any other key.
fn parse_key(key: &str) -> Option<(&str, Vec<Step>)> {
    let rest = key.strip_prefix("rb-heuristic . . . `")?;
    let (path, mut rest) = rest.split_once("`/")?;
    let mut steps = Vec::new();
    while !rest.is_empty() {
        let end = rest.find(['#', '(', '.'])?;
        let name = rest[..end].to_owned();
        match rest.as_bytes()[end] {
            b'#' => {
                steps.push(Step::Type(name));
                rest = &rest[end + 1..];
            }
            b'(' => {
                let close = rest[end..].find(").")? + end;
                steps.push(Step::Callable(name));
                rest = &rest[close + 2..];
            }
            _ => {
                steps.push(Step::Term(name));
                rest = &rest[end + 1..];
            }
        }
    }
    Some((path, steps))
}

fn join(module: &str, steps: &[Step]) -> String {
    let mut name = module.to_owned();
    for s in steps {
        let (Step::Type(n) | Step::Callable(n) | Step::Term(n)) = s;
        if !name.is_empty() {
            name.push('.');
        }
        name.push_str(n);
    }
    name
}

/// The location standing for a PyCG name rb cannot have: its path names the scope reason.
fn marker(reason: &str, name: &str) -> Loc {
    Loc {
        path: format!("<{reason}>"),
        line: 0,
        name: name.to_owned(),
    }
}

/// The scope reason of a marker location, if it is one.
fn marked(loc: &Loc) -> Option<&'static str> {
    [
        reason::BUILTIN,
        reason::EXTERNAL,
        reason::LAMBDA,
        reason::MODULE,
    ]
    .into_iter()
    .find(|r| loc.line == 0 && loc.path == format!("<{r}>"))
}

/// What a case's rb symbols are called in PyCG.
struct Names {
    /// Each PyCG name of an rb symbol, to its location (the lowest, for overloads).
    loc: BTreeMap<String, Loc>,
    /// Each module of the case, to its file.
    modules: BTreeMap<String, String>,
    /// Each located rb symbol, to its PyCG name as a callee and its identity as a caller.
    by_loc: HashMap<Loc, (String, Caller)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Caller {
    Function(String),
    Module(String),
}

impl Names {
    fn new(dump: &Dump, locator: &Locator) -> Names {
        let mut names = Names {
            loc: BTreeMap::new(),
            modules: BTreeMap::new(),
            by_loc: HashMap::new(),
        };
        for n in &dump.nodes {
            if n.kind == NodeKind::File
                && let Some(m) = module_name(&n.key)
            {
                names.modules.insert(m, n.key.clone());
            }
        }
        for n in dump.nodes.iter().filter(|n| n.kind == NodeKind::Symbol) {
            let (Ok(loc), Some((path, steps))) =
                (locator.loc(NodeKind::Symbol, &n.key), parse_key(&n.key))
            else {
                continue;
            };
            let Some(module) = module_name(path) else {
                continue;
            };
            let dotted = join(&module, &steps);
            let callable = steps.iter().rposition(|s| matches!(s, Step::Callable(_)));
            let caller = match callable {
                Some(i) => Caller::Function(join(&module, &steps[..=i])),
                None => Caller::Module(path.to_owned()),
            };
            names
                .loc
                .entry(dotted.clone())
                .and_modify(|l| *l = l.clone().min(loc.clone()))
                .or_insert_with(|| loc.clone());
            names.by_loc.insert(loc, (dotted, caller));
        }
        names
    }

    /// A gold caller: a module's file scope, or a function's location.
    fn gold_src(&self, caller: &str) -> Result<(Class, Loc), &'static str> {
        if let Some(path) = self.modules.get(caller) {
            return Ok((Class::CallsFileScope, Loc::file(path)));
        }
        if caller.contains("<lambda") {
            return Ok((Class::Calls, marker(reason::LAMBDA, caller)));
        }
        self.loc
            .get(caller)
            .map(|l| (Class::Calls, l.clone()))
            .ok_or(reason::NO_RB_SYMBOL)
    }

    /// A gold callee: its location, a marker location for what is out of scope, or no location.
    fn gold_dst(&self, callee: &str) -> Result<Loc, &'static str> {
        let marker = |reason| marker(reason, callee);
        if callee.starts_with("<builtin>") {
            return Ok(marker(reason::BUILTIN));
        }
        if callee.contains("<lambda") {
            return Ok(marker(reason::LAMBDA));
        }
        if self.modules.contains_key(callee) {
            return Ok(marker(reason::MODULE));
        }
        // A root `__init__.py` is the module "": no callee starts with its "." prefix.
        let in_case = self.modules.keys().any(|m| {
            callee
                .strip_prefix(m.as_str())
                .is_some_and(|r| r.starts_with('.'))
        });
        if !in_case {
            return Ok(marker(reason::EXTERNAL));
        }
        self.loc.get(callee).cloned().ok_or(reason::NO_RB_SYMBOL)
    }

    /// A found edge in PyCG's terms: callers that are module-level terms move to the file scope,
    /// and overloads collapse onto one location.
    fn remap(&self, id: &EdgeId) -> Option<EdgeId> {
        let src = match id.class {
            Class::CallsFileScope => (Class::CallsFileScope, id.src.clone()),
            Class::Calls => match &self.by_loc.get(&id.src)?.1 {
                Caller::Module(path) => (Class::CallsFileScope, Loc::file(path)),
                Caller::Function(name) => (Class::Calls, self.loc.get(name)?.clone()),
            },
            // PyCG scores calls only; `found` yields no other class.
            _ => return None,
        };
        let dst = self.loc.get(&self.by_loc.get(&id.dst)?.0)?.clone();
        Some(EdgeId {
            src: src.1,
            class: src.0,
            dst,
        })
    }
}

/// The cells of one case: `callgraph.json` against the case's dump.
pub fn score_case(callgraph: &str, dump: &Dump) -> Result<BTreeMap<Class, Cell>, String> {
    let graph: BTreeMap<String, Vec<String>> =
        serde_json::from_str(callgraph).map_err(|e| format!("not a PyCG call graph: {e}"))?;
    let locator = Locator::new(dump);
    let names = Names::new(dump, &locator);

    let mut gold: BTreeMap<Class, Side<()>> =
        CLASSES.iter().map(|c| (*c, Side::default())).collect();
    for (caller, callees) in &graph {
        for callee in callees.iter().collect::<BTreeSet<_>>() {
            match (names.gold_src(caller), names.gold_dst(callee)) {
                (Ok((class, src)), Ok(dst)) => {
                    let side = gold.get_mut(&class).expect("every class has a side");
                    side.located.insert(EdgeId { src, class, dst }, ());
                }
                (Err(r), _) | (_, Err(r)) => {
                    // Without a caller location the class is not known; a module caller is.
                    let class = if names.modules.contains_key(caller) {
                        Class::CallsFileScope
                    } else {
                        Class::Calls
                    };
                    let side = gold.get_mut(&class).expect("every class has a side");
                    *side.unmatchable.entry(r).or_default() += 1;
                }
            }
        }
    }

    let mut rb: BTreeMap<Class, Side<Found>> =
        CLASSES.iter().map(|c| (*c, Side::default())).collect();
    for class in CLASSES {
        let side = found(dump, &locator, class);
        for (r, n) in side.unmatchable {
            *rb.get_mut(&class)
                .expect("every class has a side")
                .unmatchable
                .entry(r)
                .or_default() += n;
        }
        for (id, f) in side.located {
            let Some(id) = names.remap(&id) else {
                let side = rb.get_mut(&class).expect("every class has a side");
                *side.unmatchable.entry(reason::NO_RB_SYMBOL).or_default() += 1;
                continue;
            };
            let side = rb.get_mut(&id.class).expect("every class has a side");
            side.insert(id, f);
        }
    }

    // A lambda can be either end; any other marker is a callee.
    let scope = |id: &EdgeId| match marked(&id.src).or_else(|| marked(&id.dst)) {
        Some(r) => Scope::Out(r),
        None => Scope::In,
    };
    Ok(CLASSES
        .iter()
        .map(|c| (*c, score(&gold[c], &rb[c], scope)))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dump::parse;
    use rb_model::Certainty;

    #[test]
    fn module_names() {
        assert_eq!(module_name("main.py").as_deref(), Some("main"));
        assert_eq!(module_name("a/b.py").as_deref(), Some("a.b"));
        assert_eq!(module_name("pkg/__init__.py").as_deref(), Some("pkg"));
        assert_eq!(module_name("__init__.py").as_deref(), Some(""));
        assert_eq!(module_name("a.ts"), None);
    }

    #[test]
    fn keys_parse_into_steps() {
        let k = "rb-heuristic . . . `a/b.py`/C#f(+1).x.";
        let (path, steps) = parse_key(k).unwrap();
        assert_eq!(path, "a/b.py");
        assert_eq!(
            steps,
            vec![
                Step::Type("C".into()),
                Step::Callable("f".into()),
                Step::Term("x".into())
            ]
        );
        assert_eq!(join("a.b", &steps), "a.b.C.f.x");
        assert_eq!(parse_key("scip-python python x . a/f()."), None);
    }

    /// `classes/nested_class_calls` at the PyCG pin, as rb (the M7 binary) dumps it.
    const NESTED_CLASS_CALLS: &str = r#"{"main.C.func":[],"main.B.__init__":[],"main.B.func":["main.C.func"],"main.A.__init__":[],"main.A.func":["main.B.__init__","main.B.func"],"main":["main.A.func","main.A.__init__"]}"#;

    fn nested_dump(extra: &[&str]) -> Dump {
        let sym = |k: &str, n: &str| {
            format!(r#"["N","symbol","rb-heuristic . . . `main.py`/{k}","{n}",{{}}]"#)
        };
        let def = |k: &str, l: i32| {
            format!(
                r#"["E","symbol","rb-heuristic . . . `main.py`/{k}","DEFINED_IN","file","main.py","treesitter",5,{{}},[["main.py",[{l},0,{l},1]]]]"#
            )
        };
        let call = |s: &str, d: &str, c: i32, l: i32| {
            format!(
                r#"["E","symbol","rb-heuristic . . . `main.py`/{s}","CALLS","symbol","rb-heuristic . . . `main.py`/{d}","heuristic",{c},{{}},[["main.py",[{l},0,{l},1]]]]"#
            )
        };
        let mut rows = vec![
            r#"["N","file","main.py","main.py",{}]"#.to_owned(),
            sym("A#", "A"),
            sym("A#__init__().", "__init__"),
            sym("A#func().", "func"),
            sym("B#", "B"),
            sym("B#__init__().", "__init__"),
            sym("B#func().", "func"),
            sym("C#", "C"),
            sym("C#func().", "func"),
            sym("a.", "a"),
            def("A#", 11),
            def("A#__init__().", 12),
            def("A#func().", 15),
            def("B#", 4),
            def("B#__init__().", 5),
            def("B#func().", 8),
            def("C#", 0),
            def("C#func().", 1),
            def("a.", 19),
            call("A#func().", "B#__init__().", 2, 16),
            call("A#func().", "B#func().", 2, 17),
            call("a.", "A#__init__().", 2, 19),
            r#"["E","file","main.py","CALLS","symbol","rb-heuristic . . . `main.py`/A#func().","heuristic",2,{},[["main.py",[20,2,20,6]]]]"#.to_owned(),
        ];
        rows.extend(extra.iter().map(|s| (*s).to_owned()));
        parse(&format!("[\"rb-dump\",2,\"x\"]\n{}\n", rows.join("\n"))).unwrap()
    }

    #[test]
    fn a_real_case_maps_both_ways() {
        let cells = score_case(NESTED_CLASS_CALLS, &nested_dump(&[])).unwrap();
        let calls = &cells[&Class::Calls];
        // A.func → B.__init__, A.func → B.func found; B.func → C.func missed.
        assert_eq!((calls.tp_total(), calls.fp_total(), calls.fn_), (2, 0, 1));
        // main → A.func (file row) and main → A.__init__ (sourced at the term `a`) both found.
        let top = &cells[&Class::CallsFileScope];
        assert_eq!((top.tp_total(), top.fp_total(), top.fn_), (2, 0, 0));
        assert_eq!(top.tp.get(&Certainty::Heuristic), Some(&2));
    }

    #[test]
    fn out_of_scope_and_unmatchable_gold() {
        let graph = r#"{"main":["<builtin>.print","os.path.join","main.<lambda1>","main.Z.gone","main.A.func"],"main.Q.nope":["main.A.func"],"main.<lambda1>":["main.A.func"]}"#;
        let cells = score_case(graph, &nested_dump(&[])).unwrap();
        let top = &cells[&Class::CallsFileScope];
        let reasons: Vec<_> = top
            .out_of_scope
            .iter()
            .map(|(r, n)| (r.as_str(), *n))
            .collect();
        assert_eq!(
            reasons,
            vec![("builtin", 1), ("external", 1), ("lambda", 1)]
        );
        assert_eq!(top.unmatchable.get("no_rb_symbol"), Some(&1));
        assert_eq!(top.tp_total(), 1);
        let calls = &cells[&Class::Calls];
        assert_eq!(calls.unmatchable.get("no_rb_symbol"), Some(&1));
        assert_eq!(
            calls.out_of_scope.get("lambda"),
            Some(&1),
            "a lambda caller"
        );
    }

    #[test]
    fn overloads_collapse_and_wrong_edges_are_fp() {
        let extra = [
            r#"["N","symbol","rb-heuristic . . . `main.py`/C#func(+1).","func",{}]"#,
            r#"["E","symbol","rb-heuristic . . . `main.py`/C#func(+1).","DEFINED_IN","file","main.py","treesitter",5,{},[["main.py",[3,0,3,1]]]]"#,
            r#"["E","symbol","rb-heuristic . . . `main.py`/B#func().","CALLS","symbol","rb-heuristic . . . `main.py`/C#func(+1).","heuristic",1,{},[["main.py",[9,0,9,1]]]]"#,
            r#"["E","symbol","rb-heuristic . . . `main.py`/B#func().","CALLS","symbol","rb-heuristic . . . `main.py`/A#func().","heuristic",1,{},[["main.py",[9,0,9,1]]]]"#,
        ];
        let cells = score_case(NESTED_CLASS_CALLS, &nested_dump(&extra)).unwrap();
        let calls = &cells[&Class::Calls];
        // B.func → C.func(+1) is B.func → C.func: found now; B.func → A.func is wrong.
        assert_eq!((calls.tp_total(), calls.fn_), (3, 0));
        assert_eq!(calls.fp.get(&Certainty::Ambiguous), Some(&1));
        assert_eq!((calls.ambiguous_sites, calls.ambiguous_hits), (1, 1));
    }

    fn dump_of(rows: &[String]) -> Dump {
        parse(&format!("[\"rb-dump\",2,\"x\"]\n{}\n", rows.join("\n"))).unwrap()
    }

    fn sym_rows(path: &str, k: &str, name: &str, line: i32) -> [String; 2] {
        [
            format!(r#"["N","symbol","rb-heuristic . . . `{path}`/{k}","{name}",{{}}]"#),
            format!(
                r#"["E","symbol","rb-heuristic . . . `{path}`/{k}","DEFINED_IN","file","{path}","treesitter",5,{{}},[["{path}",[{line},0,{line},1]]]]"#
            ),
        ]
    }

    #[test]
    fn a_nested_function_calls_from_itself() {
        let mut rows = vec![r#"["N","file","main.py","main.py",{}]"#.to_owned()];
        rows.extend(sym_rows("main.py", "check().", "check", 0));
        rows.extend(sym_rows("main.py", "check().helper().", "helper", 1));
        rows.extend(sym_rows("main.py", "g().", "g", 5));
        rows.push(r#"["E","symbol","rb-heuristic . . . `main.py`/check().helper().","CALLS","symbol","rb-heuristic . . . `main.py`/g().","heuristic",2,{},[["main.py",[2,4,2,5]]]]"#.to_owned());
        let cells = score_case(r#"{"main.check.helper":["main.g"]}"#, &dump_of(&rows)).unwrap();
        let calls = &cells[&Class::Calls];
        assert_eq!((calls.tp_total(), calls.fp_total(), calls.fn_), (1, 0, 0));
    }

    #[test]
    fn a_root_package_does_not_make_every_callee_internal() {
        let rows = vec![
            r#"["N","file","__init__.py","__init__.py",{}]"#.to_owned(),
            r#"["N","file","main.py","main.py",{}]"#.to_owned(),
        ];
        let cells = score_case(r#"{"main":["os.path.join"]}"#, &dump_of(&rows)).unwrap();
        let top = &cells[&Class::CallsFileScope];
        assert_eq!(top.out_of_scope.get(reason::EXTERNAL), Some(&1));
    }

    /// `imports/init_func_import` at the PyCG pin, as rb (the M7 binary) dumps it: a package's
    /// `__init__.py` is the package (`nested.func`).
    #[test]
    fn a_package_init_is_the_package() {
        let mut rows = vec![
            r#"["N","file","main.py","main.py",{}]"#.to_owned(),
            r#"["N","file","nested/__init__.py","__init__.py",{}]"#.to_owned(),
            r#"["N","file","nested/mod.py","mod.py",{}]"#.to_owned(),
        ];
        rows.extend(sym_rows("nested/__init__.py", "func().", "func", 2));
        rows.extend(sym_rows("nested/mod.py", "func2().", "func2", 0));
        rows.push(r#"["E","file","main.py","CALLS","symbol","rb-heuristic . . . `nested/__init__.py`/func().","heuristic",2,{},[["main.py",[2,0,2,4]]]]"#.to_owned());
        let graph = r#"{"main":["nested.func","nested.mod.func2"],"nested":[],"nested.func":[],"nested.mod":[],"nested.mod.func2":[]}"#;
        let cells = score_case(graph, &dump_of(&rows)).unwrap();
        let top = &cells[&Class::CallsFileScope];
        assert_eq!((top.tp_total(), top.fp_total(), top.fn_), (1, 0, 1));
        assert_eq!(cells[&Class::Calls].status(), crate::score::Status::Vacuous);
    }

    /// `functions/imported_call` at the PyCG pin: rb finds no call (`a = func; a()`), so the
    /// module-level gold edge is a miss, not an exclusion.
    #[test]
    fn an_unfound_imported_call_is_a_miss() {
        let mut rows = vec![
            r#"["N","file","main.py","main.py",{}]"#.to_owned(),
            r#"["N","file","to_import.py","to_import.py",{}]"#.to_owned(),
        ];
        rows.extend(sym_rows("main.py", "a.", "a", 2));
        rows.extend(sym_rows("to_import.py", "func().", "func", 0));
        let graph = r#"{"main":["to_import.func"],"to_import.func":[],"to_import":[]}"#;
        let cells = score_case(graph, &dump_of(&rows)).unwrap();
        let top = &cells[&Class::CallsFileScope];
        assert_eq!(
            (top.tp_total(), top.fn_, top.status()),
            (0, 1, crate::score::Status::Flagged)
        );
    }
}
