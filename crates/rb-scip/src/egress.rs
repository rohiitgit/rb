//! The fetch phase's egress (contract M5 D15, S21): a job's fetch containers attach only to an
//! internal Docker network whose one other member is a tinyproxy container started for that run,
//! which forwards nothing but `CONNECT <host>:443` for the hosts of an allow-list. This module
//! holds the allow-list and generates what the proxy and the clients read: the proxy's
//! configuration and filter, Maven's `settings.xml` and the package managers' environment.

use std::path::Path;

/// The locally built proxy image (`docker/egress-proxy`): the pinned Alpine base with
/// tinyproxy 1.11.3-r0. Absent, every job is `unsupported`: no fetch runs unguarded.
pub const PROXY_IMAGE: &str = "rb-egress-proxy:1.11.3";
/// The internal network the fetch containers attach to: no route out, no external DNS.
pub const FETCH_NETWORK: &str = "rb-scip-fetch";
/// The bridge network the proxy reaches the registries through.
pub const EGRESS_NETWORK: &str = "rb-scip-egress";
/// The label on both networks and on every proxy container (`rb gc --egress` removes them).
pub const LABEL: &str = "rb.egress";
/// The proxy's port on the fetch network.
pub const PROXY_PORT: u16 = 8888;
/// Where a run's proxy configuration directory is mounted in the proxy, and its client files
/// (`settings.xml`) in the fetch containers.
pub const PROXY_CONFIG_AT: &str = "/etc/tinyproxy";
pub const CLIENT_CONFIG_AT: &str = "/rb-egress";

/// The registries the built-in recipes fetch from: Maven Central (both of its host names) and
/// the npm and Yarn registries. The fixtures' fetches reach only `repo.maven.apache.org` and
/// `registry.npmjs.org` (measured through an open proxy, contract M5 amendment 12).
pub const REGISTRIES: &[&str] = &[
    "registry.npmjs.org",
    "registry.yarnpkg.com",
    "repo.maven.apache.org",
    "repo1.maven.org",
];

/// The fetch phase's allow-list: [`REGISTRIES`] plus the extra hosts the indexing host
/// configures (`RB_SCIP_REGISTRIES`), sorted, without repeats. Part of every index's cache key,
/// so a fetch that failed on a host outside the list is retried once the host is added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Egress {
    pub hosts: Vec<String>,
}

impl Default for Egress {
    fn default() -> Self {
        Egress::with(&[]).expect("the built-in registries are valid host names")
    }
}

impl Egress {
    /// [`REGISTRIES`] plus `extra`. Each extra host must be a lower-case DNS name of at least
    /// two labels whose last label is not numeric (no IP address, port, wildcard, scheme or
    /// path): the proxy's filter matches it literally.
    pub fn with(extra: &[String]) -> Result<Egress, String> {
        let mut hosts: Vec<String> = REGISTRIES.iter().map(|h| (*h).to_owned()).collect();
        for h in extra {
            let h = h.trim();
            if !host_name(h) {
                return Err(format!(
                    "invalid registry host {h:?}: use a lower-case DNS name such as \
                     repo.example.com (no scheme, port, path, wildcard or IP address)"
                ));
            }
            hosts.push(h.to_owned());
        }
        hosts.sort();
        hosts.dedup();
        Ok(Egress { hosts })
    }

    /// The proxy's configuration (tinyproxy 1.11): every request is checked against the filter
    /// file by its whole URL, which for `CONNECT` is `host:port` and for plain HTTP
    /// `http://host/…`; the filter lists `host:443` only, and anything that does not match is
    /// refused (`FilterDefaultDeny`), so plain HTTP, other ports and IP addresses never pass.
    /// Every connection and refusal is logged.
    pub fn proxy_config(&self) -> String {
        format!(
            "Port {PROXY_PORT}\nListen 0.0.0.0\nTimeout 600\nMaxClients 64\nLogLevel Connect\n\
             DisableViaHeader Yes\nConnectPort 443\nFilterDefaultDeny Yes\nFilterURLs On\n\
             FilterType fnmatch\nFilter \"{PROXY_CONFIG_AT}/filter\"\n"
        )
    }

    /// The filter: one `host:443` line per allowed host.
    pub fn proxy_filter(&self) -> String {
        self.hosts.iter().map(|h| format!("{h}:443\n")).collect()
    }

    /// Writes the proxy's `tinyproxy.conf` and `filter` into `dir` (mounted read-only at
    /// [`PROXY_CONFIG_AT`] in the proxy).
    pub fn write_config(&self, dir: &Path) -> anyhow::Result<()> {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("tinyproxy.conf"), self.proxy_config())?;
        std::fs::write(dir.join("filter"), self.proxy_filter())?;
        Ok(())
    }
}

/// A lower-case DNS name of at least two labels, the last not numeric.
fn host_name(h: &str) -> bool {
    let labels: Vec<&str> = h.split('.').collect();
    labels.len() >= 2
        && h.len() <= 253
        && labels.iter().all(|l| {
            !l.is_empty()
                && l.len() <= 63
                && !l.starts_with('-')
                && !l.ends_with('-')
                && l.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        && !labels
            .last()
            .is_some_and(|l| l.bytes().all(|b| b.is_ascii_digit()))
}

/// Maven's settings for a proxy at `address`: Maven ignores the proxy environment variables,
/// and selects a proxy by the repository's protocol, so one entry per protocol.
pub fn maven_settings(address: &str) -> String {
    let proxy = |protocol: &str| {
        format!(
            "    <proxy>\n      <id>rb-egress-{protocol}</id>\n      <active>true</active>\n      \
             <protocol>{protocol}</protocol>\n      <host>{address}</host>\n      \
             <port>{PROXY_PORT}</port>\n    </proxy>\n"
        )
    };
    format!(
        "<settings>\n  <proxies>\n{}{}  </proxies>\n</settings>\n",
        proxy("https"),
        proxy("http")
    )
}

/// The environment that sends npm, Yarn and pnpm (and anything else that honours the usual
/// variables) through the proxy at `address`.
pub fn proxy_env(address: &str) -> Vec<(String, String)> {
    let url = format!("http://{address}:{PROXY_PORT}");
    [
        "HTTPS_PROXY",
        "HTTP_PROXY",
        "https_proxy",
        "http_proxy",
        "npm_config_https_proxy",
        "npm_config_proxy",
        "YARN_HTTPS_PROXY",
        "YARN_HTTP_PROXY",
    ]
    .iter()
    .map(|k| ((*k).to_owned(), url.clone()))
    .collect()
}

/// The hosts the proxy refused, from its log (`Proxying refused on filtered url "…"`), sorted
/// and without repeats: the log's timestamps and order are not functions of the commit.
pub fn refusals(log: &str) -> Vec<String> {
    let mut out: Vec<String> = log
        .lines()
        .filter_map(|l| {
            let rest = l.split("Proxying refused on filtered url \"").nth(1)?;
            Some(rest.split('"').next()?.to_owned())
        })
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_allow_list_is_the_registries_plus_valid_extra_hosts() {
        assert_eq!(Egress::default().hosts, REGISTRIES);
        let extra = Egress::with(&["repo.spring.io".into(), "registry.npmjs.org".into()]).unwrap();
        assert_eq!(
            extra.hosts,
            [
                "registry.npmjs.org",
                "registry.yarnpkg.com",
                "repo.maven.apache.org",
                "repo.spring.io",
                "repo1.maven.org"
            ]
        );
        for bad in [
            "*.example.com",
            "Repo.Example.com",
            "example",
            "1.2.3.4",
            "repo.example.com:8443",
            "https://repo.example.com",
            "repo.example.com/maven2",
            "repo..example.com",
            "-repo.example.com",
            "",
        ] {
            assert!(Egress::with(&[bad.to_owned()]).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_proxy_allows_only_connect_to_listed_hosts_on_443() {
        let e = Egress::default();
        let conf = e.proxy_config();
        for line in [
            "FilterDefaultDeny Yes",
            "FilterURLs On",
            "FilterType fnmatch",
            "ConnectPort 443",
            "Filter \"/etc/tinyproxy/filter\"",
            "Port 8888",
            "LogLevel Connect",
        ] {
            assert!(conf.lines().any(|l| l == line), "{line}\n{conf}");
        }
        assert_eq!(
            e.proxy_filter(),
            "registry.npmjs.org:443\nregistry.yarnpkg.com:443\nrepo.maven.apache.org:443\nrepo1.maven.org:443\n"
        );
        let tmp = tempfile::tempdir().unwrap();
        e.write_config(tmp.path()).unwrap();
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("filter")).unwrap(),
            e.proxy_filter()
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("tinyproxy.conf")).unwrap(),
            conf
        );
    }

    #[test]
    fn clients_are_pointed_at_the_proxy() {
        let s = maven_settings("172.30.0.2");
        for want in [
            "<protocol>https</protocol>",
            "<protocol>http</protocol>",
            "<host>172.30.0.2</host>",
            "<port>8888</port>",
        ] {
            assert!(s.contains(want), "{want}\n{s}");
        }
        let env = proxy_env("172.30.0.2");
        assert!(env.contains(&("HTTPS_PROXY".into(), "http://172.30.0.2:8888".into())));
        assert!(env.contains(&(
            "npm_config_https_proxy".into(),
            "http://172.30.0.2:8888".into()
        )));
    }

    #[test]
    fn refusals_are_read_from_the_log_without_times() {
        let log = "CONNECT   Oct 06 23:33:11.069 [1]: Request (file descriptor 4): CONNECT example.com:443 HTTP/1.1\nNOTICE    Oct 06 23:33:11.069 [1]: Proxying refused on filtered url \"example.com:443\"\nNOTICE    Oct 06 23:34:11.000 [1]: Proxying refused on filtered url \"1.1.1.1:443\"\nNOTICE    Oct 06 23:35:11.000 [1]: Proxying refused on filtered url \"example.com:443\"\n";
        assert_eq!(refusals(log), ["1.1.1.1:443", "example.com:443"]);
        assert!(refusals("").is_empty());
    }
}
