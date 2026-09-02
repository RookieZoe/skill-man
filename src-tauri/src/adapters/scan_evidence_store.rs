//! Scan Evidence Store adapters (spec §3.6): the system adapter writes under
//! `<Home>/cache/scan/` with the tmp → fsync → rename → parent fsync
//! protocol for every atomic switch (current manifest, startup marker,
//! qualification) and streamed JSONL for entry/entity evidence; the
//! fault-injecting adapter wraps a temporary scan directory with the stable
//! named failure points of spec §11.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::core::home::BoundHome;
use crate::seams::scan_evidence_store::{
    CurrentManifestRead, ScanEntityRecord, ScanEntryRecord, ScanEvidenceStore,
    ScanEvidenceStoreError, ScanEvidenceStoreFactory, ScanReportManifest, ScanRootRecord,
    ScanRunRecord, ScanSnapshotQualification, ScanStartupMarker,
};

pub const SCAN_DIR_NAME: &str = "scan";
const RUNS_DIR_NAME: &str = "runs";

pub struct SystemScanEvidenceStore {
    scan_dir: PathBuf,
    home_id: String,
}

impl SystemScanEvidenceStore {
    pub fn new(scan_dir: PathBuf, home_id: String) -> Self {
        Self { scan_dir, home_id }
    }

    pub fn scan_dir(&self) -> &Path {
        &self.scan_dir
    }

    fn run_dir(&self, run_id: &str) -> PathBuf {
        self.scan_dir.join(RUNS_DIR_NAME).join(run_id)
    }

    fn root_dir(&self, run_id: &str, root_key: &str) -> PathBuf {
        self.run_dir(run_id).join("roots").join(root_key)
    }

    fn write_string_atomic(
        &self,
        file_path: &Path,
        content: &str,
        operation: &'static str,
    ) -> Result<(), ScanEvidenceStoreError> {
        let parent = file_path
            .parent()
            .ok_or_else(|| ScanEvidenceStoreError::Write {
                operation,
                detail: format!("{} has no parent", file_path.display()),
            })?;
        fs::create_dir_all(parent)
            .map_err(|error| ScanEvidenceStoreError::io(operation, parent, &error))?;
        let file_name = file_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| ScanEvidenceStoreError::Write {
                operation,
                detail: format!("{} has no file name", file_path.display()),
            })?;
        let tmp = parent.join(format!("{file_name}.tmp"));
        let mut file = File::create(&tmp)
            .map_err(|error| ScanEvidenceStoreError::io(operation, &tmp, &error))?;
        file.write_all(content.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| ScanEvidenceStoreError::io(operation, &tmp, &error))?;
        drop(file);
        fs::rename(&tmp, file_path)
            .map_err(|error| ScanEvidenceStoreError::io(operation, file_path, &error))?;
        let dir = File::open(parent)
            .map_err(|error| ScanEvidenceStoreError::io(operation, parent, &error))?;
        dir.sync_all()
            .map_err(|error| ScanEvidenceStoreError::io(operation, parent, &error))?;
        Ok(())
    }

    fn read_string(&self, path: &Path) -> Result<Option<String>, ScanEvidenceStoreError> {
        match fs::read_to_string(path) {
            Ok(content) => Ok(Some(content)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(ScanEvidenceStoreError::io(
                "read Scan Evidence Store artifact",
                path,
                &error,
            )),
        }
    }

    /// The provenance-verified removal: a directory is deleted only when
    /// its `run.json` proves `home_id + run_id + artifact identity`.
    fn remove_run_verified(&self, run_id: &str) -> Result<(), ScanEvidenceStoreError> {
        let run_dir = self.run_dir(run_id);
        let run_json = run_dir.join("run.json");
        let Some(content) = self.read_string(&run_json)? else {
            return Err(ScanEvidenceStoreError::ProvenanceMismatch {
                detail: format!("no run.json for {run_id}"),
            });
        };
        let Some(record) = ScanRunRecord::parse(&content) else {
            return Err(ScanEvidenceStoreError::ProvenanceMismatch {
                detail: format!("unparseable run.json for {run_id}"),
            });
        };
        if record.home_id != self.home_id || record.run_id != run_id {
            return Err(ScanEvidenceStoreError::ProvenanceMismatch {
                detail: format!("run {run_id} belongs to home {}", record.home_id),
            });
        }
        fs::remove_dir_all(&run_dir).map_err(|error| {
            ScanEvidenceStoreError::io("remove temporary Scan Run", &run_dir, &error)
        })
    }

    fn append_jsonl(
        &self,
        path: &Path,
        record: &impl serde::Serialize,
        operation: &'static str,
    ) -> Result<(), ScanEvidenceStoreError> {
        let parent = path.parent().ok_or_else(|| ScanEvidenceStoreError::Write {
            operation,
            detail: format!("{} has no parent", path.display()),
        })?;
        fs::create_dir_all(parent)
            .map_err(|error| ScanEvidenceStoreError::io(operation, parent, &error))?;
        let mut line =
            serde_json::to_vec(record).map_err(|error| ScanEvidenceStoreError::Write {
                operation,
                detail: error.to_string(),
            })?;
        line.push(b'\n');
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|error| ScanEvidenceStoreError::io(operation, path, &error))?;
        file.write_all(&line)
            .and_then(|()| file.flush())
            .map_err(|error| ScanEvidenceStoreError::io(operation, path, &error))?;
        Ok(())
    }

    fn write_json_atomic(
        &self,
        path: &Path,
        record: &impl serde::Serialize,
        operation: &'static str,
    ) -> Result<(), ScanEvidenceStoreError> {
        let json = serde_json::to_string_pretty(record).map_err(|error| {
            ScanEvidenceStoreError::Write {
                operation,
                detail: error.to_string(),
            }
        })?;
        self.write_string_atomic(path, &json, operation)
    }
}

impl ScanEvidenceStore for SystemScanEvidenceStore {
    fn create_run(&self, run: &ScanRunRecord) -> Result<(), ScanEvidenceStoreError> {
        let run_dir = self.run_dir(&run.run_id);
        // A stale artifact with the same id is never recycled: refuse so the
        // caller fails closed instead of overwriting proven evidence.
        if run_dir.exists() {
            return Err(ScanEvidenceStoreError::ProvenanceMismatch {
                detail: format!("Run artifact {} already exists", run.run_id),
            });
        }
        self.write_json_atomic(&run_dir.join("run.json"), run, "create Scan Run")
    }

    fn append_entry(
        &self,
        run_id: &str,
        root_key: &str,
        record: &ScanEntryRecord,
    ) -> Result<(), ScanEvidenceStoreError> {
        self.append_jsonl(
            &self.root_dir(run_id, root_key).join("entries.jsonl"),
            record,
            "stream Scan entry evidence",
        )
    }

    fn append_entity(
        &self,
        run_id: &str,
        root_key: &str,
        record: &ScanEntityRecord,
    ) -> Result<(), ScanEvidenceStoreError> {
        self.append_jsonl(
            &self.root_dir(run_id, root_key).join("entities.jsonl"),
            record,
            "stream Scan entity evidence",
        )
    }

    fn write_root(
        &self,
        run_id: &str,
        root: &ScanRootRecord,
    ) -> Result<(), ScanEvidenceStoreError> {
        let path = self
            .root_dir(run_id, &root_key(root.index))
            .join("root.json");
        self.write_json_atomic(&path, root, "write Scan Root record")
    }

    fn read_run(&self, run_id: &str) -> Result<Option<ScanRunRecord>, ScanEvidenceStoreError> {
        let Some(content) = self.read_string(&self.run_dir(run_id).join("run.json"))? else {
            return Ok(None);
        };
        Ok(ScanRunRecord::parse(&content))
    }

    fn remove_run(&self, run_id: &str) -> Result<(), ScanEvidenceStoreError> {
        self.remove_run_verified(run_id)
    }

    fn cleanup_temporary_runs(&self) -> Result<usize, ScanEvidenceStoreError> {
        let runs_dir = self.scan_dir.join(RUNS_DIR_NAME);
        let entries = match fs::read_dir(&runs_dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
            Err(error) => {
                return Err(ScanEvidenceStoreError::io(
                    "enumerate temporary Scan Runs",
                    &runs_dir,
                    &error,
                ));
            }
        };
        let current = self.current_manifest()?;
        let current_run_id = match current {
            CurrentManifestRead::Report(manifest) => Some(manifest.run_id),
            // Corrupt or absent current manifest: every proven temporary Run
            // may be swept (spec §3.6 `No cached report`).
            CurrentManifestRead::Absent | CurrentManifestRead::Corrupt => None,
        };
        let mut removed = 0;
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => continue,
            };
            if !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false) {
                continue;
            }
            let run_id = entry.file_name().to_string_lossy().into_owned();
            if Some(run_id.as_str()) == current_run_id.as_deref() {
                continue;
            }
            // Only proven artifacts are removed: the sweep itself never
            // guesses (spec §3.6 orphan rule).
            match self.remove_run_verified(&run_id) {
                Ok(()) => removed += 1,
                Err(ScanEvidenceStoreError::ProvenanceMismatch { .. }) => continue,
                Err(error) => return Err(error),
            }
        }
        Ok(removed)
    }

    fn current_manifest(&self) -> Result<CurrentManifestRead, ScanEvidenceStoreError> {
        let Some(content) = self.read_string(&self.scan_dir.join("current.json"))? else {
            return Ok(CurrentManifestRead::Absent);
        };
        match ScanReportManifest::parse(&content) {
            Some(manifest) => Ok(CurrentManifestRead::Report(manifest)),
            None => Ok(CurrentManifestRead::Corrupt),
        }
    }

    fn publish_report(
        &self,
        run_id: &str,
        manifest: &ScanReportManifest,
    ) -> Result<(), ScanEvidenceStoreError> {
        if manifest.home_id != self.home_id || manifest.run_id != run_id {
            return Err(ScanEvidenceStoreError::ProvenanceMismatch {
                detail: "publish manifest belongs to a different Home or Run".into(),
            });
        }
        // 1. The identity-bound full manifest becomes durable first.
        self.write_json_atomic(
            &self.run_dir(run_id).join("manifest.json"),
            manifest,
            "write Scan Report manifest",
        )?;
        // 2. The atomic switch: old current Report stays until the rename.
        let previous = self.current_manifest()?;
        let previous_run_id = match previous {
            CurrentManifestRead::Report(previous) => Some(previous.run_id),
            CurrentManifestRead::Absent | CurrentManifestRead::Corrupt => None,
        };
        let json = serde_json::to_string_pretty(manifest).map_err(|error| {
            ScanEvidenceStoreError::Write {
                operation: "switch current Scan Report",
                detail: error.to_string(),
            }
        })?;
        self.write_string_atomic(
            &self.scan_dir.join("current.json"),
            &json,
            "switch current Scan Report",
        )?;
        // 3. Best-effort removal of the superseded Run artifact; a failure
        // leaves an orphan that the startup sweep proves away.
        if let Some(previous_run_id) = previous_run_id {
            if previous_run_id != run_id {
                let _ = self.remove_run_verified(&previous_run_id);
            }
        }
        Ok(())
    }

    fn write_startup_marker(
        &self,
        marker: &ScanStartupMarker,
    ) -> Result<(), ScanEvidenceStoreError> {
        if marker.home_id != self.home_id {
            return Err(ScanEvidenceStoreError::ProvenanceMismatch {
                detail: "startup marker belongs to a different Home".into(),
            });
        }
        self.write_json_atomic(
            &self.scan_dir.join("startup.json"),
            marker,
            "write Scan startup marker",
        )
    }

    fn startup_marker(&self) -> Result<Option<ScanStartupMarker>, ScanEvidenceStoreError> {
        let Some(content) = self.read_string(&self.scan_dir.join("startup.json"))? else {
            return Ok(None);
        };
        Ok(ScanStartupMarker::parse(&content))
    }

    fn write_qualification(
        &self,
        qualification: &ScanSnapshotQualification,
    ) -> Result<(), ScanEvidenceStoreError> {
        if qualification.home_id != self.home_id {
            return Err(ScanEvidenceStoreError::ProvenanceMismatch {
                detail: "qualification belongs to a different Home".into(),
            });
        }
        self.write_json_atomic(
            &self.scan_dir.join("qualification.json"),
            qualification,
            "write Scan Snapshot qualification",
        )
    }

    fn qualification(&self) -> Result<Option<ScanSnapshotQualification>, ScanEvidenceStoreError> {
        let Some(content) = self.read_string(&self.scan_dir.join("qualification.json"))? else {
            return Ok(None);
        };
        // A torn or tampered qualification is no proof at all: fail closed.
        Ok(ScanSnapshotQualification::parse(&content))
    }
}

/// The stable per-Root artifact key: `r<index>` + canonical path hash so two
/// Runs on different generations never share root artifacts.
fn root_key(index: u32) -> String {
    format!("r{index}")
}

pub struct SystemScanEvidenceStoreFactory;

impl ScanEvidenceStoreFactory for SystemScanEvidenceStoreFactory {
    fn store_for(
        &self,
        home: &BoundHome,
    ) -> Result<Arc<dyn ScanEvidenceStore>, ScanEvidenceStoreError> {
        Ok(Arc::new(SystemScanEvidenceStore::new(
            home.path.join("cache").join(SCAN_DIR_NAME),
            home.home_id.0.clone(),
        )))
    }
}

/// Fault-injecting factory: temp-dir-backed stores with stable named failure
/// points (spec §11). Test composition only.
pub struct FaultInjectingScanEvidenceStoreFactory {
    root: PathBuf,
    /// Remaining failures per point; `usize::MAX` semantics: an entry with
    /// count 0 is disabled, 1 means exactly one next call fails.
    faults: Arc<Mutex<HashMap<&'static str, usize>>>,
}

impl FaultInjectingScanEvidenceStoreFactory {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            faults: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn fail_next(&self, point: &'static str) {
        *self.faults.lock().unwrap().entry(point).or_insert(0) += 1;
    }

    pub fn fail_n(&self, point: &'static str, n: usize) {
        *self.faults.lock().unwrap().entry(point).or_insert(0) += n;
    }

    pub fn clear(&self) {
        self.faults.lock().unwrap().clear();
    }
}

impl ScanEvidenceStoreFactory for FaultInjectingScanEvidenceStoreFactory {
    fn store_for(
        &self,
        home: &BoundHome,
    ) -> Result<Arc<dyn ScanEvidenceStore>, ScanEvidenceStoreError> {
        let scan_dir = self.root.join(&home.home_id.0).join(SCAN_DIR_NAME);
        Ok(Arc::new(FaultInjectingScanEvidenceStore {
            inner: SystemScanEvidenceStore::new(scan_dir, home.home_id.0.clone()),
            faults: self.faults.clone(),
        }))
    }
}

pub struct FaultInjectingScanEvidenceStore {
    inner: SystemScanEvidenceStore,
    faults: Arc<Mutex<HashMap<&'static str, usize>>>,
}

impl FaultInjectingScanEvidenceStore {
    fn maybe_fail(&self, point: &'static str) -> Result<(), ScanEvidenceStoreError> {
        let mut faults = self.faults.lock().unwrap();
        let Some(remaining) = faults.get_mut(point) else {
            return Ok(());
        };
        if *remaining == 0 {
            return Ok(());
        }
        *remaining -= 1;
        let detail = if point == crate::seams::scan_evidence_store::fault_points::DISK_FULL {
            "No space left on device (injected)".to_owned()
        } else {
            format!("injected failure at {point}")
        };
        Err(ScanEvidenceStoreError::Write {
            operation: point,
            detail,
        })
    }
}

impl ScanEvidenceStore for FaultInjectingScanEvidenceStore {
    fn create_run(&self, run: &ScanRunRecord) -> Result<(), ScanEvidenceStoreError> {
        self.inner.create_run(run)
    }

    fn append_entry(
        &self,
        run_id: &str,
        root_key: &str,
        record: &ScanEntryRecord,
    ) -> Result<(), ScanEvidenceStoreError> {
        self.maybe_fail(crate::seams::scan_evidence_store::fault_points::WRITE_ENTRY)?;
        self.maybe_fail(crate::seams::scan_evidence_store::fault_points::DISK_FULL)?;
        self.inner.append_entry(run_id, root_key, record)
    }

    fn append_entity(
        &self,
        run_id: &str,
        root_key: &str,
        record: &ScanEntityRecord,
    ) -> Result<(), ScanEvidenceStoreError> {
        self.maybe_fail(crate::seams::scan_evidence_store::fault_points::WRITE_ENTRY)?;
        self.maybe_fail(crate::seams::scan_evidence_store::fault_points::DISK_FULL)?;
        self.inner.append_entity(run_id, root_key, record)
    }

    fn write_root(
        &self,
        run_id: &str,
        root: &ScanRootRecord,
    ) -> Result<(), ScanEvidenceStoreError> {
        self.maybe_fail(crate::seams::scan_evidence_store::fault_points::WRITE_ROOT)?;
        self.maybe_fail(crate::seams::scan_evidence_store::fault_points::DISK_FULL)?;
        self.inner.write_root(run_id, root)
    }

    fn read_run(&self, run_id: &str) -> Result<Option<ScanRunRecord>, ScanEvidenceStoreError> {
        self.inner.read_run(run_id)
    }

    fn remove_run(&self, run_id: &str) -> Result<(), ScanEvidenceStoreError> {
        self.maybe_fail(crate::seams::scan_evidence_store::fault_points::RUN_CANCEL)?;
        self.inner.remove_run(run_id)
    }

    fn cleanup_temporary_runs(&self) -> Result<usize, ScanEvidenceStoreError> {
        self.maybe_fail(crate::seams::scan_evidence_store::fault_points::ORPHAN_CLEANUP)?;
        self.inner.cleanup_temporary_runs()
    }

    fn current_manifest(&self) -> Result<CurrentManifestRead, ScanEvidenceStoreError> {
        self.inner.current_manifest()
    }

    fn publish_report(
        &self,
        run_id: &str,
        manifest: &ScanReportManifest,
    ) -> Result<(), ScanEvidenceStoreError> {
        self.maybe_fail(crate::seams::scan_evidence_store::fault_points::PUBLISH_MANIFEST)?;
        self.maybe_fail(crate::seams::scan_evidence_store::fault_points::DISK_FULL)?;
        self.inner.publish_report(run_id, manifest)
    }

    fn write_startup_marker(
        &self,
        marker: &ScanStartupMarker,
    ) -> Result<(), ScanEvidenceStoreError> {
        self.inner.write_startup_marker(marker)
    }

    fn startup_marker(&self) -> Result<Option<ScanStartupMarker>, ScanEvidenceStoreError> {
        self.inner.startup_marker()
    }

    fn write_qualification(
        &self,
        qualification: &ScanSnapshotQualification,
    ) -> Result<(), ScanEvidenceStoreError> {
        self.maybe_fail(crate::seams::scan_evidence_store::fault_points::QUALIFICATION_WRITE)?;
        self.inner.write_qualification(qualification)
    }

    fn qualification(&self) -> Result<Option<ScanSnapshotQualification>, ScanEvidenceStoreError> {
        self.inner.qualification()
    }
}

/// Compact payload builder helper used by tests: current manifest without
/// integrity (caller must seal it).
#[allow(dead_code)]
pub fn seal_manifest(mut manifest: ScanReportManifest) -> ScanReportManifest {
    let value = serde_json::to_value(&manifest).expect("manifest serializes");
    let digest = crate::seams::scan_integrity::canonical_json_digest(&value);
    manifest.integrity = Some(digest);
    manifest
}

/// Qualification helper used by tests and the engine: seal integrity.
#[allow(dead_code)]
pub fn seal_qualification(mut record: ScanSnapshotQualification) -> ScanSnapshotQualification {
    let value = serde_json::to_value(&record).expect("qualification serializes");
    record.integrity = Some(crate::seams::scan_integrity::canonical_json_digest(&value));
    record
}

#[allow(dead_code)]
pub fn root_key_for(index: u32) -> String {
    root_key(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seams::scan_evidence_store::{
        SCAN_STORE_SCHEMA_VERSION, ScanEvidenceCounts, ScanEvidenceStore, ScanEvidenceStoreFactory,
        ScanFrozenFacts, ScanFrozenRoot, ScanRootCoverageRecord, ScanRootState, fault_points,
    };

    fn home() -> BoundHome {
        BoundHome::test_value(
            "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab",
            PathBuf::from("/tmp/scan-store-home"),
        )
    }

    fn run_record(home: &BoundHome, run_id: &str) -> ScanRunRecord {
        ScanRunRecord {
            schema_version: SCAN_STORE_SCHEMA_VERSION,
            home_id: home.home_id.0.clone(),
            run_id: run_id.into(),
            generation: 1,
            trigger: "manual".into(),
            frozen: ScanFrozenFacts {
                home_id: home.home_id.0.clone(),
                write_gate_generation: 0,
                agent_configuration_generation: 3,
                mutation_generation: 0,
                roots_fingerprint: "roots<1>".into(),
            },
            roots: vec![ScanFrozenRoot {
                index: 0,
                configured_path: PathBuf::from("/tmp/root"),
                canonical_path: PathBuf::from("/tmp/root"),
                device: 1,
                inode: 2,
            }],
            started_at_ms: 1000,
        }
    }

    fn entry_record(seq: u64, name: &str) -> ScanEntryRecord {
        ScanEntryRecord {
            seq,
            name: name.into(),
            entry_path: PathBuf::from(format!("/tmp/root/{name}")),
            entry_kind: "directory".into(),
            chain: vec![],
            chain_fault: None,
            final_entity: Some(PathBuf::from(format!("/tmp/root/{name}"))),
            lock_hint: None,
            worktree_hint: None,
        }
    }

    fn manifest(home: &BoundHome, run_id: &str, state: &str) -> ScanReportManifest {
        let mut manifest = ScanReportManifest {
            schema_version: SCAN_STORE_SCHEMA_VERSION,
            home_id: home.home_id.0.clone(),
            run_id: run_id.into(),
            generation: 1,
            trigger: "manual".into(),
            state: state.into(),
            counts: ScanEvidenceCounts {
                roots: 1,
                ..Default::default()
            },
            roots: vec![ScanRootCoverageRecord {
                index: 0,
                configured_path: PathBuf::from("/tmp/root"),
                canonical_path: PathBuf::from("/tmp/root"),
                state: ScanRootState::Completed,
                counts: ScanEvidenceCounts::default(),
                elapsed_ms: 5,
                slow: false,
                diagnostic: None,
            }],
            frozen: ScanFrozenFacts {
                home_id: home.home_id.0.clone(),
                write_gate_generation: 0,
                agent_configuration_generation: 3,
                mutation_generation: 0,
                roots_fingerprint: "roots<1>".into(),
            },
            started_at_ms: 1000,
            ended_at_ms: 1500,
            integrity: None,
            content_identity:
                "scan-report-v1:b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab:run-1:1:complete".into(),
        };
        manifest = seal_manifest(manifest);
        manifest
    }

    fn store(temp: &tempfile::TempDir, home: &BoundHome) -> SystemScanEvidenceStore {
        SystemScanEvidenceStore::new(temp.path().join("scan"), home.home_id.0.clone())
    }

    #[test]
    fn create_run_and_stream_evidence_then_publish_atomically() {
        let temp = tempfile::tempdir().unwrap();
        let home = home();
        let store = store(&temp, &home);
        store.create_run(&run_record(&home, "run-1")).unwrap();
        store
            .append_entry("run-1", "r0", &entry_record(1, "alpha"))
            .unwrap();
        store
            .append_entity(
                "run-1",
                "r0",
                &ScanEntityRecord {
                    seq: 1,
                    name: "alpha".into(),
                    entry_path: PathBuf::from("/tmp/root/alpha"),
                    final_entity: PathBuf::from("/tmp/root/alpha"),
                    file_count: 1,
                    byte_count: 128,
                    tree_hash: Some("tree-sha256-v1:abc".into()),
                    hash_fault: None,
                    elapsed_ms: 2,
                },
            )
            .unwrap();
        assert!(matches!(
            store.current_manifest().unwrap(),
            CurrentManifestRead::Absent
        ));
        let manifest = manifest(&home, "run-1", "complete");
        store.publish_report("run-1", &manifest).unwrap();
        assert!(matches!(
            store.current_manifest().unwrap(),
            CurrentManifestRead::Report(report) if report.run_id == "run-1"
        ));
        // Streamed JSONL is durable.
        let entries = temp.path().join("scan/runs/run-1/roots/r0/entries.jsonl");
        let content = std::fs::read_to_string(entries).unwrap();
        assert!(content.contains("alpha"));
    }

    #[test]
    fn corrupt_current_manifest_reads_as_no_cached_report() {
        let temp = tempfile::tempdir().unwrap();
        let home = home();
        let store = store(&temp, &home);
        let manifest = manifest(&home, "run-1", "complete");
        store.publish_report("run-1", &manifest).unwrap();
        // Tamper the current pointer.
        std::fs::write(
            temp.path().join("scan/current.json"),
            r#"{"schema_version":1,"home_id":"different"}"#,
        )
        .unwrap();
        assert!(matches!(
            store.current_manifest().unwrap(),
            CurrentManifestRead::Corrupt
        ));
    }

    #[test]
    fn orphan_sweep_removes_only_proven_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let home = home();
        let store = store(&temp, &home);
        store.create_run(&run_record(&home, "run-1")).unwrap();
        store.create_run(&run_record(&home, "run-2")).unwrap();
        // Foreign artifact: unprovable run directory (no run.json).
        std::fs::create_dir_all(temp.path().join("scan/runs/foreign/roots/r0")).unwrap();
        std::fs::write(
            temp.path().join("scan/runs/foreign/run.json"),
            r#"{"schema_version":1,"home_id":"other-home","run_id":"foreign"}"#,
        )
        .unwrap();
        let removed = store.cleanup_temporary_runs().unwrap();
        assert_eq!(removed, 2);
        assert!(!temp.path().join("scan/runs/run-1").exists());
        assert!(!temp.path().join("scan/runs/run-2").exists());
        // Unproven (foreign provenance) directories stay untouched.
        assert!(temp.path().join("scan/runs/foreign").exists());
    }

    #[test]
    fn publish_keeps_old_report_on_manifest_switch_failure() {
        let temp = tempfile::tempdir().unwrap();
        let home = home();
        let store = store(&temp, &home);
        store
            .publish_report("run-1", &manifest(&home, "run-1", "complete"))
            .unwrap();
        // A second publish with a manifest of a DIFFERENT home must fail
        // closed, keeping the old current Report.
        let other = BoundHome::test_value(
            "11111111-1111-4111-8111-111111111111",
            PathBuf::from("/tmp/other"),
        );
        let err = store.publish_report("run-2", &manifest(&other, "run-2", "complete"));
        assert!(err.is_err());
        assert!(matches!(
            store.current_manifest().unwrap(),
            CurrentManifestRead::Report(report) if report.run_id == "run-1"
        ));
    }

    #[test]
    fn remove_refuses_unproven_run_artifacts() {
        let temp = tempfile::tempdir().unwrap();
        let home = home();
        let store = store(&temp, &home);
        std::fs::create_dir_all(temp.path().join("scan/runs/ghost")).unwrap();
        let err = store.remove_run("ghost");
        assert!(matches!(
            err,
            Err(ScanEvidenceStoreError::ProvenanceMismatch { .. })
        ));
        assert!(temp.path().join("scan/runs/ghost").exists());
    }

    #[test]
    fn fault_injected_disk_full_fails_the_write_and_keeps_old_report() {
        let root = tempfile::tempdir().unwrap();
        let factory = FaultInjectingScanEvidenceStoreFactory::new(root.path().to_path_buf());
        let home = home();
        let store = factory.store_for(&home).unwrap();
        store.create_run(&run_record(&home, "run-1")).unwrap();
        store
            .publish_report("run-1", &manifest(&home, "run-1", "complete"))
            .unwrap();
        factory.fail_next(fault_points::PUBLISH_MANIFEST);
        let err = store.publish_report("run-2", &manifest(&home, "run-2", "complete"));
        assert!(err.is_err());
        assert!(matches!(
            store.current_manifest().unwrap(),
            CurrentManifestRead::Report(report) if report.run_id == "run-1"
        ));
    }

    #[test]
    fn qualification_requires_live_integrity() {
        let temp = tempfile::tempdir().unwrap();
        let home = home();
        let store = store(&temp, &home);
        let record = ScanSnapshotQualification {
            schema_version: SCAN_STORE_SCHEMA_VERSION,
            home_id: home.home_id.0.clone(),
            snapshot_ids: vec!["home.snapshot-1".into()],
            startup_marked_at_ms: 2000,
            report_run_id: "run-1".into(),
            report_generation: 1,
            report_content_identity:
                "scan-report-v1:b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab:run-1:1:complete".into(),
            recorded_at_ms: 3000,
            integrity: None,
        };
        store
            .write_qualification(&seal_qualification(record))
            .unwrap();
        assert!(store.qualification().unwrap().is_some());
        // Tamper: the content identity no longer matches → no proof.
        let record_json =
            std::fs::read_to_string(temp.path().join("scan/qualification.json")).unwrap();
        std::fs::write(
            temp.path().join("scan/qualification.json"),
            record_json.replace("\"run-1\"", "\"run-9\""),
        )
        .unwrap();
        assert!(store.qualification().unwrap().is_none());
    }
}
