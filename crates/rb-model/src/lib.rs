//! Repository Brain graph model (design §8.2, §8.5, §6.3).
//!
//! Identity is always the natural key `(NodeKind, key)`. Dense numeric ids are assigned only when a
//! [`GraphBuilder`] is finished, in sorted key order, so they are deterministic for a given input.

pub mod diff;
mod graph;
mod manifest;

pub use graph::{
    Attrs, Diagnostic, Edge, Evidence, Graph, GraphBuilder, NewEdge, Node, Span, canonical_dump,
    sort_diagnostics,
};
pub use manifest::{FileEntry, Manifest, Skipped};

use std::fmt;
use std::str::FromStr;

use anyhow::bail;

/// Defines a closed string-backed enum with `as_str`, `ALL`, `Display` and `FromStr`.
macro_rules! string_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $text:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];

            pub fn as_str(self) -> &'static str {
                match self { $($name::$variant => $text),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = anyhow::Error;

            fn from_str(s: &str) -> anyhow::Result<Self> {
                match s {
                    $($text => Ok($name::$variant),)+
                    other => bail!(concat!("unknown ", stringify!($name), " {:?}"), other),
                }
            }
        }
    };
}

string_enum! {
    /// Node kinds (design §8.2). The string form is what `rb.node.kind` stores.
    NodeKind {
        Symbol => "symbol",
        File => "file",
        Service => "service",
        Table => "table",
        Owner => "owner",
        Endpoint => "endpoint",
        Package => "package",
        Topic => "topic",
    }
}

impl NodeKind {
    /// Vertex label in the AGE projection.
    pub fn age_label(self) -> &'static str {
        match self {
            NodeKind::Symbol => "Symbol",
            NodeKind::File => "File",
            NodeKind::Service => "Service",
            NodeKind::Table => "Table",
            NodeKind::Owner => "Owner",
            NodeKind::Endpoint => "Endpoint",
            NodeKind::Package => "Package",
            NodeKind::Topic => "Topic",
        }
    }
}

string_enum! {
    /// Edge kinds (design §8.5). The string form is both `rb.edge.kind` and the AGE edge label.
    EdgeKind {
        DefinedIn => "DEFINED_IN",
        InService => "IN_SERVICE",
        Contains => "CONTAINS",
        Calls => "CALLS",
        References => "REFERENCES",
        Implements => "IMPLEMENTS",
        Injects => "INJECTS",
        Queries => "QUERIES",
        MapsTo => "MAPS_TO",
        Migrates => "MIGRATES",
        Exposes => "EXPOSES",
        HandledBy => "HANDLED_BY",
        Consumes => "CONSUMES",
        OwnedBy => "OWNED_BY",
        DependsOn => "DEPENDS_ON",
        Publishes => "PUBLISHES",
        Subscribes => "SUBSCRIBES",
    }
}

string_enum! {
    /// What a `Service` node represents (stored as `attrs.role`; design §4.3).
    ServiceRole {
        Service => "service",
        Library => "library",
        Unassigned => "unassigned",
    }
}

string_enum! {
    /// How an edge was derived (design §6.3).
    Provenance {
        Scip => "scip",
        Treesitter => "treesitter",
        Heuristic => "heuristic",
        Di => "di",
        HttpStitch => "http_stitch",
        SqlLiteral => "sql_literal",
        SqlFile => "sql_file",
        Orm => "orm",
        Codeowners => "codeowners",
        Manifest => "manifest",
        Config => "config",
    }
}

/// How certain an edge is (design §6.3). Ordered: `Ambiguous < … < Exact`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Certainty {
    Ambiguous = 1,
    Heuristic = 2,
    Synthetic = 3,
    Dispatch = 4,
    Exact = 5,
}

impl Certainty {
    pub fn as_str(self) -> &'static str {
        match self {
            Certainty::Ambiguous => "ambiguous",
            Certainty::Heuristic => "heuristic",
            Certainty::Synthetic => "synthetic",
            Certainty::Dispatch => "dispatch",
            Certainty::Exact => "exact",
        }
    }

    /// The value stored in `rb.edge.certainty` and in the AGE projection.
    pub fn rank(self) -> i16 {
        self as i16
    }

    pub fn from_rank(rank: i16) -> anyhow::Result<Self> {
        Ok(match rank {
            1 => Certainty::Ambiguous,
            2 => Certainty::Heuristic,
            3 => Certainty::Synthetic,
            4 => Certainty::Dispatch,
            5 => Certainty::Exact,
            other => bail!("certainty rank out of range: {other}"),
        })
    }
}

/// Natural identity of a node within a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeKey {
    pub kind: NodeKind,
    pub key: String,
}

impl NodeKey {
    pub fn new(kind: NodeKind, key: impl Into<String>) -> Self {
        Self {
            kind,
            key: key.into(),
        }
    }
}

impl fmt::Display for NodeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind, self.key)
    }
}

/// A test source (contract M4b invariant 6; shared by DI, RTA, orphans and `rb impact`'s tests): Maven `src/test/`, a `test`/`tests`/`__tests__` directory,
/// `*Test.java`/`*Tests.java`/`*IT.java`, `*.test.*`/`*.spec.*`, `*_test.go`,
/// `test_*.py`/`*_test.py`.
pub fn is_test_path(path: &str) -> bool {
    let (dirs, name) = path.rsplit_once('/').unwrap_or(("", path));
    if dirs
        .split('/')
        .any(|d| matches!(d, "test" | "tests" | "__tests__"))
    {
        return true;
    }
    if let Some(stem) = name.strip_suffix(".java") {
        return stem.ends_with("Test") || stem.ends_with("Tests") || stem.ends_with("IT");
    }
    if let Some(stem) = name.strip_suffix(".py") {
        return stem.starts_with("test_") || stem.ends_with("_test");
    }
    name.ends_with("_test.go") || name.contains(".test.") || name.contains(".spec.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn string_forms_round_trip() {
        for k in NodeKind::ALL {
            assert_eq!(k.as_str().parse::<NodeKind>().unwrap(), *k);
        }
        for k in EdgeKind::ALL {
            assert_eq!(k.as_str().parse::<EdgeKind>().unwrap(), *k);
        }
        for p in Provenance::ALL {
            assert_eq!(p.as_str().parse::<Provenance>().unwrap(), *p);
        }
        assert!("nope".parse::<NodeKind>().is_err());
    }

    #[test]
    fn certainty_is_ordered_and_round_trips() {
        assert!(Certainty::Exact > Certainty::Dispatch);
        assert!(Certainty::Heuristic > Certainty::Ambiguous);
        for r in 1..=5 {
            assert_eq!(Certainty::from_rank(r).unwrap().rank(), r);
        }
        assert!(Certainty::from_rank(0).is_err());
    }

    #[test]
    fn test_paths() {
        for (path, want) in [
            ("src/test/java/a/B.java", true),
            ("a/__tests__/x.ts", true),
            ("tests/x.py", true),
            ("src/main/java/a/BTest.java", true),
            ("src/main/java/a/BTests.java", true),
            ("src/main/java/a/BIT.java", true),
            ("web/a.spec.ts", true),
            ("web/a.test.js", true),
            ("pkg/a_test.go", true),
            ("pkg/test_a.py", true),
            ("pkg/a_test.py", true),
            ("src/main/java/a/Testing.java", false),
            ("src/main/java/a/Contest.java", false),
            ("pkg/latest/a.go", false),
        ] {
            assert_eq!(is_test_path(path), want, "{path}");
        }
    }
}
