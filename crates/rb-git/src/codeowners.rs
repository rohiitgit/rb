//! CODEOWNERS (design §4.2).
//!
//! GitHub semantics: the first file found among [`LOCATIONS`] is used; the **last** matching rule
//! wins; a matching rule with no owners clears ownership; invalid lines are skipped (and reported).

use crate::glob::Pattern;

/// Where GitHub looks for CODEOWNERS, in precedence order.
pub const LOCATIONS: [&str; 3] = [".github/CODEOWNERS", "CODEOWNERS", "docs/CODEOWNERS"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    /// 0-based line number in the CODEOWNERS file.
    pub line: usize,
    /// Length of the line in bytes (for evidence spans).
    pub line_len: usize,
    pub pattern: Pattern,
    /// Empty when the rule deliberately clears ownership.
    pub owners: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineError {
    pub line: usize,
    pub line_len: usize,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Codeowners {
    /// Repo-relative path of the CODEOWNERS file.
    pub path: String,
    pub rules: Vec<Rule>,
    pub errors: Vec<LineError>,
}

impl Codeowners {
    pub fn parse(path: &str, text: &str) -> Self {
        let mut rules = Vec::new();
        let mut errors = Vec::new();
        for (line, raw) in text.split('\n').enumerate() {
            let raw = raw.strip_suffix('\r').unwrap_or(raw);
            let line_len = raw.len();
            let content = if line == 0 {
                crate::strip_bom(raw)
            } else {
                raw
            };
            // Inline comments start at a token beginning with `#`.
            let tokens: Vec<&str> = content
                .split_whitespace()
                .take_while(|t| !t.starts_with('#'))
                .collect();
            let Some((pattern, owners)) = tokens.split_first() else {
                continue;
            };
            let parsed = Pattern::parse(pattern).and_then(|p| {
                match owners.iter().find(|o| !is_valid_owner(o)) {
                    Some(bad) => Err(format!("invalid owner {bad:?}")),
                    None => Ok(p),
                }
            });
            match parsed {
                Ok(pattern) => rules.push(Rule {
                    line,
                    line_len,
                    pattern,
                    // GitHub handles and emails are case-insensitive; one owner, one key.
                    owners: owners.iter().map(|o| o.to_lowercase()).collect(),
                }),
                Err(message) => errors.push(LineError {
                    line,
                    line_len,
                    message,
                }),
            }
        }
        Self {
            path: path.into(),
            rules,
            errors,
        }
    }

    /// The rule that decides ownership of `file` (the last matching one), if any.
    pub fn rule_for(&self, file: &str) -> Option<&Rule> {
        self.rules.iter().rev().find(|r| r.pattern.matches(file))
    }
}

/// `@user`, `@org/team`, or an email address.
fn is_valid_owner(owner: &str) -> bool {
    let name_ok = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    if let Some(handle) = owner.strip_prefix('@') {
        return match handle.split_once('/') {
            Some((org, team)) => name_ok(org) && name_ok(team),
            None => name_ok(handle),
        };
    }
    matches!(owner.split_once('@'), Some((local, domain))
        if !local.is_empty() && domain.contains('.') && !domain.contains('@'))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GitHub's documented example CODEOWNERS file (docs.github.com, "About code owners"),
    /// including its two alternative `/apps/` endings as separate files below.
    const GITHUB_EXAMPLE: &str = "\
# This is a comment.
*       @global-owner1 @global-owner2
*.js    @js-owner #This is an inline comment.
*.go docs@example.com
*.txt @octo-org/octocats
/build/logs/ @doctocat
docs/*  docs@example.com
apps/ @octocat
/docs/ @doctocat
/scripts/ @doctocat @octocat
**/logs @octocat
";

    fn owners(co: &Codeowners, file: &str) -> Option<Vec<String>> {
        co.rule_for(file).map(|r| r.owners.clone())
    }

    fn v(xs: &[&str]) -> Option<Vec<String>> {
        Some(xs.iter().map(|s| (*s).to_owned()).collect())
    }

    #[test]
    fn last_matching_rule_wins() {
        let co = Codeowners::parse(".github/CODEOWNERS", GITHUB_EXAMPLE);
        assert!(co.errors.is_empty(), "{:?}", co.errors);
        assert_eq!(
            owners(&co, "README.md"),
            v(&["@global-owner1", "@global-owner2"])
        );
        assert_eq!(owners(&co, "src/app.js"), v(&["@js-owner"]));
        assert_eq!(owners(&co, "cmd/main.go"), v(&["docs@example.com"]));
        assert_eq!(owners(&co, "notes.txt"), v(&["@octo-org/octocats"]));
        // `/build/logs/` matches, but the later `**/logs` wins.
        assert_eq!(owners(&co, "build/logs/x.log"), v(&["@octocat"]));
        // `docs/*` matches, but the later `/docs/` wins.
        assert_eq!(owners(&co, "docs/getting-started.md"), v(&["@doctocat"]));
        assert_eq!(
            owners(&co, "docs/build-app/troubleshooting.md"),
            v(&["@doctocat"])
        );
        assert_eq!(owners(&co, "web/apps/a.rb"), v(&["@octocat"]));
        assert_eq!(
            owners(&co, "scripts/deploy.sh"),
            v(&["@doctocat", "@octocat"])
        );
    }

    #[test]
    fn empty_owner_list_clears_ownership() {
        let co = Codeowners::parse("CODEOWNERS", "/apps/ @octocat\n/apps/github\n");
        assert_eq!(owners(&co, "apps/x.rb"), v(&["@octocat"]));
        assert_eq!(owners(&co, "apps/github/y.rb"), Some(vec![]));

        let co = Codeowners::parse("CODEOWNERS", "/apps/ @octocat\n/apps/github @doctocat\n");
        assert_eq!(owners(&co, "apps/github/y.rb"), v(&["@doctocat"]));
    }

    #[test]
    fn unmatched_files_have_no_rule() {
        let co = Codeowners::parse("CODEOWNERS", "/src/ @a\n");
        assert_eq!(co.rule_for("README.md"), None);
    }

    #[test]
    fn invalid_lines_are_reported_and_skipped() {
        let text = "*.js @a\n!b.js @b\n[ab].rs @c\n*.go not-an-owner\n*.py @x/\n*.md @ok\n";
        let co = Codeowners::parse("CODEOWNERS", text);
        let bad: Vec<usize> = co.errors.iter().map(|e| e.line).collect();
        assert_eq!(bad, [1, 2, 3, 4]);
        assert_eq!(co.rules.len(), 2);
        assert_eq!(owners(&co, "b.js"), v(&["@a"]));
        assert_eq!(co.rules[1].line, 5);
        assert_eq!(co.rules[1].line_len, "*.md @ok".len());
    }

    #[test]
    fn bom_and_crlf_are_handled() {
        let co = Codeowners::parse("CODEOWNERS", "\u{feff}* @a\r\n*.md @docs\r\n");
        assert!(co.errors.is_empty(), "{:?}", co.errors);
        assert_eq!(owners(&co, "x.txt"), v(&["@a"]));
        assert_eq!(owners(&co, "README.md"), v(&["@docs"]));
        // Spans count raw bytes: the BOM (3 bytes) stays in line 0, the `\r` is not part of a line.
        assert_eq!(co.rules[0].line_len, "\u{feff}* @a".len());
        assert_eq!(co.rules[1].line_len, "*.md @docs".len());
    }

    #[test]
    fn owners_are_case_insensitive() {
        let co = Codeowners::parse("CODEOWNERS", "* @Alice @Acme/Core Docs@Acme.io\n");
        assert_eq!(
            owners(&co, "a"),
            v(&["@alice", "@acme/core", "docs@acme.io"])
        );
    }

    #[test]
    fn owner_syntax() {
        for ok in ["@user", "@org/team-name", "@a.b_c", "docs@example.com"] {
            assert!(is_valid_owner(ok), "{ok}");
        }
        for bad in ["user", "@", "@org/", "@org/a/b", "a@b", "@us er"] {
            assert!(!is_valid_owner(bad), "{bad}");
        }
    }
}
