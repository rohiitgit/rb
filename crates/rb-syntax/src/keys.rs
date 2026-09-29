//! Heuristic symbol keys in SCIP symbol shape (design §5.1, §5.4).
//!
//! `rb-heuristic . . . <path>/<descriptors>`: the manager/package/version fields are empty (`.`),
//! the file path is a backtick-escaped namespace descriptor, and each definition contributes a
//! descriptor with SCIP's suffixes (`/` module, `#` type, `.` term, `().` callable). A key depends
//! only on its file's path and contents. SCIP keys replace these per project once M3 lands.

use std::collections::HashMap;

use crate::{DefKind, Definition, FileFacts};

const SCHEME_AND_PACKAGE: &str = "rb-heuristic . . . ";

/// One key per definition, index-aligned with `facts.definitions`.
pub fn symbol_keys(path: &str, facts: &FileFacts) -> Vec<String> {
    let defs = &facts.definitions;
    let descriptors = descriptors(defs);
    let root = format!("{SCHEME_AND_PACKAGE}{}/", escape(path));
    let mut keys: Vec<Option<String>> = vec![None; defs.len()];
    for i in 0..defs.len() {
        key_of(i, defs, &descriptors, &root, &mut keys);
    }
    keys.into_iter().map(|k| k.unwrap_or_default()).collect()
}

/// A definition's key is its parent's key (or the file root, plus a Go receiver) + its descriptor.
fn key_of(
    i: usize,
    defs: &[Definition],
    descriptors: &[String],
    root: &str,
    keys: &mut [Option<String>],
) -> String {
    if let Some(k) = &keys[i] {
        return k.clone();
    }
    let prefix = match (defs[i].parent, &defs[i].receiver) {
        (Some(p), _) => key_of(p, defs, descriptors, root, keys),
        (None, Some(receiver)) => format!("{root}{}#", escape(receiver)),
        (None, None) => root.to_owned(),
    };
    let key = format!("{prefix}{}", descriptors[i]);
    keys[i] = Some(key.clone());
    key
}

/// SCIP descriptors. Callables that share a name within one scope get `(+n)` in source order;
/// repeated type-like declarations (e.g. TypeScript declaration merging) share one descriptor.
fn descriptors(defs: &[Definition]) -> Vec<String> {
    let mut seen: HashMap<(Option<usize>, Option<&str>, &str), usize> = HashMap::new();
    defs.iter()
        .map(|d| {
            let name = escape(&d.name);
            match d.kind {
                DefKind::Module => format!("{name}/"),
                DefKind::Class | DefKind::Interface | DefKind::Type => format!("{name}#"),
                DefKind::Constant => format!("{name}."),
                DefKind::Function | DefKind::Method => {
                    let ordinal = seen
                        .entry((d.parent, d.receiver.as_deref(), d.name.as_str()))
                        .or_insert(0);
                    let descriptor = match *ordinal {
                        0 => format!("{name}()."),
                        n => format!("{name}(+{n})."),
                    };
                    *ordinal += 1;
                    descriptor
                }
            }
        })
        .collect()
}

/// SCIP identifier escaping: plain if `[A-Za-z0-9_+$-]+`, else backticks with ` doubled.
pub(crate) fn escape(name: &str) -> String {
    let plain = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-' | '$'));
    if plain {
        name.to_owned()
    } else {
        format!("`{}`", name.replace('`', "``"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Language, extract};

    fn keys(language: Language, path: &str, src: &str) -> Vec<String> {
        symbol_keys(path, &extract(language, src.as_bytes()).unwrap())
    }

    const PREFIX: &str = "rb-heuristic . . . ";

    #[test]
    fn overloads_get_ordinal_disambiguators_in_source_order() {
        let src =
            "class A {\n  A() {}\n  void f(int x) {}\n  void f(String s) {}\n  void g() {}\n}\n";
        let got = keys(Language::Java, "src/A.java", src);
        let expected: Vec<String> = [
            "`src/A.java`/A#",
            "`src/A.java`/A#A().",
            "`src/A.java`/A#f().",
            "`src/A.java`/A#f(+1).",
            "`src/A.java`/A#g().",
        ]
        .iter()
        .map(|k| format!("{PREFIX}{k}"))
        .collect();
        assert_eq!(got, expected);
    }

    #[test]
    fn descriptors_follow_scip_suffixes_and_escaping() {
        let src = "class C:\n    def m(self):\n        pass\n\ndef café():\n    pass\n\nX = 1\n";
        let got = keys(Language::Python, "a b/c.py", src);
        assert_eq!(
            got,
            [
                format!("{PREFIX}`a b/c.py`/C#"),
                format!("{PREFIX}`a b/c.py`/C#m()."),
                format!("{PREFIX}`a b/c.py`/`café`()."),
                format!("{PREFIX}`a b/c.py`/X."),
            ]
        );
        assert_eq!(super::escape("a`b"), "`a``b`");
        assert_eq!(super::escape("$ok_1-x+"), "$ok_1-x+");
    }

    #[test]
    fn go_methods_are_keyed_under_their_receiver() {
        let src = "package p\ntype Circle struct{}\nfunc (c Circle) Area() int { return 0 }\nfunc (s *Square) Area() int { return 0 }\n";
        assert_eq!(
            keys(Language::Go, "p.go", src),
            [
                format!("{PREFIX}`p.go`/Circle#"),
                format!("{PREFIX}`p.go`/Circle#Area()."),
                format!("{PREFIX}`p.go`/Square#Area()."),
            ]
        );
    }

    #[test]
    fn keys_depend_only_on_path_and_contents() {
        let src = "def f():\n    def g():\n        pass\n";
        let a = keys(Language::Python, "m.py", src);
        let b = keys(Language::Python, "m.py", src);
        assert_eq!(a, b);
        assert_eq!(a[1], format!("{PREFIX}`m.py`/f().g()."));
        assert_ne!(a, keys(Language::Python, "n.py", src));
    }

    #[test]
    fn repeated_type_declarations_share_one_key() {
        // TypeScript declaration merging: both declarations are the same symbol.
        let src = "interface I { a: string }\ninterface I { b: string }\n";
        let got = keys(Language::TypeScript, "i.ts", src);
        assert_eq!(
            got,
            [format!("{PREFIX}`i.ts`/I#"), format!("{PREFIX}`i.ts`/I#")]
        );
    }
}
