//! Canonical JSON digest for Scan Evidence Store artifacts (spec §3.6
//! "完整性校验"): the integrity field must not depend on key insertion
//! order, so the payload is canonicalized (recursively sorted keys) before
//! hashing. A tampered or torn artifact therefore fails its own digest and
//! reads as absent (`No cached report`), fail-closed.

use serde_json::Value;
use sha2::{Digest, Sha256};

pub fn canonical_json_digest(value: &Value) -> String {
    let canonical = canonicalize(value);
    let bytes = canonical.as_bytes();
    let digest = Sha256::digest(bytes);
    format!("sha256:{digest:x}")
}

/// Recursively sorted serialization: objects emit keys in BTreeMap order,
/// arrays keep their order, numbers/strings raw. Maps are rebuilt from
/// sorted keys so the digest is stable even if `serde_json` is built with
/// `preserve_order`.
fn canonicalize(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(number) => number.to_string(),
        Value::String(text) => escape_json_string(text),
        Value::Array(items) => {
            let mut out = String::from("[");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&canonicalize(item));
            }
            out.push(']');
            out
        }
        Value::Object(map) => {
            let mut entries: Vec<_> = map.iter().collect::<Vec<_>>();
            entries.sort_by_key(|(key, _)| *key);
            let mut out = String::from("{");
            for (index, (key, value)) in entries.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                out.push_str(&escape_json_string(key));
                out.push(':');
                out.push_str(&canonicalize(value));
            }
            out.push('}');
            out
        }
    }
}

fn escape_json_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_is_key_order_independent() {
        let first = serde_json::json!({"a": 1, "b": 2, "nested": {"z": true, "a": [1, 2]}});
        let second = serde_json::json!({"nested": {"a": [1, 2], "z": true}, "b": 2, "a": 1});
        assert_eq!(
            canonical_json_digest(&first),
            canonical_json_digest(&second)
        );
    }

    #[test]
    fn digest_changes_with_payload() {
        let value = serde_json::json!({"a": 1});
        let changed = serde_json::json!({"a": 2});
        assert_ne!(
            canonical_json_digest(&value),
            canonical_json_digest(&changed)
        );
    }
}
