//! Scan Run execution engine (spec §4.10, ADR-0020): the worker pool, the
//! streaming evidence pipeline, the progress/watchdog coordination and the
//! terminal publish. All state lives in the coordinator; this module only
//! runs one Run to completion.
//!
//! Concurrency caps: Root walkers 4, tree hash 2, local Git probe 2.
//! Backpressure: every queue is bounded; producers block instead of
//! unbounded memory. Cancellation and Supersede are cooperative at record
//! granularity. Root evidence is atomic per Root (a terminal record or a
//! typed failure diagnostic — never a partial candidate); a Report is only
//! published by the manifest switch, and a store-wide failure keeps the old
//! current Report (Run → `Failed`).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::seams::filesystem::{
    ChainFault, EvidenceChain, EvidenceChainHopKind, FileSystemError, LinkSourceEntryKind,
    ScannedSkillEntry, TreeScanEntry,
};
use crate::seams::scan_evidence_store::{
    SCAN_STORE_SCHEMA_VERSION, ScanChainFaultRecord, ScanChainHopRecord, ScanEntityIndexStats,
    ScanEntityRecord, ScanEntryRecord, ScanEvidenceStoreError, ScanLockHintRecord,
    ScanObjectIdentity, ScanRootCoverageRecord, ScanRootRecord, ScanRootState,
    ScanSnapshotQualification, ScanWorktreeHintRecord, fault_points,
};

use super::{
    RunSlot, ScanCoordinator, ScanPhase, ScanRootViewState, ScanRunState, ScanTrigger,
    build_run_snapshot, seal_manifest,
};

pub(crate) const ROOT_WORKERS: usize = 4;
pub(crate) const TREE_HASH_WORKERS: usize = 2;
pub(crate) const GIT_PROBE_WORKERS: usize = 2;
const QUEUE_CAPACITY: usize = 512;
const ROOT_SLOW_MS: u64 = 10_000;
const RUN_SLOW_MS: u64 = 30_000;
const WATCHDOG_TICK_MS: u64 = 250;

/// The engine entry point: spawns the Run thread and returns immediately.
pub(crate) fn execute(coordinator: Arc<ScanCoordinator>, slot: Arc<RunSlot>) {
    let slot_handle = slot.clone();
    let joined = std::thread::Builder::new()
        .name("scan-run".into())
        .spawn(move || run(coordinator, slot_handle))
        .expect("spawn Scan Run thread");
    *slot
        .handle
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(joined);
}

/// Per-run engine coordination state; every array is indexed by the planned
/// Root index (dense `0..progress.roots.len()`).
struct EngineShared {
    /// Outstanding entity hash jobs per Root.
    pending: Vec<AtomicUsize>,
    /// Cooperative stop flag per Root: a root-local write failure or the
    /// typed Unresponsive isolation stops this Root only.
    root_stop: Vec<AtomicBool>,
    /// Store-wide failure (disk full / store unavailable / manifest
    /// switch): the whole Run fails and keeps the old Report.
    store_wide: AtomicBool,
    store_wide_detail: Mutex<Option<String>>,
    /// Local Git probe cap (no more than 2 concurrent probes).
    git_probe: ProbeSemaphore,
    /// Memoized lock discovery per Run (facts, never the lock body).
    lock_report: Mutex<Option<Vec<crate::seams::installer_lock_store::LockFileReport>>>,
    /// Memoized worktree hints per (entry path, Root): the probe result
    /// depends on the exact entry the upward walk starts from, so siblings
    /// under one Root never share a cached hint.
    worktree_memo:
        Mutex<std::collections::HashMap<(std::path::PathBuf, u32), Option<WorktreeHintMemo>>>,
    /// Deterministic per-Root entry/entity sequence counters.
    seq: Vec<AtomicUsize>,
    entity_seq: Vec<AtomicUsize>,
    /// Generation-bound canonical entity de-duplication: final object
    /// identities already aggregated in this Run (ADR-0017). Live progress
    /// counts distinct entities; the Report's exact index is built from the
    /// healthy Roots' evidence at finalize.
    entity_ids: Mutex<std::collections::HashSet<(u64, u64)>>,
    /// Planned Root index → walkable position (engine array index).
    positions: std::collections::HashMap<u32, usize>,
}

#[derive(Clone, Debug)]
struct WorktreeHintMemo {
    repository_root: std::path::PathBuf,
    gitdir_kind: String,
    remote_urls: Vec<String>,
    head_ref: Option<String>,
}

const PROBE_CAPACITY: usize = GIT_PROBE_WORKERS;

struct ProbeSemaphore {
    permits: Mutex<usize>,
    available: Condvar,
}

impl ProbeSemaphore {
    fn new(capacity: usize) -> Self {
        Self {
            permits: Mutex::new(capacity),
            available: Condvar::new(),
        }
    }
    fn acquire(&self) {
        let mut permits = self.permits.lock().unwrap_or_else(|p| p.into_inner());
        while *permits == 0 {
            permits = self
                .available
                .wait(permits)
                .unwrap_or_else(|p| p.into_inner());
        }
        *permits -= 1;
    }
    fn release(&self) {
        let mut permits = self.permits.lock().unwrap_or_else(|p| p.into_inner());
        *permits += 1;
        self.available.notify_one();
    }
}

#[allow(clippy::large_enum_variant)]
enum StoreItem {
    Entry {
        root_index: u32,
        record: ScanEntryRecord,
    },
    Entity {
        root_index: u32,
        record: ScanEntityRecord,
    },
    Root {
        root_index: u32,
        record: ScanRootRecord,
    },
}

struct HashJob {
    root_index: u32,
    name: String,
    entry_path: std::path::PathBuf,
    final_entity: std::path::PathBuf,
    identity: Option<ScanObjectIdentity>,
}

fn run(coordinator: Arc<ScanCoordinator>, slot: Arc<RunSlot>) {
    let record = slot.record.clone();
    {
        let mut progress = slot.progress_lock();
        progress.state = ScanRunState::Running;
        progress.phase = ScanPhase::Planning;
    }
    coordinator.publish_progress(&slot, true);
    // The temporary Run artifact is created with its durable provenance
    // before any evidence is streamed; failure is store-wide → Failed.
    if let Err(error) = slot.store.create_run(&record) {
        set_state_best_effort(&slot, ScanRunState::Failed, Some(&format!("{error}")));
        let _ = slot.store.remove_run(&record.run_id);
        coordinator.publish_progress(&slot, true);
        finish(coordinator, slot);
        return;
    }
    if coordinator.frozen_superseded(&record.frozen) {
        set_state_best_effort(
            &slot,
            ScanRunState::Superseded,
            Some("frozen generations changed before the walk started"),
        );
        let _ = slot.store.remove_run(&record.run_id);
        coordinator.publish_progress(&slot, true);
        finish(coordinator, slot);
        return;
    }
    if slot.shared.cancel.load(Ordering::Acquire) {
        set_state_best_effort(
            &slot,
            ScanRunState::Cancelled,
            Some("cancelled before walking"),
        );
        let _ = slot.store.remove_run(&record.run_id);
        coordinator.publish_progress(&slot, true);
        finish(coordinator, slot);
        return;
    }
    let walkable = {
        let progress = slot.progress_lock();
        progress
            .roots
            .iter()
            .filter(|root| root.state == ScanRootViewState::Pending)
            .map(|root| root.index)
            .collect::<Vec<_>>()
    };
    let engine = Arc::new(EngineShared {
        pending: (0..walkable.len()).map(|_| AtomicUsize::new(0)).collect(),
        root_stop: (0..walkable.len())
            .map(|_| AtomicBool::new(false))
            .collect(),
        store_wide: AtomicBool::new(false),
        store_wide_detail: Mutex::new(None),
        git_probe: ProbeSemaphore::new(PROBE_CAPACITY),
        lock_report: Mutex::new(None),
        worktree_memo: Mutex::new(std::collections::HashMap::new()),
        seq: (0..walkable.len()).map(|_| AtomicUsize::new(0)).collect(),
        entity_seq: (0..walkable.len()).map(|_| AtomicUsize::new(0)).collect(),
        entity_ids: Mutex::new(std::collections::HashSet::new()),
        positions: walkable
            .iter()
            .enumerate()
            .map(|(index, root)| (*root, index))
            .collect(),
    });

    let (writer_tx, writer_rx) = sync_channel::<StoreItem>(QUEUE_CAPACITY);
    let (hash_tx, hash_rx) = sync_channel::<HashJob>(QUEUE_CAPACITY);
    let hash_rx = Arc::new(Mutex::new(hash_rx));

    let writer = {
        let slot = slot.clone();
        let engine = engine.clone();
        std::thread::Builder::new()
            .name("scan-writer".into())
            .spawn(move || writer_loop(&slot, &engine, writer_rx))
            .expect("spawn Scan writer thread")
    };
    let hash_workers = (0..TREE_HASH_WORKERS)
        .map(|_| {
            let coordinator = coordinator.clone();
            let slot = slot.clone();
            let engine = engine.clone();
            let writer_tx = writer_tx.clone();
            let hash_rx = hash_rx.clone();
            std::thread::Builder::new()
                .name("scan-hash".into())
                .spawn(move || hash_worker_loop(coordinator, slot, engine, writer_tx, hash_rx))
                .expect("spawn Scan hash thread")
        })
        .collect::<Vec<_>>();
    drop(hash_rx);

    // Shared pull queue: a Root is claimed exactly once; the worker pool
    // caps concurrent walks at ROOT_WORKERS (spec §4.10).
    let work_queue: Arc<Mutex<std::collections::VecDeque<u32>>> =
        Arc::new(Mutex::new(walkable.into_iter().collect()));
    let mut root_workers = Vec::new();
    for worker_index in 0..ROOT_WORKERS {
        let coordinator = coordinator.clone();
        let slot = slot.clone();
        let engine = engine.clone();
        let writer_tx = writer_tx.clone();
        let hash_tx = hash_tx.clone();
        let work_queue = work_queue.clone();
        root_workers.push(
            std::thread::Builder::new()
                .name(format!("scan-root-{worker_index}"))
                .spawn(move || {
                    root_worker(coordinator, slot, engine, writer_tx, hash_tx, work_queue)
                })
                .expect("spawn Scan root thread"),
        );
    }
    drop(hash_tx);
    drop(writer_tx);

    let watchdog = {
        let coordinator = coordinator.clone();
        let slot = slot.clone();
        let engine = engine.clone();
        std::thread::Builder::new()
            .name("scan-watchdog".into())
            .spawn(move || watchdog_loop(coordinator, slot, engine))
            .expect("spawn Scan watchdog thread")
    };

    for root in root_workers {
        let _ = root.join();
    }
    // The walk is done: release the watchdog and the remaining channel
    // senders so the workers drain and exit (never a circular join wait:
    // the watchdog only exits on terminal state, which finalize sets).
    slot.shared.done.store(true, Ordering::Release);
    let _ = watchdog.join();
    for worker in hash_workers {
        let _ = worker.join();
    }
    let _ = writer.join();

    finalize(coordinator, slot, engine);
}

#[allow(clippy::too_many_arguments)]
/// One root worker: walks the walkable Roots one at a time, streams entry
/// evidence, enqueues entity hashing, then waits for that Root's entity
/// evidence to flush before committing the Root record.
fn root_worker(
    coordinator: Arc<ScanCoordinator>,
    slot: Arc<RunSlot>,
    engine: Arc<EngineShared>,
    writer_tx: std::sync::mpsc::SyncSender<StoreItem>,
    hash_tx: std::sync::mpsc::SyncSender<HashJob>,
    work_queue: Arc<Mutex<std::collections::VecDeque<u32>>>,
) {
    loop {
        if cancelled_or_superseded(&slot) || engine.store_wide.load(Ordering::Acquire) {
            return;
        }
        let claimed = {
            let mut queue = work_queue
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            queue.pop_front()
        };
        let Some(root_index) = claimed else {
            return;
        };
        let heap_index = engine
            .heap_of(&root_index)
            .expect("a claimed Root is mapped");
        process_root(
            &coordinator,
            &slot,
            &engine,
            &writer_tx,
            &hash_tx,
            root_index,
            heap_index,
        );
    }
}

fn process_root(
    coordinator: &ScanCoordinator,
    slot: &RunSlot,
    engine: &EngineShared,
    writer_tx: &std::sync::mpsc::SyncSender<StoreItem>,
    hash_tx: &std::sync::mpsc::SyncSender<HashJob>,
    root_index: u32,
    heap_index: usize,
) {
    // The frozen identity is re-verified before any evidence is streamed
    // (spec §4.10: identity replacement is a failure, never a guess).
    let canonical = {
        let progress = slot.progress_lock();
        let root = progress
            .roots
            .iter()
            .find(|root| root.index == root_index)
            .expect("walkable Root is in progress");
        root.canonical_path.clone()
    };
    let identity_ok = coordinator
        .filesystem
        .directory_fingerprint(&canonical)
        .map(|fingerprint| {
            let progress = slot.progress_lock();
            let root = progress
                .roots
                .iter()
                .find(|root| root.index == root_index)
                .expect("walkable Root is in progress");
            fingerprint.device == root.device && fingerprint.inode == root.inode
        })
        .unwrap_or(false);
    if !identity_ok {
        fail_root_local(
            slot,
            engine,
            root_index,
            heap_index,
            "Root identity changed before walking".to_owned(),
        );
        return;
    }
    {
        let mut progress = slot.progress_lock();
        let root = progress
            .roots
            .iter_mut()
            .find(|root| root.index == root_index)
            .expect("walkable Root is in progress");
        root.state = ScanRootViewState::Walking;
        root.last_progress = Instant::now();
        progress.current_root = Some(root_index);
        progress.phase = ScanPhase::Walking;
    }
    coordinator.publish_progress(slot, true);

    let outcome = walk_root(
        coordinator,
        slot,
        engine,
        writer_tx,
        hash_tx,
        root_index,
        heap_index,
        canonical,
    );
    // A store-wide failure supersedes root bookkeeping.
    while engine.pending[heap_index].load(Ordering::Acquire) > 0 {
        if engine.store_wide.load(Ordering::Acquire) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    if let Err(error) = outcome {
        fail_root_local(slot, engine, root_index, heap_index, error);
        return;
    }
    if cancelled_or_superseded(slot) || engine.store_wide.load(Ordering::Acquire) {
        return;
    }
    // Root atomic commit: only the complete transaction writes the terminal
    // record; a failed/unresponsive Root publishes a typed record carrying
    // coverage + diagnostic only (spec §4.10 — never a half candidate).
    let mut progress = slot.progress_lock();
    let root = progress
        .roots
        .iter_mut()
        .find(|root| root.index == root_index)
        .expect("walked Root is in progress");
    let (state, diagnostic): (ScanRootState, Option<String>) = match root.state {
        ScanRootViewState::Unresponsive => (
            ScanRootState::Unresponsive,
            Some(
                root.diagnostic
                    .clone()
                    .unwrap_or_else(|| "unresponsive".into()),
            ),
        ),
        ScanRootViewState::Failed => (
            ScanRootState::Failed,
            Some(root.diagnostic.clone().unwrap_or_else(|| "failed".into())),
        ),
        _ => {
            root.state = ScanRootViewState::Completed;
            (ScanRootState::Completed, None)
        }
    };
    drop(progress);
    let record = build_root_record(slot, root_index, state, diagnostic);
    let _ = writer_tx.send(StoreItem::Root { root_index, record });
}

#[allow(clippy::too_many_arguments)]
fn walk_root(
    coordinator: &ScanCoordinator,
    slot: &RunSlot,
    engine: &EngineShared,
    writer_tx: &std::sync::mpsc::SyncSender<StoreItem>,
    hash_tx: &std::sync::mpsc::SyncSender<HashJob>,
    root_index: u32,
    heap_index: usize,
    canonical: std::path::PathBuf,
) -> Result<(), String> {
    coordinator
        .filesystem
        .scan_skills_directory_stream(&canonical, &mut |entry: ScannedSkillEntry| {
            if cancelled_or_superseded(slot)
                || engine.store_wide.load(Ordering::Acquire)
                || engine.root_stop[heap_index].load(Ordering::Acquire)
            {
                return Ok(false);
            }
            let outcome = process_entry(
                coordinator,
                slot,
                engine,
                writer_tx,
                hash_tx,
                root_index,
                heap_index,
                canonical.clone(),
                entry,
            );
            match outcome {
                Ok(()) => Ok(true),
                Err(error) => Err(FileSystemError::Io {
                    operation: "Scan entry evidence",
                    path: canonical.clone(),
                    source: std::io::Error::other(error),
                }),
            }
        })
        .map_err(|error| error.to_string())
}

#[allow(clippy::too_many_arguments)]
fn process_entry(
    coordinator: &ScanCoordinator,
    slot: &RunSlot,
    engine: &EngineShared,
    writer_tx: &std::sync::mpsc::SyncSender<StoreItem>,
    hash_tx: &std::sync::mpsc::SyncSender<HashJob>,
    root_index: u32,
    heap_index: usize,
    canonical: std::path::PathBuf,
    entry: ScannedSkillEntry,
) -> Result<(), String> {
    // Bounded chain evidence (16-hop/cycle/non-UTF-8 fail closed; the fault
    // is recorded, never a partial fingerprint).
    let chain = coordinator
        .filesystem
        .inspect_evidence_chain(&entry.entry_path)
        .map_err(|error| error.to_string())?;
    let lock_hint = lock_hint_for(coordinator, engine, &entry.name);
    let worktree_hint = worktree_hint_for(
        coordinator,
        engine,
        root_index,
        &canonical,
        &entry.entry_path,
    )?;
    let (chain_hops, chain_fault) = chain_to_record(&chain);
    // Generation-bound object identity of the resolved final entity
    // (ADR-0017): valid only inside this Report, so the appearance carries
    // the fact without ever becoming a persistent Skill identity. A read
    // failure yields no identity (never a guessed entity).
    let identity = chain.final_entity.as_ref().and_then(|final_entity| {
        coordinator
            .filesystem
            .directory_fingerprint(final_entity)
            .ok()
            .map(|fingerprint| ScanObjectIdentity {
                device: fingerprint.device,
                inode: fingerprint.inode,
            })
    });
    let record = ScanEntryRecord {
        seq: engine.seq[heap_index].fetch_add(1, Ordering::Relaxed) as u64,
        name: entry.name.clone(),
        entry_path: entry.entry_path.clone(),
        entry_kind: match entry.kind {
            LinkSourceEntryKind::Directory => "directory".into(),
            LinkSourceEntryKind::Symlink { .. } => "symlink".into(),
        },
        chain: chain_hops,
        chain_fault,
        final_entity: chain.final_entity.clone(),
        identity,
        lock_hint,
        worktree_hint,
    };
    writer_tx
        .send(StoreItem::Entry { root_index, record })
        .map_err(|_| "the Evidence Store writer stopped".to_owned())?;
    {
        let mut progress = slot.progress_lock();
        let root = progress
            .roots
            .iter_mut()
            .find(|root| root.index == root_index)
            .expect("walkable Root is in progress");
        root.counts.entries += 1;
        root.last_progress = Instant::now();
        progress.counts.entries += 1;
    }
    if let Some(final_entity) = &chain.final_entity {
        engine.pending[heap_index].fetch_add(1, Ordering::AcqRel);
        hash_tx
            .send(HashJob {
                root_index,
                name: entry.name.clone(),
                entry_path: entry.entry_path.clone(),
                final_entity: final_entity.clone(),
                identity,
            })
            .map_err(|_| "the entity hash queue stopped".to_owned())?;
    }
    coordinator.publish_progress(slot, false);
    Ok(())
}

fn hash_worker_loop(
    coordinator: Arc<ScanCoordinator>,
    slot: Arc<RunSlot>,
    engine: Arc<EngineShared>,
    writer_tx: std::sync::mpsc::SyncSender<StoreItem>,
    receiver: Arc<Mutex<std::sync::mpsc::Receiver<HashJob>>>,
) {
    // Jobs whose Root is stopped/cancelled are skipped, but every job
    // decrements pending so the Root worker can never hang.
    loop {
        let job = receiver.lock().unwrap_or_else(|p| p.into_inner()).recv();
        let Ok(job) = job else { break };
        let heap_index = engine.heap_of(&job.root_index);
        let Some(heap_index) = heap_index else {
            continue;
        };
        let drop_job = cancelled_or_superseded(&slot)
            || engine.store_wide.load(Ordering::Acquire)
            || engine.root_stop[heap_index].load(Ordering::Acquire);
        if drop_job {
            engine.pending[heap_index].fetch_sub(1, Ordering::AcqRel);
            continue;
        }
        let started = Instant::now();
        let stats = coordinator
            .filesystem
            .scan_tree_statistics(&job.final_entity, &mut |_entry: &TreeScanEntry| Ok(true));
        let (mut file_count, mut byte_count, mut tree_hash, mut hash_fault) = match stats {
            Ok(stats) => (stats.file_count, stats.byte_count, stats.tree_hash, None),
            Err(error) => (0, 0, None, Some(error.to_string())),
        };
        // Identity replacement guard (ADR-0017): the walk-time identity must
        // still be the object that was hashed. A mismatch or unreadable
        // identity discards the tree facts — never a partial fingerprint
        // across two different objects.
        let verified_identity = match (job.identity, tree_hash.as_ref()) {
            (Some(pre), Some(_)) => {
                match coordinator
                    .filesystem
                    .directory_fingerprint(&job.final_entity)
                {
                    Ok(post) if post.device == pre.device && post.inode == pre.inode => Some(pre),
                    _ => {
                        file_count = 0;
                        byte_count = 0;
                        tree_hash = None;
                        hash_fault =
                            Some("final entity identity changed during hashing".to_owned());
                        None
                    }
                }
            }
            _ => None,
        };
        // Canonical entity aggregation (ADR-0017): all appearances that
        // resolve to the same file-system object form one generation-bound
        // entity. Only a successfully hashed object with a readable identity
        // joins an entity; a fault stays an appearance without a guess.
        let newly_assigned = match verified_identity {
            Some(identity) => {
                let key = (identity.device, identity.inode);
                let mut ids = engine.entity_ids.lock().unwrap_or_else(|p| p.into_inner());
                ids.insert(key)
            }
            _ => false,
        };
        {
            let mut progress = slot.progress_lock();
            if let Some(root) = progress
                .roots
                .iter_mut()
                .find(|root| root.index == job.root_index)
            {
                if newly_assigned {
                    root.counts.entities += 1;
                }
                root.counts.files += file_count;
                root.counts.bytes = root.counts.bytes.saturating_add(byte_count);
                root.last_progress = Instant::now();
            }
            if newly_assigned {
                progress.counts.entities += 1;
            }
            progress.counts.files += file_count;
            progress.counts.bytes = progress.counts.bytes.saturating_add(byte_count);
            if progress.phase == ScanPhase::Walking {
                progress.phase = ScanPhase::Hashing;
            }
        }
        let record = ScanEntityRecord {
            seq: engine.entity_seq[heap_index].fetch_add(1, Ordering::Relaxed) as u64,
            name: job.name.clone(),
            entry_path: job.entry_path.clone(),
            final_entity: job.final_entity.clone(),
            identity: verified_identity,
            file_count,
            byte_count,
            tree_hash,
            hash_fault,
            elapsed_ms: started.elapsed().as_millis() as u64,
        };
        if writer_tx
            .send(StoreItem::Entity {
                root_index: job.root_index,
                record,
            })
            .is_err()
        {
            // The writer stopped: mark store-wide so the Run fails closed.
            engine.store_wide.store(true, Ordering::Release);
            engine.pending[heap_index].fetch_sub(1, Ordering::AcqRel);
            continue;
        }
        engine.pending[heap_index].fetch_sub(1, Ordering::AcqRel);
        coordinator.publish_progress(&slot, false);
    }
}

fn writer_loop(
    slot: &RunSlot,
    engine: &EngineShared,
    receiver: std::sync::mpsc::Receiver<StoreItem>,
) {
    let run_id = &slot.record.run_id;
    while let Ok(item) = receiver.recv() {
        if engine.store_wide.load(Ordering::Acquire) {
            // Store-wide: keep draining so producers unblock; every record
            // is discarded (the temporary Run is removed afterwards).
            continue;
        }
        let result = match &item {
            StoreItem::Entry { root_index, record } => {
                slot.store
                    .append_entry(run_id, &root_key(*root_index), record)
            }
            StoreItem::Entity { root_index, record } => {
                slot.store
                    .append_entity(run_id, &root_key(*root_index), record)
            }
            StoreItem::Root { record, .. } => slot.store.write_root(run_id, record),
        };
        if let Err(error) = result {
            classify_store_error(engine, slot, &item, &error);
        }
    }
}

/// Classify a store failure: a store-wide problem fails the whole Run (old
/// Report kept); a root-local write problem fails only that Root with a
/// typed diagnostic and the other Roots continue (spec §4.10).
fn classify_store_error(
    engine: &EngineShared,
    slot: &RunSlot,
    item: &StoreItem,
    error: &ScanEvidenceStoreError,
) {
    let store_wide = match error {
        ScanEvidenceStoreError::Write { operation, .. } => *operation == fault_points::DISK_FULL,
        ScanEvidenceStoreError::Unavailable { .. }
        | ScanEvidenceStoreError::ProvenanceMismatch { .. } => true,
    };
    if store_wide {
        engine.store_wide.store(true, Ordering::Release);
        *engine
            .store_wide_detail
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(error.to_string());
        return;
    }
    let (root_index, heap_index) = match item {
        StoreItem::Entry { root_index, .. }
        | StoreItem::Entity { root_index, .. }
        | StoreItem::Root { root_index, .. } => {
            let root_index = *root_index;
            (
                root_index,
                // An unknown position means the Root never streamed; the
                // failure is store-wide in practice — fail closed.
                engine.heap_of(&root_index).unwrap_or(0),
            )
        }
    };
    fail_root_local(slot, engine, root_index, heap_index, format!("{error}"));
}

fn watchdog_loop(coordinator: Arc<ScanCoordinator>, slot: Arc<RunSlot>, engine: Arc<EngineShared>) {
    loop {
        std::thread::sleep(Duration::from_millis(WATCHDOG_TICK_MS));
        {
            let progress = slot.progress_lock();
            if progress.state.is_terminal() {
                return;
            }
        }
        if slot.shared.done.load(Ordering::Acquire) {
            return;
        }
        if coordinator.frozen_superseded(&slot.record.frozen) {
            set_state_best_effort(
                &slot,
                ScanRunState::Superseded,
                Some("a product write changed frozen generations"),
            );
            slot.shared.cancel.store(true, Ordering::Release);
            coordinator.publish_progress(&slot, true);
            return;
        }
        if slot.shared.cancel.load(Ordering::Acquire) || engine.store_wide.load(Ordering::Acquire) {
            return;
        }
        let mut progress = slot.progress_lock();
        let now = Instant::now();
        if now.duration_since(progress.started).as_millis() as u64 >= RUN_SLOW_MS {
            progress.slow = true;
        }
        let mut isolated: Vec<u32> = Vec::new();
        let mut settled_failed = 0_u64;
        for root in &mut progress.roots {
            if root.state == ScanRootViewState::Pending {
                continue;
            }
            let elapsed = root.started.elapsed().as_millis() as u64;
            if elapsed >= ROOT_SLOW_MS {
                root.slow = true;
            }
            if root.state == ScanRootViewState::Walking
                && now.duration_since(root.last_progress).as_millis() as u64
                    >= coordinator.unresponsive_ms
            {
                root.state = ScanRootViewState::Unresponsive;
                root.diagnostic = Some(format!(
                    "no entry/byte/probe progress for {}ms",
                    coordinator.unresponsive_ms
                ));
                isolated.push(root.index);
            }
            if matches!(
                root.state,
                ScanRootViewState::Unresponsive | ScanRootViewState::Failed
            ) {
                settled_failed += 1;
            }
        }
        progress.counts.failed_roots = settled_failed;
        if !isolated.is_empty() {
            for root_index in &isolated {
                if let Some(heap_index) = engine.heap_of(root_index) {
                    engine.root_stop[heap_index].store(true, Ordering::Release);
                }
            }
            drop(progress);
            coordinator.publish_progress(&slot, true);
        } else {
            drop(progress);
            coordinator.publish_progress(&slot, false);
        }
    }
}

fn finalize(coordinator: Arc<ScanCoordinator>, slot: Arc<RunSlot>, engine: Arc<EngineShared>) {
    {
        let mut progress = slot.progress_lock();
        progress.phase = ScanPhase::Finalizing;
    }
    coordinator.publish_progress(&slot, true);
    // Store-wide failure: the whole Run Failed and the old Report stays.
    if engine.store_wide.load(Ordering::Acquire) {
        let detail = engine
            .store_wide_detail
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
            .unwrap_or_else(|| "the Scan Evidence Store reported a store-wide failure".into());
        set_state_best_effort(&slot, ScanRunState::Failed, Some(&detail));
        let _ = slot.store.remove_run(&slot.record.run_id);
        coordinator.publish_progress(&slot, true);
        finish(coordinator, slot);
        return;
    }
    let current_state = slot.progress_lock().state;
    if current_state == ScanRunState::Superseded {
        let _ = slot.store.remove_run(&slot.record.run_id);
        coordinator.publish_progress(&slot, true);
        finish(coordinator, slot);
        return;
    }
    if slot.shared.cancel.load(Ordering::Acquire) {
        set_state_best_effort(
            &slot,
            ScanRunState::Cancelled,
            Some("cancelled by the user"),
        );
        let _ = slot.store.remove_run(&slot.record.run_id);
        coordinator.publish_progress(&slot, true);
        finish(coordinator, slot);
        return;
    }
    if current_state != ScanRunState::Running {
        // A terminal state arrived through another path; nothing to publish.
        let _ = slot.store.remove_run(&slot.record.run_id);
        coordinator.publish_progress(&slot, true);
        finish(coordinator, slot);
        return;
    }
    // Canonical entity index build (ADR-0017, spec §4.10): only healthy
    // Roots' evidence aggregates into entities/appearances; a failed Root
    // contributes typed diagnostics only. A store-side failure is
    // store-wide — the Run fails and the old Report stays.
    let index_stats = match slot.store.build_entity_index(&slot.record.run_id) {
        Ok(stats) => stats,
        Err(error) => {
            set_state_best_effort(
                &slot,
                ScanRunState::Failed,
                Some(&format!(
                    "the canonical entity index could not be built: {error}"
                )),
            );
            let _ = slot.store.remove_run(&slot.record.run_id);
            coordinator.publish_progress(&slot, true);
            finish(coordinator, slot);
            return;
        }
    };
    // Source classification (spec §8.2, ADR-0017): the pure Core rules over
    // the entity index plus the managed/Catalog facts. Classification is
    // immutable evidence of the Report — any failure (store write, Catalog
    // facts, lock discovery) fails the whole Run and keeps the old Report.
    let classification = match classify_run(&coordinator, &slot) {
        Ok(output) => output,
        Err(detail) => {
            set_state_best_effort(
                &slot,
                ScanRunState::Failed,
                Some(&format!("source classification failed: {detail}")),
            );
            let _ = slot.store.remove_run(&slot.record.run_id);
            coordinator.publish_progress(&slot, true);
            finish(coordinator, slot);
            return;
        }
    };
    let manifest = build_manifest(&coordinator, &slot, index_stats, classification.counts);
    match slot.store.publish_report(&slot.record.run_id, &manifest) {
        Ok(()) => {
            set_state_best_effort(&slot, ScanRunState::Completed, None);
            let terminal = build_run_snapshot(&slot);
            let report = Arc::new(manifest.clone());
            {
                let mut state = coordinator.state_lock();
                state.run = None;
                state.last_run = Some(terminal);
                state.published_report = Some(report);
            }
            coordinator.publish_progress(&slot, true);
            maybe_record_qualification(&coordinator, &slot, &manifest);
            finish(coordinator, slot);
        }
        Err(error) => {
            set_state_best_effort(&slot, ScanRunState::Failed, Some(&format!("{error}")));
            let _ = slot.store.remove_run(&slot.record.run_id);
            coordinator.publish_progress(&slot, true);
            finish(coordinator, slot);
        }
    }
}

/// Manual Complete Report + durable startup marker + restore ledger chain:
/// the unforgeable Safety Snapshot deletion qualification (spec §2.1
/// invariant 6, ADR-0020). A qualification write failure only keeps the
/// Snapshot unqualified — it never revokes the published Report.
fn maybe_record_qualification(
    coordinator: &ScanCoordinator,
    slot: &RunSlot,
    manifest: &crate::seams::scan_evidence_store::ScanReportManifest,
) {
    if manifest.trigger != ScanTrigger::Manual.as_str() || manifest.state != "complete" {
        return;
    }
    let Ok(marker) = slot.store.startup_marker() else {
        return;
    };
    let Some(marker) = marker else { return };
    if marker.home_id != manifest.home_id {
        return;
    }
    let Ok(ledger) = coordinator.app_state.load() else {
        return;
    };
    let restore_records = ledger
        .recovery_ledger
        .completed
        .iter()
        .filter(|record| {
            record.home_id.as_ref().map(|id| id.0.clone()) == Some(manifest.home_id.clone())
                && record.snapshot_path.is_some()
        })
        .collect::<Vec<_>>();
    if restore_records.is_empty() {
        return;
    }
    let newest_restore_ms = restore_records
        .iter()
        .filter_map(|record| super::rfc3339_to_epoch_millis(&record.created_at))
        .max()
        .unwrap_or(0);
    if marker.marked_at_ms < newest_restore_ms {
        return;
    }
    let snapshot_ids = restore_records
        .iter()
        .filter_map(|record| {
            record
                .snapshot_path
                .as_ref()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned())
        })
        .collect::<Vec<_>>();
    let mut qualification = ScanSnapshotQualification {
        schema_version: SCAN_STORE_SCHEMA_VERSION,
        home_id: manifest.home_id.clone(),
        snapshot_ids,
        startup_marked_at_ms: marker.marked_at_ms,
        report_run_id: manifest.run_id.clone(),
        report_generation: manifest.generation,
        report_content_identity: manifest.content_identity.clone(),
        recorded_at_ms: (coordinator.clock.unix_epoch_nanos() / 1_000_000) as u64,
        integrity: None,
    };
    let mut payload = qualification.clone();
    payload.integrity = None;
    qualification.integrity = Some(crate::seams::scan_integrity::canonical_json_digest(
        &serde_json::to_value(&payload).expect("qualification serializes"),
    ));
    let _ = slot.store.write_qualification(&qualification);
}

fn build_manifest(
    coordinator: &ScanCoordinator,
    slot: &RunSlot,
    index_stats: ScanEntityIndexStats,
    source_counts: crate::seams::scan_evidence_store::ScanSourceCounts,
) -> crate::seams::scan_evidence_store::ScanReportManifest {
    let progress = slot.progress_lock();
    let failed_roots = progress
        .roots
        .iter()
        .filter(|root| root.state != ScanRootViewState::Completed)
        .count() as u64;
    let roots = progress
        .roots
        .iter()
        .map(|root| ScanRootCoverageRecord {
            index: root.index,
            configured_path: root.configured_path.clone(),
            canonical_path: root.canonical_path.clone(),
            state: match root.state {
                ScanRootViewState::Completed => ScanRootState::Completed,
                ScanRootViewState::Unresponsive => ScanRootState::Unresponsive,
                _ => ScanRootState::Failed,
            },
            consumer_agents: root.consumer_agents.clone(),
            counts: root.counts,
            elapsed_ms: root.started.elapsed().as_millis() as u64,
            slow: root.slow,
            diagnostic: root.diagnostic.clone(),
        })
        .collect::<Vec<_>>();
    let mut counts = progress.counts;
    counts.failed_roots = failed_roots;
    // The Report-level funnel counts aggregate the healthy Roots' evidence
    // only (spec §8.1: a partial report claims only the healthy Roots'
    // known appearances — a failed Root's streamed entries never count).
    counts.entries = index_stats.appearances;
    counts.entities = index_stats.entities;
    let state = if failed_roots == 0 {
        "complete"
    } else {
        "incomplete"
    };
    let ended_at_ms = (coordinator.clock.unix_epoch_nanos() / 1_000_000) as u64;
    let mut manifest = crate::seams::scan_evidence_store::ScanReportManifest {
        schema_version: SCAN_STORE_SCHEMA_VERSION,
        home_id: slot.record.home_id.clone(),
        run_id: slot.record.run_id.clone(),
        generation: slot.record.generation,
        trigger: slot.record.trigger.clone(),
        state: state.to_owned(),
        counts,
        source_counts,
        roots,
        frozen: slot.record.frozen.clone(),
        started_at_ms: slot.record.started_at_ms,
        ended_at_ms,
        integrity: None,
        content_identity: format!(
            "scan-report-v1:{}:{}:{}:{}",
            slot.record.home_id, slot.record.run_id, slot.record.generation, state
        ),
    };
    manifest = seal_manifest(manifest);
    manifest
}

fn build_root_record(
    slot: &RunSlot,
    root_index: u32,
    state: ScanRootState,
    diagnostic: Option<String>,
) -> ScanRootRecord {
    let progress = slot.progress_lock();
    let root = progress
        .roots
        .iter()
        .find(|root| root.index == root_index)
        .expect("walked Root is in progress");
    ScanRootRecord {
        schema_version: SCAN_STORE_SCHEMA_VERSION,
        home_id: slot.record.home_id.clone(),
        run_id: slot.record.run_id.clone(),
        index: root.index,
        configured_path: root.configured_path.clone(),
        canonical_path: root.canonical_path.clone(),
        state,
        counts: root.counts,
        elapsed_ms: root.started.elapsed().as_millis() as u64,
        slow: root.slow,
        diagnostic,
        completed_at_ms: slot.record.started_at_ms,
    }
}

/// The source classification pass of a Run (spec §8.2): reads the entity
/// rows from the Evidence Store, assembles the Catalog/lock facts through
/// the Core seams (fail closed on any read error), runs the pure
/// classifier and persists the immutable classification index. The caller
/// treats every error here as store-wide: no classification, no Report.
fn classify_run(
    coordinator: &ScanCoordinator,
    slot: &RunSlot,
) -> Result<crate::core::scan::classification::ScanClassificationOutput, String> {
    use crate::core::scan::classification::ScanClassificationContext;

    let gate = coordinator.write_gate.snapshot();
    let bound = match &gate.state {
        crate::core::write_gate::WriteGateState::Open(home) => home.clone(),
        other => {
            return Err(format!(
                "the Bound Home is not open ({})",
                super::write_gate_state_summary(other)
            ));
        }
    };
    let rows = slot
        .store
        .classification_rows(&slot.record.run_id)
        .map_err(|error| error.to_string())?;
    let managed = coordinator
        .managed_facts
        .read()
        .map_err(|error| error.to_string())?;
    let lock_reports = coordinator
        .lock_store
        .discover()
        .map_err(|error| error.to_string())?;
    let progress = slot.progress_lock();
    let report_complete = progress.counts.failed_roots == 0;
    drop(progress);

    let mut control_zones = Vec::new();
    control_zones.push(bound.path.clone());
    control_zones.extend(
        slot.record
            .roots
            .iter()
            .map(|root| root.canonical_path.clone()),
    );
    control_zones.push(coordinator.app_state_path.clone());
    let faulted_lock_roots = lock_reports
        .iter()
        .filter_map(|report| {
            report.fault.as_ref().map(|fault| {
                (
                    report.path.parent().map(std::path::Path::to_path_buf),
                    fault,
                )
            })
        })
        .filter_map(|(root, fault)| root.map(|root| (root, format!("{fault:?}"))))
        .collect::<Vec<_>>();
    for report in &lock_reports {
        if let Some(root) = report.path.parent() {
            control_zones.push(root.to_path_buf());
        }
    }
    let context = ScanClassificationContext {
        control_zones,
        home_skills_path: Some(bound.path.join("skills")),
        managed_entity_paths: managed
            .iter()
            .map(|fact| fact.final_entity_path.clone())
            .collect(),
        managed_directory_names: managed
            .iter()
            .map(|fact| fact.directory_name.clone())
            .collect(),
        faulted_lock_roots,
        report_complete,
    };
    let output = crate::core::scan::classification::classify(rows, &context);
    slot.store
        .write_classification(
            &slot.record.run_id,
            &output.verdicts,
            &output.git_groups,
            &output.conflict_sets,
        )
        .map_err(|error| error.to_string())?;
    Ok(output)
}

fn fail_root_local(
    slot: &RunSlot,
    engine: &EngineShared,
    root_index: u32,
    heap_index: usize,
    diagnostic: String,
) {
    engine.root_stop[heap_index].store(true, Ordering::Release);
    let mut progress = slot.progress_lock();
    let root = progress
        .roots
        .iter_mut()
        .find(|root| root.index == root_index)
        .expect("walked Root is in progress");
    if root.state != ScanRootViewState::Completed && root.state != ScanRootViewState::Unresponsive {
        root.state = ScanRootViewState::Failed;
        root.diagnostic = Some(diagnostic);
    }
    progress.counts.failed_roots = progress
        .roots
        .iter()
        .filter(|root| {
            matches!(
                root.state,
                ScanRootViewState::Failed | ScanRootViewState::Unresponsive
            )
        })
        .count() as u64;
}

impl EngineShared {
    fn heap_of(&self, root_index: &u32) -> Option<usize> {
        self.positions.get(root_index).copied()
    }
}

fn chain_to_record(
    chain: &EvidenceChain,
) -> (Vec<ScanChainHopRecord>, Option<ScanChainFaultRecord>) {
    let hops = chain
        .hops
        .iter()
        .map(|hop| ScanChainHopRecord {
            path: hop.path.clone(),
            kind: match &hop.kind {
                EvidenceChainHopKind::Directory => "directory".into(),
                EvidenceChainHopKind::Symlink { target } => {
                    format!("symlink:{}", target.to_string_lossy())
                }
            },
            device: hop.device,
            inode: hop.inode,
            target: match &hop.kind {
                EvidenceChainHopKind::Symlink { target } => Some(target.clone()),
                EvidenceChainHopKind::Directory => None,
            },
        })
        .collect();
    let fault = chain.fault.as_ref().map(|fault| ScanChainFaultRecord {
        kind: chain_fault_kind(fault).into(),
        at: fault_position(fault),
        detail: match fault {
            ChainFault::ReadFailed { detail, .. } => Some(detail.clone()),
            _ => None,
        },
    });
    (hops, fault)
}

fn chain_fault_kind(fault: &ChainFault) -> &'static str {
    match fault {
        ChainFault::Dangling { .. } => "dangling",
        ChainFault::Cycle { .. } => "cycle",
        ChainFault::HopLimit { .. } => "hop_limit",
        ChainFault::NonUtf8 { .. } => "non_utf8",
        ChainFault::ReadFailed { .. } => "read_failed",
        ChainFault::NotDirectory { .. } => "not_directory",
        ChainFault::IdentityReplaced { .. } => "identity_replaced",
    }
}

fn fault_position(fault: &ChainFault) -> std::path::PathBuf {
    match fault {
        ChainFault::Dangling { at }
        | ChainFault::Cycle { at }
        | ChainFault::HopLimit { at }
        | ChainFault::NonUtf8 { at }
        | ChainFault::ReadFailed { at, .. }
        | ChainFault::NotDirectory { at }
        | ChainFault::IdentityReplaced { at } => at.clone(),
    }
}

fn lock_hint_for(
    coordinator: &ScanCoordinator,
    engine: &EngineShared,
    entry_name: &str,
) -> Option<ScanLockHintRecord> {
    let mut report = engine.lock_report.lock().unwrap_or_else(|p| p.into_inner());
    if report.is_none() {
        *report = coordinator.lock_store.discover().ok();
    }
    let report = report.as_ref()?;
    for lock in report {
        if let Some(entry) = lock.entries.iter().find(|entry| entry.name == entry_name) {
            return Some(ScanLockHintRecord {
                lock_path: lock.path.clone(),
                entry_name: entry.name.clone(),
                fingerprint: lock.fingerprint.clone(),
                faulted: false,
                fault: None,
                source_type: Some(entry.source_type.clone()),
                source_url: Some(entry.source_url.clone()),
                requested_ref: entry.requested_ref.clone(),
                skill_path: Some(entry.skill_path.clone()),
            });
        }
        if let Some(fault) = lock
            .entry_faults
            .iter()
            .find(|fault| fault.name == entry_name)
        {
            return Some(ScanLockHintRecord {
                lock_path: lock.path.clone(),
                entry_name: fault.name.clone(),
                fingerprint: lock.fingerprint.clone(),
                faulted: true,
                fault: Some(fault.reason.clone()),
                source_type: None,
                source_url: None,
                requested_ref: None,
                skill_path: None,
            });
        }
    }
    None
}

fn worktree_hint_for(
    coordinator: &ScanCoordinator,
    engine: &EngineShared,
    root_index: u32,
    canonical: &std::path::Path,
    entry_path: &std::path::Path,
) -> Result<Option<ScanWorktreeHintRecord>, String> {
    let key = (entry_path.to_path_buf(), root_index);
    if let Some(cached) = engine
        .worktree_memo
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&key)
        .cloned()
    {
        return Ok(cached.as_ref().map(worktree_hint_record));
    }
    engine.git_probe.acquire();
    let result = coordinator.git_probe.probe_worktree(entry_path, canonical);
    engine.git_probe.release();
    let hint_memo = match result {
        Ok(Some(hint)) => Some(WorktreeHintMemo {
            repository_root: hint.repository_root.clone(),
            gitdir_kind: hint.gitdir_kind_name().to_owned(),
            remote_urls: hint
                .remote_urls
                .iter()
                .map(|remote| remote.url.clone())
                .collect(),
            head_ref: hint.head_ref.clone(),
        }),
        Ok(None) => None,
        Err(_) => None,
    };
    engine
        .worktree_memo
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key, hint_memo.clone());
    Ok(hint_memo.as_ref().map(worktree_hint_record))
}

fn worktree_hint_record(hint: &WorktreeHintMemo) -> ScanWorktreeHintRecord {
    ScanWorktreeHintRecord {
        repository_root: hint.repository_root.clone(),
        gitdir_kind: hint.gitdir_kind.clone(),
        remote_urls: hint.remote_urls.clone(),
        head_ref: hint.head_ref.clone(),
    }
}

fn set_state_best_effort(slot: &RunSlot, state: ScanRunState, diagnostic: Option<&str>) {
    let mut progress = slot.progress_lock();
    if !progress.state.is_terminal() {
        progress.state = state;
        if let Some(diagnostic) = diagnostic {
            progress.diagnostic = Some(diagnostic.to_owned());
        }
    }
}

fn cancelled_or_superseded(slot: &RunSlot) -> bool {
    slot.shared.cancel.load(Ordering::Acquire)
}

fn root_key(index: u32) -> String {
    format!("r{index}")
}

fn finish(coordinator: Arc<ScanCoordinator>, slot: Arc<RunSlot>) {
    let terminal = build_run_snapshot(&slot);
    {
        let mut state = coordinator.state_lock();
        state.run = None;
        state.last_run = Some(terminal);
    }
    coordinator.publish_progress(&slot, true);
}
