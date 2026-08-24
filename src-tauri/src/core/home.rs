//! Home identity value objects (spec §3.3, CONTEXT "Home Identity").
//!
//! A logical Skill Man Home is proven by four matching values: the bootstrap
//! locator (`home-binding.json`), the Home marker (`.skill-man-home.json`),
//! the Catalog SQLite `catalog_meta` row and the current volume identity.
//! `BoundHome` is the verified value object all product write modules and the
//! `WriteGate::Open` state are constructed from; it can only be produced by
//! the bootstrap authority after that four-way check.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Stable logical Home identity: a UUID v4 generated once at first binding.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct HomeId(pub String);

impl HomeId {
    /// Validate the canonical UUID v4 text shape (`8-4-4-4-12` with version
    /// nibble `4`). Used when parsing locator, marker and Catalog identity
    /// values so a malformed id is a closed `AppStateUnavailable` /
    /// `HomeIdentityMismatch`, never a guess.
    pub fn parse(value: &str) -> Option<Self> {
        if !is_uuid_v4(value) {
            return None;
        }
        Some(Self(value.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Volume facts observed for a path. `fsid` is mount-scoped diagnostic data;
/// APFS `uuid` is the persistent volume identity used after a restart.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VolumeIdentity {
    pub fsid: String,
    pub uuid: String,
}

impl VolumeIdentity {
    /// Compare the persistent part of a recorded volume identity. macOS can
    /// assign a new `statfs.f_fsid` after restart, so using it to restore a
    /// Home would reject the same APFS volume.
    pub fn matches_persisted_uuid(&self, persisted_uuid: &str) -> bool {
        self.uuid == persisted_uuid
    }
}

/// The verified Home value object. Construction happens only inside the
/// bootstrap authority after locator/marker/Catalog/volume agree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundHome {
    pub home_id: HomeId,
    pub path: PathBuf,
    pub volume_fsid: String,
    pub volume_uuid: String,
    pub bound_at: String,
}

impl BoundHome {
    /// Test/prototype composition only: a dummy identity so services can be
    /// constructed with an open gate without a bootstrap authority. Real
    /// bound homes always come from `BootstrapService` verification.
    pub fn test_value(home_id: &str, path: PathBuf) -> Self {
        Self {
            home_id: HomeId(home_id.to_string()),
            path,
            volume_fsid: "test-fsid".into(),
            volume_uuid: "test-uuid".into(),
            bound_at: "2026-01-01T00:00:00Z".into(),
        }
    }
}

/// Parsed `.skill-man-home.json` marker. Unknown schema or malformed identity
/// values are a closed mismatch, never a repair.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HomeMarker {
    pub schema_version: u32,
    pub home_id: HomeId,
    pub volume_fsid: String,
    pub volume_uuid: String,
    pub created_at: String,
}

impl HomeMarker {
    pub const FILE_NAME: &'static str = ".skill-man-home.json";
    pub const SCHEMA_VERSION: u32 = 1;

    /// Strict parse: rejects unknown schema, non-UUID home ids and missing
    /// volume identity so a corrupt marker cannot be treated as Bound.
    pub fn parse(json: &str) -> Option<Self> {
        let marker: HomeMarker = serde_json::from_str(json).ok()?;
        if marker.schema_version != Self::SCHEMA_VERSION {
            return None;
        }
        HomeId::parse(&marker.home_id.0)?;
        if marker.volume_fsid.is_empty() || marker.volume_uuid.is_empty() {
            return None;
        }
        Some(marker)
    }

    /// Recovery Profile parsing validates the durable logical facts only.
    /// Volume fields remain parsed but are intentionally not used as recovery
    /// eligibility: mount identity is neither a locator nor proof of a Home.
    pub fn parse_recovery_profile(json: &str) -> Option<Self> {
        let marker: HomeMarker = serde_json::from_str(json).ok()?;
        if marker.schema_version != Self::SCHEMA_VERSION || marker.created_at.is_empty() {
            return None;
        }
        HomeId::parse(&marker.home_id.0)?;
        Some(marker)
    }

    pub fn matches_persistent_volume(&self, volume: &VolumeIdentity) -> bool {
        volume.matches_persisted_uuid(&self.volume_uuid)
    }
}

fn is_uuid_v4(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    let hex =
        |range: std::ops::Range<usize>| bytes[range].iter().all(|byte| byte.is_ascii_hexdigit());
    if bytes[8] != b'-' || bytes[13] != b'-' || bytes[18] != b'-' || bytes[23] != b'-' {
        return false;
    }
    if !hex(0..8) || !hex(9..13) || !hex(14..18) || !hex(19..23) || !hex(24..36) {
        return false;
    }
    // Version nibble must be 4; variant nibble must be 8/9/a/b.
    bytes[14] == b'4' && matches!(bytes[19], b'8' | b'9' | b'a' | b'b' | b'A' | b'B')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn home_id_accepts_only_uuid_v4_shape() {
        assert!(HomeId::parse("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab").is_some());
        assert!(HomeId::parse("b1c4e6f8-1a2b-4c3d-9e9f-0123456789ab").is_some());
        // Wrong version nibble (not 4).
        assert!(HomeId::parse("b1c4e6f8-1a2b-3c3d-8e9f-0123456789ab").is_none());
        // Wrong variant nibble.
        assert!(HomeId::parse("b1c4e6f8-1a2b-4c3d-7e9f-0123456789ab").is_none());
        // Wrong separators / length.
        assert!(HomeId::parse("b1c4e6f81a2b4c3d8e9f0123456789ab").is_none());
        assert!(HomeId::parse("").is_none());
    }

    #[test]
    fn marker_parse_rejects_unknown_schema_and_missing_volume() {
        let valid = r#"{
            "schema_version": 1,
            "home_id": "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
            "volume_fsid": "fsid-1",
            "volume_uuid": "uuid-1",
            "created_at": "2026-08-01T00:00:00Z"
        }"#;
        let marker = HomeMarker::parse(valid).expect("valid marker parses");
        assert!(marker.matches_persistent_volume(&VolumeIdentity {
            fsid: "fsid-after-reboot".into(),
            uuid: "uuid-1".into(),
        }));
        assert!(!marker.matches_persistent_volume(&VolumeIdentity {
            fsid: "fsid-1".into(),
            uuid: "uuid-other".into(),
        }));

        let bad_schema = valid.replace("\"schema_version\": 1", "\"schema_version\": 2");
        assert!(HomeMarker::parse(&bad_schema).is_none());

        let bad_id = valid.replace("b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab", "not-a-uuid");
        assert!(HomeMarker::parse(&bad_id).is_none());

        let missing_uuid = valid.replace(",\n            \"volume_uuid\": \"uuid-1\"", "");
        assert!(HomeMarker::parse(&missing_uuid).is_none());
    }
}
