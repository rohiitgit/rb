//! URL constant folding (contract M4a S12): a URL template's holes resolve through constants in
//! effect, imported constants, concatenation, env references with a literal default, Java
//! `@Value` fields and Spring placeholders backed by `application.properties`. Mutable bindings
//! never fold.
//!
//! Termination is by construction: an on-path guard on bindings, a depth cap
//! ([`MAX_FOLD_DEPTH`]) and a step budget ([`MAX_FOLD_STEPS`]); only results computed without
//! hitting any of them are memoised (lesson 2026-09-30).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use rb_syntax::{
    Arg, Binding, DefKind, FileFacts, Init, Language, ObjectEntry, Span, StrTemplate, TemplatePart,
};

use crate::clients::{MAX_SPREADS, UrlValue};
use crate::endpoint::Piece;
use crate::routes::FileScope;

/// Nested resolutions followed at most.
pub const MAX_FOLD_DEPTH: usize = 16;
/// Name resolutions per fold at most, so even a wide cyclic input stays bounded.
pub const MAX_FOLD_STEPS: usize = 10_000;

/// Where a folded value comes from, weakest last. A result is labelled by the weakest source
/// that contributed text to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    Unresolved,
    EnvDefault,
    Config,
    Constant,
    Literal,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Unresolved => "unresolved",
            Origin::EnvDefault => "env_default",
            Origin::Config => "config",
            Origin::Constant => "constant",
            Origin::Literal => "literal",
        }
    }
}

/// A folded value: literal pieces, holes nothing resolved, and its label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folded {
    pub pieces: Vec<Piece>,
    /// The weakest source among the parts that resolved; [`Origin::Unresolved`] when nothing
    /// resolved to text.
    pub origin: Origin,
}

impl Folded {
    fn hole() -> Self {
        Folded {
            pieces: vec![Piece::Hole],
            origin: Origin::Unresolved,
        }
    }

    fn text(text: String, origin: Origin) -> Self {
        Folded {
            pieces: vec![Piece::Lit(text)],
            origin,
        }
    }

    /// The text, when no hole is left.
    pub fn literal(&self) -> Option<String> {
        self.pieces
            .iter()
            .map(|p| match p {
                Piece::Lit(l) => Some(l.as_str()),
                Piece::Hole => None,
            })
            .collect()
    }
}

/// What folding needs beyond one file's facts. `file` identifies a file to the caller.
pub trait Lookup {
    fn facts(&self, file: usize) -> &FileFacts;
    fn language(&self, file: usize) -> Language;
    /// A name not bound in `file` (an imported constant, `Type.CONST` of another file, Go
    /// `pkg.Const`), written at `at`: the file and binding index it names, if any. The position
    /// lets a local variable shadow a same-named package or type qualifier.
    fn external(&self, file: usize, name: &str, at: Span) -> Option<(usize, usize)>;
    /// The `application.properties` value of `key` for the service owning `file`.
    fn property(&self, file: usize, key: &str) -> Option<String>;
}

/// The value of a URL template written in `file`.
pub fn fold_template(lookup: &dyn Lookup, file: usize, template: &StrTemplate) -> Folded {
    Folder::new(lookup).template(file, template, 0).0
}

/// The value of a name (`API`, `this.base`, `Paths.ROOT`) written in `file` at `at`.
pub fn fold_name(lookup: &dyn Lookup, file: usize, name: &str, at: Span) -> Folded {
    Folder::new(lookup).name(file, name, at, 0).0
}

/// Whether a name reads the environment (`process.env.X`, `import.meta.env.X`).
pub fn is_env(name: &str) -> bool {
    name.starts_with("process.env.") || name.starts_with("import.meta.env.")
}

/// One fold's working state: per-file lookup indexes, member-write proofs
/// and memoised bindings. Build one per file and reuse it across that
/// file's folds (the per-file indexes then cost once, not once per site);
/// the step budget resets per fold, exactly as with a fresh folder.
pub struct Folder<'l> {
    lookup: &'l dyn Lookup,
    /// Each file's lookup indexes, built on first use.
    scopes: HashMap<usize, FileScope<'l>>,
    /// Each file's member-write proofs, built on first member fold.
    members: HashMap<usize, FileMembers>,
    on_path: HashSet<(usize, Span)>,
    memo: HashMap<(usize, Span), Folded>,
    steps: usize,
}

/// A result and whether it is clean (no guard, cap or budget was hit while computing it).
type Out = (Folded, bool);

impl<'l> Folder<'l> {
    /// A folder for `lookup`, shared across one file's folds (see the struct
    /// docs).
    pub fn new(lookup: &'l dyn Lookup) -> Self {
        Folder {
            lookup,
            scopes: HashMap::new(),
            members: HashMap::new(),
            on_path: HashSet::new(),
            memo: HashMap::new(),
            steps: 0,
        }
    }

    /// Folds with this folder's caches and a fresh step budget per call.
    pub fn fold_template(&mut self, file: usize, template: &StrTemplate) -> Folded {
        self.steps = 0;
        self.template(file, template, 0).0
    }

    /// Folds with this folder's caches and a fresh step budget per call.
    pub fn fold_name(&mut self, file: usize, name: &str, at: Span) -> Folded {
        self.steps = 0;
        self.name(file, name, at, 0).0
    }

    /// What `member` of `class` in `file` folds to (contract M6b S6): its
    /// single constructor-assigned value folded where written, or `None`.
    /// TypeScript/JavaScript and Java only (D6).
    fn member_fold(
        &mut self,
        file: usize,
        member: &str,
        class: Option<usize>,
        depth: usize,
    ) -> Option<Out> {
        if !matches!(
            self.lookup.language(file),
            Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Java
        ) {
            return None;
        }
        let class = class?;
        let (init, at) = {
            let facts = self.lookup.facts(file);
            let proofs = self
                .members
                .entry(file)
                .or_insert_with(|| FileMembers::build(facts, self.lookup.language(file)));
            proofs.single(facts, class, member)?
        };
        let key = (file, at);
        if !self.on_path.insert(key) {
            return Some((Folded::hole(), false));
        }
        let out = self.fold_init(file, &Some(init), at, depth + 1);
        self.on_path.remove(&key);
        Some(out)
    }

    fn template(&mut self, file: usize, t: &StrTemplate, depth: usize) -> Out {
        let mut pieces = Vec::new();
        let mut origin = Origin::Literal;
        let mut any_text = false;
        let mut clean = true;
        for part in &t.parts {
            match part {
                TemplatePart::Lit { text } => {
                    let f = if self.lookup.language(file) == Language::Java && text.contains("${") {
                        placeholders(self.lookup, file, text)
                    } else {
                        Folded::text(text.clone(), Origin::Literal)
                    };
                    if f.origin != Origin::Unresolved {
                        any_text = true;
                        origin = origin.min(f.origin);
                    }
                    pieces.extend(f.pieces);
                }
                TemplatePart::Hole { expr, span } => {
                    let (f, c) = self.name(file, expr, *span, depth);
                    clean &= c;
                    if f.origin != Origin::Unresolved {
                        any_text = true;
                        origin = origin.min(f.origin);
                    }
                    pieces.extend(f.pieces);
                }
            }
        }
        let origin = if any_text { origin } else { Origin::Unresolved };
        (Folded { pieces, origin }, clean)
    }

    fn name(&mut self, file: usize, name: &str, at: Span, depth: usize) -> Out {
        self.steps += 1;
        if depth >= MAX_FOLD_DEPTH || self.steps > MAX_FOLD_STEPS {
            return (Folded::hole(), false);
        }
        if is_env(name) {
            return (Folded::hole(), true);
        }
        let lookup = self.lookup;
        let language = lookup.language(file);
        // A member read folds from its single constructor-assigned value
        // (contract M6b S6): `this.x` in any of the folded languages, or a
        // Java name resolving to a member binding (a bare field, `Type.x`).
        // Anything unproven falls through to the normal path below.
        if matches!(
            language,
            Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Java
        ) {
            let read: Option<(&str, Option<usize>)> = match name.strip_prefix("this.") {
                Some(member) if !member.contains('.') => {
                    let scope = self
                        .scopes
                        .entry(file)
                        .or_insert_with(|| FileScope::new(lookup.facts(file)));
                    let class = local_binding(scope, language, name, at)
                        .filter(|&bi| scope.bindings[bi].member)
                        .and_then(|bi| scope.bindings[bi].scope)
                        .or_else(|| {
                            let proofs = self.members.entry(file).or_insert_with(|| {
                                FileMembers::build(lookup.facts(file), lookup.language(file))
                            });
                            proofs.innermost_class(lookup.facts(file), at)
                        });
                    Some((member, class))
                }
                _ if language == Language::Java => {
                    let scope = self
                        .scopes
                        .entry(file)
                        .or_insert_with(|| FileScope::new(lookup.facts(file)));
                    match local_binding(scope, language, name, at) {
                        Some(bi) if scope.bindings[bi].member => {
                            let member = name.rsplit('.').next().unwrap_or(name);
                            Some((member, scope.bindings[bi].scope))
                        }
                        _ => None,
                    }
                }
                _ => None,
            };
            if let Some((member, class)) = read
                && let Some(out) = self.member_fold(file, member, class, depth)
            {
                return out;
            }
        }
        let scope = self
            .scopes
            .entry(file)
            .or_insert_with(|| FileScope::new(lookup.facts(file)));
        let local = local_binding(scope, language, name, at)
            .map(|b| (file, b))
            .or_else(|| lookup.external(file, name, at));
        match local {
            Some((f, b)) => self.binding(f, b, depth + 1),
            None => (Folded::hole(), true),
        }
    }

    fn binding(&mut self, file: usize, index: usize, depth: usize) -> Out {
        let facts = self.lookup.facts(file);
        let Some(b) = facts.bindings.get(index) else {
            return (Folded::hole(), true);
        };
        let key = (file, b.span);
        if let Some(done) = self.memo.get(&key) {
            return (done.clone(), true);
        }
        if !self.on_path.insert(key) {
            return (Folded::hole(), false);
        }
        let out = self.binding_value(file, b, depth);
        self.on_path.remove(&key);
        if out.1 {
            self.memo.insert(key, out.0.clone());
        }
        out
    }

    fn binding_value(&mut self, file: usize, b: &Binding, depth: usize) -> Out {
        // `@Value("${k:d}")` (Java): configuration, whatever the field's mutability.
        if let Some(value) = b
            .decorations
            .iter()
            .find(|d| d.name.rsplit('.').next() == Some("Value"))
            .and_then(|d| {
                d.args
                    .iter()
                    .find(|a| a.key.is_none() || a.key.as_deref() == Some("value"))
            })
        {
            return (placeholders(self.lookup, file, &value.value), true);
        }
        // A member folds from its single constructor-assigned value
        // (contract M6b S6), whatever its declared constness; anything
        // unproven falls through below.
        if b.member
            && matches!(
                self.lookup.language(file),
                Language::TypeScript | Language::Tsx | Language::JavaScript | Language::Java
            )
            && let Some(out) = self.member_fold(file, &b.name, b.scope, depth)
        {
            return out;
        }
        if !b.constant {
            return (Folded::hole(), true);
        }
        self.fold_init(file, &b.init, b.span, depth)
    }

    /// A constant-like value folded: a literal, a name, or an env-default
    /// fallback, relabelled `constant` at best. Anything else (a call, an
    /// object, nothing) is a hole. `at` is where names in it resolve.
    fn fold_init(&mut self, file: usize, init: &Option<Init>, at: Span, depth: usize) -> Out {
        match init {
            Some(Init::Template { template }) => {
                let (f, clean) = self.template(file, template, depth);
                (relabel(f, Origin::Constant), clean)
            }
            Some(Init::Name { text, span }) => {
                let (f, clean) = self.name(file, text, *span, depth);
                (relabel(f, Origin::Constant), clean)
            }
            Some(Init::Fallback { primary, default }) => {
                if is_env(primary) {
                    let (f, clean) = self.template(file, default, depth);
                    return (relabel(f, Origin::EnvDefault), clean);
                }
                let (f, clean) = self.name(file, primary, at, depth);
                (relabel(f, Origin::Constant), clean)
            }
            Some(Init::Call { .. } | Init::New { .. } | Init::Object { .. }) | None => {
                (Folded::hole(), true)
            }
        }
    }
}

/// A folded constant is at best `constant`: its text was not written at the use.
fn relabel(mut f: Folded, at_best: Origin) -> Folded {
    if f.origin != Origin::Unresolved {
        f.origin = f.origin.min(at_best);
    }
    f
}

/// The binding `name` refers to within its own file: a local or parameter in effect, a member
/// (`this.x`, a Java field by its bare name) of an enclosing type, or `Type.CONST` of a type
/// declared in the file. Returns its index in `facts.bindings`.
pub(crate) fn local_binding(
    facts: &FileScope<'_>,
    language: Language,
    name: &str,
    at: Span,
) -> Option<usize> {
    if !name.contains('.')
        && let Some(i) = facts.binding_index_at(name, at)
    {
        return Some(i);
    }
    let (owner, member) = match name.strip_prefix("this.") {
        Some(member) if !member.contains('.') => (None, member),
        _ if !name.contains('.') && language == Language::Java => (None, name),
        _ => match name.rsplit_once('.') {
            Some((owner, member)) if !owner.contains('.') => (Some(owner), member),
            _ => return None,
        },
    };
    let contains =
        |span: Span| (span[0], span[1]) <= (at[0], at[1]) && (at[2], at[3]) <= (span[2], span[3]);
    facts
        .members(member)
        .iter()
        .map(|&i| (i, &facts.bindings[i]))
        .filter(|(_, b)| {
            let Some(scope) = b.scope.map(|s| &facts.definitions[s]) else {
                return false;
            };
            if !matches!(scope.kind, DefKind::Class | DefKind::Interface) {
                return false;
            }
            match owner {
                Some(owner) => scope.name == owner,
                None => contains(scope.span),
            }
        })
        // The innermost enclosing type wins.
        .max_by_key(|(_, b)| b.scope)
        .map(|(i, _)| i)
}

/// The `baseURL` values a spread name may leave in effect, read through its
/// declaration (contract M6b S5): a same-file object binding's entries, or —
/// through [`Lookup::external`] — a uniquely resolved declaration's object
/// entries, with spreads followed the same way (visited set, the pack's
/// [`MAX_SPREADS`](crate::clients::MAX_SPREADS) bound). A name that folds to
/// a string, resolves ambiguously or not at all, or names no object
/// contributes nothing here (strings fold through the normal path; anything
/// else is a hole there too).
///
/// Values come back as written: entry values keep their defining spans, so
/// the caller folds them where it folds everything else (usually the use
/// site; a literal needs no context, a name resolves where it is visible).
pub fn object_values(
    lookup: &dyn Lookup,
    file: usize,
    name: &str,
    at: Span,
    seen: &mut HashSet<(usize, Span)>,
    out: &mut Vec<UrlValue>,
) {
    crate::work::tick(1);
    if seen.len() >= MAX_SPREADS {
        return;
    }
    let scope = FileScope::new(lookup.facts(file));
    if let Some(bi) = scope.binding_index_at(name, at)
        && let Some(Init::Object { entries }) = &scope.bindings[bi].init
    {
        let key = (file, scope.bindings[bi].span);
        if seen.insert(key) {
            object_entries(lookup, file, entries, seen, out);
        }
        return;
    }
    let Some((target, bi)) = lookup.external(file, name, at) else {
        return;
    };
    let facts = lookup.facts(target);
    let key = (target, facts.bindings[bi].span);
    if !seen.insert(key) {
        return;
    }
    if let Some(Init::Object { entries }) = &facts.bindings[bi].init {
        object_entries(lookup, target, entries, seen, out);
    }
}

/// The `baseURL` values `entries` may leave in effect: its literals and
/// names from the last `baseURL` on, and its spreads through
/// [`object_values`].
fn object_entries(
    lookup: &dyn Lookup,
    file: usize,
    entries: &[ObjectEntry],
    seen: &mut HashSet<(usize, Span)>,
    out: &mut Vec<UrlValue>,
) {
    let from = entries
        .iter()
        .rposition(|e| e.key == "baseURL")
        .unwrap_or(0);
    for e in &entries[from..] {
        crate::work::tick(1);
        if e.key == "baseURL" {
            push_value(out, &UrlValue::of(Some(&e.value)));
            continue;
        }
        if e.key != "..." {
            continue;
        }
        let Arg::Name { text, span } = &e.value else {
            continue;
        };
        object_values(lookup, file, text, *span, seen, out);
    }
}

/// A readable value, once, without repeats.
fn push_value(out: &mut Vec<UrlValue>, value: &UrlValue) {
    if *value != UrlValue::Unknown && !out.iter().any(|v| crate::clients::same_value(v, value)) {
        out.push(value.clone());
    }
}

/// Proof that members hold single constructor-assigned values (contract
/// M6b S6), per file: every member write grouped under its class (with
/// same-file subclasses merged in), the classes an `Object.assign` over
/// `this` kills every proof in, and the file's classes for read-side lookup.
#[derive(Debug, Default)]
struct FileMembers {
    /// (class, member) → member-write indices: the class's own writes plus
    /// its same-file subclasses', in source order.
    writes: HashMap<(usize, String), Vec<usize>>,
    /// Classes with an `Object.assign/defineProperty(this, …)` chain.
    assigned: HashSet<usize>,
    /// Class and interface definition indices, in source order.
    classes: Vec<usize>,
}

impl FileMembers {
    /// Groups one file's member writes for single-assignment proofs. Every
    /// loop ticks what it examines (contract invariant 9).
    fn build(facts: &FileFacts, language: Language) -> Self {
        let mut out = FileMembers::default();
        // The class each definition sits in, if any.
        let mut class_of: HashMap<usize, usize> = HashMap::new();
        for (di, d) in facts.definitions.iter().enumerate() {
            crate::work::tick(1);
            if matches!(d.kind, DefKind::Class | DefKind::Interface) {
                out.classes.push(di);
            }
            let mut cur = Some(di);
            while let Some(c) = cur {
                if matches!(
                    facts.definitions[c].kind,
                    DefKind::Class | DefKind::Interface
                ) {
                    class_of.insert(di, c);
                    break;
                }
                cur = facts.definitions[c].parent;
            }
        }
        // Direct superclasses by extended simple name (over-approximate: a
        // collision only kills more proofs, never folds more).
        let mut supers: HashMap<usize, Vec<usize>> = HashMap::new();
        for (di, d) in facts.definitions.iter().enumerate() {
            crate::work::tick(1);
            if !matches!(d.kind, DefKind::Class | DefKind::Interface) {
                continue;
            }
            // Interfaces carry no fields of their own; a class implementing
            // one does not inherit a member implementation from it.
            for s in d.supertypes.iter().filter(|s| !s.implements) {
                crate::work::tick(1);
                let simple = s.name.rsplit('.').next().unwrap_or(&s.name);
                for (ci, c) in facts.definitions.iter().enumerate() {
                    crate::work::tick(1);
                    if ci != di
                        && matches!(c.kind, DefKind::Class | DefKind::Interface)
                        && c.name == simple
                    {
                        supers.entry(di).or_default().push(ci);
                    }
                }
            }
        }
        // A write counts for its class and every superclass: a subclass
        // write overrides what its instances read.
        for (wi, w) in facts.member_writes.iter().enumerate() {
            crate::work::tick(1);
            let Some(class) = w.enclosing.and_then(|d| class_of.get(&d).copied()) else {
                continue;
            };
            let mut seen = HashSet::new();
            let mut stack = vec![class];
            while let Some(c) = stack.pop() {
                if !seen.insert(c) {
                    continue;
                }
                out.writes
                    .entry((c, w.member.clone()))
                    .or_default()
                    .push(wi);
                if let Some(parents) = supers.get(&c) {
                    stack.extend(parents.iter().copied());
                }
            }
        }
        // `Object.assign/defineProperty(this, …)` may rewrite any member of
        // the chain's class. Not import-gated: a local named `Object` with
        // an `assign` method is absurd; recording the kill unconditionally
        // can only turn folds into holes.
        let object_assign = language != Language::Java;
        if object_assign {
            for chain in facts.chains.iter() {
                crate::work::tick(1);
                if chain.root.as_deref() != Some("Object") {
                    continue;
                }
                let Some(link) = chain.links.first() else {
                    continue;
                };
                if !matches!(
                    link.name.as_str(),
                    "assign" | "defineProperty" | "defineProperties"
                ) {
                    continue;
                }
                if !matches!(link.args.first(), Some(Arg::Name { text, .. }) if text == "this") {
                    continue;
                }
                if let Some(def) = chain.enclosing
                    && let Some(class) = class_of.get(&def).copied()
                {
                    out.assigned.insert(class);
                }
            }
        }
        out
    }

    /// The single constructor write of `member` in `class`: exactly one
    /// write names it there, on `this`, in a constructor — with the write's
    /// value and span, for folding where written. Foldability itself is
    /// `fold_init`'s allow-list (a call folds to a hole there, the same as
    /// here), so this stays permissive on purpose.
    fn single(&self, facts: &FileFacts, class: usize, member: &str) -> Option<(Init, Span)> {
        if self.assigned.contains(&class) {
            return None;
        }
        let [wi] = self.writes.get(&(class, member.to_owned()))?.as_slice() else {
            return None;
        };
        let w = &facts.member_writes[*wi];
        if !w.on_this || !w.ctor {
            return None;
        }
        w.value.clone().map(|init| (init, w.span))
    }

    /// The innermost class or interface containing `at`, if any.
    fn innermost_class(&self, facts: &FileFacts, at: Span) -> Option<usize> {
        let mut best: Option<(usize, i32, i32)> = None;
        for &ci in &self.classes {
            crate::work::tick(1);
            let span = facts.definitions[ci].span;
            if (span[0], span[1]) <= (at[0], at[1]) && (at[2], at[3]) <= (span[2], span[3]) {
                let start = (span[0], span[1]);
                if best.is_none_or(|(_, r, c)| start > (r, c)) {
                    best = Some((ci, span[0], span[1]));
                }
            }
        }
        best.map(|(ci, _, _)| ci)
    }
}

/// Resolves `${k}` / `${k:default}` placeholders in a Spring string.
fn placeholders(lookup: &dyn Lookup, file: usize, text: &str) -> Folded {
    let mut pieces = Vec::new();
    let mut origin = Origin::Literal;
    let mut resolved_any = false;
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        pieces.push(Piece::Lit(rest[..start].to_owned()));
        if start > 0 {
            resolved_any = true;
        }
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else {
            pieces.push(Piece::Lit(rest[start..].to_owned()));
            rest = "";
            break;
        };
        let inner = &after[..end];
        let (key, default) = match inner.split_once(':') {
            Some((k, d)) => (k.trim(), Some(d)),
            None => (inner.trim(), None),
        };
        match (lookup.property(file, key), default) {
            (Some(v), _) => {
                pieces.push(Piece::Lit(v));
                origin = origin.min(Origin::Config);
                resolved_any = true;
            }
            (None, Some(d)) => {
                pieces.push(Piece::Lit(d.to_owned()));
                origin = origin.min(Origin::EnvDefault);
                resolved_any = true;
            }
            (None, None) => pieces.push(Piece::Hole),
        }
        rest = &after[end + 1..];
    }
    if !rest.is_empty() {
        pieces.push(Piece::Lit(rest.to_owned()));
        resolved_any = true;
    }
    let pieces: Vec<Piece> = pieces
        .into_iter()
        .filter(|p| !matches!(p, Piece::Lit(l) if l.is_empty()))
        .collect();
    Folded {
        pieces,
        origin: if resolved_any {
            origin
        } else {
            Origin::Unresolved
        },
    }
}

/// A Java `.properties` file as key → value: `key=value`, `key: value` or `key value`; `#` and
/// `!` comments; a trailing backslash continues the line; `\t \n \r \f \uHHHH` and other
/// escaped characters are decoded. The last assignment of a key wins.
pub fn parse_properties(text: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut logical = String::new();
    let mut lines = text.lines().peekable();
    while let Some(line) = lines.next() {
        let trimmed = line.trim_start();
        if logical.is_empty() && (trimmed.is_empty() || trimmed.starts_with(['#', '!'])) {
            continue;
        }
        let trailing = trimmed.len() - trimmed.trim_end_matches('\\').len();
        if trailing % 2 == 1 && lines.peek().is_some() {
            logical.push_str(&trimmed[..trimmed.len() - 1]);
            continue;
        }
        logical.push_str(trimmed);
        let entry = std::mem::take(&mut logical);
        let (key, value) = split_property(&entry);
        out.insert(unescape(&key), unescape(value.trim_start()));
    }
    out
}

/// The dotted keys a Spring YAML file sets (`spring:` then `  cloud:` gives `spring` and
/// `spring.cloud`), read by indentation: comments, document markers and list items are skipped
/// and quotes around a key dropped; flow mappings (`stripe: { url: x }`) are read too. Not a
/// YAML parser (none is a dependency): it answers only whether a file sets a key, for the
/// `config_unread` caps (evaluator pass 1, P7; amendment 12).
pub fn yaml_keys(text: &str) -> BTreeSet<String> {
    yaml_entries(text).into_keys().collect()
}

/// [`yaml_keys`] with each key's scalar value, unquoted (`""` for a mapping, a sequence's text
/// as written), for the few keys whose value decides something (`spring.cloud.config.enabled`).
/// Every document's entries, a later document's value winning.
pub fn yaml_entries(text: &str) -> BTreeMap<String, String> {
    yaml_documents(text).into_iter().flatten().collect()
}

/// [`yaml_entries`] per document (`---` separates them), in order.
pub fn yaml_documents(text: &str) -> Vec<BTreeMap<String, String>> {
    let mut docs = vec![BTreeMap::new()];
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        let out = docs.last_mut().expect("never empty");
        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with('-') {
            if trimmed.starts_with("---") {
                stack.clear();
                docs.push(BTreeMap::new());
            }
            continue;
        }
        let indent = line.len() - trimmed.len();
        while stack.last().is_some_and(|(i, _)| *i >= indent) {
            stack.pop();
        }
        let prefix = stack
            .iter()
            .map(|(_, k)| k.as_str())
            .collect::<Vec<_>>()
            .join(".");
        // A document or block that is one flow mapping.
        if trimmed.starts_with('{') {
            flow_entries(&flow_text(trimmed, &mut lines), &prefix, out);
            continue;
        }
        let Some((key, value)) = yaml_key(trimmed) else {
            continue;
        };
        let full = dotted(&prefix, key);
        let value = value.trim();
        if value.starts_with('{') {
            out.insert(full.clone(), String::new());
            flow_entries(&flow_text(value, &mut lines), &full, out);
        } else {
            out.insert(full, scalar(value, true));
        }
        stack.push((indent, key.to_owned()));
    }
    docs
}

/// A `.properties` file's documents (Spring Boot separates them with a `#---` or `!---` line),
/// each parsed by [`parse_properties`], in order.
pub fn properties_documents(text: &str) -> Vec<BTreeMap<String, String>> {
    let mut docs = vec![String::new()];
    for line in text.lines() {
        if matches!(line.trim_end(), "#---" | "!---") {
            docs.push(String::new());
        } else {
            let doc = docs.last_mut().expect("never empty");
            doc.push_str(line);
            doc.push('\n');
        }
    }
    docs.iter().map(|d| parse_properties(d)).collect()
}

/// Whether a configuration document applies only under a condition, so it is not a base
/// document (evaluator pass 4, T5): it carries any `spring.config.activate.*` key
/// (`on-profile`, `on-cloud-platform`, …), or the legacy `spring.profiles` with a value or a
/// list (`spring.profiles.active`/`include`/`group` alone are no condition).
pub fn conditional_document(doc: &BTreeMap<String, String>) -> bool {
    let legacy = |k: &str| {
        k.strip_prefix("spring.profiles")
            .is_some_and(|rest| rest.starts_with('['))
    };
    doc.keys()
        .any(|k| k.starts_with("spring.config.activate.") || legacy(k))
        || doc.get("spring.profiles").is_some_and(|v| {
            !v.trim().is_empty() || !doc.keys().any(|k| k.starts_with("spring.profiles."))
        })
}

fn dotted(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_owned()
    } else {
        format!("{prefix}.{key}")
    }
}

/// A flow collection starting at `start`, continued over the next lines until its brackets
/// close (outside quotes).
fn flow_text<'t>(start: &str, lines: &mut impl Iterator<Item = &'t str>) -> String {
    let mut text = start.to_owned();
    while flow_depth(&text) > 0 {
        let Some(next) = lines.next() else { break };
        text.push('\n');
        text.push_str(next);
    }
    text
}

/// The bracket depth at the end of `text`, quotes skipped.
fn flow_depth(text: &str) -> usize {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '{' | '[') => depth += 1,
            (None, '}' | ']') => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    depth
}

/// The entries of the flow mapping `text` (`{a: 1, b: {c: 2}}`) under `prefix`, nested
/// mappings included.
fn flow_entries(text: &str, prefix: &str, out: &mut BTreeMap<String, String>) {
    let Some(inner) = text.trim().strip_prefix('{') else {
        return;
    };
    // The entries at depth 0 up to the closing brace, split at commas outside quotes.
    let mut entries = vec![String::new()];
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    for c in inner.chars() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '{' | '[') => depth += 1,
            (None, '}' | ']') if depth == 0 => break,
            (None, '}' | ']') => depth -= 1,
            (None, ',') if depth == 0 => {
                entries.push(String::new());
                continue;
            }
            _ => {}
        }
        entries.last_mut().expect("never empty").push(c);
    }
    for entry in entries {
        let Some((key, value)) = yaml_key(entry.trim()) else {
            continue;
        };
        let full = dotted(prefix, key);
        let value = value.trim();
        if value.starts_with('{') {
            out.insert(full.clone(), String::new());
            flow_entries(value, &full, out);
        } else {
            out.insert(full, scalar(value, false));
        }
    }
}

/// A scalar as written: unquoted, and in block style without a trailing comment.
fn scalar(value: &str, block: bool) -> String {
    for q in ['"', '\''] {
        if let Some(rest) = value.strip_prefix(q) {
            return rest.split(q).next().unwrap_or(rest).to_owned();
        }
    }
    let value = if block {
        value.split(" #").next().unwrap_or(value)
    } else {
        value
    };
    value.trim().to_owned()
}

/// The key of a `key: value` or `key:` entry, unquoted, and the text after the colon. The
/// colon is the first one outside quotes followed by whitespace or the end (or right after a
/// quoted key, as in `{"a":1}`).
fn yaml_key(line: &str) -> Option<(&str, &str)> {
    let mut quote: Option<char> = None;
    let mut colon = None;
    for (i, c) in line.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') if i == 0 => quote = Some(c),
            (None, ':') => {
                let next = line[i + 1..].chars().next();
                let after_quote = i > 0 && line[..i].ends_with(['"', '\'']);
                if next.is_none_or(char::is_whitespace) || after_quote {
                    colon = Some(i);
                    break;
                }
            }
            _ => {}
        }
    }
    let colon = colon?;
    let key = line[..colon].trim();
    let key = key
        .strip_prefix('\'')
        .and_then(|k| k.strip_suffix('\''))
        .or_else(|| key.strip_prefix('"').and_then(|k| k.strip_suffix('"')))
        .unwrap_or(key);
    (!key.is_empty() && !key.contains(char::is_whitespace)).then_some((key, &line[colon + 1..]))
}

/// Splits at the first unescaped `=`, `:` or whitespace (with surrounding whitespace).
fn split_property(line: &str) -> (String, &str) {
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => escaped = true,
            '=' | ':' => return (line[..i].trim_end().to_owned(), &line[i + 1..]),
            c if c.is_whitespace() => {
                let rest = line[i..].trim_start();
                let rest = rest.strip_prefix(['=', ':']).unwrap_or(rest);
                return (line[..i].to_owned(), rest);
            }
            _ => {}
        }
    }
    (line.to_owned(), "")
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('f') => out.push('\u{c}'),
            Some('u') => {
                let hex: String = chars.by_ref().take(4).collect();
                match u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    None => {
                        out.push_str("\\u");
                        out.push_str(&hex);
                    }
                }
            }
            Some(other) => out.push(other),
            None => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rb_syntax::extract;

    /// Files by index, with TS named imports and Java/Go qualified names resolved by module
    /// path → file index, and one properties map.
    struct Files {
        files: Vec<(Language, FileFacts, &'static str)>,
        properties: BTreeMap<String, String>,
    }

    impl Files {
        fn new(files: &[(Language, &'static str, &str)]) -> Self {
            Files {
                files: files
                    .iter()
                    .map(|(l, path, src)| (*l, extract(*l, src.as_bytes()).unwrap(), *path))
                    .collect(),
                properties: BTreeMap::new(),
            }
        }

        fn top_level(&self, file: usize, name: &str) -> Option<usize> {
            let facts = &self.files[file].1;
            facts.bindings.iter().position(|b| {
                b.name == name
                    && (b.scope.is_none()
                        || facts.definitions[b.scope.unwrap()].kind == DefKind::Class)
            })
        }
    }

    impl Lookup for Files {
        fn facts(&self, file: usize) -> &FileFacts {
            &self.files[file].1
        }
        fn language(&self, file: usize) -> Language {
            self.files[file].0
        }
        fn external(&self, file: usize, name: &str, _at: Span) -> Option<(usize, usize)> {
            let facts = &self.files[file].1;
            let (head, member) = name
                .rsplit_once('.')
                .map_or((None, name), |(h, m)| (Some(h), m));
            let mut targets = Vec::new();
            match head {
                // TS: a named import.
                None => {
                    for import in facts
                        .imports
                        .iter()
                        .filter(|i| i.names.iter().any(|n| n.local == name))
                    {
                        let module = import.module.trim_start_matches("./");
                        if let Some(target) = self.files.iter().position(|(_, _, p)| *p == module) {
                            targets.push(target);
                        }
                    }
                }
                // Java `Type.CONST` / Go `pkg.Const`: a file named like the qualifier.
                Some(h) => {
                    if let Some(target) = self.files.iter().position(|(_, _, p)| *p == h) {
                        targets.push(target);
                    }
                }
            }
            // Like the real resolution: an ambiguous name resolves nowhere.
            let [target] = targets.as_slice() else {
                return None;
            };
            Some((*target, self.top_level(*target, member)?))
        }
        fn property(&self, _file: usize, key: &str) -> Option<String> {
            self.properties.get(key).cloned()
        }
    }

    fn show(f: &Folded) -> String {
        let pieces: String = f
            .pieces
            .iter()
            .map(|p| match p {
                Piece::Lit(l) => l.clone(),
                Piece::Hole => "{?}".to_owned(),
            })
            .collect();
        format!("{pieces} [{}]", f.origin.as_str())
    }

    /// Folds the first argument of the last call in file 0.
    fn fold_last_call(files: &Files) -> String {
        let facts = files.facts(0);
        let chain = facts.chains.last().unwrap();
        match &chain.links.last().unwrap().args[0] {
            rb_syntax::Arg::Template { template } => show(&fold_template(files, 0, template)),
            rb_syntax::Arg::Name { text, span } => show(&fold_name(files, 0, text, *span)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn same_file_constants_concatenation_and_env_defaults() {
        let cases = [
            (
                "const A = \"http://a\";\nget(`${A}/x`);\n",
                "http://a/x [constant]",
            ),
            ("get(\"/lit\");\n", "/lit [literal]"),
            (
                "const H = \"http://h\";\nconst P = H + \"/p\";\nget(P + \"/x\");\n",
                "http://h/p/x [constant]",
            ),
            (
                "const API = import.meta.env.VITE_API_URL ?? \"http://gateway\";\nget(`${API}/api/v1/login`);\n",
                "http://gateway/api/v1/login [env_default]",
            ),
            (
                "const B = process.env.B || 'http://b';\nget(B);\n",
                "http://b [env_default]",
            ),
            ("get(`${process.env.X}/y`);\n", "{?}/y [literal]"),
            // Mutable bindings never fold.
            ("let M = \"http://m\";\nget(`${M}/x`);\n", "{?}/x [literal]"),
            ("function f(id) { get(`/u/${id}`); }\n", "/u/{?} [literal]"),
            // A call is not folded.
            ("const C = mk();\nget(C);\n", "{?} [unresolved]"),
            // A shadowing local hides the constant.
            (
                "const S = \"/outer\";\nfunction f() { let S = x; get(S); }\n",
                "{?} [unresolved]",
            ),
        ];
        for (src, want) in cases {
            let files = Files::new(&[(Language::TypeScript, "a", src)]);
            assert_eq!(fold_last_call(&files), want, "{src}");
        }
    }

    #[test]
    fn java_fields_values_and_placeholders() {
        let src = "class C {\n  static final String BASE = \"http://auth\";\n  String mutable = \"http://m\";\n  @Value(\"${auth.url:http://default}\") String authUrl;\n  @Value(\"${missing}\") String missing;\n  void m(String id) {\n    a(BASE + \"/x/\" + id);\n    a(mutable + \"/m\");\n    a(authUrl + \"/v\");\n    a(missing + \"/n\");\n    a(this.authUrl);\n    a(C.BASE);\n    a(\"${feign.url}/p\");\n  }\n}\n";
        let mut files = Files::new(&[(Language::Java, "C", src)]);
        let all = |files: &Files| -> Vec<String> {
            let facts = files.facts(0);
            facts
                .chains
                .iter()
                .map(|c| match &c.links[0].args[0] {
                    rb_syntax::Arg::Template { template } => {
                        show(&fold_template(files, 0, template))
                    }
                    rb_syntax::Arg::Name { text, span } => show(&fold_name(files, 0, text, *span)),
                    other => panic!("{other:?}"),
                })
                .collect()
        };
        assert_eq!(
            all(&files),
            [
                "http://auth/x/{?} [constant]",
                "{?}/m [literal]",
                "http://default/v [env_default]",
                "{?}/n [literal]",
                "http://default [env_default]",
                "http://auth [constant]",
                "{?}/p [literal]",
            ]
        );
        files
            .properties
            .insert("auth.url".into(), "http://configured".into());
        files
            .properties
            .insert("feign.url".into(), "http://notification:3000".into());
        let got = all(&files);
        assert_eq!(got[2], "http://configured/v [config]");
        assert_eq!(got[6], "http://notification:3000/p [config]");
    }

    #[test]
    fn imported_constants_across_files() {
        let files = Files::new(&[
            (
                Language::TypeScript,
                "a",
                "import { BASE } from \"./b\";\nget(`${BASE}/x`);\n",
            ),
            (
                Language::TypeScript,
                "b",
                "import { ROOT } from \"./c\";\nexport const BASE = ROOT + \"/api\";\n",
            ),
            (
                Language::TypeScript,
                "c",
                "export const ROOT = \"http://svc\";\n",
            ),
        ]);
        assert_eq!(fold_last_call(&files), "http://svc/api/x [constant]");
        let java = Files::new(&[
            (
                Language::Java,
                "C",
                "class C { void m() { a(Paths.ROOT + \"/x\"); } }\n",
            ),
            (
                Language::Java,
                "Paths",
                "class Paths { static final String ROOT = \"/root\"; }\n",
            ),
        ]);
        assert_eq!(fold_last_call(&java), "/root/x [constant]");
        let go = Files::new(&[
            (
                Language::Go,
                "main",
                "package main\nfunc f() { get(cfg.Base + \"/x\") }\n",
            ),
            (
                Language::Go,
                "cfg",
                "package cfg\nconst Base = \"http://billing\"\n",
            ),
        ]);
        assert_eq!(fold_last_call(&go), "http://billing/x [constant]");
        // A Go `var` is mutable: never folded, even across packages.
        let go = Files::new(&[
            (
                Language::Go,
                "main2",
                "package main\nfunc f() { get(cfg.Mutable + \"/x\") }\n",
            ),
            (
                Language::Go,
                "cfg",
                "package cfg\nvar Mutable = \"http://m\"\n",
            ),
        ]);
        assert_eq!(fold_last_call(&go), "{?}/x [literal]");
    }

    /// Every recursion edge has its cycle test: same file (Java fields see each other), across
    /// files through imports, and a self-reference.
    #[test]
    fn cycles_terminate_unresolved() {
        let java = Files::new(&[(
            Language::Java,
            "C",
            "class C {\n  static final String A = B;\n  static final String B = A;\n  static final String S = S + \"/x\";\n  void m() { a(A); a(S); }\n}\n",
        )]);
        let facts = java.facts(0);
        let got: Vec<String> = facts
            .chains
            .iter()
            .map(|c| match &c.links[0].args[0] {
                rb_syntax::Arg::Name { text, span } => show(&fold_name(&java, 0, text, *span)),
                other => panic!("{other:?}"),
            })
            .collect();
        // `S = S + "/x"` keeps its literal part around the hole it cannot fill.
        assert_eq!(got, ["{?} [unresolved]", "{?}/x [constant]"]);

        let ts = Files::new(&[
            (
                Language::TypeScript,
                "a",
                "import { B } from \"./b\";\nexport const A = B;\nget(A);\n",
            ),
            (
                Language::TypeScript,
                "b",
                "import { A } from \"./a\";\nexport const B = A;\n",
            ),
        ]);
        assert_eq!(fold_last_call(&ts), "{?} [unresolved]");
    }

    /// A 1,000-link chain is cut at the depth cap; a short one resolves.
    #[test]
    fn long_chains_stop_at_the_depth_cap() {
        let mut src = String::from("const A0 = \"/x\";\n");
        for i in 1..1000 {
            src.push_str(&format!("const A{i} = A{};\n", i - 1));
        }
        src.push_str("get(A10);\nget(A999);\n");
        let files = Files::new(&[(Language::TypeScript, "a", &src)]);
        let facts = files.facts(0);
        let got: Vec<String> = facts
            .chains
            .iter()
            .map(|c| match &c.links[0].args[0] {
                rb_syntax::Arg::Name { text, span } => show(&fold_name(&files, 0, text, *span)),
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(got, ["/x [constant]", "{?} [unresolved]"]);
    }

    /// A wide diamond (each constant the concatenation of the one below with itself) costs one
    /// resolution per constant and per use, not one per path: clean results are memoised, so
    /// 2^14 paths take at most two resolutions per level.
    #[test]
    fn wide_diamonds_are_memoised() {
        let mut src = String::from("const D0 = \"a\";\n");
        for i in 1..15 {
            src.push_str(&format!("const D{i} = D{} + D{};\n", i - 1, i - 1));
        }
        src.push_str("get(D14);\n");
        let files = Files::new(&[(Language::TypeScript, "a", &src)]);
        let facts = files.facts(0);
        let rb_syntax::Arg::Name { text, span } = &facts.chains[0].links[0].args[0] else {
            panic!()
        };
        let mut folder = Folder::new(&files);
        let (folded, clean) = folder.name(0, text, *span, 0);
        assert!(clean);
        assert_eq!(folded.literal().unwrap(), "a".repeat(1 << 14));
        assert!(folder.steps <= 2 * 15, "{} resolutions", folder.steps);
    }

    /// Evaluator pass 1, P5: a result computed while a guard was hit is never memoised. In
    /// `A + B` with `A = B + "x"` and `B = A`, folding `A` meets `B` on the way back to `A` (the
    /// on-path guard): `B` is a hole there only because `A` was on the path. Memoised, the later
    /// `B` would read as that hole instead of its own value, `{?}x`.
    #[test]
    fn results_cut_by_a_guard_are_not_memoised() {
        let java = Files::new(&[(
            Language::Java,
            "C",
            "class C {\n  static final String A = B + \"x\";\n  static final String B = A;\n  void m() { a(A + B); }\n}\n",
        )]);
        assert_eq!(fold_last_call(&java), "{?}x{?}x [constant]");
    }

    /// Evaluator pass 1, P5: the step budget bounds a wide input whose results are never clean
    /// (so never memoised): each `Di = D(i-1) + D(i-1)` down to a self-reference costs 2^14
    /// resolutions without the budget; with it, folding stops after `MAX_FOLD_STEPS` and the
    /// result is not clean.
    #[test]
    fn the_step_budget_bounds_unclean_diamonds() {
        let mut src = String::from("const Z = Z;\nconst D0 = Z + \"a\";\n");
        for i in 1..15 {
            src.push_str(&format!("const D{i} = D{} + D{};\n", i - 1, i - 1));
        }
        src.push_str("get(D14);\n");
        let files = Files::new(&[(Language::TypeScript, "a", &src)]);
        let facts = files.facts(0);
        let rb_syntax::Arg::Name { text, span } = &facts.chains[0].links[0].args[0] else {
            panic!()
        };
        let mut folder = Folder::new(&files);
        let (folded, clean) = folder.name(0, text, *span, 0);
        assert!(!clean);
        assert!(folded.pieces.contains(&Piece::Hole));
        // Past the budget, each pending level returns after one more (refused) resolution.
        assert!(
            folder.steps <= MAX_FOLD_STEPS + MAX_FOLD_DEPTH,
            "{} resolutions",
            folder.steps
        );
    }

    /// Evaluator pass 1, P7: the dotted keys a Spring YAML file sets, by indentation (no YAML
    /// parser is a dependency); used only to ask whether an unread file sets a key, so a key too
    /// many is a cap too many, never a wrong value.
    #[test]
    fn yaml_keys_are_dotted_by_indentation() {
        let text = "# comment\nspring:\n  application:\n    name: b\n  cloud.config:\n    uri: \"http://config:8888\"\n\nusers:\n  url: http://legacy  # trailing\n  list:\n    - a: 1\n'quoted.key': x\n---\nserver:\n  port: 1\n  servlet.context-path: /accounts\nnot a key\n";
        let keys: Vec<String> = yaml_keys(text).into_iter().collect();
        assert_eq!(
            keys,
            [
                "quoted.key",
                "server",
                "server.port",
                "server.servlet.context-path",
                "spring",
                "spring.application",
                "spring.application.name",
                "spring.cloud.config",
                "spring.cloud.config.uri",
                "users",
                "users.list",
                "users.url",
            ]
        );
    }

    /// Contract M4a amendment 12 (evaluator pass 2, Q3): flow mappings (`stripe: { url: x }`,
    /// nested, over several lines, and a document that is one) set their dotted keys too, and
    /// scalar values are kept for the keys that need one (`spring.cloud.config.enabled`).
    #[test]
    fn yaml_flow_mappings_and_values() {
        let text = "stripe: { url: \"https://api.stripe.com\", 'timeout': 3 }\nspring: {cloud: {config: {enabled: false, uri: \"http://c:8888\"}}}\nlist: [a, b]\nmulti: {\n  a: {b: 1},\n  c: \"x, y: z\"\n}\nplain:\n  key: \"v\" # comment\n---\n{server: {port: 1}}\n";
        let entries = yaml_entries(text);
        let keys: Vec<&str> = entries.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "list",
                "multi",
                "multi.a",
                "multi.a.b",
                "multi.c",
                "plain",
                "plain.key",
                "server",
                "server.port",
                "spring",
                "spring.cloud",
                "spring.cloud.config",
                "spring.cloud.config.enabled",
                "spring.cloud.config.uri",
                "stripe",
                "stripe.timeout",
                "stripe.url",
            ]
        );
        assert_eq!(entries["spring.cloud.config.enabled"], "false");
        assert_eq!(entries["stripe.url"], "https://api.stripe.com");
        assert_eq!(entries["plain.key"], "v");
        assert_eq!(entries["multi.c"], "x, y: z");
        assert_eq!(yaml_keys(text), entries.into_keys().collect());
    }

    #[test]
    fn properties_files() {
        let text = "# comment\n! also\nserver.servlet.context-path=/auth\nfeign.url : http://n:3000\nkey value with spaces\nlong = a\\\n    b\nesc=tab\\there \\u0041\nempty=\nserver.servlet.context-path=/last\n";
        let p = parse_properties(text);
        assert_eq!(
            p.get("server.servlet.context-path").map(String::as_str),
            Some("/last")
        );
        assert_eq!(
            p.get("feign.url").map(String::as_str),
            Some("http://n:3000")
        );
        assert_eq!(p.get("key").map(String::as_str), Some("value with spaces"));
        assert_eq!(p.get("long").map(String::as_str), Some("ab"));
        assert_eq!(p.get("esc").map(String::as_str), Some("tab\there A"));
        assert_eq!(p.get("empty").map(String::as_str), Some(""));
        assert_eq!(p.len(), 6);
    }

    /// Renders `baseURL` values as written: literals quoted, names bare.
    fn shown_values(values: &[crate::clients::UrlValue]) -> Vec<String> {
        values
            .iter()
            .map(|v| match v {
                crate::clients::UrlValue::Template(t) => format!(
                    "'{}'",
                    t.parts
                        .iter()
                        .map(|p| match p {
                            TemplatePart::Lit { text } => text.clone(),
                            TemplatePart::Hole { expr, .. } => format!("{{{expr}}}"),
                        })
                        .collect::<String>()
                ),
                crate::clients::UrlValue::Name { text, .. } => text.clone(),
                crate::clients::UrlValue::Unknown => "?".into(),
                crate::clients::UrlValue::Contested(_) => "contested".into(),
            })
            .collect()
    }

    /// The `baseURL` values the spread `...{name}` in file 0 may leave, read
    /// through a uniquely resolved declaration (contract M6b S5).
    fn spread_values(files: &Files, file: usize, name: &str) -> Vec<String> {
        let facts = files.facts(file);
        let span = facts
            .chains
            .iter()
            .flat_map(|c| &c.links)
            .flat_map(|l| &l.args)
            .find_map(|a| match a {
                rb_syntax::Arg::Name { text, span } if text == name => Some(*span),
                _ => None,
            })
            .unwrap();
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        object_values(files, file, name, span, &mut seen, &mut out);
        shown_values(&out)
    }

    /// Contract M6b S5: a spread name resolves through a unique declaration
    /// to the object it holds, across files. An ambiguous or missing target,
    /// a cycle, a string (which folds through the normal path) and a
    /// parameter name contribute nothing here.
    #[test]
    fn object_values_resolve_through_unique_declarations() {
        let files = Files::new(&[
            (
                Language::TypeScript,
                "use",
                "import { sharedOpts } from \"./opts\";\nimport { multi } from \"./multi\";\nget(sharedOpts, multi);\n",
            ),
            (
                Language::TypeScript,
                "opts",
                "export const sharedOpts = { baseURL: \"http://orders\" };\n",
            ),
            (
                Language::TypeScript,
                "multi",
                "import { deep } from \"./deep\";\nexport const multi = { ...deep, retries: 1 };\n",
            ),
            (
                Language::TypeScript,
                "deep",
                "export const deep = { baseURL: \"https://api.stripe.com\" };\n",
            ),
        ]);
        assert_eq!(spread_values(&files, 0, "sharedOpts"), ["'http://orders'"]);
        // Through two files of spreads.
        assert_eq!(
            spread_values(&files, 0, "multi"),
            ["'https://api.stripe.com'"]
        );
        // A string constant folds normally, so it contributes no object value.
        let strings = Files::new(&[
            (
                Language::TypeScript,
                "use",
                "import { API } from \"./consts\";\nget(API);\n",
            ),
            (
                Language::TypeScript,
                "consts",
                "export const API = \"http://orders\";\n",
            ),
        ]);
        assert!(spread_values(&strings, 0, "API").is_empty());
        // Ambiguous (the same local imported twice — the extractor keeps
        // both, the real resolution resolves neither), missing and cyclic
        // targets: none.
        let ambiguous = Files::new(&[
            (
                Language::TypeScript,
                "use",
                "import { dup } from \"./a\";\nimport { dup } from \"./b\";\nget(dup);\n",
            ),
            (
                Language::TypeScript,
                "a",
                "export const dup = { baseURL: \"http://a\" };\n",
            ),
            (
                Language::TypeScript,
                "b",
                "export const dup = { baseURL: \"http://b\" };\n",
            ),
        ]);
        assert!(spread_values(&ambiguous, 0, "dup").is_empty());
        let cyclic = Files::new(&[
            (
                Language::TypeScript,
                "use",
                "import { ca } from \"./ca\";\nget(ca);\n",
            ),
            (
                Language::TypeScript,
                "ca",
                "import { cb } from \"./cb\";\nexport const ca = { ...cb };\n",
            ),
            (
                Language::TypeScript,
                "cb",
                "import { ca } from \"./ca\";\nexport const cb = { ...ca };\n",
            ),
        ]);
        assert!(spread_values(&cyclic, 0, "ca").is_empty());
    }

    /// Contract M6b S5, invariant 9: n distinct spread names cost linear
    /// work — 4× the names makes at most 5× the counted steps, the same
    /// pattern the stitcher runs (one expansion per contested name).
    #[test]
    fn object_values_grow_linearly() {
        fn work(n: usize) -> u64 {
            // Only the mock's paths need `'static`; the sources are borrowed.
            let mut paths: Vec<&'static str> = Vec::new();
            let mut sources: Vec<String> = Vec::new();
            let mut use_src = String::new();
            for i in 0..n {
                use_src.push_str(&format!("import {{ o{i} }} from \"./o{i}\";\n"));
            }
            use_src.push_str("get(");
            for i in 0..n {
                use_src.push_str(&format!("o{i}, "));
            }
            use_src.push_str(");\n");
            sources.push(use_src);
            paths.push("use");
            for i in 0..n {
                paths.push(Box::leak(format!("o{i}").into_boxed_str()));
                sources.push(format!(
                    "export const o{i} = {{ baseURL: \"http://o{i}\" }};\n"
                ));
            }
            let refs: Vec<(Language, &'static str, &str)> = paths
                .iter()
                .zip(sources.iter())
                .map(|(p, s)| (Language::TypeScript, *p, s.as_str()))
                .collect();
            let files = Files::new(&refs);
            let facts = files.facts(0);
            let names: Vec<(String, Span)> = facts
                .chains
                .iter()
                .flat_map(|c| &c.links)
                .flat_map(|l| &l.args)
                .filter_map(|a| match a {
                    rb_syntax::Arg::Name { text, span } => Some((text.clone(), *span)),
                    _ => None,
                })
                .collect();
            crate::work::take();
            let mut total = 0;
            for (name, span) in &names {
                let mut seen = HashSet::new();
                let mut out = Vec::new();
                object_values(&files, 0, name, *span, &mut seen, &mut out);
                total += out.len();
            }
            assert_eq!(total, n);
            crate::work::take()
        }
        let (small, large) = (work(25), work(100));
        assert!(small > 0 && large <= 5 * small, "{small} → {large}");
    }

    /// Folds the first argument of the last call of one TypeScript file.
    fn fold_one(src: &str) -> String {
        let files = Files::new(&[(Language::TypeScript, "a", src)]);
        fold_last_call(&files)
    }

    /// Contract M6b S6: `this.x` folds when assigned exactly once, in a
    /// constructor, to a foldable value — a literal, a name, or an env
    /// default. The value folds where it is written.
    #[test]
    fn members_fold_from_a_single_constructor_write() {
        // A literal: the value is a constant wherever it is read.
        assert_eq!(
            fold_one(
                "class C {\n  svc: string;\n  constructor() {\n    this.svc = \"http://orders\";\n  }\n  m() {\n    get(`${this.svc}/x`);\n  }\n}\n"
            ),
            "http://orders/x [constant]"
        );
        // A name: folded where written (here a same-file constant).
        assert_eq!(
            fold_one(
                "const B = \"http://b\";\nclass C {\n  svc: string;\n  constructor() {\n    this.svc = B;\n  }\n  m() {\n    get(`${this.svc}/x`);\n  }\n}\n"
            ),
            "http://b/x [constant]"
        );
        // The ftgo shape: a constructor parameter inside holes out the host
        // but keeps the literal tail.
        assert_eq!(
            fold_one(
                "class C {\n  consumerService: string;\n  constructor(options) {\n    this.consumerService = `${options.baseUrl}/consumers`;\n  }\n  m(id) {\n    get(`${this.consumerService}/${id}`);\n  }\n}\n"
            ),
            "{?}/consumers/{?} [constant]"
        );
        // Java blank finals assign once in a constructor by definition.
        let java = Files::new(&[(
            Language::Java,
            "C",
            "class C {\n  final String s;\n  C() {\n    this.s = \"http://j\";\n  }\n  String m() {\n    return get(this.s);\n  }\n}\n",
        )]);
        assert_eq!(fold_last_call(&java), "http://j [constant]");
    }

    /// Contract M6b S6: anything else stays a hole — a second write, a write
    /// outside a constructor, an update, an alias write, a subclass write, an
    /// `Object.assign` over `this`, an unfoldable value, a field
    /// initialiser (not a constructor write, by contract).
    #[test]
    fn members_without_a_single_constructor_write_stay_holes() {
        let cls = |body: &str| {
            format!(
                "class C {{\n  svc: string;\n  constructor() {{\n{body}  }}\n  m() {{\n    get(`${{this.svc}}/x`);\n  }}\n}}\n"
            )
        };
        // Written twice.
        assert_eq!(
            fold_one(&cls(
                "    this.svc = \"http://a\";\n    this.svc = \"http://b\";\n"
            )),
            "{?}/x [literal]"
        );
        // Written outside any constructor.
        assert_eq!(
            fold_one(
                "class C {\n  svc: string;\n  m() {\n    this.svc = \"http://a\";\n    get(`${this.svc}/x`);\n  }\n}\n"
            ),
            "{?}/x [literal]"
        );
        // An update reads and writes.
        assert_eq!(
            fold_one(&cls("    this.svc = \"http://a\";\n    this.svc++;\n")),
            "{?}/x [literal]"
        );
        // Written through an alias.
        assert_eq!(
            fold_one(
                "class C {\n  svc: string;\n  constructor(s) {\n    this.svc = \"http://a\";\n    s.svc = \"http://evil\";\n  }\n  m() {\n    get(`${this.svc}/x`);\n  }\n}\n"
            ),
            "{?}/x [literal]"
        );
        // Only ever written through an alias: still a hole.
        assert_eq!(
            fold_one(
                "class C {\n  svc: string;\n  constructor(s) {\n    s.svc = \"http://a\";\n  }\n  m() {\n    get(`${this.svc}/x`);\n  }\n}\n"
            ),
            "{?}/x [literal]"
        );
        // Written in a subclass (the subclass sorts first, so the read
        // stays last for `fold_last_call`).
        assert_eq!(
            fold_one(
                "class D extends C {\n  constructor() {\n    super();\n    this.svc = \"http://evil\";\n  }\n}\nclass C {\n  svc: string;\n  constructor() {\n    this.svc = \"http://a\";\n  }\n  m() {\n    get(`${this.svc}/x`);\n  }\n}\n"
            ),
            "{?}/x [literal]"
        );
        // `Object.assign` over `this` may rewrite anything.
        assert_eq!(
            fold_one(&cls(
                "    this.svc = \"http://a\";\n    Object.assign(this, opts);\n"
            )),
            "{?}/x [literal]"
        );
        // An unfoldable value (a call) is no value.
        assert_eq!(fold_one(&cls("    this.svc = mk();\n")), "{?}/x [literal]");
        // A field initialiser is not a constructor write.
        assert_eq!(
            fold_one(
                "class C {\n  svc = \"http://i\";\n  m() {\n    get(`${this.svc}/x`);\n  }\n}\n"
            ),
            "{?}/x [literal]"
        );
        // A cycle folds to holes and terminates.
        assert_eq!(
            fold_one(
                "class C {\n  a: string;\n  b: string;\n  constructor() {\n    this.a = this.b;\n    this.b = this.a;\n  }\n  m() {\n    get(`${this.a}/x`);\n  }\n}\n"
            ),
            "{?}/x [literal]"
        );
        // Java: a method write kills the constructor one.
        let java_two = Files::new(&[(
            Language::Java,
            "C",
            "class C {\n  String s;\n  C() {\n    this.s = \"http://a\";\n  }\n  void n() {\n    this.s = \"http://evil\";\n  }\n  String m() {\n    return get(this.s);\n  }\n}\n",
        )]);
        assert_eq!(fold_last_call(&java_two), "{?} [unresolved]");
    }

    /// Contract M6b S6, invariant 9: n singly-written members cost linear
    /// work — 4× the members makes at most 5× the counted steps, folding
    /// through one folder per file as production does.
    #[test]
    fn member_proofs_grow_linearly() {
        fn work(n: usize) -> u64 {
            let mut src = String::new();
            src.push_str("class C {\n  constructor() {\n");
            for i in 0..n {
                src.push_str(&format!("    this.m{i} = \"http://o{i}\";\n"));
            }
            src.push_str("  }\n  m() {\n");
            for i in 0..n {
                src.push_str(&format!("    get(`${{this.m{i}}}/x`);\n"));
            }
            src.push_str("  }\n}\n");
            let files = Files::new(&[(Language::TypeScript, "a", src.as_str())]);
            crate::work::take();
            let facts = files.facts(0);
            let mut folder = Folder::new(&files);
            let mut shown = Vec::new();
            for chain in facts.chains.iter() {
                if let Some(rb_syntax::Arg::Template { template }) =
                    chain.links.last().and_then(|l| l.args.first())
                {
                    shown.push(show(&folder.fold_template(0, template)));
                }
            }
            // Every member folded to its literal: the proof holds per member.
            let want: Vec<String> = (0..n)
                .map(|i| format!("http://o{i}/x [constant]"))
                .collect();
            assert_eq!(shown, want);
            crate::work::take()
        }
        let (small, large) = (work(25), work(100));
        assert!(small > 0 && large <= 5 * small, "{small} → {large}");
    }
}
