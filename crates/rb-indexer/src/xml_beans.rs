//! Spring beans XML (contract M6j S4): `<bean>`, `<alias>` and
//! `<context:component-scan>` read off the generic [`Element`] tree. Pure functions of the
//! document text; the DI stage resolves classes and wires beans and points.

use crate::packages::{Element, parse_xml};

/// A `<bean>` that instantiates a class (contract M6j S4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct XmlBean {
    /// The `id`, first `name` token, or empty (a nameless bean is still a bean, by type).
    pub(crate) id: String,
    /// The `class` attribute; beans without one (templates, `parent` children) are skipped
    /// by the reader and never reach this struct.
    pub(crate) class: String,
    /// `primary="true"`.
    pub(crate) primary: bool,
    /// `constructor-arg`/`property` references, in document order.
    pub(crate) refs: Vec<XmlRef>,
    /// Span of the `<bean>` element.
    pub(crate) span: [i32; 4],
}

/// A `ref` edge inside a bean: a constructor argument or a set property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct XmlRef {
    pub(crate) kind: XmlRefKind,
    /// The property `name`, the constructor argument's `name`, `index`, or `#position`.
    pub(crate) name: String,
    /// The referenced bean id (`ref`).
    pub(crate) target: String,
    /// Span of the `constructor-arg`/`property` element.
    pub(crate) span: [i32; 4],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum XmlRefKind {
    ConstructorArg,
    Property,
}

/// A Spring beans document: beans, component-scan packages and aliases.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct XmlBeansDoc {
    pub(crate) beans: Vec<XmlBean>,
    /// `base-package` tokens of every `<context:component-scan>`.
    pub(crate) scans: Vec<String>,
    /// `(alias, name)` pairs: `alias` names the same bean as `name`.
    pub(crate) aliases: Vec<(String, String)>,
}

/// Reads a Spring beans document: root `<beans>` (prefixed or not) carrying the Spring
/// beans namespace. Anything else (MyBatis mappers, poms, plain XML) gives `None`;
/// unbalanced XML gives `None` (an unread file taints, never panics).
pub(crate) fn read_spring_beans(text: &str) -> Option<XmlBeansDoc> {
    if !text.contains("springframework.org/schema/beans") {
        return None;
    }
    let root = parse_xml(text)?;
    if local(&root.name) != "beans" {
        return None;
    }
    let mut doc = XmlBeansDoc::default();
    for child in &root.children {
        match local(&child.name) {
            "bean" => {
                if let Some(bean) = read_bean(child, text) {
                    doc.beans.push(bean);
                }
            }
            "alias" => {
                if let (Some(name), Some(alias)) = (child.attr("name"), child.attr("alias"))
                    && !name.is_empty()
                    && !alias.is_empty()
                {
                    doc.aliases.push((alias.to_owned(), name.to_owned()));
                }
            }
            "component-scan" => {
                if let Some(packages) = child.attr("base-package") {
                    for package in packages.split([',', ';', ' ', '\t', '\n']) {
                        let package = package.trim();
                        if !package.is_empty() && !doc.scans.iter().any(|p| p == package) {
                            doc.scans.push(package.to_owned());
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Some(doc)
}

/// A `<bean>` element, or `None` when it instantiates nothing (`abstract`, no `class`).
fn read_bean(element: &Element, text: &str) -> Option<XmlBean> {
    if element.attr("abstract").is_some_and(|v| v == "true") {
        return None;
    }
    let class = element.attr("class")?.to_owned();
    if class.is_empty() {
        return None;
    }
    let id = match element.attr("id") {
        Some(id) if !id.is_empty() => id.to_owned(),
        _ => element
            .attr("name")
            .unwrap_or_default()
            .split([',', ';', ' ', '\t', '\n'])
            .map(str::trim)
            .find(|t| !t.is_empty())
            .unwrap_or_default()
            .to_owned(),
    };
    let mut refs = Vec::new();
    let mut position = 0usize;
    for child in &element.children {
        match local(&child.name) {
            "constructor-arg" => {
                if let Some(target) = child.attr("ref") {
                    if target.is_empty() {
                        continue;
                    }
                    let name = child
                        .attr("name")
                        .or_else(|| child.attr("index"))
                        .map(str::to_owned)
                        .unwrap_or_else(|| format!("#{position}"));
                    position += 1;
                    refs.push(XmlRef {
                        kind: XmlRefKind::ConstructorArg,
                        name,
                        target: target.to_owned(),
                        span: span_of(text, child),
                    });
                }
            }
            "property" => {
                if let (Some(name), Some(target)) = (child.attr("name"), child.attr("ref")) {
                    if name.is_empty() || target.is_empty() {
                        continue;
                    }
                    refs.push(XmlRef {
                        kind: XmlRefKind::Property,
                        name: name.to_owned(),
                        target: target.to_owned(),
                        span: span_of(text, child),
                    });
                }
            }
            _ => {}
        }
    }
    Some(XmlBean {
        id,
        class,
        primary: element.attr("primary").is_some_and(|v| v == "true"),
        refs,
        span: span_of(text, element),
    })
}

/// The local tag name (`context:component-scan` → `component-scan`).
fn local(name: &str) -> &str {
    name.rsplit(':').next().unwrap_or(name)
}

/// Row/column span of a byte range in `text`.
fn span_of(text: &str, element: &Element) -> [i32; 4] {
    let (r0, c0) = point_of(text, element.start);
    let (r1, c1) = point_of(text, element.end);
    [r0, c0, r1, c1]
}

fn point_of(text: &str, offset: usize) -> (i32, i32) {
    let offset = offset.min(text.len());
    let mut row = 0i32;
    let mut col = 0i32;
    for byte in text.as_bytes()[..offset].iter() {
        if *byte == b'\n' {
            row += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (row, col)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEANS: &str = "<beans xmlns=\"http://www.springframework.org/schema/beans\"\n       xmlns:context=\"http://www.springframework.org/schema/context\">\n  <context:component-scan base-package=\"com.acme.scanned, com.acme.extra, com.acme.scanned\"/>\n  <bean id=\"store\" class=\"com.acme.shop.Store\">\n    <constructor-arg ref=\"db\"/>\n    <property name=\"name\" value=\"x\"/>\n    <property name=\"helper\" ref=\"helper\"/>\n  </bean>\n  <bean id=\"db\" class=\"com.acme.shop.Db\" primary=\"true\"/>\n  <alias name=\"store\" alias=\"shop\"/>\n  <bean id=\"proto\" class=\"com.acme.shop.Proto\" scope=\"prototype\"/>\n  <bean id=\"tmpl\" abstract=\"true\" class=\"com.acme.shop.Base\"/>\n  <bean class=\"com.acme.shop.Plain\"/>\n  <bean id=\"noclass\"/>\n</beans>\n";

    /// Contract M6j S4: beans, refs, scans and aliases read; templates, value-only
    /// properties and class-less beans skipped.
    #[test]
    fn beans_refs_scans_and_aliases() {
        let doc = read_spring_beans(BEANS).unwrap();
        assert_eq!(doc.scans, ["com.acme.scanned", "com.acme.extra"]);
        assert_eq!(doc.aliases, [("shop".to_owned(), "store".to_owned())]);
        let ids: Vec<&str> = doc.beans.iter().map(|b| b.id.as_str()).collect();
        assert_eq!(ids, ["store", "db", "proto", ""]);
        assert_eq!(doc.beans[0].class, "com.acme.shop.Store");
        assert!(!doc.beans[0].primary);
        assert!(doc.beans[1].primary);
        let refs: Vec<(bool, String, String)> = doc.beans[0]
            .refs
            .iter()
            .map(|r| {
                (
                    r.kind == XmlRefKind::ConstructorArg,
                    r.name.clone(),
                    r.target.clone(),
                )
            })
            .collect();
        assert_eq!(
            refs,
            [
                (true, "#0".to_owned(), "db".to_owned()),
                (false, "helper".to_owned(), "helper".to_owned()),
            ]
        );
        // The `value`-only property contributes no ref; spans land on their elements.
        assert_eq!(doc.beans[0].refs.len(), 2);
        assert_eq!(doc.beans[0].span[0], 3);
        assert_eq!(doc.beans[0].refs[0].span[0], 4);
    }

    /// Contract M6j S4: non-beans XML and unbalanced documents give `None`.
    #[test]
    fn non_beans_xml_gives_none() {
        assert!(read_spring_beans("<mapper namespace=\"x\"></mapper>").is_none());
        assert!(read_spring_beans("<beans></beans>").is_none());
        assert!(
            read_spring_beans("<beans xmlns=\"http://www.springframework.org/schema/beans\">")
                .is_none()
        );
    }
}
