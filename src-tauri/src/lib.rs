use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use chrono::{DateTime, Utc};
use mailparse::MailHeaderMap;
use rusqlite::{
    params, params_from_iter, types::Value, Connection, OpenFlags, OptionalExtension, Transaction,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    borrow::Cow,
    collections::{HashMap, HashSet, VecDeque},
    env,
    ffi::OsStr,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::{ffi::OsStrExt, fs::MetadataExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
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
mod search;
mod search_cancellation;
mod search_pagination;
#[cfg(test)]
mod search_stress;
mod threading;

use calendar_msg::CalendarItemDetails;
use search::{
    build_message_query_source, conversation_sort_clause, message_keyset_condition,
    message_sort_clause, query_source_with_condition, resolve_message_keyset_boundary,
    search_match_context_from_row, search_match_select_sql, validate_conversation_sort,
    validate_message_sort_workspace_count, MessageQuerySource, MessageSearchCriteria, MessageSort,
    SearchFilters, SearchMatchContext, SearchValidationError,
};
use search_cancellation::{
    is_sqlite_interrupt, SearchCancellationError, SearchCancellationRegistry,
    SearchOperationCategory, SearchOperationGuard, SEARCH_CANCELLED_CODE,
};
use search_pagination::{
    opaque_hash, MessageCursorContext, MessageCursorPosition, SearchCursorCodec, SearchCursorError,
    UNSUPPORTED_SEARCH_CURSOR_CODE,
};

use threading::{
    assign_threads, extract_email_addresses, extract_message_ids, normalize_thread_subject,
    ThreadInput,
};

const APP_SUPPORT_NAME: &str = "PST QuickView";
const SETUP_COMMAND: &str = "brew install libpst";
const MISSING_READPST_INSTRUCTIONS: &str =
    "Install libpst with Homebrew or use a PST QuickView build that includes readpst.";
const ROOT_FOLDER_NAME: &str = "PST";
const NEXT_TO_PST_WORKSPACE_DIR: &str = ".pst-quickview.noindex";
const LEGACY_NEXT_TO_PST_WORKSPACE_DIR: &str = ".pst-quickview";
const WORKSPACE_MARKER_FILE: &str = ".pst-quickview-workspace";
const WORKSPACE_METADATA_FILE: &str = "metadata.json";
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
const DEFAULT_MESSAGE_PAGE_SIZE: i64 = 250;
const MAX_MESSAGE_PAGE_SIZE: i64 = 1000;
const DEFAULT_CONVERSATION_PAGE_SIZE: i64 = 100;
const MAX_CONVERSATION_PAGE_SIZE: i64 = 500;
const CONVERSATION_MESSAGE_PAGE_SIZE: i64 = 100;
const CONVERSATION_SCHEMA_VERSION: &str = "conversation-index-v1";
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
const MESSAGE_FORMAT_PROBE_BYTES: usize = 64 * 1024;

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
            message: format!("readpst was not found. {MISSING_READPST_INSTRUCTIONS}"),
            setup_command: Some(format!(
                "{MISSING_READPST_INSTRUCTIONS} Command: {SETUP_COMMAND}"
            )),
            code: None,
        }
    }
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
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MessageCountResult {
    total_count: i64,
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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportProgress {
    stage: String,
    current: Option<usize>,
    total: Option<usize>,
    message: String,
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

#[derive(Clone, Debug)]
struct ReadpstLocation {
    path: PathBuf,
    source: ReadpstSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReadpstSource {
    BundledIntel,
    BundledAppleSilicon,
    BundledUniversal,
    System,
}

impl ReadpstSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::BundledIntel => "bundled_intel",
            Self::BundledAppleSilicon => "bundled_apple_silicon",
            Self::BundledUniversal => "bundled_universal",
            Self::System => "system",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::BundledIntel => "bundled Intel",
            Self::BundledAppleSilicon => "bundled Apple Silicon",
            Self::BundledUniversal => "bundled universal",
            Self::System => "system",
        }
    }
}

#[derive(Default)]
struct AppState {
    active_workspace: Mutex<Option<ActiveWorkspace>>,
    open_workspaces: Mutex<HashMap<String, ActiveWorkspace>>,
    import_running: AtomicBool,
    cancel_import_requested: AtomicBool,
    readpst_pid: Mutex<Option<u32>>,
    external_file_opens: Mutex<ExternalFileOpenState>,
    search_cancellations: Arc<SearchCancellationRegistry>,
    search_cursor_codec: SearchCursorCodec,
}

#[derive(Default)]
struct ExternalFileOpenState {
    frontend_ready: bool,
    external_open_received: bool,
    pending: VecDeque<ExternalFileOpenBatch>,
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
    Reimport,
}

impl ImportAction {
    fn from_arg(value: Option<String>) -> AppResult<Self> {
        match value.as_deref().unwrap_or("import") {
            "import" => Ok(Self::Import),
            "open_existing" => Ok(Self::OpenExisting),
            "resume_index" => Ok(Self::ResumeIndex),
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

#[derive(Debug, Default)]
struct IndexStats {
    discovered: usize,
    indexed: usize,
    error_count: usize,
    last_error: Option<String>,
}

pub fn run() {
    let _ = append_application_log("application", "started", None, None);
    let app = tauri::Builder::default()
        .menu(|handle| {
            let about_metadata = AboutMetadata {
                name: Some(APP_SUPPORT_NAME.to_string()),
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
                        APP_SUPPORT_NAME,
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
        .invoke_handler(tauri::generate_handler![
            check_readpst,
            get_app_diagnostics,
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
            prepare_external_file_opens,
            reindex_existing_emls,
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
        .build(tauri::generate_context!())
        .expect("error while building PST QuickView");

    app.run(|app_handle, event| {
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
        }
    });
}

#[tauri::command]
fn check_readpst() -> ReadpstStatus {
    let status = match find_readpst() {
        Some(location) => ReadpstStatus {
            available: true,
            version: Some(readpst_version(&location.path)),
            path: Some(location.path.display().to_string()),
            source: location.source.as_str().to_string(),
            source_label: location.source.label().to_string(),
            setup_command: SETUP_COMMAND.to_string(),
        },
        None => ReadpstStatus {
            available: false,
            path: None,
            version: None,
            source: "missing".to_string(),
            source_label: "missing".to_string(),
            setup_command: format!("{MISSING_READPST_INSTRUCTIONS} Command: {SETUP_COMMAND}"),
        },
    };
    let _ = append_application_log(
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
        .map(|workspace| active_database_diagnostics(workspace))
        .unwrap_or((None, "no active workspace".to_string()));
    let _ = append_application_log(
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
fn reveal_application_logs() -> AppResult<()> {
    let logs_dir = application_logs_dir()?;
    fs::create_dir_all(&logs_dir)?;
    let _ = append_application_log("application_log", "revealed", None, None);
    reveal_path(&logs_dir)
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
fn pick_message_file() -> Option<String> {
    rfd::FileDialog::new()
        .set_title("Open Message")
        .add_filter("Email Message", &["eml", "msg"])
        .pick_file()
        .map(|path| path.display().to_string())
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
fn prepare_external_file_opens(paths: Vec<String>) -> ExternalFileOpenBatch {
    build_external_file_open_batch(paths.into_iter().map(PathBuf::from).collect(), Vec::new())
}

#[tauri::command]
fn plan_pst_open(path: String, workspace_location_mode: String) -> AppResult<PstOpenPlan> {
    let mode = WorkspaceLocationMode::from_arg(&workspace_location_mode)?;
    let identity = identify_pst(&PathBuf::from(path))?;
    let selection = select_workspace(&identity, mode)?;
    let preflight = workspace_preflight(&identity, &selection);
    let mut existing_workspaces = find_existing_workspaces(&identity)?
        .into_iter()
        .map(|candidate| existing_workspace_summary(&candidate))
        .collect::<AppResult<Vec<_>>>()?;
    sort_existing_workspace_summaries(&mut existing_workspaces, &selection.path);

    Ok(PstOpenPlan {
        pst_path: identity.display_path,
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
async fn open_pst(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
    workspace_location_mode: String,
    existing_workspace_path: Option<String>,
    allow_duplicate: bool,
    import_action: Option<String>,
) -> AppResult<WorkspaceSummary> {
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
    let pst_path = pst_path
        .canonicalize()
        .map_err(|_| AppError::new("PST or workspace not available."))?;
    let workspace_path = workspace_path
        .canonicalize()
        .map_err(|_| AppError::new("PST or workspace not available."))?;

    validate_session_workspace_path(&pst_path, &workspace_path, &workspace_id, location_mode)?;

    let conn = open_workspace_db_for_upgrade(&workspace_path)?;
    let message_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;
    let import_status = metadata_value(&conn, "import_status")?
        .or_else(|| workspace_metadata_status(&workspace_path));
    let complete = import_status
        .as_deref()
        .is_some_and(|status| status == ImportStatus::Complete.as_str())
        || (import_status.is_none() && message_count > 0);
    if !complete {
        return Err(AppError::new("Workspace is not complete."));
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

    {
        let mut open_guard = state
            .open_workspaces
            .lock()
            .map_err(|_| AppError::new("Could not lock open workspace state."))?;
        open_guard.insert(active.id.clone(), active.clone());
    }

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
) -> AppResult<WorkspaceSummary> {
    if state.import_running.swap(true, Ordering::SeqCst) {
        return Err(AppError::new(
            "An import or indexing task is already running. Wait for it to finish before reindexing.",
        ));
    }
    state.cancel_import_requested.store(false, Ordering::SeqCst);

    let active = match active_workspace_for_id(&state, &workspace_id) {
        Ok(active) => active,
        Err(error) => {
            state.import_running.store(false, Ordering::SeqCst);
            state.cancel_import_requested.store(false, Ordering::SeqCst);
            return Err(error);
        }
    };

    let result =
        tauri::async_runtime::spawn_blocking(move || reindex_existing_emls_blocking(&app, active))
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
fn cancel_import(state: State<AppState>) -> AppResult<CancelImportResult> {
    if !state.import_running.load(Ordering::SeqCst) {
        return Ok(CancelImportResult {
            requested: false,
            pid: None,
            message: "No import is currently running.".to_string(),
        });
    }

    state.cancel_import_requested.store(true, Ordering::SeqCst);
    let pid = *state
        .readpst_pid
        .lock()
        .map_err(|_| AppError::new("Could not lock readpst process state."))?;

    if let Some(pid) = pid {
        terminate_process(pid, false)?;
    }

    Ok(CancelImportResult {
        requested: true,
        pid,
        message: match pid {
            Some(pid) => format!("Cancel requested. Sent terminate signal to readpst pid {pid}."),
            None => "Cancel requested. Indexing will stop at the next safe checkpoint.".to_string(),
        },
    })
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
    let workspace = resolve_workspace_for_id(&state, &workspace_id)?;
    let conn = open_workspace_db_for_read(&workspace)?;

    let mut statement = conn.prepare(
        "SELECT f.id,
                f.parent_id,
                f.path,
                f.name,
                COUNT(m.id) AS direct_message_count
           FROM folders f
           LEFT JOIN messages m ON m.folder_id = f.id
          GROUP BY f.id, f.parent_id, f.path, f.name
          ORDER BY f.path",
    )?;

    let rows = statement.query_map([], |row| {
        Ok(Folder {
            id: row.get(0)?,
            parent_id: row.get(1)?,
            path: row.get(2)?,
            name: row.get(3)?,
            message_count: row.get(4)?,
            direct_message_count: row.get(4)?,
        })
    })?;

    let mut folders = collect_rows(rows)?;
    let direct_counts = folders
        .iter()
        .map(|folder| (folder.path.clone(), folder.direct_message_count))
        .collect::<Vec<_>>();

    for folder in &mut folders {
        folder.message_count = direct_counts
            .iter()
            .filter(|(path, _)| path_is_self_or_descendant(path, &folder.path))
            .map(|(_, count)| *count)
            .sum();
    }

    Ok(folders)
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

#[tauri::command]
async fn list_messages(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    workspace_id: String,
    folder_id: Option<i64>,
    query: Option<String>,
    include_subfolders: bool,
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
    validate_message_sort_workspace_count(sort_order.as_deref(), 1)?;
    let workspace = resolve_workspace_for_id(&state, &workspace_id)?;
    let cursor_codec = state.search_cursor_codec.clone();
    run_search_worker(operation, move |operation| {
        let conn = open_workspace_db_for_search(&workspace, operation)?;
        query_messages_cursor_page(
            &conn,
            &workspace,
            &workspace_id,
            folder_id,
            include_subfolders,
            &criteria,
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

#[tauri::command]
async fn count_messages(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    workspace_id: String,
    folder_id: Option<i64>,
    query: Option<String>,
    include_subfolders: bool,
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
    let workspace = resolve_workspace_for_id(&state, &workspace_id)?;
    run_search_worker(operation, move |operation| {
        let conn = open_workspace_db_for_search(&workspace, operation)?;
        Ok(MessageCountResult {
            total_count: query_message_count(
                &conn,
                folder_id,
                include_subfolders,
                &criteria,
                Some(operation),
            )?,
        })
    })
    .await
}

#[tauri::command]
async fn search_messages_multi(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    workspace_ids: Vec<String>,
    query: Option<String>,
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
    ensure_multi_workspace_cursor_absent(cursor.as_deref())?;
    let criteria = MessageSearchCriteria::from_inputs(query, search_filters)?;
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
        query_multi_workspace_message_page(
            workspaces,
            &criteria,
            &sort_order_value,
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

#[tauri::command]
async fn count_messages_multi(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    workspace_ids: Vec<String>,
    query: Option<String>,
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
    let criteria = MessageSearchCriteria::from_inputs(query, search_filters)?;
    let mut seen = HashSet::new();
    let mut workspaces = Vec::new();
    for workspace_id in workspace_ids {
        operation.check_cancelled()?;
        if seen.insert(workspace_id.clone()) {
            workspaces.push(active_workspace_for_id(&state, &workspace_id)?);
        }
    }
    run_search_worker(operation, move |operation| {
        count_multi_workspace_messages(workspaces, &criteria, operation)
    })
    .await
}

struct MessageWorkspaceCursor {
    active: ActiveWorkspace,
    conn: Connection,
    pst_display_name: String,
    next_offset: i64,
    buffer: VecDeque<MessageListItem>,
    exhausted: bool,
}

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
        let page = query_messages_page(
            &self.conn,
            None,
            false,
            criteria,
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
    })
}

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
        total_count += count;
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

#[tauri::command]
async fn list_conversations(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    scopes: Vec<ConversationWorkspaceScope>,
    query: Option<String>,
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
    let criteria = MessageSearchCriteria::from_inputs(query, search_filters)?;
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

#[tauri::command]
async fn count_conversations(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    scopes: Vec<ConversationWorkspaceScope>,
    query: Option<String>,
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
    let criteria = MessageSearchCriteria::from_inputs(query, search_filters)?;
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
        let page = query_conversation_summaries_page(
            &self.conn,
            &self.active,
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
    for (scope, active) in resolved_scopes {
        operation.check_cancelled()?;
        let conn = open_workspace_db_for_search(&active.path, operation)?;
        if !conversation_data_is_indexed(&conn)? {
            unindexed_workspaces.push(conversation_workspace_issue(&active));
            continue;
        }
        cursors.push(ConversationCursor {
            active,
            conn,
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
        let (workspace_conversations, workspace_messages) = conversation_counts(
            &conn,
            scope.folder_id,
            scope.include_subfolders,
            criteria,
            Some(operation),
        )?;
        total_count += workspace_conversations;
        matching_message_count += workspace_messages;
        operation.check_cancelled()?;
    }
    Ok(ConversationCountResult {
        total_count,
        matching_message_count,
    })
}

#[tauri::command]
async fn get_conversation_messages(
    window: tauri::WebviewWindow,
    state: State<'_, AppState>,
    workspace_id: String,
    conversation_id: String,
    folder_id: Option<i64>,
    include_subfolders: bool,
    query: Option<String>,
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
    let criteria = MessageSearchCriteria::from_inputs(query, search_filters)?;
    let active = active_workspace_for_id(&state, &workspace_id)?;
    run_search_worker(operation, move |operation| {
        let conn = open_workspace_db_for_search(&active.path, operation)?;
        if !conversation_data_is_indexed(&conn)? {
            return Err(AppError::new(
                "Conversation data is not indexed for this workspace.",
            ));
        }
        let source = build_message_query_source(&conn, folder_id, include_subfolders, &criteria)?;
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
        let total_message_count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM messages WHERE conversation_id = ?1",
            params![conversation_id],
            |row| row.get(0),
        )?;
        operation.check_cancelled()?;

        let page_limit = limit
            .unwrap_or(CONVERSATION_MESSAGE_PAGE_SIZE)
            .clamp(1, MAX_MESSAGE_PAGE_SIZE);
        let page_offset = offset.unwrap_or(0).max(0);
        let mut items = if show_entire_conversation {
            query_entire_conversation_page(
                &conn,
                &source,
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
    let workspace = resolve_workspace_for_id(&state, &workspace_id)?;
    let conn = open_workspace_db_for_read(&workspace)?;

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
                attachments: Vec::new(),
            })
        })
        .optional()?
        .ok_or_else(|| AppError::new("Message was not found in this workspace."))?;

    let body_html_schema_current =
        metadata_value(&conn, "body_html_schema_version")?.as_deref() == Some("body-html-v1");
    detail.can_reindex_from_eml = !detail.body_html_available
        && !body_html_schema_current
        && workspace.join("extracted").join(&detail.eml_path).is_file();
    detail.attachments = list_attachments(&conn, message_id)?;
    Ok(detail)
}

#[tauri::command]
fn get_message_diagnostics(
    state: State<AppState>,
    workspace_id: String,
    message_id: i64,
) -> AppResult<MessageDiagnostics> {
    let source = resolve_source_eml(&state, &workspace_id, message_id)?;
    let conn = open_workspace_db_for_read(&source.workspace_root)?;
    let (
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
    ): (
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        String,
        Option<i64>,
        Option<i64>,
        String,
    ) = conn
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
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                    row.get(12)?,
                ))
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

    let mut bytes = Vec::new();
    File::open(&source.source_path)?.read_to_end(&mut bytes)?;
    match mailparse::parse_mail(&bytes) {
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
    allow_remote_images: bool,
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
    let render = sanitize_email_html(&body_html, cid_images, allow_remote_images);

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
    allow_remote_resources: bool,
) -> AppResult<SourceEmlView> {
    let source = resolve_source_eml(&state, &workspace_id, message_id)?;
    let conn = open_workspace_db_for_read(&source.workspace_root)?;
    let attachments = list_attachments(&conn, message_id)?;
    source_eml_view_from_path(
        &source.source_path,
        source.relative_eml_path,
        "workspace",
        message_id,
        allow_remote_resources,
        Some(attachments),
    )
}

#[tauri::command]
fn get_standalone_message_view(
    path: String,
    allow_remote_resources: bool,
) -> AppResult<SourceEmlView> {
    let source_path = canonical_standalone_message_path(path)?;
    let source_name = source_path
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("message")
        .to_string();
    source_eml_view_from_path(
        &source_path,
        source_name,
        "standalone",
        0,
        allow_remote_resources,
        None,
    )
}

fn source_eml_view_from_path(
    source_path: &Path,
    eml_path: String,
    source_kind: &str,
    message_id: i64,
    allow_remote_resources: bool,
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
        sanitize_email_html(
            &message.body_html,
            message.cid_images.clone(),
            allow_remote_resources,
        )
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
    })
}

fn standalone_message_from_path(source_path: &Path) -> AppResult<StandaloneMessage> {
    match StandaloneSourceFormat::from_path(source_path)? {
        StandaloneSourceFormat::Eml => standalone_eml_message(source_path),
        StandaloneSourceFormat::Msg => standalone_msg_message(source_path),
    }
}

fn standalone_eml_message(source_path: &Path) -> AppResult<StandaloneMessage> {
    let mut bytes = Vec::new();
    File::open(source_path)?.read_to_end(&mut bytes)?;
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
    let file = File::open(source_path)?;
    let outlook = msg_parser::Outlook::from_reader(file)
        .map_err(|error| AppError::new(format!("Could not parse Outlook MSG: {error}")))?;
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

    let raw_source = msg_structured_raw_source(
        source_path,
        &outlook,
        &body_html,
        &body_text,
        body_source,
        &selected_rtf_source,
        native_rtf_present,
        !body_like_rtf_attachment_indexes.is_empty(),
        promoted_rtf_index,
        !body_html.is_empty() && body_source == BodySource::RtfHtmlConverted,
        &unresolved_cids,
        &parse_warnings,
        &attachment_diagnostics,
        calendar.as_ref(),
    );
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

fn msg_structured_raw_source(
    source_path: &Path,
    outlook: &msg_parser::Outlook,
    body_html: &str,
    body_text: &str,
    body_source: BodySource,
    selected_body_source: &str,
    native_rtf_present: bool,
    body_like_rtf_attachment_detected: bool,
    promoted_rtf_index: Option<usize>,
    rtf_html_recovered: bool,
    unresolved_cids: &[String],
    parse_warnings: &[String],
    attachments: &[MsgAttachmentDiagnostic],
    calendar: Option<&CalendarItemDetails>,
) -> String {
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
                    &format!(
                        "EML export failed: message_id={message_id} error={}",
                        error.message
                    ),
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
                    &format!(
                        "Save source EML failed: message_id={message_id} error={}",
                        error.message
                    ),
                );
            }
            Ok(ExportOriginalEmlResult::failed(message_id, error.message))
        }
    }
}

#[tauri::command]
fn save_standalone_source_message_as(path: String) -> AppResult<ExportOriginalEmlResult> {
    match save_standalone_source_message_as_inner(path) {
        Ok(result) => Ok(result),
        Err(error) => Ok(ExportOriginalEmlResult::failed(0, error.message)),
    }
}

#[tauri::command]
fn save_printable_html_as(
    default_filename: String,
    html: String,
) -> AppResult<SavePrintableHtmlResult> {
    let default_filename = sanitize_printable_html_filename(&default_filename);
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

    let mut output = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&output_path)?;
    output.write_all(html.as_bytes())?;
    output.flush()?;

    let output_path = output_path.canonicalize()?;
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
fn reveal_saved_html(output_path: String) -> AppResult<()> {
    let path = PathBuf::from(output_path);
    if !path.exists() {
        return Err(AppError::new(format!(
            "Saved HTML does not exist: {}",
            path.display()
        )));
    }
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
fn reveal_saved_eml(output_path: String) -> AppResult<()> {
    let path = PathBuf::from(output_path);
    if !path.exists() {
        return Err(AppError::new(format!(
            "Saved source message does not exist: {}",
            path.display()
        )));
    }
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
                        "Export failed: message_id={message_id} attachment_id={attachment_id} error={}",
                        error.message
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
                        "Open attachment failed: message_id={message_id} attachment_id={attachment_id} error={}",
                        error.message
                    ),
                );
            }
            Ok(ExportAttachmentResult::failed(attachment_id, error.message))
        }
    }
}

#[tauri::command]
fn export_standalone_message_attachment(
    path: String,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    match export_standalone_message_attachment_inner(path, attachment_id) {
        Ok(result) => Ok(result),
        Err(error) => Ok(ExportAttachmentResult::failed(attachment_id, error.message)),
    }
}

#[tauri::command]
fn open_standalone_message_attachment(
    path: String,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    match open_standalone_message_attachment_inner(path, attachment_id) {
        Ok(result) => Ok(result),
        Err(error) => Ok(ExportAttachmentResult::failed(attachment_id, error.message)),
    }
}

#[tauri::command]
fn reveal_exported_file(state: State<AppState>, output_path: String) -> AppResult<()> {
    let active = active_workspace(&state)?;
    reveal_exported_file_in_workspace(&active.path, &output_path)
}

#[tauri::command]
fn reveal_exported_file_for_workspace(
    state: State<AppState>,
    workspace_id: String,
    output_path: String,
) -> AppResult<()> {
    let active = active_workspace_for_id(&state, &workspace_id)?;
    reveal_exported_file_in_workspace(&active.path, &output_path)
}

#[tauri::command]
fn reveal_standalone_exported_file(output_path: String) -> AppResult<()> {
    let path = PathBuf::from(output_path);
    if !path.exists() {
        return Err(AppError::new(format!(
            "Exported file does not exist: {}",
            path.display()
        )));
    }
    if !path_is_under_any_standalone_export_root(&path)? {
        return Err(AppError::new(format!(
            "Refusing to reveal a file outside the standalone message export folders: {}",
            path.display()
        )));
    }
    reveal_path(&path)
}

fn reveal_exported_file_in_workspace(workspace: &Path, output_path: &str) -> AppResult<()> {
    let workspace_root = workspace.canonicalize()?;
    if !workspace_root.join(WORKSPACE_MARKER_FILE).is_file() {
        return Err(AppError::new(format!(
            "Active workspace marker was not found: {}",
            workspace_root.join(WORKSPACE_MARKER_FILE).display()
        )));
    }
    let path = PathBuf::from(output_path);
    let exports_root = workspace_root.join("exports");
    if !path.exists() {
        return Err(AppError::new(format!(
            "Exported file does not exist: {}",
            path.display()
        )));
    }
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
    let path = PathBuf::from(output_path).canonicalize()?;

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
    path: String,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    let source_path = canonical_standalone_message_path(path)?;
    if StandaloneSourceFormat::from_path(&source_path)? == StandaloneSourceFormat::Msg {
        let file = File::open(&source_path)?;
        let outlook = msg_parser::Outlook::from_reader(file)
            .map_err(|error| AppError::new(format!("Could not parse Outlook MSG: {error}")))?;
        let attachment = msg_attachment_by_id(&outlook, attachment_id)?;
        if attachment.attach_method == 6 {
            return Err(AppError::new(
                "OLE object attachments can be exported but are not opened automatically.",
            ));
        }
    }

    let result = export_standalone_message_attachment_from_path(&source_path, attachment_id)?;
    let output_path = result
        .output_path
        .as_deref()
        .ok_or_else(|| AppError::new("Attachment export did not produce an output path."))?;
    let path = PathBuf::from(output_path).canonicalize()?;
    if !path_is_under_any_standalone_export_root(&path)? {
        return Err(AppError::new(format!(
            "Refusing to open a file outside the standalone message export folders: {}",
            path.display()
        )));
    }

    open_path_with_default_app(&path)?;
    Ok(result)
}

fn export_standalone_message_attachment_inner(
    path: String,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    let source_path = canonical_standalone_message_path(path)?;
    export_standalone_message_attachment_from_path(&source_path, attachment_id)
}

fn export_standalone_message_attachment_from_path(
    source_path: &Path,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    match StandaloneSourceFormat::from_path(source_path)? {
        StandaloneSourceFormat::Eml => {
            export_standalone_eml_attachment_from_path(source_path, attachment_id)
        }
        StandaloneSourceFormat::Msg => {
            export_standalone_msg_attachment_from_path(source_path, attachment_id)
        }
    }
}

fn export_standalone_eml_attachment_from_path(
    source_path: &Path,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    let parsed_message = parse_eml(&source_path)?;
    let attachment = parsed_message
        .attachments
        .iter()
        .find(|attachment| attachment.attachment_index + 1 == attachment_id)
        .ok_or_else(|| AppError::new("Attachment was not found in this EML."))?;

    let mut bytes = Vec::new();
    File::open(source_path)?.read_to_end(&mut bytes)?;
    let parsed = mailparse::parse_mail(&bytes).map_err(|error| AppError::new(error.to_string()))?;
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

    let export_dir = standalone_message_export_root(source_path)?.join("attachments");
    fs::create_dir_all(&export_dir)?;
    let export_dir = export_dir.canonicalize()?;
    let output_path = unique_export_path(&export_dir, &sanitized_filename)?;
    if !path_parent_is_root(&output_path, &export_dir)? {
        return Err(AppError::new(
            "Refusing to write outside the standalone EML attachment export folder.",
        ));
    }

    let mut output = File::create(&output_path)?;
    output.write_all(&decoded)?;
    output.flush()?;
    let output_path = output_path.canonicalize()?;
    if !path_is_under_root(
        &output_path,
        &standalone_exports_dir(StandaloneSourceFormat::Eml)?,
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
    source_path: &Path,
    attachment_id: i64,
) -> AppResult<ExportAttachmentResult> {
    let file = File::open(source_path)?;
    let outlook = msg_parser::Outlook::from_reader(file)
        .map_err(|error| AppError::new(format!("Could not parse Outlook MSG: {error}")))?;
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

    let export_dir = standalone_message_export_root(source_path)?.join("attachments");
    fs::create_dir_all(&export_dir)?;
    let export_dir = export_dir.canonicalize()?;
    let output_path = unique_export_path(&export_dir, &sanitized_filename)?;
    if !path_parent_is_root(&output_path, &export_dir)? {
        return Err(AppError::new(
            "Refusing to write outside the standalone MSG attachment export folder.",
        ));
    }

    let mut output = File::create(&output_path)?;
    output.write_all(&attachment.payload_bytes)?;
    output.flush()?;
    let output_path = output_path.canonicalize()?;
    if !path_is_under_root(
        &output_path,
        &standalone_exports_dir(StandaloneSourceFormat::Msg)?,
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

fn msg_attachment_by_id<'a>(
    outlook: &'a msg_parser::Outlook,
    attachment_id: i64,
) -> AppResult<&'a msg_parser::Attachment> {
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

    fn export_root_name(self) -> &'static str {
        match self {
            Self::Eml => "eml-exports",
            Self::Msg => "msg-exports",
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
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output_path)?;
    std::io::copy(&mut input, &mut output)?;
    output.flush()?;

    let output_path = output_path.canonicalize()?;
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

fn save_standalone_source_message_as_inner(path: String) -> AppResult<ExportOriginalEmlResult> {
    let source_path = canonical_standalone_message_path(path)?;
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
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output_path)?;
    std::io::copy(&mut input, &mut output)?;
    output.flush()?;

    let output_path = output_path.canonicalize()?;
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
    fs::create_dir_all(&export_dir)?;
    let export_dir = export_dir.canonicalize()?;
    let filename = export_message_eml_filename(message_id, &date, &subject);
    let output_path = unique_export_path(&export_dir, &filename)?;
    if !path_parent_is_root(&output_path, &export_dir)? {
        return Err(AppError::new(
            "Refusing to write outside the message export folder.",
        ));
    }

    append_export_log(
        &workspace_root,
        &format!("EML export requested: message_id={message_id}"),
    )?;

    fs::copy(&source_eml_path, &output_path)?;
    let output_path = output_path.canonicalize()?;
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

    let mut bytes = Vec::new();
    File::open(&eml_path)?.read_to_end(&mut bytes)?;
    let parsed = mailparse::parse_mail(&bytes).map_err(|error| AppError::new(error.to_string()))?;
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
    fs::create_dir_all(&export_dir)?;
    let output_path = unique_export_path(&export_dir, &sanitized_filename)?;
    if !path_parent_is_root(&output_path, &export_dir)? {
        return Err(AppError::new(
            "Refusing to write outside the message export folder.",
        ));
    }

    let mut output = File::create(&output_path)?;
    output.write_all(&decoded)?;
    output.flush()?;

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
    let Some(location_mode) = workspace_location_for_identity(&requested_path, &identity)? else {
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

    let candidates = find_existing_workspaces(&identity)?;
    let candidate = select_existing_workspace(&candidates, &requested_path)?;
    if !candidate.path.join(WORKSPACE_MARKER_FILE).exists() {
        ensure_workspace_marker(
            &candidate.path,
            &candidate.workspace_id,
            &identity,
            candidate.mode,
        )?;
    }
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

    if let Err(error) = validate_workspace_for_delete(&active) {
        result.error = Some(error.message);
        result.exists_after = active.path.exists();
        result.remaining_entries = remaining_entries(&active.path);
        result.message = delete_result_message(&result);
        return Ok(result);
    }

    if let Err(error) = close_workspace_db_before_delete(&active.path) {
        result.error = Some(format!(
            "Failed to close SQLite before deleting {}: {}",
            active.path.display(),
            error.message
        ));
        result.exists_after = active.path.exists();
        result.remaining_entries = remaining_entries(&active.path);
        result.message = delete_result_message(&result);
        return Ok(result);
    }

    if let Err(error) = fs::remove_dir_all(&active.path) {
        result.error = Some(format!(
            "Failed to delete workspace {}: {}",
            active.path.display(),
            error
        ));
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
    let _ = append_application_log(
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
) -> AppResult<WorkspaceSummary> {
    let readpst = find_readpst().ok_or_else(AppError::missing_readpst)?;
    let identity = identify_pst(&pst_path)?;
    let selection = select_workspace(&identity, requested_mode)?;
    let existing_workspaces = find_existing_workspaces(&identity)?;

    if let Some(existing_workspace_path) = existing_workspace_path {
        let existing_workspace =
            select_existing_workspace(&existing_workspaces, &existing_workspace_path)?;
        match action {
            ImportAction::OpenExisting => {
                let conn = Connection::open(existing_workspace.path.join("index.sqlite"))?;
                initialize_schema(&conn)?;
                upsert_current_fingerprint_metadata(
                    &conn,
                    &identity,
                    &existing_workspace.path,
                    existing_workspace.mode,
                )?;
                ensure_workspace_marker(
                    &existing_workspace.path,
                    &existing_workspace.workspace_id,
                    &identity,
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
                    &existing_workspace.path,
                    existing_workspace.mode,
                );
            }
            ImportAction::Reimport => {
                return import_workspace_from_pst(
                    &app,
                    &readpst.path,
                    &identity,
                    existing_workspace.path,
                    existing_workspace.mode,
                    true,
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
        &readpst.path,
        &identity,
        workspace,
        workspace_mode,
        false,
    )
}

fn import_workspace_from_pst(
    app: &tauri::AppHandle,
    readpst: &Path,
    identity: &PstIdentity,
    workspace: PathBuf,
    workspace_mode: WorkspaceLocationMode,
    force_reimport: bool,
) -> AppResult<WorkspaceSummary> {
    let eml_dir = workspace.join("extracted");
    let index_path = workspace.join("index.sqlite");
    let readpst_path = readpst.display().to_string();
    let readpst_version_text = readpst_version(readpst);

    fs::create_dir_all(workspace.join("logs"))?;
    ensure_workspace_marker(
        &workspace,
        &identity.workspace_id,
        &identity,
        workspace_mode,
    )?;
    initialize_import_log(&workspace, &readpst_path, &readpst_version_text, identity)?;
    mark_workspace_status(
        &workspace,
        identity,
        workspace_mode,
        ImportStatus::Pending,
        None,
        None,
        None,
    )?;
    set_workspace_readpst_metadata(&workspace, &readpst_path, &readpst_version_text)?;

    emit_progress(
        &app,
        "Checking PST",
        None,
        None,
        "Checking PST and workspace requirements.",
    );

    warn_about_storage(&app, &identity, &workspace)?;

    let stale_extracting_dir = workspace.join("extracting");
    let temp_index = workspace.join("index.tmp.sqlite");
    if stale_extracting_dir.exists() {
        fs::remove_dir_all(&stale_extracting_dir)?;
    }
    if temp_index.exists() {
        fs::remove_file(&temp_index)?;
    }
    if force_reimport && eml_dir.exists() {
        fs::remove_dir_all(&eml_dir)?;
    }
    fs::create_dir_all(&eml_dir)?;

    emit_progress(
        &app,
        "Preparing workspace",
        None,
        None,
        "Preparing the local workspace.",
    );

    let import_result = (|| {
        mark_workspace_status(
            &workspace,
            identity,
            workspace_mode,
            ImportStatus::Running,
            None,
            Some(0),
            Some(0),
        )?;
        run_readpst(app, readpst, &identity.canonical_path, &workspace, &eml_dir)?;
        let stats = index_eml_files(
            app,
            identity,
            &eml_dir,
            &temp_index,
            &workspace,
            workspace_mode,
            &readpst_path,
            &readpst_version_text,
        )?;

        emit_progress(
            app,
            "Preparing workspace",
            None,
            None,
            "Moving the completed local index into place.",
        );

        if index_path.exists() {
            fs::remove_file(&index_path)?;
        }
        fs::rename(&temp_index, &index_path)?;

        mark_workspace_status(
            &workspace,
            identity,
            workspace_mode,
            ImportStatus::Complete,
            stats.last_error.as_deref(),
            Some(stats.indexed),
            Some(stats.error_count),
        )?;

        Ok::<IndexStats, AppError>(stats)
    })();

    let stats = match import_result {
        Ok(stats) => stats,
        Err(error) => {
            if !workspace.is_dir() {
                return Err(error);
            }
            let cancelled = app
                .state::<AppState>()
                .cancel_import_requested
                .load(Ordering::SeqCst);
            let status = if cancelled {
                ImportStatus::Cancelled
            } else {
                ImportStatus::Failed
            };
            let _ = mark_workspace_status(
                &workspace,
                identity,
                workspace_mode,
                status,
                Some(&error.message),
                None,
                None,
            );
            let _ = append_import_log(
                &workspace,
                &format!("Import ended as {}: {}", status.as_str(), error.message),
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
    emit_progress(
        app,
        "Complete",
        Some(stats.indexed),
        Some(stats.discovered),
        "Import and indexing complete.",
    );

    Ok(summary)
}

fn resume_index_existing_workspace(
    app: &tauri::AppHandle,
    identity: &PstIdentity,
    workspace: &Path,
    workspace_mode: WorkspaceLocationMode,
) -> AppResult<WorkspaceSummary> {
    let extract_dir = workspace.join("extracted");
    if !extract_dir.exists() {
        return Err(AppError::new(format!(
            "Cannot resume indexing because extracted EML files were not found: {}",
            extract_dir.display()
        )));
    }

    initialize_import_log(
        workspace,
        "readpst not run during resume",
        "readpst not run during resume",
        identity,
    )?;
    mark_workspace_status(
        workspace,
        identity,
        workspace_mode,
        ImportStatus::Running,
        None,
        Some(0),
        Some(0),
    )?;
    set_workspace_readpst_metadata(
        workspace,
        "readpst not run during resume",
        "readpst not run during resume",
    )?;

    let temp_index = workspace.join("index.tmp.sqlite");
    if temp_index.exists() {
        fs::remove_file(&temp_index)?;
    }

    let result = index_eml_files(
        app,
        identity,
        &extract_dir,
        &temp_index,
        workspace,
        workspace_mode,
        "readpst not run during resume",
        "readpst not run during resume",
    );

    let stats = match result {
        Ok(stats) => stats,
        Err(error) => {
            if !workspace.is_dir() {
                return Err(error);
            }
            let cancelled = app
                .state::<AppState>()
                .cancel_import_requested
                .load(Ordering::SeqCst);
            let status = if cancelled {
                ImportStatus::Cancelled
            } else {
                ImportStatus::Failed
            };
            let _ = mark_workspace_status(
                workspace,
                identity,
                workspace_mode,
                status,
                Some(&error.message),
                None,
                None,
            );
            return Err(error);
        }
    };

    let index_path = workspace.join("index.sqlite");
    if index_path.exists() {
        fs::remove_file(&index_path)?;
    }
    fs::rename(&temp_index, &index_path)?;
    mark_workspace_status(
        workspace,
        identity,
        workspace_mode,
        ImportStatus::Complete,
        stats.last_error.as_deref(),
        Some(stats.indexed),
        Some(stats.error_count),
    )?;

    let summary = workspace_summary(
        identity,
        &identity.workspace_id,
        workspace,
        workspace_mode,
        true,
    )?;
    emit_progress(
        app,
        "Complete",
        Some(stats.indexed),
        Some(stats.discovered),
        "Resume indexing complete.",
    );

    Ok(summary)
}

fn reindex_existing_emls_blocking(
    app: &tauri::AppHandle,
    active: ActiveWorkspace,
) -> AppResult<WorkspaceSummary> {
    if !active.path.is_dir() {
        return Err(AppError::new(format!(
            "Workspace is unavailable: {}",
            active.path.display()
        )));
    }
    let marker = active.path.join(WORKSPACE_MARKER_FILE);
    if !marker.exists() {
        return Err(AppError::new(format!(
            "Refusing to reindex {} because it is missing {}.",
            active.path.display(),
            WORKSPACE_MARKER_FILE
        )));
    }

    let extract_dir = active.path.join("extracted");
    if !extract_dir.is_dir() {
        return Err(AppError::new(format!(
            "Cannot reindex because extracted EML files were not found: {}",
            extract_dir.display()
        )));
    }

    let index_path = active.path.join("index.sqlite");
    if !index_path.exists() {
        return Err(AppError::new(format!(
            "Cannot reindex because the current SQLite index was not found: {}",
            index_path.display()
        )));
    }

    append_import_log(
        &active.path,
        "Reindex existing EMLs requested. readpst will not be run.",
    )?;

    let existing_metadata = {
        let conn = Connection::open(&index_path)?;
        initialize_schema(&conn)?;
        read_import_metadata(&conn)?
    };

    let temp_index = active.path.join("index.reindex.tmp.sqlite");
    remove_sqlite_file_set(&temp_index)?;

    let result =
        reindex_eml_files_to_index(app, &active, &extract_dir, &temp_index, existing_metadata);

    let stats = match result {
        Ok(stats) => stats,
        Err(error) => {
            let _ = remove_sqlite_file_set(&temp_index);
            let cancelled = app
                .state::<AppState>()
                .cancel_import_requested
                .load(Ordering::SeqCst);
            let status = if cancelled {
                ImportStatus::Cancelled.as_str()
            } else {
                ImportStatus::Failed.as_str()
            };
            let _ = append_import_log(
                &active.path,
                &format!("Reindex existing EMLs ended as {status}: {}", error.message),
            );
            return Err(error);
        }
    };

    close_workspace_db_before_delete(&active.path)?;
    remove_sqlite_sidecars(&index_path)?;
    fs::rename(&temp_index, &index_path)?;
    remove_sqlite_sidecars(&temp_index)?;
    update_workspace_metadata_after_reindex(&active, &stats)?;

    emit_progress(
        app,
        "Complete",
        Some(stats.indexed),
        Some(stats.discovered),
        "Reindex existing EMLs complete.",
    );
    append_import_log(
        &active.path,
        &format!(
            "Reindex existing EMLs complete. indexed={} discovered={} errors={}",
            stats.indexed, stats.discovered, stats.error_count
        ),
    )?;

    workspace_summary_from_active(&active, true)
}

fn find_readpst() -> Option<ReadpstLocation> {
    if let Some(location) = find_bundled_readpst() {
        return Some(location);
    }

    for candidate in [
        PathBuf::from("/usr/local/bin/readpst"),
        PathBuf::from("/opt/homebrew/bin/readpst"),
    ] {
        if is_executable_file(&candidate) {
            return Some(ReadpstLocation {
                path: candidate,
                source: ReadpstSource::System,
            });
        }
    }

    let path_value = env::var_os("PATH")?;
    for directory in env::split_paths(&path_value) {
        let candidate = directory.join("readpst");
        if is_executable_file(&candidate) {
            return Some(ReadpstLocation {
                path: candidate,
                source: ReadpstSource::System,
            });
        }
    }

    None
}

fn find_bundled_readpst() -> Option<ReadpstLocation> {
    for (name, source) in bundled_readpst_names() {
        for directory in bundled_readpst_search_dirs() {
            let candidate = directory.join(name);
            if is_executable_file(&candidate) {
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
        current_arch_source = ReadpstSource::BundledAppleSilicon;
        names.push(("readpst-aarch64-apple-darwin", current_arch_source));
    } else {
        current_arch_source = ReadpstSource::BundledIntel;
        names.push(("readpst-x86_64-apple-darwin", current_arch_source));
    }
    names.push((
        "readpst-universal-apple-darwin",
        ReadpstSource::BundledUniversal,
    ));
    names.push(("readpst", current_arch_source));
    names
}

fn classify_bundled_readpst_source(path: &Path, fallback: ReadpstSource) -> ReadpstSource {
    if fallback == ReadpstSource::BundledUniversal || is_macho_universal_binary(path) {
        ReadpstSource::BundledUniversal
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

    if let Ok(current_dir) = env::current_dir() {
        dirs.push(current_dir.join("src-tauri").join("binaries"));
        dirs.push(current_dir.join("binaries"));
    }

    dirs
}

fn is_executable_file(path: &Path) -> bool {
    path.is_file()
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

    let canonical_path = path
        .canonicalize()
        .map_err(|_| AppError::new("The selected PST does not exist or cannot be read."))?;
    let metadata = fs::metadata(&canonical_path)?;
    if !metadata.is_file() {
        return Err(AppError::new("The selected path is not a file."));
    }
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

fn app_support_dir() -> AppResult<PathBuf> {
    let home = env::var_os("HOME").ok_or_else(|| AppError::new("Could not locate HOME."))?;
    Ok(PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join(APP_SUPPORT_NAME))
}

fn application_logs_dir() -> AppResult<PathBuf> {
    Ok(app_support_dir()?.join("logs"))
}

fn application_log_path() -> AppResult<PathBuf> {
    Ok(application_logs_dir()?.join(APPLICATION_LOG_FILE))
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
    let Ok(conn) = Connection::open_with_flags(&database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
    else {
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

fn workspaces_dir() -> AppResult<PathBuf> {
    let directory = app_support_dir()?.join("workspaces");
    fs::create_dir_all(&directory)?;
    Ok(directory)
}

fn standalone_exports_dir(format: StandaloneSourceFormat) -> AppResult<PathBuf> {
    let directory = app_support_dir()?.join(format.export_root_name());
    fs::create_dir_all(&directory)?;
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
            eprintln!("Could not deliver Finder file-open event: {error}");
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
        eprintln!("Could not unminimize the main window: {error}");
    }
    if let Err(error) = window.show() {
        eprintln!("Could not show the main window: {error}");
    }
    if let Err(error) = window.set_focus() {
        eprintln!("Could not focus the main window: {error}");
    }
}

fn canonical_standalone_message_path(path: String) -> AppResult<PathBuf> {
    let path = PathBuf::from(path);
    let path = path
        .canonicalize()
        .map_err(|error| AppError::new(format!("Message file is not available: {error}")))?;
    if !path.is_file() {
        return Err(AppError::new(format!(
            "Selected message path is not a file: {}",
            path.display()
        )));
    }
    let metadata = path.metadata().map_err(|error| {
        AppError::new(format!(
            "Could not inspect message file {}: {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file() {
        return Err(AppError::new(format!(
            "Selected message path is not a regular file: {}",
            path.display()
        )));
    }
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

fn path_is_under_any_standalone_export_root(path: &Path) -> AppResult<bool> {
    for format in [StandaloneSourceFormat::Eml, StandaloneSourceFormat::Msg] {
        if path_is_under_root(path, &standalone_exports_dir(format)?)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn standalone_message_export_root(source_path: &Path) -> AppResult<PathBuf> {
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
    let root = standalone_exports_dir(format)?.join(sha256_text(&key));
    fs::create_dir_all(&root)?;
    Ok(root)
}

fn select_workspace(
    identity: &PstIdentity,
    requested_mode: WorkspaceLocationMode,
) -> AppResult<WorkspaceSelection> {
    match requested_mode {
        WorkspaceLocationMode::AppSupport => Ok(WorkspaceSelection {
            path: workspaces_dir()?.join(&identity.workspace_id),
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
                path: workspaces_dir()?.join(&identity.workspace_id),
                mode: WorkspaceLocationMode::AppSupport,
                warning: Some(format!(
                    "The PST folder is not writable, so the workspace will use App Support instead of {}.",
                    root.display()
                )),
            }),
            None => Ok(WorkspaceSelection {
                path: workspaces_dir()?.join(&identity.workspace_id),
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

    let test_path = parent.join(format!(".pst-quickview-write-test-{}", unix_timestamp()));
    match File::create(&test_path) {
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

    let test_path = test_dir.join(format!(
        ".pst-quickview-preflight-{}-{}",
        unix_timestamp(),
        std::process::id()
    ));

    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&test_path)
    {
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

fn workspace_path_for_id(workspace_id: &str) -> AppResult<PathBuf> {
    if workspace_id.len() != 64 || !workspace_id.chars().all(|char| char.is_ascii_hexdigit()) {
        return Err(AppError::new("Invalid workspace id."));
    }

    Ok(workspaces_dir()?.join(workspace_id))
}

fn validate_session_workspace_path(
    pst_path: &Path,
    workspace_path: &Path,
    workspace_id: &str,
    location_mode: WorkspaceLocationMode,
) -> AppResult<()> {
    if !pst_path.is_file() {
        return Err(AppError::new("PST or workspace not available."));
    }
    if !workspace_path.is_dir() {
        return Err(AppError::new("PST or workspace not available."));
    }
    if workspace_id_from_path(workspace_path)? != workspace_id {
        return Err(AppError::new(
            "Saved workspace id does not match the workspace path.",
        ));
    }

    let marker = workspace_path.join(WORKSPACE_MARKER_FILE);
    if !marker.is_file() {
        return Err(AppError::new("Saved workspace marker was not found."));
    }
    let marker_contents = fs::read_to_string(&marker).unwrap_or_default();
    if !marker_contents.contains("app=PST QuickView") {
        return Err(AppError::new("Saved workspace marker is not valid."));
    }

    match location_mode {
        WorkspaceLocationMode::AppSupport => {
            let root = workspaces_dir()?;
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

    Ok(())
}

fn set_active_workspace(state: &State<'_, AppState>, summary: &WorkspaceSummary) -> AppResult<()> {
    let mode = WorkspaceLocationMode::from_arg(&summary.workspace_location_mode)?;
    let workspace_path = PathBuf::from(&summary.workspace_path);
    let workspace_path = workspace_path.canonicalize().unwrap_or(workspace_path);
    let pst_path = PathBuf::from(&summary.pst_path);
    let pst_path = pst_path.canonicalize().unwrap_or(pst_path);
    let active = ActiveWorkspace {
        id: summary.id.clone(),
        path: workspace_path,
        pst_path,
        fingerprint: summary.fingerprint.clone(),
        location_mode: mode,
    };

    {
        let mut open_guard = state
            .open_workspaces
            .lock()
            .map_err(|_| AppError::new("Could not lock open workspace state."))?;
        open_guard.insert(active.id.clone(), active.clone());
    }

    let mut guard = state
        .active_workspace
        .lock()
        .map_err(|_| AppError::new("Could not lock active workspace state."))?;
    *guard = Some(active);

    Ok(())
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

    workspace_path_for_id(workspace_id)
}

fn workspace_location_for_identity(
    path: &Path,
    identity: &PstIdentity,
) -> AppResult<Option<WorkspaceLocationMode>> {
    let app_support_root = workspaces_dir()?;
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
    if !db_path.exists() {
        return Err(AppError::new("This workspace does not have an index yet."));
    }

    let conn = Connection::open(db_path)?;
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
    if !db_path.is_file() {
        return Err(AppError::new("This workspace does not have an index yet."));
    }

    // Do not use immutable=1: active WAL workspaces must remain visible to readers.
    let conn = Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    Ok(conn)
}

fn configure_workspace_db_for_read(conn: &Connection) -> AppResult<()> {
    conn.execute_batch("PRAGMA query_only = ON; PRAGMA foreign_keys = ON;")?;
    verify_read_query_schema(&conn)?;
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
    let metadata_path = workspace.join(WORKSPACE_METADATA_FILE);
    let contents = fs::read_to_string(metadata_path).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(&contents).ok()?;
    value
        .get("import_status")
        .or_else(|| value.get("importStatus"))
        .and_then(|status| status.as_str())
        .map(str::to_string)
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
    fs::create_dir_all(workspace)?;
    let now = Utc::now().to_rfc3339();
    let metadata_path = workspace.join(WORKSPACE_METADATA_FILE);
    let existing_metadata = fs::read_to_string(&metadata_path)
        .ok()
        .and_then(|contents| serde_json::from_str::<serde_json::Value>(&contents).ok());
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
    fs::write(
        &metadata_path,
        serde_json::to_string_pretty(&metadata)
            .map_err(|error| AppError::new(error.to_string()))?,
    )?;

    let index_path = workspace.join("index.sqlite");
    if index_path.exists() {
        let conn = Connection::open(index_path)?;
        initialize_schema(&conn)?;
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

    fs::write(
        metadata_path,
        serde_json::to_string_pretty(&value).map_err(|error| AppError::new(error.to_string()))?,
    )?;
    Ok(())
}

fn initialize_import_log(
    workspace: &Path,
    readpst_path: &str,
    readpst_version: &str,
    identity: &PstIdentity,
) -> AppResult<()> {
    fs::create_dir_all(workspace.join("logs"))?;
    let log_path = import_log_path(workspace);
    rotate_existing_log(&log_path, LOG_BACKUP_COUNT)?;
    let mut log = File::create(&log_path)?;
    writeln!(log, "PST QuickView import log")?;
    writeln!(log, "started_at={}", Utc::now().to_rfc3339())?;
    writeln!(log, "original_pst_path={}", identity.display_path)?;
    writeln!(
        log,
        "pst_content_fingerprint={}",
        identity.content_fingerprint
    )?;
    writeln!(log, "workspace_path={}", workspace.display())?;
    writeln!(log, "readpst_path={readpst_path}")?;
    writeln!(log, "readpst_version={readpst_version}")?;
    writeln!(log)?;
    Ok(())
}

fn append_import_log(workspace: &Path, message: &str) -> AppResult<()> {
    if !workspace.is_dir() {
        return Err(AppError::new(format!(
            "Workspace was deleted during import: {}",
            workspace.display()
        )));
    }
    let logs_dir = workspace.join("logs");
    if !logs_dir.is_dir() {
        fs::create_dir_all(&logs_dir)?;
    }
    let log_path = import_log_path(workspace);
    rotate_log_if_needed(&log_path, IMPORT_LOG_MAX_BYTES, LOG_BACKUP_COUNT)?;
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    writeln!(log, "[{}] {message}", Utc::now().to_rfc3339())?;
    Ok(())
}

fn append_export_log(workspace: &Path, message: &str) -> AppResult<()> {
    if !workspace.is_dir() {
        return Err(AppError::new(format!(
            "Workspace is unavailable: {}",
            workspace.display()
        )));
    }
    let logs_dir = workspace.join("logs");
    fs::create_dir_all(&logs_dir)?;
    let log_path = logs_dir.join("exports.log");
    rotate_log_if_needed(&log_path, EXPORT_LOG_MAX_BYTES, LOG_BACKUP_COUNT)?;
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    writeln!(log, "[{}] {message}", Utc::now().to_rfc3339())?;
    Ok(())
}

fn append_application_log(
    operation: &str,
    stage: &str,
    workspace_id: Option<&str>,
    error: Option<&str>,
) -> AppResult<()> {
    let logs_dir = application_logs_dir()?;
    fs::create_dir_all(&logs_dir)?;
    let log_path = application_log_path()?;
    rotate_log_if_needed(&log_path, APPLICATION_LOG_MAX_BYTES, LOG_BACKUP_COUNT)?;
    let mut log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    writeln!(
        log,
        "[{}] operation={} stage={} workspace_id={} error={}",
        Utc::now().to_rfc3339(),
        safe_log_value(operation),
        safe_log_value(stage),
        workspace_id
            .map(safe_log_value)
            .unwrap_or_else(|| "-".to_string()),
        error.map(safe_log_value).unwrap_or_else(|| "-".to_string())
    )?;
    Ok(())
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

fn rotate_log_if_needed(path: &Path, max_bytes: u64, backups: usize) -> AppResult<()> {
    if path
        .metadata()
        .map(|metadata| metadata.len() < max_bytes)
        .unwrap_or(true)
    {
        return Ok(());
    }
    rotate_existing_log(path, backups)
}

fn rotate_existing_log(path: &Path, backups: usize) -> AppResult<()> {
    if backups == 0 || !path.exists() {
        return Ok(());
    }
    let oldest = rotated_log_path(path, backups);
    if oldest.exists() {
        fs::remove_file(oldest)?;
    }
    for index in (1..backups).rev() {
        let source = rotated_log_path(path, index);
        if source.exists() {
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

fn import_log_tail(workspace: &Path, max_bytes: u64) -> String {
    let path = import_log_path(workspace);
    let Ok(mut file) = File::open(&path) else {
        return "Import log could not be opened.".to_string();
    };
    let Ok(metadata) = file.metadata() else {
        return "Import log metadata could not be read.".to_string();
    };
    let start = metadata.len().saturating_sub(max_bytes);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return "Import log tail could not be read.".to_string();
    }

    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return "Import log tail could not be read.".to_string();
    }

    let text = String::from_utf8_lossy(&bytes).trim().to_string();
    if text.is_empty() {
        "No readpst output was captured.".to_string()
    } else {
        text
    }
}

fn readpst_version(readpst: &Path) -> String {
    for arg in ["-V", "--version"] {
        if let Ok(output) = Command::new(readpst).arg(arg).output() {
            let combined = [output.stdout, output.stderr].concat();
            let text = String::from_utf8_lossy(&combined).trim().to_string();
            if !text.is_empty() {
                return text.lines().next().unwrap_or("unknown").to_string();
            }
        }
    }
    "unknown".to_string()
}

fn terminate_process(pid: u32, force: bool) -> AppResult<()> {
    let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
    let result = unsafe { libc::kill(pid as libc::pid_t, signal) };
    if result == 0 {
        return Ok(());
    }

    let error = std::io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }

    Err(AppError::new(format!(
        "Could not send signal {signal} to readpst pid {pid}: {error}"
    )))
}

fn find_existing_workspaces(identity: &PstIdentity) -> AppResult<Vec<WorkspaceCandidate>> {
    let mut candidates = Vec::new();
    let app_support_root = workspaces_dir()?;
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
    if !root.is_dir() {
        return Ok(());
    }

    for workspace_id in [&identity.workspace_id, &identity.legacy_workspace_id] {
        let workspace = root.join(workspace_id);
        if !workspace.is_dir() {
            continue;
        }
        if workspace_matches_identity(&workspace, identity)? {
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
        if !workspace.is_dir() {
            continue;
        }

        if workspace_matches_identity(&workspace, identity)? {
            candidates.push(WorkspaceCandidate {
                workspace_id: workspace_id_from_path(&workspace)?,
                path: workspace,
                mode,
            });
        }
    }

    Ok(())
}

fn workspace_matches_identity(workspace: &Path, identity: &PstIdentity) -> AppResult<bool> {
    if !workspace.is_dir() {
        return Ok(false);
    }

    let index_path = workspace.join("index.sqlite");
    let marker_path = workspace.join(WORKSPACE_MARKER_FILE);
    if marker_path.exists() {
        let marker = fs::read_to_string(marker_path).unwrap_or_default();
        if marker.contains(&identity.content_fingerprint) {
            return Ok(true);
        }
    }

    if workspace
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name == identity.workspace_id || name == identity.legacy_workspace_id)
    {
        return Ok(true);
    }

    Ok(index_path.exists() && workspace_identity_matches(&index_path, identity)?)
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

fn workspace_identity_matches(index_path: &Path, identity: &PstIdentity) -> AppResult<bool> {
    let conn = Connection::open(index_path)?;
    initialize_schema(&conn)?;

    let metadata_size_matches =
        metadata_value(&conn, "pst_size")?.as_deref() == Some(&identity.size.to_string());

    if metadata_size_matches
        && metadata_value(&conn, "pst_content_fingerprint")?.as_deref()
            == Some(identity.content_fingerprint.as_str())
    {
        return Ok(true);
    }

    Ok(metadata_size_matches
        && metadata_value(&conn, "pst_path")?.as_deref() == Some(identity.display_path.as_str()))
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
        let conn = Connection::open(index_path)?;
        initialize_schema(&conn)?;
        let message_count: i64 =
            conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;
        let import_status = metadata_value(&conn, "import_status")?.unwrap_or_else(|| {
            workspace_metadata_status(&candidate.path).unwrap_or_else(|| "pending".to_string())
        });
        (message_count, import_status)
    } else {
        (
            0,
            workspace_metadata_status(&candidate.path).unwrap_or_else(|| "pending".to_string()),
        )
    };

    Ok(ExistingWorkspace {
        workspace_id: candidate.workspace_id.clone(),
        workspace_path: candidate.path.display().to_string(),
        workspace_location_mode: candidate.mode.as_str().to_string(),
        workspace_location_label: candidate.mode.label().to_string(),
        is_complete: import_status == ImportStatus::Complete.as_str(),
        import_status,
        can_resume: candidate.path.join("extracted").is_dir(),
        can_reimport: candidate.path.is_dir(),
        message_count,
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
    workspace: &Path,
    workspace_mode: WorkspaceLocationMode,
) -> AppResult<()> {
    let values = [
        ("original_pst_path", identity.display_path.clone()),
        ("pst_path", identity.display_path.clone()),
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
        ("workspace_id", identity.workspace_id.clone()),
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
    let conn = Connection::open(workspace.join("index.sqlite"))?;
    let message_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;
    let folder_count: i64 = conn.query_row("SELECT COUNT(*) FROM folders", [], |row| row.get(0))?;

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
    })
}

fn workspace_summary_from_active(
    active: &ActiveWorkspace,
    reused_existing: bool,
) -> AppResult<WorkspaceSummary> {
    let conn = Connection::open(active.path.join("index.sqlite"))?;
    initialize_schema(&conn)?;
    let message_count: i64 =
        conn.query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))?;
    let folder_count: i64 = conn.query_row("SELECT COUNT(*) FROM folders", [], |row| row.get(0))?;
    let fingerprint_strategy = metadata_value(&conn, "pst_fingerprint_strategy")?
        .unwrap_or_else(|| "existing workspace".to_string());

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
    }

    fs::write(
        metadata_path,
        serde_json::to_string_pretty(&value).map_err(|error| AppError::new(error.to_string()))?,
    )?;
    Ok(())
}

fn run_readpst(
    app: &tauri::AppHandle,
    readpst: &Path,
    pst_path: &Path,
    workspace: &Path,
    output_dir: &Path,
) -> AppResult<()> {
    emit_progress(
        app,
        "Running readpst",
        None,
        None,
        "Running readpst locally. The original PST is opened read-only.",
    );

    append_import_log(
        workspace,
        "Command: readpst -e -8 -q -o <workspace>/extracted <pst>",
    )?;

    let log_path = import_log_path(workspace);
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    let stderr = stdout.try_clone()?;

    let mut child = Command::new(readpst)
        .arg("-e")
        .arg("-8")
        .arg("-q")
        .arg("-o")
        .arg(output_dir)
        .arg(pst_path)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| AppError::new(format!("Could not run readpst: {error}")))?;

    let pid = child.id();
    let state = app.state::<AppState>();
    {
        let mut guard = state
            .readpst_pid
            .lock()
            .map_err(|_| AppError::new("Could not lock readpst process state."))?;
        *guard = Some(pid);
    }

    append_import_log(workspace, &format!("readpst pid={pid}"))?;

    let status_result = loop {
        if !workspace.is_dir() {
            let _ = terminate_process(pid, false);
            for _ in 0..20 {
                if child.try_wait()?.is_some() {
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
            if child.try_wait()?.is_none() {
                let _ = terminate_process(pid, true);
                let _ = child.wait();
            }
            break Err(AppError::new(format!(
                "Workspace was deleted during import: {}",
                workspace.display()
            )));
        }

        if state.cancel_import_requested.load(Ordering::SeqCst) {
            append_import_log(workspace, "Cancel requested. Terminating readpst.")?;
            let _ = terminate_process(pid, false);

            let mut status = None;
            for _ in 0..20 {
                if let Some(next_status) = child.try_wait()? {
                    status = Some(next_status);
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }

            if status.is_none() {
                append_import_log(
                    workspace,
                    "readpst did not exit after SIGTERM. Sending SIGKILL.",
                )?;
                let _ = terminate_process(pid, true);
                status = Some(child.wait()?);
            }

            if let Some(status) = status {
                append_import_log(workspace, &format!("readpst stopped with status {status}"))?;
            }

            break Err(AppError::new(format!(
                "Import cancelled. Log: {}",
                log_path.display()
            )));
        }

        if let Some(status) = child.try_wait()? {
            break Ok(status);
        }

        thread::sleep(Duration::from_millis(250));
    };

    {
        let mut guard = state
            .readpst_pid
            .lock()
            .map_err(|_| AppError::new("Could not lock readpst process state."))?;
        *guard = None;
    }

    let status = status_result?;
    append_import_log(workspace, &format!("readpst exited with status {status}"))?;

    if !status.success() {
        let code = status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "terminated by signal".to_string());
        let short_error = import_log_tail(workspace, 1600);
        return Err(AppError::new(format!(
            "readpst exited non-zero (exit code: {code}). Short error: {short_error}. Log: {}",
            log_path.display()
        )));
    }

    Ok(())
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
    fs::create_dir_all(workspace)?;
    let marker = workspace.join(WORKSPACE_MARKER_FILE);
    let contents = format!(
        "app=PST QuickView\nworkspace_id={workspace_id}\npst_content_fingerprint={}\nworkspace_location_mode={}\noriginal_pst_path={}\n",
        identity.content_fingerprint,
        workspace_mode.as_str(),
        identity.display_path
    );
    fs::write(marker, contents)?;
    Ok(())
}

fn validate_workspace_for_delete(active: &ActiveWorkspace) -> AppResult<()> {
    if !active.path.exists() {
        return Err(AppError::new(format!(
            "Workspace does not exist: {}",
            active.path.display()
        )));
    }

    let marker = active.path.join(WORKSPACE_MARKER_FILE);
    if !marker.exists() {
        return Err(AppError::new(format!(
            "Refusing to delete {} because it is missing {}.",
            active.path.display(),
            WORKSPACE_MARKER_FILE
        )));
    }

    let marker_contents = fs::read_to_string(&marker).unwrap_or_default();
    if !marker_contents.contains("app=PST QuickView") {
        return Err(AppError::new(format!(
            "Refusing to delete {} because the workspace marker is not valid.",
            active.path.display()
        )));
    }

    if active
        .path
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| name != active.id)
    {
        return Err(AppError::new(format!(
            "Refusing to delete {} because it does not match the active workspace id.",
            active.path.display()
        )));
    }

    let index_path = active.path.join("index.sqlite");
    if index_path.exists() {
        let conn = Connection::open(&index_path)?;
        initialize_schema(&conn)?;
        if let Some(fingerprint) = metadata_value(&conn, "pst_content_fingerprint")? {
            if fingerprint != active.fingerprint {
                return Err(AppError::new(format!(
                    "Refusing to delete {} because its fingerprint does not match the active PST.",
                    active.path.display()
                )));
            }
        }
    }

    Ok(())
}

fn close_workspace_db_before_delete(workspace: &Path) -> AppResult<()> {
    let index_path = workspace.join("index.sqlite");
    if !index_path.exists() {
        return Ok(());
    }

    let conn = Connection::open(index_path)?;
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA optimize;");
    drop(conn);
    Ok(())
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

    Ok((stat.f_bavail as u64).saturating_mul(stat.f_bsize as u64))
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
    for suffix in ["-wal", "-shm"] {
        let sidecar = parent.join(format!("{file_name}{suffix}"));
        if sidecar.exists() {
            fs::remove_file(sidecar)?;
        }
    }
    Ok(())
}

fn is_eml_file(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|ext| ext.eq_ignore_ascii_case("eml"))
        .unwrap_or(false)
}

fn count_eml_files(extract_dir: &Path) -> AppResult<usize> {
    let mut count = 0usize;
    for entry in WalkDir::new(extract_dir) {
        let entry = entry?;
        if entry.file_type().is_file() && is_eml_file(entry.path()) {
            count = count.saturating_add(1);
        }
    }
    Ok(count)
}

struct IndexProgressThrottle {
    last_emit: Option<Instant>,
    last_indexed: usize,
}

impl IndexProgressThrottle {
    fn new() -> Self {
        Self {
            last_emit: None,
            last_indexed: 0,
        }
    }

    fn should_emit(&mut self, indexed: usize, total: usize) -> bool {
        let now = Instant::now();
        let due_by_count =
            indexed.saturating_sub(self.last_indexed) >= INDEX_PROGRESS_MESSAGE_INTERVAL;
        let due_by_time = self
            .last_emit
            .map(|last_emit| now.duration_since(last_emit) >= INDEX_PROGRESS_TIME_INTERVAL)
            .unwrap_or(true);
        let due_by_completion = indexed == total;

        if due_by_count || due_by_time || due_by_completion {
            self.last_emit = Some(now);
            self.last_indexed = indexed;
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

fn set_index_progress_metadata(conn: &Connection, stats: &IndexStats) -> AppResult<()> {
    for (key, value) in [
        ("message_count_indexed", stats.indexed.to_string()),
        ("error_count", stats.error_count.to_string()),
        ("last_error", stats.last_error.clone().unwrap_or_default()),
        ("discovered_messages", stats.discovered.to_string()),
        ("total_discovered_eml_count", stats.discovered.to_string()),
    ] {
        set_metadata_value(conn, key, value)?;
    }
    Ok(())
}

fn set_reindex_metadata_values(
    conn: &Connection,
    active: &ActiveWorkspace,
    status: ImportStatus,
    stats: Option<&IndexStats>,
    last_error: Option<&str>,
) -> AppResult<()> {
    let now = Utc::now().to_rfc3339();
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
        set_metadata_value(conn, key, value)?;
    }

    if matches!(
        status,
        ImportStatus::Complete | ImportStatus::Failed | ImportStatus::Cancelled
    ) {
        set_metadata_value(conn, "finished_at", &now)?;
    }
    if let Some(stats) = stats {
        set_metadata_value(conn, "message_count_indexed", stats.indexed)?;
        set_metadata_value(conn, "error_count", stats.error_count)?;
        set_metadata_value(conn, "discovered_messages", stats.discovered)?;
        set_metadata_value(conn, "total_discovered_eml_count", stats.discovered)?;
        set_metadata_value(
            conn,
            "last_error",
            stats.last_error.clone().unwrap_or_default(),
        )?;
    }
    if let Some(error) = last_error {
        set_metadata_value(conn, "last_error", error)?;
    }

    Ok(())
}

fn stream_index_eml_files(
    app: &tauri::AppHandle,
    workspace: &Path,
    extract_dir: &Path,
    conn: &mut Connection,
    total_eml_count: usize,
    progress_message: &str,
    cancel_message: &str,
) -> AppResult<IndexStats> {
    let mut transaction = conn.transaction()?;
    let mut folder_cache: HashMap<String, i64> = HashMap::new();
    let root_id = ensure_folder(&transaction, &mut folder_cache, "")?;
    let mut stats = IndexStats {
        discovered: total_eml_count,
        ..IndexStats::default()
    };
    let mut batch_count = 0usize;
    let mut progress_throttle = IndexProgressThrottle::new();

    append_import_log(
        workspace,
        &format!(
            "Streaming EML indexing started. total_eml_count={} batch_size={}",
            total_eml_count, INDEX_BATCH_SIZE
        ),
    )?;

    for entry in WalkDir::new(extract_dir) {
        let entry = entry?;
        if !entry.file_type().is_file() || !is_eml_file(entry.path()) {
            continue;
        }

        if !workspace.is_dir() {
            return Err(AppError::new(format!(
                "Workspace was deleted during indexing: {}",
                workspace.display()
            )));
        }

        if app
            .state::<AppState>()
            .cancel_import_requested
            .load(Ordering::SeqCst)
        {
            append_import_log(workspace, cancel_message)?;
            return Err(AppError::new(format!(
                "{} Log: {}",
                cancel_message,
                import_log_path(workspace).display()
            )));
        }

        let eml_path = entry.path();
        let relative_path = relative_path_string(extract_dir, eml_path)?;
        let folder_path = eml_path
            .parent()
            .and_then(|parent| parent.strip_prefix(extract_dir).ok())
            .map(path_to_slash_string)
            .unwrap_or_default();
        let folder_id = if folder_path.is_empty() {
            root_id
        } else {
            ensure_folder(&transaction, &mut folder_cache, &folder_path)?
        };

        let parsed = match parse_eml(eml_path) {
            Ok(parsed) => parsed,
            Err(error) => {
                stats.error_count += 1;
                let message = format!("Failed to parse {relative_path}: {}", error.message);
                stats.last_error = Some(message.clone());
                append_import_log(workspace, &message)?;
                ParsedMessage {
                    subject: "(Could not parse message)".to_string(),
                    body: message,
                    body_source: BodySource::ParseError,
                    ..ParsedMessage::default()
                }
            }
        };

        if let Some(fallback) = parsed.rtf_body_fallback.as_deref() {
            append_import_log(
                workspace,
                &format!("RTF body fallback used for {relative_path}: {fallback}"),
            )?;
        }

        insert_message(&transaction, folder_id, &relative_path, parsed)?;
        stats.indexed += 1;
        batch_count += 1;

        if batch_count >= INDEX_BATCH_SIZE {
            transaction.commit()?;
            set_index_progress_metadata(conn, &stats)?;
            append_import_log(
                workspace,
                &format!(
                    "Index batch committed. indexed={} total={} errors={}",
                    stats.indexed, stats.discovered, stats.error_count
                ),
            )?;
            transaction = conn.transaction()?;
            batch_count = 0;
        }

        if progress_throttle.should_emit(stats.indexed, stats.discovered) {
            emit_progress(
                app,
                "Indexing messages",
                Some(stats.indexed),
                Some(stats.discovered),
                &format!(
                    "{} Indexed {} of {} EML files.",
                    progress_message, stats.indexed, stats.discovered
                ),
            );
        }
    }

    transaction.commit()?;
    set_index_progress_metadata(conn, &stats)?;
    append_import_log(
        workspace,
        &format!(
            "Streaming EML indexing complete. indexed={} total={} errors={}",
            stats.indexed, stats.discovered, stats.error_count
        ),
    )?;

    Ok(stats)
}

fn reindex_eml_files_to_index(
    app: &tauri::AppHandle,
    active: &ActiveWorkspace,
    extract_dir: &Path,
    index_path: &Path,
    existing_metadata: HashMap<String, String>,
) -> AppResult<IndexStats> {
    emit_progress(
        app,
        "Scanning extracted EML files",
        None,
        None,
        "Scanning existing extracted EML files.",
    );

    let total_eml_count = count_eml_files(extract_dir)?;

    emit_progress(
        app,
        "Indexing messages",
        Some(0),
        Some(total_eml_count),
        "Reindexing existing EML files. readpst is not running.",
    );

    let mut conn = Connection::open(index_path)?;
    initialize_schema(&conn)?;
    write_import_metadata(&conn, &existing_metadata)?;
    set_reindex_metadata_values(&conn, active, ImportStatus::Running, None, None)?;

    let stats = stream_index_eml_files(
        app,
        &active.path,
        extract_dir,
        &mut conn,
        total_eml_count,
        "Reindexing existing EML files.",
        "Reindex cancelled. Stopping EML indexing.",
    )?;
    build_conversation_index(app, &active.path, &mut conn, &active.id)?;
    set_reindex_metadata_values(&conn, active, ImportStatus::Complete, Some(&stats), None)?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA optimize;")?;
    drop(conn);

    Ok(stats)
}

fn index_eml_files(
    app: &tauri::AppHandle,
    identity: &PstIdentity,
    extract_dir: &Path,
    index_path: &Path,
    workspace: &Path,
    workspace_mode: WorkspaceLocationMode,
    readpst_path: &str,
    readpst_version: &str,
) -> AppResult<IndexStats> {
    emit_progress(
        app,
        "Scanning extracted EML files",
        None,
        None,
        "Scanning extracted EML files.",
    );

    let total_eml_count = count_eml_files(extract_dir)?;

    emit_progress(
        app,
        "Indexing messages",
        Some(0),
        Some(total_eml_count),
        "Creating the local SQLite index.",
    );

    let mut conn = Connection::open(index_path)?;
    initialize_schema(&conn)?;
    insert_import_metadata(
        &conn,
        identity,
        total_eml_count,
        workspace,
        workspace_mode,
        readpst_path,
        readpst_version,
    )?;

    let stats = stream_index_eml_files(
        app,
        workspace,
        extract_dir,
        &mut conn,
        total_eml_count,
        "Creating the local SQLite index.",
        "Import cancelled. Stopping message indexing.",
    )?;
    build_conversation_index(app, workspace, &mut conn, &identity.workspace_id)?;
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA optimize;")?;

    Ok(stats)
}

fn initialize_schema(conn: &Connection) -> AppResult<()> {
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
    )
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

fn insert_import_metadata(
    conn: &Connection,
    identity: &PstIdentity,
    discovered_messages: usize,
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
        ("workspace_id", identity.workspace_id.clone()),
        ("import_status", ImportStatus::Running.as_str().to_string()),
        ("discovered_messages", discovered_messages.to_string()),
        (
            "total_discovered_eml_count",
            discovered_messages.to_string(),
        ),
        ("message_count_indexed", "0".to_string()),
        ("error_count", "0".to_string()),
        ("last_error", String::new()),
        ("readpst_path", readpst_path.to_string()),
        ("readpst_version", readpst_version.to_string()),
        ("body_html_schema_version", "body-html-v1".to_string()),
        ("started_at", now.clone()),
        ("finished_at", String::new()),
        ("imported_at", now),
    ];

    for (key, value) in values {
        conn.execute(
            "INSERT OR REPLACE INTO import_metadata (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
    }

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

fn path_is_self_or_descendant(path: &str, ancestor: &str) -> bool {
    if ancestor.is_empty() {
        return true;
    }

    path == ancestor
        || path
            .strip_prefix(ancestor)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn insert_message(
    transaction: &Transaction<'_>,
    folder_id: i64,
    relative_path: &str,
    parsed: ParsedMessage,
) -> AppResult<()> {
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

    transaction.execute(
        "INSERT INTO messages
             (folder_id, eml_path, subject, sender, recipients, date, body, body_source, body_html,
              snippet, attachment_names, has_attachments, message_id_header_raw, message_id_header,
              in_reply_to_raw, in_reply_to, references_header_raw, references_header,
              normalized_subject, thread_warning)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)",
        params![
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

    Ok(())
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
    append_import_log(workspace, "Conversation assignment started.")?;

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
    let inputs = collect_rows(rows)?;
    drop(statement);

    let assignments = assign_threads(&inputs);
    let transaction = conn.transaction()?;
    let mut method_counts = HashMap::<String, usize>::new();
    for (index, assignment) in assignments.iter().enumerate() {
        if index % INDEX_PROGRESS_MESSAGE_INTERVAL == 0
            && app
                .state::<AppState>()
                .cancel_import_requested
                .load(Ordering::SeqCst)
        {
            return Err(AppError::new(
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
    }
    transaction.commit()?;
    set_metadata_value(
        conn,
        "conversation_schema_version",
        CONVERSATION_SCHEMA_VERSION,
    )?;
    mark_workspace_conversation_schema(workspace)?;
    append_import_log(
        workspace,
        &format!(
            "Conversation assignment complete. messages={} header={} references={} subject_fallback={} standalone={}",
            assignments.len(),
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

fn nonempty_string(value: String) -> Option<String> {
    (!value.trim().is_empty()).then_some(value)
}

fn thread_timestamp(value: &str) -> Option<i64> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|date| date.timestamp())
        .or_else(|| mailparse::dateparse(value).ok())
}

fn mark_workspace_conversation_schema(workspace: &Path) -> AppResult<()> {
    let metadata_path = workspace.join(WORKSPACE_METADATA_FILE);
    let contents = fs::read_to_string(&metadata_path).unwrap_or_else(|_| "{}".to_string());
    let mut value = serde_json::from_str::<serde_json::Value>(&contents)
        .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new()));
    if !value.is_object() {
        value = serde_json::Value::Object(serde_json::Map::new());
    }
    if let Some(object) = value.as_object_mut() {
        object.insert(
            "conversation_schema_version".to_string(),
            serde_json::Value::String(CONVERSATION_SCHEMA_VERSION.to_string()),
        );
        object.insert(
            "updated_at".to_string(),
            serde_json::Value::String(Utc::now().to_rfc3339()),
        );
    }
    fs::write(
        metadata_path,
        serde_json::to_string_pretty(&value).map_err(|error| AppError::new(error.to_string()))?,
    )?;
    Ok(())
}

fn parse_eml(path: &Path) -> AppResult<ParsedMessage> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    let parsed = mailparse::parse_mail(&bytes).map_err(|error| AppError::new(error.to_string()))?;

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
        disposition.params.get("filename").is_some() || part.ctype.params.get("name").is_some();
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
    if find_ascii_case_insensitive(rtf, "\\fromhtml").is_none() {
        return None;
    }

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
        .rsplit(|char| matches!(char, '/' | '\\' | ':'))
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

    let sanitized = sanitized
        .trim()
        .trim_matches('.')
        .chars()
        .take(180)
        .collect::<String>();

    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        "attachment".to_string()
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
                        if index + 1 <= chars.len() {
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
                        if !state.skipping_destination {
                            match control {
                                '\t' => output.push('\t'),
                                _ => {}
                            }
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
        "td" | "th" => {
            if !output.is_empty() && !output.ends_with(' ') && !output.ends_with('\n') {
                output.push(' ');
            }
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
        is_http_url(src.trim())
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

fn sanitize_email_html(
    body_html: &str,
    cid_images: HashMap<String, String>,
    allow_remote_images: bool,
) -> SanitizedEmailHtml {
    let blocked_remote_count = Arc::new(AtomicUsize::new(0));
    let embedded_image_count = Arc::new(AtomicUsize::new(0));
    let blocked_remote_count_for_filter = Arc::clone(&blocked_remote_count);
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
    let url_schemes = HashSet::from(["cid", "data", "http", "https"]);

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

            if is_http_url(trimmed) {
                if allow_remote_images {
                    return Some(Cow::Borrowed(value));
                }
                blocked_remote_count_for_filter.fetch_add(1, Ordering::SeqCst);
                return None;
            }

            if is_safe_image_data_url(trimmed) {
                return Some(Cow::Borrowed(value));
            }

            None
        });

    let sanitized_html = builder.clean(body_html).to_string();
    let remote_image_count = blocked_remote_count.load(Ordering::SeqCst);
    SanitizedEmailHtml {
        sanitized_html,
        remote_images_blocked: remote_image_count > 0,
        remote_image_count,
        embedded_image_count: embedded_image_count.load(Ordering::SeqCst),
    }
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
    let mut bytes = Vec::new();
    File::open(&eml_path)?.read_to_end(&mut bytes)?;
    let parsed = mailparse::parse_mail(&bytes).map_err(|error| AppError::new(error.to_string()))?;
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

fn is_http_url(value: &str) -> bool {
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

fn query_messages_page(
    conn: &Connection,
    folder_id: Option<i64>,
    include_subfolders: bool,
    criteria: &MessageSearchCriteria,
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
    let source = build_message_query_source(conn, folder_id, include_subfolders, criteria)?;
    check_search_operation(operation)?;
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
    })
}

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

fn check_search_operation(operation: Option<&SearchOperationGuard>) -> AppResult<()> {
    operation
        .map(SearchOperationGuard::check_cancelled)
        .transpose()?;
    Ok(())
}

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

#[allow(clippy::too_many_arguments)]
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
    check_search_operation(operation)?;
    let source = build_message_query_source(conn, folder_id, include_subfolders, criteria)?;
    let (where_sql, mut query_params) =
        query_source_with_condition(&source, "COALESCE(m.conversation_id, '') <> ''");
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
                matched.conversation_root_id,
                COALESCE(
                    (SELECT NULLIF(root.normalized_subject, '')
                       FROM messages root
                      WHERE root.id = matched.conversation_root_id),
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
                  WHERE all_messages.conversation_id = matched.conversation_id) AS total_count,
                EXISTS (
                    SELECT 1
                      FROM messages attachment_messages
                     WHERE attachment_messages.conversation_id = matched.conversation_id
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
fn query_conversation_summaries_page(
    conn: &Connection,
    active: &ActiveWorkspace,
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
    let items = query_conversation_summaries(
        conn,
        active,
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
    Ok(path_to_slash_string(relative))
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

fn unique_export_path(export_dir: &Path, sanitized_filename: &str) -> AppResult<PathBuf> {
    let sanitized_filename = sanitize_attachment_filename(sanitized_filename);
    let (stem, extension) = filename_stem_and_extension(&sanitized_filename);

    for suffix in 0..10_000 {
        let filename = if suffix == 0 {
            sanitized_filename.clone()
        } else if extension.is_empty() {
            format!("{stem}-{suffix}")
        } else {
            format!("{stem}-{suffix}.{extension}")
        };
        let candidate = export_dir.join(filename);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(AppError::new(format!(
        "Could not create a unique export filename for {sanitized_filename}."
    )))
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
    let mut filename = export_message_eml_filename(0, date, subject);
    if format != StandaloneSourceFormat::Eml {
        filename.truncate(filename.len().saturating_sub(".eml".len()));
        filename.push('.');
        filename.push_str(format.source_extension());
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
    if trimmed.len() >= 10 {
        let prefix = &trimmed[..10];
        if prefix
            .chars()
            .all(|char| char.is_ascii_digit() || char == '-')
        {
            return prefix.to_string();
        }
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
    message: &str,
) {
    let _ = app.emit(
        "import-progress",
        ImportProgress {
            stage: stage.to_string(),
            current,
            total,
            message: message.to_string(),
        },
    );
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
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
            eprintln!(
                "SKIPPED optional external fixture: {} is unavailable. Set {} to override the path.",
                path.display(),
                env_name
            );
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
        cfb::open(path).unwrap_or_else(|error| {
            panic!(
                "fixture must be a readable compound MSG file ({}): {error}",
                path.display()
            )
        });
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

        let result = sanitize_email_html(html, cid_images, false);

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

        let first = query_messages_page(
            &conn,
            None,
            false,
            &criteria,
            Some("newest"),
            Some(3),
            Some(0),
            None,
        )
        .unwrap();
        assert_eq!(first.requested_offset, 0);
        assert_eq!(first.returned_count, 3);
        assert_eq!(first.items.len(), 3, "the lookahead row must not leak");
        assert!(first.has_more);
        assert_eq!(
            first.items.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![5, 4, 3]
        );

        let second = query_messages_page(
            &conn,
            None,
            false,
            &criteria,
            Some("newest"),
            Some(3),
            Some(3),
            None,
        )
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
        let page = query_messages_page(
            &conn,
            None,
            false,
            &criteria,
            Some("relevance"),
            Some(3),
            Some(0),
            None,
        )
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
            let expected = query_messages_page(
                &conn,
                None,
                false,
                &criteria,
                Some(sort),
                Some(100),
                Some(0),
                None,
            )
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
        let result =
            query_messages_page(&conn, None, false, &criteria, None, Some(10), Some(0), None)
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
        let relevance_result = query_messages_page(
            &conn,
            None,
            false,
            &criteria,
            Some("relevance"),
            Some(10),
            Some(0),
            None,
        )
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
        let error = workspace_path_for_id("/tmp/source.pst")
            .expect_err("search commands accept validated workspace ids, not source paths");
        assert_eq!(error.to_string(), "Invalid workspace id.");
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
            "migrated {}: messages={} attachments={} user_version={}",
            path.display(),
            before_messages,
            before_attachments,
            SQLITE_SCHEMA_VERSION_CURRENT
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
            .all(|attachment| attachment.filename.to_ascii_lowercase() != "rtf-body.rtf"));
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
            false,
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
            "Rich fixture passed read-only check: path={} identity_pinned={} attachments={} inline_resources={}",
            path.display(),
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
            false,
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
            "Legacy fixture passed read-only check: path={} identity_pinned={} body_source={} body_text_bytes={}",
            path.display(),
            expected_sha256.is_some(),
            view.body_source,
            view.body_text.len()
        );
    }
}
