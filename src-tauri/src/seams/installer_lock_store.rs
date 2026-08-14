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
use serde::{Deserialize, Deserializer as SerdeDeserializer, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// The exact lock schema version this build audits; unknown future versions
/// are never interpreted as v3 (research "skill lock contract").
pub const LOCK_SCHEMA_VERSION: u64 = 3;

/// One strictly parsed v3 lock entry. `requested_ref` is absent exactly when
/// the installer did not record a ref (track `HEAD`); time fields and
/// `plugin_name` are display-only and never participate in trust (spec
/// §8.2).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
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

/// Why an exact-entry CAS lock rewrite refused to write (spec §8.4 step 4,
/// ADR-0013 §5.1): the file or entry changed since the plan was frozen, or
/// the file cannot be strictly parsed. A refusal never writes.
#[derive(Debug, Error)]
pub enum LockReleaseError {
    #[error("the lock file changed since the plan was frozen")]
    FingerprintChanged,
    #[error("the lock entry '{0}' is no longer present")]
    EntryMissing(String),
    #[error("the lock key '{0}' is already occupied")]
    EntryOccupied(String),
    #[error("the lock file cannot be strictly rewritten: {0}")]
    Invalid(String),
    #[error("the lock file could not be read or written: {0}")]
    Io(String),
}

/// Known installer lock locations, strict v3 parse and full fingerprint
/// (spec §4.6). Read-only in this slice.
pub trait InstallerLockStore: Send + Sync {
    /// Probe every known lock location and strictly parse each present file.
    /// Absent files simply produce no report; every present file yields a
    /// report, valid or faulted.
    fn discover(&self) -> Result<Vec<LockFileReport>, InstallerLockError>;

    /// CAS-release exactly one lock entry (the Ownership Handoff logical
    /// commit point): the full-file fingerprint and the exact entry must
    /// still match, then the entry is removed while every other top-level
    /// value, other entry and unknown JSON field is preserved and the file
    /// is re-serialized as a valid v3 lock (temp → fsync → rename → parent
    /// fsync). Any change refuses without writing.
    fn release_entry(
        &self,
        lock_path: &Path,
        frozen_fingerprint: &str,
        entry: &LockEntry,
    ) -> Result<(), LockReleaseError> {
        let _ = (lock_path, frozen_fingerprint, entry);
        Err(LockReleaseError::Invalid(
            "this lock store cannot rewrite lock files".into(),
        ))
    }

    /// CAS-restore one exact entry during conditional Undo (ADR-0013 §5.1):
    /// the lock must still parse strictly as v3 and the key must be
    /// unoccupied; every other value is preserved.
    fn restore_entry(&self, lock_path: &Path, entry: &LockEntry) -> Result<(), LockReleaseError> {
        let _ = (lock_path, entry);
        Err(LockReleaseError::Invalid(
            "this lock store cannot rewrite lock files".into(),
        ))
    }
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
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\\')
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
                fault: Some(LockFileFault::InvalidJson("skills must be present".into())),
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

/// Pure exact-entry CAS release (spec §8.4 step 4): returns the rewritten
/// v3 bytes only when the full-file fingerprint and the exact audited entry
/// still match. Every other top-level value, other entry and unknown JSON
/// field is preserved; the last entry removal keeps a valid empty `skills`
/// object. Never writes; the caller owns the atomic write protocol.
pub fn release_lock_entry_bytes(
    bytes: &[u8],
    frozen_fingerprint: &str,
    entry: &LockEntry,
) -> Result<Vec<u8>, LockReleaseError> {
    let current = format!("{:x}", Sha256::digest(bytes));
    if current != frozen_fingerprint {
        return Err(LockReleaseError::FingerprintChanged);
    }
    let value = parse_json_no_duplicates(bytes).map_err(LockReleaseError::Invalid)?;
    let object = value
        .as_object()
        .ok_or_else(|| LockReleaseError::Invalid("the lock file must be a JSON object".into()))?;
    let version = object
        .get("version")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| LockReleaseError::Invalid("version must be the audited v3".into()))?;
    if version != LOCK_SCHEMA_VERSION {
        return Err(LockReleaseError::Invalid(format!(
            "version {version} is not the audited v3"
        )));
    }
    let skills = object
        .get("skills")
        .and_then(|value| value.as_object())
        .ok_or_else(|| LockReleaseError::Invalid("skills must be an object".into()))?;
    let live = skills
        .get(&entry.name)
        .ok_or_else(|| LockReleaseError::EntryMissing(entry.name.clone()))?;
    // The entry object never carries its own key as a field; re-inject it
    // so the strict LockEntry deserialization matches the frozen shape.
    let mut live_with_name = live.clone();
    live_with_name
        .as_object_mut()
        .ok_or_else(|| LockReleaseError::Invalid("the entry must be an object".into()))?
        .insert("name".into(), serde_json::Value::String(entry.name.clone()));
    let live_entry: LockEntry = serde_json::from_value(live_with_name).map_err(|error| {
        LockReleaseError::Invalid(format!("the entry is not a strict v3 entry: {error}"))
    })?;
    if &live_entry != entry {
        return Err(LockReleaseError::Invalid(
            "the exact entry changed since the plan was frozen".into(),
        ));
    }
    let mut rewritten = object.clone();
    let skills = rewritten
        .get_mut("skills")
        .and_then(|value| value.as_object_mut())
        .expect("skills object verified above");
    skills.remove(&entry.name);
    serde_json::to_vec_pretty(&rewritten)
        .map_err(|error| LockReleaseError::Invalid(error.to_string()))
}

/// Pure exact-entry CAS restore (ADR-0013 §5.1 conditional Undo): returns
/// the rewritten v3 bytes only when the lock still parses strictly as v3
/// and the key is unoccupied. Every other value is preserved.
pub fn restore_lock_entry_bytes(
    bytes: &[u8],
    entry: &LockEntry,
) -> Result<Vec<u8>, LockReleaseError> {
    let value = parse_json_no_duplicates(bytes).map_err(LockReleaseError::Invalid)?;
    let object = value
        .as_object()
        .ok_or_else(|| LockReleaseError::Invalid("the lock file must be a JSON object".into()))?;
    let version = object
        .get("version")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| LockReleaseError::Invalid("version must be the audited v3".into()))?;
    if version != LOCK_SCHEMA_VERSION {
        return Err(LockReleaseError::Invalid(format!(
            "version {version} is not the audited v3"
        )));
    }
    let skills = object
        .get("skills")
        .and_then(|value| value.as_object())
        .ok_or_else(|| LockReleaseError::Invalid("skills must be an object".into()))?;
    if skills.contains_key(&entry.name) {
        return Err(LockReleaseError::EntryOccupied(entry.name.clone()));
    }
    let mut entry_value = serde_json::to_value(entry)
        .map_err(|error| LockReleaseError::Invalid(error.to_string()))?;
    entry_value
        .as_object_mut()
        .ok_or_else(|| LockReleaseError::Invalid("the entry must be an object".into()))?
        .remove("name");
    let mut rewritten = object.clone();
    let skills = rewritten
        .get_mut("skills")
        .and_then(|value| value.as_object_mut())
        .expect("skills object verified above");
    skills.insert(entry.name.clone(), entry_value);
    serde_json::to_vec_pretty(&rewritten)
        .map_err(|error| LockReleaseError::Invalid(error.to_string()))
}

/// Deserialize a JSON document rejecting duplicate object keys at every
/// nesting level; `Err` carries a human-readable reason.
fn parse_json_no_duplicates(bytes: &[u8]) -> Result<serde_json::Value, String> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let value = deserializer
        .deserialize_any(StrictValueVisitor)
        .map_err(|error| error.to_string())?;
    deserializer.end().map_err(|error| error.to_string())?;
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
                return Err(SerdeError::custom(format!("duplicate key: {key}")));
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

        let nested =
            br#"{"version": 3, "skills": {"a": {"sourceType": "github", "sourceType": "gitlab"}}}"#;
        let lock = report(nested);
        assert!(
            matches!(lock.fault, Some(LockFileFault::DuplicateKey(key)) if key == "sourceType")
        );
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
        assert!(reasons.iter().any(|(name, _)| *name == "missing-hash"));
        assert!(
            reasons
                .iter()
                .any(|(name, reason)| *name == "wrong-type" && reason.contains("sourceType"))
        );
        assert!(reasons.iter().any(|(name, _)| *name == "../escape"));
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

    fn entry(name: &str) -> LockEntry {
        LockEntry {
            name: name.into(),
            source_type: "github".into(),
            source: format!("acme/{name}"),
            source_url: format!("https://github.com/acme/{name}"),
            requested_ref: None,
            skill_path: format!("skills/{name}"),
            skill_folder_hash: "0123456789abcdef0123456789abcdef01234567".into(),
            installed_at: None,
            updated_at: None,
            plugin_name: None,
        }
    }

    fn fingerprint(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }

    #[test]
    fn release_entry_keeps_unknown_fields_and_other_entries() {
        let json = r#"{
            "version": 3,
            "installerVersion": "9.9.9",
            "installed": [{"name": "other"}],
            "skills": {
                "networking": {
                    "sourceType": "github",
                    "source": "acme/networking",
                    "sourceUrl": "https://github.com/acme/networking",
                    "skillPath": "skills/networking",
                    "skillFolderHash": "0123456789abcdef0123456789abcdef01234567"
                },
                "audio": {
                    "sourceType": "github",
                    "source": "acme/audio",
                    "sourceUrl": "https://github.com/acme/audio",
                    "skillPath": "skills/audio",
                    "skillFolderHash": "0123456789abcdef0123456789abcdef01234567"
                }
            }
        }"#;
        let bytes = json.as_bytes();
        let rewritten = release_lock_entry_bytes(bytes, &fingerprint(bytes), &entry("networking"))
            .expect("CAS release");
        let value: serde_json::Value = serde_json::from_slice(&rewritten).expect("rewritten JSON");
        assert_eq!(value["version"], 3);
        assert_eq!(value["installerVersion"], "9.9.9", "unknown top-level kept");
        assert_eq!(value["installed"][0]["name"], "other", "unknown array kept");
        let skills = value["skills"].as_object().expect("skills object");
        assert!(!skills.contains_key("networking"), "released entry gone");
        assert!(skills.contains_key("audio"), "other entry kept");
        // The rewritten file is a strict valid v3 lock.
        let report = report(&rewritten);
        assert_eq!(report.fault, None);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].name, "audio");
    }

    #[test]
    fn release_entry_keeps_a_valid_empty_lock_after_the_last_entry() {
        let bytes = valid_entry_json().into_bytes();
        let mut frozen = entry("networking");
        frozen.installed_at = Some("2026-08-01T00:00:00Z".into());
        frozen.updated_at = Some("2026-08-01T00:00:00Z".into());
        frozen.plugin_name = Some("networking".into());
        let rewritten =
            release_lock_entry_bytes(&bytes, &fingerprint(&bytes), &frozen).expect("CAS release");
        let value: serde_json::Value = serde_json::from_slice(&rewritten).expect("rewritten JSON");
        assert_eq!(value["version"], 3);
        assert!(
            value["skills"].as_object().expect("skills").is_empty(),
            "the last entry removal keeps a valid empty skills object"
        );
        assert_eq!(report(&rewritten).fault, None);
    }

    #[test]
    fn release_entry_refuses_when_the_fingerprint_changed() {
        let bytes = valid_entry_json().into_bytes();
        let error = release_lock_entry_bytes(&bytes, "deadbeef", &entry("networking"))
            .expect_err("changed fingerprint refuses");
        assert!(matches!(error, LockReleaseError::FingerprintChanged));
    }

    #[test]
    fn release_entry_refuses_when_the_entry_differs() {
        // The full-file fingerprint matches but the frozen entry differs
        // (a stale plan frozen against a different entry shape).
        let bytes = valid_entry_json().into_bytes();
        let mut other = entry("networking");
        other.skill_folder_hash = "ffffffffffffffffffffffffffffffffffffffff".into();
        let error = release_lock_entry_bytes(&bytes, &fingerprint(&bytes), &other)
            .expect_err("changed entry refuses");
        assert!(matches!(error, LockReleaseError::Invalid(_)));
    }

    #[test]
    fn restore_entry_refuses_when_the_key_is_occupied() {
        let bytes = valid_entry_json().into_bytes();
        let error = restore_lock_entry_bytes(&bytes, &entry("networking"))
            .expect_err("occupied key refuses");
        assert!(matches!(
            error,
            LockReleaseError::EntryOccupied(name) if name == "networking"
        ));
    }

    #[test]
    fn restore_entry_readds_the_exact_entry_and_keeps_other_values() {
        let bytes = valid_entry_json().into_bytes();
        let mut frozen = entry("networking");
        frozen.installed_at = Some("2026-08-01T00:00:00Z".into());
        frozen.updated_at = Some("2026-08-01T00:00:00Z".into());
        frozen.plugin_name = Some("networking".into());
        let released =
            release_lock_entry_bytes(&bytes, &fingerprint(&bytes), &frozen).expect("release");
        let restored = restore_lock_entry_bytes(&released, &frozen).expect("restore");
        let value: serde_json::Value = serde_json::from_slice(&restored).expect("restored JSON");
        assert_eq!(value["version"], 3);
        assert_eq!(
            value["skills"]["networking"]["skillFolderHash"],
            "0123456789abcdef0123456789abcdef01234567"
        );
        let report = report(&restored);
        assert_eq!(report.fault, None);
        assert_eq!(report.entries.len(), 1);
        assert_eq!(report.entries[0].name, "networking");
    }
}
