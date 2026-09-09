use super::*;
use crate::import_integrity_phase2 as phase2;
use rusqlite::params;
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_TEST_DIRECTORY: AtomicU64 = AtomicU64::new(1);

const SYNTHETIC_PST_BYTES: &[u8] = b"!BDNphase-two-synthetic-pst-records";
const NORMAL_EML: &[u8] = b"From: Ada <ada@example.test>\r\n\
To: Grace <grace@example.test>\r\n\
Date: Thu, 01 Jan 2026 12:00:00 +0000\r\n\
Message-ID: <phase-two-1@example.test>\r\n\
Subject: Fictional status\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
Synthetic body.\r\n";
const SECOND_EML: &[u8] = b"From: Grace <grace@example.test>\r\n\
To: Ada <ada@example.test>\r\n\
Date: Thu, 01 Jan 2026 12:01:00 +0000\r\n\
Message-ID: <phase-two-2@example.test>\r\n\
In-Reply-To: <phase-two-1@example.test>\r\n\
References: <phase-two-1@example.test>\r\n\
Subject: Re: Fictional status\r\n\
Content-Type: text/plain; charset=utf-8\r\n\
\r\n\
Synthetic reply.\r\n";
const NO_SUBJECT_EMPTY_BODY_EML: &[u8] = b"From: Blank <blank@example.test>\r\n\
To: Nobody <nobody@example.test>\r\n\
Date: Thu, 01 Jan 2026 12:02:00 +0000\r\n\
Message-ID: <phase-two-blank@example.test>\r\n\
\r\n";

struct TestDirectory {
    path: PathBuf,
}

impl TestDirectory {
    fn new(label: &str) -> Self {
        let sequence = NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = env::temp_dir().join(format!(
            "pst-quickview-phase2-{label}-{}-{sequence}-{nonce}",
            std::process::id()
        ));
        create_private_directory_all(&path).expect("synthetic test root should be created");
        Self { path }
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct SyntheticWorkspace {
    _root: TestDirectory,
    active: ActiveWorkspace,
    identity: PstIdentity,
    discovery: EmlDiscovery,
    stats: IndexStats,
    source_hash: String,
}

impl SyntheticWorkspace {
    fn valid(label: &str) -> Self {
        Self::with_records(
            label,
            &[
                ("Inbox/one.eml", NORMAL_EML),
                ("Inbox/two.eml", SECOND_EML),
                ("Archive/no-subject.eml", NO_SUBJECT_EMPTY_BODY_EML),
            ],
        )
    }

    fn with_duplicates(label: &str) -> Self {
        Self::with_records(
            label,
            &[
                ("Inbox/original.eml", NORMAL_EML),
                ("Archive/legitimate-copy.eml", NORMAL_EML),
                ("Archive/no-subject.eml", NO_SUBJECT_EMPTY_BODY_EML),
            ],
        )
    }

    fn with_records(label: &str, records: &[(&str, &[u8])]) -> Self {
        Self::with_prepared_records(label, records, |_| {})
    }

    fn with_prepared_records<F>(label: &str, records: &[(&str, &[u8])], prepare: F) -> Self
    where
        F: FnOnce(&Path),
    {
        let root = TestDirectory::new(label);
        let source = root.path.join("fictional.pst");
        fs::write(&source, SYNTHETIC_PST_BYTES).expect("synthetic source should be written");
        let identity = identify_pst(&source).expect("synthetic PST identity should be created");
        let workspace = root.path.join(&identity.workspace_id);
        create_private_directory_all(&workspace).expect("synthetic workspace should be created");
        let extracted = workspace.join("extracted");
        create_private_directory_all(&extracted).expect("extraction root should be created");

        for (relative, bytes) in records {
            let path = extracted.join(relative);
            create_private_directory_all(path.parent().expect("record should have a parent"))
                .expect("record parent should be created");
            fs::write(path, bytes).expect("synthetic EML should be written");
        }
        prepare(&extracted);

        let marker = WorkspaceMarker {
            app: WORKSPACE_MARKER_APP_NAME.to_string(),
            workspace_id: identity.workspace_id.clone(),
            pst_content_fingerprint: identity.content_fingerprint.clone(),
            workspace_location_mode: WorkspaceLocationMode::AppSupport.as_str().to_string(),
            original_pst_path: identity.display_path.clone(),
        };
        write_workspace_metadata_atomically(
            &workspace.join(WORKSPACE_MARKER_FILE),
            &serde_json::to_value(marker).expect("marker should serialize"),
        )
        .expect("marker should be persisted");
        initialize_import_log(&workspace, "synthetic-readpst-0")
            .expect("synthetic import log should initialize");

        let counts = phase2::StateCounts::default();
        for (stage, reason) in [
            (phase2::WorkspaceState::Preflight, "preflight_ready"),
            (phase2::WorkspaceState::Extracting, "extraction_running"),
            (
                phase2::WorkspaceState::ExtractionComplete,
                "extraction_complete",
            ),
            (phase2::WorkspaceState::Discovering, "discovery_running"),
        ] {
            phase2::transition_state(
                &workspace,
                &identity.workspace_id,
                stage,
                counts,
                Some(reason),
            )
            .expect("pre-index state should persist");
        }

        let discovery =
            discover_eml_candidates(&extracted).expect("synthetic extraction should be discovered");
        let now = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        phase2::write_manifest_for_discovery(
            &workspace,
            &identity,
            &identity.workspace_id,
            "synthetic-readpst-0",
            &now,
            &now,
            "success",
            &discovery,
            None,
        )
        .expect("manifest should persist");
        phase2::transition_state(
            &workspace,
            &identity.workspace_id,
            phase2::WorkspaceState::Indexing,
            phase2::StateCounts {
                discovered_candidates: Some(discovery.count),
                canonical_indexed: Some(0),
            },
            Some("indexing_running"),
        )
        .expect("indexing should persist");

        let index_path = workspace.join("index.sqlite");
        let mut conn = open_sqlite_write_connection(&index_path)
            .expect("synthetic index should open for writing");
        initialize_schema(&conn).expect("current schema should initialize");
        insert_import_metadata(
            &mut conn,
            &identity,
            &identity.workspace_id,
            discovery.count,
            &discovery.identity_manifest,
            &workspace,
            WorkspaceLocationMode::AppSupport,
            "bundled-readpst",
            "synthetic-readpst-0",
        )
        .expect("import metadata should initialize");
        let stats = stream_index_eml_files_with_control(
            &workspace,
            &extracted,
            &mut conn,
            &discovery,
            2,
            "Synthetic cancellation.",
            |_| false,
            |_| {},
        )
        .expect("synthetic candidates should index");
        phase2::transition_state(
            &workspace,
            &identity.workspace_id,
            phase2::WorkspaceState::BuildingConversations,
            phase2::StateCounts {
                discovered_candidates: Some(stats.discovered),
                canonical_indexed: Some(stats.indexed),
            },
            Some("building_conversations"),
        )
        .expect("conversation state should persist");
        apply_conversation_index(&mut conn, &identity.workspace_id, || false)
            .expect("conversation metadata should build");
        finalize_import_index_metadata(&mut conn, &stats).expect("final counters should persist");
        optimize_then_checked_truncate(&conn).expect("synthetic index should checkpoint");
        drop(conn);

        phase2::update_manifest_from_index(
            &workspace,
            &identity.workspace_id,
            &index_path,
            &stats,
            "complete",
        )
        .expect("manifest index aggregates should persist");

        let active = ActiveWorkspace {
            id: identity.workspace_id.clone(),
            path: workspace,
            pst_path: identity.canonical_path.clone(),
            fingerprint: identity.content_fingerprint.clone(),
            location_mode: WorkspaceLocationMode::AppSupport,
        };
        let source_hash = sha256_file(&active.pst_path).expect("source hash should be readable");
        Self {
            _root: root,
            active,
            identity,
            discovery,
            stats,
            source_hash,
        }
    }

    fn manifest(&self) -> phase2::ImportManifest {
        phase2::read_manifest(&self.active.path, &self.active.id)
            .expect("manifest should be readable")
            .expect("manifest should be present")
    }

    fn index_path(&self) -> PathBuf {
        self.active.path.join("index.sqlite")
    }

    fn source_unchanged(&self) {
        let metadata =
            fs::metadata(&self.active.pst_path).expect("source metadata should remain readable");
        assert_eq!(
            sha256_file(&self.active.pst_path).expect("source hash should remain readable"),
            self.source_hash,
            "verification and indexing must not alter source bytes"
        );
        assert_eq!(
            self.identity.size,
            metadata.len(),
            "verification and indexing must not alter source size"
        );
        let modified_ns = metadata
            .modified()
            .expect("source modification time should remain readable")
            .duration_since(UNIX_EPOCH)
            .expect("synthetic source time should follow the Unix epoch")
            .as_nanos();
        assert_eq!(
            self.identity.modified_ns, modified_ns,
            "verification and indexing must not alter source modification time"
        );
    }

    fn source_and_extraction_unchanged(&self) {
        self.source_unchanged();
        let current = discover_eml_candidates(&self.active.path.join("extracted"))
            .expect("synthetic extraction should remain discoverable");
        assert_eq!(current.count, self.discovery.count);
        assert_eq!(current.identity_manifest, self.discovery.identity_manifest);
    }
}

fn raw_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).expect("synthetic JSON should be readable"))
        .expect("synthetic JSON should parse")
}

fn write_raw_json(path: &Path, value: &Value) {
    fs::write(
        path,
        serde_json::to_vec_pretty(value).expect("synthetic JSON should serialize"),
    )
    .expect("synthetic JSON should be written");
}

fn rewrite_as_existing_legacy_incomplete_without_lineage(
    state_path: &Path,
    canonical_elapsed_ms: u64,
) -> Value {
    let mut value = raw_json(state_path);
    let object = value.as_object_mut().unwrap();
    object.insert("state".to_string(), serde_json::json!("incomplete"));
    object.insert("lastCompletedStage".to_string(), Value::Null);
    object.insert("interruptedStage".to_string(), Value::Null);
    object.remove("verifiedCompleteLineage");
    object.insert(
        "elapsedDurationMs".to_string(),
        serde_json::json!(canonical_elapsed_ms),
    );
    object.insert("activeTimingVersion".to_string(), serde_json::json!(1));
    object.insert(
        "activeTimingHistoryComplete".to_string(),
        serde_json::json!(true),
    );
    object.insert("activeSessionStartedAt".to_string(), Value::Null);
    object.insert(
        "activeSessionUpdatesImportTiming".to_string(),
        serde_json::json!(false),
    );
    object.insert(
        "accumulatedActiveElapsedMs".to_string(),
        serde_json::json!(canonical_elapsed_ms),
    );
    object.insert("stageElapsedDurationMs".to_string(), serde_json::json!(0));
    object.insert("lastActivityAt".to_string(), Value::Null);
    object.insert("completedDurationMs".to_string(), Value::Null);
    object.insert("pendingCompletionDurationMs".to_string(), Value::Null);
    object.insert("operationTiming".to_string(), Value::Null);
    object.insert(
        "reason".to_string(),
        serde_json::json!("verification_required"),
    );
    write_raw_json(state_path, &value);
    value
}

fn persist_synthetic_completed_timing(
    workspace: &Path,
    workspace_id: &str,
    duration_ms: u64,
) -> phase2::WorkspaceImportState {
    phase2::transition_state(
        workspace,
        workspace_id,
        phase2::WorkspaceState::Preflight,
        phase2::StateCounts::default(),
        Some("preflight_running"),
    )
    .unwrap();
    let session =
        phase2::begin_active_session(workspace, workspace_id, true, false, None, None).unwrap();
    for stage in [
        phase2::WorkspaceState::Extracting,
        phase2::WorkspaceState::ExtractionComplete,
        phase2::WorkspaceState::Discovering,
        phase2::WorkspaceState::Indexing,
        phase2::WorkspaceState::BuildingConversations,
        phase2::WorkspaceState::Verifying,
    ] {
        phase2::transition_state(
            workspace,
            workspace_id,
            stage,
            phase2::StateCounts::default(),
            Some("synthetic_transition"),
        )
        .unwrap();
    }
    phase2::checkpoint_active_session(
        workspace,
        workspace_id,
        session.operation_generation,
        phase2::WorkspaceState::Verifying,
        duration_ms,
        duration_ms,
        true,
        false,
        true,
    )
    .unwrap();
    phase2::transition_state(
        workspace,
        workspace_id,
        phase2::WorkspaceState::Complete,
        phase2::StateCounts::default(),
        Some("verification_complete"),
    )
    .unwrap();
    phase2::checkpoint_active_session(
        workspace,
        workspace_id,
        session.operation_generation,
        phase2::WorkspaceState::Complete,
        duration_ms,
        0,
        true,
        true,
        true,
    )
    .unwrap()
}

fn sqlite_companion(path: &Path, suffix: &str) -> PathBuf {
    let file_name = path.file_name().unwrap().to_string_lossy();
    path.parent().unwrap().join(format!("{file_name}{suffix}"))
}

fn create_committed_wal_crash_layout(source: &Path, destination: &Path) {
    let scratch = destination.with_extension("wal-source.sqlite");
    fs::copy(source, &scratch).unwrap();
    let conn = open_sqlite_write_connection(&scratch).unwrap();
    conn.execute_batch("PRAGMA wal_autocheckpoint = 0;")
        .unwrap();
    set_metadata_value(&conn, "import_status", "running").unwrap();
    set_metadata_value(&conn, "synthetic_hard_crash_probe", "committed").unwrap();
    let scratch_wal = sqlite_companion(&scratch, "-wal");
    assert!(fs::metadata(&scratch_wal).is_ok_and(|metadata| metadata.len() > 0));
    fs::copy(&scratch, destination).unwrap();
    for suffix in ["-wal", "-shm"] {
        let source_sidecar = sqlite_companion(&scratch, suffix);
        if source_sidecar.exists() {
            fs::copy(source_sidecar, sqlite_companion(destination, suffix)).unwrap();
        }
    }
    drop(conn);
    let _ = fs::remove_file(&scratch);
    for suffix in ["-wal", "-shm", "-journal"] {
        let _ = fs::remove_file(sqlite_companion(&scratch, suffix));
    }
}

fn verify_for_test(
    fixture: &SyntheticWorkspace,
    operation_id: &str,
    level: phase2::VerificationLevel,
) -> phase2::VerificationReport {
    phase2::verify_active_workspace_for_test(&fixture.active, operation_id, level)
        .expect("synthetic verification should return a structured report")
}

#[test]
fn workspace_state_transitions_persist_monotonic_revisions_and_counts() {
    let root = TestDirectory::new("state-transitions");
    let workspace_id = "1".repeat(64);
    let workspace = root.path.join(&workspace_id);
    create_private_directory_all(&workspace).unwrap();
    let stages = [
        phase2::WorkspaceState::Preflight,
        phase2::WorkspaceState::Extracting,
        phase2::WorkspaceState::ExtractionComplete,
        phase2::WorkspaceState::Discovering,
        phase2::WorkspaceState::Indexing,
        phase2::WorkspaceState::BuildingConversations,
        phase2::WorkspaceState::Verifying,
        phase2::WorkspaceState::Complete,
    ];

    for (index, stage) in stages.into_iter().enumerate() {
        let record = phase2::transition_state(
            &workspace,
            &workspace_id,
            stage,
            phase2::StateCounts {
                discovered_candidates: Some(12),
                canonical_indexed: Some(index.min(12)),
            },
            Some("synthetic_transition"),
        )
        .expect("valid state transition should persist");
        assert_eq!(record.revision, index as u64 + 1);
        assert_eq!(record.state, stage);
        assert_eq!(record.discovered_candidates, 12);
        assert_eq!(record.canonical_indexed, index.min(12) as u64);
        assert_eq!(record.remaining_candidates, Some(12 - index.min(12) as u64));
        assert!(DateTime::parse_from_rfc3339(&record.updated_at).is_ok());
        assert!(record.updated_at.ends_with('Z'));
    }

    let persisted = phase2::read_state(&workspace, &workspace_id)
        .unwrap()
        .expect("state should remain present");
    assert_eq!(persisted.state, phase2::WorkspaceState::Complete);
    assert_eq!(
        persisted.last_completed_stage,
        Some(phase2::WorkspaceState::Verifying)
    );
}

#[test]
fn active_session_timing_persists_resume_time_without_counting_downtime() {
    let root = TestDirectory::new("active-session-timing");
    let workspace_id = "b".repeat(64);
    let workspace = root.path.join(&workspace_id);
    create_private_directory_all(&workspace).unwrap();
    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Preflight,
        phase2::StateCounts::default(),
        Some("preflight_running"),
    )
    .unwrap();

    let first_session =
        phase2::begin_active_session(&workspace, &workspace_id, true, false, None, None).unwrap();
    assert_eq!(first_session.operation_generation, 1);
    assert_eq!(
        first_session.active_timing_version,
        phase2::ACTIVE_TIMING_VERSION
    );
    assert!(first_session.active_timing_history_complete);
    assert!(first_session.active_session_started_at.is_some());
    assert!(first_session.last_activity_at.is_some());

    let checkpoint = phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        first_session.operation_generation,
        phase2::WorkspaceState::Preflight,
        1_200,
        1_200,
        true,
        false,
        false,
    )
    .unwrap();
    assert_eq!(checkpoint.elapsed_duration_ms, 1_200);
    assert_eq!(checkpoint.stage_elapsed_duration_ms, 1_200);

    let extracting = phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Extracting,
        phase2::StateCounts::default(),
        Some("extraction_running"),
    )
    .unwrap();
    assert_eq!(extracting.elapsed_duration_ms, 1_200);
    assert_eq!(extracting.stage_elapsed_duration_ms, 0);
    assert_eq!(extracting.operation_generation, 1);

    phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        1,
        phase2::WorkspaceState::Extracting,
        2_500,
        1_300,
        false,
        false,
        false,
    )
    .unwrap();
    let cancelled = phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Cancelled,
        phase2::StateCounts::default(),
        Some("import_cancelled"),
    )
    .unwrap();
    assert_eq!(cancelled.stage_elapsed_duration_ms, 1_300);
    let paused = phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        1,
        phase2::WorkspaceState::Cancelled,
        3_000,
        1_800,
        true,
        true,
        false,
    )
    .unwrap();
    assert_eq!(paused.elapsed_duration_ms, 3_000);
    assert_eq!(paused.stage_elapsed_duration_ms, 1_800);
    assert!(paused.active_session_started_at.is_none());

    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Preflight,
        phase2::StateCounts::default(),
        Some("preflight_running"),
    )
    .unwrap();
    let resumed =
        phase2::begin_active_session(&workspace, &workspace_id, true, true, None, None).unwrap();
    assert_eq!(resumed.operation_generation, 2);
    assert!(resumed.active_timing_history_complete);
    assert_eq!(resumed.elapsed_duration_ms, 3_000);
    let resumed_checkpoint = phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        2,
        phase2::WorkspaceState::Preflight,
        3_800,
        800,
        true,
        false,
        false,
    )
    .unwrap();
    assert_eq!(resumed_checkpoint.elapsed_duration_ms, 3_800);
    assert_eq!(resumed_checkpoint.stage_elapsed_duration_ms, 800);
}

#[test]
fn a_new_operation_stage_resets_persisted_stage_time_before_its_first_checkpoint() {
    let root = TestDirectory::new("active-session-stage-reset");
    let workspace_id = "f".repeat(64);
    let workspace = root.path.join(&workspace_id);
    create_private_directory_all(&workspace).unwrap();
    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Preflight,
        phase2::StateCounts::default(),
        Some("preflight_running"),
    )
    .unwrap();
    let first =
        phase2::begin_active_session(&workspace, &workspace_id, true, false, None, None).unwrap();
    phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        first.operation_generation,
        phase2::WorkspaceState::Preflight,
        60_000,
        60_000,
        true,
        true,
        false,
    )
    .unwrap();
    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Incomplete,
        phase2::StateCounts::default(),
        Some("interrupted_process"),
    )
    .unwrap();

    let restarted =
        phase2::begin_active_session(&workspace, &workspace_id, true, true, None, None).unwrap();
    assert_eq!(restarted.stage_elapsed_duration_ms, 0);
    let checkpoint = phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        restarted.operation_generation,
        phase2::WorkspaceState::Incomplete,
        75_000,
        15_000,
        true,
        false,
        false,
    )
    .expect("a long prerequisite scan must not regress the previous stage timer");
    assert_eq!(checkpoint.elapsed_duration_ms, 75_000);
    assert_eq!(checkpoint.stage_elapsed_duration_ms, 15_000);
}

#[test]
fn completed_import_duration_is_recorded_once_and_survives_later_verification_time() {
    let root = TestDirectory::new("completed-import-duration");
    let workspace_id = "d".repeat(64);
    let workspace = root.path.join(&workspace_id);
    create_private_directory_all(&workspace).unwrap();
    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Preflight,
        phase2::StateCounts::default(),
        Some("preflight_running"),
    )
    .unwrap();
    let session =
        phase2::begin_active_session(&workspace, &workspace_id, true, false, None, None).unwrap();
    for stage in [
        phase2::WorkspaceState::Extracting,
        phase2::WorkspaceState::ExtractionComplete,
        phase2::WorkspaceState::Discovering,
        phase2::WorkspaceState::Indexing,
        phase2::WorkspaceState::BuildingConversations,
        phase2::WorkspaceState::Verifying,
    ] {
        phase2::transition_state(
            &workspace,
            &workspace_id,
            stage,
            phase2::StateCounts::default(),
            Some("synthetic_transition"),
        )
        .unwrap();
    }
    let before_finish = phase2::read_state(&workspace, &workspace_id)
        .unwrap()
        .unwrap();
    assert!(before_finish.completed_duration_ms.is_none());

    phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        session.operation_generation,
        phase2::WorkspaceState::Verifying,
        18_032,
        18_032,
        true,
        false,
        true,
    )
    .unwrap();
    let durable_complete = phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Complete,
        phase2::StateCounts::default(),
        Some("verification_complete"),
    )
    .unwrap();
    assert_eq!(durable_complete.completed_duration_ms, Some(18_032));
    let completed = phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        session.operation_generation,
        phase2::WorkspaceState::Complete,
        18_100,
        0,
        true,
        true,
        true,
    )
    .unwrap();
    assert_eq!(completed.completed_duration_ms, Some(18_032));

    let verification = phase2::begin_active_session(
        &workspace,
        &workspace_id,
        false,
        false,
        Some(phase2::OperationTimingKind::Verification),
        Some(phase2::WorkspaceState::Verifying),
    )
    .unwrap();
    let verifying = phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Verifying,
        phase2::StateCounts::default(),
        Some("verification_running"),
    )
    .unwrap();
    assert_eq!(verifying.elapsed_duration_ms, 18_032);
    assert_eq!(verifying.stage_elapsed_duration_ms, 0);
    phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        verification.operation_generation,
        phase2::WorkspaceState::Verifying,
        1_968,
        1_968,
        true,
        false,
        false,
    )
    .unwrap();
    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Complete,
        phase2::StateCounts::default(),
        Some("verification_complete"),
    )
    .unwrap();
    let after_verification = phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        verification.operation_generation,
        phase2::WorkspaceState::Complete,
        2_000,
        2_000,
        true,
        true,
        false,
    )
    .unwrap();
    assert_eq!(after_verification.elapsed_duration_ms, 18_032);
    assert_eq!(after_verification.stage_elapsed_duration_ms, 0);
    assert_eq!(after_verification.completed_duration_ms, Some(18_032));
    assert!(after_verification.operation_timing.is_none());
}

#[test]
fn prior_complete_verification_timing_survives_cancel_failure_reopen_and_same_stage_retry() {
    let root = TestDirectory::new("prior-complete-verification-timing");
    let workspace_id = "9".repeat(64);
    let workspace = root.path.join(&workspace_id);
    create_private_directory_all(&workspace).unwrap();
    let completed = persist_synthetic_completed_timing(&workspace, &workspace_id, 18_032);
    let canonical_duration = completed.completed_duration_ms;

    let first = phase2::begin_active_session(
        &workspace,
        &workspace_id,
        false,
        false,
        Some(phase2::OperationTimingKind::Verification),
        Some(phase2::WorkspaceState::Verifying),
    )
    .unwrap();
    assert_eq!(first.operation_timing.unwrap().active_elapsed_ms, 0);
    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Verifying,
        phase2::StateCounts::default(),
        Some("verification_running"),
    )
    .unwrap();
    phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        first.operation_generation,
        phase2::WorkspaceState::Verifying,
        1_200,
        1_200,
        true,
        false,
        false,
    )
    .unwrap();
    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Complete,
        phase2::StateCounts::default(),
        Some("verification_cancelled_prior_complete_preserved"),
    )
    .unwrap();
    let cancelled = phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        first.operation_generation,
        phase2::WorkspaceState::Complete,
        1_500,
        1_500,
        true,
        true,
        false,
    )
    .unwrap();
    assert_eq!(cancelled.completed_duration_ms, canonical_duration);
    assert_eq!(cancelled.elapsed_duration_ms, 18_032);
    assert_eq!(cancelled.operation_timing.unwrap().active_elapsed_ms, 1_500);

    let reopened = phase2::read_state(&workspace, &workspace_id)
        .unwrap()
        .unwrap();
    assert!(reopened.active_session_started_at.is_none());
    let resumed = phase2::begin_active_session(
        &workspace,
        &workspace_id,
        false,
        false,
        Some(phase2::OperationTimingKind::Verification),
        Some(phase2::WorkspaceState::Verifying),
    )
    .unwrap();
    let resumed_timing = resumed.operation_timing.unwrap();
    assert_eq!(resumed_timing.active_elapsed_ms, 1_500);
    assert_eq!(resumed_timing.stage_elapsed_ms, 1_500);
    assert_eq!(resumed_timing.generation, resumed.operation_generation);
    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Verifying,
        phase2::StateCounts::default(),
        Some("verification_running"),
    )
    .unwrap();
    phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        resumed.operation_generation,
        phase2::WorkspaceState::Verifying,
        2_300,
        2_300,
        true,
        false,
        false,
    )
    .unwrap();
    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Complete,
        phase2::StateCounts::default(),
        Some("verification_failed_prior_complete_preserved"),
    )
    .unwrap();
    let failed = phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        resumed.operation_generation,
        phase2::WorkspaceState::Complete,
        2_500,
        2_500,
        true,
        true,
        false,
    )
    .unwrap();
    assert_eq!(failed.completed_duration_ms, canonical_duration);
    assert_eq!(failed.operation_timing.unwrap().active_elapsed_ms, 2_500);

    let unrelated = phase2::begin_active_session(
        &workspace,
        &workspace_id,
        false,
        false,
        Some(phase2::OperationTimingKind::Repair),
        Some(phase2::WorkspaceState::Preflight),
    )
    .unwrap();
    let unrelated_timing = unrelated.operation_timing.unwrap();
    assert_eq!(unrelated_timing.active_elapsed_ms, 0);
    assert_eq!(unrelated_timing.stage_elapsed_ms, 0);
    assert_eq!(unrelated.completed_duration_ms, canonical_duration);
}

#[test]
fn prior_complete_repair_timing_survives_crash_and_resets_only_on_stage_change() {
    let root = TestDirectory::new("prior-complete-repair-timing");
    let workspace_id = "8".repeat(64);
    let workspace = root.path.join(&workspace_id);
    create_private_directory_all(&workspace).unwrap();
    let completed = persist_synthetic_completed_timing(&workspace, &workspace_id, 22_000);

    let repair = phase2::begin_active_session(
        &workspace,
        &workspace_id,
        false,
        false,
        Some(phase2::OperationTimingKind::Repair),
        Some(phase2::WorkspaceState::Preflight),
    )
    .unwrap();
    phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        repair.operation_generation,
        phase2::WorkspaceState::Complete,
        500,
        500,
        true,
        false,
        false,
    )
    .unwrap();
    let before_first_transition = phase2::recover_interrupted_state(
        &workspace,
        &workspace_id,
        phase2::read_state(&workspace, &workspace_id)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        before_first_transition.state,
        phase2::WorkspaceState::Complete
    );
    assert!(before_first_transition.active_session_started_at.is_none());
    assert_eq!(
        before_first_transition
            .operation_timing
            .unwrap()
            .active_elapsed_ms,
        500
    );
    let repair = phase2::begin_active_session(
        &workspace,
        &workspace_id,
        false,
        false,
        Some(phase2::OperationTimingKind::Repair),
        Some(phase2::WorkspaceState::Preflight),
    )
    .unwrap();
    assert_eq!(repair.operation_timing.unwrap().active_elapsed_ms, 500);
    assert_eq!(repair.operation_timing.unwrap().stage_elapsed_ms, 500);
    let discovering = phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Discovering,
        phase2::StateCounts::default(),
        Some("repair_discovering"),
    )
    .unwrap();
    let discovering_timing = discovering.operation_timing.unwrap();
    assert_eq!(discovering_timing.active_elapsed_ms, 500);
    assert_eq!(discovering_timing.stage_elapsed_ms, 0);
    phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        repair.operation_generation,
        phase2::WorkspaceState::Discovering,
        900,
        400,
        true,
        false,
        false,
    )
    .unwrap();
    let indexing = phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Indexing,
        phase2::StateCounts::default(),
        Some("repair_indexing"),
    )
    .unwrap();
    assert_eq!(indexing.operation_timing.unwrap().active_elapsed_ms, 900);
    assert_eq!(indexing.operation_timing.unwrap().stage_elapsed_ms, 0);
    phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        repair.operation_generation,
        phase2::WorkspaceState::Indexing,
        1_200,
        300,
        true,
        false,
        false,
    )
    .unwrap();

    let recovered = phase2::recover_interrupted_state(
        &workspace,
        &workspace_id,
        phase2::read_state(&workspace, &workspace_id)
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(recovered.state, phase2::WorkspaceState::Incomplete);
    assert!(recovered.active_session_started_at.is_none());
    let recovered_timing = recovered.operation_timing.unwrap();
    assert_eq!(recovered_timing.active_elapsed_ms, 1_200);
    assert_eq!(recovered_timing.stage_elapsed_ms, 300);
    assert_eq!(
        recovered.completed_duration_ms,
        completed.completed_duration_ms
    );

    let resumed = phase2::begin_active_session(
        &workspace,
        &workspace_id,
        false,
        false,
        Some(phase2::OperationTimingKind::Repair),
        Some(phase2::WorkspaceState::Indexing),
    )
    .unwrap();
    let resumed_timing = resumed.operation_timing.unwrap();
    assert_eq!(resumed_timing.active_elapsed_ms, 1_200);
    assert_eq!(resumed_timing.stage_elapsed_ms, 300);
    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Indexing,
        phase2::StateCounts::default(),
        Some("repair_indexing"),
    )
    .unwrap();
    phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        resumed.operation_generation,
        phase2::WorkspaceState::Indexing,
        1_700,
        800,
        true,
        false,
        false,
    )
    .unwrap();
    let conversations = phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::BuildingConversations,
        phase2::StateCounts::default(),
        Some("building_conversations"),
    )
    .unwrap();
    assert_eq!(
        conversations.operation_timing.unwrap().active_elapsed_ms,
        1_700
    );
    assert_eq!(conversations.operation_timing.unwrap().stage_elapsed_ms, 0);
    let verifying = phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Verifying,
        phase2::StateCounts::default(),
        Some("repair_verifying"),
    )
    .unwrap();
    let verifying_timing = verifying.operation_timing.unwrap();
    assert_eq!(verifying_timing.active_elapsed_ms, 1_700);
    assert_eq!(verifying_timing.stage_elapsed_ms, 0);
    assert_eq!(
        verifying.completed_duration_ms,
        completed.completed_duration_ms
    );
    phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        resumed.operation_generation,
        phase2::WorkspaceState::Verifying,
        1_900,
        200,
        true,
        false,
        false,
    )
    .unwrap();
    let cancelled_repair = phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Cancelled,
        phase2::StateCounts::default(),
        Some("repair_cancelled"),
    )
    .unwrap();
    assert!(phase2::state_has_complete_lineage(&cancelled_repair));
    phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        resumed.operation_generation,
        phase2::WorkspaceState::Cancelled,
        2_000,
        300,
        true,
        true,
        false,
    )
    .unwrap();

    let different_stage_retry = phase2::begin_active_session(
        &workspace,
        &workspace_id,
        false,
        false,
        Some(phase2::OperationTimingKind::Repair),
        Some(phase2::WorkspaceState::Preflight),
    )
    .unwrap();
    let different_stage_timing = different_stage_retry.operation_timing.unwrap();
    assert_eq!(different_stage_timing.active_elapsed_ms, 2_000);
    assert_eq!(different_stage_timing.stage_elapsed_ms, 0);
    let immediate_crash =
        phase2::recover_interrupted_state(&workspace, &workspace_id, different_stage_retry)
            .unwrap();
    assert!(immediate_crash.active_session_started_at.is_none());
    assert_eq!(
        immediate_crash.operation_timing.unwrap().active_elapsed_ms,
        2_000
    );
    assert_eq!(
        immediate_crash.operation_timing.unwrap().stage_elapsed_ms,
        0
    );
    assert_eq!(
        immediate_crash.completed_duration_ms,
        completed.completed_duration_ms
    );

    let failed_retry = phase2::begin_active_session(
        &workspace,
        &workspace_id,
        false,
        false,
        Some(phase2::OperationTimingKind::Repair),
        Some(phase2::WorkspaceState::Preflight),
    )
    .unwrap();
    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Preflight,
        phase2::StateCounts::default(),
        Some("preflight_running"),
    )
    .unwrap();
    phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        failed_retry.operation_generation,
        phase2::WorkspaceState::Preflight,
        2_100,
        100,
        true,
        false,
        false,
    )
    .unwrap();
    let failed_repair = phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Failed,
        phase2::StateCounts::default(),
        Some("repair_failed"),
    )
    .unwrap();
    assert!(phase2::state_has_complete_lineage(&failed_repair));
    let failed_finished = phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        failed_retry.operation_generation,
        phase2::WorkspaceState::Failed,
        2_200,
        200,
        true,
        true,
        false,
    )
    .unwrap();
    assert_eq!(
        failed_finished.completed_duration_ms,
        completed.completed_duration_ms
    );
    assert_eq!(failed_finished.elapsed_duration_ms, 22_000);
}

#[test]
fn non_import_operation_timing_provenance_fails_closed_without_mutation() {
    let root = TestDirectory::new("operation-timing-validation");
    let workspace_id = "7".repeat(64);
    let workspace = root.path.join(&workspace_id);
    create_private_directory_all(&workspace).unwrap();
    persist_synthetic_completed_timing(&workspace, &workspace_id, 12_000);
    let session = phase2::begin_active_session(
        &workspace,
        &workspace_id,
        false,
        false,
        Some(phase2::OperationTimingKind::Repair),
        Some(phase2::WorkspaceState::Preflight),
    )
    .unwrap();
    phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        session.operation_generation,
        phase2::WorkspaceState::Complete,
        1_000,
        500,
        true,
        false,
        false,
    )
    .unwrap();
    let state_path = workspace.join(phase2::IMPORT_STATE_FILE);
    let baseline = raw_json(&state_path);

    for mutate in [
        ("kind", serde_json::json!("future_operation")),
        ("stage", serde_json::json!("complete")),
        ("generation", serde_json::json!(0)),
        ("generation", serde_json::json!(9_007_199_254_740_992_u64)),
        (
            "activeElapsedMs",
            serde_json::json!(9_007_199_254_740_992_u64),
        ),
        ("stageElapsedMs", serde_json::json!(1_001)),
    ] {
        let mut hostile = baseline.clone();
        hostile["operationTiming"][mutate.0] = mutate.1;
        write_raw_json(&state_path, &hostile);
        let before = fs::read(&state_path).unwrap();
        assert!(phase2::read_state(&workspace, &workspace_id).is_err());
        assert_eq!(fs::read(&state_path).unwrap(), before);
    }

    for field_change in [
        ("operationTiming", Value::Null),
        ("activeSessionUpdatesImportTiming", Value::Bool(true)),
    ] {
        let mut hostile = baseline.clone();
        hostile[field_change.0] = field_change.1;
        write_raw_json(&state_path, &hostile);
        let before = fs::read(&state_path).unwrap();
        assert!(phase2::read_state(&workspace, &workspace_id).is_err());
        assert_eq!(fs::read(&state_path).unwrap(), before);
    }

    write_raw_json(&state_path, &baseline);
    let before_checkpoint = fs::read(&state_path).unwrap();
    assert!(phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        session.operation_generation,
        phase2::WorkspaceState::Complete,
        999,
        499,
        true,
        false,
        false,
    )
    .is_err());
    assert_eq!(fs::read(&state_path).unwrap(), before_checkpoint);
    assert!(phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        session.operation_generation.saturating_add(1),
        phase2::WorkspaceState::Complete,
        1_100,
        600,
        true,
        false,
        false,
    )
    .is_err());
    assert_eq!(fs::read(&state_path).unwrap(), before_checkpoint);
    assert!(phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        session.operation_generation,
        phase2::WorkspaceState::Verifying,
        1_100,
        600,
        true,
        false,
        false,
    )
    .is_err());
    assert_eq!(fs::read(&state_path).unwrap(), before_checkpoint);
}

#[test]
fn terminal_crash_recovery_closes_stale_session_and_unblocks_future_operations() {
    let root = TestDirectory::new("terminal-session-recovery");
    let workspace_id = "e".repeat(64);
    let workspace = root.path.join(&workspace_id);
    create_private_directory_all(&workspace).unwrap();
    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Preflight,
        phase2::StateCounts::default(),
        Some("preflight_running"),
    )
    .unwrap();
    let session =
        phase2::begin_active_session(&workspace, &workspace_id, true, false, None, None).unwrap();
    for stage in [
        phase2::WorkspaceState::Extracting,
        phase2::WorkspaceState::ExtractionComplete,
        phase2::WorkspaceState::Discovering,
        phase2::WorkspaceState::Indexing,
        phase2::WorkspaceState::BuildingConversations,
        phase2::WorkspaceState::Verifying,
    ] {
        phase2::transition_state(
            &workspace,
            &workspace_id,
            stage,
            phase2::StateCounts::default(),
            Some("synthetic_transition"),
        )
        .unwrap();
    }
    phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        session.operation_generation,
        phase2::WorkspaceState::Verifying,
        9_500,
        1_200,
        true,
        false,
        false,
    )
    .unwrap();
    let complete = phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Complete,
        phase2::StateCounts::default(),
        Some("verification_complete"),
    )
    .unwrap();
    assert!(complete.active_session_started_at.is_some());
    assert!(complete.completed_duration_ms.is_none());

    let recovered = phase2::recover_interrupted_state(&workspace, &workspace_id, complete).unwrap();
    assert_eq!(recovered.state, phase2::WorkspaceState::Complete);
    assert!(recovered.active_session_started_at.is_none());
    assert_eq!(recovered.completed_duration_ms, None);
    assert!(
        phase2::begin_active_session(&workspace, &workspace_id, true, false, None, None,).is_ok()
    );
}

#[test]
fn fresh_reimport_resets_lineage_pass_counts_and_timing_but_advances_generation() {
    let root = TestDirectory::new("fresh-reimport-state");
    let workspace_id = "f".repeat(64);
    let workspace = root.path.join(&workspace_id);
    create_private_directory_all(&workspace).unwrap();
    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Preflight,
        phase2::StateCounts::default(),
        Some("preflight_running"),
    )
    .unwrap();
    let session =
        phase2::begin_active_session(&workspace, &workspace_id, true, false, None, None).unwrap();
    for stage in [
        phase2::WorkspaceState::Extracting,
        phase2::WorkspaceState::ExtractionComplete,
        phase2::WorkspaceState::Discovering,
        phase2::WorkspaceState::Indexing,
        phase2::WorkspaceState::BuildingConversations,
        phase2::WorkspaceState::Verifying,
        phase2::WorkspaceState::Complete,
    ] {
        phase2::transition_state(
            &workspace,
            &workspace_id,
            stage,
            phase2::StateCounts {
                discovered_candidates: Some(12),
                canonical_indexed: Some(10),
            },
            Some("synthetic_transition"),
        )
        .unwrap();
    }
    let completed = phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        session.operation_generation,
        phase2::WorkspaceState::Complete,
        18_032,
        0,
        true,
        true,
        true,
    )
    .unwrap();
    assert_eq!(completed.completed_duration_ms, Some(18_032));

    let restarted = phase2::restart_import_state(&workspace, &workspace_id).unwrap();
    assert_eq!(restarted.state, phase2::WorkspaceState::Preflight);
    assert_eq!(restarted.last_completed_stage, None);
    assert_eq!(restarted.interrupted_stage, None);
    assert!(!restarted.verified_complete_lineage);
    assert_eq!(restarted.discovered_candidates, 0);
    assert_eq!(restarted.canonical_indexed, 0);
    assert_eq!(restarted.elapsed_duration_ms, 0);
    assert_eq!(restarted.stage_elapsed_duration_ms, 0);
    assert_eq!(restarted.completed_duration_ms, None);
    assert_eq!(restarted.operation_generation, session.operation_generation);
    let next =
        phase2::begin_active_session(&workspace, &workspace_id, true, false, None, None).unwrap();
    assert_eq!(next.operation_generation, session.operation_generation + 1);
}

#[test]
fn start_fresh_invalidates_prior_report_and_new_lineage_verifies_normally() {
    let fixture = SyntheticWorkspace::valid("start-fresh-report-lineage");
    let previous = phase2::verify_active_workspace_for_test(
        &fixture.active,
        "start-fresh-report-lineage-previous",
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    assert_eq!(previous.result, phase2::VerificationResult::Verified);
    assert!(fixture
        .active
        .path
        .join(phase2::LAST_VERIFICATION_FILE)
        .is_file());

    let restarted = phase2::restart_import_state(&fixture.active.path, &fixture.active.id).unwrap();
    assert_eq!(restarted.state, phase2::WorkspaceState::Preflight);
    assert!(!restarted.verified_complete_lineage);
    assert!(
        phase2::get_last_verification(&fixture.active.path, &fixture.active.id)
            .unwrap()
            .is_none()
    );
    let ensured =
        phase2::ensure_state_for_workspace(&fixture.active.path, &fixture.active.id).unwrap();
    assert_eq!(ensured.state, phase2::WorkspaceState::Preflight);
    assert!(!ensured.verified_complete_lineage);

    for (stage, discovered, canonical) in [
        (phase2::WorkspaceState::Extracting, 0, 0),
        (phase2::WorkspaceState::ExtractionComplete, 0, 0),
        (
            phase2::WorkspaceState::Discovering,
            fixture.stats.processed,
            0,
        ),
        (
            phase2::WorkspaceState::Indexing,
            fixture.stats.processed,
            fixture.stats.indexed,
        ),
        (
            phase2::WorkspaceState::BuildingConversations,
            fixture.stats.processed,
            fixture.stats.indexed,
        ),
        (
            phase2::WorkspaceState::Verifying,
            fixture.stats.processed,
            fixture.stats.indexed,
        ),
    ] {
        phase2::transition_state(
            &fixture.active.path,
            &fixture.active.id,
            stage,
            phase2::StateCounts {
                discovered_candidates: Some(discovered),
                canonical_indexed: Some(canonical),
            },
            Some("start_fresh_test"),
        )
        .unwrap();
    }

    let current = phase2::verify_active_workspace_for_test(
        &fixture.active,
        "start-fresh-report-lineage-current",
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    assert_eq!(current.result, phase2::VerificationResult::Verified);
    assert_eq!(current.workspace_state, phase2::WorkspaceState::Complete);
    let current_report = phase2::get_last_verification(&fixture.active.path, &fixture.active.id)
        .unwrap()
        .unwrap();
    assert_eq!(
        current_report.operation_id,
        "start-fresh-report-lineage-current"
    );
    fixture.source_and_extraction_unchanged();
}

#[test]
fn start_fresh_refuses_linked_verification_metadata_without_state_mutation() {
    let fixture = SyntheticWorkspace::valid("start-fresh-linked-report");
    phase2::verify_active_workspace_for_test(
        &fixture.active,
        "start-fresh-linked-report-setup",
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    let state_path = fixture.active.path.join(phase2::IMPORT_STATE_FILE);
    let report_path = fixture.active.path.join(phase2::LAST_VERIFICATION_FILE);
    let outside = fixture
        .active
        .path
        .parent()
        .unwrap()
        .join("outside-report.json");
    let outside_bytes = b"synthetic outside metadata";
    fs::write(&outside, outside_bytes).unwrap();
    fs::remove_file(&report_path).unwrap();
    std::os::unix::fs::symlink(&outside, &report_path).unwrap();
    let state_before = fs::read(&state_path).unwrap();

    let error = phase2::restart_import_state(&fixture.active.path, &fixture.active.id)
        .expect_err("Start Fresh must refuse linked verification metadata");
    assert!(error.message.contains("unsafe to invalidate"));
    assert_eq!(fs::read(&state_path).unwrap(), state_before);
    assert_eq!(fs::read(&outside).unwrap(), outside_bytes);
    fixture.source_and_extraction_unchanged();
}

#[test]
fn legacy_phase2_state_without_phase3_timing_fields_remains_readable() {
    let root = TestDirectory::new("legacy-phase2-timing");
    let workspace_id = "c".repeat(64);
    let workspace = root.path.join(&workspace_id);
    create_private_directory_all(&workspace).unwrap();
    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Incomplete,
        phase2::StateCounts::default(),
        Some("verification_required"),
    )
    .unwrap();
    let state_path = workspace.join(phase2::IMPORT_STATE_FILE);
    let mut value = raw_json(&state_path);
    let object = value.as_object_mut().unwrap();
    object.insert(
        "elapsedDurationMs".to_string(),
        serde_json::json!(86_400_000_u64),
    );
    for key in [
        "activeTimingVersion",
        "activeTimingHistoryComplete",
        "operationGeneration",
        "activeSessionStartedAt",
        "activeSessionUpdatesImportTiming",
        "accumulatedActiveElapsedMs",
        "stageElapsedDurationMs",
        "lastActivityAt",
        "completedDurationMs",
        "pendingCompletionDurationMs",
        "operationTiming",
        "verifiedCompleteLineage",
    ] {
        object.remove(key);
    }
    write_raw_json(&state_path, &value);

    let legacy = phase2::read_state(&workspace, &workspace_id)
        .unwrap()
        .unwrap();
    assert_eq!(legacy.operation_generation, 0);
    assert_eq!(legacy.elapsed_duration_ms, 86_400_000);
    assert_eq!(legacy.active_timing_version, 0);
    assert!(!legacy.active_timing_history_complete);
    assert_eq!(legacy.accumulated_active_elapsed_ms, 0);
    assert_eq!(legacy.stage_elapsed_duration_ms, 0);
    assert!(legacy.active_session_started_at.is_none());
    assert!(legacy.last_activity_at.is_none());
    assert!(legacy.completed_duration_ms.is_none());

    let session =
        phase2::begin_active_session(&workspace, &workspace_id, true, true, None, None).unwrap();
    assert_eq!(session.active_timing_version, phase2::ACTIVE_TIMING_VERSION);
    assert!(!session.active_timing_history_complete);
    assert_eq!(session.elapsed_duration_ms, 0);
    assert_eq!(session.accumulated_active_elapsed_ms, 0);
    assert_eq!(session.stage_elapsed_duration_ms, 0);

    let verifying = phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Verifying,
        phase2::StateCounts::default(),
        Some("verification_running"),
    )
    .unwrap();
    let checkpoint = phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        verifying.operation_generation,
        phase2::WorkspaceState::Verifying,
        2_500,
        2_500,
        true,
        false,
        true,
    )
    .unwrap();
    assert_eq!(checkpoint.elapsed_duration_ms, 2_500);
    assert!(checkpoint.completed_duration_ms.is_none());
    let complete = phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Complete,
        phase2::StateCounts::default(),
        Some("verification_complete"),
    )
    .unwrap();
    assert!(complete.completed_duration_ms.is_none());
    let finished = phase2::checkpoint_active_session(
        &workspace,
        &workspace_id,
        complete.operation_generation,
        phase2::WorkspaceState::Complete,
        2_500,
        0,
        true,
        true,
        true,
    )
    .unwrap();
    assert_eq!(finished.elapsed_duration_ms, 2_500);
    assert!(finished.completed_duration_ms.is_none());
}

#[test]
fn legacy_state_adoption_uses_database_counts_without_walking_extraction() {
    let fixture = SyntheticWorkspace::valid("legacy-state-no-extraction-walk");
    fs::remove_file(fixture.active.path.join(phase2::IMPORT_STATE_FILE)).unwrap();
    let conn = open_sqlite_write_connection(&fixture.index_path()).unwrap();
    conn.execute(
        "DELETE FROM import_metadata WHERE key = 'candidates_discovered'",
        [],
    )
    .unwrap();
    checked_truncate_checkpoint(&conn).unwrap();
    drop(conn);

    let unrelated_link = fixture.active.path.join("extracted/unrelated-link");
    std::os::unix::fs::symlink("missing-target", &unrelated_link).unwrap();
    let adopted = phase2::ensure_state_for_workspace(&fixture.active.path, &fixture.active.id)
        .expect("legacy state adoption must not inspect extraction contents");
    assert_eq!(adopted.state, phase2::WorkspaceState::Incomplete);
    assert_eq!(adopted.discovered_candidates, fixture.stats.indexed as u64);
    assert_eq!(adopted.canonical_indexed, fixture.stats.indexed as u64);
    assert_eq!(adopted.active_timing_version, phase2::ACTIVE_TIMING_VERSION);
    assert!(!adopted.active_timing_history_complete);
    fs::remove_file(unrelated_link).unwrap();
    fixture.source_and_extraction_unchanged();
}

#[test]
fn legacy_verified_report_seeds_maintenance_timing_before_repair_and_resume() {
    let fixture = SyntheticWorkspace::valid("legacy-verified-repair-timing");
    let report = phase2::verify_active_workspace_for_test(
        &fixture.active,
        "legacy-verified-repair-timing-setup",
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    assert_eq!(report.result, phase2::VerificationResult::Verified);

    fs::remove_file(fixture.active.path.join(phase2::IMPORT_STATE_FILE)).unwrap();
    let adopted = phase2::ensure_state_for_workspace(&fixture.active.path, &fixture.active.id)
        .expect("a validated prior Complete report should seed legacy completion lineage");
    assert_eq!(adopted.state, phase2::WorkspaceState::Incomplete);
    assert!(adopted.verified_complete_lineage);
    assert!(!adopted.active_timing_history_complete);

    let repair = phase2::begin_active_session(
        &fixture.active.path,
        &fixture.active.id,
        false,
        false,
        Some(phase2::OperationTimingKind::Repair),
        Some(phase2::WorkspaceState::Preflight),
    )
    .unwrap();
    assert_eq!(repair.elapsed_duration_ms, 0);
    assert_eq!(repair.operation_timing.unwrap().active_elapsed_ms, 0);
    let checkpoint = phase2::checkpoint_active_session(
        &fixture.active.path,
        &fixture.active.id,
        repair.operation_generation,
        phase2::WorkspaceState::Incomplete,
        750,
        750,
        true,
        false,
        false,
    )
    .unwrap();
    let recovered =
        phase2::recover_interrupted_state(&fixture.active.path, &fixture.active.id, checkpoint)
            .unwrap();
    assert!(recovered.verified_complete_lineage);
    assert_eq!(recovered.operation_timing.unwrap().active_elapsed_ms, 750);

    let retried = phase2::begin_active_session(
        &fixture.active.path,
        &fixture.active.id,
        false,
        false,
        Some(phase2::OperationTimingKind::Repair),
        Some(phase2::WorkspaceState::Preflight),
    )
    .unwrap();
    assert_eq!(retried.operation_timing.unwrap().active_elapsed_ms, 750);
    assert_eq!(retried.operation_timing.unwrap().stage_elapsed_ms, 750);
    fixture.source_and_extraction_unchanged();
}

#[test]
fn existing_legacy_state_adopts_complete_report_before_maintenance_timing() {
    let fixture = SyntheticWorkspace::valid("existing-legacy-complete-lineage");
    let report = phase2::verify_active_workspace_for_test(
        &fixture.active,
        "existing-legacy-complete-lineage-setup",
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    assert_eq!(report.result, phase2::VerificationResult::Verified);

    let state_path = fixture.active.path.join(phase2::IMPORT_STATE_FILE);
    let report_path = fixture.active.path.join(phase2::LAST_VERIFICATION_FILE);
    let legacy = rewrite_as_existing_legacy_incomplete_without_lineage(&state_path, 42_000);
    let legacy_revision = legacy["revision"].as_u64().unwrap();
    let report_bytes = fs::read(&report_path).unwrap();
    let before = phase2::read_state(&fixture.active.path, &fixture.active.id)
        .unwrap()
        .unwrap();
    assert!(!phase2::state_has_complete_lineage(&before));

    let adopted = phase2::ensure_state_for_workspace(&fixture.active.path, &fixture.active.id)
        .expect("a valid Complete report should seed an existing legacy sidecar");
    assert_eq!(adopted.state, phase2::WorkspaceState::Incomplete);
    assert!(adopted.verified_complete_lineage);
    assert_eq!(adopted.revision, legacy_revision + 1);
    assert_eq!(adopted.elapsed_duration_ms, 42_000);
    assert_eq!(adopted.accumulated_active_elapsed_ms, 42_000);

    let mut expected = legacy;
    let object = expected.as_object_mut().unwrap();
    object.insert(
        "verifiedCompleteLineage".to_string(),
        serde_json::json!(true),
    );
    object.insert(
        "revision".to_string(),
        serde_json::json!(legacy_revision + 1),
    );
    object.insert(
        "updatedAt".to_string(),
        serde_json::json!(adopted.updated_at),
    );
    assert_eq!(raw_json(&state_path), expected);
    assert_eq!(fs::read(&report_path).unwrap(), report_bytes);

    let repair = phase2::begin_active_session(
        &fixture.active.path,
        &fixture.active.id,
        false,
        false,
        Some(phase2::OperationTimingKind::Repair),
        Some(phase2::WorkspaceState::Preflight),
    )
    .unwrap();
    assert!(!repair.active_session_updates_import_timing);
    assert_eq!(repair.elapsed_duration_ms, 42_000);
    let repair_timing = repair.operation_timing.unwrap();
    assert_eq!(repair_timing.active_elapsed_ms, 0);
    assert_eq!(repair_timing.stage_elapsed_ms, 0);

    let checkpoint = phase2::checkpoint_active_session(
        &fixture.active.path,
        &fixture.active.id,
        repair.operation_generation,
        phase2::WorkspaceState::Incomplete,
        750,
        750,
        true,
        false,
        false,
    )
    .unwrap();
    let recovered =
        phase2::recover_interrupted_state(&fixture.active.path, &fixture.active.id, checkpoint)
            .unwrap();
    assert_eq!(recovered.elapsed_duration_ms, 42_000);
    assert_eq!(recovered.operation_timing.unwrap().active_elapsed_ms, 750);

    let retried = phase2::begin_active_session(
        &fixture.active.path,
        &fixture.active.id,
        false,
        false,
        Some(phase2::OperationTimingKind::Repair),
        Some(phase2::WorkspaceState::Preflight),
    )
    .unwrap();
    assert_eq!(retried.elapsed_duration_ms, 42_000);
    assert_eq!(retried.operation_timing.unwrap().active_elapsed_ms, 750);
    assert_eq!(fs::read(&report_path).unwrap(), report_bytes);
    fixture.source_and_extraction_unchanged();
}

#[test]
fn existing_legacy_lineage_adoption_fails_closed_for_malformed_report() {
    let fixture = SyntheticWorkspace::valid("malformed-existing-lineage-report");
    phase2::verify_active_workspace_for_test(
        &fixture.active,
        "malformed-existing-lineage-report-setup",
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    let state_path = fixture.active.path.join(phase2::IMPORT_STATE_FILE);
    let report_path = fixture.active.path.join(phase2::LAST_VERIFICATION_FILE);
    rewrite_as_existing_legacy_incomplete_without_lineage(&state_path, 42_000);
    let mut malformed = raw_json(&report_path);
    malformed["formatVersion"] = serde_json::json!(2);
    write_raw_json(&report_path, &malformed);
    let state_bytes = fs::read(&state_path).unwrap();
    let report_bytes = fs::read(&report_path).unwrap();

    assert!(phase2::ensure_state_for_workspace(&fixture.active.path, &fixture.active.id).is_err());
    assert_eq!(fs::read(&state_path).unwrap(), state_bytes);
    assert_eq!(fs::read(&report_path).unwrap(), report_bytes);
    fixture.source_and_extraction_unchanged();
}

#[test]
fn prior_complete_repair_stage_chain_remains_verifiable_with_sticky_lineage() {
    let fixture = SyntheticWorkspace::valid("prior-complete-repair-lineage");
    let initial = phase2::verify_active_workspace_for_test(
        &fixture.active,
        "prior-complete-repair-lineage-setup",
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    assert_eq!(initial.result, phase2::VerificationResult::Verified);
    assert!(
        phase2::read_state(&fixture.active.path, &fixture.active.id)
            .unwrap()
            .unwrap()
            .verified_complete_lineage
    );

    for stage in [
        phase2::WorkspaceState::Discovering,
        phase2::WorkspaceState::Indexing,
        phase2::WorkspaceState::BuildingConversations,
        phase2::WorkspaceState::Verifying,
    ] {
        let transitioned = phase2::transition_state(
            &fixture.active.path,
            &fixture.active.id,
            stage,
            phase2::StateCounts {
                discovered_candidates: Some(fixture.stats.processed),
                canonical_indexed: Some(fixture.stats.indexed),
            },
            Some("repair_lineage_test"),
        )
        .unwrap();
        assert!(transitioned.verified_complete_lineage);
    }

    let verified = phase2::verify_active_workspace_for_test(
        &fixture.active,
        "prior-complete-repair-lineage-check",
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    assert_eq!(verified.result, phase2::VerificationResult::Verified);
    assert_eq!(verified.workspace_state, phase2::WorkspaceState::Complete);
    assert!(
        phase2::read_state(&fixture.active.path, &fixture.active.id)
            .unwrap()
            .unwrap()
            .verified_complete_lineage
    );
    fixture.source_and_extraction_unchanged();
}

#[test]
fn pre_field_complete_lineage_is_migrated_before_stage_history_is_overwritten() {
    let fixture = SyntheticWorkspace::valid("pre-field-complete-lineage-migration");
    phase2::verify_active_workspace_for_test(
        &fixture.active,
        "pre-field-complete-lineage-migration-setup",
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    let state_path = fixture.active.path.join(phase2::IMPORT_STATE_FILE);
    let mut legacy = raw_json(&state_path);
    assert_eq!(legacy["lastCompletedStage"], serde_json::json!("verifying"));
    assert_eq!(legacy["completedDurationMs"], Value::Null);
    legacy
        .as_object_mut()
        .unwrap()
        .remove("verifiedCompleteLineage");
    write_raw_json(&state_path, &legacy);
    let migrated_source = phase2::read_state(&fixture.active.path, &fixture.active.id)
        .unwrap()
        .unwrap();
    assert!(!migrated_source.verified_complete_lineage);
    assert!(phase2::state_has_complete_lineage(&migrated_source));

    phase2::transition_state(
        &fixture.active.path,
        &fixture.active.id,
        phase2::WorkspaceState::Discovering,
        phase2::StateCounts {
            discovered_candidates: Some(fixture.stats.processed),
            canonical_indexed: Some(fixture.stats.indexed),
        },
        Some("repair_lineage_test"),
    )
    .unwrap();
    let indexing = phase2::transition_state(
        &fixture.active.path,
        &fixture.active.id,
        phase2::WorkspaceState::Indexing,
        phase2::StateCounts {
            discovered_candidates: Some(fixture.stats.processed),
            canonical_indexed: Some(fixture.stats.indexed),
        },
        Some("repair_lineage_test"),
    )
    .unwrap();
    assert!(indexing.verified_complete_lineage);
    assert_eq!(
        indexing.last_completed_stage,
        Some(phase2::WorkspaceState::Discovering)
    );
    assert_eq!(indexing.completed_duration_ms, None);

    let interrupted = phase2::transition_state(
        &fixture.active.path,
        &fixture.active.id,
        phase2::WorkspaceState::Incomplete,
        phase2::StateCounts {
            discovered_candidates: Some(fixture.stats.processed),
            canonical_indexed: Some(fixture.stats.indexed),
        },
        Some("interrupted_process"),
    )
    .unwrap();
    assert!(interrupted.verified_complete_lineage);
    assert!(phase2::state_has_complete_lineage(&interrupted));
    assert_ne!(
        interrupted.last_completed_stage,
        Some(phase2::WorkspaceState::Verifying)
    );
    fixture.source_and_extraction_unchanged();
}

#[test]
fn legacy_state_adoption_rejects_contradictory_database_candidate_count() {
    let fixture = SyntheticWorkspace::valid("legacy-state-contradictory-count");
    let state_path = fixture.active.path.join(phase2::IMPORT_STATE_FILE);
    fs::remove_file(&state_path).unwrap();
    let conn = open_sqlite_write_connection(&fixture.index_path()).unwrap();
    set_metadata_value(&conn, "candidates_discovered", "1").unwrap();
    checked_truncate_checkpoint(&conn).unwrap();
    drop(conn);

    let error = phase2::ensure_state_for_workspace(&fixture.active.path, &fixture.active.id)
        .expect_err("candidate metadata below canonical count must fail closed");
    assert!(error
        .message
        .contains("candidate metadata is contradictory"));
    assert!(!state_path.exists());
    fixture.source_and_extraction_unchanged();
}

#[test]
fn missing_manifest_adoption_is_bounded_cancellable_and_retryable() {
    let fixture = SyntheticWorkspace::valid("manifest-adoption-progress");
    let expected = fixture.manifest();
    let manifest_path = fixture.active.path.join(phase2::IMPORT_MANIFEST_FILE);
    fs::remove_file(&manifest_path).unwrap();

    let mut callbacks = 0_usize;
    let error = phase2::ensure_phase2_manifest_with_progress(
        &fixture.active,
        &fixture.identity,
        |discovered, activity_observed| {
            callbacks = callbacks.saturating_add(1);
            if callbacks == 1 {
                assert_eq!(discovered, 0);
                assert!(!activity_observed);
                return Ok(());
            }
            Err(import_cancelled_error(
                "Synthetic manifest adoption cancellation.",
            ))
        },
    )
    .expect_err("bounded manifest discovery callback should cancel adoption");
    assert_eq!(error.code, Some(IMPORT_CANCELLED_CODE));
    assert!(callbacks >= 2);
    assert!(!manifest_path.exists());
    fixture.source_and_extraction_unchanged();

    let mut retry_callbacks = Vec::new();
    let retried = phase2::ensure_phase2_manifest_with_progress(
        &fixture.active,
        &fixture.identity,
        |discovered, activity_observed| {
            retry_callbacks.push((discovered, activity_observed));
            Ok(())
        },
    )
    .expect("manifest adoption should retry from the unchanged extraction");
    assert_eq!(
        retried.candidate_aggregate_digest,
        expected.candidate_aggregate_digest
    );
    assert_eq!(retried.candidate_records, expected.candidate_records);
    assert_eq!(retried.candidate_count, expected.candidate_count);
    assert_eq!(retry_callbacks.first(), Some(&(0, false)));
    assert!(retry_callbacks.iter().any(|(_, activity)| *activity));
    fixture.source_and_extraction_unchanged();
}

#[test]
fn contradictory_state_counts_are_rejected_without_revision_or_file_changes() {
    let root = TestDirectory::new("state-counter-contradiction");
    let workspace_id = "a".repeat(64);
    let workspace = root.path.join(&workspace_id);
    create_private_directory_all(&workspace).unwrap();
    let initial = phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Incomplete,
        phase2::StateCounts {
            discovered_candidates: Some(4),
            canonical_indexed: Some(4),
        },
        Some("verification_required"),
    )
    .unwrap();
    let state_path = workspace.join(phase2::IMPORT_STATE_FILE);
    let bytes_before = fs::read(&state_path).unwrap();
    assert!(phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Indexing,
        phase2::StateCounts {
            discovered_candidates: Some(4),
            canonical_indexed: Some(5),
        },
        Some("indexing_running"),
    )
    .is_err());
    assert_eq!(fs::read(&state_path).unwrap(), bytes_before);
    assert_eq!(
        phase2::read_state(&workspace, &workspace_id)
            .unwrap()
            .unwrap()
            .revision,
        initial.revision
    );
}

#[test]
fn complete_cannot_be_minted_without_verification_transition() {
    let root = TestDirectory::new("complete-gate");
    let workspace_id = "2".repeat(64);
    let workspace = root.path.join(&workspace_id);
    create_private_directory_all(&workspace).unwrap();

    assert!(phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Complete,
        phase2::StateCounts::default(),
        Some("invalid_complete")
    )
    .is_err());

    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Preflight,
        phase2::StateCounts::default(),
        Some("preflight_ready"),
    )
    .unwrap();
    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Incomplete,
        phase2::StateCounts::default(),
        Some("verification_required"),
    )
    .unwrap();
    assert!(phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Complete,
        phase2::StateCounts::default(),
        Some("invalid_complete")
    )
    .is_err());
}

#[test]
fn cancelled_and_failed_states_remain_distinct() {
    for (ordinal, terminal) in [
        phase2::WorkspaceState::Cancelled,
        phase2::WorkspaceState::Failed,
    ]
    .into_iter()
    .enumerate()
    {
        let root = TestDirectory::new("terminal-state");
        let workspace_id = format!("{:064x}", ordinal + 30);
        let workspace = root.path.join(&workspace_id);
        create_private_directory_all(&workspace).unwrap();
        phase2::transition_state(
            &workspace,
            &workspace_id,
            phase2::WorkspaceState::Preflight,
            phase2::StateCounts::default(),
            Some("preflight_ready"),
        )
        .unwrap();
        phase2::transition_state(
            &workspace,
            &workspace_id,
            phase2::WorkspaceState::Extracting,
            phase2::StateCounts::default(),
            Some("extraction_running"),
        )
        .unwrap();
        let record = phase2::transition_state(
            &workspace,
            &workspace_id,
            terminal,
            phase2::StateCounts::default(),
            Some(if terminal == phase2::WorkspaceState::Cancelled {
                "import_cancelled"
            } else {
                "import_failed"
            }),
        )
        .unwrap();
        assert_eq!(record.state, terminal);
        assert_eq!(
            record.interrupted_stage,
            Some(phase2::WorkspaceState::Extracting)
        );
    }
}

#[test]
fn future_and_unknown_workspace_states_fail_closed() {
    let root = TestDirectory::new("future-state");
    let workspace_id = "3".repeat(64);
    let workspace = root.path.join(&workspace_id);
    create_private_directory_all(&workspace).unwrap();
    phase2::transition_state(
        &workspace,
        &workspace_id,
        phase2::WorkspaceState::Incomplete,
        phase2::StateCounts::default(),
        Some("verification_required"),
    )
    .unwrap();
    let state_path = workspace.join(phase2::IMPORT_STATE_FILE);
    let mut value = raw_json(&state_path);
    value["formatVersion"] = Value::from(99);
    write_raw_json(&state_path, &value);
    assert!(phase2::read_state(&workspace, &workspace_id).is_err());
    assert_eq!(
        phase2::integrity_snapshot(&workspace, &workspace_id, None).classification,
        "unsupported"
    );

    value["formatVersion"] = Value::from(1);
    value["state"] = Value::from("future_quantum_stage");
    write_raw_json(&state_path, &value);
    assert!(phase2::read_state(&workspace, &workspace_id).is_err());
}

#[test]
fn future_database_schema_is_refused_before_phase2_adoption_without_mutation() {
    let fixture = SyntheticWorkspace::valid("future-schema-no-adoption");
    let state_path = fixture.active.path.join(phase2::IMPORT_STATE_FILE);
    let manifest_path = fixture.active.path.join(phase2::IMPORT_MANIFEST_FILE);
    fs::remove_file(&state_path).unwrap();
    fs::remove_file(&manifest_path).unwrap();
    let conn = open_sqlite_write_connection(&fixture.index_path()).unwrap();
    conn.pragma_update(None, "user_version", SQLITE_SCHEMA_VERSION_CURRENT + 1)
        .unwrap();
    checked_truncate_checkpoint(&conn).unwrap();
    drop(conn);

    let index_before = sha256_file(&fixture.index_path()).unwrap();
    assert!(recover_phase2_workspace_state(&fixture.active).is_err());
    assert!(!state_path.exists());
    assert!(!manifest_path.exists());

    let report = phase2::verify_active_workspace_for_test(
        &fixture.active,
        "future_schema_verify",
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    assert_eq!(report.result, phase2::VerificationResult::Unsupported);
    assert!(!state_path.exists());
    assert!(!manifest_path.exists());
    assert!(!fixture
        .active
        .path
        .join(phase2::LAST_VERIFICATION_FILE)
        .exists());
    assert_eq!(sha256_file(&fixture.index_path()).unwrap(), index_before);
    fixture.source_and_extraction_unchanged();
}

#[test]
fn recovery_layouts_from_another_action_are_refused_without_mutation() {
    for (label, foreign_name, requested_name) in [
        (
            "repair-refuses-resume",
            "index.tmp.sqlite",
            "index.reindex.tmp.sqlite",
        ),
        (
            "resume-refuses-repair",
            "index.reindex.tmp.sqlite",
            "index.tmp.sqlite",
        ),
    ] {
        let fixture = SyntheticWorkspace::valid(label);
        let foreign = fixture.active.path.join(foreign_name);
        fs::copy(fixture.index_path(), &foreign).unwrap();
        phase2::stage_manifest_from_index(
            &fixture.active.path,
            &fixture.active.id,
            &foreign,
            &fixture.stats,
            "complete",
        )
        .unwrap();
        let active_before = sha256_file(&fixture.index_path()).unwrap();
        let foreign_before = sha256_file(&foreign).unwrap();
        let state_before = fs::read(fixture.active.path.join(phase2::IMPORT_STATE_FILE)).unwrap();
        let manifest_before =
            fs::read(fixture.active.path.join(phase2::IMPORT_MANIFEST_FILE)).unwrap();
        let staged_before = fs::read(
            fixture
                .active
                .path
                .join(phase2::STAGED_IMPORT_MANIFEST_FILE),
        )
        .unwrap();

        assert!(
            refuse_foreign_index_recovery_layout(&fixture.active.path, requested_name).is_err()
        );
        assert_eq!(sha256_file(&fixture.index_path()).unwrap(), active_before);
        assert_eq!(sha256_file(&foreign).unwrap(), foreign_before);
        assert_eq!(
            fs::read(fixture.active.path.join(phase2::IMPORT_STATE_FILE)).unwrap(),
            state_before
        );
        assert_eq!(
            fs::read(fixture.active.path.join(phase2::IMPORT_MANIFEST_FILE)).unwrap(),
            manifest_before
        );
        assert_eq!(
            fs::read(
                fixture
                    .active
                    .path
                    .join(phase2::STAGED_IMPORT_MANIFEST_FILE)
            )
            .unwrap(),
            staged_before
        );
        fixture.source_and_extraction_unchanged();
    }
}

#[test]
fn committed_wal_temp_indexes_recover_without_losing_batches_or_touching_active_data() {
    for temp_name in ["index.tmp.sqlite", "index.reindex.tmp.sqlite"] {
        let fixture = SyntheticWorkspace::valid(if temp_name == "index.tmp.sqlite" {
            "resume-wal-crash"
        } else {
            "repair-wal-crash"
        });
        let active_before = sha256_file(&fixture.index_path()).unwrap();
        let temp = fixture.active.path.join(temp_name);
        create_committed_wal_crash_layout(&fixture.index_path(), &temp);
        assert!(sqlite_sidecar_exists(&temp));
        assert_eq!(
            open_sqlite_read_only_connection(&temp)
                .ok()
                .and_then(|conn| metadata_value(&conn, "synthetic_hard_crash_probe").ok())
                .flatten()
                .as_deref(),
            Some("committed")
        );

        checkpoint_owned_temp_recovery_index(&fixture.active, &temp).unwrap();
        assert!(!sqlite_sidecar_exists(&temp));
        let conn = open_sqlite_read_only_connection(&temp).unwrap();
        configure_sqlite_connection(&conn, true).unwrap();
        assert_eq!(
            metadata_value(&conn, "synthetic_hard_crash_probe")
                .unwrap()
                .as_deref(),
            Some("committed")
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row
                .get::<_, usize>(0))
                .unwrap(),
            fixture.stats.indexed
        );
        drop(conn);
        phase2::transition_state(
            &fixture.active.path,
            &fixture.active.id,
            phase2::WorkspaceState::Incomplete,
            phase2::StateCounts {
                discovered_candidates: Some(fixture.discovery.count),
                canonical_indexed: Some(fixture.stats.indexed),
            },
            Some("interrupted_process"),
        )
        .unwrap();
        phase2::transition_state(
            &fixture.active.path,
            &fixture.active.id,
            phase2::WorkspaceState::Indexing,
            phase2::StateCounts {
                discovered_candidates: Some(fixture.discovery.count),
                canonical_indexed: Some(fixture.stats.indexed),
            },
            Some("indexing_running"),
        )
        .unwrap();
        let mut conn = open_sqlite_write_connection(&temp).unwrap();
        prepare_import_metadata(
            &mut conn,
            &fixture.identity,
            &fixture.active.id,
            fixture.discovery.count,
            &fixture.active.path,
            fixture.active.location_mode,
            "synthetic-readpst",
            "synthetic-readpst-0",
            true,
            false,
            &fixture.discovery,
        )
        .unwrap();
        let resumed = stream_index_eml_files_with_control(
            &fixture.active.path,
            &fixture.active.path.join("extracted"),
            &mut conn,
            &fixture.discovery,
            2,
            "Synthetic cancellation.",
            |_| false,
            |_| {},
        )
        .unwrap();
        assert_eq!(resumed.inserted, 0);
        assert_eq!(resumed.skipped_existing, fixture.discovery.count);
        drop(conn);
        assert_eq!(sha256_file(&fixture.index_path()).unwrap(), active_before);
        fixture.source_and_extraction_unchanged();
    }
}

#[test]
fn identity_and_operation_inputs_reject_unsafe_values() {
    assert!(phase2::validate_relative_identity("Inbox/message.eml"));
    for value in [
        "",
        "/private/message.eml",
        "../message.eml",
        "Inbox/../message.eml",
        "Inbox\\message.eml",
        "Inbox/line\nbreak.eml",
    ] {
        assert!(!phase2::validate_relative_identity(value));
    }
    for value in ["", "contains space", "slash/value", "line\nbreak"] {
        assert!(phase2::validate_operation_id(value).is_err());
    }
    assert!(phase2::validate_operation_id("quick_verify-7.test").is_ok());
}

#[test]
fn manifest_round_trip_is_content_free_and_rejects_malformed_or_future_data() {
    let fixture = SyntheticWorkspace::valid("manifest-contract");
    let manifest = fixture.manifest();
    assert_eq!(manifest.candidate_count, fixture.discovery.count as u64);
    assert_eq!(
        manifest.canonical_message_total,
        fixture.stats.indexed as u64
    );
    assert_eq!(manifest.inserted_count, fixture.stats.inserted as u64);
    assert_eq!(manifest.skipped_existing_count, 0);
    assert_eq!(manifest.parse_failure_count, 0);
    let serialized = serde_json::to_string(&manifest).unwrap();
    assert!(!serialized.contains("Synthetic body"));
    assert!(!serialized.contains("Fictional status"));
    assert!(!serialized.contains("ada@example.test"));

    let path = fixture.active.path.join(phase2::IMPORT_MANIFEST_FILE);
    let original = raw_json(&path);
    let mut future = original.clone();
    future["formatVersion"] = Value::from(2);
    write_raw_json(&path, &future);
    assert!(phase2::read_manifest(&fixture.active.path, &fixture.active.id).is_err());

    let mut malformed = original.clone();
    malformed["candidateCount"] = Value::from(999);
    write_raw_json(&path, &malformed);
    assert!(phase2::read_manifest(&fixture.active.path, &fixture.active.id).is_err());

    let mut duplicate_identity = original;
    let first = duplicate_identity["candidateRecords"][0].clone();
    duplicate_identity["candidateRecords"]
        .as_array_mut()
        .unwrap()
        .push(first);
    duplicate_identity["candidateCount"] = Value::from(fixture.discovery.count as u64 + 1);
    write_raw_json(&path, &duplicate_identity);
    assert!(phase2::read_manifest(&fixture.active.path, &fixture.active.id).is_err());
    fixture.source_unchanged();
}

#[test]
fn changed_discovery_digest_is_refused_without_rewriting_phase2_provenance() {
    let fixture = SyntheticWorkspace::valid("discovery-digest-drift");
    let manifest_path = fixture.active.path.join(phase2::IMPORT_MANIFEST_FILE);
    let state_path = fixture.active.path.join(phase2::IMPORT_STATE_FILE);
    let manifest_before = fs::read(&manifest_path).unwrap();
    let state_before = fs::read(&state_path).unwrap();
    let index_before = sha256_file(&fixture.index_path()).unwrap();
    let candidate = &fixture.discovery.candidates[0].path;
    let mut changed = fs::read(candidate).unwrap();
    changed[0] ^= 1;
    fs::write(candidate, changed).unwrap();
    let changed_discovery =
        discover_eml_candidates(&fixture.active.path.join("extracted")).unwrap();
    assert_eq!(changed_discovery.count, fixture.discovery.count);
    assert_ne!(
        changed_discovery.identity_manifest,
        fixture.discovery.identity_manifest
    );

    assert!(phase2::ensure_manifest_for_existing_extraction(
        &fixture.active.path,
        &fixture.identity,
        &fixture.active.id,
        "synthetic-readpst-0",
        &changed_discovery,
    )
    .is_err());
    assert_eq!(fs::read(&manifest_path).unwrap(), manifest_before);
    assert_eq!(fs::read(&state_path).unwrap(), state_before);
    assert_eq!(sha256_file(&fixture.index_path()).unwrap(), index_before);
    fixture.source_unchanged();
}

#[test]
fn valid_schema_v3_workspace_without_phase2_attachment_total_adopts_compatibly() {
    let fixture = SyntheticWorkspace::valid("legacy-v3-attachment-total");
    for sidecar in [
        phase2::IMPORT_STATE_FILE,
        phase2::IMPORT_MANIFEST_FILE,
        phase2::LAST_VERIFICATION_FILE,
    ] {
        let path = fixture.active.path.join(sidecar);
        if path.exists() {
            fs::remove_file(path).unwrap();
        }
    }
    let conn = open_sqlite_write_connection(&fixture.index_path()).unwrap();
    conn.execute(
        "DELETE FROM import_metadata WHERE key = 'canonical_attachment_total'",
        [],
    )
    .unwrap();
    checked_truncate_checkpoint(&conn).unwrap();
    drop(conn);

    let report = phase2::verify_active_workspace_for_test(
        &fixture.active,
        "legacy_v3_verify",
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    assert_eq!(report.result, phase2::VerificationResult::Verified);
    assert_eq!(report.workspace_state, phase2::WorkspaceState::Complete);
    assert_eq!(report.attachments, fixture.manifest().attachment_count);
    fixture.source_and_extraction_unchanged();
}

#[test]
fn every_transient_interruption_reopens_as_incomplete_with_stage_details() {
    let routes = [
        vec![phase2::WorkspaceState::Extracting],
        vec![
            phase2::WorkspaceState::Extracting,
            phase2::WorkspaceState::ExtractionComplete,
            phase2::WorkspaceState::Discovering,
        ],
        vec![
            phase2::WorkspaceState::Extracting,
            phase2::WorkspaceState::ExtractionComplete,
            phase2::WorkspaceState::Discovering,
            phase2::WorkspaceState::Indexing,
        ],
        vec![
            phase2::WorkspaceState::Extracting,
            phase2::WorkspaceState::ExtractionComplete,
            phase2::WorkspaceState::Discovering,
            phase2::WorkspaceState::Indexing,
            phase2::WorkspaceState::BuildingConversations,
        ],
        vec![
            phase2::WorkspaceState::Extracting,
            phase2::WorkspaceState::ExtractionComplete,
            phase2::WorkspaceState::Discovering,
            phase2::WorkspaceState::Indexing,
            phase2::WorkspaceState::BuildingConversations,
            phase2::WorkspaceState::Verifying,
        ],
    ];
    for (ordinal, route) in routes.into_iter().enumerate() {
        let root = TestDirectory::new("interruption");
        let workspace_id = format!("{:064x}", ordinal + 10);
        let workspace = root.path.join(&workspace_id);
        create_private_directory_all(&workspace).unwrap();
        create_private_directory_all(&workspace.join("extracted")).unwrap();
        phase2::transition_state(
            &workspace,
            &workspace_id,
            phase2::WorkspaceState::Preflight,
            phase2::StateCounts::default(),
            Some("preflight_ready"),
        )
        .unwrap();
        for stage in route.iter().copied() {
            phase2::transition_state(
                &workspace,
                &workspace_id,
                stage,
                phase2::StateCounts {
                    discovered_candidates: Some(9),
                    canonical_indexed: Some(4),
                },
                Some("synthetic_transition"),
            )
            .unwrap();
        }
        let interrupted = *route.last().unwrap();
        let snapshot = phase2::integrity_snapshot(&workspace, &workspace_id, None);
        assert_eq!(snapshot.classification, "incomplete");
        assert_eq!(snapshot.state, interrupted);
        assert!(snapshot.can_resume_indexing);
        let details = serde_json::to_value(snapshot.resume_details.unwrap()).unwrap();
        assert_eq!(details["interruptedStage"], interrupted.as_str());
        assert_eq!(details["discoveredCandidates"], 9);
        assert_eq!(details["canonicalIndexed"], 4);
        assert_eq!(details["remainingCandidates"], 5);
    }
}

#[test]
fn interrupted_recovery_reconciles_committed_counts_without_losing_last_success() {
    let fixture = SyntheticWorkspace::valid("interrupted-recovery");
    let stale = phase2::transition_state(
        &fixture.active.path,
        &fixture.active.id,
        phase2::WorkspaceState::BuildingConversations,
        phase2::StateCounts {
            discovered_candidates: Some(99),
            canonical_indexed: Some(0),
        },
        Some("synthetic_transition"),
    )
    .unwrap();
    let last_successful_update = stale.last_successful_update.clone();
    let recovered =
        phase2::recover_interrupted_state(&fixture.active.path, &fixture.active.id, stale.clone())
            .unwrap();
    assert_eq!(recovered.state, phase2::WorkspaceState::Incomplete);
    assert_eq!(
        recovered.interrupted_stage,
        Some(phase2::WorkspaceState::BuildingConversations)
    );
    assert_eq!(
        recovered.discovered_candidates,
        fixture.discovery.count as u64
    );
    assert_eq!(recovered.canonical_indexed, fixture.stats.indexed as u64);
    assert_eq!(recovered.remaining_candidates, Some(0));
    assert_eq!(recovered.last_successful_update, last_successful_update);
    assert_eq!(recovered.revision, stale.revision + 1);
    fixture.source_and_extraction_unchanged();
}

#[test]
fn quick_and_full_extraction_checks_are_tiered() {
    let fixture = SyntheticWorkspace::valid("verification-levels");
    let manifest = fixture.manifest();
    let candidate = &manifest.candidate_records[0];
    let path = fixture
        .active
        .path
        .join("extracted")
        .join(&candidate.relative_path);
    let original = fs::read(&path).unwrap();
    let mut changed = original.clone();
    let changed_index = changed.len() - 2;
    changed[changed_index] ^= 1;
    fs::write(&path, &changed).unwrap();

    let (_, quick_mismatch) = phase2::scan_extraction_for_test(
        &fixture.active.path.join("extracted"),
        &manifest,
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    let (_, full_mismatch) = phase2::scan_extraction_for_test(
        &fixture.active.path.join("extracted"),
        &manifest,
        phase2::VerificationLevel::Full,
    )
    .unwrap();
    assert!(
        !quick_mismatch,
        "Quick Verify should avoid a full content rehash"
    );
    assert!(
        full_mismatch,
        "Full Verify should detect same-size content changes"
    );
    fs::write(path, original).unwrap();
    fixture.source_unchanged();
}

#[test]
fn extraction_volume_disappearing_between_snapshots_fails_without_index_promotion() {
    let fixture = SyntheticWorkspace::valid("extraction-volume-race");
    let manifest = fixture.manifest();
    let extract = fixture.active.path.join("extracted");
    let detached = fixture.active.path.join("synthetic-detached-extraction");
    let active_index = fixture.index_path();
    let replacement = fixture.active.path.join("index.reindex.tmp.sqlite");
    fs::copy(&active_index, &replacement).unwrap();
    let active_hash = sha256_file(&active_index).unwrap();
    let replacement_hash = sha256_file(&replacement).unwrap();

    let result = phase2::scan_extraction_with_between_passes_for_test(
        &extract,
        &manifest,
        phase2::VerificationLevel::Quick,
        || fs::rename(&extract, &detached).unwrap(),
    );
    assert!(result.is_err());
    assert_eq!(sha256_file(&active_index).unwrap(), active_hash);
    assert_eq!(sha256_file(&replacement).unwrap(), replacement_hash);
    assert!(!fixture
        .active
        .path
        .join(phase2::STAGED_IMPORT_MANIFEST_FILE)
        .exists());

    fs::rename(detached, extract).unwrap();
    fixture.source_and_extraction_unchanged();
}

#[test]
fn workspace_disappearance_before_finalization_never_recreates_a_ghost_workspace() {
    let fixture = SyntheticWorkspace::valid("workspace-disappears-finalization");
    let active_hash = sha256_file(&fixture.index_path()).unwrap();
    let detached = fixture._root.path.join("detached-workspace-volume");
    let workspace_for_hook = fixture.active.path.clone();
    let detached_for_hook = detached.clone();
    phase2::set_before_verification_finalization_hook_for_test(move || {
        fs::rename(workspace_for_hook, detached_for_hook).unwrap();
    });

    assert!(phase2::verify_active_workspace_for_test(
        &fixture.active,
        "workspace_disappeared",
        phase2::VerificationLevel::Quick,
    )
    .is_err());
    assert!(!fixture.active.path.exists());
    assert_eq!(
        sha256_file(&detached.join("index.sqlite")).unwrap(),
        active_hash
    );
    assert!(!detached.join(phase2::LAST_VERIFICATION_FILE).exists());
    fixture.source_unchanged();

    fs::rename(&detached, &fixture.active.path).unwrap();
    fixture.source_and_extraction_unchanged();
}

#[test]
fn complete_valid_workspace_verifies_and_persists_last_report() {
    let fixture = SyntheticWorkspace::valid("verified-workspace");
    fs::set_permissions(&fixture.active.pst_path, fs::Permissions::from_mode(0o400)).unwrap();
    let report = verify_for_test(
        &fixture,
        "verified_workspace",
        phase2::VerificationLevel::Quick,
    );
    assert_eq!(report.result, phase2::VerificationResult::Verified);
    assert_eq!(report.workspace_state, phase2::WorkspaceState::Complete);
    assert_eq!(report.discovered_candidates, fixture.discovery.count as u64);
    assert_eq!(report.canonical_messages, fixture.stats.indexed as u64);
    assert_eq!(report.missing_canonical_records, 0);
    assert_eq!(report.unexpected_canonical_records, 0);
    assert_eq!(report.source_integrity, phase2::SourceIntegrity::Passed);
    assert_eq!(report.conversation_state, "complete");
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning.contains("Internally verified against")));
    let last = phase2::get_last_verification(&fixture.active.path, &fixture.active.id)
        .unwrap()
        .expect("last verification should persist");
    assert_eq!(last.operation_id, "verified_workspace");
    let full = verify_for_test(
        &fixture,
        "verified_workspace_full",
        phase2::VerificationLevel::Full,
    );
    assert_eq!(full.result, phase2::VerificationResult::Verified);
    assert_eq!(full.verification_level, phase2::VerificationLevel::Full);
    fixture.source_unchanged();
}

#[test]
fn empty_workspace_requires_the_conversation_schema_marker_for_completion() {
    let fixture = SyntheticWorkspace::with_records("empty-conversation-marker", &[]);
    let verified = verify_for_test(
        &fixture,
        "empty_marker_baseline",
        phase2::VerificationLevel::Quick,
    );
    assert_eq!(verified.result, phase2::VerificationResult::Verified);
    assert_eq!(verified.canonical_messages, 0);

    let conn = open_sqlite_write_connection(&fixture.index_path()).unwrap();
    conn.execute(
        "DELETE FROM import_metadata WHERE key = 'conversation_schema_version'",
        [],
    )
    .unwrap();
    checked_truncate_checkpoint(&conn).unwrap();
    drop(conn);
    let snapshot = phase2::integrity_snapshot(
        &fixture.active.path,
        &fixture.active.id,
        Some(&fixture.active.pst_path),
    );
    assert_ne!(snapshot.classification, "complete");
    assert_eq!(snapshot.conversation_state, "incomplete");
    let report = verify_for_test(
        &fixture,
        "empty_marker_missing",
        phase2::VerificationLevel::Quick,
    );
    assert_eq!(
        report.result,
        phase2::VerificationResult::DatabaseInconsistent
    );
    fixture.source_and_extraction_unchanged();
}

#[test]
fn source_unavailable_or_changed_never_deletes_a_complete_local_workspace() {
    let fixture = SyntheticWorkspace::valid("source-availability");
    let verified = verify_for_test(&fixture, "source_initial", phase2::VerificationLevel::Quick);
    assert_eq!(verified.result, phase2::VerificationResult::Verified);

    let moved = fixture.active.pst_path.with_extension("moved.pst");
    fs::rename(&fixture.active.pst_path, &moved).unwrap();
    let unavailable = verify_for_test(
        &fixture,
        "source_unavailable",
        phase2::VerificationLevel::Quick,
    );
    assert_eq!(
        unavailable.result,
        phase2::VerificationResult::SourceUnavailable
    );
    assert_eq!(
        unavailable.workspace_state,
        phase2::WorkspaceState::Complete
    );
    assert!(fixture.index_path().is_file());
    fs::rename(&moved, &fixture.active.pst_path).unwrap();

    let mut changed_bytes = SYNTHETIC_PST_BYTES.to_vec();
    changed_bytes.push(b'!');
    fs::write(&fixture.active.pst_path, changed_bytes).unwrap();
    let changed = verify_for_test(&fixture, "source_changed", phase2::VerificationLevel::Quick);
    assert_eq!(changed.result, phase2::VerificationResult::SourceChanged);
    assert_eq!(changed.workspace_state, phase2::WorkspaceState::Complete);
    assert!(fixture.index_path().is_file());
}

#[test]
fn interrupted_reverification_preserves_prior_complete_evidence_when_source_is_absent() {
    let fixture = SyntheticWorkspace::valid("interrupted-reverification");
    let verified = verify_for_test(
        &fixture,
        "reverification_initial",
        phase2::VerificationLevel::Quick,
    );
    assert_eq!(verified.result, phase2::VerificationResult::Verified);
    fs::remove_file(fixture.active.path.join(phase2::LAST_VERIFICATION_FILE)).unwrap();
    phase2::transition_state(
        &fixture.active.path,
        &fixture.active.id,
        phase2::WorkspaceState::Verifying,
        phase2::StateCounts {
            discovered_candidates: Some(fixture.discovery.count),
            canonical_indexed: Some(fixture.stats.indexed),
        },
        Some("verification_running"),
    )
    .unwrap();
    let moved = fixture
        .active
        .pst_path
        .with_extension("temporarily-unmounted");
    fs::rename(&fixture.active.pst_path, &moved).unwrap();

    let recovered = verify_for_test(
        &fixture,
        "reverification_recovered",
        phase2::VerificationLevel::Quick,
    );
    assert_eq!(
        recovered.result,
        phase2::VerificationResult::SourceUnavailable
    );
    assert_eq!(recovered.workspace_state, phase2::WorkspaceState::Complete);
    assert!(fixture.index_path().is_file());
    fs::rename(moved, &fixture.active.pst_path).unwrap();
    fixture.source_and_extraction_unchanged();
}

#[test]
fn interrupted_conversation_rebuild_preserves_complete_lineage_without_last_report() {
    let fixture = SyntheticWorkspace::valid("conversation-lineage-no-report");
    assert_eq!(
        verify_for_test(
            &fixture,
            "conversation_lineage_initial",
            phase2::VerificationLevel::Quick,
        )
        .result,
        phase2::VerificationResult::Verified
    );
    fs::remove_file(fixture.active.path.join(phase2::LAST_VERIFICATION_FILE)).unwrap();
    phase2::transition_state(
        &fixture.active.path,
        &fixture.active.id,
        phase2::WorkspaceState::BuildingConversations,
        phase2::StateCounts {
            discovered_candidates: Some(fixture.discovery.count),
            canonical_indexed: Some(fixture.stats.indexed),
        },
        Some("conversation_rebuild_running"),
    )
    .unwrap();
    let moved = fixture
        .active
        .pst_path
        .with_extension("temporarily-unmounted");
    fs::rename(&fixture.active.pst_path, &moved).unwrap();

    let recovered = verify_for_test(
        &fixture,
        "conversation_lineage_recovered",
        phase2::VerificationLevel::Quick,
    );
    assert_eq!(
        recovered.result,
        phase2::VerificationResult::SourceUnavailable
    );
    assert_eq!(recovered.workspace_state, phase2::WorkspaceState::Complete);
    fs::rename(moved, &fixture.active.pst_path).unwrap();
    fixture.source_and_extraction_unchanged();
}

#[test]
fn moved_source_with_same_identity_verifies_at_rediscovered_location() {
    let fixture = SyntheticWorkspace::valid("source-moved");
    let initial = verify_for_test(&fixture, "moved_initial", phase2::VerificationLevel::Quick);
    assert_eq!(initial.result, phase2::VerificationResult::Verified);
    let moved = fixture._root.path.join("rediscovered-fictional.pst");
    fs::rename(&fixture.active.pst_path, &moved).unwrap();
    let moved_active = ActiveWorkspace {
        id: fixture.active.id.clone(),
        path: fixture.active.path.clone(),
        pst_path: moved.clone(),
        fingerprint: fixture.active.fingerprint.clone(),
        location_mode: fixture.active.location_mode,
    };
    assert_eq!(
        phase2::source_probe_for_test(&moved_active, &fixture.manifest()),
        phase2::SourceIntegrity::Passed
    );
    let report = phase2::verify_active_workspace_for_test(
        &moved_active,
        "moved_rediscovered",
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    assert_eq!(report.result, phase2::VerificationResult::Verified);
    assert_eq!(report.source_integrity, phase2::SourceIntegrity::Passed);
    assert_eq!(sha256_file(&moved).unwrap(), fixture.source_hash);
    assert_eq!(fs::metadata(&moved).unwrap().len(), fixture.identity.size);
    assert!(fixture.index_path().is_file());
}

#[test]
fn source_mtime_and_fingerprint_changes_are_reported_without_index_deletion() {
    let fixture = SyntheticWorkspace::valid("source-change-kinds");
    let original_metadata = fs::metadata(&fixture.active.pst_path).unwrap();
    assert_eq!(
        verify_for_test(&fixture, "change_initial", phase2::VerificationLevel::Quick).result,
        phase2::VerificationResult::Verified
    );
    std::thread::sleep(Duration::from_millis(2));
    fs::write(&fixture.active.pst_path, SYNTHETIC_PST_BYTES).unwrap();
    assert_eq!(
        verify_for_test(&fixture, "mtime_changed", phase2::VerificationLevel::Quick).result,
        phase2::VerificationResult::SourceChanged
    );

    let mut same_size_changed = SYNTHETIC_PST_BYTES.to_vec();
    *same_size_changed.last_mut().unwrap() ^= 1;
    fs::write(&fixture.active.pst_path, same_size_changed).unwrap();
    let source_path =
        std::ffi::CString::new(fixture.active.pst_path.as_os_str().as_bytes().to_vec()).unwrap();
    let times = [
        libc::timespec {
            tv_sec: original_metadata.atime(),
            tv_nsec: original_metadata.atime_nsec(),
        },
        libc::timespec {
            tv_sec: original_metadata.mtime(),
            tv_nsec: original_metadata.mtime_nsec(),
        },
    ];
    // SAFETY: `source_path` is a live NUL-terminated buffer, `times` contains
    // exactly two initialized timespec values, and the synthetic file is owned
    // exclusively by this test.
    assert_eq!(
        unsafe { libc::utimensat(libc::AT_FDCWD, source_path.as_ptr(), times.as_ptr(), 0) },
        0
    );
    assert_eq!(
        verify_for_test(
            &fixture,
            "fingerprint_changed",
            phase2::VerificationLevel::Quick
        )
        .result,
        phase2::VerificationResult::SourceChanged
    );
    assert!(fixture.index_path().is_file());
}

#[test]
fn missing_extraction_is_reported_without_discarding_indexed_messages() {
    let fixture = SyntheticWorkspace::valid("missing-extraction");
    fs::remove_file(&fixture.discovery.candidates[0].path).unwrap();
    let report = verify_for_test(
        &fixture,
        "missing_extraction",
        phase2::VerificationLevel::Quick,
    );
    assert_eq!(
        report.result,
        phase2::VerificationResult::ExtractionInconsistent
    );
    assert_eq!(report.workspace_state, phase2::WorkspaceState::Incomplete);
    assert_eq!(
        Connection::open(fixture.index_path())
            .unwrap()
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        fixture.stats.indexed as u64
    );
    fixture.source_unchanged();
}

#[test]
fn unexpected_extracted_candidate_is_reported_without_automatic_indexing() {
    let fixture = SyntheticWorkspace::valid("unexpected-extraction");
    let unexpected = fixture
        .active
        .path
        .join("extracted")
        .join("Unexpected/new-record.eml");
    create_private_directory_all(unexpected.parent().unwrap()).unwrap();
    fs::write(unexpected, SECOND_EML).unwrap();
    let report = verify_for_test(
        &fixture,
        "unexpected_extraction",
        phase2::VerificationLevel::Quick,
    );
    assert_eq!(
        report.result,
        phase2::VerificationResult::ExtractionInconsistent
    );
    assert_eq!(
        Connection::open(fixture.index_path())
            .unwrap()
            .query_row("SELECT COUNT(*) FROM messages", [], |row| row
                .get::<_, u64>(0))
            .unwrap(),
        fixture.stats.indexed as u64
    );
    fixture.source_unchanged();
}

#[test]
fn missing_and_unexpected_canonical_rows_are_classified_separately() {
    let missing = SyntheticWorkspace::valid("missing-canonical");
    let missing_path = &missing.discovery.candidates[0].relative_path;
    let conn = open_sqlite_write_connection(&missing.index_path()).unwrap();
    let id = conn
        .query_row(
            "SELECT id FROM messages WHERE eml_path = ?1",
            params![missing_path],
            |row| row.get::<_, i64>(0),
        )
        .unwrap();
    conn.execute("DELETE FROM messages_fts WHERE rowid = ?1", params![id])
        .unwrap();
    conn.execute("DELETE FROM messages WHERE id = ?1", params![id])
        .unwrap();
    checked_truncate_checkpoint(&conn).unwrap();
    drop(conn);
    let report = verify_for_test(
        &missing,
        "missing_canonical",
        phase2::VerificationLevel::Quick,
    );
    assert_eq!(report.result, phase2::VerificationResult::Repairable);
    assert_eq!(report.missing_canonical_records, 1);
    assert_eq!(report.unexpected_canonical_records, 0);

    let unexpected = SyntheticWorkspace::valid("unexpected-canonical");
    let conn = open_sqlite_write_connection(&unexpected.index_path()).unwrap();
    let folder_id = conn
        .query_row("SELECT id FROM folders ORDER BY id LIMIT 1", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    conn.execute(
        "INSERT INTO messages (
             id, folder_id, eml_path, subject, sender, recipients, date, body,
             body_source, snippet, attachment_names, has_attachments,
             normalized_subject, conversation_id, conversation_root_id,
             thread_assignment_method
         ) VALUES (
             9001, ?1, 'Unexpected/record.eml', 'Synthetic', 'sender@example.test',
             'recipient@example.test', '2026-01-01T00:00:00Z', 'Synthetic',
             'text_plain', 'Synthetic', '', 0, 'Synthetic', 'opaque-conversation',
             9001, 'standalone'
         )",
        params![folder_id],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO messages_fts (rowid, subject, sender, recipients, body, attachment_names)
         SELECT id, subject, sender, recipients, body, attachment_names
           FROM messages WHERE id = 9001",
        [],
    )
    .unwrap();
    checked_truncate_checkpoint(&conn).unwrap();
    drop(conn);
    let report = verify_for_test(
        &unexpected,
        "unexpected_canonical",
        phase2::VerificationLevel::Quick,
    );
    assert_eq!(
        report.result,
        phase2::VerificationResult::DatabaseInconsistent
    );
    assert_eq!(report.missing_canonical_records, 0);
    assert_eq!(report.unexpected_canonical_records, 1);
}

#[test]
fn database_provenance_and_attachment_totals_are_bound_to_the_manifest() {
    let fixture = SyntheticWorkspace::valid("database-provenance");
    assert_eq!(
        verify_for_test(
            &fixture,
            "provenance_baseline",
            phase2::VerificationLevel::Quick,
        )
        .result,
        phase2::VerificationResult::Verified
    );
    let attachment_total = Connection::open(fixture.index_path())
        .unwrap()
        .query_row("SELECT COUNT(*) FROM attachments", [], |row| {
            row.get::<_, u64>(0)
        })
        .unwrap();
    let mutations = [
        ("workspace_id", "f".repeat(64)),
        ("pst_content_fingerprint", "e".repeat(64)),
        (
            "canonical_import_identity_version",
            "unsupported-canonical-identity".to_string(),
        ),
        (
            "importer_generation",
            "unsupported-importer-generation".to_string(),
        ),
        ("candidate_identity_manifest", "d".repeat(64)),
        (
            "canonical_attachment_total",
            attachment_total.saturating_add(1).to_string(),
        ),
        (
            "attachments_indexed",
            attachment_total.saturating_add(1).to_string(),
        ),
    ];

    for (ordinal, (key, replacement)) in mutations.into_iter().enumerate() {
        let conn = open_sqlite_write_connection(&fixture.index_path()).unwrap();
        let original = metadata_value(&conn, key).unwrap().unwrap();
        set_metadata_value(&conn, key, replacement).unwrap();
        checked_truncate_checkpoint(&conn).unwrap();
        drop(conn);
        let report = verify_for_test(
            &fixture,
            &format!("provenance_mismatch_{ordinal}"),
            phase2::VerificationLevel::Quick,
        );
        assert_eq!(
            report.result,
            phase2::VerificationResult::DatabaseInconsistent,
            "metadata provenance key {key} must fail closed"
        );

        let conn = open_sqlite_write_connection(&fixture.index_path()).unwrap();
        set_metadata_value(&conn, key, original).unwrap();
        checked_truncate_checkpoint(&conn).unwrap();
        drop(conn);
        assert_eq!(
            verify_for_test(
                &fixture,
                &format!("provenance_restored_{ordinal}"),
                phase2::VerificationLevel::Quick,
            )
            .result,
            phase2::VerificationResult::Verified
        );
    }
    fixture.source_and_extraction_unchanged();
}

#[test]
fn relational_fts_conversation_and_counter_inconsistencies_are_detected() {
    enum Mutation {
        MissingFolder,
        MissingAttachmentMessage,
        FtsMismatch,
        ConversationIncomplete,
        CounterMismatch,
    }
    for (ordinal, mutation) in [
        Mutation::MissingFolder,
        Mutation::MissingAttachmentMessage,
        Mutation::FtsMismatch,
        Mutation::ConversationIncomplete,
        Mutation::CounterMismatch,
    ]
    .into_iter()
    .enumerate()
    {
        let fixture = SyntheticWorkspace::valid(&format!("db-inconsistent-{ordinal}"));
        let conn = Connection::open(fixture.index_path()).unwrap();
        match mutation {
            Mutation::MissingFolder => {
                conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
                conn.execute("UPDATE messages SET folder_id = 999999 WHERE id = 1", [])
                    .unwrap();
            }
            Mutation::MissingAttachmentMessage => {
                conn.execute_batch("PRAGMA foreign_keys = OFF;").unwrap();
                conn.execute(
                    "INSERT INTO attachments (
                         message_id, filename, sanitized_filename, content_type,
                         size_bytes, attachment_index, content_disposition, mime_part_path
                     ) VALUES (999999, 'synthetic.bin', 'synthetic.bin',
                               'application/octet-stream', 1, 0, 'attachment', '1')",
                    [],
                )
                .unwrap();
            }
            Mutation::FtsMismatch => {
                conn.execute("DELETE FROM messages_fts WHERE rowid = 1", [])
                    .unwrap();
            }
            Mutation::ConversationIncomplete => {
                conn.execute(
                    "UPDATE messages SET conversation_id = NULL,
                                         conversation_root_id = NULL WHERE id = 1",
                    [],
                )
                .unwrap();
            }
            Mutation::CounterMismatch => {
                set_metadata_value(&conn, "candidates_processed", "999").unwrap();
            }
        }
        drop(conn);
        let report = verify_for_test(
            &fixture,
            &format!("db_inconsistent_{ordinal}"),
            phase2::VerificationLevel::Quick,
        );
        assert_eq!(
            report.result,
            phase2::VerificationResult::DatabaseInconsistent,
            "database mutation {ordinal} should fail verification"
        );
        assert!(fixture.index_path().is_file());
        fixture.source_unchanged();
    }
}

#[test]
fn malformed_sqlite_is_rejected_without_touching_source_or_extraction() {
    let fixture = SyntheticWorkspace::valid("sqlite-integrity");
    let corrupt = fixture.active.path.join("corrupt.sqlite");
    fs::copy(fixture.index_path(), &corrupt).unwrap();
    let file = OpenOptions::new().write(true).open(&corrupt).unwrap();
    file.set_len(128).unwrap();
    drop(file);
    let manifest = fixture.manifest();
    assert!(phase2::database_check_summary_for_test(&corrupt, &manifest).is_err());
    assert!(fixture.index_path().is_file());
    assert!(fixture.active.path.join("extracted").is_dir());
    fixture.source_unchanged();
}

#[test]
fn database_verification_cancellation_interrupts_sqlite_and_preserves_source() {
    let fixture = SyntheticWorkspace::valid("database-verification-cancel");
    let error =
        phase2::database_check_cancellation_for_test(&fixture.index_path(), &fixture.manifest())
            .expect_err("database verification should stop at the bounded cancellation gate");
    assert_eq!(error.code, Some(IMPORT_CANCELLED_CODE));
    fixture.source_and_extraction_unchanged();
}

#[test]
fn extraction_verification_cancellation_is_not_misclassified_as_corruption() {
    let fixture = SyntheticWorkspace::valid("extraction-verification-cancel");
    let error = phase2::verify_active_workspace_cancellation_for_test(
        &fixture.active,
        "extraction_verification_cancel",
    )
    .expect_err("extraction verification should propagate cancellation");
    assert_eq!(error.code, Some(IMPORT_CANCELLED_CODE));
    fixture.source_and_extraction_unchanged();
}

#[test]
fn committed_import_resume_repair_and_recovery_ignore_late_cancellation() {
    for label in [
        "committed-initial-import",
        "committed-resume-in-place",
        "committed-resume-replacement",
        "committed-repair",
        "committed-conversation-repair",
        "committed-repair-recovery",
        "committed-resume-recovery",
    ] {
        let fixture = SyntheticWorkspace::valid(label);
        let report = phase2::verify_active_workspace_cancel_policy_for_test(
            &fixture.active,
            &format!("{label}-verification"),
            false,
        )
        .expect("a proven committed layout must complete verification despite a late cancel");
        assert!(matches!(
            report.result,
            phase2::VerificationResult::Verified
                | phase2::VerificationResult::SourceUnavailable
                | phase2::VerificationResult::SourceChanged
        ));
        fixture.source_unchanged();
    }

    let cancellable = SyntheticWorkspace::valid("precommit-recovery-cancellation");
    let error = phase2::verify_active_workspace_cancel_policy_for_test(
        &cancellable.active,
        "precommit-recovery-verification",
        true,
    )
    .expect_err("a pre-commit recovery check must still honor cancellation");
    assert_eq!(error.code, Some(IMPORT_CANCELLED_CODE));
    cancellable.source_unchanged();
}

#[test]
fn standalone_verification_terminalization_preserves_prior_complete_or_records_cancel() {
    let complete = SyntheticWorkspace::valid("verification-cancel-prior-complete");
    phase2::verify_active_workspace_for_test(
        &complete.active,
        "verification-cancel-prior-complete-setup",
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    let complete_start = phase2::read_state(&complete.active.path, &complete.active.id)
        .unwrap()
        .unwrap();
    phase2::transition_state(
        &complete.active.path,
        &complete.active.id,
        phase2::WorkspaceState::Verifying,
        phase2::StateCounts::default(),
        Some("verification_running"),
    )
    .unwrap();
    terminalize_standalone_verification_error(&complete.active, &complete_start, true).unwrap();
    assert_eq!(
        phase2::read_state(&complete.active.path, &complete.active.id)
            .unwrap()
            .unwrap()
            .state,
        phase2::WorkspaceState::Complete
    );
    complete.source_unchanged();

    let incomplete = SyntheticWorkspace::valid("verification-cancel-incomplete");
    phase2::transition_state(
        &incomplete.active.path,
        &incomplete.active.id,
        phase2::WorkspaceState::Incomplete,
        phase2::StateCounts::default(),
        Some("verification_incomplete"),
    )
    .unwrap();
    let incomplete_start = phase2::read_state(&incomplete.active.path, &incomplete.active.id)
        .unwrap()
        .unwrap();
    phase2::transition_state(
        &incomplete.active.path,
        &incomplete.active.id,
        phase2::WorkspaceState::Verifying,
        phase2::StateCounts::default(),
        Some("verification_running"),
    )
    .unwrap();
    terminalize_standalone_verification_error(&incomplete.active, &incomplete_start, true).unwrap();
    assert_eq!(
        phase2::read_state(&incomplete.active.path, &incomplete.active.id)
            .unwrap()
            .unwrap()
            .state,
        phase2::WorkspaceState::Cancelled
    );
    incomplete.source_unchanged();

    let recovered = SyntheticWorkspace::valid("verification-error-recovered-prior-complete");
    phase2::verify_active_workspace_for_test(
        &recovered.active,
        "verification-error-recovered-prior-complete-setup",
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    let interrupted = phase2::transition_state(
        &recovered.active.path,
        &recovered.active.id,
        phase2::WorkspaceState::Verifying,
        phase2::StateCounts::default(),
        Some("verification_running"),
    )
    .unwrap();
    let recovered_start = phase2::recover_interrupted_state(
        &recovered.active.path,
        &recovered.active.id,
        interrupted,
    )
    .unwrap();
    assert_eq!(recovered_start.state, phase2::WorkspaceState::Incomplete);
    assert!(phase2::state_has_complete_lineage(&recovered_start));
    phase2::transition_state(
        &recovered.active.path,
        &recovered.active.id,
        phase2::WorkspaceState::Verifying,
        phase2::StateCounts::default(),
        Some("verification_running"),
    )
    .unwrap();
    terminalize_standalone_verification_error(&recovered.active, &recovered_start, false).unwrap();
    assert_eq!(
        phase2::read_state(&recovered.active.path, &recovered.active.id)
            .unwrap()
            .unwrap()
            .state,
        phase2::WorkspaceState::Complete
    );
    recovered.source_unchanged();
}

#[test]
fn database_status_contradiction_fails_verification() {
    let fixture = SyntheticWorkspace::valid("status-contradiction");
    let conn = open_sqlite_write_connection(&fixture.index_path()).unwrap();
    set_metadata_value(&conn, "import_status", "failed").unwrap();
    drop(conn);
    let report = verify_for_test(
        &fixture,
        "status_contradiction",
        phase2::VerificationLevel::Quick,
    );
    assert_eq!(
        report.result,
        phase2::VerificationResult::DatabaseInconsistent
    );
    assert_eq!(report.workspace_state, phase2::WorkspaceState::Incomplete);
    assert!(fixture.index_path().is_file());
    fixture.source_unchanged();
}

#[test]
fn hostile_workspace_metadata_and_unknown_statuses_fail_closed_without_mutation() {
    use std::os::unix::fs::symlink;

    for (ordinal, mutation) in ["unknown_status", "malformed", "oversized", "symlink"]
        .into_iter()
        .enumerate()
    {
        let fixture = SyntheticWorkspace::valid(&format!("hostile-workspace-metadata-{ordinal}"));
        assert_eq!(
            verify_for_test(
                &fixture,
                &format!("metadata_baseline_{ordinal}"),
                phase2::VerificationLevel::Quick,
            )
            .result,
            phase2::VerificationResult::Verified
        );
        let metadata_path = fixture.active.path.join(WORKSPACE_METADATA_FILE);
        match mutation {
            "unknown_status" => {
                let mut metadata = raw_json(&metadata_path);
                metadata["import_status"] = Value::from("/private/hostile-status");
                write_raw_json(&metadata_path, &metadata);
            }
            "malformed" => fs::write(&metadata_path, b"{ malformed").unwrap(),
            "oversized" => fs::write(
                &metadata_path,
                vec![b' '; (MAX_WORKSPACE_METADATA_BYTES + 1) as usize],
            )
            .unwrap(),
            "symlink" => {
                let target = fixture._root.path.join("hostile-metadata-target.json");
                fs::write(&target, br#"{"import_status":"complete"}"#).unwrap();
                fs::remove_file(&metadata_path).unwrap();
                symlink(target, &metadata_path).unwrap();
            }
            _ => unreachable!(),
        }
        let state_path = fixture.active.path.join(phase2::IMPORT_STATE_FILE);
        let manifest_path = fixture.active.path.join(phase2::IMPORT_MANIFEST_FILE);
        let report_path = fixture.active.path.join(phase2::LAST_VERIFICATION_FILE);
        let log_path = import_log_path(&fixture.active.path);
        let state_before = fs::read(&state_path).unwrap();
        let manifest_before = fs::read(&manifest_path).unwrap();
        let report_before = fs::read(&report_path).unwrap();
        let log_before = fs::read(&log_path).unwrap();
        let index_before = sha256_file(&fixture.index_path()).unwrap();

        let snapshot = phase2::integrity_snapshot(
            &fixture.active.path,
            &fixture.active.id,
            Some(&fixture.active.pst_path),
        );
        assert_eq!(snapshot.classification, "unsupported");
        assert!(!snapshot.can_open);
        let report = phase2::verify_active_workspace_for_test(
            &fixture.active,
            &format!("metadata_hostile_{ordinal}"),
            phase2::VerificationLevel::Quick,
        )
        .unwrap();
        assert_eq!(report.result, phase2::VerificationResult::Unsupported);
        assert_eq!(fs::read(&state_path).unwrap(), state_before);
        assert_eq!(fs::read(&manifest_path).unwrap(), manifest_before);
        assert_eq!(fs::read(&report_path).unwrap(), report_before);
        assert_eq!(fs::read(&log_path).unwrap(), log_before);
        assert_eq!(sha256_file(&fixture.index_path()).unwrap(), index_before);
        fixture.source_and_extraction_unchanged();
    }

    let fixture = SyntheticWorkspace::valid("hostile-database-status");
    assert_eq!(
        verify_for_test(
            &fixture,
            "database_status_baseline",
            phase2::VerificationLevel::Quick,
        )
        .result,
        phase2::VerificationResult::Verified
    );
    let conn = open_sqlite_write_connection(&fixture.index_path()).unwrap();
    set_metadata_value(&conn, "import_status", "/private/hostile-status").unwrap();
    checked_truncate_checkpoint(&conn).unwrap();
    drop(conn);
    let state_path = fixture.active.path.join(phase2::IMPORT_STATE_FILE);
    let report_path = fixture.active.path.join(phase2::LAST_VERIFICATION_FILE);
    let state_before = fs::read(&state_path).unwrap();
    let report_before = fs::read(&report_path).unwrap();
    let index_before = sha256_file(&fixture.index_path()).unwrap();
    assert!(preflight_integrity_compatibility_status(&fixture.active.path, false, false).is_err());
    assert!(preflight_integrity_compatibility_status(&fixture.active.path, false, true).is_err());
    let report = phase2::verify_active_workspace_for_test(
        &fixture.active,
        "database_status_hostile",
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    assert_eq!(report.result, phase2::VerificationResult::Unsupported);
    assert_eq!(fs::read(&state_path).unwrap(), state_before);
    assert_eq!(fs::read(&report_path).unwrap(), report_before);
    assert_eq!(sha256_file(&fixture.index_path()).unwrap(), index_before);
    fixture.source_and_extraction_unchanged();
}

#[test]
fn hostile_workspace_metadata_is_rejected_before_legacy_state_adoption() {
    let fixture = SyntheticWorkspace::valid("hostile-metadata-before-adoption");
    let state_path = fixture.active.path.join(phase2::IMPORT_STATE_FILE);
    let manifest_path = fixture.active.path.join(phase2::IMPORT_MANIFEST_FILE);
    let report_path = fixture.active.path.join(phase2::LAST_VERIFICATION_FILE);
    fs::remove_file(&state_path).unwrap();
    fs::remove_file(&manifest_path).unwrap();
    assert!(!report_path.exists());

    let metadata_path = fixture.active.path.join(WORKSPACE_METADATA_FILE);
    write_raw_json(
        &metadata_path,
        &serde_json::json!({"import_status": "/private/hostile-status"}),
    );
    let metadata_before = fs::read(&metadata_path).unwrap();
    let marker_path = fixture.active.path.join(WORKSPACE_MARKER_FILE);
    let marker_before = fs::read(&marker_path).unwrap();
    let log_path = import_log_path(&fixture.active.path);
    let log_before = fs::read(&log_path).unwrap();
    let index_before = sha256_file(&fixture.index_path()).unwrap();

    let report = phase2::verify_active_workspace_for_test(
        &fixture.active,
        "hostile_metadata_before_adoption",
        phase2::VerificationLevel::Quick,
    )
    .unwrap();
    assert_eq!(report.result, phase2::VerificationResult::Unsupported);
    assert!(!state_path.exists());
    assert!(!manifest_path.exists());
    assert!(!report_path.exists());
    assert_eq!(fs::read(&metadata_path).unwrap(), metadata_before);
    assert_eq!(fs::read(&marker_path).unwrap(), marker_before);
    assert_eq!(fs::read(&log_path).unwrap(), log_before);
    assert_eq!(sha256_file(&fixture.index_path()).unwrap(), index_before);
    fixture.source_and_extraction_unchanged();
}

#[test]
fn command_status_preflight_allows_only_repairs_to_inspect_a_corrupt_active_index() {
    let fixture = SyntheticWorkspace::valid("corrupt-index-command-preflight");
    assert_eq!(
        verify_for_test(
            &fixture,
            "corrupt_preflight_baseline",
            phase2::VerificationLevel::Quick,
        )
        .result,
        phase2::VerificationResult::Verified
    );
    let corrupt_bytes = b"synthetic corrupt sqlite bytes";
    fs::write(fixture.index_path(), corrupt_bytes).unwrap();

    assert!(preflight_integrity_compatibility_status(&fixture.active.path, false, false).is_err());
    assert!(preflight_integrity_compatibility_status(&fixture.active.path, true, false).is_err());
    preflight_integrity_compatibility_status(&fixture.active.path, false, true)
        .expect("Repair may inspect an unreadable current index after strict file/status guards");
    assert_eq!(fs::read(fixture.index_path()).unwrap(), corrupt_bytes);
    fixture.source_and_extraction_unchanged();
}

#[test]
fn missing_or_mismatched_workspace_marker_fails_closed() {
    for ordinal in 0..2 {
        let fixture = SyntheticWorkspace::valid(&format!("marker-mismatch-{ordinal}"));
        let marker_path = fixture.active.path.join(WORKSPACE_MARKER_FILE);
        if ordinal == 0 {
            fs::remove_file(&marker_path).unwrap();
        } else {
            let mut marker = raw_json(&marker_path);
            marker["pst_content_fingerprint"] = Value::from("0".repeat(64));
            write_raw_json(&marker_path, &marker);
        }
        let state_path = fixture.active.path.join(phase2::IMPORT_STATE_FILE);
        let state_before = fs::read(&state_path).unwrap();
        let report = verify_for_test(
            &fixture,
            &format!("marker_mismatch_{ordinal}"),
            phase2::VerificationLevel::Quick,
        );
        assert_eq!(report.result, phase2::VerificationResult::Unsupported);
        assert_eq!(
            report.workspace_state,
            phase2::WorkspaceState::BuildingConversations
        );
        assert_eq!(fs::read(&state_path).unwrap(), state_before);
        assert!(fixture.index_path().is_file());
        fixture.source_and_extraction_unchanged();
    }
}

#[test]
fn malformed_manifest_aggregate_source_and_extraction_fields_fail_closed() {
    for (ordinal, mutate) in [
        "aggregate_digest",
        "source_after",
        "extraction_result",
        "candidate_digest",
        "timestamp",
        "fingerprint_strategy",
        "modified_ns",
        "unsafe_counter",
        "completed_relation",
        "fts_relation",
        "readpst_control",
    ]
    .into_iter()
    .enumerate()
    {
        let fixture = SyntheticWorkspace::valid(&format!("manifest-field-{ordinal}"));
        let path = fixture.active.path.join(phase2::IMPORT_MANIFEST_FILE);
        let mut value = raw_json(&path);
        match mutate {
            "aggregate_digest" => {
                value["candidateAggregateDigest"] = Value::from("0".repeat(64));
            }
            "source_after" => {
                value["sourceSizeAfter"] = Value::from(SYNTHETIC_PST_BYTES.len() as u64 + 1);
            }
            "extraction_result" => {
                value["extractionResult"] = Value::from("future_result");
            }
            "candidate_digest" => {
                value["candidateRecords"][0]["sha256"] = Value::from("g".repeat(64));
            }
            "timestamp" => {
                value["extractionCompletedAt"] = Value::from("2026-01-01T00:00:00+00:00");
            }
            "fingerprint_strategy" => {
                value["sourceFingerprintStrategy"] = Value::from("future-fingerprint-strategy");
            }
            "modified_ns" => {
                value["sourceModifiedNsBefore"] = Value::from("01");
                value["sourceModifiedNsAfter"] = Value::from("01");
            }
            "unsafe_counter" => {
                value["attachmentCount"] = Value::from(9_007_199_254_740_992_u64);
            }
            "completed_relation" => {
                value["insertedCount"] = Value::from(0);
                value["skippedExistingCount"] = Value::from(0);
            }
            "fts_relation" => {
                value["ftsRowCount"] = Value::from(0);
            }
            "readpst_control" => {
                value["readpstVersion"] = Value::from("hostile\nversion");
            }
            _ => unreachable!(),
        }
        write_raw_json(&path, &value);
        assert!(phase2::read_manifest(&fixture.active.path, &fixture.active.id).is_err());
        let state_path = fixture.active.path.join(phase2::IMPORT_STATE_FILE);
        let state_before = fs::read(&state_path).unwrap();
        let report = verify_for_test(
            &fixture,
            &format!("manifest_field_{ordinal}"),
            phase2::VerificationLevel::Quick,
        );
        assert_eq!(report.result, phase2::VerificationResult::Unsupported);
        assert_eq!(
            report.workspace_state,
            phase2::WorkspaceState::BuildingConversations
        );
        assert_eq!(fs::read(&state_path).unwrap(), state_before);
        assert!(fixture.index_path().is_file());
        fixture.source_unchanged();
    }
}

#[test]
fn duplicate_relations_and_required_indexes_are_enforced_or_detected() {
    let fixture = SyntheticWorkspace::valid("relation-enforcement");
    let conn = open_sqlite_write_connection(&fixture.index_path()).unwrap();
    let (folder_path, folder_name): (String, String) = conn
        .query_row(
            "SELECT path, name FROM folders ORDER BY id LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert!(conn
        .execute(
            "INSERT INTO folders (parent_id, path, name) VALUES (NULL, ?1, ?2)",
            params![folder_path, folder_name],
        )
        .is_err());
    let message_id = conn
        .query_row("SELECT id FROM messages ORDER BY id LIMIT 1", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap();
    for id in 0..2 {
        conn.execute(
            "INSERT INTO attachments (
                 message_id, filename, sanitized_filename, content_type,
                 size_bytes, attachment_index, content_disposition, mime_part_path
             ) VALUES (?1, ?2, ?2, 'application/octet-stream', 1, 7, 'attachment', ?3)",
            params![message_id, format!("synthetic-{id}.bin"), format!("1.{id}")],
        )
        .unwrap();
    }
    conn.execute(
        "UPDATE messages SET has_attachments = 1 WHERE id = ?1",
        params![message_id],
    )
    .unwrap();
    drop(conn);
    let report = verify_for_test(
        &fixture,
        "duplicate_attachment_relation",
        phase2::VerificationLevel::Quick,
    );
    assert_eq!(
        report.result,
        phase2::VerificationResult::DatabaseInconsistent
    );

    let missing_index = SyntheticWorkspace::valid("missing-required-index");
    let conn = open_sqlite_write_connection(&missing_index.index_path()).unwrap();
    conn.execute("DROP INDEX idx_messages_date", []).unwrap();
    drop(conn);
    let report = verify_for_test(
        &missing_index,
        "missing_required_index",
        phase2::VerificationLevel::Quick,
    );
    assert_eq!(
        report.result,
        phase2::VerificationResult::DatabaseInconsistent
    );
}

#[test]
fn possible_duplicate_diagnostics_are_informational_and_preserve_all_messages() {
    let fixture = SyntheticWorkspace::with_duplicates("possible-duplicates");
    let manifest = fixture.manifest();
    let (groups, details) =
        phase2::possible_duplicate_details_for_test(&fixture.active.id, &manifest);
    assert_eq!(groups, 1);
    assert_eq!(details.len(), 1);
    assert_eq!(details[0].len(), 2);
    assert!(details[0]
        .iter()
        .all(|identity| phase2::validate_relative_identity(identity)));

    let report = verify_for_test(
        &fixture,
        "possible_duplicates",
        phase2::VerificationLevel::Quick,
    );
    assert_eq!(report.result, phase2::VerificationResult::Verified);
    assert_eq!(report.possible_duplicate_groups, 1);
    assert!(report
        .warnings
        .iter()
        .any(|warning| warning == "Possible duplicate source records detected."));
    let serialized = serde_json::to_string(&report).unwrap();
    assert!(!serialized.contains(&fixture.active.path.display().to_string()));
    assert!(!serialized.contains(&fixture.active.pst_path.display().to_string()));
    assert!(!serialized.contains(".eml"));
    assert!(!serialized.contains("Synthetic body"));
    assert!(!serialized.contains("Fictional status"));
    assert!(!serialized.contains("ada@example.test"));
    assert!(!serialized.to_ascii_lowercase().contains("safe to delete"));
    let report_json = serde_json::to_value(&report).unwrap();
    let default_details = report_json["possibleDuplicateGroupDetails"]
        .as_array()
        .expect("opaque duplicate diagnostics should be present");
    assert_eq!(default_details.len(), 1);
    assert!(default_details[0]["opaqueId"]
        .as_str()
        .is_some_and(is_lower_hex_sha256));
    assert_eq!(
        default_details[0]["signals"][0],
        "identical_extracted_content_digest"
    );
    assert!(default_details[0]
        .get("relativeWorkspaceIdentities")
        .is_none());
    let conn = open_sqlite_read_only_connection(&fixture.index_path()).unwrap();
    let canonical = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |row| {
            row.get::<_, u64>(0)
        })
        .unwrap();
    let distinct_paths = conn
        .query_row("SELECT COUNT(DISTINCT eml_path) FROM messages", [], |row| {
            row.get::<_, u64>(0)
        })
        .unwrap();
    let blank_visible = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE eml_path = 'Archive/no-subject.eml'",
            [],
            |row| row.get::<_, u64>(0),
        )
        .unwrap();
    assert_eq!(canonical, fixture.discovery.count as u64);
    assert_eq!(distinct_paths, canonical);
    assert_eq!(blank_visible, 1);
    assert_eq!(
        metadata_value(&conn, "structurally_empty_artifacts_suppressed")
            .unwrap()
            .as_deref(),
        Some("0")
    );
    fixture.source_unchanged();
}

#[test]
fn canonical_identity_uniqueness_prevents_same_path_duplication() {
    let fixture = SyntheticWorkspace::valid("canonical-uniqueness");
    let conn = open_sqlite_write_connection(&fixture.index_path()).unwrap();
    let result = conn.execute(
        "INSERT INTO messages (folder_id, eml_path, body_source, thread_assignment_method)
         SELECT folder_id, eml_path, 'missing', 'standalone' FROM messages LIMIT 1",
        [],
    );
    assert!(result.is_err());
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        fixture.discovery.count as u64
    );
}

#[test]
fn resume_indexes_only_missing_records_preserves_ids_and_is_idempotent() {
    let fixture = SyntheticWorkspace::valid("resume-idempotence");
    let index_path = fixture.index_path();
    let before = read_stable_message_ids(&index_path).unwrap();
    let removed_path = fixture.discovery.candidates[2].relative_path.clone();
    let removed_id = before[&removed_path];
    let mut conn = open_sqlite_write_connection(&index_path).unwrap();
    conn.execute(
        "DELETE FROM messages_fts WHERE rowid = ?1",
        params![removed_id],
    )
    .unwrap();
    conn.execute("DELETE FROM messages WHERE id = ?1", params![removed_id])
        .unwrap();
    prepare_import_metadata(
        &mut conn,
        &fixture.identity,
        &fixture.active.id,
        fixture.discovery.count,
        &fixture.active.path,
        WorkspaceLocationMode::AppSupport,
        "bundled-readpst",
        "synthetic-readpst-0",
        true,
        false,
        &fixture.discovery,
    )
    .unwrap();
    phase2::transition_state(
        &fixture.active.path,
        &fixture.active.id,
        phase2::WorkspaceState::Incomplete,
        phase2::StateCounts {
            discovered_candidates: Some(fixture.discovery.count),
            canonical_indexed: Some(fixture.discovery.count - 1),
        },
        Some("verification_required"),
    )
    .unwrap();
    phase2::transition_state(
        &fixture.active.path,
        &fixture.active.id,
        phase2::WorkspaceState::Indexing,
        phase2::StateCounts {
            discovered_candidates: Some(fixture.discovery.count),
            canonical_indexed: Some(fixture.discovery.count - 1),
        },
        Some("resume_indexing"),
    )
    .unwrap();
    let resumed = stream_index_eml_files_with_preferred_ids(
        &fixture.active.path,
        &fixture.active.path.join("extracted"),
        &mut conn,
        &fixture.discovery,
        2,
        "Synthetic cancellation.",
        |_| false,
        |_| {},
        Some(&before),
    )
    .unwrap();
    assert_eq!(resumed.inserted, 1);
    assert_eq!(resumed.skipped_existing, fixture.discovery.count - 1);
    assert_eq!(read_stable_message_ids(&index_path).unwrap(), before);

    let repeated = stream_index_eml_files_with_preferred_ids(
        &fixture.active.path,
        &fixture.active.path.join("extracted"),
        &mut conn,
        &fixture.discovery,
        2,
        "Synthetic cancellation.",
        |_| false,
        |_| {},
        Some(&before),
    )
    .unwrap();
    assert_eq!(repeated.inserted, 0);
    assert_eq!(repeated.skipped_existing, fixture.discovery.count);
    assert_eq!(read_stable_message_ids(&index_path).unwrap(), before);
    fixture.source_unchanged();
}

#[test]
fn resume_preserves_preexisting_parse_failure_count() {
    let fixture = SyntheticWorkspace::with_records(
        "resume-parse-failure",
        &[
            ("Inbox/valid.eml", NORMAL_EML),
            (
                "Inbox/malformed.eml",
                b" invalid-header\r\n\r\nsynthetic malformed payload\r\n",
            ),
        ],
    );
    assert_eq!(fixture.stats.error_count, 1);
    let mut conn = open_sqlite_write_connection(&fixture.index_path()).unwrap();
    prepare_import_metadata(
        &mut conn,
        &fixture.identity,
        &fixture.active.id,
        fixture.discovery.count,
        &fixture.active.path,
        WorkspaceLocationMode::AppSupport,
        "bundled-readpst",
        "synthetic-readpst-0",
        true,
        false,
        &fixture.discovery,
    )
    .unwrap();
    assert_eq!(
        metadata_value(&conn, "parse_failures").unwrap().as_deref(),
        Some("1")
    );
    phase2::transition_state(
        &fixture.active.path,
        &fixture.active.id,
        phase2::WorkspaceState::Incomplete,
        phase2::StateCounts {
            discovered_candidates: Some(fixture.discovery.count),
            canonical_indexed: Some(fixture.discovery.count),
        },
        Some("verification_required"),
    )
    .unwrap();
    phase2::transition_state(
        &fixture.active.path,
        &fixture.active.id,
        phase2::WorkspaceState::Indexing,
        phase2::StateCounts {
            discovered_candidates: Some(fixture.discovery.count),
            canonical_indexed: Some(fixture.discovery.count),
        },
        Some("resume_indexing"),
    )
    .unwrap();
    let stats = stream_index_eml_files_with_control(
        &fixture.active.path,
        &fixture.active.path.join("extracted"),
        &mut conn,
        &fixture.discovery,
        1,
        "Synthetic cancellation.",
        |_| false,
        |_| {},
    )
    .unwrap();
    assert_eq!(stats.error_count, 1);
    assert_eq!(stats.inserted, 0);
    assert_eq!(stats.skipped_existing, fixture.discovery.count);
    assert_eq!(
        metadata_value(&conn, "parse_failures").unwrap().as_deref(),
        Some("1")
    );
    fixture.source_and_extraction_unchanged();
}

#[test]
fn large_message_import_full_verify_and_resumed_repair_preserve_every_record() {
    use std::os::unix::fs::FileExt;

    const LARGE_PATH: &str = "Inbox/large.eml";
    let large_size = MAX_MESSAGE_SOURCE_BYTES + 4096;
    let fixture = SyntheticWorkspace::with_prepared_records(
        "large-message-lifecycle",
        &[(LARGE_PATH, NORMAL_EML), ("Inbox/normal.eml", SECOND_EML)],
        |extracted| {
            OpenOptions::new()
                .write(true)
                .open(extracted.join(LARGE_PATH))
                .unwrap()
                .set_len(large_size)
                .unwrap();
        },
    );
    assert_eq!(fixture.discovery.count, 2);
    assert_eq!(fixture.stats.indexed, 2);
    assert_eq!(fixture.stats.error_count, 0);
    assert_eq!(fixture.manifest().parse_failure_count, 0);
    let large_candidate = fixture
        .discovery
        .candidates
        .iter()
        .find(|candidate| candidate.relative_path == LARGE_PATH)
        .unwrap();
    assert_eq!(large_candidate.identity.size, large_size);
    let assert_large_headers = |index_path: &Path| {
        let conn = open_sqlite_read_only_connection(index_path).unwrap();
        let row = conn
            .query_row(
                "SELECT subject, sender, body_source, length(COALESCE(body_html, '')), message_id_header
                   FROM messages WHERE eml_path = ?1",
                params![LARGE_PATH],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(row.0, "Fictional status");
        assert!(row.1.contains("ada@example.test"));
        assert_eq!(row.2, "large_message_headers");
        assert_eq!(row.3, 0);
        assert!(row.4.contains("phase-two-1@example.test"));
    };
    assert_large_headers(&fixture.index_path());
    let (retained, retained_identity) =
        read_eml_candidate_snapshot_with_progress(&large_candidate.path, |_| Ok(())).unwrap();
    assert_eq!(retained_identity, large_candidate.identity);
    assert!(retained.len() < 4096, "only headers are retained in memory");
    assert_eq!(eml_header_end(&retained, 0), Some(retained.len()));
    let preview = standalone_eml_message(&large_candidate.path).unwrap();
    assert_eq!(preview.subject, "Fictional status");
    assert_eq!(preview.body_source, BodySource::LargeMessageHeaders);
    assert!(preview.raw_source.is_empty());
    assert!(!preview.raw_source_available);
    assert!(preview.attachments.is_empty());
    assert!(preview.cid_images.is_empty());
    assert!(preview.body_text.contains("Save Source As"));
    assert!(preview
        .parse_warnings
        .iter()
        .any(|warning| warning.contains("not parsed")));

    let full = verify_for_test(
        &fixture,
        "large_message_full_verify",
        phase2::VerificationLevel::Full,
    );
    assert_eq!(full.result, phase2::VerificationResult::Verified);
    assert_eq!(full.discovered_candidates, 2);
    assert_eq!(full.canonical_messages, 2);
    assert_eq!(full.parse_failures, 0);
    assert!(full.warnings.iter().any(|warning| {
        warning.starts_with("1 large message(s)")
            && warning.contains("bodies and individual attachments were not indexed")
    }));
    let cancelled = phase2::verify_active_workspace_cancellation_for_test(
        &fixture.active,
        "large_message_full_verify_cancel",
    )
    .expect_err("Full Verify should honor cancellation while hashing a large message");
    assert_eq!(cancelled.code, Some(IMPORT_CANCELLED_CODE));

    let active_hash = sha256_file(&fixture.index_path()).unwrap();
    let active_ids = read_stable_message_ids(&fixture.index_path()).unwrap();
    let repair_path = fixture.active.path.join("index.large-repair.sqlite");
    let mut conn = open_sqlite_write_connection(&repair_path).unwrap();
    initialize_schema(&conn).unwrap();
    insert_import_metadata(
        &mut conn,
        &fixture.identity,
        &fixture.active.id,
        fixture.discovery.count,
        &fixture.discovery.identity_manifest,
        &fixture.active.path,
        WorkspaceLocationMode::AppSupport,
        "bundled-readpst",
        "synthetic-readpst-0",
    )
    .unwrap();
    for stage in [
        phase2::WorkspaceState::Incomplete,
        phase2::WorkspaceState::Indexing,
    ] {
        phase2::transition_state(
            &fixture.active.path,
            &fixture.active.id,
            stage,
            phase2::StateCounts {
                discovered_candidates: Some(fixture.discovery.count),
                canonical_indexed: Some(0),
            },
            Some("synthetic_large_message_repair"),
        )
        .unwrap();
    }
    let cancelled = stream_index_eml_files_with_preferred_ids(
        &fixture.active.path,
        &fixture.active.path.join("extracted"),
        &mut conn,
        &fixture.discovery,
        1,
        "Synthetic large-message repair cancellation.",
        |stats| stats.processed >= 1,
        |_| {},
        Some(&active_ids),
    )
    .expect_err("repair should cancel after committing the large-message record");
    assert_eq!(cancelled.code, Some(IMPORT_CANCELLED_CODE));
    assert_large_headers(&repair_path);
    let resumed = stream_index_eml_files_with_preferred_ids(
        &fixture.active.path,
        &fixture.active.path.join("extracted"),
        &mut conn,
        &fixture.discovery,
        1,
        "Synthetic large-message repair cancellation.",
        |_| false,
        |_| {},
        Some(&active_ids),
    )
    .unwrap();
    assert_eq!(resumed.indexed, 2);
    assert_eq!(resumed.inserted, 1);
    assert_eq!(resumed.skipped_existing, 1);
    assert_eq!(resumed.error_count, 0);
    assert_eq!(read_stable_message_ids(&repair_path).unwrap(), active_ids);
    assert_large_headers(&repair_path);
    assert_eq!(sha256_file(&fixture.index_path()).unwrap(), active_hash);
    drop(conn);

    // The sparse tail is beyond the parser budget. Full Verify must still hash it.
    let large_file = OpenOptions::new()
        .write(true)
        .open(&large_candidate.path)
        .unwrap();
    large_file.write_all_at(b"X", large_size - 1).unwrap();
    large_file.sync_all().unwrap();
    let (found, mismatch) = phase2::scan_extraction_for_test(
        &fixture.active.path.join("extracted"),
        &fixture.manifest(),
        phase2::VerificationLevel::Full,
    )
    .expect("Full Verify must scan large messages without applying the parser limit");
    assert_eq!(found.len(), 2);
    assert!(
        mismatch,
        "Full Verify must detect changes beyond the preview budget"
    );
    large_file.write_all_at(b"\0", large_size - 1).unwrap();
    large_file.sync_all().unwrap();
    fixture.source_and_extraction_unchanged();
}

#[test]
fn repair_indexing_cancellation_leaves_active_index_and_can_resume() {
    let fixture = SyntheticWorkspace::valid("index-cancellation");
    let active_hash = sha256_file(&fixture.index_path()).unwrap();
    let active_ids = read_stable_message_ids(&fixture.index_path()).unwrap();
    let partial_path = fixture.active.path.join("index.cancel.sqlite");
    let mut conn = open_sqlite_write_connection(&partial_path).unwrap();
    initialize_schema(&conn).unwrap();
    insert_import_metadata(
        &mut conn,
        &fixture.identity,
        &fixture.active.id,
        fixture.discovery.count,
        &fixture.discovery.identity_manifest,
        &fixture.active.path,
        WorkspaceLocationMode::AppSupport,
        "bundled-readpst",
        "synthetic-readpst-0",
    )
    .unwrap();
    phase2::transition_state(
        &fixture.active.path,
        &fixture.active.id,
        phase2::WorkspaceState::Incomplete,
        phase2::StateCounts {
            discovered_candidates: Some(fixture.discovery.count),
            canonical_indexed: Some(0),
        },
        Some("verification_required"),
    )
    .unwrap();
    phase2::transition_state(
        &fixture.active.path,
        &fixture.active.id,
        phase2::WorkspaceState::Indexing,
        phase2::StateCounts {
            discovered_candidates: Some(fixture.discovery.count),
            canonical_indexed: Some(0),
        },
        Some("indexing_running"),
    )
    .unwrap();
    let cancelled = stream_index_eml_files_with_control(
        &fixture.active.path,
        &fixture.active.path.join("extracted"),
        &mut conn,
        &fixture.discovery,
        1,
        "Synthetic cancellation.",
        |stats| stats.processed >= 1,
        |_| {},
    );
    assert!(cancelled.is_err());
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        1
    );
    assert_eq!(sha256_file(&fixture.index_path()).unwrap(), active_hash);
    assert_eq!(
        read_stable_message_ids(&fixture.index_path()).unwrap(),
        active_ids
    );
    let resumed = stream_index_eml_files_with_control(
        &fixture.active.path,
        &fixture.active.path.join("extracted"),
        &mut conn,
        &fixture.discovery,
        2,
        "Synthetic cancellation.",
        |_| false,
        |_| {},
    )
    .unwrap();
    assert_eq!(resumed.indexed, fixture.discovery.count);
    assert_eq!(resumed.inserted, fixture.discovery.count - 1);
    assert_eq!(resumed.skipped_existing, 1);
    assert_eq!(sha256_file(&fixture.index_path()).unwrap(), active_hash);
    assert_eq!(
        read_stable_message_ids(&fixture.index_path()).unwrap(),
        active_ids
    );
    fixture.source_and_extraction_unchanged();
}

#[test]
fn conversation_only_rebuild_is_idempotent_cancellable_and_preserves_rows() {
    let fixture = SyntheticWorkspace::valid("conversation-rebuild");
    let mut conn = open_sqlite_write_connection(&fixture.index_path()).unwrap();
    let folder_rows_before = conn
        .query_row("SELECT COUNT(*) FROM folders", [], |row| {
            row.get::<_, u64>(0)
        })
        .unwrap();
    let attachment_rows_before = conn
        .query_row("SELECT COUNT(*) FROM attachments", [], |row| {
            row.get::<_, u64>(0)
        })
        .unwrap();
    let read_nonconversation = |conn: &Connection| {
        let mut statement = conn
            .prepare(
                "SELECT id, folder_id, eml_path, COALESCE(subject, ''),
                        COALESCE(sender, ''), COALESCE(recipients, ''),
                        COALESCE(body, ''), has_attachments FROM messages ORDER BY id",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, i64>(7)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let before = read_nonconversation(&conn);
    conn.execute(
        "UPDATE messages SET conversation_id = NULL, conversation_parent_id = NULL,
                             conversation_root_id = NULL",
        [],
    )
    .unwrap();
    assert!(apply_conversation_index(&mut conn, &fixture.active.id, || true).is_err());
    assert_eq!(
        conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE conversation_id IS NOT NULL",
            [],
            |row| row.get::<_, u64>(0),
        )
        .unwrap(),
        0
    );

    apply_conversation_index(&mut conn, &fixture.active.id, || false).unwrap();
    let read_conversation = |conn: &Connection| {
        let mut statement = conn
            .prepare(
                "SELECT id, conversation_id, conversation_parent_id, conversation_root_id,
                        thread_assignment_method, COALESCE(thread_warning, '')
                   FROM messages ORDER BY id",
            )
            .unwrap();
        statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let first = read_conversation(&conn);
    apply_conversation_index(&mut conn, &fixture.active.id, || false).unwrap();
    assert_eq!(read_conversation(&conn), first);
    assert_eq!(read_nonconversation(&conn), before);
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM folders", [], |row| row
            .get::<_, u64>(0))
            .unwrap(),
        folder_rows_before
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM attachments", [], |row| {
            row.get::<_, u64>(0)
        })
        .unwrap(),
        attachment_rows_before
    );
    fixture.source_and_extraction_unchanged();
}

#[test]
fn replacement_verification_accepts_only_complete_consistent_indexes() {
    let fixture = SyntheticWorkspace::valid("replacement-verification");
    let active_hash = sha256_file(&fixture.index_path()).unwrap();
    phase2::verify_replacement_index(
        &fixture.active.path,
        &fixture.active.id,
        &fixture.index_path(),
    )
    .expect("valid replacement should pass internal verification");

    let invalid = fixture.active.path.join("index.invalid.sqlite");
    fs::copy(fixture.index_path(), &invalid).unwrap();
    let conn = open_sqlite_write_connection(&invalid).unwrap();
    set_metadata_value(&conn, "candidates_processed", "0").unwrap();
    checked_truncate_checkpoint(&conn).unwrap();
    drop(conn);
    assert!(
        phase2::verify_replacement_index(&fixture.active.path, &fixture.active.id, &invalid)
            .is_err()
    );
    assert!(fixture.index_path().is_file());
    assert_eq!(sha256_file(&fixture.index_path()).unwrap(), active_hash);
    fixture.source_and_extraction_unchanged();
}

#[test]
fn checked_atomic_promotion_rolls_back_when_final_verification_fails() {
    let fixture = SyntheticWorkspace::valid("promotion-rollback");
    let index_path = fixture.index_path();
    let original_hash = sha256_file(&index_path).unwrap();
    let original_ids = read_stable_message_ids(&index_path).unwrap();
    let temp = fixture.active.path.join("index.reindex.tmp.sqlite");
    fs::copy(&index_path, &temp).unwrap();
    let conn = open_sqlite_write_connection(&temp).unwrap();
    set_metadata_value(&conn, "last_reindex_at", "2026-01-01T00:00:00Z").unwrap();
    checked_truncate_checkpoint(&conn).unwrap();
    drop(conn);
    let replacement_hash = sha256_file(&temp).unwrap();
    assert_ne!(replacement_hash, original_hash);

    fail_next_phase2_final_path_verification();
    let result = promote_completed_index(
        &fixture.active.path,
        &temp,
        &[fixture.active.id.as_str()],
        &fixture.active.fingerprint,
        &fixture.discovery,
    );
    assert!(result.is_err());
    assert_eq!(sha256_file(&index_path).unwrap(), original_hash);
    assert_eq!(read_stable_message_ids(&index_path).unwrap(), original_ids);
    assert_eq!(sha256_file(&temp).unwrap(), replacement_hash);
    fixture.source_and_extraction_unchanged();
}

#[test]
fn verified_atomic_promotion_swaps_then_retires_previous_database() {
    let fixture = SyntheticWorkspace::valid("promotion-success");
    let index_path = fixture.index_path();
    let original_ids = read_stable_message_ids(&index_path).unwrap();
    let temp = fixture.active.path.join("index.reindex.tmp.sqlite");
    for day in ["02", "03"] {
        fs::copy(&index_path, &temp).unwrap();
        let conn = open_sqlite_write_connection(&temp).unwrap();
        set_metadata_value(&conn, "last_reindex_at", format!("2026-01-{day}T00:00:00Z")).unwrap();
        checked_truncate_checkpoint(&conn).unwrap();
        drop(conn);
        promote_completed_index(
            &fixture.active.path,
            &temp,
            &[fixture.active.id.as_str()],
            &fixture.active.fingerprint,
            &fixture.discovery,
        )
        .expect("verified replacement should promote atomically");
        assert!(
            !temp.exists(),
            "previous database should retire only after verification"
        );
        assert_eq!(read_stable_message_ids(&index_path).unwrap(), original_ids);
        phase2::verify_replacement_index(&fixture.active.path, &fixture.active.id, &index_path)
            .unwrap();
    }
    fixture.source_and_extraction_unchanged();
}

#[test]
fn large_message_verification_warning_accepts_only_bounded_canonical_counts() {
    let fixture = SyntheticWorkspace::valid("large-message-warning-contract");
    let conn = open_sqlite_write_connection(&fixture.index_path()).unwrap();
    conn.execute(
        "UPDATE messages SET body_source = 'large_message_headers'
          WHERE id = (SELECT MIN(id) FROM messages)",
        [],
    )
    .unwrap();
    checked_truncate_checkpoint(&conn).unwrap();
    drop(conn);

    let report = verify_for_test(
        &fixture,
        "large_message_warning_contract",
        phase2::VerificationLevel::Quick,
    );
    assert_eq!(report.result, phase2::VerificationResult::Verified);
    let warning = report
        .warnings
        .iter()
        .find(|warning| warning.starts_with("1 large message(s)"))
        .expect("the verifier must generate a warning for the retained large record");
    assert!(phase2::get_last_verification(&fixture.active.path, &fixture.active.id).is_ok());
    let suffix = warning.strip_prefix('1').unwrap();
    let path = fixture.active.path.join(phase2::LAST_VERIFICATION_FILE);
    let valid = raw_json(&path);

    for prefix in [
        "",
        "0",
        "00",
        "01",
        "+1",
        "-1",
        " 1",
        "1 ",
        "1\n",
        "1e0",
        "1.0",
        "１",
        "18446744073709551616",
        "private@example.test",
        "1/private/source.pst",
    ] {
        let mut invalid = valid.clone();
        invalid["warnings"] = serde_json::json!([format!("{prefix}{suffix}")]);
        write_raw_json(&path, &invalid);
        assert!(
            phase2::get_last_verification(&fixture.active.path, &fixture.active.id).is_err(),
            "warning prefixes must be bounded canonical decimal counts"
        );
    }
    for changed_warning in [
        format!("{}{suffix}", report.canonical_messages + 1),
        format!("{warning}\nprivate@example.test"),
        format!("{warning} "),
        warning.replace("limited preview", "private@example.test"),
    ] {
        let mut invalid = valid.clone();
        invalid["warnings"] = serde_json::json!([changed_warning]);
        write_raw_json(&path, &invalid);
        assert!(phase2::get_last_verification(&fixture.active.path, &fixture.active.id).is_err());
    }

    let mut duplicate = valid.clone();
    duplicate["warnings"] = serde_json::json!([warning, format!("2{suffix}")]);
    write_raw_json(&path, &duplicate);
    assert!(phase2::get_last_verification(&fixture.active.path, &fixture.active.id).is_err());

    for (count, accepted) in [
        (1, true),
        (MAX_READPST_EXTRACTED_ENTRIES, true),
        (MAX_READPST_EXTRACTED_ENTRIES + 1, false),
    ] {
        let mut boundary = valid.clone();
        for field in [
            "canonicalMessages",
            "discoveredCandidates",
            "insertedMessages",
            "ftsRows",
        ] {
            boundary[field] = Value::from(count);
        }
        boundary["warnings"] = serde_json::json!([format!("{count}{suffix}")]);
        write_raw_json(&path, &boundary);
        assert_eq!(
            phase2::get_last_verification(&fixture.active.path, &fixture.active.id).is_ok(),
            accepted
        );
    }
    write_raw_json(&path, &valid);
    assert_eq!(
        phase2::get_last_verification(&fixture.active.path, &fixture.active.id)
            .unwrap()
            .unwrap(),
        report
    );
    fixture.source_and_extraction_unchanged();
}

#[test]
fn last_verification_rejects_malformed_and_unsupported_reports() {
    let fixture = SyntheticWorkspace::valid("last-verification-contract");
    let report = verify_for_test(&fixture, "last_report", phase2::VerificationLevel::Quick);
    assert_eq!(report.result, phase2::VerificationResult::Verified);
    let path = fixture.active.path.join(phase2::LAST_VERIFICATION_FILE);
    let mut value = raw_json(&path);
    value["formatVersion"] = Value::from(2);
    write_raw_json(&path, &value);
    assert!(phase2::get_last_verification(&fixture.active.path, &fixture.active.id).is_err());
    value["formatVersion"] = Value::from(1);
    value["operationId"] = Value::from("unsafe operation id");
    write_raw_json(&path, &value);
    assert!(phase2::get_last_verification(&fixture.active.path, &fixture.active.id).is_err());
}

#[test]
fn local_categorical_validation_reports_only_allowed_aggregates() {
    let fixture = SyntheticWorkspace::with_duplicates("categorical-validation");
    let initial_ids = read_stable_message_ids(&fixture.index_path()).unwrap();
    let initial = verify_for_test(
        &fixture,
        "categorical_initial",
        phase2::VerificationLevel::Quick,
    );
    let temp = fixture.active.path.join("index.reindex.tmp.sqlite");
    fs::copy(fixture.index_path(), &temp).unwrap();
    let conn = open_sqlite_write_connection(&temp).unwrap();
    set_metadata_value(&conn, "last_reindex_at", "2026-01-03T00:00:00Z").unwrap();
    checked_truncate_checkpoint(&conn).unwrap();
    drop(conn);
    let repair_success = promote_completed_index(
        &fixture.active.path,
        &temp,
        &[fixture.active.id.as_str()],
        &fixture.active.fingerprint,
        &fixture.discovery,
    )
    .is_ok();
    let stable_ids = read_stable_message_ids(&fixture.index_path()).unwrap() == initial_ids;
    let final_report = verify_for_test(
        &fixture,
        "categorical_final",
        phase2::VerificationLevel::Quick,
    );
    let snapshot = phase2::integrity_snapshot(
        &fixture.active.path,
        &fixture.active.id,
        Some(&fixture.active.pst_path),
    );
    assert_eq!(initial.result, phase2::VerificationResult::Verified);
    assert_eq!(final_report.result, phase2::VerificationResult::Verified);
    assert!(repair_success);
    assert!(stable_ids);
    println!(
        "state_classification={} verification_result=verified canonical_count={} missing_count={} possible_duplicate_group_count={} repair_success={} stable_id_result={} source_integrity=passed",
        snapshot.classification,
        final_report.canonical_messages,
        final_report.missing_canonical_records,
        final_report.possible_duplicate_groups,
        repair_success,
        if stable_ids { "passed" } else { "failed" },
    );
    fixture.source_and_extraction_unchanged();
}
