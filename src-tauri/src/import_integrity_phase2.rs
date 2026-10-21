use super::*;
use chrono::SecondsFormat;
use rusqlite::ffi;
use serde::de::DeserializeOwned;
use std::ffi::c_void;
use std::path::Component;

pub(crate) const IMPORT_STATE_FILE: &str = "import-state.json";
pub(crate) const IMPORT_MANIFEST_FILE: &str = "import-manifest.json";
pub(crate) const LAST_VERIFICATION_FILE: &str = "last-verification.json";
pub(crate) const STAGED_IMPORT_MANIFEST_FILE: &str = "import-manifest.replacement.json";
const STATE_FORMAT_VERSION: u32 = 1;
pub(crate) const ACTIVE_TIMING_VERSION: u8 = 1;
const MANIFEST_FORMAT_VERSION: u32 = 1;
const VERIFICATION_FORMAT_VERSION: u32 = 1;
const MAX_STATE_BYTES: u64 = 128 * 1024;
const MAX_VERIFICATION_BYTES: u64 = 2 * 1024 * 1024;
const MAX_MANIFEST_BYTES: u64 = 512 * 1024 * 1024;
const MAX_DUPLICATE_DETAILS: usize = 100;
const MAX_OPERATION_ID_BYTES: usize = 128;
const MAX_SAFE_JSON_INTEGER: u64 = 9_007_199_254_740_991;
const READPST_INVOCATION_MODE: &str = "separate-eml:utf8:quiet";

#[cfg(test)]
std::thread_local! {
    static BEFORE_VERIFICATION_FINALIZATION_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
pub(crate) fn set_before_verification_finalization_hook_for_test<F>(hook: F)
where
    F: FnOnce() + 'static,
{
    BEFORE_VERIFICATION_FINALIZATION_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

#[cfg(test)]
fn run_before_verification_finalization_hook() {
    BEFORE_VERIFICATION_FINALIZATION_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(not(test))]
fn run_before_verification_finalization_hook() {}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkspaceState {
    Preflight,
    Extracting,
    ExtractionComplete,
    Discovering,
    Indexing,
    BuildingConversations,
    Verifying,
    Complete,
    Cancelled,
    Incomplete,
    Failed,
}

impl WorkspaceState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Preflight => "preflight",
            Self::Extracting => "extracting",
            Self::ExtractionComplete => "extraction_complete",
            Self::Discovering => "discovering",
            Self::Indexing => "indexing",
            Self::BuildingConversations => "building_conversations",
            Self::Verifying => "verifying",
            Self::Complete => "complete",
            Self::Cancelled => "cancelled",
            Self::Incomplete => "incomplete",
            Self::Failed => "failed",
        }
    }

    fn is_transient(self) -> bool {
        matches!(
            self,
            Self::Preflight
                | Self::Extracting
                | Self::ExtractionComplete
                | Self::Discovering
                | Self::Indexing
                | Self::BuildingConversations
                | Self::Verifying
        )
    }
}

pub(crate) fn state_has_complete_lineage(state: &WorkspaceImportState) -> bool {
    state.state == WorkspaceState::Complete
        || state.verified_complete_lineage
        || state.last_completed_stage == Some(WorkspaceState::Verifying)
        || state.completed_duration_ms.is_some()
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum OperationTimingKind {
    Repair,
    Verification,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct OperationTimingRecord {
    pub(crate) kind: OperationTimingKind,
    pub(crate) stage: WorkspaceState,
    pub(crate) generation: u64,
    pub(crate) active_elapsed_ms: u64,
    pub(crate) stage_elapsed_ms: u64,
}

fn valid_operation_timing_stage(kind: OperationTimingKind, stage: WorkspaceState) -> bool {
    match kind {
        OperationTimingKind::Verification => stage == WorkspaceState::Verifying,
        OperationTimingKind::Repair => matches!(
            stage,
            WorkspaceState::Preflight
                | WorkspaceState::Discovering
                | WorkspaceState::Indexing
                | WorkspaceState::BuildingConversations
                | WorkspaceState::Verifying
        ),
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct WorkspaceImportState {
    format_version: u32,
    workspace_id: String,
    pub(crate) state: WorkspaceState,
    pub(crate) revision: u64,
    created_at: String,
    pub(crate) updated_at: String,
    pub(crate) state_started_at: String,
    pub(crate) last_completed_stage: Option<WorkspaceState>,
    pub(crate) interrupted_stage: Option<WorkspaceState>,
    #[serde(default)]
    pub(crate) verified_complete_lineage: bool,
    pub(crate) discovered_candidates: u64,
    pub(crate) canonical_indexed: u64,
    pub(crate) remaining_candidates: Option<u64>,
    pub(crate) last_successful_update: String,
    pub(crate) elapsed_duration_ms: u64,
    #[serde(default)]
    pub(crate) active_timing_version: u8,
    #[serde(default)]
    pub(crate) active_timing_history_complete: bool,
    #[serde(default)]
    pub(crate) operation_generation: u64,
    #[serde(default)]
    pub(crate) active_session_started_at: Option<String>,
    #[serde(default)]
    pub(crate) active_session_updates_import_timing: bool,
    #[serde(default)]
    pub(crate) accumulated_active_elapsed_ms: u64,
    #[serde(default)]
    pub(crate) stage_elapsed_duration_ms: u64,
    #[serde(default)]
    pub(crate) last_activity_at: Option<String>,
    #[serde(default)]
    pub(crate) completed_duration_ms: Option<u64>,
    #[serde(default)]
    pending_completion_duration_ms: Option<u64>,
    #[serde(default)]
    pub(crate) operation_timing: Option<OperationTimingRecord>,
    pub(crate) reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct StateCounts {
    pub(crate) discovered_candidates: Option<usize>,
    pub(crate) canonical_indexed: Option<usize>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ManifestCandidate {
    pub(crate) relative_path: String,
    pub(crate) size_bytes: u64,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ImportManifest {
    format_version: u32,
    pub(crate) workspace_id: String,
    pub(crate) source_fingerprint: String,
    pub(crate) source_size_before: u64,
    pub(crate) source_modified_ns_before: String,
    pub(crate) source_size_after: u64,
    pub(crate) source_modified_ns_after: String,
    pub(crate) source_sha256: Option<String>,
    pub(crate) source_fingerprint_strategy: String,
    pub(crate) canonical_identity_version: String,
    pub(crate) importer_generation: String,
    pub(crate) readpst_version: String,
    pub(crate) invocation_mode: String,
    pub(crate) extraction_started_at: String,
    pub(crate) extraction_completed_at: String,
    pub(crate) extraction_result: String,
    pub(crate) candidate_records: Vec<ManifestCandidate>,
    pub(crate) candidate_count: u64,
    pub(crate) candidate_aggregate_size: u64,
    pub(crate) candidate_aggregate_digest: String,
    pub(crate) inserted_count: u64,
    pub(crate) skipped_existing_count: u64,
    pub(crate) parse_failure_count: u64,
    pub(crate) attachment_count: u64,
    pub(crate) canonical_message_total: u64,
    pub(crate) folder_count: u64,
    pub(crate) fts_row_count: u64,
    pub(crate) conversation_assignment_state: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VerificationLevel {
    Quick,
    Full,
}

impl VerificationLevel {
    pub(crate) fn from_arg(value: &str) -> AppResult<Self> {
        match value {
            "quick" => Ok(Self::Quick),
            "full" => Ok(Self::Full),
            _ => Err(AppError::new("Invalid verification level.")),
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VerificationResult {
    Verified,
    Incomplete,
    Repairable,
    SourceUnavailable,
    SourceChanged,
    ExtractionInconsistent,
    DatabaseInconsistent,
    Unsupported,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SourceIntegrity {
    Passed,
    Unavailable,
    Changed,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PossibleDuplicateGroupDetail {
    opaque_id: String,
    signals: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct VerificationReport {
    format_version: u32,
    pub(crate) operation_id: String,
    pub(crate) workspace_id: String,
    pub(crate) verification_level: VerificationLevel,
    pub(crate) result: VerificationResult,
    pub(crate) workspace_state: WorkspaceState,
    pub(crate) discovered_candidates: u64,
    pub(crate) canonical_messages: u64,
    pub(crate) inserted_messages: u64,
    pub(crate) skipped_existing_messages: u64,
    pub(crate) parse_failures: u64,
    pub(crate) attachments: u64,
    pub(crate) folders: u64,
    pub(crate) fts_rows: u64,
    pub(crate) conversation_state: String,
    pub(crate) missing_canonical_records: u64,
    pub(crate) unexpected_canonical_records: u64,
    pub(crate) possible_duplicate_groups: u64,
    pub(crate) possible_duplicate_group_details: Vec<PossibleDuplicateGroupDetail>,
    pub(crate) source_integrity: SourceIntegrity,
    pub(crate) started_at: String,
    pub(crate) completed_at: String,
    pub(crate) elapsed_duration_ms: u64,
    #[serde(default)]
    pub(crate) completed_import_duration_ms: Option<u64>,
    pub(crate) warnings: Vec<String>,
    pub(crate) recommended_actions: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct IntegritySnapshot {
    pub(crate) state: WorkspaceState,
    pub(crate) classification: &'static str,
    pub(crate) source_availability: &'static str,
    pub(crate) can_resume_indexing: bool,
    pub(crate) can_repair_index: bool,
    pub(crate) can_rebuild_conversations: bool,
    pub(crate) resume_details: Option<ResumeDetails>,
    pub(crate) has_last_verification: bool,
    pub(crate) conversation_state: String,
    pub(crate) can_open: bool,
    pub(crate) completed_duration_ms: Option<u64>,
    pub(crate) last_activity_at: Option<String>,
    pub(crate) active_elapsed_ms: u64,
    pub(crate) stage_elapsed_ms: u64,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ResumeDetails {
    pst_basename: String,
    last_completed_stage: String,
    interrupted_stage: String,
    discovered_candidates: u64,
    canonical_indexed: u64,
    remaining_candidates: Option<u64>,
    last_successful_update: String,
    elapsed_duration_ms: u64,
    stage_elapsed_duration_ms: u64,
    last_activity_at: Option<String>,
    reason: String,
}

fn canonical_utc_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn canonical_utc_now_not_before(previous: &str) -> String {
    let now = canonical_utc_now();
    if now.as_str() < previous {
        previous.to_string()
    } else {
        now
    }
}

fn parse_canonical_utc(value: &str) -> Option<DateTime<Utc>> {
    let parsed = DateTime::parse_from_rfc3339(value).ok()?;
    let utc = parsed.with_timezone(&Utc);
    (utc.to_rfc3339_opts(SecondsFormat::Millis, true) == value).then_some(utc)
}

pub(crate) fn validate_operation_id(operation_id: &str) -> AppResult<()> {
    if operation_id.is_empty()
        || operation_id.len() > MAX_OPERATION_ID_BYTES
        || !operation_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
    {
        return Err(AppError::new("Invalid workspace operation id."));
    }
    Ok(())
}

fn checked_json_path(workspace: &Path, file_name: &str) -> AppResult<PathBuf> {
    let workspace_metadata = fs::symlink_metadata(workspace)
        .map_err(|_| AppError::new("Workspace integrity metadata is unavailable."))?;
    if workspace_metadata.file_type().is_symlink() || !workspace_metadata.is_dir() {
        return Err(AppError::new(
            "Workspace integrity metadata parent is not a regular directory.",
        ));
    }
    Ok(workspace.join(file_name))
}

fn read_strict_json<T: DeserializeOwned>(
    workspace: &Path,
    file_name: &str,
    max_bytes: u64,
) -> AppResult<Option<T>> {
    let path = checked_json_path(workspace, file_name)?;
    let path_metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(AppError::new(
                "Workspace integrity metadata is unavailable.",
            ))
        }
    };
    if path_metadata.file_type().is_symlink()
        || !path_metadata.is_file()
        || path_metadata.nlink() != 1
        || path_metadata.len() > max_bytes
    {
        return Err(AppError::new(
            "Workspace integrity metadata is invalid or exceeds its safety limit.",
        ));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|_| AppError::new("Workspace integrity metadata could not be read."))?;
    let opened = file
        .metadata()
        .map_err(|_| AppError::new("Workspace integrity metadata could not be inspected."))?;
    if !opened.is_file()
        || opened.nlink() != 1
        || opened.len() > max_bytes
        || opened.dev() != path_metadata.dev()
        || opened.ino() != path_metadata.ino()
    {
        return Err(AppError::new(
            "Workspace integrity metadata changed while it was opened.",
        ));
    }
    let mut bytes = Vec::with_capacity((opened.len() as usize).min(1024 * 1024));
    Read::by_ref(&mut file)
        .take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::new("Workspace integrity metadata could not be read."))?;
    if bytes.len() as u64 > max_bytes {
        return Err(AppError::new(
            "Workspace integrity metadata exceeds its safety limit.",
        ));
    }
    let after = file
        .metadata()
        .map_err(|_| AppError::new("Workspace integrity metadata could not be inspected."))?;
    if after.dev() != opened.dev()
        || after.ino() != opened.ino()
        || after.nlink() != 1
        || after.len() != opened.len()
        || after.mtime() != opened.mtime()
        || after.mtime_nsec() != opened.mtime_nsec()
    {
        return Err(AppError::new(
            "Workspace integrity metadata changed while it was read.",
        ));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| AppError::new("Workspace integrity metadata is malformed or unsupported."))
}

fn write_strict_json<T: Serialize>(workspace: &Path, file_name: &str, value: &T) -> AppResult<()> {
    let path = checked_json_path(workspace, file_name)?;
    let value = serde_json::to_value(value)
        .map_err(|_| AppError::new("Workspace integrity metadata could not be serialized."))?;
    write_workspace_metadata_atomically(&path, &value)
}

fn validate_state_record(record: &WorkspaceImportState, workspace_id: &str) -> AppResult<()> {
    let created = parse_canonical_utc(&record.created_at);
    let updated = parse_canonical_utc(&record.updated_at);
    let state_started = parse_canonical_utc(&record.state_started_at);
    let last_successful = parse_canonical_utc(&record.last_successful_update);
    let active_session_started = record
        .active_session_started_at
        .as_deref()
        .and_then(parse_canonical_utc);
    let last_activity = record
        .last_activity_at
        .as_deref()
        .and_then(parse_canonical_utc);
    let timestamp_order_invalid = match created.zip(updated).zip(state_started).zip(last_successful)
    {
        Some((((created, updated), state_started), last_successful)) => {
            created > updated
                || state_started < created
                || state_started > updated
                || last_successful < created
                || last_successful > updated
        }
        None => true,
    };
    let optional_timestamp_invalid = record.active_session_started_at.is_some()
        != active_session_started.is_some()
        || record.last_activity_at.is_some() != last_activity.is_some()
        || active_session_started.is_some_and(|started| {
            created.is_none_or(|created| started < created)
                || updated.is_none_or(|updated| started > updated)
        })
        || last_activity.is_some_and(|activity| {
            created.is_none_or(|created| activity < created)
                || updated.is_none_or(|updated| activity > updated)
        });
    let effective_active_elapsed = record
        .elapsed_duration_ms
        .max(record.accumulated_active_elapsed_ms);
    let expected_remaining = record
        .discovered_candidates
        .checked_sub(record.canonical_indexed);
    let completed_stage_invalid = record
        .last_completed_stage
        .is_some_and(|stage| !stage.is_transient());
    let interrupted_stage_invalid = record
        .interrupted_stage
        .is_some_and(|stage| !stage.is_transient())
        || (record.state.is_transient() && record.interrupted_stage.is_some())
        || (record.state == WorkspaceState::Complete && record.interrupted_stage.is_some());
    let legacy_timing_fields_invalid = record.active_timing_version == 0
        && (record.active_timing_history_complete
            || record.operation_generation != 0
            || record.active_session_started_at.is_some()
            || record.active_session_updates_import_timing
            || record.accumulated_active_elapsed_ms != 0
            || record.stage_elapsed_duration_ms != 0
            || record.last_activity_at.is_some()
            || record.completed_duration_ms.is_some()
            || record.pending_completion_duration_ms.is_some()
            || record.operation_timing.is_some());
    let operation_timing_invalid = record.operation_timing.is_some_and(|timing| {
        !timing.stage.is_transient()
            || !valid_operation_timing_stage(timing.kind, timing.stage)
            || timing.generation == 0
            || timing.generation > MAX_SAFE_JSON_INTEGER
            || timing.generation != record.operation_generation
            || timing.active_elapsed_ms > MAX_SAFE_JSON_INTEGER
            || timing.stage_elapsed_ms > timing.active_elapsed_ms
            || record.active_session_updates_import_timing
    }) || (record.active_session_started_at.is_some()
        && !record.active_session_updates_import_timing
        && record.operation_timing.is_none());
    if record.format_version != STATE_FORMAT_VERSION
        || record.workspace_id != workspace_id
        || record.revision == 0
        || record.revision > MAX_SAFE_JSON_INTEGER
        || record.discovered_candidates > MAX_SAFE_JSON_INTEGER
        || record.canonical_indexed > MAX_SAFE_JSON_INTEGER
        || record.elapsed_duration_ms > MAX_SAFE_JSON_INTEGER
        || record.active_timing_version > ACTIVE_TIMING_VERSION
        || legacy_timing_fields_invalid
        || operation_timing_invalid
        || (record.active_timing_history_complete
            && record.active_timing_version != ACTIVE_TIMING_VERSION)
        || record.operation_generation > MAX_SAFE_JSON_INTEGER
        || record.accumulated_active_elapsed_ms > MAX_SAFE_JSON_INTEGER
        || record.stage_elapsed_duration_ms > MAX_SAFE_JSON_INTEGER
        || record
            .completed_duration_ms
            .is_some_and(|value| value > MAX_SAFE_JSON_INTEGER)
        || record
            .pending_completion_duration_ms
            .is_some_and(|value| value > effective_active_elapsed)
        || (record.pending_completion_duration_ms.is_some()
            && (record.active_session_started_at.is_none()
                || !record.active_session_updates_import_timing
                || !record.active_timing_history_complete
                || record.state != WorkspaceState::Verifying))
        || timestamp_order_invalid
        || optional_timestamp_invalid
        || (record.active_session_started_at.is_none()
            && record.active_session_updates_import_timing)
        || (record.active_timing_version == ACTIVE_TIMING_VERSION
            && record.accumulated_active_elapsed_ms != record.elapsed_duration_ms)
        || record.stage_elapsed_duration_ms > effective_active_elapsed
        || record.completed_duration_ms.is_some_and(|value| {
            !record.active_timing_history_complete || value > effective_active_elapsed
        })
        || expected_remaining != record.remaining_candidates
        || completed_stage_invalid
        || interrupted_stage_invalid
        || record
            .reason
            .as_ref()
            .is_some_and(|reason| !valid_state_reason(reason))
    {
        return Err(AppError::new(
            "Workspace state metadata is malformed or unsupported.",
        ));
    }
    Ok(())
}

fn valid_state_reason(reason: &str) -> bool {
    !reason.is_empty()
        && reason.len() <= 64
        && reason
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

pub(crate) fn read_state(
    workspace: &Path,
    workspace_id: &str,
) -> AppResult<Option<WorkspaceImportState>> {
    let record = read_strict_json(workspace, IMPORT_STATE_FILE, MAX_STATE_BYTES)?;
    if let Some(record) = &record {
        validate_state_record(record, workspace_id)?;
    }
    Ok(record)
}

fn transition_is_allowed(previous: WorkspaceState, next: WorkspaceState) -> bool {
    if previous == next {
        return true;
    }
    if matches!(next, WorkspaceState::Cancelled | WorkspaceState::Failed) {
        return previous.is_transient();
    }
    if next == WorkspaceState::Incomplete {
        return previous.is_transient();
    }
    matches!(
        (previous, next),
        (WorkspaceState::Complete, WorkspaceState::Preflight)
            | (WorkspaceState::Cancelled, WorkspaceState::Preflight)
            | (WorkspaceState::Incomplete, WorkspaceState::Preflight)
            | (WorkspaceState::Failed, WorkspaceState::Preflight)
            | (WorkspaceState::Preflight, WorkspaceState::Extracting)
            | (
                WorkspaceState::Extracting,
                WorkspaceState::ExtractionComplete
            )
            | (
                WorkspaceState::ExtractionComplete,
                WorkspaceState::Discovering
            )
            | (WorkspaceState::Discovering, WorkspaceState::Indexing)
            | (
                WorkspaceState::Indexing,
                WorkspaceState::BuildingConversations
            )
            | (
                WorkspaceState::BuildingConversations,
                WorkspaceState::Verifying
            )
            | (WorkspaceState::Verifying, WorkspaceState::Complete)
            | (WorkspaceState::Complete, WorkspaceState::Verifying)
            | (WorkspaceState::Complete, WorkspaceState::Discovering)
            | (WorkspaceState::Complete, WorkspaceState::Indexing)
            | (
                WorkspaceState::Complete,
                WorkspaceState::BuildingConversations
            )
            | (WorkspaceState::Incomplete, WorkspaceState::Discovering)
            | (WorkspaceState::Incomplete, WorkspaceState::Indexing)
            | (
                WorkspaceState::Incomplete,
                WorkspaceState::BuildingConversations
            )
            | (WorkspaceState::Incomplete, WorkspaceState::Verifying)
            | (WorkspaceState::Cancelled, WorkspaceState::Discovering)
            | (WorkspaceState::Cancelled, WorkspaceState::Indexing)
            | (
                WorkspaceState::Cancelled,
                WorkspaceState::BuildingConversations
            )
            | (WorkspaceState::Cancelled, WorkspaceState::Verifying)
            | (WorkspaceState::Failed, WorkspaceState::Discovering)
            | (WorkspaceState::Failed, WorkspaceState::Indexing)
            | (
                WorkspaceState::Failed,
                WorkspaceState::BuildingConversations
            )
            | (WorkspaceState::Failed, WorkspaceState::Verifying)
    )
}

fn completed_stage_for_transition(
    previous: WorkspaceState,
    next: WorkspaceState,
) -> Option<WorkspaceState> {
    match (previous, next) {
        (WorkspaceState::Preflight, WorkspaceState::Extracting) => Some(WorkspaceState::Preflight),
        (WorkspaceState::Extracting, WorkspaceState::ExtractionComplete) => {
            Some(WorkspaceState::Extracting)
        }
        (WorkspaceState::ExtractionComplete, WorkspaceState::Discovering) => {
            Some(WorkspaceState::ExtractionComplete)
        }
        (WorkspaceState::Discovering, WorkspaceState::Indexing) => {
            Some(WorkspaceState::Discovering)
        }
        (WorkspaceState::Indexing, WorkspaceState::BuildingConversations) => {
            Some(WorkspaceState::Indexing)
        }
        (WorkspaceState::BuildingConversations, WorkspaceState::Verifying) => {
            Some(WorkspaceState::BuildingConversations)
        }
        (WorkspaceState::Verifying, WorkspaceState::Complete) => Some(WorkspaceState::Verifying),
        _ => None,
    }
}

fn transitioned_operation_timing(
    previous: &WorkspaceImportState,
    next: WorkspaceState,
) -> Option<OperationTimingRecord> {
    let mut timing = previous.operation_timing?;
    if next.is_transient() {
        if timing.stage != next {
            timing.stage = next;
            timing.stage_elapsed_ms = 0;
        }
        return Some(timing);
    }
    if next == WorkspaceState::Complete
        || matches!(
            next,
            WorkspaceState::Cancelled | WorkspaceState::Incomplete | WorkspaceState::Failed
        )
    {
        return Some(timing);
    }
    None
}

pub(crate) fn transition_state(
    workspace: &Path,
    workspace_id: &str,
    next: WorkspaceState,
    counts: StateCounts,
    reason: Option<&str>,
) -> AppResult<WorkspaceImportState> {
    transition_state_with_timing_history(workspace, workspace_id, next, counts, reason, true, false)
}

fn transition_state_with_timing_history(
    workspace: &Path,
    workspace_id: &str,
    next: WorkspaceState,
    counts: StateCounts,
    reason: Option<&str>,
    initial_timing_history_complete: bool,
    initial_verified_complete_lineage: bool,
) -> AppResult<WorkspaceImportState> {
    validate_workspace_identity_input(workspace_id)?;
    if reason.is_some_and(|value| !valid_state_reason(value)) {
        return Err(AppError::new("Workspace state reason is invalid."));
    }
    let previous = read_state(workspace, workspace_id)?;
    let now = previous
        .as_ref()
        .map(|record| canonical_utc_now_not_before(&record.updated_at))
        .unwrap_or_else(canonical_utc_now);
    if previous.is_none()
        && !matches!(
            next,
            WorkspaceState::Preflight
                | WorkspaceState::Incomplete
                | WorkspaceState::Cancelled
                | WorkspaceState::Failed
        )
    {
        return Err(AppError::new(
            "Initial workspace state transition was refused.",
        ));
    }
    let mut record = if let Some(previous) = previous {
        if !transition_is_allowed(previous.state, next) {
            return Err(AppError::new("Workspace state transition was refused."));
        }
        let same_stage = previous.state == next;
        let interrupted_terminal = matches!(
            next,
            WorkspaceState::Incomplete | WorkspaceState::Cancelled | WorkspaceState::Failed
        ) && previous.state.is_transient();
        let accumulated_active_elapsed_ms =
            if previous.active_timing_version == ACTIVE_TIMING_VERSION {
                previous
                    .accumulated_active_elapsed_ms
                    .max(previous.elapsed_duration_ms)
            } else {
                0
            };
        let last_completed_stage =
            completed_stage_for_transition(previous.state, next).or(previous.last_completed_stage);
        let interrupted_stage = if matches!(
            next,
            WorkspaceState::Incomplete | WorkspaceState::Cancelled | WorkspaceState::Failed
        ) && previous.state.is_transient()
        {
            Some(previous.state)
        } else if next.is_transient() {
            None
        } else {
            previous.interrupted_stage
        };
        let operation_timing = transitioned_operation_timing(&previous, next);
        let verified_complete_lineage =
            state_has_complete_lineage(&previous) || next == WorkspaceState::Complete;
        WorkspaceImportState {
            format_version: STATE_FORMAT_VERSION,
            workspace_id: workspace_id.to_string(),
            state: next,
            revision: previous
                .revision
                .checked_add(1)
                .ok_or_else(|| AppError::new("Workspace state revision overflowed."))?,
            created_at: previous.created_at,
            updated_at: now.clone(),
            state_started_at: if same_stage || interrupted_terminal {
                previous.state_started_at
            } else {
                now.clone()
            },
            last_completed_stage,
            interrupted_stage,
            verified_complete_lineage,
            discovered_candidates: counts
                .discovered_candidates
                .map(|value| value as u64)
                .unwrap_or(previous.discovered_candidates),
            canonical_indexed: counts
                .canonical_indexed
                .map(|value| value as u64)
                .unwrap_or(previous.canonical_indexed),
            remaining_candidates: None,
            last_successful_update: if matches!(
                next,
                WorkspaceState::Cancelled | WorkspaceState::Incomplete | WorkspaceState::Failed
            ) {
                previous.last_successful_update
            } else {
                now.clone()
            },
            elapsed_duration_ms: accumulated_active_elapsed_ms,
            active_timing_version: previous.active_timing_version,
            active_timing_history_complete: previous.active_timing_history_complete,
            operation_generation: previous.operation_generation,
            active_session_started_at: previous.active_session_started_at,
            active_session_updates_import_timing: previous.active_session_updates_import_timing,
            accumulated_active_elapsed_ms,
            stage_elapsed_duration_ms: if previous.active_timing_version != ACTIVE_TIMING_VERSION {
                0
            } else if same_stage
                || interrupted_terminal
                || !previous.active_session_updates_import_timing
            {
                previous.stage_elapsed_duration_ms
            } else {
                0
            },
            last_activity_at: previous.last_activity_at,
            completed_duration_ms: if !previous.active_timing_history_complete {
                None
            } else if next == WorkspaceState::Complete {
                previous
                    .completed_duration_ms
                    .or(previous.pending_completion_duration_ms)
            } else {
                previous.completed_duration_ms
            },
            pending_completion_duration_ms: if same_stage {
                previous.pending_completion_duration_ms
            } else {
                None
            },
            operation_timing,
            reason: reason.map(str::to_string),
        }
    } else {
        WorkspaceImportState {
            format_version: STATE_FORMAT_VERSION,
            workspace_id: workspace_id.to_string(),
            state: next,
            revision: 1,
            created_at: now.clone(),
            updated_at: now.clone(),
            state_started_at: now.clone(),
            last_completed_stage: None,
            interrupted_stage: None,
            verified_complete_lineage: initial_verified_complete_lineage
                || next == WorkspaceState::Complete,
            discovered_candidates: counts
                .discovered_candidates
                .map(|value| value as u64)
                .unwrap_or(0),
            canonical_indexed: counts
                .canonical_indexed
                .map(|value| value as u64)
                .unwrap_or(0),
            remaining_candidates: None,
            last_successful_update: now.clone(),
            elapsed_duration_ms: 0,
            active_timing_version: ACTIVE_TIMING_VERSION,
            active_timing_history_complete: initial_timing_history_complete,
            operation_generation: 0,
            active_session_started_at: None,
            active_session_updates_import_timing: false,
            accumulated_active_elapsed_ms: 0,
            stage_elapsed_duration_ms: 0,
            last_activity_at: None,
            completed_duration_ms: None,
            pending_completion_duration_ms: None,
            operation_timing: None,
            reason: reason.map(str::to_string),
        }
    };
    if record.canonical_indexed > record.discovered_candidates {
        return Err(AppError::new("Workspace state counters are contradictory."));
    }
    record.remaining_candidates = record
        .discovered_candidates
        .checked_sub(record.canonical_indexed);
    validate_state_record(&record, workspace_id)?;
    write_strict_json(workspace, IMPORT_STATE_FILE, &record)?;
    Ok(record)
}

pub(crate) fn restart_import_state(
    workspace: &Path,
    workspace_id: &str,
) -> AppResult<WorkspaceImportState> {
    validate_workspace_identity_input(workspace_id)?;
    let previous = read_state(workspace, workspace_id)?
        .ok_or_else(|| AppError::new("Workspace state is unavailable for a fresh import."))?;
    if previous.active_session_started_at.is_some() {
        return Err(AppError::new(
            "An active import session must be recovered before starting fresh.",
        ));
    }
    let now = canonical_utc_now_not_before(&previous.updated_at);
    let record = WorkspaceImportState {
        format_version: STATE_FORMAT_VERSION,
        workspace_id: workspace_id.to_string(),
        state: WorkspaceState::Preflight,
        revision: previous
            .revision
            .checked_add(1)
            .ok_or_else(|| AppError::new("Workspace state revision overflowed."))?,
        created_at: previous.created_at,
        updated_at: now.clone(),
        state_started_at: now.clone(),
        last_completed_stage: None,
        interrupted_stage: None,
        verified_complete_lineage: false,
        discovered_candidates: 0,
        canonical_indexed: 0,
        remaining_candidates: Some(0),
        last_successful_update: now,
        elapsed_duration_ms: 0,
        active_timing_version: ACTIVE_TIMING_VERSION,
        active_timing_history_complete: true,
        operation_generation: previous.operation_generation,
        active_session_started_at: None,
        active_session_updates_import_timing: false,
        accumulated_active_elapsed_ms: 0,
        stage_elapsed_duration_ms: 0,
        last_activity_at: None,
        completed_duration_ms: None,
        pending_completion_duration_ms: None,
        operation_timing: None,
        reason: Some("preflight_running".to_string()),
    };
    validate_state_record(&record, workspace_id)?;
    clear_last_verification_for_start_fresh(workspace)?;
    write_strict_json(workspace, IMPORT_STATE_FILE, &record)?;
    Ok(record)
}

fn clear_last_verification_for_start_fresh(workspace: &Path) -> AppResult<()> {
    let path = checked_json_path(workspace, LAST_VERIFICATION_FILE)?;
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => {
            return Err(AppError::new(
                "Last verification metadata could not be inspected for Start Fresh.",
            ))
        }
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.len() > MAX_VERIFICATION_BYTES
    {
        return Err(AppError::new(
            "Last verification metadata is unsafe to invalidate for Start Fresh.",
        ));
    }
    fs::remove_file(&path).map_err(|_| {
        AppError::new("Last verification metadata could not be invalidated for Start Fresh.")
    })?;
    File::open(workspace)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| {
            AppError::new("Start Fresh verification invalidation could not be synchronized.")
        })?;
    Ok(())
}

pub(crate) fn begin_active_session(
    workspace: &Path,
    workspace_id: &str,
    update_import_timing: bool,
    reset_stage_elapsed: bool,
    operation_timing_kind: Option<OperationTimingKind>,
    operation_stage: Option<WorkspaceState>,
) -> AppResult<WorkspaceImportState> {
    let mut record = read_state(workspace, workspace_id)?
        .ok_or_else(|| AppError::new("Workspace state is unavailable for progress timing."))?;
    if record.active_session_started_at.is_some() {
        return Err(AppError::new(
            "A prior active import session must be recovered before starting another operation.",
        ));
    }
    if update_import_timing {
        if operation_timing_kind.is_some() || operation_stage.is_some() {
            return Err(AppError::new(
                "Canonical import timing cannot use non-import operation provenance.",
            ));
        }
    } else if !operation_timing_kind
        .zip(operation_stage)
        .is_some_and(|(kind, stage)| valid_operation_timing_stage(kind, stage))
    {
        return Err(AppError::new(
            "Non-import operation timing provenance is incomplete.",
        ));
    }
    if record.active_timing_version == 0 {
        // Phase 2 elapsedDurationMs was derived from UTC timestamps and can
        // include app downtime. Start Phase 3 monotonic observation at zero;
        // retain incomplete-history provenance so this partial duration can
        // never be presented as the duration of the whole import.
        record.active_timing_version = ACTIVE_TIMING_VERSION;
        record.active_timing_history_complete = false;
        record.elapsed_duration_ms = 0;
        record.accumulated_active_elapsed_ms = 0;
        record.stage_elapsed_duration_ms = 0;
        record.completed_duration_ms = None;
        record.pending_completion_duration_ms = None;
    }
    let now = canonical_utc_now_not_before(&record.updated_at);
    record.format_version = STATE_FORMAT_VERSION;
    record.revision = record
        .revision
        .checked_add(1)
        .ok_or_else(|| AppError::new("Workspace state revision overflowed."))?;
    record.operation_generation = record
        .operation_generation
        .checked_add(1)
        .ok_or_else(|| AppError::new("Workspace operation generation overflowed."))?;
    record.operation_timing = if update_import_timing {
        None
    } else {
        let kind = operation_timing_kind.expect("validated operation timing kind");
        let stage = operation_stage.expect("validated operation timing stage");
        let saved = record.operation_timing.filter(|timing| timing.kind == kind);
        let active_elapsed_ms = saved.map_or(0, |timing| timing.active_elapsed_ms);
        let stage_elapsed_ms = saved
            .filter(|timing| timing.stage == stage)
            .map_or(0, |timing| timing.stage_elapsed_ms);
        Some(OperationTimingRecord {
            kind,
            stage,
            generation: record.operation_generation,
            active_elapsed_ms,
            stage_elapsed_ms,
        })
    };
    if let Some(kind) = operation_timing_kind {
        record.reason = Some(
            match kind {
                OperationTimingKind::Repair => "repair_running",
                OperationTimingKind::Verification => "verification_running",
            }
            .to_string(),
        );
    }
    record.updated_at = now.clone();
    record.active_session_started_at = Some(now.clone());
    record.active_session_updates_import_timing = update_import_timing;
    record.last_activity_at = Some(now);
    record.accumulated_active_elapsed_ms = record
        .accumulated_active_elapsed_ms
        .max(record.elapsed_duration_ms);
    record.elapsed_duration_ms = record.accumulated_active_elapsed_ms;
    if update_import_timing && reset_stage_elapsed {
        record.stage_elapsed_duration_ms = 0;
    }
    validate_state_record(&record, workspace_id)?;
    write_strict_json(workspace, IMPORT_STATE_FILE, &record)?;
    Ok(record)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn checkpoint_active_session(
    workspace: &Path,
    workspace_id: &str,
    operation_generation: u64,
    expected_state: WorkspaceState,
    active_elapsed_ms: u64,
    stage_elapsed_ms: u64,
    activity_observed: bool,
    finish_session: bool,
    record_completion_duration: bool,
) -> AppResult<WorkspaceImportState> {
    let mut record = read_state(workspace, workspace_id)?
        .ok_or_else(|| AppError::new("Workspace state is unavailable for progress timing."))?;
    if record.operation_generation != operation_generation
        || record.state != expected_state
        || record.active_session_started_at.is_none()
    {
        return Err(AppError::new("Stale import timing update was rejected."));
    }
    let previous_active = record
        .accumulated_active_elapsed_ms
        .max(record.elapsed_duration_ms);
    let updates_import_timing = record.active_session_updates_import_timing;
    let operation_timing_contradictory = if updates_import_timing {
        record.operation_timing.is_some()
    } else {
        match record.operation_timing {
            Some(timing) => {
                timing.generation != operation_generation
                    || active_elapsed_ms < timing.active_elapsed_ms
                    || stage_elapsed_ms < timing.stage_elapsed_ms
            }
            None => true,
        }
    };
    if stage_elapsed_ms > active_elapsed_ms
        || active_elapsed_ms > MAX_SAFE_JSON_INTEGER
        || stage_elapsed_ms > MAX_SAFE_JSON_INTEGER
        || operation_timing_contradictory
        || (updates_import_timing
            && (active_elapsed_ms < previous_active
                || stage_elapsed_ms < record.stage_elapsed_duration_ms))
    {
        return Err(AppError::new("Import timing update was contradictory."));
    }
    let now = canonical_utc_now_not_before(&record.updated_at);
    record.format_version = STATE_FORMAT_VERSION;
    record.revision = record
        .revision
        .checked_add(1)
        .ok_or_else(|| AppError::new("Workspace state revision overflowed."))?;
    record.updated_at = now.clone();
    if updates_import_timing {
        let persisted_active_elapsed = if record.state == WorkspaceState::Complete {
            record.completed_duration_ms.unwrap_or(active_elapsed_ms)
        } else {
            active_elapsed_ms
        };
        record.elapsed_duration_ms = persisted_active_elapsed;
        record.accumulated_active_elapsed_ms = persisted_active_elapsed;
        record.stage_elapsed_duration_ms = if record.state == WorkspaceState::Complete {
            0
        } else {
            stage_elapsed_ms
        };
    } else if let Some(mut timing) = record.operation_timing {
        timing.active_elapsed_ms = active_elapsed_ms;
        timing.stage_elapsed_ms = stage_elapsed_ms;
        record.operation_timing = Some(timing);
    }
    if activity_observed {
        record.last_activity_at = Some(now);
    }
    if record_completion_duration && updates_import_timing && record.active_timing_history_complete
    {
        if record.state == WorkspaceState::Complete && record.completed_duration_ms.is_none() {
            record.completed_duration_ms = Some(active_elapsed_ms);
        } else if record.state == WorkspaceState::Verifying {
            record.pending_completion_duration_ms = Some(active_elapsed_ms);
        }
    }
    if finish_session {
        if record.state == WorkspaceState::Complete
            && record.reason.as_deref() == Some("verification_complete")
        {
            record.operation_timing = None;
        }
        record.active_session_started_at = None;
        record.active_session_updates_import_timing = false;
        record.pending_completion_duration_ms = None;
    }
    validate_state_record(&record, workspace_id)?;
    write_strict_json(workspace, IMPORT_STATE_FILE, &record)?;
    Ok(record)
}

fn abandon_stale_active_session(
    workspace: &Path,
    workspace_id: &str,
    mut record: WorkspaceImportState,
) -> AppResult<WorkspaceImportState> {
    if record.active_session_started_at.is_none() {
        return Ok(record);
    }
    record.format_version = STATE_FORMAT_VERSION;
    record.revision = record
        .revision
        .checked_add(1)
        .ok_or_else(|| AppError::new("Workspace state revision overflowed."))?;
    record.updated_at = canonical_utc_now_not_before(&record.updated_at);
    record.active_session_started_at = None;
    record.active_session_updates_import_timing = false;
    record.pending_completion_duration_ms = None;
    if record.state == WorkspaceState::Complete
        && record.reason.as_deref() == Some("verification_complete")
    {
        record.operation_timing = None;
    }
    record.accumulated_active_elapsed_ms = record
        .accumulated_active_elapsed_ms
        .max(record.elapsed_duration_ms);
    record.elapsed_duration_ms = record.accumulated_active_elapsed_ms;
    // Recovery closes the stale session without deriving any duration from UTC
    // timestamps, so app downtime is never added. Exact completion timing is
    // already consumed at the durable Complete boundary; interrupted pending
    // timing is cleared by the state transition instead of being guessed here.
    validate_state_record(&record, workspace_id)?;
    write_strict_json(workspace, IMPORT_STATE_FILE, &record)?;
    Ok(record)
}

pub(crate) fn validate_relative_identity(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32 * 1024
        && !value.starts_with('/')
        && !value.contains('\\')
        && !value.chars().any(char::is_control)
        && Path::new(value)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn validate_manifest(manifest: &ImportManifest, workspace_id: &str) -> AppResult<()> {
    let mut identities = HashSet::with_capacity(manifest.candidate_records.len());
    let mut aggregate_digest = Sha256::new();
    aggregate_digest.update(b"pstqv-eml-candidate-snapshot-v2\0");
    let aggregate_size = manifest
        .candidate_records
        .iter()
        .try_fold(0_u64, |total, candidate| {
            if !validate_relative_identity(&candidate.relative_path)
                || !is_lower_hex_sha256(&candidate.sha256)
                || candidate.size_bytes > MAX_READPST_EXTRACTED_FILE_BYTES
                || !identities.insert(candidate.relative_path.as_str())
            {
                return None;
            }
            aggregate_digest.update((candidate.relative_path.len() as u64).to_be_bytes());
            aggregate_digest.update(candidate.relative_path.as_bytes());
            aggregate_digest.update(candidate.size_bytes.to_be_bytes());
            aggregate_digest.update(candidate.sha256.as_bytes());
            total.checked_add(candidate.size_bytes)
        });
    let recomputed_aggregate_digest = hex::encode(aggregate_digest.finalize());
    let canonical_timestamp = |value: &str| {
        DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|parsed| {
                parsed
                    .with_timezone(&Utc)
                    .to_rfc3339_opts(SecondsFormat::Millis, true)
            })
            .as_deref()
            == Some(value)
    };
    let extraction_started = DateTime::parse_from_rfc3339(&manifest.extraction_started_at).ok();
    let extraction_completed = DateTime::parse_from_rfc3339(&manifest.extraction_completed_at).ok();
    let created = DateTime::parse_from_rfc3339(&manifest.created_at).ok();
    let updated = DateTime::parse_from_rfc3339(&manifest.updated_at).ok();
    let source_modified_before = manifest.source_modified_ns_before.parse::<u128>().ok();
    let source_modified_after = manifest.source_modified_ns_after.parse::<u128>().ok();
    let safe_counters = [
        manifest.candidate_count,
        manifest.candidate_aggregate_size,
        manifest.inserted_count,
        manifest.skipped_existing_count,
        manifest.parse_failure_count,
        manifest.attachment_count,
        manifest.canonical_message_total,
        manifest.folder_count,
        manifest.fts_row_count,
    ]
    .into_iter()
    .all(|value| value <= MAX_SAFE_JSON_INTEGER);
    let processed = manifest
        .inserted_count
        .checked_add(manifest.skipped_existing_count);
    let completed_relations_valid = manifest.conversation_assignment_state != "complete"
        || (processed == Some(manifest.candidate_count)
            && manifest.canonical_message_total == manifest.candidate_count
            && manifest.fts_row_count == manifest.canonical_message_total);
    let partial_relations_valid = processed.is_some_and(|value| value <= manifest.candidate_count)
        && manifest.canonical_message_total <= manifest.candidate_count
        && manifest.parse_failure_count <= manifest.canonical_message_total
        && manifest.fts_row_count <= manifest.canonical_message_total;
    let expected_fast_strategy = format!("fast-boundary-{FINGERPRINT_CHUNK_BYTES}-with-mtime");
    if manifest.format_version != MANIFEST_FORMAT_VERSION
        || manifest.workspace_id != workspace_id
        || !is_lower_hex_sha256(&manifest.source_fingerprint)
        || !matches!(
            manifest.source_fingerprint_strategy.as_str(),
            "full-sha256" | "existing-workspace-provenance"
        ) && manifest.source_fingerprint_strategy != expected_fast_strategy
        || manifest.canonical_identity_version != CANONICAL_IMPORT_IDENTITY_VERSION
        || manifest.importer_generation != IMPORTER_GENERATION
        || manifest.invocation_mode != READPST_INVOCATION_MODE
        || manifest.readpst_version.is_empty()
        || manifest.readpst_version.len() > 4096
        || manifest.readpst_version.chars().any(char::is_control)
        || manifest.candidate_count > MAX_READPST_EXTRACTED_ENTRIES
        || manifest.candidate_count != manifest.candidate_records.len() as u64
        || !safe_counters
        || aggregate_size != Some(manifest.candidate_aggregate_size)
        || manifest.candidate_aggregate_digest != recomputed_aggregate_digest
        || manifest
            .source_sha256
            .as_ref()
            .is_some_and(|hash| !is_lower_hex_sha256(hash))
        || manifest.source_size_before != manifest.source_size_after
        || manifest.source_modified_ns_before != manifest.source_modified_ns_after
        || source_modified_before.is_none()
        || source_modified_after.is_none()
        || source_modified_before.map(|value| value.to_string())
            != Some(manifest.source_modified_ns_before.clone())
        || source_modified_after.map(|value| value.to_string())
            != Some(manifest.source_modified_ns_after.clone())
        || (manifest.source_fingerprint_strategy == "full-sha256"
            && manifest.source_sha256.as_deref() != Some(manifest.source_fingerprint.as_str()))
        || !matches!(
            manifest.extraction_result.as_str(),
            "success" | "existing_extraction"
        )
        || !matches!(
            manifest.conversation_assignment_state.as_str(),
            "not_started" | "incomplete" | "complete"
        )
        || !partial_relations_valid
        || !completed_relations_valid
        || !canonical_timestamp(&manifest.extraction_started_at)
        || !canonical_timestamp(&manifest.extraction_completed_at)
        || !canonical_timestamp(&manifest.created_at)
        || !canonical_timestamp(&manifest.updated_at)
        || extraction_started
            .zip(extraction_completed)
            .is_none_or(|(start, end)| start > end)
        || extraction_completed
            .zip(updated)
            .is_none_or(|(completed, updated)| completed > updated)
        || created
            .zip(updated)
            .is_none_or(|(created, updated)| created > updated)
    {
        return Err(AppError::new(
            "Import manifest is malformed or unsupported.",
        ));
    }
    Ok(())
}

pub(crate) fn read_manifest(
    workspace: &Path,
    workspace_id: &str,
) -> AppResult<Option<ImportManifest>> {
    let manifest = read_strict_json(workspace, IMPORT_MANIFEST_FILE, MAX_MANIFEST_BYTES)?;
    if let Some(manifest) = &manifest {
        validate_manifest(manifest, workspace_id)?;
    }
    Ok(manifest)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn write_manifest_for_discovery(
    workspace: &Path,
    identity: &PstIdentity,
    workspace_id: &str,
    readpst_version: &str,
    extraction_started_at: &str,
    extraction_completed_at: &str,
    extraction_result: &str,
    discovery: &EmlDiscovery,
    source_after: Option<&FileIdentity>,
) -> AppResult<ImportManifest> {
    let candidate_aggregate_size = discovery
        .candidates
        .iter()
        .try_fold(0_u64, |total, candidate| {
            total.checked_add(candidate.identity.size)
        })
        .ok_or_else(|| AppError::new("Extracted candidate size counter overflowed."))?;
    let now = canonical_utc_now();
    let source_after_modified_ns = source_after
        .and_then(|value| {
            u128::try_from(value.modified_seconds)
                .ok()
                .and_then(|seconds| seconds.checked_mul(1_000_000_000))
                .and_then(|base| {
                    u128::try_from(value.modified_nanoseconds)
                        .ok()
                        .and_then(|nanos| base.checked_add(nanos))
                })
        })
        .unwrap_or(identity.modified_ns);
    let manifest = ImportManifest {
        format_version: MANIFEST_FORMAT_VERSION,
        workspace_id: workspace_id.to_string(),
        source_fingerprint: identity.content_fingerprint.clone(),
        source_size_before: identity.size,
        source_modified_ns_before: identity.modified_ns.to_string(),
        source_size_after: source_after
            .map(|value| value.size)
            .unwrap_or(identity.size),
        source_modified_ns_after: source_after_modified_ns.to_string(),
        source_sha256: source_after.map(|value| value.sha256.clone()).or_else(|| {
            (identity.fingerprint_strategy == "full-sha256")
                .then(|| identity.content_fingerprint.clone())
        }),
        source_fingerprint_strategy: identity.fingerprint_strategy.clone(),
        canonical_identity_version: CANONICAL_IMPORT_IDENTITY_VERSION.to_string(),
        importer_generation: IMPORTER_GENERATION.to_string(),
        readpst_version: readpst_version.to_string(),
        invocation_mode: READPST_INVOCATION_MODE.to_string(),
        extraction_started_at: extraction_started_at.to_string(),
        extraction_completed_at: extraction_completed_at.to_string(),
        extraction_result: extraction_result.to_string(),
        candidate_records: discovery
            .candidates
            .iter()
            .map(|candidate| ManifestCandidate {
                relative_path: candidate.relative_path.clone(),
                size_bytes: candidate.identity.size,
                sha256: candidate.identity.sha256.clone(),
            })
            .collect(),
        candidate_count: discovery.count as u64,
        candidate_aggregate_size,
        candidate_aggregate_digest: discovery.identity_manifest.clone(),
        inserted_count: 0,
        skipped_existing_count: 0,
        parse_failure_count: 0,
        attachment_count: 0,
        canonical_message_total: 0,
        folder_count: 0,
        fts_row_count: 0,
        conversation_assignment_state: "not_started".to_string(),
        created_at: now.clone(),
        updated_at: now,
    };
    validate_manifest(&manifest, workspace_id)?;
    write_strict_json(workspace, IMPORT_MANIFEST_FILE, &manifest)?;
    Ok(manifest)
}

fn manifest_from_index(
    workspace: &Path,
    workspace_id: &str,
    index_path: &Path,
    stats: &IndexStats,
    conversation_state: &str,
) -> AppResult<ImportManifest> {
    let mut manifest = read_manifest(workspace, workspace_id)?
        .ok_or_else(|| AppError::new("Import manifest is missing."))?;
    let conn = open_sqlite_read_only_connection(index_path)?;
    configure_sqlite_connection(&conn, true)?;
    let canonical_message_total = conn.query_row("SELECT COUNT(*) FROM messages", [], |row| {
        row.get::<_, u64>(0)
    })?;
    let attachment_count = conn.query_row("SELECT COUNT(*) FROM attachments", [], |row| {
        row.get::<_, u64>(0)
    })?;
    let folder_count = conn.query_row("SELECT COUNT(*) FROM folders", [], |row| {
        row.get::<_, u64>(0)
    })?;
    let fts_row_count = conn.query_row("SELECT COUNT(*) FROM messages_fts_docsize", [], |row| {
        row.get::<_, u64>(0)
    })?;
    let parse_failure_count = conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE body_source = 'parse_error'",
        [],
        |row| row.get::<_, u64>(0),
    )?;
    manifest.inserted_count = stats.inserted as u64;
    manifest.skipped_existing_count = stats.skipped_existing as u64;
    manifest.parse_failure_count = parse_failure_count;
    manifest.attachment_count = attachment_count;
    manifest.canonical_message_total = canonical_message_total;
    manifest.folder_count = folder_count;
    manifest.fts_row_count = fts_row_count;
    manifest.conversation_assignment_state = conversation_state.to_string();
    manifest.updated_at = canonical_utc_now();
    validate_manifest(&manifest, workspace_id)?;
    Ok(manifest)
}

pub(crate) fn update_manifest_from_index(
    workspace: &Path,
    workspace_id: &str,
    index_path: &Path,
    stats: &IndexStats,
    conversation_state: &str,
) -> AppResult<ImportManifest> {
    let manifest = manifest_from_index(
        workspace,
        workspace_id,
        index_path,
        stats,
        conversation_state,
    )?;
    write_strict_json(workspace, IMPORT_MANIFEST_FILE, &manifest)?;
    Ok(manifest)
}

pub(crate) fn stage_manifest_from_index(
    workspace: &Path,
    workspace_id: &str,
    index_path: &Path,
    stats: &IndexStats,
    conversation_state: &str,
) -> AppResult<ImportManifest> {
    if read_strict_json::<ImportManifest>(
        workspace,
        STAGED_IMPORT_MANIFEST_FILE,
        MAX_MANIFEST_BYTES,
    )?
    .is_some()
    {
        return Err(AppError::new(
            "Previous index-replacement recovery metadata is still present.",
        ));
    }
    let manifest = manifest_from_index(
        workspace,
        workspace_id,
        index_path,
        stats,
        conversation_state,
    )?;
    write_strict_json(workspace, STAGED_IMPORT_MANIFEST_FILE, &manifest)?;
    Ok(manifest)
}

pub(crate) fn has_staged_manifest(workspace: &Path, workspace_id: &str) -> AppResult<bool> {
    let manifest: Option<ImportManifest> =
        read_strict_json(workspace, STAGED_IMPORT_MANIFEST_FILE, MAX_MANIFEST_BYTES)?;
    if let Some(manifest) = manifest {
        validate_manifest(&manifest, workspace_id)?;
        return Ok(true);
    }
    Ok(false)
}

pub(crate) fn active_manifest_matches_staged(
    workspace: &Path,
    workspace_id: &str,
) -> AppResult<bool> {
    let active: Option<ImportManifest> =
        read_strict_json(workspace, IMPORT_MANIFEST_FILE, MAX_MANIFEST_BYTES)?;
    let staged: Option<ImportManifest> =
        read_strict_json(workspace, STAGED_IMPORT_MANIFEST_FILE, MAX_MANIFEST_BYTES)?;
    let (Some(active), Some(staged)) = (active, staged) else {
        return Ok(false);
    };
    validate_manifest(&active, workspace_id)?;
    validate_manifest(&staged, workspace_id)?;
    Ok(active == staged)
}

pub(crate) fn publish_staged_manifest(workspace: &Path, workspace_id: &str) -> AppResult<()> {
    let manifest: ImportManifest =
        read_strict_json(workspace, STAGED_IMPORT_MANIFEST_FILE, MAX_MANIFEST_BYTES)?
            .ok_or_else(|| AppError::new("Index-replacement manifest is missing."))?;
    validate_manifest(&manifest, workspace_id)?;
    write_strict_json(workspace, IMPORT_MANIFEST_FILE, &manifest)?;

    // The active manifest is already durable and authoritative. A failure to
    // retire the staged copy must not roll the database back underneath it;
    // recovery can safely recognize an identical retained copy.
    let staged_path = checked_json_path(workspace, STAGED_IMPORT_MANIFEST_FILE)?;
    if let Ok(metadata) = fs::symlink_metadata(&staged_path) {
        if !metadata.file_type().is_symlink() && metadata.is_file() && metadata.nlink() == 1 {
            let _ = fs::remove_file(&staged_path);
            let _ = File::open(workspace).and_then(|directory| directory.sync_all());
        }
    }
    Ok(())
}

pub(crate) fn discard_staged_manifest(workspace: &Path) -> AppResult<()> {
    let staged_path = checked_json_path(workspace, STAGED_IMPORT_MANIFEST_FILE)?;
    match fs::symlink_metadata(&staged_path) {
        Ok(metadata)
            if !metadata.file_type().is_symlink()
                && metadata.is_file()
                && metadata.nlink() == 1 =>
        {
            fs::remove_file(&staged_path)?;
            File::open(workspace)?.sync_all()?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        _ => Err(AppError::new(
            "Index-replacement recovery metadata is invalid.",
        )),
    }
}

pub(crate) fn ensure_manifest_for_existing_extraction(
    workspace: &Path,
    identity: &PstIdentity,
    workspace_id: &str,
    readpst_version: &str,
    discovery: &EmlDiscovery,
) -> AppResult<ImportManifest> {
    if let Some(manifest) = read_manifest(workspace, workspace_id)? {
        if manifest.source_fingerprint != identity.content_fingerprint
            || manifest.candidate_count != discovery.count as u64
            || manifest.candidate_aggregate_digest != discovery.identity_manifest
        {
            return Err(AppError::new(
                "Existing extraction does not match its import manifest.",
            ));
        }
        return Ok(manifest);
    }
    let now = canonical_utc_now();
    write_manifest_for_discovery(
        workspace,
        identity,
        workspace_id,
        readpst_version,
        &now,
        &now,
        "existing_extraction",
        discovery,
        None,
    )
}

fn metadata_usize_or(conn: &Connection, key: &str, fallback: usize) -> usize {
    metadata_value(conn, key)
        .ok()
        .flatten()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(fallback)
}

pub(crate) fn identity_from_active_workspace(active: &ActiveWorkspace) -> AppResult<PstIdentity> {
    let marker = read_workspace_marker(&active.path.join(WORKSPACE_MARKER_FILE))?;
    if marker.app != WORKSPACE_MARKER_APP_NAME
        || marker.workspace_id != active.id
        || marker.pst_content_fingerprint != active.fingerprint
        || marker.workspace_location_mode != active.location_mode.as_str()
    {
        return Err(AppError::new(
            "Workspace identity metadata is inconsistent.",
        ));
    }
    let manifest = read_manifest(&active.path, &active.id)?;
    let metadata_connection = [
        active.path.join("index.sqlite"),
        active.path.join("index.tmp.sqlite"),
    ]
    .into_iter()
    .find_map(|path| {
        if !path.is_file() {
            return None;
        }
        let conn = open_sqlite_read_only_connection(&path).ok()?;
        configure_sqlite_connection(&conn, true).ok()?;
        Some(conn)
    });
    let workspace_metadata = read_workspace_metadata_json_strict(&active.path)?;
    let metadata_value_for = |key: &str| -> AppResult<Option<String>> {
        if let Some(value) = metadata_connection
            .as_ref()
            .and_then(|conn| metadata_value(conn, key).ok().flatten())
        {
            return Ok(Some(value));
        }
        workspace_metadata_string_from_value(workspace_metadata.as_ref(), key, key)
    };
    let metadata_size = metadata_value_for("pst_size")?;
    let metadata_modified_ns = metadata_value_for("pst_modified_ns")?;
    let metadata_fingerprint_strategy = metadata_value_for("pst_fingerprint_strategy")?;
    let metadata_fingerprint = metadata_value_for("pst_fingerprint")?;
    let size = manifest
        .as_ref()
        .map(|value| value.source_size_before)
        .or_else(|| metadata_size.and_then(|value| value.parse().ok()))
        .unwrap_or(0);
    let modified_ns = manifest
        .as_ref()
        .and_then(|value| value.source_modified_ns_before.parse().ok())
        .or_else(|| metadata_modified_ns.and_then(|value| value.parse().ok()))
        .unwrap_or(0);
    let fingerprint_strategy = manifest
        .as_ref()
        .map(|value| value.source_fingerprint_strategy.clone())
        .or(metadata_fingerprint_strategy)
        .unwrap_or_else(|| "existing-workspace-provenance".to_string());
    let fingerprint =
        metadata_fingerprint.unwrap_or_else(|| marker.pst_content_fingerprint.clone());
    Ok(PstIdentity {
        canonical_path: active.pst_path.clone(),
        display_path: active.pst_path.display().to_string(),
        size,
        modified_ns,
        workspace_id: marker.pst_content_fingerprint.clone(),
        legacy_workspace_id: active.id.clone(),
        fingerprint,
        content_fingerprint: marker.pst_content_fingerprint,
        fingerprint_strategy,
    })
}

fn prior_verification_proves_complete_lineage(
    workspace: &Path,
    workspace_id: &str,
) -> AppResult<bool> {
    Ok(get_last_verification(workspace, workspace_id)?
        .is_some_and(|report| verification_report_proves_complete_lineage(&report)))
}

pub(crate) fn ensure_state_for_workspace(
    workspace: &Path,
    workspace_id: &str,
) -> AppResult<WorkspaceImportState> {
    if let Some(mut state) = read_state(workspace, workspace_id)? {
        // A legacy sidecar can predate the sticky lineage field even when a
        // strictly validated Complete report already proves that this is a
        // maintenance operation. Persist that evidence before callers choose
        // between canonical-import and repair/verification timing domains.
        if !state_has_complete_lineage(&state)
            && prior_verification_proves_complete_lineage(workspace, workspace_id)?
        {
            state.verified_complete_lineage = true;
            state.revision = state
                .revision
                .checked_add(1)
                .ok_or_else(|| AppError::new("Workspace state revision overflowed."))?;
            state.updated_at = canonical_utc_now_not_before(&state.updated_at);
            validate_state_record(&state, workspace_id)?;
            write_strict_json(workspace, IMPORT_STATE_FILE, &state)?;
        }
        return Ok(state);
    }
    let index_path = workspace.join("index.sqlite");
    let (status, canonical_count) = if index_path.is_file() {
        let conn = open_sqlite_read_only_connection(&index_path)?;
        configure_sqlite_connection(&conn, true)?;
        let count = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| {
                row.get::<_, usize>(0)
            })
            .unwrap_or(0);
        (metadata_value(&conn, "import_status")?, count)
    } else {
        (workspace_metadata_status_strict(workspace)?, 0)
    };
    let (state, reason) = match status.as_deref() {
        Some("cancelled") => (WorkspaceState::Cancelled, "previous_import_cancelled"),
        Some("failed") => (WorkspaceState::Failed, "previous_import_failed"),
        Some("pending" | "running" | "complete" | "verifying") | None => {
            (WorkspaceState::Incomplete, "verification_required")
        }
        Some(_) => {
            return Err(AppError::new(
                "Workspace import state is newer than this version supports.",
            ))
        }
    };
    let discovered = if index_path.is_file() {
        let conn = open_sqlite_read_only_connection(&index_path)?;
        configure_sqlite_connection(&conn, true)?;
        match metadata_value(&conn, "candidates_discovered")? {
            Some(value) => {
                let parsed = value
                    .parse::<usize>()
                    .map_err(|_| AppError::new("Workspace candidate metadata is contradictory."))?;
                if parsed < canonical_count {
                    return Err(AppError::new(
                        "Workspace candidate metadata is contradictory.",
                    ));
                }
                parsed
            }
            None => canonical_count,
        }
    } else {
        canonical_count
    };
    // A legacy database status is not completion proof. A strictly validated
    // prior Complete verification report is, and seeds durable lineage before
    // a repair or standalone verification chooses its timing domain.
    let verified_complete_lineage =
        prior_verification_proves_complete_lineage(workspace, workspace_id)?;
    transition_state_with_timing_history(
        workspace,
        workspace_id,
        state,
        StateCounts {
            discovered_candidates: Some(discovered),
            canonical_indexed: Some(canonical_count),
        },
        Some(reason),
        false,
        verified_complete_lineage,
    )
}

fn observed_state_counts(workspace: &Path, state: &WorkspaceImportState) -> (usize, usize) {
    let candidates = [
        workspace.join("index.sqlite"),
        workspace.join("index.reindex.tmp.sqlite"),
        workspace.join("index.tmp.sqlite"),
    ];
    let mut observations = Vec::new();
    for (path_order, index_path) in candidates.into_iter().enumerate() {
        if !index_path.is_file() {
            continue;
        }
        let Ok(conn) = open_sqlite_read_only_connection(&index_path) else {
            continue;
        };
        if configure_sqlite_connection(&conn, true).is_err()
            || metadata_value(&conn, "workspace_id")
                .ok()
                .flatten()
                .as_deref()
                != Some(state.workspace_id.as_str())
        {
            continue;
        }
        let status = metadata_value(&conn, "import_status")
            .ok()
            .flatten()
            .unwrap_or_default();
        let status_order = match (state.state, status.as_str()) {
            (WorkspaceState::Verifying, "verifying") => 0,
            (WorkspaceState::Indexing | WorkspaceState::BuildingConversations, "running") => 0,
            (WorkspaceState::Indexing | WorkspaceState::BuildingConversations, "verifying") => 1,
            (_, "running" | "verifying") => 1,
            (_, "complete") => 2,
            _ => 3,
        };
        let discovered = metadata_value(&conn, "candidates_discovered")
            .ok()
            .flatten()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(state.discovered_candidates as usize);
        let canonical = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| {
                row.get::<_, usize>(0)
            })
            .unwrap_or(state.canonical_indexed as usize);
        if canonical <= discovered {
            observations.push(((status_order, path_order), discovered, canonical));
        }
    }
    if let Some((_, discovered, canonical)) = observations.into_iter().min_by_key(|item| item.0) {
        return (discovered, canonical);
    }
    (
        state.discovered_candidates as usize,
        state.canonical_indexed as usize,
    )
}

pub(crate) fn recover_interrupted_state(
    workspace: &Path,
    workspace_id: &str,
    state: WorkspaceImportState,
) -> AppResult<WorkspaceImportState> {
    if !state.state.is_transient() {
        let recovered = abandon_stale_active_session(workspace, workspace_id, state)?;
        if recovered.state == WorkspaceState::Complete {
            sync_completed_duration_to_last_verification(workspace, workspace_id)?;
        }
        return Ok(recovered);
    }
    let (discovered, canonical) = observed_state_counts(workspace, &state);
    let recovered = transition_state(
        workspace,
        workspace_id,
        WorkspaceState::Incomplete,
        StateCounts {
            discovered_candidates: Some(discovered),
            canonical_indexed: Some(canonical),
        },
        Some("interrupted_process"),
    )?;
    abandon_stale_active_session(workspace, workspace_id, recovered)
}

pub(crate) fn ensure_phase2_manifest_with_progress<F>(
    active: &ActiveWorkspace,
    identity: &PstIdentity,
    mut on_progress: F,
) -> AppResult<ImportManifest>
where
    F: FnMut(usize, bool) -> AppResult<()>,
{
    if let Some(manifest) = read_manifest(&active.path, &active.id)? {
        if manifest.workspace_id != active.id
            || manifest.source_fingerprint != active.fingerprint
            || manifest.source_fingerprint != identity.content_fingerprint
        {
            return Err(AppError::new(
                "Import manifest identity does not match this workspace.",
            ));
        }
        return Ok(manifest);
    }
    let extract_dir = active.path.join("extracted");
    on_progress(0, false)?;
    let discovery = discover_eml_candidates_with_progress(&extract_dir, &mut on_progress)?;
    let readpst_version =
        workspace_metadata_string_strict(&active.path, "readpst_version", "readpstVersion")?
            .unwrap_or_else(|| "unavailable".to_string());
    let mut manifest = ensure_manifest_for_existing_extraction(
        &active.path,
        identity,
        &active.id,
        &readpst_version,
        &discovery,
    )?;
    let index_path = active.path.join("index.sqlite");
    if index_path.is_file() {
        let conn = open_sqlite_read_only_connection(&index_path)?;
        configure_sqlite_connection(&conn, true)?;
        let canonical_count = conn.query_row("SELECT COUNT(*) FROM messages", [], |row| {
            row.get::<_, usize>(0)
        })?;
        let mut inserted = metadata_usize_or(&conn, "canonical_messages_inserted", canonical_count);
        let mut skipped = metadata_usize_or(&conn, "existing_canonical_messages_skipped", 0);
        if inserted.checked_add(skipped) != Some(discovery.count) {
            inserted = canonical_count.min(discovery.count);
            skipped = discovery.count.saturating_sub(inserted);
        }
        let stats = IndexStats {
            discovered: discovery.count,
            processed: discovery.count,
            indexed: canonical_count,
            inserted,
            skipped_existing: skipped,
            error_count: metadata_usize_or(&conn, "parse_failures", 0),
            ..IndexStats::default()
        };
        let conversation_state = if conversation_data_is_indexed(&conn)? {
            "complete"
        } else {
            "incomplete"
        };
        drop(conn);
        manifest = update_manifest_from_index(
            &active.path,
            &active.id,
            &index_path,
            &stats,
            conversation_state,
        )?;
    }
    Ok(manifest)
}

fn source_probe(active: &ActiveWorkspace, manifest: &ImportManifest) -> SourceIntegrity {
    let before = match fs::symlink_metadata(&active.pst_path) {
        Ok(metadata)
            if !metadata.file_type().is_symlink()
                && metadata.is_file()
                && metadata.nlink() == 1 =>
        {
            metadata
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return SourceIntegrity::Unavailable
        }
        _ => return SourceIntegrity::Changed,
    };
    let modified_ns = before
        .modified()
        .unwrap_or(UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    if before.len() != manifest.source_size_after
        || modified_ns.to_string() != manifest.source_modified_ns_after
    {
        return SourceIntegrity::Changed;
    }
    let fingerprint = match fingerprint_pst(&active.pst_path, before.len(), modified_ns) {
        Ok((content, _, _)) => content,
        Err(_) => {
            return if fs::symlink_metadata(&active.pst_path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            {
                SourceIntegrity::Unavailable
            } else {
                SourceIntegrity::Changed
            }
        }
    };
    let after = match fs::symlink_metadata(&active.pst_path) {
        Ok(after) => after,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return SourceIntegrity::Unavailable
        }
        Err(_) => return SourceIntegrity::Changed,
    };
    if after.file_type().is_symlink()
        || !after.is_file()
        || after.nlink() != 1
        || after.dev() != before.dev()
        || after.ino() != before.ino()
        || after.len() != before.len()
        || after.mtime() != before.mtime()
        || after.mtime_nsec() != before.mtime_nsec()
    {
        return SourceIntegrity::Changed;
    }
    if fingerprint == manifest.source_fingerprint {
        SourceIntegrity::Passed
    } else {
        SourceIntegrity::Changed
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ExtractionEntrySnapshot {
    device: u64,
    inode: u64,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    sha256: Option<String>,
}

fn extraction_entry_snapshot(
    metadata: &fs::Metadata,
    sha256: Option<String>,
) -> ExtractionEntrySnapshot {
    ExtractionEntrySnapshot {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        sha256,
    }
}

const VERIFICATION_SCAN_PROGRESS_BATCH_SIZE: usize = 256;

fn scan_extraction_pass_with_progress<F>(
    extract_dir: &Path,
    manifest: &ImportManifest,
    level: VerificationLevel,
    mut on_progress: F,
) -> AppResult<(
    HashMap<String, ExtractionEntrySnapshot>,
    bool,
    SqliteFileState,
)>
where
    F: FnMut() -> AppResult<()>,
{
    let root_metadata = fs::symlink_metadata(extract_dir)
        .map_err(|_| AppError::new("Extracted source records are unavailable."))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(AppError::new("Extracted source records are unavailable."));
    }
    let root_state = SqliteFileState {
        device: root_metadata.dev(),
        inode: root_metadata.ino(),
        size: root_metadata.len(),
        modified_seconds: root_metadata.mtime(),
        modified_nanoseconds: root_metadata.mtime_nsec(),
    };
    let expected = manifest
        .candidate_records
        .iter()
        .map(|candidate| (candidate.relative_path.as_str(), candidate))
        .collect::<HashMap<_, _>>();
    let mut found = HashMap::with_capacity(expected.len());
    let mut content_mismatch = false;
    let mut scanned_entries = 0usize;
    for entry in WalkDir::new(extract_dir).sort_by_file_name() {
        let entry = entry.map_err(|_| AppError::new("Extracted source scan failed."))?;
        if entry.path() == extract_dir {
            continue;
        }
        scanned_entries = scanned_entries.saturating_add(1);
        if scanned_entries.is_multiple_of(VERIFICATION_SCAN_PROGRESS_BATCH_SIZE) {
            on_progress()?;
        }
        let entry_metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| AppError::new("Extracted source scan failed."))?;
        if entry_metadata.file_type().is_symlink()
            || (!entry_metadata.is_dir() && !entry_metadata.is_file())
        {
            return Err(AppError::new(
                "Extracted source records contain a linked or nonregular entry.",
            ));
        }
        if !entry_metadata.is_file() || !is_eml_file(entry.path()) {
            continue;
        }
        let relative = relative_path_string(extract_dir, entry.path())?;
        let metadata = require_regular_nonsymlink_file(entry.path(), "extracted source record")?;
        let (snapshot, observed_sha256) = if level == VerificationLevel::Full {
            let (_, identity) =
                read_eml_candidate_snapshot_with_progress(entry.path(), |_| on_progress())?;
            (
                ExtractionEntrySnapshot {
                    device: identity.device,
                    inode: identity.inode,
                    size: identity.size,
                    modified_seconds: identity.modified_seconds,
                    modified_nanoseconds: identity.modified_nanoseconds,
                    sha256: Some(identity.sha256.clone()),
                },
                Some(identity.sha256),
            )
        } else {
            (extraction_entry_snapshot(&metadata, None), None)
        };
        if found.insert(relative.clone(), snapshot).is_some() {
            return Err(AppError::new(
                "Duplicate canonical extracted identities were found.",
            ));
        }
        let Some(candidate) = expected.get(relative.as_str()) else {
            continue;
        };
        if metadata.len() != candidate.size_bytes {
            content_mismatch = true;
            continue;
        }
        if observed_sha256
            .as_deref()
            .is_some_and(|sha256| sha256 != candidate.sha256)
        {
            content_mismatch = true;
        }
    }
    on_progress()?;
    Ok((found, content_mismatch, root_state))
}

#[cfg(test)]
fn scan_extraction_pass(
    extract_dir: &Path,
    manifest: &ImportManifest,
    level: VerificationLevel,
) -> AppResult<(
    HashMap<String, ExtractionEntrySnapshot>,
    bool,
    SqliteFileState,
)> {
    scan_extraction_pass_with_progress(extract_dir, manifest, level, || Ok(()))
}

#[cfg(test)]
fn scan_extraction(
    extract_dir: &Path,
    manifest: &ImportManifest,
    level: VerificationLevel,
) -> AppResult<(HashSet<String>, bool)> {
    let first = scan_extraction_pass(extract_dir, manifest, level)?;
    let second = scan_extraction_pass(extract_dir, manifest, level)?;
    finish_extraction_scan(first, second)
}

fn finish_extraction_scan(
    first: (
        HashMap<String, ExtractionEntrySnapshot>,
        bool,
        SqliteFileState,
    ),
    second: (
        HashMap<String, ExtractionEntrySnapshot>,
        bool,
        SqliteFileState,
    ),
) -> AppResult<(HashSet<String>, bool)> {
    if first.0 != second.0 || first.2 != second.2 {
        return Err(AppError::new(
            "Extracted source records changed during verification.",
        ));
    }
    Ok((first.0.into_keys().collect(), first.1 || second.1))
}

#[derive(Default)]
struct DatabaseChecks {
    canonical_messages: u64,
    attachments: u64,
    folders: u64,
    fts_rows: u64,
    missing_canonical: u64,
    missing_canonical_paths: HashSet<String>,
    unexpected_canonical: u64,
    unexpected_canonical_paths: HashSet<String>,
    conversation_state: String,
    inconsistent: bool,
    counter_mismatch: bool,
}

const VERIFICATION_SQLITE_PROGRESS_OPS: std::os::raw::c_int = 10_000;

unsafe extern "C" fn verification_sqlite_cancel_callback(
    context: *mut c_void,
) -> std::os::raw::c_int {
    if context.is_null() {
        return 0;
    }
    // The handler is installed only while the borrowed AtomicBool remains
    // alive, and SQLite invokes it synchronously on this connection's query
    // thread. Returning nonzero interrupts the current statement.
    let cancelled = unsafe { &*context.cast::<AtomicBool>() };
    std::os::raw::c_int::from(cancelled.load(Ordering::SeqCst))
}

struct VerificationSqliteCancellation<'a> {
    connection: &'a Connection,
}

impl<'a> VerificationSqliteCancellation<'a> {
    fn install(connection: &'a Connection, cancel_requested: &'a AtomicBool) -> Self {
        let context = std::ptr::from_ref(cancel_requested)
            .cast_mut()
            .cast::<c_void>();
        // SAFETY: `connection` owns this SQLite handle for the guard lifetime;
        // `cancel_requested` outlives the registered handler; Drop unregisters
        // the callback before either borrow can end.
        unsafe {
            ffi::sqlite3_progress_handler(
                connection.handle(),
                VERIFICATION_SQLITE_PROGRESS_OPS,
                Some(verification_sqlite_cancel_callback),
                context,
            );
        }
        Self { connection }
    }
}

impl Drop for VerificationSqliteCancellation<'_> {
    fn drop(&mut self) {
        // SAFETY: this guard still owns a valid borrow of the same connection.
        unsafe {
            ffi::sqlite3_progress_handler(self.connection.handle(), 0, None, std::ptr::null_mut());
        }
    }
}

#[cfg(test)]
fn database_checks(index_path: &Path, manifest: &ImportManifest) -> AppResult<DatabaseChecks> {
    let cancel_requested = AtomicBool::new(false);
    database_checks_with_progress(index_path, manifest, &cancel_requested, || Ok(()))
}

fn database_checks_with_progress<P>(
    index_path: &Path,
    manifest: &ImportManifest,
    cancel_requested: &AtomicBool,
    mut on_progress: P,
) -> AppResult<DatabaseChecks>
where
    P: FnMut() -> AppResult<()>,
{
    let mut checked_progress = || {
        on_progress()?;
        if cancel_requested.load(Ordering::SeqCst) {
            return Err(import_cancelled_error(
                "Workspace verification was cancelled at a bounded database checkpoint.",
            ));
        }
        Ok(())
    };
    checked_progress()?;
    let result = database_checks_inner(
        index_path,
        manifest,
        cancel_requested,
        &mut checked_progress,
    );
    match result {
        Err(error)
            if cancel_requested.load(Ordering::SeqCst)
                && error.code == Some(SEARCH_CANCELLED_CODE) =>
        {
            Err(import_cancelled_error(
                "Workspace verification was cancelled during a database check.",
            ))
        }
        result => result,
    }
}

fn database_checks_inner<P>(
    index_path: &Path,
    manifest: &ImportManifest,
    cancel_requested: &AtomicBool,
    on_progress: &mut P,
) -> AppResult<DatabaseChecks>
where
    P: FnMut() -> AppResult<()>,
{
    let initial_file_state = sqlite_file_state(index_path)?;
    let conn = open_sqlite_read_only_connection(index_path)?;
    configure_sqlite_connection(&conn, true)?;
    let cancellation = VerificationSqliteCancellation::install(&conn, cancel_requested);
    conn.execute_batch("BEGIN DEFERRED TRANSACTION")?;
    if read_schema_version(&conn)? != SQLITE_SCHEMA_VERSION_CURRENT {
        return Err(AppError::new("Workspace database schema is unsupported."));
    }
    verify_no_schema_programs(&conn)?;
    verify_read_query_schema(&conn)?;
    verify_schema(&conn)?;
    on_progress()?;
    let fts_definition = conn
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'messages_fts'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?
        .ok_or_else(|| AppError::new("Workspace full-text index is missing."))?;
    let normalized_fts_definition = fts_definition
        .chars()
        .filter(|value| !value.is_ascii_whitespace() && !matches!(value, '\'' | '"' | '`'))
        .flat_map(char::to_lowercase)
        .collect::<String>();
    if normalized_fts_definition
        != "createvirtualtablemessages_ftsusingfts5(subject,sender,recipients,body,attachment_names,content=messages,content_rowid=id)"
    {
        return Err(AppError::new(
            "Workspace full-text index definition is unsupported.",
        ));
    }
    let docsize_table = conn
        .query_row(
            "SELECT type, ncol, wr, strict
               FROM pragma_table_list
              WHERE schema = 'main' AND name = 'messages_fts_docsize'",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?;
    if docsize_table != Some(("shadow".to_string(), 2, 0, 0)) {
        return Err(AppError::new(
            "Workspace full-text index shadow schema is unsupported.",
        ));
    }
    let mut docsize_statement = conn.prepare(
        "SELECT cid, name, UPPER(type), \"notnull\", COALESCE(dflt_value, ''), pk, hidden
           FROM pragma_table_xinfo('messages_fts_docsize')
          ORDER BY cid",
    )?;
    let docsize_columns = collect_rows(docsize_statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, i64>(5)?,
            row.get::<_, i64>(6)?,
        ))
    })?)?;
    drop(docsize_statement);
    if docsize_columns
        != [
            (
                0,
                "id".to_string(),
                "INTEGER".to_string(),
                0,
                String::new(),
                1,
                0,
            ),
            (
                1,
                "sz".to_string(),
                "BLOB".to_string(),
                0,
                String::new(),
                0,
                0,
            ),
        ]
    {
        return Err(AppError::new(
            "Workspace full-text index shadow columns are unsupported.",
        ));
    }
    on_progress()?;
    for index in schema_indexes() {
        let index_definition = conn
            .query_row(
                "SELECT il.\"unique\", il.partial
                   FROM pragma_index_list(?1) AS il
                  WHERE il.name = ?2",
                params![index.table, index.name],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )
            .optional()?;
        if index_definition != Some((0, 0)) {
            return Err(AppError::new(
                "Workspace database is missing a required index.",
            ));
        }
        let mut columns = conn.prepare(
            "SELECT COALESCE(name, ''), COALESCE(coll, ''), key
               FROM pragma_index_xinfo(?1)
              WHERE key = 1
              ORDER BY seqno",
        )?;
        let rows = columns.query_map(params![index.name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;
        let actual = collect_rows(rows)?;
        let expected = index
            .columns
            .iter()
            .map(|column| {
                let collation = if matches!(
                    *column,
                    "sender" | "subject" | "message_id_header" | "normalized_subject"
                ) {
                    "NOCASE"
                } else {
                    "BINARY"
                };
                ((*column).to_string(), collation.to_string(), 1_i64)
            })
            .collect::<Vec<_>>();
        if actual != expected {
            return Err(AppError::new(
                "Workspace database has an invalid required index definition.",
            ));
        }
        on_progress()?;
    }
    let integrity = conn.query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))?;
    if integrity != "ok" {
        return Err(AppError::new(
            "Workspace database failed integrity checking.",
        ));
    }
    on_progress()?;
    let foreign_key_failures =
        conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get::<_, u64>(0)
        })?;
    let canonical_messages = conn.query_row("SELECT COUNT(*) FROM messages", [], |row| {
        row.get::<_, u64>(0)
    })?;
    let attachments = conn.query_row("SELECT COUNT(*) FROM attachments", [], |row| {
        row.get::<_, u64>(0)
    })?;
    let folders = conn.query_row("SELECT COUNT(*) FROM folders", [], |row| {
        row.get::<_, u64>(0)
    })?;
    // External-content FTS5 tables proxy ordinary rowid/count reads to the
    // content table.  The docsize shadow table records the actual indexed row
    // set, including token-empty documents, so it is the read-only parity
    // source for verification.
    let fts_rows = conn.query_row("SELECT COUNT(*) FROM messages_fts_docsize", [], |row| {
        row.get::<_, u64>(0)
    })?;
    on_progress()?;
    let missing_folder_links = conn.query_row(
        "SELECT COUNT(*) FROM messages AS m LEFT JOIN folders AS f ON f.id = m.folder_id WHERE f.id IS NULL",
        [],
        |row| row.get::<_, u64>(0),
    )?;
    let missing_attachment_links = conn.query_row(
        "SELECT COUNT(*) FROM attachments AS a LEFT JOIN messages AS m ON m.id = a.message_id WHERE m.id IS NULL",
        [],
        |row| row.get::<_, u64>(0),
    )?;
    let duplicate_folder_paths = conn.query_row(
        "SELECT COUNT(*) FROM (SELECT path FROM folders GROUP BY path HAVING COUNT(*) > 1)",
        [],
        |row| row.get::<_, u64>(0),
    )?;
    let duplicate_attachment_relations = conn.query_row(
        "SELECT COUNT(*) FROM (SELECT message_id, attachment_index FROM attachments GROUP BY message_id, attachment_index HAVING COUNT(*) > 1)",
        [],
        |row| row.get::<_, u64>(0),
    )?;
    let attachment_flag_mismatches = conn.query_row(
        "SELECT COUNT(*) FROM messages AS m WHERE (m.has_attachments != 0) != EXISTS (SELECT 1 FROM attachments AS a WHERE a.message_id = m.id)",
        [],
        |row| row.get::<_, u64>(0),
    )?;
    let fts_missing = conn.query_row(
        "SELECT COUNT(*) FROM (SELECT id FROM messages EXCEPT SELECT id FROM messages_fts_docsize)",
        [],
        |row| row.get::<_, u64>(0),
    )?;
    let fts_unexpected = conn.query_row(
        "SELECT COUNT(*) FROM (SELECT id FROM messages_fts_docsize EXCEPT SELECT id FROM messages)",
        [],
        |row| row.get::<_, u64>(0),
    )?;
    on_progress()?;

    let mut manifest_paths = HashSet::with_capacity(manifest.candidate_records.len());
    for (index, candidate) in manifest.candidate_records.iter().enumerate() {
        manifest_paths.insert(candidate.relative_path.as_str());
        if (index + 1) % VERIFICATION_SCAN_PROGRESS_BATCH_SIZE == 0 {
            on_progress()?;
        }
    }
    let mut statement = conn.prepare("SELECT eml_path FROM messages")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut database_paths = HashSet::with_capacity(canonical_messages as usize);
    let mut invalid_database_path = false;
    for (index, row) in rows.enumerate() {
        let path = row?;
        if !validate_relative_identity(&path) || !database_paths.insert(path) {
            invalid_database_path = true;
        }
        if (index + 1) % VERIFICATION_SCAN_PROGRESS_BATCH_SIZE == 0 {
            on_progress()?;
        }
    }
    drop(statement);
    let mut missing_canonical_paths = HashSet::new();
    for (index, path) in manifest_paths.iter().enumerate() {
        if !database_paths.contains(*path) {
            missing_canonical_paths.insert((*path).to_string());
        }
        if (index + 1) % VERIFICATION_SCAN_PROGRESS_BATCH_SIZE == 0 {
            on_progress()?;
        }
    }
    let missing_canonical = missing_canonical_paths.len() as u64;
    let mut unexpected_canonical_paths = HashSet::new();
    for (index, path) in database_paths.iter().enumerate() {
        if !manifest_paths.contains(path.as_str()) {
            unexpected_canonical_paths.insert(path.clone());
        }
        if (index + 1) % VERIFICATION_SCAN_PROGRESS_BATCH_SIZE == 0 {
            on_progress()?;
        }
    }
    let unexpected_canonical = unexpected_canonical_paths.len() as u64;
    on_progress()?;

    let conversation_metadata_complete = metadata_value(&conn, "conversation_schema_version")?
        .as_deref()
        == Some(CONVERSATION_SCHEMA_VERSION);
    let conversation_invalid = conn.query_row(
        "SELECT COUNT(*)
           FROM messages AS m
           LEFT JOIN messages AS root ON root.id = m.conversation_root_id
           LEFT JOIN messages AS parent ON parent.id = m.conversation_parent_id
          WHERE COALESCE(m.conversation_id, '') = ''
             OR root.id IS NULL
             OR root.conversation_id != m.conversation_id
             OR (m.conversation_parent_id IS NOT NULL
                 AND (parent.id IS NULL OR parent.conversation_id != m.conversation_id))",
        [],
        |row| row.get::<_, u64>(0),
    )?;
    let conversation_state = if conversation_metadata_complete && conversation_invalid == 0 {
        "complete"
    } else {
        "incomplete"
    }
    .to_string();
    on_progress()?;

    let required_counter = |key: &str| -> Option<u64> {
        metadata_value(&conn, key)
            .ok()
            .flatten()
            .and_then(|value| value.parse().ok())
    };
    let required_text = |key: &str| -> Option<String> { metadata_value(&conn, key).ok().flatten() };
    let import_status_valid = matches!(
        metadata_value(&conn, "import_status")?.as_deref(),
        Some("complete" | "verifying")
    );
    let provenance_mismatch = required_text("workspace_id").as_deref()
        != Some(manifest.workspace_id.as_str())
        || required_text("pst_content_fingerprint").as_deref()
            != Some(manifest.source_fingerprint.as_str())
        || required_text("canonical_import_identity_version").as_deref()
            != Some(manifest.canonical_identity_version.as_str())
        || required_text("importer_generation").as_deref()
            != Some(manifest.importer_generation.as_str())
        || required_text("candidate_identity_manifest").as_deref()
            != Some(manifest.candidate_aggregate_digest.as_str());
    // `attachments_indexed` is intentionally a Phase 1 per-pass counter so an
    // idempotent resume reports only newly inserted attachments.  The additive
    // cumulative key is transactionally refreshed from SQLite on each batch.
    let pass_attachment_count = required_counter("attachments_indexed");
    let cumulative_attachment_count = required_counter("canonical_attachment_total");
    let cumulative_attachment_count_valid = cumulative_attachment_count == Some(attachments)
        || (cumulative_attachment_count.is_none()
            && manifest.extraction_result == "existing_extraction");
    let counter_mismatch = required_counter("candidates_discovered")
        != Some(manifest.candidate_count)
        || required_counter("candidates_processed") != Some(manifest.candidate_count)
        || required_counter("canonical_messages_inserted") != Some(manifest.inserted_count)
        || required_counter("existing_canonical_messages_skipped")
            != Some(manifest.skipped_existing_count)
        || required_counter("parse_failures") != Some(manifest.parse_failure_count)
        || pass_attachment_count.is_none_or(|count| count > attachments)
        || !cumulative_attachment_count_valid
        || required_counter("canonical_messages_total") != Some(canonical_messages)
        || required_counter("message_count_indexed") != Some(canonical_messages)
        || required_counter("structurally_empty_artifacts_suppressed") != Some(0)
        || manifest
            .inserted_count
            .checked_add(manifest.skipped_existing_count)
            != Some(manifest.candidate_count)
        || manifest.canonical_message_total != canonical_messages
        || manifest.attachment_count != attachments
        || manifest.folder_count != folders
        || manifest.fts_row_count != fts_rows;
    let checks = DatabaseChecks {
        canonical_messages,
        attachments,
        folders,
        fts_rows,
        missing_canonical,
        missing_canonical_paths,
        unexpected_canonical,
        unexpected_canonical_paths,
        conversation_state: conversation_state.clone(),
        inconsistent: foreign_key_failures != 0
            || missing_folder_links != 0
            || missing_attachment_links != 0
            || duplicate_folder_paths != 0
            || duplicate_attachment_relations != 0
            || attachment_flag_mismatches != 0
            || fts_missing != 0
            || fts_unexpected != 0
            || invalid_database_path
            || conversation_invalid != 0
            || !import_status_valid
            || provenance_mismatch
            || manifest.conversation_assignment_state != conversation_state,
        counter_mismatch,
    };
    on_progress()?;
    conn.execute_batch("COMMIT")?;
    drop(cancellation);
    drop(conn);
    require_unchanged_sqlite_file(index_path, initial_file_state)
        .map_err(|_| AppError::new("Workspace database changed during internal verification."))?;
    Ok(checks)
}

#[cfg(test)]
pub(crate) fn verify_replacement_index(
    workspace: &Path,
    workspace_id: &str,
    index_path: &Path,
) -> AppResult<()> {
    let cancel_requested = AtomicBool::new(false);
    verify_replacement_index_with_progress(
        workspace,
        workspace_id,
        index_path,
        &cancel_requested,
        || Ok(()),
    )
}

pub(crate) fn verify_replacement_index_with_progress<P>(
    workspace: &Path,
    workspace_id: &str,
    index_path: &Path,
    cancel_requested: &AtomicBool,
    mut on_progress: P,
) -> AppResult<()>
where
    P: FnMut() -> AppResult<()>,
{
    let manifest = read_manifest(workspace, workspace_id)?
        .ok_or_else(|| AppError::new("Import manifest is missing."))?;
    verify_replacement_index_against_manifest(
        workspace,
        index_path,
        &manifest,
        cancel_requested,
        &mut on_progress,
    )
}

pub(crate) fn verify_staged_replacement_index_with_progress<P>(
    workspace: &Path,
    workspace_id: &str,
    index_path: &Path,
    cancel_requested: &AtomicBool,
    mut on_progress: P,
) -> AppResult<()>
where
    P: FnMut() -> AppResult<()>,
{
    let manifest: ImportManifest =
        read_strict_json(workspace, STAGED_IMPORT_MANIFEST_FILE, MAX_MANIFEST_BYTES)?
            .ok_or_else(|| AppError::new("Index-replacement manifest is missing."))?;
    validate_manifest(&manifest, workspace_id)?;
    verify_replacement_index_against_manifest(
        workspace,
        index_path,
        &manifest,
        cancel_requested,
        &mut on_progress,
    )
}

fn verify_replacement_index_against_manifest<P>(
    workspace: &Path,
    index_path: &Path,
    manifest: &ImportManifest,
    cancel_requested: &AtomicBool,
    on_progress: &mut P,
) -> AppResult<()>
where
    P: FnMut() -> AppResult<()>,
{
    let extract_dir = workspace.join("extracted");
    let extraction_before = scan_extraction_pass_with_progress(
        &extract_dir,
        manifest,
        VerificationLevel::Quick,
        &mut *on_progress,
    )?;
    let checks =
        database_checks_with_progress(index_path, manifest, cancel_requested, &mut *on_progress)?;
    let extraction_after = scan_extraction_pass_with_progress(
        &extract_dir,
        manifest,
        VerificationLevel::Quick,
        &mut *on_progress,
    )?;
    let (found, content_mismatch) = finish_extraction_scan(extraction_before, extraction_after)?;
    let mut expected = HashSet::with_capacity(manifest.candidate_records.len());
    for (index, candidate) in manifest.candidate_records.iter().enumerate() {
        expected.insert(candidate.relative_path.as_str());
        if (index + 1) % VERIFICATION_SCAN_PROGRESS_BATCH_SIZE == 0 {
            on_progress()?;
        }
    }
    let mut unexpected_found = false;
    for (index, path) in found.iter().enumerate() {
        if !expected.contains(path.as_str()) {
            unexpected_found = true;
        }
        if (index + 1) % VERIFICATION_SCAN_PROGRESS_BATCH_SIZE == 0 {
            on_progress()?;
        }
    }
    on_progress()?;
    if content_mismatch || found.len() != expected.len() || unexpected_found {
        return Err(AppError::new(
            "Replacement index extraction verification failed.",
        ));
    }
    if checks.inconsistent
        || checks.counter_mismatch
        || checks.missing_canonical != 0
        || checks.unexpected_canonical != 0
        || checks.conversation_state != "complete"
        || checks.canonical_messages != manifest.candidate_count
    {
        return Err(AppError::new(
            "Replacement index internal verification failed.",
        ));
    }
    Ok(())
}

fn possible_duplicate_details(
    workspace_id: &str,
    manifest: &ImportManifest,
) -> (u64, Vec<PossibleDuplicateGroupDetail>, Vec<Vec<String>>) {
    let mut by_digest = HashMap::<&str, Vec<&str>>::new();
    for candidate in &manifest.candidate_records {
        by_digest
            .entry(candidate.sha256.as_str())
            .or_default()
            .push(candidate.relative_path.as_str());
    }
    let mut groups = by_digest
        .into_iter()
        .filter(|(_, paths)| paths.len() > 1)
        .collect::<Vec<_>>();
    groups.sort_by_key(|(digest, _)| *digest);
    let total = groups.len() as u64;
    let bounded_groups = groups
        .into_iter()
        .take(MAX_DUPLICATE_DETAILS)
        .map(|(digest, mut paths)| {
            paths.sort_unstable();
            (digest, paths)
        })
        .collect::<Vec<_>>();
    let details = bounded_groups
        .iter()
        .map(|(digest, _)| PossibleDuplicateGroupDetail {
            opaque_id: sha256_text(&format!(
                "pstqv-possible-duplicate-v1\0{workspace_id}\0{digest}"
            )),
            signals: vec!["identical_extracted_content_digest".to_string()],
        })
        .collect();
    let relative_groups = bounded_groups
        .into_iter()
        .map(|(_, paths)| paths.into_iter().map(str::to_string).collect())
        .collect();
    (total, details, relative_groups)
}

fn report_shell(
    operation_id: &str,
    workspace_id: &str,
    level: VerificationLevel,
    state: WorkspaceState,
    started_at: String,
) -> VerificationReport {
    VerificationReport {
        format_version: VERIFICATION_FORMAT_VERSION,
        operation_id: operation_id.to_string(),
        workspace_id: workspace_id.to_string(),
        verification_level: level,
        result: VerificationResult::Failed,
        workspace_state: state,
        discovered_candidates: 0,
        canonical_messages: 0,
        inserted_messages: 0,
        skipped_existing_messages: 0,
        parse_failures: 0,
        attachments: 0,
        folders: 0,
        fts_rows: 0,
        conversation_state: "unknown".to_string(),
        missing_canonical_records: 0,
        unexpected_canonical_records: 0,
        possible_duplicate_groups: 0,
        possible_duplicate_group_details: Vec::new(),
        source_integrity: SourceIntegrity::Unavailable,
        started_at,
        completed_at: String::new(),
        elapsed_duration_ms: 0,
        completed_import_duration_ms: None,
        warnings: Vec::new(),
        recommended_actions: Vec::new(),
    }
}

fn finish_report_timing(report: &mut VerificationReport, started: Instant) {
    report.completed_at = canonical_utc_now_not_before(&report.started_at);
    report.elapsed_duration_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
}

fn allowed_verification_warning(value: &str) -> bool {
    matches!(
        value,
        "Workspace state metadata is malformed or from an unsupported version."
            | "Workspace identity metadata is inconsistent."
            | "Import manifest is missing, malformed, or unsupported."
            | "Import manifest is not available because extraction or discovery is incomplete."
            | "Possible duplicate source records detected."
            | "Extracted source records do not match the persistent import manifest."
            | "Canonical extracted records are missing from the local index."
            | "The local index did not pass internal consistency checks."
            | "Internally verified against this workspace's extracted source records."
            | "The source PST is unavailable; local workspace checks completed, but source comparison could not be performed."
            | "The mounted source PST no longer matches the recorded source identity."
    )
}

fn allowed_recommended_action(value: &str) -> bool {
    matches!(
        value,
        "Use a supported PST QuickView version."
            | "Review the workspace with a supported PST QuickView version."
            | "Resume the interrupted import."
            | "Repair the index from existing extracted files."
            | "Review source availability and extraction integrity."
            | "Resume indexing or repair the index from extracted files."
            | "Rebuild conversations from existing indexed messages."
            | "Reconnect the source volume to compare source identity."
            | "Keep the local workspace and review the changed source separately."
    )
}

fn validate_verification_report(
    report: &VerificationReport,
    expected_workspace_id: &str,
) -> AppResult<()> {
    let started = parse_canonical_utc(&report.started_at);
    let completed = parse_canonical_utc(&report.completed_at);
    let timestamps_invalid = match started.zip(completed) {
        Some((started, completed)) => started > completed,
        None => true,
    };
    let mut warning_set = HashSet::with_capacity(report.warnings.len());
    let mut action_set = HashSet::with_capacity(report.recommended_actions.len());
    let mut opaque_ids = HashSet::with_capacity(report.possible_duplicate_group_details.len());
    let outcome_counts_invalid = match report
        .inserted_messages
        .checked_add(report.skipped_existing_messages)
    {
        Some(processed) => processed > report.discovered_candidates,
        None => true,
    };
    let duplicate_details_invalid = report.possible_duplicate_group_details.len()
        != usize::try_from(
            report
                .possible_duplicate_groups
                .min(MAX_DUPLICATE_DETAILS as u64),
        )
        .unwrap_or(MAX_DUPLICATE_DETAILS)
        || report.possible_duplicate_group_details.iter().any(|group| {
            !is_lower_hex_sha256(&group.opaque_id)
                || !opaque_ids.insert(group.opaque_id.as_str())
                || group.signals.as_slice() != ["identical_extracted_content_digest"]
        });
    let result_state_invalid = match report.result {
        VerificationResult::Verified => {
            report.workspace_state != WorkspaceState::Complete
                || report.source_integrity != SourceIntegrity::Passed
                || report.conversation_state != "complete"
                || report.missing_canonical_records != 0
                || report.unexpected_canonical_records != 0
                || report.canonical_messages != report.discovered_candidates
                || report.fts_rows != report.canonical_messages
                || report
                    .inserted_messages
                    .checked_add(report.skipped_existing_messages)
                    != Some(report.discovered_candidates)
        }
        VerificationResult::SourceUnavailable => {
            report.source_integrity != SourceIntegrity::Unavailable
                || report.conversation_state != "complete"
                || report.missing_canonical_records != 0
                || report.unexpected_canonical_records != 0
                || report.canonical_messages != report.discovered_candidates
                || report.fts_rows != report.canonical_messages
                || report
                    .inserted_messages
                    .checked_add(report.skipped_existing_messages)
                    != Some(report.discovered_candidates)
        }
        VerificationResult::SourceChanged => {
            report.source_integrity != SourceIntegrity::Changed
                || report.conversation_state != "complete"
                || report.missing_canonical_records != 0
                || report.unexpected_canonical_records != 0
                || report.canonical_messages != report.discovered_candidates
                || report.fts_rows != report.canonical_messages
                || report
                    .inserted_messages
                    .checked_add(report.skipped_existing_messages)
                    != Some(report.discovered_candidates)
        }
        _ => report.workspace_state == WorkspaceState::Complete,
    };
    let numeric_value_invalid = [
        report.discovered_candidates,
        report.canonical_messages,
        report.inserted_messages,
        report.skipped_existing_messages,
        report.parse_failures,
        report.attachments,
        report.folders,
        report.fts_rows,
        report.missing_canonical_records,
        report.unexpected_canonical_records,
        report.possible_duplicate_groups,
        report.elapsed_duration_ms,
    ]
    .into_iter()
    .any(|value| value > MAX_SAFE_JSON_INTEGER)
        || report
            .completed_import_duration_ms
            .is_some_and(|value| value > MAX_SAFE_JSON_INTEGER);
    if report.format_version != VERIFICATION_FORMAT_VERSION
        || report.workspace_id != expected_workspace_id
        || !is_lower_hex_sha256(&report.workspace_id)
        || validate_operation_id(&report.operation_id).is_err()
        || timestamps_invalid
        || numeric_value_invalid
        || !matches!(
            report.conversation_state.as_str(),
            "unknown" | "incomplete" | "complete"
        )
        || outcome_counts_invalid
        || report.parse_failures > report.canonical_messages
        || report.missing_canonical_records > report.discovered_candidates
        || report.possible_duplicate_groups > report.discovered_candidates / 2
        || duplicate_details_invalid
        || report.warnings.len() > 12
        || report.warnings.iter().any(|value| {
            !allowed_verification_warning(value) || !warning_set.insert(value.as_str())
        })
        || report.recommended_actions.len() > 8
        || report
            .recommended_actions
            .iter()
            .any(|value| !allowed_recommended_action(value) || !action_set.insert(value.as_str()))
        || result_state_invalid
    {
        return Err(AppError::new(
            "Last verification report is malformed or unsupported.",
        ));
    }
    Ok(())
}

fn persist_report(workspace: &Path, report: &VerificationReport) -> AppResult<()> {
    validate_verification_report(report, &report.workspace_id)?;
    write_strict_json(workspace, LAST_VERIFICATION_FILE, report)
}

fn verification_report_proves_complete_lineage(report: &VerificationReport) -> bool {
    report.workspace_state == WorkspaceState::Complete
        && matches!(
            report.result,
            VerificationResult::Verified
                | VerificationResult::SourceUnavailable
                | VerificationResult::SourceChanged
        )
}

pub(crate) fn get_last_verification(
    workspace: &Path,
    workspace_id: &str,
) -> AppResult<Option<VerificationReport>> {
    let report: Option<VerificationReport> =
        read_strict_json(workspace, LAST_VERIFICATION_FILE, MAX_VERIFICATION_BYTES)?;
    if let Some(report) = &report {
        validate_verification_report(report, workspace_id)?;
    }
    Ok(report)
}

pub(crate) fn sync_completed_duration_to_last_verification(
    workspace: &Path,
    workspace_id: &str,
) -> AppResult<()> {
    let Some(state) = read_state(workspace, workspace_id)? else {
        return Ok(());
    };
    let Some(completed_duration_ms) = state.completed_duration_ms else {
        return Ok(());
    };
    let Some(mut report) = get_last_verification(workspace, workspace_id)? else {
        return Ok(());
    };
    if report.completed_import_duration_ms == Some(completed_duration_ms) {
        return Ok(());
    }
    report.completed_import_duration_ms = Some(completed_duration_ms);
    persist_report(workspace, &report)
}

fn set_database_import_status(workspace: &Path, status: &str) -> AppResult<()> {
    let mut conn = open_workspace_db_for_upgrade(workspace)?;
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    set_metadata_value(&transaction, "import_status", status)?;
    set_metadata_value(&transaction, "updated_at", canonical_utc_now())?;
    transaction.commit()?;
    Ok(())
}

pub(crate) fn verify_active_workspace(
    app: &tauri::AppHandle,
    active: &ActiveWorkspace,
    operation_id: &str,
    level: VerificationLevel,
) -> AppResult<VerificationReport> {
    verify_active_workspace_with_policy(app, active, operation_id, level, true)
}

pub(crate) fn verify_active_workspace_after_commit(
    app: &tauri::AppHandle,
    active: &ActiveWorkspace,
    operation_id: &str,
    level: VerificationLevel,
) -> AppResult<VerificationReport> {
    verify_active_workspace_with_policy(app, active, operation_id, level, false)
}

fn verify_active_workspace_with_policy(
    app: &tauri::AppHandle,
    active: &ActiveWorkspace,
    operation_id: &str,
    level: VerificationLevel,
    honor_cancellation: bool,
) -> AppResult<VerificationReport> {
    let state = app.state::<AppState>();
    let never_cancel = AtomicBool::new(false);
    let cancel_requested = if honor_cancellation {
        &state.cancel_import_requested
    } else {
        &never_cancel
    };
    if cancel_requested.load(Ordering::SeqCst) {
        return Err(import_cancelled_error(
            "Workspace verification was cancelled.",
        ));
    }
    // Once verification has committed its report and final state, completion
    // wins over a cancellation request that arrived at that boundary. This is
    // required for replacement promotion: a late cancellation must never be
    // reported as though the prior active index were still installed.
    emit_verification_progress(app, 0, None);
    verify_active_workspace_core(
        active,
        operation_id,
        level,
        cancel_requested,
        |completed, report| {
            if cancel_requested.load(Ordering::SeqCst) {
                return Err(import_cancelled_error(
                    "Workspace verification was cancelled at a bounded checkpoint.",
                ));
            }
            emit_verification_progress(app, completed, Some(report));
            Ok(())
        },
        |discovered, activity_observed| {
            emit_manifest_adoption_progress_with_policy(
                app,
                discovered,
                activity_observed,
                honor_cancellation,
            )
        },
        // The check-4 callback above is the final cancellation gate. Once the
        // verified database/workspace metadata writes begin, a late request
        // cannot reinterpret their committed result as cancelled.
        || checkpoint_integrity_operation_with_policy(app, true, false, true),
    )
}

fn verify_active_workspace_core<P, M, C>(
    active: &ActiveWorkspace,
    operation_id: &str,
    level: VerificationLevel,
    cancel_requested: &AtomicBool,
    mut on_check_completed: P,
    mut on_manifest_progress: M,
    mut on_completion_boundary: C,
) -> AppResult<VerificationReport>
where
    P: FnMut(u64, &VerificationReport) -> AppResult<()>,
    M: FnMut(usize, bool) -> AppResult<()>,
    C: FnMut() -> AppResult<()>,
{
    validate_operation_id(operation_id)?;
    let timer = Instant::now();
    let started_at = canonical_utc_now();
    let persisted_state = read_state(&active.path, &active.id);
    let had_persisted_state = persisted_state.as_ref().is_ok_and(Option::is_some);
    let observed_state = persisted_state
        .as_ref()
        .ok()
        .and_then(|state| state.as_ref())
        .map(|state| state.state)
        .unwrap_or(WorkspaceState::Incomplete);
    // Compatibility metadata is attacker-controlled local input. Validate it
    // before legacy adoption can create import-state.json or any other Phase 2
    // sidecar. Corrupt/missing indexes remain inspectable by Verify, but a
    // readable future schema or unknown status fails closed without mutation.
    let initial_workspace_status = match workspace_metadata_status_strict(&active.path) {
        Ok(status) => status,
        Err(_) => {
            let mut report =
                report_shell(operation_id, &active.id, level, observed_state, started_at);
            report.result = VerificationResult::Unsupported;
            report
                .warnings
                .push("Workspace compatibility metadata is malformed or unsupported.".to_string());
            report
                .recommended_actions
                .push("Use a supported PST QuickView version.".to_string());
            finish_report_timing(&mut report, timer);
            return Ok(report);
        }
    };
    let initial_database_status =
        match database_import_status_strict_at_path(&active.path.join("index.sqlite"), false, true)
        {
            Ok(status) => status,
            Err(_) => {
                let mut report =
                    report_shell(operation_id, &active.id, level, observed_state, started_at);
                report.result = VerificationResult::Unsupported;
                report
                    .warnings
                    .push("Workspace database status is unsupported.".to_string());
                report
                    .recommended_actions
                    .push("Use a supported PST QuickView version.".to_string());
                finish_report_timing(&mut report, timer);
                return Ok(report);
            }
        };
    let initial_state = match persisted_state {
        Ok(Some(state)) => state,
        Ok(None) => match ensure_state_for_workspace(&active.path, &active.id) {
            Ok(state) => state,
            Err(_) => {
                let mut report = report_shell(
                    operation_id,
                    &active.id,
                    level,
                    WorkspaceState::Incomplete,
                    started_at,
                );
                report.result = VerificationResult::Unsupported;
                report.warnings.push(
                    "Workspace state metadata is malformed or from an unsupported version."
                        .to_string(),
                );
                report
                    .recommended_actions
                    .push("Use a supported PST QuickView version.".to_string());
                finish_report_timing(&mut report, timer);
                return Ok(report);
            }
        },
        Err(_) => {
            let mut report = report_shell(
                operation_id,
                &active.id,
                level,
                WorkspaceState::Incomplete,
                started_at,
            );
            report.result = VerificationResult::Unsupported;
            report.warnings.push(
                "Workspace state metadata is malformed or from an unsupported version.".to_string(),
            );
            report
                .recommended_actions
                .push("Use a supported PST QuickView version.".to_string());
            finish_report_timing(&mut report, timer);
            return Ok(report);
        }
    };
    let prior_complete_report = get_last_verification(&active.path, &active.id)
        .ok()
        .flatten()
        .is_some_and(|report| verification_report_proves_complete_lineage(&report));
    let state_has_complete_lineage = state_has_complete_lineage(&initial_state);
    let prior_complete_reverification = prior_complete_report || state_has_complete_lineage;
    let was_complete = state_has_complete_lineage || prior_complete_report;
    let mut report = report_shell(
        operation_id,
        &active.id,
        level,
        initial_state.state,
        started_at,
    );
    let identity = match identity_from_active_workspace(active) {
        Ok(identity) => identity,
        Err(_) => {
            report.result = VerificationResult::Unsupported;
            report
                .warnings
                .push("Workspace identity metadata is inconsistent.".to_string());
            report
                .recommended_actions
                .push("Review the workspace with a supported PST QuickView version.".to_string());
            finish_report_timing(&mut report, timer);
            return Ok(report);
        }
    };
    let manifest_read = read_manifest(&active.path, &active.id);
    let missing_manifest_is_expected = had_persisted_state
        && manifest_read.as_ref().is_ok_and(Option::is_none)
        && (matches!(
            initial_state.state,
            WorkspaceState::Preflight
                | WorkspaceState::Extracting
                | WorkspaceState::ExtractionComplete
                | WorkspaceState::Discovering
        ) || initial_state.interrupted_stage.is_some_and(|stage| {
            matches!(
                stage,
                WorkspaceState::Preflight
                    | WorkspaceState::Extracting
                    | WorkspaceState::ExtractionComplete
                    | WorkspaceState::Discovering
            )
        }));
    let legacy_manifest_adoption = !had_persisted_state
        || (!initial_state.active_timing_history_complete
            && initial_state.reason.as_deref() == Some("verification_required"));
    let manifest_result = match manifest_read {
        Ok(Some(_)) => {
            ensure_phase2_manifest_with_progress(active, &identity, &mut on_manifest_progress)
        }
        Ok(None) if legacy_manifest_adoption => {
            ensure_phase2_manifest_with_progress(active, &identity, &mut on_manifest_progress)
        }
        Ok(None) => Err(AppError::new("Import manifest is missing.")),
        Err(error) => Err(error),
    };
    let manifest = match manifest_result {
        Ok(manifest) => manifest,
        Err(_) => {
            if missing_manifest_is_expected {
                report.result = VerificationResult::Incomplete;
                report.warnings.push(
                    "Import manifest is not available because extraction or discovery is incomplete."
                        .to_string(),
                );
                report
                    .recommended_actions
                    .push("Resume the interrupted import.".to_string());
            } else {
                report.result = VerificationResult::Unsupported;
                report
                    .warnings
                    .push("Import manifest is missing, malformed, or unsupported.".to_string());
                report
                    .recommended_actions
                    .push("Repair the index from existing extracted files.".to_string());
            }
            finish_report_timing(&mut report, timer);
            return Ok(report);
        }
    };
    on_check_completed(1, &report)?;
    let initial_state = if initial_state.state.is_transient()
        && initial_state.active_session_started_at.is_none()
    {
        recover_interrupted_state(&active.path, &active.id, initial_state)?
    } else {
        initial_state
    };
    let missing_verified_lineage = !initial_state.verified_complete_lineage
        && initial_state.completed_duration_ms.is_none()
        && initial_state.last_completed_stage != Some(WorkspaceState::Verifying)
        && !prior_complete_report;
    let persisted_complete_contradiction = was_complete
        && (initial_database_status.as_deref() != Some("complete")
            || initial_workspace_status.as_deref() != Some("complete")
            || initial_state.discovered_candidates != manifest.candidate_count
            || initial_state.canonical_indexed != manifest.canonical_message_total
            || missing_verified_lineage
            || (initial_state.interrupted_stage.is_some() && !prior_complete_reverification));
    let verifying = transition_state(
        &active.path,
        &active.id,
        WorkspaceState::Verifying,
        StateCounts {
            discovered_candidates: Some(initial_state.discovered_candidates as usize),
            canonical_indexed: Some(initial_state.canonical_indexed as usize),
        },
        Some("verification_running"),
    )?;
    report.workspace_state = verifying.state;
    report.discovered_candidates = manifest.candidate_count;
    report.inserted_messages = manifest.inserted_count;
    report.skipped_existing_messages = manifest.skipped_existing_count;
    report.parse_failures = manifest.parse_failure_count;
    report.source_integrity = source_probe(active, &manifest);
    let (duplicate_groups, duplicate_details, _) =
        possible_duplicate_details(&active.id, &manifest);
    report.possible_duplicate_groups = duplicate_groups;
    report.possible_duplicate_group_details = duplicate_details;
    if duplicate_groups > 0 {
        report
            .warnings
            .push("Possible duplicate source records detected.".to_string());
    }
    on_check_completed(2, &report)?;

    let extract_dir = active.path.join("extracted");
    let extraction_before =
        scan_extraction_pass_with_progress(&extract_dir, &manifest, level, || {
            on_check_completed(2, &report)
        });
    let extraction = extraction_before.and_then(|before| {
        let after = scan_extraction_pass_with_progress(&extract_dir, &manifest, level, || {
            on_check_completed(2, &report)
        })?;
        finish_extraction_scan(before, after)
    });
    let extraction = match extraction {
        Err(error) if error.code == Some(IMPORT_CANCELLED_CODE) => return Err(error),
        result => result,
    };
    on_check_completed(3, &report)?;
    let database = database_checks_with_progress(
        &active.path.join("index.sqlite"),
        &manifest,
        cancel_requested,
        || on_check_completed(3, &report),
    );
    let database = match database {
        Err(error) if error.code == Some(IMPORT_CANCELLED_CODE) => return Err(error),
        result => result,
    };
    let mut local_verified = false;
    match (extraction, database) {
        (Err(_), _) => {
            report.result = VerificationResult::ExtractionInconsistent;
            report.missing_canonical_records = manifest.candidate_count;
            report.warnings.push(
                "Extracted source records do not match the persistent import manifest.".to_string(),
            );
            report
                .recommended_actions
                .push("Review source availability and extraction integrity.".to_string());
        }
        (Ok((found, content_mismatch)), Ok(checks)) => {
            report.canonical_messages = checks.canonical_messages;
            report.attachments = checks.attachments;
            report.folders = checks.folders;
            report.fts_rows = checks.fts_rows;
            report.conversation_state = checks.conversation_state.clone();
            let expected_paths = manifest
                .candidate_records
                .iter()
                .map(|candidate| candidate.relative_path.as_str())
                .collect::<HashSet<_>>();
            let missing_extracted_paths = expected_paths
                .iter()
                .filter(|path| !found.contains(**path))
                .map(|path| (*path).to_string())
                .collect::<HashSet<_>>();
            let missing_extracted = missing_extracted_paths.len();
            let mut missing_records = checks.missing_canonical_paths.clone();
            missing_records.extend(missing_extracted_paths);
            report.missing_canonical_records = missing_records.len() as u64;
            let unexpected_extracted_paths = found
                .iter()
                .filter(|path| !expected_paths.contains(path.as_str()))
                .cloned()
                .collect::<HashSet<_>>();
            let unexpected_extracted = unexpected_extracted_paths.len();
            let mut unexpected_records = checks.unexpected_canonical_paths.clone();
            unexpected_records.extend(unexpected_extracted_paths);
            report.unexpected_canonical_records = unexpected_records.len() as u64;
            if missing_extracted != 0 || unexpected_extracted != 0 || content_mismatch {
                report.result = VerificationResult::ExtractionInconsistent;
                report.warnings.push(
                    "Extracted source records do not match the persistent import manifest."
                        .to_string(),
                );
                report
                    .recommended_actions
                    .push("Review source availability and extraction integrity.".to_string());
            } else if persisted_complete_contradiction {
                report.result = VerificationResult::DatabaseInconsistent;
                report
                    .warnings
                    .push("The local index did not pass internal consistency checks.".to_string());
                report
                    .recommended_actions
                    .push("Repair the index from existing extracted files.".to_string());
            } else if checks.missing_canonical != 0 {
                report.result = VerificationResult::Repairable;
                report.warnings.push(
                    "Canonical extracted records are missing from the local index.".to_string(),
                );
                report
                    .recommended_actions
                    .push("Resume indexing or repair the index from extracted files.".to_string());
            } else if checks.unexpected_canonical != 0
                || checks.inconsistent
                || checks.counter_mismatch
                || checks.conversation_state != "complete"
            {
                report.result = VerificationResult::DatabaseInconsistent;
                report
                    .warnings
                    .push("The local index did not pass internal consistency checks.".to_string());
                if checks.conversation_state != "complete"
                    && !checks.inconsistent
                    && !checks.counter_mismatch
                    && checks.unexpected_canonical == 0
                {
                    report
                        .recommended_actions
                        .push("Rebuild conversations from existing indexed messages.".to_string());
                } else {
                    report
                        .recommended_actions
                        .push("Repair the index from existing extracted files.".to_string());
                }
            } else {
                local_verified = true;
                report.result = match report.source_integrity {
                    SourceIntegrity::Passed => VerificationResult::Verified,
                    SourceIntegrity::Unavailable => VerificationResult::SourceUnavailable,
                    SourceIntegrity::Changed => VerificationResult::SourceChanged,
                };
                match report.source_integrity {
                    SourceIntegrity::Passed => report.warnings.push(
                        "Internally verified against this workspace's extracted source records."
                            .to_string(),
                    ),
                    SourceIntegrity::Unavailable => {
                        report.warnings.push(
                            "The source PST is unavailable; local workspace checks completed, but source comparison could not be performed."
                                .to_string(),
                        );
                        report.recommended_actions.push(
                            "Reconnect the source volume to compare source identity.".to_string(),
                        );
                    }
                    SourceIntegrity::Changed => {
                        report.warnings.push(
                            "The mounted source PST no longer matches the recorded source identity."
                                .to_string(),
                        );
                        report.recommended_actions.push(
                            "Keep the local workspace and review the changed source separately."
                                .to_string(),
                        );
                    }
                }
            }
        }
        (Ok(_), Err(_)) => {
            report.result = VerificationResult::DatabaseInconsistent;
            report
                .warnings
                .push("The local index did not pass internal consistency checks.".to_string());
            report
                .recommended_actions
                .push("Repair the index from existing extracted files.".to_string());
        }
    }
    on_check_completed(4, &report)?;

    run_before_verification_finalization_hook();
    let complete =
        local_verified && (report.source_integrity == SourceIntegrity::Passed || was_complete);
    let state_canonical_indexed = report
        .canonical_messages
        .checked_sub(report.unexpected_canonical_records)
        .ok_or_else(|| AppError::new("Verification canonical counters are contradictory."))?;
    if state_canonical_indexed > report.discovered_candidates {
        return Err(AppError::new(
            "Verification canonical counters are contradictory.",
        ));
    }
    let final_state = if complete {
        set_database_import_status(&active.path, ImportStatus::Complete.as_str())?;
        let compatibility_stats = IndexStats {
            discovered: report.discovered_candidates as usize,
            processed: report.discovered_candidates as usize,
            indexed: report.canonical_messages as usize,
            inserted: report.inserted_messages as usize,
            skipped_existing: report.skipped_existing_messages as usize,
            error_count: report.parse_failures as usize,
            attachments_indexed: report.attachments as usize,
            ..IndexStats::default()
        };
        mark_existing_workspace_complete_file_only(
            &active.path,
            &identity,
            active.location_mode,
            &compatibility_stats,
        )?;
        // Capture the exact monotonic import duration after every final
        // verification metadata write and immediately before Complete becomes
        // durable. If the app stops after the transition, recovery can report
        // the completed duration without inventing a stale checkpoint value.
        on_completion_boundary()?;
        transition_state(
            &active.path,
            &active.id,
            WorkspaceState::Complete,
            StateCounts {
                discovered_candidates: Some(report.discovered_candidates as usize),
                canonical_indexed: Some(state_canonical_indexed as usize),
            },
            Some("verification_complete"),
        )?
    } else {
        transition_state(
            &active.path,
            &active.id,
            WorkspaceState::Incomplete,
            StateCounts {
                discovered_candidates: Some(report.discovered_candidates as usize),
                canonical_indexed: Some(state_canonical_indexed as usize),
            },
            Some("verification_incomplete"),
        )?
    };
    let _ = verifying;
    report.workspace_state = final_state.state;
    report.completed_import_duration_ms = final_state.completed_duration_ms;
    finish_report_timing(&mut report, timer);
    persist_report(&active.path, &report)?;
    append_import_log(
        &active.path,
        &format!(
            "state=verification_complete level={:?} result={:?} canonical={} missing={} unexpected={} possible_duplicate_groups={} elapsed_ms={}",
            level,
            report.result,
            report.canonical_messages,
            report.missing_canonical_records,
            report.unexpected_canonical_records,
            report.possible_duplicate_groups,
            report.elapsed_duration_ms
        ),
    )?;
    Ok(report)
}

fn source_availability_for_snapshot(
    source_path: Option<&Path>,
    manifest: Option<&ImportManifest>,
) -> &'static str {
    let Some(source_path) = source_path else {
        return "unavailable";
    };
    let Ok(metadata) = fs::symlink_metadata(source_path) else {
        return "unavailable";
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.nlink() != 1 {
        return "changed";
    }
    let Some(manifest) = manifest else {
        return "available";
    };
    let modified_ns = metadata
        .modified()
        .unwrap_or(UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    if metadata.len() == manifest.source_size_after
        && modified_ns.to_string() == manifest.source_modified_ns_after
    {
        "available"
    } else {
        "changed"
    }
}

fn resume_reason(code: Option<&str>) -> &'static str {
    match code {
        Some("previous_import_cancelled") => {
            "The previous import was cancelled before internal verification."
        }
        Some("previous_import_failed") => {
            "The previous import failed before internal verification."
        }
        Some("identity_inconsistent") => "Workspace identity metadata is inconsistent.",
        Some("manifest_unavailable") => "The import manifest is unavailable.",
        Some("verification_incomplete") => {
            "Internal workspace verification did not complete successfully."
        }
        Some("import_cancelled") => "The import was cancelled before completion.",
        Some("import_failed") => "The import failed before completion.",
        Some("repair_cancelled") => "Index repair was cancelled before completion.",
        Some("repair_failed") => "Index repair failed before completion.",
        Some("resume_cancelled") => "Resume indexing was cancelled before completion.",
        Some("resume_failed") => "Resume indexing failed before completion.",
        Some("conversation_rebuild_cancelled") => {
            "Conversation rebuilding was cancelled before completion."
        }
        Some("conversation_rebuild_failed") => "Conversation rebuilding failed before completion.",
        Some("promotion_finalization_pending") => {
            "The replacement index is active, but final internal verification has not completed."
        }
        Some("interrupted_process") => {
            "The previous operation was interrupted before internal verification completed."
        }
        _ => "The workspace has not completed internal verification.",
    }
}

pub(crate) fn integrity_snapshot(
    workspace: &Path,
    workspace_id: &str,
    source_path: Option<&Path>,
) -> IntegritySnapshot {
    let state_result = read_state(workspace, workspace_id);
    let state_invalid = state_result.is_err();
    let state = state_result.ok().flatten();
    let manifest_result = read_manifest(workspace, workspace_id);
    let manifest_invalid = manifest_result.is_err();
    let manifest = manifest_result.ok().flatten();
    let workspace_status_result = workspace_metadata_status_strict(workspace);
    let workspace_status_invalid = workspace_status_result.is_err();
    let workspace_status = workspace_status_result.ok().flatten();
    let index_path = workspace.join("index.sqlite");
    let future_schema = probe_sqlite_user_version(&index_path)
        .is_ok_and(|version| version.is_some_and(|value| value > SQLITE_SCHEMA_VERSION_CURRENT));
    let unsupported = state_invalid
        || manifest_invalid
        || workspace_status_invalid
        || future_schema
        || state
            .as_ref()
            .is_some_and(|state| state.state == WorkspaceState::Complete && manifest.is_none());
    let effective_state = state
        .as_ref()
        .map(|record| record.state)
        .unwrap_or(WorkspaceState::Incomplete);
    let complete_status_contradiction = effective_state == WorkspaceState::Complete
        && (open_workspace_db_for_read(workspace)
            .ok()
            .and_then(|conn| metadata_value(&conn, "import_status").ok().flatten())
            .as_deref()
            != Some("complete")
            || workspace_status.as_deref() != Some("complete"));
    let extract_exists = fs::symlink_metadata(workspace.join("extracted"))
        .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_dir());
    let index_exists = fs::symlink_metadata(&index_path).is_ok_and(|metadata| {
        !metadata.file_type().is_symlink() && metadata.is_file() && metadata.nlink() == 1
    });
    let (can_open, conversation_state) = if index_exists {
        match open_workspace_db_for_read(workspace) {
            Ok(conn) => {
                let conversation_complete = conversation_data_is_indexed(&conn).unwrap_or(false)
                    && conn
                        .query_row(
                            "SELECT COUNT(*) FROM messages WHERE COALESCE(conversation_id, '') = '' OR conversation_root_id IS NULL",
                            [],
                            |row| row.get::<_, u64>(0),
                        )
                        .is_ok_and(|invalid| invalid == 0);
                (
                    true,
                    if conversation_complete {
                        "complete".to_string()
                    } else {
                        "incomplete".to_string()
                    },
                )
            }
            Err(_) => (false, "unsupported".to_string()),
        }
    } else {
        (false, "missing".to_string())
    };
    let classification = if unsupported {
        "unsupported"
    } else if effective_state == WorkspaceState::Complete
        && !complete_status_contradiction
        && extract_exists
        && can_open
        && conversation_state == "complete"
        && manifest
            .as_ref()
            .is_some_and(|manifest| manifest.conversation_assignment_state == "complete")
    {
        "complete"
    } else {
        "incomplete"
    };
    let marker_basename = read_workspace_marker(&workspace.join(WORKSPACE_MARKER_FILE))
        .ok()
        .and_then(|marker| {
            Path::new(&marker.original_pst_path)
                .file_name()
                .and_then(OsStr::to_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "Mailbox.pst".to_string());
    let resume_details = state.as_ref().and_then(|record| {
        let (observed_discovered, observed_canonical) = observed_state_counts(workspace, record);
        (classification == "incomplete").then(|| ResumeDetails {
            pst_basename: marker_basename,
            last_completed_stage: record
                .last_completed_stage
                .map(WorkspaceState::as_str)
                .unwrap_or("none")
                .to_string(),
            interrupted_stage: record
                .interrupted_stage
                .or_else(|| record.state.is_transient().then_some(record.state))
                .map(WorkspaceState::as_str)
                .unwrap_or(record.state.as_str())
                .to_string(),
            discovered_candidates: observed_discovered as u64,
            canonical_indexed: observed_canonical as u64,
            remaining_candidates: observed_discovered
                .checked_sub(observed_canonical)
                .map(|value| value as u64),
            last_successful_update: record.last_successful_update.clone(),
            elapsed_duration_ms: record
                .elapsed_duration_ms
                .max(record.accumulated_active_elapsed_ms),
            stage_elapsed_duration_ms: record.stage_elapsed_duration_ms,
            last_activity_at: record.last_activity_at.clone(),
            reason: resume_reason(record.reason.as_deref()).to_string(),
        })
    });
    let reported_state =
        if classification == "incomplete" && effective_state == WorkspaceState::Complete {
            WorkspaceState::Incomplete
        } else {
            effective_state
        };
    IntegritySnapshot {
        state: reported_state,
        classification,
        source_availability: source_availability_for_snapshot(source_path, manifest.as_ref()),
        can_resume_indexing: !unsupported
            && effective_state != WorkspaceState::Complete
            && extract_exists,
        can_repair_index: !unsupported && classification == "incomplete" && extract_exists,
        can_rebuild_conversations: !unsupported && index_exists && conversation_state != "complete",
        resume_details,
        has_last_verification: get_last_verification(workspace, workspace_id)
            .ok()
            .flatten()
            .is_some(),
        conversation_state,
        can_open: can_open && !unsupported,
        completed_duration_ms: state
            .as_ref()
            .and_then(|record| record.completed_duration_ms),
        last_activity_at: state
            .as_ref()
            .and_then(|record| record.last_activity_at.clone()),
        active_elapsed_ms: state
            .as_ref()
            .map(|record| {
                record
                    .elapsed_duration_ms
                    .max(record.accumulated_active_elapsed_ms)
            })
            .unwrap_or(0),
        stage_elapsed_ms: state
            .as_ref()
            .map(|record| record.stage_elapsed_duration_ms)
            .unwrap_or(0),
    }
}

#[cfg(test)]
pub(crate) fn verify_active_workspace_for_test(
    active: &ActiveWorkspace,
    operation_id: &str,
    level: VerificationLevel,
) -> AppResult<VerificationReport> {
    let cancel_requested = AtomicBool::new(false);
    verify_active_workspace_core(
        active,
        operation_id,
        level,
        &cancel_requested,
        |_, _| Ok(()),
        |_, _| Ok(()),
        || Ok(()),
    )
}

#[cfg(test)]
pub(crate) fn verify_active_workspace_cancellation_for_test(
    active: &ActiveWorkspace,
    operation_id: &str,
) -> AppResult<VerificationReport> {
    let cancel_requested = AtomicBool::new(false);
    let mut checkpoints = 0_usize;
    verify_active_workspace_core(
        active,
        operation_id,
        VerificationLevel::Full,
        &cancel_requested,
        |_, _| {
            checkpoints = checkpoints.saturating_add(1);
            if checkpoints >= 3 {
                cancel_requested.store(true, Ordering::SeqCst);
                return Err(import_cancelled_error(
                    "Synthetic workspace verification cancellation.",
                ));
            }
            Ok(())
        },
        |_, _| Ok(()),
        || Ok(()),
    )
}

#[cfg(test)]
pub(crate) fn verify_active_workspace_cancel_policy_for_test(
    active: &ActiveWorkspace,
    operation_id: &str,
    honor_cancellation: bool,
) -> AppResult<VerificationReport> {
    let requested = AtomicBool::new(true);
    let never_cancel = AtomicBool::new(false);
    let effective = if honor_cancellation {
        &requested
    } else {
        &never_cancel
    };
    if effective.load(Ordering::SeqCst) {
        return Err(import_cancelled_error(
            "Synthetic post-commit verification cancellation.",
        ));
    }
    verify_active_workspace_core(
        active,
        operation_id,
        VerificationLevel::Quick,
        effective,
        |_, _| {
            if effective.load(Ordering::SeqCst) {
                return Err(import_cancelled_error(
                    "Synthetic post-commit verification cancellation.",
                ));
            }
            Ok(())
        },
        |_, _| Ok(()),
        || Ok(()),
    )
}

#[cfg(test)]
#[allow(clippy::type_complexity)]
pub(crate) fn database_check_summary_for_test(
    index_path: &Path,
    manifest: &ImportManifest,
) -> AppResult<(u64, u64, u64, u64, u64, u64, String, bool, bool)> {
    let checks = database_checks(index_path, manifest)?;
    Ok((
        checks.canonical_messages,
        checks.attachments,
        checks.folders,
        checks.fts_rows,
        checks.missing_canonical,
        checks.unexpected_canonical,
        checks.conversation_state,
        checks.inconsistent,
        checks.counter_mismatch,
    ))
}

#[cfg(test)]
pub(crate) fn database_check_cancellation_for_test(
    index_path: &Path,
    manifest: &ImportManifest,
) -> AppResult<()> {
    let cancel_requested = AtomicBool::new(false);
    let mut checkpoints = 0_usize;
    database_checks_with_progress(index_path, manifest, &cancel_requested, || {
        checkpoints = checkpoints.saturating_add(1);
        if checkpoints == 2 {
            cancel_requested.store(true, Ordering::SeqCst);
        }
        Ok(())
    })
    .map(|_| ())
}

#[cfg(test)]
pub(crate) fn possible_duplicate_details_for_test(
    workspace_id: &str,
    manifest: &ImportManifest,
) -> (u64, Vec<Vec<String>>) {
    let (count, _, relative_groups) = possible_duplicate_details(workspace_id, manifest);
    (count, relative_groups)
}

#[cfg(test)]
pub(crate) fn source_probe_for_test(
    active: &ActiveWorkspace,
    manifest: &ImportManifest,
) -> SourceIntegrity {
    source_probe(active, manifest)
}

#[cfg(test)]
pub(crate) fn scan_extraction_for_test(
    extract_dir: &Path,
    manifest: &ImportManifest,
    level: VerificationLevel,
) -> AppResult<(HashSet<String>, bool)> {
    scan_extraction(extract_dir, manifest, level)
}

#[cfg(test)]
pub(crate) fn scan_extraction_with_between_passes_for_test<F>(
    extract_dir: &Path,
    manifest: &ImportManifest,
    level: VerificationLevel,
    between_passes: F,
) -> AppResult<(HashSet<String>, bool)>
where
    F: FnOnce(),
{
    let first = scan_extraction_pass(extract_dir, manifest, level)?;
    between_passes();
    let second = scan_extraction_pass(extract_dir, manifest, level)?;
    finish_extraction_scan(first, second)
}
