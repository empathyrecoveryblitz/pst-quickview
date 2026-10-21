use super::import_progress::*;
use std::{
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

struct SyntheticProgressTempDir {
    path: PathBuf,
}

impl SyntheticProgressTempDir {
    fn new(label: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "pst-quickview-progress-{label}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&path).expect("synthetic progress directory should be created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for SyntheticProgressTempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn base_snapshot(stage: ProgressStage, revision: u64) -> ImportProgressSnapshot {
    let status_code = match stage {
        ProgressStage::Preflight => ProgressStatusCode::PreflightRunning,
        ProgressStage::Extracting => ProgressStatusCode::ReadpstRunning,
        ProgressStage::Discovering => ProgressStatusCode::DiscoveringCandidates,
        ProgressStage::Indexing => ProgressStatusCode::IndexingCandidates,
        ProgressStage::BuildingConversations => ProgressStatusCode::BuildingConversations,
        ProgressStage::Verifying => ProgressStatusCode::VerifyingWorkspace,
        ProgressStage::Complete => ProgressStatusCode::ImportComplete,
    };
    let status = if stage == ProgressStage::Complete {
        ProgressStatus::Complete
    } else {
        ProgressStatus::Active
    };
    ImportProgressSnapshot {
        format_version: IMPORT_PROGRESS_FORMAT_VERSION,
        operation_id: "operation-7".to_string(),
        generation: 1,
        state_revision: revision,
        event_sequence: revision,
        workspace_id: "workspace-opaque-42".to_string(),
        source_basename: "1998\u{2013}2017 Archive.pst".to_string(),
        source_size_bytes: 51_200_000_000,
        workspace_storage_mode: WorkspaceStorageMode::NextToPst,
        operation_kind: OperationKind::NewImport,
        stage,
        completed_stages: if stage == ProgressStage::Complete {
            ProgressStage::ALL.to_vec()
        } else {
            ProgressStage::ALL[..stage.rank()].to_vec()
        },
        status,
        determinate: false,
        processed_units: None,
        total_units: None,
        unit_kind: None,
        counters: ProgressCounters::default(),
        active_elapsed_ms: 12_000,
        operation_elapsed_ms: 12_000,
        stage_elapsed_ms: 4_000,
        last_activity_utc: Some("2026-08-23T14:15:16.123Z".to_string()),
        last_activity_age_ms: Some(3_000),
        process_state: ProcessState::NotApplicable,
        eta: None,
        status_code,
        warning_codes: Vec::new(),
        stage_correction: None,
    }
}

fn determinate_indexing_snapshot(revision: u64) -> ImportProgressSnapshot {
    let mut snapshot = base_snapshot(ProgressStage::Indexing, revision);
    snapshot.determinate = true;
    snapshot.processed_units = Some(25);
    snapshot.total_units = Some(100);
    snapshot.unit_kind = Some(ProgressUnitKind::CandidateRecords);
    snapshot.counters.candidates_discovered = Some(100);
    snapshot.counters.candidates_processed = Some(25);
    snapshot.counters.inserted_messages = Some(20);
    snapshot.counters.skipped_existing_messages = Some(4);
    snapshot.counters.parse_failures = Some(1);
    snapshot
}

fn stable_samples() -> Vec<EtaSample> {
    vec![
        EtaSample {
            active_elapsed_ms: 0,
            completed_units: 0,
        },
        EtaSample {
            active_elapsed_ms: 4_000,
            completed_units: 4,
        },
        EtaSample {
            active_elapsed_ms: 8_000,
            completed_units: 8,
        },
        EtaSample {
            active_elapsed_ms: 12_000,
            completed_units: 12,
        },
    ]
}

#[test]
fn zero_progress_stays_in_estimating_state_before_stall_threshold() {
    let samples = [
        EtaSample {
            active_elapsed_ms: 0,
            completed_units: 0,
        },
        EtaSample {
            active_elapsed_ms: 5_000,
            completed_units: 0,
        },
    ];
    assert_eq!(
        estimate_eta(EtaInput {
            determinate: true,
            processed_units: 0,
            total_units: 100,
            active_elapsed_ms: 5_000,
            last_progress_age_ms: Some(5_000),
            samples: &samples,
        }),
        EtaStatus::Estimating
    );
}

#[test]
fn eta_waits_for_sample_unit_and_active_time_thresholds() {
    let samples = stable_samples();
    for short_samples in [&samples[..3], &samples[..4]] {
        let last = short_samples.last().unwrap();
        let status = estimate_eta(EtaInput {
            determinate: true,
            processed_units: last.completed_units,
            total_units: 100,
            active_elapsed_ms: last.active_elapsed_ms,
            last_progress_age_ms: Some(0),
            samples: short_samples,
        });
        if short_samples.len() < ETA_MIN_SAMPLES
            || last.completed_units < ETA_MIN_PROGRESS_UNITS
            || last.active_elapsed_ms < ETA_MIN_ACTIVE_MS
        {
            assert_eq!(status, EtaStatus::Estimating);
        } else {
            assert!(matches!(status, EtaStatus::Estimate(_)));
        }
    }
}

#[test]
fn stable_smoothed_rate_produces_a_finite_bounded_estimate() {
    let samples = stable_samples();
    let status = estimate_eta(EtaInput {
        determinate: true,
        processed_units: 12,
        total_units: 100,
        active_elapsed_ms: 12_000,
        last_progress_age_ms: Some(0),
        samples: &samples,
    });
    let EtaStatus::Estimate(range) = status else {
        panic!("stable rate should produce an estimate");
    };
    assert!(range.lower_bound_ms > 0);
    assert!(range.upper_bound_ms >= range.lower_bound_ms);
    assert!(range.upper_bound_ms <= ETA_MAX_ESTIMATE_MS);
    assert!((79_000..=97_000).contains(&range.lower_bound_ms));
}

#[test]
fn unstable_rate_suppresses_eta() {
    let samples = [
        EtaSample {
            active_elapsed_ms: 0,
            completed_units: 0,
        },
        EtaSample {
            active_elapsed_ms: 4_000,
            completed_units: 1,
        },
        EtaSample {
            active_elapsed_ms: 8_000,
            completed_units: 15,
        },
        EtaSample {
            active_elapsed_ms: 12_000,
            completed_units: 16,
        },
    ];
    assert_eq!(
        estimate_eta(EtaInput {
            determinate: true,
            processed_units: 16,
            total_units: 100,
            active_elapsed_ms: 12_000,
            last_progress_age_ms: Some(0),
            samples: &samples,
        }),
        EtaStatus::Hidden
    );
}

#[test]
fn stalled_progress_removes_a_previous_eta() {
    let samples = stable_samples();
    assert_eq!(
        estimate_eta(EtaInput {
            determinate: true,
            processed_units: 12,
            total_units: 100,
            active_elapsed_ms: 43_001,
            last_progress_age_ms: Some(31_001),
            samples: &samples,
        }),
        EtaStatus::Hidden
    );
}

#[test]
fn indeterminate_and_completed_stages_never_have_an_eta() {
    let samples = stable_samples();
    assert_eq!(
        estimate_eta(EtaInput {
            determinate: false,
            processed_units: 12,
            total_units: 100,
            active_elapsed_ms: 12_000,
            last_progress_age_ms: Some(0),
            samples: &samples,
        }),
        EtaStatus::Hidden
    );
    assert_eq!(
        estimate_eta(EtaInput {
            determinate: true,
            processed_units: 100,
            total_units: 100,
            active_elapsed_ms: 12_000,
            last_progress_age_ms: Some(0),
            samples: &samples,
        }),
        EtaStatus::Hidden
    );
}

#[test]
fn invalid_samples_cannot_produce_negative_infinite_or_zero_eta() {
    let reversed = [
        EtaSample {
            active_elapsed_ms: 10_000,
            completed_units: 20,
        },
        EtaSample {
            active_elapsed_ms: 9_000,
            completed_units: 21,
        },
        EtaSample {
            active_elapsed_ms: 11_000,
            completed_units: 22,
        },
        EtaSample {
            active_elapsed_ms: 12_000,
            completed_units: 23,
        },
    ];
    assert_eq!(
        estimate_eta(EtaInput {
            determinate: true,
            processed_units: 23,
            total_units: 100,
            active_elapsed_ms: 12_000,
            last_progress_age_ms: Some(0),
            samples: &reversed,
        }),
        EtaStatus::Hidden
    );

    let too_slow = [
        EtaSample {
            active_elapsed_ms: 0,
            completed_units: 0,
        },
        EtaSample {
            active_elapsed_ms: 3_000_000_000,
            completed_units: 4,
        },
        EtaSample {
            active_elapsed_ms: 6_000_000_000,
            completed_units: 8,
        },
        EtaSample {
            active_elapsed_ms: 9_000_000_000,
            completed_units: 12,
        },
    ];
    assert_eq!(
        estimate_eta(EtaInput {
            determinate: true,
            processed_units: 12,
            total_units: MAX_SAFE_JSON_INTEGER,
            active_elapsed_ms: 9_000_000_000,
            last_progress_age_ms: Some(0),
            samples: &too_slow,
        }),
        EtaStatus::Hidden
    );
}

#[test]
fn resume_accumulates_prior_active_time_and_excludes_downtime() {
    let checkpoint = TimingCheckpoint {
        active_elapsed_ms: 20_000,
        stage_elapsed_ms: 5_000,
        completion_duration_ms: None,
    };
    let mut timing = ActiveTiming::resume(checkpoint, 1_000_000).unwrap();
    assert_eq!(timing.active_session_start_tick_ms(), Some(1_000_000));
    let snapshot = timing.snapshot(1_003_000);
    assert_eq!(snapshot.active_elapsed_ms, 23_000);
    assert_eq!(snapshot.stage_elapsed_ms, 8_000);
}

#[test]
fn tick_rollback_and_age_calculation_saturate_without_negative_time() {
    let mut timing = ActiveTiming::start(10_000);
    assert_eq!(timing.snapshot(9_000).active_elapsed_ms, 0);
    assert_eq!(timing.snapshot(11_500).active_elapsed_ms, 1_500);
    assert_eq!(monotonic_age_ms(5, 10), 0);

    let checkpoint = TimingCheckpoint {
        active_elapsed_ms: MAX_SAFE_JSON_INTEGER - 5,
        stage_elapsed_ms: 10,
        completion_duration_ms: None,
    };
    let mut timing = ActiveTiming::resume(checkpoint, 0).unwrap();
    let saturated = timing.snapshot(u64::MAX);
    assert_eq!(saturated.active_elapsed_ms, MAX_SAFE_JSON_INTEGER);
    assert_eq!(saturated.stage_elapsed_ms, MAX_SAFE_JSON_INTEGER);
}

#[test]
fn real_stage_transition_resets_only_stage_elapsed() {
    let mut timing = ActiveTiming::start(100);
    let before = timing.snapshot(5_100);
    assert_eq!(before.active_elapsed_ms, 5_000);
    assert_eq!(before.stage_elapsed_ms, 5_000);

    let transitioned = timing.transition_stage(6_100);
    assert_eq!(transitioned.active_elapsed_ms, 6_000);
    assert_eq!(transitioned.stage_elapsed_ms, 0);
    assert_eq!(timing.stage_session_start_tick_ms(), Some(6_100));

    let after = timing.snapshot(8_100);
    assert_eq!(after.active_elapsed_ms, 8_000);
    assert_eq!(after.stage_elapsed_ms, 2_000);
}

#[test]
fn pause_and_completion_freeze_elapsed_values() {
    let mut timing = ActiveTiming::start(0);
    let paused = timing.pause(5_000);
    assert_eq!(paused.active_elapsed_ms, 5_000);
    assert_eq!(timing.snapshot(50_000).active_elapsed_ms, 5_000);

    let mut resumed = ActiveTiming::resume(paused, 100_000).unwrap();
    let completed = resumed.complete(107_000);
    assert_eq!(completed.active_elapsed_ms, 12_000);
    assert_eq!(completed.completion_duration_ms, Some(12_000));
    assert_eq!(resumed.snapshot(200_000).active_elapsed_ms, 12_000);
    assert_eq!(resumed.complete(300_000), completed);
}

#[test]
fn completed_checkpoint_cannot_resume_or_count_downtime() {
    let checkpoint = TimingCheckpoint {
        active_elapsed_ms: 18_032,
        stage_elapsed_ms: 2_000,
        completion_duration_ms: Some(18_032),
    };
    let mut timing = ActiveTiming::resume(checkpoint, 500_000).unwrap();
    let snapshot = timing.snapshot(900_000);
    assert!(!snapshot.running);
    assert_eq!(snapshot.active_elapsed_ms, 18_032);
    assert_eq!(snapshot.completion_duration_ms, Some(18_032));
}

#[test]
fn duration_and_eta_display_round_to_useful_precision() {
    assert_eq!(format_compact_duration(999), "0s");
    assert_eq!(format_compact_duration(3_999), "3s");
    assert_eq!(format_compact_duration(188_000), "3m 08s");
    assert_eq!(format_compact_duration(3_721_000), "1h 02m");
    assert_eq!(
        format_eta_range(EtaRange::new(20_000, 50_000).unwrap()).as_deref(),
        Some("Less than a minute remaining")
    );
    assert_eq!(
        format_eta_range(EtaRange::new(430_000, 490_000).unwrap()).as_deref(),
        Some("About 8 minutes remaining")
    );
    assert_eq!(
        format_eta_range(EtaRange::new(3_700_000, 6_900_000).unwrap()).as_deref(),
        Some("Estimated 1\u{2013}2 hours remaining")
    );
}

#[test]
fn exact_stage_history_and_determinate_rules_are_enforced() {
    for (index, stage) in ProgressStage::ALL.into_iter().enumerate() {
        let snapshot = base_snapshot(stage, index as u64 + 1);
        assert!(snapshot.validate().is_ok());
    }

    let mut invalid = base_snapshot(ProgressStage::Indexing, 10);
    invalid.completed_stages = vec![ProgressStage::Extracting, ProgressStage::Preflight];
    assert_eq!(
        invalid.validate(),
        Err(ContractError::InvalidCompletedStages)
    );
    let mut future_evidence = base_snapshot(ProgressStage::Extracting, 10);
    future_evidence.completed_stages = vec![ProgressStage::Discovering];
    assert_eq!(
        future_evidence.validate(),
        Err(ContractError::InvalidCompletedStages)
    );
    let mut self_completed = base_snapshot(ProgressStage::Preflight, 10);
    self_completed.completed_stages = vec![ProgressStage::Preflight];
    assert_eq!(
        self_completed.validate(),
        Err(ContractError::InvalidCompletedStages)
    );
    let mut interrupted = base_snapshot(ProgressStage::Indexing, 10);
    interrupted.status = ProgressStatus::Incomplete;
    interrupted.status_code = ProgressStatusCode::ImportIncomplete;
    interrupted.completed_stages = vec![ProgressStage::Indexing];
    assert_eq!(
        interrupted.validate(),
        Err(ContractError::InvalidCompletedStages)
    );
    let mut gapped = base_snapshot(ProgressStage::Indexing, 10);
    gapped.completed_stages = vec![ProgressStage::Preflight, ProgressStage::Discovering];
    assert!(gapped.validate().is_ok());
    let mut legacy_verification = base_snapshot(ProgressStage::Verifying, 10);
    legacy_verification.completed_stages.clear();
    assert!(legacy_verification.validate().is_ok());

    let indexing = determinate_indexing_snapshot(11);
    assert!(indexing.validate().is_ok());
    let mut extraction = indexing;
    extraction.stage = ProgressStage::Extracting;
    extraction.completed_stages = vec![ProgressStage::Preflight];
    extraction.status_code = ProgressStatusCode::ReadpstRunning;
    extraction.eta = Some(EtaRange::new(1_000, 2_000).unwrap());
    assert_eq!(
        extraction.validate(),
        Err(ContractError::EtaForUnsuitableStage)
    );
}

#[test]
fn ordering_rejects_stale_wrong_identity_and_unexplained_regression() {
    let current = base_snapshot(ProgressStage::Verifying, 10);
    let mut next = base_snapshot(ProgressStage::Verifying, 11);
    assert_eq!(
        classify_update(Some(&current), &next),
        UpdateDecision::Accept
    );

    let duplicate = current.clone();
    assert_eq!(
        classify_update(Some(&current), &duplicate),
        UpdateDecision::AcceptDuplicate
    );

    next.state_revision = 10;
    assert_eq!(
        classify_update(Some(&current), &next),
        UpdateDecision::Accept
    );
    next.event_sequence = current.event_sequence - 1;
    assert_eq!(
        classify_update(Some(&current), &next),
        UpdateDecision::RejectStaleRevision
    );
    next.state_revision = 11;
    next.event_sequence = 11;
    next.workspace_id = "another-workspace".to_string();
    assert_eq!(
        classify_update(Some(&current), &next),
        UpdateDecision::RejectDifferentWorkspace
    );

    let backward = base_snapshot(ProgressStage::Indexing, 11);
    assert_eq!(
        classify_update(Some(&current), &backward),
        UpdateDecision::RejectBackwardStage
    );

    let mut correction = backward;
    correction.stage_correction = Some(StageCorrection::VerificationReclassified);
    correction.status_code = ProgressStatusCode::StageCorrectedByVerification;
    assert_eq!(
        classify_update(Some(&current), &correction),
        UpdateDecision::RejectBackwardStage
    );

    correction.generation = 2;
    assert_eq!(
        classify_update(Some(&current), &correction),
        UpdateDecision::AcceptExplainedCorrection
    );
}

#[test]
fn a_new_generation_may_start_fresh_but_same_generation_operation_cannot_change() {
    let current = base_snapshot(ProgressStage::Verifying, 10);
    let mut fresh = base_snapshot(ProgressStage::Preflight, 1);
    fresh.generation = 2;
    fresh.operation_id = "operation-8".to_string();
    assert_eq!(
        classify_update(Some(&current), &fresh),
        UpdateDecision::AcceptNewGeneration
    );

    fresh.generation = 1;
    fresh.state_revision = 11;
    assert_eq!(
        classify_update(Some(&current), &fresh),
        UpdateDecision::RejectDifferentOperation
    );
}

#[test]
fn wire_contract_is_flat_bounded_fixed_shape_and_privacy_safe() {
    let mut snapshot = determinate_indexing_snapshot(25);
    snapshot.eta = EtaRange::new(60_000, 90_000);
    let event = ImportProgressEvent::new(snapshot).unwrap();
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.len() <= MAX_SERIALIZED_PROGRESS_BYTES);
    assert!(json.contains("\"formatVersion\":1"));
    assert!(json.contains("\"workspaceId\":\"workspace-opaque-42\""));
    assert!(json.contains("\"sourceBasename\":\"1998"));
    assert!(json.contains("\"workspaceBytesWritten\":null"));
    assert!(json.contains("\"lowerBoundMs\":60000"));
    assert!(!json.contains("/Users/"));
    assert!(!json.contains("sourcePath"));

    let decoded: ImportProgressEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, event);
    assert_eq!(decoded.snapshot().stage, ProgressStage::Indexing);
    assert_eq!(
        decoded.into_snapshot().source_basename,
        "1998\u{2013}2017 Archive.pst"
    );
}

#[test]
fn full_paths_unknown_fields_zero_revisions_and_unbounded_warnings_fail_closed() {
    let mut snapshot = base_snapshot(ProgressStage::Extracting, 1);
    snapshot.source_basename = "/private/fictional/Archive.pst".to_string();
    assert_eq!(
        ImportProgressEvent::new(snapshot).unwrap_err(),
        ContractError::InvalidSourceBasename
    );

    let mut snapshot = base_snapshot(ProgressStage::Extracting, 1);
    snapshot.generation = 0;
    assert_eq!(
        ImportProgressEvent::new(snapshot).unwrap_err(),
        ContractError::ZeroRevision
    );

    let event = ImportProgressEvent::new(base_snapshot(ProgressStage::Extracting, 1)).unwrap();
    let mut value = serde_json::to_value(event).unwrap();
    value
        .as_object_mut()
        .unwrap()
        .insert("sourcePath".to_string(), serde_json::json!("/private/path"));
    assert!(serde_json::from_value::<ImportProgressEvent>(value).is_err());

    let mut snapshot = base_snapshot(ProgressStage::Extracting, 1);
    snapshot.warning_codes = vec![ProgressWarningCode::SourceUnavailable; MAX_WARNING_CODES + 1];
    assert_eq!(
        ImportProgressEvent::new(snapshot).unwrap_err(),
        ContractError::TooManyWarnings
    );
}

#[test]
fn canonical_timestamps_and_counter_relationships_fail_closed() {
    let mut invalid_date = base_snapshot(ProgressStage::Extracting, 1);
    invalid_date.last_activity_utc = Some("2026-02-30T14:15:16.123Z".to_string());
    assert_eq!(
        invalid_date.validate(),
        Err(ContractError::InvalidUtcTimestamp)
    );

    let mut counters = determinate_indexing_snapshot(2);
    counters.counters.candidates_processed = Some(101);
    assert_eq!(
        counters.validate(),
        Err(ContractError::InconsistentCounters)
    );
}

#[test]
fn integrated_source_basename_strips_parent_paths_and_control_characters() {
    let path = Path::new("/synthetic/private/mailbox/1998\tArchive\n.pst");
    let basename = super::privacy_safe_source_basename(path);
    assert_eq!(basename, "1998Archive.pst");
    assert!(!basename.contains("synthetic"));
    assert!(!basename.contains("private"));
    assert!(!basename.chars().any(char::is_control));
    assert!(!basename.contains(['/', '\\']));

    assert_eq!(
        super::privacy_safe_source_basename(Path::new("/")),
        "Source.pst"
    );
}

#[test]
fn integrated_stage_mapping_does_not_fabricate_legacy_completion_evidence() {
    use super::import_integrity_phase2::WorkspaceState;

    assert_eq!(
        super::progress_stage_from_workspace_state(WorkspaceState::Incomplete),
        None
    );
    assert_eq!(
        super::progress_stage_from_workspace_state(WorkspaceState::Cancelled),
        None
    );
    assert_eq!(
        super::progress_stage_from_workspace_state(WorkspaceState::Failed),
        None
    );
    assert_eq!(
        super::progress_stage_from_legacy_label("Import incomplete"),
        None
    );
    assert_eq!(
        super::progress_stage_from_workspace_state(WorkspaceState::Complete),
        Some(ProgressStage::Complete)
    );

    assert_eq!(
        super::completed_progress_stage_for_transition(
            WorkspaceState::Incomplete,
            WorkspaceState::Verifying,
        ),
        None
    );
    assert_eq!(
        super::completed_progress_stage_for_transition(
            WorkspaceState::Discovering,
            WorkspaceState::Indexing,
        ),
        Some(ProgressStage::Discovering)
    );
}

#[test]
fn persisted_lineage_fills_completed_stage_when_intermediate_event_was_skipped() {
    use super::import_integrity_phase2::{StateCounts, WorkspaceState};

    let root = SyntheticProgressTempDir::new("skipped-stage-observation");
    let workspace_id = "d".repeat(64);
    let workspace = root.path().join(&workspace_id);
    super::create_private_directory_all(&workspace).unwrap();
    for (stage, reason) in [
        (WorkspaceState::Preflight, "preflight_running"),
        (WorkspaceState::Extracting, "extraction_running"),
        (WorkspaceState::ExtractionComplete, "extraction_complete"),
        (WorkspaceState::Discovering, "discovery_running"),
    ] {
        super::import_integrity_phase2::transition_state(
            &workspace,
            &workspace_id,
            stage,
            StateCounts::default(),
            Some(reason),
        )
        .unwrap();
    }
    let record = super::import_integrity_phase2::read_state(&workspace, &workspace_id)
        .unwrap()
        .unwrap();
    let mut completed = vec![ProgressStage::Preflight];
    super::merge_persisted_completed_stages(
        &mut completed,
        &record,
        OperationKind::ResumedImport,
        ProgressStage::Discovering,
    );
    assert_eq!(
        completed,
        vec![ProgressStage::Preflight, ProgressStage::Extracting]
    );
}

#[test]
fn repair_and_verification_generations_do_not_inherit_completed_future_stages() {
    use super::import_integrity_phase2::{StateCounts, WorkspaceState};

    let root = SyntheticProgressTempDir::new("operation-specific-stage-lineage");
    let workspace_id = "e".repeat(64);
    let workspace = root.path().join(&workspace_id);
    super::create_private_directory_all(&workspace).unwrap();
    for (stage, reason) in [
        (WorkspaceState::Preflight, "preflight_running"),
        (WorkspaceState::Extracting, "extraction_running"),
        (WorkspaceState::ExtractionComplete, "extraction_complete"),
        (WorkspaceState::Discovering, "discovery_running"),
        (WorkspaceState::Indexing, "indexing_running"),
        (
            WorkspaceState::BuildingConversations,
            "conversation_building",
        ),
        (WorkspaceState::Verifying, "verification_running"),
        (WorkspaceState::Complete, "verification_passed"),
    ] {
        super::import_integrity_phase2::transition_state(
            &workspace,
            &workspace_id,
            stage,
            StateCounts::default(),
            Some(reason),
        )
        .unwrap();
    }
    let record = super::import_integrity_phase2::read_state(&workspace, &workspace_id)
        .unwrap()
        .unwrap();

    assert!(super::initial_completed_progress_stages(
        OperationKind::Repair,
        ProgressStage::Preflight,
        &record,
        false,
    )
    .is_empty());
    assert_eq!(
        super::initial_completed_progress_stages(
            OperationKind::ResumedImport,
            ProgressStage::Discovering,
            &record,
            false,
        ),
        vec![ProgressStage::Preflight, ProgressStage::Extracting]
    );
    assert_eq!(
        super::initial_completed_progress_stages(
            OperationKind::Repair,
            ProgressStage::BuildingConversations,
            &record,
            false,
        ),
        vec![
            ProgressStage::Preflight,
            ProgressStage::Extracting,
            ProgressStage::Discovering,
            ProgressStage::Indexing,
        ]
    );
    assert_eq!(
        super::initial_completed_progress_stages(
            OperationKind::Verification,
            ProgressStage::Verifying,
            &record,
            false,
        ),
        vec![
            ProgressStage::Preflight,
            ProgressStage::Extracting,
            ProgressStage::Discovering,
            ProgressStage::Indexing,
            ProgressStage::BuildingConversations,
        ]
    );
    let mut incomplete = record.clone();
    incomplete.last_completed_stage = Some(WorkspaceState::Discovering);
    assert_eq!(
        super::initial_completed_progress_stages(
            OperationKind::Verification,
            ProgressStage::Verifying,
            &incomplete,
            false,
        ),
        vec![
            ProgressStage::Preflight,
            ProgressStage::Extracting,
            ProgressStage::Discovering,
        ]
    );
    assert_eq!(
        super::initial_completed_progress_stages(
            OperationKind::Repair,
            ProgressStage::BuildingConversations,
            &incomplete,
            false,
        ),
        vec![
            ProgressStage::Preflight,
            ProgressStage::Extracting,
            ProgressStage::Discovering,
        ]
    );

    let mut repair_completed = Vec::new();
    super::merge_persisted_completed_stages(
        &mut repair_completed,
        &record,
        OperationKind::Repair,
        ProgressStage::BuildingConversations,
    );
    assert!(repair_completed.is_empty());
    assert_eq!(
        super::completed_progress_stages_for_operation_transition(
            OperationKind::Repair,
            ProgressStage::Preflight,
            WorkspaceState::Complete,
            WorkspaceState::Discovering,
        ),
        vec![ProgressStage::Preflight, ProgressStage::Extracting]
    );
}

#[test]
fn new_import_and_repair_do_not_seed_prior_manifest_parse_failures() {
    assert_eq!(
        super::initial_progress_parse_failures(OperationKind::NewImport, Some(17)),
        None
    );
    assert_eq!(
        super::initial_progress_parse_failures(OperationKind::Repair, Some(17)),
        None
    );
    assert_eq!(
        super::initial_progress_parse_failures(OperationKind::ResumedImport, Some(17)),
        Some(17)
    );
    assert_eq!(
        super::initial_progress_parse_failures(OperationKind::Verification, Some(17)),
        Some(17)
    );
}

#[test]
fn integrated_index_counter_projection_preserves_candidate_and_canonical_meanings() {
    let stats = super::IndexStats {
        discovered: 101,
        processed: 72,
        indexed: 64,
        inserted: 61,
        skipped_existing: 3,
        suppressed_empty_artifacts: 0,
        error_count: 8,
        attachments_indexed: 19,
        last_error: None,
    };
    let counters = super::progress_counters_from_index_stats(&stats);

    assert_eq!(counters.candidates_discovered, Some(101));
    assert_eq!(counters.candidates_processed, Some(72));
    assert_eq!(counters.canonical_messages, Some(64));
    assert_eq!(counters.inserted_messages, Some(61));
    assert_eq!(counters.skipped_existing_messages, Some(3));
    assert_eq!(counters.parse_failures, Some(8));
    assert_eq!(counters.attachments_indexed, Some(19));
    assert_eq!(counters.output_files_observed, None);
    assert_eq!(counters.conversation_records_processed, None);

    let wire = serde_json::to_value(&counters).unwrap();
    assert_eq!(wire["candidatesDiscovered"], 101);
    assert_eq!(wire["candidatesProcessed"], 72);
    assert_eq!(wire["canonicalMessages"], 64);
    assert_eq!(wire["insertedMessages"], 61);
    assert_eq!(wire["skippedExistingMessages"], 3);
    assert_eq!(wire["parseFailures"], 8);
    assert_eq!(wire["attachmentsIndexed"], 19);
}

fn populated_counters(value: u64) -> ProgressCounters {
    ProgressCounters {
        output_files_observed: Some(value),
        output_bytes_observed: Some(value),
        workspace_bytes_written: Some(value),
        candidates_discovered: Some(value),
        candidates_processed: Some(value),
        canonical_messages: Some(value),
        inserted_messages: Some(value),
        skipped_existing_messages: Some(value),
        parse_failures: Some(value),
        attachments_indexed: Some(value),
        conversation_records_processed: Some(value),
        conversation_records_total: Some(value),
        verification_checks_completed: Some(value),
        verification_checks_total: Some(value),
        missing_canonical_records: Some(value),
        unexpected_canonical_records: Some(value),
        possible_duplicate_groups: Some(value),
    }
}

#[test]
fn integrated_counter_merge_never_decreases_or_nulls_prior_observations() {
    let mut current = populated_counters(10);
    let update = ProgressCounters {
        output_files_observed: None,
        output_bytes_observed: Some(9),
        workspace_bytes_written: Some(11),
        candidates_discovered: None,
        candidates_processed: Some(9),
        canonical_messages: Some(11),
        inserted_messages: None,
        skipped_existing_messages: Some(9),
        parse_failures: Some(11),
        attachments_indexed: None,
        conversation_records_processed: Some(9),
        conversation_records_total: Some(11),
        verification_checks_completed: None,
        verification_checks_total: Some(9),
        missing_canonical_records: Some(11),
        unexpected_canonical_records: None,
        possible_duplicate_groups: Some(9),
    };
    super::merge_progress_counters(&mut current, update);

    assert_eq!(
        current,
        ProgressCounters {
            output_files_observed: Some(10),
            output_bytes_observed: Some(10),
            workspace_bytes_written: Some(11),
            candidates_discovered: Some(10),
            candidates_processed: Some(10),
            canonical_messages: Some(11),
            inserted_messages: Some(10),
            skipped_existing_messages: Some(10),
            parse_failures: Some(11),
            attachments_indexed: Some(10),
            conversation_records_processed: Some(10),
            conversation_records_total: Some(11),
            verification_checks_completed: Some(10),
            verification_checks_total: Some(10),
            missing_canonical_records: Some(11),
            unexpected_canonical_records: Some(10),
            possible_duplicate_groups: Some(10),
        }
    );
}

#[test]
fn integrated_readpst_observation_counts_files_not_directories_and_sums_file_bytes() {
    let fixture = SyntheticProgressTempDir::new("readpst-observation");
    let nested = fixture.path().join("synthetic-folder");
    fs::create_dir(&nested).unwrap();
    fs::write(nested.join("candidate-one.eml"), b"abc").unwrap();
    fs::write(fixture.path().join("candidate-two.eml"), b"12345").unwrap();

    let snapshot = super::inspect_readpst_output(fixture.path(), 1_024).unwrap();
    assert_eq!(snapshot.entries, 3);
    assert_eq!(snapshot.files, 2);
    assert_eq!(snapshot.total_bytes, 8);
}

#[test]
fn quiet_running_readpst_is_not_misclassified_as_stalled() {
    assert_eq!(
        super::readpst_running_process_state(std::time::Duration::from_secs(45)),
        ProcessState::RunningNoRecentSignal
    );
    assert_eq!(
        super::readpst_running_process_state(std::time::Duration::from_secs(31 * 60)),
        ProcessState::RunningNoRecentSignal
    );
}

#[test]
fn terminal_lineage_distinguishes_interrupted_reverification_from_completed_work() {
    use super::import_integrity_phase2::WorkspaceState;

    assert!(
        super::restored_prior_complete_after_interrupted_verification(
            OperationKind::Verification,
            WorkspaceState::Verifying,
            WorkspaceState::Complete,
            ProgressStatus::Cancelled,
        )
    );
    assert!(
        super::restored_prior_complete_after_interrupted_verification(
            OperationKind::Verification,
            WorkspaceState::Verifying,
            WorkspaceState::Complete,
            ProgressStatus::Failed,
        )
    );
    assert!(
        !super::restored_prior_complete_after_interrupted_verification(
            OperationKind::Verification,
            WorkspaceState::Verifying,
            WorkspaceState::Complete,
            ProgressStatus::Complete,
        )
    );
    assert!(super::should_record_import_completion(
        true,
        true,
        WorkspaceState::Complete,
    ));
    assert!(!super::should_record_import_completion(
        false,
        true,
        WorkspaceState::Complete,
    ));
    assert!(!super::should_record_import_completion(
        true,
        true,
        WorkspaceState::Verifying,
    ));
    assert!(!super::should_record_import_completion(
        true,
        false,
        WorkspaceState::Complete,
    ));
    assert!(super::newly_committed_verification_completion_wins(
        WorkspaceState::Verifying,
        WorkspaceState::Complete,
        Some("verification_complete"),
    ));
    assert!(!super::newly_committed_verification_completion_wins(
        WorkspaceState::Verifying,
        WorkspaceState::Complete,
        Some("verification_failed_prior_complete_preserved"),
    ));
}

#[test]
fn terminal_process_state_requires_an_observed_extraction_child_exit() {
    assert_eq!(
        super::terminal_process_state(ProgressStage::Extracting, ProcessState::ExitedFailure),
        ProcessState::ExitedFailure
    );
    assert_eq!(
        super::terminal_process_state(ProgressStage::Extracting, ProcessState::ExitedSuccess),
        ProcessState::ExitedSuccess
    );
    for stage in [
        ProgressStage::Preflight,
        ProgressStage::Discovering,
        ProgressStage::Indexing,
        ProgressStage::BuildingConversations,
        ProgressStage::Verifying,
        ProgressStage::Complete,
    ] {
        assert_eq!(
            super::terminal_process_state(stage, ProcessState::ExitedFailure),
            ProcessState::NotApplicable
        );
    }
}
