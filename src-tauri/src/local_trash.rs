use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock, Weak};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{
    params, Connection, DatabaseName, Error as SqliteError, ErrorCode, OpenFlags,
    OptionalExtension, TransactionBehavior,
};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::search_cancellation::{is_sqlite_interrupt, SearchOperationGuard};
use crate::sqlite_security;
use crate::{AppError, AppResult, CANONICAL_IMPORT_IDENTITY_VERSION};

pub(crate) const LOCAL_TRASH_FILE_NAME: &str = "local-state.sqlite";
const LOCAL_TRASH_ALT_FILE_NAME: &str = "local-state-alt.sqlite";
pub(crate) const LOCAL_TRASH_TEMP_TABLE: &str = "local_trash_identities";
pub(crate) const LOCAL_TRASH_IDENTITY_VERSION: &str = CANONICAL_IMPORT_IDENTITY_VERSION;
pub(crate) const MAX_LOCAL_TRASH_BATCH: usize = 256;

const LOCAL_STATE_SCHEMA_VERSION: i64 = 1;
const LOCAL_STATE_APPLICATION_ID: i64 = 0x5051_5452; // "PQTR"
const PRIVATE_FILE_MODE: u32 = 0o600;
const MAX_LOCAL_STATE_DATABASE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_LOCAL_TRASH_ROWS: usize = 250_000;
const MAX_TOTAL_IDENTITY_BYTES: usize = 64 * 1024 * 1024;
const MAX_IDENTITY_BYTES: usize = 32 * 1024;
const MAX_MUTATION_IDENTITY_BYTES: usize = 1024 * 1024;
const MAX_SAFE_JSON_INTEGER: u64 = 9_007_199_254_740_991;
const MAX_SAFE_JSON_INTEGER_I64: i64 = 9_007_199_254_740_991;
const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_millis(250);
const MIN_TIMESTAMP_MS: i64 = 946_684_800_000; // 2000-01-01T00:00:00Z
const MAX_TIMESTAMP_MS: i64 = 7_258_118_400_000; // 2200-01-01T00:00:00Z
const OPAQUE_ID_MIN_BYTES: usize = 24;
const OPAQUE_ID_MAX_BYTES: usize = 96;
const CONTROL_FORMAT_VERSION: u32 = 1;
const CONTROL_RECORD_BYTES: usize = 512;
const CONTROL_FILE_BYTES: u64 = (CONTROL_RECORD_BYTES * 2) as u64;
const CONTROL_STATUS_EMPTY: u64 = 0;
const CONTROL_STATUS_PREPARING: u64 = 0x5053_5451_5650_5245; // "PSTQVPre"
const CONTROL_STATUS_COMMITTED: u64 = 0x5053_5451_5643_4f4d; // "PSTQVCOM"
const CONTROL_MAGIC: &[u8; 16] = b"PSTQV-LTRASH-01\0";

static LOCAL_TRASH_OPERATION_GATES: OnceLock<Mutex<HashMap<FileNode, Weak<WorkspaceGate>>>> =
    OnceLock::new();

pub(crate) const ERROR_UNAVAILABLE: &str = "LOCAL_TRASH_UNAVAILABLE";
pub(crate) const ERROR_INVALID: &str = "LOCAL_TRASH_INVALID";
pub(crate) const ERROR_FUTURE_SCHEMA: &str = "LOCAL_TRASH_FUTURE_SCHEMA";
pub(crate) const ERROR_FINGERPRINT_MISMATCH: &str = "LOCAL_TRASH_FINGERPRINT_MISMATCH";
pub(crate) const ERROR_UNSAFE_FILE: &str = "LOCAL_TRASH_UNSAFE_FILE";
pub(crate) const ERROR_OVERSIZED: &str = "LOCAL_TRASH_OVERSIZED";
pub(crate) const ERROR_BUSY: &str = "LOCAL_TRASH_BUSY";
pub(crate) const ERROR_STALE_REVISION: &str = "LOCAL_TRASH_STALE_REVISION";
pub(crate) const ERROR_INVALID_INPUT: &str = "LOCAL_TRASH_INVALID_INPUT";
pub(crate) const ERROR_OPERATION_INDETERMINATE: &str = "LOCAL_TRASH_OPERATION_INDETERMINATE";
pub(crate) const ERROR_CANONICAL_IDENTITY_AMBIGUOUS: &str =
    "LOCAL_TRASH_CANONICAL_IDENTITY_AMBIGUOUS";
pub(crate) const ERROR_RESET_REFUSED: &str = "LOCAL_TRASH_RESET_REFUSED";
pub(crate) const ERROR_RESET_INDETERMINATE: &str = "LOCAL_TRASH_RESET_INDETERMINATE";

const CREATE_SCHEMA_SQL: &str = r#"
CREATE TABLE local_state_metadata (
    singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
    schema_version INTEGER NOT NULL CHECK (schema_version = 1),
    workspace_fingerprint TEXT NOT NULL,
    source_fingerprint TEXT NOT NULL,
    canonical_identity_version TEXT NOT NULL,
    generation TEXT NOT NULL,
    revision INTEGER NOT NULL CHECK (revision BETWEEN 0 AND 9007199254740991),
    last_changed_at_ms INTEGER
) STRICT;

CREATE TABLE trashed_messages (
    workspace_fingerprint TEXT NOT NULL,
    source_fingerprint TEXT NOT NULL,
    canonical_identity_version TEXT NOT NULL,
    canonical_identity TEXT NOT NULL,
    state_revision INTEGER NOT NULL CHECK (state_revision BETWEEN 1 AND 9007199254740991),
    trashed_at_ms INTEGER NOT NULL,
    last_changed_at_ms INTEGER NOT NULL,
    operation_id TEXT NOT NULL,
    PRIMARY KEY (
        workspace_fingerprint,
        source_fingerprint,
        canonical_identity_version,
        canonical_identity
    )
) STRICT, WITHOUT ROWID;
"#;

const CREATE_TEMP_TABLE_SQL: &str = r#"
DROP TABLE IF EXISTS temp.local_trash_identities;
CREATE TEMP TABLE local_trash_identities (
    workspace_fingerprint TEXT NOT NULL,
    source_fingerprint TEXT NOT NULL,
    canonical_identity_version TEXT NOT NULL,
    canonical_identity TEXT NOT NULL,
    PRIMARY KEY (
        workspace_fingerprint,
        source_fingerprint,
        canonical_identity_version,
        canonical_identity
    )
) STRICT, WITHOUT ROWID;
"#;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct LocalTrashScope {
    workspace_fingerprint: String,
    source_fingerprint: String,
}

impl LocalTrashScope {
    pub(crate) fn new(
        workspace_fingerprint: impl Into<String>,
        source_fingerprint: impl Into<String>,
    ) -> AppResult<Self> {
        let value = Self {
            workspace_fingerprint: workspace_fingerprint.into(),
            source_fingerprint: source_fingerprint.into(),
        };
        value.validate()?;
        Ok(value)
    }

    pub(crate) fn workspace_fingerprint(&self) -> &str {
        &self.workspace_fingerprint
    }

    pub(crate) fn source_fingerprint(&self) -> &str {
        &self.source_fingerprint
    }

    fn validate(&self) -> AppResult<()> {
        if !is_lower_hex_sha256(&self.workspace_fingerprint)
            || !is_lower_hex_sha256(&self.source_fingerprint)
        {
            return Err(local_error(
                ERROR_INVALID_INPUT,
                "Local Trash workspace identity is invalid.",
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LocalTrashHealth {
    Absent,
    Healthy,
    Invalid,
    FutureSchema,
    FingerprintMismatch,
    Unavailable,
}

impl LocalTrashHealth {
    fn permits_reset(self) -> bool {
        matches!(
            self,
            Self::Invalid | Self::FutureSchema | Self::FingerprintMismatch
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalTrashInspection {
    pub(crate) health: LocalTrashHealth,
    pub(crate) available: bool,
    pub(crate) schema_version: Option<i64>,
    pub(crate) generation: Option<String>,
    pub(crate) revision: u64,
    pub(crate) stored_count: usize,
    pub(crate) last_changed_at_ms: Option<i64>,
    pub(crate) fingerprint_match: Option<bool>,
    pub(crate) last_error_code: Option<&'static str>,
    #[serde(skip)]
    reset_token: Option<ResetToken>,
}

impl LocalTrashInspection {
    fn absent(canonical_workspace: PathBuf, scope: &LocalTrashScope) -> Self {
        Self {
            health: LocalTrashHealth::Absent,
            available: true,
            schema_version: None,
            generation: None,
            revision: 0,
            stored_count: 0,
            last_changed_at_ms: None,
            fingerprint_match: Some(true),
            last_error_code: None,
            reset_token: Some(ResetToken {
                canonical_workspace,
                scope: scope.clone(),
                storage: StorageSnapshot::absent(),
                selected_record: None,
            }),
        }
    }

    fn healthy(reset_token: ResetToken, details: ValidatedDetails) -> Self {
        Self {
            health: LocalTrashHealth::Healthy,
            available: true,
            schema_version: Some(LOCAL_STATE_SCHEMA_VERSION),
            generation: Some(details.header.generation),
            revision: details.header.revision,
            stored_count: details.stored_count,
            last_changed_at_ms: details.header.last_changed_at_ms,
            fingerprint_match: Some(true),
            last_error_code: None,
            reset_token: Some(reset_token),
        }
    }

    fn failed(failure: ValidationFailure) -> Self {
        Self {
            health: failure.health,
            available: false,
            schema_version: failure.schema_version,
            generation: None,
            revision: 0,
            stored_count: 0,
            last_changed_at_ms: None,
            fingerprint_match: match failure.health {
                LocalTrashHealth::FingerprintMismatch => Some(false),
                LocalTrashHealth::FutureSchema | LocalTrashHealth::Invalid => None,
                _ => None,
            },
            last_error_code: Some(failure.code),
            reset_token: failure.reset_token.map(|token| *token),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalTrashMutationResult {
    pub(crate) operation_id: String,
    pub(crate) requested_count: usize,
    pub(crate) changed_count: usize,
    pub(crate) already_trashed_count: usize,
    pub(crate) already_active_count: usize,
    pub(crate) generation: String,
    pub(crate) revision: u64,
    pub(crate) stored_count: usize,
    pub(crate) last_changed_at_ms: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalTrashQueryState {
    pub(crate) health: LocalTrashHealth,
    pub(crate) available: bool,
    pub(crate) can_reset: bool,
    pub(crate) sidecar_present: bool,
    pub(crate) schema_version: Option<i64>,
    pub(crate) generation: Option<String>,
    pub(crate) revision: u64,
    pub(crate) stored_count: usize,
    pub(crate) last_changed_at_ms: Option<i64>,
    pub(crate) fingerprint_match: Option<bool>,
    pub(crate) last_error_code: Option<&'static str>,
}

impl From<&LocalTrashInspection> for LocalTrashQueryState {
    fn from(value: &LocalTrashInspection) -> Self {
        Self {
            health: value.health,
            available: value.available,
            can_reset: value.health.permits_reset() && value.reset_token.is_some(),
            sidecar_present: !matches!(value.health, LocalTrashHealth::Absent),
            schema_version: value.schema_version,
            generation: value.generation.clone(),
            revision: value.revision,
            stored_count: if value.health == LocalTrashHealth::Healthy {
                value.stored_count
            } else {
                0
            },
            last_changed_at_ms: value.last_changed_at_ms,
            fingerprint_match: value.fingerprint_match,
            last_error_code: value.last_error_code,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalTrashResetResult {
    pub(crate) reset: bool,
    pub(crate) previous_health: LocalTrashHealth,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ResetToken {
    canonical_workspace: PathBuf,
    scope: LocalTrashScope,
    storage: StorageSnapshot,
    selected_record: Option<ControlRecord>,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
struct FileNode {
    device: u64,
    inode: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FileState {
    node: FileNode,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    changed_seconds: i64,
    changed_nanoseconds: i64,
    mode: u32,
    owner: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct StorageSnapshot {
    control: Option<FileState>,
    slots: [Option<FileState>; 2],
}

impl StorageSnapshot {
    const fn absent() -> Self {
        Self {
            control: None,
            slots: [None, None],
        }
    }

    fn is_absent(&self) -> bool {
        self.control.is_none() && self.slots.iter().all(Option::is_none)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotIndex {
    Primary,
    Alternate,
}

impl SlotIndex {
    const fn as_usize(self) -> usize {
        match self {
            Self::Primary => 0,
            Self::Alternate => 1,
        }
    }

    const fn other(self) -> Self {
        match self {
            Self::Primary => Self::Alternate,
            Self::Alternate => Self::Primary,
        }
    }

    const fn as_u8(self) -> u8 {
        self.as_usize() as u8
    }

    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Primary),
            1 => Some(Self::Alternate),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ControlRecord {
    sequence: u64,
    active_slot: SlotIndex,
    slot_nodes: [FileNode; 2],
    active_size: u64,
    revision: u64,
    workspace_fingerprint: [u8; 32],
    source_fingerprint: [u8; 32],
    identity_version_digest: [u8; 32],
    generation_digest: [u8; 32],
    active_digest: [u8; 32],
}

enum ParsedControlRecord {
    Empty,
    Preparing(ControlRecord),
    Committed(ControlRecord),
    Invalid,
}

#[derive(Clone, Debug)]
struct ValidatedHeader {
    generation: String,
    revision: u64,
    last_changed_at_ms: Option<i64>,
}

#[derive(Clone, Debug)]
struct ValidatedDetails {
    header: ValidatedHeader,
    stored_count: usize,
}

#[derive(Clone, Debug)]
struct ValidationFailure {
    health: LocalTrashHealth,
    code: &'static str,
    schema_version: Option<i64>,
    reset_token: Option<Box<ResetToken>>,
}

impl ValidationFailure {
    fn unavailable(code: &'static str) -> Self {
        Self {
            health: LocalTrashHealth::Unavailable,
            code,
            schema_version: None,
            reset_token: None,
        }
    }

    fn invalid(code: &'static str, reset_token: Option<ResetToken>) -> Self {
        Self {
            health: LocalTrashHealth::Invalid,
            code,
            schema_version: None,
            reset_token: reset_token.map(Box::new),
        }
    }

    fn future(schema_version: i64, reset_token: Option<ResetToken>) -> Self {
        Self {
            health: LocalTrashHealth::FutureSchema,
            code: ERROR_FUTURE_SCHEMA,
            schema_version: Some(schema_version),
            reset_token: reset_token.map(Box::new),
        }
    }

    fn mismatch(reset_token: Option<ResetToken>) -> Self {
        Self {
            health: LocalTrashHealth::FingerprintMismatch,
            code: ERROR_FINGERPRINT_MISMATCH,
            schema_version: Some(LOCAL_STATE_SCHEMA_VERSION),
            reset_token: reset_token.map(Box::new),
        }
    }
}

struct OpenedReader {
    conn: Connection,
    file: File,
    control_file: File,
    pinned_workspace: PinnedWorkspace,
    initial_state: FileState,
    reset_token: ResetToken,
    selected_record: ControlRecord,
    _process_guard: WorkspaceGateGuard,
}

enum ReaderOpen {
    Absent(PathBuf),
    Opened(Box<OpenedReader>),
}

fn local_error(code: &'static str, message: &'static str) -> AppError {
    AppError::coded(message, code)
}

fn sqlite_operation_error(error: SqliteError) -> AppError {
    if is_sqlite_interrupt(&error) {
        return AppError::from(error);
    }
    let code = match &error {
        SqliteError::SqliteFailure(sqlite, _)
            if matches!(
                sqlite.code,
                ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked
            ) =>
        {
            ERROR_BUSY
        }
        _ => ERROR_UNAVAILABLE,
    };
    let message = if code == ERROR_BUSY {
        "Local Trash is busy. Try again."
    } else {
        "Local Trash is unavailable."
    };
    local_error(code, message)
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn is_opaque_id(value: &str) -> bool {
    (OPAQUE_ID_MIN_BYTES..=OPAQUE_ID_MAX_BYTES).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn is_valid_timestamp(value: i64) -> bool {
    (MIN_TIMESTAMP_MS..=MAX_TIMESTAMP_MS).contains(&value)
}

fn now_utc_ms() -> AppResult<i64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash time is unavailable."))?
        .as_millis();
    let millis = i64::try_from(millis)
        .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash time is unavailable."))?;
    if !is_valid_timestamp(millis) {
        return Err(local_error(
            ERROR_UNAVAILABLE,
            "Local Trash time is unavailable.",
        ));
    }
    Ok(millis)
}

pub(crate) fn new_operation_id() -> AppResult<String> {
    let mut bytes = [0_u8; 24];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|_| {
            local_error(
                ERROR_UNAVAILABLE,
                "A Local Trash operation could not be started.",
            )
        })?;
    Ok(hex::encode(bytes))
}

fn canonical_workspace(workspace: &Path) -> Result<(PathBuf, FileNode), ValidationFailure> {
    let metadata = fs::symlink_metadata(workspace)
        .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o022 != 0
    {
        return Err(ValidationFailure::unavailable(ERROR_UNSAFE_FILE));
    }
    let canonical = workspace
        .canonicalize()
        .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
    if canonical != workspace {
        return Err(ValidationFailure::unavailable(ERROR_UNSAFE_FILE));
    }
    let canonical_metadata = fs::symlink_metadata(&canonical)
        .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
    if canonical_metadata.file_type().is_symlink()
        || !canonical_metadata.is_dir()
        || canonical_metadata.dev() != metadata.dev()
        || canonical_metadata.ino() != metadata.ino()
        || canonical_metadata.uid() != unsafe { libc::geteuid() }
        || canonical_metadata.mode() & 0o022 != 0
    {
        return Err(ValidationFailure::unavailable(ERROR_UNSAFE_FILE));
    }
    Ok((
        canonical,
        FileNode {
            device: canonical_metadata.dev(),
            inode: canonical_metadata.ino(),
        },
    ))
}

struct PinnedWorkspace {
    directory: File,
    authorized_path: PathBuf,
    device: u64,
    inode: u64,
}

impl PinnedWorkspace {
    fn node(&self) -> FileNode {
        FileNode {
            device: self.device,
            inode: self.inode,
        }
    }

    fn validate_directory(&self) -> Result<(), ValidationFailure> {
        let descriptor_metadata = self
            .directory
            .metadata()
            .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
        if !descriptor_metadata.is_dir()
            || descriptor_metadata.dev() != self.device
            || descriptor_metadata.ino() != self.inode
            || descriptor_metadata.uid() != unsafe { libc::geteuid() }
            || descriptor_metadata.mode() & 0o022 != 0
        {
            return Err(ValidationFailure::unavailable(ERROR_UNSAFE_FILE));
        }
        Ok(())
    }
}

fn pin_workspace(workspace: &Path) -> Result<PinnedWorkspace, ValidationFailure> {
    let (authorized_path, expected_node) = canonical_workspace(workspace)?;
    let directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(&authorized_path)
        .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
    let metadata = directory
        .metadata()
        .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
    if !metadata.is_dir()
        || metadata.dev() != expected_node.device
        || metadata.ino() != expected_node.inode
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o022 != 0
    {
        return Err(ValidationFailure::unavailable(ERROR_UNSAFE_FILE));
    }
    let pinned = PinnedWorkspace {
        directory,
        authorized_path: authorized_path.clone(),
        device: metadata.dev(),
        inode: metadata.ino(),
    };
    pinned.validate_directory()?;
    Ok(pinned)
}

#[cfg(test)]
fn path_for(workspace: &Path) -> PathBuf {
    workspace.join(LOCAL_TRASH_FILE_NAME)
}

#[cfg(test)]
fn companion_path(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn state_from_metadata(metadata: &fs::Metadata) -> FileState {
    FileState {
        node: FileNode {
            device: metadata.dev(),
            inode: metadata.ino(),
        },
        size: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        changed_seconds: metadata.ctime(),
        changed_nanoseconds: metadata.ctime_nsec(),
        mode: metadata.mode() & 0o7777,
        owner: metadata.uid(),
    }
}

fn private_file_state(metadata: &fs::Metadata) -> Result<FileState, ValidationFailure> {
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o7777 != PRIVATE_FILE_MODE
    {
        return Err(ValidationFailure::invalid(ERROR_UNSAFE_FILE, None));
    }
    if metadata.len() > MAX_LOCAL_STATE_DATABASE_BYTES {
        return Err(ValidationFailure::invalid(ERROR_OVERSIZED, None));
    }
    Ok(state_from_metadata(metadata))
}

fn local_state_name() -> &'static CStr {
    c"local-state.sqlite"
}

fn local_state_alt_name() -> &'static CStr {
    c"local-state-alt.sqlite"
}

fn local_state_control_name() -> &'static CStr {
    c"local-state.control"
}

fn slot_name(slot: SlotIndex) -> &'static CStr {
    match slot {
        SlotIndex::Primary => local_state_name(),
        SlotIndex::Alternate => local_state_alt_name(),
    }
}

fn slot_file_name(slot: SlotIndex) -> &'static str {
    match slot {
        SlotIndex::Primary => LOCAL_TRASH_FILE_NAME,
        SlotIndex::Alternate => LOCAL_TRASH_ALT_FILE_NAME,
    }
}

#[derive(Default)]
struct WorkspaceGateState {
    readers: usize,
    writer: bool,
    waiting_writers: usize,
}

#[derive(Default)]
struct WorkspaceGate {
    state: Mutex<WorkspaceGateState>,
    changed: Condvar,
}

enum WorkspaceGateMode {
    Read,
    Write,
}

struct WorkspaceGateGuard {
    gate: Arc<WorkspaceGate>,
    mode: WorkspaceGateMode,
}

impl Drop for WorkspaceGateGuard {
    fn drop(&mut self) {
        let mut state = self
            .gate
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match self.mode {
            WorkspaceGateMode::Read => {
                state.readers = state.readers.saturating_sub(1);
            }
            WorkspaceGateMode::Write => {
                state.writer = false;
            }
        }
        self.gate.changed.notify_all();
    }
}

fn workspace_gate(node: FileNode) -> Result<Arc<WorkspaceGate>, ValidationFailure> {
    let registry = LOCAL_TRASH_OPERATION_GATES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut gates = registry
        .lock()
        .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
    gates.retain(|_, gate| gate.strong_count() > 0);
    if let Some(gate) = gates.get(&node).and_then(Weak::upgrade) {
        return Ok(gate);
    }
    let gate = Arc::new(WorkspaceGate::default());
    gates.insert(node, Arc::downgrade(&gate));
    Ok(gate)
}

fn acquire_workspace_read_guard(
    workspace: &PinnedWorkspace,
) -> Result<WorkspaceGateGuard, ValidationFailure> {
    let gate = workspace_gate(workspace.node())?;
    let started = Instant::now();
    let mut state = gate
        .state
        .lock()
        .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
    loop {
        if !state.writer && state.waiting_writers == 0 {
            state.readers = state
                .readers
                .checked_add(1)
                .ok_or_else(|| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
            drop(state);
            return Ok(WorkspaceGateGuard {
                gate,
                mode: WorkspaceGateMode::Read,
            });
        }
        let remaining = SQLITE_BUSY_TIMEOUT.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return Err(ValidationFailure::unavailable(ERROR_BUSY));
        }
        let (next, waited) = gate
            .changed
            .wait_timeout(state, remaining)
            .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
        state = next;
        if waited.timed_out() && (state.writer || state.waiting_writers > 0) {
            return Err(ValidationFailure::unavailable(ERROR_BUSY));
        }
    }
}

fn acquire_workspace_write_guard(
    workspace: &PinnedWorkspace,
) -> Result<WorkspaceGateGuard, ValidationFailure> {
    let gate = workspace_gate(workspace.node())?;
    let started = Instant::now();
    let mut state = gate
        .state
        .lock()
        .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
    state.waiting_writers = state
        .waiting_writers
        .checked_add(1)
        .ok_or_else(|| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
    loop {
        if !state.writer && state.readers == 0 {
            state.waiting_writers = state.waiting_writers.saturating_sub(1);
            state.writer = true;
            drop(state);
            return Ok(WorkspaceGateGuard {
                gate,
                mode: WorkspaceGateMode::Write,
            });
        }
        let remaining = SQLITE_BUSY_TIMEOUT.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            state.waiting_writers = state.waiting_writers.saturating_sub(1);
            gate.changed.notify_all();
            return Err(ValidationFailure::unavailable(ERROR_BUSY));
        }
        let (next, waited) = gate
            .changed
            .wait_timeout(state, remaining)
            .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
        state = next;
        if waited.timed_out() && (state.writer || state.readers > 0) {
            state.waiting_writers = state.waiting_writers.saturating_sub(1);
            gate.changed.notify_all();
            return Err(ValidationFailure::unavailable(ERROR_BUSY));
        }
    }
}

fn acquire_file_lock(file: &File, exclusive: bool) -> std::io::Result<()> {
    let operation = if exclusive {
        libc::LOCK_EX
    } else {
        libc::LOCK_SH
    };
    let started = Instant::now();
    loop {
        let result = unsafe { libc::flock(file.as_raw_fd(), operation | libc::LOCK_NB) };
        if result == 0 {
            return Ok(());
        }
        let error = std::io::Error::last_os_error();
        let would_block = error
            .raw_os_error()
            .is_some_and(|code| code == libc::EWOULDBLOCK || code == libc::EAGAIN);
        if !would_block || started.elapsed() >= SQLITE_BUSY_TIMEOUT {
            return Err(error);
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn slot_companion_name(slot: SlotIndex, suffix: &str) -> Result<CString, ValidationFailure> {
    CString::new(format!("{}{suffix}", slot_file_name(slot)))
        .map_err(|_| ValidationFailure::unavailable(ERROR_INVALID_INPUT))
}

fn openat_file(
    workspace: &PinnedWorkspace,
    name: &CStr,
    flags: libc::c_int,
    mode: libc::c_uint,
) -> std::io::Result<File> {
    workspace
        .validate_directory()
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EPERM))?;
    let descriptor =
        unsafe { libc::openat(workspace.directory.as_raw_fd(), name.as_ptr(), flags, mode) };
    if descriptor == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(descriptor) })
    }
}

fn entry_stat(
    workspace: &PinnedWorkspace,
    name: &CStr,
) -> Result<Option<libc::stat>, ValidationFailure> {
    workspace.validate_directory()?;
    let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
    let result = unsafe {
        libc::fstatat(
            workspace.directory.as_raw_fd(),
            name.as_ptr(),
            metadata.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if result == 0 {
        return Ok(Some(unsafe { metadata.assume_init() }));
    }
    let error = std::io::Error::last_os_error();
    if error.kind() == std::io::ErrorKind::NotFound {
        Ok(None)
    } else {
        Err(ValidationFailure::unavailable(ERROR_UNAVAILABLE))
    }
}

fn entry_exists(workspace: &PinnedWorkspace, name: &CStr) -> Result<bool, ValidationFailure> {
    entry_stat(workspace, name).map(|metadata| metadata.is_some())
}

fn private_stat_state(metadata: &libc::stat) -> Result<FileState, ValidationFailure> {
    let mode = u32::from(metadata.st_mode);
    if mode & u32::from(libc::S_IFMT) != u32::from(libc::S_IFREG)
        || metadata.st_nlink != 1
        || metadata.st_uid != unsafe { libc::geteuid() }
        || mode & 0o7777 != PRIVATE_FILE_MODE
    {
        return Err(ValidationFailure::invalid(ERROR_UNSAFE_FILE, None));
    }
    let size = u64::try_from(metadata.st_size)
        .map_err(|_| ValidationFailure::invalid(ERROR_OVERSIZED, None))?;
    if size > MAX_LOCAL_STATE_DATABASE_BYTES {
        return Err(ValidationFailure::invalid(ERROR_OVERSIZED, None));
    }
    let device = u64::try_from(metadata.st_dev)
        .map_err(|_| ValidationFailure::invalid(ERROR_UNSAFE_FILE, None))?;
    let inode = metadata.st_ino;
    Ok(FileState {
        node: FileNode { device, inode },
        size,
        modified_seconds: metadata.st_mtime,
        modified_nanoseconds: metadata.st_mtime_nsec,
        changed_seconds: metadata.st_ctime,
        changed_nanoseconds: metadata.st_ctime_nsec,
        mode: mode & 0o7777,
        owner: metadata.st_uid,
    })
}

fn open_named_private_file(
    workspace: &PinnedWorkspace,
    name: &CStr,
    writable: bool,
) -> Result<(File, FileState), ValidationFailure> {
    let access = if writable {
        libc::O_RDWR
    } else {
        libc::O_RDONLY
    };
    let file = openat_file(
        workspace,
        name,
        access | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        0,
    )
    .map_err(|error| {
        if error.raw_os_error() == Some(libc::ELOOP) {
            ValidationFailure::invalid(ERROR_UNSAFE_FILE, None)
        } else {
            ValidationFailure::unavailable(ERROR_UNAVAILABLE)
        }
    })?;
    let metadata = file
        .metadata()
        .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
    let state = private_file_state(&metadata)?;
    Ok((file, state))
}

fn named_private_state(
    workspace: &PinnedWorkspace,
    name: &CStr,
) -> Result<FileState, ValidationFailure> {
    let metadata = entry_stat(workspace, name)?
        .ok_or_else(|| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
    private_stat_state(&metadata)
}

fn require_no_wal_or_shm_at(workspace: &PinnedWorkspace) -> Result<(), ValidationFailure> {
    for slot in [SlotIndex::Primary, SlotIndex::Alternate] {
        for suffix in ["-wal", "-shm"] {
            let name = slot_companion_name(slot, suffix)?;
            if entry_exists(workspace, &name)? {
                return Err(ValidationFailure::invalid(ERROR_INVALID, None));
            }
        }
    }
    Ok(())
}

fn require_no_companions_at(workspace: &PinnedWorkspace) -> Result<(), ValidationFailure> {
    require_no_wal_or_shm_at(workspace)?;
    for slot in [SlotIndex::Primary, SlotIndex::Alternate] {
        let journal = slot_companion_name(slot, "-journal")?;
        if entry_exists(workspace, &journal)? {
            return Err(ValidationFailure::unavailable(ERROR_BUSY));
        }
    }
    Ok(())
}

fn sqlite_file_descriptor_path(file: &File) -> PathBuf {
    #[cfg(target_vendor = "apple")]
    let root = "/dev/fd";
    #[cfg(not(target_vendor = "apple"))]
    let root = "/proc/self/fd";
    Path::new(root).join(file.as_raw_fd().to_string())
}

fn create_private_file_at(
    workspace: &PinnedWorkspace,
    name: &CStr,
) -> AppResult<(File, FileState)> {
    let file = openat_file(
        workspace,
        name,
        libc::O_RDWR | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        PRIVATE_FILE_MODE as libc::c_uint,
    )
    .map_err(|error| {
        if error.kind() == std::io::ErrorKind::AlreadyExists {
            local_error(ERROR_BUSY, "Local Trash changed while it was opened.")
        } else {
            local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable.")
        }
    })?;
    file.set_permissions(fs::Permissions::from_mode(PRIVATE_FILE_MODE))
        .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?;
    let metadata = file
        .metadata()
        .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?;
    let state = private_file_state(&metadata).map_err(validation_app_error)?;
    Ok((file, state))
}

fn storage_snapshot(workspace: &PinnedWorkspace) -> Result<StorageSnapshot, ValidationFailure> {
    let control = if entry_exists(workspace, local_state_control_name())? {
        let state = named_private_state(workspace, local_state_control_name())?;
        Some(state)
    } else {
        None
    };
    let mut slots = [None, None];
    for slot in [SlotIndex::Primary, SlotIndex::Alternate] {
        if entry_exists(workspace, slot_name(slot))? {
            slots[slot.as_usize()] = Some(named_private_state(workspace, slot_name(slot))?);
        }
    }
    Ok(StorageSnapshot { control, slots })
}

fn read_exact_at(file: &File, mut buffer: &mut [u8], mut offset: u64) -> std::io::Result<()> {
    while !buffer.is_empty() {
        let read = file.read_at(buffer, offset)?;
        if read == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof));
        }
        offset = offset.saturating_add(read as u64);
        buffer = &mut buffer[read..];
    }
    Ok(())
}

fn write_all_at(file: &File, mut buffer: &[u8], mut offset: u64) -> std::io::Result<()> {
    while !buffer.is_empty() {
        let written = file.write_at(buffer, offset)?;
        if written == 0 {
            return Err(std::io::Error::from(std::io::ErrorKind::WriteZero));
        }
        offset = offset.saturating_add(written as u64);
        buffer = &buffer[written..];
    }
    Ok(())
}

fn file_digest(file: &File, expected_size: u64) -> Result<[u8; 32], ValidationFailure> {
    if expected_size > MAX_LOCAL_STATE_DATABASE_BYTES {
        return Err(ValidationFailure::invalid(ERROR_OVERSIZED, None));
    }
    let mut hasher = Sha256::new();
    let mut offset = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    while offset < expected_size {
        let remaining = expected_size.saturating_sub(offset);
        let wanted = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| ValidationFailure::invalid(ERROR_OVERSIZED, None))?;
        let read = file
            .read_at(&mut buffer[..wanted], offset)
            .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
        if read == 0 {
            return Err(ValidationFailure::unavailable(ERROR_UNSAFE_FILE));
        }
        hasher.update(&buffer[..read]);
        offset = offset.saturating_add(read as u64);
    }
    let after = private_file_state(
        &file
            .metadata()
            .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?,
    )?;
    if after.size != expected_size {
        return Err(ValidationFailure::unavailable(ERROR_UNSAFE_FILE));
    }
    Ok(hasher.finalize().into())
}

fn sha256_bytes(value: &[u8]) -> [u8; 32] {
    Sha256::digest(value).into()
}

fn fingerprint_bytes(value: &str) -> Result<[u8; 32], ValidationFailure> {
    let decoded =
        hex::decode(value).map_err(|_| ValidationFailure::unavailable(ERROR_INVALID_INPUT))?;
    decoded
        .try_into()
        .map_err(|_| ValidationFailure::unavailable(ERROR_INVALID_INPUT))
}

fn put_u32(buffer: &mut [u8], offset: usize, value: u32) {
    buffer[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn put_u64(buffer: &mut [u8], offset: usize, value: u64) {
    buffer[offset..offset + 8].copy_from_slice(&value.to_be_bytes());
}

fn get_u32(buffer: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(
        buffer[offset..offset + 4]
            .try_into()
            .expect("fixed control field"),
    )
}

fn get_u64(buffer: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes(
        buffer[offset..offset + 8]
            .try_into()
            .expect("fixed control field"),
    )
}

fn get_array<const N: usize>(buffer: &[u8], offset: usize) -> [u8; N] {
    buffer[offset..offset + N]
        .try_into()
        .expect("fixed control field")
}

fn encode_control_record(record: &ControlRecord, status: u64) -> [u8; CONTROL_RECORD_BYTES] {
    let mut bytes = [0_u8; CONTROL_RECORD_BYTES];
    put_u64(&mut bytes, 0, status);
    bytes[8..24].copy_from_slice(CONTROL_MAGIC);
    put_u32(&mut bytes, 24, CONTROL_FORMAT_VERSION);
    bytes[28] = record.active_slot.as_u8();
    put_u64(&mut bytes, 32, record.sequence);
    put_u64(&mut bytes, 40, record.slot_nodes[0].device);
    put_u64(&mut bytes, 48, record.slot_nodes[0].inode);
    put_u64(&mut bytes, 56, record.slot_nodes[1].device);
    put_u64(&mut bytes, 64, record.slot_nodes[1].inode);
    put_u64(&mut bytes, 72, record.active_size);
    put_u64(&mut bytes, 80, record.revision);
    bytes[88..120].copy_from_slice(&record.workspace_fingerprint);
    bytes[120..152].copy_from_slice(&record.source_fingerprint);
    bytes[152..184].copy_from_slice(&record.identity_version_digest);
    bytes[184..216].copy_from_slice(&record.generation_digest);
    bytes[216..248].copy_from_slice(&record.active_digest);
    let checksum_offset = CONTROL_RECORD_BYTES - 32;
    let mut checksum_input = bytes;
    put_u64(&mut checksum_input, 0, CONTROL_STATUS_COMMITTED);
    checksum_input[checksum_offset..].fill(0);
    let checksum = sha256_bytes(&checksum_input);
    bytes[checksum_offset..].copy_from_slice(&checksum);
    bytes
}

fn parse_control_record(bytes: &[u8; CONTROL_RECORD_BYTES]) -> ParsedControlRecord {
    let status = get_u64(bytes, 0);
    if status == CONTROL_STATUS_EMPTY && bytes.iter().all(|byte| *byte == 0) {
        return ParsedControlRecord::Empty;
    }
    if !matches!(status, CONTROL_STATUS_PREPARING | CONTROL_STATUS_COMMITTED)
        || &bytes[8..24] != CONTROL_MAGIC
        || get_u32(bytes, 24) != CONTROL_FORMAT_VERSION
        || bytes[29..32].iter().any(|byte| *byte != 0)
        || bytes[248..CONTROL_RECORD_BYTES - 32]
            .iter()
            .any(|byte| *byte != 0)
    {
        return ParsedControlRecord::Invalid;
    }
    let checksum_offset = CONTROL_RECORD_BYTES - 32;
    let expected_checksum = get_array::<32>(bytes, checksum_offset);
    let mut checksum_input = *bytes;
    put_u64(&mut checksum_input, 0, CONTROL_STATUS_COMMITTED);
    checksum_input[checksum_offset..].fill(0);
    if sha256_bytes(&checksum_input) != expected_checksum {
        return ParsedControlRecord::Invalid;
    }
    let Some(active_slot) = SlotIndex::from_u8(bytes[28]) else {
        return ParsedControlRecord::Invalid;
    };
    let sequence = get_u64(bytes, 32);
    let slot_nodes = [
        FileNode {
            device: get_u64(bytes, 40),
            inode: get_u64(bytes, 48),
        },
        FileNode {
            device: get_u64(bytes, 56),
            inode: get_u64(bytes, 64),
        },
    ];
    let active_size = get_u64(bytes, 72);
    let revision = get_u64(bytes, 80);
    if sequence == 0
        || slot_nodes
            .iter()
            .any(|node| node.device == 0 || node.inode == 0)
        || active_size > MAX_LOCAL_STATE_DATABASE_BYTES
        || revision > MAX_SAFE_JSON_INTEGER
    {
        return ParsedControlRecord::Invalid;
    }
    let record = ControlRecord {
        sequence,
        active_slot,
        slot_nodes,
        active_size,
        revision,
        workspace_fingerprint: get_array(bytes, 88),
        source_fingerprint: get_array(bytes, 120),
        identity_version_digest: get_array(bytes, 152),
        generation_digest: get_array(bytes, 184),
        active_digest: get_array(bytes, 216),
    };
    if status == CONTROL_STATUS_PREPARING {
        ParsedControlRecord::Preparing(record)
    } else {
        ParsedControlRecord::Committed(record)
    }
}

fn read_control_records(file: &File) -> Result<[ParsedControlRecord; 2], ValidationFailure> {
    let mut file_bytes = [0_u8; CONTROL_RECORD_BYTES * 2];
    read_exact_at(file, &mut file_bytes, 0)
        .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
    let first: [u8; CONTROL_RECORD_BYTES] = file_bytes[..CONTROL_RECORD_BYTES]
        .try_into()
        .expect("fixed control record");
    let second: [u8; CONTROL_RECORD_BYTES] = file_bytes[CONTROL_RECORD_BYTES..]
        .try_into()
        .expect("fixed control record");
    Ok([parse_control_record(&first), parse_control_record(&second)])
}

fn select_control_record(
    records: [ParsedControlRecord; 2],
) -> Result<(usize, ControlRecord), ValidationFailure> {
    let mut selectable = Vec::new();
    for (index, record) in records.into_iter().enumerate() {
        let record = match record {
            ParsedControlRecord::Committed(record) | ParsedControlRecord::Preparing(record) => {
                record
            }
            ParsedControlRecord::Empty => continue,
            ParsedControlRecord::Invalid => {
                return Err(ValidationFailure::invalid(ERROR_INVALID, None));
            }
        };
        let parity_matches_slot = match record.active_slot {
            SlotIndex::Primary => record.sequence % 2 == 1,
            SlotIndex::Alternate => record.sequence % 2 == 0,
        };
        if record.active_slot.as_usize() != index || !parity_matches_slot {
            return Err(ValidationFailure::invalid(ERROR_INVALID, None));
        }
        selectable.push((index, record));
    }
    match selectable.as_slice() {
        [] => Err(ValidationFailure::invalid(ERROR_INVALID, None)),
        [(index, record)] => {
            if *index != SlotIndex::Primary.as_usize()
                || record.active_slot != SlotIndex::Primary
                || record.sequence != 1
            {
                return Err(ValidationFailure::invalid(ERROR_INVALID, None));
            }
            Ok((*index, record.clone()))
        }
        [(left_index, left), (right_index, right)] => {
            if left.sequence.abs_diff(right.sequence) != 1
                || left.slot_nodes != right.slot_nodes
                || left.workspace_fingerprint != right.workspace_fingerprint
                || left.source_fingerprint != right.source_fingerprint
                || left.identity_version_digest != right.identity_version_digest
            {
                return Err(ValidationFailure::invalid(ERROR_INVALID, None));
            }
            Ok(if left.sequence > right.sequence {
                (*left_index, left.clone())
            } else {
                (*right_index, right.clone())
            })
        }
        _ => unreachable!("control has exactly two records"),
    }
}

fn write_control_record(
    file: &File,
    record_index: usize,
    record: &ControlRecord,
) -> std::io::Result<()> {
    if record_index >= 2 {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    let offset = u64::try_from(record_index * CONTROL_RECORD_BYTES)
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    write_all_at(file, &CONTROL_STATUS_PREPARING.to_be_bytes(), offset)?;
    file.sync_all()?;
    let preparing = encode_control_record(record, CONTROL_STATUS_PREPARING);
    write_all_at(file, &preparing, offset)?;
    file.sync_all()?;
    Ok(())
}

fn publish_control_record(
    file: &File,
    record_index: usize,
    record: &ControlRecord,
) -> std::io::Result<()> {
    match write_control_record(file, record_index, record) {
        Ok(()) => Ok(()),
        Err(error) => {
            let confirmed_after_retry = file.sync_all().is_ok()
                && read_control_records(file)
                    .ok()
                    .and_then(|records| select_control_record(records).ok())
                    .is_some_and(|(index, selected)| index == record_index && selected == *record);
            if confirmed_after_retry {
                Ok(())
            } else {
                Err(error)
            }
        }
    }
}

struct PublicationSlot<'a> {
    file: &'a File,
    state: FileState,
}

fn reconcile_control_publication(
    pinned_workspace: &PinnedWorkspace,
    control_file: &File,
    expected_control_node: FileNode,
    selected_slot: PublicationSlot<'_>,
    nonselected_slot: PublicationSlot<'_>,
    selected_record_index: usize,
    selected_record: &ControlRecord,
) -> Result<(), ValidationFailure> {
    pinned_workspace.validate_directory()?;
    require_no_companions_at(pinned_workspace)?;

    let control_held = private_file_state(
        &control_file
            .metadata()
            .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?,
    )?;
    let control_named = named_private_state(pinned_workspace, local_state_control_name())?;
    if control_held.node != expected_control_node
        || control_held.size != CONTROL_FILE_BYTES
        || control_named != control_held
    {
        return Err(ValidationFailure::invalid(ERROR_UNSAFE_FILE, None));
    }

    let selected = select_control_record(read_control_records(control_file)?)?;
    let selected_held = private_file_state(
        &selected_slot
            .file
            .metadata()
            .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?,
    )?;
    let selected_named =
        named_private_state(pinned_workspace, slot_name(selected_record.active_slot))?;
    let nonselected_slot_index = selected_record.active_slot.other();
    let nonselected_held = private_file_state(
        &nonselected_slot
            .file
            .metadata()
            .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?,
    )?;
    let nonselected_named =
        named_private_state(pinned_workspace, slot_name(nonselected_slot_index))?;
    if selected.0 != selected_record_index
        || selected.1 != *selected_record
        || selected_held != selected_slot.state
        || selected_named != selected_held
        || selected_held.node != selected_record.slot_nodes[selected_record.active_slot.as_usize()]
        || selected_held.size != selected_record.active_size
        || file_digest(selected_slot.file, selected_held.size)? != selected_record.active_digest
        || nonselected_held != nonselected_slot.state
        || nonselected_named != nonselected_held
        || nonselected_held.node != selected_record.slot_nodes[nonselected_slot_index.as_usize()]
    {
        return Err(ValidationFailure::invalid(ERROR_UNSAFE_FILE, None));
    }
    Ok(())
}

fn reset_token(
    canonical_workspace: &Path,
    scope: &LocalTrashScope,
    storage: StorageSnapshot,
    selected_record: Option<ControlRecord>,
) -> ResetToken {
    ResetToken {
        canonical_workspace: canonical_workspace.to_path_buf(),
        scope: scope.clone(),
        storage,
        selected_record,
    }
}

fn open_reader(workspace: &Path, scope: &LocalTrashScope) -> Result<ReaderOpen, ValidationFailure> {
    open_reader_inner(workspace, scope, || {})
}

fn open_reader_inner<F>(
    workspace: &Path,
    scope: &LocalTrashScope,
    after_workspace_pin: F,
) -> Result<ReaderOpen, ValidationFailure>
where
    F: FnOnce(),
{
    scope
        .validate()
        .map_err(|_| ValidationFailure::unavailable(ERROR_INVALID_INPUT))?;
    let pinned_workspace = pin_workspace(workspace)?;
    let canonical_workspace = pinned_workspace.authorized_path.clone();
    after_workspace_pin();
    open_reader_from_pinned(pinned_workspace, canonical_workspace, scope)
}

fn open_reader_from_pinned(
    pinned_workspace: PinnedWorkspace,
    canonical_workspace: PathBuf,
    scope: &LocalTrashScope,
) -> Result<ReaderOpen, ValidationFailure> {
    let process_guard = acquire_workspace_read_guard(&pinned_workspace)?;
    pinned_workspace.validate_directory()?;
    require_no_companions_at(&pinned_workspace)?;
    let initial_storage = storage_snapshot(&pinned_workspace)?;
    if initial_storage.is_absent() {
        return Ok(ReaderOpen::Absent(canonical_workspace));
    }
    let provisional_token = reset_token(&canonical_workspace, scope, initial_storage.clone(), None);
    if initial_storage.control.is_none() || initial_storage.slots.iter().any(Option::is_none) {
        return Err(ValidationFailure::invalid(
            ERROR_INVALID,
            Some(provisional_token),
        ));
    }
    let (control_file, control_state) =
        open_named_private_file(&pinned_workspace, local_state_control_name(), false)?;
    if control_state != initial_storage.control.expect("checked control state")
        || control_state.size != CONTROL_FILE_BYTES
    {
        return Err(ValidationFailure::invalid(
            ERROR_INVALID,
            Some(provisional_token),
        ));
    }
    acquire_file_lock(&control_file, false)
        .map_err(|_| ValidationFailure::unavailable(ERROR_BUSY))?;
    let locked_storage = storage_snapshot(&pinned_workspace)?;
    if locked_storage != initial_storage
        || private_file_state(
            &control_file
                .metadata()
                .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?,
        )? != control_state
    {
        return Err(ValidationFailure::unavailable(ERROR_UNSAFE_FILE));
    }
    let records = read_control_records(&control_file).map_err(|mut failure| {
        if failure.reset_token.is_none() {
            failure.reset_token = Some(Box::new(provisional_token.clone()));
        }
        failure
    })?;
    let (_, selected_record) = select_control_record(records).map_err(|mut failure| {
        if failure.reset_token.is_none() {
            failure.reset_token = Some(Box::new(provisional_token.clone()));
        }
        failure
    })?;
    let token = reset_token(
        &canonical_workspace,
        scope,
        initial_storage.clone(),
        Some(selected_record.clone()),
    );
    if selected_record.workspace_fingerprint != fingerprint_bytes(scope.workspace_fingerprint())?
        || selected_record.source_fingerprint != fingerprint_bytes(scope.source_fingerprint())?
        || selected_record.identity_version_digest
            != sha256_bytes(LOCAL_TRASH_IDENTITY_VERSION.as_bytes())
    {
        return Err(ValidationFailure::mismatch(Some(token)));
    }
    for slot in [SlotIndex::Primary, SlotIndex::Alternate] {
        if initial_storage.slots[slot.as_usize()].map(|state| state.node)
            != Some(selected_record.slot_nodes[slot.as_usize()])
        {
            return Err(ValidationFailure::invalid(ERROR_UNSAFE_FILE, Some(token)));
        }
    }
    let active_slot = selected_record.active_slot;
    let (file, initial_state) =
        open_named_private_file(&pinned_workspace, slot_name(active_slot), false)?;
    if initial_state.node != selected_record.slot_nodes[active_slot.as_usize()]
        || initial_state.size != selected_record.active_size
        || file_digest(&file, initial_state.size)? != selected_record.active_digest
        || named_private_state(&pinned_workspace, slot_name(active_slot))? != initial_state
    {
        return Err(ValidationFailure::invalid(ERROR_UNSAFE_FILE, Some(token)));
    }
    let conn = Connection::open_with_flags(
        sqlite_file_descriptor_path(&file),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_EXRESCODE,
    )
    .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    let after_open = named_private_state(&pinned_workspace, slot_name(active_slot))?;
    let opened_state = private_file_state(
        &file
            .metadata()
            .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?,
    )?;
    if after_open != initial_state || opened_state != initial_state {
        return Err(ValidationFailure::unavailable(ERROR_UNSAFE_FILE));
    }
    if !conn
        .is_readonly(DatabaseName::Main)
        .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?
    {
        return Err(ValidationFailure::unavailable(ERROR_UNSAFE_FILE));
    }
    conn.busy_timeout(SQLITE_BUSY_TIMEOUT)
        .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?;
    sqlite_security::configure_connection(&conn, true)
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    Ok(ReaderOpen::Opened(Box::new(OpenedReader {
        conn,
        file,
        control_file,
        pinned_workspace,
        initial_state,
        reset_token: token,
        selected_record,
        _process_guard: process_guard,
    })))
}

fn validate_reader_unchanged(reader: &OpenedReader) -> Result<FileState, ValidationFailure> {
    reader.pinned_workspace.validate_directory()?;
    require_no_companions_at(&reader.pinned_workspace)?;
    let current = named_private_state(
        &reader.pinned_workspace,
        slot_name(reader.selected_record.active_slot),
    )?;
    let opened = private_file_state(
        &reader
            .file
            .metadata()
            .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?,
    )?;
    let control = named_private_state(&reader.pinned_workspace, local_state_control_name())?;
    let held_control = private_file_state(
        &reader
            .control_file
            .metadata()
            .map_err(|_| ValidationFailure::unavailable(ERROR_UNAVAILABLE))?,
    )?;
    if current != reader.initial_state
        || opened != reader.initial_state
        || control != reader.reset_token.storage.control.expect("healthy control")
        || held_control != control
        || file_digest(&reader.file, opened.size)? != reader.selected_record.active_digest
    {
        return Err(ValidationFailure::unavailable(ERROR_UNSAFE_FILE));
    }
    Ok(current)
}

fn query_i64(conn: &Connection, sql: &str) -> Result<i64, ValidationFailure> {
    conn.query_row(sql, [], |row| row.get::<_, i64>(0))
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, None))
}

fn verify_schema(conn: &Connection, token: &ResetToken) -> Result<(), ValidationFailure> {
    let mut statement = conn
        .prepare(
            "SELECT type, name, tbl_name
             FROM main.sqlite_schema
             WHERE name NOT LIKE 'sqlite_%'
             ORDER BY type, name",
        )
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    let mut rows = statement
        .query([])
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    let mut objects = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?
    {
        objects.push((
            row.get::<_, String>(0)
                .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?,
            row.get::<_, String>(1)
                .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?,
            row.get::<_, String>(2)
                .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?,
        ));
    }
    if objects
        != [
            (
                "table".to_string(),
                "local_state_metadata".to_string(),
                "local_state_metadata".to_string(),
            ),
            (
                "table".to_string(),
                "trashed_messages".to_string(),
                "trashed_messages".to_string(),
            ),
        ]
    {
        return Err(ValidationFailure::invalid(
            ERROR_INVALID,
            Some(token.clone()),
        ));
    }

    verify_table_shape(
        conn,
        token,
        "local_state_metadata",
        &[
            ("singleton", "INTEGER", 0, 1),
            ("schema_version", "INTEGER", 1, 0),
            ("workspace_fingerprint", "TEXT", 1, 0),
            ("source_fingerprint", "TEXT", 1, 0),
            ("canonical_identity_version", "TEXT", 1, 0),
            ("generation", "TEXT", 1, 0),
            ("revision", "INTEGER", 1, 0),
            ("last_changed_at_ms", "INTEGER", 0, 0),
        ],
        false,
    )?;
    verify_table_shape(
        conn,
        token,
        "trashed_messages",
        &[
            ("workspace_fingerprint", "TEXT", 1, 1),
            ("source_fingerprint", "TEXT", 1, 2),
            ("canonical_identity_version", "TEXT", 1, 3),
            ("canonical_identity", "TEXT", 1, 4),
            ("state_revision", "INTEGER", 1, 0),
            ("trashed_at_ms", "INTEGER", 1, 0),
            ("last_changed_at_ms", "INTEGER", 1, 0),
            ("operation_id", "TEXT", 1, 0),
        ],
        true,
    )?;
    Ok(())
}

fn verify_table_shape(
    conn: &Connection,
    token: &ResetToken,
    table: &'static str,
    expected_columns: &[(&str, &str, i64, i64)],
    expected_without_rowid: bool,
) -> Result<(), ValidationFailure> {
    let pragma = match table {
        "local_state_metadata" => "PRAGMA main.table_xinfo('local_state_metadata')",
        "trashed_messages" => "PRAGMA main.table_xinfo('trashed_messages')",
        _ => {
            return Err(ValidationFailure::invalid(
                ERROR_INVALID,
                Some(token.clone()),
            ))
        }
    };
    let mut statement = conn
        .prepare(pragma)
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    let mut rows = statement
        .query([])
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    let mut actual = Vec::new();
    while let Some(row) = rows
        .next()
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?
    {
        let hidden = row
            .get::<_, i64>(6)
            .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
        let default_value = row
            .get::<_, Option<String>>(4)
            .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
        if hidden != 0 || default_value.is_some() {
            return Err(ValidationFailure::invalid(
                ERROR_INVALID,
                Some(token.clone()),
            ));
        }
        actual.push((
            row.get::<_, String>(1)
                .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?,
            row.get::<_, String>(2)
                .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?,
            row.get::<_, i64>(3)
                .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?,
            row.get::<_, i64>(5)
                .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?,
        ));
    }
    let expected = expected_columns
        .iter()
        .map(|(name, kind, not_null, primary_key)| {
            (
                (*name).to_string(),
                (*kind).to_string(),
                *not_null,
                *primary_key,
            )
        })
        .collect::<Vec<_>>();
    if actual != expected {
        return Err(ValidationFailure::invalid(
            ERROR_INVALID,
            Some(token.clone()),
        ));
    }

    let table_properties = conn
        .query_row(
            "SELECT ncol, wr, strict FROM pragma_table_list WHERE schema = 'main' AND name = ?1",
            [table],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                ))
            },
        )
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    if table_properties
        != (
            i64::try_from(expected_columns.len()).unwrap_or(i64::MAX),
            i64::from(expected_without_rowid),
            1,
        )
    {
        return Err(ValidationFailure::invalid(
            ERROR_INVALID,
            Some(token.clone()),
        ));
    }

    let index_pragma = match table {
        "local_state_metadata" => "PRAGMA main.index_list('local_state_metadata')",
        "trashed_messages" => "PRAGMA main.index_list('trashed_messages')",
        _ => {
            return Err(ValidationFailure::invalid(
                ERROR_INVALID,
                Some(token.clone()),
            ))
        }
    };
    let mut statement = conn
        .prepare(index_pragma)
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    let indexes = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    if expected_without_rowid {
        if indexes.len() != 1
            || !indexes[0]
                .0
                .starts_with("sqlite_autoindex_trashed_messages_")
            || indexes[0].1 != 1
            || indexes[0].2 != "pk"
            || indexes[0].3 != 0
        {
            return Err(ValidationFailure::invalid(
                ERROR_INVALID,
                Some(token.clone()),
            ));
        }
        let mut statement = conn
            .prepare(
                "SELECT seqno, cid, name, desc, coll, key
                   FROM pragma_index_xinfo(?1)
                  ORDER BY seqno",
            )
            .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
        let index_shape = statement
            .query_map([indexes[0].0.as_str()], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, i64>(5)?,
                ))
            })
            .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
        let expected_index_shape = expected_columns
            .iter()
            .enumerate()
            .map(|(position, (name, _, _, primary_key))| {
                (
                    i64::try_from(position).unwrap_or(i64::MAX),
                    i64::try_from(position).unwrap_or(i64::MAX),
                    (*name).to_string(),
                    0,
                    "BINARY".to_string(),
                    i64::from(*primary_key > 0),
                )
            })
            .collect::<Vec<_>>();
        if index_shape != expected_index_shape {
            return Err(ValidationFailure::invalid(
                ERROR_INVALID,
                Some(token.clone()),
            ));
        }
    } else if !indexes.is_empty() {
        return Err(ValidationFailure::invalid(
            ERROR_INVALID,
            Some(token.clone()),
        ));
    }

    let foreign_key_pragma = match table {
        "local_state_metadata" => "PRAGMA main.foreign_key_list('local_state_metadata')",
        "trashed_messages" => "PRAGMA main.foreign_key_list('trashed_messages')",
        _ => {
            return Err(ValidationFailure::invalid(
                ERROR_INVALID,
                Some(token.clone()),
            ))
        }
    };
    if conn
        .prepare(foreign_key_pragma)
        .and_then(|mut statement| statement.exists([]))
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?
    {
        return Err(ValidationFailure::invalid(
            ERROR_INVALID,
            Some(token.clone()),
        ));
    }
    Ok(())
}

fn validate_header(
    conn: &Connection,
    scope: &LocalTrashScope,
    token: &ResetToken,
) -> Result<ValidatedHeader, ValidationFailure> {
    validate_header_for_journal(conn, scope, token, "delete")
}

fn validate_header_for_journal(
    conn: &Connection,
    scope: &LocalTrashScope,
    token: &ResetToken,
    expected_journal_mode: &str,
) -> Result<ValidatedHeader, ValidationFailure> {
    let schema_version = conn
        .query_row("PRAGMA main.user_version", [], |row| row.get::<_, i64>(0))
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    if schema_version > LOCAL_STATE_SCHEMA_VERSION {
        return Err(ValidationFailure::future(
            schema_version,
            Some(token.clone()),
        ));
    }
    if schema_version != LOCAL_STATE_SCHEMA_VERSION {
        return Err(ValidationFailure::invalid(
            ERROR_INVALID,
            Some(token.clone()),
        ));
    }
    if query_i64(conn, "PRAGMA main.application_id")? != LOCAL_STATE_APPLICATION_ID {
        return Err(ValidationFailure::invalid(
            ERROR_INVALID,
            Some(token.clone()),
        ));
    }
    let journal_mode = conn
        .query_row("PRAGMA main.journal_mode", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    if !journal_mode.eq_ignore_ascii_case(expected_journal_mode) {
        return Err(ValidationFailure::invalid(
            ERROR_INVALID,
            Some(token.clone()),
        ));
    }
    let page_size = query_i64(conn, "PRAGMA main.page_size")?;
    let page_count = query_i64(conn, "PRAGMA main.page_count")?;
    let logical_bytes = page_size
        .checked_mul(page_count)
        .and_then(|value| u64::try_from(value).ok())
        .ok_or_else(|| ValidationFailure::invalid(ERROR_OVERSIZED, Some(token.clone())))?;
    if page_size <= 0 || page_count <= 0 || logical_bytes > MAX_LOCAL_STATE_DATABASE_BYTES {
        return Err(ValidationFailure::invalid(
            ERROR_OVERSIZED,
            Some(token.clone()),
        ));
    }
    let integrity = conn
        .query_row("PRAGMA main.integrity_check(1)", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    if integrity != "ok" {
        return Err(ValidationFailure::invalid(
            ERROR_INVALID,
            Some(token.clone()),
        ));
    }
    verify_schema(conn, token)?;

    let mut statement = conn
        .prepare(
            "SELECT singleton, schema_version, workspace_fingerprint,
                    source_fingerprint, canonical_identity_version, generation,
                    revision, last_changed_at_ms
             FROM local_state_metadata
             ORDER BY singleton
             LIMIT 2",
        )
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    let mut rows = statement
        .query([])
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    let row = rows
        .next()
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?
        .ok_or_else(|| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    let singleton = row
        .get::<_, i64>(0)
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    let metadata_schema = row
        .get::<_, i64>(1)
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    let workspace_fingerprint = row
        .get::<_, String>(2)
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    let source_fingerprint = row
        .get::<_, String>(3)
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    let identity_version = row
        .get::<_, String>(4)
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    let generation = row
        .get::<_, String>(5)
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    let revision_i64 = row
        .get::<_, i64>(6)
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    let last_changed_at_ms = row
        .get::<_, Option<i64>>(7)
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    if rows
        .next()
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?
        .is_some()
        || singleton != 1
        || metadata_schema != LOCAL_STATE_SCHEMA_VERSION
        || !is_lower_hex_sha256(&workspace_fingerprint)
        || !is_lower_hex_sha256(&source_fingerprint)
        || identity_version != LOCAL_TRASH_IDENTITY_VERSION
        || !is_opaque_id(&generation)
        || !(0..=MAX_SAFE_JSON_INTEGER_I64).contains(&revision_i64)
        || last_changed_at_ms.is_some_and(|value| !is_valid_timestamp(value))
        || (revision_i64 == 0 && last_changed_at_ms.is_some())
        || (revision_i64 > 0 && last_changed_at_ms.is_none())
    {
        return Err(ValidationFailure::invalid(
            ERROR_INVALID,
            Some(token.clone()),
        ));
    }
    if workspace_fingerprint != scope.workspace_fingerprint
        || source_fingerprint != scope.source_fingerprint
    {
        return Err(ValidationFailure::mismatch(Some(token.clone())));
    }
    Ok(ValidatedHeader {
        generation,
        revision: u64::try_from(revision_i64)
            .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?,
        last_changed_at_ms,
    })
}

#[derive(Debug)]
enum StreamError {
    Invalid(ValidationFailure),
    Destination(SqliteError),
}

fn stream_valid_identities<F>(
    conn: &Connection,
    scope: &LocalTrashScope,
    token: &ResetToken,
    header: &ValidatedHeader,
    mut receive: F,
) -> Result<usize, StreamError>
where
    F: FnMut(&str) -> rusqlite::Result<()>,
{
    let invalid = || {
        StreamError::Invalid(ValidationFailure::invalid(
            ERROR_INVALID,
            Some(token.clone()),
        ))
    };
    let mut statement = conn
        .prepare(
            "SELECT workspace_fingerprint, source_fingerprint,
                    canonical_identity_version, canonical_identity, state_revision,
                    trashed_at_ms, last_changed_at_ms, operation_id
             FROM trashed_messages
             ORDER BY workspace_fingerprint, source_fingerprint,
                      canonical_identity_version, canonical_identity",
        )
        .map_err(|_| invalid())?;
    let mut rows = statement.query([]).map_err(|_| invalid())?;
    let mut count = 0_usize;
    let mut total_identity_bytes = 0_usize;
    while let Some(row) = rows.next().map_err(|_| invalid())? {
        let workspace_fingerprint = row.get::<_, String>(0).map_err(|_| invalid())?;
        let source_fingerprint = row.get::<_, String>(1).map_err(|_| invalid())?;
        let identity_version = row.get::<_, String>(2).map_err(|_| invalid())?;
        let identity = row.get::<_, String>(3).map_err(|_| invalid())?;
        let state_revision = row.get::<_, i64>(4).map_err(|_| invalid())?;
        let trashed_at_ms = row.get::<_, i64>(5).map_err(|_| invalid())?;
        let last_changed_at_ms = row.get::<_, i64>(6).map_err(|_| invalid())?;
        let operation_id = row.get::<_, String>(7).map_err(|_| invalid())?;
        count = count.checked_add(1).ok_or_else(invalid)?;
        total_identity_bytes = total_identity_bytes
            .checked_add(identity.len())
            .ok_or_else(invalid)?;
        if count > MAX_LOCAL_TRASH_ROWS
            || total_identity_bytes > MAX_TOTAL_IDENTITY_BYTES
            || identity.len() > MAX_IDENTITY_BYTES
            || workspace_fingerprint != scope.workspace_fingerprint
            || source_fingerprint != scope.source_fingerprint
            || identity_version != LOCAL_TRASH_IDENTITY_VERSION
            || !crate::import_integrity_phase2::validate_relative_identity(&identity)
            || !(1..=MAX_SAFE_JSON_INTEGER_I64).contains(&state_revision)
            || u64::try_from(state_revision).map_or(true, |value| value > header.revision)
            || !is_valid_timestamp(trashed_at_ms)
            || !is_valid_timestamp(last_changed_at_ms)
            || trashed_at_ms > last_changed_at_ms
            || header
                .last_changed_at_ms
                .map_or(true, |metadata_time| last_changed_at_ms > metadata_time)
            || !is_opaque_id(&operation_id)
        {
            return Err(invalid());
        }
        receive(&identity).map_err(StreamError::Destination)?;
    }
    Ok(count)
}

fn validate_connection(
    conn: &Connection,
    scope: &LocalTrashScope,
    token: &ResetToken,
) -> Result<ValidatedDetails, ValidationFailure> {
    validate_connection_for_journal(conn, scope, token, "delete")
}

fn validate_connection_for_journal(
    conn: &Connection,
    scope: &LocalTrashScope,
    token: &ResetToken,
    expected_journal_mode: &str,
) -> Result<ValidatedDetails, ValidationFailure> {
    let header = validate_header_for_journal(conn, scope, token, expected_journal_mode)?;
    let stored_count = match stream_valid_identities(conn, scope, token, &header, |_| Ok(())) {
        Ok(count) => count,
        Err(StreamError::Invalid(failure)) => return Err(failure),
        Err(StreamError::Destination(_)) => unreachable!("no-op receiver cannot fail"),
    };
    let oversized_operation = conn
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM trashed_messages
                 GROUP BY operation_id
                 HAVING COUNT(*) > ?1
                 LIMIT 1
             )",
            [i64::try_from(MAX_LOCAL_TRASH_BATCH).unwrap_or(i64::MAX)],
            |row| row.get::<_, i64>(0),
        )
        .map_err(|_| ValidationFailure::invalid(ERROR_INVALID, Some(token.clone())))?;
    if oversized_operation != 0 {
        return Err(ValidationFailure::invalid(
            ERROR_INVALID,
            Some(token.clone()),
        ));
    }
    Ok(ValidatedDetails {
        header,
        stored_count,
    })
}

fn validate_opened_reader(
    reader: &OpenedReader,
    scope: &LocalTrashScope,
) -> Result<ValidatedDetails, ValidationFailure> {
    let details = validate_connection(&reader.conn, scope, &reader.reset_token)?;
    if details.header.revision != reader.selected_record.revision
        || sha256_bytes(details.header.generation.as_bytes())
            != reader.selected_record.generation_digest
    {
        return Err(ValidationFailure::invalid(
            ERROR_INVALID,
            Some(reader.reset_token.clone()),
        ));
    }
    validate_reader_unchanged(reader)?;
    Ok(details)
}

fn inspection_from_reader_result(
    opened: Result<ReaderOpen, ValidationFailure>,
    scope: &LocalTrashScope,
) -> LocalTrashInspection {
    match opened {
        Ok(ReaderOpen::Absent(canonical_workspace)) => {
            LocalTrashInspection::absent(canonical_workspace, scope)
        }
        Ok(ReaderOpen::Opened(reader)) => match validate_opened_reader(&reader, scope) {
            Ok(details) => LocalTrashInspection::healthy(reader.reset_token.clone(), details),
            Err(mut failure) => {
                if failure.reset_token.is_none() {
                    failure.reset_token = Some(Box::new(reader.reset_token.clone()));
                }
                LocalTrashInspection::failed(failure)
            }
        },
        Err(failure) => LocalTrashInspection::failed(failure),
    }
}

pub(crate) fn inspect(workspace: &Path, scope: &LocalTrashScope) -> LocalTrashInspection {
    inspection_from_reader_result(open_reader(workspace, scope), scope)
}

struct OpenedWriter {
    conn: Option<Connection>,
    pinned_workspace: PinnedWorkspace,
    control_file: File,
    staged_file: File,
    staged_slot: SlotIndex,
    staged_node: FileNode,
    active_file: File,
    active_state: FileState,
    active_digest: [u8; 32],
    current_record: ControlRecord,
    reset_token: ResetToken,
    source_lock: Option<Connection>,
    _process_guard: WorkspaceGateGuard,
}

struct PublishedWriter {
    writer: OpenedWriter,
    record: ControlRecord,
    state: FileState,
    digest: [u8; 32],
}

impl OpenedWriter {
    fn connection_mut(&mut self) -> AppResult<&mut Connection> {
        self.conn.as_mut().ok_or_else(|| {
            local_error(
                ERROR_UNAVAILABLE,
                "Local Trash staging connection is unavailable.",
            )
        })
    }
}

impl Drop for OpenedWriter {
    fn drop(&mut self) {
        drop(self.conn.take());
        drop(self.source_lock.take());
    }
}

fn validation_app_error(failure: ValidationFailure) -> AppError {
    let message = match failure.health {
        LocalTrashHealth::FutureSchema => {
            "Local Trash was created by a newer version of PST QuickView."
        }
        LocalTrashHealth::FingerprintMismatch => {
            "Local Trash belongs to a different workspace or source."
        }
        LocalTrashHealth::Invalid => "Local Trash state is invalid.",
        LocalTrashHealth::Unavailable => "Local Trash is unavailable.",
        LocalTrashHealth::Absent | LocalTrashHealth::Healthy => "Local Trash is unavailable.",
    };
    local_error(failure.code, message)
}

fn configure_staged_writer(conn: &Connection) -> AppResult<()> {
    conn.busy_timeout(SQLITE_BUSY_TIMEOUT)
        .map_err(sqlite_operation_error)?;
    sqlite_security::configure_connection(conn, false).map_err(sqlite_operation_error)?;
    conn.execute_batch(
        "PRAGMA main.journal_mode = MEMORY;
         PRAGMA main.synchronous = FULL;
         PRAGMA main.secure_delete = ON;
         PRAGMA main.temp_store = MEMORY;
         PRAGMA main.locking_mode = EXCLUSIVE;
         PRAGMA main.journal_size_limit = 0;",
    )
    .map_err(sqlite_operation_error)?;
    let page_size = conn
        .query_row("PRAGMA main.page_size", [], |row| row.get::<_, i64>(0))
        .map_err(sqlite_operation_error)?;
    if page_size <= 0 {
        return Err(local_error(ERROR_INVALID, "Local Trash state is invalid."));
    }
    let max_pages = i64::try_from(MAX_LOCAL_STATE_DATABASE_BYTES / page_size as u64)
        .map_err(|_| local_error(ERROR_INVALID, "Local Trash state is invalid."))?;
    conn.pragma_update(None, "max_page_count", max_pages)
        .map_err(sqlite_operation_error)?;
    let journal_mode = conn
        .query_row("PRAGMA main.journal_mode", [], |row| {
            row.get::<_, String>(0)
        })
        .map_err(sqlite_operation_error)?;
    let synchronous = conn
        .query_row("PRAGMA main.synchronous", [], |row| row.get::<_, i64>(0))
        .map_err(sqlite_operation_error)?;
    let query_only = conn
        .query_row("PRAGMA query_only", [], |row| row.get::<_, i64>(0))
        .map_err(sqlite_operation_error)?;
    if !journal_mode.eq_ignore_ascii_case("memory") || synchronous != 2 || query_only != 0 {
        return Err(local_error(
            ERROR_UNAVAILABLE,
            "Local Trash durability settings are unavailable.",
        ));
    }
    Ok(())
}

fn initialize_database(conn: &mut Connection, scope: &LocalTrashScope) -> AppResult<()> {
    let generation = new_operation_id()?;
    let transaction = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite_operation_error)?;
    transaction
        .execute_batch(CREATE_SCHEMA_SQL)
        .map_err(sqlite_operation_error)?;
    transaction
        .pragma_update(None, "application_id", LOCAL_STATE_APPLICATION_ID)
        .map_err(sqlite_operation_error)?;
    transaction
        .pragma_update(None, "user_version", LOCAL_STATE_SCHEMA_VERSION)
        .map_err(sqlite_operation_error)?;
    transaction
        .execute(
            "INSERT INTO local_state_metadata (
                 singleton, schema_version, workspace_fingerprint,
                 source_fingerprint, canonical_identity_version, generation,
                 revision, last_changed_at_ms
             ) VALUES (1, ?1, ?2, ?3, ?4, ?5, 0, NULL)",
            params![
                LOCAL_STATE_SCHEMA_VERSION,
                scope.workspace_fingerprint(),
                scope.source_fingerprint(),
                LOCAL_TRASH_IDENTITY_VERSION,
                generation,
            ],
        )
        .map_err(sqlite_operation_error)?;
    transaction.commit().map_err(sqlite_operation_error)
}

fn open_staged_connection(file: &File) -> AppResult<Connection> {
    let conn = Connection::open_with_flags(
        sqlite_file_descriptor_path(file),
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_EXRESCODE,
    )
    .map_err(sqlite_operation_error)?;
    if conn
        .is_readonly(DatabaseName::Main)
        .map_err(sqlite_operation_error)?
    {
        return Err(local_error(
            ERROR_UNAVAILABLE,
            "Local Trash staging file is not writable.",
        ));
    }
    configure_staged_writer(&conn)?;
    Ok(conn)
}

fn copy_file_to_slot(source: &File, source_size: u64, destination: &File) -> AppResult<()> {
    if source_size > MAX_LOCAL_STATE_DATABASE_BYTES {
        return Err(local_error(
            ERROR_OVERSIZED,
            "Local Trash state is oversized.",
        ));
    }
    destination
        .set_len(0)
        .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?;
    let mut offset = 0_u64;
    let mut buffer = [0_u8; 64 * 1024];
    while offset < source_size {
        let wanted = usize::try_from((source_size - offset).min(buffer.len() as u64))
            .map_err(|_| local_error(ERROR_OVERSIZED, "Local Trash state is oversized."))?;
        let read = source
            .read_at(&mut buffer[..wanted], offset)
            .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?;
        if read == 0 {
            return Err(local_error(
                ERROR_UNSAFE_FILE,
                "Local Trash changed while it was staged.",
            ));
        }
        write_all_at(destination, &buffer[..read], offset)
            .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?;
        offset = offset.saturating_add(read as u64);
    }
    destination
        .set_len(source_size)
        .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?;
    destination
        .sync_all()
        .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))
}

fn control_record_for(
    sequence: u64,
    active_slot: SlotIndex,
    slot_nodes: [FileNode; 2],
    state: FileState,
    digest: [u8; 32],
    details: &ValidatedDetails,
    scope: &LocalTrashScope,
) -> AppResult<ControlRecord> {
    if sequence == 0 || details.header.revision > MAX_SAFE_JSON_INTEGER {
        return Err(local_error(ERROR_INVALID, "Local Trash state is invalid."));
    }
    Ok(ControlRecord {
        sequence,
        active_slot,
        slot_nodes,
        active_size: state.size,
        revision: details.header.revision,
        workspace_fingerprint: fingerprint_bytes(scope.workspace_fingerprint())
            .map_err(validation_app_error)?,
        source_fingerprint: fingerprint_bytes(scope.source_fingerprint())
            .map_err(validation_app_error)?,
        identity_version_digest: sha256_bytes(LOCAL_TRASH_IDENTITY_VERSION.as_bytes()),
        generation_digest: sha256_bytes(details.header.generation.as_bytes()),
        active_digest: digest,
    })
}

fn control_matches_scope(record: &ControlRecord, scope: &LocalTrashScope) -> AppResult<()> {
    if record.workspace_fingerprint
        != fingerprint_bytes(scope.workspace_fingerprint()).map_err(validation_app_error)?
        || record.source_fingerprint
            != fingerprint_bytes(scope.source_fingerprint()).map_err(validation_app_error)?
        || record.identity_version_digest != sha256_bytes(LOCAL_TRASH_IDENTITY_VERSION.as_bytes())
    {
        return Err(local_error(
            ERROR_FINGERPRINT_MISMATCH,
            "Local Trash belongs to a different workspace or source.",
        ));
    }
    Ok(())
}

fn open_writer(workspace: &Path, scope: &LocalTrashScope) -> AppResult<OpenedWriter> {
    open_writer_inner(workspace, scope, || {}, || {})
}

fn open_writer_inner<F, G>(
    workspace: &Path,
    scope: &LocalTrashScope,
    after_workspace_pin: F,
    after_singleton_publish: G,
) -> AppResult<OpenedWriter>
where
    F: FnOnce(),
    G: FnOnce(),
{
    scope.validate()?;
    let pinned_workspace = pin_workspace(workspace).map_err(validation_app_error)?;
    let process_guard =
        acquire_workspace_write_guard(&pinned_workspace).map_err(validation_app_error)?;
    let canonical_workspace = pinned_workspace.authorized_path.clone();
    after_workspace_pin();
    pinned_workspace
        .validate_directory()
        .map_err(validation_app_error)?;
    require_no_companions_at(&pinned_workspace).map_err(validation_app_error)?;

    let initial_storage = storage_snapshot(&pinned_workspace).map_err(validation_app_error)?;
    let (control_file, _current_record_index, current_record, current_storage) = if initial_storage
        .is_absent()
    {
        let (control_file, control_created) =
            create_private_file_at(&pinned_workspace, local_state_control_name())?;
        acquire_file_lock(&control_file, true)
            .map_err(|_| local_error(ERROR_BUSY, "Local Trash is busy. Try again."))?;
        control_file
            .set_len(CONTROL_FILE_BYTES)
            .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?;
        let (primary_file, primary_created) =
            create_private_file_at(&pinned_workspace, local_state_name())?;
        let (alternate_file, alternate_created) =
            create_private_file_at(&pinned_workspace, local_state_alt_name())?;
        let provisional_storage =
            storage_snapshot(&pinned_workspace).map_err(validation_app_error)?;
        let provisional_token = reset_token(
            &canonical_workspace,
            scope,
            provisional_storage.clone(),
            None,
        );
        let mut primary_conn = open_staged_connection(&primary_file)?;
        initialize_database(&mut primary_conn, scope)?;
        let details =
            validate_connection_for_journal(&primary_conn, scope, &provisional_token, "memory")
                .map_err(validation_app_error)?;
        drop(primary_conn);
        primary_file
            .sync_all()
            .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?;
        alternate_file
            .sync_all()
            .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?;
        control_file
            .sync_all()
            .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?;
        pinned_workspace.directory.sync_all().map_err(|_| {
            local_error(
                ERROR_UNAVAILABLE,
                "Local Trash could not be initialized durably.",
            )
        })?;
        let primary_state = named_private_state(&pinned_workspace, local_state_name())
            .map_err(validation_app_error)?;
        let slot_nodes = [primary_created.node, alternate_created.node];
        if primary_state.node != primary_created.node {
            return Err(local_error(
                ERROR_UNSAFE_FILE,
                "Local Trash changed while it was initialized.",
            ));
        }
        let primary_digest =
            file_digest(&primary_file, primary_state.size).map_err(validation_app_error)?;
        let record = control_record_for(
            1,
            SlotIndex::Primary,
            slot_nodes,
            primary_state,
            primary_digest,
            &details,
            scope,
        )?;
        publish_control_record(&control_file, 0, &record).map_err(|_| {
            local_error(
                ERROR_UNAVAILABLE,
                "Local Trash could not be initialized durably.",
            )
        })?;
        let reconciled = read_control_records(&control_file)
            .ok()
            .and_then(|records| select_control_record(records).ok())
            .is_some_and(|(index, selected)| index == 0 && selected == record);
        if !reconciled {
            return Err(local_error(
                ERROR_UNAVAILABLE,
                "Local Trash could not be initialized durably.",
            ));
        }
        let held_control = private_file_state(
            &control_file
                .metadata()
                .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?,
        )
        .map_err(validation_app_error)?;
        let named_control = named_private_state(&pinned_workspace, local_state_control_name())
            .map_err(validation_app_error)?;
        if held_control.node != control_created.node
            || held_control.size != CONTROL_FILE_BYTES
            || named_control != held_control
        {
            return Err(local_error(
                ERROR_UNSAFE_FILE,
                "Local Trash control changed during initialization.",
            ));
        }
        let storage = storage_snapshot(&pinned_workspace).map_err(validation_app_error)?;
        (control_file, 0, record, storage)
    } else {
        if initial_storage.control.is_none() || initial_storage.slots.iter().any(Option::is_none) {
            return Err(local_error(ERROR_INVALID, "Local Trash state is invalid."));
        }
        let (control_file, control_state) =
            open_named_private_file(&pinned_workspace, local_state_control_name(), true)
                .map_err(validation_app_error)?;
        if control_state != initial_storage.control.expect("checked control")
            || control_state.size != CONTROL_FILE_BYTES
        {
            return Err(local_error(ERROR_INVALID, "Local Trash state is invalid."));
        }
        acquire_file_lock(&control_file, true)
            .map_err(|_| local_error(ERROR_BUSY, "Local Trash is busy. Try again."))?;
        let locked_storage = storage_snapshot(&pinned_workspace).map_err(validation_app_error)?;
        if locked_storage != initial_storage {
            return Err(local_error(
                ERROR_BUSY,
                "Local Trash changed while it was opened.",
            ));
        }
        let (index, record) = select_control_record(
            read_control_records(&control_file).map_err(validation_app_error)?,
        )
        .map_err(validation_app_error)?;
        (control_file, index, record, locked_storage)
    };

    control_matches_scope(&current_record, scope)?;
    for slot in [SlotIndex::Primary, SlotIndex::Alternate] {
        if current_storage.slots[slot.as_usize()].map(|state| state.node)
            != Some(current_record.slot_nodes[slot.as_usize()])
        {
            return Err(local_error(
                ERROR_UNSAFE_FILE,
                "Local Trash storage identity changed.",
            ));
        }
    }
    let active_slot = current_record.active_slot;
    let staged_slot = active_slot.other();
    let (active_file, active_state) =
        open_named_private_file(&pinned_workspace, slot_name(active_slot), true)
            .map_err(validation_app_error)?;
    let active_digest =
        file_digest(&active_file, active_state.size).map_err(validation_app_error)?;
    if active_state.node != current_record.slot_nodes[active_slot.as_usize()]
        || active_state.size != current_record.active_size
        || active_digest != current_record.active_digest
        || named_private_state(&pinned_workspace, slot_name(active_slot))
            .map_err(validation_app_error)?
            != active_state
    {
        return Err(local_error(ERROR_UNSAFE_FILE, "Local Trash state changed."));
    }
    let writer_token = reset_token(
        &canonical_workspace,
        scope,
        current_storage.clone(),
        Some(current_record.clone()),
    );
    let source_lock = Connection::open_with_flags(
        sqlite_file_descriptor_path(&active_file),
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_EXRESCODE,
    )
    .map_err(sqlite_operation_error)?;
    source_lock
        .busy_timeout(SQLITE_BUSY_TIMEOUT)
        .map_err(sqlite_operation_error)?;
    sqlite_security::configure_connection(&source_lock, false).map_err(sqlite_operation_error)?;
    source_lock
        .execute_batch("BEGIN IMMEDIATE;")
        .map_err(sqlite_operation_error)?;
    let active_details =
        validate_connection(&source_lock, scope, &writer_token).map_err(validation_app_error)?;
    if active_details.header.revision != current_record.revision
        || sha256_bytes(active_details.header.generation.as_bytes())
            != current_record.generation_digest
    {
        return Err(local_error(ERROR_INVALID, "Local Trash state is invalid."));
    }
    let (staged_file, staged_state) =
        open_named_private_file(&pinned_workspace, slot_name(staged_slot), true)
            .map_err(validation_app_error)?;
    if staged_state.node != current_record.slot_nodes[staged_slot.as_usize()]
        || named_private_state(&pinned_workspace, slot_name(staged_slot))
            .map_err(validation_app_error)?
            != staged_state
    {
        return Err(local_error(
            ERROR_UNSAFE_FILE,
            "Local Trash staging identity changed.",
        ));
    }
    copy_file_to_slot(&active_file, active_state.size, &staged_file)?;
    let conn = open_staged_connection(&staged_file)?;
    validate_connection_for_journal(&conn, scope, &writer_token, "memory")
        .map_err(validation_app_error)?;
    let staged_after = private_file_state(
        &staged_file
            .metadata()
            .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?,
    )
    .map_err(validation_app_error)?;
    if staged_after.node != staged_state.node
        || named_private_state(&pinned_workspace, slot_name(staged_slot))
            .map_err(validation_app_error)?
            != staged_after
    {
        return Err(local_error(
            ERROR_UNSAFE_FILE,
            "Local Trash staging file changed while it was opened.",
        ));
    }
    let writer = OpenedWriter {
        conn: Some(conn),
        pinned_workspace,
        control_file,
        staged_file,
        staged_slot,
        staged_node: staged_state.node,
        active_file,
        active_state,
        active_digest,
        current_record,
        reset_token: writer_token,
        source_lock: Some(source_lock),
        _process_guard: process_guard,
    };
    if writer.current_record.sequence == 1 {
        let published = finish_writer_inner(writer, || {})?;
        after_singleton_publish();
        return restage_published_writer(published, scope);
    }
    Ok(writer)
}

fn finish_writer(writer: OpenedWriter) -> AppResult<()> {
    finish_writer_inner(writer, || {}).map(drop)
}

fn finish_writer_inner<F>(mut writer: OpenedWriter, after_publish: F) -> AppResult<PublishedWriter>
where
    F: FnOnce(),
{
    drop(writer.conn.take());
    writer
        .pinned_workspace
        .validate_directory()
        .map_err(validation_app_error)?;
    require_no_companions_at(&writer.pinned_workspace).map_err(validation_app_error)?;
    writer
        .staged_file
        .sync_all()
        .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?;
    let staged_named = named_private_state(&writer.pinned_workspace, slot_name(writer.staged_slot))
        .map_err(validation_app_error)?;
    let staged_held = private_file_state(
        &writer
            .staged_file
            .metadata()
            .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?,
    )
    .map_err(validation_app_error)?;
    if staged_named.node != writer.staged_node || staged_held != staged_named {
        return Err(local_error(
            ERROR_UNSAFE_FILE,
            "Local Trash staging file changed while it was saved.",
        ));
    }

    let active_held = private_file_state(
        &writer
            .active_file
            .metadata()
            .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?,
    )
    .map_err(validation_app_error)?;
    let active_named = named_private_state(
        &writer.pinned_workspace,
        slot_name(writer.current_record.active_slot),
    )
    .map_err(validation_app_error)?;
    if active_held != writer.active_state
        || active_named != writer.active_state
        || file_digest(&writer.active_file, active_held.size).map_err(validation_app_error)?
            != writer.active_digest
        || writer.source_lock.is_none()
    {
        return Err(local_error(
            ERROR_BUSY,
            "Local Trash changed before it could be saved.",
        ));
    }
    let staged_token = reset_token(
        &writer.reset_token.canonical_workspace,
        &writer.reset_token.scope,
        storage_snapshot(&writer.pinned_workspace).map_err(validation_app_error)?,
        Some(writer.current_record.clone()),
    );
    let staged_reader = Connection::open_with_flags(
        sqlite_file_descriptor_path(&writer.staged_file),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_EXRESCODE,
    )
    .map_err(sqlite_operation_error)?;
    sqlite_security::configure_connection(&staged_reader, true).map_err(sqlite_operation_error)?;
    let details = validate_connection(&staged_reader, &writer.reset_token.scope, &staged_token)
        .map_err(validation_app_error)?;
    drop(staged_reader);
    let staged_digest =
        file_digest(&writer.staged_file, staged_held.size).map_err(validation_app_error)?;
    let next_sequence = writer
        .current_record
        .sequence
        .checked_add(1)
        .ok_or_else(|| local_error(ERROR_INVALID, "Local Trash control sequence is exhausted."))?;
    let next_record = control_record_for(
        next_sequence,
        writer.staged_slot,
        writer.current_record.slot_nodes,
        staged_held,
        staged_digest,
        &details,
        &writer.reset_token.scope,
    )?;
    let next_index = writer.staged_slot.as_usize();
    let expected_control = writer
        .reset_token
        .storage
        .control
        .ok_or_else(|| local_error(ERROR_INVALID, "Local Trash control is invalid."))?;
    let control_held_before = private_file_state(
        &writer
            .control_file
            .metadata()
            .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?,
    )
    .map_err(validation_app_error)?;
    let control_named_before =
        named_private_state(&writer.pinned_workspace, local_state_control_name())
            .map_err(validation_app_error)?;
    if control_held_before != expected_control || control_named_before != expected_control {
        return Err(local_error(
            ERROR_UNSAFE_FILE,
            "Local Trash control changed before publication.",
        ));
    }
    let staged_held_before_publish = private_file_state(
        &writer
            .staged_file
            .metadata()
            .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?,
    )
    .map_err(validation_app_error)?;
    let staged_named_before_publish =
        named_private_state(&writer.pinned_workspace, slot_name(writer.staged_slot))
            .map_err(validation_app_error)?;
    let active_held_before_publish = private_file_state(
        &writer
            .active_file
            .metadata()
            .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?,
    )
    .map_err(validation_app_error)?;
    let active_named_before_publish = named_private_state(
        &writer.pinned_workspace,
        slot_name(writer.current_record.active_slot),
    )
    .map_err(validation_app_error)?;
    let selected_before_publish = select_control_record(
        read_control_records(&writer.control_file).map_err(validation_app_error)?,
    )
    .map_err(validation_app_error)?;
    if staged_held_before_publish != staged_held
        || staged_named_before_publish != staged_held
        || file_digest(&writer.staged_file, staged_held.size).map_err(validation_app_error)?
            != next_record.active_digest
        || active_held_before_publish != writer.active_state
        || active_named_before_publish != writer.active_state
        || file_digest(&writer.active_file, writer.active_state.size)
            .map_err(validation_app_error)?
            != writer.active_digest
        || selected_before_publish.0 != writer.current_record.active_slot.as_usize()
        || selected_before_publish.1 != writer.current_record
    {
        return Err(local_error(
            ERROR_UNSAFE_FILE,
            "Local Trash changed before publication.",
        ));
    }
    if publish_control_record(&writer.control_file, next_index, &next_record).is_err() {
        let prior_selector_remains = read_control_records(&writer.control_file)
            .ok()
            .and_then(|records| select_control_record(records).ok())
            .is_some_and(|(index, selected)| {
                index == writer.current_record.active_slot.as_usize()
                    && selected == writer.current_record
            });
        return Err(if prior_selector_remains {
            local_error(
                ERROR_UNAVAILABLE,
                "Local Trash save could not be published durably.",
            )
        } else {
            local_error(
                ERROR_OPERATION_INDETERMINATE,
                "Local Trash operation requires reinspection.",
            )
        });
    }
    after_publish();
    reconcile_control_publication(
        &writer.pinned_workspace,
        &writer.control_file,
        expected_control.node,
        PublicationSlot {
            file: &writer.staged_file,
            state: staged_held,
        },
        PublicationSlot {
            file: &writer.active_file,
            state: writer.active_state,
        },
        next_index,
        &next_record,
    )
    .map_err(|_| {
        local_error(
            ERROR_OPERATION_INDETERMINATE,
            "Local Trash operation requires reinspection.",
        )
    })?;
    Ok(PublishedWriter {
        writer,
        record: next_record,
        state: staged_held,
        digest: staged_digest,
    })
}

fn restage_published_writer(
    published: PublishedWriter,
    scope: &LocalTrashScope,
) -> AppResult<OpenedWriter> {
    let PublishedWriter {
        mut writer,
        record,
        state,
        digest,
    } = published;
    drop(writer.source_lock.take());
    writer
        .pinned_workspace
        .validate_directory()
        .map_err(validation_app_error)?;
    control_matches_scope(&record, scope)?;
    let expected_control_node = writer
        .reset_token
        .storage
        .control
        .ok_or_else(|| local_error(ERROR_INVALID, "Local Trash control is invalid."))?
        .node;
    reconcile_control_publication(
        &writer.pinned_workspace,
        &writer.control_file,
        expected_control_node,
        PublicationSlot {
            file: &writer.staged_file,
            state,
        },
        PublicationSlot {
            file: &writer.active_file,
            state: writer.active_state,
        },
        record.active_slot.as_usize(),
        &record,
    )
    .map_err(validation_app_error)?;
    let current_storage =
        storage_snapshot(&writer.pinned_workspace).map_err(validation_app_error)?;
    let writer_token = reset_token(
        &writer.reset_token.canonical_workspace,
        scope,
        current_storage,
        Some(record.clone()),
    );

    std::mem::swap(&mut writer.active_file, &mut writer.staged_file);
    writer.active_state = state;
    writer.active_digest = digest;
    writer.current_record = record;
    writer.staged_slot = writer.current_record.active_slot.other();
    writer.staged_node = writer.current_record.slot_nodes[writer.staged_slot.as_usize()];
    writer.reset_token = writer_token;

    let active_held = private_file_state(
        &writer
            .active_file
            .metadata()
            .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?,
    )
    .map_err(validation_app_error)?;
    let active_named = named_private_state(
        &writer.pinned_workspace,
        slot_name(writer.current_record.active_slot),
    )
    .map_err(validation_app_error)?;
    if active_held != writer.active_state
        || active_named != writer.active_state
        || file_digest(&writer.active_file, active_held.size).map_err(validation_app_error)?
            != writer.active_digest
    {
        return Err(local_error(ERROR_UNSAFE_FILE, "Local Trash state changed."));
    }
    let source_lock = Connection::open_with_flags(
        sqlite_file_descriptor_path(&writer.active_file),
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_EXRESCODE,
    )
    .map_err(sqlite_operation_error)?;
    source_lock
        .busy_timeout(SQLITE_BUSY_TIMEOUT)
        .map_err(sqlite_operation_error)?;
    sqlite_security::configure_connection(&source_lock, false).map_err(sqlite_operation_error)?;
    source_lock
        .execute_batch("BEGIN IMMEDIATE;")
        .map_err(sqlite_operation_error)?;
    let active_details = validate_connection(&source_lock, scope, &writer.reset_token)
        .map_err(validation_app_error)?;
    if active_details.header.revision != writer.current_record.revision
        || sha256_bytes(active_details.header.generation.as_bytes())
            != writer.current_record.generation_digest
    {
        return Err(local_error(ERROR_INVALID, "Local Trash state is invalid."));
    }

    let staged_state = private_file_state(
        &writer
            .staged_file
            .metadata()
            .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?,
    )
    .map_err(validation_app_error)?;
    if staged_state.node != writer.staged_node
        || named_private_state(&writer.pinned_workspace, slot_name(writer.staged_slot))
            .map_err(validation_app_error)?
            != staged_state
    {
        return Err(local_error(
            ERROR_UNSAFE_FILE,
            "Local Trash staging identity changed.",
        ));
    }
    copy_file_to_slot(
        &writer.active_file,
        writer.active_state.size,
        &writer.staged_file,
    )?;
    let conn = open_staged_connection(&writer.staged_file)?;
    validate_connection_for_journal(&conn, scope, &writer.reset_token, "memory")
        .map_err(validation_app_error)?;
    let staged_after = private_file_state(
        &writer
            .staged_file
            .metadata()
            .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?,
    )
    .map_err(validation_app_error)?;
    if staged_after.node != writer.staged_node
        || named_private_state(&writer.pinned_workspace, slot_name(writer.staged_slot))
            .map_err(validation_app_error)?
            != staged_after
    {
        return Err(local_error(
            ERROR_UNSAFE_FILE,
            "Local Trash staging file changed while it was opened.",
        ));
    }
    writer.conn = Some(conn);
    writer.source_lock = Some(source_lock);
    Ok(writer)
}

fn validate_mutation_input(operation_id: &str, identities: &[String]) -> AppResult<()> {
    if !is_opaque_id(operation_id)
        || identities.is_empty()
        || identities.len() > MAX_LOCAL_TRASH_BATCH
    {
        return Err(local_error(
            ERROR_INVALID_INPUT,
            "Local Trash operation input is invalid.",
        ));
    }
    let mut seen = HashSet::with_capacity(identities.len());
    let mut total_bytes = 0_usize;
    for identity in identities {
        total_bytes = total_bytes.checked_add(identity.len()).ok_or_else(|| {
            local_error(
                ERROR_INVALID_INPUT,
                "Local Trash operation input is invalid.",
            )
        })?;
        if identity.len() > MAX_IDENTITY_BYTES
            || total_bytes > MAX_MUTATION_IDENTITY_BYTES
            || !crate::import_integrity_phase2::validate_relative_identity(identity)
            || !seen.insert(identity.as_str())
        {
            return Err(local_error(
                ERROR_INVALID_INPUT,
                "Local Trash operation input is invalid.",
            ));
        }
    }
    Ok(())
}

fn transaction_header(conn: &Connection, scope: &LocalTrashScope) -> AppResult<ValidatedHeader> {
    let (
        workspace_fingerprint,
        source_fingerprint,
        identity_version,
        generation,
        revision,
        last_changed_at_ms,
    ) = conn
        .query_row(
            "SELECT workspace_fingerprint, source_fingerprint,
                    canonical_identity_version, generation, revision,
                    last_changed_at_ms
             FROM local_state_metadata
             WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                ))
            },
        )
        .map_err(sqlite_operation_error)?;
    if workspace_fingerprint != scope.workspace_fingerprint
        || source_fingerprint != scope.source_fingerprint
        || identity_version != LOCAL_TRASH_IDENTITY_VERSION
        || !is_opaque_id(&generation)
        || !(0..=MAX_SAFE_JSON_INTEGER_I64).contains(&revision)
        || last_changed_at_ms.is_some_and(|value| !is_valid_timestamp(value))
    {
        return Err(local_error(ERROR_INVALID, "Local Trash state is invalid."));
    }
    Ok(ValidatedHeader {
        generation,
        revision: u64::try_from(revision)
            .map_err(|_| local_error(ERROR_INVALID, "Local Trash state is invalid."))?,
        last_changed_at_ms,
    })
}

fn require_expected_revision(actual: u64, expected: u64) -> AppResult<()> {
    if actual != expected || expected > MAX_SAFE_JSON_INTEGER {
        return Err(local_error(
            ERROR_STALE_REVISION,
            "Local Trash changed. Refresh and try again.",
        ));
    }
    Ok(())
}

fn next_revision(current: u64) -> AppResult<(u64, i64)> {
    let revision = current
        .checked_add(1)
        .ok_or_else(|| local_error(ERROR_UNAVAILABLE, "Local Trash revision is unavailable."))?;
    if revision > MAX_SAFE_JSON_INTEGER {
        return Err(local_error(
            ERROR_UNAVAILABLE,
            "Local Trash revision is unavailable.",
        ));
    }
    let sqlite_revision = i64::try_from(revision)
        .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash revision is unavailable."))?;
    Ok((revision, sqlite_revision))
}

fn changed_at(header: &ValidatedHeader) -> AppResult<i64> {
    Ok(now_utc_ms()?.max(header.last_changed_at_ms.unwrap_or(MIN_TIMESTAMP_MS)))
}

fn update_metadata_revision(
    conn: &Connection,
    scope: &LocalTrashScope,
    old_revision: u64,
    new_revision: i64,
    changed_at_ms: i64,
) -> AppResult<()> {
    let old_revision = i64::try_from(old_revision)
        .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash revision is unavailable."))?;
    let changed = conn
        .execute(
            "UPDATE local_state_metadata
             SET revision = ?1, last_changed_at_ms = ?2
             WHERE singleton = 1
               AND workspace_fingerprint = ?3
               AND source_fingerprint = ?4
               AND canonical_identity_version = ?5
               AND revision = ?6",
            params![
                new_revision,
                changed_at_ms,
                scope.workspace_fingerprint(),
                scope.source_fingerprint(),
                LOCAL_TRASH_IDENTITY_VERSION,
                old_revision,
            ],
        )
        .map_err(sqlite_operation_error)?;
    if changed != 1 {
        return Err(local_error(
            ERROR_STALE_REVISION,
            "Local Trash changed. Refresh and try again.",
        ));
    }
    Ok(())
}

fn stored_count(conn: &Connection) -> AppResult<usize> {
    let count = conn
        .query_row("SELECT COUNT(*) FROM trashed_messages", [], |row| {
            row.get::<_, i64>(0)
        })
        .map_err(sqlite_operation_error)?;
    let count = usize::try_from(count)
        .map_err(|_| local_error(ERROR_INVALID, "Local Trash state is invalid."))?;
    if count > MAX_LOCAL_TRASH_ROWS {
        return Err(local_error(ERROR_INVALID, "Local Trash state is invalid."));
    }
    Ok(count)
}

fn build_mutation_result(
    operation_id: &str,
    requested_count: usize,
    changed_count: usize,
    already_trashed_count: usize,
    already_active_count: usize,
    header: ValidatedHeader,
    stored_count: usize,
) -> LocalTrashMutationResult {
    LocalTrashMutationResult {
        operation_id: operation_id.to_string(),
        requested_count,
        changed_count,
        already_trashed_count,
        already_active_count,
        generation: header.generation,
        revision: header.revision,
        stored_count,
        last_changed_at_ms: header.last_changed_at_ms,
    }
}

pub(crate) fn move_messages(
    workspace: &Path,
    scope: &LocalTrashScope,
    expected_revision: u64,
    operation_id: &str,
    identities: &[String],
) -> AppResult<LocalTrashMutationResult> {
    move_messages_inner(
        workspace,
        scope,
        expected_revision,
        operation_id,
        identities,
        || Ok(()),
    )
}

fn move_messages_inner<F>(
    workspace: &Path,
    scope: &LocalTrashScope,
    expected_revision: u64,
    operation_id: &str,
    identities: &[String],
    before_commit: F,
) -> AppResult<LocalTrashMutationResult>
where
    F: FnOnce() -> AppResult<()>,
{
    validate_mutation_input(operation_id, identities)?;
    let mut writer = open_writer(workspace, scope)?;
    let transaction = writer
        .connection_mut()?
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite_operation_error)?;
    let mut header = transaction_header(&transaction, scope)?;
    require_expected_revision(header.revision, expected_revision)?;
    let mut changed_identities = Vec::with_capacity(identities.len());
    let mut already_trashed_count = 0_usize;
    {
        let mut statement = transaction
            .prepare(
                "SELECT operation_id
                 FROM trashed_messages
                 WHERE workspace_fingerprint = ?1
                   AND source_fingerprint = ?2
                   AND canonical_identity_version = ?3
                   AND canonical_identity = ?4",
            )
            .map_err(sqlite_operation_error)?;
        for identity in identities {
            let existing = statement
                .query_row(
                    params![
                        scope.workspace_fingerprint(),
                        scope.source_fingerprint(),
                        LOCAL_TRASH_IDENTITY_VERSION,
                        identity,
                    ],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(sqlite_operation_error)?;
            if existing.is_some() {
                already_trashed_count = already_trashed_count.saturating_add(1);
            } else {
                changed_identities.push(identity);
            }
        }
    }
    if !changed_identities.is_empty() {
        let operation_id_in_use = transaction
            .query_row(
                "SELECT EXISTS(
                     SELECT 1 FROM trashed_messages WHERE operation_id = ?1 LIMIT 1
                 )",
                [operation_id],
                |row| row.get::<_, i64>(0),
            )
            .map_err(sqlite_operation_error)?;
        if operation_id_in_use != 0 {
            return Err(local_error(
                ERROR_INVALID_INPUT,
                "Local Trash operation identity is already in use.",
            ));
        }
        let (revision, sqlite_revision) = next_revision(header.revision)?;
        let changed_at_ms = changed_at(&header)?;
        {
            let mut insert = transaction
                .prepare(
                    "INSERT INTO trashed_messages (
                         workspace_fingerprint, source_fingerprint,
                         canonical_identity_version, canonical_identity,
                         state_revision, trashed_at_ms, last_changed_at_ms,
                         operation_id
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7)",
                )
                .map_err(sqlite_operation_error)?;
            for identity in &changed_identities {
                insert
                    .execute(params![
                        scope.workspace_fingerprint(),
                        scope.source_fingerprint(),
                        LOCAL_TRASH_IDENTITY_VERSION,
                        identity,
                        sqlite_revision,
                        changed_at_ms,
                        operation_id,
                    ])
                    .map_err(sqlite_operation_error)?;
            }
        }
        update_metadata_revision(
            &transaction,
            scope,
            header.revision,
            sqlite_revision,
            changed_at_ms,
        )?;
        header.revision = revision;
        header.last_changed_at_ms = Some(changed_at_ms);
    }
    let count = stored_count(&transaction)?;
    before_commit()?;
    transaction.commit().map_err(sqlite_operation_error)?;
    let result = build_mutation_result(
        operation_id,
        identities.len(),
        changed_identities.len(),
        already_trashed_count,
        0,
        header,
        count,
    );
    finish_writer(writer)?;
    Ok(result)
}

pub(crate) fn restore_messages(
    workspace: &Path,
    scope: &LocalTrashScope,
    expected_revision: u64,
    operation_id: &str,
    identities: &[String],
) -> AppResult<LocalTrashMutationResult> {
    validate_mutation_input(operation_id, identities)?;
    let mut writer = open_writer(workspace, scope)?;
    let transaction = writer
        .connection_mut()?
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite_operation_error)?;
    let mut header = transaction_header(&transaction, scope)?;
    require_expected_revision(header.revision, expected_revision)?;
    let mut changed_count = 0_usize;
    {
        let mut delete = transaction
            .prepare(
                "DELETE FROM trashed_messages
                 WHERE workspace_fingerprint = ?1
                   AND source_fingerprint = ?2
                   AND canonical_identity_version = ?3
                   AND canonical_identity = ?4",
            )
            .map_err(sqlite_operation_error)?;
        for identity in identities {
            changed_count = changed_count.saturating_add(
                delete
                    .execute(params![
                        scope.workspace_fingerprint(),
                        scope.source_fingerprint(),
                        LOCAL_TRASH_IDENTITY_VERSION,
                        identity,
                    ])
                    .map_err(sqlite_operation_error)?,
            );
        }
    }
    if changed_count > 0 {
        let (revision, sqlite_revision) = next_revision(header.revision)?;
        let changed_at_ms = changed_at(&header)?;
        update_metadata_revision(
            &transaction,
            scope,
            header.revision,
            sqlite_revision,
            changed_at_ms,
        )?;
        header.revision = revision;
        header.last_changed_at_ms = Some(changed_at_ms);
    }
    let count = stored_count(&transaction)?;
    transaction.commit().map_err(sqlite_operation_error)?;
    let result = build_mutation_result(
        operation_id,
        identities.len(),
        changed_count,
        0,
        identities.len().saturating_sub(changed_count),
        header,
        count,
    );
    finish_writer(writer)?;
    Ok(result)
}

pub(crate) fn undo_move(
    workspace: &Path,
    scope: &LocalTrashScope,
    expected_revision: u64,
    operation_id: &str,
) -> AppResult<LocalTrashMutationResult> {
    if !is_opaque_id(operation_id) {
        return Err(local_error(
            ERROR_INVALID_INPUT,
            "Local Trash operation input is invalid.",
        ));
    }
    let mut writer = open_writer(workspace, scope)?;
    let transaction = writer
        .connection_mut()?
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(sqlite_operation_error)?;
    let mut header = transaction_header(&transaction, scope)?;
    require_expected_revision(header.revision, expected_revision)?;
    let changed_count = transaction
        .execute(
            "DELETE FROM trashed_messages
             WHERE workspace_fingerprint = ?1
               AND source_fingerprint = ?2
               AND canonical_identity_version = ?3
               AND operation_id = ?4",
            params![
                scope.workspace_fingerprint(),
                scope.source_fingerprint(),
                LOCAL_TRASH_IDENTITY_VERSION,
                operation_id,
            ],
        )
        .map_err(sqlite_operation_error)?;
    if changed_count > MAX_LOCAL_TRASH_BATCH {
        return Err(local_error(ERROR_INVALID, "Local Trash state is invalid."));
    }
    if changed_count == 0 {
        return Err(local_error(
            ERROR_INVALID,
            "Local Trash Undo state no longer matches the stored operation.",
        ));
    }
    let (revision, sqlite_revision) = next_revision(header.revision)?;
    let changed_at_ms = changed_at(&header)?;
    update_metadata_revision(
        &transaction,
        scope,
        header.revision,
        sqlite_revision,
        changed_at_ms,
    )?;
    header.revision = revision;
    header.last_changed_at_ms = Some(changed_at_ms);
    let count = stored_count(&transaction)?;
    transaction.commit().map_err(sqlite_operation_error)?;
    let result = build_mutation_result(
        operation_id,
        changed_count,
        changed_count,
        0,
        0,
        header,
        count,
    );
    finish_writer(writer)?;
    Ok(result)
}

struct QueryOnlyRestore<'a> {
    conn: &'a Connection,
    armed: bool,
}

impl Drop for QueryOnlyRestore<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.conn.pragma_update(None, "query_only", true);
        }
    }
}

fn require_canonical_query_connection(conn: &Connection) -> AppResult<()> {
    sqlite_security::configure_connection(conn, true).map_err(sqlite_operation_error)?;
    if !conn
        .is_readonly(DatabaseName::Main)
        .map_err(sqlite_operation_error)?
    {
        return Err(local_error(
            ERROR_UNSAFE_FILE,
            "Local Trash query access requires a read-only workspace index.",
        ));
    }
    let query_only = conn
        .query_row("PRAGMA query_only", [], |row| row.get::<_, i64>(0))
        .map_err(sqlite_operation_error)?;
    if query_only != 1 {
        return Err(local_error(
            ERROR_UNSAFE_FILE,
            "Local Trash query access is unavailable.",
        ));
    }
    let mut statement = conn
        .prepare("PRAGMA database_list")
        .map_err(sqlite_operation_error)?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sqlite_operation_error)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(sqlite_operation_error)?;
    if names
        .iter()
        .any(|name| !matches!(name.as_str(), "main" | "temp"))
    {
        return Err(local_error(
            ERROR_UNSAFE_FILE,
            "Local Trash query access refused an attached database.",
        ));
    }
    Ok(())
}

fn with_temp_writes<T, F>(conn: &Connection, operation: F) -> AppResult<T>
where
    F: FnOnce() -> AppResult<T>,
{
    require_canonical_query_connection(conn)?;
    if let Err(error) = conn.pragma_update(None, "query_only", false) {
        let _ = conn.pragma_update(None, "query_only", true);
        return Err(sqlite_operation_error(error));
    }
    let mut restore = QueryOnlyRestore { conn, armed: true };
    let operation_result = operation();
    let restoration_result = conn
        .pragma_update(None, "query_only", true)
        .map_err(sqlite_operation_error);
    if restoration_result.is_ok() {
        restore.armed = false;
    }
    restoration_result?;
    let query_only = conn
        .query_row("PRAGMA query_only", [], |row| row.get::<_, i64>(0))
        .map_err(sqlite_operation_error)?;
    if query_only != 1 {
        return Err(local_error(
            ERROR_UNSAFE_FILE,
            "Local Trash query protection could not be restored.",
        ));
    }
    operation_result
}

fn failed_query_state(
    mut failure: ValidationFailure,
    token: Option<&ResetToken>,
) -> LocalTrashQueryState {
    if failure.reset_token.is_none() {
        failure.reset_token = token.cloned().map(Box::new);
    }
    LocalTrashQueryState::from(&LocalTrashInspection::failed(failure))
}

pub(crate) fn canonical_identity_ambiguous_query_state(
    sidecar_present: bool,
) -> LocalTrashQueryState {
    LocalTrashQueryState {
        health: LocalTrashHealth::Unavailable,
        available: false,
        can_reset: false,
        sidecar_present,
        schema_version: None,
        generation: None,
        revision: 0,
        stored_count: 0,
        last_changed_at_ms: None,
        fingerprint_match: None,
        last_error_code: Some(ERROR_CANONICAL_IDENTITY_AMBIGUOUS),
    }
}

pub(crate) fn materialize_for_query(
    canonical_conn: &Connection,
    workspace: &Path,
    scope: &LocalTrashScope,
) -> AppResult<LocalTrashQueryState> {
    materialize_for_query_inner(canonical_conn, workspace, scope, None, None)
}

pub(crate) fn materialize_for_search(
    canonical_conn: &Connection,
    workspace: &Path,
    scope: &LocalTrashScope,
    operation: &SearchOperationGuard,
) -> AppResult<LocalTrashQueryState> {
    materialize_for_query_inner(canonical_conn, workspace, scope, Some(operation), None)
}

fn check_materialization_cancelled(operation: Option<&SearchOperationGuard>) -> AppResult<()> {
    operation
        .map(SearchOperationGuard::check_cancelled)
        .transpose()?;
    Ok(())
}

fn materialize_for_query_inner(
    canonical_conn: &Connection,
    workspace: &Path,
    scope: &LocalTrashScope,
    operation: Option<&SearchOperationGuard>,
    after_reader_registered: Option<&dyn Fn()>,
) -> AppResult<LocalTrashQueryState> {
    scope.validate()?;
    check_materialization_cancelled(operation)?;
    with_temp_writes(canonical_conn, || {
        check_materialization_cancelled(operation)?;
        canonical_conn
            .execute_batch(CREATE_TEMP_TABLE_SQL)
            .map_err(sqlite_operation_error)?;
        check_materialization_cancelled(operation)?;
        if !super::has_unique_message_source_identity(canonical_conn).unwrap_or(false) {
            let sidecar_present =
                !matches!(open_reader(workspace, scope), Ok(ReaderOpen::Absent(_)));
            check_materialization_cancelled(operation)?;
            return Ok(canonical_identity_ambiguous_query_state(sidecar_present));
        }
        check_materialization_cancelled(operation)?;
        let reader = match open_reader(workspace, scope) {
            Ok(ReaderOpen::Absent(canonical_workspace)) => {
                check_materialization_cancelled(operation)?;
                let inspection = LocalTrashInspection::absent(canonical_workspace, scope);
                return Ok(LocalTrashQueryState::from(&inspection));
            }
            Ok(ReaderOpen::Opened(reader)) => reader,
            Err(failure) => {
                check_materialization_cancelled(operation)?;
                return Ok(failed_query_state(failure, None));
            }
        };
        if let Some(operation) = operation {
            operation.register_connection(&reader.conn)?;
            if let Some(after_reader_registered) = after_reader_registered {
                after_reader_registered();
            }
            operation.check_cancelled()?;
        }
        let header = match validate_header(&reader.conn, scope, &reader.reset_token) {
            Ok(header) => header,
            Err(failure) => {
                check_materialization_cancelled(operation)?;
                return Ok(failed_query_state(failure, Some(&reader.reset_token)));
            }
        };
        check_materialization_cancelled(operation)?;
        let transaction = canonical_conn
            .unchecked_transaction()
            .map_err(sqlite_operation_error)?;
        let stream_result = {
            let mut insert = transaction
                .prepare(
                    "INSERT INTO temp.local_trash_identities (
                         workspace_fingerprint, source_fingerprint,
                         canonical_identity_version, canonical_identity
                     ) VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(sqlite_operation_error)?;
            stream_valid_identities(
                &reader.conn,
                scope,
                &reader.reset_token,
                &header,
                |identity| {
                    insert
                        .execute(params![
                            scope.workspace_fingerprint(),
                            scope.source_fingerprint(),
                            LOCAL_TRASH_IDENTITY_VERSION,
                            identity,
                        ])
                        .map(|_| ())
                },
            )
        };
        let count = match stream_result {
            Ok(count) => count,
            Err(StreamError::Invalid(failure)) => {
                drop(transaction);
                check_materialization_cancelled(operation)?;
                return Ok(failed_query_state(failure, Some(&reader.reset_token)));
            }
            Err(StreamError::Destination(error)) => {
                drop(transaction);
                return Err(sqlite_operation_error(error));
            }
        };
        let oversized_operation = match reader.conn.query_row(
            "SELECT EXISTS(
                     SELECT 1
                     FROM trashed_messages
                     GROUP BY operation_id
                     HAVING COUNT(*) > ?1
                     LIMIT 1
                 )",
            [i64::try_from(MAX_LOCAL_TRASH_BATCH).unwrap_or(i64::MAX)],
            |row| row.get::<_, i64>(0),
        ) {
            Ok(value) => value,
            Err(error) => {
                drop(transaction);
                if is_sqlite_interrupt(&error) {
                    return Err(AppError::from(error));
                }
                check_materialization_cancelled(operation)?;
                return Ok(failed_query_state(
                    ValidationFailure::invalid(ERROR_INVALID, Some(reader.reset_token.clone())),
                    Some(&reader.reset_token),
                ));
            }
        };
        if oversized_operation != 0 {
            drop(transaction);
            return Ok(failed_query_state(
                ValidationFailure::invalid(ERROR_INVALID, Some(reader.reset_token.clone())),
                Some(&reader.reset_token),
            ));
        }
        if let Err(failure) = validate_reader_unchanged(&reader) {
            drop(transaction);
            check_materialization_cancelled(operation)?;
            return Ok(failed_query_state(failure, Some(&reader.reset_token)));
        }
        check_materialization_cancelled(operation)?;
        transaction.commit().map_err(sqlite_operation_error)?;
        check_materialization_cancelled(operation)?;
        let inspection = LocalTrashInspection::healthy(
            reader.reset_token.clone(),
            ValidatedDetails {
                header,
                stored_count: count,
            },
        );
        Ok(LocalTrashQueryState::from(&inspection))
    })
}

fn validate_reset_binding(
    pinned_workspace: &PinnedWorkspace,
    control_file: &File,
    slot_files: &[Option<File>; 2],
    expected_storage: &StorageSnapshot,
    expected_control_digest: [u8; 32],
    expected_record: Option<&ControlRecord>,
) -> AppResult<()> {
    pinned_workspace
        .validate_directory()
        .map_err(validation_app_error)?;
    require_no_companions_at(pinned_workspace).map_err(validation_app_error)?;
    if storage_snapshot(pinned_workspace).map_err(validation_app_error)? != *expected_storage {
        return Err(local_error(
            ERROR_RESET_REFUSED,
            "Local Trash state changed before reset.",
        ));
    }
    let expected_control = expected_storage.control.ok_or_else(|| {
        local_error(
            ERROR_RESET_REFUSED,
            "Local Trash control is unavailable for reset.",
        )
    })?;
    let held_control = private_file_state(
        &control_file
            .metadata()
            .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?,
    )
    .map_err(validation_app_error)?;
    if held_control != expected_control
        || file_digest(control_file, held_control.size).map_err(validation_app_error)?
            != expected_control_digest
    {
        return Err(local_error(
            ERROR_RESET_REFUSED,
            "Local Trash control changed before reset.",
        ));
    }
    for slot in [SlotIndex::Primary, SlotIndex::Alternate] {
        let expected_slot = expected_storage.slots[slot.as_usize()].ok_or_else(|| {
            local_error(
                ERROR_RESET_REFUSED,
                "Local Trash storage is unavailable for reset.",
            )
        })?;
        let held_slot = private_file_state(
            &slot_files[slot.as_usize()]
                .as_ref()
                .expect("opened reset slot")
                .metadata()
                .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash is unavailable."))?,
        )
        .map_err(validation_app_error)?;
        if held_slot != expected_slot {
            return Err(local_error(
                ERROR_RESET_REFUSED,
                "Local Trash storage changed before reset.",
            ));
        }
    }
    if let Some(expected_record) = expected_record {
        if held_control.size != CONTROL_FILE_BYTES {
            return Err(local_error(
                ERROR_RESET_REFUSED,
                "Local Trash control changed before reset.",
            ));
        }
        let selected = read_control_records(control_file)
            .and_then(select_control_record)
            .map_err(validation_app_error)?;
        if selected.0 != expected_record.active_slot.as_usize() || selected.1 != *expected_record {
            return Err(local_error(
                ERROR_RESET_REFUSED,
                "Local Trash control changed before reset.",
            ));
        }
    }
    Ok(())
}

pub(crate) fn reset_unhealthy(
    workspace: &Path,
    scope: &LocalTrashScope,
    inspected: &LocalTrashInspection,
) -> AppResult<LocalTrashResetResult> {
    reset_unhealthy_inner(workspace, scope, inspected, || {}, || {}, || {})
}

fn reset_unhealthy_inner<F, G, H>(
    workspace: &Path,
    scope: &LocalTrashScope,
    inspected: &LocalTrashInspection,
    before_final_validation: F,
    before_slot_stage: G,
    after_publish: H,
) -> AppResult<LocalTrashResetResult>
where
    F: FnOnce(),
    G: FnOnce(),
    H: FnOnce(),
{
    scope.validate()?;
    if !inspected.health.permits_reset() {
        return Err(local_error(
            ERROR_RESET_REFUSED,
            "Reset Local Trash State is available only for unhealthy local state.",
        ));
    }
    let expected = inspected.reset_token.as_ref().ok_or_else(|| {
        local_error(
            ERROR_RESET_REFUSED,
            "Local Trash state cannot be reset safely.",
        )
    })?;
    if expected.storage.is_absent() {
        return Err(local_error(
            ERROR_RESET_REFUSED,
            "Local Trash state cannot be reset safely.",
        ));
    }
    let pinned_workspace = pin_workspace(workspace).map_err(validation_app_error)?;
    let _process_guard =
        acquire_workspace_write_guard(&pinned_workspace).map_err(validation_app_error)?;
    let canonical_workspace = pinned_workspace.authorized_path.clone();
    if canonical_workspace != expected.canonical_workspace || scope != &expected.scope {
        return Err(local_error(
            ERROR_RESET_REFUSED,
            "Local Trash state changed before reset.",
        ));
    }
    require_no_companions_at(&pinned_workspace).map_err(validation_app_error)?;
    if storage_snapshot(&pinned_workspace).map_err(validation_app_error)? != expected.storage {
        return Err(local_error(
            ERROR_RESET_REFUSED,
            "Local Trash state changed before reset.",
        ));
    }

    let mut created_name = false;
    let (control_file, control_state) = if let Some(expected_control) = expected.storage.control {
        let (file, state) =
            open_named_private_file(&pinned_workspace, local_state_control_name(), true)
                .map_err(validation_app_error)?;
        if state != expected_control {
            return Err(local_error(
                ERROR_RESET_REFUSED,
                "Local Trash state changed before reset.",
            ));
        }
        acquire_file_lock(&file, true)
            .map_err(|_| local_error(ERROR_BUSY, "Local Trash is busy. Try again."))?;
        (file, state)
    } else {
        let (file, state) = create_private_file_at(&pinned_workspace, local_state_control_name())?;
        acquire_file_lock(&file, true)
            .map_err(|_| local_error(ERROR_BUSY, "Local Trash is busy. Try again."))?;
        created_name = true;
        (file, state)
    };

    let mut bound_storage = expected.storage.clone();
    bound_storage.control = Some(control_state);

    let mut slot_files: [Option<File>; 2] = [None, None];
    let mut slot_nodes = [FileNode {
        device: 0,
        inode: 0,
    }; 2];
    for slot in [SlotIndex::Primary, SlotIndex::Alternate] {
        let (file, state) = if let Some(expected_state) = expected.storage.slots[slot.as_usize()] {
            let opened = open_named_private_file(&pinned_workspace, slot_name(slot), true)
                .map_err(validation_app_error)?;
            if opened.1 != expected_state {
                return Err(local_error(
                    ERROR_RESET_REFUSED,
                    "Local Trash state changed before reset.",
                ));
            }
            opened
        } else {
            created_name = true;
            create_private_file_at(&pinned_workspace, slot_name(slot))?
        };
        slot_nodes[slot.as_usize()] = state.node;
        bound_storage.slots[slot.as_usize()] = Some(state);
        slot_files[slot.as_usize()] = Some(file);
    }

    if created_name {
        control_file.sync_all().map_err(|_| {
            local_error(
                ERROR_RESET_REFUSED,
                "Local Trash reset storage could not be confirmed.",
            )
        })?;
        for file in slot_files.iter().flatten() {
            file.sync_all().map_err(|_| {
                local_error(
                    ERROR_RESET_REFUSED,
                    "Local Trash reset storage could not be confirmed.",
                )
            })?;
        }
        pinned_workspace.directory.sync_all().map_err(|_| {
            local_error(
                ERROR_RESET_REFUSED,
                "Local Trash reset storage could not be confirmed.",
            )
        })?;
    }
    let expected_control_digest =
        file_digest(&control_file, control_state.size).map_err(validation_app_error)?;
    validate_reset_binding(
        &pinned_workspace,
        &control_file,
        &slot_files,
        &bound_storage,
        expected_control_digest,
        expected.selected_record.as_ref(),
    )?;
    before_final_validation();
    validate_reset_binding(
        &pinned_workspace,
        &control_file,
        &slot_files,
        &bound_storage,
        expected_control_digest,
        expected.selected_record.as_ref(),
    )?;
    before_slot_stage();
    validate_reset_binding(
        &pinned_workspace,
        &control_file,
        &slot_files,
        &bound_storage,
        expected_control_digest,
        expected.selected_record.as_ref(),
    )?;

    let rebootstrap_control = expected
        .selected_record
        .as_ref()
        .map_or(true, |record| record.sequence == u64::MAX);
    let selected_slot = if rebootstrap_control {
        SlotIndex::Primary
    } else {
        expected
            .selected_record
            .as_ref()
            .expect("non-rebootstrap reset has a selector")
            .active_slot
            .other()
    };
    let stage_error_code = if rebootstrap_control {
        ERROR_RESET_INDETERMINATE
    } else {
        ERROR_RESET_REFUSED
    };
    let stage_error = |message| local_error(stage_error_code, message);
    let staged_file = slot_files[selected_slot.as_usize()]
        .as_ref()
        .expect("opened reset slot");
    staged_file
        .set_len(0)
        .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?;
    let mut staged_conn = open_staged_connection(staged_file)
        .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?;
    initialize_database(&mut staged_conn, scope)
        .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?;
    let provisional_storage = storage_snapshot(&pinned_workspace)
        .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?;
    let provisional_token = reset_token(&canonical_workspace, scope, provisional_storage, None);
    let details =
        validate_connection_for_journal(&staged_conn, scope, &provisional_token, "memory")
            .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?;
    drop(staged_conn);
    staged_file
        .sync_all()
        .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?;
    let staged_state = private_file_state(
        &staged_file
            .metadata()
            .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?,
    )
    .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?;
    if staged_state.node != slot_nodes[selected_slot.as_usize()]
        || named_private_state(&pinned_workspace, slot_name(selected_slot))
            .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?
            != staged_state
    {
        return Err(stage_error(
            "Local Trash reset staging requires reinspection.",
        ));
    }
    let staged_digest = file_digest(staged_file, staged_state.size)
        .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?;
    let held_control = private_file_state(
        &control_file
            .metadata()
            .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?,
    )
    .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?;
    if held_control != control_state
        || named_private_state(&pinned_workspace, local_state_control_name())
            .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?
            != control_state
        || file_digest(&control_file, held_control.size)
            .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?
            != expected_control_digest
    {
        return Err(stage_error(
            "Local Trash reset staging requires reinspection.",
        ));
    }
    if let Some(current_record) = expected.selected_record.as_ref() {
        let selected = read_control_records(&control_file)
            .and_then(select_control_record)
            .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?;
        if selected.0 != current_record.active_slot.as_usize() || selected.1 != *current_record {
            return Err(stage_error(
                "Local Trash reset staging requires reinspection.",
            ));
        }
        if !rebootstrap_control {
            let active_slot = current_record.active_slot;
            let expected_active =
                bound_storage.slots[active_slot.as_usize()].expect("selected reset slot exists");
            let active_file = slot_files[active_slot.as_usize()]
                .as_ref()
                .expect("opened reset active slot");
            let held_active =
                private_file_state(&active_file.metadata().map_err(|_| {
                    stage_error("Local Trash reset staging requires reinspection.")
                })?)
                .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?;
            if held_active != expected_active
                || named_private_state(&pinned_workspace, slot_name(active_slot))
                    .map_err(|_| stage_error("Local Trash reset staging requires reinspection."))?
                    != expected_active
            {
                return Err(stage_error(
                    "Local Trash reset staging requires reinspection.",
                ));
            }
        }
    }
    let (sequence, record_index) = if !rebootstrap_control {
        let current_record = expected
            .selected_record
            .as_ref()
            .expect("non-rebootstrap reset has a selector");
        (
            current_record
                .sequence
                .checked_add(1)
                .expect("rebootstrap handles exhausted control sequences"),
            selected_slot.as_usize(),
        )
    } else {
        let empty_control = [0_u8; CONTROL_RECORD_BYTES * 2];
        write_all_at(&control_file, &empty_control, 0)
            .and_then(|_| control_file.set_len(CONTROL_FILE_BYTES))
            .and_then(|_| control_file.sync_all())
            .map_err(|_| stage_error("Local Trash reset control requires reinspection."))?;
        (1, SlotIndex::Primary.as_usize())
    };
    let record = control_record_for(
        sequence,
        selected_slot,
        slot_nodes,
        staged_state,
        staged_digest,
        &details,
        scope,
    )
    .map_err(|_| stage_error("Local Trash reset publication requires reinspection."))?;
    let held_control_before_publish = private_file_state(
        &control_file
            .metadata()
            .map_err(|_| stage_error("Local Trash reset publication requires reinspection."))?,
    )
    .map_err(|_| stage_error("Local Trash reset publication requires reinspection."))?;
    let named_control_before_publish =
        named_private_state(&pinned_workspace, local_state_control_name())
            .map_err(|_| stage_error("Local Trash reset publication requires reinspection."))?;
    let held_staged_before_publish = private_file_state(
        &staged_file
            .metadata()
            .map_err(|_| stage_error("Local Trash reset publication requires reinspection."))?,
    )
    .map_err(|_| stage_error("Local Trash reset publication requires reinspection."))?;
    let named_staged_before_publish =
        named_private_state(&pinned_workspace, slot_name(selected_slot))
            .map_err(|_| stage_error("Local Trash reset publication requires reinspection."))?;
    if held_control_before_publish.node != control_state.node
        || held_control_before_publish.size != CONTROL_FILE_BYTES
        || named_control_before_publish != held_control_before_publish
        || held_staged_before_publish != staged_state
        || named_staged_before_publish != held_staged_before_publish
        || file_digest(staged_file, held_staged_before_publish.size)
            .map_err(|_| stage_error("Local Trash reset publication requires reinspection."))?
            != record.active_digest
    {
        return Err(stage_error(
            "Local Trash reset publication requires reinspection.",
        ));
    }
    publish_control_record(&control_file, record_index, &record).map_err(|_| {
        local_error(
            ERROR_RESET_INDETERMINATE,
            "Local Trash reset publication requires reinspection.",
        )
    })?;
    let reset_publication_error = || {
        local_error(
            ERROR_RESET_INDETERMINATE,
            "Local Trash reset publication requires reinspection.",
        )
    };
    if rebootstrap_control {
        reconcile_control_publication(
            &pinned_workspace,
            &control_file,
            control_state.node,
            PublicationSlot {
                file: staged_file,
                state: staged_state,
            },
            PublicationSlot {
                file: slot_files[SlotIndex::Alternate.as_usize()]
                    .as_ref()
                    .expect("opened reset nonselected slot"),
                state: bound_storage.slots[SlotIndex::Alternate.as_usize()]
                    .expect("bound reset nonselected slot"),
            },
            record_index,
            &record,
        )
        .map_err(|_| reset_publication_error())?;

        let secondary_slot = SlotIndex::Alternate;
        let secondary_file = slot_files[secondary_slot.as_usize()]
            .as_ref()
            .expect("opened reset secondary slot");
        copy_file_to_slot(staged_file, staged_state.size, secondary_file)
            .map_err(|_| reset_publication_error())?;
        let secondary_token = reset_token(
            &canonical_workspace,
            scope,
            storage_snapshot(&pinned_workspace).map_err(|_| reset_publication_error())?,
            Some(record.clone()),
        );
        let secondary_conn =
            open_staged_connection(secondary_file).map_err(|_| reset_publication_error())?;
        let secondary_details =
            validate_connection_for_journal(&secondary_conn, scope, &secondary_token, "memory")
                .map_err(|_| reset_publication_error())?;
        drop(secondary_conn);
        secondary_file
            .sync_all()
            .map_err(|_| reset_publication_error())?;
        let secondary_state = private_file_state(
            &secondary_file
                .metadata()
                .map_err(|_| reset_publication_error())?,
        )
        .map_err(|_| reset_publication_error())?;
        let secondary_named = named_private_state(&pinned_workspace, slot_name(secondary_slot))
            .map_err(|_| reset_publication_error())?;
        let secondary_digest = file_digest(secondary_file, secondary_state.size)
            .map_err(|_| reset_publication_error())?;
        if secondary_state.node != slot_nodes[secondary_slot.as_usize()]
            || secondary_named != secondary_state
        {
            return Err(reset_publication_error());
        }
        let secondary_record = control_record_for(
            2,
            secondary_slot,
            slot_nodes,
            secondary_state,
            secondary_digest,
            &secondary_details,
            scope,
        )
        .map_err(|_| reset_publication_error())?;

        reconcile_control_publication(
            &pinned_workspace,
            &control_file,
            control_state.node,
            PublicationSlot {
                file: staged_file,
                state: staged_state,
            },
            PublicationSlot {
                file: secondary_file,
                state: secondary_state,
            },
            record_index,
            &record,
        )
        .map_err(|_| reset_publication_error())?;
        if private_file_state(
            &secondary_file
                .metadata()
                .map_err(|_| reset_publication_error())?,
        )
        .map_err(|_| reset_publication_error())?
            != secondary_state
            || named_private_state(&pinned_workspace, slot_name(secondary_slot))
                .map_err(|_| reset_publication_error())?
                != secondary_state
            || file_digest(secondary_file, secondary_state.size)
                .map_err(|_| reset_publication_error())?
                != secondary_record.active_digest
        {
            return Err(reset_publication_error());
        }
        publish_control_record(&control_file, secondary_slot.as_usize(), &secondary_record)
            .map_err(|_| reset_publication_error())?;
        after_publish();
        reconcile_control_publication(
            &pinned_workspace,
            &control_file,
            control_state.node,
            PublicationSlot {
                file: secondary_file,
                state: secondary_state,
            },
            PublicationSlot {
                file: staged_file,
                state: staged_state,
            },
            secondary_slot.as_usize(),
            &secondary_record,
        )
        .map_err(|_| reset_publication_error())?;
    } else {
        after_publish();
        let nonselected_slot = selected_slot.other();
        reconcile_control_publication(
            &pinned_workspace,
            &control_file,
            control_state.node,
            PublicationSlot {
                file: staged_file,
                state: staged_state,
            },
            PublicationSlot {
                file: slot_files[nonselected_slot.as_usize()]
                    .as_ref()
                    .expect("opened reset nonselected slot"),
                state: bound_storage.slots[nonselected_slot.as_usize()]
                    .expect("bound reset nonselected slot"),
            },
            record_index,
            &record,
        )
        .map_err(|_| reset_publication_error())?;
    }
    Ok(LocalTrashResetResult {
        reset: true,
        previous_health: inspected.health,
    })
}

#[cfg(test)]
pub(crate) fn test_active_sidecar_path(workspace: &Path) -> AppResult<PathBuf> {
    let pinned_workspace = pin_workspace(workspace).map_err(validation_app_error)?;
    let (control_file, control_state) =
        open_named_private_file(&pinned_workspace, local_state_control_name(), false)
            .map_err(validation_app_error)?;
    if control_state.size != CONTROL_FILE_BYTES {
        return Err(local_error(
            ERROR_INVALID,
            "Local Trash control is invalid.",
        ));
    }
    let (_, record) =
        select_control_record(read_control_records(&control_file).map_err(validation_app_error)?)
            .map_err(validation_app_error)?;
    Ok(workspace.join(slot_file_name(record.active_slot)))
}

#[cfg(test)]
pub(crate) fn test_reseal_active_sidecar(workspace: &Path) -> AppResult<()> {
    let pinned_workspace = pin_workspace(workspace).map_err(validation_app_error)?;
    let (control_file, control_state) =
        open_named_private_file(&pinned_workspace, local_state_control_name(), true)
            .map_err(validation_app_error)?;
    if control_state.size != CONTROL_FILE_BYTES {
        return Err(local_error(
            ERROR_INVALID,
            "Local Trash control is invalid.",
        ));
    }
    acquire_file_lock(&control_file, true)
        .map_err(|_| local_error(ERROR_BUSY, "Local Trash is busy. Try again."))?;
    let (record_index, mut record) =
        select_control_record(read_control_records(&control_file).map_err(validation_app_error)?)
            .map_err(validation_app_error)?;
    let (active_file, active_state) =
        open_named_private_file(&pinned_workspace, slot_name(record.active_slot), false)
            .map_err(validation_app_error)?;
    let conn = Connection::open_with_flags(
        sqlite_file_descriptor_path(&active_file),
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_EXRESCODE,
    )
    .map_err(sqlite_operation_error)?;
    let (revision, generation) = conn
        .query_row(
            "SELECT revision, generation FROM local_state_metadata WHERE singleton = 1",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(sqlite_operation_error)?;
    drop(conn);
    record.active_size = active_state.size;
    record.active_digest =
        file_digest(&active_file, active_state.size).map_err(validation_app_error)?;
    record.revision = u64::try_from(revision).unwrap_or(u64::MAX);
    record.generation_digest = sha256_bytes(generation.as_bytes());
    write_control_record(&control_file, record_index, &record)
        .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash control is unavailable."))
}

#[cfg(test)]
pub(crate) fn test_reseal_active_sidecar_envelope(workspace: &Path) -> AppResult<()> {
    let pinned_workspace = pin_workspace(workspace).map_err(validation_app_error)?;
    let (control_file, control_state) =
        open_named_private_file(&pinned_workspace, local_state_control_name(), true)
            .map_err(validation_app_error)?;
    if control_state.size != CONTROL_FILE_BYTES {
        return Err(local_error(
            ERROR_INVALID,
            "Local Trash control is invalid.",
        ));
    }
    acquire_file_lock(&control_file, true)
        .map_err(|_| local_error(ERROR_BUSY, "Local Trash is busy. Try again."))?;
    let (record_index, mut record) =
        select_control_record(read_control_records(&control_file).map_err(validation_app_error)?)
            .map_err(validation_app_error)?;
    let (active_file, active_state) =
        open_named_private_file(&pinned_workspace, slot_name(record.active_slot), false)
            .map_err(validation_app_error)?;
    record.active_size = active_state.size;
    record.active_digest =
        file_digest(&active_file, active_state.size).map_err(validation_app_error)?;
    write_control_record(&control_file, record_index, &record)
        .map_err(|_| local_error(ERROR_UNAVAILABLE, "Local Trash control is unavailable."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search_cancellation::{
        SearchCancellationRegistry, SearchOperationCategory, SEARCH_CANCELLED_CODE,
    };
    use std::io::Write;
    use std::os::unix::fs::{symlink, DirBuilderExt};
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(1);

    struct SyntheticWorkspace {
        path: PathBuf,
    }

    impl SyntheticWorkspace {
        fn new(label: &str) -> Self {
            let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "pstqv-local-trash-{label}-{}-{sequence}",
                std::process::id()
            ));
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(&path).unwrap();
            let path = path.canonicalize().unwrap();
            Self { path }
        }

        fn sidecar(&self) -> PathBuf {
            test_active_sidecar_path(&self.path).unwrap_or_else(|_| path_for(&self.path))
        }

        fn primary_sidecar(&self) -> PathBuf {
            path_for(&self.path)
        }

        fn alternate_sidecar(&self) -> PathBuf {
            self.path.join(LOCAL_TRASH_ALT_FILE_NAME)
        }

        fn control(&self) -> PathBuf {
            self.path
                .join(local_state_control_name().to_string_lossy().as_ref())
        }
    }

    impl Drop for SyntheticWorkspace {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    fn scope() -> LocalTrashScope {
        LocalTrashScope::new("a".repeat(64), "b".repeat(64)).unwrap()
    }

    fn other_scope() -> LocalTrashScope {
        LocalTrashScope::new("c".repeat(64), "d".repeat(64)).unwrap()
    }

    fn identity(value: &str) -> String {
        format!("Synthetic/{value}.eml")
    }

    fn operation(value: char) -> String {
        value.to_string().repeat(48)
    }

    fn raw_write_connection(path: &Path) -> Connection {
        let path = path.canonicalize().unwrap();
        Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .unwrap()
    }

    fn mutate_active_and_reseal<F>(workspace: &SyntheticWorkspace, mutation: F)
    where
        F: FnOnce(&Connection),
    {
        let conn = raw_write_connection(&workspace.sidecar());
        mutation(&conn);
        drop(conn);
        test_reseal_active_sidecar(&workspace.path).unwrap();
    }

    fn selected_control(workspace: &SyntheticWorkspace) -> (usize, ControlRecord) {
        let file = File::open(workspace.control()).unwrap();
        select_control_record(read_control_records(&file).unwrap()).unwrap()
    }

    fn synthetic_control_record(sequence: u64, active_slot: SlotIndex) -> ControlRecord {
        ControlRecord {
            sequence,
            active_slot,
            slot_nodes: [
                FileNode {
                    device: 11,
                    inode: 12,
                },
                FileNode {
                    device: 11,
                    inode: 13,
                },
            ],
            active_size: 4096,
            revision: 0,
            workspace_fingerprint: [0x11; 32],
            source_fingerprint: [0x22; 32],
            identity_version_digest: [0x33; 32],
            generation_digest: [0x44; 32],
            active_digest: [0x55; 32],
        }
    }

    fn canonical_read_connection(workspace: &SyntheticWorkspace) -> Connection {
        let path = workspace.path.join("canonical.sqlite");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE messages (eml_path TEXT NOT NULL UNIQUE) STRICT;
                 INSERT INTO messages (eml_path) VALUES
                    ('Synthetic/one.eml'),
                    ('Synthetic/two.eml'),
                    ('Synthetic/three.eml');",
            )
            .unwrap();
        }
        fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
        let path = path.canonicalize().unwrap();
        let conn = Connection::open_with_flags(
            &path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
                | OpenFlags::SQLITE_OPEN_NOFOLLOW
                | OpenFlags::SQLITE_OPEN_EXRESCODE,
        )
        .unwrap();
        sqlite_security::configure_connection(&conn, true).unwrap();
        conn
    }

    fn create_valid_empty(workspace: &SyntheticWorkspace, local_scope: &LocalTrashScope) {
        let result = restore_messages(
            &workspace.path,
            local_scope,
            0,
            &operation('r'),
            &[identity("never-trashed")],
        )
        .unwrap();
        assert_eq!(result.revision, 0);
        assert_eq!(result.stored_count, 0);
    }

    #[test]
    fn control_selector_enforces_dual_slot_history_and_rolls_preparing_forward() {
        let primary_one = synthetic_control_record(1, SlotIndex::Primary);
        let alternate_two = synthetic_control_record(2, SlotIndex::Alternate);
        let mut primary_three = synthetic_control_record(3, SlotIndex::Primary);
        primary_three.revision = 1;
        let mut alternate_four = synthetic_control_record(4, SlotIndex::Alternate);
        alternate_four.revision = 2;

        assert_eq!(
            select_control_record([
                ParsedControlRecord::Preparing(primary_one.clone()),
                ParsedControlRecord::Empty,
            ])
            .unwrap(),
            (0, primary_one.clone())
        );
        assert_eq!(
            select_control_record([
                ParsedControlRecord::Committed(primary_one.clone()),
                ParsedControlRecord::Preparing(alternate_two.clone()),
            ])
            .unwrap(),
            (1, alternate_two.clone())
        );
        assert_eq!(
            select_control_record([
                ParsedControlRecord::Committed(primary_three.clone()),
                ParsedControlRecord::Preparing(alternate_two.clone()),
            ])
            .unwrap(),
            (0, primary_three.clone())
        );
        assert_eq!(
            select_control_record([
                ParsedControlRecord::Committed(primary_three.clone()),
                ParsedControlRecord::Preparing(alternate_four.clone()),
            ])
            .unwrap(),
            (1, alternate_four)
        );

        assert!(select_control_record([
            ParsedControlRecord::Empty,
            ParsedControlRecord::Committed(alternate_two.clone()),
        ])
        .is_err());
        assert!(select_control_record([
            ParsedControlRecord::Committed(primary_three.clone()),
            ParsedControlRecord::Empty,
        ])
        .is_err());
        assert!(select_control_record([
            ParsedControlRecord::Committed(synthetic_control_record(2, SlotIndex::Primary)),
            ParsedControlRecord::Empty,
        ])
        .is_err());
        assert!(select_control_record([
            ParsedControlRecord::Committed(primary_one.clone()),
            ParsedControlRecord::Committed(synthetic_control_record(4, SlotIndex::Alternate)),
        ])
        .is_err());
        let mut mismatched_nodes = alternate_two.clone();
        mismatched_nodes.slot_nodes[1].inode += 1;
        assert!(select_control_record([
            ParsedControlRecord::Committed(primary_one.clone()),
            ParsedControlRecord::Committed(mismatched_nodes),
        ])
        .is_err());
        assert!(select_control_record([
            ParsedControlRecord::Committed(primary_one),
            ParsedControlRecord::Invalid,
        ])
        .is_err());
        assert!(
            select_control_record([ParsedControlRecord::Empty, ParsedControlRecord::Empty,])
                .is_err()
        );
    }

    #[test]
    fn control_record_parser_rejects_malformed_or_unsafe_records() {
        let record = synthetic_control_record(1, SlotIndex::Primary);
        assert!(matches!(
            parse_control_record(&encode_control_record(
                &record,
                CONTROL_STATUS_COMMITTED
            )),
            ParsedControlRecord::Committed(value) if value == record
        ));
        assert!(matches!(
            parse_control_record(&encode_control_record(
                &record,
                CONTROL_STATUS_PREPARING
            )),
            ParsedControlRecord::Preparing(value) if value == record
        ));

        let mut malformed = encode_control_record(&record, CONTROL_STATUS_COMMITTED);
        malformed[8] ^= 0x01;
        assert!(matches!(
            parse_control_record(&malformed),
            ParsedControlRecord::Invalid
        ));
        let mut malformed = encode_control_record(&record, CONTROL_STATUS_COMMITTED);
        malformed[29] = 1;
        assert!(matches!(
            parse_control_record(&malformed),
            ParsedControlRecord::Invalid
        ));
        let mut malformed = encode_control_record(&record, CONTROL_STATUS_COMMITTED);
        malformed[300] ^= 0x01;
        assert!(matches!(
            parse_control_record(&malformed),
            ParsedControlRecord::Invalid
        ));
        let mut status_only = [0_u8; CONTROL_RECORD_BYTES];
        put_u64(&mut status_only, 0, CONTROL_STATUS_PREPARING);
        assert!(matches!(
            parse_control_record(&status_only),
            ParsedControlRecord::Invalid
        ));
        for unsafe_record in [
            ControlRecord {
                sequence: 0,
                ..record.clone()
            },
            ControlRecord {
                active_size: MAX_LOCAL_STATE_DATABASE_BYTES + 1,
                ..record.clone()
            },
            ControlRecord {
                revision: MAX_SAFE_JSON_INTEGER + 1,
                ..record.clone()
            },
            ControlRecord {
                slot_nodes: [
                    FileNode {
                        device: 0,
                        inode: 12,
                    },
                    record.slot_nodes[1],
                ],
                ..record.clone()
            },
        ] {
            assert!(matches!(
                parse_control_record(&encode_control_record(
                    &unsafe_record,
                    CONTROL_STATUS_COMMITTED
                )),
                ParsedControlRecord::Invalid
            ));
        }
    }

    #[test]
    fn control_writer_rejects_offsets_outside_the_fixed_control_file() {
        let workspace = SyntheticWorkspace::new("control-bounds");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(workspace.control())
            .unwrap();
        file.set_len(CONTROL_FILE_BYTES).unwrap();
        let record = synthetic_control_record(1, SlotIndex::Primary);
        for index in [2, usize::MAX] {
            assert_eq!(
                write_control_record(&file, index, &record)
                    .unwrap_err()
                    .kind(),
                std::io::ErrorKind::InvalidInput
            );
        }
        assert_eq!(file.metadata().unwrap().len(), CONTROL_FILE_BYTES);
        assert!(fs::read(workspace.control())
            .unwrap()
            .iter()
            .all(|byte| *byte == 0));

        write_control_record(&file, 0, &record).unwrap();
        let mut first = [0_u8; CONTROL_RECORD_BYTES];
        read_exact_at(&file, &mut first, 0).unwrap();
        assert_eq!(get_u64(&first, 0), CONTROL_STATUS_PREPARING);
        assert_eq!(
            select_control_record(read_control_records(&file).unwrap()).unwrap(),
            (0, record)
        );
        assert_eq!(file.metadata().unwrap().len(), CONTROL_FILE_BYTES);
    }

    #[test]
    fn control_cutpoints_never_fall_back_from_a_torn_newer_record() {
        let workspace = SyntheticWorkspace::new("control-cutpoints");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(workspace.control())
            .unwrap();
        file.set_len(CONTROL_FILE_BYTES).unwrap();
        let primary_one = synthetic_control_record(1, SlotIndex::Primary);
        let alternate_two = synthetic_control_record(2, SlotIndex::Alternate);
        let mut primary_three = synthetic_control_record(3, SlotIndex::Primary);
        primary_three.revision = 1;
        let committed_primary = encode_control_record(&primary_one, CONTROL_STATUS_COMMITTED);
        let committed_alternate = encode_control_record(&alternate_two, CONTROL_STATUS_COMMITTED);

        write_all_at(&file, &committed_primary, 0).unwrap();
        write_all_at(
            &file,
            &CONTROL_STATUS_PREPARING.to_be_bytes(),
            CONTROL_RECORD_BYTES as u64,
        )
        .unwrap();
        file.sync_all().unwrap();
        assert!(
            select_control_record(read_control_records(&file).unwrap()).is_err(),
            "a status-only secondary write is invalid when no older secondary record exists"
        );

        write_all_at(&file, &committed_primary, 0).unwrap();
        write_all_at(&file, &committed_alternate, CONTROL_RECORD_BYTES as u64).unwrap();
        file.sync_all().unwrap();

        write_all_at(&file, &CONTROL_STATUS_PREPARING.to_be_bytes(), 0).unwrap();
        file.sync_all().unwrap();
        assert_eq!(
            select_control_record(read_control_records(&file).unwrap()).unwrap(),
            (1, alternate_two.clone())
        );

        let preparing_primary = encode_control_record(&primary_three, CONTROL_STATUS_PREPARING);
        write_all_at(&file, &preparing_primary[..127], 0).unwrap();
        file.sync_all().unwrap();
        assert!(select_control_record(read_control_records(&file).unwrap()).is_err());

        write_all_at(&file, &preparing_primary, 0).unwrap();
        file.sync_all().unwrap();
        assert_eq!(
            select_control_record(read_control_records(&file).unwrap()).unwrap(),
            (0, primary_three)
        );
    }

    #[test]
    fn bootstrap_and_singleton_upgrade_establish_dual_history_before_mutation() {
        let local_scope = scope();
        let bootstrap_workspace = SyntheticWorkspace::new("bootstrap-dual-history");
        let writer = open_writer(&bootstrap_workspace.path, &local_scope).unwrap();
        let selected = selected_control(&bootstrap_workspace);
        assert_eq!(
            (selected.0, selected.1.sequence, selected.1.active_slot),
            (1, 2, SlotIndex::Alternate)
        );
        let records =
            read_control_records(&File::open(bootstrap_workspace.control()).unwrap()).unwrap();
        assert!(records.iter().all(|record| matches!(
            record,
            ParsedControlRecord::Preparing(_) | ParsedControlRecord::Committed(_)
        )));
        drop(writer);

        let singleton_workspace = SyntheticWorkspace::new("singleton-upgrade");
        create_valid_empty(&singleton_workspace, &local_scope);
        let (_, current) = selected_control(&singleton_workspace);
        assert_eq!(
            (current.sequence, current.active_slot),
            (3, SlotIndex::Primary)
        );
        let singleton = ControlRecord {
            sequence: 1,
            ..current
        };
        let control = OpenOptions::new()
            .read(true)
            .write(true)
            .open(singleton_workspace.control())
            .unwrap();
        write_all_at(&control, &[0_u8; CONTROL_RECORD_BYTES * 2], 0).unwrap();
        write_control_record(&control, 0, &singleton).unwrap();
        assert_eq!(
            select_control_record(read_control_records(&control).unwrap()).unwrap(),
            (0, singleton)
        );
        drop(control);

        let moved = move_messages(
            &singleton_workspace.path,
            &local_scope,
            0,
            &operation('s'),
            &[identity("one")],
        )
        .unwrap();
        assert_eq!((moved.revision, moved.stored_count), (1, 1));
        let selected = selected_control(&singleton_workspace);
        assert_eq!(
            (selected.0, selected.1.sequence, selected.1.active_slot),
            (0, 3, SlotIndex::Primary)
        );
        let records =
            read_control_records(&File::open(singleton_workspace.control()).unwrap()).unwrap();
        assert!(records.iter().all(|record| matches!(
            record,
            ParsedControlRecord::Preparing(_) | ParsedControlRecord::Committed(_)
        )));
    }

    #[test]
    fn postpublication_namespace_swaps_are_reported_as_indeterminate() {
        let local_scope = scope();
        let writer_workspace = SyntheticWorkspace::new("writer-postpublish-swap");
        create_valid_empty(&writer_workspace, &local_scope);
        let prior_slot = writer_workspace.primary_sidecar();
        let displaced_prior_slot = writer_workspace.path.join("displaced-prior-slot.sqlite");
        let writer = open_writer(&writer_workspace.path, &local_scope).unwrap();
        let operation_error = match finish_writer_inner(writer, || {
            fs::rename(&prior_slot, &displaced_prior_slot).unwrap();
            let replacement = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&prior_slot)
                .unwrap();
            replacement.sync_all().unwrap();
        }) {
            Ok(_) => panic!("a postpublication prior-slot swap must not report success"),
            Err(error) => error,
        };
        assert_eq!(operation_error.code, Some(ERROR_OPERATION_INDETERMINATE));
        let selected = selected_control(&writer_workspace);
        assert_eq!(
            (selected.0, selected.1.sequence, selected.1.active_slot),
            (1, 4, SlotIndex::Alternate)
        );
        assert_eq!(
            inspect(&writer_workspace.path, &local_scope).health,
            LocalTrashHealth::Invalid
        );

        let reset_workspace = SyntheticWorkspace::new("reset-postpublish-swap");
        create_valid_empty(&reset_workspace, &local_scope);
        let control = OpenOptions::new()
            .write(true)
            .open(reset_workspace.control())
            .unwrap();
        write_all_at(&control, &[0xA5; 64], 0).unwrap();
        control.sync_all().unwrap();
        drop(control);
        let invalid = inspect(&reset_workspace.path, &local_scope);
        assert_eq!(invalid.health, LocalTrashHealth::Invalid);
        let control_path = reset_workspace.control();
        let displaced_control = reset_workspace.path.join("displaced-published-control");
        let reset_error = reset_unhealthy_inner(
            &reset_workspace.path,
            &local_scope,
            &invalid,
            || {},
            || {},
            || {
                fs::rename(&control_path, &displaced_control).unwrap();
                let replacement = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&control_path)
                    .unwrap();
                replacement.set_len(CONTROL_FILE_BYTES).unwrap();
                replacement.sync_all().unwrap();
            },
        )
        .unwrap_err();
        assert_eq!(reset_error.code, Some(ERROR_RESET_INDETERMINATE));
        let displaced = File::open(displaced_control).unwrap();
        let selected = select_control_record(read_control_records(&displaced).unwrap()).unwrap();
        assert_eq!(
            (selected.0, selected.1.sequence, selected.1.active_slot),
            (1, 2, SlotIndex::Alternate)
        );
        assert_eq!(
            inspect(&reset_workspace.path, &local_scope).health,
            LocalTrashHealth::Invalid
        );
    }

    #[test]
    fn workspace_gate_blocks_only_conflicting_operations_for_the_same_workspace() {
        let first = SyntheticWorkspace::new("gate-first");
        let second = SyntheticWorkspace::new("gate-second");
        let local_scope = scope();
        create_valid_empty(&first, &local_scope);
        create_valid_empty(&second, &local_scope);

        let first_reader = open_reader(&first.path, &local_scope).unwrap();
        let second_reader_same_workspace = open_reader(&first.path, &local_scope).unwrap();
        let blocked = match open_writer(&first.path, &local_scope) {
            Ok(_) => panic!("a same-workspace writer must not pass active readers"),
            Err(error) => error,
        };
        assert_eq!(blocked.code, Some(ERROR_BUSY));
        let independent_writer = open_writer(&second.path, &local_scope).unwrap();
        finish_writer(independent_writer).unwrap();
        drop(second_reader_same_workspace);
        drop(first_reader);
        let writer_after_readers = open_writer(&first.path, &local_scope).unwrap();
        finish_writer(writer_after_readers).unwrap();

        let held_writer = open_writer(&first.path, &local_scope).unwrap();
        let blocked_reader = match open_reader(&first.path, &local_scope) {
            Ok(_) => panic!("a same-workspace reader must not pass an active writer"),
            Err(error) => error,
        };
        assert_eq!(blocked_reader.code, ERROR_BUSY);
        let independent_writer = open_writer(&second.path, &local_scope).unwrap();
        finish_writer(independent_writer).unwrap();
        drop(held_writer);
    }

    #[test]
    fn reset_publishes_a_new_selector_or_rebootstraps_invalid_control() {
        let local_scope = scope();
        let selected_workspace = SyntheticWorkspace::new("reset-selected-control");
        create_valid_empty(&selected_workspace, &local_scope);
        mutate_active_and_reseal(&selected_workspace, |conn| {
            conn.pragma_update(None, "user_version", LOCAL_STATE_SCHEMA_VERSION + 1)
                .unwrap();
        });
        let future = inspect(&selected_workspace.path, &local_scope);
        assert_eq!(future.health, LocalTrashHealth::FutureSchema);
        let before = fs::read(selected_workspace.control()).unwrap();
        reset_unhealthy(&selected_workspace.path, &local_scope, &future).unwrap();
        let after = fs::read(selected_workspace.control()).unwrap();
        assert_eq!(
            &before[..CONTROL_RECORD_BYTES],
            &after[..CONTROL_RECORD_BYTES],
            "the selected Primary history record must not be truncated"
        );
        let selected = selected_control(&selected_workspace);
        assert_eq!(
            (selected.0, selected.1.sequence, selected.1.active_slot),
            (1, 4, SlotIndex::Alternate)
        );
        let healthy = inspect(&selected_workspace.path, &local_scope);
        assert_eq!(
            (healthy.health, healthy.revision, healthy.stored_count),
            (LocalTrashHealth::Healthy, 0, 0)
        );

        let invalid_workspace = SyntheticWorkspace::new("reset-invalid-control");
        create_valid_empty(&invalid_workspace, &local_scope);
        let control = OpenOptions::new()
            .write(true)
            .open(invalid_workspace.control())
            .unwrap();
        write_all_at(&control, &[0xA5; 64], 0).unwrap();
        control.sync_all().unwrap();
        drop(control);
        let invalid = inspect(&invalid_workspace.path, &local_scope);
        assert_eq!(invalid.health, LocalTrashHealth::Invalid);
        reset_unhealthy(&invalid_workspace.path, &local_scope, &invalid).unwrap();
        let selected = selected_control(&invalid_workspace);
        assert_eq!(
            (selected.0, selected.1.sequence, selected.1.active_slot),
            (1, 2, SlotIndex::Alternate)
        );
        let records =
            read_control_records(&File::open(invalid_workspace.control()).unwrap()).unwrap();
        assert!(records.iter().all(|record| matches!(
            record,
            ParsedControlRecord::Preparing(_) | ParsedControlRecord::Committed(_)
        )));
        assert_eq!(
            fs::metadata(invalid_workspace.control()).unwrap().len(),
            CONTROL_FILE_BYTES
        );
    }

    #[test]
    fn selector_alternates_and_unpublished_staging_never_changes_visibility() {
        let workspace = SyntheticWorkspace::new("selector-alternation");
        let local_scope = scope();
        let message = identity("one");
        move_messages(
            &workspace.path,
            &local_scope,
            0,
            &operation('a'),
            std::slice::from_ref(&message),
        )
        .unwrap();
        let first = selected_control(&workspace);
        assert_eq!(
            (first.0, first.1.sequence, first.1.active_slot),
            (0, 3, SlotIndex::Primary)
        );
        let slot_nodes = first.1.slot_nodes;

        move_messages(
            &workspace.path,
            &local_scope,
            1,
            &operation('b'),
            std::slice::from_ref(&message),
        )
        .unwrap();
        let second = selected_control(&workspace);
        assert_eq!(
            (second.0, second.1.sequence, second.1.active_slot),
            (1, 4, SlotIndex::Alternate)
        );
        assert_eq!(second.1.slot_nodes, slot_nodes);

        let mut writer = open_writer(&workspace.path, &local_scope).unwrap();
        writer
            .connection_mut()
            .unwrap()
            .execute("DELETE FROM trashed_messages", [])
            .unwrap();
        writer
            .connection_mut()
            .unwrap()
            .execute(
                "UPDATE local_state_metadata SET revision = revision + 1 WHERE singleton = 1",
                [],
            )
            .unwrap();
        drop(writer);
        let still_selected = inspect(&workspace.path, &local_scope);
        assert_eq!(
            (
                still_selected.health,
                still_selected.revision,
                still_selected.stored_count,
            ),
            (LocalTrashHealth::Healthy, 1, 1)
        );
        assert_eq!(selected_control(&workspace), second);

        let restored = restore_messages(
            &workspace.path,
            &local_scope,
            1,
            &operation('c'),
            &[message],
        )
        .unwrap();
        assert_eq!((restored.revision, restored.stored_count), (2, 0));
        let third = selected_control(&workspace);
        assert_eq!(
            (third.0, third.1.sequence, third.1.active_slot),
            (0, 5, SlotIndex::Primary)
        );
        assert_eq!(third.1.slot_nodes, slot_nodes);
        assert_eq!(
            fs::metadata(workspace.control()).unwrap().len(),
            CONTROL_FILE_BYTES
        );
    }

    #[test]
    fn production_local_trash_never_uses_namespace_destructive_apis() {
        let source = include_str!("local_trash.rs");
        let production = source
            .split("\n#[cfg(test)]\nmod tests")
            .next()
            .expect("production source boundary");
        for forbidden in [
            "unlinkat",
            "libc::unlink(",
            "renameat",
            "renameat2",
            "renameatx_np",
            "RENAME_SWAP",
            "fs::rename(",
            "std::fs::rename(",
            "libc::rename(",
            "remove_file",
            "remove_dir(",
            "remove_dir_all",
        ] {
            assert!(
                !production.contains(forbidden),
                "production Local Trash must not contain {forbidden}"
            );
        }
    }

    #[test]
    fn first_creation_is_private_versioned_and_scope_bound() {
        let workspace = SyntheticWorkspace::new("create");
        let local_scope = scope();
        let absent = inspect(&workspace.path, &local_scope);
        assert_eq!(absent.health, LocalTrashHealth::Absent);
        assert!(absent.available);
        assert_eq!(absent.revision, 0);

        let moved = move_messages(
            &workspace.path,
            &local_scope,
            0,
            &operation('a'),
            &[identity("one")],
        )
        .unwrap();
        assert_eq!(moved.changed_count, 1);
        assert_eq!(moved.revision, 1);
        assert_eq!(moved.stored_count, 1);

        for path in [
            workspace.primary_sidecar(),
            workspace.alternate_sidecar(),
            workspace.control(),
        ] {
            let metadata = fs::symlink_metadata(path).unwrap();
            assert_eq!(metadata.mode() & 0o7777, 0o600);
            assert_eq!(metadata.nlink(), 1);
        }
        assert_eq!(
            fs::metadata(workspace.control()).unwrap().len(),
            CONTROL_FILE_BYTES
        );
        let sidecar = workspace.sidecar().canonicalize().unwrap();
        let conn = Connection::open_with_flags(
            sidecar,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )
        .unwrap();
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            LOCAL_STATE_SCHEMA_VERSION
        );
        assert_eq!(
            conn.query_row("PRAGMA application_id", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            LOCAL_STATE_APPLICATION_ID
        );
        assert_eq!(
            conn.query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
                .unwrap(),
            "delete"
        );
        let stored_scope = conn
            .query_row(
                "SELECT workspace_fingerprint, source_fingerprint,
                        canonical_identity_version
                 FROM trashed_messages",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            stored_scope,
            (
                local_scope.workspace_fingerprint().to_string(),
                local_scope.source_fingerprint().to_string(),
                LOCAL_TRASH_IDENTITY_VERSION.to_string(),
            )
        );
        for path in [workspace.primary_sidecar(), workspace.alternate_sidecar()] {
            assert!(!companion_path(&path, "-wal").exists());
            assert!(!companion_path(&path, "-shm").exists());
            assert!(!companion_path(&path, "-journal").exists());
        }
        let control_file = File::open(workspace.control()).unwrap();
        let records = read_control_records(&control_file).unwrap();
        assert!(records.iter().all(|record| matches!(
            record,
            ParsedControlRecord::Preparing(_) | ParsedControlRecord::Committed(_)
        )));
        let (index, selected) = select_control_record(records).unwrap();
        assert_eq!(
            (index, selected.sequence, selected.active_slot),
            (0, 3, SlotIndex::Primary)
        );
        let mut names = fs::read_dir(&workspace.path)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            vec![
                LOCAL_TRASH_ALT_FILE_NAME.to_string(),
                "local-state.control".to_string(),
                LOCAL_TRASH_FILE_NAME.to_string(),
            ]
        );
    }

    #[test]
    fn move_restore_repeat_and_restart_are_idempotent() {
        let workspace = SyntheticWorkspace::new("lifecycle");
        let local_scope = scope();
        let message = identity("one");
        let first = move_messages(
            &workspace.path,
            &local_scope,
            0,
            &operation('a'),
            std::slice::from_ref(&message),
        )
        .unwrap();
        assert_eq!((first.changed_count, first.already_trashed_count), (1, 0));
        let after_restart = inspect(&workspace.path, &local_scope);
        assert_eq!(after_restart.health, LocalTrashHealth::Healthy);
        assert_eq!((after_restart.revision, after_restart.stored_count), (1, 1));
        assert_eq!(
            after_restart.generation.as_deref(),
            Some(first.generation.as_str())
        );

        let repeated = move_messages(
            &workspace.path,
            &local_scope,
            1,
            &operation('b'),
            std::slice::from_ref(&message),
        )
        .unwrap();
        assert_eq!(
            (repeated.changed_count, repeated.already_trashed_count),
            (0, 1)
        );
        assert_eq!(repeated.revision, 1);

        let restored = restore_messages(
            &workspace.path,
            &local_scope,
            1,
            &operation('c'),
            std::slice::from_ref(&message),
        )
        .unwrap();
        assert_eq!(
            (restored.changed_count, restored.already_active_count),
            (1, 0)
        );
        assert_eq!((restored.revision, restored.stored_count), (2, 0));
        let repeated_restore = restore_messages(
            &workspace.path,
            &local_scope,
            2,
            &operation('d'),
            &[message],
        )
        .unwrap();
        assert_eq!(
            (
                repeated_restore.changed_count,
                repeated_restore.already_active_count,
            ),
            (0, 1)
        );
        assert_eq!(repeated_restore.revision, 2);
    }

    #[test]
    fn undo_only_restores_rows_changed_by_the_matching_operation() {
        let workspace = SyntheticWorkspace::new("undo");
        let local_scope = scope();
        let first_identity = identity("one");
        let second_identity = identity("two");
        let first_operation = operation('a');
        let second_operation = operation('b');
        move_messages(
            &workspace.path,
            &local_scope,
            0,
            &first_operation,
            std::slice::from_ref(&first_identity),
        )
        .unwrap();
        let second = move_messages(
            &workspace.path,
            &local_scope,
            1,
            &second_operation,
            &[first_identity.clone(), second_identity],
        )
        .unwrap();
        assert_eq!((second.changed_count, second.already_trashed_count), (1, 1));
        let undone = undo_move(&workspace.path, &local_scope, 2, &second_operation).unwrap();
        assert_eq!(
            (undone.changed_count, undone.stored_count, undone.revision),
            (1, 1, 3)
        );

        let conn = Connection::open(workspace.sidecar()).unwrap();
        let remaining = conn
            .query_row(
                "SELECT canonical_identity, operation_id FROM trashed_messages",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .unwrap();
        assert_eq!(remaining, (first_identity, first_operation.clone()));
        drop(conn);
        let final_undo = undo_move(&workspace.path, &local_scope, 3, &first_operation).unwrap();
        assert_eq!((final_undo.changed_count, final_undo.stored_count), (1, 0));
    }

    #[test]
    fn undo_refuses_a_missing_operation_without_advancing_revision() {
        let workspace = SyntheticWorkspace::new("undo-missing");
        let local_scope = scope();
        let operation_id = operation('a');
        move_messages(
            &workspace.path,
            &local_scope,
            0,
            &operation_id,
            &[identity("one")],
        )
        .unwrap();
        mutate_active_and_reseal(&workspace, |conn| {
            conn.execute(
                "DELETE FROM trashed_messages WHERE operation_id = ?1",
                [&operation_id],
            )
            .unwrap();
        });

        let error = undo_move(&workspace.path, &local_scope, 1, &operation_id).unwrap_err();
        assert_eq!(error.code, Some(ERROR_INVALID));
        let inspected = inspect(&workspace.path, &local_scope);
        assert_eq!((inspected.revision, inspected.stored_count), (1, 0));
    }

    #[test]
    fn stale_revision_and_operation_id_reuse_are_rejected() {
        let workspace = SyntheticWorkspace::new("stale");
        let local_scope = scope();
        let operation_id = operation('a');
        move_messages(
            &workspace.path,
            &local_scope,
            0,
            &operation_id,
            &[identity("one")],
        )
        .unwrap();
        let stale = move_messages(
            &workspace.path,
            &local_scope,
            0,
            &operation('b'),
            &[identity("two")],
        )
        .unwrap_err();
        assert_eq!(stale.code, Some(ERROR_STALE_REVISION));
        let reused = move_messages(
            &workspace.path,
            &local_scope,
            1,
            &operation_id,
            &[identity("two")],
        )
        .unwrap_err();
        assert_eq!(reused.code, Some(ERROR_INVALID_INPUT));
        assert_eq!(inspect(&workspace.path, &local_scope).stored_count, 1);
    }

    #[test]
    fn injected_interruption_rolls_back_the_complete_transaction() {
        let workspace = SyntheticWorkspace::new("interrupt");
        let local_scope = scope();
        let result = move_messages_inner(
            &workspace.path,
            &local_scope,
            0,
            &operation('a'),
            &[identity("one"), identity("two")],
            || {
                Err(local_error(
                    ERROR_UNAVAILABLE,
                    "Synthetic Local Trash interruption.",
                ))
            },
        );
        assert!(result.is_err());
        let inspected = inspect(&workspace.path, &local_scope);
        assert_eq!(inspected.health, LocalTrashHealth::Healthy);
        assert_eq!((inspected.revision, inspected.stored_count), (0, 0));
        assert_eq!(fs::read_dir(&workspace.path).unwrap().count(), 3);
        let control = File::open(workspace.control()).unwrap();
        let (index, record) =
            select_control_record(read_control_records(&control).unwrap()).unwrap();
        assert_eq!(
            (index, record.sequence, record.active_slot),
            (1, 2, SlotIndex::Alternate)
        );
    }

    #[test]
    fn wrong_fingerprint_fails_open_without_reassigning_rows() {
        let workspace = SyntheticWorkspace::new("fingerprint");
        let local_scope = scope();
        move_messages(
            &workspace.path,
            &local_scope,
            0,
            &operation('a'),
            &[identity("one")],
        )
        .unwrap();
        let wrong = inspect(&workspace.path, &other_scope());
        assert_eq!(wrong.health, LocalTrashHealth::FingerprintMismatch);
        assert_eq!(wrong.fingerprint_match, Some(false));
        assert_eq!(wrong.last_error_code, Some(ERROR_FINGERPRINT_MISMATCH));

        let canonical = canonical_read_connection(&workspace);
        let state = materialize_for_query(&canonical, &workspace.path, &other_scope()).unwrap();
        assert_eq!(state.health, LocalTrashHealth::FingerprintMismatch);
        assert_eq!(state.stored_count, 0);
        assert_eq!(
            canonical
                .query_row(
                    "SELECT COUNT(*) FROM temp.local_trash_identities",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn ambiguous_canonical_identity_keeps_materialization_empty_and_unavailable() {
        let workspace = SyntheticWorkspace::new("ambiguous-canonical-identity");
        let canonical_path = workspace.path.join("ambiguous-canonical.sqlite");
        {
            let conn = Connection::open(&canonical_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE messages (
                     id INTEGER PRIMARY KEY,
                     eml_path TEXT NOT NULL
                 ) STRICT;
                 INSERT INTO messages (eml_path) VALUES
                    ('Synthetic/duplicate.eml'),
                    ('Synthetic/duplicate.eml');",
            )
            .unwrap();
        }
        fs::set_permissions(&canonical_path, fs::Permissions::from_mode(0o400)).unwrap();
        let canonical_path = canonical_path.canonicalize().unwrap();
        let canonical = Connection::open_with_flags(
            canonical_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
                | OpenFlags::SQLITE_OPEN_NOFOLLOW
                | OpenFlags::SQLITE_OPEN_EXRESCODE,
        )
        .unwrap();
        sqlite_security::configure_connection(&canonical, true).unwrap();

        let absent = materialize_for_query(&canonical, &workspace.path, &scope()).unwrap();
        assert_eq!(absent.health, LocalTrashHealth::Unavailable);
        assert!(!absent.available);
        assert!(!absent.can_reset);
        assert!(!absent.sidecar_present);
        assert_eq!(
            absent.last_error_code,
            Some(ERROR_CANONICAL_IDENTITY_AMBIGUOUS)
        );

        create_valid_empty(&workspace, &scope());
        let present = materialize_for_query(&canonical, &workspace.path, &scope()).unwrap();
        assert_eq!(present.health, LocalTrashHealth::Unavailable);
        assert!(!present.available);
        assert!(!present.can_reset);
        assert!(present.sidecar_present);
        assert_eq!(
            present.last_error_code,
            Some(ERROR_CANONICAL_IDENTITY_AMBIGUOUS)
        );
        assert_eq!(
            canonical
                .query_row(
                    "SELECT COUNT(*) FROM temp.local_trash_identities",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn materialization_streams_to_temp_and_restores_query_only() {
        let workspace = SyntheticWorkspace::new("materialize");
        let local_scope = scope();
        move_messages(
            &workspace.path,
            &local_scope,
            0,
            &operation('a'),
            &[identity("one"), identity("two")],
        )
        .unwrap();
        let canonical = canonical_read_connection(&workspace);
        let state = materialize_for_query(&canonical, &workspace.path, &local_scope).unwrap();
        assert_eq!(state.health, LocalTrashHealth::Healthy);
        assert_eq!((state.revision, state.stored_count), (1, 2));
        assert_eq!(
            canonical
                .query_row("PRAGMA query_only", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            canonical
                .query_row(
                    "SELECT COUNT(*)
                     FROM messages m
                     WHERE EXISTS (
                         SELECT 1 FROM temp.local_trash_identities lt
                         WHERE lt.workspace_fingerprint = ?1
                           AND lt.source_fingerprint = ?2
                           AND lt.canonical_identity_version = ?3
                           AND lt.canonical_identity = m.eml_path
                     )",
                    params![
                        local_scope.workspace_fingerprint(),
                        local_scope.source_fingerprint(),
                        LOCAL_TRASH_IDENTITY_VERSION,
                    ],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        assert_eq!(
            canonical
                .query_row(
                    "SELECT COUNT(*)
                     FROM messages m
                     WHERE NOT EXISTS (
                         SELECT 1 FROM temp.local_trash_identities lt
                         WHERE lt.workspace_fingerprint = ?1
                           AND lt.source_fingerprint = ?2
                           AND lt.canonical_identity_version = ?3
                           AND lt.canonical_identity = m.eml_path
                     )",
                    params![
                        local_scope.workspace_fingerprint(),
                        local_scope.source_fingerprint(),
                        LOCAL_TRASH_IDENTITY_VERSION,
                    ],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );
        assert!(canonical
            .execute_batch("ATTACH ':memory:' AS forbidden")
            .is_err());
    }

    #[test]
    fn cancellation_during_materialization_interrupts_both_readers_and_rolls_back_temp_rows() {
        let workspace = SyntheticWorkspace::new("materialize-cancel");
        let local_scope = scope();
        move_messages(
            &workspace.path,
            &local_scope,
            0,
            &operation('a'),
            &[identity("one"), identity("two")],
        )
        .unwrap();
        let canonical = canonical_read_connection(&workspace);
        let registry = Arc::new(SearchCancellationRegistry::default());
        let operation_id = "local-trash-materialization";
        let guard = registry
            .begin_operation(
                "synthetic-window",
                7,
                operation_id,
                SearchOperationCategory::MessagePage,
            )
            .unwrap();
        guard.register_connection(&canonical).unwrap();
        let cancel = || {
            let outcome = registry
                .cancel_operation("synthetic-window", 7, operation_id)
                .unwrap();
            assert_eq!((outcome.operations, outcome.handles), (1, 2));
        };

        let error = materialize_for_query_inner(
            &canonical,
            &workspace.path,
            &local_scope,
            Some(&guard),
            Some(&cancel),
        )
        .unwrap_err();
        assert_eq!(error.code, Some(SEARCH_CANCELLED_CODE));
        assert_eq!(
            canonical
                .query_row("PRAGMA query_only", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            canonical
                .query_row(
                    "SELECT COUNT(*) FROM temp.local_trash_identities",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn corrupt_future_and_extra_schema_objects_are_fail_open() {
        let corrupt_workspace = SyntheticWorkspace::new("corrupt");
        let corrupt_path = corrupt_workspace.sidecar();
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&corrupt_path)
                .unwrap();
            use std::io::Write;
            file.write_all(b"not a sqlite database").unwrap();
            file.sync_all().unwrap();
        }
        assert_eq!(
            inspect(&corrupt_workspace.path, &scope()).health,
            LocalTrashHealth::Invalid
        );
        let canonical = canonical_read_connection(&corrupt_workspace);
        let query_state =
            materialize_for_query(&canonical, &corrupt_workspace.path, &scope()).unwrap();
        assert_eq!(query_state.health, LocalTrashHealth::Invalid);
        assert!(query_state.can_reset);
        assert_eq!(
            canonical
                .query_row(
                    "SELECT COUNT(*) FROM temp.local_trash_identities",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );

        let future_workspace = SyntheticWorkspace::new("future");
        create_valid_empty(&future_workspace, &scope());
        mutate_active_and_reseal(&future_workspace, |conn| {
            conn.pragma_update(None, "user_version", LOCAL_STATE_SCHEMA_VERSION + 1)
                .unwrap();
        });
        let future = inspect(&future_workspace.path, &scope());
        assert_eq!(future.health, LocalTrashHealth::FutureSchema);
        assert_eq!(future.schema_version, Some(LOCAL_STATE_SCHEMA_VERSION + 1));

        let schema_workspace = SyntheticWorkspace::new("schema-object");
        create_valid_empty(&schema_workspace, &scope());
        mutate_active_and_reseal(&schema_workspace, |conn| {
            conn.execute_batch(
                "CREATE VIEW hostile_view AS SELECT canonical_identity FROM trashed_messages;",
            )
            .unwrap();
        });
        assert_eq!(
            inspect(&schema_workspace.path, &scope()).health,
            LocalTrashHealth::Invalid
        );

        let collation_workspace = SyntheticWorkspace::new("hostile-collation");
        create_valid_empty(&collation_workspace, &scope());
        mutate_active_and_reseal(&collation_workspace, |conn| {
            conn.execute_batch(
                "BEGIN IMMEDIATE;
                 ALTER TABLE trashed_messages RENAME TO trashed_messages_original;
                 CREATE TABLE trashed_messages (
                     workspace_fingerprint TEXT NOT NULL,
                     source_fingerprint TEXT NOT NULL,
                     canonical_identity_version TEXT NOT NULL,
                     canonical_identity TEXT COLLATE NOCASE NOT NULL,
                     state_revision INTEGER NOT NULL
                         CHECK (state_revision BETWEEN 1 AND 9007199254740991),
                     trashed_at_ms INTEGER NOT NULL,
                     last_changed_at_ms INTEGER NOT NULL,
                     operation_id TEXT NOT NULL,
                     PRIMARY KEY (
                         workspace_fingerprint,
                         source_fingerprint,
                         canonical_identity_version,
                         canonical_identity
                     )
                 ) STRICT, WITHOUT ROWID;
                 DROP TABLE trashed_messages_original;
                 COMMIT;",
            )
            .unwrap();
        });
        assert_eq!(
            inspect(&collation_workspace.path, &scope()).health,
            LocalTrashHealth::Invalid
        );
    }

    #[test]
    fn linked_and_oversized_sidecars_are_never_opened() {
        let symlink_workspace = SyntheticWorkspace::new("symlink");
        let outside = symlink_workspace.path.join("outside.sqlite");
        {
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&outside)
                .unwrap();
            file.sync_all().unwrap();
        }
        symlink(&outside, symlink_workspace.sidecar()).unwrap();
        let symlinked = inspect(&symlink_workspace.path, &scope());
        assert_eq!(symlinked.health, LocalTrashHealth::Invalid);
        assert_eq!(symlinked.last_error_code, Some(ERROR_UNSAFE_FILE));

        let hardlink_workspace = SyntheticWorkspace::new("hardlink");
        let outside = hardlink_workspace.path.join("outside.sqlite");
        {
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&outside)
                .unwrap();
            file.sync_all().unwrap();
        }
        fs::hard_link(&outside, hardlink_workspace.sidecar()).unwrap();
        let hardlinked = inspect(&hardlink_workspace.path, &scope());
        assert_eq!(hardlinked.health, LocalTrashHealth::Invalid);
        assert_eq!(hardlinked.last_error_code, Some(ERROR_UNSAFE_FILE));

        let oversized_workspace = SyntheticWorkspace::new("oversized");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(oversized_workspace.sidecar())
            .unwrap();
        file.set_len(MAX_LOCAL_STATE_DATABASE_BYTES + 1).unwrap();
        file.sync_all().unwrap();
        let oversized = inspect(&oversized_workspace.path, &scope());
        assert_eq!(oversized.health, LocalTrashHealth::Invalid);
        assert_eq!(oversized.last_error_code, Some(ERROR_OVERSIZED));
    }

    #[test]
    fn an_unvalidated_companion_file_is_not_advertised_as_resettable() {
        let workspace = SyntheticWorkspace::new("unresettable-companion");
        let local_scope = scope();
        create_valid_empty(&workspace, &local_scope);
        let companion = companion_path(&workspace.sidecar(), "-wal");
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&companion)
            .unwrap();
        file.sync_all().unwrap();
        drop(file);

        let canonical = canonical_read_connection(&workspace);
        let query_state = materialize_for_query(&canonical, &workspace.path, &local_scope).unwrap();
        assert_eq!(query_state.health, LocalTrashHealth::Invalid);
        assert_eq!(query_state.last_error_code, Some(ERROR_INVALID));
        assert!(!query_state.can_reset);
    }

    #[test]
    fn redirected_workspace_ancestor_cannot_escape_the_authorized_path() {
        let root = SyntheticWorkspace::new("ancestor-redirect");
        let original_parent = root.path.join("original-parent");
        let moved_parent = root.path.join("moved-parent");
        let redirect_parent = root.path.join("redirect-parent");
        let original_workspace = original_parent.join("workspace");
        let redirect_workspace = redirect_parent.join("workspace");
        for path in [
            &original_parent,
            &redirect_parent,
            &original_workspace,
            &redirect_workspace,
        ] {
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(path).unwrap();
        }
        let authorized_workspace = original_workspace.canonicalize().unwrap();
        fs::rename(&original_parent, &moved_parent).unwrap();
        symlink(&redirect_parent, &original_parent).unwrap();

        let error = move_messages(
            &authorized_workspace,
            &scope(),
            0,
            &operation('a'),
            &[identity("one")],
        )
        .unwrap_err();
        assert_eq!(error.code, Some(ERROR_UNSAFE_FILE));
        assert!(!redirect_workspace.join(LOCAL_TRASH_FILE_NAME).exists());
        assert!(!moved_parent
            .join("workspace")
            .join(LOCAL_TRASH_FILE_NAME)
            .exists());
    }

    #[test]
    fn pinned_workspace_directory_survives_ancestor_swap_during_open() {
        let root = SyntheticWorkspace::new("ancestor-race");
        let original_parent = root.path.join("original-parent");
        let moved_parent = root.path.join("moved-parent");
        let redirect_parent = root.path.join("redirect-parent");
        let original_workspace = original_parent.join("workspace");
        let redirect_workspace = redirect_parent.join("workspace");
        for path in [
            &original_parent,
            &redirect_parent,
            &original_workspace,
            &redirect_workspace,
        ] {
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(path).unwrap();
        }
        let authorized_workspace = original_workspace.canonicalize().unwrap();
        move_messages(
            &authorized_workspace,
            &scope(),
            0,
            &operation('a'),
            &[identity("one")],
        )
        .unwrap();
        move_messages(
            &redirect_workspace.canonicalize().unwrap(),
            &scope(),
            0,
            &operation('b'),
            &[identity("two")],
        )
        .unwrap();

        let reader = open_reader_inner(&authorized_workspace, &scope(), || {
            fs::rename(&original_parent, &moved_parent).unwrap();
            symlink(&redirect_parent, &original_parent).unwrap();
        })
        .unwrap();
        let ReaderOpen::Opened(reader) = reader else {
            panic!("the pinned original workspace must remain readable");
        };
        let header = validate_header(&reader.conn, &scope(), &reader.reset_token).unwrap();
        let mut identities = Vec::new();
        stream_valid_identities(
            &reader.conn,
            &scope(),
            &reader.reset_token,
            &header,
            |value| {
                identities.push(value.to_string());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(identities, vec![identity("one")]);
        assert!(redirect_workspace.join(LOCAL_TRASH_FILE_NAME).exists());
        assert!(moved_parent
            .join("workspace")
            .join(LOCAL_TRASH_FILE_NAME)
            .exists());

        let writer_root = SyntheticWorkspace::new("ancestor-writer-race");
        let original_parent = writer_root.path.join("original-parent");
        let moved_parent = writer_root.path.join("moved-parent");
        let redirect_parent = writer_root.path.join("redirect-parent");
        let original_workspace = original_parent.join("workspace");
        let redirect_workspace = redirect_parent.join("workspace");
        for path in [
            &original_parent,
            &redirect_parent,
            &original_workspace,
            &redirect_workspace,
        ] {
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(path).unwrap();
        }
        let authorized_workspace = original_workspace.canonicalize().unwrap();
        let writer = open_writer_inner(
            &authorized_workspace,
            &scope(),
            || {},
            || {
                fs::rename(&original_parent, &moved_parent).unwrap();
                symlink(&redirect_parent, &original_parent).unwrap();
            },
        )
        .unwrap();
        finish_writer(writer).unwrap();
        assert!(!redirect_workspace.join(LOCAL_TRASH_FILE_NAME).exists());
        assert!(moved_parent
            .join("workspace")
            .join(LOCAL_TRASH_FILE_NAME)
            .exists());
    }

    #[test]
    fn reset_remains_anchored_to_the_pinned_workspace_after_ancestor_swap() {
        let root = SyntheticWorkspace::new("ancestor-reset-race");
        let original_parent = root.path.join("original-parent");
        let moved_parent = root.path.join("moved-parent");
        let redirect_parent = root.path.join("redirect-parent");
        let original_workspace = original_parent.join("workspace");
        let redirect_workspace = redirect_parent.join("workspace");
        for path in [
            &original_parent,
            &redirect_parent,
            &original_workspace,
            &redirect_workspace,
        ] {
            let mut builder = fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(path).unwrap();
        }
        let authorized_workspace = original_workspace.canonicalize().unwrap();
        let redirect_workspace = redirect_workspace.canonicalize().unwrap();
        let local_scope = scope();
        restore_messages(
            &authorized_workspace,
            &local_scope,
            0,
            &operation('a'),
            &[identity("original")],
        )
        .unwrap();
        restore_messages(
            &redirect_workspace,
            &local_scope,
            0,
            &operation('b'),
            &[identity("redirect")],
        )
        .unwrap();
        let original_sidecar = test_active_sidecar_path(&authorized_workspace).unwrap();
        raw_write_connection(&original_sidecar)
            .pragma_update(None, "user_version", LOCAL_STATE_SCHEMA_VERSION + 1)
            .unwrap();
        test_reseal_active_sidecar(&authorized_workspace).unwrap();
        let future = inspect(&authorized_workspace, &local_scope);
        assert_eq!(future.health, LocalTrashHealth::FutureSchema);
        let redirect_paths = [
            redirect_workspace.join(LOCAL_TRASH_FILE_NAME),
            redirect_workspace.join(LOCAL_TRASH_ALT_FILE_NAME),
            redirect_workspace.join("local-state.control"),
        ];
        let redirect_before = redirect_paths
            .iter()
            .map(|path| {
                (
                    fs::read(path).unwrap(),
                    state_from_metadata(&fs::metadata(path).unwrap()),
                )
            })
            .collect::<Vec<_>>();

        let reset = reset_unhealthy_inner(
            &authorized_workspace,
            &local_scope,
            &future,
            || {},
            || {
                fs::rename(&original_parent, &moved_parent).unwrap();
                symlink(&redirect_parent, &original_parent).unwrap();
            },
            || {},
        )
        .unwrap();
        assert!(reset.reset);
        let moved_workspace = moved_parent.join("workspace").canonicalize().unwrap();
        let reset_inspection = inspect(&moved_workspace, &local_scope);
        assert_eq!(reset_inspection.health, LocalTrashHealth::Healthy);
        assert_eq!(
            (reset_inspection.revision, reset_inspection.stored_count),
            (0, 0)
        );
        for (path, before) in redirect_paths.iter().zip(&redirect_before) {
            assert_eq!(fs::read(path).unwrap(), before.0);
            assert_eq!(state_from_metadata(&fs::metadata(path).unwrap()), before.1);
        }
        assert_eq!(
            fs::read_dir(moved_parent.join("workspace"))
                .unwrap()
                .count(),
            3
        );
        assert_eq!(fs::read_dir(&redirect_workspace).unwrap().count(), 3);
    }

    #[test]
    fn locks_fail_bounded_without_partial_changes() {
        let workspace = SyntheticWorkspace::new("locked");
        let local_scope = scope();
        create_valid_empty(&workspace, &local_scope);
        let lock = raw_write_connection(&workspace.sidecar());
        lock.execute_batch("BEGIN IMMEDIATE").unwrap();
        let result = move_messages(
            &workspace.path,
            &local_scope,
            0,
            &operation('a'),
            &[identity("one")],
        )
        .unwrap_err();
        assert_eq!(result.code, Some(ERROR_BUSY));
        lock.execute_batch("ROLLBACK").unwrap();
        drop(lock);
        let inspected = inspect(&workspace.path, &local_scope);
        assert_eq!((inspected.revision, inspected.stored_count), (0, 0));
    }

    #[test]
    fn invalid_values_and_batches_are_rejected() {
        let workspace = SyntheticWorkspace::new("invalid-input");
        let local_scope = scope();
        for invalid_identity in ["", "/absolute.eml", "../escape.eml", "bad\\path.eml"] {
            let error = move_messages(
                &workspace.path,
                &local_scope,
                0,
                &operation('a'),
                &[invalid_identity.to_string()],
            )
            .unwrap_err();
            assert_eq!(error.code, Some(ERROR_INVALID_INPUT));
        }
        let duplicate = move_messages(
            &workspace.path,
            &local_scope,
            0,
            &operation('a'),
            &[identity("one"), identity("one")],
        )
        .unwrap_err();
        assert_eq!(duplicate.code, Some(ERROR_INVALID_INPUT));
        let oversized_batch = (0..=MAX_LOCAL_TRASH_BATCH)
            .map(|index| identity(&format!("row-{index}")))
            .collect::<Vec<_>>();
        assert_eq!(
            move_messages(
                &workspace.path,
                &local_scope,
                0,
                &operation('a'),
                &oversized_batch,
            )
            .unwrap_err()
            .code,
            Some(ERROR_INVALID_INPUT)
        );
        assert_eq!(
            move_messages(
                &workspace.path,
                &local_scope,
                0,
                "contains spaces and mailbox words",
                &[identity("one")],
            )
            .unwrap_err()
            .code,
            Some(ERROR_INVALID_INPUT)
        );
        assert_eq!(
            inspect(&workspace.path, &local_scope).health,
            LocalTrashHealth::Absent
        );
    }

    #[test]
    fn invalid_stored_timestamp_and_operation_id_fail_open() {
        let timestamp_workspace = SyntheticWorkspace::new("invalid-time");
        let local_scope = scope();
        move_messages(
            &timestamp_workspace.path,
            &local_scope,
            0,
            &operation('a'),
            &[identity("one")],
        )
        .unwrap();
        mutate_active_and_reseal(&timestamp_workspace, |conn| {
            conn.execute("UPDATE trashed_messages SET trashed_at_ms = -1", [])
                .unwrap();
        });
        assert_eq!(
            inspect(&timestamp_workspace.path, &local_scope).health,
            LocalTrashHealth::Invalid
        );

        let operation_workspace = SyntheticWorkspace::new("invalid-operation");
        move_messages(
            &operation_workspace.path,
            &local_scope,
            0,
            &operation('a'),
            &[identity("one")],
        )
        .unwrap();
        mutate_active_and_reseal(&operation_workspace, |conn| {
            conn.execute(
                "UPDATE trashed_messages SET operation_id = 'mailbox words'",
                [],
            )
            .unwrap();
        });
        assert_eq!(
            inspect(&operation_workspace.path, &local_scope).health,
            LocalTrashHealth::Invalid
        );
    }

    #[test]
    fn reset_is_exact_unhealthy_only_and_detects_substitution() {
        let healthy_workspace = SyntheticWorkspace::new("healthy-reset");
        let local_scope = scope();
        create_valid_empty(&healthy_workspace, &local_scope);
        let healthy = inspect(&healthy_workspace.path, &local_scope);
        assert_eq!(healthy.health, LocalTrashHealth::Healthy);
        assert_eq!(
            reset_unhealthy(&healthy_workspace.path, &local_scope, &healthy)
                .unwrap_err()
                .code,
            Some(ERROR_RESET_REFUSED)
        );
        assert!(healthy_workspace.sidecar().exists());

        let corrupt_workspace = SyntheticWorkspace::new("corrupt-reset");
        let path = corrupt_workspace.sidecar();
        {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .unwrap();
            use std::io::Write;
            file.write_all(b"invalid sqlite").unwrap();
            file.sync_all().unwrap();
        }
        let corrupt = inspect(&corrupt_workspace.path, &local_scope);
        assert_eq!(corrupt.health, LocalTrashHealth::Invalid);
        let reset = reset_unhealthy(&corrupt_workspace.path, &local_scope, &corrupt).unwrap();
        assert!(reset.reset);
        assert!(path.exists());
        let reset_inspection = inspect(&corrupt_workspace.path, &local_scope);
        assert_eq!(reset_inspection.health, LocalTrashHealth::Healthy);
        assert_eq!(
            (reset_inspection.revision, reset_inspection.stored_count),
            (0, 0)
        );
        assert_eq!(fs::read_dir(&corrupt_workspace.path).unwrap().count(), 3);

        let substituted_workspace = SyntheticWorkspace::new("substituted-reset");
        create_valid_empty(&substituted_workspace, &local_scope);
        mutate_active_and_reseal(&substituted_workspace, |conn| {
            conn.pragma_update(None, "user_version", LOCAL_STATE_SCHEMA_VERSION + 1)
                .unwrap();
        });
        let future = inspect(&substituted_workspace.path, &local_scope);
        assert_eq!(future.health, LocalTrashHealth::FutureSchema);
        let path = substituted_workspace.sidecar();
        let moved_path = substituted_workspace.path.join("prior-local-state.sqlite");
        let result = reset_unhealthy_inner(
            &substituted_workspace.path,
            &local_scope,
            &future,
            || {
                fs::rename(&path, &moved_path).unwrap();
                let file = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)
                    .unwrap();
                file.sync_all().unwrap();
            },
            || {},
            || {},
        )
        .unwrap_err();
        assert_eq!(result.code, Some(ERROR_RESET_REFUSED));
        assert!(path.exists());
        assert!(moved_path.exists());

        let post_validation_workspace = SyntheticWorkspace::new("post-validation-reset-race");
        create_valid_empty(&post_validation_workspace, &local_scope);
        mutate_active_and_reseal(&post_validation_workspace, |conn| {
            conn.pragma_update(None, "user_version", LOCAL_STATE_SCHEMA_VERSION + 1)
                .unwrap();
        });
        let future = inspect(&post_validation_workspace.path, &local_scope);
        let path = post_validation_workspace.sidecar();
        let preserved_original = post_validation_workspace
            .path
            .join("preserved-original-local-state.sqlite");
        let replacement_bytes = b"synthetic replacement must not be deleted";
        let result = reset_unhealthy_inner(
            &post_validation_workspace.path,
            &local_scope,
            &future,
            || {},
            || {
                fs::rename(&path, &preserved_original).unwrap();
                let mut replacement = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(0o600)
                    .open(&path)
                    .unwrap();
                replacement.write_all(replacement_bytes).unwrap();
                replacement.sync_all().unwrap();
            },
            || {},
        )
        .unwrap_err();
        assert_eq!(result.code, Some(ERROR_RESET_REFUSED));
        assert!(preserved_original.exists());
        assert_eq!(fs::read(&path).unwrap(), replacement_bytes);
    }

    #[test]
    fn unsafe_permissions_are_fail_open_and_not_resettable() {
        let workspace = SyntheticWorkspace::new("permissions");
        let local_scope = scope();
        create_valid_empty(&workspace, &local_scope);
        fs::set_permissions(workspace.sidecar(), fs::Permissions::from_mode(0o644)).unwrap();
        let inspected = inspect(&workspace.path, &local_scope);
        assert_eq!(inspected.health, LocalTrashHealth::Invalid);
        assert_eq!(inspected.last_error_code, Some(ERROR_UNSAFE_FILE));
        assert_eq!(
            reset_unhealthy(&workspace.path, &local_scope, &inspected)
                .unwrap_err()
                .code,
            Some(ERROR_RESET_REFUSED)
        );
        assert!(workspace.sidecar().exists());
    }

    #[test]
    fn revisions_are_bounded_to_exact_javascript_integers() {
        let workspace = SyntheticWorkspace::new("revision-bound");
        let local_scope = scope();
        move_messages(
            &workspace.path,
            &local_scope,
            0,
            &operation('a'),
            &[identity("one")],
        )
        .unwrap();
        mutate_active_and_reseal(&workspace, |conn| {
            conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
                .unwrap();
            conn.execute(
                "UPDATE local_state_metadata SET revision = ?1 WHERE singleton = 1",
                params![MAX_SAFE_JSON_INTEGER_I64],
            )
            .unwrap();
        });
        let inspected = inspect(&workspace.path, &local_scope);
        assert_eq!(inspected.health, LocalTrashHealth::Healthy);
        assert_eq!(inspected.revision, MAX_SAFE_JSON_INTEGER);
        let overflow = move_messages(
            &workspace.path,
            &local_scope,
            MAX_SAFE_JSON_INTEGER,
            &operation('b'),
            &[identity("two")],
        )
        .unwrap_err();
        assert_eq!(overflow.code, Some(ERROR_UNAVAILABLE));
        let unchanged = inspect(&workspace.path, &local_scope);
        assert_eq!(
            (unchanged.revision, unchanged.stored_count),
            (MAX_SAFE_JSON_INTEGER, 1)
        );

        mutate_active_and_reseal(&workspace, |conn| {
            conn.execute_batch("PRAGMA ignore_check_constraints = ON;")
                .unwrap();
            conn.execute(
                "UPDATE local_state_metadata SET revision = ?1 WHERE singleton = 1",
                params![MAX_SAFE_JSON_INTEGER_I64 + 1],
            )
            .unwrap();
        });
        let invalid = inspect(&workspace.path, &local_scope);
        assert_eq!(invalid.health, LocalTrashHealth::Invalid);
        assert_eq!(invalid.last_error_code, Some(ERROR_INVALID));
    }
}
