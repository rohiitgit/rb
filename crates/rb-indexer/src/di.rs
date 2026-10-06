//! Spring DI resolution (contract M4b S4–S8, F2): injection points to the beans Spring would
//! inject, as `INJECTS` edges with provenance `di`.
//!
//! Every `INJECTS` edge is `heuristic` (one candidate, or each candidate of a collection) or
//! `ambiguous` (2–16 candidates): never `synthetic` or `exact` (orchestrator decision on F2).
//! The narrowing outcome is the edge's `resolution`; the gates that would have prevented a
//! plain resolution are its `capped_by`. Nothing here adds an edge a gate could remove: a gate
//! only moves a point from bucket `plain` to `capped`.

use std::collections::{BTreeMap, BTreeSet};

use rb_frameworks::clients::RawScan;
use rb_frameworks::di::{
    self as spring, Bean, FileDi, Form, InjectionPoint, Origin, ScanKind, Wrapper,
};
use rb_frameworks::work;
use rb_model::{
    Certainty, Diagnostic, EdgeKind, Evidence, GraphBuilder, NewEdge, NodeKey, NodeKind,
    Provenance, Span,
};
use rb_syntax::{FileFacts, Language, MentionRole};
use serde_json::{Value, json};

use crate::resolve::Index;

/// Supertype levels followed for assignability (invariant 8, as M3a).
pub(crate) const MAX_SUPERTYPE_DEPTH: usize = 64;

/// More candidates than this give no edge (`di_too_ambiguous`).
pub(crate) const MAX_CANDIDATES: usize = 16;

type Sym<'a> = (&'a str, usize);

pub(crate) use rb_model::is_test_path;

/// What DI resolution reads.
pub(crate) struct Inputs<'a> {
    pub files: &'a BTreeMap<&'a str, (Language, &'a FileFacts)>,
    pub keys: &'a BTreeMap<&'a str, Vec<String>>,
    pub index: &'a Index<'a>,
    /// A file's service directory.
    pub service_of: &'a dyn Fn(&str) -> String,
    /// The deployable services (`role = service`): never on another service's classpath.
    pub deployable: &'a BTreeSet<String>,
    /// Service → the services its manifests depend on (`via=package`, S16).
    pub depends: &'a BTreeMap<String, BTreeSet<String>>,
    /// The raw-text scan of the files without facts (gate D's findings, gate M's words).
    pub unparsed: &'a RawScan,
}

/// The coverage buckets (S7, amendment 2), in the order `rb status` prints them.
pub(crate) const BUCKETS: &[&str] = &[
    "plain",
    "capped",
    "ambiguous",
    "too_ambiguous",
    "external",
    "unresolved",
    "optional_absent",
    "not_a_bean",
    "test",
];

/// One resolved injection: an edge from the point's owner to a bean.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Injection {
    pub src: String,
    pub dst: String,
    pub certainty: Certainty,
    pub attrs: serde_json::Map<String, Value>,
    pub at: Evidence,
}

/// The result of DI resolution over a snapshot.
#[derive(Debug, Default)]
pub(crate) struct Resolution {
    pub injections: Vec<Injection>,
    /// Bucket → points (every bucket present).
    pub buckets: BTreeMap<&'static str, usize>,
    /// Gate → points it capped.
    pub gates: BTreeMap<&'static str, usize>,
    pub diagnostics: Vec<Diagnostic>,
    /// The interfaces Spring Data repositories extend, by simple name, in every Java file
    /// (fragment interfaces for RTA's rule (h), D3).
    pub fragments: BTreeSet<String>,
    /// The main-source injection points and the beans they resolve to uniquely, for the bases
    /// of injected HTTP clients (contract M4b S21).
    pub points: Points,
}

/// An injection point by name, injected type and span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PointRef {
    pub name: String,
    pub element: String,
    pub span: Span,
}

/// Injection points as client bases need them (contract M4b S21).
#[derive(Debug, Default)]
pub(crate) struct Points {
    /// Per `(path, owner)`: every main-source point.
    pub by_owner: BTreeMap<(String, usize), Vec<PointRef>>,
    /// Per `(path, point span)`: the one bean a point resolved to, as `(path, definition)`.
    pub unique: BTreeMap<(String, Span), (String, usize)>,
}

/// A bean with where it is.
struct BeanAt<'a, 'b> {
    path: &'a str,
    bean: &'b Bean,
    service: String,
    package: &'a str,
    /// Types it is assignable to (itself and its resolved supertypes, transitively).
    types: BTreeSet<Sym<'a>>,
    /// A `@Bean` method whose return type did not resolve: assignable through the types it
    /// creates (gate `bean_type_inferred`).
    inferred: bool,
}

/// The scan roots of one service (D8, S8).
#[derive(Default)]
struct Roots {
    /// `None`: unknown (no root class, several, or an unread attribute).
    components: Option<Vec<String>>,
    repositories: Option<Vec<String>>,
    /// `None`: Feign not enabled; `Some(None)`: enabled with unread roots.
    feign: Option<Option<Vec<String>>>,
    unread: bool,
}

fn under(package: &str, roots: &[String]) -> bool {
    roots.iter().any(|r| {
        r.is_empty()
            || package == r
            || package
                .strip_prefix(r.as_str())
                .is_some_and(|rest| rest.starts_with('.'))
    })
}

fn simple(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// The types `start` is assignable to: itself and its supertypes, breadth-first with a visited
/// set, at most [`MAX_SUPERTYPE_DEPTH`] levels (deeper ones are not assignable: conservative).
pub(crate) fn assignable<'a>(index: &Index<'a>, start: &[Sym<'a>]) -> BTreeSet<Sym<'a>> {
    let mut seen: BTreeSet<Sym<'a>> = start.iter().copied().collect();
    let mut level: Vec<Sym<'a>> = start.to_vec();
    for _ in 0..MAX_SUPERTYPE_DEPTH {
        let next: Vec<Sym<'a>> = level
            .iter()
            .flat_map(|&t| {
                let s = index.direct_supertypes(t);
                work::tick(s.len() + 1);
                s
            })
            .filter(|s| seen.insert(*s))
            .collect();
        if next.is_empty() {
            break;
        }
        level = next;
    }
    seen
}

/// Gate M's allowed roles (S6).
const MENTION_ALLOWED: &[MentionRole] = &[
    MentionRole::Declaration,
    MentionRole::Import,
    MentionRole::Supertype,
    MentionRole::Type,
    MentionRole::New,
    MentionRole::Member,
    MentionRole::Annotation,
    MentionRole::Logger,
];

/// Resolves every injection point of the snapshot (S4–S8).
pub(crate) fn resolve<'a>(inputs: &Inputs<'a>) -> Resolution {
    let index = inputs.index;
    let java: Vec<(&'a str, &'a FileFacts)> = inputs
        .files
        .iter()
        .filter(|(_, (language, _))| *language == Language::Java)
        .map(|(path, (_, facts))| (*path, *facts))
        .collect();
    let bases = rb_frameworks::repository_bases(java.iter().flat_map(|(_, f)| &f.definitions));
    let per_file: Vec<(&'a str, &'a FileFacts, FileDi)> = java
        .iter()
        .map(|&(path, facts)| {
            work::tick(1);
            (path, facts, spring::di(Language::Java, facts, &bases))
        })
        .collect();

    // Gate D (S6; amendment 3: main sources only).
    let surface = per_file
        .iter()
        .any(|(path, _, d)| !is_test_path(path) && !d.surface.is_empty())
        || inputs
            .unparsed
            .di_surface
            .iter()
            .any(|(path, _)| !is_test_path(path));
    // Gate M (S6): names with a mention outside the allowed roles in any parsed Java file, or
    // in an unparsed non-inert file.
    let mut mentioned: BTreeSet<&str> = BTreeSet::new();
    for (_, facts, _) in &per_file {
        for (name, roles) in &facts.mentions {
            work::tick(1);
            if roles.iter().any(|r| !MENTION_ALLOWED.contains(r)) {
                mentioned.insert(name.as_str());
            }
        }
    }
    let gate_m = |name: &str| {
        inputs.unparsed.words_unread
            || mentioned.contains(name)
            || inputs.unparsed.words.contains(name)
    };

    // Roots per service, from main sources.
    let mut roots: BTreeMap<String, Roots> = BTreeMap::new();
    {
        let mut decls: BTreeMap<String, Vec<(&str, &spring::ScanDecl)>> = BTreeMap::new();
        for (path, _, d) in per_file.iter().filter(|(p, _, _)| !is_test_path(p)) {
            for s in &d.scans {
                decls
                    .entry((inputs.service_of)(path))
                    .or_default()
                    .push((path, s));
            }
        }
        for (service, decls) in decls {
            let of = |kind: ScanKind| -> Vec<(&str, &spring::ScanDecl)> {
                decls
                    .iter()
                    .filter(|(_, s)| s.kind == kind)
                    .copied()
                    .collect()
            };
            let known = |ds: &[(&str, &spring::ScanDecl)]| -> Option<Vec<String>> {
                (!ds.iter().any(|(_, s)| s.unread))
                    .then(|| ds.iter().flat_map(|(_, s)| s.packages.clone()).collect())
            };
            let components = of(ScanKind::Components);
            let classes: BTreeSet<(&str, usize)> =
                components.iter().map(|(p, s)| (*p, s.class)).collect();
            let r = roots.entry(service).or_default();
            r.unread = decls.iter().any(|(_, s)| s.unread);
            r.components = (classes.len() == 1).then(|| known(&components)).flatten();
            let repositories = of(ScanKind::Repositories);
            r.repositories = if repositories.is_empty() {
                r.components.clone()
            } else {
                known(&repositories)
            };
            let feign = of(ScanKind::Feign);
            r.feign = (!feign.is_empty()).then(|| known(&feign));
        }
    }

    // Beans and unread classes from main sources.
    let mut beans: Vec<BeanAt<'a, '_>> = Vec::new();
    let mut unread: Vec<(String, BTreeSet<Sym<'a>>)> = Vec::new();
    for (path, facts, d) in per_file.iter().filter(|(p, _, _)| !is_test_path(p)) {
        let (path, facts): (&'a str, &'a FileFacts) = (*path, *facts);
        let service = (inputs.service_of)(path);
        let package = facts.package.as_deref().unwrap_or_default();
        for bean in &d.beans {
            let def = &facts.definitions[bean.definition];
            let (start, inferred) = match bean.origin {
                Origin::BeanMethod => {
                    let found = index.type_candidates(path, def.parent, &bean.type_name);
                    if found.is_empty() {
                        let created: Vec<Sym<'_>> = bean
                            .created
                            .iter()
                            .flat_map(|c| index.type_candidates(path, Some(bean.definition), c))
                            .collect();
                        (created, true)
                    } else {
                        (found, false)
                    }
                }
                _ => (vec![(path, bean.definition)], false),
            };
            beans.push(BeanAt {
                path,
                bean,
                service: service.clone(),
                package,
                types: assignable(index, &start),
                inferred,
            });
        }
        for &c in &d.unread_classes {
            unread.push((service.clone(), assignable(index, &[(path, c)])));
        }
    }
    let mut by_type: BTreeMap<Sym<'a>, Vec<usize>> = BTreeMap::new();
    for (i, b) in beans.iter().enumerate() {
        for &t in &b.types {
            work::tick(1);
            by_type.entry(t).or_default().push(i);
        }
    }
    // `@Bean` methods by their return type's simple name, for external injected types (S4).
    let mut by_return: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, b) in beans.iter().enumerate() {
        if b.bean.origin == Origin::BeanMethod {
            by_return
                .entry(simple(&b.bean.type_name))
                .or_default()
                .push(i);
        }
    }
    let mut unread_by_type: BTreeMap<Sym<'a>, Vec<&str>> = BTreeMap::new();
    for (service, types) in &unread {
        for &t in types {
            unread_by_type.entry(t).or_default().push(service.as_str());
        }
    }

    let mut out = Resolution {
        fragments: per_file
            .iter()
            .flat_map(|(_, _, d)| &d.beans)
            .filter(|b| b.origin == Origin::SpringData)
            .flat_map(|b| b.extends.iter().cloned())
            .collect(),
        ..Resolution::default()
    };
    for b in BUCKETS {
        out.buckets.insert(b, 0);
    }
    let mut classpaths: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (path, facts, d) in &per_file {
        let test = is_test_path(path);
        let service = (inputs.service_of)(path);
        let classpath = classpaths
            .entry(service.clone())
            .or_insert_with(|| {
                crate::packages::classpath(&service, inputs.depends)
                    .into_iter()
                    .filter(|s| *s == service || !inputs.deployable.contains(s))
                    .collect()
            })
            .clone();
        let service_roots = roots.get(&service);
        for &(class, span) in &d.ambiguous_constructors {
            if test {
                *out.buckets.get_mut("test").unwrap() += 1;
                continue;
            }
            *out.buckets.get_mut("unresolved").unwrap() += 1;
            out.diagnostics.push(site_diagnostic(
                "di_unresolved",
                path,
                span,
                &facts.definitions[class].name,
                "constructor_ambiguous",
            ));
        }
        for p in &d.points {
            work::tick(1);
            let ctx = PointContext {
                inputs,
                path,
                facts,
                file_di: d,
                classpath: &classpath,
                roots: service_roots,
                beans: work::Counted::new(&beans),
                by_type: &by_type,
                by_return: &by_return,
                unread_by_type: &unread_by_type,
                surface,
                gate_m: &gate_m,
            };
            if test {
                *out.buckets.get_mut("test").unwrap() += 1;
                continue;
            }
            out.points
                .by_owner
                .entry(((*path).to_owned(), p.owner))
                .or_default()
                .push(PointRef {
                    name: p.name.clone(),
                    element: p.element.clone(),
                    span: p.span,
                });
            resolve_point(&ctx, p, &mut out);
        }
    }
    out
}

struct PointContext<'c, 'a> {
    inputs: &'c Inputs<'a>,
    path: &'a str,
    facts: &'a FileFacts,
    file_di: &'c FileDi,
    classpath: &'c BTreeSet<String>,
    roots: Option<&'c Roots>,
    /// Counted, so any scan of the beans per point shows in the growth test (M4a amendment 13).
    beans: work::Counted<'c, BeanAt<'a, 'c>>,
    by_type: &'c BTreeMap<Sym<'a>, Vec<usize>>,
    by_return: &'c BTreeMap<&'c str, Vec<usize>>,
    unread_by_type: &'c BTreeMap<Sym<'a>, Vec<&'c str>>,
    surface: bool,
    gate_m: &'c dyn Fn(&str) -> bool,
}

fn site_diagnostic(kind: &str, path: &str, span: Span, ty: &str, detail: &str) -> Diagnostic {
    Diagnostic {
        kind: kind.to_owned(),
        path: Some(path.to_owned()),
        span: Some(span),
        detail: json!({ "type": ty, "detail": detail }),
    }
}

/// The bean's class or interface name (gate M), or a `@Bean` method's return type.
fn bean_name_for_gate(b: &BeanAt<'_, '_>) -> String {
    simple(&b.bean.type_name).to_owned()
}

fn resolve_point(ctx: &PointContext<'_, '_>, p: &InjectionPoint, out: &mut Resolution) {
    let inputs = ctx.inputs;
    let index = inputs.index;
    let defs = &ctx.facts.definitions;
    let bucket = |out: &mut Resolution, b: &'static str| *out.buckets.get_mut(b).unwrap() += 1;
    if !p.owner_is_bean {
        bucket(out, "not_a_bean");
        out.diagnostics.push(site_diagnostic(
            "di_not_a_bean",
            ctx.path,
            p.span,
            &p.element,
            "no_stereotype",
        ));
        return;
    }
    let roots = ctx.roots;
    let components = roots.and_then(|r| r.components.as_ref());
    // The consumer must itself be scanned.
    let consumer_class = match p.form {
        Form::BeanParameter => defs[p.owner].parent,
        _ => Some(p.owner),
    };
    let package = ctx.facts.package.as_deref().unwrap_or_default();
    if let Some(roots) = components
        && !under(package, roots)
    {
        bucket(out, "not_a_bean");
        out.diagnostics.push(site_diagnostic(
            "di_not_a_bean",
            ctx.path,
            p.span,
            &p.element,
            "outside_scan_roots",
        ));
        return;
    }
    let consumer_conditional = ctx.file_di.beans.iter().any(|b| {
        (Some(b.definition) == consumer_class || b.definition == p.owner) && b.conditional
    });

    let mut gates: BTreeSet<&'static str> = BTreeSet::new();
    if ctx.surface {
        gates.insert("surface");
    }
    if p.generic {
        gates.insert("generic");
    }
    if !p.annotations.is_empty() {
        gates.insert("annotation_unread");
    }
    if components.is_none() {
        gates.insert("scan_roots_unknown");
    }
    if roots.is_some_and(|r| r.unread) {
        gates.insert("scan_unread");
    }

    // Eligible beans for this service (classpath, roots).
    let eligible = |b: &BeanAt<'_, '_>| -> bool {
        if !ctx.classpath.contains(&b.service) {
            return false;
        }
        match b.bean.origin {
            Origin::Stereotype | Origin::BeanMethod => {
                components.is_none_or(|r| under(b.package, r))
            }
            Origin::SpringData => roots
                .and_then(|r| r.repositories.as_ref())
                .is_none_or(|r| under(b.package, r)),
            Origin::Feign => match roots.and_then(|r| r.feign.as_ref()) {
                None => false,
                Some(None) => true,
                Some(Some(r)) => under(b.package, r),
            },
        }
    };

    let types = index.type_candidates(ctx.path, Some(p.owner), &p.element);
    let mut candidates: Vec<usize>;
    if types.is_empty() {
        // An external type (or one not in the snapshot): a repository `@Bean` method returning
        // it by name is the candidate, capped (S4).
        candidates = ctx
            .by_return
            .get(simple(&p.element))
            .into_iter()
            .flatten()
            .copied()
            .filter(|&i| {
                work::tick(1);
                eligible(&ctx.beans[i])
            })
            .collect();
        if candidates.is_empty() {
            bucket(out, "external");
            return;
        }
        gates.insert("external_type");
    } else {
        let mut set: BTreeSet<usize> = BTreeSet::new();
        for t in &types {
            for &i in ctx.by_type.get(t).into_iter().flatten() {
                work::tick(1);
                let b = &ctx.beans[i];
                if eligible(b) {
                    set.insert(i);
                } else if !ctx.classpath.contains(&b.service)
                    && !ctx.inputs.deployable.contains(&b.service)
                {
                    gates.insert("classpath_unknown");
                }
            }
            for service in ctx.unread_by_type.get(t).into_iter().flatten() {
                if ctx.classpath.contains(*service) {
                    gates.insert("annotation_unread");
                }
            }
        }
        candidates = set.into_iter().collect();
    }
    // Deterministic order: by target key.
    let key = |i: usize| -> &str {
        let b = &ctx.beans[i];
        &inputs.keys[b.path][b.bean.definition]
    };
    candidates.sort_by(|a, b| key(*a).cmp(key(*b)));
    candidates.dedup();
    for &i in &candidates {
        let b = &ctx.beans[i];
        // Matched through the types its body creates (not by name as an external type).
        if b.inferred && !types.is_empty() {
            gates.insert("bean_type_inferred");
        }
        if !b.bean.annotations.is_empty() {
            gates.insert("annotation_unread");
        }
    }
    if (ctx.gate_m)(simple(&p.element))
        || candidates
            .iter()
            .any(|&i| (ctx.gate_m)(&bean_name_for_gate(&ctx.beans[i])))
    {
        gates.insert("mention");
    }

    // Narrowing (D9).
    let named = |i: usize, name: &str| ctx.beans[i].bean.names.iter().any(|n| n == name);
    let mut chosen = candidates.clone();
    let mut resolution = "unique";
    let mut qualifier = None;
    let mut primary = false;
    let mut by_name = false;
    if let Some(name) = &p.by_name {
        let hits: Vec<usize> = chosen.iter().copied().filter(|&i| named(i, name)).collect();
        if hits.len() == 1 {
            chosen = hits;
            resolution = "name";
        }
        by_name = true;
    }
    let collection = p.wrapper == Some(Wrapper::Collection);
    if let Some(q) = &p.qualifier {
        chosen.retain(|&i| named(i, q) || ctx.beans[i].bean.qualifiers.iter().any(|x| x == q));
        qualifier = Some(q.clone());
        resolution = "qualifier";
        if chosen.is_empty() {
            bucket(out, "unresolved");
            out.diagnostics.push(site_diagnostic(
                "di_unresolved",
                ctx.path,
                p.span,
                &p.element,
                "qualifier",
            ));
            return;
        }
    }
    if !collection && chosen.len() > 1 {
        let primaries: Vec<usize> = chosen
            .iter()
            .copied()
            .filter(|&i| ctx.beans[i].bean.primary)
            .collect();
        if primaries.len() == 1 {
            chosen = primaries;
            primary = true;
            resolution = "primary";
        }
    }
    if !collection && chosen.len() > 1 {
        let hits: Vec<usize> = chosen
            .iter()
            .copied()
            .filter(|&i| named(i, &p.name))
            .collect();
        if hits.len() == 1 {
            chosen = hits;
            by_name = true;
            resolution = "name";
        }
    }
    if by_name {
        gates.insert("by_name");
    }
    if chosen.is_empty() {
        if p.wrapper == Some(Wrapper::Optional) {
            bucket(out, "optional_absent");
        } else {
            bucket(out, "unresolved");
            out.diagnostics.push(site_diagnostic(
                "di_unresolved",
                ctx.path,
                p.span,
                &p.element,
                "no_bean",
            ));
        }
        return;
    }
    if collection {
        resolution = "collection";
    } else if chosen.len() > 1 {
        resolution = "ambiguous";
    }
    if !collection && chosen.len() > MAX_CANDIDATES {
        bucket(out, "too_ambiguous");
        out.diagnostics.push(Diagnostic {
            kind: "di_too_ambiguous".into(),
            path: Some(ctx.path.to_owned()),
            span: Some(p.span),
            detail: json!({ "type": p.element, "candidates": chosen.len() }),
        });
        return;
    }
    // Conditionals among the consumer, the candidates and those narrowing removed.
    let conditional =
        consumer_conditional || candidates.iter().any(|&i| ctx.beans[i].bean.conditional);
    let several = !collection && chosen.len() > 1;
    let certainty = if several {
        Certainty::Ambiguous
    } else {
        Certainty::Heuristic
    };
    let label = if several {
        "ambiguous"
    } else if gates.is_empty() && !conditional {
        "plain"
    } else {
        "capped"
    };
    bucket(out, label);
    for g in &gates {
        *out.gates.entry(g).or_default() += 1;
    }
    // A wrapper with one candidate holds that bean too (`rt[0]` of a one-bean array).
    if let [i] = chosen[..] {
        let b = &ctx.beans[i];
        out.points.unique.insert(
            (ctx.path.to_owned(), p.span),
            (b.path.to_owned(), b.bean.definition),
        );
    }
    let src = inputs.keys[ctx.path][p.owner].clone();
    for &i in &chosen {
        let b = &ctx.beans[i];
        let mut attrs = serde_json::Map::new();
        attrs.insert("origin".into(), b.bean.origin.as_str().into());
        attrs.insert("via".into(), p.form.as_str().into());
        attrs.insert("resolution".into(), resolution.into());
        if let Some(q) = &qualifier {
            attrs.insert("qualifier".into(), q.as_str().into());
        }
        if primary {
            attrs.insert("primary".into(), true.into());
        }
        if by_name {
            attrs.insert("by_name".into(), true.into());
        }
        if collection {
            attrs.insert("collection".into(), true.into());
        }
        if conditional {
            attrs.insert("conditional".into(), true.into());
        }
        if several {
            attrs.insert("candidates".into(), chosen.len().into());
        }
        if !gates.is_empty() {
            attrs.insert(
                "capped_by".into(),
                Value::Array(gates.iter().map(|g| Value::from(*g)).collect()),
            );
        }
        out.injections.push(Injection {
            src: src.clone(),
            dst: inputs.keys[b.path][b.bean.definition].clone(),
            certainty,
            attrs,
            at: Evidence {
                path: ctx.path.to_owned(),
                span: p.span,
            },
        });
    }
}

/// Adds the `INJECTS` edges (one per `(owner, bean)`: the best site's certainty and attributes,
/// every site as evidence, `sites = n`; invariant 3), the `di_*` diagnostics and one
/// `di_coverage`.
pub(crate) fn add_injects(b: &mut GraphBuilder, r: Resolution) -> anyhow::Result<Vec<Diagnostic>> {
    let mut merged: BTreeMap<(String, String), Vec<Injection>> = BTreeMap::new();
    for i in r.injections {
        merged
            .entry((i.src.clone(), i.dst.clone()))
            .or_default()
            .push(i);
    }
    for ((src, dst), mut sites) in merged {
        sites.sort_by(|a, b| b.certainty.cmp(&a.certainty).then_with(|| a.at.cmp(&b.at)));
        let best = &sites[0];
        let mut attrs = best.attrs.clone();
        attrs.insert("sites".into(), sites.len().into());
        let mut edge = NewEdge::new(
            NodeKey::new(NodeKind::Symbol, src),
            EdgeKind::Injects,
            NodeKey::new(NodeKind::Symbol, dst),
            Provenance::Di,
            best.certainty,
        )
        .attrs(attrs);
        for s in &sites {
            edge = edge.evidence(s.at.clone());
        }
        b.add_edge(edge)?;
    }
    let mut diagnostics = r.diagnostics;
    let points: usize = r.buckets.values().sum();
    diagnostics.push(Diagnostic {
        kind: "di_coverage".into(),
        path: None,
        span: None,
        detail: json!({
            "points": points,
            "buckets": r.buckets,
            "gates": r.gates,
        }),
    });
    Ok(diagnostics)
}

/// Service → the services it depends on by `via=package` (S16), for classpaths.
pub(crate) fn package_dependencies(
    edges: &[crate::packages::ManifestEdge],
) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for e in edges {
        if let crate::packages::Target::Service(t) = &e.target {
            out.entry(e.service.clone()).or_default().insert(t.clone());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One resolved snapshot: `src -> dst [certainty] {attrs}` per injection (keys shortened to
    /// the descriptor), the buckets and the gates.
    struct Run {
        edges: Vec<String>,
        buckets: BTreeMap<&'static str, usize>,
        gates: BTreeMap<&'static str, usize>,
        diagnostics: Vec<String>,
    }

    fn run_with(files: &[(&str, &str)], services: &[(&str, &str, bool)], unparsed: RawScan) -> Run {
        let facts: Vec<(String, FileFacts)> = files
            .iter()
            .map(|(p, src)| {
                (
                    (*p).to_owned(),
                    rb_syntax::extract(Language::Java, src.as_bytes()).unwrap(),
                )
            })
            .collect();
        let by_path: BTreeMap<&str, (Language, &FileFacts)> = facts
            .iter()
            .map(|(p, f)| (p.as_str(), (Language::Java, f)))
            .collect();
        let keys: BTreeMap<&str, Vec<String>> = by_path
            .iter()
            .map(|(p, (_, f))| (*p, rb_syntax::symbol_keys(p, f)))
            .collect();
        let go = BTreeMap::new();
        let index = Index::new(&by_path, &go, &keys);
        let service_of = |path: &str| {
            services
                .iter()
                .filter(|(dir, _, _)| path.starts_with(&format!("{dir}/")))
                .max_by_key(|(dir, _, _)| dir.len())
                .map_or_else(|| ".".to_owned(), |(dir, _, _)| (*dir).to_owned())
        };
        let deployable: BTreeSet<String> = services
            .iter()
            .filter(|(_, _, d)| *d)
            .map(|(dir, _, _)| (*dir).to_owned())
            .collect();
        let mut depends: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (dir, deps, _) in services {
            for d in deps.split(',').filter(|d| !d.is_empty()) {
                depends
                    .entry((*dir).to_owned())
                    .or_default()
                    .insert(d.to_owned());
            }
        }
        let r = resolve(&Inputs {
            files: &by_path,
            keys: &keys,
            index: &index,
            service_of: &service_of,
            deployable: &deployable,
            depends: &depends,
            unparsed: &unparsed,
        });
        let short = |k: &str| {
            k.rsplit_once("`/")
                .map_or(k.to_owned(), |(_, d)| d.to_owned())
        };
        // Through the builder, so the per-edge merge (invariant 3) is what is asserted.
        let mut b = GraphBuilder::new();
        for (path, (_, f)) in &by_path {
            b.add_node(
                NodeKey::new(NodeKind::File, *path),
                *path,
                Default::default(),
            )
            .unwrap();
            crate::syntax::add_symbols(&mut b, path, f, &keys[path]).unwrap();
        }
        let (buckets, gates) = (r.buckets.clone(), r.gates.clone());
        let diagnostics = add_injects(&mut b, r).unwrap();
        let g = b.finish().unwrap();
        let key = |id: i64| g.nodes().iter().find(|n| n.id == id).unwrap().key.clone();
        let mut edges: Vec<String> = g
            .edges()
            .iter()
            .filter(|e| e.kind == EdgeKind::Injects)
            .map(|e| {
                assert_eq!(e.provenance, Provenance::Di);
                assert!(
                    e.certainty <= Certainty::Heuristic,
                    "never above heuristic (F2)"
                );
                format!(
                    "{} -> {} [{}] {}",
                    short(&key(e.src)),
                    short(&key(e.dst)),
                    e.certainty.as_str(),
                    Value::Object(e.attrs.clone())
                )
            })
            .collect();
        edges.sort();
        let r = Resolution {
            buckets,
            gates,
            diagnostics,
            ..Resolution::default()
        };
        Run {
            edges,
            buckets: r.buckets.into_iter().filter(|(_, n)| *n > 0).collect(),
            gates: r.gates,
            diagnostics: r
                .diagnostics
                .iter()
                .map(|d| format!("{} {}", d.kind, d.detail))
                .collect(),
        }
    }

    fn run(files: &[(&str, &str)]) -> Run {
        run_with(files, &[], RawScan::default())
    }

    const APP: (&str, &str) = (
        "src/main/java/com/acme/App.java",
        "package com.acme;\nimport org.springframework.boot.autoconfigure.SpringBootApplication;\n@SpringBootApplication public class App {}\n",
    );
    const IMPORTS: &str = "package com.acme;\nimport org.springframework.stereotype.*;\nimport org.springframework.context.annotation.Bean;\nimport org.springframework.context.annotation.Configuration;\nimport org.springframework.context.annotation.Import;\nimport org.springframework.context.annotation.Primary;\nimport org.springframework.context.annotation.Profile;\nimport org.springframework.beans.factory.annotation.Autowired;\nimport org.springframework.beans.factory.annotation.Qualifier;\nimport java.util.*;\n";

    fn src(body: &str) -> String {
        format!("{IMPORTS}{body}")
    }

    fn one(body: &str) -> Run {
        let s = src(body);
        run(&[APP, ("src/main/java/com/acme/A.java", &s)])
    }

    /// S5 (F2): the plain row, then each gate alone lowering it to `capped`, each biting on
    /// its own input.
    #[test]
    fn the_plain_row_and_each_gate_that_caps_it() {
        let base = "interface Repo {}\n@Component class RepoImpl implements Repo {}\n@Service class Svc { Svc(Repo r) {} }\n";
        let plain = one(base);
        assert_eq!(
            plain.edges,
            [
                r#"Svc# -> RepoImpl# [heuristic] {"origin":"stereotype","resolution":"unique","sites":1,"via":"constructor"}"#
            ]
        );
        assert_eq!(plain.buckets, BTreeMap::from([("plain", 1)]));
        let gate = |r: &Run| -> Vec<&'static str> { r.gates.keys().copied().collect() };
        // Roots unknown: no root class.
        let s = src(base);
        let r = run(&[("src/main/java/com/acme/A.java", &s)]);
        assert_eq!(
            (gate(&r), r.buckets.get("capped")),
            (vec!["scan_roots_unknown"], Some(&1))
        );
        // Unread scan attribute.
        let app = "package com.acme;\nimport org.springframework.boot.autoconfigure.SpringBootApplication;\n@SpringBootApplication(nameGenerator = G.class) public class App {}\n";
        let r = run(&[
            ("src/main/java/com/acme/App.java", app),
            ("src/main/java/com/acme/A.java", &s),
        ]);
        assert_eq!(gate(&r), ["scan_roots_unknown", "scan_unread"]);
        // Generic injected type.
        let r = one(
            "interface Repo<T> {}\n@Component class RepoImpl implements Repo<String> {}\n@Service class Svc { Svc(Repo<String> r) {} }\n",
        );
        assert_eq!(gate(&r), ["generic"]);
        // An annotation outside the lists on the point, on a candidate, an unread class.
        let r = one(
            "interface Repo {}\n@Component class RepoImpl implements Repo {}\n@Service class Svc { Svc(@Custom Repo r) {} }\n",
        );
        assert_eq!(gate(&r), ["annotation_unread"]);
        let r = one(
            "interface Repo {}\n@Component @Custom class RepoImpl implements Repo {}\n@Service class Svc { Svc(Repo r) {} }\n",
        );
        assert_eq!(gate(&r), ["annotation_unread"]);
        let r = one(&format!(
            "{base}@MyStereotype class Other implements Repo {{}}\n"
        ));
        assert_eq!(gate(&r), ["annotation_unread"]);
        // A `@Bean` method whose return type is external: inferred from `new`.
        let r = one(
            "interface Repo {}\nclass RepoImpl implements Repo {}\n@Configuration class Cfg { @Bean Object repo() { return new RepoImpl(); } }\n@Service class Svc { Svc(Repo r) {} }\n",
        );
        assert_eq!(gate(&r), ["bean_type_inferred"]);
        // An external type named by a repository `@Bean`.
        let r = one(
            "import org.x.Clock;\n@Configuration class Cfg { @Bean Clock clock() { return null; } }\n@Service class Svc { Svc(Clock c) {} }\n",
        );
        assert_eq!(gate(&r), ["external_type"]);
        // Name fallback.
        let r = one(
            "interface Repo {}\n@Component class Main implements Repo {}\n@Component class Other implements Repo {}\n@Service class Svc { Svc(Repo main) {} }\n",
        );
        assert_eq!(gate(&r), ["by_name"]);
        assert!(
            r.edges[0].contains(r#""by_name":true"#)
                && r.edges[0].contains(r#""resolution":"name""#),
            "{:?}",
            r.edges
        );
        // Gate D: a surface finding in a main source, and in a file without facts.
        let r = one(&format!(
            "{base}class Pp implements org.springframework.beans.factory.config.BeanPostProcessor {{}}\n"
        ));
        assert_eq!(gate(&r), ["surface"]);
        let raw = RawScan {
            di_surface: vec![("src/main/kotlin/C.kt".into(), "raw Bean".into())],
            ..RawScan::default()
        };
        let r = run_with(&[APP, ("src/main/java/com/acme/A.java", &s)], &[], raw);
        assert_eq!(gate(&r), ["surface"]);
        // Gate D ignores test sources (amendment 3).
        let t = "package com.acme;\nimport org.springframework.context.annotation.FilterType;\nclass T {}\n";
        let r = run(&[
            APP,
            ("src/main/java/com/acme/A.java", &s),
            ("src/test/java/com/acme/T.java", t),
        ]);
        assert_eq!(r.buckets, BTreeMap::from([("plain", 1)]));
        // Gate M: a candidate in a disallowed role, the injected type in an unparsed file.
        let r = one(&format!("{base}class U {{ Object o = RepoImpl.class; }}\n"));
        assert_eq!(gate(&r), ["mention"]);
        let raw = RawScan {
            words: BTreeSet::from(["Repo".to_owned()]),
            ..RawScan::default()
        };
        let r = run_with(&[APP, ("src/main/java/com/acme/A.java", &s)], &[], raw);
        assert_eq!(gate(&r), ["mention"]);
        // Controls stay plain: a logger, `new` in a test, an unrelated `@Import`.
        let r = run(&[
            APP,
            (
                "src/main/java/com/acme/A.java",
                &src(&format!(
                    "{base}class L {{ Object l = LoggerFactory.getLogger(RepoImpl.class); }}\n@Import(App.class) class I {{}}\n"
                )),
            ),
            (
                "src/test/java/com/acme/ATest.java",
                "package com.acme;\nclass ATest { Object o = new RepoImpl(); }\n",
            ),
        ]);
        assert_eq!(r.buckets, BTreeMap::from([("plain", 1)]));
        // Classpath unknown: an assignable bean in a library outside the classpath.
        let lib = "package com.acme;\nimport org.springframework.stereotype.Component;\n@Component class LibRepo implements Repo {}\n";
        let r = run_with(
            &[
                ("svc/src/main/java/com/acme/App.java", APP.1),
                ("svc/src/main/java/com/acme/A.java", &s),
                ("lib/src/main/java/com/acme/L.java", lib),
            ],
            &[("svc", "", true), ("lib", "", false)],
            RawScan::default(),
        );
        assert_eq!(gate(&r), ["classpath_unknown"]);
        // On the classpath, it is a second candidate.
        let r = run_with(
            &[
                ("svc/src/main/java/com/acme/App.java", APP.1),
                ("svc/src/main/java/com/acme/A.java", &s),
                ("lib/src/main/java/com/acme/L.java", lib),
            ],
            &[("svc", "lib", true), ("lib", "", false)],
            RawScan::default(),
        );
        assert_eq!(r.buckets, BTreeMap::from([("ambiguous", 1)]));
        // A deployable service is never on another's classpath, nor a gate.
        let r = run_with(
            &[
                ("svc/src/main/java/com/acme/App.java", APP.1),
                ("svc/src/main/java/com/acme/A.java", &s),
                ("lib/src/main/java/com/acme/L.java", lib),
            ],
            &[("svc", "lib", true), ("lib", "", true)],
            RawScan::default(),
        );
        assert_eq!(r.buckets, BTreeMap::from([("plain", 1)]));
    }

    /// D9's narrowing, one row each, and the coverage buckets.
    #[test]
    fn narrowing_wrappers_and_buckets() {
        let hashers = "interface Hasher {}\n@Component @Primary class Bcrypt implements Hasher {}\n@Component class Plain implements Hasher {}\n";
        // `@Primary` wins; a qualifier beats `@Primary`; a qualifier naming no bean.
        let r = one(&format!(
            "{hashers}@Service class Svc {{ Svc(Hasher h, @Qualifier(\"plain\") Hasher q) {{}} @Autowired @Qualifier(\"none\") Hasher x; }}\n"
        ));
        assert_eq!(
            r.edges,
            [
                r#"Svc# -> Bcrypt# [heuristic] {"origin":"stereotype","primary":true,"resolution":"primary","sites":1,"via":"constructor"}"#,
                r#"Svc# -> Plain# [heuristic] {"origin":"stereotype","qualifier":"plain","resolution":"qualifier","sites":1,"via":"constructor"}"#,
            ]
        );
        assert_eq!(r.buckets, BTreeMap::from([("plain", 2), ("unresolved", 1)]));
        assert!(
            r.diagnostics.iter().any(|d| d.contains("\"qualifier\"")),
            "{:?}",
            r.diagnostics
        );
        // A qualifier shared by two beans, one `@Primary`: the primary among them.
        let r = one(
            "interface H {}\n@Component @Qualifier(\"q\") @Primary class P1 implements H {}\n@Component @Qualifier(\"q\") class P2 implements H {}\n@Service class Svc { Svc(@Qualifier(\"q\") H h) {} }\n",
        );
        assert_eq!(r.edges.len(), 1, "{:?}", r.edges);
        assert!(
            r.edges[0].contains("P1#") && r.edges[0].contains(r#""primary":true"#),
            "{:?}",
            r.edges
        );
        // Two candidates, nothing narrows: ambiguous, one edge each.
        let r = one(
            "interface H {}\n@Component class A1 implements H {}\n@Component class A2 implements H {}\n@Service class Svc { Svc(H h) {} }\n",
        );
        assert_eq!(r.buckets, BTreeMap::from([("ambiguous", 1)]));
        assert!(
            r.edges
                .iter()
                .all(|e| e.contains("[ambiguous]") && e.contains(r#""candidates":2"#)),
            "{:?}",
            r.edges
        );
        // Collections: every candidate; arrays and `Map<String, T>` too.
        let r = one(
            "interface N {}\n@Component class E implements N {}\n@Component class S implements N {}\n@Service class Svc { Svc(List<N> a, N[] b, Map<String, N> c) {} }\n",
        );
        assert_eq!(r.edges.len(), 2, "{:?}", r.edges);
        assert!(
            r.edges
                .iter()
                .all(|e| e.contains(r#""collection":true"#) && e.contains(r#""sites":3"#))
        );
        assert_eq!(r.buckets, BTreeMap::from([("plain", 3)]));
        // Optional with no bean; a repository type with no bean; an external type.
        let r = one(
            "interface Missing {}\nimport org.x.Ext;\n@Service class Svc { Svc(Optional<Missing> o, Missing m, Ext e) {} }\n",
        );
        assert_eq!(
            r.buckets,
            BTreeMap::from([("external", 1), ("optional_absent", 1), ("unresolved", 1)])
        );
        // Conditional candidates: ambiguous edges marked conditional.
        let r = one(
            "interface T {}\n@Component @Profile(\"prod\") class J implements T {}\n@Component @Profile(\"dev\") class D implements T {}\n@Service class Svc { @Autowired void set(T t) {} }\n",
        );
        assert!(
            r.edges
                .iter()
                .all(|e| e.contains(r#""conditional":true"#) && e.contains(r#""via":"setter""#)),
            "{:?}",
            r.edges
        );
        // A conditional the narrowing removed still makes the edge conditional (not plain).
        let r = one(
            "interface T {}\n@Component @Primary class J implements T {}\n@Component @Profile(\"dev\") class D implements T {}\n@Service class Svc { Svc(T t) {} }\n",
        );
        assert_eq!(r.buckets, BTreeMap::from([("capped", 1)]));
        // `@Resource` by name, a not-a-bean field, test sources, an ambiguous constructor.
        let r = run(&[
            APP,
            (
                "src/main/java/com/acme/A.java",
                &src(
                    "import jakarta.annotation.Resource;\ninterface T {}\n@Component class First implements T {}\n@Component class Second implements T {}\n@Service class Svc { @Resource(name = \"second\") T t; }\nclass Legacy { @Autowired T t; }\n@Service class Two { Two(T a) {} Two(String s) {} }\n",
                ),
            ),
            (
                "src/test/java/com/acme/ATest.java",
                &src("@Service class ATest { ATest(T t) {} }\n"),
            ),
        ]);
        assert_eq!(r.edges.len(), 1);
        assert!(
            r.edges[0].contains("Second#") && r.edges[0].contains(r#""resolution":"name""#),
            "{:?}",
            r.edges
        );
        assert_eq!(
            r.buckets,
            BTreeMap::from([
                ("capped", 1),
                ("not_a_bean", 1),
                ("test", 1),
                ("unresolved", 1)
            ])
        );
        // `@Bean` factories and their parameters; Spring Data repositories.
        let r = one(
            "interface Sink {}\nclass LogSink implements Sink {}\n@Configuration class Cfg { @Bean Sink sink(Clockish c) { return new LogSink(); } }\n@Component class Clockish {}\ninterface UserRepository extends org.springframework.data.jpa.repository.JpaRepository<User, Long> {}\n@Service class Svc { Svc(Sink s, UserRepository u) {} }\n",
        );
        assert_eq!(
            r.edges,
            [
                r#"Cfg#sink(). -> Clockish# [heuristic] {"origin":"stereotype","resolution":"unique","sites":1,"via":"bean_parameter"}"#,
                r#"Svc# -> Cfg#sink(). [heuristic] {"origin":"bean_method","resolution":"unique","sites":1,"via":"constructor"}"#,
                r#"Svc# -> UserRepository# [heuristic] {"origin":"spring_data","resolution":"unique","sites":1,"via":"constructor"}"#,
            ]
        );
    }

    /// S8: repository and Feign roots; a stereotype outside the scan roots.
    #[test]
    fn repository_feign_and_scan_roots() {
        let feign = "package com.acme.client;\nimport org.springframework.cloud.openfeign.FeignClient;\n@FeignClient(name = \"mfa\") public interface Mfa {}\n";
        let svc = src(
            "import com.acme.client.Mfa;\nimport com.acme.other.Far;\n@Service class Svc { Svc(Mfa m, Far f) {} }\n",
        );
        let far = "package com.acme.other;\nimport org.springframework.stereotype.Component;\n@Component public class Far {}\n";
        let app = "package com.acme;\nimport org.springframework.boot.autoconfigure.SpringBootApplication;\nimport org.springframework.cloud.openfeign.EnableFeignClients;\n@SpringBootApplication(scanBasePackages = \"com.acme\") @EnableFeignClients(basePackages = \"com.acme.client\") public class App {}\n";
        let files = |app: &'static str| {
            vec![
                ("src/main/java/com/acme/App.java", app),
                ("src/main/java/com/acme/client/Mfa.java", feign),
                ("src/main/java/com/acme/other/Far.java", far),
            ]
        };
        let mut f = files(app);
        f.push(("src/main/java/com/acme/Svc.java", &svc));
        let r = run(&f);
        assert_eq!(r.edges.len(), 2, "{:?}", r.edges);
        assert!(r.edges[1].contains(r#""origin":"feign""#), "{:?}", r.edges);
        // No `@EnableFeignClients`: the Feign interface is no candidate.
        let app2 = "package com.acme;\nimport org.springframework.boot.autoconfigure.SpringBootApplication;\n@SpringBootApplication public class App {}\n";
        let mut f = files(app2);
        f.push(("src/main/java/com/acme/Svc.java", &svc));
        let r = run(&f);
        assert_eq!(r.buckets, BTreeMap::from([("plain", 1), ("unresolved", 1)]));
        // Roots exclude `com.acme.other`.
        let app3 = "package com.acme.main;\nimport org.springframework.boot.autoconfigure.SpringBootApplication;\n@SpringBootApplication(scanBasePackages = {\"com.acme\", \"com.acme.client\"}) public class App {}\n";
        let narrow = "package com.acme.main;\nimport org.springframework.boot.autoconfigure.SpringBootApplication;\n@SpringBootApplication(scanBasePackages = \"com.acme.main\") public class App {}\n";
        let mut f = files(narrow);
        let svc_main = svc.replace("package com.acme;", "package com.acme.main;");
        f.push(("src/main/java/com/acme/main/Svc.java", &svc_main));
        let r = run(&f);
        assert_eq!(
            r.buckets,
            BTreeMap::from([("unresolved", 2)]),
            "{:?}",
            r.edges
        );
        let _ = app3;
        // A repository interface outside the repository roots is no candidate.
        let repo = "package com.acme.repo;\npublic interface Users extends org.springframework.data.repository.CrudRepository<U, Long> {}\n";
        let jpa = "package com.acme;\nimport org.springframework.boot.autoconfigure.SpringBootApplication;\nimport org.springframework.data.jpa.repository.config.EnableJpaRepositories;\n@SpringBootApplication @EnableJpaRepositories(\"com.acme.elsewhere\") public class App {}\n";
        let user = src("import com.acme.repo.Users;\n@Service class Svc { Svc(Users u) {} }\n");
        let r = run(&[
            ("src/main/java/com/acme/App.java", jpa),
            ("src/main/java/com/acme/repo/Users.java", repo),
            ("src/main/java/com/acme/Svc.java", &user),
        ]);
        assert_eq!(r.buckets, BTreeMap::from([("unresolved", 1)]));
    }

    /// S4: assignability at depth 1, depth 2 through a class and an interface, the reverse at
    /// each depth, two same-named types, a supertype cycle, and the depth cap.
    #[test]
    fn assignability_depths_cycles_and_same_names() {
        let target = |r: &Run| {
            r.edges
                .iter()
                .map(|e| {
                    e.split(" -> ")
                        .nth(1)
                        .unwrap()
                        .split(' ')
                        .next()
                        .unwrap()
                        .to_owned()
                })
                .collect::<Vec<_>>()
        };
        // Depth 2 through a class, and through an interface.
        let r = one(
            "interface T {}\nabstract class Base implements T {}\n@Component class Impl extends Base {}\n@Service class Svc { Svc(T t) {} }\n",
        );
        assert_eq!(target(&r), ["Impl#"]);
        let r = one(
            "interface T {}\ninterface Mid extends T {}\n@Component class Impl implements Mid {}\n@Service class Svc { Svc(T t) {} }\n",
        );
        assert_eq!(target(&r), ["Impl#"]);
        // Reverse: a bean of the supertype is not assignable to the subtype, at depth 1 and 2.
        let r = one(
            "interface T {}\ninterface Mid extends T {}\n@Component class Impl implements T {}\n@Service class Svc { Svc(Mid m) {} }\n",
        );
        assert_eq!(r.buckets, BTreeMap::from([("unresolved", 1)]));
        let r = one(
            "interface T {}\ninterface Mid extends T {}\ninterface Low extends Mid {}\n@Component class Impl implements Mid {}\n@Service class Svc { Svc(Low l) {} }\n",
        );
        assert_eq!(r.buckets, BTreeMap::from([("unresolved", 1)]));
        // Two types named `T`: only the imported one is the injected type.
        let r = run(&[
            APP,
            (
                "src/main/java/com/acme/a/T.java",
                "package com.acme.a;\npublic interface T {}\n",
            ),
            (
                "src/main/java/com/acme/b/T.java",
                "package com.acme.b;\npublic interface T {}\n",
            ),
            (
                "src/main/java/com/acme/Impls.java",
                &src(
                    "@Component class Wrong implements com.acme.b.T {}\n@Component class Right implements com.acme.a.T {}\n",
                ),
            ),
            (
                "src/main/java/com/acme/Svc.java",
                &src("import com.acme.a.T;\n@Service class Svc { Svc(T t) {} }\n"),
            ),
        ]);
        assert_eq!(target(&r), ["Right#"]);
        // A cycle terminates.
        let r = one(
            "interface A extends B {}\ninterface B extends A {}\n@Component class Impl implements A {}\n@Service class Svc { Svc(B b) {} }\n",
        );
        assert_eq!(target(&r), ["Impl#"]);
        // Depth: a chain longer than the cap is cut (not assignable: conservative).
        let deep = |n: usize| {
            let mut s = String::from("interface I0 {}\n");
            for i in 1..=n {
                s += &format!("interface I{i} extends I{} {{}}\n", i - 1);
            }
            s += &format!(
                "@Component class Impl implements I{n} {{}}\n@Service class Svc {{ Svc(I0 t) {{}} }}\n"
            );
            one(&s)
        };
        assert_eq!(deep(10).buckets, BTreeMap::from([("plain", 1)]));
        assert_eq!(
            deep(MAX_SUPERTYPE_DEPTH + 5).buckets,
            BTreeMap::from([("unresolved", 1)])
        );
    }

    /// Invariant 11: every injection point falls in exactly one bucket.
    #[test]
    fn buckets_sum_to_the_points() {
        let r = one(
            "interface H {}\n@Component class A1 implements H {}\n@Component class A2 implements H {}\ninterface M {}\nimport org.x.Ext;\n@Service class Svc { Svc(H h, @Qualifier(\"a1\") H q, M m, Optional<M> o, Ext e, List<H> l) {} }\nclass NotBean { @Autowired H h; }\n@Service class Two { Two(H a) {} Two(M b) {} }\n",
        );
        let total: usize = r.buckets.values().sum();
        // Six constructor points, one field, one ambiguous constructor.
        assert_eq!(total, 8, "{:?}", r.buckets);
        assert_eq!(
            r.buckets,
            BTreeMap::from([
                ("ambiguous", 1),
                ("external", 1),
                ("not_a_bean", 1),
                ("optional_absent", 1),
                ("plain", 2),
                ("unresolved", 2)
            ])
        );
    }

    fn work_for(n: usize) -> u64 {
        let mut body = String::new();
        for i in 0..n {
            body += &format!(
                "interface T{i} {{}}\n@Component class C{i} implements T{i} {{}}\n@Service class S{i} {{ S{i}(T{i} t, org.x.E{i} e) {{}} }}\n@Configuration class K{i} {{ @Bean org.x.F{i} f{i}() {{ return null; }} }}\n"
            );
        }
        let s = src(&body);
        let files = [APP, ("src/main/java/com/acme/A.java", s.as_str())];
        let facts: Vec<(String, FileFacts)> = files
            .iter()
            .map(|(p, s)| {
                (
                    (*p).to_owned(),
                    rb_syntax::extract(Language::Java, s.as_bytes()).unwrap(),
                )
            })
            .collect();
        let by_path: BTreeMap<&str, (Language, &FileFacts)> = facts
            .iter()
            .map(|(p, f)| (p.as_str(), (Language::Java, f)))
            .collect();
        let keys: BTreeMap<&str, Vec<String>> = by_path
            .iter()
            .map(|(p, (_, f))| (*p, rb_syntax::symbol_keys(p, f)))
            .collect();
        let go = BTreeMap::new();
        let index = Index::new(&by_path, &go, &keys);
        let service_of = |_: &str| ".".to_owned();
        let empty = BTreeSet::new();
        let depends = BTreeMap::new();
        let raw = RawScan::default();
        work::take();
        let r = resolve(&Inputs {
            files: &by_path,
            keys: &keys,
            index: &index,
            service_of: &service_of,
            deployable: &empty,
            depends: &depends,
            unparsed: &raw,
        });
        assert_eq!(r.injections.len(), n);
        work::take()
    }

    /// Invariant 9: DI resolution grows linearly in beans and points of distinct types
    /// (counted operations, at most 5× for 4× the input).
    #[test]
    fn di_resolution_grows_linearly() {
        let (small, large) = (work_for(100), work_for(400));
        assert!(large <= small * 5, "{small} -> {large}");
    }

    /// S23's pathological input: 2,000 beans of one interface against 2,000 single-valued
    /// points, all `too_ambiguous`, within a coarse wall-clock bound.
    #[test]
    fn many_beans_of_one_type_are_too_ambiguous() {
        let mut body = String::from("interface T {}\n");
        for i in 0..2000 {
            body += &format!(
                "@Component class C{i} implements T {{}}\n@Service class S{i} {{ S{i}(T t) {{}} }}\n"
            );
        }
        let start = std::time::Instant::now();
        let r = one(&body);
        assert_eq!(r.buckets, BTreeMap::from([("too_ambiguous", 2000)]));
        assert!(r.edges.is_empty());
        assert!(
            start.elapsed() < std::time::Duration::from_secs(60),
            "{:?}",
            start.elapsed()
        );
    }
}
