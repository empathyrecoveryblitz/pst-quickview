export type ReadpstStatus = {
  available: boolean;
  path: string | null;
  version: string | null;
  source: string;
  sourceLabel: string;
  setupCommand: string;
};

export type AppDiagnostics = {
  appVersion: string;
  macosVersion: string;
  cpuArchitecture: string;
  executableArchitecture: string;
  readpstSource: string;
  readpstVersion: string;
  openPstCount: number;
  activeWorkspaceMode: string;
  activeWorkspacePath: string;
  databaseSchemaVersion: number | null;
  conversationDataStatus: string;
};

export type WorkspaceSummary = {
  id: string;
  pstPath: string;
  workspacePath: string;
  emlDir: string;
  indexPath: string;
  messageCount: number;
  folderCount: number;
  reusedExisting: boolean;
  fingerprint: string;
  fingerprintStrategy: string;
  workspaceLocationMode: WorkspaceLocationMode;
  workspaceLocationLabel: string;
  pstSizeBytes: number;
  integrityState: WorkspaceIntegrityState;
  integrityClassification: WorkspaceIntegrityClassification;
  sourceAvailability: WorkspaceSourceAvailability;
  canResumeIndexing: boolean;
  canRepairIndex: boolean;
  canRebuildConversations: boolean;
  resumeDetails: ResumeDetails | null;
  hasLastVerification: boolean;
  conversationState: string;
  completedImportDurationMs: number | null;
  lastActivityAt: string | null;
  activeImportElapsedMs: number;
  currentStageElapsedMs: number;
};

export type Folder = {
  id: number;
  parentId: number | null;
  path: string;
  name: string;
  messageCount: number;
  directMessageCount: number;
  activeMessageCount: number;
  activeDirectMessageCount: number;
  localTrashMessageCount: number;
  localTrashDirectMessageCount: number;
};

export type SearchHighlightRange = {
  start: number;
  end: number;
};

export type SearchMatchedField =
  | "subject"
  | "sender"
  | "recipients"
  | "body"
  | "attachment";

export type SearchMatchContext = {
  snippetText: string;
  highlightRanges: SearchHighlightRange[];
  matchedFields: SearchMatchedField[];
};

export type MessageListItem = {
  id: number;
  folderId: number;
  folderPath?: string;
  folderName?: string;
  subject: string;
  sender: string;
  recipients: string;
  date: string;
  snippet: string;
  hasAttachments: boolean;
  attachmentCount: number;
  searchMatchContext?: SearchMatchContext;
  workspaceId?: string;
  pstDisplayName?: string;
  workspacePath?: string;
};

export type LocalTrashQueryHealth = {
  workspaceId: string;
  available: boolean;
  canReset: boolean;
  sidecarPresent: boolean;
  schemaVersion: number | null;
  stateRevision: number;
  fingerprintMatch: boolean;
  lastErrorCode: string | null;
};

export type MessagePageResult = {
  items: MessageListItem[];
  requestedOffset: number;
  returnedCount: number;
  hasMore: boolean;
  nextCursor: string | null;
  paginationMode: "cursor" | "offset";
  localTrashStates: LocalTrashQueryHealth[];
};

export type MessageCountResult = {
  totalCount: number;
};

export type WorkspaceSearchCount = {
  workspaceId: string;
  pstDisplayName: string;
  count: number;
};

export type MultiMessagePageResult = MessagePageResult;

export type MultiMessageCountResult = MessageCountResult & {
  perWorkspaceCounts: WorkspaceSearchCount[];
};

export type ConversationWorkspaceScope = {
  workspaceId: string;
  folderId: number | null;
  includeSubfolders: boolean;
};

export type ConversationSummary = {
  conversationId: string;
  conversationRootId: number | null;
  subject: string;
  latestSender: string;
  participants: string[];
  latestDate: string;
  snippet: string;
  matchingMessageCount: number;
  totalMessageCount: number;
  hasAttachments: boolean;
  latestMessageId: number;
  assignmentMethod: string;
  workspaceId: string;
  pstDisplayName: string;
  workspacePath: string;
};

export type ConversationWorkspaceIssue = {
  workspaceId: string;
  pstDisplayName: string;
  workspacePath: string;
  canReindex: boolean;
};

export type ConversationPageResult = {
  items: ConversationSummary[];
  requestedOffset: number;
  returnedCount: number;
  hasMore: boolean;
  indexedWorkspaceCount: number;
  unindexedWorkspaces: ConversationWorkspaceIssue[];
  localTrashStates: LocalTrashQueryHealth[];
};

export type ConversationCountResult = {
  totalCount: number;
  matchingMessageCount: number;
};

export type ConversationMessageItem = MessageListItem & {
  matchesScope: boolean;
};

export type ConversationMessagesResult = {
  items: ConversationMessageItem[];
  matchingMessageCount: number;
  totalMessageCount: number;
  showingEntireConversation: boolean;
};

export type SearchFilters = {
  from: string | null;
  recipients: string | null;
  subject: string | null;
  body: string | null;
  attachment: string | null;
  hasAttachments: "any" | "yes" | "no";
  dateFrom: string | null;
  dateTo: string | null;
};

export type MessageVisibility = "active" | "trashed";

export type LocalTrashSummary = {
  workspaceId: string;
  available: boolean;
  canReset: boolean;
  sidecarPresent: boolean;
  trashedMessageCount: number;
  orphanedStateCount: number;
  schemaVersion: number | null;
  stateRevision: number;
  lastChangedAt: string | null;
  fingerprintMatch: boolean;
  lastErrorCode: string | null;
};

export type LocalTrashMutationResult = {
  workspaceId: string;
  result:
    | "moved"
    | "already_trashed"
    | "restored"
    | "already_active"
    | "undone";
  changedCount: number;
  stateRevision: number;
  operationId: string | null;
};

export type LocalTrashUndoState = {
  workspaceId: string;
  operationId: string;
  expectedRevision: number;
};

export type LocalTrashResetResult = {
  workspaceId: string;
  reset: boolean;
};

export type Attachment = {
  id: number;
  filename: string;
  sanitizedFilename: string;
  contentType: string;
  sizeBytes: number | null;
  attachmentIndex: number;
  contentDisposition: string;
};

export type ExportAttachmentResult = {
  exported: boolean;
  attachmentId: number;
  filename: string;
  sanitizedFilename: string;
  outputPath: string | null;
  sizeBytes: number | null;
  contentType: string;
  error: string | null;
};

export type ExportOriginalEmlResult = {
  exported: boolean;
  messageId: number;
  filename: string;
  workspacePath: string | null;
  exportDir: string | null;
  outputPath: string | null;
  sizeBytes: number | null;
  error: string | null;
};

export type MessageDetail = MessageListItem & {
  body: string;
  bodySource: "text_plain" | "html_converted" | "rtf_converted" | "missing" | "parse_error" | string;
  bodyHtmlAvailable: boolean;
  emlPath: string;
  canReindexFromEml: boolean;
  inLocalTrash: boolean;
  localTrashAvailable: boolean;
  localTrashErrorCode: string | null;
  attachments: Attachment[];
};

export type HtmlRenderResult = {
  htmlAvailable: boolean;
  sanitizedHtml: string;
  remoteImagesBlocked: boolean;
  remoteImageCount: number;
  embeddedImageCount: number;
  error: string | null;
};

export type MessageDiagnostics = {
  messageId: number;
  subject: string;
  bodySource: string;
  hasBodyText: boolean;
  hasBodyHtml: boolean;
  sourceEmlPath: string;
  attachmentCount: number;
  attachments: DiagnosticAttachment[];
  detectedBodyMimePart: string | null;
  rtfBodyPromoted: boolean;
  rtfBodySuppressedFromAttachments: boolean;
  remoteImagesDetected: boolean;
  cidImagesDetected: boolean;
  parseWarnings: string[];
  mimeParts: MimePartDiagnostic[];
  messageIdHeader: string;
  inReplyTo: string;
  referencesHeader: string;
  normalizedSubject: string;
  conversationId: string;
  threadAssignmentMethod: string;
  detectedParent: string | null;
  detectedRoot: string | null;
};

export type DiagnosticAttachment = {
  filename: string;
  contentType: string;
};

export type MimePartDiagnostic = {
  path: string;
  contentType: string;
  contentDisposition: string;
  filename: string;
  contentId: string;
  sizeBytes: number | null;
  role: string;
};

export type CalendarPropertyDiagnostic = {
  name: string;
  propertyId: string;
  propertyType: string;
  value: string;
  source: string;
};

export type CalendarItemDetails = {
  itemType: string;
  messageClass: string;
  organizer: string;
  organizerSource: string;
  requiredAttendees: string;
  requiredAttendeesSource: string;
  optionalAttendees: string;
  optionalAttendeesSource: string;
  resources: string;
  start: string;
  end: string;
  startRaw: string;
  endRaw: string;
  timeZone: string;
  timeZoneSource: string;
  timeZoneUncertain: boolean;
  allDay: boolean | null;
  location: string;
  recurrenceSummary: string;
  recurrenceAvailable: boolean;
  recurrenceRawSummary: string;
  meetingStatus: string;
  responseStatus: string;
  reminder: string;
  sensitivity: string;
  categories: string[];
  creationTime: string;
  modificationTime: string;
  propertyDiagnostics: CalendarPropertyDiagnostic[];
  parseWarnings: string[];
  unsupportedProperties: string[];
};

export type SourceEmlView = {
  messageId: number;
  emlPath: string;
  sourcePath: string;
  sourceKind: "workspace" | "standalone" | string;
  sourceFormat: "eml" | "msg" | string;
  sourceLabel: string;
  messageClass: string;
  subject: string;
  sender: string;
  recipients: string;
  date: string;
  bodyText: string;
  bodySource: "text_plain" | "html_converted" | "rtf_converted" | "missing" | "parse_error" | string;
  bodyHtmlAvailable: boolean;
  inLocalTrash: boolean;
  localTrashAvailable: boolean;
  localTrashErrorCode: string | null;
  sanitizedHtml: string;
  remoteImagesBlocked: boolean;
  remoteImageCount: number;
  embeddedImageCount: number;
  rawSource: string;
  rawSourceAvailable: boolean;
  parseWarnings: string[];
  attachments: Attachment[];
  inlineResources: Attachment[];
  messageIdHeader: string;
  inReplyTo: string;
  referencesHeader: string;
  normalizedSubject: string;
  calendar: CalendarItemDetails | null;
};

export type ExternalFileOpen = {
  path: string;
  fileKind: "pst" | "eml" | "msg" | string;
  stableId: string;
};

export type ExternalFileOpenBatch = {
  files: ExternalFileOpen[];
  warnings: string[];
  skippedCount: number;
};

export type ExternalFileOpenReady = {
  batches: ExternalFileOpenBatch[];
  externalOpenReceived: boolean;
};

export type SavePrintableHtmlResult = {
  saved: boolean;
  filename: string;
  outputPath: string | null;
  sizeBytes: number | null;
  error: string | null;
};

export type CancelImportResult = {
  requested: boolean;
  pid: number | null;
  message: string;
};

export type WorkspaceSize = {
  workspacePath: string;
  workspaceLocationMode: WorkspaceLocationMode;
  workspaceLocationLabel: string;
  totalBytes: number;
  extractedEmlBytes: number;
  sqliteIndexBytes: number;
  logsBytes: number;
  attachmentsBytes: number;
  availableDiskBytes: number | null;
};

export type WorkspaceLocationMode = "app_support" | "next_to_pst";

export type WorkspaceIntegrityState =
  | "preflight"
  | "extracting"
  | "extraction_complete"
  | "discovering"
  | "indexing"
  | "building_conversations"
  | "verifying"
  | "complete"
  | "cancelled"
  | "incomplete"
  | "failed";

export type WorkspaceIntegrityClassification = "complete" | "incomplete" | "unsupported";

export type WorkspaceSourceAvailability = "available" | "unavailable" | "changed";

export type ResumeDetails = {
  pstBasename: string;
  lastCompletedStage: string;
  interruptedStage: string;
  discoveredCandidates: number;
  canonicalIndexed: number;
  remainingCandidates: number | null;
  lastSuccessfulUpdate: string;
  elapsedDurationMs: number;
  stageElapsedDurationMs: number;
  lastActivityAt: string | null;
  reason: string;
};

export type VerificationLevel = "quick" | "full";

export type VerificationResult =
  | "verified"
  | "incomplete"
  | "repairable"
  | "source_unavailable"
  | "source_changed"
  | "extraction_inconsistent"
  | "database_inconsistent"
  | "unsupported"
  | "failed";

export type PossibleDuplicateGroupDetail = {
  opaqueId: string;
  signals: string[];
};

export type VerificationReport = {
  formatVersion: number;
  operationId: string;
  workspaceId: string;
  verificationLevel: VerificationLevel;
  result: VerificationResult;
  workspaceState: WorkspaceIntegrityState;
  discoveredCandidates: number;
  canonicalMessages: number;
  insertedMessages: number;
  skippedExistingMessages: number;
  parseFailures: number;
  attachments: number;
  folders: number;
  ftsRows: number;
  conversationState: string;
  missingCanonicalRecords: number;
  unexpectedCanonicalRecords: number;
  possibleDuplicateGroups: number;
  possibleDuplicateGroupDetails: PossibleDuplicateGroupDetail[];
  sourceIntegrity: "passed" | "unavailable" | "changed";
  startedAt: string;
  completedAt: string;
  elapsedDurationMs: number;
  completedImportDurationMs: number | null;
  warnings: string[];
  recommendedActions: string[];
};

export type WorkspacePreflight = {
  originalPstPath: string;
  originalPstExists: boolean;
  originalPstReadable: boolean;
  pstSizeBytes: number;
  workspacePath: string;
  workspaceParentPath: string;
  workspaceLocationMode: WorkspaceLocationMode;
  workspaceLocationLabel: string;
  workspaceParentWritable: boolean;
  workspaceParentWriteError: string | null;
  availableDiskBytes: number | null;
  estimatedRequiredBytes: number;
  hasEnoughSpace: boolean | null;
  spaceWarning: boolean;
  warningRequired: boolean;
  warnings: string[];
};

export type ExistingWorkspace = {
  workspaceId: string;
  workspacePath: string;
  workspaceLocationMode: WorkspaceLocationMode;
  workspaceLocationLabel: string;
  importStatus: string;
  isComplete: boolean;
  canResume: boolean;
  canReimport: boolean;
  messageCount: number;
  canOpen: boolean;
  integrityState: WorkspaceIntegrityState;
  integrityClassification: WorkspaceIntegrityClassification;
  canRepairIndex: boolean;
  canRebuildConversations: boolean;
  resumeDetails: ResumeDetails | null;
  hasLastVerification: boolean;
};

export type PstOpenPlan = {
  pstPath: string;
  fingerprint: string;
  workspaceId: string;
  selectedWorkspacePath: string;
  selectedWorkspaceLocationMode: WorkspaceLocationMode;
  selectedWorkspaceLocationLabel: string;
  fallbackWarning: string | null;
  preflight: WorkspacePreflight;
  existingWorkspaces: ExistingWorkspace[];
};

export type DeleteResult = {
  attemptedPath: string;
  existedBefore: boolean;
  markerExisted: boolean;
  deleted: boolean;
  alreadyMissing: boolean;
  existsAfter: boolean;
  removedEmptyParent: boolean;
  parentPath: string | null;
  error: string | null;
  remainingEntries: string[];
  message: string;
};

export type BackendError = {
  message: string;
  setupCommand?: string | null;
  code?: string | null;
};
