//! Call chains (contract M4a S1): every outermost call expression with its receiver root and its
//! links in source order, each link with a summary of its arguments. Nothing here knows a
//! framework: route and client packs read these facts.

use std::collections::HashMap;

use tree_sitter::Node;

use crate::{Arg, CallLink, Init, Language, MAX_EXPR_BYTES, ObjectEntry, span, strings, text};

/// A chain before its enclosing definition is resolved.
pub(crate) struct RawChain<'t> {
    pub node: Node<'t>,
    pub root: Option<String>,
    pub links: Vec<CallLink>,
    /// The left side as written when the chain is the right side of an assignment.
    pub assigned: Option<String>,
}

/// Node kinds of one grammar that make up calls and their arguments.
struct Syntax {
    call: &'static str,
    /// Member access whose object is a receiver: `(kind, object field, property field)`.
    member: (&'static str, &'static str, &'static str),
    /// Expressions that are a name or a member path when made only of these.
    names: &'static [&'static str],
    functions: &'static [&'static str],
    object: Option<&'static str>,
    /// Calls inside these are metadata (decorators), not calls.
    skip: &'static [&'static str],
}

fn syntax(language: Language) -> Syntax {
    match language {
        Language::TypeScript | Language::Tsx | Language::JavaScript => Syntax {
            call: "call_expression",
            member: ("member_expression", "object", "property"),
            names: &[
                "identifier",
                "this",
                "property_identifier",
                "private_property_identifier",
                "meta_property",
                "import",
            ],
            functions: &[
                "arrow_function",
                "function_expression",
                "function",
                "generator_function",
            ],
            object: Some("object"),
            skip: &["decorator"],
        },
        Language::Java => Syntax {
            call: "method_invocation",
            member: ("field_access", "object", "field"),
            names: &["identifier", "this", "super"],
            functions: &["lambda_expression", "method_reference"],
            object: None,
            skip: &["annotation", "marker_annotation"],
        },
        Language::Go => Syntax {
            call: "call_expression",
            member: ("selector_expression", "operand", "field"),
            names: &["identifier", "field_identifier", "package_identifier"],
            functions: &["func_literal"],
            object: None,
            skip: &[],
        },
        Language::Python => Syntax {
            call: "call",
            member: ("attribute", "object", "attribute"),
            names: &["identifier"],
            functions: &["lambda"],
            object: None,
            skip: &["decorator"],
        },
    }
}

/// Every outermost call chain in the file, in source order.
pub(crate) fn chains<'t>(language: Language, root: Node<'t>, source: &[u8]) -> Vec<RawChain<'t>> {
    let syn = syntax(language);
    let mut out = Vec::new();
    let mut stack = vec![root];
    let mut cursor = root.walk();
    while let Some(node) = stack.pop() {
        if syn.skip.contains(&node.kind()) {
            continue;
        }
        if node.kind() == syn.call && !is_inner_link(language, &syn, node) {
            out.push(chain(language, &syn, node, source));
        }
        let children: Vec<Node<'t>> = node.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    out
}

/// The callee and arguments of a call: `(receiver, name, arguments)`. The receiver is the object
/// of a member callee (`a.b` in `a.b.c()`), or the callee itself when it is a call (`f()` in
/// `f()(x)`).
fn split<'t>(
    language: Language,
    syn: &Syntax,
    call: Node<'t>,
) -> (Option<Node<'t>>, Option<Node<'t>>, Option<Node<'t>>) {
    let arguments = call.child_by_field_name("arguments");
    if language == Language::Java {
        return (
            call.child_by_field_name("object"),
            call.child_by_field_name("name"),
            arguments,
        );
    }
    let Some(function) = call.child_by_field_name("function") else {
        return (None, None, arguments);
    };
    let (member, object, property) = syn.member;
    if function.kind() == member {
        (
            function.child_by_field_name(object),
            function.child_by_field_name(property),
            arguments,
        )
    } else if syn.names.contains(&function.kind()) {
        (None, Some(function), arguments)
    } else if function.kind() == syn.call {
        (Some(function), None, arguments)
    } else {
        // An immediately invoked function, `arr[0](x)`, …: no name and no receiver.
        (None, None, arguments)
    }
}

/// A call that is the receiver of another call in the same chain (`a.b()` in `a.b().c()`).
fn is_inner_link(language: Language, syn: &Syntax, call: Node<'_>) -> bool {
    let Some(parent) = call.parent() else {
        return false;
    };
    let outer = if parent.kind() == syn.call {
        Some(parent)
    } else if parent.kind() == syn.member.0 {
        parent.parent().filter(|p| p.kind() == syn.call)
    } else {
        None
    };
    outer.is_some_and(|outer| split(language, syn, outer).0 == Some(call))
}

fn chain<'t>(language: Language, syn: &Syntax, outer: Node<'t>, source: &[u8]) -> RawChain<'t> {
    let mut links = Vec::new();
    let mut root = None;
    let mut call = outer;
    loop {
        let (receiver, name, arguments) = split(language, syn, call);
        let callee = name.or_else(|| call.child_by_field_name("function"));
        links.push(CallLink {
            name: name.map(|n| text(n, source)).unwrap_or_default(),
            span: span(callee.unwrap_or(call)),
            args: arguments.map_or_else(Vec::new, |a| args(language, syn, a, source)),
        });
        match receiver {
            Some(r) if r.kind() == syn.call => call = r,
            Some(r) => {
                root = Some(capped(text(r, source)));
                break;
            }
            None => break,
        }
    }
    links.reverse();
    RawChain {
        node: outer,
        root,
        links,
        assigned: assignment_target(outer, source),
    }
}

/// The left side of the assignment whose right side is `call` (parentheses looked through):
/// `this.rt = b.build()`, `web = WebClient.create(…)` (TS/JS and Java `assignment_expression`).
fn assignment_target(call: Node<'_>, source: &[u8]) -> Option<String> {
    let mut node = call;
    let mut parent = node.parent()?;
    while parent.kind() == "parenthesized_expression" {
        node = parent;
        parent = node.parent()?;
    }
    if parent.kind() != "assignment_expression"
        || parent.child_by_field_name("right").map(|r| r.id()) != Some(node.id())
    {
        return None;
    }
    parent
        .child_by_field_name("left")
        .map(|l| capped(text(l, source)))
}

fn args(language: Language, syn: &Syntax, arguments: Node<'_>, source: &[u8]) -> Vec<Arg> {
    // A tagged template's argument is the template itself.
    if let Some(template) = strings::string_template(language, arguments, source) {
        return vec![Arg::Template { template }];
    }
    let mut c = arguments.walk();
    arguments
        .named_children(&mut c)
        .filter(|a| !matches!(a.kind(), "comment" | "line_comment" | "block_comment"))
        .map(|a| summary(language, syn, a, source, 0))
        .collect()
}

/// The summary of one argument expression; object literals are read one level deep.
fn summary(language: Language, syn: &Syntax, node: Node<'_>, source: &[u8], depth: usize) -> Arg {
    let mut node = node;
    while node.kind() == "parenthesized_expression" && node.named_child_count() == 1 {
        match node.named_child(0) {
            Some(inner) => node = inner,
            None => break,
        }
    }
    if let Some(template) = strings::string_template(language, node, source) {
        return Arg::Template { template };
    }
    if is_name(syn, node) {
        return Arg::Name {
            text: capped(text(node, source)),
            span: span(node),
        };
    }
    if syn.functions.contains(&node.kind()) {
        return Arg::Function { span: span(node) };
    }
    if syn.object == Some(node.kind()) && depth == 0 {
        let mut c = node.walk();
        let entries = node
            .named_children(&mut c)
            .filter_map(|e| match e.kind() {
                "pair" => {
                    let key = e.child_by_field_name("key")?;
                    let key = strings::string_template(language, key, source)
                        .and_then(|t| match t.parts.as_slice() {
                            [crate::TemplatePart::Lit { text }] => Some(text.clone()),
                            _ => None,
                        })
                        .unwrap_or_else(|| text(key, source));
                    let value = e.child_by_field_name("value")?;
                    Some(ObjectEntry {
                        key,
                        value: summary(language, syn, value, source, depth + 1),
                    })
                }
                "shorthand_property_identifier" => Some(ObjectEntry {
                    key: text(e, source),
                    value: Arg::Name {
                        text: text(e, source),
                        span: span(e),
                    },
                }),
                "method_definition" => Some(ObjectEntry {
                    key: e
                        .child_by_field_name("name")
                        .map(|n| text(n, source))
                        .unwrap_or_default(),
                    value: Arg::Function { span: span(e) },
                }),
                // `...cfg`: it may set any key, so it is kept (contract M4a amendment 12).
                "spread_element" => Some(ObjectEntry {
                    key: "...".into(),
                    value: e.named_child(0).map_or(Arg::Other { span: span(e) }, |v| {
                        summary(language, syn, v, source, depth + 1)
                    }),
                }),
                _ => None,
            })
            .collect();
        return Arg::Object {
            span: span(node),
            entries,
        };
    }
    if language == Language::Java && node.kind() == "object_creation_expression" && depth == 0 {
        let type_name = node
            .child_by_field_name("type")
            .map(|t| {
                // Without type arguments: `Factory<X>` is `Factory`.
                let t = match t.kind() {
                    "generic_type" => t.named_child(0).unwrap_or(t),
                    _ => t,
                };
                capped(text(t, source))
            })
            .unwrap_or_default();
        let args = node
            .child_by_field_name("arguments")
            .map_or_else(Vec::new, |a| {
                let mut c = a.walk();
                a.named_children(&mut c)
                    .filter(|a| !matches!(a.kind(), "line_comment" | "block_comment"))
                    .map(|a| summary(language, syn, a, source, depth + 1))
                    .collect()
            });
        return Arg::New {
            type_name,
            span: span(node),
            args,
        };
    }
    Arg::Other { span: span(node) }
}

/// A binding's initialiser: a string, a call (by its index in `chains`, keyed by node id), a
/// name, `name ?? 'lit'` / `name || 'lit'`, or a TS/JS object literal. Parentheses and
/// `await` are looked through.
pub(crate) fn init(
    language: Language,
    value: Node<'_>,
    source: &[u8],
    chains: &HashMap<usize, usize>,
) -> Option<Init> {
    let syn = syntax(language);
    let mut node = value;
    while matches!(node.kind(), "parenthesized_expression" | "await_expression")
        && node.named_child_count() == 1
    {
        node = node.named_child(0)?;
    }
    if let Some(template) = strings::string_template(language, node, source) {
        return Some(Init::Template { template });
    }
    if let Some(&chain) = chains.get(&node.id()) {
        return Some(Init::Call { chain });
    }
    if language == Language::Java && node.kind() == "object_creation_expression" {
        let type_name = node.child_by_field_name("type").map(|t| {
            let t = match t.kind() {
                "generic_type" => t.named_child(0).unwrap_or(t),
                _ => t,
            };
            capped(text(t, source))
        })?;
        let args = node.child_by_field_name("arguments").map_or(0, |a| {
            let mut c = a.walk();
            a.named_children(&mut c)
                .filter(|a| !matches!(a.kind(), "line_comment" | "block_comment"))
                .count()
        });
        let mut c = node.walk();
        let body = node
            .named_children(&mut c)
            .any(|n| n.kind() == "class_body");
        return Some(Init::New {
            type_name,
            args,
            body,
        });
    }
    if syn.object == Some(node.kind())
        && let Arg::Object { entries, .. } = summary(language, &syn, node, source, 0)
    {
        return Some(Init::Object { entries });
    }
    if is_name(&syn, node) {
        return Some(Init::Name {
            text: capped(text(node, source)),
            span: span(node),
        });
    }
    let fallback = node.kind() == "binary_expression"
        && node
            .child_by_field_name("operator")
            .is_some_and(|op| matches!(op.kind(), "??" | "||"));
    if fallback {
        let primary = node
            .child_by_field_name("left")
            .filter(|l| is_name(&syn, *l))?;
        let default =
            strings::string_template(language, node.child_by_field_name("right")?, source)?;
        return Some(Init::Fallback {
            primary: capped(text(primary, source)),
            default,
        });
    }
    None
}

/// An identifier, or a member path made only of names (`this.base`, `import.meta.env.X`,
/// `HttpMethod.POST`, `pkg.Const`).
fn is_name(syn: &Syntax, node: Node<'_>) -> bool {
    if syn.names.contains(&node.kind()) {
        return true;
    }
    let (member, object, property) = syn.member;
    node.kind() == member
        && node
            .child_by_field_name(object)
            .is_some_and(|o| is_name(syn, o))
        && node
            .child_by_field_name(property)
            .is_some_and(|p| syn.names.contains(&p.kind()))
}

/// `s` cut to at most [`MAX_EXPR_BYTES`] bytes, on a character boundary.
pub(crate) fn capped(mut s: String) -> String {
    if s.len() > MAX_EXPR_BYTES {
        let mut end = MAX_EXPR_BYTES;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

#[cfg(test)]
mod tests {
    use crate::{Arg, CallChain, Language, Span, TemplatePart, extract};

    /// The span of the `n`th (0-based) occurrence of `token` in `src`, derived from the text.
    fn at(src: &str, token: &str, n: usize) -> Span {
        let start = src
            .match_indices(token)
            .nth(n)
            .unwrap_or_else(|| panic!("{token} #{n}"))
            .0;
        let pos = |byte: usize| {
            let before = &src[..byte];
            let row = before.matches('\n').count() as i32;
            let col = (byte - before.rfind('\n').map_or(0, |i| i + 1)) as i32;
            (row, col)
        };
        let (a, b) = (pos(start), pos(start + token.len()));
        [a.0, a.1, b.0, b.1]
    }

    /// The span of `token` inside the first occurrence of `context`.
    fn within(src: &str, context: &str, token: &str) -> Span {
        let base = src.find(context).expect(context);
        let offset = context.find(token).expect(token);
        let n = src[..base + offset].matches(token).count();
        at(src, token, n)
    }

    fn chains(language: Language, src: &str) -> Vec<CallChain> {
        extract(language, src.as_bytes()).unwrap().chains
    }

    /// `root.link(args).link(args)` with argument kinds abbreviated, per chain.
    fn outline(language: Language, src: &str) -> Vec<String> {
        chains(language, src)
            .iter()
            .map(|c| {
                let links: Vec<String> = c
                    .links
                    .iter()
                    .map(|l| {
                        let args: Vec<String> = l.args.iter().map(short).collect();
                        format!("{}({})", l.name, args.join(", "))
                    })
                    .collect();
                format!(
                    "{}{}",
                    c.root
                        .as_deref()
                        .map(|r| format!("{r}."))
                        .unwrap_or_default(),
                    links.join(".")
                )
            })
            .collect()
    }

    fn short(a: &Arg) -> String {
        match a {
            Arg::Template { template } => {
                let parts: Vec<String> = template
                    .parts
                    .iter()
                    .map(|p| match p {
                        TemplatePart::Lit { text } => text.clone(),
                        TemplatePart::Hole { expr, .. } => format!("{{{expr}}}"),
                    })
                    .collect();
                format!("'{}'", parts.join(""))
            }
            Arg::Name { text, .. } => text.clone(),
            Arg::Function { .. } => "fn".into(),
            Arg::Object { entries, .. } => {
                let e: Vec<String> = entries
                    .iter()
                    .map(|e| format!("{}: {}", e.key, short(&e.value)))
                    .collect();
                format!("{{{}}}", e.join(", "))
            }
            Arg::New {
                type_name, args, ..
            } => {
                let a: Vec<String> = args.iter().map(short).collect();
                format!("new {type_name}({})", a.join(", "))
            }
            Arg::Other { .. } => "?".into(),
        }
    }

    #[test]
    fn typescript_chains_nested_calls_and_callbacks() {
        let src = "router.route('/x').get(h).post(h2);\napp.use(\"/api\", asyncHandler(async (req, res) => { await this.repo.save(req.body); }));\nconst r = await fetch(`${base}/u/${id}?q=1`, { method: 'POST', headers, deep: { a: 1 } });\naxios.post(API + \"/login\", creds).then(r => r.data);\n// app.get('/commented', h);\n";
        assert_eq!(
            outline(Language::TypeScript, src),
            [
                "router.route('/x').get(h).post(h2)",
                "app.use('/api', ?)",
                "asyncHandler(fn)",
                "this.repo.save(req.body)",
                "fetch('{base}/u/{id}?q=1', {method: 'POST', headers: headers, deep: ?})",
                "axios.post('{API}/login', creds).then(fn)",
            ]
        );
        let all = chains(Language::TypeScript, src);
        // Spans come from the source text: the chain, a link's name, an argument, a hole.
        assert_eq!(all[0].span, {
            let a = at(src, "router", 0);
            let b = at(src, "(h2)", 0);
            [a[0], a[1], b[2], b[3]]
        });
        assert_eq!(all[0].links[2].span, at(src, "post", 0));
        let Arg::Template { template } = &all[4].links[0].args[0] else {
            panic!("{:?}", all[4]);
        };
        assert_eq!(
            template.parts[0],
            TemplatePart::Hole {
                expr: "base".into(),
                span: at(src, "base", 0)
            }
        );
        // The wrapper call is an argument of `use` and a chain of its own, at the same span.
        assert_eq!(all[1].links[0].args[1].span(), all[2].span);
        assert_eq!(all[0].links[1].args[0].span(), within(src, "get(h)", "h"));
    }

    #[test]
    fn tsx_chains() {
        let src = "export function App() {\n  return <button onClick={() => api.post('/x', { id })} />;\n}\n";
        assert_eq!(outline(Language::Tsx, src), ["api.post('/x', {id: id})"]);
    }

    #[test]
    fn calls_inside_callbacks_keep_their_enclosing_definition() {
        let src = "export function routes(app) {\n  app.get('/a', (req, res) => { res.json(load()); });\n}\n";
        let facts = extract(Language::JavaScript, src.as_bytes()).unwrap();
        let names: Vec<(String, Option<String>)> = facts
            .chains
            .iter()
            .map(|c| {
                (
                    c.links.last().unwrap().name.clone(),
                    c.enclosing.map(|i| facts.definitions[i].name.clone()),
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                ("get".to_owned(), Some("routes".to_owned())),
                ("json".to_owned(), Some("routes".to_owned())),
                ("load".to_owned(), Some("routes".to_owned())),
            ]
        );
    }

    #[test]
    fn decorators_are_not_calls() {
        let src = "class C { @Get(':id') find() { return go(); } }\n";
        assert_eq!(outline(Language::TypeScript, src), ["go()"]);
    }

    #[test]
    fn java_chains_concatenation_and_text_blocks() {
        let src = "class C {\n  void m() {\n    webClient.post().uri(\"/users/{id}\", id).retrieve().bodyToMono(String.class);\n    this.restTemplate.exchange(BASE + \"/x/\" + id, HttpMethod.POST, null, String.class);\n    list.forEach(x -> send(x));\n    rest.getForObject(\"\"\"\n        /a\\tb\n        \"\"\", String.class);\n  }\n}\n";
        assert_eq!(
            outline(Language::Java, src),
            [
                "webClient.post().uri('/users/{id}', id).retrieve().bodyToMono(?)",
                "this.restTemplate.exchange('{BASE}/x/{id}', HttpMethod.POST, ?, ?)",
                "list.forEach(fn)",
                "send(x)",
                "rest.getForObject('/a\tb\n', ?)",
            ]
        );
        let all = chains(Language::Java, src);
        let Arg::Template { template } = &all[1].links[0].args[0] else {
            panic!()
        };
        assert_eq!(
            template.parts[2],
            TemplatePart::Hole {
                expr: "id".into(),
                span: within(src, "+ id,", "id")
            }
        );
    }

    /// Contract M4a amendment 12 (evaluator pass 2, Q1): what can set a client's base is in the
    /// facts. An object literal keeps its spreads (key `...`), in order; a Java object creation
    /// argument keeps its type and arguments; a call assigned to a name keeps the name.
    #[test]
    fn spreads_object_creations_and_assignment_targets() {
        let ts = "axios.get(u, { ...defaults, baseURL: B, ...more });\nthis.api = axios.create({ baseURL: B });\nx = f(1);\nconst y = g();\n";
        assert_eq!(
            outline(Language::TypeScript, ts),
            [
                "axios.get(u, {...: defaults, baseURL: B, ...: more})",
                "axios.create({baseURL: B})",
                "f(?)",
                "g()",
            ]
        );
        let assigned: Vec<Option<String>> = chains(Language::TypeScript, ts)
            .into_iter()
            .map(|c| c.assigned)
            .collect();
        assert_eq!(
            assigned,
            [None, Some("this.api".into()), Some("x".into()), None]
        );
        let java = "class C {\n  RestTemplate rt;\n  WebClient web;\n  C(RestTemplateBuilder b) {\n    this.rt = b.rootUri(\"http://a\").build();\n    web = (WebClient.create(\"http://w\"));\n    rt.setUriTemplateHandler(new org.springframework.web.util.DefaultUriBuilderFactory(\"http://h\"));\n    rt.setUriTemplateHandler(new Factory<X>(base, new Inner(\"deep\")));\n  }\n}\n";
        assert_eq!(
            outline(Language::Java, java),
            [
                "b.rootUri('http://a').build()",
                "WebClient.create('http://w')",
                "rt.setUriTemplateHandler(new org.springframework.web.util.DefaultUriBuilderFactory('http://h'))",
                "rt.setUriTemplateHandler(new Factory(base, ?))",
            ]
        );
        let assigned: Vec<Option<String>> = chains(Language::Java, java)
            .into_iter()
            .map(|c| c.assigned)
            .collect();
        assert_eq!(
            assigned,
            [Some("this.rt".into()), Some("web".into()), None, None]
        );
    }

    #[test]
    fn go_chains_groups_and_sprintf() {
        let src = "package main\nfunc main() {\n\tr.Group(\"/v1\").POST(\"/x/:id\", h.Pay)\n\thttp.Post(fmt.Sprintf(\"%s/internal/%d\", base, n), \"application/json\", nil)\n\tgo func() { notify() }()\n}\n";
        assert_eq!(
            outline(Language::Go, src),
            [
                "r.Group('/v1').POST('/x/:id', h.Pay)",
                "http.Post(?, 'application/json', ?)",
                "fmt.Sprintf('%s/internal/%d', base, n)",
                "()",
                "notify()",
            ]
        );
    }

    #[test]
    fn python_smoke() {
        let src = "def f():\n    requests.get(f\"{BASE}/users\", timeout=3).json()\n";
        assert_eq!(
            outline(Language::Python, src),
            ["requests.get('{BASE}/users', ?).json()"]
        );
    }

    #[test]
    fn long_expressions_are_capped() {
        let long = "a".repeat(400);
        let src = format!("{long}.b.c(`${{{long}}}`);\n");
        let all = chains(Language::JavaScript, &src);
        assert_eq!(
            all[0].root.as_deref().map(str::len),
            Some(crate::MAX_EXPR_BYTES)
        );
        let Arg::Template { template } = &all[0].links[0].args[0] else {
            panic!()
        };
        let TemplatePart::Hole { expr, .. } = &template.parts[0] else {
            panic!()
        };
        assert_eq!(expr.len(), crate::MAX_EXPR_BYTES);
        // Cut on a character boundary.
        assert_eq!(super::capped("é".repeat(200)).len(), 256);
        assert_eq!(super::capped("€".repeat(100)).len(), 255);
    }
}
