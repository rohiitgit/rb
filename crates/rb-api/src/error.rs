//! Errors and their statuses (contract M5 D5): one table maps each kind to the CLI's exit code and
//! the server's HTTP status, so the two surfaces cannot disagree on what a failure is.

use std::fmt;

use serde::Serialize;
use serde_json::Value;

/// What went wrong, as named in an error body's `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// A parameter is unknown, missing, malformed or outside its bounds (never clamped, D6).
    InvalidParam,
    /// A repository, target, table, service or snapshot that does not exist.
    NotFound,
    /// Several candidates and no choice (design §9.1: never a silent pick); `candidates` lists them.
    Ambiguous,
    /// The repository exists but has no live snapshot.
    NoLiveSnapshot,
    /// The snapshot existed and retention has deleted it.
    SnapshotNotRetained,
    /// The database refused a Cypher query (syntax, a write, a privilege).
    Cypher,
    /// A Cypher query was cancelled by its statement timeout.
    CypherTimeout,
    /// Server only: every worker and queue slot is taken.
    Busy,
    /// Server only: the request did not finish within its wall-clock deadline.
    DeadlineExceeded,
    /// Anything else (a broken connection, corrupt rows).
    Internal,
    /// Server only: the `Host` header is not the server's loopback address (DNS rebinding).
    MisdirectedRequest,
    /// Server only: a method the endpoint does not take (`OPTIONS` included: no CORS).
    MethodNotAllowed,
    /// Server only: a `POST` body that is not `application/json`.
    UnsupportedMediaType,
}

impl ErrorKind {
    pub const ALL: [ErrorKind; 13] = [
        ErrorKind::InvalidParam,
        ErrorKind::NotFound,
        ErrorKind::Ambiguous,
        ErrorKind::NoLiveSnapshot,
        ErrorKind::SnapshotNotRetained,
        ErrorKind::Cypher,
        ErrorKind::CypherTimeout,
        ErrorKind::Busy,
        ErrorKind::DeadlineExceeded,
        ErrorKind::Internal,
        ErrorKind::MisdirectedRequest,
        ErrorKind::MethodNotAllowed,
        ErrorKind::UnsupportedMediaType,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ErrorKind::InvalidParam => "invalid_param",
            ErrorKind::NotFound => "not_found",
            ErrorKind::Ambiguous => "ambiguous",
            ErrorKind::NoLiveSnapshot => "no_live_snapshot",
            ErrorKind::SnapshotNotRetained => "snapshot_not_retained",
            ErrorKind::Cypher => "cypher",
            ErrorKind::CypherTimeout => "cypher_timeout",
            ErrorKind::Busy => "busy",
            ErrorKind::DeadlineExceeded => "deadline_exceeded",
            ErrorKind::Internal => "internal",
            ErrorKind::MisdirectedRequest => "misdirected_request",
            ErrorKind::MethodNotAllowed => "method_not_allowed",
            ErrorKind::UnsupportedMediaType => "unsupported_media_type",
        }
    }

    /// The CLI's exit code (D5). `busy`, `deadline_exceeded` and the three transport kinds only
    /// occur in the server.
    pub fn exit_code(self) -> u8 {
        match self {
            ErrorKind::InvalidParam => 2,
            ErrorKind::NotFound | ErrorKind::NoLiveSnapshot | ErrorKind::SnapshotNotRetained => 3,
            ErrorKind::Ambiguous => 4,
            ErrorKind::Cypher | ErrorKind::CypherTimeout => 5,
            ErrorKind::Busy
            | ErrorKind::DeadlineExceeded
            | ErrorKind::Internal
            | ErrorKind::MisdirectedRequest
            | ErrorKind::MethodNotAllowed
            | ErrorKind::UnsupportedMediaType => 1,
        }
    }

    /// The server's HTTP status (D5).
    pub fn status(self) -> u16 {
        match self {
            ErrorKind::InvalidParam => 400,
            ErrorKind::NotFound | ErrorKind::NoLiveSnapshot => 404,
            ErrorKind::Ambiguous => 409,
            ErrorKind::SnapshotNotRetained => 410,
            ErrorKind::Cypher | ErrorKind::CypherTimeout => 422,
            ErrorKind::Busy | ErrorKind::DeadlineExceeded => 503,
            ErrorKind::Internal => 500,
            ErrorKind::MisdirectedRequest => 421,
            ErrorKind::MethodNotAllowed => 405,
            ErrorKind::UnsupportedMediaType => 415,
        }
    }
}

/// A refused request: `{"api": 1, "error": {"kind", "message", "param"?, "candidates"?}}`.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiError {
    pub kind: ErrorKind,
    pub message: String,
    /// The parameter an `invalid_param` error is about, by its API name.
    pub param: Option<String>,
    /// For `ambiguous`: the candidates, in the order the CLI lists them.
    pub candidates: Vec<Value>,
}

/// Parameters given positionally on the command line: they have no flag to name.
const POSITIONAL: &[&str] = &["target", "q", "endpoint", "table", "name", "query"];

impl ApiError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        ApiError {
            kind,
            message: message.into(),
            param: None,
            candidates: Vec::new(),
        }
    }

    /// `invalid_param` about `param`: the message is `<param> <detail>`.
    pub fn invalid(param: &str, detail: impl fmt::Display) -> Self {
        ApiError {
            param: Some(param.to_owned()),
            ..ApiError::new(ErrorKind::InvalidParam, format!("{param} {detail}"))
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        ApiError::new(ErrorKind::NotFound, message)
    }

    pub fn ambiguous(message: impl Into<String>, candidates: Vec<Value>) -> Self {
        ApiError {
            candidates,
            ..ApiError::new(ErrorKind::Ambiguous, message)
        }
    }

    /// The error body, pretty-printed with a final newline, as the server sends it and as
    /// `rb … --json` prints it.
    pub fn body(&self) -> String {
        #[derive(Serialize)]
        struct Body<'a> {
            api: u32,
            error: Inner<'a>,
        }
        #[derive(Serialize)]
        struct Inner<'a> {
            kind: &'static str,
            message: &'a str,
            #[serde(skip_serializing_if = "Option::is_none")]
            param: Option<&'a str>,
            #[serde(skip_serializing_if = "<[Value]>::is_empty")]
            candidates: &'a [Value],
        }
        let body = Body {
            api: crate::API_VERSION,
            error: Inner {
                kind: self.kind.as_str(),
                message: &self.message,
                param: self.param.as_deref(),
                candidates: &self.candidates,
            },
        };
        let mut out = serde_json::to_string_pretty(&body).expect("an error body serialises");
        out.push('\n');
        out
    }

    /// The message as the CLI's text mode prints it: a named parameter appears as its flag
    /// (`depth` → `--depth`, `timeout_ms` → `--timeout-ms`).
    pub fn cli_text(&self) -> String {
        match &self.param {
            Some(p) if !POSITIONAL.contains(&p.as_str()) => match self.message.strip_prefix(p) {
                Some(rest) => format!("--{}{rest}", p.replace('_', "-")),
                None => self.message.clone(),
            },
            _ => self.message.clone(),
        }
    }
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

/// An `ApiError` carried through an `anyhow` chain is itself; a refused Cypher query is
/// `cypher`/`cypher_timeout`, or `invalid_param` for a snapshot without a projection (D17);
/// anything else is `internal`, with its whole chain as the message.
impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        let e = match e.downcast::<ApiError>() {
            Ok(api) => return api,
            Err(e) => e,
        };
        match e.downcast_ref::<rb_store::CypherError>() {
            Some(c @ rb_store::CypherError::NoProjection { .. }) => {
                ApiError::invalid("snapshot", format!("is not live: {c}"))
            }
            Some(c @ rb_store::CypherError::Timeout { .. }) => {
                ApiError::new(ErrorKind::CypherTimeout, c.to_string())
            }
            Some(c @ rb_store::CypherError::Refused(_)) => {
                ApiError::new(ErrorKind::Cypher, c.to_string())
            }
            None => ApiError::new(ErrorKind::Internal, format!("{e:#}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Contract M5 D5: the table, every kind once.
    #[test]
    fn every_kind_has_its_exit_code_and_status() {
        let table: Vec<(&str, u8, u16)> = ErrorKind::ALL
            .iter()
            .map(|k| (k.as_str(), k.exit_code(), k.status()))
            .collect();
        assert_eq!(
            table,
            [
                ("invalid_param", 2, 400),
                ("not_found", 3, 404),
                ("ambiguous", 4, 409),
                ("no_live_snapshot", 3, 404),
                ("snapshot_not_retained", 3, 410),
                ("cypher", 5, 422),
                ("cypher_timeout", 5, 422),
                ("busy", 1, 503),
                ("deadline_exceeded", 1, 503),
                ("internal", 1, 500),
                ("misdirected_request", 1, 421),
                ("method_not_allowed", 1, 405),
                ("unsupported_media_type", 1, 415),
            ]
        );
    }

    #[test]
    fn error_bodies_name_kind_message_param_and_candidates() {
        let e = ApiError::invalid("depth", "must be between 1 and 25 (got 26)");
        assert_eq!(
            e.body(),
            "{\n  \"api\": 1,\n  \"error\": {\n    \"kind\": \"invalid_param\",\n    \"message\": \"depth must be between 1 and 25 (got 26)\",\n    \"param\": \"depth\"\n  }\n}\n"
        );
        assert_eq!(e.cli_text(), "--depth must be between 1 and 25 (got 26)");
        assert_eq!(
            ApiError::invalid("timeout_ms", "must be between 1 and 30000 (got 0)").cli_text(),
            "--timeout-ms must be between 1 and 30000 (got 0)"
        );
        // A positional parameter has no flag: the message stays as it is.
        assert_eq!(
            ApiError::invalid("target", "is required").cli_text(),
            "target is required"
        );
        let a = ApiError::ambiguous(
            "x matches 2",
            vec![json!({"key": "a"}), json!({"key": "b"})],
        );
        let body: Value = serde_json::from_str(&a.body()).unwrap();
        assert_eq!(body["error"]["kind"], "ambiguous");
        assert_eq!(body["error"]["candidates"][1]["key"], "b");
        assert!(body["error"].get("param").is_none());
    }

    /// Errors travel through `anyhow` (the read transaction's closure) and come back as
    /// themselves; Cypher refusals are classified.
    #[test]
    fn anyhow_errors_are_classified() {
        let api = ApiError::not_found("no table");
        assert_eq!(ApiError::from(anyhow::Error::from(api.clone())), api);
        let timeout = anyhow::Error::from(rb_store::CypherError::Timeout { limit_ms: 5 });
        assert_eq!(ApiError::from(timeout).kind, ErrorKind::CypherTimeout);
        let refused = anyhow::Error::from(rb_store::CypherError::Refused("x".into()));
        assert_eq!(ApiError::from(refused).kind, ErrorKind::Cypher);
        let retired = anyhow::Error::from(rb_store::CypherError::NoProjection { snapshot: 3 });
        let e = ApiError::from(retired);
        assert_eq!(
            (e.kind, e.param.as_deref()),
            (ErrorKind::InvalidParam, Some("snapshot"))
        );
        let other = anyhow::anyhow!("inner").context("outer");
        assert_eq!(
            ApiError::from(other),
            ApiError::new(ErrorKind::Internal, "outer: inner")
        );
    }
}
