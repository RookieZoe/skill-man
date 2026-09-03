//! Managed Skill facts for the source classification of a Scan Report
//! (spec §8.2, #84): the read-only Catalog facts the classifier needs to
//! mark already-Managed entities and typed name collisions. The seam has no
//! write method, so a Scan can never seed, migrate or repair — the same
//! discipline as the Source Capability Scan (#91).
//!
//! #91's Current/Legacy capability is consumed through these facts plus the
//! Catalog path authority: classification never guesses a skill's Managed
//! status from a schema version or a global UNIQUE.

use std::path::PathBuf;

/// One Managed Skill fact: the Directory Identity (display name, Source
/// Content) and the canonical final entity path the durable pointer holds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedSkillPathFact {
    pub directory_name: String,
    pub final_entity_path: PathBuf,
}

pub trait ScanManagedFactsReader: Send + Sync {
    /// Every Managed Skill of the active Bound Home (all source kinds).
    fn read(&self) -> Result<Vec<ManagedSkillPathFact>, String>;
}

/// Fail-closed default: no managed facts can never be fabricated until the
/// composition root wires the system adapter.
pub struct UnavailableScanManagedFactsReader;

impl ScanManagedFactsReader for UnavailableScanManagedFactsReader {
    fn read(&self) -> Result<Vec<ManagedSkillPathFact>, String> {
        Err("no Scan Managed Facts reader is configured".to_owned())
    }
}
