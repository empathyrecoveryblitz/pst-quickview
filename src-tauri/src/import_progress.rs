use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use std::fmt;

pub const IMPORT_PROGRESS_FORMAT_VERSION: u16 = 1;
pub const MAX_SAFE_JSON_INTEGER: u64 = 9_007_199_254_740_991;
pub const MAX_SERIALIZED_PROGRESS_BYTES: usize = 8 * 1024;
pub const MAX_WARNING_CODES: usize = 16;

pub const ETA_MIN_SAMPLES: usize = 4;
pub const ETA_MIN_PROGRESS_UNITS: u64 = 10;
pub const ETA_MIN_ACTIVE_MS: u64 = 10_000;
pub const ETA_STALL_AFTER_MS: u64 = 30_000;
pub const ETA_MAX_ESTIMATE_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
const ETA_MAX_RECENT_SAMPLES: usize = 16;
const ETA_EWMA_ALPHA: f64 = 0.35;
const ETA_MAX_COEFFICIENT_OF_VARIATION: f64 = 0.35;
const ETA_MIN_UNCERTAINTY: f64 = 0.10;
const ETA_MAX_UNCERTAINTY: f64 = 0.30;
const MIN_UNFINISHED_ETA_MS: u64 = 1_000;

const MAX_OPERATION_ID_BYTES: usize = 128;
const MAX_WORKSPACE_ID_BYTES: usize = 256;
const MAX_SOURCE_BASENAME_BYTES: usize = 1_024;
const MAX_SOURCE_BASENAME_CHARS: usize = 255;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressStage {
    Preflight,
    Extracting,
    Discovering,
    Indexing,
    BuildingConversations,
    Verifying,
    Complete,
}

impl ProgressStage {
    pub const ALL: [Self; 7] = [
        Self::Preflight,
        Self::Extracting,
        Self::Discovering,
        Self::Indexing,
        Self::BuildingConversations,
        Self::Verifying,
        Self::Complete,
    ];

    pub const fn rank(self) -> usize {
        match self {
            Self::Preflight => 0,
            Self::Extracting => 1,
            Self::Discovering => 2,
            Self::Indexing => 3,
            Self::BuildingConversations => 4,
            Self::Verifying => 5,
            Self::Complete => 6,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    NewImport,
    ResumedImport,
    Repair,
    Verification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceStorageMode {
    AppSupport,
    NextToPst,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressUnitKind {
    Files,
    Bytes,
    CandidateRecords,
    CanonicalMessages,
    ConversationRecords,
    VerificationChecks,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessState {
    NotApplicable,
    Starting,
    RunningRecentActivity,
    RunningNoRecentSignal,
    ExitedSuccess,
    ExitedFailure,
    CancellationRequested,
    StalledEvidence,
    UnknownAfterRestart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressStatus {
    Active,
    Complete,
    Cancelled,
    Incomplete,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageCorrection {
    VerificationReclassified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressStatusCode {
    PreflightRunning,
    ReadpstRunning,
    DiscoveringCandidates,
    IndexingCandidates,
    BuildingConversations,
    VerifyingWorkspace,
    ImportComplete,
    CancellationRequested,
    ImportCancelled,
    ImportIncomplete,
    ImportFailed,
    StageCorrectedByVerification,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProgressWarningCode {
    NoRecentActivitySignal,
    SourceUnavailable,
    SourceChanged,
    PossibleDuplicateGroups,
    VerificationIncomplete,
    ProgressSignalUnavailable,
    PriorTimingUnavailable,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProgressCounters {
    pub output_files_observed: Option<u64>,
    pub output_bytes_observed: Option<u64>,
    pub workspace_bytes_written: Option<u64>,
    pub candidates_discovered: Option<u64>,
    pub candidates_processed: Option<u64>,
    pub canonical_messages: Option<u64>,
    pub inserted_messages: Option<u64>,
    pub skipped_existing_messages: Option<u64>,
    pub parse_failures: Option<u64>,
    pub attachments_indexed: Option<u64>,
    pub conversation_records_processed: Option<u64>,
    pub conversation_records_total: Option<u64>,
    pub verification_checks_completed: Option<u64>,
    pub verification_checks_total: Option<u64>,
    pub missing_canonical_records: Option<u64>,
    pub unexpected_canonical_records: Option<u64>,
    pub possible_duplicate_groups: Option<u64>,
}

impl ProgressCounters {
    fn values(&self) -> [(&'static str, Option<u64>); 17] {
        [
            ("outputFilesObserved", self.output_files_observed),
            ("outputBytesObserved", self.output_bytes_observed),
            ("workspaceBytesWritten", self.workspace_bytes_written),
            ("candidatesDiscovered", self.candidates_discovered),
            ("candidatesProcessed", self.candidates_processed),
            ("canonicalMessages", self.canonical_messages),
            ("insertedMessages", self.inserted_messages),
            ("skippedExistingMessages", self.skipped_existing_messages),
            ("parseFailures", self.parse_failures),
            ("attachmentsIndexed", self.attachments_indexed),
            (
                "conversationRecordsProcessed",
                self.conversation_records_processed,
            ),
            ("conversationRecordsTotal", self.conversation_records_total),
            (
                "verificationChecksCompleted",
                self.verification_checks_completed,
            ),
            ("verificationChecksTotal", self.verification_checks_total),
            ("missingCanonicalRecords", self.missing_canonical_records),
            (
                "unexpectedCanonicalRecords",
                self.unexpected_canonical_records,
            ),
            ("possibleDuplicateGroups", self.possible_duplicate_groups),
        ]
    }

    fn validate(&self) -> Result<(), ContractError> {
        for (field, value) in self.values() {
            validate_optional_safe_integer(field, value)?;
        }

        validate_optional_pair(
            "candidatesProcessed",
            self.candidates_processed,
            "candidatesDiscovered",
            self.candidates_discovered,
        )?;
        validate_optional_pair(
            "conversationRecordsProcessed",
            self.conversation_records_processed,
            "conversationRecordsTotal",
            self.conversation_records_total,
        )?;
        validate_optional_pair(
            "verificationChecksCompleted",
            self.verification_checks_completed,
            "verificationChecksTotal",
            self.verification_checks_total,
        )?;

        if let (Some(inserted), Some(skipped), Some(processed)) = (
            self.inserted_messages,
            self.skipped_existing_messages,
            self.candidates_processed,
        ) {
            if inserted.saturating_add(skipped) > processed {
                return Err(ContractError::InconsistentCounters);
            }
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EtaRange {
    pub lower_bound_ms: u64,
    pub upper_bound_ms: u64,
}

impl EtaRange {
    pub fn new(lower_bound_ms: u64, upper_bound_ms: u64) -> Option<Self> {
        let range = Self {
            lower_bound_ms,
            upper_bound_ms,
        };
        range.is_valid().then_some(range)
    }

    pub fn is_valid(self) -> bool {
        self.lower_bound_ms > 0
            && self.lower_bound_ms <= self.upper_bound_ms
            && self.upper_bound_ms <= ETA_MAX_ESTIMATE_MS
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ImportProgressSnapshot {
    pub format_version: u16,
    pub operation_id: String,
    pub generation: u64,
    pub state_revision: u64,
    pub event_sequence: u64,
    pub workspace_id: String,
    pub source_basename: String,
    pub source_size_bytes: u64,
    pub workspace_storage_mode: WorkspaceStorageMode,
    pub operation_kind: OperationKind,
    pub stage: ProgressStage,
    pub completed_stages: Vec<ProgressStage>,
    pub status: ProgressStatus,
    pub determinate: bool,
    pub processed_units: Option<u64>,
    pub total_units: Option<u64>,
    pub unit_kind: Option<ProgressUnitKind>,
    pub counters: ProgressCounters,
    pub active_elapsed_ms: u64,
    pub operation_elapsed_ms: u64,
    pub stage_elapsed_ms: u64,
    pub last_activity_utc: Option<String>,
    pub last_activity_age_ms: Option<u64>,
    pub process_state: ProcessState,
    pub eta: Option<EtaRange>,
    pub status_code: ProgressStatusCode,
    pub warning_codes: Vec<ProgressWarningCode>,
    pub stage_correction: Option<StageCorrection>,
}

impl ImportProgressSnapshot {
    pub fn validate(&self) -> Result<(), ContractError> {
        if self.format_version != IMPORT_PROGRESS_FORMAT_VERSION {
            return Err(ContractError::UnsupportedFormatVersion);
        }
        validate_opaque_id("operationId", &self.operation_id, MAX_OPERATION_ID_BYTES)?;
        validate_opaque_id("workspaceId", &self.workspace_id, MAX_WORKSPACE_ID_BYTES)?;
        validate_source_basename(&self.source_basename)?;
        validate_safe_integer("generation", self.generation)?;
        validate_safe_integer("stateRevision", self.state_revision)?;
        validate_safe_integer("eventSequence", self.event_sequence)?;
        if self.generation == 0 || self.state_revision == 0 || self.event_sequence == 0 {
            return Err(ContractError::ZeroRevision);
        }
        validate_safe_integer("sourceSizeBytes", self.source_size_bytes)?;
        validate_safe_integer("activeElapsedMs", self.active_elapsed_ms)?;
        validate_safe_integer("operationElapsedMs", self.operation_elapsed_ms)?;
        validate_safe_integer("stageElapsedMs", self.stage_elapsed_ms)?;
        validate_optional_safe_integer("processedUnits", self.processed_units)?;
        validate_optional_safe_integer("totalUnits", self.total_units)?;
        validate_optional_safe_integer("lastActivityAgeMs", self.last_activity_age_ms)?;
        self.counters.validate()?;

        if let Some(timestamp) = &self.last_activity_utc {
            if !is_canonical_utc_millis(timestamp) {
                return Err(ContractError::InvalidUtcTimestamp);
            }
        }
        if self.last_activity_age_ms.is_some() && self.last_activity_utc.is_none() {
            return Err(ContractError::ActivityAgeWithoutTimestamp);
        }

        self.validate_stage_history()?;
        self.validate_progress_measurement()?;
        self.validate_status()?;
        self.validate_codes()?;

        if self.warning_codes.len() > MAX_WARNING_CODES {
            return Err(ContractError::TooManyWarnings);
        }
        for (index, warning) in self.warning_codes.iter().enumerate() {
            if self.warning_codes[..index].contains(warning) {
                return Err(ContractError::DuplicateWarning);
            }
        }

        Ok(())
    }

    fn validate_stage_history(&self) -> Result<(), ContractError> {
        if self.completed_stages.len() > ProgressStage::ALL.len() {
            return Err(ContractError::InvalidCompletedStages);
        }
        let mut previous_rank = None;
        for stage in &self.completed_stages {
            let rank = stage.rank();
            if previous_rank.is_some_and(|previous| rank <= previous) {
                return Err(ContractError::InvalidCompletedStages);
            }
            previous_rank = Some(rank);
        }
        if self.status == ProgressStatus::Complete {
            if self.completed_stages.as_slice() != ProgressStage::ALL {
                return Err(ContractError::InvalidCompletedStages);
            }
        } else if self.completed_stages.contains(&ProgressStage::Complete)
            || self
                .completed_stages
                .iter()
                .any(|stage| stage.rank() >= self.stage.rank())
        {
            return Err(ContractError::InvalidCompletedStages);
        }
        Ok(())
    }

    fn validate_progress_measurement(&self) -> Result<(), ContractError> {
        if self.determinate {
            if self.status != ProgressStatus::Active || self.stage == ProgressStage::Complete {
                return Err(ContractError::DeterminateTerminalState);
            }
            let (processed, total, _) =
                match (self.processed_units, self.total_units, self.unit_kind) {
                    (Some(processed), Some(total), Some(unit)) => (processed, total, unit),
                    _ => return Err(ContractError::IncompleteDeterminateMeasurement),
                };
            if total == 0 || processed > total {
                return Err(ContractError::InvalidDeterminateMeasurement);
            }
        } else {
            if self.total_units.is_some() || self.eta.is_some() {
                return Err(ContractError::FakeIndeterminateMeasurement);
            }
            if self.processed_units.is_some() != self.unit_kind.is_some() {
                return Err(ContractError::IncompleteObservedMeasurement);
            }
        }

        if let Some(eta) = self.eta {
            if !eta.is_valid() {
                return Err(ContractError::InvalidEta);
            }
            if !matches!(
                self.stage,
                ProgressStage::Indexing | ProgressStage::BuildingConversations
            ) {
                return Err(ContractError::EtaForUnsuitableStage);
            }
            if self.processed_units == self.total_units {
                return Err(ContractError::EtaForFinishedWork);
            }
        }
        Ok(())
    }

    fn validate_status(&self) -> Result<(), ContractError> {
        match self.status {
            ProgressStatus::Active => {
                if self.stage == ProgressStage::Complete {
                    return Err(ContractError::InvalidTerminalStage);
                }
            }
            ProgressStatus::Complete => {
                if self.stage != ProgressStage::Complete
                    || self.determinate
                    || self.processed_units.is_some()
                    || self.total_units.is_some()
                    || self.unit_kind.is_some()
                    || self.eta.is_some()
                {
                    return Err(ContractError::InvalidTerminalStage);
                }
            }
            ProgressStatus::Cancelled | ProgressStatus::Incomplete | ProgressStatus::Failed => {
                if self.stage == ProgressStage::Complete || self.determinate || self.eta.is_some() {
                    return Err(ContractError::InvalidTerminalStage);
                }
            }
        }
        Ok(())
    }

    fn validate_codes(&self) -> Result<(), ContractError> {
        let status_matches = match self.status {
            ProgressStatus::Active => matches!(
                self.status_code,
                ProgressStatusCode::PreflightRunning
                    | ProgressStatusCode::ReadpstRunning
                    | ProgressStatusCode::DiscoveringCandidates
                    | ProgressStatusCode::IndexingCandidates
                    | ProgressStatusCode::BuildingConversations
                    | ProgressStatusCode::VerifyingWorkspace
                    | ProgressStatusCode::CancellationRequested
                    | ProgressStatusCode::StageCorrectedByVerification
            ),
            ProgressStatus::Complete => self.status_code == ProgressStatusCode::ImportComplete,
            ProgressStatus::Cancelled => self.status_code == ProgressStatusCode::ImportCancelled,
            ProgressStatus::Incomplete => self.status_code == ProgressStatusCode::ImportIncomplete,
            ProgressStatus::Failed => self.status_code == ProgressStatusCode::ImportFailed,
        };
        if !status_matches {
            return Err(ContractError::StatusCodeMismatch);
        }

        let expected_stage_code = match self.stage {
            ProgressStage::Preflight => ProgressStatusCode::PreflightRunning,
            ProgressStage::Extracting => ProgressStatusCode::ReadpstRunning,
            ProgressStage::Discovering => ProgressStatusCode::DiscoveringCandidates,
            ProgressStage::Indexing => ProgressStatusCode::IndexingCandidates,
            ProgressStage::BuildingConversations => ProgressStatusCode::BuildingConversations,
            ProgressStage::Verifying => ProgressStatusCode::VerifyingWorkspace,
            ProgressStage::Complete => ProgressStatusCode::ImportComplete,
        };
        if self.status == ProgressStatus::Active
            && self.stage_correction.is_none()
            && self.status_code != ProgressStatusCode::CancellationRequested
            && self.status_code != expected_stage_code
        {
            return Err(ContractError::StatusCodeMismatch);
        }

        let correction_matches = matches!(
            (self.stage_correction, self.status_code),
            (
                Some(StageCorrection::VerificationReclassified),
                ProgressStatusCode::StageCorrectedByVerification
            ) | (None, ProgressStatusCode::PreflightRunning)
                | (None, ProgressStatusCode::ReadpstRunning)
                | (None, ProgressStatusCode::DiscoveringCandidates)
                | (None, ProgressStatusCode::IndexingCandidates)
                | (None, ProgressStatusCode::BuildingConversations)
                | (None, ProgressStatusCode::VerifyingWorkspace)
                | (None, ProgressStatusCode::ImportComplete)
                | (None, ProgressStatusCode::CancellationRequested)
                | (None, ProgressStatusCode::ImportCancelled)
                | (None, ProgressStatusCode::ImportIncomplete)
                | (None, ProgressStatusCode::ImportFailed)
        );
        if !correction_matches {
            return Err(ContractError::CorrectionCodeMismatch);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct ImportProgressEvent(ImportProgressSnapshot);

impl ImportProgressEvent {
    pub fn new(snapshot: ImportProgressSnapshot) -> Result<Self, ContractError> {
        snapshot.validate()?;
        Ok(Self(snapshot))
    }

    pub fn snapshot(&self) -> &ImportProgressSnapshot {
        &self.0
    }

    pub fn into_snapshot(self) -> ImportProgressSnapshot {
        self.0
    }
}

impl<'de> Deserialize<'de> for ImportProgressEvent {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let snapshot = ImportProgressSnapshot::deserialize(deserializer)?;
        Self::new(snapshot).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateDecision {
    AcceptInitial,
    AcceptDuplicate,
    Accept,
    AcceptNewGeneration,
    AcceptExplainedCorrection,
    RejectInvalid,
    RejectDifferentWorkspace,
    RejectDifferentOperation,
    RejectStaleGeneration,
    RejectStaleRevision,
    RejectBackwardStage,
}

pub fn classify_update(
    current: Option<&ImportProgressSnapshot>,
    incoming: &ImportProgressSnapshot,
) -> UpdateDecision {
    if incoming.validate().is_err() {
        return UpdateDecision::RejectInvalid;
    }
    let Some(current) = current else {
        return UpdateDecision::AcceptInitial;
    };
    if current.validate().is_err() {
        return UpdateDecision::RejectInvalid;
    }
    if incoming.workspace_id != current.workspace_id {
        return UpdateDecision::RejectDifferentWorkspace;
    }
    if incoming.generation < current.generation {
        return UpdateDecision::RejectStaleGeneration;
    }
    if incoming.generation > current.generation {
        return if incoming.stage.rank() < current.stage.rank()
            && incoming.stage_correction == Some(StageCorrection::VerificationReclassified)
        {
            UpdateDecision::AcceptExplainedCorrection
        } else {
            UpdateDecision::AcceptNewGeneration
        };
    }
    if incoming.operation_id != current.operation_id {
        return UpdateDecision::RejectDifferentOperation;
    }
    if incoming.state_revision < current.state_revision {
        return UpdateDecision::RejectStaleRevision;
    }
    if incoming.state_revision == current.state_revision {
        if incoming.event_sequence < current.event_sequence {
            return UpdateDecision::RejectStaleRevision;
        }
        if incoming.event_sequence == current.event_sequence {
            return if incoming == current {
                UpdateDecision::AcceptDuplicate
            } else {
                UpdateDecision::RejectStaleRevision
            };
        }
        if incoming.stage != current.stage {
            return UpdateDecision::RejectBackwardStage;
        }
    }
    if incoming.stage.rank() < current.stage.rank() {
        return UpdateDecision::RejectBackwardStage;
    }
    UpdateDecision::Accept
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TimingCheckpoint {
    pub active_elapsed_ms: u64,
    pub stage_elapsed_ms: u64,
    pub completion_duration_ms: Option<u64>,
}

impl TimingCheckpoint {
    pub const fn empty() -> Self {
        Self {
            active_elapsed_ms: 0,
            stage_elapsed_ms: 0,
            completion_duration_ms: None,
        }
    }

    pub fn validate(self) -> Result<(), TimingError> {
        if self.active_elapsed_ms > MAX_SAFE_JSON_INTEGER
            || self.stage_elapsed_ms > MAX_SAFE_JSON_INTEGER
            || self
                .completion_duration_ms
                .is_some_and(|duration| duration > MAX_SAFE_JSON_INTEGER)
        {
            return Err(TimingError::OutOfRange);
        }
        if self.stage_elapsed_ms > self.active_elapsed_ms {
            return Err(TimingError::StageExceedsActive);
        }
        if self
            .completion_duration_ms
            .is_some_and(|duration| duration != self.active_elapsed_ms)
        {
            return Err(TimingError::CompletionMismatch);
        }
        Ok(())
    }
}

impl Default for TimingCheckpoint {
    fn default() -> Self {
        Self::empty()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimingSnapshot {
    pub active_elapsed_ms: u64,
    pub stage_elapsed_ms: u64,
    pub completion_duration_ms: Option<u64>,
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTiming {
    active_elapsed_ms: u64,
    stage_elapsed_ms: u64,
    last_tick_ms: u64,
    active_session_start_tick_ms: Option<u64>,
    stage_session_start_tick_ms: Option<u64>,
    completion_duration_ms: Option<u64>,
    running: bool,
}

impl ActiveTiming {
    pub fn start(now_tick_ms: u64) -> Self {
        Self {
            active_elapsed_ms: 0,
            stage_elapsed_ms: 0,
            last_tick_ms: now_tick_ms,
            active_session_start_tick_ms: Some(now_tick_ms),
            stage_session_start_tick_ms: Some(now_tick_ms),
            completion_duration_ms: None,
            running: true,
        }
    }

    pub fn resume(checkpoint: TimingCheckpoint, now_tick_ms: u64) -> Result<Self, TimingError> {
        checkpoint.validate()?;
        let completed = checkpoint.completion_duration_ms.is_some();
        Ok(Self {
            active_elapsed_ms: checkpoint.active_elapsed_ms,
            stage_elapsed_ms: checkpoint.stage_elapsed_ms,
            last_tick_ms: now_tick_ms,
            active_session_start_tick_ms: (!completed).then_some(now_tick_ms),
            stage_session_start_tick_ms: (!completed).then_some(now_tick_ms),
            completion_duration_ms: checkpoint.completion_duration_ms,
            running: !completed,
        })
    }

    pub fn snapshot(&mut self, now_tick_ms: u64) -> TimingSnapshot {
        self.advance(now_tick_ms);
        self.current_snapshot()
    }

    pub fn transition_stage(&mut self, now_tick_ms: u64) -> TimingSnapshot {
        self.advance(now_tick_ms);
        if self.running {
            self.stage_elapsed_ms = 0;
            self.stage_session_start_tick_ms = Some(self.last_tick_ms);
        }
        self.current_snapshot()
    }

    pub fn pause(&mut self, now_tick_ms: u64) -> TimingCheckpoint {
        self.advance(now_tick_ms);
        self.running = false;
        self.active_session_start_tick_ms = None;
        self.stage_session_start_tick_ms = None;
        self.checkpoint()
    }

    pub fn complete(&mut self, now_tick_ms: u64) -> TimingCheckpoint {
        if self.completion_duration_ms.is_none() {
            self.advance(now_tick_ms);
            self.completion_duration_ms = Some(self.active_elapsed_ms);
            self.running = false;
            self.active_session_start_tick_ms = None;
            self.stage_session_start_tick_ms = None;
        }
        self.checkpoint()
    }

    pub fn checkpoint(&self) -> TimingCheckpoint {
        TimingCheckpoint {
            active_elapsed_ms: self.active_elapsed_ms,
            stage_elapsed_ms: self.stage_elapsed_ms,
            completion_duration_ms: self.completion_duration_ms,
        }
    }

    pub fn active_session_start_tick_ms(&self) -> Option<u64> {
        self.active_session_start_tick_ms
    }

    pub fn stage_session_start_tick_ms(&self) -> Option<u64> {
        self.stage_session_start_tick_ms
    }

    fn advance(&mut self, now_tick_ms: u64) {
        let effective_tick = self.last_tick_ms.max(now_tick_ms);
        if self.running {
            let delta = effective_tick.saturating_sub(self.last_tick_ms);
            self.active_elapsed_ms = bounded_elapsed_add(self.active_elapsed_ms, delta);
            self.stage_elapsed_ms = bounded_elapsed_add(self.stage_elapsed_ms, delta);
        }
        self.last_tick_ms = effective_tick;
    }

    fn current_snapshot(&self) -> TimingSnapshot {
        TimingSnapshot {
            active_elapsed_ms: self.active_elapsed_ms,
            stage_elapsed_ms: self.stage_elapsed_ms,
            completion_duration_ms: self.completion_duration_ms,
            running: self.running,
        }
    }
}

pub const fn monotonic_age_ms(now_tick_ms: u64, activity_tick_ms: u64) -> u64 {
    now_tick_ms.saturating_sub(activity_tick_ms)
}

fn bounded_elapsed_add(current: u64, delta: u64) -> u64 {
    current.saturating_add(delta).min(MAX_SAFE_JSON_INTEGER)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EtaSample {
    pub active_elapsed_ms: u64,
    pub completed_units: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct EtaInput<'a> {
    pub determinate: bool,
    pub processed_units: u64,
    pub total_units: u64,
    pub active_elapsed_ms: u64,
    pub last_progress_age_ms: Option<u64>,
    pub samples: &'a [EtaSample],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EtaStatus {
    Hidden,
    Estimating,
    Estimate(EtaRange),
}

pub fn estimate_eta(input: EtaInput<'_>) -> EtaStatus {
    if !input.determinate
        || input.total_units == 0
        || input.processed_units >= input.total_units
        || input.processed_units > MAX_SAFE_JSON_INTEGER
        || input.total_units > MAX_SAFE_JSON_INTEGER
        || input.active_elapsed_ms > MAX_SAFE_JSON_INTEGER
    {
        return EtaStatus::Hidden;
    }

    let explicit_stall = input
        .last_progress_age_ms
        .is_some_and(|age| age > ETA_STALL_AFTER_MS);
    if explicit_stall {
        return EtaStatus::Hidden;
    }
    if input.samples.len() < ETA_MIN_SAMPLES
        || input.processed_units < ETA_MIN_PROGRESS_UNITS
        || input.active_elapsed_ms < ETA_MIN_ACTIVE_MS
    {
        return EtaStatus::Estimating;
    }

    let sample_start = input.samples.len().saturating_sub(ETA_MAX_RECENT_SAMPLES);
    let samples = &input.samples[sample_start..];
    if match samples.last() {
        Some(sample) => {
            sample.completed_units != input.processed_units
                || sample.active_elapsed_ms > input.active_elapsed_ms
        }
        None => true,
    } {
        return EtaStatus::Hidden;
    }

    for pair in samples.windows(2) {
        if pair[1].active_elapsed_ms <= pair[0].active_elapsed_ms
            || pair[1].completed_units < pair[0].completed_units
            || pair[1].active_elapsed_ms > MAX_SAFE_JSON_INTEGER
            || pair[1].completed_units > MAX_SAFE_JSON_INTEGER
        {
            return EtaStatus::Hidden;
        }
    }

    let first = samples[0];
    let last = samples[samples.len() - 1];
    if last.completed_units.saturating_sub(first.completed_units) < ETA_MIN_PROGRESS_UNITS
        || last
            .active_elapsed_ms
            .saturating_sub(first.active_elapsed_ms)
            < ETA_MIN_ACTIVE_MS
    {
        return EtaStatus::Estimating;
    }

    let mut last_progress_tick = first.active_elapsed_ms;
    let mut rates = Vec::with_capacity(samples.len().saturating_sub(1));
    for pair in samples.windows(2) {
        let elapsed = pair[1].active_elapsed_ms - pair[0].active_elapsed_ms;
        let completed = pair[1].completed_units - pair[0].completed_units;
        if completed > 0 {
            last_progress_tick = pair[1].active_elapsed_ms;
        }
        rates.push(completed as f64 / elapsed as f64);
    }
    if input.active_elapsed_ms.saturating_sub(last_progress_tick) > ETA_STALL_AFTER_MS {
        return EtaStatus::Hidden;
    }

    let mean = rates.iter().sum::<f64>() / rates.len() as f64;
    if !mean.is_finite() || mean <= 0.0 {
        return EtaStatus::Hidden;
    }
    let variance = rates
        .iter()
        .map(|rate| {
            let difference = rate - mean;
            difference * difference
        })
        .sum::<f64>()
        / rates.len() as f64;
    let coefficient_of_variation = variance.sqrt() / mean;
    if !coefficient_of_variation.is_finite()
        || coefficient_of_variation > ETA_MAX_COEFFICIENT_OF_VARIATION
    {
        return EtaStatus::Hidden;
    }

    let mut smoothed_rate = rates[0];
    for rate in &rates[1..] {
        smoothed_rate = ETA_EWMA_ALPHA * rate + (1.0 - ETA_EWMA_ALPHA) * smoothed_rate;
    }
    if !smoothed_rate.is_finite() || smoothed_rate <= 0.0 {
        return EtaStatus::Hidden;
    }

    let remaining_units = input.total_units - input.processed_units;
    let central_ms = remaining_units as f64 / smoothed_rate;
    if !central_ms.is_finite() || central_ms <= 0.0 || central_ms > ETA_MAX_ESTIMATE_MS as f64 {
        return EtaStatus::Hidden;
    }
    let central_ms = central_ms.round().max(MIN_UNFINISHED_ETA_MS as f64) as u64;
    let uncertainty = coefficient_of_variation.clamp(ETA_MIN_UNCERTAINTY, ETA_MAX_UNCERTAINTY);
    let lower = ((central_ms as f64) * (1.0 - uncertainty))
        .round()
        .max(MIN_UNFINISHED_ETA_MS as f64) as u64;
    let upper = ((central_ms as f64) * (1.0 + uncertainty))
        .round()
        .max(lower as f64)
        .min(ETA_MAX_ESTIMATE_MS as f64) as u64;

    EtaRange::new(lower, upper).map_or(EtaStatus::Hidden, EtaStatus::Estimate)
}

pub fn format_compact_duration(milliseconds: u64) -> String {
    let seconds = milliseconds / 1_000;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m {:02}s", seconds % 60);
    }
    let hours = minutes / 60;
    format!("{hours}h {:02}m", minutes % 60)
}

pub fn format_eta_range(range: EtaRange) -> Option<String> {
    if !range.is_valid() {
        return None;
    }
    if range.upper_bound_ms <= 60_000 {
        return Some("Less than a minute remaining".to_string());
    }

    let lower_minutes = div_ceil(range.lower_bound_ms, 60_000).max(1);
    let upper_minutes = div_ceil(range.upper_bound_ms, 60_000).max(lower_minutes);
    if upper_minutes < 60 {
        if upper_minutes.saturating_sub(lower_minutes) <= 1 {
            let midpoint_ms = range.lower_bound_ms
                + range.upper_bound_ms.saturating_sub(range.lower_bound_ms) / 2;
            let about = midpoint_ms.saturating_add(30_000) / 60_000;
            return Some(format!("About {about} minutes remaining"));
        }
        return Some(format!(
            "Estimated {lower_minutes}\u{2013}{upper_minutes} minutes remaining"
        ));
    }

    let lower_hours = (range.lower_bound_ms / 3_600_000).max(1);
    let upper_hours = div_ceil(range.upper_bound_ms, 3_600_000).max(lower_hours);
    if lower_hours == upper_hours {
        Some(format!("About {lower_hours} hours remaining"))
    } else {
        Some(format!(
            "Estimated {lower_hours}\u{2013}{upper_hours} hours remaining"
        ))
    }
}

const fn div_ceil(value: u64, divisor: u64) -> u64 {
    value / divisor + (value % divisor != 0) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractError {
    UnsupportedFormatVersion,
    InvalidOpaqueId(&'static str),
    InvalidSourceBasename,
    ZeroRevision,
    IntegerOutOfRange(&'static str),
    InvalidUtcTimestamp,
    ActivityAgeWithoutTimestamp,
    InvalidCompletedStages,
    DeterminateTerminalState,
    IncompleteDeterminateMeasurement,
    InvalidDeterminateMeasurement,
    FakeIndeterminateMeasurement,
    IncompleteObservedMeasurement,
    InvalidEta,
    EtaForUnsuitableStage,
    EtaForFinishedWork,
    InvalidTerminalStage,
    StatusCodeMismatch,
    CorrectionCodeMismatch,
    TooManyWarnings,
    DuplicateWarning,
    InconsistentCounters,
}

impl fmt::Display for ContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormatVersion => formatter.write_str("unsupported progress format"),
            Self::InvalidOpaqueId(field) => write!(formatter, "invalid opaque {field}"),
            Self::InvalidSourceBasename => formatter.write_str("invalid source basename"),
            Self::ZeroRevision => {
                formatter.write_str("generation, revision, and event sequence must be positive")
            }
            Self::IntegerOutOfRange(field) => write!(formatter, "{field} is out of range"),
            Self::InvalidUtcTimestamp => formatter.write_str("invalid canonical UTC timestamp"),
            Self::ActivityAgeWithoutTimestamp => {
                formatter.write_str("activity age requires a timestamp")
            }
            Self::InvalidCompletedStages => formatter.write_str("invalid completed stage sequence"),
            Self::DeterminateTerminalState => {
                formatter.write_str("terminal progress cannot be determinate")
            }
            Self::IncompleteDeterminateMeasurement => {
                formatter.write_str("determinate progress requires processed, total, and unit")
            }
            Self::InvalidDeterminateMeasurement => {
                formatter.write_str("invalid determinate progress measurement")
            }
            Self::FakeIndeterminateMeasurement => {
                formatter.write_str("indeterminate progress cannot include total or ETA")
            }
            Self::IncompleteObservedMeasurement => {
                formatter.write_str("observed progress requires both count and unit")
            }
            Self::InvalidEta => formatter.write_str("invalid ETA range"),
            Self::EtaForUnsuitableStage => formatter.write_str("ETA is not allowed for this stage"),
            Self::EtaForFinishedWork => formatter.write_str("finished work cannot include an ETA"),
            Self::InvalidTerminalStage => formatter.write_str("invalid terminal progress state"),
            Self::StatusCodeMismatch => formatter.write_str("status code does not match progress"),
            Self::CorrectionCodeMismatch => {
                formatter.write_str("stage correction requires its explanation code")
            }
            Self::TooManyWarnings => formatter.write_str("too many progress warnings"),
            Self::DuplicateWarning => formatter.write_str("duplicate progress warning"),
            Self::InconsistentCounters => formatter.write_str("inconsistent progress counters"),
        }
    }
}

impl std::error::Error for ContractError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimingError {
    OutOfRange,
    StageExceedsActive,
    CompletionMismatch,
}

impl fmt::Display for TimingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutOfRange => formatter.write_str("timing checkpoint is out of range"),
            Self::StageExceedsActive => formatter.write_str("stage time exceeds active time"),
            Self::CompletionMismatch => {
                formatter.write_str("completion duration must equal active time")
            }
        }
    }
}

impl std::error::Error for TimingError {}

fn validate_safe_integer(field: &'static str, value: u64) -> Result<(), ContractError> {
    if value > MAX_SAFE_JSON_INTEGER {
        Err(ContractError::IntegerOutOfRange(field))
    } else {
        Ok(())
    }
}

fn validate_optional_safe_integer(
    field: &'static str,
    value: Option<u64>,
) -> Result<(), ContractError> {
    value.map_or(Ok(()), |value| validate_safe_integer(field, value))
}

fn validate_optional_pair(
    _processed_field: &'static str,
    processed: Option<u64>,
    _total_field: &'static str,
    total: Option<u64>,
) -> Result<(), ContractError> {
    if let (Some(processed), Some(total)) = (processed, total) {
        if processed > total {
            return Err(ContractError::InconsistentCounters);
        }
    }
    Ok(())
}

fn validate_opaque_id(
    field: &'static str,
    value: &str,
    max_bytes: usize,
) -> Result<(), ContractError> {
    if value.is_empty()
        || value.len() > max_bytes
        || matches!(value, "." | "..")
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        return Err(ContractError::InvalidOpaqueId(field));
    }
    Ok(())
}

fn validate_source_basename(value: &str) -> Result<(), ContractError> {
    if value.is_empty()
        || value.len() > MAX_SOURCE_BASENAME_BYTES
        || value.chars().count() > MAX_SOURCE_BASENAME_CHARS
        || matches!(value, "." | "..")
        || value
            .chars()
            .any(|character| character == '/' || character == '\\' || character.is_control())
    {
        return Err(ContractError::InvalidSourceBasename);
    }
    Ok(())
}

fn is_canonical_utc_millis(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 24
        || bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || bytes[19] != b'.'
        || bytes[23] != b'Z'
    {
        return false;
    }
    for index in [
        0_usize, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18, 20, 21, 22,
    ] {
        if !bytes[index].is_ascii_digit() {
            return false;
        }
    }

    let year = decimal(bytes, 0, 4);
    let month = decimal(bytes, 5, 7);
    let day = decimal(bytes, 8, 10);
    let hour = decimal(bytes, 11, 13);
    let minute = decimal(bytes, 14, 16);
    let second = decimal(bytes, 17, 19);
    if year == 0 || !(1..=12).contains(&month) || hour > 23 || minute > 59 || second > 59 {
        return false;
    }
    let max_day = match month {
        2 if is_leap_year(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=max_day).contains(&day)
}

fn decimal(bytes: &[u8], start: usize, end: usize) -> u32 {
    bytes[start..end]
        .iter()
        .fold(0, |value, digit| value * 10 + u32::from(digit - b'0'))
}

const fn is_leap_year(year: u32) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}
