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
};

export type Folder = {
  id: number;
  parentId: number | null;
  path: string;
  name: string;
  messageCount: number;
  directMessageCount: number;
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

export type MessagePageResult = {
  items: MessageListItem[];
  requestedOffset: number;
  returnedCount: number;
  hasMore: boolean;
  nextCursor: string | null;
  paginationMode: "cursor" | "offset";
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

export type ImportProgress = {
  stage: string;
  current: number | null;
  total: number | null;
  message: string;
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
};

export type PstOpenPlan = {
  pstPath: string;
  fingerprint: string;
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
