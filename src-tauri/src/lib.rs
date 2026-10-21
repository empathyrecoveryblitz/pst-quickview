use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use chrono::{DateTime, SecondsFormat, Utc};
use mailparse::MailHeaderMap;
use rusqlite::{
    params, params_from_iter, types::Value, Connection, OpenFlags, OptionalExtension, Transaction,
    TransactionBehavior,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(target_vendor = "apple")]
use std::ffi::CString;
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    env,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tauri::{
    menu::{AboutMetadata, Menu, PredefinedMenuItem, Submenu},
    Emitter, Manager, State,
};
use walkdir::WalkDir;

mod calendar_msg;
mod import_integrity_phase2;
#[cfg(test)]
mod import_integrity_phase2_tests;
#[cfg(test)]
mod import_integrity_tests;
pub mod import_progress;
#[cfg(test)]
mod import_progress_tests;
mod local_trash;
mod message_security;
mod runtime_profile;
mod search;
mod search_cancellation;
mod search_pagination;
#[cfg(test)]
mod search_stress;
mod sqlite_security;
mod support_session;
mod threading;

use calendar_msg::CalendarItemDetails;
use message_security::{
    open_bounded_message_file, read_bounded_message_source, validate_mime_preflight,
    validate_outlook_message, validate_parsed_mail, validate_rendered_message_text,
    MAX_MESSAGE_SOURCE_BYTES, MAX_MESSAGE_TEXT_BYTES,
};
use runtime_profile::{
    RuntimeProfile, RuntimeProfileIdentity, RuntimeProfilePaths, RuntimeProfileState,
};
use search::{
    build_message_query_source, conversation_sort_clause, message_keyset_condition,
    message_sort_clause, query_source_with_condition, resolve_message_keyset_boundary,
    search_match_context_from_row, search_match_select_sql, validate_conversation_sort,
    validate_message_sort_workspace_count, MessageQuerySource, MessageSearchCriteria, MessageSort,
    MessageVisibility, SearchFilters, SearchMatchContext, SearchValidationError,
};
use search_cancellation::{
    is_sqlite_interrupt, SearchCancellationError, SearchCancellationRegistry,
    SearchOperationCategory, SearchOperationGuard, SEARCH_CANCELLED_CODE,
};
use search_pagination::{
    opaque_hash, MessageCursorContext, MessageCursorPosition, SearchCursorCodec, SearchCursorError,
    UNSUPPORTED_SEARCH_CURSOR_CODE,
};
use sqlite_security::{
    configure_connection as configure_sqlite_connection, MAX_SQLITE_DATABASE_BYTES,
};

use threading::{
    assign_threads_with_progress, extract_email_addresses, extract_message_ids,
    normalize_thread_subject, ThreadAssignmentControl, ThreadInput,
};

const PRODUCT_DISPLAY_NAME: &str = "PST QuickView";
const WORKSPACE_MARKER_APP_NAME: &str = "PST QuickView";
const MISSING_READPST_INSTRUCTIONS: &str =
    "The bundled ReadPST helper is missing or invalid. Reinstall PST QuickView from a verified package.";
const ROOT_FOLDER_NAME: &str = "PST";
const NEXT_TO_PST_WORKSPACE_DIR: &str = ".pst-quickview.noindex";
const LEGACY_NEXT_TO_PST_WORKSPACE_DIR: &str = ".pst-quickview";
const WORKSPACE_MARKER_FILE: &str = ".pst-quickview-workspace";
const WORKSPACE_METADATA_FILE: &str = "metadata.json";
const MAX_WORKSPACE_METADATA_BYTES: u64 = 2 * 1024 * 1024;
const IMPORT_LOG_FILE: &str = "import.log";
const APPLICATION_LOG_FILE: &str = "application.log";
const PROJECT_LICENSE_RESOURCE: &str = "LICENSE";
const THIRD_PARTY_NOTICES_RESOURCE: &str = "THIRD_PARTY_NOTICES.md";
const IMPORT_LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;
const APPLICATION_LOG_MAX_BYTES: u64 = 1024 * 1024;
const EXPORT_LOG_MAX_BYTES: u64 = 2 * 1024 * 1024;
const LOG_BACKUP_COUNT: usize = 2;
const FULL_HASH_LIMIT_BYTES: u64 = 512 * 1024 * 1024;
const FINGERPRINT_CHUNK_BYTES: usize = 4 * 1024 * 1024;
const MAX_EMBEDDED_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const MAX_RTF_GROUP_DEPTH: usize = 4_096;
const MAX_PRINTABLE_HTML_BYTES: usize = 128 * 1024 * 1024;
const DEFAULT_MESSAGE_PAGE_SIZE: i64 = 250;
const MAX_MESSAGE_PAGE_SIZE: i64 = 1000;
const DEFAULT_CONVERSATION_PAGE_SIZE: i64 = 100;
const MAX_CONVERSATION_PAGE_SIZE: i64 = 500;
const CONVERSATION_MESSAGE_PAGE_SIZE: i64 = 100;
const CONVERSATION_SCHEMA_VERSION: &str = "conversation-index-v1";
const CANONICAL_IMPORT_IDENTITY_VERSION: &str = "workspace-relative-eml-v1";
const IMPORTER_GENERATION: &str = "readpst-separate-eml-v1";
// Once an atomic rename has occurred, a later error may leave either the new
// or restored layout visible after a crash unless that layout is validated and
// directory-synchronized. Callers must not project a definitive failed status
// over that fail-closed state.
const INDEX_PROMOTION_STATE_UNCERTAIN_CODE: &str = "INDEX_PROMOTION_STATE_UNCERTAIN";
const IMPORT_CANCELLED_CODE: &str = "IMPORT_CANCELLED";
// Structural schema progression: 1 = threading columns, 2 = conversation columns,
// 3 = verified conversation and attachment indexes. Legacy databases report 0.
const SQLITE_SCHEMA_VERSION_CURRENT: i64 = 3;
const INDEX_BATCH_SIZE: usize = 500;
const INDEX_PROGRESS_MESSAGE_INTERVAL: usize = 100;
const INDEX_PROGRESS_TIME_INTERVAL: Duration = Duration::from_millis(500);
const GIB_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_STANDALONE_ATTACHMENT_EXPORT_BYTES: usize = 256 * 1024 * 1024;
const MAX_EXTERNAL_FILES_PER_REQUEST: usize = 10;
const MAX_PENDING_EXTERNAL_FILES: usize = 32;
const MAX_PENDING_EXTERNAL_BATCHES: usize = 16;
const MAX_AUTHORIZED_STANDALONE_FILES: usize = 128;
const MAX_AUTHORIZED_EXPORT_FILES: usize = 512;
const MAX_PREVIEW_WINDOWS: usize = 32;
const MAX_OPEN_WORKSPACES: usize = 64;
const MAX_WORKSPACE_ID_SCALARS: usize = 256;
const MAX_CONVERSATION_ID_SCALARS: usize = 4_096;
const MAX_SEARCH_OFFSET_ROWS: i64 = 5_000_000;
const PREVIEW_WINDOW_LABEL_PREFIX: &str = "message-preview-";
const PREVIEW_WINDOW_ROUTE: &str = "index.html?messagePreview=1";
const MESSAGE_FORMAT_PROBE_BYTES: usize = 64 * 1024;
const PRIVATE_DIRECTORY_MODE: u32 = 0o700;
const PRIVATE_FILE_MODE: u32 = 0o600;
const READPST_CAPTURE_MAX_BYTES: usize = 256 * 1024;
const MAX_READPST_STREAM_BYTES: u64 = 64 * 1024 * 1024;
const READPST_VERSION_TIMEOUT: Duration = Duration::from_secs(5);
const READPST_POLL_INTERVAL: Duration = Duration::from_millis(100);
const READPST_TERMINATE_GRACE: Duration = Duration::from_secs(2);
const READPST_OUTPUT_SCAN_INTERVAL: Duration = Duration::from_secs(15);
const READPST_ACTIVITY_EMIT_INTERVAL: Duration = Duration::from_secs(1);
const READPST_ABSOLUTE_TIMEOUT: Duration = Duration::from_secs(48 * 60 * 60);
const MAX_READPST_EXTRACTED_ENTRIES: u64 = 2_000_000;
const MAX_READPST_EXTRACTED_FILE_BYTES: u64 = 512 * 1024 * 1024;
const READPST_OUTPUT_OVERHEAD_BYTES: u64 = 1024 * 1024 * 1024;
const READPST_OUTPUT_SIZE_RATIO: u64 = 8;
const READPST_X86_64_SHA256: &str =
    "500857ad0bcce39e8353cbd32cccd4789d6e84e8d082f188b7ba5da0f75f069b";
const READPST_ARM64_SHA256: &str =
    "31a374e4d08ceb34149222d3eebb08642cb65d88015250b0931abd5c9a8839fe";
const READPST_UNIVERSAL_SHA256: &str =
    "95b378b108b17c7fd26f697f59a17e3915f9ea6a7435ced1ed26197c872aaae4";

static NEXT_WRITE_PROBE: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
std::thread_local! {
    static FAIL_NEXT_INDEX_PROMOTION_AFTER_COMMIT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static FAIL_NEXT_PHASE2_FINAL_PATH_VERIFICATION: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn fail_next_index_promotion_after_commit() {
    FAIL_NEXT_INDEX_PROMOTION_AFTER_COMMIT.with(|flag| flag.set(true));
}

#[cfg(test)]
fn take_index_promotion_after_commit_failure() -> bool {
    FAIL_NEXT_INDEX_PROMOTION_AFTER_COMMIT.with(|flag| flag.replace(false))
}

#[cfg(test)]
fn fail_next_phase2_final_path_verification() {
    FAIL_NEXT_PHASE2_FINAL_PATH_VERIFICATION.with(|flag| flag.set(true));
}

#[cfg(test)]
fn take_phase2_final_path_verification_failure() -> bool {
    FAIL_NEXT_PHASE2_FINAL_PATH_VERIFICATION.with(|flag| flag.replace(false))
}

#[cfg(not(test))]
fn take_phase2_final_path_verification_failure() -> bool {
    false
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    message: String,
    setup_command: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'static str>,
}

impl AppError {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            setup_command: None,
            code: None,
        }
    }

    fn coded(message: impl Into<String>, code: &'static str) -> Self {
        Self {
            message: message.into(),
            setup_command: None,
            code: Some(code),
        }
    }

    fn search_cancelled() -> Self {
        Self {
            message: "Search cancelled.".to_string(),
            setup_command: None,
            code: Some(SEARCH_CANCELLED_CODE),
        }
    }

    fn missing_readpst() -> Self {
        Self {
            message: MISSING_READPST_INSTRUCTIONS.to_string(),
            setup_command: Some(MISSING_READPST_INSTRUCTIONS.to_string()),
            code: None,
        }
    }
}

fn import_cancelled_error(message: impl Into<String>) -> AppError {
    AppError::coded(message, IMPORT_CANCELLED_CODE)
}

impl std::fmt::Display for AppError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(error: std::io::Error) -> Self {
        Self::new(error.to_string())
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(error: rusqlite::Error) -> Self {
        if is_sqlite_interrupt(&error) {
            Self::search_cancelled()
        } else {
            Self::new(error.to_string())
        }
    }
}

impl From<SearchCancellationError> for AppError {
    fn from(error: SearchCancellationError) -> Self {
        if error == SearchCancellationError::Cancelled {
            Self::search_cancelled()
        } else {
            Self::new(error.to_string())
        }
    }
}

impl From<SearchValidationError> for AppError {
    fn from(error: SearchValidationError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<SearchCursorError> for AppError {
    fn from(error: SearchCursorError) -> Self {
        Self::coded(error.to_string(), error.code())
    }
}

impl From<runtime_profile::RuntimeProfileError> for AppError {
    fn from(error: runtime_profile::RuntimeProfileError) -> Self {
        Self::new(error.to_string())
    }
}

impl From<walkdir::Error> for AppError {
    fn from(error: walkdir::Error) -> Self {
        Self::new(error.to_string())
    }
}

type AppResult<T> = Result<T, AppError>;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadpstStatus {
    available: bool,
    path: Option<String>,
    version: Option<String>,
    source: String,
    source_label: String,
    setup_command: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AppDiagnostics {
    app_version: String,
    macos_version: String,
    cpu_architecture: String,
    executable_architecture: String,
    readpst_source: String,
    readpst_version: String,
    open_pst_count: usize,
    active_workspace_mode: String,
    active_workspace_path: String,
    database_schema_version: Option<i64>,
    conversation_data_status: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceSummary {
    id: String,
    pst_path: String,
    workspace_path: String,
    eml_dir: String,
    index_path: String,
    message_count: i64,
    folder_count: i64,
    reused_existing: bool,
    fingerprint: String,
    fingerprint_strategy: String,
    workspace_location_mode: String,
    workspace_location_label: String,
    pst_size_bytes: u64,
    integrity_state: import_integrity_phase2::WorkspaceState,
    integrity_classification: String,
    source_availability: String,
    can_resume_indexing: bool,
    can_repair_index: bool,
    can_rebuild_conversations: bool,
    resume_details: Option<import_integrity_phase2::ResumeDetails>,
    has_last_verification: bool,
    conversation_state: String,
    completed_import_duration_ms: Option<u64>,
    last_activity_at: Option<String>,
    active_import_elapsed_ms: u64,
    current_stage_elapsed_ms: u64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Folder {
    id: i64,
    parent_id: Option<i64>,
    path: String,
    name: String,
    message_count: i64,
    direct_message_count: i64,
    active_message_count: i64,
    active_direct_message_count: i64,
    local_trash_message_count: i64,
    local_trash_direct_message_count: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageListItem {
    id: i64,
    folder_id: i64,
    folder_path: String,
    folder_name: String,
    subject: String,
    sender: String,
    recipients: String,
    date: String,
    snippet: String,
    has_attachments: bool,
    attachment_count: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    search_match_context: Option<SearchMatchContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pst_display_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    workspace_path: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MessagePageResult {
    items: Vec<MessageListItem>,
    requested_offset: i64,
    returned_count: usize,
    has_more: bool,
    next_cursor: Option<String>,
    pagination_mode: &'static str,
    local_trash_states: Vec<LocalTrashQueryHealth>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageCountResult {
    total_count: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalTrashSummary {
    workspace_id: String,
    available: bool,
    can_reset: bool,
    sidecar_present: bool,
    trashed_message_count: i64,
    orphaned_state_count: i64,
    schema_version: Option<i64>,
    state_revision: u64,
    last_changed_at: Option<String>,
    fingerprint_match: bool,
    last_error_code: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalTrashQueryHealth {
    workspace_id: String,
    available: bool,
    can_reset: bool,
    sidecar_present: bool,
    schema_version: Option<i64>,
    state_revision: u64,
    fingerprint_match: bool,
    last_error_code: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalTrashMutationResult {
    workspace_id: String,
    result: &'static str,
    changed_count: usize,
    state_revision: u64,
    operation_id: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalTrashResetResult {
    workspace_id: String,
    reset: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationSummary {
    conversation_id: String,
    conversation_root_id: Option<i64>,
    subject: String,
    latest_sender: String,
    participants: Vec<String>,
    latest_date: String,
    snippet: String,
    matching_message_count: i64,
    total_message_count: i64,
    has_attachments: bool,
    latest_message_id: i64,
    assignment_method: String,
    workspace_id: String,
    pst_display_name: String,
    workspace_path: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversationWorkspaceScope {
    workspace_id: String,
    folder_id: Option<i64>,
    include_subfolders: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationWorkspaceIssue {
    workspace_id: String,
    pst_display_name: String,
    workspace_path: String,
    can_reindex: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationPageResult {
    items: Vec<ConversationSummary>,
    requested_offset: i64,
    returned_count: usize,
    has_more: bool,
    indexed_workspace_count: usize,
    unindexed_workspaces: Vec<ConversationWorkspaceIssue>,
    local_trash_states: Vec<LocalTrashQueryHealth>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationCountResult {
    total_count: i64,
    matching_message_count: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationMessageItem {
    #[serde(flatten)]
    message: MessageListItem,
    matches_scope: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ConversationMessagesResult {
    items: Vec<ConversationMessageItem>,
    matching_message_count: i64,
    total_message_count: i64,
    showing_entire_conversation: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceSearchCount {
    workspace_id: String,
    pst_display_name: String,
    count: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MultiMessagePageResult {
    items: Vec<MessageListItem>,
    requested_offset: i64,
    returned_count: usize,
    has_more: bool,
    next_cursor: Option<String>,
    pagination_mode: &'static str,
    local_trash_states: Vec<LocalTrashQueryHealth>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MultiMessageCountResult {
    total_count: i64,
    per_workspace_counts: Vec<WorkspaceSearchCount>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Attachment {
    id: i64,
    filename: String,
    sanitized_filename: String,
    content_type: String,
    size_bytes: Option<i64>,
    attachment_index: i64,
    content_disposition: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportAttachmentResult {
    exported: bool,
    attachment_id: i64,
    filename: String,
    sanitized_filename: String,
    output_path: Option<String>,
    size_bytes: Option<i64>,
    content_type: String,
    error: Option<String>,
}

impl ExportAttachmentResult {
    fn failed(attachment_id: i64, error: impl Into<String>) -> Self {
        Self {
            exported: false,
            attachment_id,
            filename: String::new(),
            sanitized_filename: String::new(),
            output_path: None,
            size_bytes: None,
            content_type: String::new(),
            error: Some(error.into()),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SavePrintableHtmlResult {
    saved: bool,
    filename: String,
    output_path: Option<String>,
    size_bytes: Option<i64>,
    error: Option<String>,
}

impl SavePrintableHtmlResult {
    fn cancelled(filename: String) -> Self {
        Self {
            saved: false,
            filename,
            output_path: None,
            size_bytes: None,
            error: None,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageDetail {
    id: i64,
    folder_id: i64,
    subject: String,
    sender: String,
    recipients: String,
    date: String,
    snippet: String,
    has_attachments: bool,
    attachment_count: i64,
    body: String,
    body_source: String,
    body_html_available: bool,
    eml_path: String,
    can_reindex_from_eml: bool,
    in_local_trash: bool,
    local_trash_available: bool,
    local_trash_error_code: Option<String>,
    attachments: Vec<Attachment>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HtmlRenderResult {
    html_available: bool,
    sanitized_html: String,
    remote_images_blocked: bool,
    remote_image_count: usize,
    embedded_image_count: usize,
    error: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageDiagnostics {
    message_id: i64,
    subject: String,
    body_source: String,
    has_body_text: bool,
    has_body_html: bool,
    source_eml_path: String,
    attachment_count: usize,
    attachments: Vec<DiagnosticAttachment>,
    detected_body_mime_part: Option<String>,
    rtf_body_promoted: bool,
    rtf_body_suppressed_from_attachments: bool,
    remote_images_detected: bool,
    cid_images_detected: bool,
    parse_warnings: Vec<String>,
    mime_parts: Vec<MimePartDiagnostic>,
    message_id_header: String,
    in_reply_to: String,
    references_header: String,
    normalized_subject: String,
    conversation_id: String,
    thread_assignment_method: String,
    detected_parent: Option<String>,
    detected_root: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DiagnosticAttachment {
    filename: String,
    content_type: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MimePartDiagnostic {
    path: String,
    content_type: String,
    content_disposition: String,
    filename: String,
    content_id: String,
    size_bytes: Option<i64>,
    role: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceEmlView {
    message_id: i64,
    eml_path: String,
    source_path: String,
    source_kind: String,
    source_format: String,
    source_label: String,
    message_class: String,
    subject: String,
    sender: String,
    recipients: String,
    date: String,
    body_text: String,
    body_source: String,
    body_html_available: bool,
    sanitized_html: String,
    remote_images_blocked: bool,
    remote_image_count: usize,
    embedded_image_count: usize,
    raw_source: String,
    raw_source_available: bool,
    parse_warnings: Vec<String>,
    attachments: Vec<Attachment>,
    inline_resources: Vec<Attachment>,
    message_id_header: String,
    in_reply_to: String,
    references_header: String,
    normalized_subject: String,
    calendar: Option<CalendarItemDetails>,
    in_local_trash: bool,
    local_trash_available: bool,
    local_trash_error_code: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExternalFileOpen {
    path: String,
    file_kind: String,
    stable_id: String,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExternalFileOpenBatch {
    files: Vec<ExternalFileOpen>,
    warnings: Vec<String>,
    skipped_count: usize,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExternalFileOpenReady {
    batches: Vec<ExternalFileOpenBatch>,
    external_open_received: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum PreviewWindowRequest {
    Workspace {
        workspace_id: String,
        message_id: i64,
    },
    Standalone {
        stable_id: String,
    },
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum PreviewWindowTarget {
    Workspace {
        workspace_id: String,
        message_id: i64,
    },
    Standalone {
        stable_id: String,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewWindowOpenResult {
    created: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CancelImportResult {
    requested: bool,
    pid: Option<u32>,
    message: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CancelSearchResult {
    cancelled_operations: usize,
    interrupted_connections: usize,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceSize {
    workspace_path: String,
    workspace_location_mode: String,
    workspace_location_label: String,
    total_bytes: u64,
    extracted_eml_bytes: u64,
    sqlite_index_bytes: u64,
    logs_bytes: u64,
    attachments_bytes: u64,
    available_disk_bytes: Option<u64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct WorkspacePreflight {
    original_pst_path: String,
    original_pst_exists: bool,
    original_pst_readable: bool,
    pst_size_bytes: u64,
    workspace_path: String,
    workspace_parent_path: String,
    workspace_location_mode: String,
    workspace_location_label: String,
    workspace_parent_writable: bool,
    workspace_parent_write_error: Option<String>,
    available_disk_bytes: Option<u64>,
    estimated_required_bytes: u64,
    has_enough_space: Option<bool>,
    space_warning: bool,
    warning_required: bool,
    warnings: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PstOpenPlan {
    pst_path: String,
    workspace_id: String,
    fingerprint: String,
    selected_workspace_path: String,
    selected_workspace_location_mode: String,
    selected_workspace_location_label: String,
    fallback_warning: Option<String>,
    preflight: WorkspacePreflight,
    existing_workspaces: Vec<ExistingWorkspace>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExistingWorkspace {
    workspace_id: String,
    workspace_path: String,
    workspace_location_mode: String,
    workspace_location_label: String,
    import_status: String,
    is_complete: bool,
    can_resume: bool,
    can_reimport: bool,
    message_count: i64,
    can_open: bool,
    integrity_state: import_integrity_phase2::WorkspaceState,
    integrity_classification: String,
    can_repair_index: bool,
    can_rebuild_conversations: bool,
    resume_details: Option<import_integrity_phase2::ResumeDetails>,
    has_last_verification: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteResult {
    attempted_path: String,
    existed_before: bool,
    marker_existed: bool,
    deleted: bool,
    already_missing: bool,
    exists_after: bool,
    removed_empty_parent: bool,
    parent_path: Option<String>,
    error: Option<String>,
    remaining_entries: Vec<String>,
    message: String,
}

impl DeleteResult {
    fn new(active: &ActiveWorkspace) -> Self {
        let marker = active.path.join(WORKSPACE_MARKER_FILE);
        Self {
            attempted_path: active.path.display().to_string(),
            existed_before: active.path.exists(),
            marker_existed: marker.exists(),
            deleted: false,
            already_missing: false,
            exists_after: active.path.exists(),
            removed_empty_parent: false,
            parent_path: active.path.parent().map(|path| path.display().to_string()),
            error: None,
            remaining_entries: Vec::new(),
            message: String::new(),
        }
    }
}

#[derive(Debug)]
struct PstIdentity {
    canonical_path: PathBuf,
    display_path: String,
    size: u64,
    modified_ns: u128,
    workspace_id: String,
    legacy_workspace_id: String,
    fingerprint: String,
    content_fingerprint: String,
    fingerprint_strategy: String,
}

#[derive(Clone, Debug)]
struct ActiveWorkspace {
    id: String,
    path: PathBuf,
    pst_path: PathBuf,
    fingerprint: String,
    location_mode: WorkspaceLocationMode,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct WorkspaceMarker {
    app: String,
    workspace_id: String,
    pst_content_fingerprint: String,
    workspace_location_mode: String,
    original_pst_path: String,
}

#[derive(Debug)]
struct ValidatedWorkspaceDelete {
    device: u64,
    inode: u64,
}

#[derive(Clone, Debug)]
struct ReadpstLocation {
    path: PathBuf,
    source: ReadpstSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadpstSource {
    Intel,
    AppleSilicon,
    Universal,
}

impl ReadpstSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::Intel => "bundled_intel",
            Self::AppleSilicon => "bundled_apple_silicon",
            Self::Universal => "bundled_universal",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Intel => "bundled Intel",
            Self::AppleSilicon => "bundled Apple Silicon",
            Self::Universal => "bundled universal",
        }
    }
}

#[derive(Default)]
struct AppState {
    runtime_profile: RuntimeProfileState,
    active_workspace: Mutex<Option<ActiveWorkspace>>,
    open_workspaces: Mutex<HashMap<String, ActiveWorkspace>>,
    import_running: AtomicBool,
    cancel_import_requested: AtomicBool,
    readpst_pid: Mutex<Option<u32>>,
    external_file_opens: Mutex<ExternalFileOpenState>,
    exported_files: Mutex<ExportAuthorizationRegistry>,
    preview_windows: Mutex<PreviewWindowRegistry>,
    search_cancellations: Arc<SearchCancellationRegistry>,
    search_cursor_codec: SearchCursorCodec,
    integrity_operation: Mutex<Option<IntegrityOperationScope>>,
    local_trash_undo: Mutex<Option<LocalTrashUndoRecord>>,
    local_trash_operation_sequence: AtomicU64,
}

impl AppState {
    fn initialize_runtime_profile(&self, profile: RuntimeProfile) -> AppResult<&RuntimeProfile> {
        self.runtime_profile
            .initialize(profile)
            .map_err(AppError::from)
    }

    fn runtime_profile(&self) -> AppResult<&RuntimeProfile> {
        self.runtime_profile.get().map_err(AppError::from)
    }
}

#[derive(Clone)]
struct LocalTrashUndoRecord {
    workspace_id: String,
    operation_id: String,
    state_revision: u64,
}

struct IntegrityOperationScope {
    workspace_id: String,
    workspace_path: PathBuf,
    operation_id: String,
    operation_kind: import_progress::OperationKind,
    workspace_storage_mode: import_progress::WorkspaceStorageMode,
    source_basename: String,
    source_size_bytes: u64,
    generation: u64,
    state_revision: u64,
    event_sequence: u64,
    persisted_state: import_integrity_phase2::WorkspaceState,
    completed_stages: Vec<import_progress::ProgressStage>,
    stage: import_progress::ProgressStage,
    status: import_progress::ProgressStatus,
    timing: import_progress::ActiveTiming,
    updates_import_timing: bool,
    completion_duration_eligible: bool,
    canonical_import_elapsed_ms: u64,
    monotonic_origin: Instant,
    last_activity_tick_ms: Option<u64>,
    last_activity_utc: Option<String>,
    last_checkpoint_tick_ms: u64,
    process_state: import_progress::ProcessState,
    stage_correction: Option<import_progress::StageCorrection>,
    counters: import_progress::ProgressCounters,
    warning_codes: Vec<import_progress::ProgressWarningCode>,
    eta_samples: Vec<import_progress::EtaSample>,
    last_processed_units: Option<u64>,
    persistence_error: Option<String>,
    terminal_status_override: Option<import_progress::ProgressStatus>,
}

#[derive(Default)]
struct ExternalFileOpenState {
    frontend_ready: bool,
    external_open_received: bool,
    pending: VecDeque<ExternalFileOpenBatch>,
    authorized_standalone_files: HashMap<String, AuthorizedStandaloneFile>,
    authorization_order: VecDeque<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    sha256: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SqliteFileState {
    device: u64,
    inode: u64,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
}

#[derive(Clone, Debug)]
struct AuthorizedStandaloneFile {
    canonical_path: PathBuf,
    identity: FileIdentity,
}

#[derive(Clone, Debug)]
struct AuthorizedExportFile {
    canonical_path: PathBuf,
    identity: FileIdentity,
}

#[derive(Default)]
struct ExportAuthorizationRegistry {
    by_path: HashMap<String, AuthorizedExportFile>,
    order: VecDeque<String>,
}

impl ExportAuthorizationRegistry {
    fn register(&mut self, record: AuthorizedExportFile) {
        let key = record.canonical_path.display().to_string();
        if self.by_path.insert(key.clone(), record).is_some() {
            self.order.retain(|existing| existing != &key);
        }
        self.order.push_back(key);
        while self.by_path.len() > MAX_AUTHORIZED_EXPORT_FILES {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.by_path.remove(&oldest);
        }
    }

    fn remove(&mut self, path: &str) {
        self.by_path.remove(path);
        self.order.retain(|existing| existing != path);
    }
}

#[derive(Default)]
struct PreviewWindowRegistry {
    targets_by_label: HashMap<String, PreviewWindowTarget>,
    labels_by_target: HashMap<PreviewWindowTarget, String>,
}

impl PreviewWindowRegistry {
    fn register(&mut self, target: PreviewWindowTarget) -> AppResult<String> {
        if let Some(label) = self.labels_by_target.get(&target) {
            return Ok(label.clone());
        }
        if self.targets_by_label.len() >= MAX_PREVIEW_WINDOWS {
            return Err(AppError::new(
                "Close an existing message preview before opening another one.",
            ));
        }

        let label = preview_window_label(&target);
        if self.targets_by_label.contains_key(&label) {
            return Err(AppError::new("Could not allocate a unique preview window."));
        }
        self.targets_by_label.insert(label.clone(), target.clone());
        self.labels_by_target.insert(target, label.clone());
        Ok(label)
    }

    fn target(&self, label: &str) -> Option<PreviewWindowTarget> {
        self.targets_by_label.get(label).cloned()
    }

    fn remove(&mut self, label: &str) {
        if let Some(target) = self.targets_by_label.remove(label) {
            self.labels_by_target.remove(&target);
        }
    }
}

fn preview_window_label(target: &PreviewWindowTarget) -> String {
    let target_identity = match target {
        PreviewWindowTarget::Workspace {
            workspace_id,
            message_id,
        } => format!("workspace\0{workspace_id}\0{message_id}"),
        PreviewWindowTarget::Standalone { stable_id } => {
            format!("standalone\0{stable_id}")
        }
    };
    format!(
        "{PREVIEW_WINDOW_LABEL_PREFIX}{}",
        sha256_text(&target_identity)
    )
}

fn is_valid_preview_window_label(label: &str) -> bool {
    label
        .strip_prefix(PREVIEW_WINDOW_LABEL_PREFIX)
        .is_some_and(is_lower_hex_sha256)
}

fn is_lower_hex_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImportStatus {
    Pending,
    Running,
    Complete,
    Failed,
    Cancelled,
}

impl ImportStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Complete => "complete",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImportAction {
    Import,
    OpenExisting,
    ResumeIndex,
    RepairIndex,
    Reimport,
}

impl ImportAction {
    fn from_arg(value: Option<String>) -> AppResult<Self> {
        match value.as_deref().unwrap_or("import") {
            "import" => Ok(Self::Import),
            "open_existing" => Ok(Self::OpenExisting),
            "resume_index" => Ok(Self::ResumeIndex),
            "repair_index" => Ok(Self::RepairIndex),
            "reimport" => Ok(Self::Reimport),
            _ => Err(AppError::new("Invalid import action.")),
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct WorkspaceMetadata {
    app_version: String,
    original_pst_path: String,
    pst_fingerprint: String,
    pst_size: String,
    pst_modified_ns: String,
    pst_fingerprint_strategy: String,
    workspace_path: String,
    workspace_mode: String,
    import_status: String,
    created_at: String,
    updated_at: String,
    started_at: Option<String>,
    finished_at: Option<String>,
    message_count_indexed: usize,
    error_count: usize,
    last_error: Option<String>,
    readpst_path: Option<String>,
    readpst_version: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkspaceLocationMode {
    AppSupport,
    NextToPst,
}

impl WorkspaceLocationMode {
    fn from_arg(value: &str) -> AppResult<Self> {
        match value {
            "app_support" => Ok(Self::AppSupport),
            "next_to_pst" => Ok(Self::NextToPst),
            _ => Err(AppError::new("Invalid workspace location mode.")),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::AppSupport => "app_support",
            Self::NextToPst => "next_to_pst",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::AppSupport => "App Support",
            Self::NextToPst => "Next to PST",
        }
    }
}

#[derive(Debug)]
struct WorkspaceSelection {
    path: PathBuf,
    mode: WorkspaceLocationMode,
    warning: Option<String>,
}

#[derive(Clone, Debug)]
struct WorkspaceCandidate {
    workspace_id: String,
    path: PathBuf,
    mode: WorkspaceLocationMode,
}

#[derive(Debug, Default)]
struct ParsedMessage {
    subject: String,
    sender: String,
    recipients: String,
    date: String,
    body: String,
    body_source: BodySource,
    body_html: String,
    attachments: Vec<AttachmentDraft>,
    rtf_body_fallback: Option<String>,
    message_id_header_raw: String,
    message_id_header: String,
    in_reply_to_raw: String,
    in_reply_to: String,
    references_header_raw: String,
    references_header: String,
    normalized_subject: String,
    thread_warning: String,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum BodySource {
    TextPlain,
    HtmlConverted,
    RtfConverted,
    RtfHtmlConverted,
    #[default]
    Missing,
    ParseError,
}

impl BodySource {
    fn as_str(self) -> &'static str {
        match self {
            Self::TextPlain => "text_plain",
            Self::HtmlConverted => "html_converted",
            Self::RtfConverted => "rtf_converted",
            Self::RtfHtmlConverted => "rtf_html_converted",
            Self::Missing => "missing",
            Self::ParseError => "parse_error",
        }
    }
}

#[derive(Debug, Default)]
struct AttachmentDraft {
    filename: String,
    sanitized_filename: String,
    content_type: String,
    size_bytes: Option<i64>,
    attachment_index: i64,
    content_disposition: String,
    mime_part_path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportOriginalEmlResult {
    exported: bool,
    message_id: i64,
    filename: String,
    workspace_path: Option<String>,
    export_dir: Option<String>,
    output_path: Option<String>,
    size_bytes: Option<i64>,
    error: Option<String>,
}

impl ExportOriginalEmlResult {
    fn failed(message_id: i64, error: String) -> Self {
        Self {
            exported: false,
            message_id,
            filename: String::new(),
            workspace_path: None,
            export_dir: None,
            output_path: None,
            size_bytes: None,
            error: Some(error),
        }
    }
}

#[derive(Debug)]
struct RtfBodyCandidate {
    text: String,
    html: Option<String>,
    kind: RtfConversionKind,
    part_path: String,
    filename: Option<String>,
    content_type: String,
    content_disposition: String,
}

#[derive(Debug)]
struct RtfConversion {
    text: String,
    html: Option<String>,
    kind: RtfConversionKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RtfConversionKind {
    FromText,
    FromHtml,
    Rtf,
}

impl RtfConversionKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::FromText => "fromtext",
            Self::FromHtml => "fromhtml1",
            Self::Rtf => "rtf",
        }
    }
}

struct MessageBodySelection {
    body: String,
    body_source: BodySource,
    body_html: String,
    rtf_body_part_path: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct IndexStats {
    discovered: usize,
    processed: usize,
    indexed: usize,
    inserted: usize,
    skipped_existing: usize,
    // Intentionally remains zero until a future extractor-provenance design can
    // prove that an output is auxiliary rather than a canonical source message.
    suppressed_empty_artifacts: usize,
    error_count: usize,
    attachments_indexed: usize,
    last_error: Option<String>,
}

fn progress_counters_from_index_stats(stats: &IndexStats) -> import_progress::ProgressCounters {
    import_progress::ProgressCounters {
        candidates_discovered: u64::try_from(stats.discovered).ok(),
        candidates_processed: u64::try_from(stats.processed).ok(),
        canonical_messages: u64::try_from(stats.indexed).ok(),
        inserted_messages: u64::try_from(stats.inserted).ok(),
        skipped_existing_messages: u64::try_from(stats.skipped_existing).ok(),
        parse_failures: u64::try_from(stats.error_count).ok(),
        attachments_indexed: u64::try_from(stats.attachments_indexed).ok(),
        ..import_progress::ProgressCounters::default()
    }
}

fn emit_index_progress(app: &tauri::AppHandle, stats: &IndexStats) {
    emit_progress_observation(
        app,
        ProgressObservation {
            processed_units: u64::try_from(stats.processed).ok(),
            total_units: u64::try_from(stats.discovered).ok(),
            unit_kind: Some(import_progress::ProgressUnitKind::CandidateRecords),
            activity_observed: true,
            counters: Some(progress_counters_from_index_stats(stats)),
            ..ProgressObservation::default()
        },
    );
}

fn emit_verification_progress(
    app: &tauri::AppHandle,
    completed: u64,
    report: Option<&import_integrity_phase2::VerificationReport>,
) {
    const VERIFICATION_CHECK_TOTAL: u64 = 4;
    let mut warning_codes = Vec::new();
    if let Some(report) = report {
        match report.source_integrity {
            import_integrity_phase2::SourceIntegrity::Unavailable => {
                warning_codes.push(import_progress::ProgressWarningCode::SourceUnavailable)
            }
            import_integrity_phase2::SourceIntegrity::Changed => {
                warning_codes.push(import_progress::ProgressWarningCode::SourceChanged)
            }
            import_integrity_phase2::SourceIntegrity::Passed => {}
        }
        if report.possible_duplicate_groups > 0 {
            warning_codes.push(import_progress::ProgressWarningCode::PossibleDuplicateGroups);
        }
    }
    emit_progress_observation(
        app,
        ProgressObservation {
            processed_units: Some(completed.min(VERIFICATION_CHECK_TOTAL)),
            total_units: Some(VERIFICATION_CHECK_TOTAL),
            unit_kind: Some(import_progress::ProgressUnitKind::VerificationChecks),
            activity_observed: completed > 0,
            counters: Some(import_progress::ProgressCounters {
                candidates_discovered: report.map(|value| value.discovered_candidates),
                canonical_messages: report.map(|value| value.canonical_messages),
                verification_checks_completed: Some(completed.min(VERIFICATION_CHECK_TOTAL)),
                verification_checks_total: Some(VERIFICATION_CHECK_TOTAL),
                missing_canonical_records: report.map(|value| value.missing_canonical_records),
                unexpected_canonical_records: report
                    .map(|value| value.unexpected_canonical_records),
                possible_duplicate_groups: report.map(|value| value.possible_duplicate_groups),
                ..import_progress::ProgressCounters::default()
            }),
            warning_codes,
            ..ProgressObservation::default()
        },
    );
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InsertMessageOutcome {
    Inserted {
        message_id: i64,
        attachment_count: usize,
    },
    Existing {
        message_id: i64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResumeIndexPreflight {
    Initialized,
    Pristine,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WalCheckpointResult {
    busy: i64,
    log_frames: i64,
    checkpointed_frames: i64,
}

pub fn run() {
    let context = tauri::generate_context!();
    let expected_identity = match RuntimeProfileIdentity::from_config(context.config()) {
        Ok(identity) => identity,
        Err(_) => {
            eprintln!("PST QuickView startup stopped: packaged application identity is invalid.");
            return;
        }
    };
    let app = tauri::Builder::default()
        .menu(|handle| {
            let about_metadata = AboutMetadata {
                name: Some(PRODUCT_DISPLAY_NAME.to_string()),
                version: Some(handle.package_info().version.to_string()),
                ..Default::default()
            };
            let window_menu = Submenu::with_items(
                handle,
                "Window",
                true,
                &[
                    &PredefinedMenuItem::minimize(handle, None)?,
                    &PredefinedMenuItem::maximize(handle, None)?,
                    &PredefinedMenuItem::separator(handle)?,
                    &PredefinedMenuItem::close_window(handle, None)?,
                ],
            )?;
            let help_menu = Submenu::with_items(handle, "Help", true, &[])?;

            Menu::with_items(
                handle,
                &[
                    &Submenu::with_items(
                        handle,
                        PRODUCT_DISPLAY_NAME,
                        true,
                        &[
                            &PredefinedMenuItem::about(
                                handle,
                                Some("About PST QuickView"),
                                Some(about_metadata),
                            )?,
                            &PredefinedMenuItem::separator(handle)?,
                            &PredefinedMenuItem::services(handle, None)?,
                            &PredefinedMenuItem::separator(handle)?,
                            &PredefinedMenuItem::hide(handle, Some("Hide PST QuickView"))?,
                            &PredefinedMenuItem::hide_others(handle, None)?,
                            &PredefinedMenuItem::separator(handle)?,
                            &PredefinedMenuItem::quit(handle, Some("Quit PST QuickView"))?,
                        ],
                    )?,
                    &Submenu::with_items(
                        handle,
                        "File",
                        true,
                        &[&PredefinedMenuItem::close_window(handle, None)?],
                    )?,
                    &Submenu::with_items(
                        handle,
                        "Edit",
                        true,
                        &[
                            &PredefinedMenuItem::undo(handle, None)?,
                            &PredefinedMenuItem::redo(handle, None)?,
                            &PredefinedMenuItem::separator(handle)?,
                            &PredefinedMenuItem::cut(handle, None)?,
                            &PredefinedMenuItem::copy(handle, None)?,
                            &PredefinedMenuItem::paste(handle, None)?,
                            &PredefinedMenuItem::select_all(handle, None)?,
                        ],
                    )?,
                    &Submenu::with_items(
                        handle,
                        "View",
                        true,
                        &[&PredefinedMenuItem::fullscreen(handle, None)?],
                    )?,
                    &window_menu,
                    &help_menu,
                ],
            )
        })
        .manage(AppState::default())
        .manage(support_session::SupportSessionState::default())
        .setup(move |app| -> Result<(), Box<dyn std::error::Error>> {
            let profile =
                RuntimeProfile::resolve(app, &expected_identity).map_err(AppError::from)?;
            let state = app.state::<AppState>();
            let profile = state.initialize_runtime_profile(profile)?;
            append_application_log(profile.paths(), "application", "started", None, None)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            check_readpst,
            get_app_diagnostics,
            support_session::start_support_session,
            support_session::request_support_session,
            support_session::support_service_status,
            support_session::bootstrap_support_session,
            support_session::publish_support_frame,
            support_session::poll_support_frame,
            support_session::support_submit_action,
            support_session::stop_support_session,
            support_session::support_session_status,
            reveal_application_logs,
            reveal_project_license,
            reveal_third_party_notices,
            pick_pst_file,
            pick_message_file,
            plan_pst_open,
            open_pst,
            open_existing_workspace_from_session,
            activate_workspace,
            close_workspace,
            cancel_import,
            cancel_search_operation,
            list_folders,
            read_local_trash_summary,
            move_message_to_local_trash,
            restore_message_from_local_trash,
            undo_local_trash_operation,
            reset_local_trash_state,
            list_messages,
            count_messages,
            search_messages_multi,
            count_messages_multi,
            list_conversations,
            count_conversations,
            get_conversation_messages,
            get_message,
            get_message_diagnostics,
            render_message_html,
            get_source_eml_view,
            get_standalone_message_view,
            frontend_ready_for_file_opens,
            open_message_preview_window,
            get_preview_window_target,
            reindex_existing_emls,
            verify_workspace_import,
            get_last_workspace_verification,
            resume_workspace_indexing,
            repair_workspace_index,
            rebuild_workspace_conversations,
            reveal_eml,
            save_source_eml_as,
            save_standalone_source_message_as,
            save_printable_html_as,
            reveal_saved_html,
            reveal_saved_eml,
            export_original_eml,
            export_attachment,
            open_attachment,
            export_standalone_message_attachment,
            open_standalone_message_attachment,
            reveal_exported_file,
            reveal_exported_file_for_workspace,
            reveal_standalone_exported_file,
            reveal_import_log,
            reveal_original_pst_in_finder,
            reveal_workspace_in_finder,
            reveal_original_and_workspace_in_finder,
            get_workspace_size,
            delete_workspace,
            delete_planned_workspace
        ])
        .build(context)
        .expect("error while building PST QuickView");

    app.run(|app_handle, event| {
        support_session::handle_run_event(app_handle, &event);
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Opened { urls } = event {
            let mut candidates = Vec::new();
            let mut warnings = Vec::new();
            for url in urls {
                match url.to_file_path() {
                    Ok(path) => candidates.push(path),
                    Err(()) => warnings.push(format!("Skipped unsupported non-file URL: {url}")),
                }
            }
            dispatch_external_file_open_batch(
                app_handle,
                build_external_file_open_batch(candidates, warnings),
            );
            return;
        }

        if let tauri::RunEvent::WebviewEvent { label, event, .. } = event {
            if label != "main" {
                return;
            }
            let tauri::WebviewEvent::DragDrop(drag_event) = event else {
                return;
            };
            match drag_event {
                tauri::DragDropEvent::Enter { .. } | tauri::DragDropEvent::Over { .. } => {
                    let _ = app_handle.emit_to("main", "external-file-drag-active", true);
                }
                tauri::DragDropEvent::Leave => {
                    let _ = app_handle.emit_to("main", "external-file-drag-active", false);
                }
                tauri::DragDropEvent::Drop { paths, .. } => {
                    let _ = app_handle.emit_to("main", "external-file-drag-active", false);
                    dispatch_external_file_open_batch(
                        app_handle,
                        build_external_file_open_batch(paths, Vec::new()),
                    );
                }
                _ => {}
            }
        }
    });
}

#[tauri::command]
fn check_readpst(state: State<AppState>) -> ReadpstStatus {
    let status = match find_readpst() {
        Some(location) => ReadpstStatus {
            available: true,
            version: Some(readpst_version(&location.path)),
            path: Some(location.path.display().to_string()),
            source: location.source.as_str().to_string(),
            source_label: location.source.label().to_string(),
            setup_command: String::new(),
        },
        None => ReadpstStatus {
            available: false,
            path: None,
            version: None,
            source: "missing".to_string(),
            source_label: "missing".to_string(),
            setup_command: MISSING_READPST_INSTRUCTIONS.to_string(),
        },
    };
    append_application_log_best_effort(
        &state,
        "readpst_check",
        if status.available {
            "available"
        } else {
            "missing"
        },
        None,
        None,
    );
    status
}

#[tauri::command]
fn get_app_diagnostics(state: State<AppState>) -> AppDiagnostics {
    let active = state
        .active_workspace
        .lock()
        .ok()
        .and_then(|guard| guard.clone());
    let open_pst_count = state
        .open_workspaces
        .lock()
        .map(|guard| guard.len())
        .unwrap_or_default();
    let readpst = find_readpst();
    let (readpst_source, readpst_version_text) = readpst
        .as_ref()
        .map(|location| {
            (
                location.source.label().to_string(),
                readpst_version(&location.path),
            )
        })
        .unwrap_or_else(|| ("missing".to_string(), "unavailable".to_string()));

    let (database_schema_version, conversation_data_status) = active
        .as_ref()
        .map(active_database_diagnostics)
        .unwrap_or((None, "no active workspace".to_string()));
    append_application_log_best_effort(
        &state,
        "diagnostics",
        "opened",
        active.as_ref().map(|workspace| workspace.id.as_str()),
        None,
    );

    AppDiagnostics {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        macos_version: current_macos_version(),
        cpu_architecture: current_cpu_architecture(),
        executable_architecture: current_executable_architecture(),
        readpst_source,
        readpst_version: readpst_version_text,
        open_pst_count,
        active_workspace_mode: active
            .as_ref()
            .map(|workspace| workspace.location_mode.label().to_string())
            .unwrap_or_else(|| "No active workspace".to_string()),
        active_workspace_path: active
            .as_ref()
            .map(|workspace| workspace.path.display().to_string())
            .unwrap_or_default(),
        database_schema_version,
        conversation_data_status,
    }
}

#[tauri::command]
fn reveal_application_logs(state: State<AppState>) -> AppResult<()> {
    let profile_paths = state.runtime_profile()?.paths();
    let logs_dir = application_logs_dir(profile_paths);
    create_private_directory_all(logs_dir)?;
    append_application_log_best_effort(&state, "application_log", "revealed", None, None);
    reveal_path(logs_dir)
}

#[tauri::command]
fn reveal_project_license(app: tauri::AppHandle) -> AppResult<()> {
    reveal_packaged_resource(&app, PROJECT_LICENSE_RESOURCE, "project license")
}

#[tauri::command]
fn reveal_third_party_notices(app: tauri::AppHandle) -> AppResult<()> {
    reveal_packaged_resource(&app, THIRD_PARTY_NOTICES_RESOURCE, "third-party notices")
}

fn reveal_packaged_resource(
    app: &tauri::AppHandle,
    relative_path: &str,
    label: &str,
) -> AppResult<()> {
    let resource_dir = app
        .path()
        .resource_dir()
        .map_err(|error| AppError::new(format!("Could not locate packaged {label}: {error}")))?;
    let resource_path = resource_dir.join(relative_path);
    if !resource_path.is_file() {
        return Err(AppError::new(format!(
            "Packaged {label} was not found: {}",
            resource_path.display()
        )));
    }
    reveal_path(&resource_path)
}

#[tauri::command]
fn pick_pst_file() -> Option<String> {
    rfd::FileDialog::new()
        .set_title("Open PST")
        .add_filter("Outlook PST", &["pst"])
        .pick_file()
        .map(|path| path.display().to_string())
}

#[tauri::command]
fn pick_message_file(state: State<AppState>) -> AppResult<Option<ExternalFileOpen>> {
    let Some(path) = rfd::FileDialog::new()
        .set_title("Open Message")
        .add_filter("Email Message", &["eml", "msg"])
        .pick_file()
    else {
        return Ok(None);
    };
    let file = prepare_external_file_open(path)?;
    let mut external = state
        .external_file_opens
        .lock()
        .map_err(|_| AppError::new("External file-open state lock was poisoned."))?;
    authorize_standalone_file(&mut external, &file)?;
    Ok(Some(file))
}

#[tauri::command]
fn frontend_ready_for_file_opens(state: State<AppState>) -> AppResult<ExternalFileOpenReady> {
    let mut external = state
        .external_file_opens
        .lock()
        .map_err(|_| AppError::new("External file-open state lock was poisoned."))?;
    external.frontend_ready = true;
    Ok(ExternalFileOpenReady {
        batches: external.pending.drain(..).collect(),
        external_open_received: external.external_open_received,
    })
}

#[tauri::command]
async fn open_message_preview_window(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    request: PreviewWindowRequest,
) -> AppResult<PreviewWindowOpenResult> {
    if window.label() != "main" {
        return Err(AppError::new(
            "Only the main application window can open a message preview.",
        ));
    }

    let target = match request {
        PreviewWindowRequest::Workspace {
            workspace_id,
            message_id,
        } => {
            validate_workspace_identity_input(&workspace_id)?;
            if message_id <= 0 {
                return Err(AppError::new("Invalid message preview identity."));
            }
            resolve_source_eml(&state, &workspace_id, message_id)?;
            PreviewWindowTarget::Workspace {
                workspace_id,
                message_id,
            }
        }
        PreviewWindowRequest::Standalone { stable_id } => {
            authorized_standalone_path(&state, &stable_id)?;
            PreviewWindowTarget::Standalone { stable_id }
        }
    };

    let label = {
        let mut registry = state
            .preview_windows
            .lock()
            .map_err(|_| AppError::new("Message preview state lock was poisoned."))?;
        registry.register(target)?
    };

    if let Some(existing) = app.get_webview_window(&label) {
        focus_preview_window(&existing)?;
        return Ok(PreviewWindowOpenResult { created: false });
    }

    let build_result = tauri::WebviewWindowBuilder::new(
        &app,
        label.clone(),
        tauri::WebviewUrl::App(PathBuf::from(PREVIEW_WINDOW_ROUTE)),
    )
    .title("Message Preview")
    .inner_size(900.0, 700.0)
    .min_inner_size(720.0, 520.0)
    .resizable(true)
    .focused(true)
    .build();

    let preview = match build_result {
        Ok(preview) => preview,
        Err(error) => {
            if let Ok(mut registry) = state.preview_windows.lock() {
                registry.remove(&label);
            }
            return Err(AppError::new(format!(
                "Could not create the local message preview window: {error}"
            )));
        }
    };

    let cleanup_app = app.clone();
    let cleanup_label = label.clone();
    preview.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            let state = cleanup_app.state::<AppState>();
            if let Ok(mut registry) = state.preview_windows.lock() {
                registry.remove(&cleanup_label);
            };
        }
    });
    focus_preview_window(&preview)?;
    Ok(PreviewWindowOpenResult { created: true })
}

#[tauri::command]
fn get_preview_window_target(
    window: tauri::WebviewWindow,
    state: State<AppState>,
) -> AppResult<PreviewWindowTarget> {
    let label = window.label();
    if !is_valid_preview_window_label(label) {
        return Err(AppError::new("Invalid message preview window identity."));
    }
    state
        .preview_windows
        .lock()
        .map_err(|_| AppError::new("Message preview state lock was poisoned."))?
        .target(label)
        .ok_or_else(|| AppError::new("Message preview authorization is unavailable."))
}

fn focus_preview_window(window: &tauri::WebviewWindow) -> AppResult<()> {
    window
        .unminimize()
        .map_err(|_| AppError::new("Could not restore the message preview window."))?;
    window
        .show()
        .map_err(|_| AppError::new("Could not show the message preview window."))?;
    window
        .set_focus()
        .map_err(|_| AppError::new("Could not focus the message preview window."))?;
    Ok(())
}

#[tauri::command]
fn plan_pst_open(
    state: State<AppState>,
    path: String,
    workspace_location_mode: String,
) -> AppResult<PstOpenPlan> {
    let profile_paths = state.runtime_profile()?.paths();
    let mode = WorkspaceLocationMode::from_arg(&workspace_location_mode)?;
    let identity = identify_pst(&PathBuf::from(path))?;
    let selection = select_workspace(profile_paths, &identity, mode)?;
    let preflight = workspace_preflight(&identity, &selection);
    let mut existing_workspaces = find_existing_workspaces(profile_paths, &identity)?
        .into_iter()
        .map(|candidate| existing_workspace_summary(&candidate))
        .collect::<AppResult<Vec<_>>>()?;
    sort_existing_workspace_summaries(&mut existing_workspaces, &selection.path);

    Ok(PstOpenPlan {
        pst_path: identity.display_path,
        workspace_id: identity.workspace_id,
        fingerprint: identity.content_fingerprint,
        selected_workspace_path: selection.path.display().to_string(),
        selected_workspace_location_mode: selection.mode.as_str().to_string(),
        selected_workspace_location_label: selection.mode.label().to_string(),
        fallback_warning: selection.warning,
        preflight,
        existing_workspaces,
    })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn open_pst(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
    workspace_location_mode: String,
    existing_workspace_path: Option<String>,
    allow_duplicate: bool,
    import_action: Option<String>,
    operation_id: String,
) -> AppResult<WorkspaceSummary> {
    import_integrity_phase2::validate_operation_id(&operation_id)?;
    let _profile = state.runtime_profile()?;
    let mode = WorkspaceLocationMode::from_arg(&workspace_location_mode)?;
    let action = ImportAction::from_arg(import_action)?;
    if state.import_running.swap(true, Ordering::SeqCst) {
        return Err(AppError::new(
            "An import or indexing task is already running. Wait for it to finish before opening another PST.",
        ));
    }
    state.cancel_import_requested.store(false, Ordering::SeqCst);

    let result = tauri::async_runtime::spawn_blocking(move || {
        open_pst_blocking(
            app,
            PathBuf::from(path),
            mode,
            existing_workspace_path.map(PathBuf::from),
            allow_duplicate,
            action,
            operation_id,
        )
    })
    .await
    .map_err(|error| AppError::new(format!("Import worker failed: {error}")));

    state.import_running.store(false, Ordering::SeqCst);
    state.cancel_import_requested.store(false, Ordering::SeqCst);
    if let Ok(mut pid) = state.readpst_pid.lock() {
        *pid = None;
    }
    let result = result?;
    if let Ok(summary) = &result {
        set_active_workspace(&state, summary)?;
    }
    result
}

#[tauri::command]
fn open_existing_workspace_from_session(
    state: State<AppState>,
    pst_path: String,
    workspace_path: String,
    workspace_id: String,
    workspace_location_mode: String,
) -> AppResult<WorkspaceSummary> {
    let location_mode = WorkspaceLocationMode::from_arg(&workspace_location_mode)?;
    let pst_path = PathBuf::from(pst_path);
    let workspace_path = PathBuf::from(workspace_path);
    validate_session_workspace_scope_before_access(
        state.runtime_profile()?.paths(),
        &pst_path,
        &workspace_path,
        &workspace_id,
        location_mode,
    )?;
    let pst_path = pst_path.canonicalize().unwrap_or(pst_path);
    let workspace_path = workspace_path
        .canonicalize()
        .map_err(|_| AppError::new("PST or workspace not available."))?;

    validate_session_workspace_path(
        state.runtime_profile()?.paths(),
        &pst_path,
        &workspace_path,
        &workspace_id,
        location_mode,
    )?;

    let conn = open_workspace_db_for_upgrade(&workspace_path)?;
    let integrity = import_integrity_phase2::integrity_snapshot(
        &workspace_path,
        &workspace_id,
        Some(&pst_path),
    );
    if !integrity.can_open {
        return Err(AppError::new("Workspace index is not usable."));
    }

    let fingerprint = metadata_value(&conn, "pst_content_fingerprint")?
        .or_else(|| metadata_value(&conn, "pst_fingerprint").ok().flatten())
        .unwrap_or_else(|| workspace_id.clone());
    drop(conn);

    let active = ActiveWorkspace {
        id: workspace_id,
        path: workspace_path,
        pst_path,
        fingerprint,
        location_mode,
    };

    remember_open_workspace(state.inner(), &active)?;

    {
        let mut active_guard = state
            .active_workspace
            .lock()
            .map_err(|_| AppError::new("Could not lock active workspace state."))?;
        *active_guard = Some(active.clone());
    }

    workspace_summary_from_active(&active, true)
}

#[tauri::command]
fn activate_workspace(state: State<AppState>, workspace_id: String) -> AppResult<WorkspaceSummary> {
    let active = active_workspace_for_id(&state, &workspace_id)?;
    {
        let mut guard = state
            .active_workspace
            .lock()
            .map_err(|_| AppError::new("Could not lock active workspace state."))?;
        *guard = Some(active.clone());
    }
    workspace_summary_from_active(&active, true)
}

#[tauri::command]
fn close_workspace(
    state: State<AppState>,
    workspace_id: String,
) -> AppResult<Option<WorkspaceSummary>> {
    let replacement = {
        let mut open_guard = state
            .open_workspaces
            .lock()
            .map_err(|_| AppError::new("Could not lock open workspace state."))?;
        open_guard.remove(&workspace_id);
        open_guard.values().next().cloned()
    };

    let next_active = {
        let mut active_guard = state
            .active_workspace
            .lock()
            .map_err(|_| AppError::new("Could not lock active workspace state."))?;
        if active_guard
            .as_ref()
            .is_some_and(|active| active.id == workspace_id)
        {
            *active_guard = replacement;
            active_guard.clone()
        } else {
            active_guard.clone()
        }
    };

    next_active
        .as_ref()
        .map(|active| workspace_summary_from_active(active, true))
        .transpose()
}

#[tauri::command]
async fn reindex_existing_emls(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
    operation_id: String,
) -> AppResult<WorkspaceSummary> {
    import_integrity_phase2::validate_operation_id(&operation_id)?;
    if state.import_running.swap(true, Ordering::SeqCst) {
        return Err(AppError::new(
            "An import or indexing task is already running. Wait for it to finish before reindexing.",
        ));
    }
    state.cancel_import_requested.store(false, Ordering::SeqCst);

    let active = match workspace_for_integrity_operation(&state, &workspace_id) {
        Ok(active) => active,
        Err(error) => {
            state.import_running.store(false, Ordering::SeqCst);
            state.cancel_import_requested.store(false, Ordering::SeqCst);
            return Err(error);
        }
    };

    let result = tauri::async_runtime::spawn_blocking(move || {
        reindex_existing_emls_blocking(&app, active, Some(&operation_id))
    })
    .await
    .map_err(|error| AppError::new(format!("Reindex worker failed: {error}")));

    state.import_running.store(false, Ordering::SeqCst);
    state.cancel_import_requested.store(false, Ordering::SeqCst);
    if let Ok(mut pid) = state.readpst_pid.lock() {
        *pid = None;
    }
    let result = result?;
    if let Ok(summary) = &result {
        set_active_workspace(&state, summary)?;
    }
    result
}

#[tauri::command]
async fn verify_workspace_import(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
    operation_id: String,
    verification_level: String,
) -> AppResult<import_integrity_phase2::VerificationReport> {
    import_integrity_phase2::validate_operation_id(&operation_id)?;
    let level = import_integrity_phase2::VerificationLevel::from_arg(&verification_level)?;
    if state.import_running.swap(true, Ordering::SeqCst) {
        return Err(AppError::new(
            "A workspace import, verification, or repair operation is already running.",
        ));
    }
    state.cancel_import_requested.store(false, Ordering::SeqCst);
    let active = match workspace_for_integrity_operation(&state, &workspace_id) {
        Ok(active) => active,
        Err(error) => {
            state.import_running.store(false, Ordering::SeqCst);
            return Err(error);
        }
    };
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        recover_phase2_workspace_state(&active)?;
        let mut progress_guard = IntegrityOperationFinishGuard::begin(
            &worker_app,
            &active,
            &operation_id,
            import_progress::OperationKind::Verification,
            import_progress::ProgressStage::Verifying,
        )?;
        let operation_result = (|| {
            let verification_start_state =
                import_integrity_phase2::read_state(&active.path, &active.id)?
                    .ok_or_else(|| AppError::new("Workspace verification state is unavailable."))?;
            emit_progress(
                &worker_app,
                "Verifying",
                None,
                None,
                "Verifying internal workspace integrity.",
            );
            let report = match import_integrity_phase2::verify_active_workspace(
                &worker_app,
                &active,
                &operation_id,
                level,
            ) {
                Ok(report) => report,
                Err(error) => {
                    let cancelled = error.code == Some(IMPORT_CANCELLED_CODE);
                    return combine_integrity_operation_result(
                        Err(error),
                        terminalize_standalone_verification_error(
                            &active,
                            &verification_start_state,
                            cancelled,
                        ),
                    );
                }
            };
            if report.workspace_state != import_integrity_phase2::WorkspaceState::Complete
                || !matches!(
                    report.result,
                    import_integrity_phase2::VerificationResult::Verified
                        | import_integrity_phase2::VerificationResult::SourceUnavailable
                        | import_integrity_phase2::VerificationResult::SourceChanged
                )
            {
                set_integrity_operation_terminal_status_override(
                    &worker_app,
                    &operation_id,
                    import_progress::ProgressStatus::Incomplete,
                )?;
            }
            Ok(report)
        })();
        progress_guard.finish_with_result(operation_result)
    })
    .await
    .map_err(|error| AppError::new(format!("Verification worker failed: {error}")));
    state.import_running.store(false, Ordering::SeqCst);
    state.cancel_import_requested.store(false, Ordering::SeqCst);
    if let Ok(mut pid) = state.readpst_pid.lock() {
        *pid = None;
    }
    result?
}

fn terminalize_standalone_verification_error(
    active: &ActiveWorkspace,
    verification_start_state: &import_integrity_phase2::WorkspaceImportState,
    cancelled: bool,
) -> AppResult<()> {
    let current = import_integrity_phase2::read_state(&active.path, &active.id)?
        .ok_or_else(|| AppError::new("Workspace verification state disappeared."))?;
    if current.state != import_integrity_phase2::WorkspaceState::Verifying {
        return Ok(());
    }
    let (terminal_state, reason) =
        if import_integrity_phase2::state_has_complete_lineage(verification_start_state) {
            (
                import_integrity_phase2::WorkspaceState::Complete,
                if cancelled {
                    "verification_cancelled_prior_complete_preserved"
                } else {
                    "verification_failed_prior_complete_preserved"
                },
            )
        } else if cancelled {
            (
                import_integrity_phase2::WorkspaceState::Cancelled,
                "verification_cancelled",
            )
        } else {
            (
                import_integrity_phase2::WorkspaceState::Failed,
                "verification_failed",
            )
        };
    import_integrity_phase2::transition_state(
        &active.path,
        &active.id,
        terminal_state,
        import_integrity_phase2::StateCounts {
            discovered_candidates: usize::try_from(current.discovered_candidates).ok(),
            canonical_indexed: usize::try_from(current.canonical_indexed).ok(),
        },
        Some(reason),
    )?;
    Ok(())
}

#[tauri::command]
fn get_last_workspace_verification(
    state: State<AppState>,
    workspace_id: String,
) -> AppResult<Option<import_integrity_phase2::VerificationReport>> {
    let active = workspace_for_integrity_operation(&state, &workspace_id)?;
    import_integrity_phase2::get_last_verification(&active.path, &active.id)
}

#[tauri::command]
async fn resume_workspace_indexing(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
    operation_id: String,
) -> AppResult<WorkspaceSummary> {
    import_integrity_phase2::validate_operation_id(&operation_id)?;
    if state.import_running.swap(true, Ordering::SeqCst) {
        return Err(AppError::new(
            "A workspace import, verification, or repair operation is already running.",
        ));
    }
    state.cancel_import_requested.store(false, Ordering::SeqCst);
    let active = match workspace_for_integrity_operation(&state, &workspace_id) {
        Ok(active) => active,
        Err(error) => {
            state.import_running.store(false, Ordering::SeqCst);
            return Err(error);
        }
    };
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        resume_active_workspace_indexing_blocking(&worker_app, active, &operation_id)
    })
    .await
    .map_err(|error| AppError::new(format!("Resume indexing worker failed: {error}")));
    state.import_running.store(false, Ordering::SeqCst);
    state.cancel_import_requested.store(false, Ordering::SeqCst);
    if let Ok(mut pid) = state.readpst_pid.lock() {
        *pid = None;
    }
    result?
}

#[tauri::command]
async fn repair_workspace_index(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
    operation_id: String,
) -> AppResult<WorkspaceSummary> {
    import_integrity_phase2::validate_operation_id(&operation_id)?;
    if state.import_running.swap(true, Ordering::SeqCst) {
        return Err(AppError::new(
            "A workspace import, verification, or repair operation is already running.",
        ));
    }
    state.cancel_import_requested.store(false, Ordering::SeqCst);
    let active = match workspace_for_integrity_operation(&state, &workspace_id) {
        Ok(active) => active,
        Err(error) => {
            state.import_running.store(false, Ordering::SeqCst);
            return Err(error);
        }
    };
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        reindex_existing_emls_blocking(&worker_app, active, Some(&operation_id))
    })
    .await
    .map_err(|error| AppError::new(format!("Repair index worker failed: {error}")));
    state.import_running.store(false, Ordering::SeqCst);
    state.cancel_import_requested.store(false, Ordering::SeqCst);
    if let Ok(mut pid) = state.readpst_pid.lock() {
        *pid = None;
    }
    result?
}

#[tauri::command]
async fn rebuild_workspace_conversations(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
    operation_id: String,
) -> AppResult<WorkspaceSummary> {
    import_integrity_phase2::validate_operation_id(&operation_id)?;
    if state.import_running.swap(true, Ordering::SeqCst) {
        return Err(AppError::new(
            "A workspace import, verification, or repair operation is already running.",
        ));
    }
    state.cancel_import_requested.store(false, Ordering::SeqCst);
    let active = match workspace_for_integrity_operation(&state, &workspace_id) {
        Ok(active) => active,
        Err(error) => {
            state.import_running.store(false, Ordering::SeqCst);
            return Err(error);
        }
    };
    let worker_app = app.clone();
    let result = tauri::async_runtime::spawn_blocking(move || {
        rebuild_workspace_conversations_blocking(&worker_app, active, &operation_id)
    })
    .await
    .map_err(|error| AppError::new(format!("Conversation rebuild worker failed: {error}")));
    state.import_running.store(false, Ordering::SeqCst);
    state.cancel_import_requested.store(false, Ordering::SeqCst);
    if let Ok(mut pid) = state.readpst_pid.lock() {
        *pid = None;
    }
    result?
}

#[tauri::command]
fn cancel_import(
    app: tauri::AppHandle,
    state: State<AppState>,
    workspace_id: String,
    operation_id: String,
    generation: u64,
) -> AppResult<CancelImportResult> {
    validate_workspace_identity_input(&workspace_id)?;
    import_integrity_phase2::validate_operation_id(&operation_id)?;
    if generation == 0 || generation > import_progress::MAX_SAFE_JSON_INTEGER {
        return Err(AppError::new("Invalid workspace operation generation."));
    }
    if !state.import_running.load(Ordering::SeqCst) {
        return Err(AppError::new(
            "The requested import operation is no longer active.",
        ));
    }
    let matches_active_scope = state
        .integrity_operation
        .lock()
        .map_err(|_| AppError::new("Workspace operation scope is unavailable."))?
        .as_ref()
        .is_some_and(|scope| {
            cancellation_token_matches(
                &scope.workspace_id,
                &scope.operation_id,
                scope.generation,
                scope.status,
                &workspace_id,
                &operation_id,
                generation,
            )
        });
    if !matches_active_scope {
        return Err(AppError::new(
            "Stale import cancellation request was rejected.",
        ));
    }

    state.cancel_import_requested.store(true, Ordering::SeqCst);
    emit_progress_observation(
        &app,
        ProgressObservation {
            activity_observed: true,
            process_state: Some(import_progress::ProcessState::CancellationRequested),
            status_code: Some(import_progress::ProgressStatusCode::CancellationRequested),
            ..ProgressObservation::default()
        },
    );
    let pid = *state
        .readpst_pid
        .lock()
        .map_err(|_| AppError::new("Could not lock readpst process state."))?;

    if let Some(pid) = pid {
        terminate_process_group(pid, false)?;
    }

    Ok(CancelImportResult {
        requested: true,
        pid,
        message: match pid {
            Some(pid) => {
                format!("Cancel requested. Sent terminate signal to ReadPST process group {pid}.")
            }
            None => "Cancel requested. The local operation will stop at the next safe checkpoint."
                .to_string(),
        },
    })
}

#[allow(clippy::too_many_arguments)]
fn cancellation_token_matches(
    active_workspace_id: &str,
    active_operation_id: &str,
    active_generation: u64,
    active_status: import_progress::ProgressStatus,
    requested_workspace_id: &str,
    requested_operation_id: &str,
    requested_generation: u64,
) -> bool {
    active_status == import_progress::ProgressStatus::Active
        && active_workspace_id == requested_workspace_id
        && active_operation_id == requested_operation_id
        && active_generation == requested_generation
}

#[tauri::command]
fn cancel_search_operation(
    window: tauri::WebviewWindow,
    state: State<AppState>,
    search_generation: u64,
    search_operation_id: Option<String>,
) -> AppResult<CancelSearchResult> {
    let outcome = if let Some(operation_id) = search_operation_id {
        state.search_cancellations.cancel_operation(
            window.label(),
            search_generation,
            &operation_id,
        )?
    } else {
        state
            .search_cancellations
            .cancel_generation(window.label(), search_generation)?
    };
    Ok(CancelSearchResult {
        cancelled_operations: outcome.operations,
        interrupted_connections: outcome.handles,
    })
}

#[tauri::command]
fn list_folders(state: State<AppState>, workspace_id: String) -> AppResult<Vec<Folder>> {
    let active = active_workspace_for_id(&state, &workspace_id)?;
    let conn = open_workspace_db_for_read(&active.path)?;
    query_folders_with_local_trash(&conn, &active)
}

fn query_folders_with_local_trash(
    conn: &Connection,
    active: &ActiveWorkspace,
) -> AppResult<Vec<Folder>> {
    let scope = local_trash_scope_for_active(active)?;
    local_trash::materialize_for_query(conn, &active.path, &scope)?;
    let trashed_predicate = local_trash_membership_predicate("m", &scope, true)?;

    let mut statement = conn.prepare(&format!(
        "SELECT f.id,
                f.parent_id,
                f.path,
                f.name,
                COUNT(m.id) AS direct_message_count,
                SUM(CASE WHEN m.id IS NOT NULL AND {trashed_predicate} THEN 1 ELSE 0 END)
                    AS local_trash_direct_message_count
           FROM folders f
           LEFT JOIN messages m ON m.folder_id = f.id
          GROUP BY f.id, f.parent_id, f.path, f.name
          ORDER BY f.path"
    ))?;

    let rows = statement.query_map([], |row| {
        Ok(Folder {
            id: row.get(0)?,
            parent_id: row.get(1)?,
            path: row.get(2)?,
            name: row.get(3)?,
            message_count: row.get(4)?,
            direct_message_count: row.get(4)?,
            active_message_count: row.get::<_, i64>(4)? - row.get::<_, i64>(5)?,
            active_direct_message_count: row.get::<_, i64>(4)? - row.get::<_, i64>(5)?,
            local_trash_message_count: 0,
            local_trash_direct_message_count: row.get(5)?,
        })
    })?;

    let mut folders = collect_rows(rows)?;
    roll_up_folder_local_trash_counts(&mut folders)?;
    Ok(folders)
}

fn roll_up_folder_local_trash_counts(folders: &mut [Folder]) -> AppResult<()> {
    let mut direct_counts = folders
        .iter()
        .map(|folder| {
            (
                folder.path.clone(),
                folder.direct_message_count,
                folder.active_direct_message_count,
                folder.local_trash_direct_message_count,
            )
        })
        .collect::<Vec<_>>();
    direct_counts.sort_by(|left, right| left.0.cmp(&right.0));

    let mut prefix_counts = Vec::with_capacity(direct_counts.len() + 1);
    prefix_counts.push((0_i64, 0_i64, 0_i64));
    for (_, canonical, active, trashed) in &direct_counts {
        if *canonical < 0 || *active < 0 || *trashed < 0 {
            return Err(AppError::new("Folder message counts are invalid."));
        }
        let previous = *prefix_counts
            .last()
            .ok_or_else(|| AppError::new("Folder message counts are unavailable."))?;
        prefix_counts.push((
            previous.0.checked_add(*canonical).ok_or_else(|| {
                AppError::new("Folder message counts exceed the supported range.")
            })?,
            previous.1.checked_add(*active).ok_or_else(|| {
                AppError::new("Folder message counts exceed the supported range.")
            })?,
            previous.2.checked_add(*trashed).ok_or_else(|| {
                AppError::new("Folder message counts exceed the supported range.")
            })?,
        ));
    }

    for folder in folders {
        let descendant_counts = if folder.path.is_empty() {
            *prefix_counts
                .last()
                .ok_or_else(|| AppError::new("Folder message counts are unavailable."))?
        } else {
            // Descendant paths form the bounded lexical range `path/` through
            // (but excluding) `path0`; the exact folder's direct count is added
            // separately. This avoids an all-folder scan for every folder.
            let lower = format!("{}/", folder.path);
            let upper = format!("{}0", folder.path);
            let start = direct_counts.partition_point(|(path, _, _, _)| path < &lower);
            let end = direct_counts.partition_point(|(path, _, _, _)| path < &upper);
            let before = prefix_counts[start];
            let after = prefix_counts[end];
            (
                folder
                    .direct_message_count
                    .checked_add(after.0 - before.0)
                    .ok_or_else(|| {
                        AppError::new("Folder message counts exceed the supported range.")
                    })?,
                folder
                    .active_direct_message_count
                    .checked_add(after.1 - before.1)
                    .ok_or_else(|| {
                        AppError::new("Folder message counts exceed the supported range.")
                    })?,
                folder
                    .local_trash_direct_message_count
                    .checked_add(after.2 - before.2)
                    .ok_or_else(|| {
                        AppError::new("Folder message counts exceed the supported range.")
                    })?,
            )
        };
        folder.message_count = descendant_counts.0;
        folder.active_message_count = descendant_counts.1;
        folder.local_trash_message_count = descendant_counts.2;
    }
    Ok(())
}

#[tauri::command]
fn read_local_trash_summary(
    state: State<AppState>,
    workspace_id: String,
) -> AppResult<LocalTrashSummary> {
    let active = active_workspace_for_id(&state, &workspace_id)?;
    let scope = local_trash_scope_for_active(&active)?;
    let conn = open_workspace_db_for_read(&active.path)?;
    let local_state = local_trash::materialize_for_query(&conn, &active.path, &scope)?;
    let trashed_predicate = local_trash_membership_predicate("m", &scope, true)?;
    let trashed_message_count: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM messages m WHERE {trashed_predicate}"),
        [],
        |row| row.get(0),
    )?;
    let stored_count = i64::try_from(local_state.stored_count)
        .map_err(|_| AppError::new("Local Trash count exceeds the supported range."))?;
    let orphaned_state_count = stored_count.saturating_sub(trashed_message_count);

    Ok(LocalTrashSummary {
        workspace_id: active.id,
        available: local_state.available,
        can_reset: local_state.can_reset,
        sidecar_present: local_state.sidecar_present,
        trashed_message_count,
        orphaned_state_count,
        schema_version: local_state.schema_version,
        state_revision: local_state.revision,
        last_changed_at: local_state
            .last_changed_at_ms
            .and_then(local_trash_timestamp),
        fingerprint_match: local_state.fingerprint_match.unwrap_or(false),
        last_error_code: local_state.last_error_code.map(str::to_string),
    })
}

fn local_trash_timestamp(timestamp_ms: i64) -> Option<String> {
    DateTime::<Utc>::from_timestamp_millis(timestamp_ms)
        .map(|value| value.to_rfc3339_opts(SecondsFormat::Millis, true))
}

fn local_trash_query_health(
    workspace_id: &str,
    state: &local_trash::LocalTrashQueryState,
) -> LocalTrashQueryHealth {
    LocalTrashQueryHealth {
        workspace_id: workspace_id.to_string(),
        available: state.available,
        can_reset: state.can_reset,
        sidecar_present: state.sidecar_present,
        schema_version: state.schema_version,
        state_revision: state.revision,
        fingerprint_match: state.fingerprint_match.unwrap_or(false),
        last_error_code: state.last_error_code.map(str::to_string),
    }
}

fn canonical_message_identity(conn: &Connection, message_id: i64) -> AppResult<String> {
    if !has_unique_message_source_identity(conn).unwrap_or(false) {
        return Err(AppError::coded(
            "Local Trash is unavailable because canonical message identity is ambiguous.",
            local_trash::ERROR_CANONICAL_IDENTITY_AMBIGUOUS,
        ));
    }
    let (identity, message_id_count) = conn.query_row(
        "SELECT MIN(eml_path), COUNT(*) FROM messages WHERE id = ?1",
        params![message_id],
        |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?)),
    )?;
    if message_id_count == 0 {
        return Err(AppError::new("Message was not found in this workspace."));
    }
    if message_id_count != 1 {
        return Err(AppError::coded(
            "Local Trash is unavailable because canonical message identity is ambiguous.",
            local_trash::ERROR_CANONICAL_IDENTITY_AMBIGUOUS,
        ));
    }
    let identity = identity.ok_or_else(|| {
        AppError::coded(
            "Local Trash is unavailable because canonical message identity is ambiguous.",
            local_trash::ERROR_CANONICAL_IDENTITY_AMBIGUOUS,
        )
    })?;
    let identity_count = conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE eml_path = ?1",
        [&identity],
        |row| row.get::<_, i64>(0),
    )?;
    if identity_count != 1 {
        return Err(AppError::coded(
            "Local Trash is unavailable because canonical message identity is ambiguous.",
            local_trash::ERROR_CANONICAL_IDENTITY_AMBIGUOUS,
        ));
    }
    Ok(identity)
}

fn local_trash_message_state(
    conn: &Connection,
    active: &ActiveWorkspace,
    canonical_identity: &str,
) -> AppResult<(bool, bool, Option<String>)> {
    let scope = local_trash_scope_for_active(active)?;
    let local_state = local_trash::materialize_for_query(conn, &active.path, &scope)?;
    let in_local_trash = conn.query_row(
        &format!(
            "SELECT EXISTS (
                 SELECT 1 FROM temp.{} lt
                  WHERE lt.workspace_fingerprint = ?1
                    AND lt.source_fingerprint = ?2
                    AND lt.canonical_identity_version = ?3
                    AND lt.canonical_identity = ?4
             )",
            local_trash::LOCAL_TRASH_TEMP_TABLE,
        ),
        params![
            scope.workspace_fingerprint(),
            scope.source_fingerprint(),
            local_trash::LOCAL_TRASH_IDENTITY_VERSION,
            canonical_identity,
        ],
        |row| row.get::<_, i64>(0),
    )? != 0;
    Ok((
        in_local_trash,
        local_state.available,
        local_state.last_error_code.map(str::to_string),
    ))
}

fn local_trash_mutation_result(
    workspace_id: String,
    result: &'static str,
    mutation: &local_trash::LocalTrashMutationResult,
    expose_operation_id: bool,
) -> LocalTrashMutationResult {
    LocalTrashMutationResult {
        workspace_id,
        result,
        changed_count: mutation.changed_count,
        state_revision: mutation.revision,
        operation_id: expose_operation_id.then(|| mutation.operation_id.clone()),
    }
}

#[tauri::command]
fn move_message_to_local_trash(
    state: State<AppState>,
    workspace_id: String,
    message_id: i64,
    expected_revision: u64,
) -> AppResult<LocalTrashMutationResult> {
    let active = active_workspace_for_id(&state, &workspace_id)?;
    let conn = open_workspace_db_for_read(&active.path)?;
    let identity = canonical_message_identity(&conn, message_id)?;
    drop(conn);
    let scope = local_trash_scope_for_active(&active)?;
    let operation_id = local_trash::new_operation_id()?;
    let mut undo = state
        .local_trash_undo
        .lock()
        .map_err(|_| AppError::new("Could not lock Local Trash Undo state."))?;
    state
        .local_trash_operation_sequence
        .fetch_add(1, Ordering::SeqCst);
    let mutation_result = local_trash::move_messages(
        &active.path,
        &scope,
        expected_revision,
        &operation_id,
        &[identity],
    );
    let mutation = match mutation_result {
        Ok(mutation) => mutation,
        Err(error) => {
            if error.code == Some(local_trash::ERROR_OPERATION_INDETERMINATE) {
                *undo = None;
            }
            return Err(error);
        }
    };
    let changed = mutation.changed_count > 0;
    if changed {
        *undo = Some(LocalTrashUndoRecord {
            workspace_id: active.id.clone(),
            operation_id: operation_id.clone(),
            state_revision: mutation.revision,
        });
    }
    Ok(local_trash_mutation_result(
        active.id,
        if changed { "moved" } else { "already_trashed" },
        &mutation,
        changed,
    ))
}

#[tauri::command]
fn restore_message_from_local_trash(
    state: State<AppState>,
    workspace_id: String,
    message_id: i64,
    expected_revision: u64,
) -> AppResult<LocalTrashMutationResult> {
    let active = active_workspace_for_id(&state, &workspace_id)?;
    let conn = open_workspace_db_for_read(&active.path)?;
    let identity = canonical_message_identity(&conn, message_id)?;
    drop(conn);
    let scope = local_trash_scope_for_active(&active)?;
    let operation_id = local_trash::new_operation_id()?;
    let mut undo = state
        .local_trash_undo
        .lock()
        .map_err(|_| AppError::new("Could not lock Local Trash Undo state."))?;
    state
        .local_trash_operation_sequence
        .fetch_add(1, Ordering::SeqCst);
    let mutation_result = local_trash::restore_messages(
        &active.path,
        &scope,
        expected_revision,
        &operation_id,
        &[identity],
    );
    let mutation = match mutation_result {
        Ok(mutation) => mutation,
        Err(error) => {
            if error.code == Some(local_trash::ERROR_OPERATION_INDETERMINATE) {
                *undo = None;
            }
            return Err(error);
        }
    };
    let changed = mutation.changed_count > 0;
    if changed {
        *undo = None;
    }
    Ok(local_trash_mutation_result(
        active.id,
        if changed {
            "restored"
        } else {
            "already_active"
        },
        &mutation,
        false,
    ))
}

#[tauri::command]
fn undo_local_trash_operation(
    state: State<AppState>,
    workspace_id: String,
    operation_id: String,
    expected_revision: u64,
) -> AppResult<LocalTrashMutationResult> {
    let active = active_workspace_for_id(&state, &workspace_id)?;
    let scope = local_trash_scope_for_active(&active)?;
    let mut undo = state
        .local_trash_undo
        .lock()
        .map_err(|_| AppError::new("Could not lock Local Trash Undo state."))?;
    let current = undo
        .as_ref()
        .filter(|record| {
            record.workspace_id == active.id
                && record.operation_id == operation_id
                && record.state_revision == expected_revision
        })
        .ok_or_else(|| AppError::new("This Local Trash move is no longer available to Undo."))?;
    let mutation_result = local_trash::undo_move(
        &active.path,
        &scope,
        current.state_revision,
        &current.operation_id,
    );
    let mutation = match mutation_result {
        Ok(mutation) => mutation,
        Err(error) => {
            if error.code == Some(local_trash::ERROR_OPERATION_INDETERMINATE) {
                *undo = None;
            }
            return Err(error);
        }
    };
    *undo = None;
    Ok(local_trash_mutation_result(
        active.id, "undone", &mutation, true,
    ))
}

#[tauri::command]
fn reset_local_trash_state(
    state: State<AppState>,
    workspace_id: String,
) -> AppResult<LocalTrashResetResult> {
    let active = active_workspace_for_id(&state, &workspace_id)?;
    let scope = local_trash_scope_for_active(&active)?;
    let inspected = local_trash::inspect(&active.path, &scope);
    let mut undo = state
        .local_trash_undo
        .lock()
        .map_err(|_| AppError::new("Could not lock Local Trash Undo state."))?;
    let reset_result = local_trash::reset_unhealthy(&active.path, &scope, &inspected);
    let result = match reset_result {
        Ok(result) => result,
        Err(error) => {
            if error.code == Some(local_trash::ERROR_RESET_INDETERMINATE) {
                *undo = None;
            }
            return Err(error);
        }
    };
    if result.reset {
        *undo = None;
    }
    Ok(LocalTrashResetResult {
        workspace_id: active.id,
        reset: result.reset,
    })
}

fn local_trash_scope_for_active(
    active: &ActiveWorkspace,
) -> AppResult<local_trash::LocalTrashScope> {
    let scope = local_trash::LocalTrashScope::new(active.id.clone(), active.fingerprint.clone());
    #[cfg(test)]
    if scope.is_err() {
        return local_trash::LocalTrashScope::new(
            sha256_text(&active.id),
            sha256_text(&active.fingerprint),
        );
    }
    scope
}

fn local_trash_membership_predicate(
    message_alias: &str,
    scope: &local_trash::LocalTrashScope,
    is_member: bool,
) -> AppResult<String> {
    if !matches!(
        message_alias,
        "m" | "root" | "participant_messages" | "all_messages" | "attachment_messages"
    ) {
        return Err(AppError::new("Local Trash query alias is invalid."));
    }
    let membership = format!(
        "EXISTS (
             SELECT 1
               FROM temp.{} lt
              WHERE lt.workspace_fingerprint = '{}'
                AND lt.source_fingerprint = '{}'
                AND lt.canonical_identity_version = '{}'
                AND lt.canonical_identity = {message_alias}.eml_path
         )",
        local_trash::LOCAL_TRASH_TEMP_TABLE,
        scope.workspace_fingerprint(),
        scope.source_fingerprint(),
        local_trash::LOCAL_TRASH_IDENTITY_VERSION,
    );
    Ok(if is_member {
        membership
    } else {
        format!("NOT {membership}")
    })
}

fn local_trash_visibility_predicate(
    message_alias: &str,
    scope: &local_trash::LocalTrashScope,
    visibility: MessageVisibility,
) -> AppResult<String> {
    local_trash_membership_predicate(
        message_alias,
        scope,
        matches!(visibility, MessageVisibility::Trashed),
    )
}

fn visible_message_query_source(
    conn: &Connection,
    active: &ActiveWorkspace,
    visibility: MessageVisibility,
    folder_id: Option<i64>,
    include_subfolders: bool,
    criteria: &MessageSearchCriteria,
    operation: Option<&SearchOperationGuard>,
) -> AppResult<(MessageQuerySource, local_trash::LocalTrashQueryState)> {
    let scope = local_trash_scope_for_active(active)?;
    let local_state = if let Some(operation) = operation {
        local_trash::materialize_for_search(conn, &active.path, &scope, operation)?
    } else {
        local_trash::materialize_for_query(conn, &active.path, &scope)?
    };
    let source = visible_message_query_source_after_materialization(
        conn,
        &scope,
        visibility,
        folder_id,
        include_subfolders,
        criteria,
    )?;
    Ok((source, local_state))
}

fn visible_message_query_source_after_materialization(
    conn: &Connection,
    scope: &local_trash::LocalTrashScope,
    visibility: MessageVisibility,
    folder_id: Option<i64>,
    include_subfolders: bool,
    criteria: &MessageSearchCriteria,
) -> AppResult<MessageQuerySource> {
    let source = build_message_query_source(
        conn,
        if matches!(visibility, MessageVisibility::Trashed) {
            None
        } else {
            folder_id
        },
        if matches!(visibility, MessageVisibility::Trashed) {
            false
        } else {
            include_subfolders
        },
        criteria,
    )?;
    let predicate = local_trash_visibility_predicate("m", scope, visibility)?;
    let (where_sql, params) = query_source_with_condition(&source, &predicate);
    Ok(MessageQuerySource {
        from_sql: source.from_sql,
        where_sql,
        params,
        has_text_match: source.has_text_match,
    })
}

fn require_active_conversation_visibility(visibility: MessageVisibility) -> AppResult<()> {
    if matches!(visibility, MessageVisibility::Trashed) {
        return Err(AppError::new(
            "Local Trash is available in Messages view only.",
        ));
    }
    Ok(())
}

async fn run_search_worker<T, F>(operation: SearchOperationGuard, work: F) -> AppResult<T>
where
    T: Send + 'static,
    F: FnOnce(&SearchOperationGuard) -> AppResult<T> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(move || work(&operation))
        .await
        .map_err(|_| AppError::new("Search worker could not complete."))?
}

// Tauri serializes these as stable top-level IPC arguments; grouping them would change the API.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn list_messages(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    workspace_id: String,
    folder_id: Option<i64>,
    query: Option<String>,
    include_subfolders: bool,
    visibility: Option<String>,
    search_filters: Option<SearchFilters>,
    sort_order: Option<String>,
    limit: Option<i64>,
    cursor: Option<String>,
    search_generation: u64,
    search_operation_id: String,
) -> AppResult<MessagePageResult> {
    let operation = state.search_cancellations.begin_operation(
        window.label(),
        search_generation,
        &search_operation_id,
        SearchOperationCategory::MessagePage,
    )?;
    let criteria = MessageSearchCriteria::from_inputs(query, search_filters)?;
    let visibility = MessageVisibility::from_arg(visibility)?;
    validate_message_sort_workspace_count(sort_order.as_deref(), 1)?;
    let active = active_workspace_for_id(&state, &workspace_id)?;
    let cursor_codec = state.search_cursor_codec.clone();
    run_search_worker(operation, move |operation| {
        let conn = open_workspace_db_for_search(&active.path, operation)?;
        query_messages_cursor_page_visible(
            &conn,
            &active,
            folder_id,
            include_subfolders,
            &criteria,
            visibility,
            sort_order.as_deref(),
            limit,
            cursor.as_deref(),
            search_generation,
            &cursor_codec,
            Some(operation),
        )
    })
    .await
}

// Tauri serializes these as stable top-level IPC arguments; grouping them would change the API.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn count_messages(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    workspace_id: String,
    folder_id: Option<i64>,
    query: Option<String>,
    include_subfolders: bool,
    visibility: Option<String>,
    search_filters: Option<SearchFilters>,
    search_generation: u64,
    search_operation_id: String,
) -> AppResult<MessageCountResult> {
    let operation = state.search_cancellations.begin_operation(
        window.label(),
        search_generation,
        &search_operation_id,
        SearchOperationCategory::MessageCount,
    )?;
    let criteria = MessageSearchCriteria::from_inputs(query, search_filters)?;
    let visibility = MessageVisibility::from_arg(visibility)?;
    let active = active_workspace_for_id(&state, &workspace_id)?;
    run_search_worker(operation, move |operation| {
        let conn = open_workspace_db_for_search(&active.path, operation)?;
        Ok(MessageCountResult {
            total_count: query_message_count_visible(
                &conn,
                &active,
                folder_id,
                include_subfolders,
                &criteria,
                visibility,
                Some(operation),
            )?,
        })
    })
    .await
}

// Tauri serializes these as stable top-level IPC arguments; grouping them would change the API.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn search_messages_multi(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    workspace_ids: Vec<String>,
    query: Option<String>,
    visibility: Option<String>,
    search_filters: Option<SearchFilters>,
    sort_order: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    cursor: Option<String>,
    search_generation: u64,
    search_operation_id: String,
) -> AppResult<MultiMessagePageResult> {
    let operation = state.search_cancellations.begin_operation(
        window.label(),
        search_generation,
        &search_operation_id,
        SearchOperationCategory::MessagePage,
    )?;
    validate_workspace_batch_size(workspace_ids.len())?;
    let offset = validate_search_offset(offset)?;
    ensure_multi_workspace_cursor_absent(cursor.as_deref())?;
    let criteria = MessageSearchCriteria::from_inputs(query, search_filters)?;
    let visibility = MessageVisibility::from_arg(visibility)?;
    let sort_order_value = sort_order.unwrap_or_else(|| "newest".to_string());
    let mut seen = HashSet::new();
    let mut workspaces = Vec::new();
    for workspace_id in workspace_ids {
        operation.check_cancelled()?;
        if !seen.insert(workspace_id.clone()) {
            continue;
        }
        workspaces.push(active_workspace_for_id(&state, &workspace_id)?);
    }

    validate_message_sort_workspace_count(Some(&sort_order_value), workspaces.len())?;
    run_search_worker(operation, move |operation| {
        query_multi_workspace_message_page_visible(
            workspaces,
            &criteria,
            &sort_order_value,
            visibility,
            limit,
            offset,
            operation,
        )
    })
    .await
}

fn ensure_multi_workspace_cursor_absent(cursor: Option<&str>) -> AppResult<()> {
    if cursor.is_some() {
        return Err(AppError::coded(
            "Cursor pagination is not supported for multi-workspace searches.",
            UNSUPPORTED_SEARCH_CURSOR_CODE,
        ));
    }
    Ok(())
}

// Tauri serializes these as stable top-level IPC arguments; grouping them would change the API.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn count_messages_multi(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    workspace_ids: Vec<String>,
    query: Option<String>,
    visibility: Option<String>,
    search_filters: Option<SearchFilters>,
    search_generation: u64,
    search_operation_id: String,
) -> AppResult<MultiMessageCountResult> {
    let operation = state.search_cancellations.begin_operation(
        window.label(),
        search_generation,
        &search_operation_id,
        SearchOperationCategory::MessageCount,
    )?;
    validate_workspace_batch_size(workspace_ids.len())?;
    let criteria = MessageSearchCriteria::from_inputs(query, search_filters)?;
    let visibility = MessageVisibility::from_arg(visibility)?;
    let mut seen = HashSet::new();
    let mut workspaces = Vec::new();
    for workspace_id in workspace_ids {
        operation.check_cancelled()?;
        if seen.insert(workspace_id.clone()) {
            workspaces.push(active_workspace_for_id(&state, &workspace_id)?);
        }
    }
    run_search_worker(operation, move |operation| {
        count_multi_workspace_messages_visible(workspaces, &criteria, visibility, operation)
    })
    .await
}

struct VisibleMessageWorkspaceCursor {
    active: ActiveWorkspace,
    conn: Connection,
    scope: local_trash::LocalTrashScope,
    visibility: MessageVisibility,
    pst_display_name: String,
    next_offset: i64,
    buffer: VecDeque<MessageListItem>,
    exhausted: bool,
}

impl VisibleMessageWorkspaceCursor {
    fn fill(
        &mut self,
        criteria: &MessageSearchCriteria,
        sort_order: &str,
        operation: &SearchOperationGuard,
    ) -> AppResult<()> {
        operation.check_cancelled()?;
        if self.exhausted || !self.buffer.is_empty() {
            return Ok(());
        }
        let source = visible_message_query_source_after_materialization(
            &self.conn,
            &self.scope,
            self.visibility,
            None,
            false,
            criteria,
        )?;
        let page = query_messages_page_from_source(
            &self.conn,
            source,
            Some(sort_order),
            Some(DEFAULT_MESSAGE_PAGE_SIZE),
            Some(self.next_offset),
            Some(operation),
        )?;
        self.next_offset += page.returned_count as i64;
        self.exhausted = !page.has_more;
        self.buffer = page.items.into();
        Ok(())
    }
}

fn query_multi_workspace_message_page_visible(
    workspaces: Vec<ActiveWorkspace>,
    criteria: &MessageSearchCriteria,
    sort_order: &str,
    visibility: MessageVisibility,
    limit: Option<i64>,
    offset: Option<i64>,
    operation: &SearchOperationGuard,
) -> AppResult<MultiMessagePageResult> {
    let page_limit = limit
        .unwrap_or(DEFAULT_MESSAGE_PAGE_SIZE)
        .clamp(1, MAX_MESSAGE_PAGE_SIZE) as usize;
    let page_offset = offset.unwrap_or(0).max(0) as usize;
    let mut local_trash_states = Vec::with_capacity(workspaces.len());
    let mut cursors = workspaces
        .into_iter()
        .map(|active| {
            operation.check_cancelled()?;
            let conn = open_workspace_db_for_search(&active.path, operation)?;
            let scope = local_trash_scope_for_active(&active)?;
            let local_state =
                local_trash::materialize_for_search(&conn, &active.path, &scope, operation)?;
            local_trash_states.push(local_trash_query_health(&active.id, &local_state));
            Ok(VisibleMessageWorkspaceCursor {
                pst_display_name: pst_display_name(&active),
                active,
                conn,
                scope,
                visibility,
                next_offset: 0,
                buffer: VecDeque::new(),
                exhausted: false,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;

    let mut skipped = 0usize;
    let mut items = Vec::with_capacity(page_limit + 1);
    while items.len() < page_limit + 1 {
        operation.check_cancelled()?;
        for cursor in &mut cursors {
            cursor.fill(criteria, sort_order, operation)?;
        }

        let mut best_index = None;
        for (index, cursor) in cursors.iter().enumerate() {
            let Some(item) = cursor.buffer.front() else {
                continue;
            };
            let Some(current_best_index) = best_index else {
                best_index = Some(index);
                continue;
            };
            let best_item = cursors[current_best_index]
                .buffer
                .front()
                .expect("best cursor had a front item");
            if compare_message_items(item, best_item, sort_order).is_lt() {
                best_index = Some(index);
            }
        }

        let Some(best_index) = best_index else {
            break;
        };
        let cursor = &mut cursors[best_index];
        let mut item = cursor
            .buffer
            .pop_front()
            .expect("selected cursor had a front item");
        if skipped < page_offset {
            skipped += 1;
            continue;
        }
        item.workspace_id = Some(cursor.active.id.clone());
        item.pst_display_name = Some(cursor.pst_display_name.clone());
        item.workspace_path = Some(cursor.active.path.display().to_string());
        items.push(item);
    }

    let page = finish_bounded_page(items, page_limit as i64, page_offset as i64);
    Ok(MultiMessagePageResult {
        items: page.items,
        requested_offset: page.requested_offset,
        returned_count: page.returned_count,
        has_more: page.has_more,
        next_cursor: None,
        pagination_mode: "offset",
        local_trash_states,
    })
}

fn count_multi_workspace_messages_visible(
    workspaces: Vec<ActiveWorkspace>,
    criteria: &MessageSearchCriteria,
    visibility: MessageVisibility,
    operation: &SearchOperationGuard,
) -> AppResult<MultiMessageCountResult> {
    let mut total_count = 0i64;
    let mut per_workspace_counts = Vec::with_capacity(workspaces.len());
    for active in workspaces {
        operation.check_cancelled()?;
        let conn = open_workspace_db_for_search(&active.path, operation)?;
        let count = query_message_count_visible(
            &conn,
            &active,
            None,
            false,
            criteria,
            visibility,
            Some(operation),
        )?;
        total_count = checked_result_count(total_count, count)?;
        per_workspace_counts.push(WorkspaceSearchCount {
            workspace_id: active.id.clone(),
            pst_display_name: pst_display_name(&active),
            count,
        });
        operation.check_cancelled()?;
    }
    Ok(MultiMessageCountResult {
        total_count,
        per_workspace_counts,
    })
}

#[cfg(test)]
struct MessageWorkspaceCursor {
    active: ActiveWorkspace,
    conn: Connection,
    pst_display_name: String,
    next_offset: i64,
    buffer: VecDeque<MessageListItem>,
    exhausted: bool,
}

#[cfg(test)]
impl MessageWorkspaceCursor {
    fn fill(
        &mut self,
        criteria: &MessageSearchCriteria,
        sort_order: &str,
        operation: &SearchOperationGuard,
    ) -> AppResult<()> {
        operation.check_cancelled()?;
        if self.exhausted || !self.buffer.is_empty() {
            return Ok(());
        }
        let page = query_messages_page(MessagePageQuery {
            conn: &self.conn,
            folder_id: None,
            include_subfolders: false,
            criteria,
            sort_order: Some(sort_order),
            limit: Some(DEFAULT_MESSAGE_PAGE_SIZE),
            offset: Some(self.next_offset),
            operation: Some(operation),
        })?;
        self.next_offset += page.returned_count as i64;
        self.exhausted = !page.has_more;
        self.buffer = page.items.into();
        Ok(())
    }
}

#[cfg(test)]
fn query_multi_workspace_message_page(
    workspaces: Vec<ActiveWorkspace>,
    criteria: &MessageSearchCriteria,
    sort_order: &str,
    limit: Option<i64>,
    offset: Option<i64>,
    operation: &SearchOperationGuard,
) -> AppResult<MultiMessagePageResult> {
    let page_limit = limit
        .unwrap_or(DEFAULT_MESSAGE_PAGE_SIZE)
        .clamp(1, MAX_MESSAGE_PAGE_SIZE) as usize;
    let page_offset = offset.unwrap_or(0).max(0) as usize;
    let mut cursors = workspaces
        .into_iter()
        .map(|active| {
            operation.check_cancelled()?;
            let conn = open_workspace_db_for_search(&active.path, operation)?;
            Ok(MessageWorkspaceCursor {
                pst_display_name: pst_display_name(&active),
                active,
                conn,
                next_offset: 0,
                buffer: VecDeque::new(),
                exhausted: false,
            })
        })
        .collect::<AppResult<Vec<_>>>()?;

    let mut skipped = 0usize;
    let mut items = Vec::with_capacity(page_limit + 1);
    while items.len() < page_limit + 1 {
        operation.check_cancelled()?;
        for cursor in &mut cursors {
            cursor.fill(criteria, sort_order, operation)?;
        }

        let mut best_index = None;
        for (index, cursor) in cursors.iter().enumerate() {
            let Some(item) = cursor.buffer.front() else {
                continue;
            };
            let Some(current_best_index) = best_index else {
                best_index = Some(index);
                continue;
            };
            let best_item = cursors[current_best_index]
                .buffer
                .front()
                .expect("best cursor had a front item");
            if compare_message_items(item, best_item, sort_order).is_lt() {
                best_index = Some(index);
            }
        }

        let Some(best_index) = best_index else {
            break;
        };
        let cursor = &mut cursors[best_index];
        let mut item = cursor
            .buffer
            .pop_front()
            .expect("selected cursor had a front item");
        if skipped < page_offset {
            skipped += 1;
            continue;
        }
        item.workspace_id = Some(cursor.active.id.clone());
        item.pst_display_name = Some(cursor.pst_display_name.clone());
        item.workspace_path = Some(cursor.active.path.display().to_string());
        items.push(item);
    }

    let page = finish_bounded_page(items, page_limit as i64, page_offset as i64);
    Ok(MultiMessagePageResult {
        items: page.items,
        requested_offset: page.requested_offset,
        returned_count: page.returned_count,
        has_more: page.has_more,
        next_cursor: None,
        pagination_mode: "offset",
        local_trash_states: Vec::new(),
    })
}

#[cfg(test)]
fn count_multi_workspace_messages(
    workspaces: Vec<ActiveWorkspace>,
    criteria: &MessageSearchCriteria,
    operation: &SearchOperationGuard,
) -> AppResult<MultiMessageCountResult> {
    let mut total_count = 0i64;
    let mut per_workspace_counts = Vec::with_capacity(workspaces.len());
    for active in workspaces {
        operation.check_cancelled()?;
        let conn = open_workspace_db_for_search(&active.path, operation)?;
        let count = query_message_count(&conn, None, false, criteria, Some(operation))?;
        total_count = checked_result_count(total_count, count)?;
        per_workspace_counts.push(WorkspaceSearchCount {
            workspace_id: active.id.clone(),
            pst_display_name: pst_display_name(&active),
            count,
        });
        operation.check_cancelled()?;
    }
    Ok(MultiMessageCountResult {
        total_count,
        per_workspace_counts,
    })
}

// Tauri serializes these as stable top-level IPC arguments; grouping them would change the API.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn list_conversations(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    scopes: Vec<ConversationWorkspaceScope>,
    query: Option<String>,
    visibility: Option<String>,
    search_filters: Option<SearchFilters>,
    conversation_sort: Option<String>,
    limit: Option<i64>,
    offset: Option<i64>,
    search_generation: u64,
    search_operation_id: String,
) -> AppResult<ConversationPageResult> {
    let operation = state.search_cancellations.begin_operation(
        window.label(),
        search_generation,
        &search_operation_id,
        SearchOperationCategory::ConversationPage,
    )?;
    validate_workspace_batch_size(scopes.len())?;
    let offset = validate_search_offset(offset)?;
    let criteria = MessageSearchCriteria::from_inputs(query, search_filters)?;
    let visibility = MessageVisibility::from_arg(visibility)?;
    require_active_conversation_visibility(visibility)?;
    let sort = conversation_sort.unwrap_or_else(|| "newest".to_string());
    validate_conversation_sort(&sort)?;

    let mut seen = HashSet::new();
    let mut resolved_scopes = Vec::new();
    for scope in scopes {
        operation.check_cancelled()?;
        if !seen.insert(scope.workspace_id.clone()) {
            continue;
        }
        let active = active_workspace_for_id(&state, &scope.workspace_id)?;
        resolved_scopes.push((scope, active));
    }

    run_search_worker(operation, move |operation| {
        query_conversation_page_for_scopes(
            resolved_scopes,
            &criteria,
            &sort,
            limit,
            offset,
            operation,
        )
    })
    .await
}

// Tauri serializes these as stable top-level IPC arguments; grouping them would change the API.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn count_conversations(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    scopes: Vec<ConversationWorkspaceScope>,
    query: Option<String>,
    visibility: Option<String>,
    search_filters: Option<SearchFilters>,
    search_generation: u64,
    search_operation_id: String,
) -> AppResult<ConversationCountResult> {
    let operation = state.search_cancellations.begin_operation(
        window.label(),
        search_generation,
        &search_operation_id,
        SearchOperationCategory::ConversationCount,
    )?;
    validate_workspace_batch_size(scopes.len())?;
    let criteria = MessageSearchCriteria::from_inputs(query, search_filters)?;
    let visibility = MessageVisibility::from_arg(visibility)?;
    require_active_conversation_visibility(visibility)?;
    let mut seen = HashSet::new();
    let mut resolved_scopes = Vec::new();
    for scope in scopes {
        operation.check_cancelled()?;
        if seen.insert(scope.workspace_id.clone()) {
            let active = active_workspace_for_id(&state, &scope.workspace_id)?;
            resolved_scopes.push((scope, active));
        }
    }
    run_search_worker(operation, move |operation| {
        count_conversations_for_scopes(resolved_scopes, &criteria, operation)
    })
    .await
}

struct ConversationCursor {
    active: ActiveWorkspace,
    conn: Connection,
    local_trash_scope: local_trash::LocalTrashScope,
    folder_id: Option<i64>,
    include_subfolders: bool,
    next_offset: i64,
    buffer: VecDeque<ConversationSummary>,
    exhausted: bool,
}

impl ConversationCursor {
    fn fill(
        &mut self,
        criteria: &MessageSearchCriteria,
        sort: &str,
        operation: &SearchOperationGuard,
    ) -> AppResult<()> {
        operation.check_cancelled()?;
        if self.exhausted || !self.buffer.is_empty() {
            return Ok(());
        }
        let page = query_conversation_summaries_page_visible(
            &self.conn,
            &self.active,
            &self.local_trash_scope,
            self.folder_id,
            self.include_subfolders,
            criteria,
            sort,
            DEFAULT_CONVERSATION_PAGE_SIZE,
            self.next_offset,
            Some(operation),
        )?;
        self.next_offset += page.returned_count as i64;
        self.exhausted = !page.has_more;
        self.buffer = page.items.into();
        Ok(())
    }
}

fn conversation_workspace_issue(active: &ActiveWorkspace) -> ConversationWorkspaceIssue {
    ConversationWorkspaceIssue {
        workspace_id: active.id.clone(),
        pst_display_name: pst_display_name(active),
        workspace_path: active.path.display().to_string(),
        can_reindex: active.path.join("extracted").is_dir(),
    }
}

fn query_conversation_page_for_scopes(
    resolved_scopes: Vec<(ConversationWorkspaceScope, ActiveWorkspace)>,
    criteria: &MessageSearchCriteria,
    sort: &str,
    limit: Option<i64>,
    offset: Option<i64>,
    operation: &SearchOperationGuard,
) -> AppResult<ConversationPageResult> {
    let page_limit = limit
        .unwrap_or(DEFAULT_CONVERSATION_PAGE_SIZE)
        .clamp(1, MAX_CONVERSATION_PAGE_SIZE) as usize;
    let page_offset = offset.unwrap_or(0).max(0) as usize;
    let mut cursors = Vec::new();
    let mut unindexed_workspaces = Vec::new();
    let mut local_trash_states = Vec::new();
    for (scope, active) in resolved_scopes {
        operation.check_cancelled()?;
        let conn = open_workspace_db_for_search(&active.path, operation)?;
        if !conversation_data_is_indexed(&conn)? {
            unindexed_workspaces.push(conversation_workspace_issue(&active));
            continue;
        }
        let local_trash_scope = local_trash_scope_for_active(&active)?;
        let local_state = local_trash::materialize_for_search(
            &conn,
            &active.path,
            &local_trash_scope,
            operation,
        )?;
        local_trash_states.push(local_trash_query_health(&active.id, &local_state));
        cursors.push(ConversationCursor {
            active,
            conn,
            local_trash_scope,
            folder_id: scope.folder_id,
            include_subfolders: scope.include_subfolders,
            next_offset: 0,
            buffer: VecDeque::new(),
            exhausted: false,
        });
    }
    let indexed_workspace_count = cursors.len();
    let mut skipped = 0usize;
    let mut items = Vec::with_capacity(page_limit + 1);
    while items.len() < page_limit + 1 {
        operation.check_cancelled()?;
        for cursor in &mut cursors {
            cursor.fill(criteria, sort, operation)?;
        }
        let mut best_index = None;
        for (index, cursor) in cursors.iter().enumerate() {
            let Some(item) = cursor.buffer.front() else {
                continue;
            };
            let Some(current_best) = best_index else {
                best_index = Some(index);
                continue;
            };
            let best_item = cursors[current_best]
                .buffer
                .front()
                .expect("best conversation cursor had an item");
            if compare_conversation_summaries(item, best_item, sort).is_lt() {
                best_index = Some(index);
            }
        }
        let Some(best_index) = best_index else {
            break;
        };
        let item = cursors[best_index]
            .buffer
            .pop_front()
            .expect("selected conversation cursor had an item");
        if skipped < page_offset {
            skipped += 1;
            continue;
        }
        items.push(item);
    }

    let page = finish_bounded_page(items, page_limit as i64, page_offset as i64);
    Ok(ConversationPageResult {
        items: page.items,
        requested_offset: page.requested_offset,
        returned_count: page.returned_count,
        has_more: page.has_more,
        indexed_workspace_count,
        unindexed_workspaces,
        local_trash_states,
    })
}

fn count_conversations_for_scopes(
    resolved_scopes: Vec<(ConversationWorkspaceScope, ActiveWorkspace)>,
    criteria: &MessageSearchCriteria,
    operation: &SearchOperationGuard,
) -> AppResult<ConversationCountResult> {
    let mut total_count = 0i64;
    let mut matching_message_count = 0i64;
    for (scope, active) in resolved_scopes {
        operation.check_cancelled()?;
        let conn = open_workspace_db_for_search(&active.path, operation)?;
        if !conversation_data_is_indexed(&conn)? {
            continue;
        }
        let (workspace_conversations, workspace_messages) = conversation_counts_visible(
            &conn,
            &active,
            scope.folder_id,
            scope.include_subfolders,
            criteria,
            Some(operation),
        )?;
        total_count = checked_result_count(total_count, workspace_conversations)?;
        matching_message_count = checked_result_count(matching_message_count, workspace_messages)?;
        operation.check_cancelled()?;
    }
    Ok(ConversationCountResult {
        total_count,
        matching_message_count,
    })
}

// Tauri serializes these as stable top-level IPC arguments; grouping them would change the API.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
async fn get_conversation_messages(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    workspace_id: String,
    conversation_id: String,
    folder_id: Option<i64>,
    include_subfolders: bool,
    query: Option<String>,
    visibility: Option<String>,
    search_filters: Option<SearchFilters>,
    show_entire_conversation: bool,
    limit: Option<i64>,
    offset: Option<i64>,
    search_generation: u64,
    search_operation_id: String,
) -> AppResult<ConversationMessagesResult> {
    let operation = state.search_cancellations.begin_operation(
        window.label(),
        search_generation,
        &search_operation_id,
        SearchOperationCategory::ExpandedConversation,
    )?;
    validate_conversation_identity_input(&conversation_id)?;
    let offset = validate_search_offset(offset)?;
    let criteria = MessageSearchCriteria::from_inputs(query, search_filters)?;
    let visibility = MessageVisibility::from_arg(visibility)?;
    require_active_conversation_visibility(visibility)?;
    let active = active_workspace_for_id(&state, &workspace_id)?;
    run_search_worker(operation, move |operation| {
        let conn = open_workspace_db_for_search(&active.path, operation)?;
        if !conversation_data_is_indexed(&conn)? {
            return Err(AppError::new(
                "Conversation data is not indexed for this workspace.",
            ));
        }
        let local_trash_scope = local_trash_scope_for_active(&active)?;
        local_trash::materialize_for_search(&conn, &active.path, &local_trash_scope, operation)?;
        let source = visible_message_query_source_after_materialization(
            &conn,
            &local_trash_scope,
            MessageVisibility::Active,
            folder_id,
            include_subfolders,
            &criteria,
        )?;
        let (matching_where, mut matching_params) =
            query_source_with_condition(&source, "m.conversation_id = ?");
        matching_params.push(Value::Text(conversation_id.clone()));
        let matching_count_sql = format!("SELECT COUNT(*){}{}", source.from_sql, matching_where);
        let matching_message_count = conn.query_row(
            &matching_count_sql,
            params_from_iter(matching_params.iter()),
            |row| row.get(0),
        )?;
        operation.check_cancelled()?;
        let active_predicate = local_trash_membership_predicate("m", &local_trash_scope, false)?;
        let total_message_count: i64 = conn.query_row(
            &format!(
                "SELECT COUNT(*) FROM messages m
                  WHERE m.conversation_id = ?1 AND {active_predicate}"
            ),
            params![conversation_id],
            |row| row.get(0),
        )?;
        operation.check_cancelled()?;

        let page_limit = limit
            .unwrap_or(CONVERSATION_MESSAGE_PAGE_SIZE)
            .clamp(1, MAX_MESSAGE_PAGE_SIZE);
        let page_offset = offset.unwrap_or(0).max(0);
        let mut items = if show_entire_conversation {
            query_entire_conversation_page_visible(
                &conn,
                &source,
                &local_trash_scope,
                &conversation_id,
                page_limit,
                page_offset,
                Some(operation),
            )?
        } else {
            query_matching_conversation_page(
                &conn,
                &source,
                &conversation_id,
                page_limit,
                page_offset,
                Some(operation),
            )?
        };
        for item in &mut items {
            item.message.workspace_id = Some(active.id.clone());
            item.message.pst_display_name = Some(pst_display_name(&active));
            item.message.workspace_path = Some(active.path.display().to_string());
        }

        Ok(ConversationMessagesResult {
            items,
            matching_message_count,
            total_message_count,
            showing_entire_conversation: show_entire_conversation,
        })
    })
    .await
}

#[tauri::command]
fn get_message(
    state: State<AppState>,
    workspace_id: String,
    message_id: i64,
) -> AppResult<MessageDetail> {
    let active = active_workspace_for_id(&state, &workspace_id)?;
    let conn = open_workspace_db_for_read(&active.path)?;

    let mut statement = conn.prepare(
        "SELECT id,
                folder_id,
                subject,
                sender,
                recipients,
                date,
                snippet,
                has_attachments,
                (SELECT COUNT(*) FROM attachments a WHERE a.message_id = messages.id) AS attachment_count,
                body,
                body_source,
                CASE WHEN body_html IS NOT NULL AND TRIM(body_html) <> '' THEN 1 ELSE 0 END AS body_html_available,
                eml_path
           FROM messages
          WHERE id = ?1",
    )?;

    let mut detail = statement
        .query_row(params![message_id], |row| {
            let attachment_count = row.get::<_, i64>(8)?;
            Ok(MessageDetail {
                id: row.get(0)?,
                folder_id: row.get(1)?,
                subject: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                sender: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                recipients: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                date: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                snippet: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                has_attachments: row.get::<_, i64>(7)? != 0 || attachment_count > 0,
                attachment_count: row.get(8)?,
                body: row.get::<_, Option<String>>(9)?.unwrap_or_default(),
                body_source: row.get::<_, Option<String>>(10)?.unwrap_or_default(),
                body_html_available: row.get::<_, i64>(11)? != 0,
                eml_path: row.get(12)?,
                can_reindex_from_eml: false,
                in_local_trash: false,
                local_trash_available: true,
                local_trash_error_code: None,
                attachments: Vec::new(),
            })
        })
        .optional()?
        .ok_or_else(|| AppError::new("Message was not found in this workspace."))?;

    let body_html_schema_current =
        metadata_value(&conn, "body_html_schema_version")?.as_deref() == Some("body-html-v1");
    detail.can_reindex_from_eml = !detail.body_html_available
        && !body_html_schema_current
        && active
            .path
            .join("extracted")
            .join(&detail.eml_path)
            .is_file();
    detail.attachments = list_attachments(&conn, message_id)?;
    let (in_local_trash, local_trash_available, local_trash_error_code) =
        local_trash_message_state(&conn, &active, &detail.eml_path)?;
    detail.in_local_trash = in_local_trash;
    detail.local_trash_available = local_trash_available;
    detail.local_trash_error_code = local_trash_error_code;
    Ok(detail)
}

struct MessageDiagnosticsRow {
    subject: String,
    body_source: String,
    body: String,
    body_html: String,
    message_id_header: String,
    in_reply_to: String,
    references_header: String,
    normalized_subject: String,
    conversation_id: String,
    thread_assignment_method: String,
    conversation_parent_id: Option<i64>,
    conversation_root_id: Option<i64>,
    thread_warning: String,
}

#[tauri::command]
fn get_message_diagnostics(
    state: State<AppState>,
    workspace_id: String,
    message_id: i64,
) -> AppResult<MessageDiagnostics> {
    let source = resolve_source_eml(&state, &workspace_id, message_id)?;
    let conn = open_workspace_db_for_read(&source.workspace_root)?;
    let MessageDiagnosticsRow {
        subject,
        body_source,
        body,
        body_html,
        message_id_header,
        in_reply_to,
        references_header,
        normalized_subject,
        conversation_id,
        thread_assignment_method,
        conversation_parent_id,
        conversation_root_id,
        thread_warning,
    } = conn
        .query_row(
            "SELECT COALESCE(subject, ''),
                    COALESCE(body_source, ''),
                    COALESCE(body, ''),
                    COALESCE(body_html, ''),
                    COALESCE(message_id_header_raw, ''),
                    COALESCE(in_reply_to_raw, ''),
                    COALESCE(references_header_raw, ''),
                    COALESCE(normalized_subject, ''),
                    COALESCE(conversation_id, ''),
                    COALESCE(thread_assignment_method, 'standalone'),
                    conversation_parent_id,
                    conversation_root_id,
                    COALESCE(thread_warning, '')
               FROM messages
              WHERE id = ?1",
            params![message_id],
            |row| {
                Ok(MessageDiagnosticsRow {
                    subject: row.get(0)?,
                    body_source: row.get(1)?,
                    body: row.get(2)?,
                    body_html: row.get(3)?,
                    message_id_header: row.get(4)?,
                    in_reply_to: row.get(5)?,
                    references_header: row.get(6)?,
                    normalized_subject: row.get(7)?,
                    conversation_id: row.get(8)?,
                    thread_assignment_method: row.get(9)?,
                    conversation_parent_id: row.get(10)?,
                    conversation_root_id: row.get(11)?,
                    thread_warning: row.get(12)?,
                })
            },
        )
        .optional()?
        .ok_or_else(|| AppError::new("Message was not found in this workspace."))?;
    let attachments = list_attachments(&conn, message_id)?;
    let diagnostic_attachments = attachments
        .iter()
        .map(|attachment| DiagnosticAttachment {
            filename: attachment.filename.clone(),
            content_type: attachment.content_type.clone(),
        })
        .collect::<Vec<_>>();

    let mut parse_warnings = Vec::new();
    if !thread_warning.trim().is_empty() {
        parse_warnings.push(thread_warning);
    }
    let mut mime_parts = Vec::new();
    let mut detected_body_mime_part = None;
    let mut rtf_body_promoted = false;
    let mut rtf_body_suppressed_from_attachments = false;
    let mut remote_images_detected = html_has_remote_images(&body_html);
    let mut cid_images_detected = html_has_cid_images(&body_html);

    let bytes = read_bounded_message_source(&source.source_path)?;
    match parse_mail_bytes(&bytes) {
        Ok(parsed_mail) => match parse_eml(&source.source_path) {
            Ok(parsed_message) => {
                if parsed_message.body_source.as_str() != body_source {
                    parse_warnings.push(format!(
                        "Current parser would set body_source={} after reindex.",
                        parsed_message.body_source.as_str()
                    ));
                }

                let promoted_rtf_part_path = parsed_message
                    .rtf_body_fallback
                    .as_deref()
                    .and_then(parse_logged_rtf_part_path);
                rtf_body_promoted = promoted_rtf_part_path.is_some();
                rtf_body_suppressed_from_attachments = rtf_body_promoted;
                if let Some(fallback) = parsed_message.rtf_body_fallback.as_deref() {
                    parse_warnings.push(format!("RTF body promoted: {fallback}"));
                }

                let diagnostic_html = if parsed_message.body_html.trim().is_empty() {
                    body_html.as_str()
                } else {
                    parsed_message.body_html.as_str()
                };
                remote_images_detected |= html_has_remote_images(diagnostic_html);
                cid_images_detected |= html_has_cid_images(diagnostic_html);

                collect_mime_part_diagnostics(
                    &parsed_mail,
                    "0",
                    promoted_rtf_part_path.as_deref(),
                    &mut mime_parts,
                );
                cid_images_detected |= mime_parts.iter().any(|part| {
                    !part.content_id.trim().is_empty()
                        && part.content_type.to_ascii_lowercase().starts_with("image/")
                });
                detected_body_mime_part = mime_parts
                    .iter()
                    .find(|part| part.role.contains("body"))
                    .map(describe_mime_part);
            }
            Err(error) => {
                parse_warnings.push(format!(
                    "Source EML parsed as MIME, but body extraction failed: {}",
                    error.message
                ));
                collect_mime_part_diagnostics(&parsed_mail, "0", None, &mut mime_parts);
                detected_body_mime_part = mime_parts
                    .iter()
                    .find(|part| part.role.contains("body"))
                    .map(describe_mime_part);
            }
        },
        Err(error) => {
            parse_warnings.push(format!("Source EML MIME parse failed: {error}"));
        }
    }

    if !body_html.trim().is_empty() && detected_body_mime_part.is_none() {
        detected_body_mime_part = Some("indexed body_html".to_string());
    }

    let detected_parent = conversation_parent_id
        .map(|id| diagnostic_message_label(&conn, id))
        .transpose()?;
    let detected_root = conversation_root_id
        .map(|id| diagnostic_message_label(&conn, id))
        .transpose()?;

    Ok(MessageDiagnostics {
        message_id,
        subject,
        body_source,
        has_body_text: !body.trim().is_empty(),
        has_body_html: !body_html.trim().is_empty(),
        source_eml_path: source.source_path.display().to_string(),
        attachment_count: attachments.len(),
        attachments: diagnostic_attachments,
        detected_body_mime_part,
        rtf_body_promoted,
        rtf_body_suppressed_from_attachments,
        remote_images_detected,
        cid_images_detected,
        parse_warnings,
        mime_parts,
        message_id_header,
        in_reply_to,
        references_header,
        normalized_subject,
        conversation_id,
        thread_assignment_method,
        detected_parent,
        detected_root,
    })
}

fn diagnostic_message_label(conn: &Connection, message_id: i64) -> AppResult<String> {
    conn.query_row(
        "SELECT printf('%d - %s', id, COALESCE(NULLIF(subject, ''), '(no subject)'))
           FROM messages
          WHERE id = ?1",
        params![message_id],
        |row| row.get(0),
    )
    .optional()?
    .ok_or_else(|| AppError::new(format!("Thread message {message_id} was not found.")))
}

#[tauri::command]
fn render_message_html(
    state: State<AppState>,
    workspace_id: String,
    message_id: i64,
) -> AppResult<HtmlRenderResult> {
    let active = active_workspace_for_id(&state, &workspace_id)?;
    let conn = open_workspace_db_for_read(&active.path)?;
    let (body_html, eml_path): (Option<String>, String) = conn
        .query_row(
            "SELECT body_html, eml_path FROM messages WHERE id = ?1",
            params![message_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| AppError::new("Message was not found in this workspace."))?;

    let Some(body_html) = body_html.filter(|value| !value.trim().is_empty()) else {
        return Ok(HtmlRenderResult {
            html_available: false,
            sanitized_html: String::new(),
            remote_images_blocked: false,
            remote_image_count: 0,
            embedded_image_count: 0,
            error: None,
        });
    };

    let cid_images = load_cid_images_for_message(&active.path, &eml_path).unwrap_or_default();
    let render = sanitize_email_html(&body_html, cid_images);

    Ok(HtmlRenderResult {
        html_available: true,
        sanitized_html: render.sanitized_html,
        remote_images_blocked: render.remote_images_blocked,
        remote_image_count: render.remote_image_count,
        embedded_image_count: render.embedded_image_count,
        error: None,
    })
}

#[tauri::command]
fn get_source_eml_view(
    state: State<AppState>,
    workspace_id: String,
    message_id: i64,
) -> AppResult<SourceEmlView> {
    let active = active_workspace_for_id(&state, &workspace_id)?;
    let source = resolve_source_eml(&state, &workspace_id, message_id)?;
    let conn = open_workspace_db_for_read(&source.workspace_root)?;
    let attachments = list_attachments(&conn, message_id)?;
    let (in_local_trash, local_trash_available, local_trash_error_code) =
        local_trash_message_state(&conn, &active, &source.relative_eml_path)?;
    let mut view = source_eml_view_from_path(
        &source.source_path,
        source.relative_eml_path,
        "workspace",
        message_id,
        Some(attachments),
    )?;
    view.in_local_trash = in_local_trash;
    view.local_trash_available = local_trash_available;
    view.local_trash_error_code = local_trash_error_code;
    Ok(view)
}

#[tauri::command]
fn get_standalone_message_view(
    state: State<AppState>,
    stable_id: String,
) -> AppResult<SourceEmlView> {
    let source_path = authorized_standalone_path(&state, &stable_id)?;
    let source_name = source_path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("message")
        .to_string();
    source_eml_view_from_path(&source_path, source_name, "standalone", 0, None)
}

fn source_eml_view_from_path(
    source_path: &Path,
    eml_path: String,
    source_kind: &str,
    message_id: i64,
    attachments: Option<Vec<Attachment>>,
) -> AppResult<SourceEmlView> {
    let mut message = if source_kind == "workspace" {
        standalone_eml_message(source_path)?
    } else {
        standalone_message_from_path(source_path)?
    };
    if let Some(attachments) = attachments {
        message.attachments = attachments;
    }

    let render = if message.body_html.trim().is_empty() {
        SanitizedEmailHtml {
            sanitized_html: String::new(),
            remote_images_blocked: false,
            remote_image_count: 0,
            embedded_image_count: 0,
        }
    } else {
        sanitize_email_html(&message.body_html, message.cid_images.clone())
    };
    let body_html_available = !message.body_html.trim().is_empty();
    let recipients = format_recipients_summary(&message.to, &message.cc, &message.bcc);

    Ok(SourceEmlView {
        message_id,
        eml_path,
        source_path: source_path.display().to_string(),
        source_kind: source_kind.to_string(),
        source_format: message.source_format.as_str().to_string(),
        source_label: message.source_format.label().to_string(),
        message_class: message.message_class,
        subject: message.subject,
        sender: message.sender,
        recipients,
        date: message.date,
        body_text: message.body_text,
        body_source: message.body_source.as_str().to_string(),
        body_html_available,
        sanitized_html: render.sanitized_html,
        remote_images_blocked: render.remote_images_blocked,
        remote_image_count: render.remote_image_count,
        embedded_image_count: render.embedded_image_count,
        raw_source: message.raw_source,
        raw_source_available: message.raw_source_available,
        parse_warnings: message.parse_warnings,
        attachments: message.attachments,
        inline_resources: message.inline_resources,
        message_id_header: message.message_id_header,
        in_reply_to: message.in_reply_to,
        references_header: message.references_header,
        normalized_subject: message.normalized_subject,
        calendar: message.calendar,
        in_local_trash: false,
        local_trash_available: source_kind == "workspace",
        local_trash_error_code: None,
    })
}

fn standalone_message_from_path(source_path: &Path) -> AppResult<StandaloneMessage> {
    match StandaloneSourceFormat::from_path(source_path)? {
        StandaloneSourceFormat::Eml => standalone_eml_message(source_path),
        StandaloneSourceFormat::Msg => standalone_msg_message(source_path),
    }
}

fn standalone_eml_message(source_path: &Path) -> AppResult<StandaloneMessage> {
    let bytes = read_bounded_message_source(source_path)?;
    let raw_source = String::from_utf8_lossy(&bytes).to_string();
    let parsed = parse_eml(source_path)?;
    let attachments = parsed
        .attachments
        .iter()
        .map(attachment_from_draft)
        .collect();
    let cid_images = load_cid_images_from_eml_path(source_path).unwrap_or_default();

    Ok(StandaloneMessage {
        source_format: StandaloneSourceFormat::Eml,
        message_class: String::new(),
        subject: parsed.subject,
        sender: parsed.sender,
        to: parsed.recipients,
        cc: String::new(),
        bcc: String::new(),
        date: parsed.date,
        body_text: parsed.body,
        body_html: parsed.body_html,
        body_source: parsed.body_source,
        attachments,
        cid_images,
        raw_source,
        raw_source_available: true,
        parse_warnings: Vec::new(),
        inline_resources: Vec::new(),
        message_id_header: parsed.message_id_header_raw,
        in_reply_to: parsed.in_reply_to_raw,
        references_header: parsed.references_header_raw,
        normalized_subject: parsed.normalized_subject,
        calendar: None,
    })
}

fn standalone_msg_message(source_path: &Path) -> AppResult<StandaloneMessage> {
    let outlook = parse_msg(source_path)?;
    let mut parse_warnings = Vec::new();
    let message_id_header = first_non_empty([
        outlook.headers.message_id.as_str(),
        transport_header_value(&outlook.headers.raw, "Message-ID")
            .as_deref()
            .unwrap_or_default(),
    ]);
    let in_reply_to =
        transport_header_value(&outlook.headers.raw, "In-Reply-To").unwrap_or_default();
    let references_header =
        transport_header_value(&outlook.headers.raw, "References").unwrap_or_default();
    let message_class = outlook.message_class.trim().to_string();
    let calendar = calendar_msg::calendar_item_details(source_path, &outlook);
    if !message_class.is_empty() && !message_class.starts_with("IPM.Note") && calendar.is_none() {
        parse_warnings.push(format!(
            "This Outlook item type has limited preview support: {message_class}."
        ));
    } else if message_class.is_empty() {
        parse_warnings.push("Message class was not present in this MSG.".to_string());
    }

    let plain_text = normalize_plain_text_body(&outlook.body);
    let direct_html = outlook.html.trim().to_string();
    let native_rtf_present = !outlook.rtf_compressed.trim().is_empty();
    let mut rtf_candidates = Vec::new();

    if let Some(rtf) = outlook.rtf_decompressed() {
        let body = String::from_utf8_lossy(&rtf);
        if let Some(conversion) = convert_rtf_payload(&body) {
            rtf_candidates.push(MsgRtfBodyCandidate {
                source_label: "native compressed RTF body property".to_string(),
                attachment_index: None,
                conversion,
            });
        } else {
            parse_warnings.push(
                "Native compressed RTF body was present but could not be converted.".to_string(),
            );
        }
    } else if native_rtf_present {
        parse_warnings.push("Native compressed RTF body could not be decompressed.".to_string());
    }

    let mut body_like_rtf_attachment_indexes = HashSet::new();
    for (index, attachment) in outlook.attachments.iter().enumerate() {
        if !msg_attachment_is_body_like_rtf(attachment) {
            continue;
        }
        body_like_rtf_attachment_indexes.insert(index);
        let filename = msg_attachment_filename(index, attachment);
        if let Some(conversion) = convert_rtf_bytes(&attachment.payload_bytes) {
            parse_warnings.push(format!(
                "Body-like RTF attachment detected: {filename} ({}).",
                conversion.kind.as_str()
            ));
            rtf_candidates.push(MsgRtfBodyCandidate {
                source_label: format!("body-like RTF attachment {filename}"),
                attachment_index: Some(index),
                conversion,
            });
        } else {
            parse_warnings.push(format!(
                "Body-like RTF attachment detected but could not be converted: {filename}."
            ));
        }
    }

    let html_rtf_candidate = rtf_candidates
        .iter()
        .filter(|candidate| {
            candidate
                .conversion
                .html
                .as_ref()
                .is_some_and(|html| !html.trim().is_empty())
        })
        .max_by_key(|candidate| {
            candidate
                .conversion
                .html
                .as_ref()
                .map_or(0, |html| html.trim().len())
        });
    let text_rtf_candidate = rtf_candidates
        .iter()
        .filter(|candidate| !candidate.conversion.text.trim().is_empty())
        .max_by_key(|candidate| candidate.conversion.text.trim().len());

    let (body_text, body_html, body_source, selected_rtf_source, promoted_rtf_index) =
        if !direct_html.is_empty() {
            let body_text = if plain_text.trim().is_empty() {
                html_to_text(&direct_html)
            } else {
                plain_text.clone()
            };
            (
                body_text,
                direct_html,
                BodySource::HtmlConverted,
                "direct MSG HTML body".to_string(),
                None,
            )
        } else if let Some(candidate) = html_rtf_candidate {
            let html = candidate.conversion.html.clone().unwrap_or_default();
            parse_warnings.push(format!(
                "HTML recovered from {} and selected as the message body.",
                candidate.source_label
            ));
            (
                html_to_text(&html),
                html,
                BodySource::RtfHtmlConverted,
                candidate.source_label.clone(),
                candidate.attachment_index,
            )
        } else if let Some(candidate) = text_rtf_candidate {
            parse_warnings.push(format!(
                "Plain text recovered from {} and selected as the message body.",
                candidate.source_label
            ));
            (
                candidate.conversion.text.clone(),
                String::new(),
                BodySource::RtfConverted,
                candidate.source_label.clone(),
                candidate.attachment_index,
            )
        } else if !plain_text.trim().is_empty() {
            (
                plain_text.clone(),
                String::new(),
                BodySource::TextPlain,
                "direct MSG plain-text body".to_string(),
                None,
            )
        } else {
            (
                String::new(),
                String::new(),
                BodySource::Missing,
                "no readable body".to_string(),
                None,
            )
        };
    validate_rendered_message_text(&body_text, &body_html)?;

    if let Some(index) = promoted_rtf_index {
        parse_warnings.push(format!(
            "{} was promoted to the message body and hidden from Attachments.",
            msg_attachment_filename(index, &outlook.attachments[index])
        ));
    }

    let referenced_cids = html_cid_references(&body_html);
    let mut resolved_cids = HashSet::new();
    let mut attachments = Vec::new();
    let mut inline_resources = Vec::new();
    let mut cid_images = HashMap::new();
    let mut attachment_diagnostics = Vec::new();

    for (index, attachment) in outlook.attachments.iter().enumerate() {
        let mut metadata = msg_attachment_metadata(index, attachment);
        let content_id = normalize_cid_value(&attachment.content_id);
        let is_safe_image = is_safe_embedded_image_mime(&metadata.content_type)
            && !attachment.payload_bytes.is_empty()
            && attachment.payload_bytes.len() <= MAX_EMBEDDED_IMAGE_BYTES;
        let is_referenced_inline =
            is_safe_image && !content_id.is_empty() && referenced_cids.contains(&content_id);

        let (classification, classification_reason) = if promoted_rtf_index == Some(index) {
            (
                "promoted RTF body".to_string(),
                "Known body-like RTF attachment selected as the message body and suppressed from the normal attachment list.".to_string(),
            )
        } else if is_referenced_inline {
            metadata.content_disposition = "inline".to_string();
            let data_url = format!(
                "data:{};base64,{}",
                metadata.content_type,
                BASE64_STANDARD.encode(&attachment.payload_bytes)
            );
            cid_images.insert(content_id.clone(), data_url);
            resolved_cids.insert(content_id.clone());
            inline_resources.push(metadata.clone());
            (
                "inline".to_string(),
                "Exact Content-ID reference found in the selected message HTML.".to_string(),
            )
        } else {
            if is_safe_image && !content_id.is_empty() {
                metadata.content_disposition = "possibly-inline".to_string();
            }
            attachments.push(metadata.clone());
            if body_like_rtf_attachment_indexes.contains(&index) {
                (
                    "body-like RTF candidate".to_string(),
                    "Known body filename and RTF content were detected, but a higher-priority body source was selected or conversion failed.".to_string(),
                )
            } else if is_safe_image && !content_id.is_empty() {
                (
                    "possibly inline".to_string(),
                    "Image has a Content-ID, but the selected HTML does not reference it; kept in Attachments.".to_string(),
                )
            } else {
                (
                    "attachment".to_string(),
                    "No supported inline-body reference matched this attachment.".to_string(),
                )
            }
        };

        attachment_diagnostics.push(MsgAttachmentDiagnostic {
            id: index as i64 + 1,
            filename: metadata.filename,
            content_type: metadata.content_type,
            content_id,
            attach_method: attachment.attach_method,
            hidden: None,
            rendering_position: None,
            attachment_flags: None,
            content_location: None,
            classification,
            classification_reason,
        });
    }

    let unresolved_cids = referenced_cids
        .difference(&resolved_cids)
        .cloned()
        .collect::<Vec<_>>();
    if !unresolved_cids.is_empty() {
        parse_warnings.push(
            "Some legacy inline images could not be placed in the original message layout."
                .to_string(),
        );
    }
    if !inline_resources.is_empty() {
        parse_warnings.push(format!(
            "Reconstructed {} inline image resource(s) from exact Content-ID references.",
            inline_resources.len()
        ));
    }

    let raw_source = msg_structured_raw_source(MsgStructuredRawSource {
        source_path,
        outlook: &outlook,
        body_html: &body_html,
        body_text: &body_text,
        body_source,
        selected_body_source: &selected_rtf_source,
        native_rtf_present,
        body_like_rtf_attachment_detected: !body_like_rtf_attachment_indexes.is_empty(),
        promoted_rtf_index,
        rtf_html_recovered: !body_html.is_empty() && body_source == BodySource::RtfHtmlConverted,
        unresolved_cids: &unresolved_cids,
        parse_warnings: &parse_warnings,
        attachments: &attachment_diagnostics,
        calendar: calendar.as_ref(),
    });
    let normalized_subject = normalize_thread_subject(&outlook.subject);

    Ok(StandaloneMessage {
        source_format: StandaloneSourceFormat::Msg,
        message_class,
        subject: outlook.subject,
        sender: format_msg_person(&outlook.sender),
        to: format_msg_people(&outlook.to),
        cc: format_msg_people(&outlook.cc),
        bcc: format_msg_people(&outlook.bcc),
        date: first_non_empty([
            outlook.message_delivery_time.as_str(),
            outlook.client_submit_time.as_str(),
            outlook.creation_time.as_str(),
            outlook.last_modification_time.as_str(),
            outlook.headers.date.as_str(),
        ]),
        body_text,
        body_html,
        body_source,
        attachments,
        cid_images,
        raw_source,
        raw_source_available: false,
        parse_warnings,
        inline_resources,
        message_id_header,
        in_reply_to,
        references_header,
        normalized_subject,
        calendar,
    })
}

fn parse_msg(source_path: &Path) -> AppResult<msg_parser::Outlook> {
    let file = open_bounded_message_file(source_path)?;
    let outlook = msg_parser::Outlook::from_reader(file)
        .map_err(|_| AppError::new("Could not parse Outlook MSG."))?;
    validate_outlook_message(&outlook)?;
    Ok(outlook)
}

fn parse_mail_bytes(bytes: &[u8]) -> AppResult<mailparse::ParsedMail<'_>> {
    validate_mime_preflight(bytes)?;
    let parsed = mailparse::parse_mail(bytes).map_err(|_| AppError::new("Could not parse EML."))?;
    validate_parsed_mail(&parsed)?;
    Ok(parsed)
}

fn transport_header_value(raw_headers: &str, target_name: &str) -> Option<String> {
    let mut current_name = String::new();
    let mut current_value = String::new();
    let mut result = None;

    let flush = |name: &str, value: &str, result: &mut Option<String>| {
        if name.eq_ignore_ascii_case(target_name) && result.is_none() {
            *result = Some(value.split_whitespace().collect::<Vec<_>>().join(" "));
        }
    };

    for line in raw_headers.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if !current_name.is_empty() {
                current_value.push(' ');
                current_value.push_str(line.trim());
            }
            continue;
        }

        flush(&current_name, &current_value, &mut result);
        if result.is_some() {
            break;
        }
        let Some((name, value)) = line.split_once(':') else {
            current_name.clear();
            current_value.clear();
            continue;
        };
        current_name = name.trim().to_string();
        current_value = value.trim().to_string();
    }
    flush(&current_name, &current_value, &mut result);
    result.filter(|value| !value.trim().is_empty())
}

fn format_msg_person(person: &msg_parser::Person) -> String {
    if person.name.trim().is_empty() {
        person.email.trim().to_string()
    } else if person.email.trim().is_empty() || person.name.trim() == person.email.trim() {
        person.name.trim().to_string()
    } else {
        format!("{} <{}>", person.name.trim(), person.email.trim())
    }
}

fn format_msg_people(people: &[msg_parser::Person]) -> String {
    people
        .iter()
        .map(format_msg_person)
        .filter(|value| !value.trim().is_empty())
        .collect::<Vec<_>>()
        .join("; ")
}

fn format_recipients_summary(to: &str, cc: &str, bcc: &str) -> String {
    let mut fields = Vec::new();
    if !to.trim().is_empty() {
        fields.push(format!("To: {}", to.trim()));
    }
    if !cc.trim().is_empty() {
        fields.push(format!("Cc: {}", cc.trim()));
    }
    if !bcc.trim().is_empty() {
        fields.push(format!("Bcc: {}", bcc.trim()));
    }
    fields.join("; ")
}

fn first_non_empty<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    values
        .into_iter()
        .find(|value| !value.trim().is_empty())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn msg_attachment_filename(index: usize, attachment: &msg_parser::Attachment) -> String {
    let mut filename = first_non_empty([
        attachment.long_file_name.as_str(),
        attachment.file_name.as_str(),
        attachment.display_name.as_str(),
    ]);
    if filename.is_empty() {
        filename = if attachment.is_embedded_message() {
            format!("embedded-message-{}.msg", index + 1)
        } else if attachment.extension.trim().is_empty() {
            format!("attachment-{}", index + 1)
        } else {
            format!("attachment-{}{}", index + 1, attachment.extension.trim())
        };
    }
    if attachment.is_embedded_message() && !filename.to_ascii_lowercase().ends_with(".msg") {
        filename.push_str(".msg");
    }
    filename
}

fn msg_attachment_is_body_like_rtf(attachment: &msg_parser::Attachment) -> bool {
    let filename = first_non_empty([
        attachment.long_file_name.as_str(),
        attachment.file_name.as_str(),
        attachment.display_name.as_str(),
    ])
    .to_ascii_lowercase();
    if !is_known_msg_rtf_body_filename(&filename) {
        return false;
    }

    let content_type = attachment.mime_tag.trim().to_ascii_lowercase();
    let has_rtf_type = matches!(
        content_type.as_str(),
        "text/rtf" | "application/rtf" | "application/x-rtf"
    );
    has_rtf_type || find_rtf_start_in_bytes(&attachment.payload_bytes).is_some()
}

fn is_known_msg_rtf_body_filename(filename: &str) -> bool {
    matches!(
        filename.trim().to_ascii_lowercase().as_str(),
        "rtf-body.rtf" | "body.rtf" | "message.rtf"
    )
}

fn convert_rtf_bytes(bytes: &[u8]) -> Option<RtfConversion> {
    let start = find_rtf_start_in_bytes(bytes)?;
    let body = String::from_utf8_lossy(&bytes[start..]);
    convert_rtf_payload(&body)
}

fn html_cid_references(html: &str) -> HashSet<String> {
    html_img_tags(html)
        .filter_map(|tag| html_tag_attribute(&tag, "src").map(str::to_string))
        .filter(|src| {
            src.trim()
                .get(0..4)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cid:"))
        })
        .map(|src| normalize_cid_value(&src))
        .filter(|cid| !cid.is_empty())
        .collect()
}

fn msg_attachment_metadata(index: usize, attachment: &msg_parser::Attachment) -> Attachment {
    let filename = msg_attachment_filename(index, attachment);

    let content_type = if attachment.is_embedded_message() {
        "application/vnd.ms-outlook".to_string()
    } else if attachment.mime_tag.trim().is_empty() {
        "application/octet-stream".to_string()
    } else {
        attachment.mime_tag.trim().to_string()
    };
    let content_disposition = if attachment.is_embedded_message() {
        "embedded-message"
    } else if attachment.attach_method == 6 {
        "ole-object"
    } else {
        "attachment"
    };

    Attachment {
        id: index as i64 + 1,
        filename: filename.clone(),
        sanitized_filename: sanitize_attachment_filename(&filename),
        content_type,
        size_bytes: Some(attachment.payload_bytes.len() as i64),
        attachment_index: index as i64,
        content_disposition: content_disposition.to_string(),
    }
}

struct MsgStructuredRawSource<'a> {
    source_path: &'a Path,
    outlook: &'a msg_parser::Outlook,
    body_html: &'a str,
    body_text: &'a str,
    body_source: BodySource,
    selected_body_source: &'a str,
    native_rtf_present: bool,
    body_like_rtf_attachment_detected: bool,
    promoted_rtf_index: Option<usize>,
    rtf_html_recovered: bool,
    unresolved_cids: &'a [String],
    parse_warnings: &'a [String],
    attachments: &'a [MsgAttachmentDiagnostic],
    calendar: Option<&'a CalendarItemDetails>,
}

fn msg_structured_raw_source(input: MsgStructuredRawSource<'_>) -> String {
    let MsgStructuredRawSource {
        source_path,
        outlook,
        body_html,
        body_text,
        body_source,
        selected_body_source,
        native_rtf_present,
        body_like_rtf_attachment_detected,
        promoted_rtf_index,
        rtf_html_recovered,
        unresolved_cids,
        parse_warnings,
        attachments,
        calendar,
    } = input;
    let file_size = source_path.metadata().ok().map(|metadata| metadata.len());
    let mut lines = vec![
        "Outlook MSG structured diagnostics".to_string(),
        format!("Source path: {}", source_path.display()),
        format!(
            "File size: {}",
            file_size
                .map(format_bytes)
                .unwrap_or_else(|| "unknown".to_string())
        ),
        format!("Message class: {}", empty_label(&outlook.message_class)),
        format!("Subject: {}", empty_label(&outlook.subject)),
        format!(
            "Sender: {}",
            empty_label(&format_msg_person(&outlook.sender))
        ),
        format!("To: {}", empty_label(&format_msg_people(&outlook.to))),
        format!("Cc: {}", empty_label(&format_msg_people(&outlook.cc))),
        format!("Bcc: {}", empty_label(&format_msg_people(&outlook.bcc))),
        format!("Delivered: {}", empty_label(&outlook.message_delivery_time)),
        format!("Submitted: {}", empty_label(&outlook.client_submit_time)),
        format!("Created: {}", empty_label(&outlook.creation_time)),
        format!("Modified: {}", empty_label(&outlook.last_modification_time)),
        format!("Direct HTML present: {}", !outlook.html.trim().is_empty()),
        format!("Plain body present: {}", !outlook.body.trim().is_empty()),
        format!("Native/compressed RTF present: {native_rtf_present}"),
        format!(
            "Body-like RTF attachment detected: {body_like_rtf_attachment_detected}"
        ),
        format!("Selected body source: {} ({})", body_source.as_str(), selected_body_source),
        format!("RTF-to-HTML recovery succeeded: {rtf_html_recovered}"),
        format!("Readable plain text available: {}", !body_text.trim().is_empty()),
        format!("Renderable HTML available: {}", !body_html.trim().is_empty()),
        format!(
            "Promoted RTF attachment suppressed: {}",
            promoted_rtf_index.is_some()
        ),
        format!("Unresolved inline CID references: {}", unresolved_cids.len()),
        format!("Original attachment records: {}", attachments.len()),
        "Attachment hidden/flags/rendering-position/content-location metadata: unavailable in msg_parser 0.3.6".to_string(),
    ];

    if !parse_warnings.is_empty() {
        lines.push(String::new());
        lines.push("Parse warnings:".to_string());
        lines.extend(parse_warnings.iter().map(|warning| format!("- {warning}")));
    }

    if !attachments.is_empty() {
        lines.push(String::new());
        lines.push("Attachments:".to_string());
        lines.extend(attachments.iter().map(|attachment| {
            format!(
                "- [{}] {} | type={} | content_id={} | method={} | hidden={} | rendering_position={} | attachment_flags={} | content_location={} | classification={} | reason={}",
                attachment.id,
                empty_label(&attachment.filename),
                empty_label(&attachment.content_type),
                empty_label(&attachment.content_id),
                attachment.attach_method,
                diagnostic_option_label(attachment.hidden),
                diagnostic_option_label(attachment.rendering_position),
                diagnostic_option_label(attachment.attachment_flags),
                attachment.content_location.as_deref().unwrap_or("unavailable"),
                attachment.classification,
                attachment.classification_reason,
            )
        }));
    }

    if let Some(calendar) = calendar {
        lines.push(String::new());
        lines.extend(calendar_msg::diagnostic_lines(calendar));
    }

    lines.join("\n")
}

fn diagnostic_option_label<T: std::fmt::Display>(value: Option<T>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "unavailable".to_string())
}

fn empty_label(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        "(none)".to_string()
    } else {
        value.to_string()
    }
}

#[tauri::command]
fn reveal_eml(state: State<AppState>, workspace_id: String, message_id: i64) -> AppResult<()> {
    let workspace = resolve_workspace_for_id(&state, &workspace_id)?;
    let conn = open_workspace_db_for_read(&workspace)?;
    let relative_path: String = conn
        .query_row(
            "SELECT eml_path FROM messages WHERE id = ?1",
            params![message_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or_else(|| AppError::new("Message was not found in this workspace."))?;

    let eml_path = workspace.join("extracted").join(relative_path);
    reveal_path(&eml_path)
}

#[tauri::command]
fn export_original_eml(
    state: State<AppState>,
    workspace_id: String,
    message_id: i64,
) -> AppResult<ExportOriginalEmlResult> {
    match export_original_eml_inner(&state, &workspace_id, message_id) {
        Ok(result) => Ok(result),
        Err(error) => {
            if let Ok(active) = active_workspace(&state) {
                let _ = append_export_log(
                    &active.path,
                    &format!("EML export failed: message_id={message_id}"),
                );
            }
            Ok(ExportOriginalEmlResult::failed(message_id, error.message))
        }
    }
}

#[tauri::command]
fn save_source_eml_as(
    state: State<AppState>,
    workspace_id: String,
    message_id: i64,
) -> AppResult<ExportOriginalEmlResult> {
    match save_source_eml_as_inner(&state, &workspace_id, message_id) {
        Ok(result) => Ok(result),
        Err(error) => {
            if let Ok(active) = active_workspace(&state) {
                let _ = append_export_log(
                    &active.path,
                    &format!("Save source EML failed: message_id={message_id}"),
                );
            }
            Ok(ExportOriginalEmlResult::failed(message_id, error.message))
        }
    }
}

#[tauri::command]
fn save_standalone_source_message_as(
    state: State<AppState>,
    stable_id: String,
) -> AppResult<ExportOriginalEmlResult> {
    match save_standalone_source_message_as_inner(&state, &stable_id) {
        Ok(result) => Ok(result),
        Err(error) => Ok(ExportOriginalEmlResult::failed(0, error.message)),
    }
}

#[tauri::command]
fn save_printable_html_as(
    state: State<AppState>,
    default_filename: String,
    html: String,
) -> AppResult<SavePrintableHtmlResult> {
    let default_filename = sanitize_printable_html_filename(&default_filename);
    let html = sanitize_printable_html_document(&html)?;
    let Some(mut output_path) = rfd::FileDialog::new()
        .set_title("Save Printable HTML")
        .set_file_name(&default_filename)
        .add_filter("HTML", &["html", "htm"])
        .save_file()
    else {
        return Ok(SavePrintableHtmlResult::cancelled(default_filename));
    };

    if output_path.extension().is_none() {
        output_path.set_extension("html");
    }
    if output_path.exists() {
        return Err(AppError::new(
            "Destination already exists. Choose a different filename.",
        ));
    }

    let mut output = create_private_file_new(&output_path)?;
    output.write_all(html.as_bytes())?;
    output.flush()?;
    drop(output);

    let output_path = register_export_file(&state, &output_path)?;
    let filename = output_path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_string();
    let size_bytes = output_path
        .metadata()
        .ok()
        .map(|metadata| metadata.len() as i64);

    Ok(SavePrintableHtmlResult {
        saved: true,
        filename,
        output_path: Some(output_path.display().to_string()),
        size_bytes,
        error: None,
    })
}

#[tauri::command]
fn reveal_saved_html(state: State<AppState>, output_path: String) -> AppResult<()> {
    let path = authorized_export_path(&state, &output_path)?;
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension != "html" && extension != "htm" {
        return Err(AppError::new(format!(
            "Refusing to reveal a non-HTML printable export: {}",
            path.display()
        )));
    }
    reveal_path(&path)
}

#[tauri::command]
fn reveal_saved_eml(state: State<AppState>, output_path: String) -> AppResult<()> {
    let path = authorized_export_path(&state, &output_path)?;
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    if extension != "eml" && extension != "msg" {
        return Err(AppError::new(format!(
            "Refusing to reveal a non-message source export: {}",
            path.display()
        )));
    }
    reveal_path(&path)
}

#[tauri::command]
fn export_attachment(
    state: State<AppState>,
    workspace_id: String,
    message_id: i64,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    match export_attachment_inner(&state, &workspace_id, message_id, attachment_id) {
        Ok(result) => Ok(result),
        Err(error) => {
            if let Ok(active) = active_workspace(&state) {
                let _ = append_export_log(
                    &active.path,
                    &format!(
                        "Export failed: message_id={message_id} attachment_id={attachment_id}"
                    ),
                );
            }
            Ok(ExportAttachmentResult::failed(attachment_id, error.message))
        }
    }
}

#[tauri::command]
fn open_attachment(
    state: State<AppState>,
    workspace_id: String,
    message_id: i64,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    match open_attachment_inner(&state, &workspace_id, message_id, attachment_id) {
        Ok(result) => Ok(result),
        Err(error) => {
            if let Ok(active) = active_workspace_for_id(&state, &workspace_id) {
                let _ = append_export_log(
                    &active.path,
                    &format!(
                        "Open attachment failed: message_id={message_id} attachment_id={attachment_id}"
                    ),
                );
            }
            Ok(ExportAttachmentResult::failed(attachment_id, error.message))
        }
    }
}

#[tauri::command]
fn export_standalone_message_attachment(
    state: State<AppState>,
    stable_id: String,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    match export_standalone_message_attachment_inner(&state, &stable_id, attachment_id) {
        Ok(result) => Ok(result),
        Err(error) => Ok(ExportAttachmentResult::failed(attachment_id, error.message)),
    }
}

#[tauri::command]
fn open_standalone_message_attachment(
    state: State<AppState>,
    stable_id: String,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    match open_standalone_message_attachment_inner(&state, &stable_id, attachment_id) {
        Ok(result) => Ok(result),
        Err(error) => Ok(ExportAttachmentResult::failed(attachment_id, error.message)),
    }
}

#[tauri::command]
fn reveal_exported_file(state: State<AppState>, output_path: String) -> AppResult<()> {
    let active = active_workspace(&state)?;
    reveal_exported_file_in_workspace(&state, &active.path, &output_path)
}

#[tauri::command]
fn reveal_exported_file_for_workspace(
    state: State<AppState>,
    workspace_id: String,
    output_path: String,
) -> AppResult<()> {
    let active = active_workspace_for_id(&state, &workspace_id)?;
    reveal_exported_file_in_workspace(&state, &active.path, &output_path)
}

#[tauri::command]
fn reveal_standalone_exported_file(state: State<AppState>, output_path: String) -> AppResult<()> {
    let path = authorized_export_path(&state, &output_path)?;
    if !path_is_under_any_standalone_export_root(state.runtime_profile()?.paths(), &path)? {
        return Err(AppError::new(format!(
            "Refusing to reveal a file outside the standalone message export folders: {}",
            path.display()
        )));
    }
    reveal_path(&path)
}

fn reveal_exported_file_in_workspace(
    state: &AppState,
    workspace: &Path,
    output_path: &str,
) -> AppResult<()> {
    let workspace_root = workspace.canonicalize()?;
    if !workspace_root.join(WORKSPACE_MARKER_FILE).is_file() {
        return Err(AppError::new(format!(
            "Active workspace marker was not found: {}",
            workspace_root.join(WORKSPACE_MARKER_FILE).display()
        )));
    }
    let path = authorized_export_path(state, output_path)?;
    let exports_root = workspace_root.join("exports");
    if !path_is_under_root(&path, &exports_root)? {
        return Err(AppError::new(format!(
            "Refusing to reveal a file outside this workspace export folder: {}",
            path.display()
        )));
    }
    reveal_path(&path)
}

fn open_attachment_inner(
    state: &State<AppState>,
    workspace_id: &str,
    message_id: i64,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    let result = export_attachment_inner(state, workspace_id, message_id, attachment_id)?;
    let output_path = result
        .output_path
        .as_deref()
        .ok_or_else(|| AppError::new("Attachment export did not produce an output path."))?;
    let active = active_workspace_for_id(state, workspace_id)?;
    let workspace_root = active.path.canonicalize()?;
    let exports_root = workspace_root.join("exports");
    let path = authorized_export_path(state, output_path)?;

    if !path_is_under_root(&path, &exports_root)? {
        return Err(AppError::new(format!(
            "Refusing to open a file outside this workspace export folder: {}",
            path.display()
        )));
    }

    open_path_with_default_app(&path)?;
    append_export_log(
        &workspace_root,
        &format!(
            "Opened exported attachment copy: message_id={message_id} attachment_id={attachment_id}"
        ),
    )?;

    Ok(result)
}

fn open_standalone_message_attachment_inner(
    state: &State<AppState>,
    stable_id: &str,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    let source_path = authorized_standalone_path(state, stable_id)?;
    if StandaloneSourceFormat::from_path(&source_path)? == StandaloneSourceFormat::Msg {
        let outlook = parse_msg(&source_path)?;
        let attachment = msg_attachment_by_id(&outlook, attachment_id)?;
        if attachment.attach_method == 6 {
            return Err(AppError::new(
                "OLE object attachments can be exported but are not opened automatically.",
            ));
        }
    }

    let result =
        export_standalone_message_attachment_from_path(state, &source_path, attachment_id)?;
    let output_path = result
        .output_path
        .as_deref()
        .ok_or_else(|| AppError::new("Attachment export did not produce an output path."))?;
    let path = authorized_export_path(state, output_path)?;
    if !path_is_under_any_standalone_export_root(state.runtime_profile()?.paths(), &path)? {
        return Err(AppError::new(format!(
            "Refusing to open a file outside the standalone message export folders: {}",
            path.display()
        )));
    }

    open_path_with_default_app(&path)?;
    Ok(result)
}

fn export_standalone_message_attachment_inner(
    state: &State<AppState>,
    stable_id: &str,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    let source_path = authorized_standalone_path(state, stable_id)?;
    export_standalone_message_attachment_from_path(state, &source_path, attachment_id)
}

fn export_standalone_message_attachment_from_path(
    state: &State<AppState>,
    source_path: &Path,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    match StandaloneSourceFormat::from_path(source_path)? {
        StandaloneSourceFormat::Eml => {
            export_standalone_eml_attachment_from_path(state, source_path, attachment_id)
        }
        StandaloneSourceFormat::Msg => {
            export_standalone_msg_attachment_from_path(state, source_path, attachment_id)
        }
    }
}

fn export_standalone_eml_attachment_from_path(
    state: &State<AppState>,
    source_path: &Path,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    let parsed_message = parse_eml(source_path)?;
    let attachment = parsed_message
        .attachments
        .iter()
        .find(|attachment| attachment.attachment_index + 1 == attachment_id)
        .ok_or_else(|| AppError::new("Attachment was not found in this EML."))?;

    let bytes = read_bounded_message_source(source_path)?;
    let parsed = parse_mail_bytes(&bytes)?;
    let mut counter = 0_i64;
    let target_part = find_attachment_part(
        &parsed,
        "0",
        attachment.attachment_index,
        Some(&attachment.mime_part_path),
        &mut counter,
    )
    .ok_or_else(|| {
        AppError::new(format!(
            "Attachment part was not found in source EML for attachment id {attachment_id}."
        ))
    })?;

    let decoded = target_part.get_body_raw().map_err(|error| {
        AppError::new(format!(
            "Could not decode attachment body for attachment id {attachment_id}: {error}"
        ))
    })?;
    if decoded.len() > MAX_STANDALONE_ATTACHMENT_EXPORT_BYTES {
        return Err(AppError::new(format!(
            "Attachment {attachment_id} is too large to export safely."
        )));
    }
    let sanitized_filename =
        sanitize_attachment_filename(if attachment.sanitized_filename.trim().is_empty() {
            &attachment.filename
        } else {
            &attachment.sanitized_filename
        });
    if sanitized_filename.trim().is_empty()
        || sanitized_filename == "."
        || sanitized_filename == ".."
    {
        return Err(AppError::new("Attachment filename is not safe to export."));
    }

    let profile_paths = state.runtime_profile()?.paths();
    let export_dir =
        standalone_message_export_root(profile_paths, source_path)?.join("attachments");
    create_private_directory_all(&export_dir)?;
    let export_dir = export_dir.canonicalize()?;
    let (output_path, mut output) =
        create_unique_private_export_file(&export_dir, &sanitized_filename)?;
    if !path_parent_is_root(&output_path, &export_dir)? {
        return Err(AppError::new(
            "Refusing to write outside the standalone EML attachment export folder.",
        ));
    }

    output.write_all(&decoded)?;
    output.flush()?;
    drop(output);
    let output_path = register_export_file(state, &output_path)?;
    if !path_is_under_root(
        &output_path,
        &standalone_exports_dir(profile_paths, StandaloneSourceFormat::Eml)?,
    )? {
        return Err(AppError::new(format!(
            "Refusing to report an exported attachment outside the standalone EML export folder: {}",
            output_path.display()
        )));
    }

    Ok(ExportAttachmentResult {
        exported: true,
        attachment_id,
        filename: attachment.filename.clone(),
        sanitized_filename,
        output_path: Some(output_path.display().to_string()),
        size_bytes: Some(decoded.len() as i64).or(attachment.size_bytes),
        content_type: attachment.content_type.clone(),
        error: None,
    })
}

fn export_standalone_msg_attachment_from_path(
    state: &State<AppState>,
    source_path: &Path,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    let outlook = parse_msg(source_path)?;
    let attachment = msg_attachment_by_id(&outlook, attachment_id)?;
    let metadata = msg_attachment_metadata((attachment_id - 1) as usize, attachment);

    if attachment.payload_bytes.is_empty() {
        return Err(AppError::new(format!(
            "Attachment {attachment_id} has no exportable payload."
        )));
    }
    if attachment.payload_bytes.len() > MAX_STANDALONE_ATTACHMENT_EXPORT_BYTES {
        return Err(AppError::new(format!(
            "Attachment {} is too large to export safely ({} limit).",
            metadata.filename,
            format_bytes(MAX_STANDALONE_ATTACHMENT_EXPORT_BYTES as u64)
        )));
    }

    let sanitized_filename =
        sanitize_attachment_filename(if metadata.sanitized_filename.trim().is_empty() {
            &metadata.filename
        } else {
            &metadata.sanitized_filename
        });
    if sanitized_filename.trim().is_empty()
        || sanitized_filename == "."
        || sanitized_filename == ".."
    {
        return Err(AppError::new("Attachment filename is not safe to export."));
    }

    let profile_paths = state.runtime_profile()?.paths();
    let export_dir =
        standalone_message_export_root(profile_paths, source_path)?.join("attachments");
    create_private_directory_all(&export_dir)?;
    let export_dir = export_dir.canonicalize()?;
    let (output_path, mut output) =
        create_unique_private_export_file(&export_dir, &sanitized_filename)?;
    if !path_parent_is_root(&output_path, &export_dir)? {
        return Err(AppError::new(
            "Refusing to write outside the standalone MSG attachment export folder.",
        ));
    }

    output.write_all(&attachment.payload_bytes)?;
    output.flush()?;
    drop(output);
    let output_path = register_export_file(state, &output_path)?;
    if !path_is_under_root(
        &output_path,
        &standalone_exports_dir(profile_paths, StandaloneSourceFormat::Msg)?,
    )? {
        return Err(AppError::new(format!(
            "Refusing to report an exported attachment outside the standalone MSG export folder: {}",
            output_path.display()
        )));
    }

    Ok(ExportAttachmentResult {
        exported: true,
        attachment_id,
        filename: metadata.filename,
        sanitized_filename,
        output_path: Some(output_path.display().to_string()),
        size_bytes: Some(attachment.payload_bytes.len() as i64),
        content_type: metadata.content_type,
        error: None,
    })
}

fn msg_attachment_by_id(
    outlook: &msg_parser::Outlook,
    attachment_id: i64,
) -> AppResult<&msg_parser::Attachment> {
    if attachment_id < 1 {
        return Err(AppError::new("Attachment id must be positive."));
    }
    let index = usize::try_from(attachment_id - 1)
        .map_err(|_| AppError::new("Attachment id is outside the supported range."))?;
    outlook
        .attachments
        .get(index)
        .ok_or_else(|| AppError::new("Attachment was not found in this MSG."))
}

struct AttachmentExportLookup {
    eml_path: String,
    filename: String,
    sanitized_filename: String,
    content_type: String,
    size_bytes: Option<i64>,
    attachment_index: i64,
    mime_part_path: Option<String>,
}

struct SourceEmlLocation {
    workspace_root: PathBuf,
    relative_eml_path: String,
    source_path: PathBuf,
    subject: String,
    date: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StandaloneSourceFormat {
    Eml,
    Msg,
}

impl StandaloneSourceFormat {
    fn from_path(path: &Path) -> AppResult<Self> {
        match path
            .extension()
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "eml" => Ok(Self::Eml),
            "msg" => Ok(Self::Msg),
            _ => Err(AppError::new(format!(
                "Selected file is not a supported message file: {}",
                path.display()
            ))),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Eml => "eml",
            Self::Msg => "msg",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Eml => "EML Message",
            Self::Msg => "Outlook MSG Message",
        }
    }

    fn source_extension(self) -> &'static str {
        match self {
            Self::Eml => "eml",
            Self::Msg => "msg",
        }
    }
}

struct StandaloneMessage {
    source_format: StandaloneSourceFormat,
    message_class: String,
    subject: String,
    sender: String,
    to: String,
    cc: String,
    bcc: String,
    date: String,
    body_text: String,
    body_html: String,
    body_source: BodySource,
    attachments: Vec<Attachment>,
    cid_images: HashMap<String, String>,
    raw_source: String,
    raw_source_available: bool,
    parse_warnings: Vec<String>,
    inline_resources: Vec<Attachment>,
    message_id_header: String,
    in_reply_to: String,
    references_header: String,
    normalized_subject: String,
    calendar: Option<CalendarItemDetails>,
}

struct MsgRtfBodyCandidate {
    source_label: String,
    attachment_index: Option<usize>,
    conversion: RtfConversion,
}

struct MsgAttachmentDiagnostic {
    id: i64,
    filename: String,
    content_type: String,
    content_id: String,
    attach_method: u32,
    hidden: Option<bool>,
    rendering_position: Option<i32>,
    attachment_flags: Option<u32>,
    content_location: Option<String>,
    classification: String,
    classification_reason: String,
}

fn resolve_source_eml(
    state: &State<AppState>,
    workspace_id: &str,
    message_id: i64,
) -> AppResult<SourceEmlLocation> {
    let active = active_workspace_for_id(state, workspace_id)?;
    if !active.path.is_dir() {
        return Err(AppError::new(format!(
            "Workspace is unavailable: {}",
            active.path.display()
        )));
    }
    let workspace_root = active.path.canonicalize()?;
    if !workspace_root.join(WORKSPACE_MARKER_FILE).is_file() {
        return Err(AppError::new(format!(
            "Active workspace marker was not found: {}",
            workspace_root.join(WORKSPACE_MARKER_FILE).display()
        )));
    }

    let conn = open_workspace_db_for_read(&workspace_root)?;
    let (relative_eml_path, subject, date): (String, String, String) = conn
        .query_row(
            "SELECT eml_path, COALESCE(subject, ''), COALESCE(date, '')
               FROM messages
              WHERE id = ?1",
            params![message_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?
        .ok_or_else(|| AppError::new("Message was not found in this workspace."))?;

    let extracted_root = workspace_root.join("extracted");
    let source_path = extracted_root.join(&relative_eml_path);
    if !source_path.exists() {
        return Err(AppError::new(format!(
            "Source EML not found: {}",
            source_path.display()
        )));
    }
    if !path_is_under_root(&source_path, &extracted_root)? {
        return Err(AppError::new(
            "Refusing to read an EML outside this workspace.",
        ));
    }

    Ok(SourceEmlLocation {
        workspace_root,
        relative_eml_path,
        source_path: source_path.canonicalize()?,
        subject,
        date,
    })
}

fn save_source_eml_as_inner(
    state: &State<AppState>,
    workspace_id: &str,
    message_id: i64,
) -> AppResult<ExportOriginalEmlResult> {
    let source = resolve_source_eml(state, workspace_id, message_id)?;
    let default_filename = export_message_eml_filename(message_id, &source.date, &source.subject);
    let Some(output_path) = rfd::FileDialog::new()
        .set_title("Save Source EML As")
        .set_file_name(&default_filename)
        .add_filter("Email Message", &["eml"])
        .save_file()
    else {
        return Ok(ExportOriginalEmlResult {
            exported: false,
            message_id,
            filename: default_filename,
            workspace_path: Some(source.workspace_root.display().to_string()),
            export_dir: None,
            output_path: None,
            size_bytes: None,
            error: None,
        });
    };

    if output_path.exists() {
        return Err(AppError::new(format!(
            "Destination already exists. Choose a different filename: {}",
            output_path.display()
        )));
    }

    append_export_log(
        &source.workspace_root,
        &format!("Save source EML requested: message_id={message_id}"),
    )?;

    let mut input = File::open(&source.source_path)?;
    let mut output = create_private_file_new(&output_path)?;
    std::io::copy(&mut input, &mut output)?;
    output.flush()?;
    drop(output);

    let output_path = register_export_file(state, &output_path)?;
    let size_bytes = output_path
        .metadata()
        .ok()
        .map(|metadata| metadata.len() as i64);
    let result = ExportOriginalEmlResult {
        exported: true,
        message_id,
        filename: output_path
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_string(),
        workspace_path: Some(source.workspace_root.display().to_string()),
        export_dir: None,
        output_path: Some(output_path.display().to_string()),
        size_bytes,
        error: None,
    };

    append_export_log(
        &source.workspace_root,
        &format!(
            "Source EML saved: message_id={message_id} size_bytes={}",
            result.size_bytes.unwrap_or_default()
        ),
    )?;

    Ok(result)
}

fn save_standalone_source_message_as_inner(
    state: &State<AppState>,
    stable_id: &str,
) -> AppResult<ExportOriginalEmlResult> {
    let source_path = authorized_standalone_path(state, stable_id)?;
    let format = StandaloneSourceFormat::from_path(&source_path)?;
    let message = standalone_message_from_path(&source_path).ok();
    let default_filename = export_source_message_filename(
        format,
        message
            .as_ref()
            .map(|message| message.date.as_str())
            .unwrap_or_default(),
        message
            .as_ref()
            .map(|message| message.subject.as_str())
            .unwrap_or_else(|| {
                source_path
                    .file_stem()
                    .and_then(OsStr::to_str)
                    .unwrap_or("message")
            }),
    );
    let Some(output_path) = rfd::FileDialog::new()
        .set_title(format!(
            "Save Source {} As",
            format.source_extension().to_ascii_uppercase()
        ))
        .set_file_name(&default_filename)
        .add_filter(format.label(), &[format.source_extension()])
        .save_file()
    else {
        return Ok(ExportOriginalEmlResult {
            exported: false,
            message_id: 0,
            filename: default_filename,
            workspace_path: None,
            export_dir: None,
            output_path: None,
            size_bytes: None,
            error: None,
        });
    };

    if output_path.exists() {
        return Err(AppError::new(format!(
            "Destination already exists. Choose a different filename: {}",
            output_path.display()
        )));
    }

    let mut input = File::open(&source_path)?;
    let mut output = create_private_file_new(&output_path)?;
    std::io::copy(&mut input, &mut output)?;
    output.flush()?;
    drop(output);

    let output_path = register_export_file(state, &output_path)?;
    let size_bytes = output_path
        .metadata()
        .ok()
        .map(|metadata| metadata.len() as i64);
    Ok(ExportOriginalEmlResult {
        exported: true,
        message_id: 0,
        filename: output_path
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_string(),
        workspace_path: None,
        export_dir: None,
        output_path: Some(output_path.display().to_string()),
        size_bytes,
        error: None,
    })
}

fn export_original_eml_inner(
    state: &State<AppState>,
    workspace_id: &str,
    message_id: i64,
) -> AppResult<ExportOriginalEmlResult> {
    let active = active_workspace_for_id(state, workspace_id)?;
    if !active.path.is_dir() {
        return Err(AppError::new(format!(
            "Workspace is unavailable: {}",
            active.path.display()
        )));
    }
    let workspace_root = active.path.canonicalize()?;
    if !workspace_root.join(WORKSPACE_MARKER_FILE).is_file() {
        return Err(AppError::new(format!(
            "Active workspace marker was not found: {}",
            workspace_root.join(WORKSPACE_MARKER_FILE).display()
        )));
    }

    let conn = open_workspace_db_for_read(&workspace_root)?;
    let (relative_eml_path, subject, date): (String, String, String) = conn
        .query_row(
            "SELECT eml_path, COALESCE(subject, ''), COALESCE(date, '')
               FROM messages
              WHERE id = ?1",
            params![message_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?
        .ok_or_else(|| AppError::new("Message was not found in this workspace."))?;

    let extracted_root = workspace_root.join("extracted");
    let source_eml_path = extracted_root.join(&relative_eml_path);
    if !source_eml_path.exists() {
        return Err(AppError::new(format!(
            "Source EML not found for message export: {}",
            source_eml_path.display()
        )));
    }
    if !path_is_under_root(&source_eml_path, &extracted_root)? {
        return Err(AppError::new(
            "Refusing to read an EML outside this workspace.",
        ));
    }

    let export_dir = workspace_root.join("exports").join("messages");
    create_private_directory_all(&export_dir)?;
    let export_dir = export_dir.canonicalize()?;
    let filename = export_message_eml_filename(message_id, &date, &subject);
    let (output_path, mut output) = create_unique_private_export_file(&export_dir, &filename)?;
    if !path_parent_is_root(&output_path, &export_dir)? {
        return Err(AppError::new(
            "Refusing to write outside the message export folder.",
        ));
    }

    append_export_log(
        &workspace_root,
        &format!("EML export requested: message_id={message_id}"),
    )?;

    let mut input = File::open(&source_eml_path)?;
    std::io::copy(&mut input, &mut output)?;
    output.flush()?;
    drop(output);
    let output_path = register_export_file(state, &output_path)?;
    if !path_parent_is_root(&output_path, &export_dir)?
        || !path_is_under_root(&output_path, &export_dir)?
    {
        return Err(AppError::new(format!(
            "Refusing to report an exported EML outside this workspace export folder: {}",
            output_path.display()
        )));
    }
    let size_bytes = output_path
        .metadata()
        .ok()
        .map(|metadata| metadata.len() as i64);
    let result = ExportOriginalEmlResult {
        exported: true,
        message_id,
        filename: output_path
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or_default()
            .to_string(),
        workspace_path: Some(workspace_root.display().to_string()),
        export_dir: Some(export_dir.display().to_string()),
        output_path: Some(output_path.display().to_string()),
        size_bytes,
        error: None,
    };

    append_export_log(
        &workspace_root,
        &format!(
            "EML exported: message_id={message_id} size_bytes={}",
            result.size_bytes.unwrap_or_default()
        ),
    )?;

    Ok(result)
}

fn export_attachment_inner(
    state: &State<AppState>,
    workspace_id: &str,
    message_id: i64,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    let active = active_workspace_for_id(state, workspace_id)?;
    if !active.path.is_dir() {
        return Err(AppError::new(format!(
            "Workspace is unavailable: {}",
            active.path.display()
        )));
    }

    let conn = open_workspace_db_for_read(&active.path)?;
    let attachment = load_attachment_for_export(&conn, message_id, attachment_id)?;
    let eml_path = active.path.join("extracted").join(&attachment.eml_path);
    if !eml_path.exists() {
        return Err(AppError::new(format!(
            "Source EML not found for attachment export: {}",
            eml_path.display()
        )));
    }

    let extracted_root = active.path.join("extracted");
    if !path_is_under_root(&eml_path, &extracted_root)? {
        return Err(AppError::new(
            "Refusing to read an EML outside this workspace.",
        ));
    }

    append_export_log(
        &active.path,
        &format!("Export requested: message_id={message_id} attachment_id={attachment_id}"),
    )?;

    let bytes = read_bounded_message_source(&eml_path)?;
    let parsed = parse_mail_bytes(&bytes)?;
    let mut counter = 0_i64;
    let target_mime_part_path = attachment
        .mime_part_path
        .as_deref()
        .filter(|path| !path.is_empty());
    let mut target_part = find_attachment_part(
        &parsed,
        "0",
        attachment.attachment_index,
        target_mime_part_path,
        &mut counter,
    );
    if target_part.is_none() && target_mime_part_path.is_some() {
        counter = 0;
        target_part = find_attachment_part(
            &parsed,
            "0",
            attachment.attachment_index,
            None,
            &mut counter,
        );
    }
    let target_part = target_part.ok_or_else(|| {
        AppError::new(format!(
            "Attachment part was not found in source EML for attachment id {attachment_id}."
        ))
    })?;

    let decoded = target_part.get_body_raw().map_err(|error| {
        AppError::new(format!(
            "Could not decode attachment body for attachment id {attachment_id}: {error}"
        ))
    })?;
    if decoded.len() > MAX_STANDALONE_ATTACHMENT_EXPORT_BYTES {
        return Err(AppError::new(format!(
            "Attachment {attachment_id} is too large to export safely."
        )));
    }

    let sanitized_filename =
        sanitize_attachment_filename(if attachment.sanitized_filename.trim().is_empty() {
            &attachment.filename
        } else {
            &attachment.sanitized_filename
        });
    if sanitized_filename.trim().is_empty()
        || sanitized_filename == "."
        || sanitized_filename == ".."
    {
        return Err(AppError::new("Attachment filename is not safe to export."));
    }

    let export_dir = active.path.join("exports").join(message_id.to_string());
    create_private_directory_all(&export_dir)?;
    let (output_path, mut output) =
        create_unique_private_export_file(&export_dir, &sanitized_filename)?;
    if !path_parent_is_root(&output_path, &export_dir)? {
        return Err(AppError::new(
            "Refusing to write outside the message export folder.",
        ));
    }

    output.write_all(&decoded)?;
    output.flush()?;
    drop(output);
    let output_path = register_export_file(state, &output_path)?;

    let result = ExportAttachmentResult {
        exported: true,
        attachment_id,
        filename: attachment.filename.clone(),
        sanitized_filename,
        output_path: Some(output_path.display().to_string()),
        size_bytes: Some(decoded.len() as i64).or(attachment.size_bytes),
        content_type: attachment.content_type.clone(),
        error: None,
    };

    append_export_log(
        &active.path,
        &format!(
            "Exported: message_id={message_id} attachment_id={attachment_id} size_bytes={}",
            result.size_bytes.unwrap_or_default()
        ),
    )?;

    Ok(result)
}

fn load_attachment_for_export(
    conn: &Connection,
    message_id: i64,
    attachment_id: i64,
) -> AppResult<AttachmentExportLookup> {
    conn.query_row(
        "SELECT m.eml_path,
                COALESCE(a.filename, ''),
                COALESCE(a.sanitized_filename, ''),
                COALESCE(a.content_type, ''),
                a.size_bytes,
                a.attachment_index,
                a.mime_part_path
           FROM attachments a
           JOIN messages m ON m.id = a.message_id
          WHERE a.id = ?1
            AND a.message_id = ?2",
        params![attachment_id, message_id],
        |row| {
            Ok(AttachmentExportLookup {
                eml_path: row.get(0)?,
                filename: row.get(1)?,
                sanitized_filename: row.get(2)?,
                content_type: row.get(3)?,
                size_bytes: row.get(4)?,
                attachment_index: row.get(5)?,
                mime_part_path: row.get(6)?,
            })
        },
    )
    .optional()?
    .ok_or_else(|| AppError::new("Attachment was not found for this message."))
}

#[tauri::command]
fn reveal_import_log(state: State<AppState>) -> AppResult<()> {
    let active = active_workspace(&state)?;
    let log_path = import_log_path(&active.path);
    if !log_path.exists() {
        return Err(AppError::new(format!(
            "Import log not found: {}",
            log_path.display()
        )));
    }
    reveal_path(&log_path)
}

#[tauri::command]
fn reveal_original_pst_in_finder(state: State<AppState>) -> AppResult<()> {
    let active = active_workspace(&state)?;
    if !active.pst_path.exists() {
        return Err(AppError::new("Original PST not found."));
    }
    reveal_path(&active.pst_path)
}

#[tauri::command]
fn reveal_workspace_in_finder(state: State<AppState>) -> AppResult<()> {
    let active = active_workspace(&state)?;
    if !active.path.exists() {
        return Err(AppError::new(format!(
            "Workspace not found: {}",
            active.path.display()
        )));
    }
    reveal_path(&active.path)
}

#[tauri::command]
fn reveal_original_and_workspace_in_finder(state: State<AppState>) -> AppResult<()> {
    let active = active_workspace(&state)?;
    let mut failures = Vec::new();

    if !active.pst_path.exists() {
        failures.push("Original PST not found.".to_string());
    } else if let Err(error) = reveal_path(&active.pst_path) {
        failures.push(format!("Original PST: {}", error.message));
    }

    if !active.path.exists() {
        failures.push(format!("Workspace not found: {}", active.path.display()));
    } else if let Err(error) = reveal_path(&active.path) {
        failures.push(format!("Workspace: {}", error.message));
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(AppError::new(failures.join(" ")))
    }
}

#[tauri::command]
fn get_workspace_size(state: State<AppState>, workspace_id: String) -> AppResult<WorkspaceSize> {
    let active = active_workspace_for_id(&state, &workspace_id)?;
    workspace_size(&active.path, active.location_mode)
}

#[tauri::command]
fn delete_workspace(
    state: State<AppState>,
    workspace_id: Option<String>,
) -> AppResult<DeleteResult> {
    let active = active_workspace(&state)?;

    if state.import_running.load(Ordering::SeqCst) {
        let mut result = DeleteResult::new(&active);
        result.error = Some("Cancel import before deleting this workspace.".to_string());
        result.exists_after = active.path.exists();
        result.remaining_entries = remaining_entries(&active.path);
        result.message = delete_result_message(&result);
        return Ok(result);
    }

    if workspace_id
        .as_deref()
        .is_some_and(|requested_id| requested_id != active.id)
    {
        let mut result = DeleteResult::new(&active);
        result.error = Some("Requested workspace is not the active workspace.".to_string());
        result.exists_after = active.path.exists();
        result.remaining_entries = remaining_entries(&active.path);
        result.message = delete_result_message(&result);
        return Ok(result);
    }

    delete_workspace_path(&state, active, true)
}

#[tauri::command]
fn delete_planned_workspace(
    state: State<AppState>,
    pst_path: String,
    workspace_path: String,
) -> AppResult<DeleteResult> {
    let requested_path = PathBuf::from(&workspace_path);

    if state.import_running.load(Ordering::SeqCst) {
        let active = ActiveWorkspace {
            id: requested_path
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or_default()
                .to_string(),
            path: requested_path,
            pst_path: PathBuf::from(&pst_path),
            fingerprint: String::new(),
            location_mode: WorkspaceLocationMode::AppSupport,
        };
        let mut result = DeleteResult::new(&active);
        result.error = Some("Cancel import before deleting this workspace.".to_string());
        result.message = delete_result_message(&result);
        return Ok(result);
    }

    let identity = identify_pst(&PathBuf::from(&pst_path))?;
    let profile_paths = state.runtime_profile()?.paths();
    let Some(location_mode) =
        workspace_location_for_identity(profile_paths, &requested_path, &identity)?
    else {
        let active = active_workspace_from_path(
            requested_path,
            identity.canonical_path.clone(),
            identity.content_fingerprint.clone(),
            WorkspaceLocationMode::AppSupport,
        );
        let mut result = DeleteResult::new(&active);
        result.error = Some(format!(
            "Refusing to delete {} because it is not an expected PST QuickView workspace for this PST.",
            active.path.display()
        ));
        result.message = delete_result_message(&result);
        return Ok(result);
    };

    if !requested_path.exists() {
        let active = active_workspace_from_path(
            requested_path,
            identity.canonical_path,
            identity.content_fingerprint,
            location_mode,
        );
        let mut result = DeleteResult::new(&active);
        result.already_missing = true;
        result.message =
            "Workspace folder was already missing. Refreshed workspace list.".to_string();
        return Ok(result);
    }

    if !requested_path.is_dir() {
        let active = active_workspace_from_path(
            requested_path,
            identity.canonical_path,
            identity.content_fingerprint,
            location_mode,
        );
        let mut result = DeleteResult::new(&active);
        result.error = Some(format!(
            "Refusing to delete {} because it is not a directory.",
            active.path.display()
        ));
        result.message = delete_result_message(&result);
        return Ok(result);
    }

    let candidates = find_existing_workspaces(profile_paths, &identity)?;
    let candidate = select_existing_workspace(&candidates, &requested_path)?;
    let active = ActiveWorkspace {
        id: candidate.workspace_id,
        path: candidate.path,
        pst_path: identity.canonical_path,
        fingerprint: identity.content_fingerprint,
        location_mode: candidate.mode,
    };
    delete_workspace_path(&state, active, false)
}

fn delete_workspace_path(
    state: &State<AppState>,
    active: ActiveWorkspace,
    clear_active: bool,
) -> AppResult<DeleteResult> {
    let mut result = DeleteResult::new(&active);

    let validated = match validate_workspace_for_delete(state.runtime_profile()?.paths(), &active) {
        Ok(validated) => validated,
        Err(error) => {
            result.error = Some(error.message);
            result.exists_after = active.path.exists();
            result.remaining_entries = remaining_entries(&active.path);
            result.message = delete_result_message(&result);
            return Ok(result);
        }
    };

    if let Err(error) = remove_validated_workspace(&active.path, &validated) {
        result.error = Some(error.message);
    }

    result.exists_after = active.path.exists();
    result.deleted = result.existed_before && !result.exists_after;
    if result.exists_after {
        result.remaining_entries = remaining_entries(&active.path);
        if result.error.is_none() {
            result.error = Some(format!(
                "remove_dir_all returned success, but workspace still exists: {}",
                active.path.display()
            ));
        }
    }

    if result.deleted && !result.exists_after {
        state.cancel_import_requested.store(false, Ordering::SeqCst);
        if let Ok(mut pid) = state.readpst_pid.lock() {
            *pid = None;
        }

        match remove_empty_next_to_pst_parent(&active) {
            Ok(removed) => {
                result.removed_empty_parent = removed;
            }
            Err(error) => {
                result.error = Some(error.message);
            }
        }

        let replacement = {
            let mut open_guard = state
                .open_workspaces
                .lock()
                .map_err(|_| AppError::new("Could not lock open workspace state."))?;
            open_guard.remove(&active.id);
            open_guard.values().next().cloned()
        };

        if clear_active {
            let mut guard = state
                .active_workspace
                .lock()
                .map_err(|_| AppError::new("Could not lock active workspace state."))?;
            if guard
                .as_ref()
                .is_some_and(|current| current.id == active.id)
            {
                *guard = replacement;
            }
        }
    }

    result.message = delete_result_message(&result);
    append_application_log_best_effort(
        state,
        "workspace_delete",
        if result.deleted { "complete" } else { "failed" },
        Some(&active.id),
        result.error.as_deref(),
    );
    Ok(result)
}

fn open_pst_blocking(
    app: tauri::AppHandle,
    pst_path: PathBuf,
    requested_mode: WorkspaceLocationMode,
    existing_workspace_path: Option<PathBuf>,
    allow_duplicate: bool,
    action: ImportAction,
    operation_id: String,
) -> AppResult<WorkspaceSummary> {
    let profile_paths = app.state::<AppState>().runtime_profile()?.paths().clone();
    let identity = identify_pst(&pst_path)?;
    let selection = select_workspace(&profile_paths, &identity, requested_mode)?;
    let existing_workspaces = find_existing_workspaces(&profile_paths, &identity)?;

    if let Some(existing_workspace_path) = existing_workspace_path {
        let existing_workspace =
            select_existing_workspace(&existing_workspaces, &existing_workspace_path)?;
        match action {
            ImportAction::OpenExisting => {
                let conn = open_workspace_db_for_upgrade(&existing_workspace.path)?;
                upsert_current_fingerprint_metadata(
                    &conn,
                    &identity,
                    &existing_workspace.workspace_id,
                    &existing_workspace.path,
                    existing_workspace.mode,
                )?;
                emit_progress(
                    &app,
                    "Complete",
                    None,
                    None,
                    "Opened the selected existing local index.",
                );

                return workspace_summary(
                    &identity,
                    &existing_workspace.workspace_id,
                    &existing_workspace.path,
                    existing_workspace.mode,
                    true,
                );
            }
            ImportAction::ResumeIndex => {
                return resume_index_existing_workspace(
                    &app,
                    &identity,
                    &existing_workspace.workspace_id,
                    &existing_workspace.path,
                    existing_workspace.mode,
                    &operation_id,
                );
            }
            ImportAction::RepairIndex => {
                let active = ActiveWorkspace {
                    id: existing_workspace.workspace_id,
                    path: existing_workspace.path,
                    pst_path: identity.canonical_path.clone(),
                    fingerprint: identity.content_fingerprint.clone(),
                    location_mode: existing_workspace.mode,
                };
                return reindex_existing_emls_blocking(&app, active, Some(&operation_id));
            }
            ImportAction::Reimport => {
                return import_workspace_from_pst(
                    &app,
                    &find_readpst().ok_or_else(AppError::missing_readpst)?.path,
                    &identity,
                    existing_workspace.path,
                    existing_workspace.mode,
                    true,
                    &operation_id,
                );
            }
            ImportAction::Import => {}
        }
    }

    if !existing_workspaces.is_empty() && !allow_duplicate {
        return Err(AppError::new(
            "This PST appears to already be indexed. Choose an existing workspace, create a workspace in the selected location, or cancel.",
        ));
    }

    if existing_workspaces
        .iter()
        .any(|candidate| candidate.path == selection.path)
    {
        return Err(AppError::new(
            "The selected workspace location already has an index for this PST. Open the existing workspace instead.",
        ));
    }

    if let Some(warning) = selection.warning {
        emit_progress(&app, "Preparing workspace", None, None, &warning);
    }

    let workspace = selection.path;
    let workspace_mode = selection.mode;
    import_workspace_from_pst(
        &app,
        &find_readpst().ok_or_else(AppError::missing_readpst)?.path,
        &identity,
        workspace,
        workspace_mode,
        false,
        &operation_id,
    )
}

fn import_workspace_from_pst(
    app: &tauri::AppHandle,
    readpst: &Path,
    identity: &PstIdentity,
    workspace: PathBuf,
    workspace_mode: WorkspaceLocationMode,
    force_reimport: bool,
    operation_id: &str,
) -> AppResult<WorkspaceSummary> {
    let eml_dir = workspace.join("extracted");
    let index_path = workspace.join("index.sqlite");
    let readpst_path = readpst.display().to_string();
    let readpst_version_text = readpst_version(readpst);
    let progress_active = ActiveWorkspace {
        id: identity.workspace_id.clone(),
        path: workspace.clone(),
        pst_path: identity.canonical_path.clone(),
        fingerprint: identity.content_fingerprint.clone(),
        location_mode: workspace_mode,
    };

    if force_reimport {
        let allowed_workspace_ids = [
            identity.workspace_id.as_str(),
            identity.legacy_workspace_id.as_str(),
        ];
        match fs::symlink_metadata(&index_path) {
            Ok(_) => preflight_owned_current_index(
                &index_path,
                &allowed_workspace_ids,
                &identity.content_fingerprint,
            )?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if sqlite_sidecar_exists(&index_path) {
                    return Err(AppError::new(
                        "The existing workspace index file set is incomplete; reimport was refused.",
                    ));
                }
            }
            Err(_) => {
                return Err(AppError::new(
                    "The existing workspace index is unavailable.",
                ))
            }
        }
    }

    create_private_directory_all(&workspace.join("logs"))?;
    ensure_workspace_marker(&workspace, &identity.workspace_id, identity, workspace_mode)?;
    initialize_import_log(&workspace, &readpst_version_text)?;
    if force_reimport {
        recover_phase2_workspace_state(&progress_active)?;
        import_integrity_phase2::restart_import_state(&workspace, &identity.workspace_id)?;
    } else {
        import_integrity_phase2::transition_state(
            &workspace,
            &identity.workspace_id,
            import_integrity_phase2::WorkspaceState::Preflight,
            import_integrity_phase2::StateCounts::default(),
            Some("preflight_running"),
        )?;
    }
    mark_workspace_status_file_only(
        &workspace,
        identity,
        workspace_mode,
        ImportStatus::Pending,
        None,
        None,
        None,
    )?;
    let mut progress_guard = IntegrityOperationFinishGuard::begin(
        app,
        &progress_active,
        operation_id,
        import_progress::OperationKind::NewImport,
        import_progress::ProgressStage::Preflight,
    )?;
    let operation_result = (|| {
        set_workspace_readpst_metadata(&workspace, &readpst_path, &readpst_version_text)?;

        emit_progress(
            app,
            "Checking PST",
            None,
            None,
            "Checking PST and workspace requirements.",
        );

        warn_about_storage(app, identity, &workspace)?;

        let stale_extracting_dir = workspace.join("extracting");
        let temp_index = workspace.join("index.tmp.sqlite");
        remove_readpst_output_directory(&workspace, &stale_extracting_dir)?;
        remove_sqlite_file_set(&temp_index)?;
        if force_reimport {
            remove_readpst_output_directory(&workspace, &eml_dir)?;
        }
        if fs::symlink_metadata(&eml_dir).is_ok() {
            return Err(AppError::new(
                "Refusing to mix a new ReadPST run with existing extracted output.",
            ));
        }
        create_private_directory_all(&stale_extracting_dir)?;

        emit_progress(
            app,
            "Preparing workspace",
            None,
            None,
            "Preparing the local workspace.",
        );

        let mut promotion_state_may_have_changed = false;
        let import_result = (|| {
            mark_workspace_status_file_only(
                &workspace,
                identity,
                workspace_mode,
                ImportStatus::Running,
                None,
                Some(0),
                Some(0),
            )?;
            let extracting_state = import_integrity_phase2::transition_state(
                &workspace,
                &identity.workspace_id,
                import_integrity_phase2::WorkspaceState::Extracting,
                import_integrity_phase2::StateCounts::default(),
                Some("extraction_running"),
            )?;
            let source_after = run_readpst(
                app,
                readpst,
                &identity.canonical_path,
                &workspace,
                &stale_extracting_dir,
            )?;
            if fs::symlink_metadata(&eml_dir).is_ok() {
                let _ = remove_readpst_output_directory(&workspace, &stale_extracting_dir);
                return Err(AppError::new(
                    "Extracted output destination appeared during import; refusing to replace it.",
                ));
            }
            fs::rename(&stale_extracting_dir, &eml_dir)?;
            let extraction_complete_state = import_integrity_phase2::transition_state(
                &workspace,
                &identity.workspace_id,
                import_integrity_phase2::WorkspaceState::ExtractionComplete,
                import_integrity_phase2::StateCounts::default(),
                Some("extraction_complete"),
            )?;
            import_integrity_phase2::transition_state(
                &workspace,
                &identity.workspace_id,
                import_integrity_phase2::WorkspaceState::Discovering,
                import_integrity_phase2::StateCounts::default(),
                Some("discovery_running"),
            )?;
            emit_discovery_progress(app, 0, false)?;
            let discovery =
                discover_eml_candidates_with_progress(&eml_dir, |discovered, activity| {
                    emit_discovery_progress(app, discovered, activity)
                })?;
            import_integrity_phase2::write_manifest_for_discovery(
                &workspace,
                identity,
                &identity.workspace_id,
                &readpst_version_text,
                &extracting_state.state_started_at,
                &extraction_complete_state.updated_at,
                "success",
                &discovery,
                Some(&source_after),
            )?;
            let stats = index_eml_files(IndexEmlFiles {
                app,
                identity,
                workspace_id: &identity.workspace_id,
                extract_dir: &eml_dir,
                index_path: &temp_index,
                workspace: &workspace,
                workspace_mode,
                readpst_path: &readpst_path,
                readpst_version: &readpst_version_text,
                resume_existing: false,
            })?;
            verify_staged_replacement_index_with_operation_progress(
                app,
                &workspace,
                &identity.workspace_id,
                &temp_index,
                true,
            )?;

            emit_progress(
                app,
                "Preparing workspace",
                None,
                None,
                "Moving the completed local index into place.",
            );

            let promotion_discovery =
                discover_eml_candidates_with_progress(&eml_dir, |_, activity| {
                    checkpoint_integrity_operation(app, activity)
                })?;
            let allowed_workspace_ids = [
                identity.workspace_id.as_str(),
                identity.legacy_workspace_id.as_str(),
            ];
            if let Err(error) = promote_completed_index_with_operation_progress(
                app,
                &workspace,
                &temp_index,
                &allowed_workspace_ids,
                &identity.content_fingerprint,
                &promotion_discovery,
            ) {
                if error.code == Some(INDEX_PROMOTION_STATE_UNCERTAIN_CODE) {
                    promotion_state_may_have_changed = true;
                }
                return Err(error);
            }
            promotion_state_may_have_changed = true;

            let active = ActiveWorkspace {
                id: identity.workspace_id.clone(),
                path: workspace.clone(),
                pst_path: identity.canonical_path.clone(),
                fingerprint: identity.content_fingerprint.clone(),
                location_mode: workspace_mode,
            };
            let verification = import_integrity_phase2::verify_active_workspace_after_commit(
                app,
                &active,
                "initial_import_verification",
                import_integrity_phase2::VerificationLevel::Quick,
            )?;
            if verification.result == import_integrity_phase2::VerificationResult::Verified {
                mark_existing_workspace_complete_file_only(
                    &workspace,
                    identity,
                    workspace_mode,
                    &stats,
                )?;
            }

            Ok::<IndexStats, AppError>(stats)
        })();

        let _stats = match import_result {
            Ok(stats) => stats,
            Err(error) => {
                if !workspace.is_dir() {
                    return Err(error);
                }
                let cancellation_error = error.code == Some(IMPORT_CANCELLED_CODE);
                let promotion_committed = promotion_state_may_have_changed
                    && !import_integrity_phase2::has_staged_manifest(
                        &workspace,
                        &identity.workspace_id,
                    )
                    .unwrap_or(true)
                    && verify_replacement_index_with_operation_progress(
                        app,
                        &workspace,
                        &identity.workspace_id,
                        &index_path,
                        false,
                    )
                    .is_ok();
                let disposition = integrity_completion_failure_disposition(
                    promotion_committed,
                    promotion_state_may_have_changed && !promotion_committed,
                    cancellation_error,
                );
                let pre_promotion_failure = matches!(
                    disposition,
                    IntegrityCompletionFailureDisposition::CancelledBeforePromotion
                        | IntegrityCompletionFailureDisposition::FailedBeforePromotion
                );
                if pre_promotion_failure {
                    let status = if disposition
                        == IntegrityCompletionFailureDisposition::CancelledBeforePromotion
                    {
                        ImportStatus::Cancelled
                    } else {
                        ImportStatus::Failed
                    };
                    let _ = mark_workspace_status_file_only(
                        &workspace,
                        identity,
                        workspace_mode,
                        status,
                        Some(&error.message),
                        None,
                        None,
                    );
                    let _ = import_integrity_phase2::discard_staged_manifest(&workspace);
                    let cancelled = status == ImportStatus::Cancelled;
                    let _ = import_integrity_phase2::transition_state(
                        &workspace,
                        &identity.workspace_id,
                        if cancelled {
                            import_integrity_phase2::WorkspaceState::Cancelled
                        } else {
                            import_integrity_phase2::WorkspaceState::Failed
                        },
                        import_integrity_phase2::StateCounts::default(),
                        Some(if cancelled {
                            "import_cancelled"
                        } else {
                            "import_failed"
                        }),
                    );
                } else if disposition == IntegrityCompletionFailureDisposition::CompletionWins
                    && import_integrity_phase2::read_state(&workspace, &identity.workspace_id)
                        .ok()
                        .flatten()
                        .is_some_and(|state| {
                            state.state == import_integrity_phase2::WorkspaceState::Verifying
                        })
                {
                    let _ = import_integrity_phase2::transition_state(
                        &workspace,
                        &identity.workspace_id,
                        import_integrity_phase2::WorkspaceState::Incomplete,
                        import_integrity_phase2::StateCounts::default(),
                        Some("promotion_finalization_pending"),
                    );
                }
                let _ = append_import_log(
                    &workspace,
                    match disposition {
                        IntegrityCompletionFailureDisposition::CancelledBeforePromotion => {
                            "state=cancelled stage=pre_promotion"
                        }
                        IntegrityCompletionFailureDisposition::FailedBeforePromotion => {
                            "state=failed stage=pre_promotion"
                        }
                        IntegrityCompletionFailureDisposition::CompletionWins => {
                            "state=incomplete reason=promotion_finalization_pending"
                        }
                        IntegrityCompletionFailureDisposition::PreserveUncertain => {
                            "state=uncertain recovery_data=preserved"
                        }
                    },
                );
                return Err(error);
            }
        };

        let summary = workspace_summary(
            identity,
            &identity.workspace_id,
            &workspace,
            workspace_mode,
            false,
        )?;
        Ok(summary)
    })();
    progress_guard.finish_with_result(operation_result)
}

fn resume_index_existing_workspace(
    app: &tauri::AppHandle,
    identity: &PstIdentity,
    workspace_id: &str,
    workspace: &Path,
    workspace_mode: WorkspaceLocationMode,
    operation_id: &str,
) -> AppResult<WorkspaceSummary> {
    let active = ActiveWorkspace {
        id: workspace_id.to_string(),
        path: workspace.to_path_buf(),
        pst_path: identity.canonical_path.clone(),
        fingerprint: identity.content_fingerprint.clone(),
        location_mode: workspace_mode,
    };
    preflight_integrity_compatibility_status(workspace, false, false)?;
    database_import_status_strict_at_path(&workspace.join("index.tmp.sqlite"), false, true)?;
    refuse_foreign_index_recovery_layout(workspace, "index.tmp.sqlite")?;
    recover_phase2_workspace_state(&active)?;
    import_integrity_phase2::read_state(workspace, workspace_id)?
        .ok_or_else(|| AppError::new("Workspace state is unavailable for resume progress."))?;
    let mut progress_guard = IntegrityOperationFinishGuard::begin(
        app,
        &active,
        operation_id,
        import_progress::OperationKind::ResumedImport,
        import_progress::ProgressStage::Discovering,
    )?;
    let operation_result = (|| {
        let extract_dir = workspace.join("extracted");
        let extract_metadata = fs::symlink_metadata(&extract_dir).map_err(|_| {
            AppError::new("Cannot resume because extracted source records are unavailable.")
        })?;
        if extract_metadata.file_type().is_symlink() || !extract_metadata.is_dir() {
            return Err(AppError::new(
                "Cannot resume indexing because extracted source records are unavailable.",
            ));
        }

        let preflight_discovery =
            discover_eml_candidates_with_progress(&extract_dir, |discovered, activity| {
                emit_discovery_progress(app, discovered, activity)
            })?;
        let resume_readpst_path =
            workspace_metadata_string_strict(workspace, "readpst_path", "readpstPath")?
                .unwrap_or_else(|| "readpst provenance unavailable".to_string());
        let resume_readpst_version =
            workspace_metadata_string_strict(workspace, "readpst_version", "readpstVersion")?
                .unwrap_or_else(|| "readpst provenance unavailable".to_string());
        import_integrity_phase2::ensure_manifest_for_existing_extraction(
            workspace,
            identity,
            workspace_id,
            &resume_readpst_version,
            &preflight_discovery,
        )?;
        if let Some(summary) =
            recover_interrupted_resume_layout(app, &active, "resume_index_recovery")?
        {
            return Ok(summary);
        }

        let temp_index = workspace.join("index.tmp.sqlite");
        let index_path = workspace.join("index.sqlite");
        let (resume_index, replace_final_index) = if fs::symlink_metadata(&temp_index).is_ok() {
            (temp_index.clone(), true)
        } else if sqlite_sidecar_exists(&temp_index) {
            return Err(AppError::new(
                "Cannot resume indexing because the partial index file set is incomplete.",
            ));
        } else if fs::symlink_metadata(&index_path).is_ok() {
            (index_path.clone(), false)
        } else {
            (temp_index.clone(), true)
        };
        let preserve_main_metadata =
            replace_final_index && fs::symlink_metadata(&index_path).is_ok();
        if replace_final_index
            && fs::symlink_metadata(&index_path)
                .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
            && sqlite_sidecar_exists(&index_path)
        {
            return Err(AppError::new(
                "Cannot resume indexing because the final index file set is incomplete.",
            ));
        }

        import_integrity_phase2::transition_state(
            workspace,
            workspace_id,
            import_integrity_phase2::WorkspaceState::Discovering,
            import_integrity_phase2::StateCounts::default(),
            Some("resume_discovering"),
        )?;
        let allowed_workspace_ids = [
            workspace_id,
            identity.workspace_id.as_str(),
            identity.legacy_workspace_id.as_str(),
        ];
        match fs::symlink_metadata(&resume_index) {
            Ok(_) => {
                preflight_resume_index(
                    &resume_index,
                    identity,
                    workspace_id,
                    &extract_dir,
                    &preflight_discovery,
                )?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(AppError::new("Workspace index is unavailable.")),
        }
        if preserve_main_metadata {
            preflight_owned_current_index(
                &index_path,
                &allowed_workspace_ids,
                &identity.content_fingerprint,
            )?;
        }

        initialize_import_log(workspace, &resume_readpst_version)?;
        let (starting_message_count, starting_error_count) =
            read_index_progress_counts(&resume_index);
        if !preserve_main_metadata {
            if replace_final_index {
                mark_workspace_status_file_only(
                    workspace,
                    identity,
                    workspace_mode,
                    ImportStatus::Running,
                    None,
                    starting_message_count,
                    starting_error_count,
                )?;
            } else {
                mark_workspace_status(
                    workspace,
                    identity,
                    workspace_mode,
                    ImportStatus::Running,
                    None,
                    starting_message_count,
                    starting_error_count,
                )?;
            }
        }

        let result = index_eml_files(IndexEmlFiles {
            app,
            identity,
            workspace_id,
            extract_dir: &extract_dir,
            index_path: &resume_index,
            workspace,
            workspace_mode,
            readpst_path: &resume_readpst_path,
            readpst_version: &resume_readpst_version,
            resume_existing: true,
        });

        let stats = match result {
            Ok(stats) => stats,
            Err(error) => {
                if !workspace.is_dir() {
                    return Err(error);
                }
                let cancelled = error.code == Some(IMPORT_CANCELLED_CODE);
                let status = if cancelled {
                    ImportStatus::Cancelled
                } else {
                    ImportStatus::Failed
                };
                if !preserve_main_metadata {
                    let (message_count, error_count) = read_index_progress_counts(&resume_index);
                    if replace_final_index {
                        let _ = mark_workspace_status_file_only(
                            workspace,
                            identity,
                            workspace_mode,
                            status,
                            Some(&error.message),
                            message_count,
                            error_count,
                        );
                    } else {
                        let _ = mark_workspace_status(
                            workspace,
                            identity,
                            workspace_mode,
                            status,
                            Some(&error.message),
                            message_count,
                            error_count,
                        );
                    }
                }
                let _ = append_import_log(
                    workspace,
                    &format!("state={} stage=indexing", status.as_str()),
                );
                let _ = import_integrity_phase2::transition_state(
                    workspace,
                    workspace_id,
                    if cancelled {
                        import_integrity_phase2::WorkspaceState::Cancelled
                    } else {
                        import_integrity_phase2::WorkspaceState::Failed
                    },
                    import_integrity_phase2::StateCounts::default(),
                    Some(if cancelled {
                        "resume_cancelled"
                    } else {
                        "resume_failed"
                    }),
                );
                return Err(error);
            }
        };

        let mut promotion_committed = !replace_final_index;
        let mut promotion_state_uncertain = false;
        let completion = (|| -> AppResult<import_integrity_phase2::VerificationReport> {
            let cancelled = || {
                app.state::<AppState>()
                    .cancel_import_requested
                    .load(Ordering::SeqCst)
            };
            if replace_final_index && cancelled() {
                return Err(import_cancelled_error(
                    "Resume indexing was cancelled before verification.",
                ));
            }
            if replace_final_index {
                verify_staged_replacement_index_with_operation_progress(
                    app,
                    workspace,
                    workspace_id,
                    &resume_index,
                    true,
                )?;
                let promotion_discovery =
                    discover_eml_candidates_with_progress(&extract_dir, |_, activity| {
                        checkpoint_integrity_operation(app, activity)
                    })?;
                if cancelled() {
                    return Err(import_cancelled_error(
                        "Resume indexing was cancelled before promotion.",
                    ));
                }
                if let Err(error) = promote_completed_index_with_operation_progress(
                    app,
                    workspace,
                    &resume_index,
                    &allowed_workspace_ids,
                    &identity.content_fingerprint,
                    &promotion_discovery,
                ) {
                    let active_replacement_is_verified =
                        !import_integrity_phase2::has_staged_manifest(workspace, workspace_id)
                            .unwrap_or(true)
                            && verify_replacement_index_with_operation_progress(
                                app,
                                workspace,
                                workspace_id,
                                &workspace.join("index.sqlite"),
                                false,
                            )
                            .is_ok();
                    if !active_replacement_is_verified {
                        if error.code == Some(INDEX_PROMOTION_STATE_UNCERTAIN_CODE) {
                            promotion_state_uncertain = true;
                        }
                        return Err(error);
                    }
                }
                promotion_committed = true;
            } else {
                verify_replacement_index_with_operation_progress(
                    app,
                    workspace,
                    workspace_id,
                    &resume_index,
                    false,
                )?;
            }
            import_integrity_phase2::verify_active_workspace_after_commit(
                app,
                &active,
                "resume_indexing_verification",
                import_integrity_phase2::VerificationLevel::Quick,
            )
        })();
        let verification = match completion {
            Ok(report) => report,
            Err(error) => {
                let disposition = integrity_completion_failure_disposition(
                    promotion_committed,
                    promotion_state_uncertain,
                    error.code == Some(IMPORT_CANCELLED_CODE),
                );
                let pre_promotion_failure = matches!(
                    disposition,
                    IntegrityCompletionFailureDisposition::CancelledBeforePromotion
                        | IntegrityCompletionFailureDisposition::FailedBeforePromotion
                );
                if pre_promotion_failure {
                    let _ = import_integrity_phase2::discard_staged_manifest(workspace);
                }
                if pre_promotion_failure {
                    let cancelled = disposition
                        == IntegrityCompletionFailureDisposition::CancelledBeforePromotion;
                    let _ = import_integrity_phase2::transition_state(
                        workspace,
                        workspace_id,
                        if cancelled {
                            import_integrity_phase2::WorkspaceState::Cancelled
                        } else {
                            import_integrity_phase2::WorkspaceState::Failed
                        },
                        import_integrity_phase2::StateCounts {
                            discovered_candidates: Some(stats.discovered),
                            canonical_indexed: Some(stats.indexed),
                        },
                        Some(if cancelled {
                            "resume_cancelled"
                        } else {
                            "resume_failed"
                        }),
                    );
                } else if disposition == IntegrityCompletionFailureDisposition::CompletionWins
                    && import_integrity_phase2::read_state(workspace, workspace_id)
                        .ok()
                        .flatten()
                        .is_some_and(|state| {
                            state.state == import_integrity_phase2::WorkspaceState::Verifying
                        })
                {
                    let _ = import_integrity_phase2::transition_state(
                        workspace,
                        workspace_id,
                        import_integrity_phase2::WorkspaceState::Incomplete,
                        import_integrity_phase2::StateCounts {
                            discovered_candidates: Some(stats.discovered),
                            canonical_indexed: Some(stats.indexed),
                        },
                        Some("promotion_finalization_pending"),
                    );
                }
                let _ = append_import_log(
                    workspace,
                    match disposition {
                        IntegrityCompletionFailureDisposition::CancelledBeforePromotion => {
                            "state=cancelled stage=pre_promotion"
                        }
                        IntegrityCompletionFailureDisposition::FailedBeforePromotion => {
                            "state=failed stage=pre_promotion"
                        }
                        IntegrityCompletionFailureDisposition::CompletionWins => {
                            "state=incomplete reason=promotion_finalization_pending"
                        }
                        IntegrityCompletionFailureDisposition::PreserveUncertain => {
                            "state=uncertain recovery_data=preserved"
                        }
                    },
                );
                return Err(error);
            }
        };
        if verification.result == import_integrity_phase2::VerificationResult::Verified {
            mark_existing_workspace_complete_file_only(
                workspace,
                identity,
                workspace_mode,
                &stats,
            )?;
        }
        if replace_final_index && promotion_committed && resume_index.is_file() {
            retire_repair_recovery_index(workspace, &resume_index)?;
        }

        let summary = workspace_summary(identity, workspace_id, workspace, workspace_mode, true)?;
        Ok(summary)
    })();
    progress_guard.finish_with_result(operation_result)
}

fn resume_active_workspace_indexing_blocking(
    app: &tauri::AppHandle,
    active: ActiveWorkspace,
    operation_id: &str,
) -> AppResult<WorkspaceSummary> {
    import_integrity_phase2::validate_operation_id(operation_id)?;
    preflight_integrity_compatibility_status(&active.path, false, false)?;
    database_import_status_strict_at_path(&active.path.join("index.tmp.sqlite"), false, true)?;
    let identity = import_integrity_phase2::identity_from_active_workspace(&active)?;
    refuse_foreign_index_recovery_layout(&active.path, "index.tmp.sqlite")?;
    recover_phase2_workspace_state(&active)?;
    resume_index_existing_workspace(
        app,
        &identity,
        &active.id,
        &active.path,
        active.location_mode,
        operation_id,
    )
}

fn verification_has_valid_local_index(
    report: &import_integrity_phase2::VerificationReport,
) -> bool {
    matches!(
        report.result,
        import_integrity_phase2::VerificationResult::Verified
            | import_integrity_phase2::VerificationResult::SourceUnavailable
            | import_integrity_phase2::VerificationResult::SourceChanged
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IntegrityCompletionFailureDisposition {
    CompletionWins,
    PreserveUncertain,
    CancelledBeforePromotion,
    FailedBeforePromotion,
}

fn integrity_completion_failure_disposition(
    promotion_committed: bool,
    promotion_state_uncertain: bool,
    cancel_requested: bool,
) -> IntegrityCompletionFailureDisposition {
    if promotion_committed {
        IntegrityCompletionFailureDisposition::CompletionWins
    } else if promotion_state_uncertain {
        IntegrityCompletionFailureDisposition::PreserveUncertain
    } else if cancel_requested {
        IntegrityCompletionFailureDisposition::CancelledBeforePromotion
    } else {
        IntegrityCompletionFailureDisposition::FailedBeforePromotion
    }
}

#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn integrity_completion_failure_disposition_for_test(
    promotion_committed: bool,
    promotion_state_uncertain: bool,
    cancel_requested: bool,
) -> &'static str {
    match integrity_completion_failure_disposition(
        promotion_committed,
        promotion_state_uncertain,
        cancel_requested,
    ) {
        IntegrityCompletionFailureDisposition::CompletionWins => "completion_wins",
        IntegrityCompletionFailureDisposition::PreserveUncertain => "preserve_uncertain",
        IntegrityCompletionFailureDisposition::CancelledBeforePromotion => {
            "cancelled_before_promotion"
        }
        IntegrityCompletionFailureDisposition::FailedBeforePromotion => "failed_before_promotion",
    }
}

fn retire_repair_recovery_index(workspace: &Path, path: &Path) -> AppResult<()> {
    if path.parent() != Some(workspace)
        || !matches!(
            path.file_name().and_then(OsStr::to_str),
            Some("index.reindex.tmp.sqlite" | "index.tmp.sqlite")
        )
    {
        return Err(AppError::new(
            "Refusing to retire unexpected index-replacement recovery data.",
        ));
    }
    let metadata = require_regular_nonsymlink_file(path, "index-replacement recovery database")?;
    if metadata.nlink() != 1 || sqlite_sidecar_exists(path) {
        return Err(AppError::new(
            "Index-replacement recovery data is linked or has sidecars.",
        ));
    }
    fs::remove_file(path)?;
    File::open(workspace)?.sync_all()?;
    Ok(())
}

fn refuse_foreign_index_recovery_layout(workspace: &Path, operation_temp: &str) -> AppResult<()> {
    let foreign_temp = match operation_temp {
        "index.tmp.sqlite" => "index.reindex.tmp.sqlite",
        "index.reindex.tmp.sqlite" => "index.tmp.sqlite",
        _ => {
            return Err(AppError::new(
                "Index recovery operation identity is invalid.",
            ))
        }
    };
    let foreign_path = workspace.join(foreign_temp);
    match fs::symlink_metadata(&foreign_path) {
        Ok(_) => return Err(AppError::new(
            "Recovery data belongs to a different index operation; no workspace data was changed.",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => {
            return Err(AppError::new(
                "Index recovery data could not be classified; no workspace data was changed.",
            ))
        }
    }
    if sqlite_sidecar_exists(&foreign_path) {
        return Err(AppError::new(
            "Recovery sidecars belong to a different index operation; no workspace data was changed.",
        ));
    }
    Ok(())
}

fn recovery_index_import_status(path: &Path) -> Option<String> {
    let conn = open_sqlite_read_only_connection(path).ok()?;
    configure_sqlite_connection(&conn, true).ok()?;
    metadata_value(&conn, "import_status").ok().flatten()
}

fn checkpoint_owned_temp_recovery_index_with_progress(
    app: &tauri::AppHandle,
    active: &ActiveWorkspace,
    temp_index: &Path,
) -> AppResult<()> {
    let identity = import_integrity_phase2::identity_from_active_workspace(active)?;
    let extract_dir = active.path.join("extracted");
    let discovery = discover_eml_candidates_with_progress(&extract_dir, |_, activity| {
        checkpoint_integrity_operation(app, activity)
    })?;
    preflight_resume_index(temp_index, &identity, &active.id, &extract_dir, &discovery)?;
    let conn = open_sqlite_write_connection(temp_index)?;
    checked_truncate_checkpoint(&conn)?;
    drop(conn);
    preflight_resume_index(temp_index, &identity, &active.id, &extract_dir, &discovery)?;
    remove_checkpointed_sqlite_sidecars(temp_index)?;
    Ok(())
}

#[cfg(test)]
fn checkpoint_owned_temp_recovery_index(
    active: &ActiveWorkspace,
    temp_index: &Path,
) -> AppResult<()> {
    let identity = import_integrity_phase2::identity_from_active_workspace(active)?;
    let extract_dir = active.path.join("extracted");
    let discovery = discover_eml_candidates(&extract_dir)?;
    preflight_resume_index(temp_index, &identity, &active.id, &extract_dir, &discovery)?;
    let conn = open_sqlite_write_connection(temp_index)?;
    checked_truncate_checkpoint(&conn)?;
    drop(conn);
    preflight_resume_index(temp_index, &identity, &active.id, &extract_dir, &discovery)?;
    remove_checkpointed_sqlite_sidecars(temp_index)?;
    Ok(())
}

fn recover_interrupted_repair_layout(
    app: &tauri::AppHandle,
    active: &ActiveWorkspace,
    operation_id: &str,
) -> AppResult<Option<WorkspaceSummary>> {
    refuse_foreign_index_recovery_layout(&active.path, "index.reindex.tmp.sqlite")?;
    let index_path = active.path.join("index.sqlite");
    let temp_index = active.path.join("index.reindex.tmp.sqlite");
    let temp_exists = fs::symlink_metadata(&temp_index).is_ok();
    if !temp_exists && sqlite_sidecar_exists(&temp_index) {
        return Err(AppError::new(
            "Index-replacement recovery sidecars exist without their database.",
        ));
    }
    let staged_exists = import_integrity_phase2::has_staged_manifest(&active.path, &active.id)?;
    if !temp_exists && !staged_exists {
        return Ok(None);
    }
    if temp_exists {
        require_regular_nonsymlink_file(&temp_index, "index-replacement recovery database")?;
        if sqlite_sidecar_exists(&temp_index) {
            checkpoint_owned_temp_recovery_index_with_progress(app, active, &temp_index)?;
        }
    }

    let active_exists = index_path.is_file();
    let active_matches_current = active_exists
        && replacement_index_matches_with_operation_progress(
            app,
            &active.path,
            &active.id,
            &index_path,
            false,
        )?;
    let active_matches_staged = staged_exists
        && active_exists
        && replacement_index_matches_with_operation_progress(
            app,
            &active.path,
            &active.id,
            &index_path,
            true,
        )?;
    let temp_matches_staged = staged_exists
        && temp_exists
        && replacement_index_matches_with_operation_progress(
            app,
            &active.path,
            &active.id,
            &temp_index,
            true,
        )?;

    if active_matches_staged {
        prepare_recovery_verification_progress(app, operation_id)?;
        import_integrity_phase2::publish_staged_manifest(&active.path, &active.id)?;
        let report = import_integrity_phase2::verify_active_workspace_after_commit(
            app,
            active,
            operation_id,
            import_integrity_phase2::VerificationLevel::Quick,
        )?;
        if !verification_has_valid_local_index(&report) {
            return Err(AppError::new(
                "Recovered replacement index did not pass final internal verification.",
            ));
        }
        if temp_exists {
            retire_repair_recovery_index(&active.path, &temp_index)?;
        }
        return workspace_summary_from_active(active, true).map(Some);
    }

    if staged_exists && temp_matches_staged {
        // The checked swap had not committed. The current active index (or the
        // intentional absence of one) is preserved. A temp database matching
        // the staged manifest proves it is still pre-swap scratch, even when
        // the active database itself is corrupt or incomplete.
        retire_repair_recovery_index(&active.path, &temp_index)?;
        import_integrity_phase2::discard_staged_manifest(&active.path)?;
        return Ok(None);
    }

    if !staged_exists && active_matches_current {
        prepare_recovery_verification_progress(app, operation_id)?;
        let report = import_integrity_phase2::verify_active_workspace(
            app,
            active,
            operation_id,
            import_integrity_phase2::VerificationLevel::Quick,
        )?;
        if !verification_has_valid_local_index(&report) {
            return Err(AppError::new(
                "Recovered active index did not pass final internal verification.",
            ));
        }
        if temp_exists {
            retire_repair_recovery_index(&active.path, &temp_index)?;
        }
        return workspace_summary_from_active(active, true).map(Some);
    }

    let interrupted_repair_scratch = !staged_exists
        && temp_exists
        && recovery_index_import_status(&temp_index)
            .as_deref()
            .is_some_and(|status| matches!(status, "running" | "verifying"))
        && import_integrity_phase2::read_state(&active.path, &active.id)
            .ok()
            .flatten()
            .is_some_and(|state| {
                state.interrupted_stage.is_some_and(|stage| {
                    matches!(
                        stage,
                        import_integrity_phase2::WorkspaceState::Indexing
                            | import_integrity_phase2::WorkspaceState::BuildingConversations
                            | import_integrity_phase2::WorkspaceState::Verifying
                    )
                })
            });
    if interrupted_repair_scratch {
        let identity = import_integrity_phase2::identity_from_active_workspace(active)?;
        let discovery = discover_eml_candidates_with_progress(
            &active.path.join("extracted"),
            |_, activity| checkpoint_integrity_operation(app, activity),
        )?;
        preflight_resume_index(
            &temp_index,
            &identity,
            &active.id,
            &active.path.join("extracted"),
            &discovery,
        )?;
        retire_repair_recovery_index(&active.path, &temp_index)?;
        return Ok(None);
    }

    Err(AppError::new(
        "Index-replacement recovery data is ambiguous; no database or manifest was changed.",
    ))
}

fn recover_interrupted_resume_layout(
    app: &tauri::AppHandle,
    active: &ActiveWorkspace,
    operation_id: &str,
) -> AppResult<Option<WorkspaceSummary>> {
    refuse_foreign_index_recovery_layout(&active.path, "index.tmp.sqlite")?;
    let index_path = active.path.join("index.sqlite");
    let temp_index = active.path.join("index.tmp.sqlite");
    let temp_exists = fs::symlink_metadata(&temp_index).is_ok();
    if !temp_exists && sqlite_sidecar_exists(&temp_index) {
        return Err(AppError::new(
            "Resume recovery sidecars exist without their database.",
        ));
    }
    let staged_exists = import_integrity_phase2::has_staged_manifest(&active.path, &active.id)?;
    if !temp_exists && !staged_exists {
        return Ok(None);
    }
    if temp_exists {
        require_regular_nonsymlink_file(&temp_index, "resume recovery database")?;
        if sqlite_sidecar_exists(&temp_index) {
            checkpoint_owned_temp_recovery_index_with_progress(app, active, &temp_index)?;
        }
    }

    let active_exists = index_path.is_file();
    let active_matches_current = active_exists
        && replacement_index_matches_with_operation_progress(
            app,
            &active.path,
            &active.id,
            &index_path,
            false,
        )?;
    let active_matches_staged = staged_exists
        && active_exists
        && replacement_index_matches_with_operation_progress(
            app,
            &active.path,
            &active.id,
            &index_path,
            true,
        )?;
    let temp_matches_staged = staged_exists
        && temp_exists
        && replacement_index_matches_with_operation_progress(
            app,
            &active.path,
            &active.id,
            &temp_index,
            true,
        )?;

    if active_matches_staged {
        prepare_recovery_verification_progress(app, operation_id)?;
        import_integrity_phase2::publish_staged_manifest(&active.path, &active.id)?;
        let report = import_integrity_phase2::verify_active_workspace_after_commit(
            app,
            active,
            operation_id,
            import_integrity_phase2::VerificationLevel::Quick,
        )?;
        if !verification_has_valid_local_index(&report) {
            return Err(AppError::new(
                "Recovered resumed index did not pass final internal verification.",
            ));
        }
        if temp_exists {
            retire_repair_recovery_index(&active.path, &temp_index)?;
        }
        return workspace_summary_from_active(active, true).map(Some);
    }

    if staged_exists && temp_matches_staged {
        retire_repair_recovery_index(&active.path, &temp_index)?;
        import_integrity_phase2::discard_staged_manifest(&active.path)?;
        return Ok(None);
    }

    if !staged_exists && temp_exists {
        match recovery_index_import_status(&temp_index).as_deref() {
            Some("running" | "verifying") => return Ok(None),
            Some("complete") if active_matches_current => {
                prepare_recovery_verification_progress(app, operation_id)?;
                let report = import_integrity_phase2::verify_active_workspace_after_commit(
                    app,
                    active,
                    operation_id,
                    import_integrity_phase2::VerificationLevel::Quick,
                )?;
                if !verification_has_valid_local_index(&report) {
                    return Err(AppError::new(
                        "Recovered active index did not pass final internal verification.",
                    ));
                }
                retire_repair_recovery_index(&active.path, &temp_index)?;
                return workspace_summary_from_active(active, true).map(Some);
            }
            _ => {}
        }
    }

    Err(AppError::new(
        "Resume recovery data is ambiguous; no database or manifest was changed.",
    ))
}

fn reindex_existing_emls_blocking(
    app: &tauri::AppHandle,
    active: ActiveWorkspace,
    operation_id: Option<&str>,
) -> AppResult<WorkspaceSummary> {
    if !fs::symlink_metadata(&active.path)
        .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_dir())
    {
        return Err(AppError::new("Workspace is unavailable."));
    }
    let marker = active.path.join(WORKSPACE_MARKER_FILE);
    if !marker.exists() {
        return Err(AppError::new(
            "Refusing to repair a workspace with an invalid marker.",
        ));
    }

    let extract_dir = active.path.join("extracted");
    if !fs::symlink_metadata(&extract_dir)
        .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_dir())
    {
        return Err(AppError::new(
            "Cannot repair because extracted source records are unavailable.",
        ));
    }

    let index_path = active.path.join("index.sqlite");
    preflight_integrity_compatibility_status(&active.path, false, true)?;
    database_import_status_strict_at_path(
        &active.path.join("index.reindex.tmp.sqlite"),
        false,
        true,
    )?;
    refuse_foreign_index_recovery_layout(&active.path, "index.reindex.tmp.sqlite")?;

    let verification_operation_id = operation_id.unwrap_or("legacy_reindex");
    let identity = import_integrity_phase2::identity_from_active_workspace(&active)?;
    import_integrity_phase2::ensure_state_for_workspace(&active.path, &active.id)?;
    recover_phase2_workspace_state(&active)?;
    let mut progress_guard = IntegrityOperationFinishGuard::begin(
        app,
        &active,
        verification_operation_id,
        import_progress::OperationKind::Repair,
        import_progress::ProgressStage::Preflight,
    )?;
    let operation_result = (|| {
        // Recovery must inspect the persisted interrupted-stage evidence before
        // a new repair transition clears it.
        if let Some(summary) =
            recover_interrupted_repair_layout(app, &active, verification_operation_id)?
        {
            return Ok(summary);
        }
        // Reconcile the reused extraction while still in Preflight. Extracting
        // is marked reused only after the bounded discovery and manifest
        // identity checks both succeed.
        let discovery = discover_eml_candidates_with_progress(&extract_dir, |_, activity| {
            checkpoint_integrity_operation(app, activity)
        })?;
        let readpst_version =
            workspace_metadata_string_strict(&active.path, "readpst_version", "readpstVersion")?
                .unwrap_or_else(|| "unavailable".to_string());
        import_integrity_phase2::ensure_manifest_for_existing_extraction(
            &active.path,
            &identity,
            &active.id,
            &readpst_version,
            &discovery,
        )?;
        append_import_log(
            &active.path,
            "state=repair_requested extraction_reused=true",
        )?;
        let (existing_metadata, stable_ids) = match open_workspace_db_for_read(&active.path) {
            Ok(conn) => (
                read_import_metadata(&conn)?,
                read_stable_message_ids(&index_path)?,
            ),
            Err(_) => (HashMap::new(), HashMap::new()),
        };
        import_integrity_phase2::transition_state(
            &active.path,
            &active.id,
            import_integrity_phase2::WorkspaceState::Discovering,
            import_integrity_phase2::StateCounts {
                discovered_candidates: Some(discovery.count),
                canonical_indexed: None,
            },
            Some("repair_discovering"),
        )?;
        emit_discovery_progress(app, discovery.count, true)?;

        let temp_index = active.path.join("index.reindex.tmp.sqlite");
        if temp_index.exists()
            || sqlite_sidecar_exists(&temp_index)
            || import_integrity_phase2::has_staged_manifest(&active.path, &active.id)?
        {
            return Err(AppError::new(
                "Index-replacement recovery did not reach a safe retry state.",
            ));
        }

        let result = reindex_eml_files_to_index(
            app,
            &active,
            &extract_dir,
            &temp_index,
            existing_metadata,
            &stable_ids,
        );

        let stats = match result {
            Ok(stats) => stats,
            Err(error) => {
                let _ = remove_sqlite_file_set(&temp_index);
                let cancelled = error.code == Some(IMPORT_CANCELLED_CODE);
                let status = if cancelled {
                    ImportStatus::Cancelled.as_str()
                } else {
                    ImportStatus::Failed.as_str()
                };
                let _ = append_import_log(&active.path, &format!("state={status} stage=indexing"));
                let _ = import_integrity_phase2::transition_state(
                    &active.path,
                    &active.id,
                    if cancelled {
                        import_integrity_phase2::WorkspaceState::Cancelled
                    } else {
                        import_integrity_phase2::WorkspaceState::Failed
                    },
                    import_integrity_phase2::StateCounts::default(),
                    Some(if cancelled {
                        "repair_cancelled"
                    } else {
                        "repair_failed"
                    }),
                );
                return Err(error);
            }
        };

        let mut promotion_committed = false;
        let mut promotion_state_uncertain = false;
        let completion = (|| -> AppResult<import_integrity_phase2::VerificationReport> {
            let cancelled = || {
                app.state::<AppState>()
                    .cancel_import_requested
                    .load(Ordering::SeqCst)
            };
            if cancelled() {
                return Err(import_cancelled_error(
                    "Index repair was cancelled before verification.",
                ));
            }
            validate_replacement_stable_ids(&temp_index, &stable_ids)?;
            import_integrity_phase2::stage_manifest_from_index(
                &active.path,
                &active.id,
                &temp_index,
                &stats,
                "complete",
            )?;
            if cancelled() {
                return Err(import_cancelled_error(
                    "Index repair was cancelled before verification.",
                ));
            }
            import_integrity_phase2::transition_state(
                &active.path,
                &active.id,
                import_integrity_phase2::WorkspaceState::Verifying,
                import_integrity_phase2::StateCounts {
                    discovered_candidates: Some(stats.discovered),
                    canonical_indexed: Some(stats.indexed),
                },
                Some("repair_verifying"),
            )?;
            verify_staged_replacement_index_with_operation_progress(
                app,
                &active.path,
                &active.id,
                &temp_index,
                true,
            )?;

            let promotion_discovery =
                discover_eml_candidates_with_progress(&extract_dir, |_, activity| {
                    checkpoint_integrity_operation(app, activity)
                })?;
            // This is the final cancellable boundary. Once the checked atomic swap
            // begins, a late cancellation cannot truthfully claim that the prior
            // active index remains installed.
            if cancelled() {
                return Err(import_cancelled_error(
                    "Index repair was cancelled before promotion.",
                ));
            }
            let promotion = promote_repaired_index_with_operation_progress(
                app,
                &active.path,
                &temp_index,
                &[active.id.as_str()],
                &active.fingerprint,
                &promotion_discovery,
            );
            if let Err(error) = promotion {
                let active_replacement_is_verified =
                    !import_integrity_phase2::has_staged_manifest(&active.path, &active.id)
                        .unwrap_or(true)
                        && verify_replacement_index_with_operation_progress(
                            app,
                            &active.path,
                            &active.id,
                            &active.path.join("index.sqlite"),
                            false,
                        )
                        .is_ok();
                if !active_replacement_is_verified {
                    if error.code == Some(INDEX_PROMOTION_STATE_UNCERTAIN_CODE) {
                        promotion_state_uncertain = true;
                    }
                    return Err(error);
                }
            }
            promotion_committed = true;
            let report = import_integrity_phase2::verify_active_workspace_after_commit(
                app,
                &active,
                verification_operation_id,
                import_integrity_phase2::VerificationLevel::Quick,
            )?;
            if !matches!(
                report.result,
                import_integrity_phase2::VerificationResult::Verified
                    | import_integrity_phase2::VerificationResult::SourceUnavailable
                    | import_integrity_phase2::VerificationResult::SourceChanged
            ) {
                return Err(AppError::new(
                "The replacement index is active but did not pass final internal verification; extracted records and the source PST were not modified.",
            ));
            }
            if promotion_committed && temp_index.is_file() {
                retire_repair_recovery_index(&active.path, &temp_index)?;
            }
            if report.workspace_state == import_integrity_phase2::WorkspaceState::Complete {
                update_workspace_metadata_after_reindex(&active, &stats)?;
            }
            Ok(report)
        })();
        let report = match completion {
            Ok(report) => report,
            Err(error) => {
                let disposition = integrity_completion_failure_disposition(
                    promotion_committed,
                    promotion_state_uncertain,
                    error.code == Some(IMPORT_CANCELLED_CODE),
                );
                let pre_promotion_failure = matches!(
                    disposition,
                    IntegrityCompletionFailureDisposition::CancelledBeforePromotion
                        | IntegrityCompletionFailureDisposition::FailedBeforePromotion
                );
                if pre_promotion_failure
                    && import_integrity_phase2::has_staged_manifest(&active.path, &active.id)
                        .unwrap_or(false)
                {
                    let _ = import_integrity_phase2::discard_staged_manifest(&active.path);
                }
                if pre_promotion_failure {
                    let _ = remove_sqlite_file_set(&temp_index);
                }
                if pre_promotion_failure {
                    let cancelled = disposition
                        == IntegrityCompletionFailureDisposition::CancelledBeforePromotion;
                    let _ = import_integrity_phase2::transition_state(
                        &active.path,
                        &active.id,
                        if cancelled {
                            import_integrity_phase2::WorkspaceState::Cancelled
                        } else {
                            import_integrity_phase2::WorkspaceState::Failed
                        },
                        import_integrity_phase2::StateCounts {
                            discovered_candidates: Some(stats.discovered),
                            canonical_indexed: Some(stats.indexed),
                        },
                        Some(if cancelled {
                            "repair_cancelled"
                        } else {
                            "repair_failed"
                        }),
                    );
                } else if disposition == IntegrityCompletionFailureDisposition::CompletionWins
                    && import_integrity_phase2::read_state(&active.path, &active.id)
                        .ok()
                        .flatten()
                        .is_some_and(|state| {
                            state.state == import_integrity_phase2::WorkspaceState::Verifying
                        })
                {
                    let _ = import_integrity_phase2::transition_state(
                        &active.path,
                        &active.id,
                        import_integrity_phase2::WorkspaceState::Incomplete,
                        import_integrity_phase2::StateCounts {
                            discovered_candidates: Some(stats.discovered),
                            canonical_indexed: Some(stats.indexed),
                        },
                        Some("promotion_finalization_pending"),
                    );
                }
                let _ = append_import_log(
                    &active.path,
                    match disposition {
                        IntegrityCompletionFailureDisposition::CancelledBeforePromotion => {
                            "state=cancelled stage=pre_promotion"
                        }
                        IntegrityCompletionFailureDisposition::FailedBeforePromotion => {
                            "state=failed stage=pre_promotion"
                        }
                        IntegrityCompletionFailureDisposition::CompletionWins => {
                            "state=incomplete reason=promotion_finalization_pending"
                        }
                        IntegrityCompletionFailureDisposition::PreserveUncertain => {
                            "state=uncertain recovery_data=preserved"
                        }
                    },
                );
                return Err(error);
            }
        };

        append_import_log(
            &active.path,
            &format!(
                "state=repair_finished verification_result={:?} workspace_state={} indexed={} discovered={} errors={}",
                report.result,
                report.workspace_state.as_str(),
                stats.indexed,
                stats.discovered,
                stats.error_count
            ),
        )?;

        workspace_summary_from_active(&active, true)
    })();
    progress_guard.finish_with_result(operation_result)
}

fn rebuild_workspace_conversations_blocking(
    app: &tauri::AppHandle,
    active: ActiveWorkspace,
    operation_id: &str,
) -> AppResult<WorkspaceSummary> {
    import_integrity_phase2::validate_operation_id(operation_id)?;
    preflight_integrity_compatibility_status(&active.path, true, false)?;
    let identity = import_integrity_phase2::identity_from_active_workspace(&active)?;
    recover_phase2_workspace_state(&active)?;
    let mut progress_guard = IntegrityOperationFinishGuard::begin(
        app,
        &active,
        operation_id,
        import_progress::OperationKind::Repair,
        import_progress::ProgressStage::BuildingConversations,
    )?;
    let operation_result = (|| {
        import_integrity_phase2::ensure_phase2_manifest_with_progress(
            &active,
            &identity,
            |_, activity_observed| {
                if activity_observed {
                    emit_activity_only_progress(app)
                } else {
                    checkpoint_integrity_operation(app, false)
                }
            },
        )?;
        let index_path = active.path.join("index.sqlite");
        let starting_count = {
            let conn = open_workspace_db_for_read(&active.path)?;
            conn.query_row("SELECT COUNT(*) FROM messages", [], |row| {
                row.get::<_, usize>(0)
            })?
        };
        // The accepted Phase 2 manifest and readable canonical index prove the
        // reused prerequisites for this conversation-only repair generation.
        mark_reused_repair_prerequisites(
            app,
            operation_id,
            import_progress::ProgressStage::Indexing,
        )?;
        import_integrity_phase2::transition_state(
            &active.path,
            &active.id,
            import_integrity_phase2::WorkspaceState::BuildingConversations,
            import_integrity_phase2::StateCounts {
                discovered_candidates: None,
                canonical_indexed: Some(starting_count),
            },
            Some("conversation_rebuild_running"),
        )?;
        let mut conn = open_workspace_db_for_upgrade(&active.path)?;
        let result = build_conversation_index(app, &active.path, &mut conn, &active.id);
        if let Err(error) = result {
            let cancelled = error.code == Some(IMPORT_CANCELLED_CODE);
            let _ = import_integrity_phase2::transition_state(
                &active.path,
                &active.id,
                if cancelled {
                    import_integrity_phase2::WorkspaceState::Cancelled
                } else {
                    import_integrity_phase2::WorkspaceState::Failed
                },
                import_integrity_phase2::StateCounts {
                    discovered_candidates: None,
                    canonical_indexed: Some(starting_count),
                },
                Some(if cancelled {
                    "conversation_rebuild_cancelled"
                } else {
                    "conversation_rebuild_failed"
                }),
            );
            return Err(error);
        }
        let manifest = import_integrity_phase2::read_manifest(&active.path, &active.id)?
            .ok_or_else(|| AppError::new("Import manifest is missing."))?;
        let stats = IndexStats {
            discovered: manifest.candidate_count as usize,
            processed: manifest.candidate_count as usize,
            indexed: starting_count,
            inserted: manifest.inserted_count as usize,
            skipped_existing: manifest.skipped_existing_count as usize,
            error_count: manifest.parse_failure_count as usize,
            ..IndexStats::default()
        };
        drop(conn);
        import_integrity_phase2::update_manifest_from_index(
            &active.path,
            &active.id,
            &index_path,
            &stats,
            "complete",
        )?;
        import_integrity_phase2::verify_active_workspace_after_commit(
            app,
            &active,
            operation_id,
            import_integrity_phase2::VerificationLevel::Quick,
        )?;
        workspace_summary_from_active(&active, true)
    })();
    progress_guard.finish_with_result(operation_result)
}

fn read_stable_message_ids(index_path: &Path) -> AppResult<HashMap<String, i64>> {
    let conn = open_sqlite_read_only_connection(index_path)?;
    configure_sqlite_connection(&conn, true)?;
    verify_read_query_schema(&conn)?;
    let mut statement = conn.prepare("SELECT eml_path, id FROM messages ORDER BY eml_path")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
    })?;
    let mut stable_ids = HashMap::new();
    let mut used_ids = HashSet::new();
    for row in rows {
        let (relative_path, id) = row?;
        if id <= 0
            || !import_integrity_phase2::validate_relative_identity(&relative_path)
            || !used_ids.insert(id)
            || stable_ids.insert(relative_path, id).is_some()
        {
            return Err(AppError::new(
                "The existing index has an invalid stable message identity mapping.",
            ));
        }
    }
    Ok(stable_ids)
}

fn recover_phase2_workspace_state(active: &ActiveWorkspace) -> AppResult<()> {
    if probe_sqlite_user_version(&active.path.join("index.sqlite"))
        .is_ok_and(|version| version.is_some_and(|value| value > SQLITE_SCHEMA_VERSION_CURRENT))
    {
        return Err(AppError::new(
            "Workspace index schema is newer than this version supports.",
        ));
    }
    let state = import_integrity_phase2::ensure_state_for_workspace(&active.path, &active.id)?;
    import_integrity_phase2::recover_interrupted_state(&active.path, &active.id, state)?;
    Ok(())
}

fn validate_replacement_stable_ids(
    index_path: &Path,
    expected: &HashMap<String, i64>,
) -> AppResult<()> {
    let actual = read_stable_message_ids(index_path)?;
    for (relative_path, expected_id) in expected {
        if let Some(actual_id) = actual.get(relative_path) {
            if actual_id != expected_id {
                return Err(AppError::new(
                    "Replacement index did not preserve a stable message identity.",
                ));
            }
        }
    }
    Ok(())
}

fn find_readpst() -> Option<ReadpstLocation> {
    find_bundled_readpst()
}

fn find_bundled_readpst() -> Option<ReadpstLocation> {
    for (name, source) in bundled_readpst_names() {
        for directory in bundled_readpst_search_dirs() {
            let candidate = directory.join(name);
            if is_executable_file(&candidate) {
                let Ok(candidate) = candidate.canonicalize() else {
                    continue;
                };
                if !is_verified_bundled_readpst_file(&candidate) {
                    continue;
                }
                let source = classify_bundled_readpst_source(&candidate, source);
                return Some(ReadpstLocation {
                    path: candidate,
                    source,
                });
            }
        }
    }

    None
}

fn bundled_readpst_names() -> Vec<(&'static str, ReadpstSource)> {
    let mut names = Vec::new();
    let current_arch_source;
    if cfg!(target_arch = "aarch64") {
        current_arch_source = ReadpstSource::AppleSilicon;
        names.push(("readpst-aarch64-apple-darwin", current_arch_source));
    } else {
        current_arch_source = ReadpstSource::Intel;
        names.push(("readpst-x86_64-apple-darwin", current_arch_source));
    }
    names.push(("readpst-universal-apple-darwin", ReadpstSource::Universal));
    names.push(("readpst", current_arch_source));
    names
}

fn classify_bundled_readpst_source(path: &Path, fallback: ReadpstSource) -> ReadpstSource {
    if fallback == ReadpstSource::Universal || is_macho_universal_binary(path) {
        ReadpstSource::Universal
    } else {
        fallback
    }
}

fn is_macho_universal_binary(path: &Path) -> bool {
    let mut magic = [0_u8; 4];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut magic))
        .is_ok()
        && matches!(magic, [0xca, 0xfe, 0xba, 0xbe] | [0xca, 0xfe, 0xba, 0xbf])
}

fn bundled_readpst_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(exe_path) = env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            dirs.push(exe_dir.to_path_buf());
            dirs.push(exe_dir.join("binaries"));

            if exe_dir.file_name().and_then(OsStr::to_str) == Some("MacOS") {
                if let Some(contents_dir) = exe_dir.parent() {
                    dirs.push(contents_dir.join("Resources"));
                    dirs.push(contents_dir.join("Resources").join("binaries"));
                }
            }
        }
    }

    dirs
}

fn is_executable_file(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        !metadata.file_type().is_symlink()
            && metadata.is_file()
            && metadata.nlink() == 1
            && metadata.mode() & 0o111 != 0
    })
}

fn is_verified_bundled_readpst_file(path: &Path) -> bool {
    if !is_executable_file(path) {
        return false;
    }
    sha256_file(path).is_ok_and(|hash| {
        matches!(
            hash.as_str(),
            READPST_X86_64_SHA256 | READPST_ARM64_SHA256 | READPST_UNIVERSAL_SHA256
        )
    })
}

fn identify_pst(path: &Path) -> AppResult<PstIdentity> {
    let canonical_path = canonical_pst_path(path)?;
    let metadata = fs::metadata(&canonical_path)?;
    let modified = metadata
        .modified()
        .unwrap_or(UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let display_path = canonical_path.display().to_string();

    let (content_fingerprint, fingerprint, fingerprint_strategy) =
        fingerprint_pst(&canonical_path, metadata.len(), modified)?;
    let legacy_workspace_id = legacy_workspace_id(&display_path, metadata.len(), modified);

    Ok(PstIdentity {
        canonical_path,
        display_path,
        size: metadata.len(),
        modified_ns: modified,
        workspace_id: content_fingerprint.clone(),
        legacy_workspace_id,
        fingerprint,
        content_fingerprint,
        fingerprint_strategy,
    })
}

fn canonical_pst_path(path: &Path) -> AppResult<PathBuf> {
    if path
        .extension()
        .and_then(OsStr::to_str)
        .map(str::to_lowercase)
        .as_deref()
        != Some("pst")
    {
        return Err(AppError::new("Choose a file with a .pst extension."));
    }

    require_regular_nonsymlink_file(path, "source PST")?;

    let canonical_path = path
        .canonicalize()
        .map_err(|_| AppError::new("The selected PST does not exist or cannot be read."))?;
    require_regular_nonsymlink_file(&canonical_path, "source PST")?;
    let mut file = File::open(&canonical_path)
        .map_err(|_| AppError::new("The selected PST exists but cannot be read."))?;
    let mut signature = [0_u8; 4];
    file.read_exact(&mut signature).map_err(|error| {
        AppError::new(format!(
            "The selected PST header could not be read: {error}"
        ))
    })?;
    if signature != *b"!BDN" {
        return Err(AppError::new(
            "The selected file does not have a valid Outlook PST header.",
        ));
    }
    Ok(canonical_path)
}

fn fingerprint_pst(
    path: &Path,
    size: u64,
    modified_ns: u128,
) -> AppResult<(String, String, String)> {
    if size <= FULL_HASH_LIMIT_BYTES {
        let hash = sha256_file(path)?;
        return Ok((hash.clone(), hash, "full-sha256".to_string()));
    }

    let first_hash = sha256_file_chunk(path, 0, FINGERPRINT_CHUNK_BYTES)?;
    let last_offset = size.saturating_sub(FINGERPRINT_CHUNK_BYTES as u64);
    let last_hash = sha256_file_chunk(path, last_offset, FINGERPRINT_CHUNK_BYTES)?;

    let content_key = format!("fast-content-v1:{size}:{first_hash}:{last_hash}");
    let content_fingerprint = sha256_text(&content_key);
    let fingerprint = sha256_text(&format!("{content_key}:{modified_ns}"));

    Ok((
        content_fingerprint,
        fingerprint,
        format!("fast-boundary-{FINGERPRINT_CHUNK_BYTES}-with-mtime"),
    ))
}

fn sha256_file(path: &Path) -> AppResult<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1024 * 1024];

    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        hasher.update(&buffer[..bytes_read]);
    }

    Ok(hex::encode(hasher.finalize()))
}

fn sha256_file_chunk(path: &Path, offset: u64, max_len: usize) -> AppResult<String> {
    let mut file = File::open(path)?;
    file.seek(SeekFrom::Start(offset))?;

    let mut buffer = vec![0; max_len];
    let bytes_read = file.read(&mut buffer)?;
    let mut hasher = Sha256::new();
    hasher.update(&buffer[..bytes_read]);
    Ok(hex::encode(hasher.finalize()))
}

fn sha256_text(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

fn legacy_workspace_id(display_path: &str, size: u64, modified_ns: u128) -> String {
    let mut hasher = Sha256::new();
    hasher.update(display_path.as_bytes());
    hasher.update(size.to_le_bytes());
    hasher.update(modified_ns.to_le_bytes());
    hex::encode(hasher.finalize())
}

fn application_logs_dir(profile_paths: &RuntimeProfilePaths) -> &Path {
    profile_paths.log_root()
}

fn application_log_path(profile_paths: &RuntimeProfilePaths) -> PathBuf {
    application_logs_dir(profile_paths).join(APPLICATION_LOG_FILE)
}

fn current_macos_version() -> String {
    Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "unavailable".to_string())
}

fn current_cpu_architecture() -> String {
    match env::consts::ARCH {
        "aarch64" => "arm64".to_string(),
        value => value.to_string(),
    }
}

fn current_executable_architecture() -> String {
    env::current_exe()
        .ok()
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| macho_architectures(&bytes))
        .unwrap_or_else(current_cpu_architecture)
}

fn macho_architectures(bytes: &[u8]) -> Option<String> {
    let magic = bytes.get(..4)?;
    if matches!(magic, [0xca, 0xfe, 0xba, 0xbe] | [0xca, 0xfe, 0xba, 0xbf]) {
        let count = u32::from_be_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
        let entry_size = if magic == [0xca, 0xfe, 0xba, 0xbf] {
            32
        } else {
            20
        };
        let mut architectures = Vec::new();
        for index in 0..count {
            let offset = 8 + index * entry_size;
            let cpu_type = u32::from_be_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?);
            if let Some(architecture) = macho_cpu_type_label(cpu_type) {
                if !architectures.contains(&architecture) {
                    architectures.push(architecture);
                }
            }
        }
        if !architectures.is_empty() {
            return Some(architectures.join(" + "));
        }
    }

    let (little_endian, cpu_offset) = match magic {
        [0xcf, 0xfa, 0xed, 0xfe] | [0xce, 0xfa, 0xed, 0xfe] => (true, 4),
        [0xfe, 0xed, 0xfa, 0xcf] | [0xfe, 0xed, 0xfa, 0xce] => (false, 4),
        _ => return None,
    };
    let raw = bytes.get(cpu_offset..cpu_offset + 4)?.try_into().ok()?;
    let cpu_type = if little_endian {
        u32::from_le_bytes(raw)
    } else {
        u32::from_be_bytes(raw)
    };
    macho_cpu_type_label(cpu_type).map(str::to_string)
}

fn macho_cpu_type_label(cpu_type: u32) -> Option<&'static str> {
    match cpu_type {
        0x0100_0007 => Some("x86_64"),
        0x0100_000c => Some("arm64"),
        _ => None,
    }
}

fn active_database_diagnostics(active: &ActiveWorkspace) -> (Option<i64>, String) {
    let database_path = active.path.join("index.sqlite");
    if !database_path.is_file() {
        return (None, "index unavailable".to_string());
    }
    let Ok(conn) = open_workspace_db_for_read(&active.path) else {
        return (None, "index unavailable".to_string());
    };
    let schema_version = conn
        .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
        .ok();
    let conversation_status = conn
        .query_row(
            "SELECT value FROM import_metadata WHERE key = 'conversation_schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .ok()
        .flatten()
        .map(|value| {
            if value == CONVERSATION_SCHEMA_VERSION {
                "indexed"
            } else {
                "reindex required"
            }
        })
        .unwrap_or("reindex required")
        .to_string();
    (schema_version, conversation_status)
}

fn workspaces_dir(profile_paths: &RuntimeProfilePaths) -> AppResult<PathBuf> {
    create_private_directory_all(profile_paths.application_data_root())?;
    let directory = profile_paths
        .workspace_registry_session_root()
        .to_path_buf();
    create_private_directory_all(&directory)?;
    Ok(directory)
}

fn standalone_exports_dir(
    profile_paths: &RuntimeProfilePaths,
    format: StandaloneSourceFormat,
) -> AppResult<PathBuf> {
    create_private_directory_all(profile_paths.application_data_root())?;
    let directory = match format {
        StandaloneSourceFormat::Eml => profile_paths.standalone_eml_export_root(),
        StandaloneSourceFormat::Msg => profile_paths.standalone_msg_export_root(),
    }
    .to_path_buf();
    create_private_directory_all(&directory)?;
    Ok(directory)
}

fn build_external_file_open_batch(
    candidates: Vec<PathBuf>,
    mut warnings: Vec<String>,
) -> ExternalFileOpenBatch {
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    let mut skipped_count = 0;

    for candidate in candidates {
        let display_path = candidate.display().to_string();
        match prepare_external_file_open(candidate) {
            Ok(file) => {
                if !seen.insert(file.path.clone()) {
                    continue;
                }
                if files.len() < MAX_EXTERNAL_FILES_PER_REQUEST {
                    files.push(file);
                } else {
                    skipped_count += 1;
                }
            }
            Err(error) => {
                skipped_count += 1;
                warnings.push(format!("Skipped {display_path}: {error}"));
            }
        }
    }

    if files.len() == MAX_EXTERNAL_FILES_PER_REQUEST && skipped_count > 0 {
        warnings.push(format!(
            "PST QuickView opens at most {MAX_EXTERNAL_FILES_PER_REQUEST} files at once; skipped {skipped_count} additional or unsupported file{}.",
            if skipped_count == 1 { "" } else { "s" }
        ));
    }

    ExternalFileOpenBatch {
        files,
        warnings,
        skipped_count,
    }
}

fn authorize_standalone_file(
    state: &mut ExternalFileOpenState,
    file: &ExternalFileOpen,
) -> AppResult<()> {
    if file.file_kind == "pst" {
        return Ok(());
    }
    if !is_lower_hex_sha256(&file.stable_id) {
        return Err(AppError::new("Invalid standalone message identity."));
    }
    let canonical_path = canonical_standalone_message_path(file.path.clone())?;
    let canonical_string = canonical_path.display().to_string();
    if canonical_string != file.path || sha256_text(&canonical_string) != file.stable_id {
        return Err(AppError::new(
            "Standalone message identity does not match the selected file.",
        ));
    }
    let identity = file_identity(&canonical_path, "standalone message")?;

    if state
        .authorized_standalone_files
        .contains_key(&file.stable_id)
    {
        state.authorization_order.retain(|id| id != &file.stable_id);
    }
    while state.authorized_standalone_files.len() >= MAX_AUTHORIZED_STANDALONE_FILES {
        let Some(oldest) = state.authorization_order.pop_front() else {
            break;
        };
        state.authorized_standalone_files.remove(&oldest);
    }
    state.authorized_standalone_files.insert(
        file.stable_id.clone(),
        AuthorizedStandaloneFile {
            canonical_path,
            identity,
        },
    );
    state.authorization_order.push_back(file.stable_id.clone());
    Ok(())
}

fn authorize_standalone_files(
    state: &mut ExternalFileOpenState,
    batch: &ExternalFileOpenBatch,
) -> AppResult<()> {
    for file in batch.files.iter().filter(|file| file.file_kind != "pst") {
        authorize_standalone_file(state, file)?;
    }
    Ok(())
}

fn authorized_standalone_path(state: &AppState, stable_id: &str) -> AppResult<PathBuf> {
    if !is_lower_hex_sha256(stable_id) {
        return Err(AppError::new("Invalid standalone preview identity."));
    }
    let record = state
        .external_file_opens
        .lock()
        .map_err(|_| AppError::new("External file-open state lock was poisoned."))?
        .authorized_standalone_files
        .get(stable_id)
        .cloned()
        .ok_or_else(|| AppError::new("Standalone preview authorization is unavailable."))?;

    let canonical = canonical_standalone_message_path(record.canonical_path.display().to_string())?;
    let canonical_string = canonical.display().to_string();
    if canonical != record.canonical_path
        || sha256_text(&canonical_string) != stable_id
        || file_identity(&canonical, "standalone message")? != record.identity
    {
        if let Ok(mut external) = state.external_file_opens.lock() {
            external.authorized_standalone_files.remove(stable_id);
            external.authorization_order.retain(|id| id != stable_id);
        }
        return Err(AppError::new(
            "Standalone preview authorization no longer matches the selected file.",
        ));
    }
    Ok(canonical)
}

fn prepare_external_file_open(path: PathBuf) -> AppResult<ExternalFileOpen> {
    let extension = path
        .extension()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_ascii_lowercase();
    let (canonical, file_kind) = if extension == "pst" {
        (canonical_pst_path(&path)?, "pst".to_string())
    } else {
        let canonical = canonical_standalone_message_path(path.display().to_string())?;
        let format = StandaloneSourceFormat::from_path(&canonical)?;
        (canonical, format.as_str().to_string())
    };
    Ok(ExternalFileOpen {
        stable_id: sha256_text(&canonical.display().to_string()),
        path: canonical.display().to_string(),
        file_kind,
    })
}

fn dispatch_external_file_open_batch(app: &tauri::AppHandle, batch: ExternalFileOpenBatch) {
    if batch.files.iter().any(|file| file.file_kind == "pst") {
        show_and_focus_main_window(app);
    }
    let state = app.state::<AppState>();
    let should_emit = {
        let Ok(mut external) = state.external_file_opens.lock() else {
            eprintln!("External file-open state lock was poisoned.");
            return;
        };
        if authorize_standalone_files(&mut external, &batch).is_err() {
            eprintln!("Could not authorize a selected standalone message.");
            return;
        }
        external.external_open_received = true;
        if external.frontend_ready {
            true
        } else {
            queue_external_file_open_batch(&mut external, batch.clone());
            false
        }
    };

    if should_emit {
        if let Err(error) = app.emit_to("main", "external-file-open", batch.clone()) {
            let _ = error;
            eprintln!("Could not deliver Finder file-open event.");
            if let Ok(mut external) = state.external_file_opens.lock() {
                external.frontend_ready = false;
                queue_external_file_open_batch(&mut external, batch);
            }
        }
    }
}

fn queue_external_file_open_batch(
    state: &mut ExternalFileOpenState,
    mut batch: ExternalFileOpenBatch,
) {
    let queued_paths = state
        .pending
        .iter()
        .flat_map(|pending| pending.files.iter().map(|file| file.path.as_str()))
        .collect::<HashSet<_>>();
    batch
        .files
        .retain(|file| !queued_paths.contains(file.path.as_str()));

    let queued_file_count = state
        .pending
        .iter()
        .map(|pending| pending.files.len())
        .sum::<usize>();
    let available = MAX_PENDING_EXTERNAL_FILES.saturating_sub(queued_file_count);
    if batch.files.len() > available {
        let dropped = batch.files.len() - available;
        batch.files.truncate(available);
        batch.skipped_count += dropped;
        batch.warnings.push(format!(
            "Skipped {dropped} file{} because the startup file-open queue is full.",
            if dropped == 1 { "" } else { "s" }
        ));
    }

    if batch.files.is_empty() && batch.warnings.is_empty() {
        return;
    }
    if state.pending.len() >= MAX_PENDING_EXTERNAL_BATCHES {
        if let Some(last) = state.pending.back_mut() {
            last.skipped_count += batch.files.len();
            last.warnings.push(
                "Additional file-open requests were skipped because the startup queue is full."
                    .to_string(),
            );
        }
        return;
    }
    state.pending.push_back(batch);
}

fn show_and_focus_main_window(app: &tauri::AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        eprintln!("Could not focus the main window because it is not available.");
        return;
    };
    if let Err(error) = window.unminimize() {
        let _ = error;
        eprintln!("Could not unminimize the main window.");
    }
    if let Err(error) = window.show() {
        let _ = error;
        eprintln!("Could not show the main window.");
    }
    if let Err(error) = window.set_focus() {
        let _ = error;
        eprintln!("Could not focus the main window.");
    }
}

fn canonical_standalone_message_path(path: String) -> AppResult<PathBuf> {
    let path = PathBuf::from(path);
    require_regular_nonsymlink_file(&path, "standalone message")?;
    let path = path
        .canonicalize()
        .map_err(|error| AppError::new(format!("Message file is not available: {error}")))?;
    require_regular_nonsymlink_file(&path, "standalone message")?;
    let format = StandaloneSourceFormat::from_path(&path)?;
    validate_standalone_message_format(&path, format)?;
    Ok(path)
}

fn validate_standalone_message_format(
    path: &Path,
    format: StandaloneSourceFormat,
) -> AppResult<()> {
    let mut file = File::open(path).map_err(|error| {
        AppError::new(format!(
            "Message file is not readable: {}: {error}",
            path.display()
        ))
    })?;
    let mut prefix = vec![0_u8; MESSAGE_FORMAT_PROBE_BYTES];
    let bytes_read = file.read(&mut prefix).map_err(|error| {
        AppError::new(format!(
            "Could not read message file {}: {error}",
            path.display()
        ))
    })?;
    prefix.truncate(bytes_read);
    if prefix.is_empty() {
        return Err(AppError::new(format!(
            "Message file is empty: {}",
            path.display()
        )));
    }

    match format {
        StandaloneSourceFormat::Msg => {
            const COMPOUND_FILE_SIGNATURE: &[u8; 8] = b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1";
            if !prefix.starts_with(COMPOUND_FILE_SIGNATURE) {
                return Err(AppError::new(format!(
                    "File has a .msg extension but is not an Outlook compound message: {}",
                    path.display()
                )));
            }
        }
        StandaloneSourceFormat::Eml => {
            if prefix.contains(&0) || !looks_like_rfc822_message(&prefix) {
                return Err(AppError::new(format!(
                    "File has a .eml extension but does not contain recognizable RFC 822 headers: {}",
                    path.display()
                )));
            }
        }
    }
    Ok(())
}

fn looks_like_rfc822_message(prefix: &[u8]) -> bool {
    let text = String::from_utf8_lossy(prefix);
    let mut header_count = 0;
    for line in text.lines().take(200) {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            break;
        }
        if line.starts_with(' ') || line.starts_with('\t') || line.starts_with("From ") {
            continue;
        }
        let Some((name, _)) = line.split_once(':') else {
            continue;
        };
        if !name.is_empty()
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            header_count += 1;
        }
    }
    header_count > 0
}

fn path_is_under_any_standalone_export_root(
    profile_paths: &RuntimeProfilePaths,
    path: &Path,
) -> AppResult<bool> {
    for format in [StandaloneSourceFormat::Eml, StandaloneSourceFormat::Msg] {
        if path_is_under_root(path, &standalone_exports_dir(profile_paths, format)?)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn standalone_message_export_root(
    profile_paths: &RuntimeProfilePaths,
    source_path: &Path,
) -> AppResult<PathBuf> {
    let format = StandaloneSourceFormat::from_path(source_path)?;
    let metadata = source_path.metadata()?;
    let modified_ns = metadata
        .modified()
        .unwrap_or(UNIX_EPOCH)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let key = format!(
        "standalone-{}-v1:{}:{}:{}",
        format.as_str(),
        source_path.display(),
        metadata.len(),
        modified_ns
    );
    let root = standalone_exports_dir(profile_paths, format)?.join(sha256_text(&key));
    create_private_directory_all(&root)?;
    Ok(root)
}

fn select_workspace(
    profile_paths: &RuntimeProfilePaths,
    identity: &PstIdentity,
    requested_mode: WorkspaceLocationMode,
) -> AppResult<WorkspaceSelection> {
    match requested_mode {
        WorkspaceLocationMode::AppSupport => Ok(WorkspaceSelection {
            path: workspaces_dir(profile_paths)?.join(&identity.workspace_id),
            mode: WorkspaceLocationMode::AppSupport,
            warning: None,
        }),
        WorkspaceLocationMode::NextToPst => match next_to_pst_root(identity) {
            Some(root) if pst_parent_is_writable(identity) => Ok(WorkspaceSelection {
                path: root.join(&identity.workspace_id),
                mode: WorkspaceLocationMode::NextToPst,
                warning: None,
            }),
            Some(root) => Ok(WorkspaceSelection {
                path: workspaces_dir(profile_paths)?.join(&identity.workspace_id),
                mode: WorkspaceLocationMode::AppSupport,
                warning: Some(format!(
                    "The PST folder is not writable, so the workspace will use App Support instead of {}.",
                    root.display()
                )),
            }),
            None => Ok(WorkspaceSelection {
                path: workspaces_dir(profile_paths)?.join(&identity.workspace_id),
                mode: WorkspaceLocationMode::AppSupport,
                warning: Some("Could not locate the PST parent folder, so the workspace will use App Support.".to_string()),
            }),
        },
    }
}

fn next_to_pst_root(identity: &PstIdentity) -> Option<PathBuf> {
    identity
        .canonical_path
        .parent()
        .map(|parent| parent.join(NEXT_TO_PST_WORKSPACE_DIR))
}

fn legacy_next_to_pst_root(identity: &PstIdentity) -> Option<PathBuf> {
    identity
        .canonical_path
        .parent()
        .map(|parent| parent.join(LEGACY_NEXT_TO_PST_WORKSPACE_DIR))
}

fn next_to_pst_candidate_roots(identity: &PstIdentity) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(root) = next_to_pst_root(identity) {
        roots.push(root);
    }
    if let Some(root) = legacy_next_to_pst_root(identity) {
        roots.push(root);
    }
    roots
}

fn pst_parent_is_writable(identity: &PstIdentity) -> bool {
    let Some(parent) = identity.canonical_path.parent() else {
        return false;
    };

    let test_path = temporary_write_probe_path(parent, "write-test");
    match create_private_file_new(&test_path) {
        Ok(_) => {
            let _ = fs::remove_file(test_path);
            true
        }
        Err(_) => false,
    }
}

fn workspace_preflight(
    identity: &PstIdentity,
    selection: &WorkspaceSelection,
) -> WorkspacePreflight {
    let workspace_parent = selection
        .path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| selection.path.clone());
    let original_pst_exists = identity.canonical_path.exists();
    let original_pst_readable = File::open(&identity.canonical_path).is_ok();
    let estimated_required_bytes = estimate_required_cache_bytes(identity.size);
    let available_disk_bytes = available_disk_bytes_for_path(&selection.path).ok();
    let has_enough_space =
        available_disk_bytes.map(|available| available >= estimated_required_bytes);
    let space_warning = has_enough_space == Some(false);
    let (workspace_parent_writable, workspace_parent_write_error) =
        workspace_parent_write_status(&workspace_parent);

    let mut warnings = Vec::new();
    if let Some(warning) = &selection.warning {
        warnings.push(warning.clone());
    }
    if !original_pst_exists {
        warnings.push(format!(
            "The original PST was not found: {}.",
            identity.display_path
        ));
    } else if !original_pst_readable {
        warnings.push(format!(
            "The original PST exists but cannot be read: {}.",
            identity.display_path
        ));
    }
    if !workspace_parent_writable {
        warnings.push(format!(
            "The selected workspace parent is not writable: {}{}",
            workspace_parent.display(),
            workspace_parent_write_error
                .as_ref()
                .map(|error| format!(" ({error})"))
                .unwrap_or_default()
        ));
    }
    if space_warning {
        if let Some(available) = available_disk_bytes {
            warnings.push(format!(
                "This PST is {}. The selected cache volume has {} free. Import may require about {}.",
                format_bytes(identity.size),
                format_bytes(available),
                format_bytes(estimated_required_bytes)
            ));
        }
    }

    WorkspacePreflight {
        original_pst_path: identity.display_path.clone(),
        original_pst_exists,
        original_pst_readable,
        pst_size_bytes: identity.size,
        workspace_path: selection.path.display().to_string(),
        workspace_parent_path: workspace_parent.display().to_string(),
        workspace_location_mode: selection.mode.as_str().to_string(),
        workspace_location_label: selection.mode.label().to_string(),
        workspace_parent_writable,
        workspace_parent_write_error,
        available_disk_bytes,
        estimated_required_bytes,
        has_enough_space,
        space_warning,
        warning_required: !warnings.is_empty(),
        warnings,
    }
}

fn estimate_required_cache_bytes(pst_size: u64) -> u64 {
    let one_and_half = ((pst_size as u128) * 3 / 2).min(u64::MAX as u128) as u64;
    let plus_two_gib = pst_size.saturating_add(2 * GIB_BYTES);
    one_and_half.max(plus_two_gib)
}

fn temporary_write_probe_path(parent: &Path, purpose: &str) -> PathBuf {
    let sequence = NEXT_WRITE_PROBE.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    parent.join(format!(
        ".pst-quickview-{purpose}-{}-{timestamp}-{sequence}",
        std::process::id()
    ))
}

fn workspace_parent_write_status(parent: &Path) -> (bool, Option<String>) {
    if parent.exists() && !parent.is_dir() {
        return (
            false,
            Some(format!(
                "{} exists but is not a directory",
                parent.display()
            )),
        );
    }

    let Some(test_dir) = nearest_existing_ancestor(parent) else {
        return (
            false,
            Some("no existing parent directory was found".to_string()),
        );
    };

    if !test_dir.is_dir() {
        return (
            false,
            Some(format!("{} is not a directory", test_dir.display())),
        );
    }

    let test_path = temporary_write_probe_path(&test_dir, "preflight");

    match create_private_file_new(&test_path) {
        Ok(mut file) => {
            let write_result = file.write_all(b"pst-quickview-preflight");
            drop(file);
            let cleanup_result = fs::remove_file(&test_path);
            if let Err(error) = write_result {
                return (false, Some(error.to_string()));
            }
            if let Err(error) = cleanup_result {
                return (
                    false,
                    Some(format!(
                        "write test succeeded but cleanup failed for {}: {}",
                        test_path.display(),
                        error
                    )),
                );
            }
            (true, None)
        }
        Err(error) => (false, Some(error.to_string())),
    }
}

fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = Some(path);
    while let Some(candidate) = current {
        if candidate.exists() {
            return Some(candidate.to_path_buf());
        }
        current = candidate.parent();
    }
    None
}

fn workspace_path_for_id(
    profile_paths: &RuntimeProfilePaths,
    workspace_id: &str,
) -> AppResult<PathBuf> {
    if !is_lower_hex_sha256(workspace_id) {
        return Err(AppError::new("Invalid workspace id."));
    }

    Ok(workspaces_dir(profile_paths)?.join(workspace_id))
}

fn validate_session_workspace_path(
    profile_paths: &RuntimeProfilePaths,
    pst_path: &Path,
    workspace_path: &Path,
    workspace_id: &str,
    location_mode: WorkspaceLocationMode,
) -> AppResult<()> {
    match fs::symlink_metadata(pst_path) {
        Ok(_) => {
            require_regular_nonsymlink_file(pst_path, "source PST")?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(AppError::new("PST or workspace not available.")),
    }
    let workspace_metadata = fs::symlink_metadata(workspace_path)
        .map_err(|_| AppError::new("PST or workspace not available."))?;
    if workspace_metadata.file_type().is_symlink() || !workspace_metadata.is_dir() {
        return Err(AppError::new("PST or workspace not available."));
    }
    if !is_lower_hex_sha256(workspace_id) || workspace_id_from_path(workspace_path)? != workspace_id
    {
        return Err(AppError::new(
            "Saved workspace id does not match the workspace path.",
        ));
    }

    match location_mode {
        WorkspaceLocationMode::AppSupport => {
            let root = workspaces_dir(profile_paths)?;
            let parent = workspace_path
                .parent()
                .ok_or_else(|| AppError::new("Workspace path does not have a parent."))?;
            if !paths_equivalent(parent, &root) {
                return Err(AppError::new(
                    "Saved App Support workspace is outside the PST QuickView workspace root.",
                ));
            }
        }
        WorkspaceLocationMode::NextToPst => {
            let workspace_parent = workspace_path
                .parent()
                .ok_or_else(|| AppError::new("Workspace path does not have a parent."))?;
            let parent_name = workspace_parent.file_name().and_then(OsStr::to_str);
            if parent_name != Some(NEXT_TO_PST_WORKSPACE_DIR)
                && parent_name != Some(LEGACY_NEXT_TO_PST_WORKSPACE_DIR)
            {
                return Err(AppError::new(
                    "Saved next-to-PST workspace is not under a PST QuickView cache folder.",
                ));
            }

            let pst_parent = pst_path
                .parent()
                .ok_or_else(|| AppError::new("PST path does not have a parent."))?;
            let cache_parent = workspace_parent
                .parent()
                .ok_or_else(|| AppError::new("Workspace cache folder does not have a parent."))?;
            if !paths_equivalent(pst_parent, cache_parent) {
                return Err(AppError::new(
                    "Saved next-to-PST workspace is not beside the selected PST.",
                ));
            }
        }
    }

    let marker = workspace_path.join(WORKSPACE_MARKER_FILE);
    let marker = read_workspace_marker(&marker)?;
    if marker.app != WORKSPACE_MARKER_APP_NAME
        || marker.workspace_id != workspace_id
        || marker.workspace_location_mode != location_mode.as_str()
    {
        return Err(AppError::new("Saved workspace marker is not valid."));
    }

    Ok(())
}

fn validate_session_workspace_scope_before_access(
    profile_paths: &RuntimeProfilePaths,
    pst_path: &Path,
    workspace_path: &Path,
    workspace_id: &str,
    location_mode: WorkspaceLocationMode,
) -> AppResult<()> {
    if !is_lower_hex_sha256(workspace_id)
        || workspace_path.file_name() != Some(OsStr::new(workspace_id))
    {
        return Err(AppError::new("Saved workspace path is not valid."));
    }
    match location_mode {
        WorkspaceLocationMode::AppSupport => {
            if workspace_path
                != profile_paths
                    .workspace_registry_session_root()
                    .join(workspace_id)
            {
                return Err(AppError::new(
                    "Saved App Support workspace is outside the PST QuickView workspace root.",
                ));
            }
        }
        WorkspaceLocationMode::NextToPst => {
            let workspace_parent = workspace_path
                .parent()
                .ok_or_else(|| AppError::new("Workspace path does not have a parent."))?;
            let parent_name = workspace_parent.file_name().and_then(OsStr::to_str);
            if parent_name != Some(NEXT_TO_PST_WORKSPACE_DIR)
                && parent_name != Some(LEGACY_NEXT_TO_PST_WORKSPACE_DIR)
            {
                return Err(AppError::new(
                    "Saved next-to-PST workspace is not under a PST QuickView cache folder.",
                ));
            }
            let cache_parent = workspace_parent
                .parent()
                .ok_or_else(|| AppError::new("Workspace cache folder does not have a parent."))?;
            let pst_parent = pst_path
                .parent()
                .ok_or_else(|| AppError::new("PST path does not have a parent."))?;
            if cache_parent != pst_parent {
                return Err(AppError::new(
                    "Saved next-to-PST workspace is not beside the selected PST.",
                ));
            }
        }
    }
    Ok(())
}

fn set_active_workspace(state: &State<'_, AppState>, summary: &WorkspaceSummary) -> AppResult<()> {
    let mode = WorkspaceLocationMode::from_arg(&summary.workspace_location_mode)?;
    let workspace_path = PathBuf::from(&summary.workspace_path);
    let requested_pst_path = PathBuf::from(&summary.pst_path);
    validate_session_workspace_scope_before_access(
        state.runtime_profile()?.paths(),
        &requested_pst_path,
        &workspace_path,
        &summary.id,
        mode,
    )?;
    let workspace_path = workspace_path.canonicalize()?;
    let pst_path = requested_pst_path
        .canonicalize()
        .unwrap_or(requested_pst_path);
    validate_session_workspace_path(
        state.runtime_profile()?.paths(),
        &pst_path,
        &workspace_path,
        &summary.id,
        mode,
    )?;
    let active = ActiveWorkspace {
        id: summary.id.clone(),
        path: workspace_path,
        pst_path,
        fingerprint: summary.fingerprint.clone(),
        location_mode: mode,
    };

    remember_open_workspace(state.inner(), &active)?;

    let mut guard = state
        .active_workspace
        .lock()
        .map_err(|_| AppError::new("Could not lock active workspace state."))?;
    *guard = Some(active);

    Ok(())
}

fn remember_open_workspace(state: &AppState, active: &ActiveWorkspace) -> AppResult<()> {
    validate_workspace_identity_input(&active.id)?;
    let mut open_guard = state
        .open_workspaces
        .lock()
        .map_err(|_| AppError::new("Could not lock open workspace state."))?;
    if !open_guard.contains_key(&active.id) && open_guard.len() >= MAX_OPEN_WORKSPACES {
        return Err(AppError::new(format!(
            "At most {MAX_OPEN_WORKSPACES} PST workspaces can be open at once."
        )));
    }
    open_guard.insert(active.id.clone(), active.clone());
    Ok(())
}

fn validate_workspace_batch_size(count: usize) -> AppResult<()> {
    if count > MAX_OPEN_WORKSPACES {
        return Err(AppError::new(format!(
            "A search can target at most {MAX_OPEN_WORKSPACES} open PST workspaces."
        )));
    }
    Ok(())
}

fn validate_workspace_identity_input(workspace_id: &str) -> AppResult<()> {
    if workspace_id.is_empty()
        || workspace_id.len() > MAX_WORKSPACE_ID_SCALARS.saturating_mul(4)
        || workspace_id.chars().count() > MAX_WORKSPACE_ID_SCALARS
        || workspace_id.chars().any(char::is_control)
    {
        return Err(AppError::new("Invalid workspace identity."));
    }
    Ok(())
}

fn validate_conversation_identity_input(conversation_id: &str) -> AppResult<()> {
    if conversation_id.is_empty()
        || conversation_id.len() > MAX_CONVERSATION_ID_SCALARS.saturating_mul(4)
        || conversation_id.chars().count() > MAX_CONVERSATION_ID_SCALARS
        || conversation_id.chars().any(char::is_control)
    {
        return Err(AppError::new("Invalid conversation identity."));
    }
    Ok(())
}

fn validate_search_offset(offset: Option<i64>) -> AppResult<Option<i64>> {
    if offset.is_some_and(|value| !(0..=MAX_SEARCH_OFFSET_ROWS).contains(&value)) {
        return Err(AppError::new(format!(
            "Search offset must be between 0 and {MAX_SEARCH_OFFSET_ROWS} rows."
        )));
    }
    Ok(offset)
}

fn checked_result_count(current: i64, additional: i64) -> AppResult<i64> {
    if current < 0 || additional < 0 {
        return Err(AppError::new("Workspace returned an invalid result count."));
    }
    current
        .checked_add(additional)
        .ok_or_else(|| AppError::new("Combined result count exceeds the supported range."))
}

fn active_workspace(state: &State<'_, AppState>) -> AppResult<ActiveWorkspace> {
    state
        .active_workspace
        .lock()
        .map_err(|_| AppError::new("Could not lock active workspace state."))?
        .clone()
        .ok_or_else(|| AppError::new("No PST workspace is currently open."))
}

fn active_workspace_for_id(
    state: &State<'_, AppState>,
    workspace_id: &str,
) -> AppResult<ActiveWorkspace> {
    validate_workspace_identity_input(workspace_id)?;
    if let Ok(active) = active_workspace(state) {
        if active.id == workspace_id {
            return Ok(active);
        }
    }

    state
        .open_workspaces
        .lock()
        .map_err(|_| AppError::new("Could not lock open workspace state."))?
        .get(workspace_id)
        .cloned()
        .ok_or_else(|| AppError::new("Requested workspace is not open."))
}

fn workspace_for_integrity_operation(
    state: &State<'_, AppState>,
    workspace_id: &str,
) -> AppResult<ActiveWorkspace> {
    if let Ok(active) = active_workspace_for_id(state, workspace_id) {
        return Ok(active);
    }
    if !is_lower_hex_sha256(workspace_id) {
        return Err(AppError::new("Requested workspace identity is invalid."));
    }
    let workspace = workspace_path_for_id(state.runtime_profile()?.paths(), workspace_id)?;
    let workspace_metadata = fs::symlink_metadata(&workspace)
        .map_err(|_| AppError::new("Requested workspace is unavailable."))?;
    if workspace_metadata.file_type().is_symlink() || !workspace_metadata.is_dir() {
        return Err(AppError::new("Requested workspace is unavailable."));
    }
    let parent = workspace
        .parent()
        .ok_or_else(|| AppError::new("Requested workspace is unavailable."))?;
    if !paths_equivalent(parent, &workspaces_dir(state.runtime_profile()?.paths())?) {
        return Err(AppError::new(
            "Requested workspace is outside the trusted root.",
        ));
    }
    let marker = read_workspace_marker(&workspace.join(WORKSPACE_MARKER_FILE))?;
    if marker.app != WORKSPACE_MARKER_APP_NAME
        || marker.workspace_id != workspace_id
        || marker.workspace_location_mode != WorkspaceLocationMode::AppSupport.as_str()
        || marker.pst_content_fingerprint != workspace_id
    {
        return Err(AppError::new("Requested workspace marker is invalid."));
    }
    Ok(ActiveWorkspace {
        id: workspace_id.to_string(),
        path: workspace,
        pst_path: PathBuf::from(marker.original_pst_path),
        fingerprint: marker.pst_content_fingerprint,
        location_mode: WorkspaceLocationMode::AppSupport,
    })
}

fn active_workspace_from_path(
    path: PathBuf,
    pst_path: PathBuf,
    fingerprint: String,
    location_mode: WorkspaceLocationMode,
) -> ActiveWorkspace {
    let id = path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or_default()
        .to_string();

    ActiveWorkspace {
        id,
        path,
        pst_path,
        fingerprint,
        location_mode,
    }
}

fn resolve_workspace_for_id(state: &State<'_, AppState>, workspace_id: &str) -> AppResult<PathBuf> {
    if let Ok(active) = active_workspace_for_id(state, workspace_id) {
        return Ok(active.path);
    }

    workspace_path_for_id(state.runtime_profile()?.paths(), workspace_id)
}

fn workspace_location_for_identity(
    profile_paths: &RuntimeProfilePaths,
    path: &Path,
    identity: &PstIdentity,
) -> AppResult<Option<WorkspaceLocationMode>> {
    let app_support_root = workspaces_dir(profile_paths)?;
    if workspace_path_is_direct_child_for_identity(path, &app_support_root, identity) {
        return Ok(Some(WorkspaceLocationMode::AppSupport));
    }

    for next_to_pst_root in next_to_pst_candidate_roots(identity) {
        if workspace_path_is_direct_child_for_identity(path, &next_to_pst_root, identity) {
            return Ok(Some(WorkspaceLocationMode::NextToPst));
        }
    }

    Ok(None)
}

fn workspace_path_is_direct_child_for_identity(
    path: &Path,
    root: &Path,
    identity: &PstIdentity,
) -> bool {
    if !path
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name == identity.workspace_id || name == identity.legacy_workspace_id)
    {
        return false;
    }

    path.parent()
        .is_some_and(|parent| paths_equivalent(parent, root))
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    let left = left.canonicalize().unwrap_or_else(|_| left.to_path_buf());
    let right = right.canonicalize().unwrap_or_else(|_| right.to_path_buf());
    left == right
}

fn open_workspace_db_for_upgrade(workspace: &Path) -> AppResult<Connection> {
    let db_path = workspace.join("index.sqlite");
    if !db_path.is_file() {
        return Err(AppError::new("This workspace does not have an index yet."));
    }

    let conn = open_sqlite_write_connection(&db_path)?;
    initialize_schema(&conn)?;
    Ok(conn)
}

fn open_workspace_db_for_read(workspace: &Path) -> AppResult<Connection> {
    let conn = open_workspace_db_read_only_connection(workspace)?;
    configure_workspace_db_for_read(&conn)?;
    Ok(conn)
}

fn open_workspace_db_for_search(
    workspace: &Path,
    operation: &SearchOperationGuard,
) -> AppResult<Connection> {
    operation.check_cancelled()?;
    let conn = open_workspace_db_read_only_connection(workspace)?;
    operation.register_connection(&conn)?;
    configure_workspace_db_for_read(&conn)?;
    operation.check_cancelled()?;
    Ok(conn)
}

fn open_workspace_db_read_only_connection(workspace: &Path) -> AppResult<Connection> {
    let db_path = workspace.join("index.sqlite");
    open_sqlite_read_only_connection(&db_path)
}

fn open_sqlite_read_only_connection(db_path: &Path) -> AppResult<Connection> {
    validate_sqlite_file_set(db_path, true)?;
    let open_path = db_path
        .canonicalize()
        .map_err(|_| AppError::new("Workspace index is unavailable."))?;
    // Do not use immutable=1: active WAL workspaces must remain visible to readers.
    let conn = Connection::open_with_flags(
        &open_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_EXRESCODE,
    )?;
    Ok(conn)
}

fn probe_sqlite_user_version(db_path: &Path) -> AppResult<Option<i64>> {
    match fs::symlink_metadata(db_path) {
        Ok(_) => {
            let conn = open_sqlite_read_only_connection(db_path)?;
            configure_sqlite_connection(&conn, true)?;
            Ok(Some(read_schema_version(&conn)?))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(AppError::new("Workspace index is unavailable.")),
    }
}

fn configure_workspace_db_for_read(conn: &Connection) -> AppResult<()> {
    configure_sqlite_connection(conn, true)?;
    verify_read_query_schema(conn)?;
    Ok(())
}

fn open_sqlite_write_connection(db_path: &Path) -> AppResult<Connection> {
    validate_sqlite_file_set(db_path, false)?;
    let parent = db_path
        .parent()
        .ok_or_else(|| AppError::new("Workspace index path has no parent directory."))?;
    let parent_metadata = fs::symlink_metadata(parent)
        .map_err(|_| AppError::new("Workspace index parent is unavailable."))?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return Err(AppError::new(
            "Refusing a symlink or non-directory workspace index parent.",
        ));
    }
    let canonical_parent = parent
        .canonicalize()
        .map_err(|_| AppError::new("Workspace index parent is unavailable."))?;
    let file_name = db_path
        .file_name()
        .ok_or_else(|| AppError::new("Workspace index path has no file name."))?;
    let open_path = canonical_parent.join(file_name);

    let conn = Connection::open_with_flags(
        &open_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_PRIVATE_CACHE
            | OpenFlags::SQLITE_OPEN_NOFOLLOW
            | OpenFlags::SQLITE_OPEN_EXRESCODE,
    )?;
    configure_sqlite_connection(&conn, false)?;
    Ok(conn)
}

fn validate_sqlite_file_set(db_path: &Path, require_database: bool) -> AppResult<()> {
    let companion = |suffix: &str| {
        let mut path = db_path.as_os_str().to_os_string();
        path.push(suffix);
        PathBuf::from(path)
    };
    for (path, required) in [
        (db_path.to_path_buf(), require_database),
        (companion("-wal"), false),
        (companion("-shm"), false),
        (companion("-journal"), false),
    ] {
        match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.nlink() != 1
                {
                    return Err(AppError::new(
                        "Refusing a symlink, hardlink, or nonregular workspace index file.",
                    ));
                }
                if metadata.len() > MAX_SQLITE_DATABASE_BYTES {
                    return Err(AppError::new(
                        "Workspace index exceeds the supported defensive size limit.",
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !required => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(AppError::new("This workspace does not have an index yet."));
            }
            Err(_) => return Err(AppError::new("Workspace index is unavailable.")),
        }
    }
    Ok(())
}

fn verify_read_query_schema(conn: &Connection) -> AppResult<()> {
    let version = read_schema_version(conn)?;
    if version > SQLITE_SCHEMA_VERSION_CURRENT {
        return Err(AppError::new(format!(
            "Workspace index schema version {version} is newer than this version of PST QuickView supports."
        )));
    }
    if version < SQLITE_SCHEMA_VERSION_CURRENT {
        return Err(AppError::new(format!(
            "Workspace index schema version {version} requires upgrade before it can be searched. Reopen the workspace to run the normal upgrade."
        )));
    }

    verify_no_schema_programs(conn)?;
    for table in ["import_metadata", "folders", "messages", "attachments"] {
        verify_schema_object_type(conn, table, "table")?;
    }

    for (table, columns) in [
        ("import_metadata", &["key", "value"] as &[&str]),
        ("folders", &["id", "parent_id", "path", "name"]),
        (
            "messages",
            &[
                "id",
                "folder_id",
                "eml_path",
                "subject",
                "sender",
                "recipients",
                "date",
                "body",
                "body_source",
                "body_html",
                "snippet",
                "attachment_names",
                "has_attachments",
                "message_id_header_raw",
                "message_id_header",
                "in_reply_to_raw",
                "in_reply_to",
                "references_header_raw",
                "references_header",
                "normalized_subject",
                "conversation_id",
                "conversation_parent_id",
                "conversation_root_id",
                "thread_assignment_method",
                "thread_warning",
            ],
        ),
        (
            "attachments",
            &[
                "id",
                "message_id",
                "filename",
                "sanitized_filename",
                "content_type",
                "size_bytes",
                "attachment_index",
                "content_disposition",
                "mime_part_path",
            ],
        ),
        (
            "messages_fts",
            &[
                "subject",
                "sender",
                "recipients",
                "body",
                "attachment_names",
            ],
        ),
    ] {
        verify_required_columns(conn, table, columns).map_err(|error| {
            AppError::new(format!(
                "Workspace index schema is incompatible for read queries: {error}"
            ))
        })?;
    }

    let fts_sql = conn
        .query_row(
            "SELECT sql FROM sqlite_master WHERE type = 'table' AND name = 'messages_fts'",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if !fts_sql
        .as_deref()
        .is_some_and(|sql| sql.to_ascii_lowercase().contains("using fts5"))
    {
        return Err(AppError::new(
            "Workspace index schema is incompatible for read queries: messages_fts is missing or is not an FTS5 table.",
        ));
    }
    Ok(())
}

fn verify_no_schema_programs(conn: &Connection) -> AppResult<()> {
    let count = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type IN ('trigger', 'view')",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if count != 0 {
        return Err(AppError::new(
            "Workspace index contains unsupported executable schema objects.",
        ));
    }
    Ok(())
}

fn verify_schema_object_type(conn: &Connection, name: &str, expected: &str) -> AppResult<()> {
    let actual = conn
        .query_row(
            "SELECT type FROM sqlite_master WHERE name = ?1",
            params![name],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    if actual.as_deref() != Some(expected) {
        return Err(AppError::new(
            "Workspace index contains an invalid required schema object.",
        ));
    }
    Ok(())
}

fn metadata_value(conn: &Connection, key: &str) -> AppResult<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT value FROM import_metadata WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()?)
}

fn conversation_data_is_indexed(conn: &Connection) -> AppResult<bool> {
    Ok(
        metadata_value(conn, "conversation_schema_version")?.as_deref()
            == Some(CONVERSATION_SCHEMA_VERSION),
    )
}

fn set_metadata_value(conn: &Connection, key: &str, value: impl ToString) -> AppResult<()> {
    conn.execute(
        "INSERT OR REPLACE INTO import_metadata (key, value) VALUES (?1, ?2)",
        params![key, value.to_string()],
    )?;
    Ok(())
}

fn workspace_metadata_status(workspace: &Path) -> Option<String> {
    workspace_metadata_status_strict(workspace).ok().flatten()
}

fn read_workspace_metadata_json_strict(workspace: &Path) -> AppResult<Option<serde_json::Value>> {
    let workspace_metadata = fs::symlink_metadata(workspace)
        .map_err(|_| AppError::new("Workspace metadata is unavailable."))?;
    if workspace_metadata.file_type().is_symlink() || !workspace_metadata.is_dir() {
        return Err(AppError::new("Workspace metadata parent is invalid."));
    }
    let metadata_path = workspace.join(WORKSPACE_METADATA_FILE);
    let path_metadata = match fs::symlink_metadata(&metadata_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(AppError::new("Workspace metadata is unavailable.")),
    };
    if path_metadata.file_type().is_symlink()
        || !path_metadata.is_file()
        || path_metadata.nlink() != 1
        || path_metadata.len() > MAX_WORKSPACE_METADATA_BYTES
    {
        return Err(AppError::new("Workspace metadata is invalid."));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(&metadata_path)
        .map_err(|_| AppError::new("Workspace metadata could not be read."))?;
    let opened = file
        .metadata()
        .map_err(|_| AppError::new("Workspace metadata could not be inspected."))?;
    if !opened.is_file()
        || opened.nlink() != 1
        || opened.len() > MAX_WORKSPACE_METADATA_BYTES
        || opened.dev() != path_metadata.dev()
        || opened.ino() != path_metadata.ino()
    {
        return Err(AppError::new(
            "Workspace metadata changed while it was opened.",
        ));
    }
    let mut bytes = Vec::with_capacity((opened.len() as usize).min(64 * 1024));
    Read::by_ref(&mut file)
        .take(MAX_WORKSPACE_METADATA_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|_| AppError::new("Workspace metadata could not be read."))?;
    if bytes.len() as u64 > MAX_WORKSPACE_METADATA_BYTES {
        return Err(AppError::new("Workspace metadata is too large."));
    }
    let after = file
        .metadata()
        .map_err(|_| AppError::new("Workspace metadata could not be inspected."))?;
    if after.dev() != opened.dev()
        || after.ino() != opened.ino()
        || after.nlink() != 1
        || after.len() != opened.len()
        || after.mtime() != opened.mtime()
        || after.mtime_nsec() != opened.mtime_nsec()
    {
        return Err(AppError::new(
            "Workspace metadata changed while it was read.",
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|_| AppError::new("Workspace metadata is malformed."))?;
    if !value.is_object() {
        return Err(AppError::new("Workspace metadata is malformed."));
    }
    Ok(Some(value))
}

fn workspace_metadata_string_from_value(
    metadata: Option<&serde_json::Value>,
    snake_key: &str,
    camel_key: &str,
) -> AppResult<Option<String>> {
    let Some(metadata) = metadata else {
        return Ok(None);
    };
    let snake = metadata.get(snake_key);
    let camel = metadata.get(camel_key);
    if snake.is_some() && camel.is_some() && snake != camel {
        return Err(AppError::new(
            "Workspace metadata contains contradictory fields.",
        ));
    }
    let Some(value) = snake.or(camel) else {
        return Ok(None);
    };
    let value = value
        .as_str()
        .ok_or_else(|| AppError::new("Workspace metadata contains an invalid string field."))?;
    if value.len() > 64 * 1024 || value.chars().any(char::is_control) {
        return Err(AppError::new("Workspace metadata string is invalid."));
    }
    Ok(Some(value.to_string()))
}

fn workspace_metadata_string_strict(
    workspace: &Path,
    snake_key: &str,
    camel_key: &str,
) -> AppResult<Option<String>> {
    let metadata = read_workspace_metadata_json_strict(workspace)?;
    workspace_metadata_string_from_value(metadata.as_ref(), snake_key, camel_key)
}

fn workspace_metadata_status_strict(workspace: &Path) -> AppResult<Option<String>> {
    let status = workspace_metadata_string_strict(workspace, "import_status", "importStatus")?;
    if status
        .as_deref()
        .is_some_and(|value| !import_status_is_supported(value))
    {
        return Err(AppError::new(
            "Workspace metadata import status is unsupported.",
        ));
    }
    Ok(status)
}

fn import_status_is_supported(value: &str) -> bool {
    matches!(
        value,
        "pending" | "running" | "verifying" | "complete" | "failed" | "cancelled" | "incomplete"
    )
}

fn database_import_status_strict_at_path(
    index_path: &Path,
    require_index: bool,
    allow_unreadable_index: bool,
) -> AppResult<Option<String>> {
    match fs::symlink_metadata(index_path) {
        Ok(_) => validate_sqlite_file_set(index_path, true)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if sqlite_sidecar_exists(index_path) {
                return Err(AppError::new(
                    "Workspace index sidecars exist without their database.",
                ));
            }
            if require_index {
                return Err(AppError::new("This workspace does not have an index yet."));
            }
            return Ok(None);
        }
        Err(_) => return Err(AppError::new("Workspace index is unavailable.")),
    }

    let conn = match open_sqlite_read_only_connection(index_path) {
        Ok(conn) => conn,
        Err(_) if allow_unreadable_index => return Ok(None),
        Err(_) => return Err(AppError::new("Workspace index could not be read safely.")),
    };
    if configure_sqlite_connection(&conn, true).is_err() {
        if allow_unreadable_index {
            return Ok(None);
        }
        return Err(AppError::new("Workspace index could not be read safely."));
    }
    let schema_version = match read_schema_version(&conn) {
        Ok(version) => version,
        Err(_) if allow_unreadable_index => return Ok(None),
        Err(_) => return Err(AppError::new("Workspace index schema is unreadable.")),
    };
    if schema_version > SQLITE_SCHEMA_VERSION_CURRENT {
        return Err(AppError::new(
            "Workspace index schema is newer than this version supports.",
        ));
    }
    let status = match metadata_value(&conn, "import_status") {
        Ok(status) => status,
        Err(_) if allow_unreadable_index => return Ok(None),
        Err(_) => {
            return Err(AppError::new(
                "Workspace database import status is unreadable.",
            ))
        }
    };
    if status
        .as_deref()
        .is_some_and(|value| !import_status_is_supported(value))
    {
        return Err(AppError::new(
            "Workspace database import status is unsupported.",
        ));
    }
    Ok(status)
}

fn preflight_integrity_compatibility_status(
    workspace: &Path,
    require_active_index: bool,
    allow_unreadable_active_index: bool,
) -> AppResult<()> {
    workspace_metadata_status_strict(workspace)?;
    database_import_status_strict_at_path(
        &workspace.join("index.sqlite"),
        require_active_index,
        allow_unreadable_active_index,
    )?;
    Ok(())
}

fn read_index_progress_counts(index_path: &Path) -> (Option<usize>, Option<usize>) {
    let Ok(conn) = open_sqlite_read_only_connection(index_path) else {
        return (None, None);
    };
    if configure_sqlite_connection(&conn, true).is_err() {
        return (None, None);
    }
    let message_count = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
        .ok();
    let error_count = metadata_value(&conn, "error_count")
        .ok()
        .flatten()
        .and_then(|value| value.parse::<usize>().ok());
    (message_count, error_count)
}

fn mark_workspace_status(
    workspace: &Path,
    identity: &PstIdentity,
    workspace_mode: WorkspaceLocationMode,
    status: ImportStatus,
    last_error: Option<&str>,
    message_count_indexed: Option<usize>,
    error_count: Option<usize>,
) -> AppResult<()> {
    mark_workspace_status_with_index_update(
        workspace,
        identity,
        workspace_mode,
        status,
        last_error,
        message_count_indexed,
        error_count,
        true,
        None,
        true,
    )
}

fn mark_workspace_status_file_only(
    workspace: &Path,
    identity: &PstIdentity,
    workspace_mode: WorkspaceLocationMode,
    status: ImportStatus,
    last_error: Option<&str>,
    message_count_indexed: Option<usize>,
    error_count: Option<usize>,
) -> AppResult<()> {
    mark_workspace_status_with_index_update(
        workspace,
        identity,
        workspace_mode,
        status,
        last_error,
        message_count_indexed,
        error_count,
        false,
        None,
        true,
    )
}

#[cfg(test)]
fn mark_workspace_complete_file_only(
    workspace: &Path,
    identity: &PstIdentity,
    workspace_mode: WorkspaceLocationMode,
    stats: &IndexStats,
) -> AppResult<()> {
    mark_workspace_status_with_index_update(
        workspace,
        identity,
        workspace_mode,
        ImportStatus::Complete,
        stats.last_error.as_deref(),
        Some(stats.indexed),
        Some(stats.error_count),
        false,
        Some(stats),
        false,
    )
}

fn mark_existing_workspace_complete_file_only(
    workspace: &Path,
    identity: &PstIdentity,
    workspace_mode: WorkspaceLocationMode,
    stats: &IndexStats,
) -> AppResult<()> {
    mark_workspace_status_with_index_update(
        workspace,
        identity,
        workspace_mode,
        ImportStatus::Complete,
        stats.last_error.as_deref(),
        Some(stats.indexed),
        Some(stats.error_count),
        false,
        Some(stats),
        true,
    )
}

#[derive(Clone, Debug)]
struct ExistingWorkspaceWriteGuard {
    directory_device: u64,
    directory_inode: u64,
    marker_state: Option<SqliteFileState>,
    marker: Option<WorkspaceMarker>,
}

fn capture_existing_workspace_write_guard(
    workspace: &Path,
    identity: &PstIdentity,
    workspace_mode: WorkspaceLocationMode,
    require_marker: bool,
) -> AppResult<ExistingWorkspaceWriteGuard> {
    let directory = fs::symlink_metadata(workspace)
        .map_err(|_| AppError::new("Workspace became unavailable before metadata update."))?;
    if directory.file_type().is_symlink() || !directory.is_dir() {
        return Err(AppError::new("Workspace metadata parent is invalid."));
    }
    let marker_path = workspace.join(WORKSPACE_MARKER_FILE);
    let (marker_state, marker) = match fs::symlink_metadata(&marker_path) {
        Ok(_) => {
            let marker_state = sqlite_file_state(&marker_path)?;
            let marker = read_workspace_marker(&marker_path)?;
            let workspace_id = workspace_id_from_path(workspace)?;
            if marker.app != WORKSPACE_MARKER_APP_NAME
                || marker.workspace_id != workspace_id
                || marker.pst_content_fingerprint != identity.content_fingerprint
                || marker.workspace_location_mode != workspace_mode.as_str()
            {
                return Err(AppError::new(
                    "Workspace marker changed before metadata update.",
                ));
            }
            (Some(marker_state), Some(marker))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && !require_marker => {
            (None, None)
        }
        Err(_) => {
            return Err(AppError::new(
                "Workspace marker is unavailable before metadata update.",
            ))
        }
    };
    Ok(ExistingWorkspaceWriteGuard {
        directory_device: directory.dev(),
        directory_inode: directory.ino(),
        marker_state,
        marker,
    })
}

fn require_existing_workspace_write_guard(
    workspace: &Path,
    guard: &ExistingWorkspaceWriteGuard,
) -> AppResult<()> {
    let directory = fs::symlink_metadata(workspace)
        .map_err(|_| AppError::new("Workspace became unavailable during metadata update."))?;
    if directory.file_type().is_symlink()
        || !directory.is_dir()
        || directory.dev() != guard.directory_device
        || directory.ino() != guard.directory_inode
    {
        return Err(AppError::new(
            "Workspace changed during metadata update; no replacement directory was created.",
        ));
    }
    if let (Some(marker_state), Some(marker)) = (&guard.marker_state, &guard.marker) {
        if sqlite_file_state(&workspace.join(WORKSPACE_MARKER_FILE))? != *marker_state
            || read_workspace_marker(&workspace.join(WORKSPACE_MARKER_FILE))? != *marker
        {
            return Err(AppError::new(
                "Workspace marker changed during metadata update.",
            ));
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn mark_workspace_status_with_index_update(
    workspace: &Path,
    identity: &PstIdentity,
    workspace_mode: WorkspaceLocationMode,
    status: ImportStatus,
    last_error: Option<&str>,
    message_count_indexed: Option<usize>,
    error_count: Option<usize>,
    update_index: bool,
    stats: Option<&IndexStats>,
    require_marker: bool,
) -> AppResult<()> {
    let workspace_guard = capture_existing_workspace_write_guard(
        workspace,
        identity,
        workspace_mode,
        require_marker,
    )?;
    let now = Utc::now().to_rfc3339();
    let metadata_path = workspace.join(WORKSPACE_METADATA_FILE);
    let existing_metadata = read_workspace_metadata_json_strict(workspace)?;
    let existing_string = |snake_key: &str, camel_key: &str| -> Option<String> {
        existing_metadata
            .as_ref()
            .and_then(|value| value.get(snake_key).or_else(|| value.get(camel_key)))
            .and_then(|value| value.as_str())
            .map(str::to_string)
    };
    let existing_usize = |snake_key: &str, camel_key: &str| -> Option<usize> {
        existing_metadata
            .as_ref()
            .and_then(|value| value.get(snake_key).or_else(|| value.get(camel_key)))
            .and_then(|value| value.as_u64())
            .map(|value| value as usize)
    };
    let created_at = existing_string("created_at", "createdAt").unwrap_or_else(|| now.clone());
    let started_at = if matches!(status, ImportStatus::Running) {
        existing_string("started_at", "startedAt").or_else(|| Some(now.clone()))
    } else {
        existing_string("started_at", "startedAt")
    };
    let finished_at = if matches!(
        status,
        ImportStatus::Complete | ImportStatus::Failed | ImportStatus::Cancelled
    ) {
        Some(now.clone())
    } else {
        existing_string("finished_at", "finishedAt")
    };
    let metadata_last_error = last_error.map(str::to_string).or_else(|| {
        if matches!(
            status,
            ImportStatus::Pending | ImportStatus::Running | ImportStatus::Complete
        ) {
            None
        } else {
            existing_string("last_error", "lastError")
        }
    });

    let metadata = WorkspaceMetadata {
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        original_pst_path: identity.display_path.clone(),
        pst_fingerprint: identity.content_fingerprint.clone(),
        pst_size: identity.size.to_string(),
        pst_modified_ns: identity.modified_ns.to_string(),
        pst_fingerprint_strategy: identity.fingerprint_strategy.clone(),
        workspace_path: workspace.display().to_string(),
        workspace_mode: workspace_mode.as_str().to_string(),
        import_status: status.as_str().to_string(),
        created_at,
        updated_at: now.clone(),
        started_at,
        finished_at,
        message_count_indexed: message_count_indexed
            .or_else(|| existing_usize("message_count_indexed", "messageCountIndexed"))
            .unwrap_or(0),
        error_count: error_count
            .or_else(|| existing_usize("error_count", "errorCount"))
            .unwrap_or(0),
        last_error: metadata_last_error,
        readpst_path: existing_string("readpst_path", "readpstPath"),
        readpst_version: existing_string("readpst_version", "readpstVersion"),
    };
    let mut metadata_json =
        serde_json::to_value(metadata).map_err(|error| AppError::new(error.to_string()))?;
    if let (Some(object), Some(stats)) = (metadata_json.as_object_mut(), stats) {
        insert_workspace_import_integrity_counters(object, stats);
        object.insert(
            "conversation_schema_version".to_string(),
            serde_json::Value::String(CONVERSATION_SCHEMA_VERSION.to_string()),
        );
    }
    require_existing_workspace_write_guard(workspace, &workspace_guard)?;
    write_workspace_metadata_atomically(&metadata_path, &metadata_json)?;
    require_existing_workspace_write_guard(workspace, &workspace_guard)?;

    let index_path = workspace.join("index.sqlite");
    if update_index && index_path.exists() {
        let conn = open_workspace_db_for_upgrade(workspace)?;
        set_metadata_value(&conn, "import_status", status.as_str())?;
        set_metadata_value(&conn, "updated_at", &now)?;
        if matches!(status, ImportStatus::Running) && metadata_value(&conn, "started_at")?.is_none()
        {
            set_metadata_value(&conn, "started_at", &now)?;
        }
        if matches!(
            status,
            ImportStatus::Complete | ImportStatus::Failed | ImportStatus::Cancelled
        ) {
            set_metadata_value(&conn, "finished_at", &now)?;
        }
        if let Some(count) = message_count_indexed {
            set_metadata_value(&conn, "message_count_indexed", count)?;
        }
        if let Some(count) = error_count {
            set_metadata_value(&conn, "error_count", count)?;
        }
        if let Some(error) = last_error {
            set_metadata_value(&conn, "last_error", error)?;
        }
    }

    Ok(())
}

fn set_workspace_readpst_metadata(
    workspace: &Path,
    readpst_path: &str,
    readpst_version: &str,
) -> AppResult<()> {
    let metadata_path = workspace.join(WORKSPACE_METADATA_FILE);
    let contents = fs::read_to_string(&metadata_path).unwrap_or_else(|_| "{}".to_string());
    let mut value = serde_json::from_str::<serde_json::Value>(&contents)
        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
    if !value.is_object() {
        value = serde_json::Value::Object(serde_json::Map::new());
    }

    if let Some(object) = value.as_object_mut() {
        object.insert(
            "readpst_path".to_string(),
            serde_json::Value::String(readpst_path.to_string()),
        );
        object.insert(
            "readpst_version".to_string(),
            serde_json::Value::String(readpst_version.to_string()),
        );
        object.insert(
            "updated_at".to_string(),
            serde_json::Value::String(Utc::now().to_rfc3339()),
        );
    }

    write_workspace_metadata_atomically(&metadata_path, &value)?;
    Ok(())
}

fn initialize_import_log(workspace: &Path, _readpst_version: &str) -> AppResult<()> {
    create_private_directory_all(&workspace.join("logs"))?;
    let log_path = import_log_path(workspace);
    rotate_existing_log(&log_path, LOG_BACKUP_COUNT)?;
    let mut log = create_private_file_new(&log_path)?;
    writeln!(log, "state=log_initialized")?;
    writeln!(log, "started_at={}", Utc::now().to_rfc3339())?;
    writeln!(log)?;
    Ok(())
}

fn append_import_log(workspace: &Path, message: &str) -> AppResult<()> {
    if !workspace.is_dir() {
        return Err(AppError::new("Workspace was deleted during import."));
    }
    let logs_dir = workspace.join("logs");
    if !logs_dir.is_dir() {
        create_private_directory_all(&logs_dir)?;
    }
    let log_path = import_log_path(workspace);
    rotate_log_if_needed(&log_path, IMPORT_LOG_MAX_BYTES, LOG_BACKUP_COUNT)?;
    let mut log = open_private_log_for_append(&log_path)?;
    writeln!(
        log,
        "[{}] {}",
        Utc::now().to_rfc3339(),
        safe_log_value(message)
    )?;
    Ok(())
}

fn append_export_log(workspace: &Path, message: &str) -> AppResult<()> {
    if !workspace.is_dir() {
        return Err(AppError::new("Workspace is unavailable."));
    }
    let logs_dir = workspace.join("logs");
    create_private_directory_all(&logs_dir)?;
    let log_path = logs_dir.join("exports.log");
    rotate_log_if_needed(&log_path, EXPORT_LOG_MAX_BYTES, LOG_BACKUP_COUNT)?;
    let mut log = open_private_log_for_append(&log_path)?;
    writeln!(
        log,
        "[{}] {}",
        Utc::now().to_rfc3339(),
        safe_log_value(message)
    )?;
    Ok(())
}

fn append_application_log(
    profile_paths: &RuntimeProfilePaths,
    operation: &str,
    stage: &str,
    _workspace_id: Option<&str>,
    error: Option<&str>,
) -> AppResult<()> {
    let logs_dir = application_logs_dir(profile_paths);
    create_private_directory_all(logs_dir)?;
    let log_path = application_log_path(profile_paths);
    rotate_log_if_needed(&log_path, APPLICATION_LOG_MAX_BYTES, LOG_BACKUP_COUNT)?;
    let mut log = open_private_log_for_append(&log_path)?;
    writeln!(
        log,
        "[{}] operation={} stage={} error={}",
        Utc::now().to_rfc3339(),
        safe_log_value(operation),
        safe_log_value(stage),
        if error.is_some() { "present" } else { "-" }
    )?;
    Ok(())
}

fn append_application_log_best_effort(
    state: &AppState,
    operation: &str,
    stage: &str,
    workspace_id: Option<&str>,
    error: Option<&str>,
) {
    let result = state.runtime_profile().and_then(|profile| {
        append_application_log(profile.paths(), operation, stage, workspace_id, error)
    });
    if result.is_err() {
        eprintln!("PST QuickView application log write failed.");
    }
}

fn safe_log_value(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if matches!(character, '\n' | '\r' | '\t') {
                ' '
            } else {
                character
            }
        })
        .take(1000)
        .collect()
}

fn open_private_log_for_append(path: &Path) -> AppResult<File> {
    if let Some(metadata) = validated_log_metadata(path)? {
        fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_FILE_MODE))?;
        if metadata.len() > IMPORT_LOG_MAX_BYTES.max(EXPORT_LOG_MAX_BYTES) {
            return Err(AppError::new("Log file exceeds the supported size limit."));
        }
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(PRIVATE_FILE_MODE)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.nlink() != 1 {
        return Err(AppError::new("Refusing a linked or nonregular log file."));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_FILE_MODE))?;
    Ok(file)
}

fn validated_log_metadata(path: &Path) -> AppResult<Option<fs::Metadata>> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.nlink() != 1 {
                return Err(AppError::new("Refusing a linked or nonregular log file."));
            }
            Ok(Some(metadata))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(AppError::new("Log file is unavailable.")),
    }
}

fn rotate_log_if_needed(path: &Path, max_bytes: u64, backups: usize) -> AppResult<()> {
    if validated_log_metadata(path)?.map_or(true, |metadata| metadata.len() < max_bytes) {
        return Ok(());
    }
    rotate_existing_log(path, backups)
}

fn rotate_existing_log(path: &Path, backups: usize) -> AppResult<()> {
    if backups == 0 || validated_log_metadata(path)?.is_none() {
        return Ok(());
    }
    let oldest = rotated_log_path(path, backups);
    if validated_log_metadata(&oldest)?.is_some() {
        fs::remove_file(oldest)?;
    }
    for index in (1..backups).rev() {
        let source = rotated_log_path(path, index);
        if validated_log_metadata(&source)?.is_some() {
            if validated_log_metadata(&rotated_log_path(path, index + 1))?.is_some() {
                return Err(AppError::new("Log rotation destination already exists."));
            }
            fs::rename(source, rotated_log_path(path, index + 1))?;
        }
    }
    fs::rename(path, rotated_log_path(path, 1))?;
    Ok(())
}

fn rotated_log_path(path: &Path, index: usize) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(format!(".{index}"));
    PathBuf::from(value)
}

fn import_log_path(workspace: &Path) -> PathBuf {
    workspace.join("logs").join(IMPORT_LOG_FILE)
}

#[derive(Debug, Default)]
struct BoundedProcessOutput {
    tail: Vec<u8>,
    total_bytes: u64,
}

impl BoundedProcessOutput {
    fn truncated(&self) -> bool {
        self.total_bytes > self.tail.len() as u64
    }
}

type ProcessOutputReader = thread::JoinHandle<std::io::Result<BoundedProcessOutput>>;

struct ProcessOutputReaders {
    stdout: ProcessOutputReader,
    stderr: ProcessOutputReader,
    stdout_bytes: Arc<AtomicU64>,
    stderr_bytes: Arc<AtomicU64>,
}

impl ProcessOutputReaders {
    fn observed_bytes(&self) -> u64 {
        self.stdout_bytes
            .load(Ordering::Acquire)
            .saturating_add(self.stderr_bytes.load(Ordering::Acquire))
    }
}

fn drain_bounded_process_output(
    mut reader: impl Read,
    max_bytes: usize,
    observed_bytes: &AtomicU64,
) -> std::io::Result<BoundedProcessOutput> {
    let mut tail = VecDeque::with_capacity(max_bytes.min(64 * 1024));
    let mut total_bytes = 0_u64;
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total_bytes = total_bytes.saturating_add(read as u64);
        observed_bytes.store(total_bytes, Ordering::Release);
        if max_bytes == 0 {
            continue;
        }
        if read >= max_bytes {
            tail.clear();
            tail.extend(&buffer[read - max_bytes..read]);
            continue;
        }
        let overflow = tail.len().saturating_add(read).saturating_sub(max_bytes);
        if overflow > 0 {
            tail.drain(..overflow);
        }
        tail.extend(&buffer[..read]);
    }
    Ok(BoundedProcessOutput {
        tail: tail.into_iter().collect(),
        total_bytes,
    })
}

fn configure_isolated_readpst_command(command: &mut Command) {
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
}

fn spawn_process_output_readers(child: &mut Child) -> AppResult<ProcessOutputReaders> {
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| AppError::new("ReadPST stdout pipe was not available."))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| AppError::new("ReadPST stderr pipe was not available."))?;
    let stdout_bytes = Arc::new(AtomicU64::new(0));
    let stderr_bytes = Arc::new(AtomicU64::new(0));
    let stdout_counter = Arc::clone(&stdout_bytes);
    let stderr_counter = Arc::clone(&stderr_bytes);
    Ok(ProcessOutputReaders {
        stdout: thread::spawn(move || {
            drain_bounded_process_output(stdout, READPST_CAPTURE_MAX_BYTES, &stdout_counter)
        }),
        stderr: thread::spawn(move || {
            drain_bounded_process_output(stderr, READPST_CAPTURE_MAX_BYTES, &stderr_counter)
        }),
        stdout_bytes,
        stderr_bytes,
    })
}

fn join_process_output_reader(reader: ProcessOutputReader) -> AppResult<BoundedProcessOutput> {
    reader
        .join()
        .map_err(|_| AppError::new("ReadPST output reader stopped unexpectedly."))?
        .map_err(AppError::from)
}

fn run_isolated_bounded_command(
    mut command: Command,
    timeout: Duration,
) -> AppResult<(ExitStatus, BoundedProcessOutput, BoundedProcessOutput)> {
    let mut child = command.spawn()?;
    let pid = child.id();
    let output_readers = match spawn_process_output_readers(&mut child) {
        Ok(readers) => readers,
        Err(error) => {
            let _ = terminate_and_wait_for_process_group(&mut child, pid);
            return Err(error);
        }
    };
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None)
                if Instant::now() < deadline
                    && output_readers.observed_bytes() <= MAX_READPST_STREAM_BYTES =>
            {
                thread::sleep(READPST_POLL_INTERVAL)
            }
            Ok(None) if output_readers.observed_bytes() > MAX_READPST_STREAM_BYTES => {
                let _ = terminate_and_wait_for_process_group(&mut child, pid);
                break Err(AppError::new(
                    "ReadPST helper output exceeded the safety limit.",
                ));
            }
            Ok(None) => {
                let _ = terminate_and_wait_for_process_group(&mut child, pid);
                break Err(AppError::new("ReadPST helper timed out."));
            }
            Err(error) => {
                let _ = terminate_and_wait_for_process_group(&mut child, pid);
                break Err(AppError::from(error));
            }
        }
    };
    let stdout = join_process_output_reader(output_readers.stdout)?;
    let stderr = join_process_output_reader(output_readers.stderr)?;
    Ok((status?, stdout, stderr))
}

fn readpst_version(readpst: &Path) -> String {
    for arg in ["-V", "--version"] {
        let mut command = Command::new(readpst);
        command.arg(arg);
        configure_isolated_readpst_command(&mut command);
        if let Ok((_status, stdout, stderr)) =
            run_isolated_bounded_command(command, READPST_VERSION_TIMEOUT)
        {
            let combined = [stdout.tail, stderr.tail].concat();
            let text = String::from_utf8_lossy(&combined).trim().to_string();
            if !text.is_empty() {
                return text.lines().next().unwrap_or("unknown").to_string();
            }
        }
    }
    "unknown".to_string()
}

fn terminate_process_group(pid: u32, force: bool) -> AppResult<()> {
    let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
    let process_group = -(pid as libc::pid_t);
    let result = unsafe { libc::kill(process_group, signal) };
    if result == 0 {
        return Ok(());
    }

    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }

    Err(AppError::new(format!(
        "Could not stop the ReadPST process group with signal {signal}: {error}"
    )))
}

fn terminate_and_wait_for_process_group(child: &mut Child, pid: u32) -> AppResult<ExitStatus> {
    let _ = terminate_process_group(pid, false);
    let deadline = Instant::now() + READPST_TERMINATE_GRACE;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if Instant::now() >= deadline {
            break;
        }
        thread::sleep(READPST_POLL_INTERVAL);
    }
    let _ = terminate_process_group(pid, true);
    child.wait().map_err(AppError::from)
}

fn find_existing_workspaces(
    profile_paths: &RuntimeProfilePaths,
    identity: &PstIdentity,
) -> AppResult<Vec<WorkspaceCandidate>> {
    let mut candidates = Vec::new();
    let app_support_root = workspaces_dir(profile_paths)?;
    collect_existing_workspaces_in_root(
        identity,
        &app_support_root,
        WorkspaceLocationMode::AppSupport,
        &mut candidates,
    )?;

    for next_to_pst_root in next_to_pst_candidate_roots(identity) {
        collect_existing_workspaces_in_root(
            identity,
            &next_to_pst_root,
            WorkspaceLocationMode::NextToPst,
            &mut candidates,
        )?;
    }

    dedupe_candidates(candidates)
}

fn collect_existing_workspaces_in_root(
    identity: &PstIdentity,
    root: &Path,
    mode: WorkspaceLocationMode,
    candidates: &mut Vec<WorkspaceCandidate>,
) -> AppResult<()> {
    match fs::symlink_metadata(root) {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
        Ok(_) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(AppError::new("Workspace cache root is unavailable.")),
    }

    for workspace_id in [&identity.workspace_id, &identity.legacy_workspace_id] {
        let workspace = root.join(workspace_id);
        if !fs::symlink_metadata(&workspace)
            .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_dir())
        {
            continue;
        }
        if workspace_matches_identity(&workspace, identity, mode)? {
            candidates.push(WorkspaceCandidate {
                workspace_id: workspace_id_from_path(&workspace)?,
                path: workspace,
                mode,
            });
        }
    }

    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let workspace = entry.path();
        if !fs::symlink_metadata(&workspace)
            .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_dir())
        {
            continue;
        }

        if workspace_matches_identity(&workspace, identity, mode)? {
            candidates.push(WorkspaceCandidate {
                workspace_id: workspace_id_from_path(&workspace)?,
                path: workspace,
                mode,
            });
        }
    }

    Ok(())
}

fn workspace_matches_identity(
    workspace: &Path,
    identity: &PstIdentity,
    mode: WorkspaceLocationMode,
) -> AppResult<bool> {
    let Ok(metadata) = fs::symlink_metadata(workspace) else {
        return Ok(false);
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Ok(false);
    }

    let workspace_id = workspace_id_from_path(workspace)?;
    if workspace_id != identity.workspace_id && workspace_id != identity.legacy_workspace_id {
        return Ok(false);
    }
    let marker_path = workspace.join(WORKSPACE_MARKER_FILE);
    let Ok(marker) = read_workspace_marker(&marker_path) else {
        return Ok(false);
    };
    Ok(marker.app == WORKSPACE_MARKER_APP_NAME
        && marker.workspace_id == workspace_id
        && marker.pst_content_fingerprint == identity.content_fingerprint
        && marker.workspace_location_mode == mode.as_str())
}

fn sort_existing_workspace_summaries(
    workspaces: &mut [ExistingWorkspace],
    selected_workspace_path: &Path,
) {
    workspaces.sort_by_key(|workspace| {
        let is_selected = paths_equivalent(
            Path::new(&workspace.workspace_path),
            selected_workspace_path,
        );
        (
            !is_selected,
            !workspace.is_complete,
            workspace.workspace_location_label.clone(),
            workspace.workspace_path.clone(),
        )
    });
}

fn dedupe_candidates(candidates: Vec<WorkspaceCandidate>) -> AppResult<Vec<WorkspaceCandidate>> {
    let mut deduped = Vec::new();
    let mut seen = Vec::<PathBuf>::new();

    for candidate in candidates {
        let canonical = candidate
            .path
            .canonicalize()
            .unwrap_or(candidate.path.clone());
        if seen.iter().any(|path| path == &canonical) {
            continue;
        }
        seen.push(canonical);
        deduped.push(candidate);
    }

    Ok(deduped)
}

fn existing_workspace_summary(candidate: &WorkspaceCandidate) -> AppResult<ExistingWorkspace> {
    let index_path = candidate.path.join("index.sqlite");
    let (message_count, import_status) = if index_path.exists() {
        match open_sqlite_read_only_connection(&index_path).and_then(|conn| {
            configure_sqlite_connection(&conn, true)?;
            verify_no_schema_programs(&conn)?;
            verify_schema_object_type(&conn, "messages", "table")?;
            verify_schema_object_type(&conn, "import_metadata", "table")?;
            verify_required_columns(&conn, "messages", &["id"])?;
            verify_required_columns(&conn, "import_metadata", &["key", "value"])?;
            let message_count = conn.query_row("SELECT COUNT(*) FROM messages", [], |row| {
                row.get::<_, i64>(0)
            })?;
            let status = metadata_value(&conn, "import_status")?.unwrap_or_else(|| {
                workspace_metadata_status(&candidate.path).unwrap_or_else(|| "pending".to_string())
            });
            Ok((message_count, status))
        }) {
            Ok(summary) => summary,
            Err(_) => (
                0,
                workspace_metadata_status(&candidate.path)
                    .unwrap_or_else(|| "incomplete".to_string()),
            ),
        }
    } else {
        (
            0,
            workspace_metadata_status(&candidate.path).unwrap_or_else(|| "pending".to_string()),
        )
    };

    let import_status = match import_status.as_str() {
        "pending" | "running" | "verifying" | "complete" | "failed" | "cancelled"
        | "incomplete" => import_status,
        _ => "unsupported".to_string(),
    };

    let marker_source = read_workspace_marker(&candidate.path.join(WORKSPACE_MARKER_FILE))
        .ok()
        .map(|marker| PathBuf::from(marker.original_pst_path));
    let integrity = import_integrity_phase2::integrity_snapshot(
        &candidate.path,
        &candidate.workspace_id,
        marker_source.as_deref(),
    );
    Ok(ExistingWorkspace {
        workspace_id: candidate.workspace_id.clone(),
        workspace_path: candidate.path.display().to_string(),
        workspace_location_mode: candidate.mode.as_str().to_string(),
        workspace_location_label: candidate.mode.label().to_string(),
        is_complete: integrity.classification == "complete",
        import_status,
        can_resume: integrity.can_resume_indexing,
        can_reimport: candidate.path.is_dir(),
        message_count,
        can_open: integrity.can_open,
        integrity_state: integrity.state,
        integrity_classification: integrity.classification.to_string(),
        can_repair_index: integrity.can_repair_index,
        can_rebuild_conversations: integrity.can_rebuild_conversations,
        resume_details: integrity.resume_details,
        has_last_verification: integrity.has_last_verification,
    })
}

fn select_existing_workspace(
    candidates: &[WorkspaceCandidate],
    requested_path: &Path,
) -> AppResult<WorkspaceCandidate> {
    let requested = requested_path
        .canonicalize()
        .unwrap_or_else(|_| requested_path.to_path_buf());

    candidates
        .iter()
        .find(|candidate| {
            candidate
                .path
                .canonicalize()
                .unwrap_or(candidate.path.clone())
                == requested
        })
        .cloned()
        .ok_or_else(|| AppError::new("Selected existing workspace was not found for this PST."))
}

fn workspace_id_from_path(path: &Path) -> AppResult<String> {
    let id = path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| AppError::new("Workspace path does not have a valid id."))?;

    if id.len() != 64 || !id.chars().all(|char| char.is_ascii_hexdigit()) {
        return Err(AppError::new("Workspace path does not have a valid id."));
    }

    Ok(id.to_string())
}

fn upsert_current_fingerprint_metadata(
    conn: &Connection,
    identity: &PstIdentity,
    workspace_id: &str,
    workspace: &Path,
    workspace_mode: WorkspaceLocationMode,
) -> AppResult<()> {
    let values = [
        ("pst_last_opened_path", identity.display_path.clone()),
        ("pst_size", identity.size.to_string()),
        ("pst_modified_ns", identity.modified_ns.to_string()),
        ("pst_fingerprint", identity.fingerprint.clone()),
        (
            "pst_content_fingerprint",
            identity.content_fingerprint.clone(),
        ),
        (
            "pst_fingerprint_strategy",
            identity.fingerprint_strategy.clone(),
        ),
        ("workspace_path", workspace.display().to_string()),
        (
            "workspace_location_mode",
            workspace_mode.as_str().to_string(),
        ),
        ("workspace_id", workspace_id.to_string()),
        ("last_opened_at", Utc::now().to_rfc3339()),
    ];

    for (key, value) in values {
        conn.execute(
            "INSERT OR REPLACE INTO import_metadata (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
    }

    Ok(())
}

fn workspace_summary(
    identity: &PstIdentity,
    workspace_id: &str,
    workspace: &Path,
    workspace_mode: WorkspaceLocationMode,
    reused_existing: bool,
) -> AppResult<WorkspaceSummary> {
    let conn = open_workspace_db_for_read(workspace)?;
    let message_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;
    let folder_count: i64 = conn.query_row("SELECT COUNT(*) FROM folders", [], |row| row.get(0))?;

    let integrity = import_integrity_phase2::integrity_snapshot(
        workspace,
        workspace_id,
        Some(&identity.canonical_path),
    );
    Ok(WorkspaceSummary {
        id: workspace_id.to_string(),
        pst_path: identity.display_path.clone(),
        workspace_path: workspace.display().to_string(),
        eml_dir: workspace.join("extracted").display().to_string(),
        index_path: workspace.join("index.sqlite").display().to_string(),
        message_count,
        folder_count,
        reused_existing,
        fingerprint: identity.content_fingerprint.clone(),
        fingerprint_strategy: identity.fingerprint_strategy.clone(),
        workspace_location_mode: workspace_mode.as_str().to_string(),
        workspace_location_label: workspace_mode.label().to_string(),
        pst_size_bytes: identity.size,
        integrity_state: integrity.state,
        integrity_classification: integrity.classification.to_string(),
        source_availability: integrity.source_availability.to_string(),
        can_resume_indexing: integrity.can_resume_indexing,
        can_repair_index: integrity.can_repair_index,
        can_rebuild_conversations: integrity.can_rebuild_conversations,
        resume_details: integrity.resume_details,
        has_last_verification: integrity.has_last_verification,
        conversation_state: integrity.conversation_state,
        completed_import_duration_ms: integrity.completed_duration_ms,
        last_activity_at: integrity.last_activity_at,
        active_import_elapsed_ms: integrity.active_elapsed_ms,
        current_stage_elapsed_ms: integrity.stage_elapsed_ms,
    })
}

fn workspace_summary_from_active(
    active: &ActiveWorkspace,
    reused_existing: bool,
) -> AppResult<WorkspaceSummary> {
    let conn = open_workspace_db_for_read(&active.path)?;
    let message_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;
    let folder_count: i64 = conn.query_row("SELECT COUNT(*) FROM folders", [], |row| row.get(0))?;
    let fingerprint_strategy = metadata_value(&conn, "pst_fingerprint_strategy")?
        .unwrap_or_else(|| "existing workspace".to_string());
    let pst_size_bytes = metadata_value(&conn, "pst_size")?
        .and_then(|value| value.parse::<u64>().ok())
        .or_else(|| {
            fs::metadata(&active.pst_path)
                .ok()
                .map(|metadata| metadata.len())
        })
        .unwrap_or(0);

    let integrity = import_integrity_phase2::integrity_snapshot(
        &active.path,
        &active.id,
        Some(&active.pst_path),
    );
    Ok(WorkspaceSummary {
        id: active.id.clone(),
        pst_path: active.pst_path.display().to_string(),
        workspace_path: active.path.display().to_string(),
        eml_dir: active.path.join("extracted").display().to_string(),
        index_path: active.path.join("index.sqlite").display().to_string(),
        message_count,
        folder_count,
        reused_existing,
        fingerprint: active.fingerprint.clone(),
        fingerprint_strategy,
        workspace_location_mode: active.location_mode.as_str().to_string(),
        workspace_location_label: active.location_mode.label().to_string(),
        pst_size_bytes,
        integrity_state: integrity.state,
        integrity_classification: integrity.classification.to_string(),
        source_availability: integrity.source_availability.to_string(),
        can_resume_indexing: integrity.can_resume_indexing,
        can_repair_index: integrity.can_repair_index,
        can_rebuild_conversations: integrity.can_rebuild_conversations,
        resume_details: integrity.resume_details,
        has_last_verification: integrity.has_last_verification,
        conversation_state: integrity.conversation_state,
        completed_import_duration_ms: integrity.completed_duration_ms,
        last_activity_at: integrity.last_activity_at,
        active_import_elapsed_ms: integrity.active_elapsed_ms,
        current_stage_elapsed_ms: integrity.stage_elapsed_ms,
    })
}

fn update_workspace_metadata_after_reindex(
    active: &ActiveWorkspace,
    stats: &IndexStats,
) -> AppResult<()> {
    let metadata_path = active.path.join(WORKSPACE_METADATA_FILE);
    let contents = fs::read_to_string(&metadata_path).unwrap_or_else(|_| "{}".to_string());
    let mut value = serde_json::from_str::<serde_json::Value>(&contents)
        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
    if !value.is_object() {
        value = serde_json::Value::Object(serde_json::Map::new());
    }

    let now = Utc::now().to_rfc3339();
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "app_version".to_string(),
            serde_json::Value::String(env!("CARGO_PKG_VERSION").to_string()),
        );
        object.insert(
            "original_pst_path".to_string(),
            serde_json::Value::String(active.pst_path.display().to_string()),
        );
        object.insert(
            "pst_fingerprint".to_string(),
            serde_json::Value::String(active.fingerprint.clone()),
        );
        object.insert(
            "workspace_path".to_string(),
            serde_json::Value::String(active.path.display().to_string()),
        );
        object.insert(
            "workspace_mode".to_string(),
            serde_json::Value::String(active.location_mode.as_str().to_string()),
        );
        object.insert(
            "import_status".to_string(),
            serde_json::Value::String(ImportStatus::Complete.as_str().to_string()),
        );
        object.insert(
            "updated_at".to_string(),
            serde_json::Value::String(now.clone()),
        );
        object.insert("finished_at".to_string(), serde_json::Value::String(now));
        object.insert(
            "message_count_indexed".to_string(),
            serde_json::Value::from(stats.indexed),
        );
        object.insert(
            "error_count".to_string(),
            serde_json::Value::from(stats.error_count),
        );
        object.insert(
            "last_error".to_string(),
            stats
                .last_error
                .clone()
                .map(serde_json::Value::String)
                .unwrap_or(serde_json::Value::Null),
        );
        object.insert(
            "reindex_source".to_string(),
            serde_json::Value::String("existing_emls".to_string()),
        );
        object.insert(
            "body_html_schema_version".to_string(),
            serde_json::Value::String("body-html-v1".to_string()),
        );
        object.insert(
            "conversation_schema_version".to_string(),
            serde_json::Value::String(CONVERSATION_SCHEMA_VERSION.to_string()),
        );
        insert_workspace_import_integrity_counters(object, stats);
    }

    write_workspace_metadata_atomically(&metadata_path, &value)?;
    Ok(())
}

fn insert_workspace_import_integrity_counters(
    object: &mut serde_json::Map<String, serde_json::Value>,
    stats: &IndexStats,
) {
    for (key, value) in [
        ("candidates_discovered", stats.discovered),
        ("candidates_processed", stats.processed),
        ("canonical_messages_inserted", stats.inserted),
        (
            "existing_canonical_messages_skipped",
            stats.skipped_existing,
        ),
        (
            "structurally_empty_artifacts_suppressed",
            stats.suppressed_empty_artifacts,
        ),
        ("parse_failures", stats.error_count),
        ("attachments_indexed", stats.attachments_indexed),
        ("canonical_messages_total", stats.indexed),
    ] {
        object.insert(key.to_string(), serde_json::Value::from(value));
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct ReadpstOutputSnapshot {
    entries: u64,
    files: u64,
    total_bytes: u64,
    activity_hash: u64,
}

fn emit_readpst_progress(
    app: &tauri::AppHandle,
    snapshot: Option<&ReadpstOutputSnapshot>,
    activity_observed: bool,
    process_state: import_progress::ProcessState,
) {
    emit_progress_observation(
        app,
        ProgressObservation {
            processed_units: snapshot.map(|value| value.files),
            total_units: None,
            unit_kind: snapshot.map(|_| import_progress::ProgressUnitKind::Files),
            activity_observed,
            process_state: Some(process_state),
            counters: Some(import_progress::ProgressCounters {
                output_files_observed: snapshot.map(|value| value.files),
                output_bytes_observed: snapshot.map(|value| value.total_bytes),
                workspace_bytes_written: None,
                ..import_progress::ProgressCounters::default()
            }),
            ..ProgressObservation::default()
        },
    );
}

struct ActiveReadpstProcess<'a> {
    state: &'a AppState,
    pid: u32,
}

impl Drop for ActiveReadpstProcess<'_> {
    fn drop(&mut self) {
        if let Ok(mut active_pid) = self.state.readpst_pid.lock() {
            if *active_pid == Some(self.pid) {
                *active_pid = None;
            }
        }
    }
}

fn register_readpst_process(state: &AppState, pid: u32) -> AppResult<ActiveReadpstProcess<'_>> {
    let mut active_pid = state
        .readpst_pid
        .lock()
        .map_err(|_| AppError::new("Could not lock ReadPST process state."))?;
    if active_pid.is_some() {
        return Err(AppError::new(
            "Another ReadPST process is already registered.",
        ));
    }
    *active_pid = Some(pid);
    drop(active_pid);
    Ok(ActiveReadpstProcess { state, pid })
}

fn readpst_output_byte_limit(source_bytes: u64) -> u64 {
    source_bytes
        .saturating_mul(READPST_OUTPUT_SIZE_RATIO)
        .saturating_add(READPST_OUTPUT_OVERHEAD_BYTES)
}

const READPST_VALIDATION_PROGRESS_BATCH_SIZE: u64 = 1_024;

fn inspect_readpst_output_with_progress<F>(
    root: &Path,
    max_total_bytes: u64,
    mut on_progress: F,
) -> AppResult<ReadpstOutputSnapshot>
where
    F: FnMut() -> AppResult<()>,
{
    let root_metadata = fs::symlink_metadata(root)
        .map_err(|_| AppError::new("ReadPST output directory is unavailable."))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(AppError::new(
            "ReadPST output root was replaced by an unsafe file type.",
        ));
    }

    let mut snapshot = ReadpstOutputSnapshot::default();
    for entry in WalkDir::new(root).follow_links(false).max_open(32) {
        let entry = entry.map_err(|_| AppError::new("Could not inspect ReadPST output safely."))?;
        if entry.depth() == 0 {
            continue;
        }
        snapshot.entries = snapshot.entries.saturating_add(1);
        if snapshot.entries % READPST_VALIDATION_PROGRESS_BATCH_SIZE == 0 {
            on_progress()?;
        }
        if snapshot.entries > MAX_READPST_EXTRACTED_ENTRIES {
            return Err(AppError::new(
                "ReadPST output exceeded the extracted-entry safety limit.",
            ));
        }
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| AppError::new("Could not inspect a ReadPST output entry safely."))?;
        let file_type = metadata.file_type();
        if file_type.is_symlink() {
            return Err(AppError::new(
                "ReadPST produced a symlink; import was refused.",
            ));
        }
        if metadata.is_dir() {
            snapshot.activity_hash = snapshot
                .activity_hash
                .rotate_left(7)
                .wrapping_add(metadata.ino())
                .wrapping_add(metadata.mtime() as u64)
                .wrapping_add(metadata.mtime_nsec() as u64);
            continue;
        }
        if !metadata.is_file() || metadata.nlink() != 1 {
            return Err(AppError::new(
                "ReadPST produced a nonregular or hardlinked output; import was refused.",
            ));
        }
        snapshot.files = snapshot.files.saturating_add(1);
        if metadata.len() > MAX_READPST_EXTRACTED_FILE_BYTES {
            return Err(AppError::new(
                "A ReadPST output file exceeded the per-file safety limit.",
            ));
        }
        snapshot.total_bytes = snapshot.total_bytes.saturating_add(metadata.len());
        if snapshot.total_bytes > max_total_bytes {
            return Err(AppError::new(
                "ReadPST output exceeded the total extraction safety limit.",
            ));
        }
        snapshot.activity_hash = snapshot
            .activity_hash
            .rotate_left(7)
            .wrapping_add(metadata.ino())
            .wrapping_add(metadata.len())
            .wrapping_add(metadata.mtime() as u64)
            .wrapping_add(metadata.mtime_nsec() as u64);
    }
    Ok(snapshot)
}

#[cfg(test)]
fn inspect_readpst_output(root: &Path, max_total_bytes: u64) -> AppResult<ReadpstOutputSnapshot> {
    inspect_readpst_output_with_progress(root, max_total_bytes, || Ok(()))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ReadpstRootActivity {
    inode: u64,
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
}

fn inspect_readpst_root_activity(root: &Path) -> AppResult<ReadpstRootActivity> {
    let metadata = fs::symlink_metadata(root)
        .map_err(|_| AppError::new("ReadPST output directory is unavailable."))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AppError::new(
            "ReadPST output root was replaced by an unsafe file type.",
        ));
    }
    Ok(ReadpstRootActivity {
        inode: metadata.ino(),
        size: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
    })
}

fn validate_readpst_cleanup_root(workspace: &Path, path: &Path) -> AppResult<()> {
    let allowed_name = matches!(
        path.file_name().and_then(OsStr::to_str),
        Some("extracting" | "extracted" | "readpst-tmp")
    );
    if !allowed_name || path.parent() != Some(workspace) {
        return Err(AppError::new(
            "Refusing to remove an unexpected ReadPST output path.",
        ));
    }
    let workspace_metadata = fs::symlink_metadata(workspace)
        .map_err(|_| AppError::new("Workspace is unavailable during ReadPST cleanup."))?;
    let root_metadata = fs::symlink_metadata(path)
        .map_err(|_| AppError::new("ReadPST cleanup root is unavailable."))?;
    if workspace_metadata.file_type().is_symlink()
        || !workspace_metadata.is_dir()
        || root_metadata.file_type().is_symlink()
        || !root_metadata.is_dir()
        || workspace_metadata.dev() != root_metadata.dev()
    {
        return Err(AppError::new(
            "Refusing an unsafe or cross-device ReadPST cleanup root.",
        ));
    }
    for entry in WalkDir::new(path).follow_links(false).max_open(32) {
        let entry = entry.map_err(|_| AppError::new("Could not inspect ReadPST cleanup root."))?;
        let metadata = fs::symlink_metadata(entry.path())
            .map_err(|_| AppError::new("Could not inspect ReadPST cleanup entry."))?;
        if !metadata.file_type().is_symlink() && metadata.dev() != root_metadata.dev() {
            return Err(AppError::new(
                "Refusing to cross a filesystem boundary during ReadPST cleanup.",
            ));
        }
    }
    Ok(())
}

fn remove_readpst_output_directory(workspace: &Path, path: &Path) -> AppResult<()> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(AppError::from(error)),
    }
    validate_readpst_cleanup_root(workspace, path)?;
    fs::remove_dir_all(path).map_err(AppError::from)
}

fn readpst_workspace_is_intact(workspace: &Path) -> bool {
    fs::symlink_metadata(workspace)
        .is_ok_and(|metadata| !metadata.file_type().is_symlink() && metadata.is_dir())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadpstObservedExit {
    Cancelled,
    Success,
    Failure,
}

fn classify_readpst_observed_exit(
    cancellation_requested: bool,
    status_success: bool,
) -> ReadpstObservedExit {
    if cancellation_requested {
        ReadpstObservedExit::Cancelled
    } else if status_success {
        ReadpstObservedExit::Success
    } else {
        ReadpstObservedExit::Failure
    }
}

fn readpst_running_process_state(time_since_signal: Duration) -> import_progress::ProcessState {
    if time_since_signal > Duration::from_secs(30) {
        import_progress::ProcessState::RunningNoRecentSignal
    } else {
        import_progress::ProcessState::RunningRecentActivity
    }
}

fn monitor_readpst_child(
    app: &tauri::AppHandle,
    child: &mut Child,
    state: &AppState,
    workspace: &Path,
    output_dir: &Path,
    output_readers: &ProcessOutputReaders,
) -> AppResult<ExitStatus> {
    let started = Instant::now();
    let mut last_progress = started;
    let mut next_scan = started;
    let mut previous_root_activity = None;
    let mut previous_process_output_bytes = 0_u64;
    let mut next_activity_emit = started;
    loop {
        if state.cancel_import_requested.load(Ordering::SeqCst) {
            emit_readpst_progress(
                app,
                None,
                true,
                import_progress::ProcessState::CancellationRequested,
            );
            return Err(import_cancelled_error("Import cancelled."));
        }
        if let Some(status) = child.try_wait()? {
            let observed_exit = classify_readpst_observed_exit(
                state.cancel_import_requested.load(Ordering::SeqCst),
                status.success(),
            );
            if observed_exit == ReadpstObservedExit::Cancelled {
                emit_readpst_progress(
                    app,
                    None,
                    true,
                    import_progress::ProcessState::CancellationRequested,
                );
                return Err(import_cancelled_error("Import cancelled."));
            }
            emit_readpst_progress(
                app,
                None,
                true,
                if observed_exit == ReadpstObservedExit::Success {
                    import_progress::ProcessState::ExitedSuccess
                } else {
                    import_progress::ProcessState::ExitedFailure
                },
            );
            return Ok(status);
        }
        if !readpst_workspace_is_intact(workspace) {
            return Err(AppError::new("Workspace became unavailable during import."));
        }
        let observed_process_output_bytes = output_readers.observed_bytes();
        if observed_process_output_bytes > MAX_READPST_STREAM_BYTES {
            return Err(AppError::new(
                "ReadPST process output exceeded the safety limit.",
            ));
        }

        let now = Instant::now();
        if observed_process_output_bytes != previous_process_output_bytes {
            last_progress = now;
            previous_process_output_bytes = observed_process_output_bytes;
            if now >= next_activity_emit {
                emit_readpst_progress(
                    app,
                    None,
                    true,
                    import_progress::ProcessState::RunningRecentActivity,
                );
                next_activity_emit = now + READPST_ACTIVITY_EMIT_INTERVAL;
            }
        }
        if now.duration_since(started) >= READPST_ABSOLUTE_TIMEOUT {
            return Err(AppError::new(
                "ReadPST exceeded the absolute import time limit.",
            ));
        }
        if now >= next_scan {
            let root_activity = inspect_readpst_root_activity(output_dir)?;
            let changed = previous_root_activity.as_ref() != Some(&root_activity);
            if changed {
                last_progress = now;
            }
            emit_readpst_progress(
                app,
                None,
                changed,
                readpst_running_process_state(now.duration_since(last_progress)),
            );
            previous_root_activity = Some(root_activity);
            next_scan = now + READPST_OUTPUT_SCAN_INTERVAL;
        }
        thread::sleep(READPST_POLL_INTERVAL);
    }
}

fn verify_source_identity_unchanged(path: &Path, expected: &FileIdentity) -> AppResult<()> {
    if file_identity(path, "source PST").as_ref().ok() == Some(expected) {
        Ok(())
    } else {
        Err(AppError::new(
            "Source PST identity changed during import; import was refused.",
        ))
    }
}

fn cleanup_readpst_work_directories(
    workspace: &Path,
    output_dir: &Path,
    temp_dir: &Path,
) -> AppResult<()> {
    remove_readpst_output_directory(workspace, output_dir)?;
    remove_readpst_output_directory(workspace, temp_dir)
}

fn run_readpst(
    app: &tauri::AppHandle,
    readpst: &Path,
    pst_path: &Path,
    workspace: &Path,
    output_dir: &Path,
) -> AppResult<FileIdentity> {
    if !is_verified_bundled_readpst_file(readpst) {
        return Err(AppError::new(
            "Bundled ReadPST helper is missing or invalid.",
        ));
    }
    let source_before = file_identity(pst_path, "source PST")?;
    let source_bytes = source_before.size;
    let temp_dir = workspace.join("readpst-tmp");
    remove_readpst_output_directory(workspace, &temp_dir)?;
    create_private_directory_all(&temp_dir)?;

    emit_readpst_progress(app, None, true, import_progress::ProcessState::Starting);

    append_import_log(workspace, "state=extracting")?;

    let mut command = Command::new(readpst);
    command
        .arg("-e")
        .arg("-8")
        .arg("-q")
        .arg("-o")
        .arg(output_dir)
        .arg("--")
        .arg(pst_path);
    configure_isolated_readpst_command(&mut command);
    command.env("TMPDIR", &temp_dir);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = cleanup_readpst_work_directories(workspace, output_dir, &temp_dir);
            return Err(AppError::new(format!(
                "Could not start bundled ReadPST: {error}"
            )));
        }
    };

    let pid = child.id();
    let state = app.state::<AppState>();
    let registration = match register_readpst_process(&state, pid) {
        Ok(registration) => registration,
        Err(error) => {
            let _ = terminate_and_wait_for_process_group(&mut child, pid);
            let source_result = verify_source_identity_unchanged(pst_path, &source_before);
            let cleanup_result = cleanup_readpst_work_directories(workspace, output_dir, &temp_dir);
            source_result?;
            cleanup_result?;
            return Err(error);
        }
    };
    let output_readers = match spawn_process_output_readers(&mut child) {
        Ok(readers) => readers,
        Err(error) => {
            let _ = terminate_and_wait_for_process_group(&mut child, pid);
            drop(registration);
            let source_result = verify_source_identity_unchanged(pst_path, &source_before);
            let cleanup_result = cleanup_readpst_work_directories(workspace, output_dir, &temp_dir);
            source_result?;
            cleanup_result?;
            return Err(error);
        }
    };
    emit_readpst_progress(
        app,
        None,
        true,
        import_progress::ProcessState::RunningRecentActivity,
    );

    let status_result = monitor_readpst_child(
        app,
        &mut child,
        &state,
        workspace,
        output_dir,
        &output_readers,
    );
    if status_result.is_err() {
        let _ = terminate_and_wait_for_process_group(&mut child, pid);
    }
    drop(registration);
    let stdout = join_process_output_reader(output_readers.stdout);
    let stderr = join_process_output_reader(output_readers.stderr);
    let source_after = file_identity(pst_path, "source PST")?;
    if source_after != source_before {
        let _ = cleanup_readpst_work_directories(workspace, output_dir, &temp_dir);
        return Err(AppError::new(
            "Source PST identity changed during import; import was refused.",
        ));
    }
    if remove_readpst_output_directory(workspace, &temp_dir).is_err() {
        let _ = remove_readpst_output_directory(workspace, output_dir);
        return Err(AppError::new(
            "ReadPST temporary output could not be safely removed.",
        ));
    }

    let (stdout, stderr) = match (stdout, stderr) {
        (Ok(stdout), Ok(stderr)) => (stdout, stderr),
        _ => {
            remove_readpst_output_directory(workspace, output_dir)?;
            return Err(AppError::new(
                "ReadPST output capture failed; partial output was removed.",
            ));
        }
    };
    let output_summary = format!(
        "state=extracting stdout_bytes={} stderr_bytes={} truncated={}",
        stdout.total_bytes,
        stderr.total_bytes,
        stdout.truncated() || stderr.truncated()
    );
    let _ = append_import_log(workspace, &output_summary);

    let status = match status_result {
        Ok(status) => status,
        Err(error) => {
            let cleanup = remove_readpst_output_directory(workspace, output_dir);
            if cleanup.is_err() {
                return Err(AppError::new(
                    "ReadPST failed and partial output could not be safely removed.",
                ));
            }
            return Err(error);
        }
    };
    if !status.success() {
        remove_readpst_output_directory(workspace, output_dir)?;
        return Err(AppError::new(format!(
            "ReadPST exited unsuccessfully ({}); partial output was removed.",
            status
                .code()
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string())
        )));
    }

    let snapshot = match inspect_readpst_output_with_progress(
        output_dir,
        readpst_output_byte_limit(source_bytes),
        || {
            emit_readpst_progress(
                app,
                None,
                true,
                import_progress::ProcessState::ExitedSuccess,
            );
            checkpoint_integrity_operation(app, true)
        },
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            remove_readpst_output_directory(workspace, output_dir)?;
            return Err(error);
        }
    };
    emit_readpst_progress(
        app,
        Some(&snapshot),
        true,
        import_progress::ProcessState::ExitedSuccess,
    );
    let _ = append_import_log(
        workspace,
        &format!(
            "state=extraction_complete output_entries={} output_bytes={}",
            snapshot.entries, snapshot.total_bytes
        ),
    );

    Ok(source_after)
}

fn warn_about_storage(
    app: &tauri::AppHandle,
    identity: &PstIdentity,
    workspaces_root: &Path,
) -> AppResult<()> {
    let available = available_disk_bytes_for_path(workspaces_root).ok();
    let estimated_required = estimate_required_cache_bytes(identity.size);
    let mut message = format!(
        "Importing creates a local searchable copy of messages. The original PST will not be modified. PST size: {}. Estimated cache requirement: {}.",
        format_bytes(identity.size),
        format_bytes(estimated_required)
    );

    if let Some(available) = available {
        message.push_str(&format!(" Available space: {}.", format_bytes(available)));
        if available < estimated_required {
            emit_progress(app, "Storage Warning", None, None, &message);
            return Ok(());
        }
    }

    emit_progress(app, "Storage Warning", None, None, &message);
    Ok(())
}

fn workspace_size(
    workspace: &Path,
    workspace_mode: WorkspaceLocationMode,
) -> AppResult<WorkspaceSize> {
    Ok(WorkspaceSize {
        workspace_path: workspace.display().to_string(),
        workspace_location_mode: workspace_mode.as_str().to_string(),
        workspace_location_label: workspace_mode.label().to_string(),
        total_bytes: directory_size(workspace)?,
        extracted_eml_bytes: directory_size(&workspace.join("extracted"))?,
        sqlite_index_bytes: sqlite_size(workspace)?,
        logs_bytes: directory_size(&workspace.join("logs"))?,
        attachments_bytes: directory_size(&workspace.join("attachments"))?,
        available_disk_bytes: available_disk_bytes(workspace).ok(),
    })
}

fn ensure_workspace_marker(
    workspace: &Path,
    workspace_id: &str,
    identity: &PstIdentity,
    workspace_mode: WorkspaceLocationMode,
) -> AppResult<()> {
    create_private_directory_all(workspace)?;
    let marker = workspace.join(WORKSPACE_MARKER_FILE);
    let expected = WorkspaceMarker {
        app: WORKSPACE_MARKER_APP_NAME.to_string(),
        workspace_id: workspace_id.to_string(),
        pst_content_fingerprint: identity.content_fingerprint.clone(),
        workspace_location_mode: workspace_mode.as_str().to_string(),
        original_pst_path: identity.display_path.clone(),
    };

    if marker.exists() {
        let actual = read_workspace_marker(&marker)?;
        validate_workspace_marker_values(&actual, &expected)?;
        return Ok(());
    }

    let contents = serde_json::to_vec_pretty(&expected)
        .map_err(|error| AppError::new(format!("Could not serialize workspace marker: {error}")))?;
    let mut output = create_private_file_new(&marker)?;
    output.write_all(&contents)?;
    output.write_all(b"\n")?;
    output.flush()?;
    Ok(())
}

fn read_workspace_marker(path: &Path) -> AppResult<WorkspaceMarker> {
    require_regular_nonsymlink_file(path, "workspace marker")?;
    let metadata = path.metadata()?;
    if metadata.len() > 16 * 1024 {
        return Err(AppError::new("Workspace marker is unreasonably large."));
    }
    let contents = fs::read_to_string(path)
        .map_err(|error| AppError::new(format!("Could not read workspace marker: {error}")))?;
    if contents.trim_start().starts_with('{') {
        return serde_json::from_str(&contents)
            .map_err(|error| AppError::new(format!("Workspace marker is invalid: {error}")));
    }

    let mut values = HashMap::<&str, &str>::new();
    for line in contents.lines() {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| AppError::new("Legacy workspace marker is malformed."))?;
        if !matches!(
            key,
            "app"
                | "workspace_id"
                | "pst_content_fingerprint"
                | "workspace_location_mode"
                | "original_pst_path"
        ) || values.insert(key, value).is_some()
        {
            return Err(AppError::new(
                "Legacy workspace marker contains unknown or duplicate fields.",
            ));
        }
    }
    if values.len() != 5 {
        return Err(AppError::new(
            "Legacy workspace marker is missing required fields.",
        ));
    }
    Ok(WorkspaceMarker {
        app: values["app"].to_string(),
        workspace_id: values["workspace_id"].to_string(),
        pst_content_fingerprint: values["pst_content_fingerprint"].to_string(),
        workspace_location_mode: values["workspace_location_mode"].to_string(),
        original_pst_path: values["original_pst_path"].to_string(),
    })
}

fn validate_workspace_marker_values(
    actual: &WorkspaceMarker,
    expected: &WorkspaceMarker,
) -> AppResult<()> {
    if actual.app != expected.app
        || actual.workspace_id != expected.workspace_id
        || actual.pst_content_fingerprint != expected.pst_content_fingerprint
        || actual.workspace_location_mode != expected.workspace_location_mode
        || actual.original_pst_path != expected.original_pst_path
    {
        return Err(AppError::new(
            "Workspace marker does not match the selected source and workspace.",
        ));
    }
    Ok(())
}

fn validate_workspace_location(
    profile_paths: &RuntimeProfilePaths,
    active: &ActiveWorkspace,
) -> AppResult<()> {
    require_regular_nonsymlink_file(&active.pst_path, "source PST")?;
    let workspace = active.path.canonicalize()?;
    let source = active.pst_path.canonicalize()?;
    if workspace == source || source.starts_with(&workspace) || workspace.starts_with(&source) {
        return Err(AppError::new(
            "Workspace and source PST paths overlap; deletion is not allowed.",
        ));
    }

    let parent = workspace
        .parent()
        .ok_or_else(|| AppError::new("Workspace path does not have a parent."))?;
    match active.location_mode {
        WorkspaceLocationMode::AppSupport => {
            if parent != workspaces_dir(profile_paths)?.canonicalize()? {
                return Err(AppError::new(
                    "App Support workspace is outside the trusted workspace root.",
                ));
            }
        }
        WorkspaceLocationMode::NextToPst => {
            let parent_name = parent.file_name().and_then(OsStr::to_str);
            if parent_name != Some(NEXT_TO_PST_WORKSPACE_DIR)
                && parent_name != Some(LEGACY_NEXT_TO_PST_WORKSPACE_DIR)
            {
                return Err(AppError::new(
                    "Next-to-PST workspace is outside a trusted cache directory.",
                ));
            }
            let cache_parent = parent
                .parent()
                .ok_or_else(|| AppError::new("Workspace cache has no parent."))?;
            let source_parent = source
                .parent()
                .ok_or_else(|| AppError::new("Source PST has no parent."))?;
            if cache_parent != source_parent {
                return Err(AppError::new(
                    "Next-to-PST workspace is not beside the selected source PST.",
                ));
            }
        }
    }
    Ok(())
}

fn require_regular_nonsymlink_file(path: &Path, label: &str) -> AppResult<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| AppError::new(format!("Could not inspect {label}: {error}")))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.nlink() != 1 {
        return Err(AppError::new(format!(
            "Refusing a symlink, hardlink, or nonregular {label}."
        )));
    }
    Ok(metadata)
}

fn file_identity(path: &Path, label: &str) -> AppResult<FileIdentity> {
    let metadata = require_regular_nonsymlink_file(path, label)?;
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
        sha256: sha256_file(path)?,
    })
}

fn register_export_file(state: &AppState, path: &Path) -> AppResult<PathBuf> {
    let canonical_path = path.canonicalize()?;
    let record = AuthorizedExportFile {
        identity: file_identity(&canonical_path, "exported file")?,
        canonical_path: canonical_path.clone(),
    };
    state
        .exported_files
        .lock()
        .map_err(|_| AppError::new("Export authorization state lock was poisoned."))?
        .register(record);
    Ok(canonical_path)
}

fn authorized_export_path(state: &AppState, requested_path: &str) -> AppResult<PathBuf> {
    let record = state
        .exported_files
        .lock()
        .map_err(|_| AppError::new("Export authorization state lock was poisoned."))?
        .by_path
        .get(requested_path)
        .cloned()
        .ok_or_else(|| AppError::new("Exported file authorization is unavailable."))?;
    let current_path = record.canonical_path.canonicalize();
    let current_identity = current_path
        .as_ref()
        .ok()
        .and_then(|path| file_identity(path, "exported file").ok());
    if current_path.as_ref().ok() != Some(&record.canonical_path)
        || current_identity.as_ref() != Some(&record.identity)
        || record.canonical_path.display().to_string() != requested_path
    {
        if let Ok(mut registry) = state.exported_files.lock() {
            registry.remove(requested_path);
        }
        return Err(AppError::new(
            "Exported file changed or disappeared; refusing the requested action.",
        ));
    }
    Ok(record.canonical_path)
}

fn create_private_directory_all(path: &Path) -> AppResult<()> {
    fs::create_dir_all(path)?;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AppError::new(
            "Refusing a symlink or non-directory private output path.",
        ));
    }
    fs::set_permissions(path, fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE))?;
    Ok(())
}

fn create_private_file_new(path: &Path) -> AppResult<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(PRIVATE_FILE_MODE)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)
        .map_err(AppError::from)
}

fn write_workspace_metadata_atomically(
    metadata_path: &Path,
    value: &serde_json::Value,
) -> AppResult<()> {
    let parent = metadata_path
        .parent()
        .ok_or_else(|| AppError::new("Workspace metadata path has no parent."))?;
    let parent_metadata = fs::symlink_metadata(parent)?;
    if parent_metadata.file_type().is_symlink() || !parent_metadata.is_dir() {
        return Err(AppError::new(
            "Refusing a symlink or non-directory workspace metadata parent.",
        ));
    }
    match fs::symlink_metadata(metadata_path) {
        Ok(metadata)
            if metadata.file_type().is_symlink()
                || !metadata.is_file()
                || metadata.nlink() != 1 =>
        {
            return Err(AppError::new(
                "Refusing a linked or nonregular workspace metadata file.",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err(AppError::new("Workspace metadata is unavailable.")),
    }

    let file_name = metadata_path
        .file_name()
        .and_then(OsStr::to_str)
        .ok_or_else(|| AppError::new("Workspace metadata path has no file name."))?;
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary_path = parent.join(format!(".{file_name}.{}-{unique}.tmp", std::process::id()));
    let serialized =
        serde_json::to_vec_pretty(value).map_err(|error| AppError::new(error.to_string()))?;
    let write_result = (|| {
        let mut file = create_private_file_new(&temporary_path)?;
        file.write_all(&serialized)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary_path, metadata_path)?;
        File::open(parent)?.sync_all()?;
        Ok::<(), AppError>(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    write_result
}

fn validate_workspace_for_delete(
    profile_paths: &RuntimeProfilePaths,
    active: &ActiveWorkspace,
) -> AppResult<ValidatedWorkspaceDelete> {
    let workspace_metadata = fs::symlink_metadata(&active.path).map_err(|error| {
        AppError::new(format!(
            "Could not inspect workspace before deletion: {error}"
        ))
    })?;
    if workspace_metadata.file_type().is_symlink() || !workspace_metadata.is_dir() {
        return Err(AppError::new(format!(
            "Refusing to delete a symlink or non-directory workspace: {}",
            active.path.display()
        )));
    }

    validate_workspace_location(profile_paths, active)?;

    let marker = active.path.join(WORKSPACE_MARKER_FILE);
    let actual_marker = read_workspace_marker(&marker)?;
    let expected_marker = WorkspaceMarker {
        app: WORKSPACE_MARKER_APP_NAME.to_string(),
        workspace_id: active.id.clone(),
        pst_content_fingerprint: active.fingerprint.clone(),
        workspace_location_mode: active.location_mode.as_str().to_string(),
        original_pst_path: active.pst_path.display().to_string(),
    };
    validate_workspace_marker_values(&actual_marker, &expected_marker)?;

    if active.path.file_name().and_then(OsStr::to_str) != Some(active.id.as_str())
        || !is_lower_hex_sha256(&active.id)
    {
        return Err(AppError::new(format!(
            "Refusing to delete {} because it does not match the active workspace id.",
            active.path.display()
        )));
    }

    let index_path = active.path.join("index.sqlite");
    if index_path.exists() {
        require_regular_nonsymlink_file(&index_path, "workspace index")?;
        let conn = open_workspace_db_for_read(&active.path)?;
        let fingerprint = metadata_value(&conn, "pst_content_fingerprint")?
            .ok_or_else(|| AppError::new("Workspace index fingerprint is missing."))?;
        if fingerprint != active.fingerprint {
            return Err(AppError::new(format!(
                "Refusing to delete {} because its fingerprint does not match the active PST.",
                active.path.display()
            )));
        }
    }

    Ok(ValidatedWorkspaceDelete {
        device: workspace_metadata.dev(),
        inode: workspace_metadata.ino(),
    })
}

fn remove_validated_workspace(
    workspace: &Path,
    validated: &ValidatedWorkspaceDelete,
) -> AppResult<()> {
    let parent = workspace
        .parent()
        .ok_or_else(|| AppError::new("Workspace deletion target has no parent."))?;
    let token = sha256_text(&format!(
        "{}:{}:{}:{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        validated.device,
        validated.inode
    ));
    let quarantine = parent.join(format!(".pst-quickview-delete-{token}"));
    if quarantine.exists() {
        return Err(AppError::new(
            "Could not allocate a private workspace deletion path.",
        ));
    }

    fs::rename(workspace, &quarantine).map_err(|error| {
        AppError::new(format!("Could not isolate workspace for deletion: {error}"))
    })?;
    let moved_metadata = fs::symlink_metadata(&quarantine)?;
    if moved_metadata.file_type().is_symlink()
        || !moved_metadata.is_dir()
        || moved_metadata.dev() != validated.device
        || moved_metadata.ino() != validated.inode
    {
        if !workspace.exists() {
            let _ = fs::rename(&quarantine, workspace);
        }
        return Err(AppError::new(
            "Workspace changed during deletion validation; nothing was deleted.",
        ));
    }

    if let Err(error) = fs::remove_dir_all(&quarantine) {
        if !workspace.exists() {
            let _ = fs::rename(&quarantine, workspace);
        }
        return Err(AppError::new(format!(
            "Failed to delete the validated workspace: {error}"
        )));
    }
    Ok(())
}

fn checked_truncate_checkpoint(conn: &Connection) -> AppResult<WalCheckpointResult> {
    conn.busy_timeout(Duration::from_secs(5))?;
    let result = conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        Ok(WalCheckpointResult {
            busy: row.get(0)?,
            log_frames: row.get(1)?,
            checkpointed_frames: row.get(2)?,
        })
    })?;
    if result.busy != 0 || result.log_frames != result.checkpointed_frames {
        return Err(AppError::new(
            "The workspace index is busy and could not be checkpointed safely; no index was replaced.",
        ));
    }
    Ok(result)
}

fn optimize_then_checked_truncate(conn: &Connection) -> AppResult<WalCheckpointResult> {
    conn.execute_batch("PRAGMA optimize;")?;
    checked_truncate_checkpoint(conn)
}

fn remove_empty_next_to_pst_parent(active: &ActiveWorkspace) -> AppResult<bool> {
    if active.location_mode != WorkspaceLocationMode::NextToPst {
        return Ok(false);
    }

    let Some(parent) = active.path.parent() else {
        return Ok(false);
    };

    let parent_name = parent.file_name().and_then(OsStr::to_str);
    if parent_name != Some(NEXT_TO_PST_WORKSPACE_DIR)
        && parent_name != Some(LEGACY_NEXT_TO_PST_WORKSPACE_DIR)
    {
        return Ok(false);
    }

    if parent.exists() && fs::read_dir(parent)?.next().is_none() {
        fs::remove_dir(parent).map_err(|error| {
            AppError::new(format!(
                "Workspace was deleted, but empty parent cleanup failed for {}: {}",
                parent.display(),
                error
            ))
        })?;
        return Ok(true);
    }

    Ok(false)
}

fn remaining_entries(path: &Path) -> Vec<String> {
    if !path.exists() {
        return Vec::new();
    }

    let Ok(entries) = fs::read_dir(path) else {
        return vec!["<could not read directory entries>".to_string()];
    };

    entries
        .take(20)
        .map(|entry| match entry {
            Ok(entry) => entry.path().display().to_string(),
            Err(error) => format!("<error reading entry: {error}>"),
        })
        .collect()
}

fn delete_result_message(result: &DeleteResult) -> String {
    if result.deleted && !result.exists_after {
        let mut message = format!(
            "Deleted workspace {}. The original PST was not deleted.",
            result.attempted_path
        );
        if result.removed_empty_parent {
            if let Some(parent) = &result.parent_path {
                message.push_str(&format!(" Removed empty parent {parent}."));
            }
        }
        if let Some(error) = &result.error {
            message.push_str(&format!(" Cleanup warning: {error}"));
        }
        message
    } else {
        format!(
            "Failed to delete workspace {}: {}",
            result.attempted_path,
            result
                .error
                .as_deref()
                .unwrap_or("workspace still exists after deletion attempt")
        )
    }
}

fn directory_size(path: &Path) -> AppResult<u64> {
    if !path.exists() {
        return Ok(0);
    }

    let mut total = 0_u64;
    for entry in WalkDir::new(path) {
        let entry = entry?;
        if entry.file_type().is_file() {
            total = total.saturating_add(entry.metadata()?.len());
        }
    }
    Ok(total)
}

fn sqlite_size(workspace: &Path) -> AppResult<u64> {
    let mut total = 0_u64;
    for name in ["index.sqlite", "index.sqlite-wal", "index.sqlite-shm"] {
        let path = workspace.join(name);
        if path.exists() {
            total = total.saturating_add(fs::metadata(path)?.len());
        }
    }
    Ok(total)
}

fn available_disk_bytes(path: &Path) -> AppResult<u64> {
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes())
        .map_err(|_| AppError::new("Workspace path contains a null byte."))?;
    let mut stat: libc::statfs = unsafe { std::mem::zeroed() };
    let result = unsafe { libc::statfs(c_path.as_ptr(), &mut stat) };
    if result != 0 {
        return Err(AppError::new(std::io::Error::last_os_error().to_string()));
    }

    Ok(stat.f_bavail.saturating_mul(stat.f_bsize as u64))
}

fn available_disk_bytes_for_path(path: &Path) -> AppResult<u64> {
    let check_path = nearest_existing_ancestor(path).ok_or_else(|| {
        AppError::new(format!(
            "Could not find an existing parent directory for {}.",
            path.display()
        ))
    })?;
    available_disk_bytes(&check_path)
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit_index = 0;

    while value >= 1024.0 && unit_index < UNITS.len() - 1 {
        value /= 1024.0;
        unit_index += 1;
    }

    if unit_index == 0 {
        format!("{bytes} {}", UNITS[unit_index])
    } else {
        format!("{value:.1} {}", UNITS[unit_index])
    }
}

fn remove_sqlite_file_set(path: &Path) -> AppResult<()> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    remove_sqlite_sidecars(path)
}

fn remove_sqlite_sidecars(path: &Path) -> AppResult<()> {
    let Some(file_name) = path.file_name().and_then(OsStr::to_str) else {
        return Ok(());
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = parent.join(format!("{file_name}{suffix}"));
        if sidecar.exists() {
            fs::remove_file(sidecar)?;
        }
    }
    Ok(())
}

fn remove_checkpointed_sqlite_sidecars(path: &Path) -> AppResult<()> {
    let Some(file_name) = path.file_name().and_then(OsStr::to_str) else {
        return Err(AppError::new("Workspace index path has no file name."));
    };
    let parent = path
        .parent()
        .ok_or_else(|| AppError::new("Workspace index path has no parent."))?;
    let mut removable = Vec::new();
    for suffix in ["-wal", "-shm", "-journal"] {
        let sidecar = parent.join(format!("{file_name}{suffix}"));
        match fs::symlink_metadata(&sidecar) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.nlink() != 1
                {
                    return Err(AppError::new(
                        "Refusing a linked or nonregular workspace index sidecar.",
                    ));
                }
                if matches!(suffix, "-wal" | "-journal") && metadata.len() != 0 {
                    return Err(AppError::new(
                        "A workspace index contains uncheckpointed data; no index was replaced.",
                    ));
                }
                removable.push(sidecar);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(AppError::new("Workspace index sidecar is unavailable.")),
        }
    }
    for sidecar in removable {
        fs::remove_file(sidecar)?;
    }
    File::open(parent)?.sync_all()?;
    Ok(())
}

fn sqlite_file_state(path: &Path) -> AppResult<SqliteFileState> {
    let metadata = require_regular_nonsymlink_file(path, "workspace index")?;
    Ok(SqliteFileState {
        device: metadata.dev(),
        inode: metadata.ino(),
        size: metadata.len(),
        modified_seconds: metadata.mtime(),
        modified_nanoseconds: metadata.mtime_nsec(),
    })
}

fn require_unchanged_sqlite_file(path: &Path, expected: SqliteFileState) -> AppResult<()> {
    if sqlite_file_state(path)? != expected {
        return Err(AppError::new(
            "A workspace index changed during validation; no index was replaced.",
        ));
    }
    Ok(())
}

fn sync_unchanged_sqlite_file(path: &Path, expected: SqliteFileState) -> AppResult<()> {
    require_unchanged_sqlite_file(path, expected)?;
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW)
        .open(path)?;
    let opened = file.metadata()?;
    let opened_state = SqliteFileState {
        device: opened.dev(),
        inode: opened.ino(),
        size: opened.len(),
        modified_seconds: opened.mtime(),
        modified_nanoseconds: opened.mtime_nsec(),
    };
    if opened.nlink() != 1 || opened_state != expected {
        return Err(AppError::new(
            "A workspace index changed while it was opened; no index was replaced.",
        ));
    }
    file.sync_all()?;
    require_unchanged_sqlite_file(path, expected)
}

fn preflight_owned_current_index(
    index_path: &Path,
    allowed_workspace_ids: &[&str],
    source_fingerprint: &str,
) -> AppResult<()> {
    let conn = open_sqlite_read_only_connection(index_path)?;
    configure_sqlite_connection(&conn, true)?;
    if read_schema_version(&conn)? != SQLITE_SCHEMA_VERSION_CURRENT {
        return Err(AppError::new(
            "The existing workspace index schema is not eligible for safe replacement.",
        ));
    }
    verify_read_query_schema(&conn)?;
    verify_schema(&conn)?;
    if !has_valid_import_metadata_key(&conn)? {
        return Err(AppError::new(
            "The existing workspace index has an invalid metadata identity key.",
        ));
    }
    let stored_workspace_id = metadata_value(&conn, "workspace_id")?;
    if !stored_workspace_id
        .as_deref()
        .is_some_and(|stored| allowed_workspace_ids.contains(&stored))
    {
        return Err(AppError::new(
            "The existing workspace index belongs to a different workspace.",
        ));
    }
    if metadata_value(&conn, "pst_content_fingerprint")?.as_deref() != Some(source_fingerprint) {
        return Err(AppError::new(
            "The existing workspace index belongs to a different source.",
        ));
    }
    let quick_check = conn.query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))?;
    if quick_check != "ok" {
        return Err(AppError::new(
            "The existing workspace index failed SQLite integrity checking.",
        ));
    }
    Ok(())
}

fn required_metadata_usize(conn: &Connection, key: &str) -> AppResult<usize> {
    metadata_value(conn, key)?
        .and_then(|value| value.parse::<usize>().ok())
        .ok_or_else(|| AppError::new("The completed index has invalid integrity counters."))
}

fn preflight_completed_index(
    index_path: &Path,
    allowed_workspace_ids: &[&str],
    source_fingerprint: &str,
    discovery: &EmlDiscovery,
) -> AppResult<()> {
    preflight_owned_current_index(index_path, allowed_workspace_ids, source_fingerprint)?;
    let conn = open_sqlite_read_only_connection(index_path)?;
    configure_sqlite_connection(&conn, true)?;
    for (key, expected) in [
        (
            "canonical_import_identity_version",
            CANONICAL_IMPORT_IDENTITY_VERSION,
        ),
        ("importer_generation", IMPORTER_GENERATION),
        ("candidate_identity_manifest", &discovery.identity_manifest),
        ("structurally_empty_artifacts_suppressed", "0"),
    ] {
        if metadata_value(&conn, key)?.as_deref() != Some(expected) {
            return Err(AppError::new(
                "The replacement index is missing required completed import provenance.",
            ));
        }
    }
    if !matches!(
        metadata_value(&conn, "import_status")?.as_deref(),
        Some("complete" | "verifying")
    ) {
        return Err(AppError::new(
            "The replacement index is not awaiting or carrying completed verification.",
        ));
    }
    let database_total = conn.query_row("SELECT COUNT(*) FROM messages", [], |row| {
        row.get::<_, usize>(0)
    })?;
    let discovered = required_metadata_usize(&conn, "candidates_discovered")?;
    let processed = required_metadata_usize(&conn, "candidates_processed")?;
    let inserted = required_metadata_usize(&conn, "canonical_messages_inserted")?;
    let skipped = required_metadata_usize(&conn, "existing_canonical_messages_skipped")?;
    let canonical_total = required_metadata_usize(&conn, "canonical_messages_total")?;
    let message_count = required_metadata_usize(&conn, "message_count_indexed")?;
    if discovered != discovery.count
        || processed != discovery.count
        || inserted.checked_add(skipped) != Some(discovery.count)
        || database_total != discovery.count
        || canonical_total != database_total
        || message_count != database_total
    {
        return Err(AppError::new(
            "The completed replacement index counters are inconsistent.",
        ));
    }
    validate_existing_message_identity_subset(&conn, discovery)?;
    Ok(())
}

fn checkpoint_index_for_promotion(
    index_path: &Path,
    expected: SqliteFileState,
) -> AppResult<SqliteFileState> {
    require_unchanged_sqlite_file(index_path, expected)?;
    let conn = open_sqlite_write_connection(index_path)?;
    require_unchanged_sqlite_file(index_path, expected)?;
    checked_truncate_checkpoint(&conn)?;
    drop(conn);
    remove_checkpointed_sqlite_sidecars(index_path)?;
    sqlite_file_state(index_path)
}

fn restore_pre_promotion_index_layout(
    workspace: &Path,
    temp_index: &Path,
    index_path: &Path,
    replacement_state: SqliteFileState,
    original_state: SqliteFileState,
) -> AppResult<()> {
    require_unchanged_sqlite_file(index_path, replacement_state)?;
    require_unchanged_sqlite_file(temp_index, original_state)?;
    if sqlite_sidecar_exists(index_path) || sqlite_sidecar_exists(temp_index) {
        return Err(AppError::new(
            "A workspace index sidecar appeared before promotion rollback.",
        ));
    }
    swap_index_files(temp_index, index_path)?;
    if sqlite_file_state(index_path)? != original_state
        || sqlite_file_state(temp_index)? != replacement_state
        || sqlite_sidecar_exists(index_path)
        || sqlite_sidecar_exists(temp_index)
    {
        return Err(AppError::new(
            "Workspace index entries changed during promotion rollback.",
        ));
    }
    File::open(workspace)?.sync_all()?;
    Ok(())
}

#[cfg(target_vendor = "apple")]
fn rename_index_exclusive(source: &Path, destination: &Path) -> AppResult<()> {
    rename_index_with_flags(source, destination, libc::RENAME_EXCL)
}

#[cfg(target_vendor = "apple")]
fn swap_index_files(left: &Path, right: &Path) -> AppResult<()> {
    rename_index_with_flags(left, right, libc::RENAME_SWAP)
}

#[cfg(target_vendor = "apple")]
fn rename_index_with_flags(source: &Path, destination: &Path, flags: u32) -> AppResult<()> {
    let source = CString::new(source.as_os_str().as_bytes())
        .map_err(|_| AppError::new("Workspace index path contains a null byte."))?;
    let destination = CString::new(destination.as_os_str().as_bytes())
        .map_err(|_| AppError::new("Workspace index path contains a null byte."))?;
    let result = unsafe { libc::renamex_np(source.as_ptr(), destination.as_ptr(), flags) };
    if result != 0 {
        return Err(AppError::new(format!(
            "Atomic workspace index rename failed: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

#[cfg(not(target_vendor = "apple"))]
fn rename_index_exclusive(source: &Path, destination: &Path) -> AppResult<()> {
    fs::hard_link(source, destination)?;
    if let Err(error) = fs::remove_file(source) {
        let _ = fs::remove_file(destination);
        return Err(AppError::new(format!(
            "Atomic workspace index rename failed: {error}"
        )));
    }
    Ok(())
}

#[cfg(not(target_vendor = "apple"))]
fn swap_index_files(left: &Path, right: &Path) -> AppResult<()> {
    let backup = left.with_extension("swap-backup");
    rename_index_exclusive(right, &backup)?;
    if let Err(error) = fs::rename(left, right) {
        let _ = fs::rename(&backup, right);
        return Err(AppError::new(format!(
            "Atomic workspace index swap failed: {error}"
        )));
    }
    if let Err(error) = fs::rename(&backup, left) {
        let _ = fs::rename(right, left);
        let _ = fs::rename(&backup, right);
        return Err(AppError::new(format!(
            "Atomic workspace index swap failed: {error}"
        )));
    }
    Ok(())
}

#[cfg(test)]
fn promote_completed_index(
    workspace: &Path,
    temp_index: &Path,
    allowed_workspace_ids: &[&str],
    source_fingerprint: &str,
    discovery: &EmlDiscovery,
) -> AppResult<()> {
    promote_completed_index_with_policy(
        None,
        workspace,
        temp_index,
        allowed_workspace_ids,
        source_fingerprint,
        discovery,
        false,
    )
}

fn promote_completed_index_with_operation_progress(
    app: &tauri::AppHandle,
    workspace: &Path,
    temp_index: &Path,
    allowed_workspace_ids: &[&str],
    source_fingerprint: &str,
    discovery: &EmlDiscovery,
) -> AppResult<()> {
    promote_completed_index_with_policy(
        Some(app),
        workspace,
        temp_index,
        allowed_workspace_ids,
        source_fingerprint,
        discovery,
        false,
    )
}

fn promote_repaired_index_with_operation_progress(
    app: &tauri::AppHandle,
    workspace: &Path,
    temp_index: &Path,
    allowed_workspace_ids: &[&str],
    source_fingerprint: &str,
    discovery: &EmlDiscovery,
) -> AppResult<()> {
    promote_completed_index_with_policy(
        Some(app),
        workspace,
        temp_index,
        allowed_workspace_ids,
        source_fingerprint,
        discovery,
        true,
    )
}

fn promote_completed_index_with_policy(
    app: Option<&tauri::AppHandle>,
    workspace: &Path,
    temp_index: &Path,
    allowed_workspace_ids: &[&str],
    source_fingerprint: &str,
    discovery: &EmlDiscovery,
    allow_unreadable_owned_main: bool,
) -> AppResult<()> {
    if temp_index.parent() != Some(workspace)
        || !matches!(
            temp_index.file_name().and_then(OsStr::to_str),
            Some("index.tmp.sqlite" | "index.reindex.tmp.sqlite")
        )
    {
        return Err(AppError::new(
            "Refusing to promote an unexpected workspace index path.",
        ));
    }
    let index_path = workspace.join("index.sqlite");
    let has_staged_manifest =
        import_integrity_phase2::has_staged_manifest(workspace, allowed_workspace_ids[0])?;
    let temp_initial_state = sqlite_file_state(temp_index)?;
    preflight_completed_index(
        temp_index,
        allowed_workspace_ids,
        source_fingerprint,
        discovery,
    )?;
    require_unchanged_sqlite_file(temp_index, temp_initial_state)?;
    let temp_preflight_state = temp_initial_state;
    let (main_preflight_state, main_is_readable) = match fs::symlink_metadata(&index_path) {
        Ok(_) => {
            let initial_state = sqlite_file_state(&index_path)?;
            let readable = match preflight_owned_current_index(
                &index_path,
                allowed_workspace_ids,
                source_fingerprint,
            ) {
                Ok(()) => true,
                Err(error) if allow_unreadable_owned_main => {
                    if sqlite_sidecar_exists(&index_path) {
                        return Err(AppError::new(
                            "The unreadable active index has sidecars; repair promotion was refused.",
                        ));
                    }
                    let marker = read_workspace_marker(&workspace.join(WORKSPACE_MARKER_FILE))?;
                    if marker.app != WORKSPACE_MARKER_APP_NAME
                        || !allowed_workspace_ids.contains(&marker.workspace_id.as_str())
                        || marker.pst_content_fingerprint != source_fingerprint
                    {
                        return Err(error);
                    }
                    sync_unchanged_sqlite_file(&index_path, initial_state)?;
                    false
                }
                Err(error) => return Err(error),
            };
            require_unchanged_sqlite_file(&index_path, initial_state)?;
            (Some(initial_state), readable)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if sqlite_sidecar_exists(&index_path) {
                return Err(AppError::new(
                    "The final workspace index file set is incomplete; no index was replaced.",
                ));
            }
            (None, false)
        }
        Err(_) => return Err(AppError::new("The final workspace index is unavailable.")),
    };

    let temp_checkpointed_state = checkpoint_index_for_promotion(temp_index, temp_preflight_state)?;
    preflight_completed_index(
        temp_index,
        allowed_workspace_ids,
        source_fingerprint,
        discovery,
    )?;
    require_unchanged_sqlite_file(temp_index, temp_checkpointed_state)?;
    remove_checkpointed_sqlite_sidecars(temp_index)?;
    let main_checkpointed_state = if let Some(expected) = main_preflight_state {
        if main_is_readable {
            let checkpointed = checkpoint_index_for_promotion(&index_path, expected)?;
            preflight_owned_current_index(&index_path, allowed_workspace_ids, source_fingerprint)?;
            require_unchanged_sqlite_file(&index_path, checkpointed)?;
            remove_checkpointed_sqlite_sidecars(&index_path)?;
            Some(checkpointed)
        } else {
            sync_unchanged_sqlite_file(&index_path, expected)?;
            require_unchanged_sqlite_file(&index_path, expected)?;
            Some(expected)
        }
    } else {
        match fs::symlink_metadata(&index_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(AppError::new(
                    "A final workspace index appeared during promotion; nothing was replaced.",
                ))
            }
            Err(_) => return Err(AppError::new("The final workspace index is unavailable.")),
        }
        if sqlite_sidecar_exists(&index_path) {
            return Err(AppError::new(
                "A final workspace index sidecar appeared during promotion; nothing was replaced.",
            ));
        }
        None
    };

    sync_unchanged_sqlite_file(temp_index, temp_checkpointed_state)?;
    if let Some(expected) = main_checkpointed_state {
        require_unchanged_sqlite_file(&index_path, expected)?;
        if sqlite_sidecar_exists(&index_path) || sqlite_sidecar_exists(temp_index) {
            return Err(AppError::new(
                "A workspace index sidecar appeared during promotion; nothing was replaced.",
            ));
        }
        require_unchanged_sqlite_file(temp_index, temp_checkpointed_state)?;
        swap_index_files(temp_index, &index_path)?;
        let swap_valid = sqlite_file_state(&index_path)
            .is_ok_and(|state| state == temp_checkpointed_state)
            && sqlite_file_state(temp_index).is_ok_and(|state| state == expected)
            && !sqlite_sidecar_exists(&index_path)
            && !sqlite_sidecar_exists(temp_index);
        if !swap_valid {
            return Err(AppError::coded(
                "Workspace index entries changed after atomic promotion; the final index state must be reviewed before retrying.",
                INDEX_PROMOTION_STATE_UNCERTAIN_CODE,
            ));
        }
        if let Err(error) = File::open(workspace).and_then(|directory| directory.sync_all()) {
            return match restore_pre_promotion_index_layout(
                workspace,
                temp_index,
                &index_path,
                temp_checkpointed_state,
                expected,
            ) {
                Ok(()) => Err(AppError::new(format!(
                    "Workspace index promotion was rolled back after its directory could not be synchronized: {error}"
                ))),
                Err(rollback_error) => Err(AppError::coded(
                    format!(
                        "The workspace index was atomically promoted, but directory synchronization and checked rollback both failed: {}; {}",
                        error, rollback_error.message
                    ),
                    INDEX_PROMOTION_STATE_UNCERTAIN_CODE,
                )),
            };
        }
        if has_staged_manifest
            || workspace
                .join(import_integrity_phase2::IMPORT_MANIFEST_FILE)
                .is_file()
        {
            let final_verification = if take_phase2_final_path_verification_failure() {
                Err(AppError::new(
                    "Synthetic final-path replacement verification failure.",
                ))
            } else if has_staged_manifest {
                verify_promoted_index_with_optional_progress(
                    app,
                    workspace,
                    allowed_workspace_ids[0],
                    &index_path,
                    true,
                )
            } else {
                verify_promoted_index_with_optional_progress(
                    app,
                    workspace,
                    allowed_workspace_ids[0],
                    &index_path,
                    false,
                )
            };
            if let Err(error) = final_verification {
                return match restore_pre_promotion_index_layout(
                    workspace,
                    temp_index,
                    &index_path,
                    temp_checkpointed_state,
                    expected,
                ) {
                    Ok(()) => Err(AppError::new(format!(
                        "Workspace index promotion was rolled back after final-path verification failed: {error}"
                    ))),
                    Err(rollback_error) => Err(AppError::coded(
                        format!(
                            "The workspace index failed final-path verification and checked rollback also failed: {}; {}",
                            error, rollback_error.message
                        ),
                        INDEX_PROMOTION_STATE_UNCERTAIN_CODE,
                    )),
                };
            }
        }
        if has_staged_manifest {
            if let Err(error) = import_integrity_phase2::publish_staged_manifest(
                workspace,
                allowed_workspace_ids[0],
            ) {
                match import_integrity_phase2::active_manifest_matches_staged(
                    workspace,
                    allowed_workspace_ids[0],
                ) {
                    Ok(true) => {
                        return Err(AppError::coded(
                            "The replacement database and manifest are active, but metadata durability could not be confirmed; recovery data was preserved.",
                            INDEX_PROMOTION_STATE_UNCERTAIN_CODE,
                        ));
                    }
                    Err(_) => {
                        return Err(AppError::coded(
                            "Replacement metadata state could not be classified after promotion; recovery data was preserved.",
                            INDEX_PROMOTION_STATE_UNCERTAIN_CODE,
                        ));
                    }
                    Ok(false) => {}
                }
                return match restore_pre_promotion_index_layout(
                    workspace,
                    temp_index,
                    &index_path,
                    temp_checkpointed_state,
                    expected,
                ) {
                    Ok(()) => Err(AppError::new(format!(
                        "Workspace index promotion was rolled back because replacement metadata could not be committed: {error}"
                    ))),
                    Err(rollback_error) => Err(AppError::coded(
                        format!(
                            "Replacement metadata commit and checked index rollback both failed: {}; {}",
                            error, rollback_error.message
                        ),
                        INDEX_PROMOTION_STATE_UNCERTAIN_CODE,
                    )),
                };
            }
        }
        if let Err(error) = fs::remove_file(temp_index) {
            if has_staged_manifest {
                return Err(AppError::coded(
                    format!(
                        "The replacement index is active and verified, but the rollback backup could not be retired: {error}"
                    ),
                    INDEX_PROMOTION_STATE_UNCERTAIN_CODE,
                ));
            }
            return match restore_pre_promotion_index_layout(
                workspace,
                temp_index,
                &index_path,
                temp_checkpointed_state,
                expected,
            ) {
                Ok(()) => Err(AppError::new(format!(
                    "Workspace index promotion was rolled back because the replaced index could not be retired: {error}"
                ))),
                Err(rollback_error) => Err(AppError::coded(
                    format!(
                        "The workspace index was atomically promoted, but old-index retirement and checked rollback both failed: {}; {}",
                        error, rollback_error.message
                    ),
                    INDEX_PROMOTION_STATE_UNCERTAIN_CODE,
                )),
            };
        }
    } else {
        match fs::symlink_metadata(&index_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => {
                return Err(AppError::new(
                    "A final workspace index appeared during promotion; nothing was replaced.",
                ))
            }
            Err(_) => return Err(AppError::new("The final workspace index is unavailable.")),
        }
        if sqlite_sidecar_exists(&index_path) || sqlite_sidecar_exists(temp_index) {
            return Err(AppError::new(
                "A workspace index sidecar appeared during promotion; nothing was replaced.",
            ));
        }
        require_unchanged_sqlite_file(temp_index, temp_checkpointed_state)?;
        rename_index_exclusive(temp_index, &index_path)?;
        if !sqlite_file_state(&index_path).is_ok_and(|state| state == temp_checkpointed_state)
            || sqlite_sidecar_exists(&index_path)
        {
            return Err(AppError::coded(
                "Workspace index entries changed after atomic promotion; the final index state must be reviewed before retrying.",
                INDEX_PROMOTION_STATE_UNCERTAIN_CODE,
            ));
        }
        if has_staged_manifest
            || workspace
                .join(import_integrity_phase2::IMPORT_MANIFEST_FILE)
                .is_file()
        {
            let final_verification = if take_phase2_final_path_verification_failure() {
                Err(AppError::new(
                    "Synthetic final-path replacement verification failure.",
                ))
            } else if has_staged_manifest {
                verify_promoted_index_with_optional_progress(
                    app,
                    workspace,
                    allowed_workspace_ids[0],
                    &index_path,
                    true,
                )
            } else {
                verify_promoted_index_with_optional_progress(
                    app,
                    workspace,
                    allowed_workspace_ids[0],
                    &index_path,
                    false,
                )
            };
            if let Err(error) = final_verification {
                let rollback = (|| {
                    require_unchanged_sqlite_file(&index_path, temp_checkpointed_state)?;
                    if sqlite_sidecar_exists(&index_path) || sqlite_sidecar_exists(temp_index) {
                        return Err(AppError::new(
                            "Workspace index sidecars changed during checked rollback.",
                        ));
                    }
                    rename_index_exclusive(&index_path, temp_index)?;
                    File::open(workspace)?.sync_all()?;
                    Ok::<(), AppError>(())
                })();
                return match rollback {
                    Ok(()) => Err(AppError::new(format!(
                        "Workspace index promotion was rolled back after final-path verification failed: {error}"
                    ))),
                    Err(rollback_error) => Err(AppError::coded(
                        format!(
                            "The replacement index failed final-path verification and checked rollback also failed: {}; {}",
                            error, rollback_error.message
                        ),
                        INDEX_PROMOTION_STATE_UNCERTAIN_CODE,
                    )),
                };
            }
        }
        if has_staged_manifest {
            if let Err(error) = import_integrity_phase2::publish_staged_manifest(
                workspace,
                allowed_workspace_ids[0],
            ) {
                match import_integrity_phase2::active_manifest_matches_staged(
                    workspace,
                    allowed_workspace_ids[0],
                ) {
                    Ok(true) => {
                        return Err(AppError::coded(
                            "The replacement database and manifest are active, but metadata durability could not be confirmed; recovery data was preserved.",
                            INDEX_PROMOTION_STATE_UNCERTAIN_CODE,
                        ));
                    }
                    Err(_) => {
                        return Err(AppError::coded(
                            "Replacement metadata state could not be classified after promotion; recovery data was preserved.",
                            INDEX_PROMOTION_STATE_UNCERTAIN_CODE,
                        ));
                    }
                    Ok(false) => {}
                }
                let rollback = (|| {
                    require_unchanged_sqlite_file(&index_path, temp_checkpointed_state)?;
                    rename_index_exclusive(&index_path, temp_index)?;
                    File::open(workspace)?.sync_all()?;
                    Ok::<(), AppError>(())
                })();
                return match rollback {
                    Ok(()) => Err(AppError::new(format!(
                        "Workspace index promotion was rolled back because replacement metadata could not be committed: {error}"
                    ))),
                    Err(rollback_error) => Err(AppError::coded(
                        format!(
                            "Replacement metadata commit and checked index rollback both failed: {}; {}",
                            error, rollback_error.message
                        ),
                        INDEX_PROMOTION_STATE_UNCERTAIN_CODE,
                    )),
                };
            }
        }
    }
    #[cfg(test)]
    if take_index_promotion_after_commit_failure() {
        return Err(AppError::coded(
            "Synthetic failure after committed workspace index promotion.",
            INDEX_PROMOTION_STATE_UNCERTAIN_CODE,
        ));
    }
    File::open(workspace)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            AppError::coded(
                format!(
                    "The completed workspace index was installed, but its directory could not be synchronized: {error}"
                ),
                INDEX_PROMOTION_STATE_UNCERTAIN_CODE,
            )
        })?;
    Ok(())
}

fn sqlite_sidecar_exists(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(OsStr::to_str) else {
        return false;
    };
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    ["-wal", "-shm", "-journal"]
        .iter()
        .any(|suffix| fs::symlink_metadata(parent.join(format!("{file_name}{suffix}"))).is_ok())
}

fn is_eml_file(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|ext| ext.eq_ignore_ascii_case("eml"))
        .unwrap_or(false)
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EmlDiscovery {
    count: usize,
    identity_manifest: String,
    candidates: Vec<EmlCandidate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct EmlCandidate {
    path: PathBuf,
    relative_path: String,
    identity: FileIdentity,
}

const DISCOVERY_PROGRESS_BATCH_SIZE: usize = 256;
const DISCOVERY_HASH_PROGRESS_BYTES: u64 = 8 * 1024 * 1024;

fn discover_eml_candidates_with_progress<F>(
    extract_dir: &Path,
    mut on_progress: F,
) -> AppResult<EmlDiscovery>
where
    F: FnMut(usize, bool) -> AppResult<()>,
{
    let root_metadata = fs::symlink_metadata(extract_dir)
        .map_err(|_| AppError::new("Extracted source records are unavailable."))?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        return Err(AppError::new(
            "Extracted source records must use a regular directory.",
        ));
    }
    let mut candidates = Vec::new();
    let mut manifest = Sha256::new();
    let mut traversed_entries = 0_usize;
    manifest.update(b"pstqv-eml-candidate-snapshot-v2\0");
    for entry in WalkDir::new(extract_dir).sort_by_file_name() {
        let entry = entry?;
        if entry.path() == extract_dir {
            continue;
        }
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() || (!metadata.is_dir() && !metadata.is_file()) {
            return Err(AppError::new(
                "Extracted source records contain a linked or nonregular entry.",
            ));
        }
        if metadata.is_file() && is_eml_file(entry.path()) {
            let relative_path = relative_path_string(extract_dir, entry.path())?;
            let identity = snapshot_eml_candidate_with_progress(entry.path(), || {
                on_progress(candidates.len(), true)
            })?;
            let relative_bytes = relative_path.as_bytes();
            manifest.update((relative_bytes.len() as u64).to_be_bytes());
            manifest.update(relative_bytes);
            manifest.update(identity.size.to_be_bytes());
            manifest.update(identity.sha256.as_bytes());
            candidates.push(EmlCandidate {
                path: entry.path().to_path_buf(),
                relative_path,
                identity,
            });
            if candidates.len() % DISCOVERY_PROGRESS_BATCH_SIZE == 0 {
                on_progress(candidates.len(), true)?;
            }
        }
        traversed_entries = traversed_entries.saturating_add(1);
        if traversed_entries % DISCOVERY_PROGRESS_BATCH_SIZE == 0 {
            on_progress(candidates.len(), true)?;
        }
    }
    let count = candidates.len();
    on_progress(count, traversed_entries > 0)?;
    Ok(EmlDiscovery {
        count,
        identity_manifest: hex::encode(manifest.finalize()),
        candidates,
    })
}

#[cfg(test)]
fn discover_eml_candidates(extract_dir: &Path) -> AppResult<EmlDiscovery> {
    discover_eml_candidates_with_progress(extract_dir, |_, _| Ok(()))
}

fn emit_discovery_progress(
    app: &tauri::AppHandle,
    discovered: usize,
    activity_observed: bool,
) -> AppResult<()> {
    if app
        .state::<AppState>()
        .cancel_import_requested
        .load(Ordering::SeqCst)
    {
        return Err(import_cancelled_error(
            "Candidate discovery was cancelled at a bounded checkpoint.",
        ));
    }
    emit_progress_observation(
        app,
        ProgressObservation {
            processed_units: u64::try_from(discovered).ok(),
            total_units: None,
            unit_kind: Some(import_progress::ProgressUnitKind::CandidateRecords),
            activity_observed,
            counters: Some(import_progress::ProgressCounters {
                candidates_discovered: u64::try_from(discovered).ok(),
                ..import_progress::ProgressCounters::default()
            }),
            ..ProgressObservation::default()
        },
    );
    Ok(())
}

fn emit_manifest_adoption_progress_with_policy(
    app: &tauri::AppHandle,
    discovered: usize,
    activity_observed: bool,
    honor_cancellation: bool,
) -> AppResult<()> {
    if honor_cancellation
        && app
            .state::<AppState>()
            .cancel_import_requested
            .load(Ordering::SeqCst)
    {
        return Err(import_cancelled_error(
            "Workspace manifest discovery was cancelled at a bounded checkpoint.",
        ));
    }
    let discovered = u64::try_from(discovered)
        .map_err(|_| AppError::new("Candidate discovery counter overflowed."))?;
    emit_progress_observation(
        app,
        ProgressObservation {
            processed_units: Some(discovered),
            total_units: None,
            unit_kind: Some(import_progress::ProgressUnitKind::CandidateRecords),
            activity_observed,
            counters: Some(import_progress::ProgressCounters {
                candidates_discovered: Some(discovered),
                ..import_progress::ProgressCounters::default()
            }),
            ..ProgressObservation::default()
        },
    );
    checkpoint_integrity_operation_with_policy(app, activity_observed, honor_cancellation, false)
}

fn checkpoint_integrity_operation(
    app: &tauri::AppHandle,
    activity_observed: bool,
) -> AppResult<()> {
    checkpoint_integrity_operation_with_policy(app, activity_observed, true, false)
}

fn checkpoint_integrity_operation_with_policy(
    app: &tauri::AppHandle,
    activity_observed: bool,
    honor_cancellation: bool,
    force_checkpoint: bool,
) -> AppResult<()> {
    let state = app.state::<AppState>();
    if honor_cancellation && state.cancel_import_requested.load(Ordering::SeqCst) {
        return Err(import_cancelled_error(
            "The workspace operation was cancelled at a bounded checkpoint.",
        ));
    }
    let mut guard = state
        .integrity_operation
        .lock()
        .map_err(|_| AppError::new("Workspace operation scope is unavailable."))?;
    let Some(scope) = guard.as_mut() else {
        return Ok(());
    };
    if scope.persistence_error.is_some() {
        return Err(AppError::new(
            "Import progress timing could not be persisted safely.",
        ));
    }
    let now_tick_ms = scope
        .monotonic_origin
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(import_progress::MAX_SAFE_JSON_INTEGER);
    if activity_observed {
        scope.last_activity_tick_ms = Some(now_tick_ms);
        scope.last_activity_utc = Some(canonical_progress_utc_now());
    }
    if !force_checkpoint && now_tick_ms.saturating_sub(scope.last_checkpoint_tick_ms) < 15_000 {
        return Ok(());
    }
    let record = import_integrity_phase2::read_state(&scope.workspace_path, &scope.workspace_id)?
        .ok_or_else(|| AppError::new("Workspace progress state disappeared."))?;
    if record.operation_generation != scope.generation {
        return Err(AppError::new(
            "Workspace progress operation generation changed unexpectedly.",
        ));
    }
    if record.revision < scope.state_revision {
        return Err(AppError::new("Workspace progress state regressed."));
    }
    let previous_stage = scope.stage;
    record_progress_transition(scope, record.state);
    merge_persisted_completed_stages(
        &mut scope.completed_stages,
        &record,
        scope.operation_kind,
        progress_stage_from_workspace_state(record.state).unwrap_or(scope.stage),
    );
    apply_persisted_progress_correction(scope, &record);
    if let Some(persisted_stage) = progress_stage_from_workspace_state(record.state)
        .filter(|stage| *stage != import_progress::ProgressStage::Complete)
    {
        scope.stage = persisted_stage;
    }
    if scope.stage != previous_stage {
        scope.timing.transition_stage(now_tick_ms);
        scope.eta_samples.clear();
        scope.last_processed_units = None;
    }
    let timing = scope.timing.snapshot(now_tick_ms);
    let saved = import_integrity_phase2::checkpoint_active_session(
        &scope.workspace_path,
        &scope.workspace_id,
        scope.generation,
        record.state,
        timing.active_elapsed_ms,
        timing.stage_elapsed_ms,
        activity_observed,
        false,
        force_checkpoint && scope.updates_import_timing && scope.completion_duration_eligible,
    )?;
    scope.state_revision = saved.revision;
    scope.last_checkpoint_tick_ms = now_tick_ms;
    Ok(())
}

fn emit_activity_only_progress(app: &tauri::AppHandle) -> AppResult<()> {
    emit_activity_only_progress_with_policy(app, true)
}

fn emit_activity_only_progress_with_policy(
    app: &tauri::AppHandle,
    honor_cancellation: bool,
) -> AppResult<()> {
    emit_progress_observation(
        app,
        ProgressObservation {
            activity_observed: true,
            ..ProgressObservation::default()
        },
    );
    checkpoint_integrity_operation_with_policy(app, true, honor_cancellation, false)
}

fn verify_replacement_index_with_operation_progress(
    app: &tauri::AppHandle,
    workspace: &Path,
    workspace_id: &str,
    index_path: &Path,
    honor_cancellation: bool,
) -> AppResult<()> {
    let state = app.state::<AppState>();
    let never_cancel = AtomicBool::new(false);
    let cancel_requested = if honor_cancellation {
        &state.cancel_import_requested
    } else {
        &never_cancel
    };
    import_integrity_phase2::verify_replacement_index_with_progress(
        workspace,
        workspace_id,
        index_path,
        cancel_requested,
        || emit_activity_only_progress_with_policy(app, honor_cancellation),
    )
}

fn verify_staged_replacement_index_with_operation_progress(
    app: &tauri::AppHandle,
    workspace: &Path,
    workspace_id: &str,
    index_path: &Path,
    honor_cancellation: bool,
) -> AppResult<()> {
    let state = app.state::<AppState>();
    let never_cancel = AtomicBool::new(false);
    let cancel_requested = if honor_cancellation {
        &state.cancel_import_requested
    } else {
        &never_cancel
    };
    import_integrity_phase2::verify_staged_replacement_index_with_progress(
        workspace,
        workspace_id,
        index_path,
        cancel_requested,
        || emit_activity_only_progress_with_policy(app, honor_cancellation),
    )
}

fn replacement_index_matches_with_operation_progress(
    app: &tauri::AppHandle,
    workspace: &Path,
    workspace_id: &str,
    index_path: &Path,
    staged: bool,
) -> AppResult<bool> {
    let result = if staged {
        verify_staged_replacement_index_with_operation_progress(
            app,
            workspace,
            workspace_id,
            index_path,
            true,
        )
    } else {
        verify_replacement_index_with_operation_progress(
            app,
            workspace,
            workspace_id,
            index_path,
            true,
        )
    };
    match result {
        Ok(()) => Ok(true),
        Err(error) if error.code == Some(IMPORT_CANCELLED_CODE) => Err(error),
        Err(_) => Ok(false),
    }
}

fn verify_promoted_index_with_optional_progress(
    app: Option<&tauri::AppHandle>,
    workspace: &Path,
    workspace_id: &str,
    index_path: &Path,
    staged: bool,
) -> AppResult<()> {
    if let Some(app) = app {
        if staged {
            return verify_staged_replacement_index_with_operation_progress(
                app,
                workspace,
                workspace_id,
                index_path,
                false,
            );
        }
        return verify_replacement_index_with_operation_progress(
            app,
            workspace,
            workspace_id,
            index_path,
            false,
        );
    }

    let never_cancel = AtomicBool::new(false);
    if staged {
        import_integrity_phase2::verify_staged_replacement_index_with_progress(
            workspace,
            workspace_id,
            index_path,
            &never_cancel,
            || Ok(()),
        )
    } else {
        import_integrity_phase2::verify_replacement_index_with_progress(
            workspace,
            workspace_id,
            index_path,
            &never_cancel,
            || Ok(()),
        )
    }
}

fn snapshot_eml_candidate_with_progress<F>(
    path: &Path,
    mut on_progress: F,
) -> AppResult<FileIdentity>
where
    F: FnMut() -> AppResult<()>,
{
    let (_, identity) = read_eml_candidate_snapshot_with_progress(path, |_| on_progress())?;
    Ok(identity)
}

fn read_eml_candidate_snapshot(path: &Path) -> AppResult<(Vec<u8>, FileIdentity)> {
    read_eml_candidate_snapshot_with_progress(path, |_| Ok(()))
}

fn read_eml_candidate_snapshot_with_progress<F>(
    path: &Path,
    mut on_progress: F,
) -> AppResult<(Vec<u8>, FileIdentity)>
where
    F: FnMut(u64) -> AppResult<()>,
{
    let mut file = open_bounded_message_file(path)?;
    let before = file.metadata()?;
    let mut bytes = Vec::with_capacity((before.len() as usize).min(1024 * 1024));
    let mut buffer = [0_u8; 1024 * 1024];
    let mut next_progress_bytes = DISCOVERY_HASH_PROGRESS_BYTES;
    loop {
        let bytes_read = file.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        let next_len = (bytes.len() as u64)
            .checked_add(bytes_read as u64)
            .ok_or_else(|| AppError::new("An extracted EML candidate exceeds the safety limit."))?;
        if next_len > MAX_MESSAGE_SOURCE_BYTES {
            return Err(AppError::new(
                "An extracted EML candidate exceeds the safety limit.",
            ));
        }
        bytes.extend_from_slice(&buffer[..bytes_read]);
        if next_len >= next_progress_bytes {
            on_progress(next_len)?;
            next_progress_bytes = next_progress_bytes.saturating_add(DISCOVERY_HASH_PROGRESS_BYTES);
        }
    }
    let after = file.metadata()?;
    if before.dev() != after.dev()
        || before.ino() != after.ino()
        || before.nlink() != 1
        || after.nlink() != 1
        || before.len() != after.len()
        || before.mtime() != after.mtime()
        || before.mtime_nsec() != after.mtime_nsec()
    {
        return Err(AppError::new(
            "An extracted EML candidate changed during discovery; indexing was refused.",
        ));
    }
    let identity = FileIdentity {
        device: before.dev(),
        inode: before.ino(),
        size: before.len(),
        modified_seconds: before.mtime(),
        modified_nanoseconds: before.mtime_nsec(),
        sha256: hex::encode(Sha256::digest(&bytes)),
    };
    Ok((bytes, identity))
}

#[cfg(test)]
fn count_eml_files(extract_dir: &Path) -> AppResult<usize> {
    Ok(discover_eml_candidates(extract_dir)?.count)
}

struct IndexProgressThrottle {
    last_emit: Option<Instant>,
    last_processed: usize,
}

impl IndexProgressThrottle {
    fn new() -> Self {
        Self {
            last_emit: None,
            last_processed: 0,
        }
    }

    fn should_emit(&mut self, processed: usize, total: usize) -> bool {
        let now = Instant::now();
        let due_by_count =
            processed.saturating_sub(self.last_processed) >= INDEX_PROGRESS_MESSAGE_INTERVAL;
        let due_by_time = self
            .last_emit
            .map(|last_emit| now.duration_since(last_emit) >= INDEX_PROGRESS_TIME_INTERVAL)
            .unwrap_or(true);
        let due_by_completion = processed == total;

        if due_by_count || due_by_time || due_by_completion {
            self.last_emit = Some(now);
            self.last_processed = processed;
            true
        } else {
            false
        }
    }
}

fn read_import_metadata(conn: &Connection) -> AppResult<HashMap<String, String>> {
    let mut statement = conn.prepare("SELECT key, value FROM import_metadata")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut metadata = HashMap::new();
    for row in rows {
        let (key, value) = row?;
        metadata.insert(key, value);
    }
    Ok(metadata)
}

fn write_import_metadata(conn: &Connection, metadata: &HashMap<String, String>) -> AppResult<()> {
    for (key, value) in metadata {
        conn.execute(
            "INSERT OR REPLACE INTO import_metadata (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
    }
    Ok(())
}

fn write_index_progress_metadata(conn: &Connection, stats: &IndexStats) -> AppResult<()> {
    let canonical_attachment_total =
        conn.query_row("SELECT COUNT(*) FROM attachments", [], |row| {
            row.get::<_, usize>(0)
        })?;
    for (key, value) in [
        ("message_count_indexed", stats.indexed.to_string()),
        ("error_count", stats.error_count.to_string()),
        ("last_error", stats.last_error.clone().unwrap_or_default()),
        ("discovered_messages", stats.discovered.to_string()),
        ("total_discovered_eml_count", stats.discovered.to_string()),
        ("candidates_discovered", stats.discovered.to_string()),
        ("candidates_processed", stats.processed.to_string()),
        ("canonical_messages_inserted", stats.inserted.to_string()),
        (
            "existing_canonical_messages_skipped",
            stats.skipped_existing.to_string(),
        ),
        (
            "structurally_empty_artifacts_suppressed",
            stats.suppressed_empty_artifacts.to_string(),
        ),
        ("parse_failures", stats.error_count.to_string()),
        ("attachments_indexed", stats.attachments_indexed.to_string()),
        (
            "canonical_attachment_total",
            canonical_attachment_total.to_string(),
        ),
        ("canonical_messages_total", stats.indexed.to_string()),
    ] {
        set_metadata_value(conn, key, value)?;
    }
    Ok(())
}

fn finalize_import_index_metadata(conn: &mut Connection, stats: &IndexStats) -> AppResult<()> {
    if stats.suppressed_empty_artifacts != 0 {
        return Err(AppError::new(
            "Artifact suppression is disabled until extractor provenance can prove an auxiliary identity.",
        ));
    }
    let phase2_state_exists = conn.path().is_some_and(|path| {
        Path::new(path).parent().is_some_and(|parent| {
            parent
                .join(import_integrity_phase2::IMPORT_STATE_FILE)
                .is_file()
        })
    });
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let database_total = transaction.query_row("SELECT COUNT(*) FROM messages", [], |row| {
        row.get::<_, usize>(0)
    })?;
    if database_total != stats.indexed {
        return Err(AppError::new(
            "The final canonical message count changed unexpectedly.",
        ));
    }
    let now = Utc::now().to_rfc3339();
    write_index_progress_metadata(&transaction, stats)?;
    set_metadata_value(
        &transaction,
        "import_status",
        if phase2_state_exists {
            import_integrity_phase2::WorkspaceState::Verifying.as_str()
        } else {
            ImportStatus::Complete.as_str()
        },
    )?;
    set_metadata_value(&transaction, "updated_at", &now)?;
    if !phase2_state_exists {
        set_metadata_value(&transaction, "finished_at", &now)?;
    }
    transaction.commit()?;
    Ok(())
}

fn commit_index_batch(
    transaction: Transaction<'_>,
    stats: &mut IndexStats,
    initial_canonical_count: usize,
    require_complete_candidate_set: bool,
) -> AppResult<()> {
    if stats.suppressed_empty_artifacts != 0 {
        return Err(AppError::new(
            "Artifact suppression is disabled until extractor provenance can prove an auxiliary identity.",
        ));
    }
    let database_total = transaction.query_row("SELECT COUNT(*) FROM messages", [], |row| {
        row.get::<_, usize>(0)
    })?;
    let candidate_outcomes = stats
        .inserted
        .checked_add(stats.skipped_existing)
        .and_then(|value| value.checked_add(stats.suppressed_empty_artifacts))
        .ok_or_else(|| AppError::new("Import candidate counters overflowed."))?;
    if stats.processed != candidate_outcomes || stats.processed > stats.discovered {
        return Err(AppError::new(
            "Import candidate outcome counters are inconsistent.",
        ));
    }
    if stats.error_count > database_total {
        return Err(AppError::new(
            "Import parse-failure counters are inconsistent.",
        ));
    }
    let expected_database_total = initial_canonical_count
        .checked_add(stats.inserted)
        .ok_or_else(|| AppError::new("Canonical message counters overflowed."))?;
    if database_total != expected_database_total {
        return Err(AppError::new(
            "The canonical database count changed unexpectedly during indexing.",
        ));
    }
    if require_complete_candidate_set {
        let completed_canonical_outcomes = stats
            .inserted
            .checked_add(stats.skipped_existing)
            .ok_or_else(|| AppError::new("Canonical message counters overflowed."))?;
        if stats.processed != stats.discovered || database_total != completed_canonical_outcomes {
            return Err(AppError::new(
                "The completed canonical index counters are inconsistent.",
            ));
        }
    }
    stats.indexed = database_total;
    write_index_progress_metadata(&transaction, stats)?;
    transaction.commit()?;
    Ok(())
}

fn set_reindex_metadata_values(
    conn: &mut Connection,
    active: &ActiveWorkspace,
    status: ImportStatus,
    stats: Option<&IndexStats>,
    last_error: Option<&str>,
) -> AppResult<()> {
    let now = Utc::now().to_rfc3339();
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (key, value) in [
        ("app_version", env!("CARGO_PKG_VERSION").to_string()),
        ("original_pst_path", active.pst_path.display().to_string()),
        ("pst_path", active.pst_path.display().to_string()),
        ("pst_content_fingerprint", active.fingerprint.clone()),
        ("pst_fingerprint", active.fingerprint.clone()),
        ("workspace_path", active.path.display().to_string()),
        (
            "workspace_location_mode",
            active.location_mode.as_str().to_string(),
        ),
        ("workspace_id", active.id.clone()),
        (
            "canonical_import_identity_version",
            CANONICAL_IMPORT_IDENTITY_VERSION.to_string(),
        ),
        ("importer_generation", IMPORTER_GENERATION.to_string()),
        ("import_status", status.as_str().to_string()),
        ("updated_at", now.clone()),
        ("last_reindex_at", now.clone()),
        ("reindex_source", "existing_emls".to_string()),
        (
            "attachment_metadata_schema_version",
            "attachment-metadata-v1".to_string(),
        ),
        ("body_html_schema_version", "body-html-v1".to_string()),
    ] {
        set_metadata_value(&transaction, key, value)?;
    }

    if matches!(
        status,
        ImportStatus::Complete | ImportStatus::Failed | ImportStatus::Cancelled
    ) {
        set_metadata_value(&transaction, "finished_at", &now)?;
    }
    if let Some(stats) = stats {
        write_index_progress_metadata(&transaction, stats)?;
    }
    if let Some(error) = last_error {
        set_metadata_value(&transaction, "last_error", error)?;
    }

    transaction.commit()?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn stream_index_eml_files(
    app: &tauri::AppHandle,
    workspace: &Path,
    extract_dir: &Path,
    conn: &mut Connection,
    discovery: &EmlDiscovery,
    _progress_message: &str,
    cancel_message: &str,
) -> AppResult<IndexStats> {
    let mut progress_throttle = IndexProgressThrottle::new();
    let state = app.state::<AppState>();

    stream_index_eml_files_with_control(
        workspace,
        extract_dir,
        conn,
        discovery,
        INDEX_BATCH_SIZE,
        cancel_message,
        |_| state.cancel_import_requested.load(Ordering::SeqCst),
        |stats| {
            if progress_throttle.should_emit(stats.processed, stats.discovered) {
                emit_index_progress(app, stats);
            }
        },
    )
}

#[allow(clippy::too_many_arguments)]
fn stream_index_eml_files_with_control<C, P>(
    workspace: &Path,
    extract_dir: &Path,
    conn: &mut Connection,
    discovery: &EmlDiscovery,
    batch_size: usize,
    cancel_message: &str,
    is_cancelled: C,
    on_progress: P,
) -> AppResult<IndexStats>
where
    C: FnMut(&IndexStats) -> bool,
    P: FnMut(&IndexStats),
{
    stream_index_eml_files_with_preferred_ids(
        workspace,
        extract_dir,
        conn,
        discovery,
        batch_size,
        cancel_message,
        is_cancelled,
        on_progress,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn stream_index_eml_files_with_preferred_ids<C, P>(
    workspace: &Path,
    extract_dir: &Path,
    conn: &mut Connection,
    discovery: &EmlDiscovery,
    batch_size: usize,
    cancel_message: &str,
    mut is_cancelled: C,
    mut on_progress: P,
    preferred_ids: Option<&HashMap<String, i64>>,
) -> AppResult<IndexStats>
where
    C: FnMut(&IndexStats) -> bool,
    P: FnMut(&IndexStats),
{
    let mut transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let initial_count = transaction.query_row("SELECT COUNT(*) FROM messages", [], |row| {
        row.get::<_, usize>(0)
    })?;
    let initial_parse_failure_count = transaction.query_row(
        "SELECT COUNT(*) FROM messages WHERE body_source = 'parse_error'",
        [],
        |row| row.get::<_, usize>(0),
    )?;
    let mut folder_cache: HashMap<String, i64> = HashMap::new();
    let mut stats = IndexStats {
        discovered: discovery.count,
        indexed: initial_count,
        error_count: initial_parse_failure_count,
        ..IndexStats::default()
    };
    let mut batch_count = 0usize;
    let batch_size = batch_size.max(1);

    append_import_log(
        workspace,
        &format!(
            "state=indexing total_eml_count={} batch_size={}",
            discovery.count, batch_size
        ),
    )?;

    for candidate in &discovery.candidates {
        if !workspace.is_dir() {
            return Err(AppError::new(
                "Workspace became unavailable during indexing.",
            ));
        }

        if is_cancelled(&stats) {
            append_import_log(workspace, "state=cancelled stage=indexing")?;
            return Err(import_cancelled_error(cancel_message));
        }

        let eml_path = &candidate.path;
        let relative_path = candidate.relative_path.clone();
        if existing_message_id(&transaction, &relative_path)?.is_some() {
            stats.skipped_existing = stats.skipped_existing.saturating_add(1);
        } else {
            let bytes = read_discovered_eml_candidate(candidate)?;
            let parsed = match parse_eml_bytes(&bytes) {
                Ok(parsed) => parsed,
                Err(_) => {
                    stats.error_count = stats.error_count.saturating_add(1);
                    let message = format!(
                        "Message parsing failed during local indexing (error_count={}).",
                        stats.error_count
                    );
                    stats.last_error = Some(message.clone());
                    append_import_log(
                        workspace,
                        &format!("state=indexing parse_failures={}", stats.error_count),
                    )?;
                    ParsedMessage {
                        subject: "(Could not parse message)".to_string(),
                        body: message,
                        body_source: BodySource::ParseError,
                        ..ParsedMessage::default()
                    }
                }
            };

            if parsed.rtf_body_fallback.is_some() {
                append_import_log(workspace, "state=indexing rtf_fallback_increment=1")?;
            }
            let folder_path = eml_path
                .parent()
                .and_then(|parent| parent.strip_prefix(extract_dir).ok())
                .map(path_to_slash_string)
                .unwrap_or_default();
            let folder_id = ensure_folder(&transaction, &mut folder_cache, &folder_path)?;
            let outcome = if let Some(preferred_ids) = preferred_ids {
                insert_message_with_id(
                    &transaction,
                    folder_id,
                    &relative_path,
                    parsed,
                    preferred_ids.get(&relative_path).copied(),
                )?
            } else {
                insert_message(&transaction, folder_id, &relative_path, parsed)?
            };
            match outcome {
                InsertMessageOutcome::Inserted {
                    attachment_count, ..
                } => {
                    stats.inserted = stats.inserted.saturating_add(1);
                    stats.indexed = stats.indexed.saturating_add(1);
                    stats.attachments_indexed =
                        stats.attachments_indexed.saturating_add(attachment_count);
                }
                InsertMessageOutcome::Existing { .. } => {
                    stats.skipped_existing = stats.skipped_existing.saturating_add(1);
                }
            }
        }

        stats.processed = stats.processed.saturating_add(1);
        batch_count = batch_count.saturating_add(1);

        if batch_count >= batch_size {
            commit_index_batch(transaction, &mut stats, initial_count, false)?;
            checkpoint_phase2_index_state(workspace, &stats)?;
            append_import_log(
                workspace,
                &format!(
                    "state=indexing processed={} discovered={} canonical_total={} inserted={} skipped={} suppressed={} parse_failures={} attachments_indexed={}",
                    stats.processed,
                    stats.discovered,
                    stats.indexed,
                    stats.inserted,
                    stats.skipped_existing,
                    stats.suppressed_empty_artifacts,
                    stats.error_count,
                    stats.attachments_indexed
                ),
            )?;
            transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            batch_count = 0;
        }
        on_progress(&stats);
    }

    let final_discovery = discover_eml_candidates_with_progress(extract_dir, |_, _| {
        if is_cancelled(&stats) {
            return Err(import_cancelled_error(cancel_message));
        }
        on_progress(&stats);
        Ok(())
    })?;
    if final_discovery.count != discovery.count
        || final_discovery.identity_manifest != discovery.identity_manifest
    {
        return Err(AppError::new(
            "Extracted message discovery changed during indexing.",
        ));
    }
    commit_index_batch(transaction, &mut stats, initial_count, true)?;
    checkpoint_phase2_index_state(workspace, &stats)?;
    append_import_log(
        workspace,
        &format!(
            "state=indexing_complete processed={} discovered={} canonical_total={} inserted={} skipped={} suppressed={} parse_failures={} attachments_indexed={}",
            stats.processed,
            stats.discovered,
            stats.indexed,
            stats.inserted,
            stats.skipped_existing,
            stats.suppressed_empty_artifacts,
            stats.error_count,
            stats.attachments_indexed
        ),
    )?;

    Ok(stats)
}

fn checkpoint_phase2_index_state(workspace: &Path, stats: &IndexStats) -> AppResult<()> {
    if !workspace
        .join(import_integrity_phase2::IMPORT_STATE_FILE)
        .is_file()
    {
        return Ok(());
    }
    let workspace_id = workspace_id_from_path(workspace)?;
    let Some(state) = import_integrity_phase2::read_state(workspace, &workspace_id)? else {
        return Ok(());
    };
    if state.state != import_integrity_phase2::WorkspaceState::Indexing {
        return Err(AppError::new(
            "Workspace state changed unexpectedly during indexing.",
        ));
    }
    import_integrity_phase2::transition_state(
        workspace,
        &workspace_id,
        import_integrity_phase2::WorkspaceState::Indexing,
        import_integrity_phase2::StateCounts {
            discovered_candidates: Some(stats.discovered),
            canonical_indexed: Some(stats.indexed),
        },
        Some("indexing_progress"),
    )?;
    Ok(())
}

fn reindex_eml_files_to_index(
    app: &tauri::AppHandle,
    active: &ActiveWorkspace,
    extract_dir: &Path,
    index_path: &Path,
    existing_metadata: HashMap<String, String>,
    stable_ids: &HashMap<String, i64>,
) -> AppResult<IndexStats> {
    emit_progress(
        app,
        "Scanning extracted EML files",
        None,
        None,
        "Scanning existing extracted EML files.",
    );

    let discovery = discover_eml_candidates_with_progress(extract_dir, |discovered, activity| {
        emit_discovery_progress(app, discovered, activity)
    })?;
    let total_eml_count = discovery.count;
    import_integrity_phase2::transition_state(
        &active.path,
        &active.id,
        import_integrity_phase2::WorkspaceState::Indexing,
        import_integrity_phase2::StateCounts {
            discovered_candidates: Some(total_eml_count),
            canonical_indexed: Some(0),
        },
        Some("repair_indexing"),
    )?;
    let mut next_new_id = stable_ids
        .values()
        .copied()
        .max()
        .unwrap_or(0)
        .checked_add(1)
        .ok_or_else(|| AppError::new("Stable message identity range is exhausted."))?;
    let mut replacement_ids = HashMap::with_capacity(discovery.count);
    for candidate in &discovery.candidates {
        let id = if let Some(id) = stable_ids.get(&candidate.relative_path) {
            *id
        } else {
            let id = next_new_id;
            next_new_id = next_new_id
                .checked_add(1)
                .ok_or_else(|| AppError::new("Stable message identity range is exhausted."))?;
            id
        };
        if id <= 0
            || replacement_ids
                .insert(candidate.relative_path.clone(), id)
                .is_some()
        {
            return Err(AppError::new("Stable message identity mapping is invalid."));
        }
    }
    if replacement_ids
        .values()
        .copied()
        .collect::<HashSet<_>>()
        .len()
        != replacement_ids.len()
    {
        return Err(AppError::new(
            "Stable message identity mapping contains a collision.",
        ));
    }

    let mut conn = open_sqlite_write_connection(index_path)?;
    initialize_schema(&conn)?;
    write_import_metadata(&conn, &existing_metadata)?;
    let initial_stats = IndexStats {
        discovered: total_eml_count,
        ..IndexStats::default()
    };
    emit_index_progress(app, &initial_stats);
    set_reindex_metadata_values(
        &mut conn,
        active,
        ImportStatus::Running,
        Some(&initial_stats),
        None,
    )?;
    set_metadata_value(
        &conn,
        "candidate_identity_manifest",
        &discovery.identity_manifest,
    )?;

    let mut progress_throttle = IndexProgressThrottle::new();
    let state = app.state::<AppState>();
    let stats = stream_index_eml_files_with_preferred_ids(
        &active.path,
        extract_dir,
        &mut conn,
        &discovery,
        INDEX_BATCH_SIZE,
        "Reindex cancelled. Stopping EML indexing.",
        |_| state.cancel_import_requested.load(Ordering::SeqCst),
        |stats| {
            if progress_throttle.should_emit(stats.processed, stats.discovered) {
                emit_index_progress(app, stats);
            }
        },
        Some(&replacement_ids),
    )?;
    emit_index_progress(app, &stats);
    import_integrity_phase2::transition_state(
        &active.path,
        &active.id,
        import_integrity_phase2::WorkspaceState::BuildingConversations,
        import_integrity_phase2::StateCounts {
            discovered_candidates: Some(stats.discovered),
            canonical_indexed: Some(stats.indexed),
        },
        Some("repair_building_conversations"),
    )?;
    build_conversation_index(app, &active.path, &mut conn, &active.id)?;
    set_reindex_metadata_values(&mut conn, active, ImportStatus::Running, Some(&stats), None)?;
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    set_metadata_value(
        &transaction,
        "import_status",
        import_integrity_phase2::WorkspaceState::Verifying.as_str(),
    )?;
    set_metadata_value(&transaction, "updated_at", Utc::now().to_rfc3339())?;
    transaction.commit()?;
    optimize_then_checked_truncate(&conn)?;
    drop(conn);

    Ok(stats)
}

struct IndexEmlFiles<'a> {
    app: &'a tauri::AppHandle,
    identity: &'a PstIdentity,
    workspace_id: &'a str,
    extract_dir: &'a Path,
    index_path: &'a Path,
    workspace: &'a Path,
    workspace_mode: WorkspaceLocationMode,
    readpst_path: &'a str,
    readpst_version: &'a str,
    resume_existing: bool,
}

fn index_eml_files(input: IndexEmlFiles<'_>) -> AppResult<IndexStats> {
    let IndexEmlFiles {
        app,
        identity,
        workspace_id,
        extract_dir,
        index_path,
        workspace,
        workspace_mode,
        readpst_path,
        readpst_version,
        resume_existing,
    } = input;
    emit_progress(
        app,
        "Scanning extracted EML files",
        None,
        None,
        "Scanning extracted EML files.",
    );

    let discovery = discover_eml_candidates_with_progress(extract_dir, |discovered, activity| {
        emit_discovery_progress(app, discovered, activity)
    })?;
    let total_eml_count = discovery.count;
    transition_phase2_if_present(
        workspace,
        workspace_id,
        import_integrity_phase2::WorkspaceState::Indexing,
        StateCountsFromStats {
            discovered: total_eml_count,
            canonical: read_index_progress_counts(index_path).0.unwrap_or(0),
        },
        "indexing_running",
    )?;

    emit_index_progress(
        app,
        &IndexStats {
            discovered: total_eml_count,
            ..IndexStats::default()
        },
    );

    let initialize_import_metadata = match fs::symlink_metadata(index_path) {
        Ok(_) if resume_existing => matches!(
            preflight_resume_index(index_path, identity, workspace_id, extract_dir, &discovery)?,
            ResumeIndexPreflight::Pristine
        ),
        Ok(_) => {
            return Err(AppError::new(
                "Refusing to replace an unexpected existing partial index.",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => return Err(AppError::new("Workspace index is unavailable.")),
    };

    let mut conn = open_sqlite_write_connection(index_path)?;
    initialize_schema(&conn)?;
    prepare_import_metadata(
        &mut conn,
        identity,
        workspace_id,
        total_eml_count,
        workspace,
        workspace_mode,
        readpst_path,
        readpst_version,
        resume_existing,
        initialize_import_metadata,
        &discovery,
    )?;

    let stats = stream_index_eml_files(
        app,
        workspace,
        extract_dir,
        &mut conn,
        &discovery,
        "Creating the local SQLite index.",
        "Import cancelled. Stopping message indexing.",
    )?;
    emit_index_progress(app, &stats);
    transition_phase2_if_present(
        workspace,
        workspace_id,
        import_integrity_phase2::WorkspaceState::BuildingConversations,
        StateCountsFromStats {
            discovered: stats.discovered,
            canonical: stats.indexed,
        },
        "building_conversations",
    )?;
    build_conversation_index(app, workspace, &mut conn, workspace_id)?;
    finalize_import_index_metadata(&mut conn, &stats)?;
    optimize_then_checked_truncate(&conn)?;
    if workspace
        .join(import_integrity_phase2::IMPORT_MANIFEST_FILE)
        .is_file()
    {
        if index_path == workspace.join("index.sqlite") {
            import_integrity_phase2::update_manifest_from_index(
                workspace,
                workspace_id,
                index_path,
                &stats,
                "complete",
            )?;
        } else {
            import_integrity_phase2::stage_manifest_from_index(
                workspace,
                workspace_id,
                index_path,
                &stats,
                "complete",
            )?;
        }
        transition_phase2_if_present(
            workspace,
            workspace_id,
            import_integrity_phase2::WorkspaceState::Verifying,
            StateCountsFromStats {
                discovered: stats.discovered,
                canonical: stats.indexed,
            },
            "verification_pending",
        )?;
    }

    Ok(stats)
}

struct StateCountsFromStats {
    discovered: usize,
    canonical: usize,
}

fn transition_phase2_if_present(
    workspace: &Path,
    workspace_id: &str,
    state: import_integrity_phase2::WorkspaceState,
    counts: StateCountsFromStats,
    reason: &str,
) -> AppResult<()> {
    if workspace
        .join(import_integrity_phase2::IMPORT_STATE_FILE)
        .is_file()
    {
        import_integrity_phase2::transition_state(
            workspace,
            workspace_id,
            state,
            import_integrity_phase2::StateCounts {
                discovered_candidates: Some(counts.discovered),
                canonical_indexed: Some(counts.canonical),
            },
            Some(reason),
        )?;
    }
    Ok(())
}

fn initialize_schema(conn: &Connection) -> AppResult<()> {
    configure_sqlite_connection(conn, false)?;
    let version = read_schema_version(conn)?;
    if version > SQLITE_SCHEMA_VERSION_CURRENT {
        return Err(AppError::new(format!(
            "Workspace index schema version {version} is newer than this version of PST QuickView supports."
        )));
    }
    verify_no_schema_programs(conn)?;
    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         PRAGMA journal_mode = WAL;",
    )?;
    migrate_schema(conn, schema_indexes()).map_err(|error| {
        let database_path = conn.path().unwrap_or("(temporary SQLite database)");
        AppError::new(format!(
            "Could not migrate PST QuickView workspace index at {database_path}: {error}. The workspace was not deleted and the source PST was not modified."
        ))
    })
}

fn create_base_schema(conn: &Connection) -> AppResult<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS import_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS folders (
            id INTEGER PRIMARY KEY,
            parent_id INTEGER REFERENCES folders(id) ON DELETE CASCADE,
            path TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS messages (
            id INTEGER PRIMARY KEY,
            folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
            eml_path TEXT NOT NULL UNIQUE,
            subject TEXT,
            sender TEXT,
            recipients TEXT,
            date TEXT,
            body TEXT,
            body_source TEXT NOT NULL DEFAULT 'missing',
            body_html TEXT,
            snippet TEXT,
            attachment_names TEXT,
            has_attachments INTEGER NOT NULL DEFAULT 0,
            message_id_header_raw TEXT,
            message_id_header TEXT,
            in_reply_to_raw TEXT,
            in_reply_to TEXT,
            references_header_raw TEXT,
            references_header TEXT,
            normalized_subject TEXT,
            conversation_id TEXT,
            conversation_parent_id INTEGER REFERENCES messages(id),
            conversation_root_id INTEGER REFERENCES messages(id),
            thread_assignment_method TEXT NOT NULL DEFAULT 'standalone',
            thread_warning TEXT,
            imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS attachments (
            id INTEGER PRIMARY KEY,
            message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
            filename TEXT NOT NULL,
            sanitized_filename TEXT NOT NULL,
            content_type TEXT,
            size_bytes INTEGER,
            attachment_index INTEGER NOT NULL DEFAULT 0,
            content_disposition TEXT,
            mime_part_path TEXT
        );
        ",
    )?;
    Ok(())
}

#[derive(Clone, Copy)]
struct SchemaIndex {
    name: &'static str,
    table: &'static str,
    columns: &'static [&'static str],
    sql: &'static str,
}

fn schema_indexes() -> &'static [SchemaIndex] {
    &[
        SchemaIndex {
            name: "idx_messages_folder_date",
            table: "messages",
            columns: &["folder_id", "date"],
            sql: "CREATE INDEX IF NOT EXISTS idx_messages_folder_date ON messages(folder_id, date)",
        },
        SchemaIndex {
            name: "idx_messages_folder_id",
            table: "messages",
            columns: &["folder_id"],
            sql: "CREATE INDEX IF NOT EXISTS idx_messages_folder_id ON messages(folder_id)",
        },
        SchemaIndex {
            name: "idx_messages_date",
            table: "messages",
            columns: &["date"],
            sql: "CREATE INDEX IF NOT EXISTS idx_messages_date ON messages(date)",
        },
        SchemaIndex {
            name: "idx_messages_sender_nocase",
            table: "messages",
            columns: &["sender"],
            sql: "CREATE INDEX IF NOT EXISTS idx_messages_sender_nocase ON messages(sender COLLATE NOCASE)",
        },
        SchemaIndex {
            name: "idx_messages_subject_nocase",
            table: "messages",
            columns: &["subject"],
            sql: "CREATE INDEX IF NOT EXISTS idx_messages_subject_nocase ON messages(subject COLLATE NOCASE)",
        },
        SchemaIndex {
            name: "idx_messages_has_attachments",
            table: "messages",
            columns: &["has_attachments"],
            sql: "CREATE INDEX IF NOT EXISTS idx_messages_has_attachments ON messages(has_attachments)",
        },
        SchemaIndex {
            name: "idx_messages_message_id",
            table: "messages",
            columns: &["message_id_header"],
            sql: "CREATE INDEX IF NOT EXISTS idx_messages_message_id ON messages(message_id_header COLLATE NOCASE)",
        },
        SchemaIndex {
            name: "idx_messages_conversation_id",
            table: "messages",
            columns: &["conversation_id"],
            sql: "CREATE INDEX IF NOT EXISTS idx_messages_conversation_id ON messages(conversation_id)",
        },
        SchemaIndex {
            name: "idx_messages_conversation_date",
            table: "messages",
            columns: &["conversation_id", "date"],
            sql: "CREATE INDEX IF NOT EXISTS idx_messages_conversation_date ON messages(conversation_id, date)",
        },
        SchemaIndex {
            name: "idx_messages_folder_conversation",
            table: "messages",
            columns: &["folder_id", "conversation_id"],
            sql: "CREATE INDEX IF NOT EXISTS idx_messages_folder_conversation ON messages(folder_id, conversation_id)",
        },
        SchemaIndex {
            name: "idx_messages_normalized_subject",
            table: "messages",
            columns: &["normalized_subject"],
            sql: "CREATE INDEX IF NOT EXISTS idx_messages_normalized_subject ON messages(normalized_subject COLLATE NOCASE)",
        },
        SchemaIndex {
            name: "idx_attachments_message_id",
            table: "attachments",
            columns: &["message_id"],
            sql: "CREATE INDEX IF NOT EXISTS idx_attachments_message_id ON attachments(message_id)",
        },
        SchemaIndex {
            name: "idx_attachments_filename",
            table: "attachments",
            columns: &["filename"],
            sql: "CREATE INDEX IF NOT EXISTS idx_attachments_filename ON attachments(filename)",
        },
        SchemaIndex {
            name: "idx_attachments_sanitized_filename",
            table: "attachments",
            columns: &["sanitized_filename"],
            sql: "CREATE INDEX IF NOT EXISTS idx_attachments_sanitized_filename ON attachments(sanitized_filename)",
        },
    ]
}

fn migrate_schema(conn: &Connection, indexes: &[SchemaIndex]) -> AppResult<()> {
    let previous_version = read_schema_version(conn)?;
    if previous_version > SQLITE_SCHEMA_VERSION_CURRENT {
        return Err(AppError::new(format!(
            "Workspace index schema version {previous_version} is newer than this version of PST QuickView supports."
        )));
    }
    conn.execute_batch("BEGIN IMMEDIATE")?;

    let migration = (|| -> AppResult<()> {
        schema_step("create base tables", || create_base_schema(conn))?;
        schema_step("add message attachment columns", || {
            add_column_if_missing(
                conn,
                "messages",
                "attachment_names",
                "ALTER TABLE messages ADD COLUMN attachment_names TEXT",
            )
        })?;
        schema_step("add message body columns", || {
            ensure_message_body_schema(conn)
        })?;
        schema_step("add Conversation View columns", || {
            ensure_threading_schema(conn)
        })?;
        schema_step("add attachment metadata columns", || {
            ensure_attachment_schema(conn)
        })?;
        schema_step("verify migrated columns", || verify_schema(conn))?;
        schema_step("migrate full-text search schema", || {
            ensure_fts_schema(conn)
        })?;
        schema_step("migrate attachment metadata", || {
            migrate_attachment_metadata_if_needed(conn)
        })?;
        schema_step("create schema indexes", || {
            create_schema_indexes(conn, indexes)
        })?;
        schema_step("commit schema version", || {
            conn.execute_batch(&format!(
                "PRAGMA user_version = {SQLITE_SCHEMA_VERSION_CURRENT};"
            ))?;
            Ok(())
        })?;
        Ok(())
    })();

    match migration {
        Ok(()) => conn.execute_batch("COMMIT").map_err(|error| {
            let _ = conn.execute_batch("ROLLBACK");
            AppError::new(format!(
                "Schema migration commit failed from version {previous_version}: {error}"
            ))
        }),
        Err(error) => {
            let _ = conn.execute_batch("ROLLBACK");
            Err(AppError::new(format!(
                "Schema migration failed from version {previous_version}: {error}"
            )))
        }
    }
}

fn schema_step(step: &str, operation: impl FnOnce() -> AppResult<()>) -> AppResult<()> {
    operation().map_err(|error| AppError::new(format!("{step}: {error}")))
}

fn read_schema_version(conn: &Connection) -> AppResult<i64> {
    Ok(conn.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

fn verify_schema(conn: &Connection) -> AppResult<()> {
    verify_required_columns(
        conn,
        "messages",
        &[
            "id",
            "folder_id",
            "date",
            "sender",
            "subject",
            "body_source",
            "body_html",
            "attachment_names",
            "has_attachments",
            "message_id_header_raw",
            "message_id_header",
            "in_reply_to_raw",
            "in_reply_to",
            "references_header_raw",
            "references_header",
            "normalized_subject",
            "conversation_id",
            "conversation_parent_id",
            "conversation_root_id",
            "thread_assignment_method",
            "thread_warning",
        ],
    )?;
    verify_required_columns(
        conn,
        "attachments",
        &[
            "id",
            "message_id",
            "filename",
            "sanitized_filename",
            "content_type",
            "size_bytes",
            "attachment_index",
            "content_disposition",
            "mime_part_path",
        ],
    )?;
    if !has_unique_message_source_identity(conn)? {
        return Err(AppError::new(
            "messages.eml_path is not uniquely constrained; the workspace cannot be resumed safely.",
        ));
    }
    Ok(())
}

fn has_unique_message_source_identity(conn: &Connection) -> AppResult<bool> {
    has_single_nonpartial_binary_unique_key(conn, "messages", "eml_path")
}

fn has_valid_import_metadata_key(conn: &Connection) -> AppResult<bool> {
    let column_definition = |column: &str| -> AppResult<Option<(String, i64, i64)>> {
        Ok(conn
            .query_row(
                "SELECT UPPER(type), \"notnull\", pk
                   FROM pragma_table_info('import_metadata')
                  WHERE name = ?1",
                params![column],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?)
    };
    let Some((key_type, _, key_primary_key)) = column_definition("key")? else {
        return Ok(false);
    };
    let Some((value_type, value_not_null, value_primary_key)) = column_definition("value")? else {
        return Ok(false);
    };
    if key_type != "TEXT"
        || key_primary_key != 1
        || value_type != "TEXT"
        || value_not_null != 1
        || value_primary_key != 0
        || !has_single_nonpartial_binary_unique_key(conn, "import_metadata", "key")?
    {
        return Ok(false);
    }
    let invalid_key_count = conn.query_row(
        "SELECT COUNT(*) - COUNT(DISTINCT \"key\") FROM import_metadata",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(invalid_key_count == 0)
}

fn has_single_nonpartial_binary_unique_key(
    conn: &Connection,
    table: &str,
    key: &str,
) -> AppResult<bool> {
    let mut statement = conn.prepare(
        "SELECT name
           FROM pragma_index_list(?1)
          WHERE \"unique\" = 1
            AND partial = 0
          ORDER BY seq",
    )?;
    let rows = statement.query_map(params![table], |row| row.get::<_, String>(0))?;
    let indexes = collect_rows(rows)?;
    drop(statement);

    for index in indexes {
        let mut columns = conn.prepare(
            "SELECT COALESCE(name, ''), COALESCE(coll, '')
               FROM pragma_index_xinfo(?1)
              WHERE key = 1
              ORDER BY seqno",
        )?;
        let rows = columns.query_map(params![index], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        if collect_rows(rows)? == [(key.to_string(), "BINARY".to_string())] {
            return Ok(true);
        }
    }
    Ok(false)
}

fn verify_required_columns(
    conn: &Connection,
    table: &str,
    required_columns: &[&str],
) -> AppResult<()> {
    let missing = missing_columns(conn, table, required_columns)?;
    if missing.is_empty() {
        return Ok(());
    }
    Err(AppError::new(format!(
        "table {table} is missing required column(s): {}",
        missing.join(", ")
    )))
}

fn create_schema_indexes(conn: &Connection, indexes: &[SchemaIndex]) -> AppResult<()> {
    for index in indexes {
        let missing = missing_columns(conn, index.table, index.columns)?;
        if !missing.is_empty() {
            return Err(AppError::new(format!(
                "index {} on table {} requires missing column(s): {}",
                index.name,
                index.table,
                missing.join(", ")
            )));
        }
        conn.execute(index.sql, []).map_err(|error| {
            AppError::new(format!(
                "failed to create index {} on table {}: {}",
                index.name, index.table, error
            ))
        })?;
    }
    Ok(())
}

fn ensure_threading_schema(conn: &Connection) -> AppResult<()> {
    for (column, sql) in [
        (
            "message_id_header_raw",
            "ALTER TABLE messages ADD COLUMN message_id_header_raw TEXT",
        ),
        (
            "message_id_header",
            "ALTER TABLE messages ADD COLUMN message_id_header TEXT",
        ),
        (
            "in_reply_to_raw",
            "ALTER TABLE messages ADD COLUMN in_reply_to_raw TEXT",
        ),
        (
            "in_reply_to",
            "ALTER TABLE messages ADD COLUMN in_reply_to TEXT",
        ),
        (
            "references_header_raw",
            "ALTER TABLE messages ADD COLUMN references_header_raw TEXT",
        ),
        (
            "references_header",
            "ALTER TABLE messages ADD COLUMN references_header TEXT",
        ),
        (
            "normalized_subject",
            "ALTER TABLE messages ADD COLUMN normalized_subject TEXT",
        ),
        (
            "conversation_id",
            "ALTER TABLE messages ADD COLUMN conversation_id TEXT",
        ),
        (
            "conversation_parent_id",
            "ALTER TABLE messages ADD COLUMN conversation_parent_id INTEGER REFERENCES messages(id)",
        ),
        (
            "conversation_root_id",
            "ALTER TABLE messages ADD COLUMN conversation_root_id INTEGER REFERENCES messages(id)",
        ),
        (
            "thread_assignment_method",
            "ALTER TABLE messages ADD COLUMN thread_assignment_method TEXT NOT NULL DEFAULT 'standalone'",
        ),
        (
            "thread_warning",
            "ALTER TABLE messages ADD COLUMN thread_warning TEXT",
        ),
    ] {
        add_column_if_missing(conn, "messages", column, sql)?;
    }

    Ok(())
}

fn ensure_message_body_schema(conn: &Connection) -> AppResult<()> {
    add_column_if_missing(
        conn,
        "messages",
        "body_source",
        "ALTER TABLE messages ADD COLUMN body_source TEXT NOT NULL DEFAULT 'missing'",
    )?;
    add_column_if_missing(
        conn,
        "messages",
        "body_html",
        "ALTER TABLE messages ADD COLUMN body_html TEXT",
    )?;

    conn.execute(
        "UPDATE messages
            SET body_source = CASE
                WHEN body IS NULL OR TRIM(body) = '' THEN 'missing'
                ELSE 'text_plain'
            END
          WHERE body_source IS NULL
             OR body_source = ''
             OR body_source = 'missing'",
        [],
    )?;

    Ok(())
}

fn ensure_attachment_schema(conn: &Connection) -> AppResult<()> {
    add_column_if_missing(
        conn,
        "attachments",
        "filename",
        "ALTER TABLE attachments ADD COLUMN filename TEXT",
    )?;
    add_column_if_missing(
        conn,
        "attachments",
        "sanitized_filename",
        "ALTER TABLE attachments ADD COLUMN sanitized_filename TEXT",
    )?;
    add_column_if_missing(
        conn,
        "attachments",
        "size_bytes",
        "ALTER TABLE attachments ADD COLUMN size_bytes INTEGER",
    )?;
    add_column_if_missing(
        conn,
        "attachments",
        "attachment_index",
        "ALTER TABLE attachments ADD COLUMN attachment_index INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(
        conn,
        "attachments",
        "content_disposition",
        "ALTER TABLE attachments ADD COLUMN content_disposition TEXT",
    )?;
    add_column_if_missing(
        conn,
        "attachments",
        "mime_part_path",
        "ALTER TABLE attachments ADD COLUMN mime_part_path TEXT",
    )?;

    Ok(())
}

fn migrate_attachment_metadata_if_needed(conn: &Connection) -> AppResult<()> {
    if metadata_value(conn, "attachment_metadata_schema_version")?.as_deref()
        == Some("attachment-metadata-v1")
    {
        return Ok(());
    }

    if column_exists(conn, "attachments", "file_name")? {
        conn.execute(
            "UPDATE attachments
                SET filename = COALESCE(NULLIF(filename, ''), file_name)
              WHERE filename IS NULL OR filename = ''",
            [],
        )?;
    }

    if column_exists(conn, "attachments", "size")? {
        conn.execute(
            "UPDATE attachments
                SET size_bytes = COALESCE(size_bytes, size)
              WHERE size_bytes IS NULL",
            [],
        )?;
    }

    backfill_sanitized_attachment_filenames(conn)?;
    refresh_message_attachment_names(conn)?;
    conn.execute(
        "INSERT INTO messages_fts(messages_fts) VALUES('rebuild')",
        [],
    )?;
    set_metadata_value(
        conn,
        "attachment_metadata_schema_version",
        "attachment-metadata-v1",
    )?;
    Ok(())
}

fn backfill_sanitized_attachment_filenames(conn: &Connection) -> AppResult<()> {
    let mut statement = conn.prepare(
        "SELECT id, COALESCE(NULLIF(filename, ''), 'attachment'), sanitized_filename
           FROM attachments
          WHERE sanitized_filename IS NULL
             OR sanitized_filename = ''
             OR filename IS NULL
             OR filename = ''",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        ))
    })?;
    let items = collect_rows(rows)?;
    drop(statement);

    for (id, filename, sanitized) in items {
        let next_filename = if filename.trim().is_empty() {
            "attachment".to_string()
        } else {
            filename
        };
        let next_sanitized = if sanitized.trim().is_empty() {
            sanitize_attachment_filename(&next_filename)
        } else {
            sanitize_attachment_filename(&sanitized)
        };
        conn.execute(
            "UPDATE attachments
                SET filename = ?1,
                    sanitized_filename = ?2
              WHERE id = ?3",
            params![next_filename, next_sanitized, id],
        )?;
    }

    Ok(())
}

fn refresh_message_attachment_names(conn: &Connection) -> AppResult<()> {
    conn.execute(
        "UPDATE messages
            SET attachment_names = COALESCE((
                SELECT GROUP_CONCAT(
                    TRIM(
                        COALESCE(filename, '') || ' ' ||
                        COALESCE(sanitized_filename, '') || ' ' ||
                        COALESCE(content_type, '')
                    ),
                    ' '
                )
                  FROM attachments
                 WHERE attachments.message_id = messages.id
            ), ''),
                has_attachments = CASE
                    WHEN EXISTS (
                        SELECT 1
                          FROM attachments
                         WHERE attachments.message_id = messages.id
                    )
                    THEN 1
                    ELSE 0
                END",
        [],
    )?;
    Ok(())
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    alter_sql: &str,
) -> AppResult<()> {
    if !column_exists(conn, table, column)? {
        conn.execute(alter_sql, [])?;
    }
    Ok(())
}

fn column_exists(conn: &Connection, table: &str, column: &str) -> AppResult<bool> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = statement.query_map([], |row| row.get::<_, String>(1))?;

    for name in rows {
        if name? == column {
            return Ok(true);
        }
    }

    Ok(false)
}

fn missing_columns(
    conn: &Connection,
    table: &str,
    required_columns: &[&str],
) -> AppResult<Vec<String>> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let rows = statement.query_map([], |row| row.get::<_, String>(1))?;
    let existing = collect_rows(rows)?.into_iter().collect::<HashSet<_>>();
    Ok(required_columns
        .iter()
        .filter(|column| !existing.contains(**column))
        .map(|column| (*column).to_string())
        .collect())
}

fn ensure_fts_schema(conn: &Connection) -> AppResult<()> {
    if column_exists(conn, "messages_fts", "attachment_names")? {
        return Ok(());
    }

    conn.execute("DROP TABLE IF EXISTS messages_fts", [])?;
    conn.execute(
        "CREATE VIRTUAL TABLE messages_fts
         USING fts5(subject, sender, recipients, body, attachment_names, content='messages', content_rowid='id')",
        [],
    )?;
    conn.execute(
        "INSERT INTO messages_fts (rowid, subject, sender, recipients, body, attachment_names)
         SELECT id, subject, sender, recipients, body, COALESCE(attachment_names, '')
           FROM messages",
        [],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn insert_import_metadata(
    conn: &mut Connection,
    identity: &PstIdentity,
    workspace_id: &str,
    discovered_messages: usize,
    candidate_identity_manifest: &str,
    workspace: &Path,
    workspace_mode: WorkspaceLocationMode,
    readpst_path: &str,
    readpst_version: &str,
) -> AppResult<()> {
    let now = Utc::now().to_rfc3339();
    let values = [
        ("app_version", env!("CARGO_PKG_VERSION").to_string()),
        ("original_pst_path", identity.display_path.clone()),
        ("pst_path", identity.display_path.clone()),
        ("pst_size", identity.size.to_string()),
        ("pst_modified_ns", identity.modified_ns.to_string()),
        ("pst_fingerprint", identity.fingerprint.clone()),
        (
            "pst_content_fingerprint",
            identity.content_fingerprint.clone(),
        ),
        (
            "pst_fingerprint_strategy",
            identity.fingerprint_strategy.clone(),
        ),
        ("workspace_path", workspace.display().to_string()),
        (
            "workspace_location_mode",
            workspace_mode.as_str().to_string(),
        ),
        ("workspace_id", workspace_id.to_string()),
        (
            "canonical_import_identity_version",
            CANONICAL_IMPORT_IDENTITY_VERSION.to_string(),
        ),
        ("importer_generation", IMPORTER_GENERATION.to_string()),
        ("import_status", ImportStatus::Running.as_str().to_string()),
        ("discovered_messages", discovered_messages.to_string()),
        (
            "total_discovered_eml_count",
            discovered_messages.to_string(),
        ),
        (
            "candidate_identity_manifest",
            candidate_identity_manifest.to_string(),
        ),
        ("message_count_indexed", "0".to_string()),
        ("candidates_discovered", discovered_messages.to_string()),
        ("candidates_processed", "0".to_string()),
        ("canonical_messages_inserted", "0".to_string()),
        ("existing_canonical_messages_skipped", "0".to_string()),
        ("structurally_empty_artifacts_suppressed", "0".to_string()),
        ("parse_failures", "0".to_string()),
        ("attachments_indexed", "0".to_string()),
        ("canonical_attachment_total", "0".to_string()),
        ("canonical_messages_total", "0".to_string()),
        ("error_count", "0".to_string()),
        ("last_error", String::new()),
        ("readpst_path", readpst_path.to_string()),
        ("readpst_version", readpst_version.to_string()),
        ("body_html_schema_version", "body-html-v1".to_string()),
        ("started_at", now.clone()),
        ("finished_at", String::new()),
        ("imported_at", now),
    ];

    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    for (key, value) in values {
        transaction.execute(
            "INSERT OR REPLACE INTO import_metadata (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn validate_resume_metadata(
    conn: &Connection,
    identity: &PstIdentity,
    workspace_id: &str,
    discovery: &EmlDiscovery,
) -> AppResult<()> {
    let stored_workspace_id = metadata_value(conn, "workspace_id")?;
    if !stored_workspace_id
        .as_deref()
        .is_some_and(|stored| resume_workspace_id_is_compatible(identity, workspace_id, stored))
    {
        return Err(AppError::new(
            "The partial index belongs to a different workspace; resume was refused.",
        ));
    }
    if metadata_value(conn, "pst_content_fingerprint")?.as_deref()
        != Some(identity.content_fingerprint.as_str())
    {
        return Err(AppError::new(
            "The partial index source fingerprint does not match; resume was refused.",
        ));
    }
    let stored_discovered = metadata_value(conn, "total_discovered_eml_count")?
        .or_else(|| metadata_value(conn, "discovered_messages").ok().flatten())
        .and_then(|value| value.parse::<usize>().ok());
    if stored_discovered != Some(discovery.count) {
        return Err(AppError::new(
            "The extracted message candidate set changed; resume was refused.",
        ));
    }
    let stored_identity_version = metadata_value(conn, "canonical_import_identity_version")?;
    let stored_importer_generation = metadata_value(conn, "importer_generation")?;
    let stored_manifest = metadata_value(conn, "candidate_identity_manifest")?;
    match (
        stored_identity_version.as_deref(),
        stored_importer_generation.as_deref(),
        stored_manifest.as_deref(),
    ) {
        (Some(identity_version), Some(importer_generation), Some(manifest)) => {
            if identity_version != CANONICAL_IMPORT_IDENTITY_VERSION {
                return Err(AppError::new(
                    "The partial index uses an unsupported canonical import identity.",
                ));
            }
            if importer_generation != IMPORTER_GENERATION {
                return Err(AppError::new(
                    "The partial index was created by a different importer generation.",
                ));
            }
            if manifest != discovery.identity_manifest {
                return Err(AppError::new(
                    "The extracted message candidate identities changed; resume was refused.",
                ));
            }
        }
        (None, None, None) => {
            // Compatibility for a pre-Phase-1 schema-v3 index. Its workspace/source
            // metadata, candidate count, unique canonical key, and exact existing
            // identity subset are all validated read-only before the current resume
            // engine adopts these additive provenance keys.
        }
        _ => {
            return Err(AppError::new(
                "The partial index is missing required import identity provenance; resume was refused.",
            ));
        }
    }
    Ok(())
}

fn resume_workspace_id_is_compatible(
    identity: &PstIdentity,
    selected_workspace_id: &str,
    stored_workspace_id: &str,
) -> bool {
    stored_workspace_id == selected_workspace_id
        || (selected_workspace_id == identity.legacy_workspace_id
            && stored_workspace_id == identity.workspace_id)
}

fn validate_existing_message_identity_subset(
    conn: &Connection,
    discovery: &EmlDiscovery,
) -> AppResult<()> {
    let existing_count = conn.query_row("SELECT COUNT(*) FROM messages", [], |row| {
        row.get::<_, usize>(0)
    })?;
    let mut matching_count = 0usize;
    let mut statement = conn.prepare("SELECT 1 FROM messages WHERE eml_path = ?1 LIMIT 1")?;
    for candidate in &discovery.candidates {
        if statement
            .query_row(params![candidate.relative_path], |_| Ok(()))
            .optional()?
            .is_some()
        {
            matching_count = matching_count.saturating_add(1);
        }
    }
    if matching_count != existing_count {
        return Err(AppError::new(
            "The index contains canonical identities that are absent from the extracted candidate set; resume was refused.",
        ));
    }
    Ok(())
}

fn preflight_resume_index(
    index_path: &Path,
    identity: &PstIdentity,
    workspace_id: &str,
    _extract_dir: &Path,
    discovery: &EmlDiscovery,
) -> AppResult<ResumeIndexPreflight> {
    let pristine_partial_allowed = index_path.file_name() == Some(OsStr::new("index.tmp.sqlite"));
    let conn = open_sqlite_read_only_connection(index_path)?;
    configure_sqlite_connection(&conn, true)?;
    let version = read_schema_version(&conn)?;
    if version > SQLITE_SCHEMA_VERSION_CURRENT {
        return Err(AppError::new(format!(
            "Workspace index schema version {version} is newer than this version of PST QuickView supports."
        )));
    }
    verify_no_schema_programs(&conn)?;
    let application_schema_objects = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if version == 0 && application_schema_objects == 0 {
        let quick_check =
            conn.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))?;
        if quick_check == "ok" {
            if pristine_partial_allowed {
                return Ok(ResumeIndexPreflight::Pristine);
            }
            return Err(AppError::new(
                "A pristine final workspace index cannot be adopted as resumable partial state.",
            ));
        }
        return Err(AppError::new(
            "The empty partial index failed SQLite integrity checking; resume was refused.",
        ));
    }
    for table in ["import_metadata", "messages"] {
        verify_schema_object_type(&conn, table, "table")?;
    }
    verify_required_columns(&conn, "import_metadata", &["key", "value"])?;
    verify_required_columns(&conn, "messages", &["id", "eml_path"])?;
    if !has_valid_import_metadata_key(&conn)? {
        return Err(AppError::new(
            "import_metadata.key is not a valid canonical metadata key; resume was refused.",
        ));
    }
    if !has_unique_message_source_identity(&conn)? {
        return Err(AppError::new(
            "messages.eml_path is not uniquely constrained; resume was refused.",
        ));
    }
    let non_pristine_metadata_count = conn.query_row(
        "SELECT COUNT(*)
           FROM import_metadata
          WHERE NOT (
                key = 'attachment_metadata_schema_version'
            AND value = 'attachment-metadata-v1'
          )",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    if non_pristine_metadata_count == 0 {
        let metadata_count = conn.query_row("SELECT COUNT(*) FROM import_metadata", [], |row| {
            row.get::<_, i64>(0)
        })?;
        if metadata_count > 1 {
            return Err(AppError::new(
                "The partial index has unexpected pristine metadata; resume was refused.",
            ));
        }
        if version != SQLITE_SCHEMA_VERSION_CURRENT {
            return Err(AppError::new(
                "The partial index has no import identity metadata; resume was refused.",
            ));
        }
        verify_read_query_schema(&conn)?;
        verify_schema(&conn)?;
        for table in ["folders", "messages", "attachments", "messages_fts"] {
            let count = conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get::<_, i64>(0)
            })?;
            if count != 0 {
                return Err(AppError::new(
                    "The partial index has data but no import identity metadata; resume was refused.",
                ));
            }
        }
        let quick_check =
            conn.query_row("PRAGMA quick_check", [], |row| row.get::<_, String>(0))?;
        if quick_check != "ok" {
            return Err(AppError::new(
                "The pristine partial index failed SQLite integrity checking; resume was refused.",
            ));
        }
        if pristine_partial_allowed {
            return Ok(ResumeIndexPreflight::Pristine);
        }
        return Err(AppError::new(
            "A pristine final workspace index cannot be adopted as resumable partial state.",
        ));
    }
    verify_read_query_schema(&conn)?;
    verify_schema(&conn)?;
    validate_resume_metadata(&conn, identity, workspace_id, discovery)?;
    validate_existing_message_identity_subset(&conn, discovery)?;
    let quick_check = conn.query_row("PRAGMA quick_check(1)", [], |row| row.get::<_, String>(0))?;
    if quick_check != "ok" {
        return Err(AppError::new(
            "The partial index failed SQLite integrity checking; resume was refused.",
        ));
    }
    Ok(ResumeIndexPreflight::Initialized)
}

#[allow(clippy::too_many_arguments)]
fn prepare_import_metadata(
    conn: &mut Connection,
    identity: &PstIdentity,
    workspace_id: &str,
    discovered_messages: usize,
    workspace: &Path,
    workspace_mode: WorkspaceLocationMode,
    readpst_path: &str,
    readpst_version: &str,
    resume_existing: bool,
    target_was_new: bool,
    discovery: &EmlDiscovery,
) -> AppResult<()> {
    if !resume_existing || target_was_new {
        insert_import_metadata(
            conn,
            identity,
            workspace_id,
            discovered_messages,
            &discovery.identity_manifest,
            workspace,
            workspace_mode,
            readpst_path,
            readpst_version,
        )?;
        return Ok(());
    }

    let now = Utc::now().to_rfc3339();
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing_count = transaction.query_row("SELECT COUNT(*) FROM messages", [], |row| {
        row.get::<_, usize>(0)
    })?;
    let existing_parse_failure_count = transaction.query_row(
        "SELECT COUNT(*) FROM messages WHERE body_source = 'parse_error'",
        [],
        |row| row.get::<_, usize>(0),
    )?;
    let existing_attachment_count =
        transaction.query_row("SELECT COUNT(*) FROM attachments", [], |row| {
            row.get::<_, usize>(0)
        })?;
    let legacy_provenance = metadata_value(&transaction, "canonical_import_identity_version")?
        .is_none()
        && metadata_value(&transaction, "importer_generation")?.is_none()
        && metadata_value(&transaction, "candidate_identity_manifest")?.is_none();
    let stored_workspace_id = metadata_value(&transaction, "workspace_id")?;
    validate_resume_metadata(&transaction, identity, workspace_id, discovery)?;
    if legacy_provenance {
        set_metadata_value(&transaction, "legacy_import_provenance_adopted", "true")?;
        set_metadata_value(&transaction, "legacy_import_provenance_adopted_at", &now)?;
    }
    if stored_workspace_id.as_deref() != Some(workspace_id) {
        set_metadata_value(&transaction, "legacy_workspace_identity_adopted", "true")?;
        set_metadata_value(&transaction, "legacy_workspace_identity_adopted_at", &now)?;
        set_metadata_value(&transaction, "workspace_id", workspace_id)?;
    }
    for (key, value) in [
        (
            "canonical_import_identity_version",
            CANONICAL_IMPORT_IDENTITY_VERSION.to_string(),
        ),
        ("importer_generation", IMPORTER_GENERATION.to_string()),
        (
            "candidate_identity_manifest",
            discovery.identity_manifest.clone(),
        ),
        ("import_status", ImportStatus::Running.as_str().to_string()),
        ("last_resume_at", now),
        ("last_resume_readpst_invoked", "false".to_string()),
        ("candidates_discovered", discovered_messages.to_string()),
        ("candidates_processed", "0".to_string()),
        ("canonical_messages_inserted", "0".to_string()),
        ("existing_canonical_messages_skipped", "0".to_string()),
        ("structurally_empty_artifacts_suppressed", "0".to_string()),
        ("parse_failures", existing_parse_failure_count.to_string()),
        ("attachments_indexed", existing_attachment_count.to_string()),
        (
            "canonical_attachment_total",
            existing_attachment_count.to_string(),
        ),
        ("canonical_messages_total", existing_count.to_string()),
        ("message_count_indexed", existing_count.to_string()),
        ("error_count", existing_parse_failure_count.to_string()),
        ("last_error", String::new()),
    ] {
        set_metadata_value(&transaction, key, value)?;
    }
    transaction.commit()?;
    Ok(())
}

fn ensure_folder(
    transaction: &Transaction<'_>,
    cache: &mut HashMap<String, i64>,
    folder_path: &str,
) -> AppResult<i64> {
    if let Some(id) = cache.get(folder_path) {
        return Ok(*id);
    }

    let existing = transaction
        .query_row(
            "SELECT id FROM folders WHERE path = ?1",
            params![folder_path],
            |row| row.get(0),
        )
        .optional()?;

    if let Some(id) = existing {
        cache.insert(folder_path.to_string(), id);
        return Ok(id);
    }

    let (parent_path, name) = split_folder_path(folder_path);
    let parent_id = if folder_path.is_empty() {
        None
    } else {
        Some(ensure_folder(transaction, cache, &parent_path)?)
    };

    transaction.execute(
        "INSERT INTO folders (parent_id, path, name) VALUES (?1, ?2, ?3)",
        params![parent_id, folder_path, name],
    )?;
    let id = transaction.last_insert_rowid();
    cache.insert(folder_path.to_string(), id);
    Ok(id)
}

fn split_folder_path(folder_path: &str) -> (String, String) {
    if folder_path.is_empty() {
        return (String::new(), ROOT_FOLDER_NAME.to_string());
    }

    match folder_path.rsplit_once('/') {
        Some((parent, name)) => (parent.to_string(), name.to_string()),
        None => (String::new(), folder_path.to_string()),
    }
}

fn existing_message_id(
    transaction: &Transaction<'_>,
    relative_path: &str,
) -> AppResult<Option<i64>> {
    Ok(transaction
        .query_row(
            "SELECT id FROM messages WHERE eml_path = ?1",
            params![relative_path],
            |row| row.get(0),
        )
        .optional()?)
}

fn insert_message(
    transaction: &Transaction<'_>,
    folder_id: i64,
    relative_path: &str,
    parsed: ParsedMessage,
) -> AppResult<InsertMessageOutcome> {
    insert_message_with_id(transaction, folder_id, relative_path, parsed, None)
}

fn insert_message_with_id(
    transaction: &Transaction<'_>,
    folder_id: i64,
    relative_path: &str,
    parsed: ParsedMessage,
    preferred_id: Option<i64>,
) -> AppResult<InsertMessageOutcome> {
    if preferred_id.is_some_and(|id| id <= 0) {
        return Err(AppError::new("Preferred message identity is invalid."));
    }
    let snippet = make_snippet(&parsed.body);
    let attachment_names = parsed
        .attachments
        .iter()
        .flat_map(|attachment| {
            [
                attachment.filename.as_str(),
                attachment.sanitized_filename.as_str(),
                attachment.content_type.as_str(),
            ]
        })
        .collect::<Vec<_>>()
        .join(" ");
    let has_attachments = !parsed.attachments.is_empty();

    let attachment_count = parsed.attachments.len();
    let inserted = transaction.execute(
        "INSERT INTO messages
             (id, folder_id, eml_path, subject, sender, recipients, date, body, body_source, body_html,
              snippet, attachment_names, has_attachments, message_id_header_raw, message_id_header,
              in_reply_to_raw, in_reply_to, references_header_raw, references_header,
              normalized_subject, thread_warning)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21)
         ON CONFLICT(eml_path) DO NOTHING",
        params![
            preferred_id,
            folder_id,
            relative_path,
            parsed.subject,
            parsed.sender,
            parsed.recipients,
            parsed.date,
            parsed.body,
            parsed.body_source.as_str(),
            if parsed.body_html.trim().is_empty() {
                None::<String>
            } else {
                Some(parsed.body_html)
            },
            snippet,
            attachment_names,
            if has_attachments { 1 } else { 0 },
            parsed.message_id_header_raw,
            parsed.message_id_header,
            parsed.in_reply_to_raw,
            parsed.in_reply_to,
            parsed.references_header_raw,
            parsed.references_header,
            parsed.normalized_subject,
            parsed.thread_warning,
        ],
    )?;
    if inserted == 0 {
        let message_id = existing_message_id(transaction, relative_path)?
            .ok_or_else(|| AppError::new("Canonical message conflict could not be resolved."))?;
        return Ok(InsertMessageOutcome::Existing { message_id });
    }
    let message_id = transaction.last_insert_rowid();

    for attachment in parsed.attachments {
        transaction.execute(
            "INSERT INTO attachments (
                message_id,
                filename,
                sanitized_filename,
                content_type,
                size_bytes,
                attachment_index,
                content_disposition,
                mime_part_path
            )
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                message_id,
                attachment.filename,
                attachment.sanitized_filename,
                attachment.content_type,
                attachment.size_bytes,
                attachment.attachment_index,
                attachment.content_disposition,
                attachment.mime_part_path,
            ],
        )?;
    }

    transaction.execute(
        "INSERT INTO messages_fts (rowid, subject, sender, recipients, body, attachment_names)
         SELECT id, subject, sender, recipients, body, attachment_names
           FROM messages
          WHERE id = ?1",
        params![message_id],
    )?;

    Ok(InsertMessageOutcome::Inserted {
        message_id,
        attachment_count,
    })
}

fn build_conversation_index(
    app: &tauri::AppHandle,
    workspace: &Path,
    conn: &mut Connection,
    workspace_id: &str,
) -> AppResult<()> {
    emit_progress(
        app,
        "Building conversations",
        None,
        None,
        "Assigning message threads from local headers.",
    );
    append_import_log(workspace, "state=building_conversations")?;

    let state = app.state::<AppState>();
    let (message_count, method_counts) = apply_conversation_index_with_progress(
        conn,
        workspace_id,
        || state.cancel_import_requested.load(Ordering::SeqCst),
        |progress| {
            if let Some((processed, total)) = progress {
                emit_progress_observation(
                    app,
                    ProgressObservation {
                        processed_units: u64::try_from(processed).ok(),
                        total_units: u64::try_from(total).ok(),
                        unit_kind: Some(import_progress::ProgressUnitKind::ConversationRecords),
                        activity_observed: true,
                        counters: Some(import_progress::ProgressCounters {
                            conversation_records_processed: u64::try_from(processed).ok(),
                            conversation_records_total: u64::try_from(total).ok(),
                            ..import_progress::ProgressCounters::default()
                        }),
                        ..ProgressObservation::default()
                    },
                );
                Ok(())
            } else {
                emit_activity_only_progress(app)
            }
        },
    )?;
    emit_progress_observation(
        app,
        ProgressObservation {
            processed_units: u64::try_from(message_count).ok(),
            total_units: u64::try_from(message_count).ok(),
            unit_kind: Some(import_progress::ProgressUnitKind::ConversationRecords),
            activity_observed: true,
            counters: Some(import_progress::ProgressCounters {
                conversation_records_processed: u64::try_from(message_count).ok(),
                conversation_records_total: u64::try_from(message_count).ok(),
                ..import_progress::ProgressCounters::default()
            }),
            ..ProgressObservation::default()
        },
    );
    checkpoint_integrity_operation(app, true)?;
    append_import_log(
        workspace,
        &format!(
            "state=conversation_complete messages={} header={} references={} subject_fallback={} standalone={}",
            message_count,
            method_counts.get("header").copied().unwrap_or_default(),
            method_counts.get("references").copied().unwrap_or_default(),
            method_counts
                .get("subject_fallback")
                .copied()
                .unwrap_or_default(),
            method_counts.get("standalone").copied().unwrap_or_default(),
        ),
    )?;
    Ok(())
}

#[cfg(test)]
fn apply_conversation_index<C>(
    conn: &mut Connection,
    workspace_id: &str,
    is_cancelled: C,
) -> AppResult<(usize, HashMap<String, usize>)>
where
    C: FnMut() -> bool,
{
    apply_conversation_index_with_progress(conn, workspace_id, is_cancelled, |_| Ok(()))
}

fn apply_conversation_index_with_progress<C, P>(
    conn: &mut Connection,
    workspace_id: &str,
    mut is_cancelled: C,
    mut on_progress: P,
) -> AppResult<(usize, HashMap<String, usize>)>
where
    C: FnMut() -> bool,
    P: FnMut(Option<(usize, usize)>) -> AppResult<()>,
{
    let mut statement = conn.prepare(
        "SELECT id,
                eml_path,
                COALESCE(message_id_header, ''),
                COALESCE(in_reply_to, ''),
                COALESCE(references_header, ''),
                COALESCE(normalized_subject, ''),
                COALESCE(sender, ''),
                COALESCE(recipients, ''),
                COALESCE(date, ''),
                COALESCE(thread_warning, '')
           FROM messages
          ORDER BY id",
    )?;
    let rows = statement.query_map([], |row| {
        let date = row.get::<_, String>(8)?;
        Ok(ThreadInput {
            id: row.get(0)?,
            eml_path: row.get(1)?,
            message_id: nonempty_string(row.get(2)?),
            in_reply_to: nonempty_string(row.get(3)?),
            references: row
                .get::<_, String>(4)?
                .lines()
                .filter_map(|value| nonempty_string(value.to_string()))
                .collect(),
            normalized_subject: row.get(5)?,
            sender_emails: extract_email_addresses(&row.get::<_, String>(6)?),
            recipient_emails: extract_email_addresses(&row.get::<_, String>(7)?),
            timestamp: thread_timestamp(&date),
            warning: row.get(9)?,
        })
    })?;
    let mut inputs = Vec::new();
    for (index, row) in rows.enumerate() {
        if index % threading::THREAD_ASSIGNMENT_PROGRESS_BATCH_SIZE == 0 {
            if is_cancelled() {
                return Err(import_cancelled_error(
                    "Conversation preparation was cancelled at a bounded checkpoint.",
                ));
            }
            on_progress(None)?;
        }
        inputs.push(row?);
    }
    drop(statement);

    let mut progress_error = None;
    let assignments = assign_threads_with_progress(&inputs, |_| {
        if is_cancelled() {
            return ThreadAssignmentControl::Cancel;
        }
        match on_progress(None) {
            Ok(()) => ThreadAssignmentControl::Continue,
            Err(error) => {
                progress_error = Some(error);
                ThreadAssignmentControl::Cancel
            }
        }
    });
    let assignments = match assignments {
        Ok(assignments) => assignments,
        Err(_) => {
            if let Some(error) = progress_error {
                return Err(error);
            }
            return Err(import_cancelled_error(
                "Conversation assignment was cancelled at a bounded checkpoint.",
            ));
        }
    };
    if is_cancelled() {
        return Err(import_cancelled_error(
            "Conversation assignment was cancelled before index updates.",
        ));
    }
    if !assignments.is_empty() {
        on_progress(Some((0, assignments.len())))?;
    }
    let transaction = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut method_counts = HashMap::<String, usize>::new();
    for (index, assignment) in assignments.iter().enumerate() {
        if index % INDEX_PROGRESS_MESSAGE_INTERVAL == 0 && is_cancelled() {
            return Err(import_cancelled_error(
                "Conversation assignment cancelled. Existing workspace remains recoverable.",
            ));
        }
        let conversation_id =
            sha256_text(&format!("{workspace_id}\0{}", assignment.conversation_seed));
        transaction.execute(
            "UPDATE messages
                SET conversation_id = ?1,
                    conversation_parent_id = ?2,
                    conversation_root_id = ?3,
                    thread_assignment_method = ?4,
                    thread_warning = ?5
              WHERE id = ?6",
            params![
                conversation_id,
                assignment.parent_id,
                assignment.root_id,
                assignment.method,
                assignment.warning,
                assignment.id,
            ],
        )?;
        *method_counts.entry(assignment.method.clone()).or_default() += 1;
        if (index + 1) % INDEX_PROGRESS_MESSAGE_INTERVAL == 0 && index + 1 < assignments.len() {
            on_progress(Some((index + 1, assignments.len())))?;
        }
    }
    if is_cancelled() {
        return Err(import_cancelled_error(
            "Conversation assignment was cancelled before the index commit.",
        ));
    }
    set_metadata_value(
        &transaction,
        "conversation_schema_version",
        CONVERSATION_SCHEMA_VERSION,
    )?;
    transaction.commit()?;
    Ok((assignments.len(), method_counts))
}

fn nonempty_string(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn thread_timestamp(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.timestamp())
        .or_else(|| mailparse::dateparse(value).ok())
}

fn parse_eml(path: &Path) -> AppResult<ParsedMessage> {
    let bytes = read_bounded_message_source(path)?;
    parse_eml_bytes(&bytes)
}

fn read_discovered_eml_candidate(candidate: &EmlCandidate) -> AppResult<Vec<u8>> {
    let (bytes, identity) = read_eml_candidate_snapshot(&candidate.path)?;
    if identity != candidate.identity {
        return Err(AppError::new(
            "An extracted EML candidate changed while it was read; indexing was refused.",
        ));
    }
    Ok(bytes)
}

fn parse_eml_bytes(bytes: &[u8]) -> AppResult<ParsedMessage> {
    let parsed = parse_mail_bytes(bytes)?;

    let mut text_parts = Vec::new();
    let mut html_parts = Vec::new();
    let mut rtf_candidates = Vec::new();
    let mut attachments = Vec::new();
    collect_body_parts(
        &parsed,
        "0",
        &mut text_parts,
        &mut html_parts,
        &mut rtf_candidates,
        &mut attachments,
    );

    let body_html = html_parts.join("\n\n").trim().to_string();
    let body_selection = select_message_body(text_parts, &body_html, &rtf_candidates);
    validate_rendered_message_text(&body_selection.body, &body_selection.body_html)?;
    let rtf_body_fallback = body_selection
        .rtf_body_part_path
        .as_ref()
        .and_then(|part_path| {
            let candidate = rtf_candidates
                .iter()
                .find(|candidate| candidate.part_path == *part_path)?;
            attachments.retain(|attachment| attachment.mime_part_path != *part_path);
            renumber_attachment_indexes(&mut attachments);
            Some(format!(
                "part={} kind={} content_type={} filename={} suppressed_from_attachments=true",
                candidate.part_path,
                candidate.kind.as_str(),
                candidate.content_type,
                candidate.filename.as_deref().unwrap_or("(none)")
            ))
        });

    let subject = header_value(&parsed, "Subject");
    let message_id_header_raw = header_value(&parsed, "Message-ID");
    let in_reply_to_raw = header_value(&parsed, "In-Reply-To");
    let references_header_raw = header_value(&parsed, "References");
    let message_ids = extract_message_ids(&message_id_header_raw);
    let in_reply_to_ids = extract_message_ids(&in_reply_to_raw);
    let reference_ids = extract_message_ids(&references_header_raw);
    let mut thread_warnings = Vec::new();
    if !message_id_header_raw.trim().is_empty() && message_ids.is_empty() {
        thread_warnings.push("Message-ID header could not be normalized.");
    }
    if !in_reply_to_raw.trim().is_empty() && in_reply_to_ids.is_empty() {
        thread_warnings.push("In-Reply-To header could not be normalized.");
    }
    if !references_header_raw.trim().is_empty() && reference_ids.is_empty() {
        thread_warnings.push("References header could not be normalized.");
    }

    Ok(ParsedMessage {
        normalized_subject: normalize_thread_subject(&subject),
        subject,
        sender: header_value(&parsed, "From"),
        recipients: ["To", "Cc", "Bcc"]
            .iter()
            .map(|header| header_value(&parsed, header))
            .filter(|value| !value.trim().is_empty())
            .collect::<Vec<_>>()
            .join("; "),
        date: normalized_date(&header_value(&parsed, "Date")),
        body: body_selection.body,
        body_source: body_selection.body_source,
        body_html: body_selection.body_html,
        attachments,
        rtf_body_fallback,
        message_id_header_raw,
        message_id_header: message_ids.first().cloned().unwrap_or_default(),
        in_reply_to_raw,
        in_reply_to: in_reply_to_ids.first().cloned().unwrap_or_default(),
        references_header_raw,
        references_header: reference_ids.join("\n"),
        thread_warning: thread_warnings.join(" "),
    })
}

fn collect_body_parts(
    part: &mailparse::ParsedMail<'_>,
    part_path: &str,
    text_parts: &mut Vec<String>,
    html_parts: &mut Vec<String>,
    rtf_candidates: &mut Vec<RtfBodyCandidate>,
    attachments: &mut Vec<AttachmentDraft>,
) {
    if !part.subparts.is_empty() {
        for (index, child) in part.subparts.iter().enumerate() {
            let child_path = format!("{part_path}.{index}");
            collect_body_parts(
                child,
                &child_path,
                text_parts,
                html_parts,
                rtf_candidates,
                attachments,
            );
        }
        return;
    }

    let disposition = part.get_content_disposition();
    let content_disposition = disposition_type_label(&disposition.disposition);
    let filename = disposition
        .params
        .get("filename")
        .or_else(|| part.ctype.params.get("name"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let mimetype = part.ctype.mimetype.to_lowercase();

    if is_rtf_like_part(&mimetype, filename.as_deref()) {
        if let Some(conversion) = extract_rtf_conversion_from_part(part) {
            rtf_candidates.push(RtfBodyCandidate {
                text: conversion.text,
                html: conversion.html,
                kind: conversion.kind,
                part_path: part_path.to_string(),
                filename: filename.clone(),
                content_type: mimetype.clone(),
                content_disposition: content_disposition.clone(),
            });
        }
    }

    if matches!(
        disposition.disposition,
        mailparse::DispositionType::Attachment
    ) || filename.is_some()
    {
        let attachment_index = attachments.len() as i64;
        let filename = filename.unwrap_or_else(|| format!("attachment-{}", attachment_index + 1));
        let sanitized_filename = sanitize_attachment_filename(&filename);
        attachments.push(AttachmentDraft {
            filename,
            sanitized_filename,
            content_type: mimetype,
            size_bytes: part.get_body_raw().ok().map(|bytes| bytes.len() as i64),
            attachment_index,
            content_disposition,
            mime_part_path: part_path.to_string(),
        });
        return;
    }

    if mimetype == "text/plain" {
        if let Ok(body) = part.get_body() {
            text_parts.push(body);
        }
    } else if mimetype == "text/html" {
        if let Ok(body) = part.get_body() {
            html_parts.push(body);
        }
    }
}

fn renumber_attachment_indexes(attachments: &mut [AttachmentDraft]) {
    for (index, attachment) in attachments.iter_mut().enumerate() {
        attachment.attachment_index = index as i64;
    }
}

fn disposition_type_label(disposition: &mailparse::DispositionType) -> String {
    match disposition {
        mailparse::DispositionType::Inline => "inline".to_string(),
        mailparse::DispositionType::Attachment => "attachment".to_string(),
        mailparse::DispositionType::FormData => "form-data".to_string(),
        mailparse::DispositionType::Extension(value) => value.clone(),
    }
}

fn is_attachment_part(part: &mailparse::ParsedMail<'_>) -> bool {
    let disposition = part.get_content_disposition();
    let has_filename =
        disposition.params.contains_key("filename") || part.ctype.params.contains_key("name");
    matches!(
        disposition.disposition,
        mailparse::DispositionType::Attachment
    ) || has_filename
}

fn is_rtf_like_part(mimetype: &str, filename: Option<&str>) -> bool {
    let filename = filename.unwrap_or_default().to_ascii_lowercase();
    matches!(
        mimetype,
        "text/rtf" | "application/rtf" | "application/ms-tnef" | "application/x-rtf"
    ) || filename.ends_with(".rtf")
        || filename == "winmail.dat"
}

fn is_body_like_rtf_candidate(candidate: &RtfBodyCandidate) -> bool {
    if candidate.text.trim().is_empty() {
        return false;
    }

    if candidate.content_disposition != "attachment" {
        return true;
    }

    let filename = candidate
        .filename
        .as_deref()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();

    filename.is_empty()
        || matches!(
            filename.as_str(),
            "rtf-body.rtf" | "body.rtf" | "message.rtf" | "winmail.dat"
        )
}

fn parse_logged_rtf_part_path(value: &str) -> Option<String> {
    value
        .split_whitespace()
        .find_map(|field| field.strip_prefix("part="))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn collect_mime_part_diagnostics(
    part: &mailparse::ParsedMail<'_>,
    part_path: &str,
    promoted_rtf_part_path: Option<&str>,
    output: &mut Vec<MimePartDiagnostic>,
) {
    let content_type = part.ctype.mimetype.to_lowercase();
    let disposition = part.get_content_disposition();
    let content_disposition = disposition_type_label(&disposition.disposition);
    let filename = diagnostic_part_filename(part, &disposition);
    let content_id = part
        .headers
        .get_first_value("Content-ID")
        .unwrap_or_default()
        .trim()
        .to_string();
    let size_bytes = if part.subparts.is_empty() {
        part.get_body_raw().ok().map(|bytes| bytes.len() as i64)
    } else {
        None
    };
    let role = diagnostic_part_role(
        part,
        part_path,
        &content_type,
        &content_disposition,
        filename.as_deref(),
        promoted_rtf_part_path,
    );

    output.push(MimePartDiagnostic {
        path: part_path.to_string(),
        content_type,
        content_disposition,
        filename: filename.unwrap_or_default(),
        content_id,
        size_bytes,
        role,
    });

    for (index, child) in part.subparts.iter().enumerate() {
        collect_mime_part_diagnostics(
            child,
            &format!("{part_path}.{index}"),
            promoted_rtf_part_path,
            output,
        );
    }
}

fn diagnostic_part_filename(
    part: &mailparse::ParsedMail<'_>,
    disposition: &mailparse::ParsedContentDisposition,
) -> Option<String> {
    disposition
        .params
        .get("filename")
        .or_else(|| part.ctype.params.get("name"))
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn diagnostic_part_role(
    part: &mailparse::ParsedMail<'_>,
    part_path: &str,
    content_type: &str,
    content_disposition: &str,
    filename: Option<&str>,
    promoted_rtf_part_path: Option<&str>,
) -> String {
    if !part.subparts.is_empty() {
        return "container".to_string();
    }

    if promoted_rtf_part_path.is_some_and(|promoted| promoted == part_path) {
        return "rtf body promoted, hidden from attachments".to_string();
    }

    if content_disposition == "attachment" || filename.is_some() {
        if is_body_like_rtf_filename(filename) && is_rtf_like_part(content_type, filename) {
            return "rtf body candidate".to_string();
        }
        return "attachment".to_string();
    }

    match content_type {
        "text/plain" => "plain body".to_string(),
        "text/html" => "html body".to_string(),
        _ if is_rtf_like_part(content_type, filename) => "rtf body candidate".to_string(),
        _ => "ignored".to_string(),
    }
}

fn is_body_like_rtf_filename(filename: Option<&str>) -> bool {
    let filename = filename.unwrap_or_default().trim().to_ascii_lowercase();
    filename.is_empty()
        || matches!(
            filename.as_str(),
            "rtf-body.rtf" | "body.rtf" | "message.rtf" | "winmail.dat"
        )
}

fn describe_mime_part(part: &MimePartDiagnostic) -> String {
    let mut fields = vec![
        format!("part={}", part.path),
        format!("content_type={}", part.content_type),
    ];
    if !part.filename.trim().is_empty() {
        fields.push(format!("filename={}", part.filename));
    }
    fields.push(format!("role={}", part.role));
    fields.join(" ")
}

fn extract_rtf_conversion_from_part(part: &mailparse::ParsedMail<'_>) -> Option<RtfConversion> {
    if let Ok(body) = part.get_body() {
        if let Some(conversion) = convert_rtf_payload(&body) {
            return Some(conversion);
        }
    }

    let raw = part.get_body_raw().ok()?;
    if let Some(start) = find_rtf_start_in_bytes(&raw) {
        let body = String::from_utf8_lossy(&raw[start..]);
        if let Some(conversion) = convert_rtf_payload(&body) {
            return Some(conversion);
        }
    }

    let body = String::from_utf8_lossy(&raw);
    convert_rtf_payload(&body)
}

fn convert_rtf_payload(value: &str) -> Option<RtfConversion> {
    let start = find_ascii_case_insensitive(value, "{\\rtf")?;
    let rtf = &value[start..];
    let kind = if find_ascii_case_insensitive(rtf, "\\fromhtml").is_some() {
        RtfConversionKind::FromHtml
    } else if find_ascii_case_insensitive(rtf, "\\fromtext").is_some() {
        RtfConversionKind::FromText
    } else {
        RtfConversionKind::Rtf
    };
    let html = if kind == RtfConversionKind::FromHtml {
        extract_encapsulated_html_from_rtf(rtf)
    } else {
        None
    };
    let text = html
        .as_deref()
        .map(html_to_text)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| rtf_to_text(rtf));
    if text.trim().is_empty() {
        None
    } else {
        Some(RtfConversion { text, html, kind })
    }
}

fn find_rtf_start_in_bytes(bytes: &[u8]) -> Option<usize> {
    let needle = b"{\\rtf";
    bytes.windows(needle.len()).position(|window| {
        window
            .iter()
            .zip(needle)
            .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
}

#[derive(Clone, Copy)]
struct RtfHtmlParserState {
    skipping_destination: bool,
    capture_html_tag: bool,
    unicode_skip_count: usize,
    html_rtf_suppressed: bool,
}

fn extract_encapsulated_html_from_rtf(rtf: &str) -> Option<String> {
    find_ascii_case_insensitive(rtf, "\\fromhtml")?;

    let html = rtf_fromhtml_to_html(rtf);
    let html = html.trim();
    if html.is_empty() {
        None
    } else {
        Some(html.to_string())
    }
}

fn rtf_fromhtml_to_html(rtf: &str) -> String {
    let chars = rtf.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(rtf.len().min(16_384));
    let mut stack = Vec::new();
    let mut state = RtfHtmlParserState {
        skipping_destination: false,
        capture_html_tag: false,
        unicode_skip_count: 1,
        html_rtf_suppressed: false,
    };
    let mut ignorable_destination = false;
    let mut unicode_fallback_chars_to_skip = 0usize;
    let mut index = 0usize;

    while index < chars.len() {
        let char = chars[index];
        index += 1;

        if unicode_fallback_chars_to_skip > 0 {
            unicode_fallback_chars_to_skip -= 1;
            continue;
        }

        match char {
            '{' => {
                if stack.len() >= MAX_RTF_GROUP_DEPTH {
                    return String::new();
                }
                stack.push(state);
                ignorable_destination = false;
            }
            '}' => {
                if let Some(previous) = stack.pop() {
                    state = previous;
                }
                ignorable_destination = false;
            }
            '\\' => {
                if index >= chars.len() {
                    break;
                }
                let control = chars[index];
                index += 1;

                match control {
                    '\\' | '{' | '}' => {
                        append_rtf_html_char(control, state, &mut output);
                    }
                    '\'' => {
                        if let Some(byte) = parse_rtf_hex_byte(&chars, index) {
                            append_rtf_html_char(decode_rtf_ansi_byte(byte), state, &mut output);
                            index += 2;
                        }
                    }
                    '*' => {
                        ignorable_destination = true;
                    }
                    '~' => append_rtf_html_char(' ', state, &mut output),
                    '-' | '_' => append_rtf_html_char('-', state, &mut output),
                    '\n' | '\r' => {}
                    _ if control.is_ascii_alphabetic() => {
                        let word_start = index - 1;
                        while index < chars.len() && chars[index].is_ascii_alphabetic() {
                            index += 1;
                        }
                        let word = chars[word_start..index].iter().collect::<String>();
                        let parameter = parse_rtf_control_parameter(&chars, &mut index);

                        if index < chars.len() && chars[index] == ' ' {
                            index += 1;
                        }

                        if word == "htmltag" {
                            state.skipping_destination = false;
                            state.capture_html_tag = true;
                            state.html_rtf_suppressed = false;
                            ignorable_destination = false;
                            continue;
                        }

                        if ignorable_destination || is_ignored_rtf_destination(&word) {
                            state.skipping_destination = true;
                            ignorable_destination = false;
                            continue;
                        }
                        ignorable_destination = false;

                        match word.as_str() {
                            "htmlrtf" => {
                                state.html_rtf_suppressed = parameter.unwrap_or(1) != 0;
                                continue;
                            }
                            "uc" => {
                                if let Some(count) = parameter {
                                    state.unicode_skip_count = count.max(0) as usize;
                                }
                                continue;
                            }
                            _ => {}
                        }

                        if state.skipping_destination {
                            continue;
                        }

                        match word.as_str() {
                            "par" | "line" => append_rtf_html_break(state, &mut output),
                            "tab" => append_rtf_html_char(' ', state, &mut output),
                            "emdash" => append_rtf_html_char('\u{2014}', state, &mut output),
                            "endash" => append_rtf_html_char('\u{2013}', state, &mut output),
                            "bullet" => append_rtf_html_char('\u{2022}', state, &mut output),
                            "lquote" => append_rtf_html_char('\u{2018}', state, &mut output),
                            "rquote" => append_rtf_html_char('\u{2019}', state, &mut output),
                            "ldblquote" => append_rtf_html_char('\u{201C}', state, &mut output),
                            "rdblquote" => append_rtf_html_char('\u{201D}', state, &mut output),
                            "u" => {
                                if let Some(value) = parameter {
                                    let codepoint = if value < 0 {
                                        (value + 65_536) as u32
                                    } else {
                                        value as u32
                                    };
                                    if let Some(unicode_char) = char::from_u32(codepoint) {
                                        append_rtf_html_char(unicode_char, state, &mut output);
                                    }
                                    unicode_fallback_chars_to_skip = state.unicode_skip_count;
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {
                        if control == '\t' {
                            append_rtf_html_char('\t', state, &mut output);
                        }
                    }
                }
            }
            _ => append_rtf_html_char(char, state, &mut output),
        }
        if output.len() > MAX_MESSAGE_TEXT_BYTES {
            return String::new();
        }
    }

    output
}

fn append_rtf_html_break(state: RtfHtmlParserState, output: &mut String) {
    if should_append_rtf_html_text(state) {
        output.push_str("<br>");
    }
}

fn append_rtf_html_char(char: char, state: RtfHtmlParserState, output: &mut String) {
    if !should_append_rtf_html_text(state) {
        return;
    }

    if state.capture_html_tag {
        output.push(char);
    } else {
        append_html_escaped_char(char, output);
    }
}

fn should_append_rtf_html_text(state: RtfHtmlParserState) -> bool {
    !state.skipping_destination && (state.capture_html_tag || !state.html_rtf_suppressed)
}

fn append_html_escaped_char(char: char, output: &mut String) {
    match char {
        '&' => output.push_str("&amp;"),
        '<' => output.push_str("&lt;"),
        '>' => output.push_str("&gt;"),
        '"' => output.push_str("&quot;"),
        '\'' => output.push_str("&#39;"),
        '\r' | '\n' => {
            if !output.ends_with(' ') {
                output.push(' ');
            }
        }
        '\t' => output.push(' '),
        _ => output.push(char),
    }
}

fn find_attachment_part<'a>(
    part: &'a mailparse::ParsedMail<'a>,
    part_path: &str,
    target_attachment_index: i64,
    target_mime_part_path: Option<&str>,
    attachment_counter: &mut i64,
) -> Option<&'a mailparse::ParsedMail<'a>> {
    if !part.subparts.is_empty() {
        for (index, child) in part.subparts.iter().enumerate() {
            let child_path = format!("{part_path}.{index}");
            if let Some(found) = find_attachment_part(
                child,
                &child_path,
                target_attachment_index,
                target_mime_part_path,
                attachment_counter,
            ) {
                return Some(found);
            }
        }
        return None;
    }

    if !is_attachment_part(part) {
        return None;
    }

    let current_attachment_index = *attachment_counter;
    *attachment_counter += 1;

    if target_mime_part_path.is_some_and(|target| target == part_path)
        || target_mime_part_path.is_none() && current_attachment_index == target_attachment_index
    {
        Some(part)
    } else {
        None
    }
}

fn sanitize_attachment_filename(filename: &str) -> String {
    let leaf = filename
        .rsplit(['/', '\\', ':'])
        .next()
        .unwrap_or(filename)
        .trim();
    let mut sanitized = String::with_capacity(leaf.len());
    let mut last_was_space = false;

    for char in leaf.chars() {
        if char.is_control() || matches!(char, '/' | '\\' | ':' | '\0') {
            if !sanitized.ends_with('_') {
                sanitized.push('_');
            }
            last_was_space = false;
        } else if char.is_whitespace() {
            if !last_was_space {
                sanitized.push(' ');
                last_was_space = true;
            }
        } else {
            sanitized.push(char);
            last_was_space = false;
        }
    }

    let candidate = sanitized.trim().trim_matches('.');
    let mut sanitized = String::new();
    for character in candidate.chars() {
        if sanitized.len() + character.len_utf8() > 180 {
            break;
        }
        sanitized.push(character);
    }
    let sanitized = sanitized.trim_end_matches([' ', '.']).to_string();
    let reserved_stem = sanitized
        .split('.')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let is_reserved = matches!(
        reserved_stem.as_str(),
        "con"
            | "prn"
            | "aux"
            | "nul"
            | "com1"
            | "com2"
            | "com3"
            | "com4"
            | "com5"
            | "com6"
            | "com7"
            | "com8"
            | "com9"
            | "lpt1"
            | "lpt2"
            | "lpt3"
            | "lpt4"
            | "lpt5"
            | "lpt6"
            | "lpt7"
            | "lpt8"
            | "lpt9"
    ) || matches!(reserved_stem.as_str(), "ds_store" | "localized");

    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        "attachment".to_string()
    } else if sanitized.starts_with('-') || is_reserved {
        format!("attachment-{sanitized}")
    } else {
        sanitized
    }
}

fn header_value(parsed: &mailparse::ParsedMail<'_>, header: &str) -> String {
    parsed
        .headers
        .get_first_value(header)
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn normalized_date(value: &str) -> String {
    if value.trim().is_empty() {
        return String::new();
    }

    match mailparse::dateparse(value) {
        Ok(timestamp) => DateTime::<Utc>::from_timestamp(timestamp, 0)
            .map(|date| date.to_rfc3339())
            .unwrap_or_else(|| value.to_string()),
        Err(_) => value.to_string(),
    }
}

fn select_message_body(
    text_parts: Vec<String>,
    body_html: &str,
    rtf_candidates: &[RtfBodyCandidate],
) -> MessageBodySelection {
    let plain = normalize_plain_text_body(&text_parts.join("\n\n"));
    let rtf_body_candidate = rtf_candidates
        .iter()
        .find(|candidate| is_body_like_rtf_candidate(candidate));

    if let Some(candidate) = rtf_body_candidate {
        if should_promote_rtf_body(&plain, body_html, candidate) {
            return rtf_body_selection(candidate, body_html);
        }
    }

    if !plain.trim().is_empty() {
        return MessageBodySelection {
            body: plain,
            body_source: BodySource::TextPlain,
            body_html: body_html.trim().to_string(),
            rtf_body_part_path: None,
        };
    }

    let converted = html_to_text(body_html);
    if !converted.trim().is_empty() {
        return MessageBodySelection {
            body: converted,
            body_source: BodySource::HtmlConverted,
            body_html: body_html.trim().to_string(),
            rtf_body_part_path: None,
        };
    }

    if let Some(candidate) = rtf_body_candidate {
        return rtf_body_selection(candidate, body_html);
    }

    MessageBodySelection {
        body: String::new(),
        body_source: BodySource::Missing,
        body_html: body_html.trim().to_string(),
        rtf_body_part_path: None,
    }
}

fn should_promote_rtf_body(plain: &str, body_html: &str, candidate: &RtfBodyCandidate) -> bool {
    if candidate.text.trim().is_empty() {
        return false;
    }

    if plain_text_is_useless(plain) {
        return true;
    }

    if body_html.trim().is_empty()
        && candidate
            .html
            .as_deref()
            .is_some_and(|html| !html.trim().is_empty())
    {
        return true;
    }

    plain_is_short_placeholder(plain) && candidate.text.trim().len() > plain.trim().len() + 20
}

fn rtf_body_selection(
    candidate: &RtfBodyCandidate,
    existing_body_html: &str,
) -> MessageBodySelection {
    let recovered_html = candidate
        .html
        .as_deref()
        .map(str::trim)
        .filter(|html| !html.is_empty());
    let body_html = recovered_html
        .map(str::to_string)
        .unwrap_or_else(|| existing_body_html.trim().to_string());
    let body = recovered_html
        .map(html_to_text)
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| normalize_plain_text_body(&candidate.text));

    MessageBodySelection {
        body,
        body_source: if recovered_html.is_some() {
            BodySource::RtfHtmlConverted
        } else {
            BodySource::RtfConverted
        },
        body_html,
        rtf_body_part_path: Some(candidate.part_path.clone()),
    }
}

fn plain_text_is_useless(value: &str) -> bool {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return true;
    }

    if plain_is_short_placeholder(trimmed) {
        return true;
    }

    let alphanumeric_count = trimmed
        .chars()
        .filter(|char| char.is_alphanumeric())
        .count();
    alphanumeric_count < 12
}

fn plain_is_short_placeholder(value: &str) -> bool {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    let lower = compact.to_ascii_lowercase();
    compact.chars().count() < 40
        || matches!(
            lower.as_str(),
            "this is a multi-part message in mime format."
                | "this is a multipart message in mime format."
                | "this message is in mime format."
                | "this message is in microsoft outlook rich text format."
        )
}

fn normalize_plain_text_body(value: &str) -> String {
    let normalized = value.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines = Vec::new();
    let mut blank_count = 0;

    for raw_line in normalized.lines() {
        let line = raw_line.trim_end_matches([' ', '\t']);
        if line.trim().is_empty() {
            blank_count += 1;
            if blank_count <= 2 {
                lines.push(String::new());
            }
        } else {
            blank_count = 0;
            lines.push(line.to_string());
        }
    }

    while lines.first().is_some_and(|line| line.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }

    lines.join("\n")
}

#[derive(Clone, Copy)]
struct RtfParserState {
    skipping_destination: bool,
    unicode_skip_count: usize,
}

fn rtf_to_text(rtf: &str) -> String {
    let chars = rtf.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(rtf.len().min(8192));
    let mut stack = Vec::new();
    let mut state = RtfParserState {
        skipping_destination: false,
        unicode_skip_count: 1,
    };
    let mut ignorable_destination = false;
    let mut unicode_fallback_chars_to_skip = 0usize;
    let mut index = 0usize;

    while index < chars.len() {
        let char = chars[index];
        index += 1;

        if unicode_fallback_chars_to_skip > 0 {
            unicode_fallback_chars_to_skip -= 1;
            continue;
        }

        match char {
            '{' => {
                if stack.len() >= MAX_RTF_GROUP_DEPTH {
                    return String::new();
                }
                stack.push(state);
                ignorable_destination = false;
            }
            '}' => {
                if let Some(previous) = stack.pop() {
                    state = previous;
                }
                ignorable_destination = false;
            }
            '\\' => {
                if index >= chars.len() {
                    break;
                }
                let control = chars[index];
                index += 1;

                match control {
                    '\\' | '{' | '}' => {
                        if !state.skipping_destination {
                            output.push(control);
                        }
                    }
                    '\'' => {
                        if index < chars.len() {
                            if let Some(byte) = parse_rtf_hex_byte(&chars, index) {
                                if !state.skipping_destination {
                                    output.push(decode_rtf_ansi_byte(byte));
                                }
                                index += 2;
                            }
                        }
                    }
                    '*' => {
                        ignorable_destination = true;
                    }
                    '~' => {
                        if !state.skipping_destination {
                            output.push(' ');
                        }
                    }
                    '-' | '_' => {
                        if !state.skipping_destination {
                            output.push('-');
                        }
                    }
                    '\n' | '\r' => {}
                    _ if control.is_ascii_alphabetic() => {
                        let word_start = index - 1;
                        while index < chars.len() && chars[index].is_ascii_alphabetic() {
                            index += 1;
                        }
                        let word = chars[word_start..index].iter().collect::<String>();
                        let parameter = parse_rtf_control_parameter(&chars, &mut index);

                        if index < chars.len() && chars[index] == ' ' {
                            index += 1;
                        }

                        if ignorable_destination || is_ignored_rtf_destination(&word) {
                            state.skipping_destination = true;
                            ignorable_destination = false;
                            continue;
                        }
                        ignorable_destination = false;

                        if word == "uc" {
                            if let Some(count) = parameter {
                                state.unicode_skip_count = count.max(0) as usize;
                            }
                            continue;
                        }

                        if state.skipping_destination {
                            continue;
                        }

                        match word.as_str() {
                            "par" | "line" => output.push('\n'),
                            "tab" => output.push('\t'),
                            "emdash" => output.push('\u{2014}'),
                            "endash" => output.push('\u{2013}'),
                            "bullet" => output.push('*'),
                            "lquote" => output.push('\u{2018}'),
                            "rquote" => output.push('\u{2019}'),
                            "ldblquote" => output.push('\u{201C}'),
                            "rdblquote" => output.push('\u{201D}'),
                            "u" => {
                                if let Some(value) = parameter {
                                    let codepoint = if value < 0 {
                                        (value + 65_536) as u32
                                    } else {
                                        value as u32
                                    };
                                    if let Some(unicode_char) = char::from_u32(codepoint) {
                                        output.push(unicode_char);
                                    }
                                    unicode_fallback_chars_to_skip = state.unicode_skip_count;
                                }
                            }
                            _ => {}
                        }
                    }
                    _ => {
                        if !state.skipping_destination && control == '\t' {
                            output.push('\t');
                        }
                    }
                }
            }
            _ => {
                if !state.skipping_destination {
                    output.push(char);
                }
            }
        }
        if output.len() > MAX_MESSAGE_TEXT_BYTES {
            return String::new();
        }
    }

    normalize_plain_text_body(&output)
}

fn parse_rtf_control_parameter(chars: &[char], index: &mut usize) -> Option<i32> {
    let mut sign = 1;
    if *index < chars.len() && chars[*index] == '-' {
        sign = -1;
        *index += 1;
    }

    let start = *index;
    while *index < chars.len() && chars[*index].is_ascii_digit() {
        *index += 1;
    }

    if *index == start {
        None
    } else {
        chars[start..*index]
            .iter()
            .collect::<String>()
            .parse::<i32>()
            .ok()
            .map(|value| value * sign)
    }
}

fn parse_rtf_hex_byte(chars: &[char], index: usize) -> Option<u8> {
    if index + 1 >= chars.len() {
        return None;
    }

    let high = chars[index].to_digit(16)?;
    let low = chars[index + 1].to_digit(16)?;
    Some(((high << 4) | low) as u8)
}

fn decode_rtf_ansi_byte(byte: u8) -> char {
    match byte {
        0x80 => '\u{20AC}',
        0x82 => '\u{201A}',
        0x83 => '\u{0192}',
        0x84 => '\u{201E}',
        0x85 => '\u{2026}',
        0x86 => '\u{2020}',
        0x87 => '\u{2021}',
        0x88 => '\u{02C6}',
        0x89 => '\u{2030}',
        0x8A => '\u{0160}',
        0x8B => '\u{2039}',
        0x8C => '\u{0152}',
        0x8E => '\u{017D}',
        0x91 => '\u{2018}',
        0x92 => '\u{2019}',
        0x93 => '\u{201C}',
        0x94 => '\u{201D}',
        0x95 => '\u{2022}',
        0x96 => '\u{2013}',
        0x97 => '\u{2014}',
        0x98 => '\u{02DC}',
        0x99 => '\u{2122}',
        0x9A => '\u{0161}',
        0x9B => '\u{203A}',
        0x9C => '\u{0153}',
        0x9E => '\u{017E}',
        0x9F => '\u{0178}',
        _ => char::from(byte),
    }
}

fn is_ignored_rtf_destination(word: &str) -> bool {
    matches!(
        word,
        "fonttbl"
            | "colortbl"
            | "stylesheet"
            | "info"
            | "pict"
            | "object"
            | "objdata"
            | "datastore"
            | "themedata"
            | "header"
            | "footer"
            | "headerl"
            | "headerr"
            | "headerf"
            | "footerl"
            | "footerr"
            | "footerf"
            | "annotation"
            | "xmlopen"
            | "xmlattrname"
            | "xmlattrvalue"
            | "generator"
    )
}

fn html_to_text(html: &str) -> String {
    let mut text = String::with_capacity(html.len());
    let mut cursor = 0;

    while cursor < html.len() {
        let Some(tag_offset) = html[cursor..].find('<') else {
            append_html_text(&html[cursor..], &mut text);
            break;
        };

        let tag_start = cursor + tag_offset;
        append_html_text(&html[cursor..tag_start], &mut text);

        let Some(tag_end_offset) = html[tag_start..].find('>') else {
            append_html_text(&html[tag_start..], &mut text);
            break;
        };

        let tag_end = tag_start + tag_end_offset;
        let tag = &html[tag_start + 1..tag_end];
        let tag_name = html_tag_name(tag);
        if tag.trim_start().starts_with("!--") {
            cursor = tag_end + 1;
            continue;
        }

        if !tag.trim_start().starts_with('/')
            && matches!(tag_name.as_str(), "script" | "style" | "head" | "noscript")
        {
            let close_tag = format!("</{tag_name}");
            let after_tag = tag_end + 1;
            if let Some(close_offset) = find_ascii_case_insensitive(&html[after_tag..], &close_tag)
            {
                let close_start = after_tag + close_offset;
                if let Some(close_end_offset) = html[close_start..].find('>') {
                    cursor = close_start + close_end_offset + 1;
                    continue;
                }
            }
        }

        apply_html_tag_spacing(&tag_name, tag.trim_start().starts_with('/'), &mut text);
        cursor = tag_end + 1;
    }

    normalize_plain_text_body(&text)
}

fn append_html_text(segment: &str, output: &mut String) {
    let mut chars = segment.chars().peekable();
    while let Some(char) = chars.next() {
        if char == '&' {
            let mut entity = String::new();
            while let Some(next) = chars.peek().copied() {
                if next == ';' {
                    chars.next();
                    break;
                }
                if entity.len() >= 32 || next.is_whitespace() {
                    break;
                }
                entity.push(next);
                chars.next();
            }

            if let Some(decoded) = decode_html_entity(&entity) {
                append_html_text(&decoded, output);
            } else {
                append_text_char('&', output);
                for entity_char in entity.chars() {
                    append_text_char(entity_char, output);
                }
                if !entity.is_empty() {
                    append_text_char(';', output);
                }
            }
            continue;
        }

        append_text_char(char, output);
    }
}

fn append_text_char(char: char, output: &mut String) {
    if char.is_whitespace() {
        if !output.is_empty()
            && !output.ends_with(' ')
            && !output.ends_with('\n')
            && !output.ends_with('\t')
        {
            output.push(' ');
        }
    } else {
        output.push(char);
    }
}

fn html_tag_name(tag: &str) -> String {
    tag.trim_start_matches('/')
        .trim_start()
        .chars()
        .take_while(|char| char.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn apply_html_tag_spacing(tag_name: &str, closing: bool, output: &mut String) {
    match tag_name {
        "br" => ensure_newline(output),
        "li" if !closing => {
            ensure_newline(output);
            output.push_str("- ");
        }
        "p" | "div" | "section" | "article" | "header" | "footer" | "main" | "aside" | "table"
        | "tbody" | "thead" | "tfoot" | "tr" | "blockquote" | "pre" | "h1" | "h2" | "h3" | "h4"
        | "h5" | "h6" | "ul" | "ol" => ensure_blank_line(output),
        "td" | "th" if !output.is_empty() && !output.ends_with(' ') && !output.ends_with('\n') => {
            output.push(' ');
        }
        _ => {}
    }
}

fn ensure_newline(output: &mut String) {
    if !output.is_empty() && !output.ends_with('\n') {
        output.push('\n');
    }
}

fn ensure_blank_line(output: &mut String) {
    if output.is_empty() {
        return;
    }
    if output.ends_with("\n\n") {
        return;
    }
    if output.ends_with('\n') {
        output.push('\n');
    } else {
        output.push_str("\n\n");
    }
}

fn decode_html_entity(entity: &str) -> Option<String> {
    if let Some(hex) = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"))
    {
        return u32::from_str_radix(hex, 16)
            .ok()
            .and_then(char::from_u32)
            .map(|char| char.to_string());
    }
    if let Some(decimal) = entity.strip_prefix('#') {
        return decimal
            .parse::<u32>()
            .ok()
            .and_then(char::from_u32)
            .map(|char| char.to_string());
    }

    match entity {
        "nbsp" => Some(" ".to_string()),
        "amp" => Some("&".to_string()),
        "lt" => Some("<".to_string()),
        "gt" => Some(">".to_string()),
        "quot" => Some("\"".to_string()),
        "apos" | "#39" => Some("'".to_string()),
        "lsquo" | "rsquo" => Some("'".to_string()),
        "ldquo" | "rdquo" => Some("\"".to_string()),
        "ndash" | "mdash" => Some("-".to_string()),
        "hellip" => Some("...".to_string()),
        "copy" => Some("(c)".to_string()),
        "reg" => Some("(r)".to_string()),
        "trade" => Some("(tm)".to_string()),
        _ => None,
    }
}

fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    let haystack = haystack.as_bytes();
    let needle = needle.as_bytes();
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }

    haystack
        .windows(needle.len())
        .position(|window| window.eq_ignore_ascii_case(needle))
}

fn html_has_remote_images(html: &str) -> bool {
    html_img_tags(html).any(|tag| {
        let Some(src) = html_tag_attribute(&tag, "src") else {
            return false;
        };
        is_http_or_https_url(src.trim())
    })
}

fn html_has_cid_images(html: &str) -> bool {
    html_img_tags(html).any(|tag| {
        let Some(src) = html_tag_attribute(&tag, "src") else {
            return false;
        };
        src.trim()
            .get(0..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cid:"))
    })
}

fn html_img_tags(html: &str) -> impl Iterator<Item = String> + '_ {
    let mut cursor = 0usize;
    std::iter::from_fn(move || loop {
        let open_offset = html[cursor..].find('<')?;
        let open = cursor + open_offset;
        let close_offset = html[open..].find('>')?;
        let close = open + close_offset;
        cursor = close + 1;
        let tag = &html[open + 1..close];
        if html_tag_name(tag) == "img" {
            return Some(tag.to_string());
        }
    })
}

fn html_tag_attribute<'a>(tag: &'a str, attribute: &str) -> Option<&'a str> {
    let bytes = tag.as_bytes();
    let attr_bytes = attribute.as_bytes();
    let mut cursor = 0usize;

    while cursor + attr_bytes.len() <= bytes.len() {
        if bytes[cursor..cursor + attr_bytes.len()].eq_ignore_ascii_case(attr_bytes) {
            let before_ok = cursor == 0
                || bytes[cursor - 1].is_ascii_whitespace()
                || matches!(bytes[cursor - 1], b'/' | b'<');
            let mut after = cursor + attr_bytes.len();
            while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                after += 1;
            }
            if before_ok && after < bytes.len() && bytes[after] == b'=' {
                after += 1;
                while after < bytes.len() && bytes[after].is_ascii_whitespace() {
                    after += 1;
                }
                if after >= bytes.len() {
                    return None;
                }
                if matches!(bytes[after], b'"' | b'\'') {
                    let quote = bytes[after];
                    let value_start = after + 1;
                    let value_end = bytes[value_start..]
                        .iter()
                        .position(|byte| *byte == quote)
                        .map(|offset| value_start + offset)
                        .unwrap_or(bytes.len());
                    return tag.get(value_start..value_end);
                }
                let value_start = after;
                let value_end = bytes[value_start..]
                    .iter()
                    .position(|byte| byte.is_ascii_whitespace() || *byte == b'/')
                    .map(|offset| value_start + offset)
                    .unwrap_or(bytes.len());
                return tag.get(value_start..value_end);
            }
        }
        cursor += 1;
    }

    None
}

struct SanitizedEmailHtml {
    sanitized_html: String,
    remote_images_blocked: bool,
    remote_image_count: usize,
    embedded_image_count: usize,
}

fn sanitize_email_html(body_html: &str, cid_images: HashMap<String, String>) -> SanitizedEmailHtml {
    let remote_image_count = html_img_tags(body_html)
        .filter_map(|tag| html_tag_attribute(&tag, "src").map(str::to_string))
        .filter(|src| is_blocked_external_image_reference(src))
        .count();
    let embedded_image_count = Arc::new(AtomicUsize::new(0));
    let embedded_image_count_for_filter = Arc::clone(&embedded_image_count);

    let allowed_tags = HashSet::from([
        "a",
        "abbr",
        "b",
        "blockquote",
        "br",
        "caption",
        "cite",
        "code",
        "col",
        "colgroup",
        "dd",
        "del",
        "div",
        "dl",
        "dt",
        "em",
        "figcaption",
        "figure",
        "h1",
        "h2",
        "h3",
        "h4",
        "h5",
        "h6",
        "hr",
        "i",
        "img",
        "ins",
        "li",
        "ol",
        "p",
        "pre",
        "q",
        "s",
        "small",
        "span",
        "strong",
        "sub",
        "sup",
        "table",
        "tbody",
        "td",
        "th",
        "thead",
        "tfoot",
        "tr",
        "u",
        "ul",
    ]);
    let mut tag_attributes = HashMap::new();
    tag_attributes.insert(
        "img",
        HashSet::from([
            "src", "alt", "title", "width", "height", "align", "valign", "border",
        ]),
    );
    tag_attributes.insert("ol", HashSet::from(["start"]));
    tag_attributes.insert(
        "table",
        HashSet::from([
            "border",
            "cellpadding",
            "cellspacing",
            "width",
            "height",
            "align",
        ]),
    );
    tag_attributes.insert("tbody", HashSet::from(["align", "valign"]));
    tag_attributes.insert("thead", HashSet::from(["align", "valign"]));
    tag_attributes.insert("tfoot", HashSet::from(["align", "valign"]));
    tag_attributes.insert("tr", HashSet::from(["align", "valign"]));
    tag_attributes.insert(
        "td",
        HashSet::from([
            "border", "colspan", "rowspan", "width", "height", "align", "valign",
        ]),
    );
    tag_attributes.insert(
        "th",
        HashSet::from([
            "border", "colspan", "rowspan", "scope", "width", "height", "align", "valign",
        ]),
    );
    tag_attributes.insert("col", HashSet::from(["span", "width", "align", "valign"]));
    tag_attributes.insert(
        "colgroup",
        HashSet::from(["span", "width", "align", "valign"]),
    );

    let clean_content_tags = HashSet::from([
        "script", "style", "iframe", "object", "embed", "form", "input", "button", "textarea",
        "select", "option", "meta", "link",
    ]);
    let url_schemes = HashSet::from(["cid", "data"]);

    let mut builder = ammonia::Builder::new();
    builder
        .tags(allowed_tags)
        .tag_attributes(tag_attributes)
        .generic_attributes(HashSet::new())
        .clean_content_tags(clean_content_tags)
        .url_schemes(url_schemes)
        .url_relative(ammonia::UrlRelative::Deny)
        .link_rel(None)
        .attribute_filter(move |element, attribute, value| {
            if attribute
                .get(0..2)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("on"))
            {
                return None;
            }

            if !is_safe_legacy_layout_attribute(attribute, value) {
                return None;
            }

            if element != "img" || attribute != "src" {
                return Some(Cow::Borrowed(value));
            }

            let trimmed = value.trim();
            if trimmed
                .get(0..4)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cid:"))
            {
                let cid = normalize_cid_value(trimmed);
                if let Some(data_url) = cid_images.get(&cid) {
                    embedded_image_count_for_filter.fetch_add(1, Ordering::SeqCst);
                    return Some(Cow::Owned(data_url.clone()));
                }
                return None;
            }

            if is_safe_image_data_url(trimmed) {
                return Some(Cow::Borrowed(value));
            }

            // Any image source that is not a resolved CID or an allowlisted data image must
            // never reach the WebView or printable HTML.
            None
        });

    let sanitized_html = builder.clean(body_html).to_string();
    SanitizedEmailHtml {
        sanitized_html,
        remote_images_blocked: remote_image_count > 0,
        remote_image_count,
        embedded_image_count: embedded_image_count.load(Ordering::SeqCst),
    }
}

fn sanitize_printable_html_document(html: &str) -> AppResult<String> {
    validate_printable_html_size(html.len())?;

    // Treat the frontend document as untrusted input. The mail sanitizer removes its head,
    // scripts, style, links, forms, frames, and remote resources; only the fixed local wrapper
    // below can restore document structure or CSS.
    let body = sanitize_email_html(html, HashMap::new()).sanitized_html;
    let mut document = String::with_capacity(body.len().saturating_add(1_024));
    document.push_str(
        r#"<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; img-src data:; style-src 'unsafe-inline'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; connect-src 'none'">
  <title>PST QuickView Printable Message</title>
  <style>
    @page { margin: 0.6in; }
    body { margin: 0; color: #111827; font: 13px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif; overflow-wrap: anywhere; }
    h1 { margin: 0 0 14px; font-size: 20px; line-height: 1.25; }
    dl { display: grid; grid-template-columns: max-content minmax(0, 1fr); gap: 4px 12px; margin: 0 0 14px; padding-bottom: 12px; border-bottom: 1px solid #d8dee6; }
    dt { color: #5f6b7a; font-weight: 700; }
    dd { margin: 0; }
    pre { margin: 0; white-space: pre-wrap; font: inherit; }
    img, table { max-width: 100%; }
    img { height: auto; }
    table { border-collapse: collapse; }
  </style>
</head>
<body>
"#,
    );
    document.push_str(&body);
    document.push_str("\n</body>\n</html>\n");
    Ok(document)
}

fn validate_printable_html_size(size: usize) -> AppResult<()> {
    if size > MAX_PRINTABLE_HTML_BYTES {
        return Err(AppError::new(
            "Printable HTML exceeds the supported size limit.",
        ));
    }
    Ok(())
}

fn is_blocked_external_image_reference(value: &str) -> bool {
    let value = value.trim();
    if value.is_empty()
        || value
            .get(0..4)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("cid:"))
    {
        return false;
    }

    !is_safe_image_data_url(value)
}

fn is_safe_legacy_layout_attribute(attribute: &str, value: &str) -> bool {
    match attribute {
        "width" | "height" => is_safe_legacy_dimension(value),
        "border" | "cellpadding" | "cellspacing" | "colspan" | "rowspan" | "span" => value
            .trim()
            .parse::<u32>()
            .is_ok_and(|number| number <= 1_000),
        "align" => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "left" | "right" | "center" | "justify" | "char" | "top" | "middle" | "bottom"
        ),
        "valign" => matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "top" | "middle" | "bottom" | "baseline"
        ),
        _ => true,
    }
}

fn is_safe_legacy_dimension(value: &str) -> bool {
    let value = value.trim();
    if let Some(percent) = value.strip_suffix('%') {
        return percent.parse::<u32>().is_ok_and(|number| number <= 100);
    }

    value.parse::<u32>().is_ok_and(|number| number <= 10_000)
}

fn load_cid_images_for_message(
    workspace: &Path,
    relative_eml_path: &str,
) -> AppResult<HashMap<String, String>> {
    let extracted_root = workspace.join("extracted");
    let eml_path = extracted_root.join(relative_eml_path);
    if !eml_path.exists() {
        return Ok(HashMap::new());
    }
    if !path_is_under_root(&eml_path, &extracted_root)? {
        return Err(AppError::new(
            "Refusing to read an EML outside this workspace.",
        ));
    }

    load_cid_images_from_eml_path(&eml_path)
}

fn load_cid_images_from_eml_path(eml_path: &Path) -> AppResult<HashMap<String, String>> {
    let bytes = read_bounded_message_source(eml_path)?;
    let parsed = parse_mail_bytes(&bytes)?;
    let mut cid_images = HashMap::new();
    collect_cid_images(&parsed, &mut cid_images);
    Ok(cid_images)
}

fn collect_cid_images(part: &mailparse::ParsedMail<'_>, cid_images: &mut HashMap<String, String>) {
    if !part.subparts.is_empty() {
        for child in &part.subparts {
            collect_cid_images(child, cid_images);
        }
        return;
    }

    let content_id = part
        .headers
        .get_first_value("Content-ID")
        .map(|value| normalize_cid_value(&value))
        .filter(|value| !value.is_empty());
    let Some(content_id) = content_id else {
        return;
    };

    let content_type = part.ctype.mimetype.to_ascii_lowercase();
    if !is_safe_embedded_image_mime(&content_type) {
        return;
    }

    let Ok(bytes) = part.get_body_raw() else {
        return;
    };
    if bytes.is_empty() || bytes.len() > MAX_EMBEDDED_IMAGE_BYTES {
        return;
    }

    let data_url = format!(
        "data:{};base64,{}",
        content_type,
        BASE64_STANDARD.encode(bytes)
    );
    cid_images.insert(content_id, data_url);
}

fn normalize_cid_value(value: &str) -> String {
    let trimmed = value.trim();
    let without_scheme = trimmed
        .get(0..4)
        .filter(|prefix| prefix.eq_ignore_ascii_case("cid:"))
        .map(|_| &trimmed[4..])
        .unwrap_or(trimmed)
        .trim()
        .trim_matches('<')
        .trim_matches('>')
        .trim();
    percent_decode_ascii(without_scheme).to_ascii_lowercase()
}

fn percent_decode_ascii(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
            {
                output.push(high * 16 + low);
                index += 3;
                continue;
            }
        }

        output.push(bytes[index]);
        index += 1;
    }

    String::from_utf8_lossy(&output).to_string()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn is_http_or_https_url(value: &str) -> bool {
    value
        .get(0..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
        || value
            .get(0..8)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
}

fn is_safe_embedded_image_mime(content_type: &str) -> bool {
    matches!(
        content_type,
        "image/png" | "image/jpeg" | "image/jpg" | "image/gif" | "image/webp" | "image/bmp"
    )
}

fn is_safe_image_data_url(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let Some((metadata, payload)) = lower.split_once(',') else {
        return false;
    };
    let Some(content_type) = metadata.strip_prefix("data:") else {
        return false;
    };
    if !content_type.ends_with(";base64") {
        return false;
    }
    let mime = content_type.trim_end_matches(";base64");
    if !is_safe_embedded_image_mime(mime) {
        return false;
    }
    !payload.is_empty()
        && payload
            .chars()
            .all(|char| char.is_ascii_alphanumeric() || matches!(char, '+' | '/' | '=' | '-' | '_'))
}

fn make_snippet(body: &str) -> String {
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    collapsed.chars().take(220).collect()
}

struct BoundedPage<T> {
    items: Vec<T>,
    requested_offset: i64,
    returned_count: usize,
    has_more: bool,
}

fn finish_bounded_page<T>(
    mut items: Vec<T>,
    requested_limit: i64,
    requested_offset: i64,
) -> BoundedPage<T> {
    let requested_limit = requested_limit.max(1) as usize;
    let has_more = items.len() > requested_limit;
    if has_more {
        items.truncate(requested_limit);
    }
    BoundedPage {
        returned_count: items.len(),
        items,
        requested_offset: requested_offset.max(0),
        has_more,
    }
}

#[allow(clippy::too_many_arguments)]
fn query_messages_cursor_page_visible(
    conn: &Connection,
    active: &ActiveWorkspace,
    folder_id: Option<i64>,
    include_subfolders: bool,
    criteria: &MessageSearchCriteria,
    visibility: MessageVisibility,
    sort_order: Option<&str>,
    limit: Option<i64>,
    cursor: Option<&str>,
    search_generation: u64,
    cursor_codec: &SearchCursorCodec,
    operation: Option<&SearchOperationGuard>,
) -> AppResult<MessagePageResult> {
    check_search_operation(operation)?;
    let page_limit = limit
        .unwrap_or(DEFAULT_MESSAGE_PAGE_SIZE)
        .clamp(1, MAX_MESSAGE_PAGE_SIZE);
    let (source, local_state) = visible_message_query_source(
        conn,
        active,
        visibility,
        folder_id,
        include_subfolders,
        criteria,
        operation,
    )?;
    let sort = MessageSort::from_request(sort_order, source.has_text_match)?;
    let effective_folder_id = if matches!(visibility, MessageVisibility::Trashed) {
        None
    } else {
        folder_id
    };
    let effective_include_subfolders =
        !matches!(visibility, MessageVisibility::Trashed) && include_subfolders;
    let cursor_context = MessageCursorContext {
        workspace_hash: opaque_hash(active.id.as_bytes()),
        criteria_hash: criteria.cursor_fingerprint_for_visibility(
            effective_folder_id,
            effective_include_subfolders,
            visibility,
        ),
        index_generation: workspace_search_index_generation_with_local_state(
            &active.path,
            conn,
            &local_state,
        )?,
        sort,
        search_generation,
    };
    check_search_operation(operation)?;

    let (where_sql, mut page_params) = if let Some(encoded_cursor) = cursor {
        let position = cursor_codec.decode(encoded_cursor, &cursor_context)?;
        let boundary = resolve_message_keyset_boundary(conn, &source, sort, position.message_id)?
            .ok_or(SearchCursorError::Stale)?;
        check_search_operation(operation)?;
        let (condition, boundary_params) = message_keyset_condition(&boundary);
        let (where_sql, mut params) = query_source_with_condition(&source, &condition);
        params.extend(boundary_params);
        (where_sql, params)
    } else {
        (source.where_sql.clone(), source.params.clone())
    };

    let sql = message_cursor_page_sql(&source, &where_sql, sort);
    page_params.push(Value::Integer(page_limit + 1));
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(page_params.iter()), row_to_message_item)?;
    let page = finish_bounded_page(collect_rows(rows)?, page_limit, 0);
    check_search_operation(operation)?;
    let next_cursor = if page.has_more {
        page.items.last().map(|item| {
            cursor_codec.encode(
                &cursor_context,
                MessageCursorPosition {
                    message_id: item.id,
                },
            )
        })
    } else {
        None
    };

    Ok(MessagePageResult {
        items: page.items,
        requested_offset: 0,
        returned_count: page.returned_count,
        has_more: page.has_more,
        next_cursor,
        pagination_mode: "cursor",
        local_trash_states: vec![local_trash_query_health(&active.id, &local_state)],
    })
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn query_messages_cursor_page(
    conn: &Connection,
    workspace: &Path,
    workspace_id: &str,
    folder_id: Option<i64>,
    include_subfolders: bool,
    criteria: &MessageSearchCriteria,
    sort_order: Option<&str>,
    limit: Option<i64>,
    cursor: Option<&str>,
    search_generation: u64,
    cursor_codec: &SearchCursorCodec,
    operation: Option<&SearchOperationGuard>,
) -> AppResult<MessagePageResult> {
    check_search_operation(operation)?;
    let page_limit = limit
        .unwrap_or(DEFAULT_MESSAGE_PAGE_SIZE)
        .clamp(1, MAX_MESSAGE_PAGE_SIZE);
    let source = build_message_query_source(conn, folder_id, include_subfolders, criteria)?;
    let sort = MessageSort::from_request(sort_order, source.has_text_match)?;
    let cursor_context = MessageCursorContext {
        workspace_hash: opaque_hash(workspace_id.as_bytes()),
        criteria_hash: criteria.cursor_fingerprint(folder_id, include_subfolders),
        index_generation: workspace_search_index_generation(workspace, conn)?,
        sort,
        search_generation,
    };
    check_search_operation(operation)?;

    let (where_sql, mut page_params) = if let Some(encoded_cursor) = cursor {
        let position = cursor_codec.decode(encoded_cursor, &cursor_context)?;
        let boundary = resolve_message_keyset_boundary(conn, &source, sort, position.message_id)?
            .ok_or(SearchCursorError::Stale)?;
        check_search_operation(operation)?;
        let (condition, boundary_params) = message_keyset_condition(&boundary);
        let (where_sql, mut params) = query_source_with_condition(&source, &condition);
        params.extend(boundary_params);
        (where_sql, params)
    } else {
        (source.where_sql.clone(), source.params.clone())
    };

    let sql = message_cursor_page_sql(&source, &where_sql, sort);

    page_params.push(Value::Integer(page_limit + 1));
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(page_params.iter()), row_to_message_item)?;
    let page = finish_bounded_page(collect_rows(rows)?, page_limit, 0);
    check_search_operation(operation)?;
    let next_cursor = if page.has_more {
        page.items.last().map(|item| {
            cursor_codec.encode(
                &cursor_context,
                MessageCursorPosition {
                    message_id: item.id,
                },
            )
        })
    } else {
        None
    };

    Ok(MessagePageResult {
        items: page.items,
        requested_offset: 0,
        returned_count: page.returned_count,
        has_more: page.has_more,
        next_cursor,
        pagination_mode: "cursor",
        local_trash_states: Vec::new(),
    })
}

fn message_cursor_page_sql(
    source: &MessageQuerySource,
    where_sql: &str,
    sort: MessageSort,
) -> String {
    let mut sql = String::from(
        "SELECT m.id,
                m.folder_id,
                f.path,
                f.name,
                m.subject,
                m.sender,
                m.recipients,
                m.date,
                m.snippet,
                m.has_attachments,
                (SELECT COUNT(*) FROM attachments a WHERE a.message_id = m.id) AS attachment_count",
    );
    sql.push_str(search_match_select_sql(source.has_text_match));
    sql.push_str(&source.from_sql);
    sql.push_str(where_sql);
    sql.push_str("ORDER BY ");
    sql.push_str(sort.sql());
    sql.push_str(" LIMIT ?");
    sql
}

fn workspace_search_index_generation(workspace: &Path, conn: &Connection) -> AppResult<[u8; 16]> {
    let mut hasher = Sha256::new();
    hasher.update(b"pst-quickview-search-index-generation-v1");
    for path in [
        workspace.join("index.sqlite"),
        workspace.join("index.sqlite-wal"),
    ] {
        match fs::metadata(path) {
            Ok(metadata) => {
                hasher.update([1]);
                hasher.update(metadata.dev().to_be_bytes());
                hasher.update(metadata.ino().to_be_bytes());
                hasher.update(metadata.len().to_be_bytes());
                let modified_ns = metadata
                    .modified()
                    .ok()
                    .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos())
                    .unwrap_or_default();
                hasher.update(modified_ns.to_be_bytes());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => hasher.update([0]),
            Err(_) => {
                return Err(AppError::new(
                    "Could not inspect the workspace index generation.",
                ))
            }
        }
    }

    for pragma in ["user_version", "schema_version"] {
        let value = conn.query_row(&format!("PRAGMA {pragma}"), [], |row| row.get::<_, i64>(0))?;
        hasher.update((pragma.len() as u64).to_be_bytes());
        hasher.update(pragma.as_bytes());
        hasher.update(value.to_be_bytes());
    }
    for key in [
        "workspace_id",
        "pst_content_fingerprint",
        "pst_fingerprint",
        "import_status",
        "updated_at",
        "last_reindex_at",
        "message_count_indexed",
        "conversation_schema_version",
        "attachment_metadata_schema_version",
        "body_html_schema_version",
    ] {
        hasher.update((key.len() as u64).to_be_bytes());
        hasher.update(key.as_bytes());
        if let Some(value) = metadata_value(conn, key)? {
            hasher.update([1]);
            hasher.update((value.len() as u64).to_be_bytes());
            hasher.update(value.as_bytes());
        } else {
            hasher.update([0]);
        }
    }

    Ok(opaque_hash(&hasher.finalize()))
}

fn workspace_search_index_generation_with_local_state(
    workspace: &Path,
    conn: &Connection,
    local_state: &local_trash::LocalTrashQueryState,
) -> AppResult<[u8; 16]> {
    let canonical_generation = workspace_search_index_generation(workspace, conn)?;
    let mut hasher = Sha256::new();
    hasher.update(b"pst-quickview-search-and-local-state-generation-v1");
    hasher.update(canonical_generation);
    if let Some(generation) = &local_state.generation {
        hasher.update([1]);
        hasher.update((generation.len() as u64).to_be_bytes());
        hasher.update(generation.as_bytes());
    } else {
        hasher.update([0]);
    }
    hasher.update(local_state.revision.to_be_bytes());
    hasher.update((local_state.stored_count as u64).to_be_bytes());
    if let Some(error_code) = local_state.last_error_code {
        hasher.update([1]);
        hasher.update(error_code.as_bytes());
    } else {
        hasher.update([0]);
    }
    Ok(opaque_hash(&hasher.finalize()))
}

#[cfg(test)]
struct MessagePageQuery<'a> {
    conn: &'a Connection,
    folder_id: Option<i64>,
    include_subfolders: bool,
    criteria: &'a MessageSearchCriteria,
    sort_order: Option<&'a str>,
    limit: Option<i64>,
    offset: Option<i64>,
    operation: Option<&'a SearchOperationGuard>,
}

#[cfg(test)]
fn query_messages_page(input: MessagePageQuery<'_>) -> AppResult<MessagePageResult> {
    let MessagePageQuery {
        conn,
        folder_id,
        include_subfolders,
        criteria,
        operation,
        sort_order,
        limit,
        offset,
    } = input;
    check_search_operation(operation)?;
    let source = build_message_query_source(conn, folder_id, include_subfolders, criteria)?;
    query_messages_page_from_source(conn, source, sort_order, limit, offset, operation)
}

fn query_messages_page_from_source(
    conn: &Connection,
    source: MessageQuerySource,
    sort_order: Option<&str>,
    limit: Option<i64>,
    offset: Option<i64>,
    operation: Option<&SearchOperationGuard>,
) -> AppResult<MessagePageResult> {
    check_search_operation(operation)?;
    let page_limit = limit
        .unwrap_or(DEFAULT_MESSAGE_PAGE_SIZE)
        .clamp(1, MAX_MESSAGE_PAGE_SIZE);
    let page_offset = offset.unwrap_or(0).max(0);
    let MessageQuerySource {
        from_sql,
        where_sql,
        params: query_params,
        has_text_match,
    } = source;
    let sort_clause = message_sort_clause(sort_order, has_text_match)?;

    let mut sql = String::from(
        "SELECT m.id,
                m.folder_id,
                f.path,
                f.name,
                m.subject,
                m.sender,
                m.recipients,
                m.date,
                m.snippet,
                m.has_attachments,
                (SELECT COUNT(*) FROM attachments a WHERE a.message_id = m.id) AS attachment_count",
    );
    sql.push_str(search_match_select_sql(has_text_match));
    sql.push_str(&from_sql);
    sql.push_str(&where_sql);
    sql.push_str("ORDER BY ");
    sql.push_str(sort_clause);
    sql.push_str(" LIMIT ? OFFSET ?");

    let mut page_params = query_params;
    page_params.push(Value::Integer(page_limit + 1));
    page_params.push(Value::Integer(page_offset));
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(page_params.iter()), row_to_message_item)?;
    let page = finish_bounded_page(collect_rows(rows)?, page_limit, page_offset);
    check_search_operation(operation)?;
    Ok(MessagePageResult {
        items: page.items,
        requested_offset: page.requested_offset,
        returned_count: page.returned_count,
        has_more: page.has_more,
        next_cursor: None,
        pagination_mode: "offset",
        local_trash_states: Vec::new(),
    })
}

#[cfg(test)]
fn query_message_count(
    conn: &Connection,
    folder_id: Option<i64>,
    include_subfolders: bool,
    criteria: &MessageSearchCriteria,
    operation: Option<&SearchOperationGuard>,
) -> AppResult<i64> {
    check_search_operation(operation)?;
    let source = build_message_query_source(conn, folder_id, include_subfolders, criteria)?;
    check_search_operation(operation)?;
    let sql = format!("SELECT COUNT(*){}{}", source.from_sql, source.where_sql);
    let count = conn.query_row(&sql, params_from_iter(source.params.iter()), |row| {
        row.get(0)
    })?;
    check_search_operation(operation)?;
    Ok(count)
}

fn query_message_count_visible(
    conn: &Connection,
    active: &ActiveWorkspace,
    folder_id: Option<i64>,
    include_subfolders: bool,
    criteria: &MessageSearchCriteria,
    visibility: MessageVisibility,
    operation: Option<&SearchOperationGuard>,
) -> AppResult<i64> {
    check_search_operation(operation)?;
    let (source, _) = visible_message_query_source(
        conn,
        active,
        visibility,
        folder_id,
        include_subfolders,
        criteria,
        operation,
    )?;
    check_search_operation(operation)?;
    let sql = format!("SELECT COUNT(*){}{}", source.from_sql, source.where_sql);
    let count = conn.query_row(&sql, params_from_iter(source.params.iter()), |row| {
        row.get(0)
    })?;
    check_search_operation(operation)?;
    Ok(count)
}

fn check_search_operation(operation: Option<&SearchOperationGuard>) -> AppResult<()> {
    operation
        .map(SearchOperationGuard::check_cancelled)
        .transpose()?;
    Ok(())
}

#[cfg(test)]
fn conversation_counts(
    conn: &Connection,
    folder_id: Option<i64>,
    include_subfolders: bool,
    criteria: &MessageSearchCriteria,
    operation: Option<&SearchOperationGuard>,
) -> AppResult<(i64, i64)> {
    check_search_operation(operation)?;
    let source = build_message_query_source(conn, folder_id, include_subfolders, criteria)?;
    let (where_sql, params) =
        query_source_with_condition(&source, "COALESCE(m.conversation_id, '') <> ''");
    let sql = format!(
        "SELECT COUNT(DISTINCT m.conversation_id), COUNT(*){}{}",
        source.from_sql, where_sql
    );
    let counts = conn.query_row(&sql, params_from_iter(params.iter()), |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    check_search_operation(operation)?;
    Ok(counts)
}

fn conversation_counts_visible(
    conn: &Connection,
    active: &ActiveWorkspace,
    folder_id: Option<i64>,
    include_subfolders: bool,
    criteria: &MessageSearchCriteria,
    operation: Option<&SearchOperationGuard>,
) -> AppResult<(i64, i64)> {
    check_search_operation(operation)?;
    let (source, _) = visible_message_query_source(
        conn,
        active,
        MessageVisibility::Active,
        folder_id,
        include_subfolders,
        criteria,
        operation,
    )?;
    let (where_sql, params) =
        query_source_with_condition(&source, "COALESCE(m.conversation_id, '') <> ''");
    let sql = format!(
        "SELECT COUNT(DISTINCT m.conversation_id), COUNT(*){}{}",
        source.from_sql, where_sql
    );
    let counts = conn.query_row(&sql, params_from_iter(params.iter()), |row| {
        Ok((row.get(0)?, row.get(1)?))
    })?;
    check_search_operation(operation)?;
    Ok(counts)
}

#[allow(clippy::too_many_arguments)]
#[cfg(test)]
fn query_conversation_summaries(
    conn: &Connection,
    active: &ActiveWorkspace,
    folder_id: Option<i64>,
    include_subfolders: bool,
    criteria: &MessageSearchCriteria,
    sort: &str,
    limit: i64,
    offset: i64,
    operation: Option<&SearchOperationGuard>,
) -> AppResult<Vec<ConversationSummary>> {
    query_conversation_summaries_impl(
        conn,
        active,
        folder_id,
        include_subfolders,
        criteria,
        sort,
        limit,
        offset,
        operation,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn query_conversation_summaries_visible(
    conn: &Connection,
    active: &ActiveWorkspace,
    scope: &local_trash::LocalTrashScope,
    folder_id: Option<i64>,
    include_subfolders: bool,
    criteria: &MessageSearchCriteria,
    sort: &str,
    limit: i64,
    offset: i64,
    operation: Option<&SearchOperationGuard>,
) -> AppResult<Vec<ConversationSummary>> {
    query_conversation_summaries_impl(
        conn,
        active,
        folder_id,
        include_subfolders,
        criteria,
        sort,
        limit,
        offset,
        operation,
        Some(scope),
    )
}

#[allow(clippy::too_many_arguments)]
fn query_conversation_summaries_impl(
    conn: &Connection,
    active: &ActiveWorkspace,
    folder_id: Option<i64>,
    include_subfolders: bool,
    criteria: &MessageSearchCriteria,
    sort: &str,
    limit: i64,
    offset: i64,
    operation: Option<&SearchOperationGuard>,
    local_scope: Option<&local_trash::LocalTrashScope>,
) -> AppResult<Vec<ConversationSummary>> {
    check_search_operation(operation)?;
    let source = if let Some(scope) = local_scope {
        visible_message_query_source_after_materialization(
            conn,
            scope,
            MessageVisibility::Active,
            folder_id,
            include_subfolders,
            criteria,
        )?
    } else {
        build_message_query_source(conn, folder_id, include_subfolders, criteria)?
    };
    let (where_sql, mut query_params) =
        query_source_with_condition(&source, "COALESCE(m.conversation_id, '') <> ''");
    let root_active = local_scope
        .map(|scope| local_trash_membership_predicate("root", scope, false))
        .transpose()?
        .unwrap_or_else(|| "1 = 1".to_string());
    let participant_active = local_scope
        .map(|scope| local_trash_membership_predicate("participant_messages", scope, false))
        .transpose()?
        .unwrap_or_else(|| "1 = 1".to_string());
    let all_active = local_scope
        .map(|scope| local_trash_membership_predicate("all_messages", scope, false))
        .transpose()?
        .unwrap_or_else(|| "1 = 1".to_string());
    let attachment_active = local_scope
        .map(|scope| local_trash_membership_predicate("attachment_messages", scope, false))
        .transpose()?
        .unwrap_or_else(|| "1 = 1".to_string());
    let root_id_select = if local_scope.is_some() {
        format!(
            "COALESCE(
                 (SELECT root.id
                    FROM messages root
                   WHERE root.id = matched.conversation_root_id
                     AND {root_active}),
                 matched.id
             )"
        )
    } else {
        "matched.conversation_root_id".to_string()
    };
    let mut sql = format!(
        "WITH matched AS (
            SELECT m.id,
                   m.conversation_id,
                   m.conversation_root_id,
                   m.subject,
                   m.sender,
                   m.date,
                   m.snippet,
                   m.thread_assignment_method,
                   ROW_NUMBER() OVER (
                       PARTITION BY m.conversation_id
                       ORDER BY m.date DESC, m.id DESC
                   ) AS activity_rank,
                   COUNT(*) OVER (PARTITION BY m.conversation_id) AS matching_count
              {}{}
         )
         SELECT matched.conversation_id,
                {root_id_select},
                COALESCE(
                    (SELECT NULLIF(root.normalized_subject, '')
                       FROM messages root
                      WHERE root.id = matched.conversation_root_id
                        AND {root_active}),
                    NULLIF(matched.subject, ''),
                    '(no subject)'
                ) AS conversation_subject,
                COALESCE(matched.sender, ''),
                COALESCE((
                    SELECT group_concat(participant.sender_value, char(31))
                      FROM (
                          SELECT TRIM(participant_messages.sender) AS sender_value,
                                 MAX(COALESCE(participant_messages.date, '')) AS latest_date,
                                 MAX(participant_messages.id) AS latest_id
                           FROM messages participant_messages
                           WHERE participant_messages.conversation_id = matched.conversation_id
                             AND {participant_active}
                             AND TRIM(COALESCE(participant_messages.sender, '')) <> ''
                           GROUP BY TRIM(participant_messages.sender) COLLATE NOCASE
                           ORDER BY latest_date DESC, latest_id DESC
                      ) participant
                ), '') AS participants,
                COALESCE(matched.date, ''),
                COALESCE(matched.snippet, ''),
                matched.matching_count,
                (SELECT COUNT(*)
                   FROM messages all_messages
                  WHERE all_messages.conversation_id = matched.conversation_id
                    AND {all_active}) AS total_count,
                EXISTS (
                    SELECT 1
                      FROM messages attachment_messages
                     WHERE attachment_messages.conversation_id = matched.conversation_id
                       AND {attachment_active}
                       AND (
                           attachment_messages.has_attachments != 0
                           OR EXISTS (
                               SELECT 1 FROM attachments a
                                WHERE a.message_id = attachment_messages.id
                           )
                       )
                ) AS has_attachments,
                matched.id,
                COALESCE(matched.thread_assignment_method, 'standalone')
           FROM matched
          WHERE matched.activity_rank = 1",
        source.from_sql, where_sql
    );
    sql.push_str(" ORDER BY ");
    sql.push_str(conversation_sort_clause(sort));
    sql.push_str(" LIMIT ? OFFSET ?");
    query_params.push(Value::Integer(
        limit.clamp(1, MAX_CONVERSATION_PAGE_SIZE + 1),
    ));
    query_params.push(Value::Integer(offset.max(0)));

    let workspace_id = active.id.clone();
    let workspace_path = active.path.display().to_string();
    let display_name = pst_display_name(active);
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(query_params.iter()), |row| {
        Ok(ConversationSummary {
            conversation_id: row.get(0)?,
            conversation_root_id: row.get(1)?,
            subject: row.get(2)?,
            latest_sender: row.get(3)?,
            participants: row
                .get::<_, String>(4)?
                .split('\u{1f}')
                .filter(|participant| !participant.trim().is_empty())
                .map(str::to_string)
                .collect(),
            latest_date: row.get(5)?,
            snippet: row.get(6)?,
            matching_message_count: row.get(7)?,
            total_message_count: row.get(8)?,
            has_attachments: row.get::<_, i64>(9)? != 0,
            latest_message_id: row.get(10)?,
            assignment_method: row.get(11)?,
            workspace_id: workspace_id.clone(),
            pst_display_name: display_name.clone(),
            workspace_path: workspace_path.clone(),
        })
    })?;
    let items = collect_rows(rows)?;
    check_search_operation(operation)?;
    Ok(items)
}

#[allow(clippy::too_many_arguments)]
fn query_conversation_summaries_page_visible(
    conn: &Connection,
    active: &ActiveWorkspace,
    scope: &local_trash::LocalTrashScope,
    folder_id: Option<i64>,
    include_subfolders: bool,
    criteria: &MessageSearchCriteria,
    sort: &str,
    limit: i64,
    offset: i64,
    operation: Option<&SearchOperationGuard>,
) -> AppResult<BoundedPage<ConversationSummary>> {
    let page_limit = limit.clamp(1, MAX_CONVERSATION_PAGE_SIZE);
    let page_offset = offset.max(0);
    let items = query_conversation_summaries_visible(
        conn,
        active,
        scope,
        folder_id,
        include_subfolders,
        criteria,
        sort,
        page_limit + 1,
        page_offset,
        operation,
    )?;
    Ok(finish_bounded_page(items, page_limit, page_offset))
}

fn compare_conversation_summaries(
    left: &ConversationSummary,
    right: &ConversationSummary,
    sort: &str,
) -> std::cmp::Ordering {
    let ordering = match sort {
        "oldest" => left
            .latest_date
            .cmp(&right.latest_date)
            .then_with(|| left.latest_message_id.cmp(&right.latest_message_id)),
        "subject" => left
            .subject
            .to_ascii_lowercase()
            .cmp(&right.subject.to_ascii_lowercase())
            .then_with(|| right.latest_date.cmp(&left.latest_date)),
        _ => right
            .latest_date
            .cmp(&left.latest_date)
            .then_with(|| right.latest_message_id.cmp(&left.latest_message_id)),
    };
    ordering
        .then_with(|| left.pst_display_name.cmp(&right.pst_display_name))
        .then_with(|| left.workspace_id.cmp(&right.workspace_id))
        .then_with(|| left.conversation_id.cmp(&right.conversation_id))
}

fn conversation_message_select_sql(has_text_match: bool) -> String {
    let mut sql = String::from(
        "SELECT m.id,
            m.folder_id,
            f.path,
            f.name,
            m.subject,
            m.sender,
            m.recipients,
            m.date,
            m.snippet,
            m.has_attachments,
            (SELECT COUNT(*) FROM attachments a WHERE a.message_id = m.id) AS attachment_count",
    );
    sql.push_str(search_match_select_sql(has_text_match));
    sql
}

fn query_matching_conversation_page(
    conn: &Connection,
    source: &MessageQuerySource,
    conversation_id: &str,
    limit: i64,
    offset: i64,
    operation: Option<&SearchOperationGuard>,
) -> AppResult<Vec<ConversationMessageItem>> {
    check_search_operation(operation)?;
    let (where_sql, mut params) = query_source_with_condition(source, "m.conversation_id = ?");
    params.push(Value::Text(conversation_id.to_string()));
    let sql = format!(
        "{}{}{} ORDER BY m.date ASC, m.id ASC LIMIT ? OFFSET ?",
        conversation_message_select_sql(source.has_text_match),
        source.from_sql,
        where_sql
    );
    params.push(Value::Integer(limit));
    params.push(Value::Integer(offset));
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(params.iter()), |row| {
        Ok(ConversationMessageItem {
            message: row_to_message_item(row)?,
            matches_scope: true,
        })
    })?;
    let items = collect_rows(rows)?;
    check_search_operation(operation)?;
    Ok(items)
}

#[cfg(test)]
fn query_entire_conversation_page(
    conn: &Connection,
    source: &MessageQuerySource,
    conversation_id: &str,
    limit: i64,
    offset: i64,
    operation: Option<&SearchOperationGuard>,
) -> AppResult<Vec<ConversationMessageItem>> {
    check_search_operation(operation)?;
    let (matching_where, mut params) = query_source_with_condition(source, "m.conversation_id = ?");
    params.push(Value::Text(conversation_id.to_string()));
    let sql = format!(
        "WITH matching AS (
             SELECT m.id {}{}
         )
         {},
         CASE WHEN matching.id IS NULL THEN 0 ELSE 1 END AS matches_scope
           FROM messages m
           LEFT JOIN folders f ON f.id = m.folder_id
           LEFT JOIN matching ON matching.id = m.id
          WHERE m.conversation_id = ?
          ORDER BY m.date ASC, m.id ASC
          LIMIT ? OFFSET ?",
        source.from_sql,
        matching_where,
        conversation_message_select_sql(false)
    );
    params.push(Value::Text(conversation_id.to_string()));
    params.push(Value::Integer(limit));
    params.push(Value::Integer(offset));
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(params.iter()), |row| {
        Ok(ConversationMessageItem {
            message: row_to_message_item(row)?,
            matches_scope: row.get::<_, i64>(17)? != 0,
        })
    })?;
    let items = collect_rows(rows)?;
    check_search_operation(operation)?;
    Ok(items)
}

#[allow(clippy::too_many_arguments)]
fn query_entire_conversation_page_visible(
    conn: &Connection,
    source: &MessageQuerySource,
    scope: &local_trash::LocalTrashScope,
    conversation_id: &str,
    limit: i64,
    offset: i64,
    operation: Option<&SearchOperationGuard>,
) -> AppResult<Vec<ConversationMessageItem>> {
    check_search_operation(operation)?;
    let (matching_where, mut params) = query_source_with_condition(source, "m.conversation_id = ?");
    params.push(Value::Text(conversation_id.to_string()));
    let active_predicate = local_trash_membership_predicate("m", scope, false)?;
    let sql = format!(
        "WITH matching AS (
             SELECT m.id {}{}
         )
         {},
         CASE WHEN matching.id IS NULL THEN 0 ELSE 1 END AS matches_scope
           FROM messages m
           LEFT JOIN folders f ON f.id = m.folder_id
           LEFT JOIN matching ON matching.id = m.id
          WHERE m.conversation_id = ?
            AND {active_predicate}
          ORDER BY m.date ASC, m.id ASC
          LIMIT ? OFFSET ?",
        source.from_sql,
        matching_where,
        conversation_message_select_sql(false)
    );
    params.push(Value::Text(conversation_id.to_string()));
    params.push(Value::Integer(limit));
    params.push(Value::Integer(offset));
    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(params_from_iter(params.iter()), |row| {
        Ok(ConversationMessageItem {
            message: row_to_message_item(row)?,
            matches_scope: row.get::<_, i64>(17)? != 0,
        })
    })?;
    let items = collect_rows(rows)?;
    check_search_operation(operation)?;
    Ok(items)
}

fn row_to_message_item(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageListItem> {
    let attachment_count = row.get::<_, i64>(10)?;
    Ok(MessageListItem {
        id: row.get(0)?,
        folder_id: row.get(1)?,
        folder_path: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
        folder_name: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
        subject: row.get::<_, Option<String>>(4)?.unwrap_or_default(),
        sender: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
        recipients: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
        date: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
        snippet: row.get::<_, Option<String>>(8)?.unwrap_or_default(),
        has_attachments: row.get::<_, i64>(9)? != 0 || attachment_count > 0,
        attachment_count,
        search_match_context: search_match_context_from_row(row, 11)?,
        workspace_id: None,
        pst_display_name: None,
        workspace_path: None,
    })
}

fn pst_display_name(active: &ActiveWorkspace) -> String {
    active
        .pst_path
        .file_name()
        .and_then(OsStr::to_str)
        .map(str::to_string)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| active.pst_path.display().to_string())
}

fn compare_message_items(
    left: &MessageListItem,
    right: &MessageListItem,
    sort_order: &str,
) -> std::cmp::Ordering {
    let ordering = match sort_order {
        "oldest" => left
            .date
            .cmp(&right.date)
            .then_with(|| left.id.cmp(&right.id)),
        "sender_az" => left
            .sender
            .to_ascii_lowercase()
            .cmp(&right.sender.to_ascii_lowercase())
            .then_with(|| right.date.cmp(&left.date))
            .then_with(|| right.id.cmp(&left.id)),
        "subject_az" => left
            .subject
            .to_ascii_lowercase()
            .cmp(&right.subject.to_ascii_lowercase())
            .then_with(|| right.date.cmp(&left.date))
            .then_with(|| right.id.cmp(&left.id)),
        _ => right
            .date
            .cmp(&left.date)
            .then_with(|| right.id.cmp(&left.id)),
    };

    ordering
        .then_with(|| left.pst_display_name.cmp(&right.pst_display_name))
        .then_with(|| left.workspace_id.cmp(&right.workspace_id))
}

fn list_attachments(conn: &Connection, message_id: i64) -> AppResult<Vec<Attachment>> {
    let mut statement = conn.prepare(
        "SELECT id,
                filename,
                sanitized_filename,
                content_type,
                size_bytes,
                attachment_index,
                content_disposition
           FROM attachments
          WHERE message_id = ?1
          ORDER BY attachment_index, filename",
    )?;
    let rows = statement.query_map(params![message_id], |row| {
        Ok(Attachment {
            id: row.get(0)?,
            filename: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            sanitized_filename: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            content_type: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
            size_bytes: row.get(4)?,
            attachment_index: row.get(5)?,
            content_disposition: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
        })
    })?;

    collect_rows(rows)
}

fn attachment_from_draft(draft: &AttachmentDraft) -> Attachment {
    Attachment {
        id: draft.attachment_index + 1,
        filename: draft.filename.clone(),
        sanitized_filename: draft.sanitized_filename.clone(),
        content_type: draft.content_type.clone(),
        size_bytes: draft.size_bytes,
        attachment_index: draft.attachment_index,
        content_disposition: draft.content_disposition.clone(),
    }
}

fn collect_rows<T>(
    rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>>,
) -> AppResult<Vec<T>> {
    let mut items = Vec::new();
    for item in rows {
        items.push(item?);
    }
    Ok(items)
}

fn relative_path_string(root: &Path, path: &Path) -> AppResult<String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| AppError::new("Extracted EML path was outside the workspace."))?;
    let mut components = Vec::new();
    for component in relative.components() {
        let value = component.as_os_str().to_str().ok_or_else(|| {
            AppError::new(
                "Extracted EML identity is not valid UTF-8; indexing was refused to avoid an identity collision.",
            )
        })?;
        components.push(value);
    }
    Ok(components.join("/"))
}

fn path_to_slash_string(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn path_is_under_root(path: &Path, root: &Path) -> AppResult<bool> {
    if !path.exists() {
        return Ok(false);
    }

    let canonical_path = path.canonicalize()?;
    let canonical_root = root.canonicalize()?;
    Ok(canonical_path.starts_with(canonical_root))
}

fn path_parent_is_root(path: &Path, root: &Path) -> AppResult<bool> {
    let Some(parent) = path.parent() else {
        return Ok(false);
    };
    Ok(paths_equivalent(parent, root))
}

fn create_unique_private_export_file(
    export_dir: &Path,
    sanitized_filename: &str,
) -> AppResult<(PathBuf, File)> {
    let sanitized_filename = sanitize_attachment_filename(sanitized_filename);
    let (stem, extension) = filename_stem_and_extension(&sanitized_filename);
    let mut existing_names = HashSet::new();
    for entry in fs::read_dir(export_dir)? {
        let entry = entry?;
        existing_names.insert(filename_collision_key(&entry.file_name().to_string_lossy()));
    }

    for suffix in 0..10_000 {
        let filename = if suffix == 0 {
            sanitized_filename.clone()
        } else if extension.is_empty() {
            format!("{stem}-{suffix}")
        } else {
            format!("{stem}-{suffix}.{extension}")
        };
        let candidate = export_dir.join(filename);
        if existing_names.contains(&filename_collision_key(
            candidate
                .file_name()
                .and_then(OsStr::to_str)
                .unwrap_or_default(),
        )) {
            continue;
        }
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(PRIVATE_FILE_MODE)
            .open(&candidate)
        {
            Ok(file) => return Ok((candidate, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                continue;
            }
            Err(error) => return Err(AppError::from(error)),
        }
    }

    Err(AppError::new(format!(
        "Could not create a unique export filename for {sanitized_filename}."
    )))
}

fn filename_collision_key(filename: &str) -> String {
    let mut key = String::new();
    for character in filename.chars().flat_map(char::to_lowercase) {
        if matches!(
            character,
            '\u{0300}'..='\u{036f}'
                | '\u{1ab0}'..='\u{1aff}'
                | '\u{1dc0}'..='\u{1dff}'
                | '\u{20d0}'..='\u{20ff}'
                | '\u{fe20}'..='\u{fe2f}'
        ) {
            continue;
        }
        let folded = match character {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => "a",
            'æ' => "ae",
            'ç' => "c",
            'è' | 'é' | 'ê' | 'ë' => "e",
            'ì' | 'í' | 'î' | 'ï' => "i",
            'ñ' => "n",
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' => "o",
            'œ' => "oe",
            'ù' | 'ú' | 'û' | 'ü' => "u",
            'ý' | 'ÿ' => "y",
            _ => {
                key.push(character);
                continue;
            }
        };
        key.push_str(folded);
    }
    key
}

fn export_message_eml_filename(message_id: i64, date: &str, subject: &str) -> String {
    let date_prefix = message_export_date_prefix(date);
    let subject = sanitize_attachment_filename(subject);
    let subject = subject
        .trim()
        .chars()
        .take(90)
        .collect::<String>()
        .trim()
        .to_string();
    let subject = if subject.is_empty() {
        "message".to_string()
    } else {
        subject
    };

    sanitize_attachment_filename(&format!("{date_prefix}-{subject}-{message_id}.eml"))
}

fn export_source_message_filename(
    format: StandaloneSourceFormat,
    date: &str,
    subject: &str,
) -> String {
    let filename = export_message_eml_filename(0, date, subject);
    if format != StandaloneSourceFormat::Eml {
        let stem = filename.strip_suffix(".eml").unwrap_or(&filename);
        return format!("{stem}.{}", format.source_extension());
    }
    filename
}

fn sanitize_printable_html_filename(filename: &str) -> String {
    let trimmed = filename.trim();
    let candidate = if trimmed.is_empty() {
        "message.html"
    } else {
        trimmed
    };
    let sanitized = sanitize_attachment_filename(candidate);
    let lower = sanitized.to_ascii_lowercase();
    if lower.ends_with(".html") || lower.ends_with(".htm") {
        sanitized
    } else {
        format!("{sanitized}.html")
    }
}

fn message_export_date_prefix(date: &str) -> String {
    let trimmed = date.trim();
    let Some(prefix) = trimmed.get(..10) else {
        return "undated".to_string();
    };
    let bytes = prefix.as_bytes();
    if bytes.get(4) == Some(&b'-')
        && bytes.get(7) == Some(&b'-')
        && bytes
            .iter()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
    {
        return prefix.to_string();
    }

    "undated".to_string()
}

fn filename_stem_and_extension(filename: &str) -> (String, String) {
    match filename.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => {
            (stem.to_string(), extension.to_string())
        }
        _ => (filename.to_string(), String::new()),
    }
}

fn reveal_path(path: &Path) -> AppResult<()> {
    if !path.exists() {
        return Err(AppError::new(format!(
            "Path does not exist: {}",
            path.display()
        )));
    }

    let status = Command::new("open")
        .arg("-R")
        .arg(path)
        .status()
        .map_err(|error| AppError::new(format!("Could not open Finder: {error}")))?;

    if status.success() {
        Ok(())
    } else {
        Err(AppError::new(format!(
            "Finder reveal failed with status {status}."
        )))
    }
}

fn open_path_with_default_app(path: &Path) -> AppResult<()> {
    if !path.exists() {
        return Err(AppError::new(format!(
            "Path does not exist: {}",
            path.display()
        )));
    }

    let status = Command::new("open")
        .arg(path)
        .status()
        .map_err(|error| AppError::new(format!("Could not open exported file: {error}")))?;

    if status.success() {
        Ok(())
    } else {
        Err(AppError::new(format!(
            "Opening exported file failed with status {status}."
        )))
    }
}

fn emit_progress(
    app: &tauri::AppHandle,
    stage: &str,
    current: Option<usize>,
    total: Option<usize>,
    _message: &str,
) {
    let requested_stage = progress_stage_from_legacy_label(stage);
    emit_progress_observation(
        app,
        ProgressObservation {
            processed_units: current.and_then(|value| u64::try_from(value).ok()),
            total_units: total.and_then(|value| u64::try_from(value).ok()),
            unit_kind: requested_stage.and_then(progress_unit_for_stage),
            activity_observed: true,
            process_state: requested_stage
                .filter(|value| *value == import_progress::ProgressStage::Extracting)
                .map(|_| import_progress::ProcessState::RunningRecentActivity),
            ..ProgressObservation::default()
        },
    );
}

#[derive(Default)]
struct ProgressObservation {
    processed_units: Option<u64>,
    total_units: Option<u64>,
    unit_kind: Option<import_progress::ProgressUnitKind>,
    activity_observed: bool,
    process_state: Option<import_progress::ProcessState>,
    status_code: Option<import_progress::ProgressStatusCode>,
    counters: Option<import_progress::ProgressCounters>,
    warning_codes: Vec<import_progress::ProgressWarningCode>,
}

fn merge_progress_counters(
    current: &mut import_progress::ProgressCounters,
    update: import_progress::ProgressCounters,
) {
    macro_rules! merge {
        ($field:ident) => {
            if let Some(value) = update.$field {
                current.$field = Some(current.$field.map_or(value, |saved| saved.max(value)));
            }
        };
    }
    merge!(output_files_observed);
    merge!(output_bytes_observed);
    merge!(workspace_bytes_written);
    merge!(candidates_discovered);
    merge!(candidates_processed);
    merge!(canonical_messages);
    merge!(inserted_messages);
    merge!(skipped_existing_messages);
    merge!(parse_failures);
    merge!(attachments_indexed);
    merge!(conversation_records_processed);
    merge!(conversation_records_total);
    merge!(verification_checks_completed);
    merge!(verification_checks_total);
    merge!(missing_canonical_records);
    merge!(unexpected_canonical_records);
    merge!(possible_duplicate_groups);
}

fn canonical_progress_utc_now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn privacy_safe_source_basename(path: &Path) -> String {
    let mut basename = path
        .file_name()
        .map(|value| value.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Source.pst".to_string())
        .chars()
        .filter(|value| !value.is_control() && !matches!(value, '/' | '\\'))
        .take(255)
        .collect::<String>();
    if basename.is_empty() || matches!(basename.as_str(), "." | "..") {
        basename = "Source.pst".to_string();
    }
    while basename.len() > 1_024 {
        basename.pop();
    }
    basename
}

fn progress_storage_mode(mode: WorkspaceLocationMode) -> import_progress::WorkspaceStorageMode {
    match mode {
        WorkspaceLocationMode::AppSupport => import_progress::WorkspaceStorageMode::AppSupport,
        WorkspaceLocationMode::NextToPst => import_progress::WorkspaceStorageMode::NextToPst,
    }
}

fn progress_stage_from_workspace_state(
    state: import_integrity_phase2::WorkspaceState,
) -> Option<import_progress::ProgressStage> {
    use import_integrity_phase2::WorkspaceState;
    use import_progress::ProgressStage;
    match state {
        WorkspaceState::Preflight => Some(ProgressStage::Preflight),
        WorkspaceState::Extracting | WorkspaceState::ExtractionComplete => {
            Some(ProgressStage::Extracting)
        }
        WorkspaceState::Discovering => Some(ProgressStage::Discovering),
        WorkspaceState::Indexing => Some(ProgressStage::Indexing),
        WorkspaceState::BuildingConversations => Some(ProgressStage::BuildingConversations),
        WorkspaceState::Verifying => Some(ProgressStage::Verifying),
        WorkspaceState::Complete => Some(ProgressStage::Complete),
        WorkspaceState::Cancelled | WorkspaceState::Incomplete | WorkspaceState::Failed => None,
    }
}

fn workspace_state_from_progress_stage(
    stage: import_progress::ProgressStage,
) -> Option<import_integrity_phase2::WorkspaceState> {
    use import_integrity_phase2::WorkspaceState;
    use import_progress::ProgressStage;
    match stage {
        ProgressStage::Preflight => Some(WorkspaceState::Preflight),
        ProgressStage::Extracting => Some(WorkspaceState::Extracting),
        ProgressStage::Discovering => Some(WorkspaceState::Discovering),
        ProgressStage::Indexing => Some(WorkspaceState::Indexing),
        ProgressStage::BuildingConversations => Some(WorkspaceState::BuildingConversations),
        ProgressStage::Verifying => Some(WorkspaceState::Verifying),
        ProgressStage::Complete => None,
    }
}

fn progress_stage_from_legacy_label(stage: &str) -> Option<import_progress::ProgressStage> {
    let normalized = stage.to_ascii_lowercase();
    if normalized.contains("complete") && !normalized.contains("incomplete") {
        Some(import_progress::ProgressStage::Complete)
    } else if normalized.contains("verif") || normalized.contains("validation") {
        Some(import_progress::ProgressStage::Verifying)
    } else if normalized.contains("conversation") {
        Some(import_progress::ProgressStage::BuildingConversations)
    } else if normalized.contains("index") || normalized.contains("reindex") {
        Some(import_progress::ProgressStage::Indexing)
    } else if normalized.contains("discover") || normalized.contains("scanning extracted") {
        Some(import_progress::ProgressStage::Discovering)
    } else if normalized.contains("readpst") || normalized.contains("extract") {
        Some(import_progress::ProgressStage::Extracting)
    } else if normalized.contains("check") || normalized.contains("prepar") {
        Some(import_progress::ProgressStage::Preflight)
    } else {
        None
    }
}

fn progress_unit_for_stage(
    stage: import_progress::ProgressStage,
) -> Option<import_progress::ProgressUnitKind> {
    use import_progress::{ProgressStage, ProgressUnitKind};
    match stage {
        ProgressStage::Extracting => Some(ProgressUnitKind::Files),
        ProgressStage::Discovering => Some(ProgressUnitKind::CandidateRecords),
        ProgressStage::Indexing => Some(ProgressUnitKind::CandidateRecords),
        ProgressStage::BuildingConversations => Some(ProgressUnitKind::ConversationRecords),
        ProgressStage::Verifying => Some(ProgressUnitKind::VerificationChecks),
        ProgressStage::Preflight | ProgressStage::Complete => None,
    }
}

fn progress_status_code(
    stage: import_progress::ProgressStage,
    correction: Option<import_progress::StageCorrection>,
) -> import_progress::ProgressStatusCode {
    use import_progress::{ProgressStage, ProgressStatusCode};
    if correction.is_some() {
        return ProgressStatusCode::StageCorrectedByVerification;
    }
    match stage {
        ProgressStage::Preflight => ProgressStatusCode::PreflightRunning,
        ProgressStage::Extracting => ProgressStatusCode::ReadpstRunning,
        ProgressStage::Discovering => ProgressStatusCode::DiscoveringCandidates,
        ProgressStage::Indexing => ProgressStatusCode::IndexingCandidates,
        ProgressStage::BuildingConversations => ProgressStatusCode::BuildingConversations,
        ProgressStage::Verifying => ProgressStatusCode::VerifyingWorkspace,
        ProgressStage::Complete => ProgressStatusCode::ImportComplete,
    }
}

fn completed_progress_rank_from_workspace_state(
    state: import_integrity_phase2::WorkspaceState,
) -> Option<usize> {
    use import_integrity_phase2::WorkspaceState;
    use import_progress::ProgressStage;
    match state {
        WorkspaceState::Preflight => Some(ProgressStage::Preflight.rank()),
        WorkspaceState::Extracting | WorkspaceState::ExtractionComplete => {
            Some(ProgressStage::Extracting.rank())
        }
        WorkspaceState::Discovering => Some(ProgressStage::Discovering.rank()),
        WorkspaceState::Indexing => Some(ProgressStage::Indexing.rank()),
        WorkspaceState::BuildingConversations => Some(ProgressStage::BuildingConversations.rank()),
        WorkspaceState::Verifying => Some(ProgressStage::Verifying.rank()),
        WorkspaceState::Complete => Some(ProgressStage::Complete.rank()),
        WorkspaceState::Cancelled | WorkspaceState::Incomplete | WorkspaceState::Failed => None,
    }
}

fn completed_progress_stages_from_record(
    record: &import_integrity_phase2::WorkspaceImportState,
) -> Vec<import_progress::ProgressStage> {
    record
        .last_completed_stage
        .and_then(completed_progress_rank_from_workspace_state)
        .map(|rank| import_progress::ProgressStage::ALL[..=rank].to_vec())
        .unwrap_or_default()
}

fn initial_completed_progress_stages(
    operation_kind: import_progress::OperationKind,
    initial_stage: import_progress::ProgressStage,
    record: &import_integrity_phase2::WorkspaceImportState,
    discard_persisted_lineage: bool,
) -> Vec<import_progress::ProgressStage> {
    use import_progress::OperationKind;
    match operation_kind {
        OperationKind::ResumedImport if !discard_persisted_lineage => {
            let mut completed = completed_progress_stages_from_record(record);
            completed.retain(|stage| stage.rank() < initial_stage.rank());
            completed
        }
        OperationKind::Repair | OperationKind::Verification => {
            let mut completed = completed_progress_stages_from_record(record);
            completed.retain(|stage| stage.rank() < initial_stage.rank());
            completed
        }
        OperationKind::NewImport | OperationKind::ResumedImport => Vec::new(),
    }
}

fn initial_progress_parse_failures(
    operation_kind: import_progress::OperationKind,
    persisted_parse_failures: Option<u64>,
) -> Option<u64> {
    if matches!(
        operation_kind,
        import_progress::OperationKind::NewImport | import_progress::OperationKind::Repair
    ) {
        None
    } else {
        persisted_parse_failures
    }
}

fn merge_persisted_completed_stages(
    completed_stages: &mut Vec<import_progress::ProgressStage>,
    record: &import_integrity_phase2::WorkspaceImportState,
    operation_kind: import_progress::OperationKind,
    active_stage: import_progress::ProgressStage,
) {
    if !matches!(
        operation_kind,
        import_progress::OperationKind::NewImport | import_progress::OperationKind::ResumedImport
    ) {
        return;
    }
    for stage in completed_progress_stages_from_record(record)
        .into_iter()
        .filter(|stage| stage.rank() < active_stage.rank())
    {
        if !completed_stages.contains(&stage) {
            completed_stages.push(stage);
        }
    }
    completed_stages.sort_by_key(|stage| stage.rank());
}

fn completed_progress_stage_for_transition(
    previous: import_integrity_phase2::WorkspaceState,
    next: import_integrity_phase2::WorkspaceState,
) -> Option<import_progress::ProgressStage> {
    use import_integrity_phase2::WorkspaceState;
    use import_progress::ProgressStage;
    match (previous, next) {
        (WorkspaceState::Preflight, WorkspaceState::Extracting) => Some(ProgressStage::Preflight),
        (WorkspaceState::Extracting, WorkspaceState::ExtractionComplete)
        | (WorkspaceState::ExtractionComplete, WorkspaceState::Discovering) => {
            Some(ProgressStage::Extracting)
        }
        (WorkspaceState::Discovering, WorkspaceState::Indexing) => Some(ProgressStage::Discovering),
        (WorkspaceState::Indexing, WorkspaceState::BuildingConversations) => {
            Some(ProgressStage::Indexing)
        }
        (WorkspaceState::BuildingConversations, WorkspaceState::Verifying) => {
            Some(ProgressStage::BuildingConversations)
        }
        (WorkspaceState::Verifying, WorkspaceState::Complete) => Some(ProgressStage::Verifying),
        _ => None,
    }
}

fn completed_progress_stages_for_operation_transition(
    operation_kind: import_progress::OperationKind,
    current_stage: import_progress::ProgressStage,
    previous: import_integrity_phase2::WorkspaceState,
    next: import_integrity_phase2::WorkspaceState,
) -> Vec<import_progress::ProgressStage> {
    if operation_kind == import_progress::OperationKind::Repair
        && current_stage == import_progress::ProgressStage::Preflight
        && next == import_integrity_phase2::WorkspaceState::Discovering
    {
        return vec![
            import_progress::ProgressStage::Preflight,
            import_progress::ProgressStage::Extracting,
        ];
    }
    completed_progress_stage_for_transition(previous, next)
        .into_iter()
        .collect()
}

fn restored_prior_complete_after_interrupted_verification(
    operation_kind: import_progress::OperationKind,
    previous: import_integrity_phase2::WorkspaceState,
    final_state: import_integrity_phase2::WorkspaceState,
    status: import_progress::ProgressStatus,
) -> bool {
    operation_kind == import_progress::OperationKind::Verification
        && previous == import_integrity_phase2::WorkspaceState::Verifying
        && final_state == import_integrity_phase2::WorkspaceState::Complete
        && matches!(
            status,
            import_progress::ProgressStatus::Cancelled | import_progress::ProgressStatus::Failed
        )
}

fn newly_committed_verification_completion_wins(
    previous: import_integrity_phase2::WorkspaceState,
    final_state: import_integrity_phase2::WorkspaceState,
    reason: Option<&str>,
) -> bool {
    previous == import_integrity_phase2::WorkspaceState::Verifying
        && final_state == import_integrity_phase2::WorkspaceState::Complete
        && reason == Some("verification_complete")
}

fn should_record_import_completion(
    updates_import_timing: bool,
    completion_duration_eligible: bool,
    persisted_state: import_integrity_phase2::WorkspaceState,
) -> bool {
    updates_import_timing
        && completion_duration_eligible
        && persisted_state == import_integrity_phase2::WorkspaceState::Complete
}

fn terminal_process_state(
    stage: import_progress::ProgressStage,
    observed: import_progress::ProcessState,
) -> import_progress::ProcessState {
    if stage == import_progress::ProgressStage::Extracting
        && matches!(
            observed,
            import_progress::ProcessState::ExitedSuccess
                | import_progress::ProcessState::ExitedFailure
        )
    {
        observed
    } else {
        import_progress::ProcessState::NotApplicable
    }
}

fn record_progress_transition(
    scope: &mut IntegrityOperationScope,
    next: import_integrity_phase2::WorkspaceState,
) {
    for completed in completed_progress_stages_for_operation_transition(
        scope.operation_kind,
        scope.stage,
        scope.persisted_state,
        next,
    ) {
        if !scope.completed_stages.contains(&completed) {
            scope.completed_stages.push(completed);
        }
    }
    scope.completed_stages.sort_by_key(|stage| stage.rank());
    scope.persisted_state = next;
}

fn apply_persisted_progress_correction(
    scope: &mut IntegrityOperationScope,
    record: &import_integrity_phase2::WorkspaceImportState,
) {
    if record.reason.as_deref() == Some("verification_incomplete") {
        scope
            .completed_stages
            .retain(|stage| stage.rank() < import_progress::ProgressStage::Verifying.rank());
    }
}

fn start_integrity_operation_scope(
    app: &tauri::AppHandle,
    active: &ActiveWorkspace,
    operation_id: &str,
    operation_kind: import_progress::OperationKind,
    initial_stage: import_progress::ProgressStage,
) -> AppResult<()> {
    import_integrity_phase2::validate_operation_id(operation_id)?;
    let state = app.state::<AppState>();
    let mut scope = state
        .integrity_operation
        .lock()
        .map_err(|_| AppError::new("Workspace operation scope is unavailable."))?;
    if scope.is_some() {
        return Err(AppError::new(
            "Another workspace integrity operation is already scoped.",
        ));
    }
    let previous = import_integrity_phase2::read_state(&active.path, &active.id)?
        .ok_or_else(|| AppError::new("Workspace state is unavailable for import progress."))?;
    let persisted_stage = previous
        .interrupted_stage
        .and_then(progress_stage_from_workspace_state)
        .or_else(|| progress_stage_from_workspace_state(previous.state));
    let prior_active_timing_recorded =
        previous.active_timing_version == import_integrity_phase2::ACTIVE_TIMING_VERSION;
    let completion_duration_eligible = previous.active_timing_history_complete;
    let canonical_import_elapsed_ms = if prior_active_timing_recorded {
        previous
            .accumulated_active_elapsed_ms
            .max(previous.elapsed_duration_ms)
    } else {
        0
    };
    // Any operation which first carries an incomplete import to verified
    // completion contributes to the canonical import duration. Once durable
    // Complete lineage exists, later repair or reverification has its own
    // operation timer and must not rewrite the import history.
    let updates_import_timing = !import_integrity_phase2::state_has_complete_lineage(&previous);
    let non_import_timing_kind = if updates_import_timing {
        None
    } else {
        Some(match operation_kind {
            import_progress::OperationKind::Repair => {
                import_integrity_phase2::OperationTimingKind::Repair
            }
            import_progress::OperationKind::Verification => {
                import_integrity_phase2::OperationTimingKind::Verification
            }
            import_progress::OperationKind::NewImport
            | import_progress::OperationKind::ResumedImport => {
                return Err(AppError::new(
                    "A completed import cannot start another import timing generation.",
                ));
            }
        })
    };
    let non_import_timing_stage =
        non_import_timing_kind.and_then(|_| workspace_state_from_progress_stage(initial_stage));
    let checkpoint = if updates_import_timing {
        import_progress::TimingCheckpoint {
            active_elapsed_ms: canonical_import_elapsed_ms,
            stage_elapsed_ms: previous.stage_elapsed_duration_ms,
            completion_duration_ms: None,
        }
    } else {
        let saved = previous
            .operation_timing
            .filter(|timing| Some(timing.kind) == non_import_timing_kind);
        import_progress::TimingCheckpoint {
            active_elapsed_ms: saved.map_or(0, |timing| timing.active_elapsed_ms),
            stage_elapsed_ms: saved
                .filter(|timing| Some(timing.stage) == non_import_timing_stage)
                .map_or(0, |timing| timing.stage_elapsed_ms),
            completion_duration_ms: None,
        }
    };
    let stage_correction = (previous.reason.as_deref() == Some("verification_incomplete"))
        .then_some(persisted_stage)
        .flatten()
        .filter(|stage| stage.rank() > initial_stage.rank())
        .map(|_| import_progress::StageCorrection::VerificationReclassified);
    let completed_stages = initial_completed_progress_stages(
        operation_kind,
        initial_stage,
        &previous,
        stage_correction.is_some(),
    );
    let manifest = import_integrity_phase2::read_manifest(&active.path, &active.id)?;
    let counters = import_progress::ProgressCounters {
        candidates_discovered: Some(previous.discovered_candidates),
        canonical_messages: Some(previous.canonical_indexed),
        parse_failures: initial_progress_parse_failures(
            operation_kind,
            manifest.as_ref().map(|value| value.parse_failure_count),
        ),
        ..import_progress::ProgressCounters::default()
    };
    let reset_stage_elapsed = updates_import_timing && persisted_stage != Some(initial_stage);
    let mut timing = import_progress::ActiveTiming::resume(checkpoint, 0)
        .map_err(|_| AppError::new("Saved operation timing is contradictory."))?;
    if reset_stage_elapsed {
        timing.transition_stage(0);
    }
    let now_utc = canonical_progress_utc_now();
    // Beginning the durable session is deliberately the last fallible setup
    // step. A malformed manifest or contradictory saved timer therefore cannot
    // strand an active-session marker before a finish guard exists.
    let started = import_integrity_phase2::begin_active_session(
        &active.path,
        &active.id,
        updates_import_timing,
        reset_stage_elapsed,
        non_import_timing_kind,
        non_import_timing_stage,
    )?;
    *scope = Some(IntegrityOperationScope {
        workspace_id: active.id.clone(),
        workspace_path: active.path.clone(),
        operation_id: operation_id.to_string(),
        operation_kind,
        workspace_storage_mode: progress_storage_mode(active.location_mode),
        source_basename: privacy_safe_source_basename(&active.pst_path),
        source_size_bytes: fs::metadata(&active.pst_path)
            .map(|metadata| metadata.len())
            .ok()
            .or_else(|| {
                import_integrity_phase2::identity_from_active_workspace(active)
                    .ok()
                    .map(|identity| identity.size)
            })
            .unwrap_or(0)
            .min(import_progress::MAX_SAFE_JSON_INTEGER),
        generation: started.operation_generation,
        state_revision: started.revision,
        event_sequence: 0,
        persisted_state: started.state,
        completed_stages,
        stage: initial_stage,
        status: import_progress::ProgressStatus::Active,
        timing,
        updates_import_timing,
        completion_duration_eligible,
        canonical_import_elapsed_ms,
        monotonic_origin: Instant::now(),
        last_activity_tick_ms: Some(0),
        last_activity_utc: Some(now_utc),
        last_checkpoint_tick_ms: 0,
        process_state: import_progress::ProcessState::NotApplicable,
        stage_correction,
        counters,
        warning_codes: if completion_duration_eligible {
            Vec::new()
        } else {
            vec![import_progress::ProgressWarningCode::PriorTimingUnavailable]
        },
        eta_samples: Vec::new(),
        last_processed_units: None,
        persistence_error: None,
        terminal_status_override: None,
    });
    Ok(())
}

struct IntegrityOperationFinishGuard {
    app: tauri::AppHandle,
    operation_id: String,
    finished: bool,
}

fn combine_integrity_operation_result<T>(
    operation_result: AppResult<T>,
    finish_result: AppResult<()>,
) -> AppResult<T> {
    match (operation_result, finish_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(finish_error)) => Err(finish_error),
        (Err(operation_error), Ok(())) => Err(operation_error),
        (Err(operation_error), Err(finish_error)) => {
            let code = if operation_error.code == Some(IMPORT_CANCELLED_CODE)
                || finish_error.code == Some(IMPORT_CANCELLED_CODE)
            {
                Some(IMPORT_CANCELLED_CODE)
            } else {
                operation_error.code.or(finish_error.code)
            };
            Err(AppError {
                message: format!(
                    "{} Progress finalization also failed: {}",
                    operation_error.message, finish_error.message
                ),
                setup_command: operation_error.setup_command.or(finish_error.setup_command),
                code,
            })
        }
    }
}

impl IntegrityOperationFinishGuard {
    fn begin(
        app: &tauri::AppHandle,
        active: &ActiveWorkspace,
        operation_id: &str,
        operation_kind: import_progress::OperationKind,
        initial_stage: import_progress::ProgressStage,
    ) -> AppResult<Self> {
        start_integrity_operation_scope(app, active, operation_id, operation_kind, initial_stage)?;
        Ok(Self {
            app: app.clone(),
            operation_id: operation_id.to_string(),
            finished: false,
        })
    }

    fn finish_now(&mut self) -> AppResult<()> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;
        finish_integrity_operation_scope(&self.app, &self.operation_id)
    }

    fn finish_with_result<T>(&mut self, operation_result: AppResult<T>) -> AppResult<T> {
        let override_result = match &operation_result {
            Ok(_) => Ok(()),
            Err(error) => set_integrity_operation_terminal_status_override(
                &self.app,
                &self.operation_id,
                if error.code == Some(IMPORT_CANCELLED_CODE) {
                    import_progress::ProgressStatus::Cancelled
                } else {
                    import_progress::ProgressStatus::Failed
                },
            ),
        };
        let finish_result = self.finish_now();
        let finalization_result =
            combine_integrity_operation_result(override_result, finish_result);
        combine_integrity_operation_result(operation_result, finalization_result)
    }
}

impl Drop for IntegrityOperationFinishGuard {
    fn drop(&mut self) {
        // Normal success and error paths call finish_with_result explicitly.
        // Drop is only the panic/unwind fallback where no result can be returned.
        if !self.finished {
            self.finished = true;
            let _ = finish_integrity_operation_scope(&self.app, &self.operation_id);
        }
    }
}

fn set_integrity_operation_terminal_status_override(
    app: &tauri::AppHandle,
    operation_id: &str,
    status: import_progress::ProgressStatus,
) -> AppResult<()> {
    if !matches!(
        status,
        import_progress::ProgressStatus::Cancelled
            | import_progress::ProgressStatus::Incomplete
            | import_progress::ProgressStatus::Failed
    ) {
        return Err(AppError::new("Invalid operation terminal status override."));
    }
    let state = app.state::<AppState>();
    let mut guard = state
        .integrity_operation
        .lock()
        .map_err(|_| AppError::new("Workspace operation scope is unavailable."))?;
    let scope = guard
        .as_mut()
        .ok_or_else(|| AppError::new("Workspace operation scope disappeared."))?;
    if scope.operation_id != operation_id {
        return Err(AppError::new(
            "Stale workspace operation terminal status was rejected.",
        ));
    }
    scope.terminal_status_override = Some(status);
    Ok(())
}

fn mark_reused_repair_prerequisites(
    app: &tauri::AppHandle,
    operation_id: &str,
    through: import_progress::ProgressStage,
) -> AppResult<()> {
    let state = app.state::<AppState>();
    let mut guard = state
        .integrity_operation
        .lock()
        .map_err(|_| AppError::new("Workspace operation scope is unavailable."))?;
    let scope = guard
        .as_mut()
        .ok_or_else(|| AppError::new("Workspace operation scope disappeared."))?;
    if scope.operation_id != operation_id
        || scope.operation_kind != import_progress::OperationKind::Repair
        || through.rank() >= scope.stage.rank()
    {
        return Err(AppError::new(
            "Repair prerequisite progress evidence was rejected.",
        ));
    }
    for stage in import_progress::ProgressStage::ALL
        .iter()
        .copied()
        .filter(|stage| stage.rank() <= through.rank())
    {
        if !scope.completed_stages.contains(&stage) {
            scope.completed_stages.push(stage);
        }
    }
    scope.completed_stages.sort_by_key(|stage| stage.rank());
    Ok(())
}

fn prepare_recovery_verification_progress(
    app: &tauri::AppHandle,
    operation_id: &str,
) -> AppResult<()> {
    let state = app.state::<AppState>();
    let mut guard = state
        .integrity_operation
        .lock()
        .map_err(|_| AppError::new("Workspace operation scope is unavailable."))?;
    let scope = guard
        .as_mut()
        .ok_or_else(|| AppError::new("Workspace operation scope disappeared."))?;
    if scope.operation_id != operation_id
        || !matches!(
            scope.operation_kind,
            import_progress::OperationKind::Repair | import_progress::OperationKind::ResumedImport
        )
    {
        return Err(AppError::new(
            "Recovery verification progress evidence was rejected.",
        ));
    }
    let current = import_integrity_phase2::read_state(&scope.workspace_path, &scope.workspace_id)?
        .ok_or_else(|| AppError::new("Workspace progress state disappeared."))?;
    if current.operation_generation != scope.generation || current.revision < scope.state_revision {
        return Err(AppError::new(
            "Stale recovery verification progress was rejected.",
        ));
    }
    let verifying = if current.state == import_integrity_phase2::WorkspaceState::Verifying {
        current
    } else {
        import_integrity_phase2::transition_state(
            &scope.workspace_path,
            &scope.workspace_id,
            import_integrity_phase2::WorkspaceState::Verifying,
            import_integrity_phase2::StateCounts {
                discovered_candidates: usize::try_from(current.discovered_candidates).ok(),
                canonical_indexed: usize::try_from(current.canonical_indexed).ok(),
            },
            Some("recovery_verification_running"),
        )?
    };
    if verifying.operation_generation != scope.generation {
        return Err(AppError::new(
            "Recovery verification operation generation changed unexpectedly.",
        ));
    }
    record_progress_transition(scope, verifying.state);
    scope.state_revision = verifying.revision;
    for stage in import_progress::ProgressStage::ALL
        .iter()
        .copied()
        .filter(|stage| {
            stage.rank() <= import_progress::ProgressStage::BuildingConversations.rank()
        })
    {
        if !scope.completed_stages.contains(&stage) {
            scope.completed_stages.push(stage);
        }
    }
    scope.completed_stages.sort_by_key(|stage| stage.rank());
    if scope.stage != import_progress::ProgressStage::Verifying {
        let now_tick_ms = scope
            .monotonic_origin
            .elapsed()
            .as_millis()
            .try_into()
            .unwrap_or(import_progress::MAX_SAFE_JSON_INTEGER);
        scope.stage = import_progress::ProgressStage::Verifying;
        scope.timing.transition_stage(now_tick_ms);
        scope.eta_samples.clear();
        scope.last_processed_units = None;
    }
    Ok(())
}

fn emit_progress_observation(app: &tauri::AppHandle, observation: ProgressObservation) {
    let state = app.state::<AppState>();
    let mut guard = match state.integrity_operation.lock() {
        Ok(guard) => guard,
        Err(_) => return,
    };
    let Some(scope) = guard.as_mut() else {
        return;
    };
    if scope.persistence_error.is_some() {
        return;
    }
    let now_tick_ms = scope
        .monotonic_origin
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(import_progress::MAX_SAFE_JSON_INTEGER);
    let previous_stage = scope.stage;
    let state_record =
        match import_integrity_phase2::read_state(&scope.workspace_path, &scope.workspace_id) {
            Ok(Some(record)) => record,
            Ok(None) => {
                scope.persistence_error = Some("Workspace progress state disappeared.".to_string());
                return;
            }
            Err(error) => {
                scope.persistence_error = Some(error.message);
                return;
            }
        };
    if state_record.operation_generation != scope.generation {
        scope.persistence_error =
            Some("Workspace progress operation generation changed unexpectedly.".to_string());
        return;
    }
    if state_record.revision < scope.state_revision {
        scope.persistence_error = Some("Workspace progress state regressed.".to_string());
        return;
    }
    record_progress_transition(scope, state_record.state);
    merge_persisted_completed_stages(
        &mut scope.completed_stages,
        &state_record,
        scope.operation_kind,
        progress_stage_from_workspace_state(state_record.state).unwrap_or(scope.stage),
    );
    apply_persisted_progress_correction(scope, &state_record);
    scope.state_revision = state_record.revision;
    if let Some(persisted_stage) = progress_stage_from_workspace_state(state_record.state)
        .filter(|stage| *stage != import_progress::ProgressStage::Complete)
    {
        scope.stage = persisted_stage;
    }
    if scope.stage != previous_stage {
        scope.timing.transition_stage(now_tick_ms);
        scope.eta_samples.clear();
        scope.last_processed_units = None;
    }
    if observation.activity_observed {
        scope.last_activity_tick_ms = Some(now_tick_ms);
        scope.last_activity_utc = Some(canonical_progress_utc_now());
    }
    if let Some(process_state) = observation.process_state {
        scope.process_state = process_state;
    } else if scope.stage != import_progress::ProgressStage::Extracting {
        scope.process_state = import_progress::ProcessState::NotApplicable;
    }
    if let Some(counters) = observation.counters {
        merge_progress_counters(&mut scope.counters, counters);
    } else if scope.stage == import_progress::ProgressStage::Indexing {
        scope.counters.candidates_processed = observation.processed_units;
        scope.counters.candidates_discovered = observation.total_units;
    } else if scope.stage == import_progress::ProgressStage::Discovering {
        scope.counters.candidates_discovered = observation.processed_units;
    } else if scope.stage == import_progress::ProgressStage::BuildingConversations {
        scope.counters.conversation_records_processed = observation.processed_units;
        scope.counters.conversation_records_total = observation.total_units;
    } else if scope.stage == import_progress::ProgressStage::Verifying {
        scope.counters.verification_checks_completed = observation.processed_units;
        scope.counters.verification_checks_total = observation.total_units;
    }

    let timing = scope.timing.snapshot(now_tick_ms);
    let stage_changed = scope.stage != previous_stage;
    let checkpoint_due =
        stage_changed || now_tick_ms.saturating_sub(scope.last_checkpoint_tick_ms) >= 15_000;
    if checkpoint_due {
        match import_integrity_phase2::checkpoint_active_session(
            &scope.workspace_path,
            &scope.workspace_id,
            scope.generation,
            state_record.state,
            timing.active_elapsed_ms,
            timing.stage_elapsed_ms,
            observation.activity_observed,
            false,
            false,
        ) {
            Ok(saved) => {
                scope.state_revision = saved.revision;
                scope.last_checkpoint_tick_ms = now_tick_ms;
            }
            Err(error) => {
                scope.persistence_error = Some(error.message);
                return;
            }
        }
    }

    let determinate = observation.total_units.is_some_and(|total| total > 0)
        && observation.processed_units.is_some()
        && observation.unit_kind.is_some();
    let (processed_units, total_units, unit_kind) = if determinate {
        (
            observation.processed_units,
            observation.total_units,
            observation.unit_kind,
        )
    } else {
        (
            observation.processed_units,
            None,
            observation.processed_units.and(observation.unit_kind),
        )
    };
    if determinate && scope.last_processed_units != processed_units {
        if let Some(processed) = processed_units {
            scope.eta_samples.push(import_progress::EtaSample {
                active_elapsed_ms: timing.active_elapsed_ms,
                completed_units: processed,
            });
            if scope.eta_samples.len() > 32 {
                scope.eta_samples.remove(0);
            }
            scope.last_processed_units = Some(processed);
        }
    }
    let last_activity_age_ms = scope
        .last_activity_tick_ms
        .map(|tick| import_progress::monotonic_age_ms(now_tick_ms, tick));
    if scope.stage == import_progress::ProgressStage::Extracting
        && scope.process_state == import_progress::ProcessState::RunningRecentActivity
        && last_activity_age_ms.is_some_and(|age| age > 30_000)
    {
        scope.process_state = import_progress::ProcessState::RunningNoRecentSignal;
    }
    let eta = match (determinate, processed_units, total_units) {
        (true, Some(processed), Some(total)) => {
            match import_progress::estimate_eta(import_progress::EtaInput {
                determinate: true,
                processed_units: processed,
                total_units: total,
                active_elapsed_ms: timing.active_elapsed_ms,
                last_progress_age_ms: last_activity_age_ms,
                samples: &scope.eta_samples,
            }) {
                import_progress::EtaStatus::Estimate(range) => Some(range),
                import_progress::EtaStatus::Hidden | import_progress::EtaStatus::Estimating => None,
            }
        }
        _ => None,
    };
    for warning in observation.warning_codes {
        if !scope.warning_codes.contains(&warning) {
            scope.warning_codes.push(warning);
        }
    }
    let mut warning_codes = scope.warning_codes.clone();
    if scope.process_state == import_progress::ProcessState::RunningNoRecentSignal {
        let warning = import_progress::ProgressWarningCode::NoRecentActivitySignal;
        if !warning_codes.contains(&warning) {
            warning_codes.push(warning);
        }
    }
    scope.event_sequence = match scope.event_sequence.checked_add(1) {
        Some(sequence) if sequence <= import_progress::MAX_SAFE_JSON_INTEGER => sequence,
        _ => {
            scope.persistence_error =
                Some("Workspace progress event sequence overflowed.".to_string());
            return;
        }
    };
    let correction = scope.stage_correction;
    let snapshot = import_progress::ImportProgressSnapshot {
        format_version: import_progress::IMPORT_PROGRESS_FORMAT_VERSION,
        operation_id: scope.operation_id.clone(),
        generation: scope.generation,
        state_revision: scope.state_revision,
        event_sequence: scope.event_sequence,
        workspace_id: scope.workspace_id.clone(),
        source_basename: scope.source_basename.clone(),
        source_size_bytes: scope.source_size_bytes,
        workspace_storage_mode: scope.workspace_storage_mode,
        operation_kind: scope.operation_kind,
        stage: scope.stage,
        completed_stages: scope.completed_stages.clone(),
        status: scope.status,
        determinate,
        processed_units,
        total_units,
        unit_kind,
        counters: scope.counters.clone(),
        active_elapsed_ms: if scope.updates_import_timing {
            timing.active_elapsed_ms
        } else {
            scope.canonical_import_elapsed_ms
        },
        operation_elapsed_ms: if scope.updates_import_timing {
            now_tick_ms
        } else {
            timing.active_elapsed_ms
        },
        stage_elapsed_ms: timing.stage_elapsed_ms,
        last_activity_utc: scope.last_activity_utc.clone(),
        last_activity_age_ms,
        process_state: scope.process_state,
        eta,
        status_code: observation
            .status_code
            .unwrap_or_else(|| progress_status_code(scope.stage, correction)),
        warning_codes,
        stage_correction: correction,
    };
    match import_progress::ImportProgressEvent::new(snapshot) {
        Ok(event) => {
            if app.emit("import-progress", event).is_ok() {
                scope.stage_correction = None;
            } else {
                scope.persistence_error =
                    Some("Workspace progress event could not be emitted.".to_string());
            }
        }
        Err(_) => {
            scope.persistence_error = Some("Workspace progress contract was rejected.".to_string());
        }
    }
}

fn finish_integrity_operation_scope(app: &tauri::AppHandle, operation_id: &str) -> AppResult<()> {
    let state = app.state::<AppState>();
    let mut guard = state
        .integrity_operation
        .lock()
        .map_err(|_| AppError::new("Workspace operation scope is unavailable."))?;
    let Some(mut scope) = guard.take() else {
        return Ok(());
    };
    if scope.operation_id != operation_id {
        *guard = Some(scope);
        return Err(AppError::new(
            "Stale workspace operation finish was rejected.",
        ));
    }
    drop(guard);
    if scope.persistence_error.is_some() {
        return Err(AppError::new(
            "Import progress timing could not be persisted safely.",
        ));
    }
    let now_tick_ms = scope
        .monotonic_origin
        .elapsed()
        .as_millis()
        .try_into()
        .unwrap_or(import_progress::MAX_SAFE_JSON_INTEGER);
    let state_record =
        import_integrity_phase2::read_state(&scope.workspace_path, &scope.workspace_id)?
            .ok_or_else(|| AppError::new("Workspace progress state disappeared."))?;
    if state_record.operation_generation != scope.generation {
        return Err(AppError::new(
            "Workspace progress operation generation changed unexpectedly.",
        ));
    }
    if state_record.revision < scope.state_revision {
        return Err(AppError::new("Workspace progress state regressed."));
    }
    let persisted_status = match state_record.state {
        import_integrity_phase2::WorkspaceState::Complete => {
            import_progress::ProgressStatus::Complete
        }
        import_integrity_phase2::WorkspaceState::Cancelled => {
            import_progress::ProgressStatus::Cancelled
        }
        import_integrity_phase2::WorkspaceState::Failed => import_progress::ProgressStatus::Failed,
        _ => import_progress::ProgressStatus::Incomplete,
    };
    let status = if newly_committed_verification_completion_wins(
        scope.persisted_state,
        state_record.state,
        state_record.reason.as_deref(),
    ) {
        persisted_status
    } else {
        scope.terminal_status_override.unwrap_or(persisted_status)
    };
    if restored_prior_complete_after_interrupted_verification(
        scope.operation_kind,
        scope.persisted_state,
        state_record.state,
        status,
    ) {
        scope
            .completed_stages
            .retain(|stage| stage.rank() < import_progress::ProgressStage::Verifying.rank());
        scope.persisted_state = state_record.state;
    } else {
        record_progress_transition(&mut scope, state_record.state);
    }
    merge_persisted_completed_stages(
        &mut scope.completed_stages,
        &state_record,
        scope.operation_kind,
        progress_stage_from_workspace_state(state_record.state).unwrap_or(scope.stage),
    );
    apply_persisted_progress_correction(&mut scope, &state_record);
    scope.state_revision = state_record.revision;
    let previous_stage = scope.stage;
    if status != import_progress::ProgressStatus::Complete {
        if let Some(interrupted_stage) = state_record
            .interrupted_stage
            .and_then(progress_stage_from_workspace_state)
        {
            scope.stage = interrupted_stage;
        }
    }
    if status == import_progress::ProgressStatus::Complete {
        scope.stage = import_progress::ProgressStage::Complete;
    }
    if scope.stage != previous_stage {
        scope.timing.transition_stage(now_tick_ms);
    }
    let should_record_completion = should_record_import_completion(
        scope.updates_import_timing,
        scope.completion_duration_eligible,
        state_record.state,
    );
    let timing = if status == import_progress::ProgressStatus::Complete || should_record_completion
    {
        scope.timing.complete(now_tick_ms)
    } else {
        scope.timing.pause(now_tick_ms)
    };
    let saved = import_integrity_phase2::checkpoint_active_session(
        &scope.workspace_path,
        &scope.workspace_id,
        scope.generation,
        state_record.state,
        timing.active_elapsed_ms,
        timing.stage_elapsed_ms,
        true,
        true,
        should_record_completion,
    )?;
    scope.state_revision = saved.revision;
    if status == import_progress::ProgressStatus::Complete || should_record_completion {
        import_integrity_phase2::sync_completed_duration_to_last_verification(
            &scope.workspace_path,
            &scope.workspace_id,
        )?;
    }
    scope.status = status;
    scope.process_state = terminal_process_state(scope.stage, scope.process_state);
    scope.last_activity_tick_ms = Some(now_tick_ms);
    scope.last_activity_utc = Some(canonical_progress_utc_now());
    scope.event_sequence = scope
        .event_sequence
        .checked_add(1)
        .filter(|sequence| *sequence <= import_progress::MAX_SAFE_JSON_INTEGER)
        .ok_or_else(|| AppError::new("Workspace progress event sequence overflowed."))?;
    let stage = scope.stage;
    let completed_stages = if status == import_progress::ProgressStatus::Complete {
        import_progress::ProgressStage::ALL.to_vec()
    } else {
        scope.completed_stages.clone()
    };
    let status_code = match status {
        import_progress::ProgressStatus::Complete => {
            import_progress::ProgressStatusCode::ImportComplete
        }
        import_progress::ProgressStatus::Cancelled => {
            import_progress::ProgressStatusCode::ImportCancelled
        }
        import_progress::ProgressStatus::Incomplete => {
            import_progress::ProgressStatusCode::ImportIncomplete
        }
        import_progress::ProgressStatus::Failed => {
            import_progress::ProgressStatusCode::ImportFailed
        }
        import_progress::ProgressStatus::Active => {
            progress_status_code(stage, scope.stage_correction)
        }
    };
    let snapshot = import_progress::ImportProgressSnapshot {
        format_version: import_progress::IMPORT_PROGRESS_FORMAT_VERSION,
        operation_id: scope.operation_id.clone(),
        generation: scope.generation,
        state_revision: scope.state_revision,
        event_sequence: scope.event_sequence,
        workspace_id: scope.workspace_id.clone(),
        source_basename: scope.source_basename.clone(),
        source_size_bytes: scope.source_size_bytes,
        workspace_storage_mode: scope.workspace_storage_mode,
        operation_kind: scope.operation_kind,
        stage,
        completed_stages,
        status,
        determinate: false,
        processed_units: None,
        total_units: None,
        unit_kind: None,
        counters: scope.counters.clone(),
        active_elapsed_ms: if scope.updates_import_timing {
            saved
                .accumulated_active_elapsed_ms
                .max(saved.elapsed_duration_ms)
        } else {
            scope.canonical_import_elapsed_ms
        },
        operation_elapsed_ms: if scope.updates_import_timing {
            now_tick_ms
        } else {
            timing.active_elapsed_ms
        },
        stage_elapsed_ms: timing.stage_elapsed_ms,
        last_activity_utc: scope.last_activity_utc.clone(),
        last_activity_age_ms: Some(0),
        process_state: scope.process_state,
        eta: None,
        status_code,
        warning_codes: {
            let mut warnings = scope.warning_codes.clone();
            if status == import_progress::ProgressStatus::Incomplete
                && !warnings.contains(&import_progress::ProgressWarningCode::VerificationIncomplete)
            {
                warnings.push(import_progress::ProgressWarningCode::VerificationIncomplete);
            }
            warnings
        },
        stage_correction: None,
    };
    let event = import_progress::ImportProgressEvent::new(snapshot)
        .map_err(|_| AppError::new("Workspace progress contract was rejected."))?;
    app.emit("import-progress", event)
        .map_err(|_| AppError::new("Workspace progress event could not be emitted."))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RICH_MSG_FIXTURE_ENV: &str = "PST_QUICKVIEW_RICH_MSG_FIXTURE";
    const RICH_MSG_EXPECTED_SHA256_ENV: &str = "PST_QUICKVIEW_RICH_MSG_EXPECTED_SHA256";
    const RICH_MSG_FIXTURE_LEGACY_ENV: &str = "PST_QUICKVIEW_TREVOR_MSG_FIXTURE";
    const LEGACY_MSG_FIXTURE_ENV: &str = "PST_QUICKVIEW_LEGACY_MSG_FIXTURE";
    const LEGACY_MSG_EXPECTED_SHA256_ENV: &str = "PST_QUICKVIEW_LEGACY_MSG_EXPECTED_SHA256";
    const LEGACY_MSG_FIXTURE_LEGACY_ENV: &str = "PST_QUICKVIEW_FURMAN_MSG_FIXTURE";

    const LEGACY_THREADING_COLUMNS: &[&str] = &[
        "message_id_header_raw",
        "message_id_header",
        "in_reply_to_raw",
        "in_reply_to",
        "references_header_raw",
        "references_header",
        "normalized_subject",
        "conversation_id",
        "conversation_parent_id",
        "conversation_root_id",
        "thread_assignment_method",
        "thread_warning",
    ];

    #[test]
    fn import_action_accepts_explicit_non_destructive_repair() {
        assert_eq!(
            ImportAction::from_arg(Some("repair_index".to_string())).unwrap(),
            ImportAction::RepairIndex
        );
        assert!(ImportAction::from_arg(Some("repair".to_string())).is_err());
    }

    fn temporary_test_directory(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory = env::temp_dir().join(format!(
            "pst-quickview-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).expect("temporary test directory should be created");
        directory
    }

    fn synthetic_runtime_profile(root: &Path) -> RuntimeProfile {
        const IDENTIFIER: &str = "com.local.pstquickview.synthetic-tests";
        RuntimeProfile::from_resolved(
            RuntimeProfileIdentity::from_values("PST QuickView Synthetic Tests", IDENTIFIER)
                .unwrap(),
            runtime_profile::ResolvedTauriRoots {
                app_data: root.join("data").join(IDENTIFIER),
                app_log: root.join("logs").join(IDENTIFIER),
                app_cache: root.join("cache").join(IDENTIFIER),
                app_config: root.join("config").join(IDENTIFIER),
            },
        )
        .unwrap()
    }

    fn synthetic_production_runtime_profile(root: &Path) -> RuntimeProfile {
        let identifier = runtime_profile::PRODUCTION_BUNDLE_IDENTIFIER;
        let library = root.join("Library");
        RuntimeProfile::from_resolved(
            RuntimeProfileIdentity::from_values(PRODUCT_DISPLAY_NAME, identifier).unwrap(),
            runtime_profile::ResolvedTauriRoots {
                app_data: library.join("Application Support").join(identifier),
                app_log: library.join("Logs").join(identifier),
                app_cache: library.join("Caches").join(identifier),
                app_config: library.join("Application Support").join(identifier),
            },
        )
        .unwrap()
    }

    fn synthetic_workspace_delete_fixture(name: &str) -> (PathBuf, ActiveWorkspace) {
        let root = temporary_test_directory(name);
        let source = root.join("source.pst");
        fs::write(&source, b"!BDNsynthetic read-only PST source")
            .expect("synthetic PST should be written");
        let source = source.canonicalize().unwrap();
        let workspace_id = "a".repeat(64);
        let workspace = root.join(NEXT_TO_PST_WORKSPACE_DIR).join(&workspace_id);
        create_private_directory_all(&workspace).unwrap();
        let marker = WorkspaceMarker {
            app: WORKSPACE_MARKER_APP_NAME.to_string(),
            workspace_id: workspace_id.clone(),
            pst_content_fingerprint: "synthetic-fingerprint".to_string(),
            workspace_location_mode: WorkspaceLocationMode::NextToPst.as_str().to_string(),
            original_pst_path: source.display().to_string(),
        };
        let marker_path = workspace.join(WORKSPACE_MARKER_FILE);
        let mut output = create_private_file_new(&marker_path).unwrap();
        serde_json::to_writer_pretty(&mut output, &marker).unwrap();
        output.write_all(b"\n").unwrap();
        output.flush().unwrap();
        drop(output);

        (
            root,
            ActiveWorkspace {
                id: workspace_id,
                path: workspace,
                pst_path: source,
                fingerprint: "synthetic-fingerprint".to_string(),
                location_mode: WorkspaceLocationMode::NextToPst,
            },
        )
    }

    fn external_test_file(id: usize) -> ExternalFileOpen {
        ExternalFileOpen {
            path: format!("/tmp/message-{id}.eml"),
            file_kind: "eml".to_string(),
            stable_id: format!("stable-{id}"),
        }
    }

    fn optional_external_fixture(env_name: &str, legacy_env_name: &str) -> Option<PathBuf> {
        let path = env::var_os(env_name).or_else(|| {
            env::var_os(legacy_env_name).inspect(|_| {
                eprintln!("WARN: {legacy_env_name} is deprecated; use {env_name} instead.");
            })
        });
        let Some(path) = path.map(PathBuf::from) else {
            eprintln!("SKIPPED optional external fixture: set {env_name} to a read-only MSG path.");
            return None;
        };
        if !path.is_file() {
            eprintln!("SKIPPED optional external fixture: configured path is unavailable.");
            return None;
        }
        Some(path)
    }

    fn optional_expected_fixture_hash(env_name: &str) -> Option<String> {
        let expected = env::var(env_name).ok()?.trim().to_ascii_lowercase();
        if expected.is_empty() {
            return None;
        }
        assert!(
            expected.len() == 64 && expected.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "{env_name} must contain exactly 64 hexadecimal SHA-256 characters"
        );
        Some(expected)
    }

    fn assert_read_only_msg_fixture(
        path: &Path,
        expected_sha256: Option<&str>,
    ) -> StandaloneMessage {
        let before_hash =
            sha256_file(path).expect("fixture SHA-256 should be readable before parsing");
        if let Some(expected) = expected_sha256 {
            assert_eq!(
                before_hash, expected,
                "configured fixture did not match its expected SHA-256 before parsing"
            );
        } else {
            eprintln!(
                "WARN: fixture identity was not pinned; only byte-for-byte source integrity will be verified"
            );
        }
        cfb::open(path).unwrap_or_else(|_| panic!("fixture must be a readable compound MSG file"));
        let message = standalone_msg_message(path).expect("external MSG fixture should parse");
        let after_hash =
            sha256_file(path).expect("fixture SHA-256 should be readable after parsing");
        assert_eq!(
            before_hash, after_hash,
            "source MSG must remain byte-for-byte unchanged"
        );
        if let Some(expected) = expected_sha256 {
            assert_eq!(
                after_hash, expected,
                "configured fixture did not match its expected SHA-256 after parsing"
            );
        }
        message
    }

    fn create_legacy_workspace_database(path: &Path) -> Connection {
        let conn = Connection::open(path).expect("legacy database should open");
        conn.execute_batch(
            "PRAGMA user_version = 0;
             CREATE TABLE import_metadata (
                 key TEXT PRIMARY KEY,
                 value TEXT NOT NULL
             );
             CREATE TABLE folders (
                 id INTEGER PRIMARY KEY,
                 parent_id INTEGER,
                 path TEXT NOT NULL UNIQUE,
                 name TEXT NOT NULL
             );
             CREATE TABLE messages (
                 id INTEGER PRIMARY KEY,
                 folder_id INTEGER NOT NULL,
                 eml_path TEXT NOT NULL UNIQUE,
                 subject TEXT,
                 sender TEXT,
                 recipients TEXT,
                 date TEXT,
                 body TEXT,
                 snippet TEXT,
                 has_attachments INTEGER NOT NULL DEFAULT 0,
                 imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
             );
             CREATE TABLE attachments (
                 id INTEGER PRIMARY KEY,
                 message_id INTEGER NOT NULL,
                 file_name TEXT,
                 content_type TEXT,
                 size INTEGER
             );
             CREATE VIRTUAL TABLE messages_fts
             USING fts5(subject, sender, recipients, body, content='messages', content_rowid='id');
             INSERT INTO folders (id, parent_id, path, name)
             VALUES (1, NULL, 'Inbox', 'Inbox');
             INSERT INTO messages (
                 id, folder_id, eml_path, subject, sender, recipients, date, body,
                 snippet, has_attachments
             ) VALUES (
                 1, 1, 'Inbox/legacy.eml', 'Legacy message', 'sender@example.com',
                 'recipient@example.com', '2025-01-02T03:04:05Z', 'Legacy body',
                 'Legacy body', 1
             );
             INSERT INTO attachments (id, message_id, file_name, content_type, size)
             VALUES (1, 1, 'legacy.pdf', 'application/pdf', 1234);
             INSERT INTO messages_fts (rowid, subject, sender, recipients, body)
             SELECT id, subject, sender, recipients, body FROM messages;",
        )
        .expect("legacy schema and rows should be created");
        conn
    }

    fn create_current_search_workspace(name: &str) -> (PathBuf, PathBuf) {
        let workspace = temporary_test_directory(name);
        let database_path = workspace.join("index.sqlite");
        let conn = Connection::open(&database_path).expect("search database should open");
        initialize_schema(&conn).expect("current schema should initialize");
        conn.execute(
            "INSERT INTO folders (id, parent_id, path, name) VALUES (1, NULL, 'Inbox', 'Inbox')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (
                 id, folder_id, eml_path, subject, sender, recipients, date, body,
                 body_source, snippet, attachment_names, has_attachments
             ) VALUES (
                 1, 1, 'Inbox/message.eml', 'Searchable subject', 'sender@example.com',
                 'recipient@example.com', '2026-03-01T00:00:00+00:00', 'Searchable body',
                 'text_plain', 'Searchable body', '', 0
             )",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages_fts (
                 rowid, subject, sender, recipients, body, attachment_names
             ) SELECT id, subject, sender, recipients, body, attachment_names
                 FROM messages WHERE id = 1",
            [],
        )
        .unwrap();
        drop(conn);
        (workspace, database_path)
    }

    fn create_search_workspace_with_messages(
        name: &str,
        workspace_id: &str,
        message_count: i64,
    ) -> ActiveWorkspace {
        let workspace = temporary_test_directory(name);
        fs::set_permissions(
            &workspace,
            fs::Permissions::from_mode(PRIVATE_DIRECTORY_MODE),
        )
        .expect("synthetic search workspace should be private");
        let workspace = workspace
            .canonicalize()
            .expect("synthetic search workspace should have a canonical path");
        let database_path = workspace.join("index.sqlite");
        let conn = Connection::open(&database_path).expect("search database should open");
        initialize_schema(&conn).expect("current schema should initialize");
        conn.execute(
            "INSERT INTO folders (id, parent_id, path, name) VALUES (1, NULL, 'Inbox', 'Inbox')",
            [],
        )
        .unwrap();
        set_metadata_value(
            &conn,
            "conversation_schema_version",
            CONVERSATION_SCHEMA_VERSION,
        )
        .unwrap();
        for id in 1..=message_count {
            let conversation_number = (id - 1) / 2 + 1;
            let conversation_root = (conversation_number - 1) * 2 + 1;
            let attachment_name = if id == 1 { "report.pdf" } else { "" };
            conn.execute(
                "INSERT INTO messages (
                     id, folder_id, eml_path, subject, sender, recipients, date, body,
                     body_source, snippet, attachment_names, has_attachments,
                     normalized_subject, conversation_id, conversation_root_id,
                     thread_assignment_method
                 ) VALUES (
                     ?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, 'text_plain', ?7, ?8, ?9,
                     ?10, ?11, ?12, 'header'
                 )",
                params![
                    id,
                    format!("Inbox/message-{id}.eml"),
                    format!("Shared subject {conversation_number}"),
                    format!("sender-{id}@example.test"),
                    "recipient@example.test",
                    format!("2026-01-01T00:{id:02}:00+00:00"),
                    format!("Shared body text for synthetic message {id}"),
                    attachment_name,
                    i64::from(id == 1),
                    format!("Shared subject {conversation_number}"),
                    format!("conversation-{conversation_number}"),
                    conversation_root,
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO messages_fts (
                     rowid, subject, sender, recipients, body, attachment_names
                 ) SELECT id, subject, sender, recipients, body, attachment_names
                     FROM messages WHERE id = ?1",
                params![id],
            )
            .unwrap();
        }
        drop(conn);
        ActiveWorkspace {
            id: workspace_id.to_string(),
            pst_path: workspace.join(format!("{workspace_id}.pst")),
            path: workspace,
            fingerprint: format!("fingerprint-{workspace_id}"),
            location_mode: WorkspaceLocationMode::AppSupport,
        }
    }

    fn collect_cursor_message_ids(
        active: &ActiveWorkspace,
        conn: &Connection,
        criteria: &MessageSearchCriteria,
        sort: &str,
        limit: i64,
        codec: &SearchCursorCodec,
    ) -> Vec<i64> {
        let mut cursor = None;
        let mut ids = Vec::new();
        for _ in 0..100 {
            let page = query_messages_cursor_page(
                conn,
                &active.path,
                &active.id,
                None,
                false,
                criteria,
                Some(sort),
                Some(limit),
                cursor.as_deref(),
                11,
                codec,
                None,
            )
            .expect("cursor page should load");
            ids.extend(page.items.iter().map(|item| item.id));
            if !page.has_more {
                assert!(page.next_cursor.is_none());
                return ids;
            }
            cursor = Some(page.next_cursor.expect("continuing page needs a cursor"));
        }
        panic!("cursor pagination did not terminate");
    }

    fn expect_app_error<T>(result: AppResult<T>) -> AppError {
        match result {
            Ok(_) => panic!("operation should fail"),
            Err(error) => error,
        }
    }

    fn sqlite_index_exists(conn: &Connection, name: &str) -> bool {
        conn.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM sqlite_master WHERE type = 'index' AND name = ?1
             )",
            params![name],
            |row| row.get::<_, i64>(0),
        )
        .map(|exists| exists != 0)
        .unwrap()
    }

    #[test]
    fn external_file_validation_checks_supported_formats() {
        let directory = temporary_test_directory("external-open");
        let eml_path = directory.join("message.EML");
        let invalid_msg_path = directory.join("invalid.msg");
        let pst_path = directory.join("archive.PST");
        let invalid_pst_path = directory.join("invalid.pst");
        fs::write(
            &eml_path,
            b"From: sender@example.com\r\nSubject: Test\r\n\r\nBody\r\n",
        )
        .expect("EML fixture should be written");
        fs::write(&invalid_msg_path, b"not a compound Outlook message")
            .expect("invalid MSG fixture should be written");
        fs::write(&pst_path, b"!BDNtest PST header").expect("PST fixture should be written");
        fs::write(&invalid_pst_path, b"not a PST").expect("invalid PST fixture should be written");

        let prepared =
            prepare_external_file_open(eml_path.clone()).expect("valid EML should prepare");
        assert_eq!(prepared.file_kind, "eml");
        assert_eq!(
            prepared.path,
            eml_path.canonicalize().unwrap().display().to_string()
        );
        assert!(!prepared.stable_id.is_empty());

        let prepared_pst =
            prepare_external_file_open(pst_path.clone()).expect("valid PST header should prepare");
        assert_eq!(prepared_pst.file_kind, "pst");
        assert_eq!(
            prepared_pst.path,
            pst_path.canonicalize().unwrap().display().to_string()
        );

        let error = prepare_external_file_open(invalid_msg_path)
            .expect_err("invalid MSG signature should be rejected");
        assert!(error
            .to_string()
            .contains("not an Outlook compound message"));
        let error = prepare_external_file_open(invalid_pst_path)
            .expect_err("invalid PST signature should be rejected");
        assert!(error.to_string().contains("valid Outlook PST header"));
        fs::remove_dir_all(directory).expect("temporary test directory should be removed");
    }

    #[test]
    fn workspace_deletion_requires_exact_marker_and_preserves_source() {
        let (root, active) = synthetic_workspace_delete_fixture("workspace-delete-valid");
        let profile = synthetic_runtime_profile(&root);
        fs::write(active.path.join("payload"), b"workspace data").unwrap();
        let source_hash = sha256_file(&active.pst_path).unwrap();
        let validated = validate_workspace_for_delete(profile.paths(), &active).unwrap();

        remove_validated_workspace(&active.path, &validated).unwrap();

        assert!(!active.path.exists());
        assert_eq!(sha256_file(&active.pst_path).unwrap(), source_hash);
        assert!(active.pst_path.is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_deletion_rejects_missing_or_tampered_marker() {
        let (root, active) = synthetic_workspace_delete_fixture("workspace-delete-marker");
        let profile = synthetic_runtime_profile(&root);
        let marker_path = active.path.join(WORKSPACE_MARKER_FILE);
        fs::remove_file(&marker_path).unwrap();
        assert!(validate_workspace_for_delete(profile.paths(), &active).is_err());
        assert!(active.path.is_dir());

        let mut marker = WorkspaceMarker {
            app: WORKSPACE_MARKER_APP_NAME.to_string(),
            workspace_id: active.id.clone(),
            pst_content_fingerprint: "wrong-fingerprint".to_string(),
            workspace_location_mode: active.location_mode.as_str().to_string(),
            original_pst_path: active.pst_path.display().to_string(),
        };
        let mut output = create_private_file_new(&marker_path).unwrap();
        serde_json::to_writer(&mut output, &marker).unwrap();
        drop(output);
        assert!(validate_workspace_for_delete(profile.paths(), &active).is_err());

        marker.pst_content_fingerprint = active.fingerprint.clone();
        fs::remove_file(&marker_path).unwrap();
        let mut output = create_private_file_new(&marker_path).unwrap();
        serde_json::to_writer(&mut output, &marker).unwrap();
        drop(output);
        assert!(validate_workspace_for_delete(profile.paths(), &active).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_deletion_rejects_symlink_and_identity_substitution() {
        use std::os::unix::fs::symlink;

        let (root, active) = synthetic_workspace_delete_fixture("workspace-delete-symlink");
        let profile = synthetic_runtime_profile(&root);
        let real_workspace = active.path.with_file_name("real-workspace");
        fs::rename(&active.path, &real_workspace).unwrap();
        symlink(&real_workspace, &active.path).unwrap();
        assert!(validate_workspace_for_delete(profile.paths(), &active).is_err());
        fs::remove_file(&active.path).unwrap();
        fs::rename(&real_workspace, &active.path).unwrap();

        let validated = validate_workspace_for_delete(profile.paths(), &active).unwrap();
        let wrong_identity = ValidatedWorkspaceDelete {
            device: validated.device,
            inode: validated.inode.saturating_add(1),
        };
        assert!(remove_validated_workspace(&active.path, &wrong_identity).is_err());
        assert!(active.path.is_dir());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_delete_validation_reads_index_without_modifying_it() {
        let (root, active) = synthetic_workspace_delete_fixture("workspace-delete-read-only");
        let profile = synthetic_runtime_profile(&root);
        let database_path = active.path.join("index.sqlite");
        let conn = Connection::open(&database_path).unwrap();
        initialize_schema(&conn).unwrap();
        set_metadata_value(&conn, "pst_content_fingerprint", &active.fingerprint).unwrap();
        drop(conn);
        let database_hash = sha256_file(&database_path).unwrap();

        validate_workspace_for_delete(profile.paths(), &active).unwrap();

        assert_eq!(sha256_file(&database_path).unwrap(), database_hash);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn workspace_marker_creation_is_private_and_never_overwrites_identity() {
        let root = temporary_test_directory("workspace-marker-private");
        let source = root.join("source.pst");
        fs::write(&source, b"!BDNsynthetic PST").unwrap();
        let source = source.canonicalize().unwrap();
        let source_metadata = source.metadata().unwrap();
        let workspace_id = "b".repeat(64);
        let workspace = root.join(NEXT_TO_PST_WORKSPACE_DIR).join(&workspace_id);
        let identity = PstIdentity {
            canonical_path: source.clone(),
            display_path: source.display().to_string(),
            size: source_metadata.len(),
            modified_ns: 0,
            workspace_id: workspace_id.clone(),
            legacy_workspace_id: "c".repeat(64),
            fingerprint: "fingerprint".to_string(),
            content_fingerprint: "content-fingerprint".to_string(),
            fingerprint_strategy: "synthetic".to_string(),
        };

        ensure_workspace_marker(
            &workspace,
            &workspace_id,
            &identity,
            WorkspaceLocationMode::NextToPst,
        )
        .unwrap();
        assert_eq!(
            fs::symlink_metadata(&workspace).unwrap().mode() & 0o777,
            PRIVATE_DIRECTORY_MODE
        );
        let marker_path = workspace.join(WORKSPACE_MARKER_FILE);
        assert_eq!(
            fs::symlink_metadata(&marker_path).unwrap().mode() & 0o777,
            PRIVATE_FILE_MODE
        );
        let marker_hash = sha256_file(&marker_path).unwrap();

        let mut mismatched = identity;
        mismatched.content_fingerprint = "other-content".to_string();
        assert!(ensure_workspace_marker(
            &workspace,
            &workspace_id,
            &mismatched,
            WorkspaceLocationMode::NextToPst,
        )
        .is_err());
        assert_eq!(sha256_file(&marker_path).unwrap(), marker_hash);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn attachment_filename_sanitization_bounds_hostile_names_and_preserves_unicode() {
        for hostile in [
            "../../secret.txt",
            "..\\..\\secret.txt",
            "/absolute/path.txt",
            "-leading-option",
            ".hidden",
            "CON",
            "nul.txt",
            "bad\0name.txt",
            "bad:name.txt",
            "   ",
        ] {
            let sanitized = sanitize_attachment_filename(hostile);
            assert!(!sanitized.is_empty());
            assert!(!sanitized.starts_with('.'));
            assert!(!sanitized.starts_with('-'));
            assert!(!sanitized.contains(['/', '\\', ':', '\0']));
            assert!(sanitized.len() <= 180);
        }
        assert_eq!(
            sanitize_attachment_filename("résumé-東京.txt"),
            "résumé-東京.txt"
        );
        assert!(sanitize_attachment_filename(&"😀".repeat(200)).len() <= 180);
        assert!(sanitize_attachment_filename("CON").starts_with("attachment-"));
        assert!(sanitize_attachment_filename("NUL.txt").starts_with("attachment-"));
    }

    #[test]
    fn hostile_unicode_cannot_panic_export_filename_generation() {
        for date in [
            "😀😀😀",
            "東京東京東京",
            "مرحبا-بكم",
            "e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}",
            "\u{fffd}\u{fffd}\u{fffd}\u{fffd}",
        ] {
            let generated = std::panic::catch_unwind(|| {
                export_message_eml_filename(7, date, "安全 مرحبا e\u{301} 😀")
            });
            let generated = generated.expect("hostile Unicode must not panic filename generation");
            assert!(generated.ends_with(".eml"));
            assert!(generated.starts_with("undated-"));
            assert!(generated.len() <= 180);
        }

        assert_eq!(
            message_export_date_prefix("2026-08-19T12:00:00Z"),
            "2026-08-19"
        );
        assert_eq!(message_export_date_prefix("2026/08/19"), "undated");
        assert!(export_source_message_filename(
            StandaloneSourceFormat::Msg,
            "😀😀😀",
            "東京 مرحبا e\u{301} 😀"
        )
        .ends_with(".msg"));
    }

    #[test]
    fn export_creation_is_private_atomic_and_collision_resistant() {
        use std::os::unix::fs::symlink;

        let root = temporary_test_directory("export-create");
        let export_dir = root.join("exports");
        create_private_directory_all(&export_dir).unwrap();
        fs::write(export_dir.join("Report.txt"), b"original").unwrap();
        fs::write(export_dir.join("café.txt"), b"unicode original").unwrap();

        let (case_path, mut case_file) =
            create_unique_private_export_file(&export_dir, "report.txt").unwrap();
        case_file.write_all(b"case-safe").unwrap();
        drop(case_file);
        assert_eq!(case_path.file_name().unwrap(), "report-1.txt");
        assert_eq!(
            fs::read(export_dir.join("Report.txt")).unwrap(),
            b"original"
        );
        assert_eq!(
            fs::symlink_metadata(&case_path).unwrap().mode() & 0o777,
            PRIVATE_FILE_MODE
        );

        let (unicode_path, unicode_file) =
            create_unique_private_export_file(&export_dir, "cafe\u{301}.txt").unwrap();
        drop(unicode_file);
        assert_eq!(unicode_path.file_name().unwrap(), "cafe\u{301}-1.txt");

        let external = root.join("external");
        fs::create_dir(&external).unwrap();
        let symlinked = root.join("symlinked-exports");
        symlink(&external, &symlinked).unwrap();
        assert!(create_private_directory_all(&symlinked).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn export_authorization_rejects_arbitrary_changed_and_replaced_files() {
        use std::os::unix::fs::symlink;

        let root = temporary_test_directory("export-authorization");
        let state = AppState::default();
        let path = root.join("authorized.bin");
        let mut file = create_private_file_new(&path).unwrap();
        file.write_all(b"original").unwrap();
        drop(file);
        let canonical = register_export_file(&state, &path).unwrap();
        let key = canonical.display().to_string();
        assert_eq!(authorized_export_path(&state, &key).unwrap(), canonical);
        assert!(authorized_export_path(&state, &root.join("other").display().to_string()).is_err());

        let mut changed = OpenOptions::new().append(true).open(&path).unwrap();
        changed.write_all(b"changed").unwrap();
        drop(changed);
        assert!(authorized_export_path(&state, &key).is_err());
        assert!(!state
            .exported_files
            .lock()
            .unwrap()
            .by_path
            .contains_key(&key));

        let replacement = root.join("replacement.bin");
        fs::write(&replacement, b"replacement").unwrap();
        fs::remove_file(&path).unwrap();
        let mut file = create_private_file_new(&path).unwrap();
        file.write_all(b"second").unwrap();
        drop(file);
        let canonical = register_export_file(&state, &path).unwrap();
        let key = canonical.display().to_string();
        fs::remove_file(&path).unwrap();
        symlink(&replacement, &path).unwrap();
        assert!(authorized_export_path(&state, &key).is_err());
        fs::remove_file(&path).unwrap();

        let hardlinked = root.join("hardlinked.bin");
        fs::hard_link(&replacement, &hardlinked).unwrap();
        assert!(register_export_file(&state, &hardlinked).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn standalone_authorization_rejects_unselected_and_changed_sources() {
        use std::os::unix::fs::symlink;

        let root = temporary_test_directory("standalone-identity");
        let source = root.join("message.eml");
        fs::write(
            &source,
            b"From: sender@example.test\r\nSubject: Test\r\n\r\nBody\r\n",
        )
        .unwrap();
        let selected = prepare_external_file_open(source.clone()).unwrap();
        let state = AppState::default();
        assert!(authorized_standalone_path(&state, &selected.stable_id).is_err());
        authorize_standalone_file(&mut state.external_file_opens.lock().unwrap(), &selected)
            .unwrap();
        assert_eq!(
            authorized_standalone_path(&state, &selected.stable_id).unwrap(),
            source.canonicalize().unwrap()
        );

        let mut changed = OpenOptions::new().append(true).open(&source).unwrap();
        changed.write_all(b"changed").unwrap();
        drop(changed);
        assert!(authorized_standalone_path(&state, &selected.stable_id).is_err());

        let symlink_path = root.join("symlink.eml");
        symlink(&source, &symlink_path).unwrap();
        assert!(prepare_external_file_open(symlink_path).is_err());
        let hardlink_path = root.join("hardlink.eml");
        fs::hard_link(&source, &hardlink_path).unwrap();
        assert!(prepare_external_file_open(hardlink_path).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn export_authorization_registry_is_bounded() {
        let root = temporary_test_directory("export-registry-bound");
        let state = AppState::default();
        for index in 0..MAX_AUTHORIZED_EXPORT_FILES + 7 {
            let path = root.join(format!("export-{index}.bin"));
            let file = create_private_file_new(&path).unwrap();
            drop(file);
            register_export_file(&state, &path).unwrap();
        }
        let registry = state.exported_files.lock().unwrap();
        assert_eq!(registry.by_path.len(), MAX_AUTHORIZED_EXPORT_FILES);
        assert_eq!(registry.order.len(), MAX_AUTHORIZED_EXPORT_FILES);
        drop(registry);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn open_workspace_and_search_request_state_is_strictly_bounded() {
        let state = AppState::default();
        for index in 0..MAX_OPEN_WORKSPACES {
            let active = ActiveWorkspace {
                id: format!("workspace-{index}"),
                path: PathBuf::from(format!("/synthetic/workspace-{index}")),
                pst_path: PathBuf::from(format!("/synthetic/source-{index}.pst")),
                fingerprint: format!("fingerprint-{index}"),
                location_mode: WorkspaceLocationMode::AppSupport,
            };
            remember_open_workspace(&state, &active).unwrap();
        }
        assert_eq!(
            state.open_workspaces.lock().unwrap().len(),
            MAX_OPEN_WORKSPACES
        );

        let overflow = ActiveWorkspace {
            id: "workspace-overflow".to_string(),
            path: PathBuf::from("/synthetic/workspace-overflow"),
            pst_path: PathBuf::from("/synthetic/source-overflow.pst"),
            fingerprint: "fingerprint-overflow".to_string(),
            location_mode: WorkspaceLocationMode::AppSupport,
        };
        assert!(remember_open_workspace(&state, &overflow).is_err());
        assert_eq!(
            state.open_workspaces.lock().unwrap().len(),
            MAX_OPEN_WORKSPACES
        );

        let replacement = ActiveWorkspace {
            id: "workspace-0".to_string(),
            path: PathBuf::from("/synthetic/replacement"),
            pst_path: PathBuf::from("/synthetic/replacement.pst"),
            fingerprint: "replacement".to_string(),
            location_mode: WorkspaceLocationMode::AppSupport,
        };
        remember_open_workspace(&state, &replacement).unwrap();
        assert_eq!(
            state.open_workspaces.lock().unwrap().len(),
            MAX_OPEN_WORKSPACES
        );
        assert_eq!(
            state
                .open_workspaces
                .lock()
                .unwrap()
                .get("workspace-0")
                .unwrap()
                .path,
            replacement.path
        );

        assert!(validate_workspace_batch_size(MAX_OPEN_WORKSPACES).is_ok());
        assert!(validate_workspace_batch_size(MAX_OPEN_WORKSPACES + 1).is_err());
        assert!(validate_search_offset(Some(MAX_SEARCH_OFFSET_ROWS)).is_ok());
        assert!(validate_search_offset(Some(MAX_SEARCH_OFFSET_ROWS + 1)).is_err());
        assert!(validate_search_offset(Some(-1)).is_err());
        assert!(validate_workspace_identity_input(&"x".repeat(MAX_WORKSPACE_ID_SCALARS)).is_ok());
        assert!(
            validate_workspace_identity_input(&"x".repeat(MAX_WORKSPACE_ID_SCALARS + 1)).is_err()
        );
        assert!(
            validate_conversation_identity_input(&"x".repeat(MAX_CONVERSATION_ID_SCALARS)).is_ok()
        );
        assert!(
            validate_conversation_identity_input(&"x".repeat(MAX_CONVERSATION_ID_SCALARS + 1))
                .is_err()
        );
        assert_eq!(checked_result_count(7, 11).unwrap(), 18);
        assert!(checked_result_count(i64::MAX, 1).is_err());
        assert!(checked_result_count(0, -1).is_err());
    }

    #[test]
    fn preflight_and_private_outputs_fail_safely_under_storage_errors() {
        let root = temporary_test_directory("storage-failure");
        let source = root.join("source.pst");
        fs::write(&source, b"!BDNsynthetic source").unwrap();
        let source_hash = sha256_file(&source).unwrap();
        let identity = PstIdentity {
            canonical_path: source.canonicalize().unwrap(),
            display_path: source.display().to_string(),
            size: u64::MAX,
            modified_ns: 0,
            workspace_id: "a".repeat(64),
            legacy_workspace_id: "b".repeat(64),
            fingerprint: "synthetic".to_string(),
            content_fingerprint: "synthetic-content".to_string(),
            fingerprint_strategy: "synthetic".to_string(),
        };
        let selection = WorkspaceSelection {
            path: root.join("workspace"),
            mode: WorkspaceLocationMode::AppSupport,
            warning: None,
        };
        let preflight = workspace_preflight(&identity, &selection);
        assert_eq!(preflight.has_enough_space, Some(false));
        assert!(preflight.space_warning);
        assert_eq!(sha256_file(&source).unwrap(), source_hash);

        let blocker = root.join("not-a-directory");
        fs::write(&blocker, b"blocker").unwrap();
        assert!(create_private_directory_all(&blocker.join("child")).is_err());
        assert!(create_private_file_new(&blocker.join("output.bin")).is_err());
        assert!(!blocker.join("output.bin").exists());
        assert_eq!(sha256_file(&source).unwrap(), source_hash);
        assert!(fs::read_dir(&root).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".pst-quickview-")
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn external_file_batch_keeps_valid_mixed_files_and_caps_the_request() {
        let directory = temporary_test_directory("external-mixed-batch");
        let pst_path = directory.join("archive.pst");
        let msg_path = directory.join("message.msg");
        let unsupported_path = directory.join("notes.txt");
        fs::write(&pst_path, b"!BDNtest PST header").expect("PST fixture should be written");
        fs::write(
            &msg_path,
            b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1test MSG header",
        )
        .expect("MSG fixture should be written");
        fs::write(&unsupported_path, b"unsupported").expect("text fixture should be written");

        let mut candidates = vec![
            pst_path.clone(),
            msg_path.clone(),
            unsupported_path,
            pst_path,
        ];
        for index in 0..MAX_EXTERNAL_FILES_PER_REQUEST {
            let eml_path = directory.join(format!("message-{index}.eml"));
            fs::write(
                &eml_path,
                format!("From: sender@example.com\r\nSubject: Message {index}\r\n\r\nBody\r\n"),
            )
            .expect("EML fixture should be written");
            candidates.push(eml_path);
        }

        let batch = build_external_file_open_batch(candidates, Vec::new());
        assert_eq!(batch.files.len(), MAX_EXTERNAL_FILES_PER_REQUEST);
        assert!(batch.files.iter().any(|file| file.file_kind == "pst"));
        assert!(batch.files.iter().any(|file| file.file_kind == "msg"));
        assert!(batch.files.iter().any(|file| file.file_kind == "eml"));
        assert_eq!(
            batch
                .files
                .iter()
                .filter(|file| file.file_kind == "pst")
                .count(),
            1,
            "duplicate canonical PST paths should be removed"
        );
        assert!(batch.skipped_count >= 3);
        assert!(batch
            .warnings
            .iter()
            .any(|warning| warning.contains("notes.txt")));
        assert!(batch
            .warnings
            .iter()
            .any(|warning| warning.contains("at most 10 files")));
        fs::remove_dir_all(directory).expect("temporary test directory should be removed");
    }

    #[test]
    fn startup_external_file_queue_is_bounded_and_deduplicated() {
        let mut state = ExternalFileOpenState::default();
        for batch_number in 0..4 {
            let start = batch_number * MAX_EXTERNAL_FILES_PER_REQUEST;
            let batch = ExternalFileOpenBatch {
                files: (start..start + MAX_EXTERNAL_FILES_PER_REQUEST)
                    .map(external_test_file)
                    .collect(),
                warnings: Vec::new(),
                skipped_count: 0,
            };
            queue_external_file_open_batch(&mut state, batch);
        }

        let queued = state
            .pending
            .iter()
            .flat_map(|batch| batch.files.iter())
            .collect::<Vec<_>>();
        assert_eq!(queued.len(), MAX_PENDING_EXTERNAL_FILES);
        assert_eq!(
            queued
                .iter()
                .map(|file| file.path.as_str())
                .collect::<HashSet<_>>()
                .len(),
            MAX_PENDING_EXTERNAL_FILES
        );

        queue_external_file_open_batch(
            &mut state,
            ExternalFileOpenBatch {
                files: vec![external_test_file(0)],
                warnings: Vec::new(),
                skipped_count: 0,
            },
        );
        assert_eq!(
            state
                .pending
                .iter()
                .map(|batch| batch.files.len())
                .sum::<usize>(),
            MAX_PENDING_EXTERNAL_FILES
        );
    }

    #[test]
    fn preview_registry_uses_opaque_labels_deduplicates_and_cleans_up() {
        let target = PreviewWindowTarget::Standalone {
            stable_id: "d".repeat(64),
        };
        let mut registry = PreviewWindowRegistry::default();
        let label = registry.register(target.clone()).unwrap();

        assert!(is_valid_preview_window_label(&label));
        assert!(!label.contains("synthetic"));
        assert!(!label.contains("message.eml"));
        assert_eq!(registry.register(target.clone()).unwrap(), label);
        assert_eq!(registry.targets_by_label.len(), 1);
        assert_eq!(registry.target(&label), Some(target));

        registry.remove(&label);
        assert!(registry.targets_by_label.is_empty());
        assert!(registry.labels_by_target.is_empty());
        assert!(registry.target(&label).is_none());
    }

    #[test]
    fn preview_registry_is_bounded_and_rejects_hostile_labels() {
        let mut registry = PreviewWindowRegistry::default();
        for message_id in 1..=MAX_PREVIEW_WINDOWS as i64 {
            registry
                .register(PreviewWindowTarget::Workspace {
                    workspace_id: "synthetic-workspace".to_string(),
                    message_id,
                })
                .unwrap();
        }
        assert_eq!(registry.targets_by_label.len(), MAX_PREVIEW_WINDOWS);
        assert!(registry
            .register(PreviewWindowTarget::Workspace {
                workspace_id: "synthetic-workspace".to_string(),
                message_id: MAX_PREVIEW_WINDOWS as i64 + 1,
            })
            .is_err());

        for label in [
            "main",
            "message-preview-../../main",
            "message-preview-ABCDEF",
            "message-preview-0000",
            "message-preview-gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg",
        ] {
            assert!(!is_valid_preview_window_label(label));
        }
    }

    #[test]
    fn standalone_preview_authorizations_are_bounded_and_exclude_pst_files() {
        let mut state = ExternalFileOpenState::default();
        let directory = temporary_test_directory("standalone-authorizations");
        let mut first_stable_id = String::new();
        for id in 0..MAX_AUTHORIZED_STANDALONE_FILES + 5 {
            let path = directory.join(format!("message-{id}.eml"));
            fs::write(
                &path,
                format!("From: sender@example.test\r\nSubject: {id}\r\n\r\nBody\r\n"),
            )
            .unwrap();
            let file = prepare_external_file_open(path).unwrap();
            if id == 0 {
                first_stable_id = file.stable_id.clone();
            }
            authorize_standalone_files(
                &mut state,
                &ExternalFileOpenBatch {
                    files: vec![file],
                    warnings: Vec::new(),
                    skipped_count: 0,
                },
            )
            .unwrap();
        }
        authorize_standalone_files(
            &mut state,
            &ExternalFileOpenBatch {
                files: vec![ExternalFileOpen {
                    path: "/synthetic/archive.pst".to_string(),
                    file_kind: "pst".to_string(),
                    stable_id: format!("{:064x}", usize::MAX),
                }],
                warnings: Vec::new(),
                skipped_count: 0,
            },
        )
        .unwrap();

        assert_eq!(
            state.authorized_standalone_files.len(),
            MAX_AUTHORIZED_STANDALONE_FILES
        );
        assert_eq!(
            state.authorization_order.len(),
            MAX_AUTHORIZED_STANDALONE_FILES
        );
        assert!(!state
            .authorized_standalone_files
            .contains_key(&format!("{:064x}", usize::MAX)));
        assert!(!state
            .authorized_standalone_files
            .contains_key(&first_stable_id));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn preview_target_serialization_exposes_only_the_typed_target_contract() {
        let workspace = serde_json::to_value(PreviewWindowTarget::Workspace {
            workspace_id: "synthetic-workspace".to_string(),
            message_id: 7,
        })
        .unwrap();
        assert_eq!(
            workspace,
            serde_json::json!({
                "kind": "workspace",
                "workspaceId": "synthetic-workspace",
                "messageId": 7
            })
        );
        let standalone = serde_json::to_value(PreviewWindowTarget::Standalone {
            stable_id: "e".repeat(64),
        })
        .unwrap();
        assert_eq!(
            standalone,
            serde_json::json!({
                "kind": "standalone",
                "stableId": "e".repeat(64)
            })
        );
    }

    #[test]
    fn recovers_fromhtml_rtf_and_collects_exact_cid_references() {
        let rtf = r#"{\rtf1\ansi\ansicpg1252\fromhtml1{\*\htmltag0 <html><body><p>Alex\'92s note</p><img src=\"cid:image004.png@example\"></body></html>}}"#;
        let conversion = convert_rtf_payload(rtf).expect("RTF should convert");
        let html = conversion.html.expect("fromhtml RTF should recover HTML");

        assert_eq!(conversion.kind, RtfConversionKind::FromHtml);
        assert!(conversion.text.contains("Alex’s note"));
        assert!(html_cid_references(&html).contains("image004.png@example"));
    }

    #[test]
    fn only_known_rtf_body_filenames_are_strong_body_evidence() {
        assert!(is_known_msg_rtf_body_filename("rtf-body.rtf"));
        assert!(is_known_msg_rtf_body_filename("BODY.RTF"));
        assert!(is_known_msg_rtf_body_filename("message.rtf"));
        assert!(!is_known_msg_rtf_body_filename("contract.rtf"));
        assert!(!is_known_msg_rtf_body_filename("notes.rtf"));
    }

    #[test]
    fn malformed_rtf_does_not_produce_a_body() {
        assert!(convert_rtf_bytes(b"not an RTF document").is_none());
        assert!(convert_rtf_payload("{\\rtf1\\ansi").is_none());
        let deeply_nested = format!(
            "{{\\rtf1{}text{}",
            "{".repeat(MAX_RTF_GROUP_DEPTH + 1),
            "}".repeat(MAX_RTF_GROUP_DEPTH + 2)
        );
        assert!(convert_rtf_payload(&deeply_nested).is_none());
    }

    #[test]
    fn sanitizer_preserves_bounded_legacy_layout_without_active_content() {
        let mut cid_images = HashMap::new();
        cid_images.insert(
            "icon@example".to_string(),
            "data:image/png;base64,aGVsbG8=".to_string(),
        );
        let html = r#"
            <table width="100%" height="200" align="left" cellpadding="2" cellspacing="0" border="0">
              <thead><tr valign="middle"><th colspan="2" width="600">Heading</th></tr></thead>
              <tbody><tr><td width="100000"><a href="https://example.com"><img src="cid:icon@example" width="17" height="17" align="middle" border="0" style="display:block" onerror="alert(1)"></a></td></tr></tbody>
              <tfoot><tr><td>Footer</td></tr></tfoot>
            </table>
            <script>alert(1)</script><svg onload="alert(1)"><circle></circle></svg>
        "#;

        let result = sanitize_email_html(html, cid_images);

        assert!(result.sanitized_html.contains("<table"));
        assert!(result.sanitized_html.contains("<thead>"));
        assert!(result.sanitized_html.contains("<tbody>"));
        assert!(result.sanitized_html.contains("<tfoot>"));
        assert!(result.sanitized_html.contains("width=\"17\""));
        assert!(result.sanitized_html.contains("height=\"17\""));
        assert!(result
            .sanitized_html
            .contains("data:image/png;base64,aGVsbG8="));
        assert!(!result.sanitized_html.contains("width=\"100000\""));
        assert!(!result.sanitized_html.contains("style="));
        assert!(!result.sanitized_html.contains("onerror"));
        assert!(!result.sanitized_html.contains("<script"));
        assert!(!result.sanitized_html.contains("<svg"));
    }

    #[test]
    fn sanitizer_blocks_every_external_or_active_message_resource() {
        let html = r#"
            <img src="https://remote.invalid/pixel.png">
            <img src="HTTP://127.0.0.1/private.png">
            <img src="//remote.invalid/protocol-relative.png">
            <img src="file:///private/secret.png">
            <img src="javascript:alert(1)">
            <img src="vbscript:msgbox(1)">
            <img src="data:image/svg+xml,&lt;svg onload='alert(1)'/&gt;">
            <iframe srcdoc="&lt;script&gt;alert(1)&lt;/script&gt;"></iframe>
            <object data="https://remote.invalid/object"></object>
            <embed src="https://remote.invalid/embed">
            <form action="https://remote.invalid/submit"><input></form>
            <meta http-equiv="refresh" content="0;url=https://remote.invalid/refresh">
            <base href="https://remote.invalid/">
            <style>@import url(https://remote.invalid/style.css);</style>
            <svg><script>alert(1)</script></svg>
            <math><mtext onclick="alert(1)">unsafe</mtext></math>
            <img src="data:image/png;base64,aGVsbG8=">
        "#;

        let result = sanitize_email_html(html, HashMap::new());
        let sanitized_lower = result.sanitized_html.to_ascii_lowercase();

        assert!(result.remote_images_blocked);
        assert!(result.remote_image_count >= 7);
        assert!(result
            .sanitized_html
            .contains("data:image/png;base64,aGVsbG8="));
        for forbidden in [
            "http:",
            "https:",
            "//remote.invalid",
            "file:",
            "javascript:",
            "vbscript:",
            "image/svg",
            "<iframe",
            "<object",
            "<embed",
            "<form",
            "<meta",
            "<base",
            "<style",
            "<svg",
            "<math",
            "onclick",
            "onload",
            "srcdoc",
        ] {
            assert!(
                !sanitized_lower.contains(forbidden),
                "sanitized HTML retained forbidden content: {forbidden}"
            );
        }
    }

    #[test]
    fn printable_export_rebuilds_untrusted_html_with_a_fixed_offline_document() {
        let html = r#"<!doctype html>
            <html><head>
              <title>Private synthetic title</title>
              <style>@import url(https://remote.invalid/style.css);</style>
              <meta http-equiv="refresh" content="0;url=http://127.0.0.1/private">
              <base href="file:///private/">
            </head><body onload="alert(1)">
              <h1>Safe synthetic heading</h1>
              <a href="https://user:secret@remote.invalid/track">tracked link</a>
              <img src="//remote.invalid/pixel.png" onerror="alert(1)">
              <img src="data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=">
              <img src="data:image/png;base64,aGVsbG8=">
              <iframe srcdoc="&lt;script&gt;alert(1)&lt;/script&gt;"></iframe>
              <object data="https://remote.invalid/object"></object>
              <form action="http://[::1]/submit"><input></form>
              <script>window.__TAURI_INTERNALS__.invoke('delete_workspace')</script>
            </body></html>"#;
        let document = sanitize_printable_html_document(html).unwrap();
        let lower = document.to_ascii_lowercase();

        assert!(document.contains("Safe synthetic heading"));
        assert!(document.contains("data:image/png;base64,aGVsbG8="));
        assert!(document.contains("default-src 'none'"));
        assert_eq!(lower.matches("<style>").count(), 1);
        for forbidden in [
            "remote.invalid",
            "127.0.0.1",
            "[::1]",
            "file:",
            "user:secret",
            "image/svg",
            "<iframe",
            "<object",
            "<form",
            "<script",
            "srcdoc",
            "onerror",
            "onload",
            "__tauri_internals__",
            "@import",
        ] {
            assert!(
                !lower.contains(forbidden),
                "printable HTML retained forbidden content: {forbidden}"
            );
        }
        assert!(validate_printable_html_size(MAX_PRINTABLE_HTML_BYTES).is_ok());
        assert!(validate_printable_html_size(MAX_PRINTABLE_HTML_BYTES + 1).is_err());
    }

    #[test]
    fn conversation_queries_preserve_folder_and_search_scope_with_context() {
        let conn = Connection::open_in_memory().expect("test database should open");
        initialize_schema(&conn).expect("schema should initialize");
        conn.execute(
            "INSERT INTO folders (id, parent_id, path, name) VALUES (1, NULL, 'Inbox', 'Inbox')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO folders (id, parent_id, path, name) VALUES (2, NULL, 'Sent', 'Sent')",
            [],
        )
        .unwrap();
        for (id, folder_id, subject, sender, date, body) in [
            (
                1,
                1,
                "Project update",
                "alice@example.com",
                "2026-01-01T10:00:00Z",
                "Initial status",
            ),
            (
                2,
                2,
                "Re: Project update",
                "bob@example.com",
                "2026-01-02T10:00:00Z",
                "Reply calendar detail",
            ),
        ] {
            conn.execute(
                "INSERT INTO messages (
                     id, folder_id, eml_path, subject, sender, recipients, date, body,
                     snippet, attachment_names, has_attachments, normalized_subject,
                     conversation_id, conversation_root_id, thread_assignment_method
                 ) VALUES (?1, ?2, ?3, ?4, ?5, 'team@example.com', ?6, ?7, ?7, '', 0,
                           'Project update', 'conversation-one', 1, 'header')",
                params![
                    id,
                    folder_id,
                    format!("{id}.eml"),
                    subject,
                    sender,
                    date,
                    body
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO messages_fts (rowid, subject, sender, recipients, body, attachment_names)
                 SELECT id, subject, sender, recipients, body, attachment_names
                   FROM messages WHERE id = ?1",
                params![id],
            )
            .unwrap();
        }

        let criteria = MessageSearchCriteria::from_inputs(Some("calendar".to_string()), None)
            .expect("conversation search should be valid");
        let (conversation_count, matching_count) =
            conversation_counts(&conn, None, false, &criteria, None).unwrap();
        assert_eq!((conversation_count, matching_count), (1, 1));

        let active = ActiveWorkspace {
            id: "workspace-a".to_string(),
            path: PathBuf::from("/tmp/workspace-a"),
            pst_path: PathBuf::from("/tmp/a.pst"),
            fingerprint: "fingerprint-a".to_string(),
            location_mode: WorkspaceLocationMode::AppSupport,
        };
        let summaries = query_conversation_summaries(
            &conn, &active, None, false, &criteria, "newest", 100, 0, None,
        )
        .unwrap();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].matching_message_count, 1);
        assert_eq!(summaries[0].total_message_count, 2);
        assert_eq!(
            summaries[0].participants,
            vec!["bob@example.com", "alice@example.com"]
        );

        let source = build_message_query_source(&conn, None, false, &criteria).unwrap();
        let matching =
            query_matching_conversation_page(&conn, &source, "conversation-one", 100, 0, None)
                .unwrap();
        assert_eq!(matching.len(), 1);
        assert_eq!(matching[0].message.id, 2);
        assert!(matching[0].matches_scope);
        assert_eq!(
            matching[0]
                .message
                .search_match_context
                .as_ref()
                .map(|context| context.matched_fields.as_slice()),
            Some(&[search::SearchMatchedField::Body][..])
        );

        let entire =
            query_entire_conversation_page(&conn, &source, "conversation-one", 100, 0, None)
                .unwrap();
        assert_eq!(entire.len(), 2);
        assert!(!entire[0].matches_scope);
        assert!(entire[1].matches_scope);
        assert!(entire
            .iter()
            .all(|item| item.message.search_match_context.is_none()));
    }

    #[test]
    fn message_pages_use_limit_plus_one_without_exposing_the_extra_row() {
        let active = create_search_workspace_with_messages("message-page", "workspace-page", 5);
        let conn = open_workspace_db_for_read(&active.path).unwrap();
        let criteria = MessageSearchCriteria::from_inputs(None, None).unwrap();

        let first = query_messages_page(MessagePageQuery {
            conn: &conn,
            folder_id: None,
            include_subfolders: false,
            criteria: &criteria,
            sort_order: Some("newest"),
            limit: Some(3),
            offset: Some(0),
            operation: None,
        })
        .unwrap();
        assert_eq!(first.requested_offset, 0);
        assert_eq!(first.returned_count, 3);
        assert_eq!(first.items.len(), 3, "the lookahead row must not leak");
        assert!(first.has_more);
        assert_eq!(
            first.items.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![5, 4, 3]
        );

        let second = query_messages_page(MessagePageQuery {
            conn: &conn,
            folder_id: None,
            include_subfolders: false,
            criteria: &criteria,
            sort_order: Some("newest"),
            limit: Some(3),
            offset: Some(3),
            operation: None,
        })
        .unwrap();
        assert_eq!(second.requested_offset, 3);
        assert_eq!(second.returned_count, 2);
        assert!(!second.has_more);
        assert_eq!(
            second.items.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(
            query_message_count(&conn, None, false, &criteria, None).unwrap(),
            5
        );
        let attachment_filter: SearchFilters = serde_json::from_value(serde_json::json!({
            "hasAttachments": "yes"
        }))
        .unwrap();
        let filtered = MessageSearchCriteria::from_inputs(None, Some(attachment_filter)).unwrap();
        assert_eq!(
            query_message_count(&conn, None, false, &filtered, None).unwrap(),
            1,
            "blank text plus structured filters must count independently"
        );
        let body_filter: SearchFilters = serde_json::from_value(serde_json::json!({
            "body": "synthetic"
        }))
        .unwrap();
        let text_and_filter =
            MessageSearchCriteria::from_inputs(Some("shared".to_string()), Some(body_filter))
                .unwrap();
        assert_eq!(
            query_message_count(&conn, None, false, &text_and_filter, None).unwrap(),
            5,
            "text and structured filters must share the page criteria"
        );

        drop(conn);
        fs::remove_dir_all(active.path).unwrap();
    }

    #[test]
    fn exact_limit_message_page_reports_no_more_and_preserves_context() {
        let active = create_search_workspace_with_messages("message-exact", "workspace-exact", 3);
        let conn = open_workspace_db_for_read(&active.path).unwrap();
        let criteria =
            MessageSearchCriteria::from_inputs(Some("shared".to_string()), None).unwrap();
        let page = query_messages_page(MessagePageQuery {
            conn: &conn,
            folder_id: None,
            include_subfolders: false,
            criteria: &criteria,
            sort_order: Some("relevance"),
            limit: Some(3),
            offset: Some(0),
            operation: None,
        })
        .unwrap();
        assert_eq!(page.returned_count, 3);
        assert!(!page.has_more);
        assert!(page
            .items
            .iter()
            .all(|item| item.search_match_context.is_some()));
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.id)
                .collect::<HashSet<_>>()
                .len(),
            page.items.len(),
            "stable relevance pagination must not duplicate rows"
        );

        drop(conn);
        fs::remove_dir_all(active.path).unwrap();
    }

    #[test]
    fn single_workspace_cursor_pages_match_full_order_for_every_message_sort() {
        let active =
            create_search_workspace_with_messages("message-cursor-order", "workspace-cursor", 9);
        let writer = Connection::open(active.path.join("index.sqlite")).unwrap();
        writer
            .execute_batch(
                "UPDATE messages
                    SET date = CASE id
                        WHEN 1 THEN NULL
                        WHEN 2 THEN '2026-01-01T00:00:00+00:00'
                        WHEN 3 THEN '2026-01-01T00:00:00+00:00'
                        ELSE date
                    END,
                        sender = CASE id
                            WHEN 1 THEN NULL
                            WHEN 2 THEN 'Alpha'
                            WHEN 3 THEN 'alpha'
                            WHEN 4 THEN 'Ångström'
                            WHEN 5 THEN '東京'
                            ELSE 'Zulu'
                        END,
                        subject = CASE id
                            WHEN 1 THEN NULL
                            WHEN 2 THEN 'Shared Alpha'
                            WHEN 3 THEN 'shared alpha'
                            WHEN 4 THEN 'Shared Ångström'
                            WHEN 5 THEN 'Shared 東京'
                            ELSE 'Shared Zulu'
                        END;
                 INSERT INTO messages_fts(messages_fts) VALUES('rebuild');",
            )
            .unwrap();
        drop(writer);

        let conn = open_workspace_db_for_read(&active.path).unwrap();
        let codec = SearchCursorCodec::default();
        for (sort, criteria) in [
            (
                "newest",
                MessageSearchCriteria::from_inputs(None, None).unwrap(),
            ),
            (
                "oldest",
                MessageSearchCriteria::from_inputs(None, None).unwrap(),
            ),
            (
                "sender_az",
                MessageSearchCriteria::from_inputs(None, None).unwrap(),
            ),
            (
                "subject_az",
                MessageSearchCriteria::from_inputs(None, None).unwrap(),
            ),
            (
                "relevance",
                MessageSearchCriteria::from_inputs(Some("shared".to_string()), None).unwrap(),
            ),
        ] {
            let expected = query_messages_page(MessagePageQuery {
                conn: &conn,
                folder_id: None,
                include_subfolders: false,
                criteria: &criteria,
                sort_order: Some(sort),
                limit: Some(100),
                offset: Some(0),
                operation: None,
            })
            .unwrap()
            .items
            .into_iter()
            .map(|item| item.id)
            .collect::<Vec<_>>();
            let actual = collect_cursor_message_ids(&active, &conn, &criteria, sort, 2, &codec);
            assert_eq!(actual, expected, "{sort} cursor order must match ORDER BY");
            assert_eq!(
                actual.iter().copied().collect::<HashSet<_>>().len(),
                actual.len(),
                "{sort} cursor pages must not duplicate rows"
            );
        }

        drop(conn);
        fs::remove_dir_all(active.path).unwrap();
    }

    #[test]
    fn cursor_page_uses_limit_plus_one_context_and_no_offset_sql() {
        let active = create_search_workspace_with_messages(
            "message-cursor-page",
            "workspace-cursor-page",
            5,
        );
        let conn = open_workspace_db_for_read(&active.path).unwrap();
        let criteria =
            MessageSearchCriteria::from_inputs(Some("shared".to_string()), None).unwrap();
        let source = build_message_query_source(&conn, None, false, &criteria).unwrap();
        let sql = message_cursor_page_sql(
            &source,
            &source.where_sql,
            MessageSort::from_request(Some("relevance"), source.has_text_match).unwrap(),
        );
        assert!(!sql.to_ascii_uppercase().contains(" OFFSET"));

        let codec = SearchCursorCodec::default();
        let first = query_messages_cursor_page(
            &conn,
            &active.path,
            &active.id,
            None,
            false,
            &criteria,
            Some("relevance"),
            Some(3),
            None,
            11,
            &codec,
            None,
        )
        .unwrap();
        assert_eq!(first.pagination_mode, "cursor");
        assert_eq!(first.requested_offset, 0);
        assert_eq!(first.items.len(), 3);
        assert!(first.has_more);
        assert!(first.next_cursor.is_some());
        assert!(first
            .items
            .iter()
            .all(|item| item.search_match_context.is_some()));

        let second = query_messages_cursor_page(
            &conn,
            &active.path,
            &active.id,
            None,
            false,
            &criteria,
            Some("relevance"),
            Some(3),
            first.next_cursor.as_deref(),
            11,
            &codec,
            None,
        )
        .unwrap();
        assert_eq!(second.items.len(), 2);
        assert!(!second.has_more);
        assert!(second.next_cursor.is_none());
        assert!(first
            .items
            .iter()
            .all(|first_item| second.items.iter().all(|item| item.id != first_item.id)));

        drop(conn);
        fs::remove_dir_all(active.path).unwrap();
    }

    #[test]
    fn invalid_wrong_context_and_stale_message_cursors_return_typed_errors() {
        let active =
            create_search_workspace_with_messages("message-cursor-errors", "workspace-errors", 4);
        let conn = open_workspace_db_for_read(&active.path).unwrap();
        let criteria = MessageSearchCriteria::from_inputs(None, None).unwrap();
        let codec = SearchCursorCodec::default();
        let first = query_messages_cursor_page(
            &conn,
            &active.path,
            &active.id,
            None,
            false,
            &criteria,
            Some("newest"),
            Some(2),
            None,
            11,
            &codec,
            None,
        )
        .unwrap();
        let cursor = first.next_cursor.as_deref().unwrap();
        let oversized_cursor = "x".repeat(300);

        let cases = [
            (
                "malformed",
                "not-a-cursor",
                active.id.as_str(),
                "newest",
                11,
            ),
            (
                "oversized",
                oversized_cursor.as_str(),
                active.id.as_str(),
                "newest",
                11,
            ),
            (
                "unsupported",
                "pqv-msg-v2.deadbeef",
                active.id.as_str(),
                "newest",
                11,
            ),
            ("workspace", cursor, "another-workspace", "newest", 11),
            ("sort", cursor, active.id.as_str(), "oldest", 11),
            ("generation", cursor, active.id.as_str(), "newest", 12),
        ];
        for (name, value, workspace_id, sort, generation) in cases {
            let error = expect_app_error(query_messages_cursor_page(
                &conn,
                &active.path,
                workspace_id,
                None,
                false,
                &criteria,
                Some(sort),
                Some(2),
                Some(value),
                generation,
                &codec,
                None,
            ));
            let expected_code = if name == "unsupported" {
                search_pagination::UNSUPPORTED_SEARCH_CURSOR_CODE
            } else {
                search_pagination::INVALID_SEARCH_CURSOR_CODE
            };
            assert_eq!(error.code, Some(expected_code), "{name}");
        }

        let changed_criteria =
            MessageSearchCriteria::from_inputs(Some("shared".to_string()), None).unwrap();
        let error = expect_app_error(query_messages_cursor_page(
            &conn,
            &active.path,
            &active.id,
            None,
            false,
            &changed_criteria,
            Some("newest"),
            Some(2),
            Some(cursor),
            11,
            &codec,
            None,
        ));
        assert_eq!(
            error.code,
            Some(search_pagination::INVALID_SEARCH_CURSOR_CODE)
        );

        let writer = Connection::open(active.path.join("index.sqlite")).unwrap();
        set_metadata_value(&writer, "last_reindex_at", "2099-01-01T00:00:00Z").unwrap();
        drop(writer);
        let error = expect_app_error(query_messages_cursor_page(
            &conn,
            &active.path,
            &active.id,
            None,
            false,
            &criteria,
            Some("newest"),
            Some(2),
            Some(cursor),
            11,
            &codec,
            None,
        ));
        assert_eq!(
            error.code,
            Some(search_pagination::STALE_SEARCH_CURSOR_CODE)
        );

        drop(conn);
        fs::remove_dir_all(active.path).unwrap();
    }

    #[test]
    fn cursor_cancellation_is_safe_and_multi_workspace_remains_offset_only() {
        let active =
            create_search_workspace_with_messages("message-cursor-cancel", "workspace-cancel", 4);
        let conn = open_workspace_db_for_read(&active.path).unwrap();
        let criteria = MessageSearchCriteria::from_inputs(None, None).unwrap();
        let codec = SearchCursorCodec::default();
        let registry = Arc::new(SearchCancellationRegistry::default());
        let operation = registry
            .begin_operation(
                "cursor-window",
                11,
                "message-page-1",
                SearchOperationCategory::MessagePage,
            )
            .unwrap();
        registry
            .cancel_operation("cursor-window", 11, "message-page-1")
            .unwrap();
        let error = expect_app_error(query_messages_cursor_page(
            &conn,
            &active.path,
            &active.id,
            None,
            false,
            &criteria,
            Some("newest"),
            Some(2),
            None,
            11,
            &codec,
            Some(&operation),
        ));
        assert_eq!(error.code, Some(SEARCH_CANCELLED_CODE));
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            4
        );

        assert!(ensure_multi_workspace_cursor_absent(None).is_ok());
        let unsupported = ensure_multi_workspace_cursor_absent(Some("opaque")).unwrap_err();
        assert_eq!(
            unsupported.code,
            Some(search_pagination::UNSUPPORTED_SEARCH_CURSOR_CODE)
        );
        let multi_page = query_multi_workspace_message_page(
            vec![active.clone()],
            &criteria,
            "newest",
            Some(2),
            Some(0),
            &registry
                .begin_operation(
                    "cursor-window",
                    12,
                    "message-page-2",
                    SearchOperationCategory::MessagePage,
                )
                .unwrap(),
        )
        .unwrap();
        assert_eq!(multi_page.pagination_mode, "offset");
        assert!(multi_page.next_cursor.is_none());

        drop(operation);
        drop(conn);
        fs::remove_dir_all(active.path).unwrap();
    }

    #[test]
    fn multi_workspace_page_and_count_are_independent_and_keep_identity() {
        let first = create_search_workspace_with_messages("multi-page-a", "workspace-a", 4);
        let second = create_search_workspace_with_messages("multi-page-b", "workspace-b", 3);
        let first_path = first.path.clone();
        let second_path = second.path.clone();
        let criteria = MessageSearchCriteria::from_inputs(None, None).unwrap();
        let registry = Arc::new(SearchCancellationRegistry::default());
        let page_operation = registry
            .begin_operation(
                "test-window",
                3,
                "message-page-1",
                SearchOperationCategory::MessagePage,
            )
            .unwrap();
        let count_operation = registry
            .begin_operation(
                "test-window",
                3,
                "message-count-1",
                SearchOperationCategory::MessageCount,
            )
            .unwrap();

        let page = query_multi_workspace_message_page(
            vec![first.clone(), second.clone()],
            &criteria,
            "newest",
            Some(4),
            Some(0),
            &page_operation,
        )
        .unwrap();
        assert_eq!(page.returned_count, 4);
        assert!(page.has_more);
        assert!(page.items.iter().all(|item| {
            matches!(
                item.workspace_id.as_deref(),
                Some("workspace-a" | "workspace-b")
            ) && item.pst_display_name.is_some()
        }));

        let load_more_operation = registry
            .begin_operation(
                "test-window",
                3,
                "message-load-more-1",
                SearchOperationCategory::MessagePage,
            )
            .unwrap();
        let next_page = query_multi_workspace_message_page(
            vec![first.clone(), second.clone()],
            &criteria,
            "newest",
            Some(4),
            Some(4),
            &load_more_operation,
        )
        .unwrap();
        assert_eq!(next_page.returned_count, 3);
        assert!(!next_page.has_more);
        let first_page_keys = page
            .items
            .iter()
            .map(|item| (item.workspace_id.clone(), item.id))
            .collect::<HashSet<_>>();
        assert!(next_page
            .items
            .iter()
            .all(|item| !first_page_keys.contains(&(item.workspace_id.clone(), item.id))));

        let counts =
            count_multi_workspace_messages(vec![first, second], &criteria, &count_operation)
                .unwrap();
        assert_eq!(counts.total_count, 7);
        assert_eq!(
            counts
                .per_workspace_counts
                .iter()
                .map(|count| count.count)
                .collect::<Vec<_>>(),
            vec![4, 3]
        );
        assert!(page_operation.check_cancelled().is_ok());

        drop(count_operation);
        drop(load_more_operation);
        drop(page_operation);
        fs::remove_dir_all(first_path).unwrap();
        fs::remove_dir_all(second_path).unwrap();
    }

    #[test]
    fn conversation_pages_arrive_independently_of_exact_counts() {
        let active =
            create_search_workspace_with_messages("conversation-page", "workspace-conversation", 6);
        let workspace_path = active.path.clone();
        let scope = ConversationWorkspaceScope {
            workspace_id: active.id.clone(),
            folder_id: None,
            include_subfolders: false,
        };
        let criteria = MessageSearchCriteria::from_inputs(None, None).unwrap();
        let registry = Arc::new(SearchCancellationRegistry::default());
        let page_operation = registry
            .begin_operation(
                "test-window",
                5,
                "conversation-page-1",
                SearchOperationCategory::ConversationPage,
            )
            .unwrap();
        let count_operation = registry
            .begin_operation(
                "test-window",
                5,
                "conversation-count-1",
                SearchOperationCategory::ConversationCount,
            )
            .unwrap();
        let page = query_conversation_page_for_scopes(
            vec![(scope.clone(), active.clone())],
            &criteria,
            "newest",
            Some(2),
            Some(0),
            &page_operation,
        )
        .unwrap();
        assert_eq!(page.returned_count, 2);
        assert_eq!(page.items.len(), 2);
        assert!(page.has_more);
        assert_eq!(page.indexed_workspace_count, 1);
        assert!(page.unindexed_workspaces.is_empty());

        let counts =
            count_conversations_for_scopes(vec![(scope, active)], &criteria, &count_operation)
                .unwrap();
        assert_eq!(counts.total_count, 3);
        assert_eq!(counts.matching_message_count, 6);

        drop(count_operation);
        drop(page_operation);
        fs::remove_dir_all(workspace_path).unwrap();
    }

    #[test]
    fn local_trash_folder_rollups_use_exact_path_boundaries() {
        let mut folders = [
            Folder {
                id: 1,
                parent_id: None,
                path: String::new(),
                name: "Root".to_string(),
                message_count: 0,
                direct_message_count: 0,
                active_message_count: 0,
                active_direct_message_count: 0,
                local_trash_message_count: 0,
                local_trash_direct_message_count: 0,
            },
            Folder {
                id: 2,
                parent_id: Some(1),
                path: "a".to_string(),
                name: "A".to_string(),
                message_count: 0,
                direct_message_count: 2,
                active_message_count: 0,
                active_direct_message_count: 1,
                local_trash_message_count: 0,
                local_trash_direct_message_count: 1,
            },
            Folder {
                id: 3,
                parent_id: Some(1),
                path: "a-b".to_string(),
                name: "A dash B".to_string(),
                message_count: 0,
                direct_message_count: 3,
                active_message_count: 0,
                active_direct_message_count: 3,
                local_trash_message_count: 0,
                local_trash_direct_message_count: 0,
            },
            Folder {
                id: 4,
                parent_id: Some(2),
                path: "a/child".to_string(),
                name: "Child".to_string(),
                message_count: 0,
                direct_message_count: 5,
                active_message_count: 0,
                active_direct_message_count: 4,
                local_trash_message_count: 0,
                local_trash_direct_message_count: 1,
            },
            Folder {
                id: 5,
                parent_id: Some(1),
                path: "a0".to_string(),
                name: "A zero".to_string(),
                message_count: 0,
                direct_message_count: 7,
                active_message_count: 0,
                active_direct_message_count: 7,
                local_trash_message_count: 0,
                local_trash_direct_message_count: 0,
            },
        ];

        roll_up_folder_local_trash_counts(&mut folders).unwrap();
        assert_eq!(
            (
                folders[0].message_count,
                folders[0].active_message_count,
                folders[0].local_trash_message_count,
            ),
            (17, 15, 2)
        );
        assert_eq!(
            (
                folders[1].message_count,
                folders[1].active_message_count,
                folders[1].local_trash_message_count,
            ),
            (7, 5, 2)
        );
        assert_eq!(folders[2].message_count, 3);
        assert_eq!(folders[3].message_count, 5);
        assert_eq!(folders[4].message_count, 7);
    }

    #[test]
    fn local_trash_visibility_covers_pages_counts_folders_relevance_and_conversations() {
        let mut active =
            create_search_workspace_with_messages("local-trash-integration", "workspace", 6);
        active.id = "a".repeat(64);
        active.fingerprint = "b".repeat(64);
        active.pst_path = active.path.join("synthetic-source.pst");
        fs::write(&active.pst_path, b"!BDNsynthetic local trash source").unwrap();
        let extracted = active.path.join("extracted/Inbox");
        fs::create_dir_all(&extracted).unwrap();
        let extracted_path = extracted.join("message-2.eml");
        fs::write(
            &extracted_path,
            b"From: sender@example.test\n\nSynthetic body\n",
        )
        .unwrap();

        let writer = Connection::open(active.path.join("index.sqlite")).unwrap();
        writer
            .execute(
                "INSERT INTO folders (id, parent_id, path, name)
                 VALUES (2, 1, 'Inbox/Child', 'Child')",
                [],
            )
            .unwrap();
        writer
            .execute("UPDATE messages SET folder_id = 2 WHERE id IN (3, 4)", [])
            .unwrap();
        drop(writer);

        let source_hash = sha256_file(&active.pst_path).unwrap();
        let source_metadata = fs::metadata(&active.pst_path).unwrap();
        let extracted_hash = sha256_file(&extracted_path).unwrap();
        let extracted_metadata = fs::metadata(&extracted_path).unwrap();
        let index_path = active.path.join("index.sqlite");
        let index_hash = sha256_file(&index_path).unwrap();
        let index_metadata = fs::metadata(&index_path).unwrap();
        let scope = local_trash_scope_for_active(&active).unwrap();

        let move_two_id = local_trash::new_operation_id().unwrap();
        let moved = local_trash::move_messages(
            &active.path,
            &scope,
            0,
            &move_two_id,
            &["Inbox/message-2.eml".to_string()],
        )
        .unwrap();
        assert_eq!((moved.changed_count, moved.revision), (1, 1));

        let criteria = MessageSearchCriteria::from_inputs(None, None).unwrap();
        let relevance =
            MessageSearchCriteria::from_inputs(Some("shared".to_string()), None).unwrap();
        let conn = open_workspace_db_for_read(&active.path).unwrap();
        assert_eq!(
            query_message_count_visible(
                &conn,
                &active,
                None,
                false,
                &criteria,
                MessageVisibility::Active,
                None,
            )
            .unwrap(),
            5
        );
        assert_eq!(
            query_message_count_visible(
                &conn,
                &active,
                None,
                false,
                &criteria,
                MessageVisibility::Trashed,
                None,
            )
            .unwrap(),
            1
        );
        assert_eq!(
            query_message_count_visible(
                &conn,
                &active,
                Some(2),
                true,
                &criteria,
                MessageVisibility::Active,
                None,
            )
            .unwrap(),
            2
        );

        let codec = SearchCursorCodec::default();
        let active_page = query_messages_cursor_page_visible(
            &conn,
            &active,
            None,
            false,
            &criteria,
            MessageVisibility::Active,
            Some("newest"),
            Some(2),
            None,
            41,
            &codec,
            None,
        )
        .unwrap();
        assert!(active_page.items.iter().all(|item| item.id != 2));
        let active_cursor = active_page.next_cursor.clone().unwrap();
        let cross_visibility = expect_app_error(query_messages_cursor_page_visible(
            &conn,
            &active,
            None,
            false,
            &criteria,
            MessageVisibility::Trashed,
            Some("newest"),
            Some(2),
            Some(&active_cursor),
            41,
            &codec,
            None,
        ));
        assert_eq!(
            cross_visibility.code,
            Some(search_pagination::INVALID_SEARCH_CURSOR_CODE)
        );
        let trash_relevance = query_messages_cursor_page_visible(
            &conn,
            &active,
            Some(2),
            true,
            &relevance,
            MessageVisibility::Trashed,
            Some("relevance"),
            Some(10),
            None,
            42,
            &codec,
            None,
        )
        .unwrap();
        assert_eq!(
            trash_relevance
                .items
                .iter()
                .map(|item| item.id)
                .collect::<Vec<_>>(),
            vec![2]
        );
        assert!(trash_relevance.items[0].search_match_context.is_some());

        let folders = query_folders_with_local_trash(&conn, &active).unwrap();
        let inbox = folders.iter().find(|folder| folder.id == 1).unwrap();
        assert_eq!(
            (
                inbox.message_count,
                inbox.direct_message_count,
                inbox.active_message_count,
                inbox.local_trash_message_count,
                inbox.active_direct_message_count,
                inbox.local_trash_direct_message_count,
            ),
            (6, 4, 5, 1, 3, 1)
        );
        let child = folders.iter().find(|folder| folder.id == 2).unwrap();
        assert_eq!(
            (
                child.message_count,
                child.active_message_count,
                child.local_trash_message_count,
            ),
            (2, 2, 0)
        );
        drop(conn);

        let registry = Arc::new(SearchCancellationRegistry::default());
        let page_operation = registry
            .begin_operation(
                "local-trash-test",
                1,
                "message-page",
                SearchOperationCategory::MessagePage,
            )
            .unwrap();
        let offset_page = query_multi_workspace_message_page_visible(
            vec![active.clone()],
            &criteria,
            "newest",
            MessageVisibility::Trashed,
            Some(10),
            Some(0),
            &page_operation,
        )
        .unwrap();
        assert_eq!(offset_page.items.len(), 1);
        assert_eq!(offset_page.items[0].id, 2);
        assert_eq!(offset_page.pagination_mode, "offset");

        let conversation_scope = ConversationWorkspaceScope {
            workspace_id: active.id.clone(),
            folder_id: None,
            include_subfolders: false,
        };
        let conversation_operation = registry
            .begin_operation(
                "local-trash-test",
                1,
                "conversation-page",
                SearchOperationCategory::ConversationPage,
            )
            .unwrap();
        let conversation_page = query_conversation_page_for_scopes(
            vec![(conversation_scope.clone(), active.clone())],
            &criteria,
            "newest",
            Some(10),
            Some(0),
            &conversation_operation,
        )
        .unwrap();
        assert_eq!(conversation_page.items.len(), 3);
        let first_conversation = conversation_page
            .items
            .iter()
            .find(|item| item.conversation_id == "conversation-1")
            .unwrap();
        assert_eq!(first_conversation.total_message_count, 1);
        assert_eq!(first_conversation.latest_message_id, 1);
        assert_eq!(first_conversation.conversation_root_id, Some(1));

        let count_operation = registry
            .begin_operation(
                "local-trash-test",
                1,
                "conversation-count",
                SearchOperationCategory::ConversationCount,
            )
            .unwrap();
        let conversation_count = count_conversations_for_scopes(
            vec![(conversation_scope.clone(), active.clone())],
            &criteria,
            &count_operation,
        )
        .unwrap();
        assert_eq!(
            (
                conversation_count.total_count,
                conversation_count.matching_message_count,
            ),
            (3, 5)
        );

        let expanded_conn = open_workspace_db_for_read(&active.path).unwrap();
        local_trash::materialize_for_query(&expanded_conn, &active.path, &scope).unwrap();
        let expanded_source = visible_message_query_source_after_materialization(
            &expanded_conn,
            &scope,
            MessageVisibility::Active,
            None,
            false,
            &criteria,
        )
        .unwrap();
        let expanded = query_entire_conversation_page_visible(
            &expanded_conn,
            &expanded_source,
            &scope,
            "conversation-1",
            10,
            0,
            None,
        )
        .unwrap();
        assert_eq!(
            expanded
                .iter()
                .map(|item| item.message.id)
                .collect::<Vec<_>>(),
            vec![1]
        );
        drop(expanded_conn);

        let move_one_id = local_trash::new_operation_id().unwrap();
        let moved_root = local_trash::move_messages(
            &active.path,
            &scope,
            1,
            &move_one_id,
            &["Inbox/message-1.eml".to_string()],
        )
        .unwrap();
        assert_eq!(moved_root.revision, 2);
        let stale_conn = open_workspace_db_for_read(&active.path).unwrap();
        let stale = expect_app_error(query_messages_cursor_page_visible(
            &stale_conn,
            &active,
            None,
            false,
            &criteria,
            MessageVisibility::Active,
            Some("newest"),
            Some(2),
            Some(&active_cursor),
            41,
            &codec,
            None,
        ));
        assert_eq!(
            stale.code,
            Some(search_pagination::STALE_SEARCH_CURSOR_CODE)
        );
        drop(stale_conn);

        let after_all_trashed_operation = registry
            .begin_operation(
                "local-trash-test",
                2,
                "conversation-count-2",
                SearchOperationCategory::ConversationCount,
            )
            .unwrap();
        let after_all_trashed = count_conversations_for_scopes(
            vec![(conversation_scope, active.clone())],
            &criteria,
            &after_all_trashed_operation,
        )
        .unwrap();
        assert_eq!(
            (
                after_all_trashed.total_count,
                after_all_trashed.matching_message_count,
            ),
            (2, 4)
        );

        let undone = local_trash::undo_move(&active.path, &scope, 2, &move_one_id).unwrap();
        assert_eq!((undone.changed_count, undone.revision), (1, 3));
        let restore_id = local_trash::new_operation_id().unwrap();
        let restored = local_trash::restore_messages(
            &active.path,
            &scope,
            3,
            &restore_id,
            &["Inbox/message-2.eml".to_string()],
        )
        .unwrap();
        assert_eq!((restored.changed_count, restored.revision), (1, 4));
        let repeat_restore_id = local_trash::new_operation_id().unwrap();
        let repeated = local_trash::restore_messages(
            &active.path,
            &scope,
            4,
            &repeat_restore_id,
            &["Inbox/message-2.eml".to_string()],
        )
        .unwrap();
        assert_eq!(
            (repeated.changed_count, repeated.already_active_count),
            (0, 1)
        );

        assert_eq!(sha256_file(&active.pst_path).unwrap(), source_hash);
        let source_after = fs::metadata(&active.pst_path).unwrap();
        assert_eq!(source_after.len(), source_metadata.len());
        assert_eq!(
            source_after.modified().unwrap(),
            source_metadata.modified().unwrap()
        );
        assert_eq!(sha256_file(&extracted_path).unwrap(), extracted_hash);
        let extracted_after = fs::metadata(&extracted_path).unwrap();
        assert_eq!(extracted_after.len(), extracted_metadata.len());
        assert_eq!(
            extracted_after.modified().unwrap(),
            extracted_metadata.modified().unwrap()
        );
        assert_eq!(sha256_file(&index_path).unwrap(), index_hash);
        let index_after = fs::metadata(&index_path).unwrap();
        assert_eq!(index_after.len(), index_metadata.len());
        assert_eq!(
            index_after.modified().unwrap(),
            index_metadata.modified().unwrap()
        );
        let canonical_conn = open_workspace_db_for_read(&active.path).unwrap();
        assert_eq!(
            canonical_conn
                .query_row("SELECT COUNT(*) FROM messages", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            6
        );
        assert_eq!(
            read_schema_version(&canonical_conn).unwrap(),
            SQLITE_SCHEMA_VERSION_CURRENT
        );
        drop(canonical_conn);

        drop(after_all_trashed_operation);
        drop(count_operation);
        drop(conversation_operation);
        drop(page_operation);
        fs::remove_dir_all(active.path).unwrap();
    }

    #[test]
    fn local_trash_multi_workspace_offset_and_counts_remain_scoped() {
        let mut first = create_search_workspace_with_messages("local-trash-multi-a", "first", 3);
        first.id = "c".repeat(64);
        first.fingerprint = "d".repeat(64);
        let mut second = create_search_workspace_with_messages("local-trash-multi-b", "second", 3);
        second.id = "e".repeat(64);
        second.fingerprint = "f".repeat(64);
        let first_scope = local_trash_scope_for_active(&first).unwrap();
        let second_scope = local_trash_scope_for_active(&second).unwrap();
        local_trash::move_messages(
            &first.path,
            &first_scope,
            0,
            &local_trash::new_operation_id().unwrap(),
            &["Inbox/message-1.eml".to_string()],
        )
        .unwrap();
        local_trash::move_messages(
            &second.path,
            &second_scope,
            0,
            &local_trash::new_operation_id().unwrap(),
            &["Inbox/message-2.eml".to_string()],
        )
        .unwrap();

        let criteria = MessageSearchCriteria::from_inputs(None, None).unwrap();
        let registry = Arc::new(SearchCancellationRegistry::default());
        let first_page_operation = registry
            .begin_operation(
                "local-trash-multi",
                1,
                "page-1",
                SearchOperationCategory::MessagePage,
            )
            .unwrap();
        let first_page = query_multi_workspace_message_page_visible(
            vec![first.clone(), second.clone()],
            &criteria,
            "newest",
            MessageVisibility::Trashed,
            Some(1),
            Some(0),
            &first_page_operation,
        )
        .unwrap();
        assert_eq!(first_page.items.len(), 1);
        assert!(first_page.has_more);
        let second_page_operation = registry
            .begin_operation(
                "local-trash-multi",
                1,
                "page-2",
                SearchOperationCategory::MessagePage,
            )
            .unwrap();
        let second_page = query_multi_workspace_message_page_visible(
            vec![first.clone(), second.clone()],
            &criteria,
            "newest",
            MessageVisibility::Trashed,
            Some(1),
            Some(1),
            &second_page_operation,
        )
        .unwrap();
        assert_eq!(second_page.items.len(), 1);
        assert!(!second_page.has_more);
        assert_ne!(
            first_page.items[0].workspace_id,
            second_page.items[0].workspace_id
        );

        let count_operation = registry
            .begin_operation(
                "local-trash-multi",
                1,
                "count",
                SearchOperationCategory::MessageCount,
            )
            .unwrap();
        let counts = count_multi_workspace_messages_visible(
            vec![first.clone(), second.clone()],
            &criteria,
            MessageVisibility::Trashed,
            &count_operation,
        )
        .unwrap();
        assert_eq!(counts.total_count, 2);
        assert_eq!(
            counts
                .per_workspace_counts
                .iter()
                .map(|count| count.count)
                .collect::<Vec<_>>(),
            vec![1, 1]
        );

        drop(count_operation);
        drop(second_page_operation);
        drop(first_page_operation);
        fs::remove_dir_all(first.path).unwrap();
        fs::remove_dir_all(second.path).unwrap();
    }

    fn assert_unhealthy_local_trash_queries_fail_open(
        active: &ActiveWorkspace,
        expected_error_code: &'static str,
    ) {
        let criteria = MessageSearchCriteria::from_inputs(None, None).unwrap();
        let conn = open_workspace_db_for_read(&active.path).unwrap();

        assert_eq!(
            query_message_count_visible(
                &conn,
                active,
                None,
                false,
                &criteria,
                MessageVisibility::Active,
                None,
            )
            .unwrap(),
            3
        );
        assert_eq!(
            query_message_count_visible(
                &conn,
                active,
                None,
                false,
                &criteria,
                MessageVisibility::Trashed,
                None,
            )
            .unwrap(),
            0
        );

        let codec = SearchCursorCodec::default();
        let active_page = query_messages_cursor_page_visible(
            &conn,
            active,
            None,
            false,
            &criteria,
            MessageVisibility::Active,
            Some("newest"),
            Some(10),
            None,
            71,
            &codec,
            None,
        )
        .unwrap();
        assert_eq!(active_page.items.len(), 3);
        let trash_page = query_messages_cursor_page_visible(
            &conn,
            active,
            None,
            false,
            &criteria,
            MessageVisibility::Trashed,
            Some("newest"),
            Some(10),
            None,
            71,
            &codec,
            None,
        )
        .unwrap();
        assert!(trash_page.items.is_empty());

        let folders = query_folders_with_local_trash(&conn, active).unwrap();
        let inbox = folders.iter().find(|folder| folder.id == 1).unwrap();
        assert_eq!(
            (
                inbox.message_count,
                inbox.active_message_count,
                inbox.local_trash_message_count,
            ),
            (3, 3, 0)
        );
        let (in_local_trash, available, error_code) =
            local_trash_message_state(&conn, active, "Inbox/message-1.eml").unwrap();
        assert!(!in_local_trash);
        assert!(!available);
        assert_eq!(error_code.as_deref(), Some(expected_error_code));
        drop(conn);

        let registry = Arc::new(SearchCancellationRegistry::default());
        let operation = registry
            .begin_operation(
                "local-trash-fail-open",
                1,
                "conversation-page",
                SearchOperationCategory::ConversationPage,
            )
            .unwrap();
        let page = query_conversation_page_for_scopes(
            vec![(
                ConversationWorkspaceScope {
                    workspace_id: active.id.clone(),
                    folder_id: None,
                    include_subfolders: false,
                },
                active.clone(),
            )],
            &criteria,
            "newest",
            Some(10),
            Some(0),
            &operation,
        )
        .unwrap();
        assert_eq!(page.items.len(), 2);
        assert_eq!(
            page.items
                .iter()
                .map(|item| item.total_message_count)
                .sum::<i64>(),
            3
        );
    }

    #[test]
    fn local_trash_duplicate_canonical_identity_fails_open_without_hiding_either_message() {
        let mut duplicate =
            create_search_workspace_with_messages("local-trash-duplicate-identity", "duplicate", 3);
        duplicate.id = "d".repeat(64);
        duplicate.fingerprint = "e".repeat(64);
        let writer = Connection::open(duplicate.path.join("index.sqlite")).unwrap();
        writer
            .execute_batch(
                "ALTER TABLE messages RENAME TO messages_with_unique_identity;
                 CREATE TABLE messages AS SELECT * FROM messages_with_unique_identity;
                 INSERT INTO messages SELECT * FROM messages WHERE id = 1;
                 UPDATE messages SET id = 99 WHERE rowid = (SELECT MAX(rowid) FROM messages);",
            )
            .unwrap();
        assert_eq!(
            writer
                .query_row(
                    "SELECT COUNT(*) FROM messages WHERE eml_path = 'Inbox/message-1.eml'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            2
        );
        drop(writer);

        let conn = open_workspace_db_for_read(&duplicate.path).unwrap();
        assert!(!has_unique_message_source_identity(&conn).unwrap());
        let scope = local_trash_scope_for_active(&duplicate).unwrap();
        let state = local_trash::materialize_for_query(&conn, &duplicate.path, &scope).unwrap();
        assert!(!state.available);
        assert!(!state.can_reset);
        assert!(!state.sidecar_present);
        assert_eq!(
            state.last_error_code,
            Some(local_trash::ERROR_CANONICAL_IDENTITY_AMBIGUOUS)
        );

        let criteria = MessageSearchCriteria::from_inputs(None, None).unwrap();
        assert_eq!(
            query_message_count_visible(
                &conn,
                &duplicate,
                None,
                false,
                &criteria,
                MessageVisibility::Active,
                None,
            )
            .unwrap(),
            4
        );
        assert_eq!(
            query_message_count_visible(
                &conn,
                &duplicate,
                None,
                false,
                &criteria,
                MessageVisibility::Trashed,
                None,
            )
            .unwrap(),
            0
        );
        let mutation_error = canonical_message_identity(&conn, 1).unwrap_err();
        assert_eq!(
            mutation_error.code,
            Some(local_trash::ERROR_CANONICAL_IDENTITY_AMBIGUOUS)
        );
        drop(conn);
        fs::remove_dir_all(duplicate.path).unwrap();
    }

    #[test]
    fn local_trash_duplicate_numeric_message_id_cannot_target_an_arbitrary_identity() {
        let mut duplicate_id =
            create_search_workspace_with_messages("local-trash-duplicate-id", "duplicate-id", 3);
        duplicate_id.id = "7".repeat(64);
        duplicate_id.fingerprint = "8".repeat(64);
        let writer = Connection::open(duplicate_id.path.join("index.sqlite")).unwrap();
        writer
            .execute_batch(
                "ALTER TABLE messages RENAME TO messages_with_unique_id;
                 CREATE TABLE messages AS SELECT * FROM messages_with_unique_id;
                 CREATE UNIQUE INDEX messages_eml_path_unique
                     ON messages(eml_path COLLATE BINARY);
                 UPDATE messages SET id = 1 WHERE id = 2;",
            )
            .unwrap();
        assert_eq!(
            writer
                .query_row("SELECT COUNT(*) FROM messages WHERE id = 1", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            2
        );
        drop(writer);

        let conn = open_workspace_db_for_read(&duplicate_id.path).unwrap();
        assert!(has_unique_message_source_identity(&conn).unwrap());
        let mutation_error = canonical_message_identity(&conn, 1).unwrap_err();
        assert_eq!(
            mutation_error.code,
            Some(local_trash::ERROR_CANONICAL_IDENTITY_AMBIGUOUS)
        );

        let scope = local_trash_scope_for_active(&duplicate_id).unwrap();
        let state = local_trash::materialize_for_query(&conn, &duplicate_id.path, &scope).unwrap();
        assert!(state.available);
        assert!(!state.sidecar_present);
        let criteria = MessageSearchCriteria::from_inputs(None, None).unwrap();
        assert_eq!(
            query_message_count_visible(
                &conn,
                &duplicate_id,
                None,
                false,
                &criteria,
                MessageVisibility::Active,
                None,
            )
            .unwrap(),
            3
        );
        assert_eq!(
            query_message_count_visible(
                &conn,
                &duplicate_id,
                None,
                false,
                &criteria,
                MessageVisibility::Trashed,
                None,
            )
            .unwrap(),
            0
        );
        drop(conn);
        fs::remove_dir_all(duplicate_id.path).unwrap();
    }

    #[test]
    fn local_trash_corrupt_future_wrong_fingerprint_and_unsafe_states_fail_open_in_queries() {
        let mut corrupt =
            create_search_workspace_with_messages("local-trash-corrupt-query", "corrupt", 3);
        corrupt.id = "1".repeat(64);
        corrupt.fingerprint = "2".repeat(64);
        let corrupt_scope = local_trash_scope_for_active(&corrupt).unwrap();
        local_trash::move_messages(
            &corrupt.path,
            &corrupt_scope,
            0,
            &local_trash::new_operation_id().unwrap(),
            &["Inbox/message-1.eml".to_string()],
        )
        .unwrap();
        let corrupt_path = local_trash::test_active_sidecar_path(&corrupt.path).unwrap();
        let mut corrupt_file = OpenOptions::new()
            .write(true)
            .truncate(true)
            .open(&corrupt_path)
            .unwrap();
        corrupt_file.write_all(b"not a sqlite database").unwrap();
        corrupt_file.sync_all().unwrap();
        drop(corrupt_file);
        local_trash::test_reseal_active_sidecar_envelope(&corrupt.path).unwrap();
        assert_unhealthy_local_trash_queries_fail_open(&corrupt, local_trash::ERROR_INVALID);

        let mut future =
            create_search_workspace_with_messages("local-trash-future-query", "future", 3);
        future.id = "3".repeat(64);
        future.fingerprint = "4".repeat(64);
        let future_scope = local_trash_scope_for_active(&future).unwrap();
        local_trash::move_messages(
            &future.path,
            &future_scope,
            0,
            &local_trash::new_operation_id().unwrap(),
            &["Inbox/message-1.eml".to_string()],
        )
        .unwrap();
        let future_conn =
            Connection::open(local_trash::test_active_sidecar_path(&future.path).unwrap()).unwrap();
        future_conn.pragma_update(None, "user_version", 2).unwrap();
        drop(future_conn);
        local_trash::test_reseal_active_sidecar(&future.path).unwrap();
        assert_unhealthy_local_trash_queries_fail_open(&future, local_trash::ERROR_FUTURE_SCHEMA);

        let mut mismatched =
            create_search_workspace_with_messages("local-trash-mismatch-query", "mismatch", 3);
        mismatched.id = "5".repeat(64);
        mismatched.fingerprint = "6".repeat(64);
        let original_scope = local_trash_scope_for_active(&mismatched).unwrap();
        local_trash::move_messages(
            &mismatched.path,
            &original_scope,
            0,
            &local_trash::new_operation_id().unwrap(),
            &["Inbox/message-1.eml".to_string()],
        )
        .unwrap();
        mismatched.fingerprint = "7".repeat(64);
        assert_unhealthy_local_trash_queries_fail_open(
            &mismatched,
            local_trash::ERROR_FINGERPRINT_MISMATCH,
        );

        let mut unsafe_file =
            create_search_workspace_with_messages("local-trash-unsafe-query", "unsafe", 3);
        unsafe_file.id = "8".repeat(64);
        unsafe_file.fingerprint = "9".repeat(64);
        let unsafe_scope = local_trash_scope_for_active(&unsafe_file).unwrap();
        local_trash::move_messages(
            &unsafe_file.path,
            &unsafe_scope,
            0,
            &local_trash::new_operation_id().unwrap(),
            &["Inbox/message-1.eml".to_string()],
        )
        .unwrap();
        fs::set_permissions(
            local_trash::test_active_sidecar_path(&unsafe_file.path).unwrap(),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        assert_unhealthy_local_trash_queries_fail_open(
            &unsafe_file,
            local_trash::ERROR_UNSAFE_FILE,
        );

        fs::remove_dir_all(corrupt.path).unwrap();
        fs::remove_dir_all(future.path).unwrap();
        fs::remove_dir_all(mismatched.path).unwrap();
        fs::remove_dir_all(unsafe_file.path).unwrap();
    }

    #[test]
    fn local_trash_keeps_blank_and_identical_messages_independent_and_reports_orphans() {
        let mut active =
            create_search_workspace_with_messages("local-trash-identities", "identities", 3);
        active.id = "a".repeat(64);
        active.fingerprint = "b".repeat(64);
        fs::write(&active.pst_path, b"!BDNfictional local trash source").unwrap();

        let extracted = active.path.join("extracted/Inbox");
        let attachment_root = extracted.join("attachments");
        fs::create_dir_all(&attachment_root).unwrap();
        let message_one = extracted.join("message-1.eml");
        let message_two = extracted.join("message-2.eml");
        let message_three = extracted.join("message-3.eml");
        let identical_eml = b"From: same@example.test\r\n\r\nIdentical fictional body\r\n";
        fs::write(&message_one, identical_eml).unwrap();
        fs::write(&message_two, identical_eml).unwrap();
        fs::write(
            &message_three,
            b"From: blank@example.test\r\nSubject: \r\n\r\nBlank subject body\r\n",
        )
        .unwrap();
        let attachment_one = attachment_root.join("message-1-payload.bin");
        let attachment_two = attachment_root.join("message-2-payload.bin");
        fs::write(&attachment_one, b"fictional attachment payload").unwrap();
        fs::write(&attachment_two, b"fictional attachment payload").unwrap();

        let writer = Connection::open(active.path.join("index.sqlite")).unwrap();
        writer
            .execute_batch(
                "UPDATE messages
                    SET subject = CASE WHEN id = 3 THEN '' ELSE NULL END,
                        sender = CASE WHEN id IN (1, 2) THEN 'same@example.test' ELSE sender END,
                        recipients = CASE WHEN id IN (1, 2) THEN 'same-recipient@example.test' ELSE recipients END,
                        date = CASE WHEN id IN (1, 2) THEN '2026-01-02T03:04:05+00:00' ELSE date END,
                        body = CASE WHEN id IN (1, 2) THEN 'Identical fictional body' ELSE body END,
                        snippet = CASE WHEN id IN (1, 2) THEN 'Identical fictional body' ELSE snippet END,
                        attachment_names = CASE WHEN id IN (1, 2) THEN 'payload.bin' ELSE attachment_names END,
                        has_attachments = CASE WHEN id IN (1, 2) THEN 1 ELSE has_attachments END
                  WHERE id IN (1, 2, 3);
                 INSERT INTO attachments (
                     id, message_id, filename, sanitized_filename, content_type,
                     size_bytes, attachment_index, content_disposition, mime_part_path
                 ) VALUES
                     (10, 1, 'payload.bin', 'payload.bin', 'application/octet-stream', 28, 0, 'attachment', '2'),
                     (11, 2, 'payload.bin', 'payload.bin', 'application/octet-stream', 28, 0, 'attachment', '2');
                 INSERT INTO messages_fts(messages_fts) VALUES('rebuild');",
            )
            .unwrap();
        drop(writer);

        let index_path = active.path.join("index.sqlite");
        let index_hash = sha256_file(&index_path).unwrap();
        let index_modified = fs::metadata(&index_path).unwrap().modified().unwrap();
        let source_hash = sha256_file(&active.pst_path).unwrap();
        let preserved_files = [
            &message_one,
            &message_two,
            &message_three,
            &attachment_one,
            &attachment_two,
        ]
        .into_iter()
        .map(|path| {
            (
                path.to_path_buf(),
                sha256_file(path).unwrap(),
                fs::metadata(path).unwrap().modified().unwrap(),
            )
        })
        .collect::<Vec<_>>();
        let canonical = open_workspace_db_for_read(&active.path).unwrap();
        let message_snapshot = canonical
            .prepare(
                "SELECT id, eml_path, subject, sender, recipients, date, body, snippet,
                        attachment_names, has_attachments
                   FROM messages
                  ORDER BY id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let attachment_snapshot = canonical
            .prepare(
                "SELECT id, message_id, filename, sanitized_filename, content_type,
                        size_bytes, attachment_index, content_disposition, mime_part_path
                   FROM attachments
                  ORDER BY id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        drop(canonical);

        assert_eq!(message_snapshot[0].2, None);
        assert_eq!(message_snapshot[1].2, None);
        assert_eq!(message_snapshot[2].2.as_deref(), Some(""));
        assert_eq!(message_snapshot[0].3, message_snapshot[1].3);
        assert_eq!(message_snapshot[0].4, message_snapshot[1].4);
        assert_eq!(message_snapshot[0].5, message_snapshot[1].5);
        assert_eq!(message_snapshot[0].6, message_snapshot[1].6);
        assert_eq!(message_snapshot[0].7, message_snapshot[1].7);

        let scope = local_trash_scope_for_active(&active).unwrap();
        let first_move = local_trash::move_messages(
            &active.path,
            &scope,
            0,
            &local_trash::new_operation_id().unwrap(),
            &[
                "Inbox/message-1.eml".to_string(),
                "Archived/orphaned-message.eml".to_string(),
            ],
        )
        .unwrap();
        assert_eq!((first_move.changed_count, first_move.revision), (2, 1));

        let criteria = MessageSearchCriteria::from_inputs(None, None).unwrap();
        let conn = open_workspace_db_for_read(&active.path).unwrap();
        let local_state = local_trash::materialize_for_query(&conn, &active.path, &scope).unwrap();
        let trashed_count = query_message_count_visible(
            &conn,
            &active,
            None,
            false,
            &criteria,
            MessageVisibility::Trashed,
            None,
        )
        .unwrap();
        assert_eq!((local_state.stored_count, trashed_count), (2, 1));
        assert_eq!(
            i64::try_from(local_state.stored_count)
                .unwrap()
                .saturating_sub(trashed_count),
            1,
            "one valid but no-longer-canonical identity must be reported as orphaned state"
        );
        let trashed_page = query_messages_cursor_page_visible(
            &conn,
            &active,
            None,
            false,
            &criteria,
            MessageVisibility::Trashed,
            Some("newest"),
            Some(10),
            None,
            81,
            &SearchCursorCodec::default(),
            None,
        )
        .unwrap();
        assert_eq!(
            trashed_page
                .items
                .iter()
                .map(|item| (item.id, item.subject.as_str()))
                .collect::<Vec<_>>(),
            vec![(1, "")]
        );
        assert!(
            local_trash_message_state(&conn, &active, "Inbox/message-1.eml")
                .unwrap()
                .0
        );
        assert!(
            !local_trash_message_state(&conn, &active, "Inbox/message-2.eml")
                .unwrap()
                .0
        );
        drop(conn);

        let second_move = local_trash::move_messages(
            &active.path,
            &scope,
            1,
            &local_trash::new_operation_id().unwrap(),
            &["Inbox/message-2.eml".to_string()],
        )
        .unwrap();
        assert_eq!((second_move.changed_count, second_move.revision), (1, 2));

        let registry = Arc::new(SearchCancellationRegistry::default());
        let conversation_operation = registry
            .begin_operation(
                "local-trash-identities",
                1,
                "conversation-page",
                SearchOperationCategory::ConversationPage,
            )
            .unwrap();
        let conversation_scope = ConversationWorkspaceScope {
            workspace_id: active.id.clone(),
            folder_id: None,
            include_subfolders: false,
        };
        let page = query_conversation_page_for_scopes(
            vec![(conversation_scope.clone(), active.clone())],
            &criteria,
            "newest",
            Some(10),
            Some(0),
            &conversation_operation,
        )
        .unwrap();
        assert_eq!(page.items.len(), 1);
        assert!(page
            .items
            .iter()
            .all(|item| item.conversation_id != "conversation-1"));
        let count_operation = registry
            .begin_operation(
                "local-trash-identities",
                1,
                "conversation-count",
                SearchOperationCategory::ConversationCount,
            )
            .unwrap();
        let counts = count_conversations_for_scopes(
            vec![(conversation_scope.clone(), active.clone())],
            &criteria,
            &count_operation,
        )
        .unwrap();
        assert_eq!((counts.total_count, counts.matching_message_count), (1, 1));

        let restored = local_trash::restore_messages(
            &active.path,
            &scope,
            2,
            &local_trash::new_operation_id().unwrap(),
            &["Inbox/message-1.eml".to_string()],
        )
        .unwrap();
        assert_eq!((restored.changed_count, restored.revision), (1, 3));
        let reopened_operation = registry
            .begin_operation(
                "local-trash-identities",
                2,
                "conversation-page-restored",
                SearchOperationCategory::ConversationPage,
            )
            .unwrap();
        let reopened = query_conversation_page_for_scopes(
            vec![(conversation_scope, active.clone())],
            &criteria,
            "newest",
            Some(10),
            Some(0),
            &reopened_operation,
        )
        .unwrap();
        assert!(reopened
            .items
            .iter()
            .any(|item| item.conversation_id == "conversation-1"));

        let final_conn = open_workspace_db_for_read(&active.path).unwrap();
        assert!(
            !local_trash_message_state(&final_conn, &active, "Inbox/message-1.eml")
                .unwrap()
                .0
        );
        assert!(
            local_trash_message_state(&final_conn, &active, "Inbox/message-2.eml")
                .unwrap()
                .0
        );
        let final_state =
            local_trash::materialize_for_query(&final_conn, &active.path, &scope).unwrap();
        assert_eq!(final_state.stored_count, 2);
        let final_message_snapshot = final_conn
            .prepare(
                "SELECT id, eml_path, subject, sender, recipients, date, body, snippet,
                        attachment_names, has_attachments
                   FROM messages
                  ORDER BY id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, Option<String>>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, i64>(9)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let final_attachment_snapshot = final_conn
            .prepare(
                "SELECT id, message_id, filename, sanitized_filename, content_type,
                        size_bytes, attachment_index, content_disposition, mime_part_path
                   FROM attachments
                  ORDER BY id",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, i64>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, Option<String>>(8)?,
                ))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        drop(final_conn);
        assert_eq!(final_message_snapshot, message_snapshot);
        assert_eq!(final_attachment_snapshot, attachment_snapshot);
        assert_eq!(sha256_file(&index_path).unwrap(), index_hash);
        assert_eq!(
            fs::metadata(&index_path).unwrap().modified().unwrap(),
            index_modified
        );
        assert_eq!(sha256_file(&active.pst_path).unwrap(), source_hash);
        for (path, hash, modified) in preserved_files {
            assert_eq!(sha256_file(&path).unwrap(), hash);
            assert_eq!(fs::metadata(path).unwrap().modified().unwrap(), modified);
        }

        drop(reopened_operation);
        drop(count_operation);
        drop(conversation_operation);
        fs::remove_dir_all(active.path).unwrap();
    }

    #[test]
    fn local_trash_state_survives_synthetic_canonical_index_replacement_by_stable_identity() {
        let mut active =
            create_search_workspace_with_messages("local-trash-index-replacement", "repair", 3);
        active.id = "c".repeat(64);
        active.fingerprint = "d".repeat(64);
        let scope = local_trash_scope_for_active(&active).unwrap();
        let moved = local_trash::move_messages(
            &active.path,
            &scope,
            0,
            &local_trash::new_operation_id().unwrap(),
            &["Inbox/message-1.eml".to_string()],
        )
        .unwrap();
        assert_eq!((moved.changed_count, moved.revision), (1, 1));

        let index_path = active.path.join("index.sqlite");
        let replacement_path = active.path.join("index.reindex.tmp.sqlite");
        fs::copy(&index_path, &replacement_path).unwrap();
        let replacement = open_sqlite_write_connection(&replacement_path).unwrap();
        set_metadata_value(&replacement, "last_reindex_at", "2026-08-25T00:00:00Z").unwrap();
        checked_truncate_checkpoint(&replacement).unwrap();
        drop(replacement);
        swap_index_files(&replacement_path, &index_path).unwrap();

        let inspected = local_trash::inspect(&active.path, &scope);
        assert_eq!(inspected.health, local_trash::LocalTrashHealth::Healthy);
        assert_eq!((inspected.revision, inspected.stored_count), (1, 1));
        let conn = open_workspace_db_for_read(&active.path).unwrap();
        let criteria = MessageSearchCriteria::from_inputs(None, None).unwrap();
        assert_eq!(
            query_message_count_visible(
                &conn,
                &active,
                None,
                false,
                &criteria,
                MessageVisibility::Active,
                None,
            )
            .unwrap(),
            2
        );
        assert_eq!(
            query_message_count_visible(
                &conn,
                &active,
                None,
                false,
                &criteria,
                MessageVisibility::Trashed,
                None,
            )
            .unwrap(),
            1
        );
        assert!(
            local_trash_message_state(&conn, &active, "Inbox/message-1.eml")
                .unwrap()
                .0
        );
        assert_eq!(
            read_stable_message_ids(&index_path)
                .unwrap()
                .get("Inbox/message-1.eml"),
            Some(&1)
        );
        drop(conn);
        fs::remove_dir_all(active.path).unwrap();
    }

    #[test]
    fn valid_schema_v3_search_uses_read_only_connection_without_changing_version() {
        let (workspace, database_path) = create_current_search_workspace("read-search-current");
        let conn = open_workspace_db_for_read(&workspace).expect("current workspace should open");
        assert_eq!(
            read_schema_version(&conn).unwrap(),
            SQLITE_SCHEMA_VERSION_CURRENT
        );
        assert_eq!(
            conn.query_row("PRAGMA query_only", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            1
        );

        let criteria = MessageSearchCriteria::from_inputs(Some("searchable".into()), None).unwrap();
        let result = query_messages_page(MessagePageQuery {
            conn: &conn,
            folder_id: None,
            include_subfolders: false,
            criteria: &criteria,
            sort_order: None,
            limit: Some(10),
            offset: Some(0),
            operation: None,
        })
        .expect("read search should succeed");
        assert_eq!(result.items.len(), 1);
        assert_eq!(result.returned_count, 1);
        assert!(!result.has_more);
        assert_eq!(
            query_message_count(&conn, None, false, &criteria, None).unwrap(),
            1
        );
        let context = result.items[0]
            .search_match_context
            .as_ref()
            .expect("FTS search should include additive context");
        assert_eq!(
            context.matched_fields,
            vec![
                search::SearchMatchedField::Subject,
                search::SearchMatchedField::Body,
            ]
        );
        assert!(context.snippet_text.contains("Searchable"));
        assert!(context
            .highlight_ranges
            .iter()
            .all(|range| range.start < range.end));
        let relevance_result = query_messages_page(MessagePageQuery {
            conn: &conn,
            folder_id: None,
            include_subfolders: false,
            criteria: &criteria,
            sort_order: Some("relevance"),
            limit: Some(10),
            offset: Some(0),
            operation: None,
        })
        .expect("single-workspace FTS search should support relevance");
        assert_eq!(relevance_result.items.len(), 1);
        assert!(relevance_result.items[0].search_match_context.is_some());
        assert!(
            conn.execute_batch("PRAGMA user_version = 2").is_err(),
            "read connection must reject schema writes"
        );

        let writer = Connection::open(&database_path).expect("writer should open beside reader");
        writer
            .execute_batch("BEGIN IMMEDIATE; ROLLBACK;")
            .expect("read query connection must not hold a migration transaction");
        assert_eq!(
            read_schema_version(&writer).unwrap(),
            SQLITE_SCHEMA_VERSION_CURRENT
        );
        drop(writer);
        drop(conn);
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn cancelled_count_does_not_cancel_a_valid_page_in_the_same_generation() {
        let registry = Arc::new(SearchCancellationRegistry::default());
        let page = registry
            .begin_operation(
                "test-window",
                1,
                "message-page-1",
                SearchOperationCategory::MessagePage,
            )
            .unwrap();
        let count = registry
            .begin_operation(
                "test-window",
                1,
                "message-count-1",
                SearchOperationCategory::MessageCount,
            )
            .unwrap();
        registry
            .cancel_operation("test-window", 1, "message-count-1")
            .unwrap();
        assert!(count.check_cancelled().is_err());
        assert!(page.check_cancelled().is_ok());
    }

    #[test]
    fn cancellation_between_workspace_iterations_prevents_later_work() {
        let registry = Arc::new(SearchCancellationRegistry::default());
        let operation = registry
            .begin_operation(
                "test-window",
                7,
                "message-count-1",
                SearchOperationCategory::MessageCount,
            )
            .unwrap();
        let mut visited = Vec::new();
        let result: AppResult<()> = (|| {
            for workspace in 0..3 {
                operation.check_cancelled()?;
                visited.push(workspace);
                if workspace == 0 {
                    registry.cancel_generation("test-window", 7)?;
                }
            }
            Ok(())
        })();
        let error = result.expect_err("later synthetic workspaces must be skipped");
        assert_eq!(error.code, Some(SEARCH_CANCELLED_CODE));
        assert_eq!(visited, vec![0]);
    }

    #[test]
    fn read_connection_rejects_old_schema_without_starting_migration() {
        let (workspace, database_path) = create_current_search_workspace("read-search-old");
        let writer = Connection::open(&database_path).unwrap();
        writer.execute_batch("PRAGMA user_version = 2").unwrap();
        drop(writer);

        let error = open_workspace_db_for_read(&workspace)
            .expect_err("old schema must be upgraded through workspace activation");
        assert!(error.to_string().contains("version 2 requires upgrade"));
        assert_ne!(error.code, Some(SEARCH_CANCELLED_CODE));

        let writer = Connection::open(&database_path).unwrap();
        assert_eq!(read_schema_version(&writer).unwrap(), 2);
        assert!(column_exists(&writer, "messages", "conversation_id").unwrap());
        drop(writer);
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn read_connection_rejects_future_schema_without_modification() {
        let (workspace, database_path) = create_current_search_workspace("read-search-future");
        let future_version = SQLITE_SCHEMA_VERSION_CURRENT + 1;
        let writer = Connection::open(&database_path).unwrap();
        writer
            .execute_batch(&format!("PRAGMA user_version = {future_version}"))
            .unwrap();
        drop(writer);

        let error = open_workspace_db_for_read(&workspace)
            .expect_err("future schema must not be downgraded");
        assert!(error.to_string().contains("newer than this version"));
        assert_ne!(error.code, Some(SEARCH_CANCELLED_CODE));

        let writer = Connection::open(&database_path).unwrap();
        assert_eq!(read_schema_version(&writer).unwrap(), future_version);
        drop(writer);
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn upgrade_path_rejects_future_schema_without_modification() {
        let (workspace, database_path) = create_current_search_workspace("upgrade-future");
        let future_version = SQLITE_SCHEMA_VERSION_CURRENT + 7;
        let writer = Connection::open(&database_path).unwrap();
        writer
            .execute_batch(&format!("PRAGMA user_version = {future_version}"))
            .unwrap();
        drop(writer);
        let before = sha256_file(&database_path).unwrap();

        let error = open_workspace_db_for_upgrade(&workspace)
            .expect_err("workspace activation must not downgrade a future schema");
        assert!(error.to_string().contains("newer than this version"));
        assert_eq!(sha256_file(&database_path).unwrap(), before);

        let writer = Connection::open(&database_path).unwrap();
        assert_eq!(read_schema_version(&writer).unwrap(), future_version);
        drop(writer);
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn read_connection_rejects_executable_schema_objects() {
        for (name, sql) in [
            (
                "trigger",
                "CREATE TRIGGER hostile AFTER UPDATE ON messages BEGIN SELECT 1; END",
            ),
            (
                "view",
                "CREATE VIEW hostile AS SELECT id, subject FROM messages",
            ),
        ] {
            let (workspace, database_path) =
                create_current_search_workspace(&format!("hostile-schema-{name}"));
            let writer = Connection::open(&database_path).unwrap();
            writer.execute_batch(sql).unwrap();
            drop(writer);

            let error = open_workspace_db_for_read(&workspace)
                .expect_err("executable schema objects must be rejected");
            assert!(error
                .to_string()
                .contains("unsupported executable schema objects"));
            fs::remove_dir_all(workspace).unwrap();
        }
    }

    #[test]
    fn import_progress_count_probe_rejects_hostile_sqlite_views() {
        let directory = temporary_test_directory("progress-count-sqlite-hardening");
        let database_path = directory.join("index.sqlite");
        let writer = Connection::open(&database_path).unwrap();
        writer
            .execute_batch(
                "CREATE TABLE import_metadata (
                     key TEXT PRIMARY KEY,
                     value TEXT NOT NULL
                 );
                 INSERT INTO import_metadata (key, value) VALUES ('error_count', '7');
                 CREATE VIEW messages AS SELECT 1 AS id;",
            )
            .unwrap();
        assert_eq!(
            writer
                .query_row("SELECT COUNT(*) FROM messages", [], |row| {
                    row.get::<_, usize>(0)
                })
                .unwrap(),
            1
        );
        drop(writer);

        assert_eq!(
            read_index_progress_counts(&database_path),
            (None, Some(7)),
            "the progress probe must configure SQLite defensively before reading a hostile view"
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn workspace_index_symlinks_and_hardlinks_are_rejected() {
        let root = temporary_test_directory("hostile-index-links");
        let source_workspace = root.join("source");
        fs::create_dir(&source_workspace).unwrap();
        let source_database = source_workspace.join("index.sqlite");
        let conn = Connection::open(&source_database).unwrap();
        initialize_schema(&conn).unwrap();
        drop(conn);

        let symlink_workspace = root.join("symlink");
        fs::create_dir(&symlink_workspace).unwrap();
        std::os::unix::fs::symlink(&source_database, symlink_workspace.join("index.sqlite"))
            .unwrap();
        assert!(open_workspace_db_for_read(&symlink_workspace).is_err());

        let hardlink_workspace = root.join("hardlink");
        fs::create_dir(&hardlink_workspace).unwrap();
        fs::hard_link(&source_database, hardlink_workspace.join("index.sqlite")).unwrap();
        assert!(open_workspace_db_for_read(&hardlink_workspace).is_err());
        assert!(open_workspace_db_for_upgrade(&hardlink_workspace).is_err());

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hostile_fts_shadow_data_and_invalid_text_fail_without_panicking() {
        let (workspace, database_path) = create_current_search_workspace("hostile-sqlite-data");
        let writer = Connection::open(&database_path).unwrap();
        writer
            .execute_batch("UPDATE messages SET subject = CAST(x'80' AS TEXT) WHERE id = 1;")
            .unwrap();
        drop(writer);

        let conn = open_workspace_db_for_read(&workspace).unwrap();
        let plain = MessageSearchCriteria::from_inputs(None, None).unwrap();
        let plain_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            query_messages_page(MessagePageQuery {
                conn: &conn,
                folder_id: None,
                include_subfolders: false,
                criteria: &plain,
                sort_order: None,
                limit: Some(10),
                offset: Some(0),
                operation: None,
            })
        }));
        assert!(plain_result.is_ok());
        assert!(plain_result.unwrap().is_err());
        drop(conn);

        let writer = Connection::open(&database_path).unwrap();
        writer
            .execute_batch("DELETE FROM messages_fts_data;")
            .unwrap();
        drop(writer);
        let fts_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            open_workspace_db_for_read(&workspace)
        }));
        assert!(fts_result.is_ok());
        assert!(fts_result.unwrap().is_err());

        let conn = Connection::open(&database_path).unwrap();
        assert_eq!(
            read_schema_version(&conn).unwrap(),
            SQLITE_SCHEMA_VERSION_CURRENT
        );
        drop(conn);
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn read_connection_rejects_missing_required_search_table_clearly() {
        let (workspace, database_path) = create_current_search_workspace("read-search-missing");
        let writer = Connection::open(&database_path).unwrap();
        writer.execute_batch("DROP TABLE messages_fts").unwrap();
        drop(writer);

        let error = open_workspace_db_for_read(&workspace)
            .expect_err("missing search table must be rejected");
        assert!(error.to_string().contains("messages_fts"));
        assert_ne!(error.code, Some(SEARCH_CANCELLED_CODE));

        let writer = Connection::open(&database_path).unwrap();
        assert_eq!(
            read_schema_version(&writer).unwrap(),
            SQLITE_SCHEMA_VERSION_CURRENT
        );
        drop(writer);
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn arbitrary_source_path_is_not_accepted_as_a_workspace_id() {
        let root = temporary_test_directory("profile-workspace-id-validation");
        let profile = synthetic_runtime_profile(&root);
        let error = workspace_path_for_id(profile.paths(), "/tmp/source.pst")
            .expect_err("search commands accept validated workspace ids, not source paths");
        assert_eq!(error.to_string(), "Invalid workspace id.");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_schema_migrates_columns_before_indexes_and_is_idempotent() {
        let directory = temporary_test_directory("legacy-schema-migration");
        let database_path = directory.join("index.sqlite");
        let conn = create_legacy_workspace_database(&database_path);

        assert_eq!(read_schema_version(&conn).unwrap(), 0);
        assert_eq!(
            missing_columns(&conn, "messages", LEGACY_THREADING_COLUMNS).unwrap(),
            LEGACY_THREADING_COLUMNS
        );
        assert_eq!(
            missing_columns(
                &conn,
                "messages",
                &["body_source", "body_html", "attachment_names"]
            )
            .unwrap(),
            vec!["body_source", "body_html", "attachment_names"]
        );
        assert_eq!(
            missing_columns(&conn, "attachments", &["sanitized_filename"]).unwrap(),
            vec!["sanitized_filename"]
        );

        let existing_message: (String, String) = conn
            .query_row(
                "SELECT subject, body FROM messages WHERE id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        initialize_schema(&conn).expect("legacy migration should succeed");

        assert_eq!(
            read_schema_version(&conn).unwrap(),
            SQLITE_SCHEMA_VERSION_CURRENT
        );
        assert!(missing_columns(&conn, "messages", LEGACY_THREADING_COLUMNS)
            .unwrap()
            .is_empty());
        assert!(
            missing_columns(&conn, "attachments", &["sanitized_filename"])
                .unwrap()
                .is_empty()
        );
        for index in [
            "idx_messages_message_id",
            "idx_messages_conversation_id",
            "idx_messages_conversation_date",
            "idx_messages_folder_conversation",
            "idx_messages_normalized_subject",
            "idx_attachments_message_id",
            "idx_attachments_filename",
            "idx_attachments_sanitized_filename",
        ] {
            assert!(sqlite_index_exists(&conn, index), "missing index {index}");
        }
        assert_eq!(
            conn.query_row(
                "SELECT subject, body FROM messages WHERE id = 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .unwrap(),
            existing_message
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM attachments", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(!conversation_data_is_indexed(&conn).unwrap());

        initialize_schema(&conn).expect("second migration should be idempotent");
        assert_eq!(
            read_schema_version(&conn).unwrap(),
            SQLITE_SCHEMA_VERSION_CURRENT
        );
        drop(conn);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn failed_schema_index_creation_rolls_back_version_and_columns() {
        let directory = temporary_test_directory("failed-schema-migration");
        let database_path = directory.join("index.sqlite");
        let conn = create_legacy_workspace_database(&database_path);
        let broken_index = [SchemaIndex {
            name: "idx_messages_deliberate_failure",
            table: "messages",
            columns: &["column_that_does_not_exist"],
            sql: "CREATE INDEX idx_messages_deliberate_failure ON messages(column_that_does_not_exist)",
        }];

        let error = migrate_schema(&conn, &broken_index)
            .expect_err("deliberately invalid index migration should fail");
        assert!(error
            .to_string()
            .contains("idx_messages_deliberate_failure"));
        assert!(error.to_string().contains("messages"));
        assert!(error.to_string().contains("column_that_does_not_exist"));
        assert_eq!(read_schema_version(&conn).unwrap(), 0);
        assert!(!column_exists(&conn, "messages", "message_id_header").unwrap());
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );

        initialize_schema(&conn).expect("a later valid migration should resume safely");
        assert_eq!(
            read_schema_version(&conn).unwrap(),
            SQLITE_SCHEMA_VERSION_CURRENT
        );
        drop(conn);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn fresh_database_receives_current_verified_schema() {
        let conn = Connection::open_in_memory().expect("fresh database should open");
        initialize_schema(&conn).expect("fresh schema should initialize");
        assert_eq!(
            read_schema_version(&conn).unwrap(),
            SQLITE_SCHEMA_VERSION_CURRENT
        );
        verify_schema(&conn).expect("fresh schema should contain all required columns");
        assert!(sqlite_index_exists(&conn, "idx_messages_message_id"));
        assert!(sqlite_index_exists(
            &conn,
            "idx_attachments_sanitized_filename"
        ));
    }

    #[test]
    fn executable_architecture_parser_handles_thin_and_universal_macho() {
        let mut thin_x86_64 = vec![0xcf, 0xfa, 0xed, 0xfe];
        thin_x86_64.extend_from_slice(&0x0100_0007_u32.to_le_bytes());
        assert_eq!(macho_architectures(&thin_x86_64).as_deref(), Some("x86_64"));

        let mut thin_arm64 = vec![0xcf, 0xfa, 0xed, 0xfe];
        thin_arm64.extend_from_slice(&0x0100_000c_u32.to_le_bytes());
        assert_eq!(macho_architectures(&thin_arm64).as_deref(), Some("arm64"));

        let mut universal = vec![0xca, 0xfe, 0xba, 0xbe];
        universal.extend_from_slice(&2_u32.to_be_bytes());
        for cpu_type in [0x0100_0007_u32, 0x0100_000c_u32] {
            universal.extend_from_slice(&cpu_type.to_be_bytes());
            universal.extend_from_slice(&[0_u8; 16]);
        }
        assert_eq!(
            macho_architectures(&universal).as_deref(),
            Some("x86_64 + arm64")
        );
    }

    #[test]
    fn readpst_output_capture_is_bounded_while_fully_drained() {
        let bytes = (0..1024 * 1024)
            .map(|index| (index % 251) as u8)
            .collect::<Vec<_>>();
        let counter = AtomicU64::new(0);
        let output = drain_bounded_process_output(&bytes[..], 4096, &counter).unwrap();
        assert_eq!(output.total_bytes, bytes.len() as u64);
        assert_eq!(counter.load(Ordering::Acquire), bytes.len() as u64);
        assert_eq!(output.tail, bytes[bytes.len() - 4096..]);
        assert!(output.truncated());

        let zero_counter = AtomicU64::new(0);
        let zero = drain_bounded_process_output(&bytes[..], 0, &zero_counter).unwrap();
        assert!(zero.tail.is_empty());
        assert_eq!(zero.total_bytes, bytes.len() as u64);
    }

    #[test]
    fn readpst_cancellation_takes_precedence_over_a_signal_exit() {
        assert_eq!(
            classify_readpst_observed_exit(true, false),
            ReadpstObservedExit::Cancelled
        );
        assert_eq!(
            classify_readpst_observed_exit(false, false),
            ReadpstObservedExit::Failure
        );
        assert_eq!(
            classify_readpst_observed_exit(false, true),
            ReadpstObservedExit::Success
        );
    }

    #[test]
    fn import_cancellation_token_rejects_stale_workspace_operation_and_generation() {
        use import_progress::ProgressStatus;

        assert!(cancellation_token_matches(
            "workspace-a",
            "operation-a",
            7,
            ProgressStatus::Active,
            "workspace-a",
            "operation-a",
            7,
        ));
        assert!(!cancellation_token_matches(
            "workspace-a",
            "operation-a",
            7,
            ProgressStatus::Active,
            "workspace-b",
            "operation-a",
            7,
        ));
        assert!(!cancellation_token_matches(
            "workspace-a",
            "operation-a",
            7,
            ProgressStatus::Active,
            "workspace-a",
            "operation-old",
            7,
        ));
        assert!(!cancellation_token_matches(
            "workspace-a",
            "operation-a",
            7,
            ProgressStatus::Active,
            "workspace-a",
            "operation-a",
            6,
        ));
        assert!(!cancellation_token_matches(
            "workspace-a",
            "operation-a",
            7,
            ProgressStatus::Complete,
            "workspace-a",
            "operation-a",
            7,
        ));
    }

    #[test]
    fn integrity_finish_result_surfaces_both_failures_and_preserves_cancellation() {
        let error = combine_integrity_operation_result::<()>(
            Err(import_cancelled_error("Synthetic operation cancellation.")),
            Err(AppError::new("Synthetic finalization failure.")),
        )
        .unwrap_err();
        assert_eq!(error.code, Some(IMPORT_CANCELLED_CODE));
        assert!(error.message.contains("Synthetic operation cancellation."));
        assert!(error.message.contains("Synthetic finalization failure."));

        let error = combine_integrity_operation_result::<()>(
            Err(AppError::new("Synthetic operation failure.")),
            Err(import_cancelled_error(
                "Synthetic finalization cancellation.",
            )),
        )
        .unwrap_err();
        assert_eq!(error.code, Some(IMPORT_CANCELLED_CODE));
        assert!(error.message.contains("Synthetic operation failure."));
        assert!(error
            .message
            .contains("Synthetic finalization cancellation."));
    }

    #[test]
    fn readpst_output_validation_rejects_links_nonregular_files_and_limits() {
        use std::os::unix::fs::symlink;

        let workspace = temporary_test_directory("readpst-output-validation");
        let output = workspace.join("extracting");
        create_private_directory_all(&output).unwrap();
        let message = output.join("message.eml");
        fs::write(&message, b"synthetic").unwrap();
        let snapshot = inspect_readpst_output(&output, 1024).unwrap();
        assert_eq!(snapshot.entries, 1);
        assert_eq!(snapshot.total_bytes, 9);
        assert!(inspect_readpst_output(&output, 8).is_err());

        let external = workspace.join("external.bin");
        fs::write(&external, b"external").unwrap();
        let link = output.join("link.eml");
        symlink(&external, &link).unwrap();
        assert!(inspect_readpst_output(&output, 1024).is_err());
        fs::remove_file(&link).unwrap();

        let hardlink = output.join("hardlink.eml");
        fs::hard_link(&external, &hardlink).unwrap();
        assert!(inspect_readpst_output(&output, 1024).is_err());
        fs::remove_file(&hardlink).unwrap();

        let oversized = output.join("oversized.eml");
        File::create(&oversized)
            .unwrap()
            .set_len(MAX_READPST_EXTRACTED_FILE_BYTES + 1)
            .unwrap();
        assert!(inspect_readpst_output(&output, u64::MAX).is_err());
        fs::remove_file(&oversized).unwrap();
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn readpst_cleanup_is_exact_and_does_not_follow_child_symlinks() {
        use std::os::unix::fs::symlink;

        let workspace = temporary_test_directory("readpst-cleanup");
        let output = workspace.join("extracting");
        create_private_directory_all(&output).unwrap();
        let external = workspace.join("external");
        create_private_directory_all(&external).unwrap();
        fs::write(external.join("preserved.bin"), b"preserved").unwrap();
        symlink(&external, output.join("link-outside")).unwrap();
        remove_readpst_output_directory(&workspace, &output).unwrap();
        assert!(!output.exists());
        assert_eq!(
            fs::read(external.join("preserved.bin")).unwrap(),
            b"preserved"
        );
        assert!(remove_readpst_output_directory(&workspace, &external).is_err());
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn isolated_helper_clears_environment_and_terminates_hanging_process_groups() {
        let mut environment_check = Command::new("/bin/sh");
        environment_check
            .env("PST_QUICKVIEW_SYNTHETIC_SECRET", "must-not-leak")
            .arg("-c")
            .arg("test -z \"$PST_QUICKVIEW_SYNTHETIC_SECRET\" && printf isolated || exit 9");
        configure_isolated_readpst_command(&mut environment_check);
        let (status, stdout, stderr) =
            run_isolated_bounded_command(environment_check, Duration::from_secs(2)).unwrap();
        assert!(status.success());
        assert_eq!(stdout.tail, b"isolated");
        assert!(stderr.tail.is_empty());

        let mut hanging = Command::new("/bin/sh");
        hanging
            .arg("-c")
            .arg("trap '' TERM; (trap '' TERM; sleep 30) & wait");
        configure_isolated_readpst_command(&mut hanging);
        let error = run_isolated_bounded_command(hanging, Duration::from_millis(50)).unwrap_err();
        assert_eq!(error.message, "ReadPST helper timed out.");
    }

    #[test]
    fn readpst_source_identity_detects_content_and_metadata_changes() {
        let directory = temporary_test_directory("readpst-source-identity");
        let source = directory.join("source.pst");
        fs::write(&source, b"!BDNsynthetic source").unwrap();
        let before = file_identity(&source, "source PST").unwrap();
        let mut changed = OpenOptions::new().append(true).open(&source).unwrap();
        changed.write_all(b"changed").unwrap();
        changed.flush().unwrap();
        drop(changed);
        assert_ne!(file_identity(&source, "source PST").unwrap(), before);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn bounded_log_rotation_keeps_only_configured_backups() {
        let directory = temporary_test_directory("log-rotation");
        let log_path = directory.join("application.log");
        fs::write(&log_path, b"first-generation").unwrap();
        rotate_log_if_needed(&log_path, 4, 2).unwrap();
        assert!(!log_path.exists());
        assert_eq!(
            fs::read_to_string(rotated_log_path(&log_path, 1)).unwrap(),
            "first-generation"
        );

        fs::write(&log_path, b"second-generation").unwrap();
        rotate_log_if_needed(&log_path, 4, 2).unwrap();
        assert_eq!(
            fs::read_to_string(rotated_log_path(&log_path, 1)).unwrap(),
            "second-generation"
        );
        assert_eq!(
            fs::read_to_string(rotated_log_path(&log_path, 2)).unwrap(),
            "first-generation"
        );

        fs::write(&log_path, b"third-generation").unwrap();
        rotate_log_if_needed(&log_path, 4, 2).unwrap();
        assert_eq!(
            fs::read_to_string(rotated_log_path(&log_path, 1)).unwrap(),
            "third-generation"
        );
        assert_eq!(
            fs::read_to_string(rotated_log_path(&log_path, 2)).unwrap(),
            "second-generation"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn workspace_logs_are_private_bounded_and_path_free() {
        let workspace = temporary_test_directory("private-logs");
        initialize_import_log(&workspace, "synthetic-version-not-logged").unwrap();
        append_import_log(&workspace, "state=complete indexed=3 errors=0").unwrap();
        append_export_log(
            &workspace,
            "Exported: message_id=2 attachment_id=4 size_bytes=8",
        )
        .unwrap();

        let import_path = import_log_path(&workspace);
        let export_path = workspace.join("logs").join("exports.log");
        let import_contents = fs::read_to_string(&import_path).unwrap();
        assert!(!import_contents.contains("source="));
        assert!(!import_contents.contains("helper="));
        assert!(!import_contents.contains("readpst_version="));
        assert!(!import_contents.contains(&workspace.display().to_string()));
        assert!(!import_contents.contains("fingerprint"));

        assert_eq!(
            fs::metadata(workspace.join("logs"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            PRIVATE_DIRECTORY_MODE
        );
        for path in [&import_path, &export_path] {
            let metadata = fs::metadata(path).unwrap();
            assert!(metadata.is_file());
            assert_eq!(metadata.nlink(), 1);
            assert_eq!(metadata.permissions().mode() & 0o777, PRIVATE_FILE_MODE);
        }
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    fn application_logging_uses_only_the_resolved_log_root() {
        let root = temporary_test_directory("resolved-application-log");
        let profile = synthetic_runtime_profile(&root);
        append_application_log(
            profile.paths(),
            "synthetic_profile_test",
            "complete",
            None,
            None,
        )
        .unwrap();

        let log_path = profile.paths().log_root().join(APPLICATION_LOG_FILE);
        let contents = fs::read_to_string(&log_path).unwrap();
        assert!(contents.contains("operation=synthetic_profile_test"));
        assert!(!contents.contains(&root.display().to_string()));
        assert!(!profile.paths().application_data_root().exists());
        assert_eq!(
            fs::metadata(profile.paths().log_root())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            PRIVATE_DIRECTORY_MODE
        );
        assert_eq!(
            fs::metadata(&log_path).unwrap().permissions().mode() & 0o777,
            PRIVATE_FILE_MODE
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn app_support_session_scope_rejects_outside_workspace_before_marker_access() {
        let root = temporary_test_directory("session-profile-scope");
        let profile = synthetic_runtime_profile(&root);
        let workspace_id = "d".repeat(64);
        let outside_workspace = root.join("outside-profile").join(&workspace_id);
        fs::create_dir_all(&outside_workspace).unwrap();
        let missing_source = root.join("missing-source.pst");

        let raw_error = validate_session_workspace_scope_before_access(
            profile.paths(),
            &missing_source,
            &outside_workspace,
            &workspace_id,
            WorkspaceLocationMode::AppSupport,
        )
        .unwrap_err();
        assert!(raw_error.to_string().contains("outside"));
        assert!(!profile.paths().application_data_root().exists());

        let validation_error = validate_session_workspace_path(
            profile.paths(),
            &missing_source,
            &outside_workspace,
            &workspace_id,
            WorkspaceLocationMode::AppSupport,
        )
        .unwrap_err();
        assert!(validation_error.to_string().contains("outside"));
        assert!(!outside_workspace.join(WORKSPACE_MARKER_FILE).exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn next_to_pst_session_scope_rejects_mislabeled_profile_path_before_marker_access() {
        let root = temporary_test_directory("next-to-pst-session-scope");
        let profile = synthetic_runtime_profile(&root);
        let workspace_id = "e".repeat(64);
        let source = root.join("selected-source").join("source.pst");
        let mislabeled_profile_workspace = root.join("workspaces").join(&workspace_id);
        fs::create_dir_all(&mislabeled_profile_workspace).unwrap();

        let raw_error = validate_session_workspace_scope_before_access(
            profile.paths(),
            &source,
            &mislabeled_profile_workspace,
            &workspace_id,
            WorkspaceLocationMode::NextToPst,
        )
        .unwrap_err();
        assert!(raw_error.to_string().contains("cache folder"));

        let validation_error = validate_session_workspace_path(
            profile.paths(),
            &source,
            &mislabeled_profile_workspace,
            &workspace_id,
            WorkspaceLocationMode::NextToPst,
        )
        .unwrap_err();
        assert!(validation_error.to_string().contains("cache folder"));
        assert!(!mislabeled_profile_workspace
            .join(WORKSPACE_MARKER_FILE)
            .exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn profile_owned_workspace_root_rejects_a_post_resolution_symlink() {
        use std::os::unix::fs::symlink;

        let root = temporary_test_directory("profile-root-symlink");
        let profile = synthetic_runtime_profile(&root);
        let data_root = profile.paths().application_data_root();
        let data_parent = data_root.parent().unwrap();
        fs::create_dir_all(data_parent).unwrap();
        let synthetic_legacy_target = root.join("synthetic-legacy-target");
        fs::create_dir(&synthetic_legacy_target).unwrap();
        symlink(&synthetic_legacy_target, data_root).unwrap();

        assert!(workspaces_dir(profile.paths()).is_err());
        assert!(!synthetic_legacy_target.join("workspaces").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn production_workspace_discovery_remains_legacy_profile_compatible() {
        let root = temporary_test_directory("production-workspace-discovery");
        let source = root.join("source.pst");
        fs::write(&source, b"!BDNsynthetic production source").unwrap();
        let identity = identify_pst(&source).unwrap();
        let profile = synthetic_production_runtime_profile(&root);
        let workspace = workspaces_dir(profile.paths())
            .unwrap()
            .join(&identity.workspace_id);
        create_private_directory_all(&workspace).unwrap();
        let marker = WorkspaceMarker {
            app: WORKSPACE_MARKER_APP_NAME.to_string(),
            workspace_id: identity.workspace_id.clone(),
            pst_content_fingerprint: identity.content_fingerprint.clone(),
            workspace_location_mode: WorkspaceLocationMode::AppSupport.as_str().to_string(),
            original_pst_path: identity.display_path.clone(),
        };
        let mut output = create_private_file_new(&workspace.join(WORKSPACE_MARKER_FILE)).unwrap();
        serde_json::to_writer(&mut output, &marker).unwrap();
        output.flush().unwrap();
        drop(output);

        let candidates = find_existing_workspaces(profile.paths(), &identity).unwrap();
        assert!(candidates.iter().any(|candidate| {
            candidate.path == workspace && candidate.mode == WorkspaceLocationMode::AppSupport
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn next_to_pst_routing_and_source_identity_ignore_runtime_profile() {
        let root = temporary_test_directory("next-to-pst-profile-independent");
        let source = root.join("source.pst");
        fs::write(&source, b"!BDNsynthetic profile-independent source").unwrap();
        let source_before = file_identity(&source, "source PST").unwrap();
        let identity = identify_pst(&source).unwrap();
        let roots_before = next_to_pst_candidate_roots(&identity);

        let alternate = synthetic_runtime_profile(&root);
        let production = synthetic_production_runtime_profile(&root);
        assert_ne!(
            alternate.paths().application_data_root(),
            production.paths().application_data_root()
        );
        assert_eq!(next_to_pst_candidate_roots(&identity), roots_before);
        assert_eq!(file_identity(&source, "source PST").unwrap(), source_before);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn log_writes_reject_symlinks_hardlinks_and_nonregular_paths() {
        use std::os::unix::fs::symlink;

        let workspace = temporary_test_directory("hostile-log-path");
        let logs = workspace.join("logs");
        create_private_directory_all(&logs).unwrap();
        let log_path = import_log_path(&workspace);
        let external = workspace.join("external.log");
        fs::write(&external, b"must remain unchanged").unwrap();

        symlink(&external, &log_path).unwrap();
        assert!(append_import_log(&workspace, "synthetic event").is_err());
        assert_eq!(fs::read(&external).unwrap(), b"must remain unchanged");
        fs::remove_file(&log_path).unwrap();

        fs::hard_link(&external, &log_path).unwrap();
        assert!(append_import_log(&workspace, "synthetic event").is_err());
        assert_eq!(fs::read(&external).unwrap(), b"must remain unchanged");
        fs::remove_file(&log_path).unwrap();

        fs::create_dir(&log_path).unwrap();
        assert!(append_import_log(&workspace, "synthetic event").is_err());
        fs::remove_dir_all(workspace).unwrap();
    }

    #[test]
    #[ignore = "set PST_QUICKVIEW_SCHEMA_FIXTURE to an existing workspace index.sqlite"]
    fn migrates_existing_workspace_fixture_without_reindexing() {
        let path = env::var_os("PST_QUICKVIEW_SCHEMA_FIXTURE")
            .map(PathBuf::from)
            .expect("PST_QUICKVIEW_SCHEMA_FIXTURE is required");
        let conn = Connection::open(&path).expect("workspace fixture should open");
        let before_messages = conn
            .query_row("SELECT COUNT(*) FROM messages", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        let before_attachments = conn
            .query_row("SELECT COUNT(*) FROM attachments", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();

        initialize_schema(&conn).expect("workspace fixture migration should succeed");

        assert_eq!(
            read_schema_version(&conn).unwrap(),
            SQLITE_SCHEMA_VERSION_CURRENT
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            before_messages
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM attachments", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            before_attachments
        );
        assert!(!conversation_data_is_indexed(&conn).unwrap());
        println!(
            "migration passed: messages={before_messages} attachments={before_attachments} user_version={SQLITE_SCHEMA_VERSION_CURRENT}"
        );
    }

    #[test]
    #[ignore = "optional external rich MSG fixture"]
    fn verifies_rich_msg_fixture_reconstruction() {
        let Some(path) =
            optional_external_fixture(RICH_MSG_FIXTURE_ENV, RICH_MSG_FIXTURE_LEGACY_ENV)
        else {
            return;
        };
        let expected_sha256 = optional_expected_fixture_hash(RICH_MSG_EXPECTED_SHA256_ENV);
        let message = assert_read_only_msg_fixture(&path, expected_sha256.as_deref());

        assert_eq!(message.message_class, "IPM.Note");
        assert!(
            message.calendar.is_none(),
            "normal IPM.Note must not use calendar layout"
        );
        assert_eq!(message.body_source, BodySource::RtfHtmlConverted);
        assert!(!message.body_html.trim().is_empty());
        assert!(!message.body_text.trim().is_empty());
        assert!(message
            .attachments
            .iter()
            .all(|attachment| !attachment.filename.eq_ignore_ascii_case("rtf-body.rtf")));
        let pdf_attachments = message
            .attachments
            .iter()
            .filter(|attachment| {
                attachment.filename.to_ascii_lowercase().ends_with(".pdf")
                    || attachment
                        .content_type
                        .eq_ignore_ascii_case("application/pdf")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            pdf_attachments.len(),
            2,
            "both PDFs must remain normal attachments"
        );
        assert_eq!(
            message.inline_resources.len(),
            4,
            "all exact CID-matched signature PNGs should be inline resources"
        );
        assert!(message
            .inline_resources
            .iter()
            .all(|attachment| attachment.content_type.eq_ignore_ascii_case("image/png")));
        assert!(message
            .inline_resources
            .iter()
            .any(|attachment| attachment.filename.eq_ignore_ascii_case("image004.png")));
        let image004 = message
            .inline_resources
            .iter()
            .find(|attachment| attachment.filename.eq_ignore_ascii_case("image004.png"))
            .expect("image004.png should be reconstructed inline");
        assert_eq!(image004.content_type, "image/png");
        assert_eq!(image004.id, 2);
        assert_eq!(message.attachments.len(), 2);
        assert!(message
            .raw_source
            .contains("Selected body source: rtf_html_converted"));
        assert!(message
            .raw_source
            .contains("body-like RTF attachment rtf-body.rtf"));
        assert!(message
            .raw_source
            .contains("Promoted RTF attachment suppressed: true"));
        assert!(message.raw_source.contains(
            "image004.png | type=image/png | content_id=image004.png@01d41e85.b4da5c40 | method=1"
        ));
        assert!(
            message.raw_source.matches("classification=inline").count()
                >= message.inline_resources.len()
        );

        let view = source_eml_view_from_path(
            &path,
            "rich-message-fixture.msg".to_string(),
            "standalone",
            0,
            None,
        )
        .expect("standalone viewer should render");
        assert_eq!(view.attachments.len(), 2);
        assert_eq!(view.inline_resources.len(), 4);
        assert_eq!(view.embedded_image_count, 4);
        assert!(view.sanitized_html.contains("data:image/png;base64,"));
        assert!(view.sanitized_html.contains("width=\"17\""));
        assert!(view.sanitized_html.contains("height=\"17\""));
        assert!(view.calendar.is_none());
        println!(
            "Rich fixture passed read-only check: identity_pinned={} attachments={} inline_resources={}",
            expected_sha256.is_some(),
            view.attachments.len(),
            view.inline_resources.len()
        );
    }

    #[test]
    #[ignore = "optional external legacy MSG fixture"]
    fn verifies_legacy_msg_fixture_reconstruction() {
        let Some(path) =
            optional_external_fixture(LEGACY_MSG_FIXTURE_ENV, LEGACY_MSG_FIXTURE_LEGACY_ENV)
        else {
            return;
        };
        let expected_sha256 = optional_expected_fixture_hash(LEGACY_MSG_EXPECTED_SHA256_ENV);
        let message = assert_read_only_msg_fixture(&path, expected_sha256.as_deref());

        assert_eq!(message.message_class, "IPM.Note");
        assert!(
            message.calendar.is_none(),
            "normal IPM.Note must not use calendar layout"
        );
        assert!(
            matches!(
                message.body_source,
                BodySource::RtfConverted | BodySource::RtfHtmlConverted
            ),
            "legacy RTF should be selected as the readable body, got {:?}",
            message.body_source
        );
        assert!(
            !message.body_text.trim().is_empty() || !message.body_html.trim().is_empty(),
            "legacy RTF must produce readable text or recovered HTML"
        );
        assert!(
            message.body_text.len() > 2_000,
            "the long quoted email chain should remain present"
        );
        assert!(message
            .attachments
            .iter()
            .all(|attachment| !attachment.filename.eq_ignore_ascii_case("rtf-body.rtf")));
        assert!(message
            .raw_source
            .contains("body-like RTF attachment rtf-body.rtf"));
        assert!(message
            .raw_source
            .contains("Promoted RTF attachment suppressed: true"));

        let view = source_eml_view_from_path(
            &path,
            "legacy-message-fixture.msg".to_string(),
            "standalone",
            0,
            None,
        )
        .expect("standalone legacy viewer should render");
        assert!(view.calendar.is_none());
        assert!(view
            .attachments
            .iter()
            .all(|attachment| !attachment.filename.eq_ignore_ascii_case("rtf-body.rtf")));
        assert!(!view.body_text.trim().is_empty());
        println!(
            "Legacy fixture passed read-only check: identity_pinned={} body_source={} body_text_bytes={}",
            expected_sha256.is_some(),
            view.body_source,
            view.body_text.len()
        );
    }
}
