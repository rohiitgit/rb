//! Contract packs (design §4.5, contract M6f S1): OpenAPI specs as authoritative endpoint
//! sources. JSON reads through `serde_json`; YAML through an indented line reader over
//! allow-listed shapes (no new dependency, M6e-F4 precedent): `paths:` at column 0, paths at
//! indent 2, methods at indent 4, `operationId` at indent 6. Anything else contributes no
//! operation; a tab-indented line or a `paths:` section with entries but zero readable
//! operations makes the whole document unreadable (`None`: the indexer counts
//! `openapi_unread` and creates no endpoints, today's safe direction).

use crate::work::tick;
use rb_syntax::Span;

/// One spec operation: its raw in-spec path, upper-cased method, operation ID (empty when
/// the operation states none) and evidence span — the path line for YAML, file start for
/// JSON (serde reports no positions).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SpecOp {
    pub path: String,
    pub method: String,
    pub operation: String,
    pub span: Span,
}

/// A file this stage reads, by file name.
pub fn is_spec_input(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    matches!(
        name,
        "openapi.json"
            | "openapi.yaml"
            | "openapi.yml"
            | "swagger.json"
            | "swagger.yaml"
            | "swagger.yml"
    )
}

/// The seven OpenAPI operations, lower-case as written.
fn verb(token: &str) -> bool {
    matches!(
        token,
        "get" | "put" | "post" | "delete" | "options" | "head" | "patch" | "trace"
    )
}

fn sorted(mut ops: Vec<SpecOp>) -> Vec<SpecOp> {
    ops.sort();
    ops
}

/// Reads an OpenAPI/Swagger JSON document: `None` when it has no `paths` object (not a spec,
/// or unreadable).
pub fn openapi_json(text: &str) -> Option<Vec<SpecOp>> {
    let json: serde_json::Value = serde_json::from_str(text).ok()?;
    let paths = json.get("paths")?.as_object()?;
    let mut ops = Vec::new();
    for (path, item) in paths {
        tick(1);
        if !path.starts_with('/') {
            continue;
        }
        let Some(methods) = item.as_object() else {
            continue;
        };
        for (method, op) in methods {
            tick(1);
            if !verb(method) {
                continue;
            };
            let operation = op
                .as_object()
                .and_then(|o| o.get("operationId"))
                .and_then(|o| o.as_str())
                .unwrap_or_default()
                .to_owned();
            ops.push(SpecOp {
                path: path.clone(),
                method: method.to_uppercase(),
                operation,
                span: [0, 0, 0, 0],
            });
        }
    }
    Some(sorted(ops))
}

/// Reads an OpenAPI/Swagger YAML document with the 0/2/4/6-space shapes: `None` when a line
/// is tab-indented, or the `paths:` section holds entries but yields zero operations.
pub fn openapi_yaml(text: &str) -> Option<Vec<SpecOp>> {
    let mut ops: Vec<SpecOp> = Vec::new();
    let mut in_paths = false;
    let mut had_entries = false;
    let mut path: Option<(String, Span)> = None;
    let mut op: Option<usize> = None;
    for (row, line) in text.split_inclusive('\n').enumerate() {
        tick(1);
        if line.contains('\t') {
            return None;
        }
        let indent = line.len() - line.trim_start_matches(' ').len();
        let mut content = line.trim();
        content = content.strip_suffix('\n').unwrap_or(content);
        let content = content.strip_suffix('\r').unwrap_or(content);
        let row = row as i32;
        let width = content.len() as i32;
        if content.is_empty() || content.starts_with('#') {
            continue;
        }
        if indent == 0 {
            in_paths = content == "paths:" || content.starts_with("paths: ");
            if content == "paths: {}" {
                return Some(Vec::new());
            }
            path = None;
            op = None;
            continue;
        }
        if !in_paths {
            continue;
        }
        if indent == 2 {
            had_entries = true;
            path = path_token(content).map(|path| {
                let span = [row, 0, row, indent as i32 + width];
                (path, span)
            });
            op = None;
        } else if indent == 4 {
            op = match path {
                Some(_) => method_token(content).map(|method| {
                    let (ref path, span) = path.clone().unwrap_or_default();
                    ops.push(SpecOp {
                        path: path.clone(),
                        method,
                        operation: String::new(),
                        span,
                    });
                    ops.len() - 1
                }),
                None => None,
            };
        } else if indent >= 6
            && let (Some(i), Some(rest)) = (op, content.strip_prefix("operationId:"))
            && ops[i].operation.is_empty()
        {
            ops[i].operation = scalar(rest);
        }
    }
    if had_entries && ops.is_empty() {
        return None;
    }
    Some(sorted(ops))
}

/// A path line (`/users:`, `"/users/{id}":`, trailing comments allowed): the raw path, or
/// `None` for anything else.
fn path_token(content: &str) -> Option<String> {
    let content = content.split(" #").next().unwrap_or(content).trim_end();
    let content = content.strip_suffix(':')?.trim_end();
    let path = content
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| {
            content
                .strip_prefix('\'')
                .and_then(|s| s.strip_suffix('\''))
        })
        .unwrap_or(content);
    path.starts_with('/').then(|| path.to_owned())
}

/// A method line (`get:`, `post: # comment`): the upper-cased method, or `None`.
fn method_token(content: &str) -> Option<String> {
    let content = content.split(" #").next().unwrap_or(content).trim_end();
    let token = content.strip_suffix(':').unwrap_or(content);
    verb(token).then(|| token.to_uppercase())
}

/// A scalar value (operation IDs): comments and surrounding quotes stripped.
fn scalar(rest: &str) -> String {
    let value = rest.split(" #").next().unwrap_or(rest).trim();
    value
        .strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')))
        .unwrap_or(value)
        .to_owned()
}

/// One RPC: its name, whether either side streams (recorded from the RPC text — the grammar
/// exposes no node kind for `stream`), and its name span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoRpc {
    pub name: String,
    pub stream: bool,
    pub span: rb_syntax::Span,
}

/// One proto service: its package, name, RPCs and name span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtoService {
    pub package: String,
    pub name: String,
    pub rpcs: Vec<ProtoRpc>,
    pub span: rb_syntax::Span,
}

/// A `.proto` file, by file name.
pub fn is_proto_input(path: &str) -> bool {
    path.rsplit('/').next().unwrap_or(path).ends_with(".proto")
}

/// Reads a `.proto` file's services: package (first `package` clause), service names and RPC
/// names with spans from the tree. Never fails: garbage yields no services, and an RPC
/// outside a service is ignored. The walk is bounded (depth cap 64, counted).
pub fn proto_services(text: &str) -> Vec<ProtoService> {
    let mut parser = tree_sitter::Parser::new();
    let Ok(()) = parser.set_language(&tree_sitter_proto::LANGUAGE.into()) else {
        return Vec::new();
    };
    let Some(tree) = parser.parse(text, None) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut package = String::new();
    walk(tree.root_node(), text.as_bytes(), 0, &mut package, &mut out);
    out
}

fn span_of(node: tree_sitter::Node) -> rb_syntax::Span {
    let (s, e) = (node.start_position(), node.end_position());
    [s.row as i32, s.column as i32, e.row as i32, e.column as i32]
}

fn node_text<'a>(node: tree_sitter::Node, bytes: &'a [u8]) -> &'a str {
    node.utf8_text(bytes).unwrap_or_default()
}

fn walk(
    node: tree_sitter::Node,
    bytes: &[u8],
    depth: usize,
    package: &mut String,
    out: &mut Vec<ProtoService>,
) {
    if depth > 64 {
        return;
    }
    tick(1);
    match node.kind() {
        "package" => {
            if package.is_empty() {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "full_ident" {
                        package.push_str(node_text(child, bytes));
                        break;
                    }
                }
            }
        }
        "service" => {
            let mut name = String::new();
            let mut span = [0, 0, 0, 0];
            let mut rpcs = Vec::new();
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                match child.kind() {
                    "service_name" => {
                        name = node_text(child, bytes).to_owned();
                        span = span_of(child);
                    }
                    "rpc" => {
                        let mut rpc_name = String::new();
                        let mut rpc_span = [0, 0, 0, 0];
                        let mut rcursor = child.walk();
                        for grand in child.children(&mut rcursor) {
                            if grand.kind() == "rpc_name" {
                                rpc_name = node_text(grand, bytes).to_owned();
                                rpc_span = span_of(grand);
                                break;
                            }
                        }
                        if !rpc_name.is_empty() {
                            let stream = node_text(child, bytes)
                                .split(|c: char| !c.is_alphanumeric() && c != '_')
                                .any(|w| w == "stream");
                            rpcs.push(ProtoRpc {
                                name: rpc_name,
                                stream,
                                span: rpc_span,
                            });
                        }
                    }
                    _ => {}
                }
            }
            if !name.is_empty() {
                out.push(ProtoService {
                    package: package.clone(),
                    name,
                    rpcs,
                    span,
                });
            }
        }
        _ => {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                walk(child, bytes, depth + 1, package, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work;
    use rb_syntax::Span;

    /// Contract M6f S1: JSON specs pin paths, methods and operation IDs; `servers`,
    /// `webhooks`, `components`, `$ref`s and unknown method keys contribute nothing.
    #[test]
    fn openapi_json_reads_paths_methods_and_operation_ids() {
        let doc = r##"{
            "openapi": "3.1.0",
            "servers": [{"url": "https://x.example"}],
            "paths": {
                "/users": {
                    "get": {"operationId": "listUsers"},
                    "post": {},
                    "parameters": [{"name": "q"}]
                },
                "/users/{id}": {"get": {"operationId": "getUser"}, "x-trace": {}},
                "/linked": {"$ref": "#/components/x"},
                "relative": {"get": {}}
            },
            "webhooks": {"/hook": {"post": {}}},
            "components": {}
        }"##;
        assert_eq!(
            openapi_json(doc).unwrap(),
            [
                SpecOp {
                    path: "/users".into(),
                    method: "GET".into(),
                    operation: "listUsers".into(),
                    span: [0, 0, 0, 0],
                },
                SpecOp {
                    path: "/users".into(),
                    method: "POST".into(),
                    operation: "".into(),
                    span: [0, 0, 0, 0],
                },
                SpecOp {
                    path: "/users/{id}".into(),
                    method: "GET".into(),
                    operation: "getUser".into(),
                    span: [0, 0, 0, 0],
                },
            ]
        );
        assert_eq!(
            openapi_json(r#"{"openapi": "3.0.0", "paths": {}}"#).unwrap(),
            []
        );
        assert_eq!(openapi_json(r#"{"nope": true}"#), None);
        assert_eq!(openapi_json("{broken"), None);
        assert_eq!(openapi_json(r#"{"paths": []}"#), None);
    }

    /// Contract M6f S1: YAML specs read through the 0/2/4/6 shapes; tabs, or entries with
    /// zero readable operations, make the document unreadable.
    #[test]
    fn openapi_yaml_reads_the_indented_shapes() {
        let doc = "# users API\nopenapi: 3.0.0\npaths:\n  /users:\n    get: # list\n      operationId: listUsers\n    post:\n      operationId: 'addUser'\n  \"/users/{id}\":\n    get:\n      operationId: \"getUser\"\n  /ignored:\n    parameters: []\nservers:\n  - url: https://x.example\n";
        // Positions are never hand-counted: path-line spans from the text.
        let span_of = |needle: &str| {
            let pos = doc.find(needle).unwrap();
            let row = doc[..pos].matches('\n').count() as i32;
            let end = doc[pos..].find('\n').map_or(doc.len(), |n| pos + n);
            [row, 0, row, (end - pos) as i32]
        };
        assert_eq!(
            openapi_yaml(doc).unwrap(),
            [
                SpecOp {
                    path: "/users".into(),
                    method: "GET".into(),
                    operation: "listUsers".into(),
                    span: span_of("  /users:"),
                },
                SpecOp {
                    path: "/users".into(),
                    method: "POST".into(),
                    operation: "addUser".into(),
                    span: span_of("  /users:"),
                },
                SpecOp {
                    path: "/users/{id}".into(),
                    method: "GET".into(),
                    operation: "getUser".into(),
                    span: span_of("  \"/users/{id}\":"),
                },
            ]
        );
        assert_eq!(openapi_yaml("openapi: 3.0.0\npaths: {}\n").unwrap(), []);
        assert_eq!(openapi_yaml("paths:\n\t/users:\n"), None);
        assert_eq!(openapi_yaml("paths:\n  /users:\n    get: [1]\n"), None);
        assert_eq!(openapi_yaml("nothing here\n"), Some(Vec::new()));
    }

    /// Contract M6f S1: spec file names, and only those, reach the reader.
    #[test]
    fn spec_inputs_are_the_allow_listed_basenames() {
        for f in [
            "openapi.json",
            "openapi.yaml",
            "openapi.yml",
            "swagger.json",
            "swagger.yaml",
            "swagger.yml",
            "svc/openapi.yaml",
        ] {
            assert!(is_spec_input(f), "{f}");
        }
        for f in [
            "openapi.txt",
            "myopenapi.yaml",
            "openapi.json.bak",
            "paths.yaml",
            "api.json",
        ] {
            assert!(!is_spec_input(f), "{f}");
        }
    }

    /// Contract M6f S1, invariant 5: reading 4× the lines costs at most 5× the operations.
    #[test]
    fn openapi_reading_grows_linearly() {
        let work = |n: usize| {
            let mut doc = String::from("openapi: 3.0.0\npaths:\n");
            for i in 0..n {
                doc.push_str(&format!(
                    "  /r{i}:\n    get:\n      operationId: op{i}\n    post:\n      operationId: mk{i}\n"
                ));
            }
            work::take();
            let ops = openapi_yaml(&doc).unwrap();
            assert_eq!(ops.len(), 2 * n);
            work::take()
        };
        let (small, large) = (work(100), work(400));
        assert!(large <= small * 5, "{small} -> {large}");
    }

    /// Contract M6f S2: `.proto` services pin their package, name and RPCs; streaming is
    /// recorded from the RPC text (the grammar exposes no node kind for it); anything else
    /// contributes nothing and never fails.
    #[test]
    fn proto_services_pin_package_names_and_rpcs() {
        let text = "syntax = \"proto3\";\npackage acme;\n\nservice Users {\n  rpc Get (GetReq) returns (GetResp);\n  rpc Watch (stream W) returns (stream W);\n}\n\nservice Empty {}\n\nmessage GetReq {}\n";
        let services = proto_services(text);
        assert_eq!(services.len(), 2);
        assert_eq!(services[0].package, "acme");
        assert_eq!(services[0].name, "Users");
        assert_eq!(
            services[0]
                .rpcs
                .iter()
                .map(|r| (r.name.as_str(), r.stream))
                .collect::<Vec<_>>(),
            [("Get", false), ("Watch", true)]
        );
        assert!(services[1].rpcs.is_empty());
        // Positions are never hand-counted: spans come from the tree.
        let span_of = |span: &Span| {
            let lines: Vec<&str> = text.split('\n').collect();
            let off = |row: i32, col: i32| {
                lines[..row as usize]
                    .iter()
                    .map(|l| l.len() + 1)
                    .sum::<usize>()
                    + col as usize
            };
            off(span[0], span[1])..off(span[2], span[3])
        };
        let span = services[0].rpcs[0].span;
        assert_eq!(&text[span_of(&span)], "Get");
        assert_eq!(proto_services("syntax = \"proto3\";\nmessage A {}\n"), []);
        assert_eq!(proto_services("))) garbage ((("), []);
        assert!(is_proto_input("svc/a.proto"));
        assert!(!is_proto_input("svc/a.prot"));
        assert!(!is_proto_input("svc/proto"));
    }

    /// Contract M6f S2, invariant 5: reading 4× the services costs at most 5× the operations.
    #[test]
    fn proto_reading_grows_linearly() {
        let work = |n: usize| {
            let mut text = String::from("syntax = \"proto3\";\npackage p;\n");
            for i in 0..n {
                text.push_str(&format!(
                    "service S{i} {{\n  rpc M{i} (A) returns (B);\n}}\n"
                ));
            }
            work::take();
            let services = proto_services(&text);
            assert_eq!(services.len(), n);
            work::take()
        };
        let (small, large) = (work(50), work(200));
        assert!(large <= small * 5, "{small} -> {large}");
    }
}
