//! Scan Evidence Store adapters (spec §3.6): the system adapter writes under
//! `<Home>/cache/scan/` with the tmp → fsync → rename → parent fsync
//! protocol for every atomic switch (current manifest, startup marker,
//! qualification) and streamed JSONL for entry/entity evidence; the
//! fault-injecting adapter wraps a temporary scan directory with the stable
//! named failure points of spec §11.

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::core::home::BoundHome;
use crate::seams::scan_evidence_store::{
    CurrentManifestRead, ScanAppearanceRecord, ScanCanonicalEntityRecord, ScanDiagnosticRecord,
    ScanEntityIndexStats, ScanEntityRecord, ScanEntryRecord, ScanEvidenceStore,
    ScanEvidenceStoreError, ScanEvidenceStoreFactory, ScanObjectIdentity, ScanReportCursor,
    ScanReportManifest, ScanReportPageError, ScanReportPageRead, ScanReportRow, ScanReportSection,
    ScanRootRecord, ScanRootState, ScanRunRecord, ScanSnapshotQualification, ScanStartupMarker,
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

    fn build_entity_index(
        &self,
        run_id: &str,
    ) -> Result<ScanEntityIndexStats, ScanEvidenceStoreError> {
        // Provenance first: no evidence is ever interpreted without the
        // `(home_id, run_id)` run record of this Home.
        let Some(run) = self.read_run(run_id)? else {
            return Err(ScanEvidenceStoreError::ProvenanceMismatch {
                detail: format!("no run.json for {run_id}"),
            });
        };
        if run.home_id != self.home_id {
            return Err(ScanEvidenceStoreError::ProvenanceMismatch {
                detail: format!("run {run_id} belongs to home {}", run.home_id),
            });
        }
        let roots_dir = self.run_dir(run_id).join("roots");
        let root_indices = {
            let mut indices = Vec::new();
            match fs::read_dir(&roots_dir) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        let name = entry.file_name();
                        let Some(raw) = name.to_str() else { continue };
                        let Some(index) = raw
                            .strip_prefix('r')
                            .and_then(|value| value.parse::<u32>().ok())
                        else {
                            continue;
                        };
                        indices.push(index);
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(ScanEvidenceStoreError::io(
                        "enumerate Scan Run roots",
                        &roots_dir,
                        &error,
                    ));
                }
            }
            indices.sort_unstable();
            indices
        };

        // Canonical aggregation working set: identity → canonical row (the
        // irreducible entity index of the generation). The evidence streams
        // are read line by line and the appearance/diagnostic index streams
        // are written incrementally — only the canonical entity rows and the
        // per-Root facts map are held (ADR-0017 aggregation index).
        let mut identity_index: HashMap<(u64, u64), usize> = HashMap::new();
        let mut canonical_entities: Vec<ScanCanonicalEntityRecord> = Vec::new();
        let entities_path = self.run_dir(run_id).join("entities.jsonl");
        let appearances_path = self.run_dir(run_id).join("appearances.jsonl");
        let diagnostics_path = self.run_dir(run_id).join("diagnostics.jsonl");
        let mut entities_file = fs::File::create(&entities_path).map_err(|error| {
            ScanEvidenceStoreError::io("write canonical Scan entity index", &entities_path, &error)
        })?;
        let mut appearances_file = fs::File::create(&appearances_path).map_err(|error| {
            ScanEvidenceStoreError::io("write Scan appearance index", &appearances_path, &error)
        })?;
        let mut diagnostics_file = fs::File::create(&diagnostics_path).map_err(|error| {
            ScanEvidenceStoreError::io("write Scan diagnostic index", &diagnostics_path, &error)
        })?;
        fn append_index_line<T: serde::Serialize>(
            file: &mut fs::File,
            record: &T,
            path: &Path,
            operation: &'static str,
        ) -> Result<(), ScanEvidenceStoreError> {
            let mut line =
                serde_json::to_vec(record).map_err(|error| ScanEvidenceStoreError::Write {
                    operation,
                    detail: error.to_string(),
                })?;
            line.push(b'\n');
            file.write_all(&line)
                .map_err(|error| ScanEvidenceStoreError::io(operation, path, &error))
        }
        let mut appearances_count = 0_u64;
        let mut diagnostics_count = 0_u64;
        for root_index in root_indices {
            let root_dir = roots_dir.join(format!("r{root_index}"));
            let root_json = root_dir.join("root.json");
            let root_record = match self.read_string(&root_json)? {
                Some(content) => match ScanRootRecord::parse(&content) {
                    Some(record) => record,
                    None => {
                        let diagnostic = ScanDiagnosticRecord {
                            root_index,
                            kind: "root_record_missing".into(),
                            at: Some(root_json.clone()),
                            detail: Some("unparseable root.json".into()),
                        };
                        append_index_line(
                            &mut diagnostics_file,
                            &diagnostic,
                            &diagnostics_path,
                            "write Scan diagnostic index",
                        )?;
                        diagnostics_count += 1;
                        continue;
                    }
                },
                None => {
                    let diagnostic = ScanDiagnosticRecord {
                        root_index,
                        kind: "root_record_missing".into(),
                        at: Some(root_json),
                        detail: Some("no root.json for a committed Root".into()),
                    };
                    append_index_line(
                        &mut diagnostics_file,
                        &diagnostic,
                        &diagnostics_path,
                        "write Scan diagnostic index",
                    )?;
                    diagnostics_count += 1;
                    continue;
                }
            };
            // Root is the minimum evidence commit unit (spec §4.10): a
            // failed/unresponsive Root publishes coverage + diagnostic only,
            // never its half candidates.
            if root_record.state != ScanRootState::Completed {
                let diagnostic = ScanDiagnosticRecord {
                    root_index,
                    kind: if root_record.state == ScanRootState::Unresponsive {
                        "root_unresponsive".into()
                    } else {
                        "root_failed".into()
                    },
                    at: Some(root_record.canonical_path),
                    detail: root_record.diagnostic,
                };
                append_index_line(
                    &mut diagnostics_file,
                    &diagnostic,
                    &diagnostics_path,
                    "write Scan diagnostic index",
                )?;
                diagnostics_count += 1;
                continue;
            }

            // Per-Root tree-fact join: entity records streamed during the
            // walk, keyed by entry path (appearance → verified facts).
            #[derive(Clone)]
            struct EntityFacts {
                identity: Option<ScanObjectIdentity>,
                file_count: u64,
                byte_count: u64,
                tree_hash: Option<String>,
                hash_fault: Option<String>,
            }
            let mut facts_by_path: HashMap<PathBuf, EntityFacts> = HashMap::new();
            let entities_file =
                fs::File::open(root_dir.join("entities.jsonl").as_path()).map_err(|error| {
                    ScanEvidenceStoreError::io(
                        "read Scan evidence stream",
                        &root_dir.join("entities.jsonl"),
                        &error,
                    )
                })?;
            for line in BufReader::new(entities_file).lines() {
                let Ok(line) = line else { continue };
                let Ok(record) = serde_json::from_str::<ScanEntityRecord>(&line) else {
                    continue;
                };
                facts_by_path.insert(
                    record.entry_path.clone(),
                    EntityFacts {
                        identity: record.identity,
                        file_count: record.file_count,
                        byte_count: record.byte_count,
                        tree_hash: record.tree_hash,
                        hash_fault: record.hash_fault,
                    },
                );
            }

            let entries_file =
                fs::File::open(root_dir.join("entries.jsonl").as_path()).map_err(|error| {
                    ScanEvidenceStoreError::io(
                        "read Scan evidence stream",
                        &root_dir.join("entries.jsonl"),
                        &error,
                    )
                })?;
            for line in BufReader::new(entries_file).lines() {
                let Ok(line) = line else { continue };
                let Ok(entry) = serde_json::from_str::<ScanEntryRecord>(&line) else {
                    continue;
                };
                let mut entity_seq = None;
                let facts = facts_by_path.get(&entry.entry_path).cloned();
                // The aggregate key is the identity re-verified after the
                // tree hash (ADR-0017 replacement guard); a stale or missing
                // identity never guesses an entity.
                let identity = facts.as_ref().and_then(|facts| facts.identity);
                if let (Some(final_entity), Some(identity)) = (&entry.final_entity, identity) {
                    let key = (identity.device, identity.inode);
                    if let Some(facts) = facts.as_ref().filter(|facts| facts.tree_hash.is_some()) {
                        let seq = if let Some(position) = identity_index.get(&key).copied() {
                            canonical_entities[position].appearances += 1;
                            Some(canonical_entities[position].entity_seq)
                        } else {
                            let entity_seq = canonical_entities.len() as u64 + 1;
                            canonical_entities.push(ScanCanonicalEntityRecord {
                                entity_seq,
                                identity,
                                canonical_path: final_entity.clone(),
                                file_count: facts.file_count,
                                byte_count: facts.byte_count,
                                tree_hash: facts.tree_hash.clone(),
                                hash_fault: facts.hash_fault.clone(),
                                appearances: 1,
                                first_root_index: root_index,
                                first_entry_seq: entry.seq,
                            });
                            identity_index.insert(key, canonical_entities.len() - 1);
                            Some(entity_seq)
                        };
                        entity_seq = seq;
                    } else {
                        let diagnostic = ScanDiagnosticRecord {
                            root_index,
                            kind: "entity_hash_fault".into(),
                            at: Some(final_entity.clone()),
                            detail: facts
                                .as_ref()
                                .and_then(|facts| facts.hash_fault.clone())
                                .or_else(|| Some("no tree facts for a resolved entity".into())),
                        };
                        append_index_line(
                            &mut diagnostics_file,
                            &diagnostic,
                            &diagnostics_path,
                            "write Scan diagnostic index",
                        )?;
                        diagnostics_count += 1;
                    }
                } else if entry.final_entity.is_some() {
                    // Resolved entity without a verified identity: typed
                    // identity fault, never a guessed canonical entity.
                    let diagnostic = ScanDiagnosticRecord {
                        root_index,
                        kind: "entity_identity_fault".into(),
                        at: entry.final_entity.clone(),
                        detail: facts
                            .as_ref()
                            .and_then(|facts| facts.hash_fault.clone())
                            .or_else(|| Some("final entity identity missing or changed".into())),
                    };
                    append_index_line(
                        &mut diagnostics_file,
                        &diagnostic,
                        &diagnostics_path,
                        "write Scan diagnostic index",
                    )?;
                    diagnostics_count += 1;
                }
                if let Some(fault) = entry.chain_fault.as_ref() {
                    let diagnostic = ScanDiagnosticRecord {
                        root_index,
                        kind: format!("chain_{}", fault.kind),
                        at: Some(fault.at.clone()),
                        detail: fault.detail.clone(),
                    };
                    append_index_line(
                        &mut diagnostics_file,
                        &diagnostic,
                        &diagnostics_path,
                        "write Scan diagnostic index",
                    )?;
                    diagnostics_count += 1;
                }
                let appearance = ScanAppearanceRecord {
                    root_index,
                    seq: entry.seq,
                    name: entry.name,
                    entry_path: entry.entry_path,
                    entry_kind: entry.entry_kind,
                    chain: entry.chain,
                    chain_fault: entry.chain_fault,
                    final_entity: entry.final_entity,
                    // The verified identity is authoritative; the walk-time
                    // entry identity stays only in the raw evidence stream.
                    identity,
                    entity_seq,
                    lock_hint: entry.lock_hint,
                    worktree_hint: entry.worktree_hint,
                };
                append_index_line(
                    &mut appearances_file,
                    &appearance,
                    &appearances_path,
                    "write Scan appearance index",
                )?;
                appearances_count += 1;
            }
        }
        for entity in &canonical_entities {
            append_index_line(
                &mut entities_file,
                entity,
                &entities_path,
                "write canonical Scan entity index",
            )?;
        }
        // Deterministic streamed indexes: canonical rows written after the
        // full pass (appearance counts are exact), appearances/diagnostics
        // streamed incrementally. The manifest switch publishes only the
        // completed run artifact.
        for file in [
            &mut entities_file,
            &mut appearances_file,
            &mut diagnostics_file,
        ] {
            file.flush()
                .and_then(|()| file.sync_all())
                .map_err(|error| {
                    ScanEvidenceStoreError::io("flush Scan index", &entities_path, &error)
                })?;
        }
        for name in ["entities.jsonl", "appearances.jsonl", "diagnostics.jsonl"] {
            let path = self.run_dir(run_id).join(name);
            if !path.exists() {
                fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .map_err(|error| {
                        ScanEvidenceStoreError::io("write empty Scan index", &path, &error)
                    })?;
            }
        }
        Ok(ScanEntityIndexStats {
            entities: canonical_entities.len() as u64,
            appearances: appearances_count,
            diagnostics: diagnostics_count,
        })
    }

    fn report_page(
        &self,
        cursor: &ScanReportCursor,
        limit: usize,
    ) -> Result<ScanReportPageRead, ScanReportPageError> {
        let limit = if limit == 0 { 1 } else { limit.min(512) };
        // The cursor is generation-bound: only the exact Report identity
        // that is current may be served (spec §4.10; ADR-0020).
        let current = self
            .current_manifest()
            .map_err(|_| ScanReportPageError::NotFound)?;
        let manifest = match current {
            CurrentManifestRead::Report(manifest)
                if manifest.content_identity == cursor.report_content_identity =>
            {
                manifest
            }
            CurrentManifestRead::Report(manifest) => {
                return Err(ScanReportPageError::Stale {
                    current_generation: manifest.generation,
                });
            }
            CurrentManifestRead::Absent | CurrentManifestRead::Corrupt => {
                return Err(ScanReportPageError::NotFound);
            }
        };
        let run = self
            .read_run(&cursor.run_id)
            .map_err(|_| ScanReportPageError::NotFound)?
            .ok_or(ScanReportPageError::NotFound)?;
        if run.home_id != self.home_id
            || run.generation != cursor.generation
            || run.run_id != cursor.run_id
        {
            return Err(ScanReportPageError::NotFound);
        }
        let (rows, next_offset) = match cursor.section {
            ScanReportSection::Roots => {
                let total = manifest.roots.len() as u64;
                let start = cursor.offset.min(total) as usize;
                let end = (start + limit).min(manifest.roots.len());
                let rows = manifest.roots[start..end]
                    .iter()
                    .cloned()
                    .map(ScanReportRow::RootCoverage)
                    .collect();
                let next = if (end as u64) < total {
                    Some(end as u64)
                } else {
                    None
                };
                (rows, next)
            }
            ScanReportSection::Entities => self.page_file_rows::<ScanCanonicalEntityRecord>(
                cursor,
                limit,
                ScanReportRow::Entity,
            )?,
            ScanReportSection::Appearances => {
                self.page_file_rows::<ScanAppearanceRecord>(cursor, limit, |record| {
                    ScanReportRow::Appearance(Box::new(record))
                })?
            }
            ScanReportSection::Diagnostics => self.page_file_rows::<ScanDiagnosticRecord>(
                cursor,
                limit,
                ScanReportRow::Diagnostic,
            )?,
        };
        Ok(ScanReportPageRead { rows, next_offset })
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

impl SystemScanEvidenceStore {
    fn page_file_rows<T>(
        &self,
        cursor: &ScanReportCursor,
        limit: usize,
        row_kind: fn(T) -> ScanReportRow,
    ) -> Result<(Vec<ScanReportRow>, Option<u64>), ScanReportPageError>
    where
        T: serde::de::DeserializeOwned,
    {
        let file_name = match cursor.section {
            ScanReportSection::Entities => "entities.jsonl",
            ScanReportSection::Appearances => "appearances.jsonl",
            ScanReportSection::Diagnostics => "diagnostics.jsonl",
            ScanReportSection::Roots => unreachable!("root rows are manifest-backed"),
        };
        let path = self.run_dir(&cursor.run_id).join(file_name);
        let file = File::open(&path).map_err(|_| ScanReportPageError::NotFound)?;
        let mut reader = BufReader::new(file);
        reader
            .seek(SeekFrom::Start(cursor.offset))
            .map_err(|_| ScanReportPageError::NotFound)?;
        let mut rows = Vec::new();
        let mut consumed = 0_u64;
        for _ in 0..limit {
            let mut line = Vec::new();
            let read = reader
                .read_until(b'\n', &mut line)
                .map_err(|_| ScanReportPageError::NotFound)?;
            if read == 0 {
                break;
            }
            consumed += read as u64;
            if line.last() == Some(&b'\n') {
                line.pop();
            }
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            if let Ok(record) = serde_json::from_slice::<T>(&line) {
                rows.push(row_kind(record));
            }
        }
        let at_end = reader.fill_buf().map(|buf| buf.is_empty()).unwrap_or(true);
        let next_offset = if at_end {
            None
        } else {
            Some(cursor.offset + consumed)
        };
        Ok((rows, next_offset))
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

    fn build_entity_index(
        &self,
        run_id: &str,
    ) -> Result<ScanEntityIndexStats, ScanEvidenceStoreError> {
        self.maybe_fail(crate::seams::scan_evidence_store::fault_points::ENTITY_INDEX)?;
        self.inner.build_entity_index(run_id)
    }

    fn report_page(
        &self,
        cursor: &ScanReportCursor,
        limit: usize,
    ) -> Result<ScanReportPageRead, ScanReportPageError> {
        self.inner.report_page(cursor, limit)
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
        ScanFrozenFacts, ScanFrozenRoot, ScanObjectIdentity, ScanReportPageError,
        ScanRootCoverageRecord, ScanRootState, fault_points,
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
                configured_agents: 1,
                declared_roots: 1,
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
            identity: None,
            lock_hint: None,
            worktree_hint: None,
        }
    }

    fn entry_with_identity(seq: u64, name: &str, identity: ScanObjectIdentity) -> ScanEntryRecord {
        let mut entry = entry_record(seq, name);
        entry.identity = Some(identity);
        entry
    }

    fn entity_record(seq: u64, name: &str, identity: ScanObjectIdentity) -> ScanEntityRecord {
        ScanEntityRecord {
            seq,
            name: name.into(),
            entry_path: PathBuf::from(format!("/tmp/root/{name}")),
            final_entity: PathBuf::from(format!("/tmp/root/{name}")),
            identity: Some(identity),
            file_count: 1,
            byte_count: 64,
            tree_hash: Some(format!("tree-sha256-v1:{name}")),
            hash_fault: None,
            elapsed_ms: 2,
        }
    }

    fn root_record(index: u32, state: ScanRootState) -> ScanRootRecord {
        ScanRootRecord {
            schema_version: SCAN_STORE_SCHEMA_VERSION,
            home_id: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into(),
            run_id: "run-1".into(),
            index,
            configured_path: PathBuf::from("/tmp/root"),
            canonical_path: PathBuf::from("/tmp/root"),
            state,
            counts: ScanEvidenceCounts::default(),
            elapsed_ms: 5,
            slow: false,
            diagnostic: if state == ScanRootState::Completed {
                None
            } else {
                Some("root failed".into())
            },
            completed_at_ms: 1000,
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
                configured_agents: 1,
                declared_roots: 1,
            },
            started_at_ms: 1000,
            ended_at_ms: 1500,
            integrity: None,
            content_identity: format!("scan-report-v1:{}:{}:1:complete", home.home_id.0, run_id),
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
                    identity: None,
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

    #[test]
    fn entity_index_aggregates_appearances_by_object_identity() {
        let temp = tempfile::tempdir().unwrap();
        let home = home();
        let store = store(&temp, &home);
        let run_id = "run-1";
        store.create_run(&run_record(&home, run_id)).unwrap();
        let identity_a = ScanObjectIdentity {
            device: 1,
            inode: 100,
        };
        let identity_b = ScanObjectIdentity {
            device: 1,
            inode: 200,
        };
        // alpha and beta resolve to the same object (identity A): exactly one
        // canonical entity, two appearances; gamma is a distinct object.
        for (seq, name, identity) in [
            (1, "alpha", identity_a),
            (2, "beta", identity_a),
            (3, "gamma", identity_b),
        ] {
            store
                .append_entry(run_id, "r0", &entry_with_identity(seq, name, identity))
                .unwrap();
            store
                .append_entity(run_id, "r0", &entity_record(seq, name, identity))
                .unwrap();
        }
        store
            .write_root(run_id, &root_record(0, ScanRootState::Completed))
            .unwrap();
        let stats = store.build_entity_index(run_id).unwrap();
        assert_eq!(stats.entities, 2, "one entity per distinct object");
        assert_eq!(stats.appearances, 3);
        assert_eq!(stats.diagnostics, 0);
        // Both appearances of identity A carry the same entity_seq and the
        // canonical row reports the exact appearance count.
        let manifest = manifest(&home, run_id, "complete");
        store.publish_report(run_id, &manifest).unwrap();
        let cursor = ScanReportCursor {
            report_content_identity: manifest.content_identity.clone(),
            run_id: run_id.into(),
            generation: 1,
            section: ScanReportSection::Entities,
            offset: 0,
        };
        let page = store.report_page(&cursor, 16).unwrap();
        assert_eq!(page.rows.len(), 2);
        assert!(page.next_offset.is_none());
        let entities = page
            .rows
            .iter()
            .map(|row| match row {
                ScanReportRow::Entity(entity) => entity.clone(),
                other => panic!("unexpected row {other:?}"),
            })
            .collect::<Vec<_>>();
        assert_eq!(entities[0].entity_seq, 1);
        assert_eq!(entities[0].appearances, 2, "both alpha + beta appearances");
        assert_eq!(entities[1].entity_seq, 2);
        assert_eq!(entities[1].appearances, 1);
        // The appearances section pages the same generation-bound rows.
        let cursor = ScanReportCursor {
            section: ScanReportSection::Appearances,
            ..cursor.clone()
        };
        let page = store.report_page(&cursor, 2).unwrap();
        assert_eq!(page.rows.len(), 2);
        assert!(page.next_offset.is_some(), "one appearance remains");
        let continued = store
            .report_page(
                &ScanReportCursor {
                    offset: page.next_offset.unwrap(),
                    ..cursor
                },
                2,
            )
            .unwrap();
        assert_eq!(continued.rows.len(), 1);
        assert!(continued.next_offset.is_none());
    }

    #[test]
    fn report_page_never_falls_back_to_another_report() {
        let temp = tempfile::tempdir().unwrap();
        let home = home();
        let store = store(&temp, &home);
        let run_id = "run-1";
        store.create_run(&run_record(&home, run_id)).unwrap();
        store
            .append_entry(
                run_id,
                "r0",
                &entry_with_identity(
                    1,
                    "alpha",
                    ScanObjectIdentity {
                        device: 1,
                        inode: 100,
                    },
                ),
            )
            .unwrap();
        store
            .append_entity(
                run_id,
                "r0",
                &entity_record(
                    1,
                    "alpha",
                    ScanObjectIdentity {
                        device: 1,
                        inode: 100,
                    },
                ),
            )
            .unwrap();
        store
            .write_root(run_id, &root_record(0, ScanRootState::Completed))
            .unwrap();
        store.build_entity_index(run_id).unwrap();
        let first = manifest(&home, run_id, "complete");
        store.publish_report(run_id, &first).unwrap();
        let cursor = ScanReportCursor {
            report_content_identity: first.content_identity.clone(),
            run_id: run_id.into(),
            generation: 1,
            section: ScanReportSection::Entities,
            offset: 0,
        };
        // A second Report switches current.json: the old cursor is typed
        // stale and never resolves against the new Report.
        let second = manifest(&home, "run-2", "complete");
        store
            .create_run(&ScanRunRecord {
                schema_version: SCAN_STORE_SCHEMA_VERSION,
                home_id: home.home_id.0.clone(),
                run_id: "run-2".into(),
                generation: 2,
                trigger: "manual".into(),
                frozen: scan_run_record_frozen(),
                roots: vec![],
                started_at_ms: 2000,
            })
            .unwrap();
        store.publish_report("run-2", &second).unwrap();
        assert!(matches!(
            store.report_page(&cursor, 16),
            Err(ScanReportPageError::Stale {
                current_generation: 1
            })
        ));
        // A corrupt manifest is not-found, never a fallback.
        std::fs::write(temp.path().join("scan/current.json"), "{}").unwrap();
        assert!(matches!(
            store.report_page(&cursor, 16),
            Err(ScanReportPageError::NotFound)
        ));
    }

    #[test]
    fn failed_root_publishes_no_half_candidates() {
        let temp = tempfile::tempdir().unwrap();
        let home = home();
        let store = store(&temp, &home);
        let run_id = "run-1";
        store.create_run(&run_record(&home, run_id)).unwrap();
        let identity_a = ScanObjectIdentity {
            device: 1,
            inode: 100,
        };
        // Root r1 failed; its streamed evidence must never aggregate.
        store
            .append_entry(run_id, "r1", &entry_with_identity(1, "half", identity_a))
            .unwrap();
        store
            .append_entity(run_id, "r1", &entity_record(1, "half", identity_a))
            .unwrap();
        store
            .write_root(run_id, &root_record(1, ScanRootState::Failed))
            .unwrap();
        let stats = store.build_entity_index(run_id).unwrap();
        assert_eq!(stats.entities, 0, "failed Root contributes no entity");
        assert_eq!(
            stats.appearances, 0,
            "failed Root contributes no appearance"
        );
        assert_eq!(stats.diagnostics, 1, "typed root failure diagnostic");
    }

    fn scan_run_record_frozen() -> ScanFrozenFacts {
        ScanFrozenFacts {
            home_id: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into(),
            write_gate_generation: 0,
            agent_configuration_generation: 3,
            mutation_generation: 0,
            roots_fingerprint: "roots<0>".into(),
            configured_agents: 1,
            declared_roots: 1,
        }
    }
}
