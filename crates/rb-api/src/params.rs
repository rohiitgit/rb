//! Request parameters as a string map (contract M5 S12): the server's query string and the CLI's
//! flags both produce one, and every value is validated here, against D6's bounds, never clamped.

use std::collections::BTreeMap;

use rb_model::{Certainty, NodeKind};
use rb_store::code::Precision;

use crate::error::ApiError;

/// Parameter name → its text, each name at most once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Params(BTreeMap<String, String>);

impl Params {
    pub fn new() -> Self {
        Params::default()
    }

    /// From `(name, value)` pairs, as a query string decodes; a name given twice is refused.
    pub fn from_pairs<K: Into<String>, V: Into<String>>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Result<Self, ApiError> {
        let mut map = BTreeMap::new();
        for (k, v) in pairs {
            let k = k.into();
            if map.contains_key(&k) {
                return Err(ApiError::invalid(&k, "is given more than once"));
            }
            map.insert(k, v.into());
        }
        Ok(Params(map))
    }

    pub fn set(&mut self, key: &str, value: impl Into<String>) -> &mut Self {
        self.0.insert(key.to_owned(), value.into());
        self
    }

    /// Sets `key` when `value` is given.
    pub fn set_opt(&mut self, key: &str, value: Option<impl Into<String>>) -> &mut Self {
        if let Some(v) = value {
            self.set(key, v);
        }
        self
    }

    /// A boolean flag: `true` when set, absent (its default, `false`) otherwise.
    pub fn set_flag(&mut self, key: &str, on: bool) -> &mut Self {
        if on {
            self.set(key, "true");
        }
        self
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// Refuses any parameter not in `allowed` (no silent ignore, contract M5 S13).
    pub(crate) fn only(&self, allowed: &[&str]) -> Result<(), ApiError> {
        match self.0.keys().find(|k| !allowed.contains(&k.as_str())) {
            Some(k) => Err(ApiError::invalid(
                k,
                format_args!(
                    "is not a parameter of this request (allowed: {})",
                    allowed.join(", ")
                ),
            )),
            None => Ok(()),
        }
    }

    /// A required, non-empty text.
    pub(crate) fn text(&self, key: &str) -> Result<String, ApiError> {
        match self.get(key) {
            Some(v) if !v.is_empty() => Ok(v.to_owned()),
            _ => Err(ApiError::invalid(key, "is required")),
        }
    }

    /// An optional text; empty counts as given and is refused.
    pub(crate) fn text_opt(&self, key: &str) -> Result<Option<String>, ApiError> {
        match self.get(key) {
            None => Ok(None),
            Some(_) => self.text(key).map(Some),
        }
    }

    /// An integer within `min..=max`, `default` when absent. Only decimal digits are accepted.
    pub(crate) fn int(&self, key: &str, default: u64, min: u64, max: u64) -> Result<u64, ApiError> {
        Ok(self.int_opt(key, min, max)?.unwrap_or(default))
    }

    pub(crate) fn int_opt(&self, key: &str, min: u64, max: u64) -> Result<Option<u64>, ApiError> {
        let Some(v) = self.get(key) else {
            return Ok(None);
        };
        let n = (!v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
            .then(|| v.parse::<u64>().ok())
            .flatten();
        match n {
            Some(n) if (min..=max).contains(&n) => Ok(Some(n)),
            Some(n) => Err(ApiError::invalid(
                key,
                format_args!("must be between {min} and {max} (got {n})"),
            )),
            None => Err(ApiError::invalid(
                key,
                format_args!("must be an integer between {min} and {max} (got {v:?})"),
            )),
        }
    }

    /// `true` or `false`, `false` when absent.
    pub(crate) fn boolean(&self, key: &str) -> Result<bool, ApiError> {
        match self.get(key) {
            None | Some("false") => Ok(false),
            Some("true") => Ok(true),
            Some(v) => Err(ApiError::invalid(
                key,
                format_args!("must be true or false (got {v:?})"),
            )),
        }
    }

    pub(crate) fn certainty(&self, key: &str, default: Certainty) -> Result<Certainty, ApiError> {
        self.get(key).map_or(Ok(default), |v| {
            parse_certainty(v).map_err(|_| {
                ApiError::invalid(
                    key,
                    format_args!(
                        "must be one of ambiguous, heuristic, synthetic, dispatch, exact (got {v:?})"
                    ),
                )
            })
        })
    }

    pub(crate) fn precision(&self, key: &str, default: Precision) -> Result<Precision, ApiError> {
        self.get(key).map_or(Ok(default), |v| {
            parse_precision(v).map_err(|_| {
                ApiError::invalid(key, format_args!("must be one of rta, cha (got {v:?})"))
            })
        })
    }

    pub(crate) fn kind_opt(&self, key: &str) -> Result<Option<NodeKind>, ApiError> {
        self.get(key)
            .map(|v| {
                v.parse::<NodeKind>().map_err(|_| {
                    let kinds: Vec<&str> = NodeKind::ALL.iter().map(|k| k.as_str()).collect();
                    ApiError::invalid(
                        key,
                        format_args!("must be one of {} (got {v:?})", kinds.join(", ")),
                    )
                })
            })
            .transpose()
    }
}

pub fn parse_certainty(s: &str) -> Result<Certainty, String> {
    [
        Certainty::Ambiguous,
        Certainty::Heuristic,
        Certainty::Synthetic,
        Certainty::Dispatch,
        Certainty::Exact,
    ]
    .into_iter()
    .find(|c| c.as_str() == s)
    .ok_or_else(|| {
        format!("unknown certainty {s:?} (ambiguous, heuristic, synthetic, dispatch, exact)")
    })
}

pub fn parse_precision(s: &str) -> Result<Precision, String> {
    [Precision::Rta, Precision::Cha]
        .into_iter()
        .find(|p| p.as_str() == s)
        .ok_or_else(|| format!("unknown precision {s:?} (rta, cha)"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    fn params(pairs: &[(&str, &str)]) -> Params {
        Params::from_pairs(pairs.iter().copied()).unwrap()
    }

    #[test]
    fn integers_are_bounded_never_clamped() {
        let p = params(&[
            ("depth", "25"),
            ("hops", "9"),
            ("x", "007"),
            ("y", "-1"),
            ("z", "1e3"),
        ]);
        assert_eq!(p.int("depth", 1, 1, 25), Ok(25));
        assert_eq!(p.int("absent", 4, 0, 8), Ok(4));
        let e = p.int("hops", 4, 0, 8).unwrap_err();
        assert_eq!(
            (e.kind, e.message.as_str()),
            (
                ErrorKind::InvalidParam,
                "hops must be between 0 and 8 (got 9)"
            )
        );
        assert_eq!(p.int("x", 0, 0, 10), Ok(7));
        for bad in ["y", "z"] {
            assert!(
                p.int(bad, 0, 0, 10_000)
                    .unwrap_err()
                    .message
                    .contains("must be an integer"),
                "{bad}"
            );
        }
        let huge = params(&[("n", "99999999999999999999999")]);
        assert!(huge.int("n", 0, 0, 10).is_err());
    }

    #[test]
    fn duplicates_and_unknown_names_are_refused() {
        let e = Params::from_pairs([("a", "1"), ("a", "2")]).unwrap_err();
        assert_eq!(e.param.as_deref(), Some("a"));
        let p = params(&[("depth", "1"), ("dpeth", "2")]);
        let e = p.only(&["target", "depth"]).unwrap_err();
        assert_eq!(
            e.message,
            "dpeth is not a parameter of this request (allowed: target, depth)"
        );
        assert!(params(&[("depth", "1")]).only(&["target", "depth"]).is_ok());
    }

    #[test]
    fn booleans_texts_and_enums() {
        let p = params(&[
            ("full", "true"),
            ("include_references", "yes"),
            ("q", ""),
            ("min_certainty", "exact"),
            ("precision", "vta"),
            ("kind", "symbol"),
            ("bad_kind", "symbols"),
        ]);
        assert_eq!(p.boolean("full"), Ok(true));
        assert_eq!(p.boolean("absent"), Ok(false));
        assert!(p.boolean("include_references").is_err());
        assert_eq!(p.text("q").unwrap_err().message, "q is required");
        assert_eq!(
            p.certainty("min_certainty", Certainty::Ambiguous),
            Ok(Certainty::Exact)
        );
        assert_eq!(
            p.precision("precision", Precision::Cha)
                .unwrap_err()
                .message,
            "precision must be one of rta, cha (got \"vta\")"
        );
        assert_eq!(p.kind_opt("kind"), Ok(Some(NodeKind::Symbol)));
        assert!(p.kind_opt("bad_kind").is_err());
        assert_eq!(parse_precision("cha"), Ok(Precision::Cha));
        assert!(parse_certainty("sure").is_err());
    }
}
