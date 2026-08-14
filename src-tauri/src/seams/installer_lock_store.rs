//! Installer lock evidence seam (spec §4.6, ADR-0013 §2): discover the known
//! `.skill-lock.json` locations, strictly parse the v3 schema, and record a
//! full byte fingerprint. The lock is a provenance hint, never an ownership
//! or content-integrity proof: entries only seed remote verification, and
//! file-level faults block every candidate the lock's installer root
//! governs while per-entry faults block only that entry.
//!
//! This slice is strictly read-only. The exact-entry CAS rewrite is the
//! Ownership Handoff logical commit point and is owned by the handoff
//! ticket; nothing here can write or rewrite a lock file.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::de::Error as SerdeError;
use serde::Deserializer as SerdeDeserializer;
use sha2::{Digest, Sha256};
use thiserror::Error;

/// The exact lock schema version this build audits; unknown future versions
/// are never interpreted as v3 (research "skill lock contract").
pub const LOCK_SCHEMA_VERSION: u64 = 3;

/// One strictly parsed v3 lock entry. `requested_ref` is absent exactly when
/// the installer did not record a ref (track `HEAD`); time fields and
/// `plugin_name` are display-only and never participate in trust (spec
/// §8.2).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockEntry {
    /// The object key; a safe directory name (ADR-0013 §2.2.1).
    pub name: String,
    pub source_type: String,
    pub source: String,
    pub source_url: String,
    pub requested_ref: Option<String>,
    pub skill_path: String,
    pub skill_folder_hash: String,
    pub installed_at: Option<String>,
    pub updated_at: Option<String>,
    pub plugin_name: Option<String>,
}

/// File-level faults (ADR-0013 §2.1): any one of them blocks every candidate
/// governed by the lock's installer root and forbids rewriting the file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LockFileFault {
    /// The file is not valid UTF-8.
    NotUtf8,
    /// The file is not structurally valid JSON or its shape is wrong.
    InvalidJson(String),
    /// The version is a number other than the audited v3.
    UnsupportedVersion(u64),
    /// A duplicate JSON key appears anywhere in the document.
    DuplicateKey(String),
}

/// A per-entry structural fault (ADR-0013 §2.1): it blocks only that entry;
/// the rest of the file remains usable evidence.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockEntryFault {
    pub name: String,
    pub reason: String,
}

/// One present lock file: its full byte fingerprint plus the strict parse
/// outcome. `fault` and `entries`/`entry_faults` are mutually exclusive
/// results of the parse — a file-level fault yields no entries.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LockFileReport {
    pub path: PathBuf,
    /// sha256 hex of the exact file bytes; any byte change breaks plan
    /// staleness checks (spec §8.1, §11).
    pub fingerprint: String,
    pub byte_len: u64,
    pub version: u64,
    pub fault: Option<LockFileFault>,
    pub entries: Vec<LockEntry>,
    pub entry_faults: Vec<LockEntryFault>,
}

#[derive(Debug, Error)]
pub enum InstallerLockError {
    #[error("{0}")]
    Validation(String),
    #[error("{operation} failed for '{}': {source}", path.display())]
    Io {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Known installer lock locations, strict v3 parse and full fingerprint
/// (spec §4.6). Read-only in this slice.
pub trait InstallerLockStore: Send + Sync {
    /// Probe every known lock location and strictly parse each present file.
    /// Absent files simply produce no report; every present file yields a
    /// report, valid or faulted.
    fn discover(&self) -> Result<Vec<LockFileReport>, InstallerLockError>;
}

/// Fail-closed default: no lock declarations anywhere. Used until the
/// composition root wires the system adapter; scans then classify every
/// candidate as Local (no fabricated lock evidence).
pub struct EmptyInstallerLockStore;

impl InstallerLockStore for EmptyInstallerLockStore {
    fn discover(&self) -> Result<Vec<LockFileReport>, InstallerLockError> {
        Ok(Vec::new())
    }
}

/// A safe v3 skill object key: a directory name that cannot escape the
/// installer root (ADR-0013 §2.2.1).
pub fn is_safe_lock_key(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\\')
}

/// Strictly parse lock file bytes into a report. Pure and total: every
/// outcome (valid, file fault, entry faults) is a report, never a panic.
pub fn parse_lock_bytes(path: &Path, bytes: &[u8]) -> LockFileReport {
    let fingerprint = format!("{:x}", Sha256::digest(bytes));
    let byte_len = bytes.len() as u64;
    let value = match parse_json_no_duplicates(bytes) {
        Ok(value) => value,
        Err(reason) => {
            let fault = if std::str::from_utf8(bytes).is_err() {
                LockFileFault::NotUtf8
            } else if let Some(key) = reason.strip_prefix("duplicate key:") {
                let key = key.trim();
                // serde_json appends a position suffix to custom messages.
                let key = key.split(" at line ").next().unwrap_or(key);
                LockFileFault::DuplicateKey(key.to_owned())
            } else {
                LockFileFault::InvalidJson(reason)
            };
            return LockFileReport {
                path: path.to_path_buf(),
                fingerprint,
                byte_len,
                version: 0,
                fault: Some(fault),
                entries: Vec::new(),
                entry_faults: Vec::new(),
            };
        }
    };
    let object = match value.as_object() {
        Some(object) => object,
        None => {
            return LockFileReport {
                path: path.to_path_buf(),
                fingerprint,
                byte_len,
                version: 0,
                fault: Some(LockFileFault::InvalidJson(
                    "the lock file must be a JSON object".into(),
                )),
                entries: Vec::new(),
                entry_faults: Vec::new(),
            };
        }
    };
    let version = match object.get("version") {
        Some(serde_json::Value::Number(number)) => number.as_u64(),
        Some(_) => None,
        None => None,
    };
    let Some(version) = version else {
        return LockFileReport {
            path: path.to_path_buf(),
            fingerprint,
            byte_len,
            version: 0,
            fault: Some(LockFileFault::InvalidJson(
                "version must be an integer".into(),
            )),
            entries: Vec::new(),
            entry_faults: Vec::new(),
        };
    };
    if version != LOCK_SCHEMA_VERSION {
        return LockFileReport {
            path: path.to_path_buf(),
            fingerprint,
            byte_len,
            version,
            fault: Some(LockFileFault::UnsupportedVersion(version)),
            entries: Vec::new(),
            entry_faults: Vec::new(),
        };
    }
    let skills = match object.get("skills") {
        Some(value) => value,
        None => {
            return LockFileReport {
                path: path.to_path_buf(),
                fingerprint,
                byte_len,
                version,
                fault: Some(LockFileFault::InvalidJson(
                    "skills must be present".into(),
                )),
                entries: Vec::new(),
                entry_faults: Vec::new(),
            };
        }
    };
    let skills = match skills.as_object() {
        Some(skills) => skills,
        None => {
            return LockFileReport {
                path: path.to_path_buf(),
                fingerprint,
                byte_len,
                version,
                fault: Some(LockFileFault::InvalidJson(
                    "skills must be an object".into(),
                )),
                entries: Vec::new(),
                entry_faults: Vec::new(),
            };
        }
    };
    let mut entries = Vec::new();
    let mut entry_faults = Vec::new();
    for (name, entry_value) in skills {
        if !is_safe_lock_key(name) {
            entry_faults.push(LockEntryFault {
                name: name.clone(),
                reason: "the lock key is not a safe directory name".into(),
            });
            continue;
        }
        let Some(entry) = entry_value.as_object() else {
            entry_faults.push(LockEntryFault {
                name: name.clone(),
                reason: "the entry must be an object".into(),
            });
            continue;
        };
        let string_field = |field: &str| -> Option<Result<String, String>> {
            match entry.get(field) {
                None => Some(Err(format!("{field} is missing"))),
                Some(serde_json::Value::String(value)) => Some(Ok(value.clone())),
                Some(_) => Some(Err(format!("{field} must be a string"))),
            }
        };
        let optional_string_field = |field: &str| -> Option<Result<Option<String>, String>> {
            match entry.get(field) {
                None => Some(Ok(None)),
                Some(serde_json::Value::String(value)) => Some(Ok(Some(value.clone()))),
                Some(_) => Some(Err(format!("{field} must be a string"))),
            }
        };
        let mut reasons = Vec::new();
        let source_type = string_field("sourceType");
        let source = string_field("source");
        let source_url = string_field("sourceUrl");
        let requested_ref = optional_string_field("ref");
        let skill_path = string_field("skillPath");
        let skill_folder_hash = string_field("skillFolderHash");
        let installed_at = optional_string_field("installedAt");
        let updated_at = optional_string_field("updatedAt");
        let plugin_name = optional_string_field("pluginName");
        for (field, result) in [
            ("sourceType", source_type.as_ref()),
            ("source", source.as_ref()),
            ("sourceUrl", source_url.as_ref()),
            ("skillPath", skill_path.as_ref()),
            ("skillFolderHash", skill_folder_hash.as_ref()),
        ] {
            match result {
                Some(Err(reason)) => reasons.push(format!("{field}: {reason}")),
                Some(Ok(value)) if field == "skillFolderHash" && value.trim().is_empty() => {
                    reasons.push("skillFolderHash: an empty hash is not allowed".into());
                }
                _ => {}
            }
        }
        for (field, result) in [
            ("ref", requested_ref.as_ref()),
            ("installedAt", installed_at.as_ref()),
            ("updatedAt", updated_at.as_ref()),
            ("pluginName", plugin_name.as_ref()),
        ] {
            match result {
                Some(Err(reason)) => reasons.push(format!("{field}: {reason}")),
                Some(Ok(Some(value))) if field == "ref" && value.trim().is_empty() => {
                    reasons.push("ref: an empty ref is not allowed".into());
                }
                _ => {}
            }
        }
        let Some(Ok(source_type)) = source_type else {
            entry_faults.push(LockEntryFault {
                name: name.clone(),
                reason: reasons.join("; "),
            });
            continue;
        };
        let Some(Ok(source)) = source else {
            entry_faults.push(LockEntryFault {
                name: name.clone(),
                reason: reasons.join("; "),
            });
            continue;
        };
        let Some(Ok(source_url)) = source_url else {
            entry_faults.push(LockEntryFault {
                name: name.clone(),
                reason: reasons.join("; "),
            });
            continue;
        };
        let Some(Ok(requested_ref)) = requested_ref else {
            entry_faults.push(LockEntryFault {
                name: name.clone(),
                reason: reasons.join("; "),
            });
            continue;
        };
        let Some(Ok(skill_path)) = skill_path else {
            entry_faults.push(LockEntryFault {
                name: name.clone(),
                reason: reasons.join("; "),
            });
            continue;
        };
        let Some(Ok(skill_folder_hash)) = skill_folder_hash else {
            entry_faults.push(LockEntryFault {
                name: name.clone(),
                reason: reasons.join("; "),
            });
            continue;
        };
        let Some(Ok(installed_at)) = installed_at else {
            entry_faults.push(LockEntryFault {
                name: name.clone(),
                reason: reasons.join("; "),
            });
            continue;
        };
        let Some(Ok(updated_at)) = updated_at else {
            entry_faults.push(LockEntryFault {
                name: name.clone(),
                reason: reasons.join("; "),
            });
            continue;
        };
        let Some(Ok(plugin_name)) = plugin_name else {
            entry_faults.push(LockEntryFault {
                name: name.clone(),
                reason: reasons.join("; "),
            });
            continue;
        };
        if !reasons.is_empty() {
            entry_faults.push(LockEntryFault {
                name: name.clone(),
                reason: reasons.join("; "),
            });
            continue;
        }
        entries.push(LockEntry {
            name: name.clone(),
            source_type,
            source,
            source_url,
            requested_ref,
            skill_path,
            skill_folder_hash,
            installed_at,
            updated_at,
            plugin_name,
        });
    }
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entry_faults.sort_by(|left, right| left.name.cmp(&right.name));
    LockFileReport {
        path: path.to_path_buf(),
        fingerprint,
        byte_len,
        version,
        fault: None,
        entries,
        entry_faults,
    }
}

/// Deserialize a JSON document rejecting duplicate object keys at every
/// nesting level; `Err` carries a human-readable reason.
fn parse_json_no_duplicates(bytes: &[u8]) -> Result<serde_json::Value, String> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = deserializer
        .deserialize_any(StrictValueVisitor)
        .map_err(|error| error.to_string())?;
    deserializer
        .end()
        .map_err(|error| error.to_string())?;
    Ok(value)
}

/// Visitor that errors on duplicate object keys, recursing with itself so
/// nested objects and arrays are checked too.
struct StrictValueVisitor;

impl<'de> serde::de::Visitor<'de> for StrictValueVisitor {
    type Value = serde_json::Value;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value without duplicate keys")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(serde_json::Value::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(serde_json::Value::Number(value.into()))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(serde_json::Value::Number(value.into()))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(serde_json::Value::Number)
            .ok_or_else(|| SerdeError::custom("invalid JSON number"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(serde_json::Value::String(value.to_owned()))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(serde_json::Value::String(value))
    }

    fn visit_bytes<E>(self, value: &[u8]) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        String::from_utf8(value.to_vec())
            .map(serde_json::Value::String)
            .map_err(|_| SerdeError::custom("invalid UTF-8 string"))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(serde_json::Value::Null)
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        Ok(serde_json::Value::Null)
    }

    fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
        D::Error: serde::de::Error,
    {
        deserializer.deserialize_any(self)
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut values = Vec::new();
        while let Some(value) = seq.next_element_seed(StrictValueSeed)? {
            values.push(value);
        }
        Ok(serde_json::Value::Array(values))
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        let mut keys: HashSet<String> = HashSet::new();
        let mut values = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key.clone()) {
                return Err(SerdeError::custom(format!(
                    "duplicate key: {key}"
                )));
            }
            let value = map.next_value_seed(StrictValueSeed)?;
            values.insert(key, value);
        }
        Ok(serde_json::Value::Object(values))
    }
}

/// Seed that re-enters `StrictValueVisitor` for every nested value.
struct StrictValueSeed;

impl<'de> serde::de::DeserializeSeed<'de> for StrictValueSeed {
    type Value = serde_json::Value;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictValueVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(bytes: &[u8]) -> LockFileReport {
        parse_lock_bytes(Path::new("/tmp/.skill-lock.json"), bytes)
    }

    fn valid_entry_json() -> String {
        r#"{
            "version": 3,
            "skills": {
                "networking": {
                    "sourceType": "github",
                    "source": "acme/networking",
                    "sourceUrl": "https://github.com/acme/networking",
                    "skillPath": "skills/networking",
                    "skillFolderHash": "0123456789abcdef0123456789abcdef01234567",
                    "installedAt": "2026-08-01T00:00:00Z",
                    "updatedAt": "2026-08-01T00:00:00Z",
                    "pluginName": "networking"
                }
            }
        }"#
        .to_owned()
    }

    #[test]
    fn strict_parse_accepts_a_valid_v3_lock() {
        let lock = report(valid_entry_json().as_bytes());
        assert_eq!(lock.fault, None);
        assert_eq!(lock.version, 3);
        assert_eq!(lock.entries.len(), 1);
        let entry = &lock.entries[0];
        assert_eq!(entry.name, "networking");
        assert_eq!(entry.source_type, "github");
        assert_eq!(entry.requested_ref, None);
        assert_eq!(entry.skill_folder_hash.len(), 40);
        assert!(lock.entry_faults.is_empty());
        assert!(lock.fingerprint.len() == 64);
        assert_eq!(lock.byte_len, valid_entry_json().len() as u64);
    }

    #[test]
    fn strict_parse_rejects_duplicate_keys_at_any_depth() {
        let bytes = br#"{"version": 3, "skills": {"a": {"sourceType": "github"}},"skills": {}}"#;
        let lock = report(bytes);
        assert!(matches!(
            lock.fault,
            Some(LockFileFault::DuplicateKey(key)) if key == "skills"
        ));
        assert!(lock.entries.is_empty());

        let nested = br#"{"version": 3, "skills": {"a": {"sourceType": "github", "sourceType": "gitlab"}}}"#;
        let lock = report(nested);
        assert!(matches!(lock.fault, Some(LockFileFault::DuplicateKey(key)) if key == "sourceType"));
    }

    #[test]
    fn strict_parse_rejects_non_utf8_bytes() {
        let mut bytes = valid_entry_json().into_bytes();
        bytes.push(0xff);
        let lock = report(&bytes);
        assert!(matches!(lock.fault, Some(LockFileFault::NotUtf8)));
    }

    #[test]
    fn strict_parse_rejects_invalid_json_and_wrong_shapes() {
        assert!(matches!(
            report(b"not json").fault,
            Some(LockFileFault::InvalidJson(_))
        ));
        assert!(matches!(
            report(b"[1,2,3]").fault,
            Some(LockFileFault::InvalidJson(_))
        ));
        assert!(matches!(
            report(br#"{"version": "3"}"#).fault,
            Some(LockFileFault::InvalidJson(_))
        ));
        assert!(matches!(
            report(br#"{"version": 3}"#).fault,
            Some(LockFileFault::InvalidJson(_))
        ));
        assert!(matches!(
            report(br#"{"version": 3, "skills": []}"#).fault,
            Some(LockFileFault::InvalidJson(_))
        ));
    }

    #[test]
    fn strict_parse_rejects_unknown_future_versions() {
        let bytes = br#"{"version": 4, "skills": {}}"#;
        let lock = report(bytes);
        assert!(matches!(
            lock.fault,
            Some(LockFileFault::UnsupportedVersion(4))
        ));
    }

    #[test]
    fn strict_parse_isolates_per_entry_faults() {
        let json = r#"{
            "version": 3,
            "skills": {
                "good": {
                    "sourceType": "github",
                    "source": "acme/good",
                    "sourceUrl": "https://github.com/acme/good",
                    "skillPath": "skills/good",
                    "skillFolderHash": "0123456789abcdef0123456789abcdef01234567"
                },
                "missing-hash": {
                    "sourceType": "github",
                    "source": "acme/missing-hash",
                    "sourceUrl": "https://github.com/acme/missing-hash",
                    "skillPath": "skills/missing-hash",
                    "skillFolderHash": ""
                },
                "wrong-type": {
                    "sourceType": 7,
                    "source": "acme/wrong-type",
                    "sourceUrl": "https://github.com/acme/wrong-type",
                    "skillPath": "skills/wrong-type",
                    "skillFolderHash": "0123456789abcdef0123456789abcdef01234567"
                },
                "../escape": {
                    "sourceType": "github",
                    "source": "acme/escape",
                    "sourceUrl": "https://github.com/acme/escape",
                    "skillPath": "skills/escape",
                    "skillFolderHash": "0123456789abcdef0123456789abcdef01234567"
                }
            }
        }"#;
        let lock = report(json.as_bytes());
        assert_eq!(lock.fault, None);
        assert_eq!(lock.entries.len(), 1);
        assert_eq!(lock.entries[0].name, "good");
        assert_eq!(lock.entry_faults.len(), 3);
        let reasons = lock
            .entry_faults
            .iter()
            .map(|fault| (fault.name.as_str(), fault.reason.as_str()))
            .collect::<Vec<_>>();
        assert!(reasons
            .iter()
            .any(|(name, _)| *name == "missing-hash"));
        assert!(reasons
            .iter()
            .any(|(name, reason)| *name == "wrong-type" && reason.contains("sourceType")));
        assert!(reasons
            .iter()
            .any(|(name, _)| *name == "../escape"));
    }

    #[test]
    fn strict_parse_preserves_unknown_fields_without_trusting_them() {
        let json = r#"{
            "version": 3,
            "futureTopLevel": {"x": 1},
            "skills": {
                "good": {
                    "sourceType": "github",
                    "source": "acme/good",
                    "sourceUrl": "https://github.com/acme/good",
                    "skillPath": "skills/good",
                    "skillFolderHash": "0123456789abcdef0123456789abcdef01234567",
                    "futureEntryField": "kept-but-not-trusted"
                }
            }
        }"#;
        let lock = report(json.as_bytes());
        assert_eq!(lock.fault, None);
        assert_eq!(lock.entries.len(), 1);
        assert_eq!(lock.entries[0].name, "good");
    }

    #[test]
    fn is_safe_lock_key_rejects_escaping_names() {
        assert!(is_safe_lock_key("networking"));
        assert!(is_safe_lock_key("with space"));
        assert!(!is_safe_lock_key(""));
        assert!(!is_safe_lock_key("."));
        assert!(!is_safe_lock_key(".."));
        assert!(!is_safe_lock_key("a/b"));
        assert!(!is_safe_lock_key("a\\b"));
    }
}
