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

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::core::home::BoundHome;
use crate::seams::scan_evidence_store::{
    CurrentManifestRead, ScanAppearanceRecord, ScanCanonicalEntityRecord, ScanClassificationRow,
    ScanClassificationSpool, ScanClassificationSpoolEntity, ScanClassificationWriter,
    ScanConflictSetRecord, ScanDiagnosticRecord, ScanEntityIndexStats, ScanEntityRecord,
    ScanEntryRecord, ScanEvidenceStore, ScanEvidenceStoreError, ScanEvidenceStoreFactory,
    ScanGitSourceGroupRecord, ScanLockClaimRecord, ScanReportCursor, ScanReportManifest,
    ScanReportPageError, ScanReportPageRead, ScanReportRow, ScanReportSection, ScanRootRecord,
    ScanRootState, ScanRunRecord, ScanSnapshotQualification, ScanSourceVerdictRecord,
    ScanStartupMarker,
};

pub const SCAN_DIR_NAME: &str = "scan";
const RUNS_DIR_NAME: &str = "runs";
const ENTITY_INDEX_FILE_NAME: &str = "index.sqlite3";
const CLASSIFICATION_SPOOL_FILE_NAME: &str = "classification.sqlite3";

const ENTITY_INDEX_SCHEMA: &str = r#"
PRAGMA journal_mode = DELETE;
PRAGMA synchronous = FULL;
CREATE TABLE root_entity_facts (
    root_index INTEGER NOT NULL,
    entry_path TEXT NOT NULL,
    record BLOB NOT NULL,
    matched INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (root_index, entry_path)
);
CREATE TABLE canonical_entities (
    entity_seq INTEGER PRIMARY KEY,
    device INTEGER NOT NULL,
    inode INTEGER NOT NULL,
    record BLOB NOT NULL,
    appearance_count INTEGER NOT NULL
);
CREATE UNIQUE INDEX canonical_entities_identity
    ON canonical_entities(device, inode);
CREATE TABLE appearances (
    ordinal INTEGER PRIMARY KEY AUTOINCREMENT,
    entity_seq INTEGER,
    record BLOB NOT NULL
);
"#;

const CLASSIFICATION_SPOOL_SCHEMA: &str = r#"
PRAGMA journal_mode = DELETE;
PRAGMA synchronous = FULL;
CREATE TABLE entities (
    entity_seq INTEGER PRIMARY KEY,
    payload BLOB NOT NULL,
    shape TEXT NOT NULL,
    provider TEXT,
    repository TEXT,
    identity_key TEXT NOT NULL,
    group_seq INTEGER,
    group_status TEXT,
    group_detail TEXT,
    conflict_seq INTEGER
);
CREATE INDEX entities_git_group
    ON entities(shape, provider, repository, group_seq);
CREATE INDEX entities_local_conflict
    ON entities(shape, identity_key, conflict_seq);
"#;

fn corrupt_artifact(operation: &'static str, detail: impl Into<String>) -> ScanEvidenceStoreError {
    ScanEvidenceStoreError::Corrupt {
        operation,
        detail: detail.into(),
    }
}

/// Read an immutable JSONL artifact without accepting a torn or malformed
/// line. Every writer emits a newline-terminated record, so an unterminated
/// final line is corruption rather than a partial record to skip.
fn read_jsonl_records<T>(
    path: &Path,
    operation: &'static str,
    mut visitor: impl FnMut(T) -> Result<(), ScanEvidenceStoreError>,
) -> Result<u64, ScanEvidenceStoreError>
where
    T: serde::de::DeserializeOwned,
{
    let file =
        File::open(path).map_err(|error| ScanEvidenceStoreError::io(operation, path, &error))?;
    let mut reader = BufReader::new(file);
    let mut count = 0_u64;
    loop {
        let mut line = Vec::new();
        let read = reader
            .read_until(b'\n', &mut line)
            .map_err(|error| ScanEvidenceStoreError::io(operation, path, &error))?;
        if read == 0 {
            break;
        }
        if line.last() != Some(&b'\n') {
            return Err(corrupt_artifact(
                operation,
                format!("unterminated JSONL record in {}", path.display()),
            ));
        }
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        if line.is_empty() {
            return Err(corrupt_artifact(
                operation,
                format!("empty JSONL record in {}", path.display()),
            ));
        }
        let record = serde_json::from_slice::<T>(&line).map_err(|error| {
            corrupt_artifact(
                operation,
                format!("malformed JSONL record in {}: {error}", path.display()),
            )
        })?;
        visitor(record)?;
        count += 1;
    }
    Ok(count)
}

fn validate_jsonl_records<T>(
    path: &Path,
    operation: &'static str,
) -> Result<u64, ScanEvidenceStoreError>
where
    T: serde::de::DeserializeOwned,
{
    read_jsonl_records(path, operation, |_: T| Ok(()))
}

fn append_json_line<T: serde::Serialize>(
    file: &mut File,
    path: &Path,
    record: &T,
    operation: &'static str,
) -> Result<(), ScanEvidenceStoreError> {
    let mut line = serde_json::to_vec(record).map_err(|error| ScanEvidenceStoreError::Write {
        operation,
        detail: error.to_string(),
    })?;
    line.push(b'\n');
    file.write_all(&line)
        .map_err(|error| ScanEvidenceStoreError::io(operation, path, &error))
}

struct JsonlClassificationWriter {
    verdicts: File,
    groups: File,
    conflict_sets: File,
    verdicts_path: PathBuf,
    groups_path: PathBuf,
    conflict_sets_path: PathBuf,
}

impl JsonlClassificationWriter {
    fn flush_and_sync(&mut self) -> Result<(), ScanEvidenceStoreError> {
        for (file, path) in [
            (&mut self.verdicts, &self.verdicts_path),
            (&mut self.groups, &self.groups_path),
            (&mut self.conflict_sets, &self.conflict_sets_path),
        ] {
            file.flush()
                .and_then(|()| file.sync_all())
                .map_err(|error| {
                    ScanEvidenceStoreError::io("flush Scan classification index", path, &error)
                })?;
        }
        Ok(())
    }
}

impl ScanClassificationWriter for JsonlClassificationWriter {
    fn append_verdict(
        &mut self,
        record: &ScanSourceVerdictRecord,
    ) -> Result<(), ScanEvidenceStoreError> {
        append_json_line(
            &mut self.verdicts,
            &self.verdicts_path,
            record,
            "write Scan verdict index",
        )
    }

    fn append_git_group(
        &mut self,
        record: &ScanGitSourceGroupRecord,
    ) -> Result<(), ScanEvidenceStoreError> {
        append_json_line(
            &mut self.groups,
            &self.groups_path,
            record,
            "write Scan Git source index",
        )
    }

    fn append_conflict_set(
        &mut self,
        record: &ScanConflictSetRecord,
    ) -> Result<(), ScanEvidenceStoreError> {
        append_json_line(
            &mut self.conflict_sets,
            &self.conflict_sets_path,
            record,
            "write Scan Conflict Set index",
        )
    }
}

struct SqliteClassificationSpool {
    connection: Connection,
}

impl ScanClassificationSpool for SqliteClassificationSpool {
    fn insert_entity(
        &mut self,
        entity_seq: u64,
        payload: &[u8],
        shape: &str,
        provider: Option<&str>,
        repository: Option<&str>,
        identity_key: &str,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "INSERT INTO entities \
                 (entity_seq, payload, shape, provider, repository, identity_key) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    entity_seq as i64,
                    payload,
                    shape,
                    provider,
                    repository,
                    identity_key
                ],
            )
            .map_err(|error| format!("write classification spool row: {error}"))?;
        Ok(())
    }

    fn next_git_group(&mut self) -> Result<Option<(String, String)>, String> {
        self.connection
            .query_row(
                "SELECT provider, repository FROM entities \
                 WHERE shape = 'git' AND group_seq IS NULL \
                 GROUP BY provider, repository \
                 ORDER BY provider, repository LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(|error| format!("read classification Git group: {error}"))
    }

    fn for_each_git_member(
        &mut self,
        provider: &str,
        repository: &str,
        visitor: &mut dyn FnMut(Vec<u8>) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT payload FROM entities \
                 WHERE shape = 'git' AND provider = ?1 AND repository = ?2 \
                 ORDER BY entity_seq",
            )
            .map_err(|error| format!("prepare classification Git group: {error}"))?;
        let mut rows = statement
            .query(params![provider, repository])
            .map_err(|error| format!("read classification Git group: {error}"))?;
        while let Some(row) = rows
            .next()
            .map_err(|error| format!("read classification Git group: {error}"))?
        {
            let payload: Vec<u8> = row
                .get(0)
                .map_err(|error| format!("read classification Git member: {error}"))?;
            visitor(payload)?;
        }
        Ok(())
    }

    fn mark_git_group(
        &mut self,
        provider: &str,
        repository: &str,
        group_seq: u64,
        status: &str,
        detail: Option<&str>,
    ) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE entities SET group_seq = ?1, group_status = ?2, group_detail = ?3 \
                 WHERE shape = 'git' AND provider = ?4 AND repository = ?5",
                params![group_seq as i64, status, detail, provider, repository],
            )
            .map_err(|error| format!("mark classification Git group: {error}"))?;
        Ok(())
    }

    fn next_local_conflict_key(&mut self) -> Result<Option<String>, String> {
        self.connection
            .query_row(
                "SELECT identity_key FROM entities \
                 WHERE shape = 'local' AND conflict_seq IS NULL \
                 GROUP BY identity_key HAVING COUNT(*) > 1 \
                 ORDER BY identity_key LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|error| format!("read classification Conflict Set: {error}"))
    }

    fn for_each_local_member(
        &mut self,
        identity_key: &str,
        visitor: &mut dyn FnMut(Vec<u8>) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT payload FROM entities \
                 WHERE shape = 'local' AND identity_key = ?1 \
                 ORDER BY entity_seq",
            )
            .map_err(|error| format!("prepare classification Conflict Set: {error}"))?;
        let mut rows = statement
            .query(params![identity_key])
            .map_err(|error| format!("read classification Conflict Set: {error}"))?;
        while let Some(row) = rows
            .next()
            .map_err(|error| format!("read classification Conflict Set: {error}"))?
        {
            let payload: Vec<u8> = row
                .get(0)
                .map_err(|error| format!("read classification Conflict member: {error}"))?;
            visitor(payload)?;
        }
        Ok(())
    }

    fn mark_local_conflict(&mut self, identity_key: &str, conflict_seq: u64) -> Result<(), String> {
        self.connection
            .execute(
                "UPDATE entities SET conflict_seq = ?1 \
                 WHERE shape = 'local' AND identity_key = ?2",
                params![conflict_seq as i64, identity_key],
            )
            .map_err(|error| format!("mark classification Conflict Set: {error}"))?;
        Ok(())
    }

    fn for_each_entity(
        &mut self,
        visitor: &mut dyn FnMut(ScanClassificationSpoolEntity) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut statement = self
            .connection
            .prepare(
                "SELECT payload, group_seq, group_status, group_detail, conflict_seq \
                 FROM entities ORDER BY entity_seq",
            )
            .map_err(|error| format!("prepare classification verdicts: {error}"))?;
        let mut rows = statement
            .query([])
            .map_err(|error| format!("read classification verdicts: {error}"))?;
        while let Some(row) = rows
            .next()
            .map_err(|error| format!("read classification verdicts: {error}"))?
        {
            let payload: Vec<u8> = row
                .get(0)
                .map_err(|error| format!("read classification verdict: {error}"))?;
            let group_seq = row
                .get::<_, Option<i64>>(1)
                .map_err(|error| format!("read classification Git group sequence: {error}"))?
                .map(|value| {
                    u64::try_from(value)
                        .map_err(|_| "classification Git group sequence is negative".to_owned())
                })
                .transpose()?;
            let conflict_seq = row
                .get::<_, Option<i64>>(4)
                .map_err(|error| format!("read classification Conflict Set sequence: {error}"))?
                .map(|value| {
                    u64::try_from(value)
                        .map_err(|_| "classification Conflict Set sequence is negative".to_owned())
                })
                .transpose()?;
            visitor(ScanClassificationSpoolEntity {
                payload,
                group_seq,
                group_status: row
                    .get(2)
                    .map_err(|error| format!("read classification Git group status: {error}"))?,
                group_detail: row
                    .get(3)
                    .map_err(|error| format!("read classification Git group detail: {error}"))?,
                conflict_seq,
            })?;
        }
        Ok(())
    }
}

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
            .and_then(|()| file.sync_all())
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

    /// Prove the Run belongs to this Home before any indexed read.
    fn verify_run_home(&self, run_id: &str) -> Result<(), ScanEvidenceStoreError> {
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
        Ok(())
    }

    /// First record of an immutable index stream matching the predicate, or
    /// `None`. A torn or unparseable line is corruption; silently skipping it
    /// could turn an incomplete Report into a valid-looking one.
    fn read_index_record<T>(
        &self,
        run_id: &str,
        file_name: &str,
        matches: impl Fn(&T) -> bool,
    ) -> Result<Option<T>, ScanEvidenceStoreError>
    where
        T: serde::de::DeserializeOwned,
    {
        let path = self.run_dir(run_id).join(file_name);
        let mut found = None;
        read_jsonl_records(&path, "read Scan index", |record: T| {
            if found.is_none() && matches(&record) {
                found = Some(record);
            }
            Ok(())
        })?;
        Ok(found)
    }

    /// Validate every artifact needed by the generation-bound page contract.
    /// A syntactically valid current pointer is not enough: missing or
    /// malformed referenced artifacts are treated as `No cached report`.
    fn report_artifacts_are_valid(&self, manifest: &ScanReportManifest) -> bool {
        let Some(run) = self.read_run(&manifest.run_id).ok().flatten() else {
            return false;
        };
        if run.home_id != self.home_id
            || run.run_id != manifest.run_id
            || run.generation != manifest.generation
            || run.trigger != manifest.trigger
            || run.frozen != manifest.frozen
            || run.started_at_ms != manifest.started_at_ms
        {
            return false;
        }
        let manifest_path = self.run_dir(&manifest.run_id).join("manifest.json");
        let Some(stored_manifest) = self
            .read_string(&manifest_path)
            .ok()
            .flatten()
            .and_then(|content| ScanReportManifest::parse(&content))
        else {
            return false;
        };
        if stored_manifest != *manifest {
            return false;
        }

        let valid = [
            validate_jsonl_records::<ScanCanonicalEntityRecord>(
                &self.run_dir(&manifest.run_id).join("entities.jsonl"),
                "validate Scan entity index",
            ),
            validate_jsonl_records::<ScanAppearanceRecord>(
                &self.run_dir(&manifest.run_id).join("appearances.jsonl"),
                "validate Scan appearance index",
            ),
            validate_jsonl_records::<ScanDiagnosticRecord>(
                &self.run_dir(&manifest.run_id).join("diagnostics.jsonl"),
                "validate Scan diagnostic index",
            ),
            validate_jsonl_records::<ScanSourceVerdictRecord>(
                &self.run_dir(&manifest.run_id).join("verdicts.jsonl"),
                "validate Scan verdict index",
            ),
            validate_jsonl_records::<ScanGitSourceGroupRecord>(
                &self.run_dir(&manifest.run_id).join("git_groups.jsonl"),
                "validate Scan Git source index",
            ),
            validate_jsonl_records::<ScanConflictSetRecord>(
                &self.run_dir(&manifest.run_id).join("conflict_sets.jsonl"),
                "validate Scan Conflict Set index",
            ),
        ];
        let [
            entity_count,
            appearance_count,
            _diagnostic_count,
            verdict_count,
            group_count,
            conflict_set_count,
        ] = valid;
        let (
            Ok(entity_count),
            Ok(appearance_count),
            Ok(verdict_count),
            Ok(group_count),
            Ok(conflict_set_count),
        ) = (
            entity_count,
            appearance_count,
            verdict_count,
            group_count,
            conflict_set_count,
        )
        else {
            return false;
        };
        if entity_count != manifest.counts.entities
            || appearance_count != manifest.counts.entries
            || verdict_count != entity_count
            || group_count
                != manifest.source_counts.git_groups + manifest.source_counts.git_groups_conflicted
            || conflict_set_count != manifest.source_counts.conflict_sets
        {
            return false;
        }

        for root in &manifest.roots {
            let root_json = self
                .root_dir(&manifest.run_id, &root_key(root.index))
                .join("root.json");
            let root_record = match self.read_string(&root_json).ok().flatten() {
                Some(content) => match ScanRootRecord::parse(&content) {
                    Some(record) => record,
                    None => return false,
                },
                None if root.state != ScanRootState::Completed => continue,
                None => return false,
            };
            if root_record.home_id != self.home_id
                || root_record.run_id != manifest.run_id
                || root_record.index != root.index
                || root_record.configured_path != root.configured_path
                || root_record.canonical_path != root.canonical_path
                || root_record.state != root.state
                || root_record.counts != root.counts
                || root_record.diagnostic != root.diagnostic
            {
                return false;
            }
            if root.state == ScanRootState::Completed {
                let root_dir = self.root_dir(&manifest.run_id, &root_key(root.index));
                if validate_jsonl_records::<ScanEntryRecord>(
                    &root_dir.join("entries.jsonl"),
                    "validate Scan entry evidence",
                )
                .is_err()
                    || validate_jsonl_records::<ScanEntityRecord>(
                        &root_dir.join("entities.jsonl"),
                        "validate Scan entity evidence",
                    )
                    .is_err()
                {
                    return false;
                }
            }
        }
        true
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
        let root_dir = self.root_dir(run_id, &root_key(root.index));
        fs::create_dir_all(&root_dir).map_err(|error| {
            ScanEvidenceStoreError::io("prepare Scan Root evidence", &root_dir, &error)
        })?;
        for name in ["entries.jsonl", "entities.jsonl"] {
            let path = root_dir.join(name);
            let file = fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|error| {
                    ScanEvidenceStoreError::io("prepare Scan Root evidence", &path, &error)
                })?;
            file.sync_all().map_err(|error| {
                ScanEvidenceStoreError::io("sync Scan Root evidence", &path, &error)
            })?;
        }
        let path = root_dir.join("root.json");
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
        // Canonical aggregation is disk-backed. The SQLite index is an
        // implementation detail of this temporary Run: it replaces the
        // former identity HashMap, canonical entity Vec and per-Root facts
        // HashMap without changing the JSONL/page contract.
        let index_path = self.run_dir(run_id).join(ENTITY_INDEX_FILE_NAME);
        let _ = fs::remove_file(&index_path);
        let connection =
            Connection::open(&index_path).map_err(|error| ScanEvidenceStoreError::Write {
                operation: "create Scan entity index",
                detail: format!("{}: {error}", index_path.display()),
            })?;
        connection
            .execute_batch(ENTITY_INDEX_SCHEMA)
            .map_err(|error| corrupt_artifact("create Scan entity index", error.to_string()))?;

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
        let mut appearances_count = 0_u64;
        let mut diagnostics_count = 0_u64;
        let mut next_entity_seq = 1_u64;
        for frozen_root in &run.roots {
            let root_index = frozen_root.index;
            let root_dir = roots_dir.join(format!("r{root_index}"));
            let root_json = root_dir.join("root.json");
            let root_record = match self.read_string(&root_json)? {
                Some(content) => match ScanRootRecord::parse(&content) {
                    Some(record) => record,
                    None => {
                        return Err(corrupt_artifact(
                            "read Scan Root record",
                            format!("unparseable root.json for r{root_index}"),
                        ));
                    }
                },
                None => {
                    let diagnostic = ScanDiagnosticRecord {
                        root_index,
                        kind: "root_record_missing".into(),
                        at: Some(root_json),
                        detail: Some("no root.json for a committed Root".into()),
                    };
                    append_json_line(
                        &mut diagnostics_file,
                        &diagnostics_path,
                        &diagnostic,
                        "write Scan diagnostic index",
                    )?;
                    diagnostics_count += 1;
                    continue;
                }
            };
            if root_record.home_id != self.home_id
                || root_record.run_id != run_id
                || root_record.index != frozen_root.index
                || root_record.configured_path != frozen_root.configured_path
                || root_record.canonical_path != frozen_root.canonical_path
            {
                return Err(corrupt_artifact(
                    "read Scan Root record",
                    format!("Root r{root_index} provenance does not match run.json"),
                ));
            }
            // Root is the minimum evidence commit unit (spec §4.10): a
            // failed/unresponsive Root publishes coverage + diagnostic only,
            // never its half candidates.
            if root_record.state != ScanRootState::Completed {
                let kind = if root_record
                    .diagnostic
                    .as_deref()
                    .is_some_and(|detail| detail.starts_with("non_utf8_entry"))
                {
                    "root_non_utf8"
                } else if root_record.state == ScanRootState::Unresponsive {
                    "root_unresponsive"
                } else {
                    "root_failed"
                };
                let diagnostic = ScanDiagnosticRecord {
                    root_index,
                    kind: kind.into(),
                    at: Some(root_record.canonical_path.clone()),
                    detail: root_record.diagnostic,
                };
                append_json_line(
                    &mut diagnostics_file,
                    &diagnostics_path,
                    &diagnostic,
                    "write Scan diagnostic index",
                )?;
                diagnostics_count += 1;
                continue;
            }

            // Stage one Root's entity facts in SQLite, then stream its entries
            // through a disk lookup. No number of entries can grow a Rust
            // HashMap or Vec.
            let root_entities_path = root_dir.join("entities.jsonl");
            let root_entries_path = root_dir.join("entries.jsonl");
            if !root_entities_path.is_file() || !root_entries_path.is_file() {
                return Err(corrupt_artifact(
                    "read Scan evidence stream",
                    format!("missing committed Root index for r{root_index}"),
                ));
            }
            read_jsonl_records(
                &root_entities_path,
                "read Scan evidence stream",
                |record: ScanEntityRecord| {
                    let entry_path = record.entry_path.to_string_lossy().into_owned();
                    let payload = serde_json::to_vec(&record).map_err(|error| {
                        ScanEvidenceStoreError::Write {
                            operation: "stage Scan entity facts",
                            detail: error.to_string(),
                        }
                    })?;
                    connection
                        .execute(
                            "INSERT INTO root_entity_facts(root_index, entry_path, record) \
                             VALUES (?1, ?2, ?3)",
                            params![i64::from(root_index), entry_path, payload],
                        )
                        .map_err(|error| {
                            corrupt_artifact(
                                "stage Scan entity facts",
                                format!("duplicate or invalid entity fact: {error}"),
                            )
                        })?;
                    Ok(())
                },
            )?;

            read_jsonl_records(
                &root_entries_path,
                "read Scan evidence stream",
                |entry: ScanEntryRecord| {
                    let entry_path = entry.entry_path.to_string_lossy().into_owned();
                    let fact_json: Option<Vec<u8>> = connection
                        .query_row(
                            "SELECT record FROM root_entity_facts \
                             WHERE root_index = ?1 AND entry_path = ?2",
                            params![i64::from(root_index), &entry_path],
                            |row| row.get(0),
                        )
                        .optional()
                        .map_err(|error| {
                            corrupt_artifact("join Scan evidence stream", error.to_string())
                        })?;
                    let facts = fact_json
                        .as_deref()
                        .map(serde_json::from_slice::<ScanEntityRecord>)
                        .transpose()
                        .map_err(|error| {
                            corrupt_artifact("join Scan evidence stream", error.to_string())
                        })?;
                    if entry.final_entity.is_some() != facts.is_some() {
                        return Err(corrupt_artifact(
                            "join Scan evidence stream",
                            format!(
                                "entry/entity fact mismatch at {}",
                                entry.entry_path.display()
                            ),
                        ));
                    }
                    if let (Some(expected), Some(facts)) = (&entry.final_entity, &facts) {
                        if facts.final_entity != *expected {
                            return Err(corrupt_artifact(
                                "join Scan evidence stream",
                                format!(
                                    "entry/entity final path mismatch at {}",
                                    entry.entry_path.display()
                                ),
                            ));
                        }
                    }
                    connection
                        .execute(
                            "UPDATE root_entity_facts SET matched = 1 \
                             WHERE root_index = ?1 AND entry_path = ?2",
                            params![i64::from(root_index), &entry_path],
                        )
                        .map_err(|error| {
                            corrupt_artifact("join Scan evidence stream", error.to_string())
                        })?;

                    let mut entity_seq = None;
                    // The aggregate key is the identity re-verified after
                    // the tree hash (ADR-0017 replacement guard); a stale or
                    // missing identity never guesses an entity.
                    let identity = facts.as_ref().and_then(|facts| facts.identity);
                    if let (Some(final_entity), Some(facts), Some(identity)) =
                        (&entry.final_entity, facts.as_ref(), identity)
                    {
                        if facts.tree_hash.is_some() {
                            let existing: Option<i64> = connection
                                .query_row(
                                    "SELECT entity_seq FROM canonical_entities \
                                     WHERE device = ?1 AND inode = ?2",
                                    params![identity.device as i64, identity.inode as i64],
                                    |row| row.get(0),
                                )
                                .optional()
                                .map_err(|error| {
                                    corrupt_artifact("aggregate Scan entities", error.to_string())
                                })?;
                            let seq = if let Some(existing) = existing {
                                connection
                                    .execute(
                                        "UPDATE canonical_entities \
                                         SET appearance_count = appearance_count + 1 \
                                         WHERE entity_seq = ?1",
                                        params![existing],
                                    )
                                    .map_err(|error| {
                                        corrupt_artifact(
                                            "aggregate Scan entities",
                                            error.to_string(),
                                        )
                                    })?;
                                existing as u64
                            } else {
                                let seq = next_entity_seq;
                                next_entity_seq = next_entity_seq.saturating_add(1);
                                let canonical = ScanCanonicalEntityRecord {
                                    entity_seq: seq,
                                    identity,
                                    canonical_path: final_entity.clone(),
                                    file_count: facts.file_count,
                                    byte_count: facts.byte_count,
                                    tree_hash: facts.tree_hash.clone(),
                                    hash_fault: facts.hash_fault.clone(),
                                    appearances: 1,
                                    first_root_index: root_index,
                                    first_entry_seq: entry.seq,
                                };
                                let payload = serde_json::to_vec(&canonical).map_err(|error| {
                                    ScanEvidenceStoreError::Write {
                                        operation: "stage canonical Scan entity",
                                        detail: error.to_string(),
                                    }
                                })?;
                                connection
                                    .execute(
                                        "INSERT INTO canonical_entities \
                                         (entity_seq, device, inode, record, appearance_count) \
                                         VALUES (?1, ?2, ?3, ?4, 1)",
                                        params![
                                            seq as i64,
                                            identity.device as i64,
                                            identity.inode as i64,
                                            payload
                                        ],
                                    )
                                    .map_err(|error| {
                                        corrupt_artifact(
                                            "aggregate Scan entities",
                                            error.to_string(),
                                        )
                                    })?;
                                seq
                            };
                            entity_seq = Some(seq);
                        } else {
                            let diagnostic = ScanDiagnosticRecord {
                                root_index,
                                kind: "entity_hash_fault".into(),
                                at: Some(final_entity.clone()),
                                detail: facts
                                    .hash_fault
                                    .clone()
                                    .or_else(|| Some("no tree facts for a resolved entity".into())),
                            };
                            append_json_line(
                                &mut diagnostics_file,
                                &diagnostics_path,
                                &diagnostic,
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
                                .or_else(|| {
                                    Some("final entity identity missing or changed".into())
                                }),
                        };
                        append_json_line(
                            &mut diagnostics_file,
                            &diagnostics_path,
                            &diagnostic,
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
                        append_json_line(
                            &mut diagnostics_file,
                            &diagnostics_path,
                            &diagnostic,
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
                        // The verified identity is authoritative; the
                        // walk-time entry identity stays only in raw evidence.
                        identity,
                        entity_seq,
                        lock_hint: entry.lock_hint,
                        worktree_hint: entry.worktree_hint,
                    };
                    let appearance_payload = serde_json::to_vec(&appearance).map_err(|error| {
                        ScanEvidenceStoreError::Write {
                            operation: "stage Scan appearance",
                            detail: error.to_string(),
                        }
                    })?;
                    connection
                        .execute(
                            "INSERT INTO appearances(entity_seq, record) VALUES (?1, ?2)",
                            params![entity_seq.map(|seq| seq as i64), appearance_payload],
                        )
                        .map_err(|error| {
                            corrupt_artifact("stage Scan appearances", error.to_string())
                        })?;
                    append_json_line(
                        &mut appearances_file,
                        &appearances_path,
                        &appearance,
                        "write Scan appearance index",
                    )?;
                    appearances_count += 1;
                    Ok(())
                },
            )?;
            let unmatched: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM root_entity_facts \
                     WHERE root_index = ?1 AND matched = 0",
                    params![i64::from(root_index)],
                    |row| row.get(0),
                )
                .map_err(|error| {
                    corrupt_artifact("join Scan evidence stream", error.to_string())
                })?;
            if unmatched != 0 {
                return Err(corrupt_artifact(
                    "join Scan evidence stream",
                    format!("{unmatched} entity facts have no appearance"),
                ));
            }
        }

        let mut entity_statement = connection
            .prepare(
                "SELECT record, appearance_count FROM canonical_entities \
                 ORDER BY entity_seq",
            )
            .map_err(|error| corrupt_artifact("read canonical Scan entities", error.to_string()))?;
        let mut entity_rows = entity_statement
            .query([])
            .map_err(|error| corrupt_artifact("read canonical Scan entities", error.to_string()))?;
        let mut entity_count = 0_u64;
        while let Some(row) = entity_rows
            .next()
            .map_err(|error| corrupt_artifact("read canonical Scan entities", error.to_string()))?
        {
            let payload: Vec<u8> = row.get(0).map_err(|error| {
                corrupt_artifact("read canonical Scan entities", error.to_string())
            })?;
            let appearance_count: i64 = row.get(1).map_err(|error| {
                corrupt_artifact("read canonical Scan entities", error.to_string())
            })?;
            let mut entity = serde_json::from_slice::<ScanCanonicalEntityRecord>(&payload)
                .map_err(|error| {
                    corrupt_artifact("read canonical Scan entities", error.to_string())
                })?;
            entity.appearances = u64::try_from(appearance_count).map_err(|_| {
                corrupt_artifact("read canonical Scan entities", "negative appearance count")
            })?;
            append_json_line(
                &mut entities_file,
                &entities_path,
                &entity,
                "write canonical Scan entity index",
            )?;
            entity_count += 1;
        }
        drop(entity_rows);
        drop(entity_statement);

        // Deterministic streamed indexes: every output is flushed and
        // fsynced before classification or the manifest switch can observe it.
        for (file, path) in [
            (&mut entities_file, &entities_path),
            (&mut appearances_file, &appearances_path),
            (&mut diagnostics_file, &diagnostics_path),
        ] {
            file.flush()
                .and_then(|()| file.sync_all())
                .map_err(|error| ScanEvidenceStoreError::io("flush Scan index", path, &error))?;
        }
        drop(connection);
        Ok(ScanEntityIndexStats {
            entities: entity_count,
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
            ScanReportSection::GitSources => {
                self.page_file_rows::<ScanGitSourceGroupRecord>(cursor, limit, |record| {
                    ScanReportRow::GitSourceGroup(Box::new(record))
                })?
            }
            ScanReportSection::LocalCandidates => {
                self.page_verdict_rows(cursor, limit, |verdict| verdict.verdict == "local")?
            }
            ScanReportSection::Excluded => self.page_verdict_rows(cursor, limit, |verdict| {
                matches!(verdict.verdict.as_str(), "excluded" | "already_managed")
            })?,
            ScanReportSection::ConflictSets => {
                self.page_file_rows::<ScanConflictSetRecord>(cursor, limit, |record| {
                    ScanReportRow::ConflictSet(Box::new(record))
                })?
            }
            ScanReportSection::NeedsAttention => {
                self.page_verdict_rows(cursor, limit, |verdict| {
                    matches!(
                        verdict.verdict.as_str(),
                        "blocked" | "deferred" | "identity_conflict"
                    )
                })?
            }
        };
        Ok(ScanReportPageRead { rows, next_offset })
    }

    fn read_entity(
        &self,
        run_id: &str,
        entity_seq: u64,
    ) -> Result<Option<ScanCanonicalEntityRecord>, ScanEvidenceStoreError> {
        self.verify_run_home(run_id)?;
        self.read_index_record::<ScanCanonicalEntityRecord>(run_id, "entities.jsonl", |record| {
            record.entity_seq == entity_seq
        })
    }

    fn read_verdict(
        &self,
        run_id: &str,
        entity_seq: u64,
    ) -> Result<Option<ScanSourceVerdictRecord>, ScanEvidenceStoreError> {
        self.verify_run_home(run_id)?;
        self.read_index_record::<ScanSourceVerdictRecord>(run_id, "verdicts.jsonl", |record| {
            record.entity_seq == entity_seq
        })
    }

    fn read_appearances(
        &self,
        run_id: &str,
        entity_seq: u64,
    ) -> Result<Vec<ScanAppearanceRecord>, ScanEvidenceStoreError> {
        self.verify_run_home(run_id)?;
        let path = self.run_dir(run_id).join("appearances.jsonl");
        let mut rows = Vec::new();
        read_jsonl_records(
            &path,
            "read Scan appearance index",
            |record: ScanAppearanceRecord| {
                if record.entity_seq == Some(entity_seq) {
                    rows.push(record);
                }
                Ok(())
            },
        )?;
        Ok(rows)
    }

    fn open_classification_spool(
        &self,
        run_id: &str,
    ) -> Result<Box<dyn ScanClassificationSpool>, ScanEvidenceStoreError> {
        self.verify_run_home(run_id)?;
        let path = self.run_dir(run_id).join(CLASSIFICATION_SPOOL_FILE_NAME);
        let _ = fs::remove_file(&path);
        let connection =
            Connection::open(&path).map_err(|error| ScanEvidenceStoreError::Write {
                operation: "create Scan classification spool",
                detail: format!("{}: {error}", path.display()),
            })?;
        connection
            .execute_batch(CLASSIFICATION_SPOOL_SCHEMA)
            .map_err(|error| {
                corrupt_artifact("create Scan classification spool", error.to_string())
            })?;
        Ok(Box::new(SqliteClassificationSpool { connection }))
    }

    fn stream_classification_rows(
        &self,
        run_id: &str,
        visitor: &mut dyn FnMut(ScanClassificationRow) -> Result<(), ScanEvidenceStoreError>,
    ) -> Result<(), ScanEvidenceStoreError> {
        self.verify_run_home(run_id)?;
        let index_path = self.run_dir(run_id).join(ENTITY_INDEX_FILE_NAME);
        let connection = Connection::open_with_flags(
            &index_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| corrupt_artifact("open Scan entity index", error.to_string()))?;
        let mut entity_statement = connection
            .prepare("SELECT entity_seq, record FROM canonical_entities ORDER BY entity_seq")
            .map_err(|error| corrupt_artifact("read canonical Scan entities", error.to_string()))?;
        let mut appearance_statement = connection
            .prepare("SELECT record FROM appearances WHERE entity_seq = ?1 ORDER BY ordinal")
            .map_err(|error| corrupt_artifact("read Scan appearances", error.to_string()))?;
        let mut entity_rows = entity_statement
            .query([])
            .map_err(|error| corrupt_artifact("read canonical Scan entities", error.to_string()))?;
        while let Some(entity_row) = entity_rows
            .next()
            .map_err(|error| corrupt_artifact("read canonical Scan entities", error.to_string()))?
        {
            let entity_seq: i64 = entity_row.get(0).map_err(|error| {
                corrupt_artifact("read canonical Scan entities", error.to_string())
            })?;
            let entity_payload: Vec<u8> = entity_row.get(1).map_err(|error| {
                corrupt_artifact("read canonical Scan entities", error.to_string())
            })?;
            let entity = serde_json::from_slice::<ScanCanonicalEntityRecord>(&entity_payload)
                .map_err(|error| {
                    corrupt_artifact("read canonical Scan entities", error.to_string())
                })?;
            let mut classification = ScanClassificationRow {
                entity_seq: entity.entity_seq,
                identity: entity.identity,
                canonical_path: entity.canonical_path,
                file_count: entity.file_count,
                byte_count: entity.byte_count,
                tree_hash: entity.tree_hash,
                hash_fault: entity.hash_fault,
                directory_names: Vec::new(),
                appearances: entity.appearances,
                first_root_index: entity.first_root_index,
                lock_claims: Vec::new(),
                worktree_hints: Vec::new(),
            };
            let mut appearance_rows = appearance_statement
                .query(params![entity_seq])
                .map_err(|error| corrupt_artifact("read Scan appearances", error.to_string()))?;
            while let Some(appearance_row) = appearance_rows
                .next()
                .map_err(|error| corrupt_artifact("read Scan appearances", error.to_string()))?
            {
                let appearance_payload: Vec<u8> = appearance_row.get(0).map_err(|error| {
                    corrupt_artifact("read Scan appearances", error.to_string())
                })?;
                let appearance = serde_json::from_slice::<ScanAppearanceRecord>(
                    &appearance_payload,
                )
                .map_err(|error| corrupt_artifact("read Scan appearances", error.to_string()))?;
                if appearance.entity_seq != Some(entity.entity_seq) {
                    return Err(corrupt_artifact(
                        "read Scan appearances",
                        format!(
                            "appearance points at {:?}, expected {}",
                            appearance.entity_seq, entity.entity_seq
                        ),
                    ));
                }
                if !classification
                    .directory_names
                    .iter()
                    .any(|name| name == &appearance.name)
                {
                    classification.directory_names.push(appearance.name);
                }
                if let Some(hint) = appearance.lock_hint {
                    let claim = ScanLockClaimRecord {
                        lock_path: hint.lock_path,
                        entry_name: hint.entry_name,
                        fingerprint: hint.fingerprint,
                        source_type: hint.source_type,
                        source_url: hint.source_url,
                        requested_ref: hint.requested_ref,
                        skill_path: hint.skill_path,
                    };
                    if !classification.lock_claims.iter().any(|existing| {
                        existing.lock_path == claim.lock_path
                            && existing.entry_name == claim.entry_name
                    }) {
                        classification.lock_claims.push(claim);
                    }
                }
                if let Some(hint) = appearance.worktree_hint {
                    if !classification.worktree_hints.iter().any(|existing| {
                        existing.repository_root == hint.repository_root
                            && existing.gitdir_kind == hint.gitdir_kind
                    }) {
                        classification.worktree_hints.push(hint);
                    }
                }
            }
            visitor(classification)?;
        }
        Ok(())
    }

    fn write_classification_stream(
        &self,
        run_id: &str,
        write: &mut dyn FnMut(
            &mut dyn ScanClassificationWriter,
        ) -> Result<(), ScanEvidenceStoreError>,
    ) -> Result<(), ScanEvidenceStoreError> {
        let Some(run) = self.read_run(run_id)? else {
            return Err(ScanEvidenceStoreError::ProvenanceMismatch {
                detail: format!("no run.json for {run_id}"),
            });
        };
        if run.home_id != self.home_id || run.run_id != run_id {
            return Err(ScanEvidenceStoreError::ProvenanceMismatch {
                detail: format!("run {run_id} belongs to home {}", run.home_id),
            });
        }
        let verdicts_path = self.run_dir(run_id).join("verdicts.jsonl");
        let groups_path = self.run_dir(run_id).join("git_groups.jsonl");
        let sets_path = self.run_dir(run_id).join("conflict_sets.jsonl");
        let writer = |path: &Path| {
            fs::File::create(path).map_err(|error| {
                ScanEvidenceStoreError::io("write Scan classification index", path, &error)
            })
        };
        let mut writer = JsonlClassificationWriter {
            verdicts: writer(&verdicts_path)?,
            groups: writer(&groups_path)?,
            conflict_sets: writer(&sets_path)?,
            verdicts_path,
            groups_path,
            conflict_sets_path: sets_path,
        };
        write(&mut writer)?;
        writer.flush_and_sync()
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
        let content = match self.read_string(&self.scan_dir.join("current.json")) {
            Ok(content) => content,
            Err(_) => return Ok(CurrentManifestRead::Corrupt),
        };
        let Some(content) = content else {
            return Ok(CurrentManifestRead::Absent);
        };
        let Some(manifest) = ScanReportManifest::parse(&content) else {
            return Ok(CurrentManifestRead::Corrupt);
        };
        if self.report_artifacts_are_valid(&manifest) {
            Ok(CurrentManifestRead::Report(manifest))
        } else {
            Ok(CurrentManifestRead::Corrupt)
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
        let Some(run) = self.read_run(run_id)? else {
            return Err(ScanEvidenceStoreError::ProvenanceMismatch {
                detail: format!("no run.json for {run_id}"),
            });
        };
        if run.home_id != self.home_id
            || run.run_id != run_id
            || run.generation != manifest.generation
            || run.trigger != manifest.trigger
            || run.frozen != manifest.frozen
            || run.started_at_ms != manifest.started_at_ms
        {
            return Err(ScanEvidenceStoreError::ProvenanceMismatch {
                detail: format!("manifest {run_id} does not match its run provenance"),
            });
        }
        let manifest_json = serde_json::to_string_pretty(manifest).map_err(|error| {
            ScanEvidenceStoreError::Write {
                operation: "write Scan Report manifest",
                detail: error.to_string(),
            }
        })?;
        if ScanReportManifest::parse(&manifest_json).is_none() {
            return Err(corrupt_artifact(
                "write Scan Report manifest",
                "manifest integrity or terminal coverage is invalid",
            ));
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
        self.write_string_atomic(
            &self.scan_dir.join("current.json"),
            &manifest_json,
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
            ScanReportSection::GitSources => "git_groups.jsonl",
            ScanReportSection::ConflictSets => "conflict_sets.jsonl",
            ScanReportSection::LocalCandidates
            | ScanReportSection::NeedsAttention
            | ScanReportSection::Excluded => {
                unreachable!("verdict rows use page_verdict_rows")
            }
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
            if line.last() != Some(&b'\n') {
                return Err(ScanReportPageError::NotFound);
            }
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let record =
                serde_json::from_slice::<T>(&line).map_err(|_| ScanReportPageError::NotFound)?;
            rows.push(row_kind(record));
        }
        let at_end = reader.fill_buf().map(|buf| buf.is_empty()).unwrap_or(true);
        let next_offset = if at_end {
            None
        } else {
            Some(cursor.offset + consumed)
        };
        Ok((rows, next_offset))
    }

    /// Verdict-file paging with a closed predicate (Local candidates /
    /// Needs attention / Excluded rows all live in `verdicts.jsonl`).
    fn page_verdict_rows(
        &self,
        cursor: &ScanReportCursor,
        limit: usize,
        include: impl Fn(&ScanSourceVerdictRecord) -> bool,
    ) -> Result<(Vec<ScanReportRow>, Option<u64>), ScanReportPageError> {
        let path = self.run_dir(&cursor.run_id).join("verdicts.jsonl");
        let file = File::open(&path).map_err(|_| ScanReportPageError::NotFound)?;
        let mut reader = BufReader::new(file);
        reader
            .seek(SeekFrom::Start(cursor.offset))
            .map_err(|_| ScanReportPageError::NotFound)?;
        let mut rows = Vec::new();
        let mut consumed = 0_u64;
        let mut end_of_file = false;
        while rows.len() < limit {
            let mut line = Vec::new();
            let read = reader
                .read_until(b'\n', &mut line)
                .map_err(|_| ScanReportPageError::NotFound)?;
            if read == 0 {
                end_of_file = true;
                break;
            }
            consumed += read as u64;
            if line.last() != Some(&b'\n') {
                return Err(ScanReportPageError::NotFound);
            }
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let record = serde_json::from_slice::<ScanSourceVerdictRecord>(&line)
                .map_err(|_| ScanReportPageError::NotFound)?;
            if include(&record) {
                rows.push(ScanReportRow::SourceVerdict(Box::new(record)));
            }
        }
        let at_end = reader.fill_buf().map(|buf| buf.is_empty()).unwrap_or(true);
        let next_offset = if at_end && !end_of_file {
            // The file ends exactly where the last line was read.
            None
        } else if end_of_file {
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

    fn stream_classification_rows(
        &self,
        run_id: &str,
        visitor: &mut dyn FnMut(ScanClassificationRow) -> Result<(), ScanEvidenceStoreError>,
    ) -> Result<(), ScanEvidenceStoreError> {
        self.inner.stream_classification_rows(run_id, visitor)
    }

    fn open_classification_spool(
        &self,
        run_id: &str,
    ) -> Result<Box<dyn ScanClassificationSpool>, ScanEvidenceStoreError> {
        self.inner.open_classification_spool(run_id)
    }

    fn write_classification_stream(
        &self,
        run_id: &str,
        write: &mut dyn FnMut(
            &mut dyn ScanClassificationWriter,
        ) -> Result<(), ScanEvidenceStoreError>,
    ) -> Result<(), ScanEvidenceStoreError> {
        self.maybe_fail(crate::seams::scan_evidence_store::fault_points::CLASSIFICATION)?;
        self.maybe_fail(crate::seams::scan_evidence_store::fault_points::DISK_FULL)?;
        self.inner.write_classification_stream(run_id, write)
    }

    fn report_page(
        &self,
        cursor: &ScanReportCursor,
        limit: usize,
    ) -> Result<ScanReportPageRead, ScanReportPageError> {
        self.inner.report_page(cursor, limit)
    }

    fn read_entity(
        &self,
        run_id: &str,
        entity_seq: u64,
    ) -> Result<Option<ScanCanonicalEntityRecord>, ScanEvidenceStoreError> {
        self.inner.read_entity(run_id, entity_seq)
    }

    fn read_verdict(
        &self,
        run_id: &str,
        entity_seq: u64,
    ) -> Result<Option<ScanSourceVerdictRecord>, ScanEvidenceStoreError> {
        self.inner.read_verdict(run_id, entity_seq)
    }

    fn read_appearances(
        &self,
        run_id: &str,
        entity_seq: u64,
    ) -> Result<Vec<ScanAppearanceRecord>, ScanEvidenceStoreError> {
        self.inner.read_appearances(run_id, entity_seq)
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
    manifest.integrity = None;
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
        ScanRootCoverageRecord, ScanRootState, ScanSourceCounts, fault_points,
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
                consumer_agents: Vec::new(),
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

    fn verdict_record(seq: u64, name: &str) -> ScanSourceVerdictRecord {
        ScanSourceVerdictRecord {
            entity_seq: seq,
            verdict: "local".into(),
            canonical_path: PathBuf::from(format!("/tmp/root/{name}")),
            directory_names: vec![name.into()],
            appearances: 1,
            file_count: 1,
            byte_count: 64,
            tree_hash: Some(format!("tree-sha256-v1:{name}")),
            lock_claims: Vec::new(),
            worktree_hints: Vec::new(),
            reason_kind: None,
            detail: None,
            git_refs: Vec::new(),
            git_lock_paths: Vec::new(),
            git_group_seq: None,
            conflict_set_seq: None,
            notes: Vec::new(),
            operations: Vec::new(),
        }
    }

    fn root_record(index: u32, state: ScanRootState) -> ScanRootRecord {
        root_record_for("run-1", index, state)
    }

    fn root_record_for(run_id: &str, index: u32, state: ScanRootState) -> ScanRootRecord {
        ScanRootRecord {
            schema_version: SCAN_STORE_SCHEMA_VERSION,
            home_id: "b1c4e6f8-1a2b-4c3d-8e9f-0123456789ab".into(),
            run_id: run_id.into(),
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
            source_counts: ScanSourceCounts::default(),
            roots: vec![ScanRootCoverageRecord {
                index: 0,
                configured_path: PathBuf::from("/tmp/root"),
                canonical_path: PathBuf::from("/tmp/root"),
                state: ScanRootState::Completed,
                consumer_agents: Vec::new(),
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

    fn prepare_publishable_empty_report(
        store: &dyn ScanEvidenceStore,
        home: &BoundHome,
        run_id: &str,
    ) {
        store.create_run(&run_record(home, run_id)).unwrap();
        store
            .write_root(
                run_id,
                &root_record_for(run_id, 0, ScanRootState::Completed),
            )
            .unwrap();
        store.build_entity_index(run_id).unwrap();
        store
            .write_classification_stream(run_id, &mut |_writer| Ok(()))
            .unwrap();
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
        store
            .write_root("run-1", &root_record(0, ScanRootState::Completed))
            .unwrap();
        store.build_entity_index("run-1").unwrap();
        store
            .write_classification_stream("run-1", &mut |_writer| Ok(()))
            .unwrap();
        assert!(matches!(
            store.current_manifest().unwrap(),
            CurrentManifestRead::Absent
        ));
        let mut manifest = manifest(&home, "run-1", "complete");
        manifest.counts.entries = 1;
        manifest = seal_manifest(manifest);
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
        prepare_publishable_empty_report(&store, &home, "run-1");
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
    fn malformed_or_missing_index_is_no_cached_report() {
        let temp = tempfile::tempdir().unwrap();
        let home = home();
        let store = store(&temp, &home);
        prepare_publishable_empty_report(&store, &home, "run-1");
        store
            .publish_report("run-1", &manifest(&home, "run-1", "complete"))
            .unwrap();

        std::fs::write(
            temp.path().join("scan/runs/run-1/appearances.jsonl"),
            b"{malformed}\n",
        )
        .unwrap();
        assert!(matches!(
            store.current_manifest().unwrap(),
            CurrentManifestRead::Corrupt
        ));

        std::fs::remove_file(temp.path().join("scan/runs/run-1/verdicts.jsonl")).unwrap();
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
    fn orphan_sweep_failure_keeps_proven_artifact_for_retry() {
        let root = tempfile::tempdir().unwrap();
        let factory = FaultInjectingScanEvidenceStoreFactory::new(root.path().to_path_buf());
        let home = home();
        let store = factory.store_for(&home).unwrap();
        store.create_run(&run_record(&home, "orphan")).unwrap();
        factory.fail_next(fault_points::ORPHAN_CLEANUP);
        assert!(store.cleanup_temporary_runs().is_err());
        assert!(
            root.path()
                .join(&home.home_id.0)
                .join("scan/runs/orphan")
                .exists()
        );
    }

    #[test]
    fn publish_keeps_old_report_on_manifest_switch_failure() {
        let temp = tempfile::tempdir().unwrap();
        let home = home();
        let store = store(&temp, &home);
        prepare_publishable_empty_report(&store, &home, "run-1");
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
        prepare_publishable_empty_report(store.as_ref(), &home, "run-1");
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
        let mut manifest = manifest(&home, run_id, "complete");
        manifest.counts.entries = 3;
        manifest.counts.entities = 2;
        manifest = seal_manifest(manifest);
        store
            .write_classification_stream(run_id, &mut |writer| {
                writer.append_verdict(&verdict_record(1, "alpha"))?;
                writer.append_verdict(&verdict_record(2, "gamma"))?;
                Ok(())
            })
            .unwrap();
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
    fn entity_read_contracts_serve_generation_bound_rows() {
        let temp = tempfile::tempdir().unwrap();
        let home = home();
        let store = store(&temp, &home);
        let run_id = "run-1";
        store.create_run(&run_record(&home, run_id)).unwrap();
        let identity_a = ScanObjectIdentity {
            device: 1,
            inode: 100,
        };
        // Two canonical entities; entity 1 has two appearances (alpha via
        // symlink chain, beta direct), entity 2 one appearance.
        store
            .append_entry(run_id, "r0", &entry_with_identity(1, "alpha", identity_a))
            .unwrap();
        store
            .append_entry(run_id, "r0", &entry_with_identity(2, "beta", identity_a))
            .unwrap();
        store
            .append_entry(
                run_id,
                "r0",
                &entry_with_identity(
                    3,
                    "gamma",
                    ScanObjectIdentity {
                        device: 1,
                        inode: 200,
                    },
                ),
            )
            .unwrap();
        for (seq, name) in [(1, "alpha"), (2, "beta"), (3, "gamma")] {
            let identity = if seq == 3 {
                ScanObjectIdentity {
                    device: 1,
                    inode: 200,
                }
            } else {
                identity_a
            };
            store
                .append_entity(run_id, "r0", &entity_record(seq, name, identity))
                .unwrap();
        }
        store
            .write_root(run_id, &root_record(0, ScanRootState::Completed))
            .unwrap();
        store.build_entity_index(run_id).unwrap();
        let verdicts = vec![
            ScanSourceVerdictRecord {
                entity_seq: 1,
                verdict: "local".into(),
                canonical_path: PathBuf::from("/tmp/alpha"),
                directory_names: vec!["alpha".into(), "beta".into()],
                appearances: 2,
                file_count: 3,
                byte_count: 10,
                tree_hash: Some("tree-1".into()),
                lock_claims: Vec::new(),
                worktree_hints: Vec::new(),
                reason_kind: None,
                detail: None,
                git_refs: Vec::new(),
                git_lock_paths: Vec::new(),
                git_group_seq: None,
                conflict_set_seq: None,
                notes: Vec::new(),
                operations: Vec::new(),
            },
            ScanSourceVerdictRecord {
                entity_seq: 2,
                verdict: "excluded".into(),
                canonical_path: PathBuf::from("/tmp/gamma"),
                directory_names: vec!["gamma".into()],
                appearances: 1,
                file_count: 1,
                byte_count: 1,
                tree_hash: None,
                lock_claims: Vec::new(),
                worktree_hints: Vec::new(),
                reason_kind: None,
                detail: None,
                git_refs: Vec::new(),
                git_lock_paths: Vec::new(),
                git_group_seq: None,
                conflict_set_seq: None,
                notes: Vec::new(),
                operations: Vec::new(),
            },
        ];
        store
            .write_classification_stream(run_id, &mut |writer| {
                for verdict in &verdicts {
                    writer.append_verdict(verdict)?;
                }
                Ok(())
            })
            .unwrap();
        let mut manifest = manifest(&home, run_id, "complete");
        manifest.counts.entries = 3;
        manifest.counts.entities = 2;
        manifest.source_counts.local_candidates = 1;
        manifest.source_counts.excluded = 1;
        manifest = seal_manifest(manifest);
        store.publish_report(run_id, &manifest).unwrap();

        // Entity 1: recorded facts + verdict + BOTH appearances (identical
        // object identity aggregates under one canonical row).
        let entity = store.read_entity(run_id, 1).unwrap().expect("entity 1");
        assert_eq!(
            entity.canonical_path,
            PathBuf::from("/tmp/root/alpha"),
            "the first appearance's final entity is the representative path"
        );
        assert_eq!(entity.appearances, 2);
        let verdict = store.read_verdict(run_id, 1).unwrap().expect("verdict 1");
        assert_eq!(verdict.verdict, "local");
        let appearances = store.read_appearances(run_id, 1).unwrap();
        assert_eq!(appearances.len(), 2, "both alpha + beta appearances");
        assert_eq!(
            appearances
                .iter()
                .map(|row| row.entity_seq)
                .collect::<Vec<_>>(),
            vec![Some(1), Some(1)]
        );
        // Entity 2: its own verdict, not entity 1's.
        let verdict = store.read_verdict(run_id, 2).unwrap().expect("verdict 2");
        assert_eq!(verdict.verdict, "excluded");
        assert_eq!(store.read_appearances(run_id, 2).unwrap().len(), 1);
        // A sequence that never existed is None, never a guess.
        assert!(store.read_entity(run_id, 99).unwrap().is_none());
        assert!(store.read_verdict(run_id, 99).unwrap().is_none());
        assert!(store.read_appearances(run_id, 99).unwrap().is_empty());
        // The reads are generation-bound and provenance-checked: a
        // different home can never read another Home's index.
        let other_home = BoundHome::test_value(
            "aaaaaaaa-1234-4000-8000-000000000000",
            std::path::PathBuf::from("/tmp/other-home"),
        );
        let other =
            SystemScanEvidenceStore::new(temp.path().join("scan"), other_home.home_id.0.clone());
        assert!(matches!(
            other.read_entity(run_id, 1),
            Err(ScanEvidenceStoreError::ProvenanceMismatch { .. })
        ));
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
        let mut first = manifest(&home, run_id, "complete");
        first.counts.entries = 1;
        first.counts.entities = 1;
        first = seal_manifest(first);
        store
            .write_classification_stream(run_id, &mut |writer| {
                writer.append_verdict(&verdict_record(1, "alpha"))?;
                Ok(())
            })
            .unwrap();
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
        store.create_run(&run_record(&home, "run-2")).unwrap();
        store
            .write_root(
                "run-2",
                &root_record_for("run-2", 0, ScanRootState::Completed),
            )
            .unwrap();
        store.build_entity_index("run-2").unwrap();
        store
            .write_classification_stream("run-2", &mut |_writer| Ok(()))
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
}
