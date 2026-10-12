//! Spring dependency-injection facts per file (contract M4b S2, S3, S6 gate D): beans, injection
//! points, scan roots and the DI surface a Java file shows. Pure functions of one file's syntax
//! facts; the indexer resolves types, classpaths and roots across files (S4–S8).
//!
//! Every Spring name is recognised only through an import of its package or its qualified name
//! ([`crate::routes::is_annotation`], invariant 5): a same-named annotation from another package,
//! a commented-out one or a name in a string is not Spring.

use std::collections::{BTreeMap, BTreeSet};

use rb_syntax::{
    Arg, Binding, Decoration, DecorationArg, DefKind, Definition, FileFacts, Language, Span,
    TemplatePart,
};

use crate::routes::is_annotation;
use crate::work;

const STEREOTYPE: &str = "org.springframework.stereotype";
const WEB: &str = "org.springframework.web.bind.annotation";
const CONTEXT: &str = "org.springframework.context.annotation";
const BOOT: &str = "org.springframework.boot";
const BOOT_AUTOCONFIGURE: &str = "org.springframework.boot.autoconfigure";
const BOOT_CONDITION: &str = "org.springframework.boot.autoconfigure.condition";
const BEANS_ANNOTATION: &str = "org.springframework.beans.factory.annotation";
const BEANS_FACTORY: &str = "org.springframework.beans.factory";
const CORE_ANNOTATION: &str = "org.springframework.core.annotation";
const DATA_REPOSITORY: &str = "org.springframework.data.repository";
const FEIGN: &[&str] = &[
    "org.springframework.cloud.openfeign",
    "org.springframework.cloud.netflix.feign",
];
const INJECT: &[&str] = &["jakarta.inject", "javax.inject"];
const RESOURCE: &[&str] = &["jakarta.annotation", "javax.annotation"];
const LOMBOK: &str = "lombok";
// Contract M6j S2: Guice and Dagger packages. Guice's `Inject` is its own annotation
// (`com.google.inject.Inject`, not JSR-330); Dagger binds through `dagger` annotations
// with `jakarta.inject` points.
const GUICE: &str = "com.google.inject";
const GUICE_NAME: &str = "com.google.inject.name";
const DAGGER: &str = "dagger";
// Contract M6j S3: NestJS decorators live in `@nestjs/common`.
const NEST: &str = "@nestjs/common";

/// The class-level annotations that make a class a bean when it is component-scanned (S2),
/// by package. `@SpringBootApplication` and `@SpringBootConfiguration` are meta-annotated with
/// `@Configuration` (contract M4b amendment 6).
const STEREOTYPES: &[(&str, &str)] = &[
    (STEREOTYPE, "Component"),
    (STEREOTYPE, "Service"),
    (STEREOTYPE, "Repository"),
    (STEREOTYPE, "Controller"),
    (WEB, "RestController"),
    (WEB, "ControllerAdvice"),
    (WEB, "RestControllerAdvice"),
    (CONTEXT, "Configuration"),
    (BOOT_AUTOCONFIGURE, "SpringBootApplication"),
    (BOOT, "SpringBootConfiguration"),
    // JSR-330: Spring's component scan includes `@Named` classes, named by its value
    // (evaluator pass 1, Y6).
    ("jakarta.inject", "Named"),
    ("javax.inject", "Named"),
];

/// Where a bean comes from (`INJECTS`' `origin`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Origin {
    Stereotype,
    BeanMethod,
    SpringData,
    Feign,
    /// A `@Singleton` class (Guice's own; contract M6j S2).
    Singleton,
    /// A concrete class with an `@Inject` constructor in a Guice/Dagger file (contract M6j
    /// S2): injectable without further declaration.
    Injectable,
    /// A Dagger `@Binds` method or a Guice `bind(X).to(Y)` pair (contract M6j S2): the
    /// binding declaration, typed by the bound type.
    Binds,
    /// A Spring `<bean>` (contract M6j S4): targets its resolved class definition; the
    /// `id` and aliases ride names and qualifiers.
    XmlBean,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Stereotype => "stereotype",
            Origin::BeanMethod => "bean_method",
            Origin::SpringData => "spring_data",
            Origin::Feign => "feign",
            Origin::Singleton => "singleton",
            Origin::Injectable => "injectable",
            Origin::Binds => "binds",
            Origin::XmlBean => "xml_bean",
        }
    }
}

/// A bean definition in this file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bean {
    /// Index into `FileFacts::definitions`: the class, the `@Bean` method, or the repository or
    /// Feign interface (the `INJECTS` target, D2).
    pub definition: usize,
    pub origin: Origin,
    /// The bean's type as written without type arguments: the class or interface name, or a
    /// `@Bean` method's return type.
    pub type_name: String,
    /// The `@Bean` method's return type has type arguments.
    pub generic: bool,
    /// Bean names (D9): a stereotype's literal value or the decapitalised class name; a `@Bean`
    /// method's `name`/`value` literals or its name; a repository's decapitalised name; a Feign
    /// client's `qualifiers`, or its context id plus `FeignClient`.
    pub names: Vec<String>,
    /// `@Qualifier` literals on the class or the `@Bean` method (Feign: `qualifiers`).
    pub qualifiers: Vec<String>,
    /// `@Primary` (Feign: `primary`, `true` unless a literal `false`).
    pub primary: bool,
    /// `@Profile` or `@Conditional*` on the class, or on the `@Bean` method or its class.
    pub conditional: bool,
    /// Annotations on the class or method outside the inert and DI-understood lists, as
    /// written.
    pub annotations: Vec<String>,
    /// `@Bean` methods: the types created by `new` in the body, sorted.
    pub created: Vec<String>,
    /// Spring Data repositories: the simple names of the interfaces they extend (fragment
    /// interfaces among them, D3).
    pub extends: Vec<String>,
}

/// How an injection point receives its value (`INJECTS`' `via`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Form {
    Constructor,
    Field,
    Setter,
    BeanParameter,
}

impl Form {
    pub fn as_str(self) -> &'static str {
        match self {
            Form::Constructor => "constructor",
            Form::Field => "field",
            Form::Setter => "setter",
            Form::BeanParameter => "bean_parameter",
        }
    }
}

/// A container type Spring unwraps (D9).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Wrapper {
    /// `List`/`Set`/`Collection<T>`, `Map<String, T>`, `T[]`: every candidate.
    Collection,
    /// `Optional<T>`, `ObjectProvider<T>`, `ObjectFactory<T>`, `Provider<T>`: as `T`, and no
    /// candidate is not an error.
    Optional,
}

/// One injection point (S3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InjectionPoint {
    /// Index into `FileFacts::definitions` of the owner: the class for constructor, field and
    /// setter points, the `@Bean` method for its parameters (the `INJECTS` source, D2).
    pub owner: usize,
    /// The owning class is a bean; an annotated field or method of any other class is a point
    /// of bucket `not_a_bean`.
    pub owner_is_bean: bool,
    pub form: Form,
    /// The parameter, field or property name (name fallback, `@Resource`).
    pub name: String,
    /// The declared type as written without type arguments (empty for an array).
    pub type_name: String,
    /// The injected type as written: the declared type, or a wrapper's element.
    pub element: String,
    pub wrapper: Option<Wrapper>,
    /// The injected type has type arguments that are not a wrapper's (gate `generic`).
    pub generic: bool,
    /// The declared type arguments as simple names, for exact-agreement picks
    /// (contract M6c S9): empty unless every argument is a bare simple name —
    /// a wildcard, bound, qualification or nesting anywhere empties the whole
    /// list, so those points never agree.
    pub type_args: Vec<String>,
    /// The point's `@Qualifier` literal.
    pub qualifier: Option<String>,
    /// The qualifier rides a Lombok-generated parameter because the nearest
    /// `lombok.config` chain positively lists its annotation under
    /// `copyableAnnotations` (contract M6c S8); the indexer counts these
    /// narrowings.
    pub copied_qualifier: bool,
    /// `@Resource`: the bean name it resolves by first (`name`, else the field or property
    /// name).
    pub by_name: Option<String>,
    /// Annotations on the point outside the inert and DI-understood lists, as written; on a
    /// Lombok-generated parameter, every non-inert annotation of the field (whether Lombok
    /// copies it depends on `lombok.config`).
    pub annotations: Vec<String>,
    /// The parameter, field or (Lombok) field declaration.
    pub span: Span,
}

/// Which roots a scan or enable annotation sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScanKind {
    /// `@SpringBootApplication`, `@ComponentScan`: component scan roots (D8).
    Components,
    /// `@Enable*Repositories` (Spring Data): repository roots (S8).
    Repositories,
    /// `@EnableFeignClients`: Feign roots (S8).
    Feign,
}

/// A scan or enable annotation on a class (D8, S2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanDecl {
    pub class: usize,
    pub kind: ScanKind,
    /// The literal root packages, or the annotated class's package when none is given.
    pub packages: Vec<String>,
    /// An attribute outside the read set, or a value that is not a literal (`scan_unread`).
    pub unread: bool,
}

/// A DI-surface finding (S6 gate D): what was found, where.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SurfaceFinding {
    pub what: String,
    pub span: Span,
}

/// Everything DI-related one Java file declares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileDi {
    pub beans: Vec<Bean>,
    pub points: Vec<InjectionPoint>,
    pub scans: Vec<ScanDecl>,
    /// Bean classes with several constructors and not exactly one marked (D9): no constructor
    /// points; counted as one point of bucket `unresolved`, detail `constructor_ambiguous`. The
    /// span is the class name's.
    pub ambiguous_constructors: Vec<(usize, Span)>,
    /// Classes that are not beans but carry an annotation outside the inert and DI-understood
    /// lists: a meta-annotation declared elsewhere may make them beans (gate
    /// `annotation_unread`).
    pub unread_classes: Vec<usize>,
    /// Gate D findings of this file.
    pub surface: Vec<SurfaceFinding>,
}

/// The annotations whose presence makes no framework call and registers no bean (contract M4b
/// S12 (c)), by package (`None`: any package). Shared by DI, RTA and orphan eligibility.
const INERT: &[(Option<&str>, &str)] = &[
    (Some("java.lang"), "Override"),
    (Some("java.lang"), "Deprecated"),
    (Some("java.lang"), "SuppressWarnings"),
    (Some("java.lang"), "FunctionalInterface"),
    (Some("java.lang"), "SafeVarargs"),
    (
        Some("org.springframework.transaction.annotation"),
        "Transactional",
    ),
    (Some("jakarta.transaction"), "Transactional"),
    (Some("javax.transaction"), "Transactional"),
    (
        Some("org.springframework.validation.annotation"),
        "Validated",
    ),
    (Some("jakarta.validation"), "Valid"),
    (Some("javax.validation"), "Valid"),
    (None, "NonNull"),
    (None, "Nullable"),
    (Some(LOMBOK), "Getter"),
    (Some(LOMBOK), "Setter"),
    (Some(LOMBOK), "Data"),
    (Some(LOMBOK), "ToString"),
    (Some(LOMBOK), "EqualsAndHashCode"),
    (Some(LOMBOK), "NoArgsConstructor"),
    (Some(LOMBOK), "AllArgsConstructor"),
    (Some(LOMBOK), "RequiredArgsConstructor"),
    (Some(LOMBOK), "Builder"),
    (Some("lombok.extern.slf4j"), "Slf4j"),
    (Some("lombok.extern.log4j"), "Log4j2"),
    (Some("lombok.extern.apachecommons"), "CommonsLog"),
    (Some("lombok.extern.java"), "Log"),
];

fn simple(name: &str) -> &str {
    name.rsplit('.').next().unwrap_or(name)
}

/// An inert annotation (S12 (c)): its package imported or written, `java.lang` ones also by
/// their simple name (implicitly imported), `NonNull`/`Nullable` from any package.
pub fn is_inert(facts: &FileFacts, d: &Decoration) -> bool {
    INERT.iter().any(|(package, name)| match package {
        None => simple(&d.name) == *name,
        Some("java.lang") => d.name == *name || d.name == format!("java.lang.{name}"),
        Some(package) => is_annotation(facts, d, package, name),
    })
}

fn is_any(facts: &FileFacts, d: &Decoration, packages: &[&str], name: &str) -> bool {
    packages.iter().any(|p| is_annotation(facts, d, p, name))
}

fn stereotype(facts: &FileFacts, d: &Decoration) -> bool {
    STEREOTYPES
        .iter()
        .any(|(p, n)| is_annotation(facts, d, p, n))
}

fn is_conditional(facts: &FileFacts, d: &Decoration) -> bool {
    let name = simple(&d.name);
    is_annotation(facts, d, CONTEXT, "Profile")
        || is_annotation(facts, d, CONTEXT, "Conditional")
        || (name.starts_with("ConditionalOn") && is_annotation(facts, d, BOOT_CONDITION, name))
}

/// A Spring Data `@Enable…Repositories` annotation of any store.
fn is_enable_repositories(facts: &FileFacts, d: &Decoration) -> bool {
    let name = simple(&d.name);
    if !(name.starts_with("Enable") && name.ends_with("Repositories")) {
        return false;
    }
    if d.name.starts_with("org.springframework.data.") {
        return true;
    }
    d.name == name
        && facts.imports.iter().any(|i| {
            !i.is_static
                && i.module.starts_with("org.springframework.data.")
                && (i.wildcard || i.names.iter().any(|n| n.name == name && n.local == name))
        })
}

/// Annotations DI reads or that register nothing: a class-level or method-level annotation
/// outside this set is recorded in `annotations`.
fn understood(facts: &FileFacts, d: &Decoration) -> bool {
    const CONTEXT_NAMES: &[&str] = &[
        "Bean",
        "Primary",
        "Profile",
        "Conditional",
        "Lazy",
        "Scope",
        "DependsOn",
        "Import",
        "ComponentScan",
        "Configuration",
    ];
    const LOMBOK_NAMES: &[&str] = &["Value", "Data"];
    is_inert(facts, d)
        || stereotype(facts, d)
        || is_conditional(facts, d)
        || CONTEXT_NAMES
            .iter()
            .any(|n| is_annotation(facts, d, CONTEXT, n))
        || ["Autowired", "Qualifier", "Value"]
            .iter()
            .any(|n| is_annotation(facts, d, BEANS_ANNOTATION, n))
        || is_annotation(facts, d, CORE_ANNOTATION, "Order")
        || is_any(facts, d, INJECT, "Inject")
        || is_annotation(facts, d, GUICE, "Inject")
        || is_annotation(facts, d, GUICE, "Singleton")
        || is_annotation(facts, d, GUICE, "Provides")
        || is_annotation(facts, d, GUICE_NAME, "Named")
        || is_annotation(facts, d, DAGGER, "Module")
        || is_annotation(facts, d, DAGGER, "Provides")
        || is_annotation(facts, d, DAGGER, "Binds")
        || is_annotation(facts, d, DAGGER, "Component")
        || is_annotation(facts, d, NEST, "Module")
        || is_annotation(facts, d, NEST, "Injectable")
        || is_annotation(facts, d, NEST, "Controller")
        || is_annotation(facts, d, NEST, "Inject")
        || is_any(facts, d, RESOURCE, "Resource")
        || LOMBOK_NAMES
            .iter()
            .any(|n| is_annotation(facts, d, LOMBOK, n))
        || is_any(facts, d, FEIGN, "FeignClient")
        || is_any(facts, d, FEIGN, "EnableFeignClients")
        || is_enable_repositories(facts, d)
        || is_annotation(facts, d, DATA_REPOSITORY, "NoRepositoryBean")
        // Request mappings register handlers, not beans.
        || is_web(facts, d)
}

/// An annotation of Spring MVC's `org.springframework.web.bind.annotation` package.
fn is_web(facts: &FileFacts, d: &Decoration) -> bool {
    let name = simple(&d.name);
    d.name == format!("{WEB}.{name}")
        || (d.name == name
            && facts.imports.iter().any(|i| {
                !i.is_static
                    && i.module == WEB
                    && (i.wildcard || i.names.iter().any(|n| n.name == name && n.local == name))
            }))
}

/// `Introspector.decapitalize`: the first letter lower-cased, unless the first two letters are
/// both upper case (`URLParser` stays).
pub fn decapitalize(name: &str) -> String {
    let mut chars = name.chars();
    match (chars.next(), chars.next()) {
        (Some(a), Some(b)) if a.is_uppercase() && b.is_uppercase() => name.to_owned(),
        (Some(a), _) => a.to_lowercase().chain(name.chars().skip(1)).collect(),
        (None, _) => String::new(),
    }
}

/// A literal string argument: positional or `value` (and `keys`), with array items.
fn literals(d: &Decoration, keys: &[&str]) -> Result<Vec<String>, ()> {
    let mut out = Vec::new();
    for a in d
        .args
        .iter()
        .filter(|a| a.key.as_deref().is_none_or(|k| keys.contains(&k)))
    {
        if a.items.is_empty() {
            if !a.literal {
                return Err(());
            }
            out.push(a.value.clone());
        } else {
            for item in &a.items {
                if !item.literal {
                    return Err(());
                }
                out.push(item.value.clone());
            }
        }
    }
    Ok(out)
}

/// The literal of `@Qualifier`, or of JSR-330's `@Named`, which Spring reads as a qualifier
/// (evaluator pass 1, Y6).
fn qualifier_of(facts: &FileFacts, decorations: &[Decoration]) -> Option<String> {
    decorations
        .iter()
        .find(|d| is_qualifier(facts, d))
        .and_then(|d| literals(d, &["value"]).ok())
        .and_then(|v| v.into_iter().next())
}

fn is_qualifier(facts: &FileFacts, d: &Decoration) -> bool {
    is_annotation(facts, d, BEANS_ANNOTATION, "Qualifier")
        || is_any(facts, d, INJECT, "Named")
        || is_annotation(facts, d, GUICE_NAME, "Named")
        || is_any(facts, d, &[NEST], "Inject")
}

fn has(facts: &FileFacts, decorations: &[Decoration], package: &str, name: &str) -> bool {
    decorations
        .iter()
        .any(|d| is_annotation(facts, d, package, name))
}

fn foreign(facts: &FileFacts, decorations: &[Decoration]) -> Vec<String> {
    let mut out: Vec<String> = decorations
        .iter()
        .filter(|d| !understood(facts, d))
        .map(|d| d.name.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// A type written in a file is `package.simple`: qualified, or imported by name or wildcard.
fn is_type(facts: &FileFacts, written: &str, packages: &[&str], simple_name: &str) -> bool {
    packages.iter().any(|package| {
        written == format!("{package}.{simple_name}")
            || (written == simple_name
                && facts.imports.iter().any(|i| {
                    !i.is_static
                        && i.module == *package
                        && (i.wildcard
                            || i.names
                                .iter()
                                .any(|n| n.name == simple_name && n.local == simple_name))
                }))
    })
}

/// Types never injected as beans (S3): `String` and boxed primitives.
const SCALARS: &[&str] = &[
    "String",
    "Integer",
    "Long",
    "Short",
    "Byte",
    "Character",
    "Boolean",
    "Double",
    "Float",
];

fn is_scalar(written: &str) -> bool {
    let s = written.strip_prefix("java.lang.").unwrap_or(written);
    SCALARS.contains(&s)
}

/// The class a definition belongs to: its innermost enclosing class or interface.
fn class_of(defs: &[Definition], mut i: usize) -> Option<usize> {
    while let Some(p) = defs[i].parent {
        if matches!(defs[p].kind, DefKind::Class | DefKind::Interface) {
            return Some(p);
        }
        i = p;
    }
    None
}

fn is_constructor(defs: &[Definition], i: usize, class: usize) -> bool {
    let d = &defs[i];
    d.kind == DefKind::Method
        && d.parent == Some(class)
        && d.return_type.is_none()
        && d.name == defs[class].name
}

/// The DI facts of one file (S2, S3, S6 gate D). `repository_bases` is M2b's
/// [`crate::repository_bases`] over every Java file; `path` and `lombok`
/// locate the nearest `lombok.config` chain for Lombok-copied qualifiers
/// (contract M6c S8).
pub fn di(
    language: Language,
    facts: &FileFacts,
    repository_bases: &BTreeMap<String, usize>,
    path: &str,
    lombok: &LombokConfigs,
) -> FileDi {
    let mut out = FileDi::default();
    if language != Language::Java {
        return out;
    }
    let defs = &facts.definitions;
    let scopes = Scopes::new(facts);
    let scoped = |i: usize| scopes.of(i);
    let params = |i: usize| scopes.params(i);

    let beans: BTreeSet<usize> = (0..defs.len())
        .filter(|&i| is_bean_class(facts, i))
        .collect();

    for (i, def) in defs.iter().enumerate() {
        match def.kind {
            DefKind::Class => {
                let is_bean = beans.contains(&i);
                if is_bean {
                    out.beans.push(class_bean(facts, i));
                    constructor_points(facts, i, &scopes, path, lombok, &mut out);
                } else if !foreign(facts, &def.decorations).is_empty()
                    || def.decorations.iter().any(|d| stereotype(facts, d))
                {
                    out.unread_classes.push(i);
                }
                for b in scoped(i).iter().filter(|b| b.member) {
                    if let Some(p) = annotated_field(facts, i, is_bean, b) {
                        out.points.push(p);
                    }
                }
                scans(facts, i, &mut out);
            }
            DefKind::Interface => {
                if let Some(bean) =
                    repository(facts, i, repository_bases).or_else(|| feign_client(facts, i))
                {
                    out.beans.push(bean);
                }
                scans(facts, i, &mut out);
            }
            DefKind::Method => {
                let Some(class) = class_of(defs, i) else {
                    continue;
                };
                let is_bean = beans.contains(&class);
                if def.return_type.is_some()
                    && has_any_inject(facts, &def.decorations)
                    && def.parent == Some(class)
                {
                    let resource = resource_name(facts, &def.decorations).or_else(|| {
                        has_any(facts, &def.decorations, RESOURCE, "Resource")
                            .then(|| property(&def.name))
                    });
                    for b in params(i) {
                        if let Some(mut p) = point(facts, b, class, is_bean, Form::Setter) {
                            p.by_name.clone_from(&resource);
                            out.points.push(p);
                        }
                    }
                }
                if is_bean
                    && def.parent == Some(class)
                    && has(facts, &def.decorations, CONTEXT, "Bean")
                {
                    out.beans.push(bean_method(facts, i, class));
                    for b in params(i) {
                        out.points
                            .extend(point(facts, b, i, true, Form::BeanParameter));
                    }
                }
                // Contract M6j S2: `@Provides` methods in module hosts are beans even
                // though the module class is not one (the Spring `@Bean`-in-bean case
                // above stays exclusive); Dagger `@Binds` methods bind their return type.
                if def.parent == Some(class)
                    && def.return_type.is_some()
                    && !(is_bean && has(facts, &def.decorations, CONTEXT, "Bean"))
                {
                    if def.decorations.iter().any(|d| bean_factory(facts, d))
                        && provides_host(facts, class)
                    {
                        out.beans.push(bean_method(facts, i, class));
                        for b in params(i) {
                            out.points
                                .extend(point(facts, b, i, true, Form::BeanParameter));
                        }
                    } else if def
                        .decorations
                        .iter()
                        .any(|d| is_annotation(facts, d, DAGGER, "Binds"))
                    {
                        out.beans.push(binds_bean(facts, i));
                    }
                }
            }
            _ => {}
        }
    }
    bind_chains(facts, &mut out);
    surface_of_facts(facts, &mut out.surface);
    out
}

/// NestJS dependency-injection facts per file (contract M6j S3): `@Injectable()` and
/// `@Controller()` classes as beans, same-file `providers`/`controllers` entries naming
/// undecorated classes as beans, constructor and `@Inject` field points. Custom providers
/// (`useClass`/`useValue`/`useFactory`) and dynamic modules stay unread (residue).
pub fn nest_di(facts: &FileFacts) -> FileDi {
    let mut out = FileDi::default();
    let defs = &facts.definitions;
    // Bare class names listed under `providers`/`controllers` of any `@Module`.
    let mut provided: BTreeSet<&str> = BTreeSet::new();
    for def in defs {
        for d in &def.decorations {
            if !is_annotation(facts, d, NEST, "Module") {
                continue;
            }
            for a in &d.args {
                if !matches!(a.key.as_deref(), Some("providers" | "controllers")) {
                    continue;
                }
                for item in &a.items {
                    // Literals are tokens, objects are custom providers: only bare class
                    // names declare beans.
                    if !item.literal && !item.value.is_empty() && !item.value.contains('.') {
                        provided.insert(item.value.as_str());
                    }
                }
            }
        }
    }
    let scopes = Scopes::new(facts);
    for (i, def) in defs.iter().enumerate() {
        if def.kind != DefKind::Class {
            continue;
        }
        let decorated = def.decorations.iter().any(|d| {
            is_annotation(facts, d, NEST, "Injectable")
                || is_annotation(facts, d, NEST, "Controller")
        });
        let is_bean = decorated || provided.contains(def.name.as_str());
        if is_bean {
            out.beans.push(nest_bean(facts, i));
            nest_constructor_points(facts, i, &scopes, &mut out);
        } else if !foreign(facts, &def.decorations).is_empty() {
            out.unread_classes.push(i);
        }
        for b in scopes.of(i).iter().filter(|b| b.member) {
            // Constructor parameters ride the constructor rule above (they are member
            // bindings in TypeScript); plain `@Inject` fields land here.
            if b.parameter.is_some() {
                continue;
            }
            if let Some(p) = annotated_field(facts, i, is_bean, b) {
                out.points.push(p);
            }
        }
    }
    surface_of_facts(facts, &mut out.surface);
    out
}

/// A NestJS bean record (contract M6j S3): named by the decapitalised class name.
fn nest_bean(facts: &FileFacts, i: usize) -> Bean {
    let def = &facts.definitions[i];
    Bean {
        definition: i,
        origin: Origin::Injectable,
        type_name: def.name.clone(),
        generic: false,
        names: vec![decapitalize(&short_name(&facts.definitions, i))],
        qualifiers: qualifier_of(facts, &def.decorations).into_iter().collect(),
        primary: false,
        conditional: false,
        annotations: foreign(facts, &def.decorations),
        created: Vec::new(),
        extends: Vec::new(),
    }
}

/// The constructor points of a NestJS bean class (contract M6j S3): Nest injects every
/// constructor parameter by type. TypeScript constructors are not definitions, so the rule
/// reads member bindings carrying a parameter position scoped to the class; overload
/// signatures would each contribute (residue — Nest beans do not overload constructors).
/// Constructor parameters are skipped by the field loop below, so no point doubles.
fn nest_constructor_points(facts: &FileFacts, class: usize, scopes: &Scopes<'_>, out: &mut FileDi) {
    let mut params: Vec<&Binding> = scopes
        .of(class)
        .iter()
        .copied()
        .filter(|b| b.member && b.parameter.is_some())
        .collect();
    params.sort_by_key(|b| b.parameter);
    for b in params {
        out.points
            .extend(point(facts, b, class, true, Form::Constructor));
    }
}

/// A span strictly inside another (row-major; equal spans never contain).
fn span_contains(outer: Span, inner: Span) -> bool {
    (outer[0], outer[1]) <= (inner[0], inner[1])
        && (inner[2], inner[3]) <= (outer[2], outer[3])
        && outer != inner
}

/// A template argument decoding to one literal, else `None`.
fn template_literal(arg: &Arg) -> Option<String> {
    match arg {
        Arg::Template { template } => {
            let mut text = String::new();
            for part in &template.parts {
                match part {
                    TemplatePart::Lit { text: t } => text.push_str(t),
                    TemplatePart::Hole { .. } => return None,
                }
            }
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    }
}

/// Kotlin `beans { }` registrations (contract M6j S5): `bean` calls inside a `beans`
/// call span become `Binds` beans anchored at their enclosing definition (top-level
/// `val` forms have none and stay unread); `ref()` calls inside a bean lambda become
/// points (untyped; qualified only by a literal). Custom scopes and conditionals stay
/// unread.
pub fn kotlin_di(facts: &FileFacts) -> FileDi {
    let mut out = FileDi::default();
    let beans_roots: Vec<Span> = facts
        .chains
        .iter()
        .filter(|c| c.root.is_none() && matches!(c.links.as_slice(), [l] if l.name == "beans"))
        .map(|c| c.span)
        .collect();
    if beans_roots.is_empty() {
        return out;
    }
    // Bean links with their spans: `bean` calls directly inside a `beans` block. A
    // trailing lambda rides a second, nameless link; the chain span (the whole call)
    // is the container its refs and constructed calls live in.
    struct BeanLink {
        container: Span,
        type_args: Vec<String>,
        args: Vec<Arg>,
        enclosing: Option<usize>,
    }
    let mut links: Vec<BeanLink> = Vec::new();
    for chain in &facts.chains {
        if chain.root.is_some() {
            continue;
        }
        let Some(first) = chain.links.first() else {
            continue;
        };
        if first.name != "bean" {
            continue;
        }
        if !beans_roots.iter().any(|r| span_contains(*r, chain.span)) {
            continue;
        }
        links.push(BeanLink {
            container: chain.span,
            type_args: first.type_args.clone(),
            args: first.args.clone(),
            enclosing: chain.enclosing,
        });
    }
    // Constructed types: the single non-`ref` bare call inside each bean container,
    // attributed innermost-first (a nested bean's calls belong to it, not the outer).
    // `ref()` calls likewise; each bean numbers its own in span order.
    for (n, link) in links.iter().enumerate() {
        let inner = |span: Span| {
            !links
                .iter()
                .enumerate()
                .any(|(m, o)| m != n && span_contains(o.container, span))
        };
        let mut found = facts
            .chains
            .iter()
            .filter(|c| {
                c.root.is_none()
                    && matches!(c.links.as_slice(), [l] if l.name != "ref")
                    && span_contains(link.container, c.span)
                    && inner(c.span)
            })
            .map(|c| c.links[0].name.clone());
        let constructed = match (found.next(), found.next()) {
            (Some(t), None) => Some(t),
            _ => None,
        };
        let type_name = link
            .type_args
            .first()
            .cloned()
            .or(constructed)
            .unwrap_or_default();
        if type_name.is_empty() {
            continue;
        }
        let Some(enc) = link.enclosing else { continue };
        let name = link
            .args
            .first()
            .and_then(template_literal)
            .unwrap_or_else(|| decapitalize(&type_name));
        out.beans.push(Bean {
            definition: enc,
            origin: Origin::Binds,
            type_name,
            generic: false,
            names: vec![name.clone()],
            qualifiers: vec![name],
            primary: false,
            conditional: false,
            annotations: Vec::new(),
            created: Vec::new(),
            extends: Vec::new(),
        });
        let mut refs: Vec<(Span, Option<String>)> = facts
            .chains
            .iter()
            .filter(|c| {
                c.root.is_none()
                    && matches!(c.links.as_slice(), [l] if l.name == "ref")
                    && span_contains(link.container, c.span)
                    && inner(c.span)
            })
            .map(|c| {
                let qualifier = c.links[0].args.first().and_then(template_literal);
                (c.span, qualifier)
            })
            .collect();
        refs.sort();
        for (k, (span, qualifier)) in refs.into_iter().enumerate() {
            out.points.push(InjectionPoint {
                owner: enc,
                owner_is_bean: false,
                form: Form::Constructor,
                name: format!("#{k}"),
                type_name: String::new(),
                element: String::new(),
                wrapper: None,
                generic: false,
                type_args: Vec::new(),
                qualifier,
                copied_qualifier: false,
                by_name: None,
                annotations: Vec::new(),
                span,
            });
        }
    }
    out
}

fn has_any(facts: &FileFacts, decorations: &[Decoration], packages: &[&str], name: &str) -> bool {
    decorations.iter().any(|d| is_any(facts, d, packages, name))
}

/// `@Autowired`, `@Inject` (JSR-330, Guice's own or NestJS) or `@Resource`.
fn has_any_inject(facts: &FileFacts, decorations: &[Decoration]) -> bool {
    has(facts, decorations, BEANS_ANNOTATION, "Autowired")
        || has_any(facts, decorations, INJECT, "Inject")
        || has(facts, decorations, GUICE, "Inject")
        || has_any(facts, decorations, &[NEST], "Inject")
        || has_any(facts, decorations, RESOURCE, "Resource")
}

/// `@Resource(name = "x")`'s literal name.
fn resource_name(facts: &FileFacts, decorations: &[Decoration]) -> Option<String> {
    decorations
        .iter()
        .find(|d| is_any(facts, d, RESOURCE, "Resource"))
        .and_then(|d| d.args.iter().find(|a| a.key.as_deref() == Some("name")))
        .filter(|a| a.literal && !a.value.is_empty())
        .map(|a| a.value.clone())
}

/// The bean property a setter sets: `setFoo` → `foo`.
fn property(method: &str) -> String {
    decapitalize(method.strip_prefix("set").unwrap_or(method))
}

/// A file carrying Guice or Dagger markers: any import of their packages (contract M6j
/// S2). Class-bean rules beyond `@Singleton` apply only here, so Spring files (which never
/// import these packages) read exactly as before.
fn framework_file(facts: &FileFacts) -> bool {
    facts.imports.iter().any(|i| {
        !i.is_static
            && (i.module == GUICE
                || i.module.starts_with("com.google.inject.")
                || i.module == DAGGER
                || i.module.starts_with("dagger."))
    })
}

/// A `@Singleton` class of Guice's own package (contract M6j S2).
fn singleton_class(facts: &FileFacts, i: usize) -> bool {
    facts.definitions[i]
        .decorations
        .iter()
        .any(|d| is_annotation(facts, d, GUICE, "Singleton"))
}

/// A class with an `@Inject` constructor (contract M6j S2): Guice's own `@Inject` always
/// counts; JSR-330 `@Inject` counts in a framework file. The constructor may be declared
/// (a method definition) — generated ones do not exist here (no Lombok for Guice).
fn injectable_class(facts: &FileFacts, i: usize) -> bool {
    let defs = &facts.definitions;
    (0..defs.len()).any(|c| {
        defs[c].kind == DefKind::Method
            && defs[c].parent == Some(i)
            && is_constructor(defs, c, i)
            && (defs[c]
                .decorations
                .iter()
                .any(|d| is_annotation(facts, d, GUICE, "Inject"))
                || (framework_file(facts)
                    && defs[c]
                        .decorations
                        .iter()
                        .any(|d| has_any_inject(facts, std::slice::from_ref(d)))))
    })
}

/// A bean-producing method annotation: Spring's `@Bean` or Guice/Dagger `@Provides`
/// (contract M6j S2).
fn bean_factory(facts: &FileFacts, d: &Decoration) -> bool {
    is_annotation(facts, d, CONTEXT, "Bean")
        || is_annotation(facts, d, GUICE, "Provides")
        || is_annotation(facts, d, DAGGER, "Provides")
}

/// A class hosting `@Provides` methods (contract M6j S2): a Dagger `@Module`, or a Guice
/// module by convention (a supertype whose simple name ends in `Module`). The method's
/// `@Provides` stays import-gated, so a loose host only reads a real provides method.
fn provides_host(facts: &FileFacts, class: usize) -> bool {
    let defs = &facts.definitions;
    defs[class]
        .decorations
        .iter()
        .any(|d| is_annotation(facts, d, DAGGER, "Module"))
        || defs[class].supertypes.iter().any(|s| {
            s.name
                .rsplit('.')
                .next()
                .unwrap_or(&s.name)
                .ends_with("Module")
        })
}

/// The single class literal of `bind`/`to` arguments, else `None`.
fn single_type(args: &[Arg]) -> Option<String> {
    match args {
        [Arg::Type { text, .. }] => Some(text.clone()),
        _ => None,
    }
}

/// The qualifier literal of an `annotatedWith` argument: the single `named()` chain at the
/// argument span decoding to one literal template, else `None`.
fn qualifier_argument(facts: &FileFacts, arg: &Arg) -> Option<String> {
    let chain = facts.chains.iter().find(|c| c.span == arg.span())?;
    let [link] = chain.links.as_slice() else {
        return None;
    };
    if link.name != "named" || link.args.len() != 1 {
        return None;
    }
    template_literal(&link.args[0])
}

/// Guice `bind(X).to(Y)` pairs as `Binds` beans (contract M6j S2): rootless chains whose
/// first link is `bind` with one class literal and whose last link is `to` with one; the
/// implementation rides `created` like a `@Bean` body's (engine-tested). Middle
/// `annotatedWith` links contribute the qualifier when every one resolves to a literal;
/// anything else in the middle, or an unresolvable qualifier, skips the pair (an
/// unqualified point must not match a qualified binding).
fn bind_chains(facts: &FileFacts, out: &mut FileDi) {
    for chain in &facts.chains {
        if chain.root.is_some() {
            continue;
        }
        let (first, middle, last) = match chain.links.as_slice() {
            [first, middle @ .., last] if first.name == "bind" && last.name == "to" => {
                (first, middle, last)
            }
            _ => continue,
        };
        let (Some(x), Some(y)) = (single_type(&first.args), single_type(&last.args)) else {
            continue;
        };
        let Some(enc) = chain.enclosing else { continue };
        let mut qualifier = None;
        let mut ok = true;
        for m in middle {
            if m.name != "annotatedWith" || m.args.len() != 1 {
                ok = false;
                break;
            }
            match qualifier_argument(facts, &m.args[0]) {
                Some(q) if qualifier.is_none() => qualifier = Some(q),
                _ => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        out.beans.push(Bean {
            definition: enc,
            origin: Origin::Binds,
            type_name: x,
            generic: false,
            names: Vec::new(),
            qualifiers: qualifier.into_iter().collect(),
            primary: false,
            conditional: false,
            annotations: Vec::new(),
            created: vec![y],
            extends: Vec::new(),
        });
    }
}

/// A Dagger `@Binds` method as a bean (contract M6j S2): typed by its return type, named
/// by nothing (callers never address the method), qualified by its `@Named`.
fn binds_bean(facts: &FileFacts, i: usize) -> Bean {
    let def = &facts.definitions[i];
    let (type_name, generic) = def
        .return_type
        .clone()
        .unwrap_or_default()
        .split_once('<')
        .map(|(head, _)| (head.trim().to_owned(), true))
        .unwrap_or_else(|| {
            (
                def.return_type
                    .clone()
                    .unwrap_or_default()
                    .trim()
                    .to_owned(),
                false,
            )
        });
    Bean {
        definition: i,
        origin: Origin::Binds,
        type_name,
        generic,
        names: Vec::new(),
        qualifiers: qualifier_of(facts, &def.decorations).into_iter().collect(),
        primary: false,
        conditional: def.decorations.iter().any(|d| is_conditional(facts, d)),
        annotations: foreign(facts, &def.decorations),
        created: Vec::new(),
        extends: Vec::new(),
    }
}

/// A concrete, independent class with a stereotype (S2): not abstract, top-level or a static
/// nested class (or nested in an interface, implicitly static); or a Guice/Dagger bean
/// class (contract M6j S2).
fn is_bean_class(facts: &FileFacts, i: usize) -> bool {
    let defs = &facts.definitions;
    let d = &defs[i];
    d.kind == DefKind::Class
        && !d.modifiers.iter().any(|m| m == "abstract")
        && (d.decorations.iter().any(|a| stereotype(facts, a))
            || singleton_class(facts, i)
            || injectable_class(facts, i))
        && d.parent.is_none_or(|p| {
            defs[p].kind == DefKind::Interface || d.modifiers.iter().any(|m| m == "static")
        })
}

/// The class's name for bean naming: nested classes as `Outer.Inner`.
fn short_name(defs: &[Definition], i: usize) -> String {
    let mut parts = vec![defs[i].name.as_str()];
    let mut cur = defs[i].parent;
    while let Some(p) = cur {
        if matches!(defs[p].kind, DefKind::Class | DefKind::Interface) {
            parts.push(defs[p].name.as_str());
        }
        cur = defs[p].parent;
    }
    parts.reverse();
    parts.join(".")
}

/// The bean record for a bean class (contract M6j S2): a stereotype keeps
/// `Origin::Stereotype`; otherwise `@Singleton` beats injectable-by-constructor.
fn class_bean(facts: &FileFacts, i: usize) -> Bean {
    let defs = &facts.definitions;
    if defs[i].decorations.iter().any(|a| stereotype(facts, a)) {
        stereotype_bean(facts, i, Origin::Stereotype)
    } else if singleton_class(facts, i) {
        stereotype_bean(facts, i, Origin::Singleton)
    } else {
        injectable_bean(facts, i)
    }
}

/// A concrete class with an `@Inject` constructor as a bean (contract M6j S2): named by
/// the decapitalised class name, qualified by its `@Named`.
fn injectable_bean(facts: &FileFacts, i: usize) -> Bean {
    let def = &facts.definitions[i];
    Bean {
        definition: i,
        origin: Origin::Injectable,
        type_name: def.name.clone(),
        generic: false,
        names: vec![decapitalize(&short_name(&facts.definitions, i))],
        qualifiers: qualifier_of(facts, &def.decorations).into_iter().collect(),
        primary: false,
        conditional: def.decorations.iter().any(|d| is_conditional(facts, d)),
        annotations: foreign(facts, &def.decorations),
        created: Vec::new(),
        extends: Vec::new(),
    }
}

fn stereotype_bean(facts: &FileFacts, i: usize, origin: Origin) -> Bean {
    let def = &facts.definitions[i];
    let named = def
        .decorations
        .iter()
        .filter(|d| stereotype(facts, d))
        .filter_map(|d| literals(d, &["value"]).ok())
        .flatten()
        .find(|v| !v.is_empty());
    Bean {
        definition: i,
        origin,
        type_name: def.name.clone(),
        generic: false,
        names: vec![named.unwrap_or_else(|| decapitalize(&short_name(&facts.definitions, i)))],
        qualifiers: qualifier_of(facts, &def.decorations).into_iter().collect(),
        primary: has(facts, &def.decorations, CONTEXT, "Primary"),
        conditional: def.decorations.iter().any(|d| is_conditional(facts, d)),
        annotations: foreign(facts, &def.decorations),
        created: Vec::new(),
        extends: Vec::new(),
    }
}

fn bean_method(facts: &FileFacts, i: usize, class: usize) -> Bean {
    let defs = &facts.definitions;
    let def = &defs[i];
    let written = def.return_type.clone().unwrap_or_default();
    let (type_name, generic) = match written.split_once('<') {
        Some((head, _)) => (head.trim().to_owned(), true),
        None => (written.trim().to_owned(), false),
    };
    let names = def
        .decorations
        .iter()
        .find(|d| bean_factory(facts, d))
        .and_then(|d| literals(d, &["value", "name"]).ok())
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| vec![def.name.clone()]);
    let mut created: Vec<String> = facts
        .references
        .iter()
        .filter(|r| r.kind == rb_syntax::RefKind::Class && r.enclosing == Some(i))
        .map(|r| r.name.clone())
        .collect();
    created.sort();
    created.dedup();
    let class_decorations = &defs[class].decorations;
    Bean {
        definition: i,
        origin: Origin::BeanMethod,
        type_name,
        generic,
        names,
        qualifiers: qualifier_of(facts, &def.decorations).into_iter().collect(),
        primary: has(facts, &def.decorations, CONTEXT, "Primary"),
        conditional: def
            .decorations
            .iter()
            .chain(class_decorations)
            .any(|d| is_conditional(facts, d)),
        annotations: foreign(facts, &def.decorations),
        created,
        extends: Vec::new(),
    }
}

/// A Spring Data repository interface (S2, D3): no type parameters of its own, not
/// `@NoRepositoryBean`, and a supertype that is a repository base (M2b's resolver), or a
/// store-specific base imported from `org.springframework.data.*` whose name ends in
/// `Repository`.
fn repository(
    facts: &FileFacts,
    i: usize,
    repository_bases: &BTreeMap<String, usize>,
) -> Option<Bean> {
    let def = &facts.definitions[i];
    if !def.type_params.is_empty()
        || has(facts, &def.decorations, DATA_REPOSITORY, "NoRepositoryBean")
    {
        return None;
    }
    let store_base = |written: &str| {
        let name = simple(written);
        name.ends_with("Repository")
            && (written.starts_with("org.springframework.data.")
                || (written == name
                    && facts.imports.iter().any(|imp| {
                        !imp.is_static
                            && imp.module.starts_with("org.springframework.data")
                            && (imp.wildcard
                                || imp.names.iter().any(|n| n.name == name && n.local == name))
                    })))
    };
    def.supertypes
        .iter()
        .any(|s| repository_bases.contains_key(simple(&s.name)) || store_base(&s.name))
        .then(|| Bean {
            definition: i,
            origin: Origin::SpringData,
            type_name: def.name.clone(),
            generic: false,
            names: vec![decapitalize(&def.name)],
            qualifiers: Vec::new(),
            primary: false,
            conditional: def.decorations.iter().any(|d| is_conditional(facts, d)),
            annotations: foreign(facts, &def.decorations),
            created: Vec::new(),
            extends: def
                .supertypes
                .iter()
                .map(|s| simple(&s.name).to_owned())
                .collect(),
        })
}

/// A `@FeignClient` interface (S2): a bean only when the service enables Feign (indexer).
fn feign_client(facts: &FileFacts, i: usize) -> Option<Bean> {
    let def = &facts.definitions[i];
    let d = def
        .decorations
        .iter()
        .find(|d| is_any(facts, d, FEIGN, "FeignClient"))?;
    let arg = |key: &str| -> Option<&DecorationArg> {
        d.args.iter().find(|a| match &a.key {
            Some(k) => k == key,
            None => key == "value",
        })
    };
    let qualifiers: Vec<String> = arg("qualifiers")
        .map(|a| {
            if a.items.is_empty() {
                vec![a.value.clone()]
            } else {
                a.items.iter().map(|i| i.value.clone()).collect()
            }
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|q| !q.is_empty())
        .collect();
    let context = ["contextId", "value", "name"]
        .iter()
        .find_map(|k| arg(k).filter(|a| a.literal && !a.value.is_empty()))
        .map(|a| a.value.clone());
    let names = if qualifiers.is_empty() {
        context
            .map(|c| format!("{c}FeignClient"))
            .into_iter()
            .collect()
    } else {
        qualifiers.clone()
    };
    Some(Bean {
        definition: i,
        origin: Origin::Feign,
        type_name: def.name.clone(),
        generic: false,
        names,
        qualifiers,
        primary: !arg("primary").is_some_and(|a| a.value == "false"),
        conditional: def.decorations.iter().any(|d| is_conditional(facts, d)),
        annotations: foreign(facts, &def.decorations),
        created: Vec::new(),
        extends: Vec::new(),
    })
}

/// An injection point from a parameter or field binding, or `None` when it is excluded (S3):
/// `@Value`, `String`, boxed primitives and primitives (no type name).
fn point(
    facts: &FileFacts,
    b: &Binding,
    owner: usize,
    owner_is_bean: bool,
    form: Form,
) -> Option<InjectionPoint> {
    if has(facts, &b.decorations, BEANS_ANNOTATION, "Value") {
        return None;
    }
    let (wrapper, element, generic) = unwrap(facts, b)?;
    if element.is_empty() || is_scalar(&element) {
        return None;
    }
    let annotations: Vec<String> = b
        .decorations
        .iter()
        .filter(|d| {
            !is_inert(facts, d)
                && !has_any_inject(facts, std::slice::from_ref(*d))
                && !is_qualifier(facts, d)
                && !is_annotation(facts, d, CONTEXT, "Lazy")
        })
        .map(|d| d.name.clone())
        .collect();
    // Exact-agreement arguments (contract M6c S9): the declared args as
    // simple names, wildcards marked (nesting empties the whole list, so
    // nested points never agree). The indexer refuses wildcards, bounds,
    // qualifications and nesting at decision time.
    let type_args = if !b.nested_type_args {
        b.type_args.clone()
    } else {
        Vec::new()
    };
    Some(InjectionPoint {
        owner,
        owner_is_bean,
        form,
        name: b.name.clone(),
        type_name: b.type_name.clone(),
        element,
        wrapper,
        generic,
        type_args,
        qualifier: qualifier_of(facts, &b.decorations),
        copied_qualifier: false,
        by_name: None,
        annotations,
        span: b.span,
    })
}

const JAVA_UTIL: &[&str] = &["java.util"];

/// The wrapper, injected type and `generic` flag of a binding's declared type (D9).
fn unwrap(facts: &FileFacts, b: &Binding) -> Option<(Option<Wrapper>, String, bool)> {
    // A wrapper's element with type arguments of its own is a generic point too (evaluator
    // pass 1, Y5).
    let nested = b.nested_type_args;
    // A bounded wildcard's mark is for agreement only (contract M6c S9);
    // resolution still uses the bound, as before (a bare `?` stays `?`).
    let unmark = |a: &str| {
        a.strip_prefix('?')
            .filter(|s| !s.is_empty())
            .unwrap_or(a)
            .to_owned()
    };
    if b.array {
        return Some((
            Some(Wrapper::Collection),
            unmark(b.type_args.first()?),
            nested,
        ));
    }
    let t = b.type_name.as_str();
    let one = || (b.type_args.len() == 1).then(|| unmark(&b.type_args[0]));
    if ["List", "Set", "Collection"]
        .iter()
        .any(|n| is_type(facts, t, JAVA_UTIL, n))
    {
        return Some(match one() {
            Some(e) => (Some(Wrapper::Collection), e, nested),
            None => (None, t.to_owned(), true),
        });
    }
    if is_type(facts, t, JAVA_UTIL, "Map") {
        return Some(match b.type_args.as_slice() {
            [k, v] if is_scalar(k) && simple(k) == "String" => {
                (Some(Wrapper::Collection), unmark(v), nested)
            }
            _ => (None, t.to_owned(), true),
        });
    }
    let optional = is_type(facts, t, JAVA_UTIL, "Optional")
        || is_type(facts, t, &[BEANS_FACTORY], "ObjectProvider")
        || is_type(facts, t, &[BEANS_FACTORY], "ObjectFactory")
        || is_type(facts, t, INJECT, "Provider");
    if optional {
        return Some(match one() {
            Some(e) => (Some(Wrapper::Optional), e, nested),
            None => (None, t.to_owned(), true),
        });
    }
    Some((None, t.to_owned(), !b.type_args.is_empty()))
}

/// An `@Autowired`/`@Inject`/`@Resource` field of class `class` (S3).
fn annotated_field(
    facts: &FileFacts,
    class: usize,
    is_bean: bool,
    b: &Binding,
) -> Option<InjectionPoint> {
    if !has_any_inject(facts, &b.decorations) || b.modifiers.iter().any(|m| m == "static") {
        return None;
    }
    let mut p = point(facts, b, class, is_bean, Form::Field)?;
    if has_any(facts, &b.decorations, RESOURCE, "Resource") {
        p.by_name = Some(resource_name(facts, &b.decorations).unwrap_or_else(|| b.name.clone()));
    }
    Some(p)
}

/// The file's bindings by scope, in source order.
struct Scopes<'f> {
    by_scope: BTreeMap<usize, Vec<&'f Binding>>,
}

impl<'f> Scopes<'f> {
    fn new(facts: &'f FileFacts) -> Self {
        let mut by_scope: BTreeMap<usize, Vec<&'f Binding>> = BTreeMap::new();
        for b in &facts.bindings {
            if let Some(s) = b.scope {
                by_scope.entry(s).or_default().push(b);
            }
        }
        Scopes { by_scope }
    }

    fn of(&self, i: usize) -> &[&'f Binding] {
        self.by_scope.get(&i).map_or(&[], Vec::as_slice)
    }

    /// The parameters of definition `i`, by position.
    fn params(&self, i: usize) -> Vec<&'f Binding> {
        let mut p: Vec<&'f Binding> = self
            .of(i)
            .iter()
            .copied()
            .filter(|b| b.parameter.is_some() && !b.member)
            .collect();
        p.sort_by_key(|b| b.parameter);
        p
    }
}

/// One constructor Lombok generates.
struct Generated<'f> {
    fields: Vec<&'f Binding>,
    marked: bool,
}

/// The constructor points of bean class `class` (D9, amendment 1): the only constructor, or the
/// one marked `@Autowired`/`@Inject` among several, declared or generated by Lombok.
fn constructor_points<'f>(
    facts: &'f FileFacts,
    class: usize,
    scopes: &Scopes<'f>,
    path: &str,
    lombok: &LombokConfigs,
    out: &mut FileDi,
) {
    let defs = &facts.definitions;
    let params = |i: usize| scopes.params(i);
    let declared: Vec<usize> = (0..defs.len())
        .filter(|&i| is_constructor(defs, i, class))
        .collect();
    // A record's components: its implicit canonical constructor when none is declared.
    let components: Vec<&Binding> = scopes
        .of(class)
        .iter()
        .copied()
        .filter(|b| b.parameter.is_some() && !b.member)
        .collect();
    let generated = lombok_constructors(facts, class, scopes, !declared.is_empty());
    let mut chosen: Vec<Vec<(&Binding, bool)>> = Vec::new();
    let total = declared.len()
        + generated.len()
        + usize::from(!components.is_empty() && declared.is_empty());
    if total == 0 {
        return;
    }
    if total == 1 {
        if let Some(&c) = declared.first() {
            chosen.push(params(c).into_iter().map(|b| (b, false)).collect());
        } else if let Some(g) = generated.first() {
            chosen.push(g.fields.iter().map(|b| (*b, true)).collect());
        } else {
            chosen.push(components.iter().map(|b| (*b, false)).collect());
        }
    } else {
        let mut marked: Vec<Vec<(&Binding, bool)>> = declared
            .iter()
            .filter(|&&c| {
                has(facts, &defs[c].decorations, BEANS_ANNOTATION, "Autowired")
                    || has_any(facts, &defs[c].decorations, INJECT, "Inject")
                    || has(facts, &defs[c].decorations, GUICE, "Inject")
            })
            .map(|&c| params(c).into_iter().map(|b| (b, false)).collect())
            .collect();
        marked.extend(
            generated
                .iter()
                .filter(|g| g.marked)
                .map(|g| g.fields.iter().map(|b| (*b, true)).collect()),
        );
        if marked.len() == 1 {
            chosen = marked;
        } else {
            out.ambiguous_constructors
                .push((class, defs[class].name_span));
            return;
        }
    }
    for (b, generated) in chosen.into_iter().flatten() {
        if generated {
            out.points
                .extend(lombok_point(facts, b, class, path, lombok));
        } else {
            out.points
                .extend(point(facts, b, class, true, Form::Constructor));
        }
    }
}

/// A `lombok.config` reader (contract M6c S8): the `copyableAnnotations` list
/// each Java file sees through nearest-enclosing bubbling.
#[derive(Debug, Default)]
pub struct LombokConfigs {
    /// Directory → its `lombok.config` text (`""` is the repository root).
    files: BTreeMap<String, String>,
}

impl LombokConfigs {
    /// Builds the reader from manifest texts by path (only `lombok.config`
    /// entries are kept; everything else ignored). Never fails: an unreadable
    /// chain reads as unread at query time, never here.
    pub fn build(manifests: &BTreeMap<&str, String>) -> Self {
        let mut files = BTreeMap::new();
        for (path, text) in manifests {
            if path.rsplit('/').next().unwrap_or(path) != "lombok.config" {
                continue;
            }
            let dir = match path.rfind('/') {
                Some(i) => path[..i].to_owned(),
                None => String::new(),
            };
            files.insert(dir, text.clone());
        }
        LombokConfigs { files }
    }

    /// The annotations listed under `copyableAnnotations` for the file at
    /// `path`, or `None` when the chain is unread: no file found, a file that
    /// does not parse, or no entry for the key (contract M6c D8). Files apply
    /// from the repository root down, so a nearer `=`/`!` overrides farther
    /// ones and `+=` accumulates; the nearest `config.stopBubbling` file
    /// shields everything above it.
    pub fn copyable(&self, path: &str) -> Option<BTreeSet<String>> {
        // Nearest first to find the boundary, then apply root-first.
        let mut chain: Vec<&String> = Vec::new();
        for dir in dirs(path) {
            work::tick(1);
            let Some(text) = self.files.get(&dir) else {
                continue;
            };
            chain.push(text);
            if is_stop(text) {
                break;
            }
        }
        let mut out: Option<BTreeSet<String>> = None;
        for text in chain.into_iter().rev() {
            for op in parse_lombok_config(text)? {
                match op {
                    LombokOp::Set(set) => out = Some(set),
                    LombokOp::Add(set) => {
                        out.get_or_insert_with(BTreeSet::new).extend(set);
                    }
                    LombokOp::Clear => out = Some(BTreeSet::new()),
                }
            }
        }
        out
    }

    /// Whether the annotation `name` (as written) is positively listed for the
    /// file at `path`: a simple-name match against the chain's entries.
    pub fn copies(&self, path: &str, name: &str) -> bool {
        self.copyable(path)
            .is_some_and(|set| set.iter().any(|e| simple(e) == simple(name)))
    }
}

/// Directories from the file's own outward (`a/b` for `a/b/C.java`, ending at
/// the repository root `""`).
fn dirs(path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut dir = match path.rfind('/') {
        Some(i) => path[..i].to_owned(),
        None => String::new(),
    };
    loop {
        out.push(dir.clone());
        if dir.is_empty() {
            break;
        }
        dir = match dir.rfind('/') {
            Some(i) => dir[..i].to_owned(),
            None => String::new(),
        };
    }
    out
}

/// One `copyableAnnotations` directive: `key = [...]` sets, `key += [...]`
/// appends, bare `key !` clears.
enum LombokOp {
    Set(BTreeSet<String>),
    Add(BTreeSet<String>),
    Clear,
}

/// Whether the text stops bubbling (`config.stopBubbling = true`, tolerant of
/// spacing; anything else on the line is ignored here and fails parsing only
/// if it sits on the `copyableAnnotations` key).
fn is_stop(text: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim();
        line.starts_with("config.stopBubbling") && line.contains("true")
    })
}

/// The `copyableAnnotations` directives of one `lombok.config` text, in order;
/// `None` when a line on the key is unparseable. Other keys are ignored.
fn parse_lombok_config(text: &str) -> Option<Vec<LombokOp>> {
    const KEY: &str = "lombok.copyableAnnotations";
    let mut ops = Vec::new();
    for line in text.lines() {
        // Inline trailing comments strip, as Lombok itself strips them (a
        // `#` cannot occur inside an annotation name or directive).
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(rest) = line.strip_prefix(KEY) {
            let rest = rest.trim();
            if let Some(add) = rest.strip_prefix("+=") {
                ops.push(LombokOp::Add(split_list(add)?));
            } else if rest == "!" {
                ops.push(LombokOp::Clear);
            } else {
                ops.push(LombokOp::Set(split_list(rest.strip_prefix('=')?)?));
            }
        }
    }
    Some(ops)
}

/// One `[...]` list (or a single bare value) of annotation names.
fn split_list(value: &str) -> Option<BTreeSet<String>> {
    let value = value.trim();
    if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        return Some(
            inner
                .split(',')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect::<BTreeSet<String>>(),
        );
    }
    if value.is_empty()
        || value.starts_with('[')
        || value.contains([' ', ',', '=', '!', '(', ')', '{', '}', ';', '"'])
    {
        return None;
    }
    Some(BTreeSet::from([value.to_owned()]))
}

/// A Lombok-generated constructor parameter: the field's type and name. The
/// field's qualifier rides the parameter only when `lombok.config` positively
/// lists its annotation (contract M6c S8); otherwise it stays on the field,
/// recorded as an unread annotation with no qualifier, exactly as before.
fn lombok_point(
    facts: &FileFacts,
    field: &Binding,
    class: usize,
    path: &str,
    lombok: &LombokConfigs,
) -> Option<InjectionPoint> {
    let mut p = point(facts, field, class, true, Form::Constructor)?;
    let copied = field
        .decorations
        .iter()
        .find(|d| is_qualifier(facts, d))
        .is_some_and(|d| lombok.copies(path, &d.name));
    if copied {
        p.copied_qualifier = true;
        return Some(p);
    }
    p.qualifier = None;
    p.annotations = field
        .decorations
        .iter()
        .filter(|d| !is_inert(facts, d))
        .map(|d| d.name.clone())
        .collect();
    Some(p)
}

/// The constructors Lombok generates for `class` (D9, amendment 1): `@RequiredArgsConstructor`
/// (non-initialised `final` or `@NonNull` fields), `@AllArgsConstructor` (every field but an
/// initialised `final` one), `@NoArgsConstructor`; `@Data` (required) and `lombok.Value` (all)
/// only without a declared or annotated constructor. Static fields are skipped; fields keep
/// their declaration order.
fn lombok_constructors<'f>(
    facts: &'f FileFacts,
    class: usize,
    scopes: &Scopes<'f>,
    has_declared: bool,
) -> Vec<Generated<'f>> {
    let decorations = &facts.definitions[class].decorations;
    let lombok = |name: &str| {
        decorations
            .iter()
            .find(|d| is_annotation(facts, d, LOMBOK, name))
    };
    let fields: Vec<&Binding> = scopes
        .of(class)
        .iter()
        .copied()
        .filter(|b| b.member && !b.modifiers.iter().any(|m| m == "static"))
        .collect();
    let is_final = |b: &Binding| b.modifiers.iter().any(|m| m == "final") || b.constant;
    let non_null = |b: &Binding| {
        b.decorations
            .iter()
            .any(|d| simple(&d.name).eq_ignore_ascii_case("nonnull"))
    };
    let required: Vec<&Binding> = fields
        .iter()
        .copied()
        .filter(|b| !b.initialised && (is_final(b) || non_null(b)))
        .collect();
    let all: Vec<&Binding> = fields
        .iter()
        .copied()
        .filter(|b| !(b.initialised && is_final(b)))
        .collect();
    let marked = |d: &Decoration| {
        d.args.iter().any(|a| {
            a.key
                .as_deref()
                .is_some_and(|k| k.starts_with("onConstructor"))
                && (a.value.contains("Autowired") || a.value.contains("Inject"))
        })
    };
    let mut out = Vec::new();
    let explicit = [
        "RequiredArgsConstructor",
        "AllArgsConstructor",
        "NoArgsConstructor",
    ];
    for name in explicit {
        if let Some(d) = lombok(name) {
            out.push(Generated {
                fields: match name {
                    "RequiredArgsConstructor" => required.clone(),
                    "AllArgsConstructor" => all.clone(),
                    _ => Vec::new(),
                },
                marked: marked(d),
            });
        }
    }
    if out.is_empty() && !has_declared {
        if lombok("Value").is_some() {
            out.push(Generated {
                fields: fields.iter().copied().filter(|b| !b.initialised).collect(),
                marked: false,
            });
        } else if lombok("Data").is_some() {
            out.push(Generated {
                fields: required,
                marked: false,
            });
        }
    }
    out
}

/// The attributes a scan or enable annotation may carry and still be read (S2): a root
/// attribute, or one that does not change which classes are candidates.
const SCAN_ATTRIBUTES: &[&str] = &[
    "scanBasePackages",
    "scanBasePackageClasses",
    "basePackages",
    "value",
    "basePackageClasses",
    "exclude",
    "excludeName",
    "proxyBeanMethods",
];
const ROOT_ATTRIBUTES: &[&str] = &["scanBasePackages", "basePackages", "value"];
const ROOT_CLASS_ATTRIBUTES: &[&str] = &["scanBasePackageClasses", "basePackageClasses"];

fn scans(facts: &FileFacts, class: usize, out: &mut FileDi) {
    let def = &facts.definitions[class];
    for d in &def.decorations {
        let kind = if is_annotation(facts, d, BOOT_AUTOCONFIGURE, "SpringBootApplication")
            || is_annotation(facts, d, CONTEXT, "ComponentScan")
        {
            ScanKind::Components
        } else if is_enable_repositories(facts, d) {
            ScanKind::Repositories
        } else if is_any(facts, d, FEIGN, "EnableFeignClients") {
            ScanKind::Feign
        } else {
            continue;
        };
        let mut packages = BTreeSet::new();
        let mut unread = false;
        for a in &d.args {
            let key = a.key.as_deref().unwrap_or("value");
            if !SCAN_ATTRIBUTES.contains(&key) {
                unread = true;
                continue;
            }
            let values: Vec<&DecorationArg> = if a.items.is_empty() {
                vec![a]
            } else {
                a.items.iter().collect()
            };
            if ROOT_ATTRIBUTES.contains(&key) {
                for v in values {
                    if v.literal {
                        packages.insert(v.value.clone());
                    } else {
                        unread = true;
                    }
                }
            } else if ROOT_CLASS_ATTRIBUTES.contains(&key) {
                for v in values {
                    match class_package(facts, &v.value) {
                        Some(p) => {
                            packages.insert(p);
                        }
                        None => unread = true,
                    }
                }
            }
        }
        if packages.is_empty() {
            packages.insert(facts.package.clone().unwrap_or_default());
        }
        out.scans.push(ScanDecl {
            class,
            kind,
            packages: packages.into_iter().collect(),
            unread,
        });
    }
}

/// The package of a class literal (`X.class`, `a.b.X.class`): written, imported, or this
/// file's.
fn class_package(facts: &FileFacts, literal: &str) -> Option<String> {
    let name = literal.strip_suffix(".class")?.trim();
    let first = name.split('.').next()?;
    if first.chars().next()?.is_lowercase() {
        return name.rsplit_once('.').map(|(p, _)| p.to_owned());
    }
    facts
        .imports
        .iter()
        .find(|i| !i.is_static && i.names.iter().any(|n| n.local == first))
        .map(|i| i.module.clone())
        .or_else(|| facts.package.clone())
}

/// The packages whose names gate D checks (S6).
const GATED: &[&str] = &[
    "org.springframework.beans.factory",
    "org.springframework.beans.factory.annotation",
    "org.springframework.beans.factory.config",
    "org.springframework.beans.factory.support",
    "org.springframework.context",
    "org.springframework.context.annotation",
    "org.springframework.context.support",
    "org.springframework.boot",
    "org.springframework.boot.autoconfigure",
];

/// The names of the gated packages that leave the DI surface plain (S6 gate D).
const PLAIN: &[&str] = &[
    "Component",
    "Service",
    "Repository",
    "Controller",
    "RestController",
    "Configuration",
    "ControllerAdvice",
    "RestControllerAdvice",
    "Autowired",
    "Qualifier",
    "Value",
    "Bean",
    "Primary",
    "Lazy",
    "Scope",
    "DependsOn",
    "Order",
    "Import",
    "Profile",
    "ComponentScan",
    "SpringBootApplication",
    "SpringApplication",
    "EnableAutoConfiguration",
    "ApplicationContext",
    "EventListener",
    "CommandLineRunner",
    "ApplicationRunner",
    "InitializingBean",
    "DisposableBean",
    "ConfigurationProperties",
    "EnableConfigurationProperties",
];

fn plain(name: &str) -> bool {
    PLAIN.contains(&name) || name.starts_with("Conditional") || name.starts_with("ApplicationEvent")
}

/// A qualified name as written (`org.springframework.context.support.GenericApplicationContext`)
/// that names a gated package's non-plain type.
fn gated_qualified(written: &str) -> Option<String> {
    let segments: Vec<&str> = written.split('.').collect();
    let at = segments
        .iter()
        .position(|s| s.chars().next().is_some_and(char::is_uppercase))?;
    let package = segments[..at].join(".");
    (GATED.contains(&package.as_str()) && !plain(segments[at]))
        .then(|| format!("{package}.{}", segments[at]))
}

/// Gate D's findings in a Java file's facts (S6): an import, static import or wildcard import
/// of a gated package's non-plain name; a qualified annotation or supertype naming one; any
/// other qualified use of `org.springframework` (an expression, a cast, a type argument), which
/// the facts do not resolve and so always counts.
fn surface_of_facts(facts: &FileFacts, out: &mut Vec<SurfaceFinding>) {
    for i in &facts.imports {
        let (package, names): (&str, Vec<&str>) = if i.is_static {
            match i.module.rsplit_once('.') {
                Some((p, class)) => (p, vec![class]),
                None => continue,
            }
        } else if i.wildcard {
            (i.module.as_str(), vec!["*"])
        } else {
            (
                i.module.as_str(),
                i.names.iter().map(|n| n.name.as_str()).collect(),
            )
        };
        if !GATED.contains(&package) {
            continue;
        }
        for name in names {
            if name == "*" || !plain(name) {
                out.push(SurfaceFinding {
                    what: format!("import {package}.{name}"),
                    span: i.span,
                });
            }
        }
    }
    let written = facts
        .definitions
        .iter()
        .flat_map(|d| {
            d.decorations
                .iter()
                .map(|x| (x.name.as_str(), x.span))
                .chain(d.supertypes.iter().map(|s| (s.name.as_str(), s.span)))
        })
        .chain(
            facts
                .bindings
                .iter()
                .flat_map(|b| b.decorations.iter().map(|x| (x.name.as_str(), x.span))),
        );
    for (name, span) in written {
        if let Some(what) = gated_qualified(name) {
            out.push(SurfaceFinding {
                what: format!("qualified {what}"),
                span,
            });
        }
    }
    let unresolved = facts.mentions.get("springframework").is_some_and(|roles| {
        roles.iter().any(|r| {
            !matches!(
                r,
                rb_syntax::MentionRole::Import
                    | rb_syntax::MentionRole::String
                    | rb_syntax::MentionRole::Annotation
                    | rb_syntax::MentionRole::Supertype
            )
        })
    });
    if unresolved {
        out.push(SurfaceFinding {
            what: "qualified org.springframework use".to_owned(),
            span: [0, 0, 0, 0],
        });
    }
    out.sort();
    out.dedup();
}

/// Whole words whose use as an annotation (`@Word`) or call (`Word(`) in a file without facts
/// may define or replace beans (S6 gate D's raw-text list; a word list, stated as such).
const RAW_WORDS: &[&str] = &[
    "BeanPostProcessor",
    "BeanFactoryPostProcessor",
    "BeanDefinitionRegistryPostProcessor",
    "ImportBeanDefinitionRegistrar",
    "ImportSelector",
    "FactoryBean",
    "registerBean",
    "BeanDefinition",
    "Component",
    "Service",
    "Repository",
    "Configuration",
    "Bean",
];

/// Gate D's findings in a file without facts (S6): a Spring beans XML namespace, a
/// `META-INF/spring.factories` or `META-INF/spring/*.imports` file, a `beans.xml`, or a word
/// of [`RAW_WORDS`] as an annotation or call. `text` is `None` for a file over the caller's
/// byte cap, which counts as a finding. Inert text (prose, npm lock files) has none.
pub fn surface_text(path: &str, text: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let name = path.rsplit('/').next().unwrap_or(path);
    if path.ends_with("META-INF/spring.factories")
        || (path.contains("META-INF/spring/") && name.ends_with(".imports"))
    {
        out.push("spring.factories".to_owned());
    }
    if name == "beans.xml" {
        out.push("beans.xml".to_owned());
    }
    if crate::clients::inert(path) {
        return out;
    }
    let Some(text) = text else {
        out.push("over the byte cap".to_owned());
        return out;
    };
    if name.to_ascii_lowercase().ends_with(".xml")
        && text.contains("springframework.org/schema/beans")
    {
        out.push("beans namespace".to_owned());
    }
    for word in RAW_WORDS {
        if annotation_or_call(text, word) {
            out.push(format!("raw {word}"));
        }
    }
    out
}

/// `@word` or `word(` (spaces allowed before the parenthesis), as a whole word.
fn annotation_or_call(text: &str, word: &str) -> bool {
    let ident = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'$';
    let bytes = text.as_bytes();
    text.match_indices(word).any(|(i, _)| {
        let end = i + word.len();
        let starts = i == 0 || !ident(bytes[i - 1]);
        let ends = end == bytes.len() || !ident(bytes[end]);
        if !(starts && ends) {
            return false;
        }
        let annotation = i > 0 && bytes[i - 1] == b'@';
        let call = text[end..].trim_start().starts_with('(');
        annotation || call
    })
}

#[cfg(test)]
mod tests {
    use rb_syntax::{Language, extract};

    use super::*;

    fn facts_of(src: &str) -> FileFacts {
        extract(Language::Java, src.as_bytes()).unwrap()
    }

    fn file_di(src: &str) -> (FileFacts, FileDi) {
        file_di_at(src, "src/Test.java", &LombokConfigs::default())
    }

    fn file_di_at(src: &str, path: &str, lombok: &LombokConfigs) -> (FileFacts, FileDi) {
        let facts = facts_of(src);
        let bases = crate::repository_bases(&facts.definitions);
        let out = di(Language::Java, &facts, &bases, path, lombok);
        (facts, out)
    }

    /// `origin name: type [names] q=… primary conditional` per bean.
    fn beans(src: &str) -> Vec<String> {
        let (facts, out) = file_di(src);
        out.beans
            .iter()
            .map(|b| {
                let mut s = format!(
                    "{} {}: {} [{}]",
                    b.origin.as_str(),
                    facts.definitions[b.definition].name,
                    b.type_name,
                    b.names.join(",")
                );
                if !b.qualifiers.is_empty() {
                    s += &format!(" q={}", b.qualifiers.join(","));
                }
                if b.primary {
                    s += " primary";
                }
                if b.conditional {
                    s += " conditional";
                }
                if !b.annotations.is_empty() {
                    s += &format!(" @{}", b.annotations.join(",@"));
                }
                if !b.created.is_empty() {
                    s += &format!(" new={}", b.created.join(","));
                }
                s
            })
            .collect()
    }

    /// `owner.form name: element (wrapper) q=… by=… @…` per point.
    fn points(src: &str) -> Vec<String> {
        points_with(src, "src/Test.java", &LombokConfigs::default())
    }

    /// As [`points`], with an explicit path and `lombok.config` chain.
    fn points_with(src: &str, path: &str, lombok: &LombokConfigs) -> Vec<String> {
        let (facts, out) = file_di_at(src, path, lombok);
        out.points
            .iter()
            .map(|p| {
                let mut s = format!(
                    "{}.{} {}: {}",
                    facts.definitions[p.owner].name,
                    p.form.as_str(),
                    p.name,
                    p.element
                );
                match p.wrapper {
                    Some(Wrapper::Collection) => s += " (collection)",
                    Some(Wrapper::Optional) => s += " (optional)",
                    None => {}
                }
                if p.generic {
                    s += " generic";
                }
                if let Some(q) = &p.qualifier {
                    s += &format!(" q={q}");
                }
                if let Some(n) = &p.by_name {
                    s += &format!(" by={n}");
                }
                if !p.owner_is_bean {
                    s += " not_a_bean";
                }
                if !p.annotations.is_empty() {
                    s += &format!(" @{}", p.annotations.join(",@"));
                }
                s
            })
            .collect()
    }

    const IMPORTS: &str = "package com.acme;\nimport org.springframework.stereotype.*;\nimport org.springframework.context.annotation.*;\nimport org.springframework.beans.factory.annotation.Autowired;\nimport org.springframework.beans.factory.annotation.Qualifier;\nimport org.springframework.beans.factory.annotation.Value;\nimport java.util.*;\n";

    fn with_imports(body: &str) -> String {
        format!("{IMPORTS}{body}")
    }

    /// Contract M6j S2: Guice singletons, provides methods, inject points and
    /// `bind().to()` links; the qualifier comes from `com.google.inject.name`.
    #[test]
    fn guice_singletons_provides_points_and_binds() {
        let src = "package com.acme;\nimport com.google.inject.AbstractModule;\nimport com.google.inject.Inject;\nimport com.google.inject.Provides;\nimport com.google.inject.Singleton;\nimport com.google.inject.name.Named;\n@Singleton class Store {\n  private final Db db;\n  @Inject public Store(@Named(\"main\") Db db) { this.db = db; }\n}\nclass ShopModule extends AbstractModule {\n  @Override protected void configure() {\n    bind(Service.class).to(ServiceImpl.class);\n    bind(Names.class).annotatedWith(Names.named(\"a\")).to(A.class);\n  }\n  @Provides @Singleton Db provideDb() { return new Db(); }\n}\n";
        assert_eq!(
            beans(src),
            [
                "singleton Store: Store [store]",
                "bean_method provideDb: Db [provideDb] new=Db",
                "binds configure: Service [] new=ServiceImpl",
                "binds configure: Names [] q=a new=A",
            ]
        );
        assert_eq!(points(src), ["Store.constructor db: Db q=main"]);
    }

    /// Contract M6j S2: Dagger modules, binds, provides, components and inject points;
    /// `@Component` interfaces are understood but not beans.
    #[test]
    fn dagger_modules_binds_components_and_points() {
        let src = "package com.acme;\nimport dagger.Binds;\nimport dagger.Component;\nimport dagger.Module;\nimport dagger.Provides;\nimport jakarta.inject.Inject;\nimport jakarta.inject.Singleton;\n@Module abstract class ShopModule {\n  @Binds abstract Service bindService(ServiceImpl impl);\n  @Provides static Db provideDb() { return new Db(); }\n}\n@Singleton @Component(modules = ShopModule.class) interface ShopComponent {\n  Shop shop();\n}\nclass Shop {\n  @Inject public Shop(Service svc) {}\n}\n";
        assert_eq!(
            beans(src),
            [
                "binds bindService: Service []",
                "bean_method provideDb: Db [provideDb] new=Db",
                "injectable Shop: Shop [shop]",
            ]
        );
        assert_eq!(points(src), ["Shop.constructor svc: Service"]);
        let (_, out) = file_di(src);
        assert!(out.unread_classes.is_empty());
    }

    /// Contract M6j S2: an `annotatedWith` the pack cannot read to a literal skips the
    /// pair (an unqualified point must not match a qualified binding).
    #[test]
    fn guice_unresolvable_qualifiers_skip_the_binding() {
        let src = "package com.acme;\nimport com.google.inject.AbstractModule;\nimport com.google.inject.name.Named;\nclass ShopModule extends AbstractModule {\n  @Override protected void configure() {\n    bind(Names.class).annotatedWith(Names.named(prefix + \"x\")).to(A.class);\n  }\n}\n";
        assert_eq!(beans(src), Vec::<String>::new());
    }

    /// Contract M6j S2: Guice field and setter points through Guice's own `@Inject`.
    #[test]
    fn guice_field_and_setter_points() {
        let src = "package com.acme;\nimport com.google.inject.Inject;\nimport com.google.inject.Singleton;\n@Singleton class Store {\n  @Inject Db db;\n  @Inject public void setDb(Db db) {}\n}\n";
        assert_eq!(points(src), ["Store.field db: Db", "Store.setter db: Db"]);
    }

    /// `origin name: type [names] q=…` per NestJS bean.
    fn nest_beans(src: &str) -> Vec<String> {
        let facts = extract(Language::TypeScript, src.as_bytes()).unwrap();
        let out = nest_di(&facts);
        out.beans
            .iter()
            .map(|b| {
                let mut s = format!(
                    "{} {}: {} [{}]",
                    b.origin.as_str(),
                    facts.definitions[b.definition].name,
                    b.type_name,
                    b.names.join(",")
                );
                if !b.qualifiers.is_empty() {
                    s += &format!(" q={}", b.qualifiers.join(","));
                }
                s
            })
            .collect()
    }

    /// `owner.form name: element q=…` per NestJS point.
    fn nest_points(src: &str) -> Vec<String> {
        let facts = extract(Language::TypeScript, src.as_bytes()).unwrap();
        let out = nest_di(&facts);
        out.points
            .iter()
            .map(|p| {
                let mut s = format!(
                    "{}.{} {}: {}",
                    facts.definitions[p.owner].name,
                    p.form.as_str(),
                    p.name,
                    p.element
                );
                if let Some(q) = &p.qualifier {
                    s += &format!(" q={q}");
                }
                if !p.owner_is_bean {
                    s += " not_a_bean";
                }
                s
            })
            .collect()
    }
    /// Contract M6j S3: NestJS injectables, controllers, constructor points and token
    /// qualifiers; module metadata is understood.
    #[test]
    fn nest_injectables_controllers_points_and_tokens() {
        let src = "import { Injectable, Inject, Controller, Get } from '@nestjs/common';\n@Injectable()\nexport class ShopService {\n  constructor(@Inject('DB') private db: Db) {}\n  @Inject() helper: Helper;\n}\n@Controller('shop')\nexport class ShopController {\n  constructor(private readonly svc: ShopService) {}\n  @Get()\n  list() { return null; }\n}\n";
        assert_eq!(
            nest_beans(src),
            [
                "injectable ShopService: ShopService [shopService]",
                "injectable ShopController: ShopController [shopController]",
            ]
        );
        assert_eq!(
            nest_points(src),
            [
                "ShopService.constructor db: Db q=DB",
                "ShopService.field helper: Helper",
                "ShopController.constructor svc: ShopService",
            ]
        );
    }

    /// Contract M6j S3: same-file providers entries become beans; custom providers and
    /// foreign decorators do not.
    #[test]
    fn nest_module_providers_and_decoys() {
        let src = "import { Module, Injectable } from '@nestjs/common';\nimport { Component } from './other';\nclass Helper {}\n@Injectable()\nclass Real {}\n@Component()\nclass Widget {}\n@Module({ providers: [Helper, Real, { provide: 'X', useValue: 1 }] })\nclass M {}\n";
        assert_eq!(
            nest_beans(src),
            [
                "injectable Helper: Helper [helper]",
                "injectable Real: Real [real]",
            ]
        );
        // The foreign `@Component` taints its class (gate D keeps working on TS).
        let facts = extract(Language::TypeScript, src.as_bytes()).unwrap();
        assert_eq!(nest_di(&facts).unread_classes.len(), 1);
    }

    /// Contract M6j S5: Kotlin `beans { }` registrations read as beans; `ref()`
    /// calls read as points (untyped, qualifier only when a literal names one).
    #[test]
    fn kotlin_beans_dsl_registrations_and_refs() {
        let src = "package acme\nimport org.springframework.fu.kofu.beans\nfun appBeans() = beans {\nbean<ServiceImpl>()\nbean(\"store\") { Store(ref()) }\nbean(\"cfg\", scope = SINGLETON) { Config(ref(\"db\")) }\n}\n";
        let facts = extract(Language::Kotlin, src.as_bytes()).unwrap();
        let out = kotlin_di(&facts);
        let beans: Vec<String> = out
            .beans
            .iter()
            .map(|b| {
                format!(
                    "{} {}: {} [{}]",
                    b.origin.as_str(),
                    facts.definitions[b.definition].name,
                    b.type_name,
                    b.names.join(",")
                )
            })
            .collect();
        assert_eq!(
            beans,
            [
                "binds appBeans: ServiceImpl [serviceImpl]",
                "binds appBeans: Store [store]",
                "binds appBeans: Config [cfg]",
            ]
        );
        let points: Vec<String> = out
            .points
            .iter()
            .map(|p| {
                format!(
                    "{}.{} {}: {} q={:?}",
                    facts.definitions[p.owner].name,
                    p.form.as_str(),
                    p.name,
                    p.element,
                    p.qualifier
                )
            })
            .collect();
        assert_eq!(
            points,
            [
                "appBeans.constructor #0:  q=None",
                "appBeans.constructor #0:  q=Some(\"db\")",
            ]
        );
    }

    /// Contract M6j S2 decoys: same-named annotations from other packages bind nothing;
    /// a bare jakarta `@Inject` constructor with no framework marker is no bean.
    #[test]
    fn di_decoys_give_no_beans() {
        let src = "package com.acme;\nimport com.acme.Singleton;\nimport com.acme.Provides;\n@Singleton class Plain {}\nclass Shop {\n  @Provides Db make() { return null; }\n}\n";
        assert_eq!(beans(src), Vec::<String>::new());
        let bare = "package com.acme;\nimport jakarta.inject.Inject;\nclass Shop {\n  @Inject public Shop(Service svc) {}\n}\n";
        assert_eq!(beans(bare), Vec::<String>::new());
        // Constructor points exist only for bean classes, as in Spring: no bean, no points.
        assert_eq!(points(bare), Vec::<String>::new());
    }

    #[test]
    fn stereotypes_names_qualifiers_primary_and_conditionals() {
        let src = with_imports(
            "@Component class BcryptHasher implements PasswordHasher {}\n@Service(\"plain\") @Qualifier(\"legacy\") class PlainHasher {}\n@Component @Primary @Profile(\"prod\") class URLParser {}\n@Configuration class AuditConfig {\n  @Bean AuditSink auditSink() { return new LogAuditSink(); }\n  @Bean(name = {\"a\", \"b\"}) @Primary Map<String, Object> m() { return null; }\n  @Bean @Conditional(X.class) Object o() { return new Kafka(); }\n}\n@Component static class Nested {}\nclass Outer { @Component static class Inner {} @Component class NotStatic {} }\n@Component abstract class Abstract {}\n",
        );
        assert_eq!(
            beans(&src),
            [
                "stereotype BcryptHasher: BcryptHasher [bcryptHasher]",
                "stereotype PlainHasher: PlainHasher [plain] q=legacy",
                "stereotype URLParser: URLParser [URLParser] primary conditional",
                "stereotype AuditConfig: AuditConfig [auditConfig]",
                "bean_method auditSink: AuditSink [auditSink] new=LogAuditSink",
                "bean_method m: Map [a,b] primary",
                "bean_method o: Object [o] conditional new=Kafka",
                "stereotype Nested: Nested [nested]",
                "stereotype Inner: Inner [outer.Inner]",
            ]
        );
    }

    /// Decoys (S2): a same-named `@Service` from another package, a commented `// @Component`,
    /// a stereotype on an interface, `@Bean` in a class with no stereotype.
    #[test]
    fn decoys_give_no_beans() {
        let src = "package com.acme;\nimport com.acme.support.Service;\nimport org.springframework.context.annotation.Bean;\n@Service class ReportNotifier {}\n// @Component\nclass LegacyAuth {}\n@org.springframework.stereotype.Component interface Marker {}\nclass Plain { @Bean Object b() { return null; } }\n";
        assert_eq!(beans(src), Vec::<String>::new());
        let (_, out) = file_di(src);
        // The foreign `@Service` may be a meta-annotated stereotype: the class is unread.
        assert_eq!(out.unread_classes.len(), 1);
        // Qualified stereotypes count.
        assert_eq!(
            beans("package p;\n@org.springframework.stereotype.Service class S {}\n"),
            ["stereotype S: S [s]"]
        );
    }

    #[test]
    fn spring_data_and_feign_beans() {
        let src = "package com.acme;\nimport org.springframework.data.jpa.repository.JpaRepository;\nimport org.springframework.data.mongodb.repository.MongoRepository;\nimport org.springframework.data.repository.NoRepositoryBean;\nimport org.springframework.cloud.openfeign.FeignClient;\ninterface UserRepository extends JpaRepository<User, Long>, UserRepositoryCustom {}\ninterface Accounts extends MongoRepository<Account, String> {}\n@NoRepositoryBean interface Base extends JpaRepository<User, Long> {}\ninterface Generic<T> extends JpaRepository<T, Long> {}\ninterface NotRepo extends Other {}\n@FeignClient(name = \"mfa\") interface MfaClient {}\n@FeignClient(value = \"x\", qualifiers = {\"q\"}, primary = false) interface XClient {}\n";
        assert_eq!(
            beans(src),
            [
                "spring_data UserRepository: UserRepository [userRepository]",
                "spring_data Accounts: Accounts [accounts]",
                "feign MfaClient: MfaClient [mfaFeignClient] primary",
                "feign XClient: XClient [q] q=q",
            ]
        );
        let (_, out) = file_di(src);
        assert_eq!(
            out.beans[0].extends,
            ["JpaRepository", "UserRepositoryCustom"]
        );
        // A store base from another package is no repository base.
        assert_eq!(
            beans(
                "package p;\nimport x.MongoRepository;\ninterface A extends MongoRepository<B, C> {}\n"
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn injection_point_forms_wrappers_and_exclusions() {
        let src = with_imports(
            "import jakarta.inject.Inject;\nimport jakarta.annotation.Resource;\nimport org.springframework.beans.factory.ObjectProvider;\n@Service class DefaultAuthService {\n  @Autowired private UserRepository repo;\n  @Inject Clock clock;\n  @Resource(name = \"main\") DataSource ds;\n  @Resource DataSource other;\n  @Autowired @Value(\"${x}\") Port port;\n  @Autowired static Repo shared;\n  Repo notInjected;\n  DefaultAuthService(PasswordHasher hasher, List<Notifier> ns, Map<String, Notifier> byName, Notifier[] arr, Optional<Clock> c, ObjectProvider<Sink> p, @Value(\"${k}\") String k, int n, String s, Long l, Box<Item> g, @Qualifier(\"plain\") PasswordHasher legacy, Map<Long, Notifier> m) {}\n  @Autowired void setIssuer(TokenIssuer issuer) {}\n  @Resource void setAudit(AuditSink sink) {}\n  void plainMethod(Repo r) {}\n}\n@Configuration class Cfg { @Bean Sink sink(Repo repo, @Qualifier(\"q\") Clock c) { return null; } }\nclass LegacyAuth { @Autowired private AuthService delegate; LegacyAuth(Repo r) {} }\n",
        );
        assert_eq!(
            points(&src),
            [
                "DefaultAuthService.constructor hasher: PasswordHasher",
                "DefaultAuthService.constructor ns: Notifier (collection)",
                "DefaultAuthService.constructor byName: Notifier (collection)",
                "DefaultAuthService.constructor arr: Notifier (collection)",
                "DefaultAuthService.constructor c: Clock (optional)",
                "DefaultAuthService.constructor p: Sink (optional)",
                "DefaultAuthService.constructor g: Box generic",
                "DefaultAuthService.constructor legacy: PasswordHasher q=plain",
                "DefaultAuthService.constructor m: Map generic",
                "DefaultAuthService.field repo: UserRepository",
                "DefaultAuthService.field clock: Clock",
                "DefaultAuthService.field ds: DataSource by=main",
                "DefaultAuthService.field other: DataSource by=other",
                "DefaultAuthService.setter issuer: TokenIssuer",
                "DefaultAuthService.setter sink: AuditSink by=audit",
                "sink.bean_parameter repo: Repo",
                "sink.bean_parameter c: Clock q=q",
                "LegacyAuth.field delegate: AuthService not_a_bean",
            ]
        );
    }

    #[test]
    fn constructor_selection() {
        // Several constructors, none marked: no points, one ambiguity.
        let src = with_imports("@Service class A { A(Repo r) {} A(Clock c) {} }\n");
        let (_, out) = file_di(&src);
        assert!(out.points.is_empty());
        assert_eq!(out.ambiguous_constructors.len(), 1);
        // The marked one wins.
        let src = with_imports("@Service class A { A(Repo r) {} @Autowired A(Clock c) {} }\n");
        assert_eq!(points(&src), ["A.constructor c: Clock"]);
        // A method named like its class is no constructor.
        let src = with_imports("@Service class A { A(Repo r) {} void A(Clock c) {} }\n");
        assert_eq!(points(&src), ["A.constructor r: Repo"]);
    }

    /// D9's Lombok rules, one row each, and amendment 1.
    #[test]
    fn lombok_generated_constructors() {
        let lombok = "import lombok.RequiredArgsConstructor;\nimport lombok.AllArgsConstructor;\nimport lombok.NonNull;\n";
        let row = |body: &str| points(&with_imports(&format!("{lombok}{body}")));
        assert_eq!(
            row("@Service @RequiredArgsConstructor class A { private final Repo repo; }\n"),
            ["A.constructor repo: Repo"]
        );
        assert_eq!(
            row(
                "@Service @RequiredArgsConstructor class A { @NonNull private Clock clock; Sink plain; }\n"
            ),
            ["A.constructor clock: Clock"]
        );
        assert_eq!(
            row(
                "@Service @RequiredArgsConstructor class A { private static final Repo SHARED = null; private final Repo repo; }\n"
            ),
            ["A.constructor repo: Repo"]
        );
        assert_eq!(
            row(
                "@Service @RequiredArgsConstructor class A { private final Repo repo = new Repo(); private final Clock clock; }\n"
            ),
            ["A.constructor clock: Clock"]
        );
        assert_eq!(
            row(
                "@Service @AllArgsConstructor class A { private final Repo repo; Sink sink; private final Clock c = null; }\n"
            ),
            ["A.constructor repo: Repo", "A.constructor sink: Sink"]
        );
        // Amendment 1: a declared constructor next to Lombok's gives two, none marked.
        let src = with_imports(&format!(
            "{lombok}@Service @RequiredArgsConstructor class A {{ private final Repo repo; A(Clock c) {{ this.repo = null; }} }}\n"
        ));
        let (_, out) = file_di(&src);
        assert!(out.points.is_empty());
        assert_eq!(out.ambiguous_constructors.len(), 1);
        // `onConstructor_` marks the generated one.
        assert_eq!(
            row(
                "@Service @RequiredArgsConstructor(onConstructor_ = @Autowired) class A { private final Repo repo; A(Clock c) { this.repo = null; } }\n"
            ),
            ["A.constructor repo: Repo"]
        );
        // A field's qualifier is not assumed copied: recorded as an annotation, no qualifier.
        assert_eq!(
            row(
                "@Service @RequiredArgsConstructor class A { @Qualifier(\"x\") private final Repo repo; }\n"
            ),
            ["A.constructor repo: Repo @Qualifier"]
        );
        // Not Lombok's: no generation.
        assert_eq!(
            points(&with_imports(
                "import other.RequiredArgsConstructor;\n@Service @RequiredArgsConstructor class A { private final Repo repo; }\n"
            )),
            Vec::<String>::new()
        );
    }

    /// Contract M6c S8: the `lombok.config` chain reads nearest-first with
    /// `=`/`+=`/bare-`!`, `config.stopBubbling` stops the walk, and anything
    /// unreadable or absent reads as unread.
    #[test]
    fn lombok_configs_read_nearest_chain() {
        let maps = |files: &[(&str, &str)]| {
            LombokConfigs::build(&files.iter().map(|(p, t)| (*p, (*t).to_owned())).collect())
        };
        let full = maps(&[
            (
                "lombok.config",
                "lombok.copyableAnnotations = [com.foo.Base]\n",
            ),
            (
                "svc/lombok.config",
                "# nearer wins\nlombok.copyableAnnotations += [com.acme.Qualifier]\n",
            ),
        ]);
        assert!(full.copies("svc/src/A.java", "Qualifier"));
        assert!(full.copies("svc/src/A.java", "com.acme.Qualifier"));
        assert!(full.copies("svc/src/A.java", "Base"));
        assert!(!full.copies("other/src/A.java", "Qualifier"));
        // `=` replaces: the root list is gone where the nearer file sets nothing.
        let bare = maps(&[(
            "lombok.config",
            "lombok.copyableAnnotations = [com.foo.Base]\n",
        )]);
        assert!(bare.copies("svc/src/A.java", "Base"));
        // `+=` accumulates onto what root-ward files set.
        let acc = maps(&[
            (
                "lombok.config",
                "lombok.copyableAnnotations = [com.foo.Base]\n",
            ),
            (
                "svc/lombok.config",
                "lombok.copyableAnnotations += [com.acme.Qualifier]\n",
            ),
        ]);
        assert!(acc.copies("svc/src/A.java", "Base"));
        assert!(acc.copies("svc/src/A.java", "Qualifier"));
        assert!(!acc.copies("other/src/A.java", "Qualifier"));
        // Bare `!` clears the list root-ward files set.
        let cleared = maps(&[
            (
                "lombok.config",
                "lombok.copyableAnnotations = [com.foo.Base]\n",
            ),
            ("svc/lombok.config", "lombok.copyableAnnotations !\n"),
        ]);
        assert!(!cleared.copies("svc/src/A.java", "Base"));
        assert!(cleared.copyable("svc/src/A.java").is_some());
        // `stopBubbling` in the nearer file shields everything above it.
        let stopped = maps(&[
            (
                "lombok.config",
                "lombok.copyableAnnotations = [com.foo.Base]\n",
            ),
            (
                "svc/lombok.config",
                "config.stopBubbling = true\nlombok.copyableAnnotations += [com.acme.Qualifier]\n",
            ),
        ]);
        assert!(stopped.copies("svc/src/A.java", "Qualifier"));
        assert!(!stopped.copies("svc/src/A.java", "Base"));
        assert!(stopped.copies("other/src/A.java", "Base"));
        // Unreadable chains stay unread: no file, no entry, garbage on the key.
        assert!(maps(&[]).copyable("svc/src/A.java").is_none());
        assert!(
            maps(&[("lombok.config", "lombok.otherKey = [x]\n")])
                .copyable("svc/src/A.java")
                .is_none()
        );
        assert!(
            maps(&[("lombok.config", "lombok.copyableAnnotations ???\n")])
                .copyable("svc/src/A.java")
                .is_none()
        );
        assert!(
            maps(&[("lombok.config", "lombok.copyableAnnotations = [unclosed\n")])
                .copyable("svc/src/A.java")
                .is_none()
        );
        // Evaluator pass 1, H1: inline trailing comments strip (as Lombok
        // itself strips them), so a commented directive still applies.
        let commented = maps(&[(
            "lombok.config",
            "# copy qualifiers\nlombok.copyableAnnotations = [com.foo.Base] # the base\n",
        )]);
        assert!(commented.copies("svc/src/A.java", "Base"));
    }

    /// Contract M6c S8: a field qualifier rides the Lombok-generated parameter
    /// only when the chain positively lists it; otherwise the old row (recorded
    /// annotation, no qualifier).
    #[test]
    fn lombok_copied_qualifiers_ride_the_parameter() {
        let imports = "import lombok.RequiredArgsConstructor;\n";
        let body = "@Service @RequiredArgsConstructor class A { @Qualifier(\"x\") private final Repo repo; }\n";
        let row = |lombok: &LombokConfigs| {
            points_with(
                &with_imports(&format!("{imports}{body}")),
                "svc/src/main/java/com/acme/A.java",
                lombok,
            )
        };
        let listed = LombokConfigs::build(
            &[(
                "svc/lombok.config",
                "lombok.copyableAnnotations += [org.springframework.beans.factory.annotation.Qualifier]\n".to_owned(),
            )]
            .into_iter()
            .collect(),
        );
        assert_eq!(row(&listed), ["A.constructor repo: Repo q=x"]);
        // Simple names list too.
        let simple = LombokConfigs::build(
            &[(
                "lombok.config",
                "lombok.copyableAnnotations = [Qualifier]\n".to_owned(),
            )]
            .into_iter()
            .collect(),
        );
        assert_eq!(row(&simple), ["A.constructor repo: Repo q=x"]);
        // Unlisted, and unread chains: the old row.
        let other = LombokConfigs::build(
            &[(
                "lombok.config",
                "lombok.copyableAnnotations = [com.foo.Other]\n".to_owned(),
            )]
            .into_iter()
            .collect(),
        );
        assert_eq!(row(&other), ["A.constructor repo: Repo @Qualifier"]);
        assert_eq!(
            row(&LombokConfigs::default()),
            ["A.constructor repo: Repo @Qualifier"]
        );
    }

    /// Invariant 9: chain reads grow linearly in lookups (counted, 4× → ≤ 5×);
    /// the walk is bounded by directory depth, config count never by points.
    #[test]
    fn lombok_chain_reads_grow_linearly() {
        fn work_for(n: usize) -> u64 {
            let collected: BTreeMap<&str, String> = [(
                "svc/lombok.config",
                "lombok.copyableAnnotations = [Qualifier]\n".to_owned(),
            )]
            .into_iter()
            .collect();
            let maps = LombokConfigs::build(&collected);
            work::take();
            for i in 0..n {
                assert!(maps.copies(&format!("svc/src/p{i}/A.java"), "Qualifier"));
            }
            work::take()
        }
        let (small, large) = (work_for(50), work_for(200));
        assert!(large <= small * 5, "{small} -> {large}");
    }

    #[test]
    fn scan_roots_and_unread_attributes() {
        let src = "package com.acme.auth;\nimport org.springframework.boot.autoconfigure.SpringBootApplication;\nimport org.springframework.cloud.openfeign.EnableFeignClients;\nimport org.springframework.data.jpa.repository.config.EnableJpaRepositories;\nimport com.acme.lib.Marker;\n@SpringBootApplication @EnableFeignClients @EnableJpaRepositories(basePackages = {\"com.acme.repo\", \"com.acme.other\"})\nclass App {}\n@org.springframework.context.annotation.ComponentScan(basePackageClasses = Marker.class, lazyInit = true)\nclass Cfg {}\n@SpringBootApplication(scanBasePackages = BASE)\nclass Dyn {}\n";
        let (facts, out) = file_di(src);
        let got: Vec<String> = out
            .scans
            .iter()
            .map(|s| {
                format!(
                    "{} {:?} {}{}",
                    facts.definitions[s.class].name,
                    s.kind,
                    s.packages.join(","),
                    if s.unread { " unread" } else { "" }
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                "App Components com.acme.auth",
                "App Feign com.acme.auth",
                "App Repositories com.acme.other,com.acme.repo",
                "Cfg Components com.acme.lib unread",
                "Dyn Components com.acme.auth unread",
            ]
        );
    }

    #[test]
    fn decapitalize_follows_introspector() {
        for (name, want) in [
            ("FooBah", "fooBah"),
            ("X", "x"),
            ("URL", "URL"),
            ("URLParser", "URLParser"),
            ("a", "a"),
            ("", ""),
        ] {
            assert_eq!(decapitalize(name), want, "{name}");
        }
    }

    #[test]
    fn inert_annotations_are_package_gated() {
        let facts = facts_of(
            "package p;\nimport lombok.Getter;\nimport x.Data;\nimport y.NonNull;\n@Getter @Data @NonNull @Override @org.springframework.transaction.annotation.Transactional class K {}\n",
        );
        let inert: Vec<bool> = facts.definitions[0]
            .decorations
            .iter()
            .map(|d| is_inert(&facts, d))
            .collect();
        assert_eq!(inert, [true, false, true, true, true]);
    }

    /// S6 gate D, one row per surface class (a file with facts).
    #[test]
    fn gate_d_surface_in_facts() {
        let found = |src: &str| -> Vec<String> {
            let (_, out) = file_di(src);
            out.surface.into_iter().map(|f| f.what).collect()
        };
        for (src, want) in [
            // Plain names, other packages: nothing.
            (
                "package p;\nimport org.springframework.stereotype.Component;\nimport org.springframework.context.annotation.Bean;\nimport org.springframework.context.annotation.ConditionalOnX;\nimport org.springframework.context.ApplicationEventPublisher;\nimport org.springframework.boot.test.context.SpringBootTest;\nimport org.springframework.boot.autoconfigure.condition.ConditionalOnMissingBean;\nclass K {}\n",
                vec![],
            ),
            (
                "package p;\nimport org.springframework.beans.factory.config.BeanPostProcessor;\nclass K {}\n",
                vec!["import org.springframework.beans.factory.config.BeanPostProcessor"],
            ),
            (
                "package p;\nimport org.springframework.context.annotation.*;\nclass K {}\n",
                vec!["import org.springframework.context.annotation.*"],
            ),
            (
                "package p;\nimport static org.springframework.boot.SpringApplicationBuilder.run;\nclass K {}\n",
                vec!["import org.springframework.boot.SpringApplicationBuilder"],
            ),
            (
                "package p;\nimport org.springframework.context.annotation.ImportRuntimeHints;\nclass K {}\n",
                vec!["import org.springframework.context.annotation.ImportRuntimeHints"],
            ),
            (
                "package p;\nclass K implements org.springframework.beans.factory.FactoryBean<X> {}\n",
                vec!["qualified org.springframework.beans.factory.FactoryBean"],
            ),
            (
                "package p;\n@org.springframework.context.annotation.Fallback class K {}\n",
                vec!["qualified org.springframework.context.annotation.Fallback"],
            ),
            (
                "package p;\nclass K { Object o = new org.springframework.context.support.GenericApplicationContext(); }\n",
                vec!["qualified org.springframework use"],
            ),
        ] {
            let want: Vec<String> = want.into_iter().map(str::to_owned).collect();
            assert_eq!(found(src), want, "{src}");
        }
    }

    /// S6 gate D in files without facts.
    #[test]
    fn gate_d_surface_in_text() {
        for (path, text, want) in [
            (
                "src/main/resources/META-INF/spring.factories",
                "x=y",
                vec!["spring.factories"],
            ),
            (
                "src/main/resources/META-INF/spring/org.springframework.boot.autoconfigure.AutoConfiguration.imports",
                "a.B",
                vec!["spring.factories"],
            ),
            (
                "src/main/resources/META-INF/beans.xml",
                "<beans/>",
                vec!["beans.xml"],
            ),
            (
                "ctx.xml",
                "<beans xmlns=\"http://www.springframework.org/schema/beans\"/>",
                vec!["beans namespace"],
            ),
            (
                "src/main/kotlin/Cfg.kt",
                "@Configuration class Cfg { @Bean fun b() = B() }",
                vec!["raw Configuration", "raw Bean"],
            ),
            (
                "src/main/kotlin/R.kt",
                "ctx.registerBean(X::class.java)",
                vec!["raw registerBean"],
            ),
            // Words that are no annotation or call; prose.
            (
                "src/main/kotlin/K.kt",
                "val serviceName = \"Service\"; // a Bean here",
                vec![],
            ),
            ("README.md", "@Component and registerBean(x)", vec![]),
        ] {
            let want: Vec<String> = want.into_iter().map(str::to_owned).collect();
            assert_eq!(surface_text(path, Some(text)), want, "{path}");
        }
        assert_eq!(surface_text("big.kt", None), ["over the byte cap"]);
    }
}
