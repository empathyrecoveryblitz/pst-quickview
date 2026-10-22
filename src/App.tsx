import {
  Fragment,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { RemoteSupportPanel } from "./RemoteSupportPanel";
import { RemoteSupportControl } from "./RemoteSupportControl";
import { DevelopmentUpdatePanel } from "./DevelopmentUpdatePanel";
import type {
  AppDiagnostics,
  Attachment,
  BackendError,
  CancelImportResult,
  CalendarItemDetails,
  ConversationCountResult,
  ConversationMessagesResult,
  ConversationPageResult,
  ConversationSummary,
  ConversationWorkspaceIssue,
  ConversationWorkspaceScope,
  DeleteResult,
  ExistingWorkspace,
  ExternalFileOpen,
  ExternalFileOpenBatch,
  ExternalFileOpenReady,
  ExportAttachmentResult,
  ExportOriginalEmlResult,
  Folder,
  HtmlRenderResult,
  LocalTrashMutationResult,
  LocalTrashQueryHealth,
  LocalTrashResetResult,
  LocalTrashSummary,
  LocalTrashUndoState,
  MessageDiagnostics,
  MessageDetail,
  MessageCountResult,
  MessageListItem,
  MessagePageResult,
  MultiMessageCountResult,
  MultiMessagePageResult,
  PstOpenPlan,
  ReadpstStatus,
  SavePrintableHtmlResult,
  SourceEmlView,
  VerificationReport,
  WorkspaceIntegrityClassification,
  WorkspaceLocationMode,
  WorkspacePreflight,
  WorkspaceSearchCount,
  WorkspaceSize,
  WorkspaceSummary,
  MessageVisibility,
} from "./types";
import {
  completedImportDurationText,
  formatCompactDuration as formatImportDuration,
  importCounterRows,
  importEtaPresentation,
  importHeadline,
  importIdentityPresentation,
  importProgressSemantics,
  importStageExplanation,
  importStageTracker,
  lastActivityText,
  materialImportAnnouncementKey,
  materialImportAnnouncementText,
  parseImportProgressSnapshot,
  processHealthText,
  reduceImportProgress,
  stageLabel as importStageLabel,
  terminalImportPresentation,
  unitProgressText,
  type ImportCounterKey,
  type ImportProgressSnapshot,
  type ImportSourceIdentityPeer,
} from "./importProgress";
import {
  createWorkspaceIntegrityOperationToken,
  formatElapsedDuration,
  integrityOperationIsCancellable,
  integrityOperationLabel,
  integrityOperationRunningLabel,
  isVerificationReportForOperation,
  isWorkspaceIntegrityOperationCurrent,
  possibleDuplicateSummary,
  resumeProgressDescription,
  verificationHeadline,
  verificationLevelLabel,
  verificationResultLabel,
  workspaceIntegrityActions,
  workspaceIntegrityStateLabel,
  workspaceSourceAvailabilityMessage,
  workspaceSummaryWithVerification,
  type WorkspaceIntegrityOperationKind,
  type WorkspaceIntegrityOperationToken,
} from "./workspaceIntegrity";
import packageInfo from "../package.json";
import {
  appearanceStorageKey,
  applyAppearance,
  applyStoredAppearance,
  storedAppearance,
  type AppearanceMode,
} from "./appearance";
import {
  defaultWorkspaceFolderSelection,
  isLocalTrashFolderSelection,
  localTrashWorkspaceFolderSelection,
  normalizeStoredFolderSelection,
  resolveWorkspaceFolderSelection,
  type WorkspaceFolderSelection,
} from "./sessionRestore";
import {
  normalizeSearchScope,
  saveSearchScope,
  storedSearchScope,
  type SearchScope,
} from "./searchScopePreference";
import {
  appendUniqueByKey,
  appliedSearchNonTextKey,
  appliedSearchTextKey,
  applySearchSnapshotImmediately,
  backendSearchFilters,
  buildAppliedFilterChips,
  canClearAll,
  cancellationForGeneration,
  cancellationForOperation,
  classifySearchEmptyState,
  clearAllSearchDraft,
  clearAppliedSearchSnapshot,
  commitQueuedSearchSnapshot,
  createAppliedSearchSnapshot,
  createMessagePaginationState,
  createSearchExactCountState,
  createSearchOperationIdentity,
  createSearchApplicationState,
  defaultSearchFilterDraft,
  emptySearchDraft,
  getActiveFilterCount,
  getMessagePaginationMode,
  getRelevanceAvailability,
  highlightTermsForSnapshot,
  invalidateSearchApplication,
  isAppliedSearchActive,
  isExpandedConversationResponseCurrent,
  isSearchCancellationError,
  isSearchGenerationCurrent,
  queueTextSearchSnapshot,
  removeAppliedFilter,
  replaceAppliedSearchDraft,
  cursorForMessagePage,
  settleMessagePagination,
  settleSearchExactCount,
  type AppliedFilterChipId,
  type AppliedSearchSnapshot,
  type AppliedSearchVersion,
  type ConversationSearchSort as ConversationSort,
  type MessageSearchSort as SortOrder,
  type MessagePaginationMode,
  type MessagePaginationState,
  type SearchApplicationState,
  type SearchDraft,
  type SearchFilterDraft as AdvancedSearchFilters,
  type SearchFolderScope as FolderScopeFilter,
  type SearchExactCountState,
  type SearchListMode as ListMode,
  type SearchCancellationRequest,
  type SearchOperationIdentity,
} from "./searchRequest";
import {
  HighlightedText,
  MessageResultsList,
} from "./MessageResultsList";
import { ConversationResultsList } from "./ConversationResultsList";
import {
  conversationStateKey,
  type ExpandedConversationState,
} from "./conversationResultsModel";
import {
  createResultNavigationState,
  didResultNavigationContextChange,
  resetResultNavigationMode,
  setResultNavigationActiveKey,
} from "./resultNavigation";
import {
  closeOpenPopover,
  createDismissiblePopoverController,
  toggleOpenPopover,
} from "./popoverBehavior";
import {
  buildFolderHierarchy,
  resolveFolderPresentationSelection,
  saveFolderHierarchyMode,
  storedFolderHierarchyMode,
  type FolderHierarchyMode,
  type PresentationFolderNode,
} from "./folderHierarchy";
import {
  aggregateLocalTrashSummaries,
  createLocalTrashOperationToken,
  createLocalTrashResetToken,
  effectiveAllOpenLocalTrashQueryScope,
  initialLocalTrashUndoState,
  isLocalTrashMutationResponseCurrent,
  isLocalTrashResetResponseCurrent,
  isLocalTrashUndoCurrent,
  localTrashActionDisclosure,
  localTrashResetDisclosure,
  localTrashStorageDisclosure,
  reduceLocalTrashUndoState,
  type LocalTrashOperationToken,
  type LocalTrashResetToken,
} from "./localTrash";

type FolderNode = PresentationFolderNode;
type CustomPopoverId =
  | "advanced_search"
  | "recent"
  | "workspace"
  | "layout"
  | "workspace_details"
  | "import_progress"
  | "help";
type ImportAction =
  | "import"
  | "open_existing"
  | "resume_index"
  | "repair_index"
  | "reimport";
type ReaderMode = "plain_text" | "sanitized_html";
type SourceEmlMode = "rendered" | "plain_text" | "raw_source";
type LayoutMode = "three_column" | "outlook";
type MessageListDisplayMode = "subject_first" | "sender_first";
type AllOpenFolderSelection = {
  workspaceId: string | null;
  folderId: number | null;
};
type OpenPstSession = {
  workspace: WorkspaceSummary;
  workspaceSize: WorkspaceSize | null;
  folders: Folder[];
  folderSelection: WorkspaceFolderSelection;
  canonicalFolderSelection: WorkspaceFolderSelection;
  localTrashSummary: LocalTrashSummary;
  status: WorkspaceIntegrityClassification;
};
type WorkspaceOperationState = {
  progress: ImportProgressSnapshot | null;
  progressOperationId: string | null;
  progressReceivedAtMonotonicMs: number | null;
  running: boolean;
  notice: string | null;
  error: string | null;
  setupCommand: string | null;
  deleteResult: DeleteResult | null;
  deleteStatuses: string[];
  deleteConfirmOpen: boolean;
  integrityOperationKind: WorkspaceIntegrityOperationKind | null;
  integrityOperationId: string | null;
  verificationReport: VerificationReport | null;
  verificationReportOpen: boolean;
  repairConfirmOpen: boolean;
};
type WorkspaceDeleteToast = {
  workspaceId: string;
  message: string;
  result: DeleteResult;
};
type RecentPst = {
  path: string;
  lastOpenedAt: string;
};
type SavedWorkspaceSession = {
  workspaceId: string;
  pstPath: string;
  workspacePath: string;
  displayName: string;
  workspaceLocationMode: WorkspaceLocationMode;
  workspaceLocationLabel: string;
  folderSelection: WorkspaceFolderSelection;
};
type SavedSession = {
  entries: SavedWorkspaceSession[];
  lastActiveWorkspaceId: string | null;
  savedAt: string;
};
type RefreshWorkspaceOptions = {
  resetMessageList?: boolean;
  onlyIfWorkspaceStillActive?: boolean;
  folderSelection?: WorkspaceFolderSelection;
};
type PendingPreflightOpen = {
  plan: PstOpenPlan;
  existingWorkspace: ExistingWorkspace | null;
  allowDuplicate: boolean;
  importAction: ImportAction;
};
type PendingPlannedRepair = {
  plan: PstOpenPlan;
  existingWorkspace: ExistingWorkspace;
};

const emptyWorkspaceOperationState = (): WorkspaceOperationState => ({
  progress: null,
  progressOperationId: null,
  progressReceivedAtMonotonicMs: null,
  running: false,
  notice: null,
  error: null,
  setupCommand: null,
  deleteResult: null,
  deleteStatuses: [],
  deleteConfirmOpen: false,
  integrityOperationKind: null,
  integrityOperationId: null,
  verificationReport: null,
  verificationReportOpen: false,
  repairConfirmOpen: false,
});
type PrintablePreview = {
  title: string;
  modeLabel: string;
  defaultFilename: string;
  html: string;
};
type SearchLoadingState = SearchOperationIdentity & {
  mode: ListMode;
  requestId: number;
};
type PageRequestIdentity = SearchOperationIdentity & {
  offset: number;
  cursor?: string | null;
  paginationMode?: MessagePaginationMode;
  requestId: number;
};
type CountRequestIdentity = SearchOperationIdentity & {
  mode: ListMode;
  requestId: number;
};
type ExpandedConversationRequestIdentity = SearchOperationIdentity & {
  requestId: number;
};
type SourceEmlContext =
  | { kind: "workspace"; workspaceId: string; messageId: number }
  | { kind: "standalone"; stableId: string };
type PreviewWindowTarget =
  | { kind: "workspace"; workspaceId: string; messageId: number }
  | { kind: "standalone"; stableId: string };
type PreviewWindowRequest =
  | { kind: "workspace"; workspaceId: string; messageId: number }
  | { kind: "standalone"; stableId: string };
type PreviewWindowOpenResult = { created: boolean };
const missingReadpstCommand = "brew install libpst";
const workspaceLocationStorageKey = "pstQuickView.workspaceLocationMode";
const paneWidthStorageKey = "pstQuickView.paneWidths";
const folderHiddenStorageKey = "pstQuickView.folderPaneHidden";
const collapsedFolderIdentitiesStorageKey = "pstQuickView.collapsedFoldersByHierarchy";
const layoutModeStorageKey = "pstQuickView.layoutMode";
const messageListDisplayStorageKey = "pstQuickView.messageListDisplayMode";
const listModeStorageKey = "pstQuickView.listMode";
const messageResultsInstructionsId = "message-results-keyboard-instructions";
const conversationResultsInstructionsId = "conversation-results-keyboard-instructions";
const recentPstsStorageKey = "pstQuickView.recentPsts";
const previousSessionStorageKey = "pstQuickView.previousSession";
const appVersion = packageInfo.version;
const folderPaneMin = 180;
const messagePaneMin = 280;
const readerPaneMin = 420;
const outlookMessagePaneMin = 180;
const outlookReaderPaneMin = 260;
const splitterTotalWidth = 16;
const outlookSplitterHeight = 8;
const defaultPaneWidths = {
  folder: 260,
  message: 420,
  outlookMessage: 320,
};
const messagePageSize = 250;
const conversationPageSize = 100;
const conversationMessagePageSize = 100;
const searchDebounceMs = 350;
const defaultAdvancedSearchFilters: AdvancedSearchFilters = defaultSearchFilterDraft;
const advancedSearchToggleId = "advanced-search-toggle";
const advancedSearchPanelId = "advanced-search-panel";
const recentPopoverPanelId = "recent-popover-panel";
const workspacePopoverPanelId = "workspace-popover-panel";
const layoutPopoverPanelId = "layout-popover-panel";
const workspaceDetailsPanelId = "workspace-details-panel";
const importProgressDetailsPanelId = "import-progress-details-panel";
const helpDialogId = "help-dialog";
const relevanceSortHelpId = "relevance-sort-help";
const repairIndexDescriptionId = "repair-index-description";
const repairConfirmTitleId = "repair-index-confirm-title";
const repairConfirmDescriptionId = "repair-index-confirm-description";
const localTrashResetTitleId = "local-trash-reset-title";
const localTrashResetDescriptionId = "local-trash-reset-description";
const verificationReportTitleId = "verification-report-title";
const remoteImagesBlockedMessage =
  "Remote images are blocked for privacy and security.";

type PaneWidths = typeof defaultPaneWidths;

function getErrorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    return String((error as BackendError).message);
  }
  return "An unknown error occurred.";
}

function getBackendErrorCode(error: unknown): string | null {
  if (!error || typeof error !== "object" || !("code" in error)) return null;
  const code = (error as BackendError).code;
  return typeof code === "string" && code.length > 0 ? code : null;
}

function unavailableLocalTrashSummary(
  workspaceId: string,
  lastErrorCode = "LOCAL_TRASH_UNAVAILABLE",
): LocalTrashSummary {
  return {
    workspaceId,
    available: false,
    canReset: false,
    sidecarPresent: false,
    trashedMessageCount: 0,
    orphanedStateCount: 0,
    schemaVersion: null,
    stateRevision: 0,
    lastChangedAt: null,
    fingerprintMatch: false,
    lastErrorCode,
  };
}

function localTrashHealthLabel(summary: LocalTrashSummary | null): string {
  if (!summary) return "Unavailable";
  if (
    summary.available &&
    summary.fingerprintMatch &&
    summary.lastErrorCode == null
  ) return "Available";
  return "Unavailable - messages remain visible in normal views";
}

function workspaceActiveMessageCount(session: OpenPstSession): number {
  return session.folders.reduce(
    (total, folder) =>
      total +
      Math.max(
        0,
        Number.isSafeInteger(folder.activeDirectMessageCount)
          ? folder.activeDirectMessageCount
          : folder.directMessageCount,
      ),
    0,
  );
}

function isLocalTrashHealthy(summary: LocalTrashSummary | null): boolean {
  return Boolean(
    summary?.available &&
      summary.fingerprintMatch &&
      summary.lastErrorCode == null,
  );
}

function canResetLocalTrashState(summary: LocalTrashSummary | null): boolean {
  return summary?.canReset === true;
}

function errorSummary(error: string): string {
  const lower = error.toLocaleLowerCase();
  if (lower.includes("readpst")) return "PST extraction could not start or complete.";
  if (lower.includes("workspace") && lower.includes("migration")) {
    return "The workspace database could not be upgraded.";
  }
  if (lower.includes("sqlite") || lower.includes("database") || lower.includes("index")) {
    return "The searchable workspace index could not be opened or updated.";
  }
  if (lower.includes("attachment") && (lower.includes("export") || lower.includes("open"))) {
    return "The attachment could not be exported safely.";
  }
  if (lower.includes("msg")) return "The Outlook MSG file could not be read.";
  if (lower.includes("pst")) return "The PST file could not be opened.";
  if (lower.includes("not found") || lower.includes("unavailable")) {
    return "A required file or external drive is unavailable.";
  }
  const firstSentence = error.split(/(?<=[.!?])\s/, 1)[0]?.trim();
  return firstSentence && firstSentence.length <= 180
    ? firstSentence
    : "PST QuickView could not complete the operation.";
}

function diagnosticsText(diagnostics: AppDiagnostics): string {
  return [
    "PST QuickView diagnostics",
    `App version: ${diagnostics.appVersion}`,
    `macOS version: ${diagnostics.macosVersion}`,
    `CPU architecture: ${diagnostics.cpuArchitecture}`,
    `App executable architecture: ${diagnostics.executableArchitecture}`,
    `readpst source: ${diagnostics.readpstSource}`,
    `readpst version: ${diagnostics.readpstVersion}`,
    `Open PST count: ${diagnostics.openPstCount}`,
    `Active workspace mode: ${diagnostics.activeWorkspaceMode}`,
    `Active workspace path: ${diagnostics.activeWorkspacePath || "(none)"}`,
    `Database schema version: ${diagnostics.databaseSchemaVersion ?? "(unavailable)"}`,
    `Conversation data: ${diagnostics.conversationDataStatus}`,
  ].join("\n");
}

function pstDisplayName(workspace: WorkspaceSummary | null): string {
  if (!workspace) return "PST";
  const path = workspace.pstPath || workspace.workspacePath;
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || workspace.id.slice(0, 10);
}

function pathFileName(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || path || "PST";
}

function pathParent(path: string): string {
  const normalized = path.replace(/\/+$/, "");
  const index = normalized.lastIndexOf("/");
  if (index <= 0) return normalized || "/";
  return normalized.slice(0, index);
}

function isWorkspaceLocationMode(value: unknown): value is WorkspaceLocationMode {
  return value === "app_support" || value === "next_to_pst";
}

function initialRecentPsts(): RecentPst[] {
  if (typeof window === "undefined") return [];
  try {
    const stored = JSON.parse(window.localStorage.getItem(recentPstsStorageKey) ?? "[]");
    if (!Array.isArray(stored)) return [];
    return stored
      .filter((item): item is RecentPst =>
        Boolean(
          item &&
            typeof item.path === "string" &&
            typeof item.lastOpenedAt === "string" &&
            item.path.trim(),
        ),
      )
      .slice(0, 10);
  } catch {
    return [];
  }
}

function initialSavedSession(): SavedSession | null {
  if (typeof window === "undefined") return null;
  try {
    const stored = JSON.parse(window.localStorage.getItem(previousSessionStorageKey) ?? "null");
    if (!stored || !Array.isArray(stored.entries)) return null;
    const entries = stored.entries
      .filter((entry: Partial<SavedWorkspaceSession>) =>
        Boolean(
          entry &&
            typeof entry.workspaceId === "string" &&
            typeof entry.pstPath === "string" &&
            typeof entry.workspacePath === "string" &&
            typeof entry.displayName === "string" &&
            isWorkspaceLocationMode(entry.workspaceLocationMode) &&
            typeof entry.workspaceLocationLabel === "string",
        ),
      )
      .slice(0, 10)
      .map((entry: Partial<SavedWorkspaceSession>) => ({
        workspaceId: entry.workspaceId!,
        pstPath: entry.pstPath!,
        workspacePath: entry.workspacePath!,
        displayName: entry.displayName!,
        workspaceLocationMode: entry.workspaceLocationMode!,
        workspaceLocationLabel: entry.workspaceLocationLabel!,
        folderSelection: normalizeStoredFolderSelection(
          entry.workspaceId!,
          entry.folderSelection,
        ),
      }));
    if (!entries.length) return null;
    const lastActiveWorkspaceId =
      typeof stored.lastActiveWorkspaceId === "string" ? stored.lastActiveWorkspaceId : null;
    const savedAt = typeof stored.savedAt === "string" ? stored.savedAt : "";
    return { entries, lastActiveWorkspaceId, savedAt };
  } catch {
    return null;
  }
}

function recentPstsWithPath(current: RecentPst[], path: string): RecentPst[] {
  const trimmedPath = path.trim();
  if (!trimmedPath) return current;
  return [
    { path: trimmedPath, lastOpenedAt: new Date().toISOString() },
    ...current.filter((item) => item.path !== trimmedPath),
  ].slice(0, 10);
}

function workspaceToSavedSession(session: OpenPstSession): SavedWorkspaceSession {
  const { workspace } = session;
  return {
    workspaceId: workspace.id,
    pstPath: workspace.pstPath,
    workspacePath: workspace.workspacePath,
    displayName: pstDisplayName(workspace),
    workspaceLocationMode: workspace.workspaceLocationMode,
    workspaceLocationLabel: workspace.workspaceLocationLabel,
    folderSelection: session.folderSelection,
  };
}

function savedSessionFromOpenSessions(
  sessions: OpenPstSession[],
  activeWorkspaceId: string | null,
): SavedSession | null {
  const entries = sessions.map(workspaceToSavedSession);
  if (!entries.length) return null;
  return {
    entries,
    lastActiveWorkspaceId: activeWorkspaceId,
    savedAt: new Date().toISOString(),
  };
}

function getSetupCommand(error: unknown): string | null {
  if (error && typeof error === "object" && "setupCommand" in error) {
    return (error as BackendError).setupCommand ?? null;
  }
  return null;
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), Math.max(min, max));
}

function initialPaneWidths(): PaneWidths {
  if (typeof window === "undefined") return defaultPaneWidths;

  try {
    const stored = JSON.parse(window.localStorage.getItem(paneWidthStorageKey) ?? "");
    const folder = Number(stored?.folder);
    const message = Number(stored?.message);
    const outlookMessage = Number(stored?.outlookMessage);
    if (Number.isFinite(folder) && Number.isFinite(message)) {
      return {
        folder: clamp(folder, folderPaneMin, 520),
        message: clamp(message, messagePaneMin, 720),
        outlookMessage: Number.isFinite(outlookMessage)
          ? clamp(outlookMessage, outlookMessagePaneMin, 900)
          : defaultPaneWidths.outlookMessage,
      };
    }
  } catch {
    // Ignore malformed localStorage and fall back to defaults.
  }

  return defaultPaneWidths;
}

function initialBooleanPreference(key: string, fallback: boolean): boolean {
  if (typeof window === "undefined") return fallback;
  const stored = window.localStorage.getItem(key);
  if (stored === "true") return true;
  if (stored === "false") return false;
  return fallback;
}

type CollapsedFolderIdentitiesByMode = Record<FolderHierarchyMode, Set<string>>;

function initialCollapsedFolderIdentities(): CollapsedFolderIdentitiesByMode {
  if (typeof window === "undefined") {
    return { simplified: new Set(), original: new Set() };
  }
  try {
    const stored = JSON.parse(
      window.localStorage.getItem(collapsedFolderIdentitiesStorageKey) ?? "{}",
    ) as Partial<Record<FolderHierarchyMode, unknown>>;
    const identities = (value: unknown) =>
      new Set(Array.isArray(value) ? value.filter((item): item is string => typeof item === "string") : []);
    return {
      simplified: identities(stored.simplified),
      original: identities(stored.original),
    };
  } catch {
    return { simplified: new Set(), original: new Set() };
  }
}

function initialLayoutMode(): LayoutMode {
  if (typeof window === "undefined") return "three_column";
  return window.localStorage.getItem(layoutModeStorageKey) === "outlook"
    ? "outlook"
    : "three_column";
}

function initialMessageListDisplayMode(): MessageListDisplayMode {
  if (typeof window === "undefined") return "subject_first";
  return window.localStorage.getItem(messageListDisplayStorageKey) === "sender_first"
    ? "sender_first"
    : "subject_first";
}

function initialListMode(): ListMode {
  if (typeof window === "undefined") return "messages";
  return window.localStorage.getItem(listModeStorageKey) === "conversations"
    ? "conversations"
    : "messages";
}

function folderCollapseKey(folder: FolderNode, _workspaceId?: string | null): string {
  return folder.canonicalIdentity;
}

function workspaceRootCollapseKey(workspaceId: string): string {
  return `workspace:${workspaceId.length}:${workspaceId}`;
}

function collectCollapsibleFolderKeys(
  nodes: readonly FolderNode[],
  _workspaceId?: string | null,
): string[] {
  const keys: string[] = [];
  const pending = [...nodes];
  while (pending.length) {
    const item = pending.pop()!;
    if (item.children.length) keys.push(folderCollapseKey(item));
    pending.push(...item.children);
  }
  return keys;
}

function formatCount(count: number): string {
  return new Intl.NumberFormat().format(count);
}

function formatCandidateProgress(progress: { current: number; total: number }): string {
  return `Processed ${formatCount(progress.current)} of ${formatCount(progress.total)} extracted message candidates`;
}

function BreakableFilesystemPath({ path }: { path: string }) {
  const segments = path.split("/");

  return (
    <span className="about-filesystem-path" title={path}>
      {segments.map((segment, index) => (
        <Fragment key={`${index}:${segment}`}>
          {index > 0 ? (
            <>
              /
              <wbr />
            </>
          ) : null}
          {segment}
        </Fragment>
      ))}
    </span>
  );
}

function formatBytes(bytes: number): string {
  const units = ["B", "KiB", "MiB", "GiB", "TiB"];
  let value = bytes;
  let unitIndex = 0;

  while (value >= 1024 && unitIndex < units.length - 1) {
    value /= 1024;
    unitIndex += 1;
  }

  return unitIndex === 0 ? `${bytes} ${units[unitIndex]}` : `${value.toFixed(1)} ${units[unitIndex]}`;
}

function formatOptionalBytes(bytes: number | null): string {
  return bytes == null ? "Unknown" : formatBytes(bytes);
}

function formatMaybeBytes(bytes: number | null): string {
  return bytes == null ? "" : formatBytes(bytes);
}

const importStageCounterKeys: Record<
  ImportProgressSnapshot["stage"],
  readonly ImportCounterKey[]
> = {
  preflight: [],
  extracting: ["outputFilesObserved", "outputBytesObserved", "workspaceBytesWritten"],
  discovering: ["candidatesDiscovered"],
  indexing: [
    "candidatesProcessed",
    "canonicalMessages",
    "insertedMessages",
    "skippedExistingMessages",
    "parseFailures",
    "attachmentsIndexed",
  ],
  building_conversations: ["conversationRecordsProcessed", "conversationRecordsTotal"],
  verifying: [
    "verificationChecksCompleted",
    "verificationChecksTotal",
    "missingCanonicalRecords",
    "unexpectedCanonicalRecords",
    "possibleDuplicateGroups",
  ],
  complete: [
    "candidatesDiscovered",
    "canonicalMessages",
    "attachmentsIndexed",
    "missingCanonicalRecords",
    "unexpectedCanonicalRecords",
    "possibleDuplicateGroups",
  ],
};

function monotonicMilliseconds(): number {
  return typeof performance === "undefined" ? 0 : performance.now();
}

function importWarningText(code: ImportProgressSnapshot["warningCodes"][number]): string {
  switch (code) {
    case "no_recent_activity_signal":
      return "No recent measurable activity signal.";
    case "source_unavailable":
      return "The source PST is currently unavailable.";
    case "source_changed":
      return "The available source no longer matches the recorded identity.";
    case "possible_duplicate_groups":
      return "Possible duplicate source-record groups were observed; no records were suppressed.";
    case "verification_incomplete":
      return "Internal verification is incomplete.";
    case "progress_signal_unavailable":
      return "This stage does not provide a measurable progress signal.";
    case "prior_timing_unavailable":
      return "Only active time recorded since this workspace entered Phase 3 is available.";
  }
}

function yesNo(value: boolean): string {
  return value ? "Yes" : "No";
}

function formatMessageDate(value: string | null | undefined): string {
  if (!value) return "";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return value;

  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
    year: "numeric",
    hour: "numeric",
    minute: "2-digit",
  }).format(date);
}

function escapeHtml(value: string): string {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&#39;");
}

function makeSanitizedHtmlPrintable(html: string): string {
  const template = document.createElement("template");
  template.innerHTML = html;
  template.content.querySelectorAll("a[href]").forEach((link) => {
    const href = link.getAttribute("href");
    if (href && !link.getAttribute("title")) link.setAttribute("title", href);
    link.removeAttribute("href");
  });
  return template.innerHTML;
}

function plainTextPrintBody(text: string): string {
  return `<pre class="plain-body">${escapeHtml(text || "No message body found.")}</pre>`;
}

type PrintableMessage = {
  subject: string;
  sender: string;
  recipients: string;
  date: string;
  attachmentCount: number;
  attachments: Attachment[];
  calendar: CalendarItemDetails | null;
};

function printableCalendarRows(calendar: CalendarItemDetails): string {
  const rows: Array<[string, string]> = [
    ["Item type", calendar.itemType],
    [calendar.allDay ? "Date" : "Start", formatCalendarValue(calendar.start, calendar.allDay)],
    ["End", formatCalendarValue(calendar.end, calendar.allDay)],
    ["Time zone", calendar.timeZone],
    ["Location", calendar.location],
    ["Organizer", calendar.organizer],
    ["Required attendees", calendar.requiredAttendees],
    ["Optional attendees", calendar.optionalAttendees],
    ["Resources", calendar.resources],
    ["Status", calendar.meetingStatus],
    ["Response", calendar.responseStatus],
    ["Recurrence", calendar.recurrenceSummary],
    ["Reminder", calendar.reminder],
    ["Sensitivity", calendar.sensitivity],
    ["Categories", calendar.categories.join(", ")],
  ];
  return rows
    .filter(([, value]) => value.trim())
    .map(([label, value]) => `<dt>${escapeHtml(label)}</dt><dd>${escapeHtml(value)}</dd>`)
    .join("");
}

function buildPrintDocument(
  message: PrintableMessage,
  bodyHtml: string,
  bodyModeLabel: string,
  note: string | null,
): string {
  const attachmentItems = message.attachments.length
    ? message.attachments
        .map((attachment) => {
          const size =
            attachment.sizeBytes != null ? `, ${escapeHtml(formatBytes(attachment.sizeBytes))}` : "";
          return `<li>${escapeHtml(attachment.filename || "(Unnamed attachment)")} <span>${escapeHtml(
            attachment.contentType || "unknown type",
          )}${size}</span></li>`;
        })
        .join("")
    : "<li>No attachments</li>";

  const headerRows = message.calendar
    ? printableCalendarRows(message.calendar)
    : `<dt>From</dt><dd>${escapeHtml(message.sender || "(No sender)")}</dd>
    <dt>To/Cc/Bcc</dt><dd>${escapeHtml(message.recipients || "(No recipients)")}</dd>
    <dt>Date</dt><dd>${escapeHtml(formatMessageDate(message.date) || message.date || "(No date)")}</dd>`;

  return `<!doctype html>
<html>
<head>
  <meta charset="utf-8">
  <title>${escapeHtml(message.subject || "Message")}</title>
  <style>
    @page { margin: 0.6in; }
    body {
      margin: 0;
      color: #111827;
      font: 13px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
    }
    h1 {
      margin: 0 0 14px;
      font-size: 20px;
      line-height: 1.25;
      overflow-wrap: anywhere;
    }
    .headers {
      display: grid;
      grid-template-columns: max-content minmax(0, 1fr);
      gap: 4px 12px;
      margin: 0 0 14px;
      padding-bottom: 12px;
      border-bottom: 1px solid #d8dee6;
    }
    .headers dt {
      color: #5f6b7a;
      font-weight: 700;
    }
    .headers dd {
      margin: 0;
      overflow-wrap: anywhere;
    }
    .note {
      margin: 0 0 12px;
      color: #5f6b7a;
      font-size: 12px;
    }
    .attachments {
      margin: 0 0 16px;
      padding: 10px 12px;
      border: 1px solid #d8dee6;
      border-radius: 6px;
      break-inside: avoid;
    }
    .attachments h2 {
      margin: 0 0 6px;
      font-size: 13px;
    }
    .attachments ul {
      margin: 0;
      padding-left: 18px;
    }
    .attachments span {
      color: #5f6b7a;
    }
    .body {
      overflow-wrap: anywhere;
    }
    .plain-body {
      margin: 0;
      white-space: pre-wrap;
      overflow-wrap: anywhere;
      font: inherit;
    }
    .html-body, .html-body * {
      box-sizing: border-box;
      max-width: 100%;
    }
    .html-body img {
      max-width: 100%;
      height: auto;
    }
    .html-body table {
      max-width: 100%;
      border-collapse: collapse;
    }
    .html-body a {
      color: #1f5f9f;
      text-decoration: underline;
    }
  </style>
</head>
<body>
  <h1>${escapeHtml(message.subject || "(No subject)")}</h1>
  <dl class="headers">
    ${headerRows}
    <dt>Reader</dt><dd>${escapeHtml(bodyModeLabel)}</dd>
  </dl>
  ${note ? `<p class="note">${escapeHtml(note)}</p>` : ""}
  <section class="attachments">
    <h2>Attachments (${formatCount(message.attachmentCount)})</h2>
    <ul>${attachmentItems}</ul>
  </section>
  <section class="body">${bodyHtml}</section>
</body>
</html>`;
}

function printableDatePrefix(date: string): string {
  if (/^\d{4}-\d{2}-\d{2}/.test(date)) return date.slice(0, 10);
  return "undated";
}

function printableSubjectSlug(subject: string): string {
  const cleaned = subject
    .trim()
    .replace(/[^\w\s.-]+/g, " ")
    .replace(/\s+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 90);
  return cleaned || "message";
}

function printableDefaultFilename(
  subject: string,
  date: string,
  modeLabel: string,
  messageId?: number,
): string {
  const suffix = modeLabel.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
  const idPart = messageId == null ? "" : `-${messageId}`;
  return `${printableDatePrefix(date)}-${printableSubjectSlug(subject)}${idPart}-${suffix || "printable"}.html`;
}

function cleanMessageSnippet(value: string | null | undefined): string {
  if (!value) return "";

  return value
    .replace(/\s+/g, " ")
    .replace(
      /(?:https?:\/\/|www\.)\S+|\b\S*(?:proofpoint|urldefense|mailchimp|list-manage|listmanage)\S*/gi,
      (match) =>
        /proofpoint|urldefense|mailchimp|list-manage|listmanage/i.test(match)
          ? "[tracking link]"
          : "[link]",
    )
    .replace(/(?:\[tracking link\]\s*){2,}/g, "[tracking link] ")
    .replace(/(?:\[link\]\s*){2,}/g, "[link] ")
    .trim();
}

function initialWorkspaceLocationMode(): WorkspaceLocationMode {
  const stored = window.localStorage.getItem(workspaceLocationStorageKey);
  return stored === "app_support" ? "app_support" : "next_to_pst";
}

function cacheLocationMessage(plan: PstOpenPlan): string {
  const locationMessage = plan.fallbackWarning
    ? plan.fallbackWarning
    : plan.selectedWorkspaceLocationMode === "next_to_pst"
      ? "The searchable workspace/cache will be stored next to the PST in a hidden Spotlight-safe .noindex folder."
      : "The searchable workspace/cache will be stored in local Mac App Support.";
  return `${locationMessage} ${preflightSummary(plan.preflight)}`;
}

function preflightSummary(preflight: WorkspacePreflight): string {
  return `Original PST: ${preflight.originalPstPath}. Workspace/cache: ${preflight.workspacePath}. Mode: ${preflight.workspaceLocationLabel}. Estimated cache: ${formatBytes(preflight.estimatedRequiredBytes)}. Available: ${formatOptionalBytes(preflight.availableDiskBytes)}.`;
}

function importActionNeedsPreflight(importAction: ImportAction): boolean {
  return importAction === "import" || importAction === "reimport";
}

function canContinueAfterPreflight(preflight: WorkspacePreflight): boolean {
  return (
    preflight.originalPstExists &&
    preflight.originalPstReadable &&
    preflight.workspaceParentWritable
  );
}

function preflightPrimaryLabel(preflight: WorkspacePreflight): string {
  if (!canContinueAfterPreflight(preflight)) return "Continue";
  return preflight.spaceWarning ? "Continue Anyway" : "Continue";
}

function shouldOfferAppSupportFallback(pending: PendingPreflightOpen): boolean {
  return (
    pending.importAction === "import" &&
    pending.plan.selectedWorkspaceLocationMode !== "app_support"
  );
}

function formatBodyForDisplay(body: string): string {
  const lines = body
    .replace(/\r\n/g, "\n")
    .replace(/\r/g, "\n")
    .split("\n");
  const normalized: string[] = [];
  let blankCount = 0;

  for (const rawLine of lines) {
    const line = rawLine.replace(/[ \t]+$/g, "");
    if (line.trim().length === 0) {
      blankCount += 1;
      if (blankCount <= 2) normalized.push("");
    } else {
      blankCount = 0;
      normalized.push(line);
    }
  }

  while (normalized[0] === "") normalized.shift();
  while (normalized[normalized.length - 1] === "") normalized.pop();

  return normalized.join("\n");
}

function bodyStatusMessage(message: MessageDetail, displayBody: string): string | null {
  if (message.bodySource === "parse_error") return "Body could not be parsed.";
  if (!displayBody.trim() || message.bodySource === "missing") return "No message body found.";
  if (message.bodySource === "html_converted") return "HTML-only message converted to plain text.";
  if (message.bodySource === "rtf_converted") return "RTF message body converted to plain text.";
  if (message.bodySource === "rtf_html_converted") {
    return "Outlook Rich Text message recovered from embedded HTML.";
  }
  return null;
}

function sourceBodyStatusMessage(view: SourceEmlView, displayBody: string): string | null {
  if (view.bodySource === "parse_error") return "Body could not be parsed.";
  if (!displayBody.trim() || view.bodySource === "missing") return "No message body found.";
  if (view.bodySource === "html_converted") return "HTML-only message converted to plain text.";
  if (view.bodySource === "rtf_converted") return "RTF message body converted to plain text.";
  if (view.bodySource === "rtf_html_converted") {
    return "Outlook Rich Text message recovered from embedded HTML.";
  }
  return null;
}

function sourceReconstructionWarnings(view: SourceEmlView): string[] {
  const incompletePattern =
    /could not|failed|malformed|unsupported|unavailable|no readable|could not be placed|limited preview/i;
  return view.parseWarnings.filter((warning) => incompletePattern.test(warning));
}

function formatCalendarValue(value: string, allDay: boolean | null): string {
  const raw = value.trim();
  if (!raw) return "";
  if (allDay) {
    const match = raw.match(/^(\d{4})-(\d{2})-(\d{2})/);
    if (match) {
      const localDate = new Date(Number(match[1]), Number(match[2]) - 1, Number(match[3]));
      return new Intl.DateTimeFormat(undefined, {
        year: "numeric",
        month: "short",
        day: "numeric",
      }).format(localDate);
    }
  }
  return formatMessageDate(raw) || raw;
}

function CalendarMessageHeader({ calendar, subject }: { calendar: CalendarItemDetails; subject: string }) {
  const fields: Array<[string, string, string?]> = [
    [calendar.allDay ? "Date" : "Start", formatCalendarValue(calendar.start, calendar.allDay), calendar.startRaw],
    ["End", formatCalendarValue(calendar.end, calendar.allDay), calendar.endRaw],
    ["Time zone", calendar.timeZone, calendar.timeZoneSource],
    ["Location", calendar.location],
    ["Organizer", calendar.organizer],
    ["Required", calendar.requiredAttendees],
    ["Optional", calendar.optionalAttendees],
    ["Resources", calendar.resources],
    ["Status", calendar.meetingStatus],
    ["Response", calendar.responseStatus],
    ["Recurrence", calendar.recurrenceSummary],
    ["Reminder", calendar.reminder],
    ["Sensitivity", calendar.sensitivity],
    ["Categories", calendar.categories.join(", ")],
  ];
  return (
    <>
      <section className="calendar-message-card" aria-label={`${calendar.itemType} details`}>
        <div className="calendar-message-title">
          <span>{calendar.itemType}</span>
          {calendar.allDay ? <small>All day</small> : null}
        </div>
        <h3>{subject || "(No subject)"}</h3>
        <dl>
          {fields
            .filter(([, value]) => value.trim())
            .map(([label, value, title]) => (
              <div key={label}>
                <dt>{label}</dt>
                <dd title={title || undefined}>
                  {value}
                  {label === "Time zone" && calendar.timeZoneUncertain ? (
                    <small>Source time zone uncertain</small>
                  ) : null}
                </dd>
              </div>
            ))}
        </dl>
      </section>
      <details className="message-reconstruction-details calendar-diagnostics">
        <summary>Calendar diagnostics</summary>
        <dl className="diagnostics-grid">
          <div>
            <dt>Message class</dt>
            <dd>{calendar.messageClass}</dd>
          </div>
          <div>
            <dt>Start raw</dt>
            <dd>{calendar.startRaw || "(missing)"}</dd>
          </div>
          <div>
            <dt>End raw</dt>
            <dd>{calendar.endRaw || "(missing)"}</dd>
          </div>
          <div>
            <dt>Time-zone source</dt>
            <dd>{calendar.timeZoneSource}</dd>
          </div>
          <div>
            <dt>Organizer source</dt>
            <dd>{calendar.organizerSource || "(missing)"}</dd>
          </div>
          <div>
            <dt>Required attendee source</dt>
            <dd>{calendar.requiredAttendeesSource || "(missing)"}</dd>
          </div>
          <div>
            <dt>Optional attendee source</dt>
            <dd>{calendar.optionalAttendeesSource || "(missing)"}</dd>
          </div>
          <div>
            <dt>Created</dt>
            <dd>{calendar.creationTime || "(missing)"}</dd>
          </div>
          <div>
            <dt>Modified</dt>
            <dd>{calendar.modificationTime || "(missing)"}</dd>
          </div>
        </dl>
        {calendar.propertyDiagnostics.length ? (
          <div className="calendar-property-table-wrap">
            <table className="calendar-property-table">
              <thead>
                <tr>
                  <th>Property</th>
                  <th>ID</th>
                  <th>Type</th>
                  <th>Value</th>
                  <th>Source</th>
                </tr>
              </thead>
              <tbody>
                {calendar.propertyDiagnostics.map((property) => (
                  <tr key={`${property.propertyId}-${property.name}`}>
                    <td>{property.name}</td>
                    <td>{property.propertyId}</td>
                    <td>{property.propertyType}</td>
                    <td>{property.value}</td>
                    <td>{property.source}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        ) : null}
        {calendar.parseWarnings.length ? (
          <ul>
            {calendar.parseWarnings.map((warning) => (
              <li key={warning}>{warning}</li>
            ))}
          </ul>
        ) : null}
        {calendar.unsupportedProperties.length ? (
          <ul>
            {calendar.unsupportedProperties.map((warning) => (
              <li key={warning}>{warning}</li>
            ))}
          </ul>
        ) : null}
      </details>
    </>
  );
}

function SourceMessageMetadata({ view }: { view: SourceEmlView }) {
  if (view.calendar) {
    return <CalendarMessageHeader calendar={view.calendar} subject={view.subject} />;
  }
  return (
    <>
      <dl className="headers source-eml-headers">
        <div>
          <dt>Source</dt>
          <dd>{view.sourceLabel}</dd>
        </div>
        {view.messageClass ? (
          <div>
            <dt>Class</dt>
            <dd>{view.messageClass}</dd>
          </div>
        ) : null}
        <div>
          <dt>Subject</dt>
          <dd>{view.subject || "(No subject)"}</dd>
        </div>
        <div>
          <dt>From</dt>
          <dd>{view.sender || "(No sender)"}</dd>
        </div>
        <div>
          <dt>To/Cc/Bcc</dt>
          <dd>{view.recipients || "(No recipients)"}</dd>
        </div>
        <div>
          <dt>Date</dt>
          <dd title={view.date || undefined}>{formatMessageDate(view.date) || "(No date)"}</dd>
        </div>
      </dl>
      <ThreadingHeaderDetails view={view} />
    </>
  );
}

function attachmentContentLabel(attachment: Attachment): string {
  const filename = attachment.filename.toLocaleLowerCase();
  const calendarData =
    attachment.contentType.toLocaleLowerCase() === "text/calendar" || filename.endsWith(".ics");
  return calendarData
    ? `${attachment.contentType || "text/calendar"} - Calendar data`
    : attachment.contentType || "unknown type";
}

function ThreadingHeaderDetails({ view }: { view: SourceEmlView }) {
  if (
    !view.messageIdHeader &&
    !view.inReplyTo &&
    !view.referencesHeader &&
    !view.normalizedSubject
  ) {
    return null;
  }
  return (
    <details className="message-reconstruction-details threading-header-details">
      <summary>Threading headers</summary>
      <dl className="diagnostics-grid">
        <div>
          <dt>Message-ID</dt>
          <dd>{view.messageIdHeader || "(missing)"}</dd>
        </div>
        <div>
          <dt>In-Reply-To</dt>
          <dd>{view.inReplyTo || "(missing)"}</dd>
        </div>
        <div>
          <dt>References</dt>
          <dd>{view.referencesHeader || "(missing)"}</dd>
        </div>
        <div>
          <dt>Normalized subject</dt>
          <dd>{view.normalizedSubject || "(missing)"}</dd>
        </div>
      </dl>
    </details>
  );
}

function diagnosticNotes(diagnostics: MessageDiagnostics): string[] {
  const notes = [...diagnostics.parseWarnings];
  if (diagnostics.rtfBodyPromoted) {
    notes.push("Outlook RTF body promoted from a body-like RTF part.");
  }
  if (diagnostics.rtfBodySuppressedFromAttachments) {
    notes.push("Body RTF hidden from attachment list.");
  }
  if (!diagnostics.hasBodyHtml) {
    notes.push("No HTML body found.");
  }
  if (diagnostics.bodySource === "rtf_html_converted") {
    notes.push("HTML recovered from Outlook RTF.");
  }
  if (diagnostics.remoteImagesDetected) {
    notes.push("Remote images detected; they remain blocked for privacy and security.");
  }
  if (diagnostics.cidImagesDetected) {
    notes.push("CID/embedded image references detected.");
  }

  return Array.from(new Set(notes));
}

function threadAssignmentLabel(method: string): string {
  switch (method) {
    case "header":
      return "Header (In-Reply-To)";
    case "references":
      return "References";
    case "subject_fallback":
      return "Conservative subject fallback (heuristic)";
    default:
      return "Standalone";
  }
}

function pendingOpenHeading(plan: PstOpenPlan): string {
  const hasComplete = plan.existingWorkspaces.some((workspace) => workspace.isComplete);
  const hasIncomplete = plan.existingWorkspaces.some((workspace) => !workspace.isComplete);

  if (hasComplete && hasIncomplete) return "Indexed and incomplete workspaces found.";
  if (hasComplete) return "This PST appears to already be indexed.";
  if (hasIncomplete) return "Incomplete import workspace found.";
  return "No existing workspace found.";
}

function FolderRows({
  nodes,
  workspaceId,
  selectedWorkspaceId,
  selectedFolderId,
  includeSubfolders,
  collapsedFolderPaths,
  onToggleCollapse,
  onSelect,
  level = 0,
}: {
  nodes: readonly FolderNode[];
  workspaceId: string;
  selectedWorkspaceId: string | null;
  selectedFolderId: number | null;
  includeSubfolders: boolean;
  collapsedFolderPaths: Set<string>;
  onToggleCollapse: (folder: FolderNode, workspaceId: string) => void;
  onSelect: (folder: FolderNode, workspaceId: string) => void;
  level?: number;
}) {
  return (
    <>
      {nodes.map((node) => {
        const isCollapsed = collapsedFolderPaths.has(folderCollapseKey(node, workspaceId));
        const isSelected =
          selectedWorkspaceId === workspaceId && selectedFolderId === node.canonicalFolderId;
        return (
          <div key={node.presentationId}>
            <div
              className={`folder-row folder-row-composite ${isSelected ? "selected" : ""}`}
              style={{ paddingLeft: `${12 + level * 14}px` }}
            >
              <button
                type="button"
                className="folder-toggle-button"
                data-support-control="click"
                onClick={() => onToggleCollapse(node, workspaceId)}
                disabled={!node.children.length}
                title={isCollapsed ? "Expand folder" : "Collapse folder"}
                aria-label={`${isCollapsed ? "Expand" : "Collapse"} ${node.displayName}`}
                aria-expanded={node.children.length ? !isCollapsed : undefined}
              >
                {node.children.length ? (isCollapsed ? ">" : "v") : ""}
              </button>
              <button
                type="button"
                className="folder-select-button"
                data-support-control="click"
                onClick={() => onSelect(node, workspaceId)}
                title={node.originalPath || "PST root"}
                aria-current={isSelected ? "true" : undefined}
              >
                <span className="folder-name">{node.displayName}</span>
                <span className="folder-count">
                  {includeSubfolders ? node.descendantMessageCount : node.directMessageCount}
                </span>
              </button>
            </div>
            {node.children.length > 0 && !isCollapsed ? (
              <FolderRows
                nodes={node.children}
                workspaceId={workspaceId}
                selectedWorkspaceId={selectedWorkspaceId}
                selectedFolderId={selectedFolderId}
                includeSubfolders={includeSubfolders}
                collapsedFolderPaths={collapsedFolderPaths}
                onToggleCollapse={onToggleCollapse}
                onSelect={onSelect}
                level={level + 1}
              />
            ) : null}
          </div>
        );
      })}
    </>
  );
}

function isMessagePreviewRoute(): boolean {
  if (typeof window === "undefined") return false;
  const params = new URLSearchParams(window.location.search);
  return params.get("messagePreview") === "1";
}

function sourceMessageFormatLabel(view: SourceEmlView | null): "EML" | "MSG" {
  return view?.sourceFormat === "msg" ? "MSG" : "EML";
}

function rawSourcePrintNote(view: SourceEmlView): string {
  return view.sourceFormat === "msg"
    ? "Printed structured MSG properties and diagnostics. No remote resources were loaded."
    : "Printed raw EML source text. No remote resources were loaded.";
}

function rawSourceSearchLabel(view: SourceEmlView): string {
  return view.sourceFormat === "msg"
    ? "Search structured MSG properties"
    : "Search raw EML source";
}

function MessagePreviewBootstrap() {
  const [target, setTarget] = useState<PreviewWindowTarget | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let current = true;
    invoke<PreviewWindowTarget>("get_preview_window_target")
      .then((result) => {
        if (current) setTarget(result);
      })
      .catch((err) => {
        if (current) setError(getErrorMessage(err));
      });
    return () => {
      current = false;
    };
  }, []);

  if (target) return <MessagePreviewWindow target={target} />;
  return (
    <main className="preview-window-shell">
      <section className="source-eml-modal source-eml-window" aria-label="Message Preview">
        <p className={error ? "message-action-status action-error" : "message-action-status"}>
          {error ?? "Loading message preview."}
        </p>
      </section>
    </main>
  );
}

function MessagePreviewWindow({ target }: { target: PreviewWindowTarget }) {
  const [sourceEmlView, setSourceEmlView] = useState<SourceEmlView | null>(null);
  const [sourceEmlMode, setSourceEmlMode] = useState<SourceEmlMode>("rendered");
  const [sourceEmlStatus, setSourceEmlStatus] = useState<string | null>("Loading message preview.");
  const [sourceEmlSaveResult, setSourceEmlSaveResult] = useState<ExportOriginalEmlResult | null>(
    null,
  );
  const [sourceEmlRawSearch, setSourceEmlRawSearch] = useState("");
  const [printStatus, setPrintStatus] = useState<string | null>(null);
  const [printableSaveResult, setPrintableSaveResult] = useState<SavePrintableHtmlResult | null>(
    null,
  );
  const [isSavingPrintable, setIsSavingPrintable] = useState(false);
  const [exportResults, setExportResults] = useState<Record<number, ExportAttachmentResult>>({});
  const [exportingAttachmentId, setExportingAttachmentId] = useState<number | null>(null);
  const [openingAttachmentId, setOpeningAttachmentId] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const applyStored = () =>
      applyStoredAppearance(window.localStorage, document.documentElement);
    const handleStorage = (event: StorageEvent) => {
      if (event.key === appearanceStorageKey) applyStored();
    };
    applyStored();
    window.addEventListener("storage", handleStorage);
    return () => window.removeEventListener("storage", handleStorage);
  }, []);

  async function loadSourceEml() {
    setError(null);
    setSourceEmlStatus("Loading message preview.");
    try {
      const result =
        target.kind === "workspace"
          ? await invoke<SourceEmlView>("get_source_eml_view", {
              workspaceId: target.workspaceId,
              messageId: target.messageId,
            })
          : await invoke<SourceEmlView>("get_standalone_message_view", {
              stableId: target.stableId,
            });
      setSourceEmlView(result);
      setSourceEmlStatus(null);
      const windowTitle = result.subject || "Message Preview";
      document.title = windowTitle;
      await getCurrentWindow().setTitle(windowTitle);
    } catch (err) {
      setError(getErrorMessage(err));
      setSourceEmlStatus(null);
    }
  }

  useEffect(() => {
    void loadSourceEml();
  }, [target.kind, target.kind === "workspace" ? target.workspaceId : target.stableId]);

  function buildPreviewPrintable(): PrintablePreview | null {
    if (!sourceEmlView) return null;

    let bodyHtml = "";
    let bodyModeLabel = "Rendered View";
    let note: string | null = null;

    if (sourceEmlMode === "raw_source") {
      bodyHtml = plainTextPrintBody(sourceEmlView.rawSource);
      bodyModeLabel = "Raw Source";
      note = rawSourcePrintNote(sourceEmlView);
    } else if (sourceEmlMode === "plain_text") {
      bodyHtml = plainTextPrintBody(formatBodyForDisplay(sourceEmlView.bodyText));
      bodyModeLabel = "Plain Text";
    } else if (sourceEmlView.sanitizedHtml.trim()) {
      bodyHtml = `<div class="html-body">${makeSanitizedHtmlPrintable(sourceEmlView.sanitizedHtml)}</div>`;
      bodyModeLabel = "Rendered View";
      if (sourceEmlView.remoteImagesBlocked) {
        note = "Remote images were blocked for this print.";
      }
    } else {
      bodyHtml = plainTextPrintBody(formatBodyForDisplay(sourceEmlView.bodyText));
      bodyModeLabel = "Rendered View fallback";
      note = "No sanitized HTML body was available. Printed converted plain text.";
    }

    return {
      title: sourceEmlView.subject || "(No subject)",
      modeLabel: bodyModeLabel,
      defaultFilename: printableDefaultFilename(
        sourceEmlView.subject || "message",
        sourceEmlView.date,
        bodyModeLabel,
        sourceEmlView.messageId,
      ),
      html: buildPrintDocument(
        {
          subject: sourceEmlView.subject,
          sender: sourceEmlView.sender,
          recipients: sourceEmlView.recipients,
          date: sourceEmlView.date,
          attachmentCount: sourceEmlView.attachments.length,
          attachments: sourceEmlView.attachments,
          calendar: sourceEmlView.calendar,
        },
        bodyHtml,
        bodyModeLabel,
        note,
      ),
    };
  }

  async function savePrintableHtmlAs(preview: PrintablePreview) {
    setError(null);
    setPrintStatus("Choose where to save the printable HTML.");
    setPrintableSaveResult(null);
    setSourceEmlSaveResult(null);
    setIsSavingPrintable(true);
    try {
      const result = await invoke<SavePrintableHtmlResult>("save_printable_html_as", {
        defaultFilename: preview.defaultFilename,
        html: preview.html,
      });
      setPrintableSaveResult(result);
      if (result.saved) {
        setPrintStatus(`Saved printable HTML to ${result.outputPath}`);
      } else if (result.error) {
        setError(result.error);
        setPrintStatus(null);
      } else {
        setPrintStatus("Save Printable HTML cancelled.");
      }
    } catch (err) {
      setError(getErrorMessage(err));
      setPrintStatus(null);
    } finally {
      setIsSavingPrintable(false);
    }
  }

  async function saveSourcePrintableHtml() {
    const preview = buildPreviewPrintable();
    if (!preview) return;
    await savePrintableHtmlAs(preview);
  }

  async function saveSourceEmlAs() {
    const sourceFormatLabel = sourceMessageFormatLabel(sourceEmlView);
    setError(null);
    setSourceEmlStatus(`Choose where to save the source ${sourceFormatLabel}.`);
    setSourceEmlSaveResult(null);
    setPrintableSaveResult(null);
    setPrintStatus(null);
    try {
      const result =
        target.kind === "workspace"
          ? await invoke<ExportOriginalEmlResult>("save_source_eml_as", {
              workspaceId: target.workspaceId,
              messageId: target.messageId,
            })
          : await invoke<ExportOriginalEmlResult>("save_standalone_source_message_as", {
              stableId: target.stableId,
            });
      setSourceEmlSaveResult(result);
      if (result.exported) {
        setSourceEmlStatus(`Saved source ${sourceFormatLabel} to ${result.outputPath}`);
      } else if (result.error) {
        setError(result.error);
        setSourceEmlStatus(null);
      } else {
        setSourceEmlStatus(`Save Source ${sourceFormatLabel} As... cancelled.`);
      }
    } catch (err) {
      setError(getErrorMessage(err));
      setSourceEmlStatus(null);
    }
  }

  async function exportAttachment(attachmentId: number) {
    setError(null);
    setExportingAttachmentId(attachmentId);
    try {
      const result =
        target.kind === "workspace"
          ? await invoke<ExportAttachmentResult>("export_attachment", {
              workspaceId: target.workspaceId,
              messageId: target.messageId,
              attachmentId,
            })
          : await invoke<ExportAttachmentResult>("export_standalone_message_attachment", {
              stableId: target.stableId,
              attachmentId,
            });
      setExportResults((current) => ({ ...current, [attachmentId]: result }));
      if (!result.exported) setError(result.error ?? "Attachment export failed.");
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setExportingAttachmentId(null);
    }
  }

  async function openAttachment(attachmentId: number) {
    setError(null);
    setOpeningAttachmentId(attachmentId);
    try {
      const result =
        target.kind === "workspace"
          ? await invoke<ExportAttachmentResult>("open_attachment", {
              workspaceId: target.workspaceId,
              messageId: target.messageId,
              attachmentId,
            })
          : await invoke<ExportAttachmentResult>("open_standalone_message_attachment", {
              stableId: target.stableId,
              attachmentId,
            });
      setExportResults((current) => ({ ...current, [attachmentId]: result }));
      if (!result.exported) setError(result.error ?? "Attachment open failed.");
    } catch (err) {
      setError(getErrorMessage(err));
    } finally {
      setOpeningAttachmentId(null);
    }
  }

  async function revealExportedFile(outputPath: string) {
    try {
      if (target.kind === "workspace") {
        await invoke("reveal_exported_file_for_workspace", {
          workspaceId: target.workspaceId,
          outputPath,
        });
      } else {
        await invoke("reveal_standalone_exported_file", { outputPath });
      }
      setSourceEmlStatus("Revealed exported file in Finder.");
    } catch (err) {
      setError(getErrorMessage(err));
    }
  }

  async function revealSavedHtml(outputPath: string) {
    try {
      await invoke("reveal_saved_html", { outputPath });
      setPrintStatus("Revealed saved HTML in Finder.");
    } catch (err) {
      setError(getErrorMessage(err));
    }
  }

  async function revealSavedEml(outputPath: string) {
    try {
      await invoke("reveal_saved_eml", { outputPath });
      setSourceEmlStatus(
        `Revealed saved source ${sourceMessageFormatLabel(sourceEmlView)} in Finder.`,
      );
    } catch (err) {
      setError(getErrorMessage(err));
    }
  }

  const sourceEmlBody = sourceEmlView ? formatBodyForDisplay(sourceEmlView.bodyText) : "";
  const sourceEmlBodyStatus = sourceEmlView
    ? sourceBodyStatusMessage(sourceEmlView, sourceEmlBody)
    : null;
  const sourceEmlReconstructionWarnings = sourceEmlView
    ? sourceReconstructionWarnings(sourceEmlView)
    : [];
  const sourceEmlRawTerms = sourceEmlRawSearch.trim() ? [sourceEmlRawSearch.trim()] : [];

  return (
    <main className="preview-window-shell">
      <section className="source-eml-modal source-eml-window" aria-label="Message Preview">
        <header className="source-eml-header">
          <div>
            <h2>{sourceEmlView?.subject || "Message Preview"}</h2>
            {sourceEmlView?.inLocalTrash ? (
              <span className="local-trash-badge">In Local Trash</span>
            ) : null}
            <p title={sourceEmlView?.sourcePath}>
              {sourceEmlView?.sourcePath ?? "Loading message source."}
            </p>
          </div>
        </header>

        <div className="source-eml-toolbar">
          <div className="reader-mode-toggle" role="group" aria-label="Preview view mode">
            <button
              type="button"
              className={sourceEmlMode === "rendered" ? "selected" : ""}
              onClick={() => setSourceEmlMode("rendered")}
            >
              Rendered View
            </button>
            <button
              type="button"
              className={sourceEmlMode === "plain_text" ? "selected" : ""}
              onClick={() => setSourceEmlMode("plain_text")}
            >
              Plain Text
            </button>
            <button
              type="button"
              className={sourceEmlMode === "raw_source" ? "selected" : ""}
              onClick={() => setSourceEmlMode("raw_source")}
            >
              Raw Source
            </button>
          </div>
          <div className="reader-actions">
            <button
              type="button"
              onClick={() => void saveSourcePrintableHtml()}
              disabled={!sourceEmlView || isSavingPrintable}
            >
              {isSavingPrintable ? "Saving" : "Save Printable HTML"}
            </button>
            <button type="button" onClick={() => void saveSourceEmlAs()} disabled={!sourceEmlView}>
              Save Source {sourceEmlView?.sourceFormat === "msg" ? "MSG" : "EML"} As...
            </button>
            {printableSaveResult?.saved && printableSaveResult.outputPath ? (
              <button type="button" onClick={() => void revealSavedHtml(printableSaveResult.outputPath!)}>
                Reveal Saved
              </button>
            ) : sourceEmlSaveResult?.exported && sourceEmlSaveResult.outputPath ? (
              <button type="button" onClick={() => void revealSavedEml(sourceEmlSaveResult.outputPath!)}>
                Reveal Saved
              </button>
            ) : null}
          </div>
        </div>

        {error ? <p className="message-action-status action-error">{error}</p> : null}
        {sourceEmlStatus ? <p className="message-action-status">{sourceEmlStatus}</p> : null}
        {printStatus ? <p className="message-action-status">{printStatus}</p> : null}
        {printableSaveResult?.saved ? (
          <p className="message-action-status action-ok">
            Saved printable HTML to {printableSaveResult.outputPath}
          </p>
        ) : printableSaveResult?.error ? (
          <p className="message-action-status action-error">{printableSaveResult.error}</p>
        ) : null}
        {sourceEmlSaveResult?.exported ? (
          <p className="message-action-status action-ok">
            Saved source message to {sourceEmlSaveResult.outputPath}
          </p>
        ) : sourceEmlSaveResult?.error ? (
          <p className="message-action-status action-error">{sourceEmlSaveResult.error}</p>
        ) : null}

        {sourceEmlMode === "rendered" && sourceEmlReconstructionWarnings.length ? (
          <div className="message-action-status action-warning">
            {sourceEmlReconstructionWarnings.map((warning) => (
              <div key={warning}>{warning}</div>
            ))}
          </div>
        ) : null}

        {sourceEmlView?.parseWarnings.length ? (
          <details className="message-reconstruction-details">
            <summary>Message reconstruction details</summary>
            <ul>
              {sourceEmlView.parseWarnings.map((warning) => (
                <li key={warning}>{warning}</li>
              ))}
            </ul>
          </details>
        ) : null}

        {sourceEmlView ? (
          <>
            <SourceMessageMetadata view={sourceEmlView} />

            {sourceEmlView.attachments.length ? (
              <section className="attachments source-eml-attachments">
                <h4>Attachments ({formatCount(sourceEmlView.attachments.length)})</h4>
                <p className="attachment-safety-note">
                  {target.kind === "standalone"
                    ? "Opening attachments uses your Mac's default app after exporting a safe copy to App Support. The source message is not modified."
                    : "Opening attachments uses your Mac's default app after exporting a safe copy to the workspace. The original PST is not modified."}
                </p>
                <ul>
                  {sourceEmlView.attachments.map((attachment) => {
                    const exportResult = exportResults[attachment.id];
                    return (
                      <li key={attachment.id}>
                        <div className="attachment-main">
                          <span>{attachment.filename || "(Unnamed attachment)"}</span>
                          <small>
                            {attachmentContentLabel(attachment)}
                            {attachment.sizeBytes != null ? ` - ${formatBytes(attachment.sizeBytes)}` : ""}
                          </small>
                        </div>
                        <div className="attachment-actions">
                          <button
                            type="button"
                            onClick={() => void openAttachment(attachment.id)}
                            disabled={
                              openingAttachmentId === attachment.id ||
                              exportingAttachmentId === attachment.id
                            }
                            title="Export a safe workspace copy, then open that copy with the Mac default app."
                          >
                            {openingAttachmentId === attachment.id ? "Opening" : "Open"}
                          </button>
                          <button
                            type="button"
                            onClick={() => void exportAttachment(attachment.id)}
                            disabled={
                              exportingAttachmentId === attachment.id ||
                              openingAttachmentId === attachment.id
                            }
                          >
                            {exportingAttachmentId === attachment.id ? "Exporting" : "Export"}
                          </button>
                          {exportResult?.exported && exportResult.outputPath ? (
                            <button
                              type="button"
                              onClick={() => void revealExportedFile(exportResult.outputPath!)}
                            >
                              Reveal Exported File in Finder
                            </button>
                          ) : null}
                        </div>
                        {exportResult ? (
                          <small
                            className={
                              exportResult.exported ? "attachment-export-ok" : "attachment-export-error"
                            }
                          >
                            {exportResult.exported
                              ? `Exported to ${exportResult.outputPath}`
                              : exportResult.error}
                          </small>
                        ) : null}
                      </li>
                    );
                  })}
                </ul>
              </section>
            ) : null}

            {sourceEmlView.inlineResources.length ? (
              <details className="attachments inline-resources source-eml-attachments">
                <summary>
                  Inline resources ({formatCount(sourceEmlView.inlineResources.length)})
                </summary>
                <p className="attachment-safety-note">
                  These images were matched to exact Content-ID references in the message body.
                </p>
                <ul>
                  {sourceEmlView.inlineResources.map((attachment) => {
                    const exportResult = exportResults[attachment.id];
                    return (
                      <li key={attachment.id}>
                        <div className="attachment-main">
                          <span>{attachment.filename || "(Unnamed inline resource)"}</span>
                          <small>
                            {attachment.contentType || "unknown type"}
                            {attachment.sizeBytes != null
                              ? ` - ${formatBytes(attachment.sizeBytes)}`
                              : ""}
                          </small>
                        </div>
                        <div className="attachment-actions">
                          <button
                            type="button"
                            onClick={() => void openAttachment(attachment.id)}
                            disabled={
                              openingAttachmentId === attachment.id ||
                              exportingAttachmentId === attachment.id
                            }
                            title="Export a safe copy, then open that copy with the Mac default app."
                          >
                            {openingAttachmentId === attachment.id ? "Opening" : "Open"}
                          </button>
                          <button
                            type="button"
                            onClick={() => void exportAttachment(attachment.id)}
                            disabled={
                              exportingAttachmentId === attachment.id ||
                              openingAttachmentId === attachment.id
                            }
                          >
                            {exportingAttachmentId === attachment.id ? "Exporting" : "Export"}
                          </button>
                          {exportResult?.exported && exportResult.outputPath ? (
                            <button
                              type="button"
                              onClick={() => void revealExportedFile(exportResult.outputPath!)}
                            >
                              Reveal Exported File in Finder
                            </button>
                          ) : null}
                        </div>
                      </li>
                    );
                  })}
                </ul>
              </details>
            ) : null}

            <section className="source-eml-body">
              {sourceEmlMode === "rendered" ? (
                <>
                  {sourceEmlView.remoteImagesBlocked ? (
                    <div className="remote-image-notice">
                      <span>{remoteImagesBlockedMessage}</span>
                    </div>
                  ) : null}
                  {sourceEmlView.sanitizedHtml.trim() ? (
                    <div
                      className="html-preview source-eml-html"
                      onClick={(event) => {
                        if ((event.target as HTMLElement).closest("a")) event.preventDefault();
                      }}
                      onAuxClick={(event) => {
                        if ((event.target as HTMLElement).closest("a")) event.preventDefault();
                      }}
                      dangerouslySetInnerHTML={{ __html: sourceEmlView.sanitizedHtml }}
                      data-support-untrusted="true"
                    />
                  ) : (
                    <>
                      {sourceEmlBodyStatus ? <p className="body-status">{sourceEmlBodyStatus}</p> : null}
                      <pre className="body-preview">{sourceEmlBody || "No message body found."}</pre>
                    </>
                  )}
                </>
              ) : sourceEmlMode === "plain_text" ? (
                <>
                  {sourceEmlBodyStatus ? <p className="body-status">{sourceEmlBodyStatus}</p> : null}
                  <pre className="body-preview">{sourceEmlBody || "No message body found."}</pre>
                </>
              ) : (
                <div className="raw-source-panel">
                  <p className="body-status">
                    {sourceEmlView.sourceFormat === "msg"
                      ? "Raw Source shows structured MSG properties and diagnostics. MSG files are binary and are not displayed as raw bytes."
                      : sourceEmlView.sourceKind === "standalone"
                        ? "Raw Source shows the original standalone .eml contents."
                        : "Raw Source shows the original extracted .eml contents."}
                  </p>
                  <input
                    type="search"
                    value={sourceEmlRawSearch}
                    onChange={(event) => setSourceEmlRawSearch(event.target.value)}
                    placeholder={rawSourceSearchLabel(sourceEmlView)}
                    aria-label={rawSourceSearchLabel(sourceEmlView)}
                  />
                  <pre className="raw-source-text">
                    <HighlightedText text={sourceEmlView.rawSource} terms={sourceEmlRawTerms} />
                  </pre>
                </div>
              )}
            </section>
          </>
        ) : null}
      </section>
    </main>
  );
}

function App() {
  if (isMessagePreviewRoute()) {
    return <MessagePreviewBootstrap />;
  }

  const [readpstStatus, setReadpstStatus] = useState<ReadpstStatus | null>(null);
  const [workspace, setWorkspace] = useState<WorkspaceSummary | null>(null);
  const [openPstSessions, setOpenPstSessions] = useState<OpenPstSession[]>([]);
  const [workspaceSize, setWorkspaceSize] = useState<WorkspaceSize | null>(null);
  const [folders, setFolders] = useState<Folder[]>([]);
  const [selectedFolderId, setSelectedFolderId] = useState<number | null>(null);
  const [messageVisibility, setMessageVisibility] = useState<MessageVisibility>("active");
  const [allOpenFolderSelection, setAllOpenFolderSelection] = useState<AllOpenFolderSelection>({
    workspaceId: null,
    folderId: null,
  });
  const [localTrashAllOpenWorkspaceId, setLocalTrashAllOpenWorkspaceId] = useState<
    string | null
  >(null);
  const [includeSubfolders, setIncludeSubfolders] = useState(true);
  const [foldersHidden, setFoldersHidden] = useState(() =>
    initialBooleanPreference(folderHiddenStorageKey, false),
  );
  const [folderHierarchyMode, setFolderHierarchyMode] = useState<FolderHierarchyMode>(() =>
    storedFolderHierarchyMode(typeof window === "undefined" ? null : window.localStorage),
  );
  const [collapsedFolderIdentitiesByMode, setCollapsedFolderIdentitiesByMode] = useState(
    initialCollapsedFolderIdentities,
  );
  const collapsedFolderPaths = collapsedFolderIdentitiesByMode[folderHierarchyMode];
  function setCollapsedFolderPaths(
    update: Set<string> | ((current: Set<string>) => Set<string>),
  ) {
    setCollapsedFolderIdentitiesByMode((current) => {
      const currentModeState = current[folderHierarchyMode];
      const nextModeState =
        typeof update === "function" ? update(currentModeState) : update;
      return { ...current, [folderHierarchyMode]: nextModeState };
    });
  }
  const [messages, setMessages] = useState<MessageListItem[]>([]);
  const [messageTotalCount, setMessageTotalCount] = useState(0);
  const [messageHasMore, setMessageHasMore] = useState(false);
  const [messageCountState, setMessageCountState] = useState<SearchExactCountState>(() =>
    createSearchExactCountState(0),
  );
  const [listMode, setListMode] = useState<ListMode>(initialListMode);
  const previousActiveListModeRef = useRef<ListMode>(initialListMode());
  const [localTrashUndo, setLocalTrashUndo] = useState<LocalTrashUndoState | null>(
    initialLocalTrashUndoState,
  );
  const [localTrashStatus, setLocalTrashStatus] = useState<string | null>(null);
  const [localTrashError, setLocalTrashError] = useState<string | null>(null);
  const [localTrashMutationPending, setLocalTrashMutationPending] = useState(false);
  const [localTrashResetConfirmOpen, setLocalTrashResetConfirmOpen] = useState(false);
  const [resultNavigation, setResultNavigation] = useState(createResultNavigationState);
  const setMessageNavigationKey = useCallback((key: string | null) => {
    setResultNavigation((current) => setResultNavigationActiveKey(current, "messages", key));
  }, []);
  const setConversationNavigationKey = useCallback((key: string | null) => {
    setResultNavigation((current) =>
      setResultNavigationActiveKey(current, "conversations", key),
    );
  }, []);
  const [activeSessionGeneration, setActiveSessionGeneration] = useState(0);
  const [initializingWorkspaceId, setInitializingWorkspaceId] = useState<string | null>(null);
  const [conversationSort, setConversationSort] = useState<ConversationSort>("newest");
  const [conversations, setConversations] = useState<ConversationSummary[]>([]);
  const [conversationTotalCount, setConversationTotalCount] = useState(0);
  const [conversationMatchingMessageCount, setConversationMatchingMessageCount] = useState(0);
  const [conversationHasMore, setConversationHasMore] = useState(false);
  const [conversationCountState, setConversationCountState] = useState<SearchExactCountState>(() =>
    createSearchExactCountState(0),
  );
  const [conversationIndexedWorkspaceCount, setConversationIndexedWorkspaceCount] = useState(0);
  const [conversationWorkspaceIssues, setConversationWorkspaceIssues] = useState<
    ConversationWorkspaceIssue[]
  >([]);
  const [expandedConversations, setExpandedConversations] = useState<
    Record<string, ExpandedConversationState>
  >({});
  const [selectedMessage, setSelectedMessage] = useState<MessageDetail | null>(null);
  const [search, setSearch] = useState("");
  const [openPopoverId, setOpenPopoverId] = useState<CustomPopoverId | null>(null);
  const [searchFilters, setSearchFilters] = useState<AdvancedSearchFilters>(
    defaultAdvancedSearchFilters,
  );
  const [sortOrder, setSortOrder] = useState<SortOrder>("newest");
  const [searchScope, setSearchScope] = useState<SearchScope>(() =>
    storedSearchScope(typeof window === "undefined" ? null : window.localStorage),
  );
  const [appliedSearchVersion, setAppliedSearchVersion] = useState<AppliedSearchVersion>(() => ({
    snapshot: createAppliedSearchSnapshot(
      { query: "", ...defaultAdvancedSearchFilters },
      {
        scope: searchScope,
        activeWorkspaceId: null,
        workspaceIds: [],
        selectedWorkspaceId: null,
        useMultiWorkspace: false,
        singleWorkspaceId: null,
        folderId: null,
        includeSubfolders: true,
        conversationScopes: [],
        listMode,
        messageSort: sortOrder,
        conversationSort,
        sessionGeneration: activeSessionGeneration,
      },
    ),
    generation: 0,
  }));
  const [searchLoading, setSearchLoading] = useState<SearchLoadingState | null>(null);
  const [messageLoadMoreRequest, setMessageLoadMoreRequest] =
    useState<PageRequestIdentity | null>(null);
  const [conversationLoadMoreRequest, setConversationLoadMoreRequest] =
    useState<PageRequestIdentity | null>(null);
  const [searchError, setSearchError] = useState<{ generation: number; message: string } | null>(
    null,
  );
  const [workspaceSearchCounts, setWorkspaceSearchCounts] = useState<WorkspaceSearchCount[]>([]);
  const [workspaceOperationStates, setWorkspaceOperationStates] = useState<
    Record<string, WorkspaceOperationState>
  >({});
  const [progressClockTick, setProgressClockTick] = useState(0);
  const [isBusy, setIsBusy] = useState(false);
  const [isImporting, setIsImporting] = useState(false);
  const [appearance, setAppearance] = useState<AppearanceMode>(() =>
    storedAppearance(typeof window === "undefined" ? null : window.localStorage),
  );
  const [appDiagnostics, setAppDiagnostics] = useState<AppDiagnostics | null>(null);
  const [isLoadingAppDiagnostics, setIsLoadingAppDiagnostics] = useState(false);
  const [aboutStatus, setAboutStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [setupCommand, setSetupCommand] = useState<string | null>(null);
  const [workspaceDeleteToast, setWorkspaceDeleteToast] =
    useState<WorkspaceDeleteToast | null>(null);
  const [foregroundOperationWorkspaceId, setForegroundOperationWorkspaceId] = useState<
    string | null
  >(null);
  const [foregroundImportAction, setForegroundImportAction] =
    useState<ImportAction | null>(null);
  const [restoringPlannedWorkspace, setRestoringPlannedWorkspace] = useState(false);
  const [exportResults, setExportResults] = useState<Record<number, ExportAttachmentResult>>({});
  const [exportingAttachmentId, setExportingAttachmentId] = useState<number | null>(null);
  const [openingAttachmentId, setOpeningAttachmentId] = useState<number | null>(null);
  const [printStatus, setPrintStatus] = useState<string | null>(null);
  const [printableSaveResult, setPrintableSaveResult] = useState<SavePrintableHtmlResult | null>(
    null,
  );
  const [isSavingPrintable, setIsSavingPrintable] = useState(false);
  const [sourceEmlOpen, setSourceEmlOpen] = useState(false);
  const [sourceEmlView, setSourceEmlView] = useState<SourceEmlView | null>(null);
  const [sourceEmlContext, setSourceEmlContext] = useState<SourceEmlContext | null>(null);
  const [sourceEmlMode, setSourceEmlMode] = useState<SourceEmlMode>("rendered");
  const [isLoadingSourceEml, setIsLoadingSourceEml] = useState(false);
  const [sourceEmlStatus, setSourceEmlStatus] = useState<string | null>(null);
  const [sourceEmlSaveResult, setSourceEmlSaveResult] = useState<ExportOriginalEmlResult | null>(
    null,
  );
  const [sourceEmlRawSearch, setSourceEmlRawSearch] = useState("");
  const [readerMode, setReaderMode] = useState<ReaderMode>("plain_text");
  const [htmlRender, setHtmlRender] = useState<HtmlRenderResult | null>(null);
  const [isRenderingHtml, setIsRenderingHtml] = useState(false);
  const [messageDiagnostics, setMessageDiagnostics] = useState<MessageDiagnostics | null>(null);
  const [messageDiagnosticsError, setMessageDiagnosticsError] = useState<string | null>(null);
  const [isLoadingMessageDiagnostics, setIsLoadingMessageDiagnostics] = useState(false);
  const [workspaceLocationMode, setWorkspaceLocationMode] = useState<WorkspaceLocationMode>(
    initialWorkspaceLocationMode,
  );
  const [layoutMode, setLayoutMode] = useState<LayoutMode>(initialLayoutMode);
  const [messageListDisplayMode, setMessageListDisplayMode] =
    useState<MessageListDisplayMode>(initialMessageListDisplayMode);
  const [paneWidths, setPaneWidths] = useState<PaneWidths>(initialPaneWidths);
  const [pendingOpenPlan, setPendingOpenPlan] = useState<PstOpenPlan | null>(null);
  const [pendingPreflightOpen, setPendingPreflightOpen] =
    useState<PendingPreflightOpen | null>(null);
  const [pendingPlannedRepair, setPendingPlannedRepair] =
    useState<PendingPlannedRepair | null>(null);
  const [pendingPlannedDelete, setPendingPlannedDelete] =
    useState<PendingPlannedRepair | null>(null);
  const [pendingWorkspaceFlowOwnerId, setPendingWorkspaceFlowOwnerId] = useState<string | null>(
    null,
  );
  const [recentPsts, setRecentPsts] = useState<RecentPst[]>(initialRecentPsts);
  const [savedSession, setSavedSession] = useState<SavedSession | null>(initialSavedSession);
  const [restorePromptVisible, setRestorePromptVisible] = useState(false);
  const [dropOverlayVisible, setDropOverlayVisible] = useState(false);
  const [sessionPersistenceReady, setSessionPersistenceReady] = useState(() =>
    !initialSavedSession()?.entries.length,
  );
  const [restoreStatus, setRestoreStatus] = useState<string | null>(null);
  const operationWorkspaceIdRef = useRef<string | null>(null);
  const activeWorkspaceIdRef = useRef<string | null>(null);
  const activeWorkspaceChangeSequenceRef = useRef(0);
  const restoringPlannedWorkspaceRef = useRef(false);
  const integrityOperationSequenceRef = useRef(0);
  const progressOperationSequenceRef = useRef(0);
  const integrityOperationTokensRef = useRef<
    Map<string, WorkspaceIntegrityOperationToken>
  >(new Map());
  const workspaceOperationDismissTimersRef = useRef<Map<string, number>>(new Map());
  const workspaceOperationDismissGenerationsRef = useRef<Map<string, number>>(new Map());
  const repairConfirmDialogRef = useRef<HTMLElement | null>(null);
  const repairConfirmCancelRef = useRef<HTMLButtonElement | null>(null);
  const repairConfirmReturnFocusRef = useRef<HTMLElement | null>(null);
  const deleteConfirmDialogRef = useRef<HTMLElement | null>(null);
  const deleteConfirmCancelRef = useRef<HTMLButtonElement | null>(null);
  const deleteConfirmReturnFocusRef = useRef<HTMLElement | null>(null);
  const localTrashResetDialogRef = useRef<HTMLElement | null>(null);
  const localTrashResetCancelRef = useRef<HTMLButtonElement | null>(null);
  const localTrashResetReturnFocusRef = useRef<HTMLElement | null>(null);
  const localTrashResetWorkspaceIdRef = useRef<string | null>(null);
  const verificationReportHeadingRef = useRef<HTMLHeadingElement | null>(null);
  const verificationReportReturnFocusRef = useRef<HTMLElement | null>(null);
  const paneLayoutRef = useRef<HTMLElement | null>(null);
  const messageListRef = useRef<HTMLDivElement | null>(null);
  const previewScrollRef = useRef<HTMLDivElement | null>(null);
  const activePopoverTriggerRef = useRef<HTMLElement | null>(null);
  const advancedSearchPanelRef = useRef<HTMLDivElement | null>(null);
  const recentPopoverPanelRef = useRef<HTMLDivElement | null>(null);
  const workspacePopoverPanelRef = useRef<HTMLDivElement | null>(null);
  const layoutPopoverPanelRef = useRef<HTMLDivElement | null>(null);
  const workspaceDetailsPanelRef = useRef<HTMLDivElement | null>(null);
  const importProgressDetailsPanelRef = useRef<HTMLDivElement | null>(null);
  const helpDialogRef = useRef<HTMLElement | null>(null);
  const searchApplicationRef = useRef<SearchApplicationState | null>(null);
  if (searchApplicationRef.current == null) {
    searchApplicationRef.current = createSearchApplicationState(appliedSearchVersion.snapshot);
  }
  const searchGenerationRef = useRef(appliedSearchVersion.generation);
  const messagePaginationRef = useRef<MessagePaginationState>(
    createMessagePaginationState(
      appliedSearchVersion.snapshot,
      appliedSearchVersion.generation,
    ),
  );
  const searchDebounceTimerRef = useRef<number | null>(null);
  const forceImmediateSearchApplyRef = useRef(false);
  const topLevelSearchRequestSequenceRef = useRef(0);
  const topLevelSearchRequestRef = useRef<SearchLoadingState | null>(null);
  const messageCountRequestSequenceRef = useRef(0);
  const messageCountRequestRef = useRef<CountRequestIdentity | null>(null);
  const conversationCountRequestSequenceRef = useRef(0);
  const conversationCountRequestRef = useRef<CountRequestIdentity | null>(null);
  const messageLoadMoreRequestSequenceRef = useRef(0);
  const conversationLoadMoreRequestSequenceRef = useRef(0);
  const messageLoadMoreRequestRef = useRef<PageRequestIdentity | null>(null);
  const conversationLoadMoreRequestRef = useRef<PageRequestIdentity | null>(null);
  const expandedConversationRequestSequenceRef = useRef(0);
  const expandedConversationRequestsRef = useRef<
    Map<string, ExpandedConversationRequestIdentity>
  >(new Map());
  const localTrashOperationSequenceRef = useRef(0);
  const activeLocalTrashOperationRef = useRef<LocalTrashOperationToken | null>(null);
  const activeLocalTrashResetRef = useRef<LocalTrashResetToken | null>(null);
  const localTrashUndoMessageRef = useRef<{ workspaceId: string; messageId: number } | null>(null);
  const localTrashSummaryRequestSequenceRef = useRef<Map<string, number>>(new Map());
  const openPstSessionsRef = useRef<OpenPstSession[]>([]);
  const preserveSelectedMessageRef = useRef<string | null>(null);
  const externalFileOpenChainRef = useRef<Promise<void>>(Promise.resolve());
  const externalPstQueueRef = useRef<string[]>([]);
  const externalPstProcessingRef = useRef(false);
  const activeExternalPstPathRef = useRef<string | null>(null);
  const externalBatchEnqueueRef = useRef<(batch: ExternalFileOpenBatch) => void>(() => undefined);
  const restorePromptTimerRef = useRef<number | null>(null);
  const advancedSearchOpen = openPopoverId === "advanced_search";
  const aboutOpen = openPopoverId === "help";

  function popoverPanelFor(id: CustomPopoverId): HTMLElement | null {
    switch (id) {
      case "advanced_search":
        return advancedSearchPanelRef.current;
      case "recent":
        return recentPopoverPanelRef.current;
      case "workspace":
        return workspacePopoverPanelRef.current;
      case "layout":
        return layoutPopoverPanelRef.current;
      case "workspace_details":
        return workspaceDetailsPanelRef.current;
      case "import_progress":
        return importProgressDetailsPanelRef.current;
      case "help":
        return helpDialogRef.current;
    }
  }

  function clearHelpState() {
    setAppDiagnostics(null);
    setAboutStatus(null);
  }

  function closeActivePopover() {
    if (openPopoverId === "help") clearHelpState();
    activePopoverTriggerRef.current = null;
    setOpenPopoverId(null);
  }

  function togglePopover(id: CustomPopoverId, trigger: HTMLElement) {
    const next = toggleOpenPopover(openPopoverId, id) as CustomPopoverId | null;
    if (openPopoverId === "help" && next !== "help") clearHelpState();
    if (next == null) {
      activePopoverTriggerRef.current = null;
    } else {
      activePopoverTriggerRef.current = trigger;
    }
    setOpenPopoverId(next);
  }

  useEffect(() => {
    const attachedPopoverId = openPopoverId;
    if (!attachedPopoverId) return;
    const controller = createDismissiblePopoverController({
      documentTarget: document,
      windowTarget: window,
      trigger: activePopoverTriggerRef.current,
      panel: popoverPanelFor(attachedPopoverId),
      onDismiss: () => {
        if (attachedPopoverId === "help") clearHelpState();
        activePopoverTriggerRef.current = null;
        setOpenPopoverId((current) =>
          closeOpenPopover(current, attachedPopoverId) as CustomPopoverId | null,
        );
      },
    });
    return controller.attach();
  }, [openPopoverId]);

  useEffect(() => {
    if (openPopoverId === "help") clearHelpState();
    activePopoverTriggerRef.current = null;
    setOpenPopoverId(null);
  }, [workspace?.id]);

  function clearSearchDebounceTimer() {
    if (searchDebounceTimerRef.current != null) {
      window.clearTimeout(searchDebounceTimerRef.current);
      searchDebounceTimerRef.current = null;
    }
  }

  function forceNextSearchApplication() {
    clearSearchDebounceTimer();
    forceImmediateSearchApplyRef.current = true;
  }

  function cancelBackendSearch(request: SearchCancellationRequest) {
    // Cancellation is advisory; the newer generation starts without waiting for this invoke.
    void invoke("cancel_search_operation", {
      searchGeneration: request.generation,
      searchOperationId: request.operationId,
    }).catch(() => undefined);
  }

  function publishSearchApplicationState(next: SearchApplicationState) {
    const previous = searchApplicationRef.current;
    searchApplicationRef.current = next;
    if (
      previous &&
      previous.applied.generation === next.applied.generation &&
      previous.applied.snapshot === next.applied.snapshot
    ) {
      return;
    }

    if (previous && previous.applied.generation !== next.applied.generation) {
      cancelBackendSearch(cancellationForGeneration(previous.applied.generation));
      if (
        didResultNavigationContextChange(
          previous.applied.snapshot,
          next.applied.snapshot,
          "messages",
        )
      ) {
        setResultNavigation((current) => resetResultNavigationMode(current, "messages"));
      }
      if (
        didResultNavigationContextChange(
          previous.applied.snapshot,
          next.applied.snapshot,
          "conversations",
        )
      ) {
        setResultNavigation((current) => resetResultNavigationMode(current, "conversations"));
      }
      const resultsContainer = messageListRef.current;
      if (resultsContainer?.contains(document.activeElement)) {
        try {
          resultsContainer.focus({ preventScroll: true });
        } catch {
          resultsContainer.focus();
        }
      }
    }

    searchGenerationRef.current = next.applied.generation;
    messagePaginationRef.current = createMessagePaginationState(
      next.applied.snapshot,
      next.applied.generation,
    );
    topLevelSearchRequestRef.current = null;
    messageCountRequestRef.current = null;
    conversationCountRequestRef.current = null;
    messageLoadMoreRequestRef.current = null;
    conversationLoadMoreRequestRef.current = null;
    expandedConversationRequestsRef.current.clear();
    setSearchLoading(null);
    setMessageLoadMoreRequest(null);
    setConversationLoadMoreRequest(null);
    setSearchError(null);
    setMessages([]);
    setMessageTotalCount(0);
    setMessageHasMore(false);
    setMessageCountState(createSearchExactCountState(next.applied.generation));
    setWorkspaceSearchCounts([]);
    setConversations([]);
    setConversationTotalCount(0);
    setConversationMatchingMessageCount(0);
    setConversationHasMore(false);
    setConversationCountState(createSearchExactCountState(next.applied.generation));
    setConversationIndexedWorkspaceCount(0);
    setConversationWorkspaceIssues([]);
    setExpandedConversations({});
    setAppliedSearchVersion(next.applied);
  }

  function invalidateSearchLifecycle(snapshot?: AppliedSearchSnapshot) {
    clearSearchDebounceTimer();
    const current = searchApplicationRef.current;
    if (!current) return;
    publishSearchApplicationState(invalidateSearchApplication(current, snapshot));
  }

  useEffect(() => {
    applyAppearance(appearance, document.documentElement);
    try {
      window.localStorage.setItem(appearanceStorageKey, appearance);
    } catch {
      // Appearance remains active for this session when storage is unavailable.
    }
  }, [appearance]);

  const hasLiveImportProgress = Object.values(workspaceOperationStates).some(
    (operation) => operation.progress?.status === "active",
  );

  useEffect(() => {
    if (!hasLiveImportProgress) return;
    const interval = window.setInterval(() => {
      setProgressClockTick((current) => current + 1);
    }, 1_000);
    return () => window.clearInterval(interval);
  }, [hasLiveImportProgress]);

  function updateWorkspaceOperationState(
    workspaceId: string,
    update: Partial<WorkspaceOperationState>,
  ) {
    setWorkspaceOperationStates((current) => ({
      ...current,
      [workspaceId]: {
        ...(current[workspaceId] ?? emptyWorkspaceOperationState()),
        ...update,
      },
    }));
  }

  function createProgressOperationId(prefix: "import" | "reindex"): string {
    progressOperationSequenceRef.current += 1;
    return `${prefix}-${progressOperationSequenceRef.current}`;
  }

  function clearWorkspaceOperationDismissTimer(workspaceId: string) {
    workspaceOperationDismissGenerationsRef.current.set(
      workspaceId,
      (workspaceOperationDismissGenerationsRef.current.get(workspaceId) ?? 0) + 1,
    );
    const timeout = workspaceOperationDismissTimersRef.current.get(workspaceId);
    if (timeout != null) {
      window.clearTimeout(timeout);
      workspaceOperationDismissTimersRef.current.delete(workspaceId);
    }
  }

  function clearWorkspaceOperationState(workspaceId: string) {
    clearWorkspaceOperationDismissTimer(workspaceId);
    integrityOperationTokensRef.current.delete(workspaceId);
    setWorkspaceOperationStates((current) => {
      if (!current[workspaceId]) return current;
      const next = { ...current };
      delete next[workspaceId];
      return next;
    });
  }

  function scheduleWorkspaceOperationDismiss(workspaceId: string) {
    clearWorkspaceOperationDismissTimer(workspaceId);
    const dismissGeneration = workspaceOperationDismissGenerationsRef.current.get(workspaceId);
    const timeout = window.setTimeout(() => {
      if (
        workspaceOperationDismissGenerationsRef.current.get(workspaceId) !== dismissGeneration
      ) {
        return;
      }
      setWorkspaceOperationStates((current) => {
        const state = current[workspaceId];
        if (state?.running === true || state?.progress?.status === "active") return current;
        return {
          ...current,
          [workspaceId]: {
            ...(state ?? emptyWorkspaceOperationState()),
            progress: null,
            progressOperationId: null,
            progressReceivedAtMonotonicMs: null,
            notice: null,
          },
        };
      });
      setForegroundOperationWorkspaceId((current) =>
        current === workspaceId ? null : current,
      );
      workspaceOperationDismissTimersRef.current.delete(workspaceId);
    }, 12_000);
    workspaceOperationDismissTimersRef.current.set(workspaceId, timeout);
  }

  function completeWorkspaceOperation(workspaceId: string, message: string) {
    setWorkspaceOperationStates((current) => {
      const operation = current[workspaceId] ?? emptyWorkspaceOperationState();
      const preserveProgressSlot =
        operation.progressOperationId !== null &&
        (operation.progress === null || operation.progress.status === "active");
      return {
        ...current,
        [workspaceId]: {
          ...operation,
          progress: operation.progress,
          progressOperationId: preserveProgressSlot ? operation.progressOperationId : null,
          progressReceivedAtMonotonicMs: operation.progressReceivedAtMonotonicMs,
          running: false,
          notice: message,
          error: null,
          setupCommand: null,
        },
      };
    });
    scheduleWorkspaceOperationDismiss(workspaceId);
  }

  function showWorkspaceNotice(workspaceId: string, message: string) {
    updateWorkspaceOperationState(workspaceId, {
      notice: message,
      error: null,
      setupCommand: null,
    });
    scheduleWorkspaceOperationDismiss(workspaceId);
  }

  function showWorkspaceError(workspaceId: string, message: string) {
    clearWorkspaceOperationDismissTimer(workspaceId);
    updateWorkspaceOperationState(workspaceId, {
      error: message,
      notice: null,
      setupCommand: null,
    });
  }

  function clearWorkspaceOperationMessages(workspaceId: string) {
    clearWorkspaceOperationDismissTimer(workspaceId);
    updateWorkspaceOperationState(workspaceId, {
      notice: null,
      error: null,
      setupCommand: null,
    });
  }

  function beginWorkspaceIntegrityOperation(
    workspaceId: string,
    kind: WorkspaceIntegrityOperationKind,
  ): WorkspaceIntegrityOperationToken | null {
    if (integrityOperationTokensRef.current.size > 0) return null;
    clearWorkspaceOperationDismissTimer(workspaceId);
    integrityOperationSequenceRef.current += 1;
    const operation = createWorkspaceIntegrityOperationToken(
      workspaceId,
      kind,
      integrityOperationSequenceRef.current,
    );
    integrityOperationTokensRef.current.set(workspaceId, operation);
    updateWorkspaceOperationState(workspaceId, {
      progress: null,
      progressOperationId:
        kind === "load_last_verification" ? null : operation.operationId,
      progressReceivedAtMonotonicMs: null,
      running: true,
      notice: null,
      error: null,
      setupCommand: null,
      integrityOperationKind: kind,
      integrityOperationId: operation.operationId,
      verificationReportOpen: false,
      repairConfirmOpen: false,
      ...(kind === "load_last_verification" ? {} : { verificationReport: null }),
    });
    return operation;
  }

  function workspaceIntegrityOperationIsCurrent(
    operation: WorkspaceIntegrityOperationToken,
  ): boolean {
    return isWorkspaceIntegrityOperationCurrent(
      integrityOperationTokensRef.current.get(operation.workspaceId),
      operation,
    );
  }

  function finishWorkspaceIntegrityOperation(
    operation: WorkspaceIntegrityOperationToken,
    noticeMessage: string,
    report?: VerificationReport | null,
  ) {
    if (!workspaceIntegrityOperationIsCurrent(operation)) return;
    integrityOperationTokensRef.current.delete(operation.workspaceId);
    setWorkspaceOperationStates((current) => {
      const state = current[operation.workspaceId] ?? emptyWorkspaceOperationState();
      const preserveProgressSlot =
        state.progressOperationId === operation.operationId &&
        (state.progress === null || state.progress.status === "active");
      return {
        ...current,
        [operation.workspaceId]: {
          ...state,
          progress: state.progress,
          progressOperationId: preserveProgressSlot ? state.progressOperationId : null,
          progressReceivedAtMonotonicMs: state.progressReceivedAtMonotonicMs,
          running: false,
          notice: noticeMessage,
          error: null,
          setupCommand: null,
          integrityOperationKind: null,
          integrityOperationId: null,
          ...(report === undefined ? {} : { verificationReport: report }),
        },
      };
    });
    scheduleWorkspaceOperationDismiss(operation.workspaceId);
  }

  function failWorkspaceIntegrityOperation(
    operation: WorkspaceIntegrityOperationToken,
    message: string,
  ) {
    if (!workspaceIntegrityOperationIsCurrent(operation)) return;
    integrityOperationTokensRef.current.delete(operation.workspaceId);
    setWorkspaceOperationStates((current) => {
      const state = current[operation.workspaceId] ?? emptyWorkspaceOperationState();
      const preserveProgressSlot =
        state.progressOperationId === operation.operationId &&
        (state.progress === null || state.progress.status === "active");
      return {
        ...current,
        [operation.workspaceId]: {
          ...state,
          progress: state.progress,
          progressOperationId: preserveProgressSlot ? state.progressOperationId : null,
          progressReceivedAtMonotonicMs: state.progressReceivedAtMonotonicMs,
          running: false,
          notice: null,
          error: message,
          setupCommand: null,
          integrityOperationKind: null,
          integrityOperationId: null,
        },
      };
    });
  }

  function updateWorkspaceSummaryFromVerification(
    workspaceId: string,
    report: VerificationReport,
  ) {
    setOpenPstSessions((current) =>
      current.map((session) => {
        if (session.workspace.id !== workspaceId) return session;
        const nextWorkspace = workspaceSummaryWithVerification(session.workspace, report);
        return {
          ...session,
          workspace: nextWorkspace,
          status: nextWorkspace.integrityClassification,
        };
      }),
    );
    if (activeWorkspaceIdRef.current === workspaceId) {
      setWorkspace((current) =>
        current?.id === workspaceId ? workspaceSummaryWithVerification(current, report) : current,
      );
    }
  }

  function showVerificationReport(
    workspaceId: string,
    report: VerificationReport,
    returnFocus?: HTMLElement | null,
  ) {
    if (returnFocus) verificationReportReturnFocusRef.current = returnFocus;
    updateWorkspaceOperationState(workspaceId, {
      verificationReport: report,
      verificationReportOpen: true,
    });
    if (activeWorkspaceIdRef.current === workspaceId) {
      window.requestAnimationFrame(() => verificationReportHeadingRef.current?.focus());
    }
  }

  function closeVerificationReport(workspaceId: string) {
    updateWorkspaceOperationState(workspaceId, { verificationReportOpen: false });
    const returnFocus = verificationReportReturnFocusRef.current;
    verificationReportReturnFocusRef.current = null;
    window.requestAnimationFrame(() => {
      if (returnFocus?.isConnected) returnFocus.focus();
    });
  }

  function failWorkspaceOperation(
    workspaceId: string,
    message: string,
    operationSetupCommand: string | null = null,
  ) {
    clearWorkspaceOperationDismissTimer(workspaceId);
    setWorkspaceOperationStates((current) => {
      const operation = current[workspaceId] ?? emptyWorkspaceOperationState();
      const preserveProgressSlot =
        operation.progressOperationId !== null &&
        (operation.progress === null || operation.progress.status === "active");
      return {
        ...current,
        [workspaceId]: {
          ...operation,
          progress: operation.progress,
          progressOperationId: preserveProgressSlot ? operation.progressOperationId : null,
          progressReceivedAtMonotonicMs: operation.progressReceivedAtMonotonicMs,
          running: false,
          error: message,
          setupCommand: operationSetupCommand,
          notice: null,
        },
      };
    });
  }

  function dismissWorkspaceOperation(workspaceId: string) {
    clearWorkspaceOperationDismissTimer(workspaceId);
    updateWorkspaceOperationState(workspaceId, {
      progress: null,
      progressOperationId: null,
      progressReceivedAtMonotonicMs: null,
      notice: null,
    });
  }

  const folderHierarchy = useMemo(
    () => buildFolderHierarchy(folders, workspace?.id ?? "inactive", folderHierarchyMode),
    [folderHierarchyMode, folders, workspace?.id],
  );
  const folderTree = folderHierarchy.roots;
  const openSessionFolderTrees = useMemo(
    () =>
      openPstSessions.map((session) => {
        const hierarchy = buildFolderHierarchy(
          session.folders,
          session.workspace.id,
          folderHierarchyMode,
        );
        return { session, hierarchy, tree: hierarchy.roots };
      }),
    [folderHierarchyMode, openPstSessions],
  );
  const retainedTechnicalWrapperCount = folderHierarchy.diagnostics.filter(
    (diagnostic) => diagnostic.code === "technical_wrapper_has_direct_messages",
  ).length;
  useEffect(() => {
    if (!import.meta.env.DEV || retainedTechnicalWrapperCount === 0) return;
    console.warn(
      `Folder presentation retained ${retainedTechnicalWrapperCount} proven technical wrapper${
        retainedTechnicalWrapperCount === 1 ? "" : "s"
      } because direct messages were present. No folder names or paths were logged.`,
    );
  }, [retainedTechnicalWrapperCount, workspace?.id]);
  const openWorkspaceIds = useMemo(
    () => openPstSessions.map((session) => session.workspace.id),
    [openPstSessions],
  );
  const activeOpenPstSession = workspace
    ? openPstSessions.find((session) => session.workspace.id === workspace.id) ?? null
    : null;
  const activeLocalTrashSummary = workspace
    ? activeOpenPstSession?.localTrashSummary ?? unavailableLocalTrashSummary(workspace.id)
    : null;
  const allOpenLocalTrashSummary = useMemo(
    () =>
      aggregateLocalTrashSummaries(
        openWorkspaceIds,
        openPstSessions.map((session) => session.localTrashSummary),
      ),
    [openPstSessions, openWorkspaceIds],
  );
  const activeWorkspaceOperation = workspace
    ? workspaceOperationStates[workspace.id] ?? null
    : null;
  const anyWorkspaceIntegrityOperationRunning = Object.values(workspaceOperationStates).some(
    (operation) => operation.running && operation.integrityOperationKind != null,
  );
  const operationWorkspaceId = operationWorkspaceIdRef.current;
  const pendingWorkspaceFlowBelongsToActiveTab =
    pendingWorkspaceFlowOwnerId === (workspace?.id ?? null);
  const foregroundOperationCanDisplay =
    (!pendingOpenPlan && !pendingPreflightOpen) || pendingWorkspaceFlowBelongsToActiveTab;
  const foregroundWorkspaceOperation =
    foregroundOperationCanDisplay &&
    foregroundOperationWorkspaceId &&
    !openWorkspaceIds.includes(foregroundOperationWorkspaceId)
      ? workspaceOperationStates[foregroundOperationWorkspaceId] ?? null
      : null;
  const visibleWorkspaceOperation =
    foregroundWorkspaceOperation ?? activeWorkspaceOperation;
  const canSwitchTabsDuringOperation =
    isImporting &&
    !restoringPlannedWorkspace &&
    operationWorkspaceId != null;
  const effectiveAllOpenScope = effectiveAllOpenLocalTrashQueryScope(
    {
      selectedWorkspaceId: allOpenFolderSelection.workspaceId,
      folderId: allOpenFolderSelection.folderId,
      includeSubfolders,
    },
    messageVisibility,
    localTrashAllOpenWorkspaceId,
  );
  const allOpenSelectedWorkspaceId =
    searchScope === "all_open" ? effectiveAllOpenScope.selectedWorkspaceId : null;
  const allOpenSelectedFolderId =
    searchScope === "all_open" ? effectiveAllOpenScope.folderId : null;
  const allOpenSelectedSession = allOpenSelectedWorkspaceId
    ? openPstSessions.find((session) => session.workspace.id === allOpenSelectedWorkspaceId) ?? null
    : null;
  const allOpenSelectedFolder =
    allOpenSelectedSession && allOpenSelectedFolderId != null
      ? allOpenSelectedSession.folders.find((folder) => folder.id === allOpenSelectedFolderId) ?? null
      : null;
  const currentFolderPresentationSelection = resolveFolderPresentationSelection(
    folderHierarchy,
    selectedFolderId,
  );
  const allOpenSelectedHierarchy = allOpenSelectedWorkspaceId
    ? openSessionFolderTrees.find(
        ({ session }) => session.workspace.id === allOpenSelectedWorkspaceId,
      )?.hierarchy ?? null
    : null;
  const allOpenFolderPresentationSelection = allOpenSelectedHierarchy
    ? resolveFolderPresentationSelection(allOpenSelectedHierarchy, allOpenSelectedFolderId)
    : null;
  const activeSearchWorkspaceIds =
    searchScope === "all_open"
      ? allOpenSelectedWorkspaceId
        ? [allOpenSelectedWorkspaceId]
        : openWorkspaceIds
      : workspace
        ? [workspace.id]
        : [];
  const isCrossPstSearch = searchScope !== "current";
  const shouldUseMultiWorkspaceSearch =
    searchScope === "all_open" &&
    allOpenSelectedWorkspaceId == null &&
    openWorkspaceIds.length > 1;
  const singleSearchWorkspaceId =
    searchScope === "all_open"
      ? (allOpenSelectedWorkspaceId ??
        (openWorkspaceIds.length === 1 ? openWorkspaceIds[0] : null))
      : (workspace?.id ?? null);
  const effectiveFolderId =
    messageVisibility === "trashed"
      ? null
      : searchScope === "all_open"
      ? allOpenSelectedFolderId
      : searchFilters.folderScope === "all"
        ? null
        : selectedFolderId;
  const effectiveIncludeSubfolders =
    messageVisibility === "trashed"
      ? false
      : searchScope === "all_open"
      ? includeSubfolders
      : searchFilters.folderScope === "current_subfolders";
  const conversationScopes = useMemo<ConversationWorkspaceScope[]>(() => {
    if (messageVisibility === "trashed") return [];
    if (searchScope === "all_open") {
      if (allOpenSelectedWorkspaceId) {
        return [
          {
            workspaceId: allOpenSelectedWorkspaceId,
            folderId: allOpenSelectedFolderId,
            includeSubfolders,
          },
        ];
      }
      return openWorkspaceIds.map((workspaceId) => ({
        workspaceId,
        folderId: null,
        includeSubfolders: false,
      }));
    }
    return workspace
      ? [
          {
            workspaceId: workspace.id,
            folderId: effectiveFolderId,
            includeSubfolders: effectiveIncludeSubfolders,
          },
        ]
      : [];
  }, [
    allOpenSelectedFolderId,
    allOpenSelectedWorkspaceId,
    effectiveFolderId,
    effectiveIncludeSubfolders,
    includeSubfolders,
    openWorkspaceIds,
    messageVisibility,
    searchScope,
    workspace,
  ]);
  const conversationScopeKey = conversationScopes
    .map(
      (scope) =>
        `${scope.workspaceId}:${scope.folderId ?? "all"}:${scope.includeSubfolders ? "sub" : "exact"}`,
    )
    .join("|");
  const activeSearchWorkspaceKey = activeSearchWorkspaceIds.join("|");
  const draftSearchSnapshot = useMemo(
    () =>
      createAppliedSearchSnapshot(
        { query: search, ...searchFilters },
        {
          scope: searchScope,
          activeWorkspaceId: workspace?.id ?? null,
          workspaceIds: activeSearchWorkspaceIds,
          selectedWorkspaceId: singleSearchWorkspaceId,
          useMultiWorkspace: shouldUseMultiWorkspaceSearch,
          singleWorkspaceId: singleSearchWorkspaceId,
          folderId: effectiveFolderId,
          includeSubfolders: effectiveIncludeSubfolders,
          conversationScopes,
          listMode,
          messageSort: sortOrder,
          conversationSort,
          sessionGeneration: activeSessionGeneration,
          visibility: messageVisibility,
        },
      ),
    [
      activeSearchWorkspaceKey,
      activeSessionGeneration,
      conversationScopeKey,
      conversationSort,
      effectiveFolderId,
      effectiveIncludeSubfolders,
      listMode,
      messageVisibility,
      search,
      searchFilters,
      searchScope,
      shouldUseMultiWorkspaceSearch,
      singleSearchWorkspaceId,
      sortOrder,
      workspace?.id,
    ],
  );
  const draftSearchTextKey = appliedSearchTextKey(draftSearchSnapshot);
  const draftSearchNonTextKey = appliedSearchNonTextKey(draftSearchSnapshot);
  const relevanceAvailability = getRelevanceAvailability(draftSearchSnapshot);
  const appliedSearch = appliedSearchVersion.snapshot;
  const activeAdvancedFilterCount = getActiveFilterCount(appliedSearch);
  const isSearchActive = isAppliedSearchActive(appliedSearch);
  const isSearching =
    searchLoading?.generation === appliedSearchVersion.generation &&
    searchLoading.mode === appliedSearch.listMode;
  const isLoadingMoreMessages =
    messageLoadMoreRequest?.generation === appliedSearchVersion.generation;
  const isLoadingMoreConversations =
    conversationLoadMoreRequest?.generation === appliedSearchVersion.generation;
  const activeSearchError =
    searchError?.generation === appliedSearchVersion.generation ? searchError.message : null;
  const appliedIsCrossPstSearch = appliedSearch.scope === "all_open";
  const appliedSelectedWorkspaceId = appliedIsCrossPstSearch
    ? appliedSearch.selectedWorkspaceId
    : appliedSearch.activeWorkspaceId;
  const appliedSelectedSession = appliedSelectedWorkspaceId
    ? openPstSessions.find(
        (session) => session.workspace.id === appliedSelectedWorkspaceId,
      ) ?? null
    : null;
  const appliedSelectedFolder =
    appliedSelectedSession && appliedSearch.folderId != null
      ? appliedSelectedSession.folders.find((folder) => folder.id === appliedSearch.folderId) ?? null
      : null;
  const appliedLocalTrashAvailable = appliedSelectedWorkspaceId
    ? isLocalTrashHealthy(appliedSelectedSession?.localTrashSummary ?? null)
    : allOpenLocalTrashSummary.available;
  const highlightTerms = useMemo(
    () => highlightTermsForSnapshot(appliedSearch),
    [appliedSearch],
  );
  const collapsedFolderIdentityLists = useMemo(
    () => ({
      simplified: Array.from(collapsedFolderIdentitiesByMode.simplified).sort(),
      original: Array.from(collapsedFolderIdentitiesByMode.original).sort(),
    }),
    [collapsedFolderIdentitiesByMode],
  );
  const paneLayoutModeClass =
    layoutMode === "outlook" ? "pane-layout-outlook" : "pane-layout-three";
  const paneLayoutClassName = [
    "pane-layout",
    paneLayoutModeClass,
    foldersHidden ? "folders-hidden" : "",
  ]
    .filter(Boolean)
    .join(" ");

  function resetSourceEmlViewer() {
    setSourceEmlOpen(false);
    setSourceEmlView(null);
    setSourceEmlContext(null);
    setSourceEmlMode("rendered");
    setIsLoadingSourceEml(false);
    setSourceEmlStatus(null);
    setSourceEmlSaveResult(null);
    setSourceEmlRawSearch("");
    resetPrintableStatus();
  }

  function resetPrintableStatus() {
    setPrintableSaveResult(null);
    setIsSavingPrintable(false);
    setPrintStatus(null);
  }

  useEffect(() => {
    void checkReadpstStatus();
    let disposed = false;
    let unlistenImport: (() => void) | null = null;
    let unlistenExternal: (() => void) | null = null;

    void (async () => {
      try {
        unlistenImport = await listen<unknown>("import-progress", (event) => {
          const parsed = parseImportProgressSnapshot(event.payload);
          if (!parsed.ok) return;
          const incoming = parsed.value;
          const integrityOperation = integrityOperationTokensRef.current.get(incoming.workspaceId);
          if (
            integrityOperation &&
            typeof event.payload === "object" &&
            event.payload !== null &&
            "operationId" in event.payload &&
            event.payload.operationId !== integrityOperation.operationId
          ) {
            return;
          }
          const receivedAt = monotonicMilliseconds();

          setWorkspaceOperationStates((current) => {
            const operation = current[incoming.workspaceId];
            if (!operation || operation.progressOperationId !== incoming.operationId) return current;
            const reduced = reduceImportProgress(operation.progress, incoming, {
              acceptedWorkspaceIds: Object.keys(current),
            });
            if (!reduced.accepted || !reduced.changed) return current;
            return {
              ...current,
              [incoming.workspaceId]: {
                ...operation,
                progress: reduced.snapshot,
                progressReceivedAtMonotonicMs: receivedAt,
                running: reduced.snapshot.status === "active",
                notice: null,
                error: null,
              },
            };
          });
        });
        unlistenExternal = await listen<ExternalFileOpenBatch>(
          "external-file-open",
          (event) => {
            if (restorePromptTimerRef.current != null) {
              window.clearTimeout(restorePromptTimerRef.current);
              restorePromptTimerRef.current = null;
            }
            setRestorePromptVisible(false);
            externalBatchEnqueueRef.current(event.payload);
          },
        );
        const ready = await invoke<ExternalFileOpenReady>(
          "frontend_ready_for_file_opens",
        );
        if (disposed) return;
        if (ready.externalOpenReceived || ready.batches.length > 0) {
          setRestorePromptVisible(false);
          for (const batch of ready.batches) externalBatchEnqueueRef.current(batch);
        } else {
          restorePromptTimerRef.current = window.setTimeout(() => {
            setRestorePromptVisible(Boolean(savedSession?.entries.length));
            restorePromptTimerRef.current = null;
          }, 300);
        }
      } catch (err) {
        if (!disposed) setError(getErrorMessage(err));
      }
    })();

    return () => {
      disposed = true;
      unlistenImport?.();
      unlistenExternal?.();
      if (restorePromptTimerRef.current != null) {
        window.clearTimeout(restorePromptTimerRef.current);
        restorePromptTimerRef.current = null;
      }
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    let unlistenDrop: (() => void) | null = null;
    void listen<boolean>("external-file-drag-active", (event) => {
      setDropOverlayVisible(event.payload);
    })
      .then((unlisten) => {
        if (disposed) {
          unlisten();
        } else {
          unlistenDrop = unlisten;
        }
      })
      .catch((err) => {
        if (!disposed) setError(`Could not enable file drag and drop: ${getErrorMessage(err)}`);
      });

    return () => {
      disposed = true;
      unlistenDrop?.();
    };
  }, []);

  externalBatchEnqueueRef.current = enqueueExternalFileOpenBatch;

  function enqueueExternalFileOpenBatch(batch: ExternalFileOpenBatch) {
    externalFileOpenChainRef.current = externalFileOpenChainRef.current
      .catch(() => undefined)
      .then(() => processExternalFileOpenBatch(batch));
  }

  async function processExternalFileOpenBatch(batch: ExternalFileOpenBatch) {
    setRestorePromptVisible(false);
    const messageFiles = batch.files.filter((file) => file.fileKind !== "pst");
    const pstFiles = batch.files.filter((file) => file.fileKind === "pst");
    const messageResults = await Promise.all(
      messageFiles.map((file) => openStandaloneMessagePreviewWindow(file)),
    );
    const opened = messageResults.filter(Boolean).length;
    enqueueExternalPstPaths(pstFiles.map((file) => file.path));

    if (batch.warnings.length > 0) {
      appendVisibleError(batch.warnings.join(" "));
    } else if (opened > 0 && pstFiles.length === 0) {
      setNotice(
        `Opened ${formatCount(opened)} message${opened === 1 ? "" : "s"}.`,
      );
    }
  }

  function appendVisibleError(message: string) {
    setError((current) => (current ? `${current}\n${message}` : message));
  }

  function enqueueExternalPstPaths(paths: string[]) {
    for (const path of paths) {
      if (
        activeExternalPstPathRef.current !== path &&
        !externalPstQueueRef.current.includes(path)
      ) {
        externalPstQueueRef.current.push(path);
      }
    }
    void processExternalPstQueue();
  }

  async function processExternalPstQueue() {
    if (externalPstProcessingRef.current || activeExternalPstPathRef.current) return;
    externalPstProcessingRef.current = true;

    try {
      while (externalPstQueueRef.current.length > 0) {
        const pstPath = externalPstQueueRef.current.shift()!;
        activeExternalPstPathRef.current = pstPath;
        const openSession = openPstSessions.find(
          (session) => session.workspace.pstPath === pstPath,
        );
        if (openSession) {
          await activateWorkspaceTab(openSession.workspace.id, { resetMessageList: true });
          activeExternalPstPathRef.current = null;
          setNotice(`Activated ${pstDisplayName(openSession.workspace)}.`);
          continue;
        }

        try {
          const plan = await invoke<PstOpenPlan>("plan_pst_open", {
            path: pstPath,
            workspaceLocationMode,
          });
          const matchingExisting = plan.existingWorkspaces.find(
            (existing) => existing.workspacePath === plan.selectedWorkspacePath,
          );
          const opensAutomatically =
            Boolean(matchingExisting?.isComplete) && plan.existingWorkspaces.length === 1;
          const waitsForWorkspaceChoice = plan.existingWorkspaces.length > 0 && !opensAutomatically;
          const waitsForPreflight = plan.existingWorkspaces.length === 0;

          await handlePstOpenPlan(plan, true);
          if (waitsForWorkspaceChoice || waitsForPreflight) return;
          activeExternalPstPathRef.current = null;
        } catch (err) {
          appendVisibleError(`Could not open ${pstPath}: ${getErrorMessage(err)}`);
          activeExternalPstPathRef.current = null;
        }
      }
    } finally {
      externalPstProcessingRef.current = false;
    }
  }

  function finishExternalPstRequest(pstPath: string) {
    if (activeExternalPstPathRef.current !== pstPath) return;
    activeExternalPstPathRef.current = null;
    window.queueMicrotask(() => void processExternalPstQueue());
  }

  function cancelPendingPstOpen(pstPath: string) {
    if (foregroundOperationWorkspaceId) {
      clearWorkspaceOperationState(foregroundOperationWorkspaceId);
      setForegroundOperationWorkspaceId(null);
    }
    setPendingOpenPlan(null);
    setPendingPreflightOpen(null);
    setPendingPlannedRepair(null);
    setPendingPlannedDelete(null);
    repairConfirmReturnFocusRef.current = null;
    setPendingWorkspaceFlowOwnerId(null);
    finishExternalPstRequest(pstPath);
  }

  async function checkReadpstStatus() {
    try {
      const status = await invoke<ReadpstStatus>("check_readpst");
      setReadpstStatus(status);
      if (status.available) {
        setSetupCommand(null);
        if (error?.includes("readpst")) setError(null);
      } else {
        setSetupCommand(status.setupCommand);
      }
    } catch (err) {
      setError(getErrorMessage(err));
      setSetupCommand(getSetupCommand(err));
    }
  }

  async function loadAppDiagnostics() {
    if (isLoadingAppDiagnostics) return;
    setIsLoadingAppDiagnostics(true);
    setAboutStatus(null);
    try {
      setAppDiagnostics(await invoke<AppDiagnostics>("get_app_diagnostics"));
    } catch (err) {
      setAboutStatus(`Could not load diagnostics: ${getErrorMessage(err)}`);
    } finally {
      setIsLoadingAppDiagnostics(false);
    }
  }

  async function copyAppDiagnostics() {
    if (!appDiagnostics) return;
    try {
      await navigator.clipboard.writeText(diagnosticsText(appDiagnostics));
      setAboutStatus("Diagnostics copied.");
    } catch (err) {
      setAboutStatus(`Could not copy diagnostics: ${getErrorMessage(err)}`);
    }
  }

  async function revealApplicationLogs() {
    try {
      await invoke("reveal_application_logs");
      setAboutStatus("Revealed application logs in Finder.");
    } catch (err) {
      setAboutStatus(`Could not reveal logs: ${getErrorMessage(err)}`);
    }
  }

  async function revealProjectLicense() {
    try {
      await invoke("reveal_project_license");
      setAboutStatus("Revealed the packaged project license in Finder.");
    } catch (err) {
      setAboutStatus(`Could not reveal the project license: ${getErrorMessage(err)}`);
    }
  }

  async function revealThirdPartyNotices() {
    try {
      await invoke("reveal_third_party_notices");
      setAboutStatus("Revealed the packaged third-party notices in Finder.");
    } catch (err) {
      setAboutStatus(`Could not reveal third-party notices: ${getErrorMessage(err)}`);
    }
  }

  function closeAbout() {
    closeActivePopover();
  }

  function finishSupportApproval() {
    clearHelpState();
    setOpenPopoverId((current) =>
      closeOpenPopover(current, "help") as CustomPopoverId | null,
    );
  }

  async function copyDisplayedError(message: string) {
    try {
      await navigator.clipboard.writeText(message);
      setNotice("Error details copied.");
    } catch (err) {
      setNotice(`Could not copy error details: ${getErrorMessage(err)}`);
    }
  }

  function rememberRecentPst(path: string) {
    setRecentPsts((current) => recentPstsWithPath(current, path));
  }

  function clearRecentPsts() {
    setRecentPsts([]);
    setNotice("Recent PSTs cleared.");
  }

  async function openRecentPst(path: string) {
    if (localTrashMutationPending) return;
    setError(null);
    setNotice(null);
    setSetupCommand(null);
    setPendingPreflightOpen(null);
    setPendingOpenPlan(null);
    setPendingPlannedRepair(null);
    setPendingPlannedDelete(null);
    setPendingWorkspaceFlowOwnerId(null);
    setIsBusy(true);

    try {
      const plan = await invoke<PstOpenPlan>("plan_pst_open", {
        path,
        workspaceLocationMode,
      });
      await handlePstOpenPlan(plan);
    } catch (err) {
      setError(getErrorMessage(err) || "PST or workspace not available.");
      setSetupCommand(getSetupCommand(err));
    } finally {
      setIsBusy(false);
    }
  }

  async function restorePreviousSession() {
    if (!savedSession?.entries.length) return;

    setError(null);
    setNotice(null);
    setRestoreStatus("Restoring previous session...");
    setIsBusy(true);

    const restored: OpenPstSession[] = [];
    const skipped: SavedWorkspaceSession[] = [];

    try {
      const currentSearch = searchApplicationRef.current;
      if (currentSearch) {
        const resetSnapshot = clearAppliedSearchSnapshot(
          replaceAppliedSearchDraft(currentSearch.applied.snapshot, emptySearchDraft()),
          searchScope,
        );
        invalidateSearchLifecycle(resetSnapshot);
      }
      setSearch("");
      closeActivePopover();
      setSearchFilters(defaultAdvancedSearchFilters);
      setAllOpenFolderSelection({ workspaceId: null, folderId: null });

      for (const entry of savedSession.entries) {
        try {
          const nextWorkspace = await invoke<WorkspaceSummary>("open_existing_workspace_from_session", {
            pstPath: entry.pstPath,
            workspacePath: entry.workspacePath,
            workspaceId: entry.workspaceId,
            workspaceLocationMode: entry.workspaceLocationMode,
          });
          const nextSession = await loadWorkspaceSession(
            nextWorkspace,
            entry.folderSelection,
          );
          registerWorkspaceSession(nextSession);
          restored.push(nextSession);
          rememberRecentPst(nextWorkspace.pstPath);
        } catch {
          skipped.push(entry);
        }
      }

      const preferredActiveId =
        savedSession.lastActiveWorkspaceId &&
        restored.some(
          (restoredSession) =>
            restoredSession.workspace.id === savedSession.lastActiveWorkspaceId,
        )
          ? savedSession.lastActiveWorkspaceId
          : restored[0]?.workspace.id;

      if (preferredActiveId) {
        const activeWorkspace = await invoke<WorkspaceSummary>("activate_workspace", {
          workspaceId: preferredActiveId,
        });
        const restoredSession = restored.find(
          (session) => session.workspace.id === preferredActiveId,
        );
        if (restoredSession) {
          const activeSession = { ...restoredSession, workspace: activeWorkspace };
          registerWorkspaceSession(activeSession);
          applyActiveWorkspaceSession(activeSession, true);
          setRestoreStatus("Loading messages...");
        }
      }

      if (restored.length > 0) {
        setNotice(
          skipped.length
            ? `Restored ${formatCount(restored.length)} PST${
                restored.length === 1 ? "" : "s"
              }. PST or workspace not available for ${formatCount(skipped.length)} saved item${
                skipped.length === 1 ? "" : "s"
              }.`
            : `Restored ${formatCount(restored.length)} PST${
                restored.length === 1 ? "" : "s"
              } from last session.`,
        );
        setError(null);
      } else {
        setError("PST or workspace not available.");
      }
    } finally {
      setRestorePromptVisible(false);
      setRestoreStatus(null);
      setSessionPersistenceReady(true);
      setIsBusy(false);
    }
  }

  function startFreshPreviousSession() {
    invalidateSearchLifecycle();
    window.localStorage.removeItem(previousSessionStorageKey);
    setSavedSession(null);
    setRestorePromptVisible(false);
    setSessionPersistenceReady(true);
    setRestoreStatus(null);
    setNotice("Previous session forgotten. Workspaces and original PST files were not deleted.");
  }

  useEffect(() => {
    window.localStorage.setItem(workspaceLocationStorageKey, workspaceLocationMode);
  }, [workspaceLocationMode]);

  useEffect(() => {
    const nextWorkspaceId = workspace?.id ?? null;
    if (activeWorkspaceIdRef.current !== nextWorkspaceId) {
      activeWorkspaceChangeSequenceRef.current += 1;
      activeWorkspaceIdRef.current = nextWorkspaceId;
    }
  }, [workspace?.id]);

  useEffect(() => {
    if (
      localTrashResetConfirmOpen &&
      localTrashResetWorkspaceIdRef.current !== (workspace?.id ?? null)
    ) {
      localTrashResetWorkspaceIdRef.current = null;
      localTrashResetReturnFocusRef.current = null;
      setLocalTrashResetConfirmOpen(false);
    }
  }, [localTrashResetConfirmOpen, workspace?.id]);

  useEffect(() => {
    openPstSessionsRef.current = openPstSessions;
  }, [openPstSessions]);

  useEffect(() => {
    window.localStorage.setItem(paneWidthStorageKey, JSON.stringify(paneWidths));
  }, [paneWidths]);

  useEffect(() => {
    window.localStorage.setItem(folderHiddenStorageKey, String(foldersHidden));
  }, [foldersHidden]);

  useEffect(() => {
    window.localStorage.setItem(
      collapsedFolderIdentitiesStorageKey,
      JSON.stringify(collapsedFolderIdentityLists),
    );
  }, [collapsedFolderIdentityLists]);

  useEffect(() => {
    saveFolderHierarchyMode(window.localStorage, folderHierarchyMode);
  }, [folderHierarchyMode]);

  useEffect(() => {
    window.localStorage.setItem(layoutModeStorageKey, layoutMode);
  }, [layoutMode]);

  useEffect(() => {
    window.localStorage.setItem(messageListDisplayStorageKey, messageListDisplayMode);
  }, [messageListDisplayMode]);

  useEffect(() => {
    if (messageVisibility === "active") {
      window.localStorage.setItem(listModeStorageKey, listMode);
      previousActiveListModeRef.current = listMode;
    }
  }, [listMode, messageVisibility]);

  useEffect(() => {
    saveSearchScope(window.localStorage, searchScope);
  }, [searchScope]);

  useEffect(() => {
    window.localStorage.setItem(recentPstsStorageKey, JSON.stringify(recentPsts));
  }, [recentPsts]);

  useLayoutEffect(() => {
    if (sortOrder === draftSearchSnapshot.messageSort) return;
    // Conversation and multi-workspace ranking need separate designs; never send stale relevance.
    forceNextSearchApplication();
    setSortOrder(draftSearchSnapshot.messageSort);
  }, [draftSearchSnapshot.messageSort, sortOrder]);

  useEffect(() => {
    if (!sessionPersistenceReady) return;
    const nextSession = savedSessionFromOpenSessions(openPstSessions, workspace?.id ?? null);
    if (nextSession) {
      window.localStorage.setItem(previousSessionStorageKey, JSON.stringify(nextSession));
      setSavedSession(nextSession);
    } else {
      window.localStorage.removeItem(previousSessionStorageKey);
      setSavedSession(null);
    }
  }, [openPstSessions, sessionPersistenceReady, workspace?.id]);

  useEffect(() => {
    const clampToWindow = () => setPaneWidths((current) => clampPaneWidths(current));
    clampToWindow();
    window.addEventListener("resize", clampToWindow);
    return () => window.removeEventListener("resize", clampToWindow);
  }, []);

  useLayoutEffect(() => {
    clearSearchDebounceTimer();
    const current = searchApplicationRef.current;
    if (!current) return;

    const forceImmediate = forceImmediateSearchApplyRef.current;
    forceImmediateSearchApplyRef.current = false;
    if (
      forceImmediate ||
      appliedSearchNonTextKey(current.applied.snapshot) !== draftSearchNonTextKey
    ) {
      const immediate = applySearchSnapshotImmediately(current, draftSearchSnapshot);
      publishSearchApplicationState(immediate.state);
      return;
    }

    const queued = queueTextSearchSnapshot(current, draftSearchSnapshot);
    searchApplicationRef.current = queued.state;
    if (queued.token == null) return;

    const timeout = window.setTimeout(() => {
      if (searchDebounceTimerRef.current === timeout) {
        searchDebounceTimerRef.current = null;
      }
      const pending = searchApplicationRef.current;
      if (!pending) return;
      const committed = commitQueuedSearchSnapshot(pending, queued.token!);
      publishSearchApplicationState(committed.state);
    }, searchDebounceMs);
    searchDebounceTimerRef.current = timeout;
    return () => {
      window.clearTimeout(timeout);
      if (searchDebounceTimerRef.current === timeout) {
        searchDebounceTimerRef.current = null;
      }
    };
  }, [draftSearchNonTextKey, draftSearchTextKey]);

  useEffect(() => {
    if (!notice) return;
    const timeout = window.setTimeout(() => setNotice(null), 4200);
    return () => window.clearTimeout(timeout);
  }, [notice]);

  useEffect(() => {
    if (!workspaceDeleteToast) return;
    const timeout = window.setTimeout(() => setWorkspaceDeleteToast(null), 6200);
    return () => window.clearTimeout(timeout);
  }, [workspaceDeleteToast]);

  useEffect(
    () => () => {
      clearSearchDebounceTimer();
      cancelBackendSearch(cancellationForGeneration(searchGenerationRef.current));
      for (const timeout of workspaceOperationDismissTimersRef.current.values()) {
        window.clearTimeout(timeout);
      }
      workspaceOperationDismissTimersRef.current.clear();
      workspaceOperationDismissGenerationsRef.current.clear();
    },
    [],
  );

  useLayoutEffect(() => {
    if (restoreStatus) return;
    const request = appliedSearchVersion;
    const snapshot = request.snapshot;
    const canLoadMessages = snapshot.useMultiWorkspace
      ? snapshot.workspaceIds.length > 0
      : snapshot.singleWorkspaceId != null;
    const canLoadConversations = snapshot.conversationScopes.length > 0;

    if (
      (snapshot.listMode === "messages" && !canLoadMessages) ||
      (snapshot.listMode === "conversations" && !canLoadConversations)
    ) {
      if (!isSearchGenerationCurrent(request.generation, searchGenerationRef.current)) return;
      setMessages([]);
      setMessageTotalCount(0);
      setMessageHasMore(false);
      setMessageCountState(createSearchExactCountState(request.generation));
      setWorkspaceSearchCounts([]);
      setConversations([]);
      setConversationTotalCount(0);
      setConversationMatchingMessageCount(0);
      setConversationHasMore(false);
      setConversationCountState(createSearchExactCountState(request.generation));
      setConversationIndexedWorkspaceCount(0);
      setConversationWorkspaceIssues([]);
      setExpandedConversations({});
      setSearchLoading(null);
      setMessageLoadMoreRequest(null);
      setConversationLoadMoreRequest(null);
      return;
    }

    if (snapshot.listMode === "conversations") {
      void loadConversationsPage(false, request);
      void loadConversationCount(request);
    } else {
      void loadMessagesPage(false, request);
      void loadMessageCount(request);
    }
  }, [appliedSearchVersion, restoreStatus]);

  useEffect(() => {
    if (!selectedMessage) return;
    window.requestAnimationFrame(() => {
      if (previewScrollRef.current) {
        previewScrollRef.current.scrollTop = 0;
      }
    });
  }, [selectedMessage?.id, workspace?.id]);

  useEffect(() => {
    if (!workspace || !selectedMessage || readerMode !== "sanitized_html") {
      setIsRenderingHtml(false);
      return;
    }

    if (!selectedMessage.bodyHtmlAvailable) {
      setHtmlRender({
        htmlAvailable: false,
        sanitizedHtml: "",
        remoteImagesBlocked: false,
        remoteImageCount: 0,
        embeddedImageCount: 0,
        error: null,
      });
      setIsRenderingHtml(false);
      return;
    }

    let cancelled = false;
    const messageWorkspaceId = selectedMessage.workspaceId ?? workspace.id;
    setIsRenderingHtml(true);
    invoke<HtmlRenderResult>("render_message_html", {
      workspaceId: messageWorkspaceId,
      messageId: selectedMessage.id,
    })
      .then((result) => {
        if (!cancelled) setHtmlRender(result);
      })
      .catch((err) => {
        if (!cancelled) {
          setHtmlRender({
            htmlAvailable: false,
            sanitizedHtml: "",
            remoteImagesBlocked: false,
            remoteImageCount: 0,
            embeddedImageCount: 0,
            error: getErrorMessage(err),
          });
        }
      })
      .finally(() => {
        if (!cancelled) setIsRenderingHtml(false);
      });

    return () => {
      cancelled = true;
    };
  }, [workspace, selectedMessage, readerMode]);

  async function loadWorkspaceSession(
    nextWorkspace: WorkspaceSummary,
    requestedSelection?: WorkspaceFolderSelection,
  ): Promise<OpenPstSession> {
    const [nextFolders, nextWorkspaceSize, nextLocalTrashSummary] = await Promise.all([
      invoke<Folder[]>("list_folders", {
        workspaceId: nextWorkspace.id,
      }),
      invoke<WorkspaceSize>("get_workspace_size", {
        workspaceId: nextWorkspace.id,
      }),
      invoke<LocalTrashSummary>("read_local_trash_summary", {
        workspaceId: nextWorkspace.id,
      }).catch(() => unavailableLocalTrashSummary(nextWorkspace.id)),
    ]);

    const existingSession = openPstSessions.find(
      (session) => session.workspace.id === nextWorkspace.id,
    );
    const existingSelection = existingSession?.folderSelection;
    const activeSelection =
      workspace?.id === nextWorkspace.id
        ? messageVisibility === "trashed"
          ? localTrashWorkspaceFolderSelection(nextWorkspace.id, includeSubfolders)
          : {
              workspaceId: nextWorkspace.id,
              folderId: selectedFolderId,
              virtualFolder: selectedFolderId == null ? "all_mail" as const : null,
              includeSubfolders,
            }
        : null;
    const folderSelection = resolveWorkspaceFolderSelection(
      nextWorkspace.id,
      nextFolders,
      requestedSelection ?? existingSelection ?? activeSelection,
    );
    const canonicalFolderSelection = isLocalTrashFolderSelection(folderSelection)
      ? resolveWorkspaceFolderSelection(
          nextWorkspace.id,
          nextFolders,
          existingSession?.canonicalFolderSelection ?? defaultWorkspaceFolderSelection(nextWorkspace.id),
        )
      : folderSelection;

    return {
      workspace: nextWorkspace,
      workspaceSize: nextWorkspaceSize,
      folders: nextFolders,
      folderSelection,
      canonicalFolderSelection,
      localTrashSummary: nextLocalTrashSummary,
      status: nextWorkspace.integrityClassification,
    };
  }

  function registerWorkspaceSession(nextSession: OpenPstSession) {
    setOpenPstSessions((current) => {
      const existingIndex = current.findIndex(
        (session) => session.workspace.id === nextSession.workspace.id,
      );
      const next =
        existingIndex === -1
          ? [...current, nextSession]
          : current.map((session, index) =>
              index === existingIndex ? nextSession : session,
            );
      openPstSessionsRef.current = next;
      return next;
    });
  }

  function applyActiveWorkspaceSession(nextSession: OpenPstSession, resetMessageList: boolean) {
    const { workspace: nextWorkspace, workspaceSize: nextWorkspaceSize, folders: nextFolders } =
      nextSession;
    const selection = nextSession.folderSelection;

    activeWorkspaceChangeSequenceRef.current += 1;
    activeWorkspaceIdRef.current = nextWorkspace.id;
    setWorkspace(nextWorkspace);
    setWorkspaceSize(nextWorkspaceSize);
    setFolders(nextFolders);
    const canonicalSelection = nextSession.canonicalFolderSelection;
    const opensLocalTrash = isLocalTrashFolderSelection(selection);
    if (searchScope === "current") {
      setLocalTrashAllOpenWorkspaceId(null);
      setMessageVisibility(opensLocalTrash ? "trashed" : "active");
      if (opensLocalTrash) {
        previousActiveListModeRef.current = listMode;
        setListMode("messages");
      }
    } else if (messageVisibility === "trashed") {
      setListMode("messages");
    }
    setSelectedFolderId(canonicalSelection.folderId);
    setIncludeSubfolders(canonicalSelection.includeSubfolders);
    setSearchFilters((current) => ({
      ...current,
      folderScope: canonicalSelection.includeSubfolders ? "current_subfolders" : "current",
    }));
    setInitializingWorkspaceId(nextWorkspace.id);
    setActiveSessionGeneration((current) => current + 1);

    if (resetMessageList) {
      setMessages([]);
      setMessageTotalCount(0);
      setMessageHasMore(false);
      setMessageCountState(createSearchExactCountState(searchGenerationRef.current));
      setWorkspaceSearchCounts([]);
      setConversations([]);
      setConversationTotalCount(0);
      setConversationMatchingMessageCount(0);
      setConversationHasMore(false);
      setConversationCountState(createSearchExactCountState(searchGenerationRef.current));
      setConversationWorkspaceIssues([]);
      setExpandedConversations({});
      setSelectedMessage(null);
      setExportResults({});
      resetPrintableStatus();
      resetSourceEmlViewer();
      setHtmlRender(null);
    }
  }

  async function refreshWorkspace(
    nextWorkspace: WorkspaceSummary,
    options: RefreshWorkspaceOptions = {},
  ): Promise<OpenPstSession> {
    const nextSession = await loadWorkspaceSession(nextWorkspace, options.folderSelection);
    registerWorkspaceSession(nextSession);
    if (
      options.onlyIfWorkspaceStillActive &&
      activeWorkspaceIdRef.current !== nextWorkspace.id
    ) {
      return nextSession;
    }

    applyActiveWorkspaceSession(nextSession, options.resetMessageList ?? true);
    return nextSession;
  }

  async function activateWorkspaceTab(
    workspaceId: string,
    options: RefreshWorkspaceOptions = {},
  ) {
    if (restoringPlannedWorkspaceRef.current) return;
    if (localTrashMutationPending) return;
    if (workspace?.id === workspaceId) return;

    const previousWorkspaceId = activeWorkspaceIdRef.current;
    activeWorkspaceChangeSequenceRef.current += 1;
    activeWorkspaceIdRef.current = workspaceId;
    setError(null);
    try {
      const nextWorkspace = await invoke<WorkspaceSummary>("activate_workspace", { workspaceId });
      await refreshWorkspace(nextWorkspace, {
        resetMessageList: options.resetMessageList ?? !isCrossPstSearch,
      });
    } catch (err) {
      activeWorkspaceChangeSequenceRef.current += 1;
      activeWorkspaceIdRef.current = previousWorkspaceId;
      setError(getErrorMessage(err));
    }
  }

  async function closeWorkspaceTab(workspaceId: string) {
    if (localTrashMutationPending) return;
    setError(null);
    if (workspaceOperationStates[workspaceId]?.running === true || isImporting) {
      setError("Cancel or finish the active workspace operation before closing its tab.");
      return;
    }
    try {
      const remainingSessions = openPstSessions.filter(
        (session) => session.workspace.id !== workspaceId,
      );
      const nextActive = await invoke<WorkspaceSummary | null>("close_workspace", { workspaceId });
      invalidateSearchLifecycle();
      localTrashSummaryRequestSequenceRef.current.delete(workspaceId);
      setOpenPstSessions(remainingSessions);
      clearWorkspaceOperationState(workspaceId);
      if (pendingWorkspaceFlowOwnerId === workspaceId) {
        const pendingPstPath = pendingOpenPlan?.pstPath ?? pendingPreflightOpen?.plan.pstPath;
        setPendingOpenPlan(null);
        setPendingPreflightOpen(null);
        setPendingPlannedRepair(null);
        setPendingPlannedDelete(null);
        repairConfirmReturnFocusRef.current = null;
        setPendingWorkspaceFlowOwnerId(null);
        if (pendingPstPath) finishExternalPstRequest(pendingPstPath);
      }
      if (allOpenFolderSelection.workspaceId === workspaceId) {
        setAllOpenFolderSelection({ workspaceId: null, folderId: null });
      }
      if (localTrashAllOpenWorkspaceId === workspaceId) {
        setLocalTrashAllOpenWorkspaceId(null);
      }

      if (workspace?.id !== workspaceId) return;
      if (nextActive) {
        await refreshWorkspace(nextActive);
      } else {
        clearWorkspaceState();
      }
    } catch (err) {
      setError(getErrorMessage(err));
    }
  }

  async function loadMessagesPage(append: boolean, request: AppliedSearchVersion) {
    const snapshot = request.snapshot;
    if (snapshot.listMode !== "messages") return;
    if (!snapshot.useMultiWorkspace && !snapshot.singleWorkspaceId) return;
    if (snapshot.useMultiWorkspace && snapshot.workspaceIds.length === 0) return;
    if (!isSearchGenerationCurrent(request.generation, searchGenerationRef.current)) return;

    const queriedWorkspaceIds = snapshot.useMultiWorkspace
      ? snapshot.workspaceIds
      : snapshot.singleWorkspaceId
        ? [snapshot.singleWorkspaceId]
        : [];
    const paginationMode = getMessagePaginationMode(snapshot);
    const offset = append ? messages.length : 0;
    const requestCursor = cursorForMessagePage(
      messagePaginationRef.current,
      request.generation,
      append,
    );
    if (append && paginationMode === "cursor" && !requestCursor) return;
    if (
      !append &&
      topLevelSearchRequestRef.current?.generation === request.generation &&
      topLevelSearchRequestRef.current.mode === "messages"
    ) {
      return;
    }
    let requestIdentity: SearchLoadingState | PageRequestIdentity;
    if (append) {
      const pending = messageLoadMoreRequestRef.current;
      if (pending?.generation === request.generation) return;
      const requestId = messageLoadMoreRequestSequenceRef.current + 1;
      requestIdentity = {
        ...createSearchOperationIdentity(request.generation, "message-load-more", requestId),
        offset,
        cursor: requestCursor,
        paginationMode,
        requestId,
      };
      messageLoadMoreRequestSequenceRef.current = requestIdentity.requestId;
      messageLoadMoreRequestRef.current = requestIdentity;
      setMessageLoadMoreRequest(requestIdentity);
    } else {
      const requestId = topLevelSearchRequestSequenceRef.current + 1;
      requestIdentity = {
        ...createSearchOperationIdentity(request.generation, "message-page", requestId),
        mode: "messages",
        requestId,
      };
      topLevelSearchRequestSequenceRef.current = requestIdentity.requestId;
      topLevelSearchRequestRef.current = requestIdentity;
      setSearchLoading(requestIdentity);
      setSearchError(null);
    }

    const requestIsCurrent = () => {
      if (!isSearchGenerationCurrent(request.generation, searchGenerationRef.current)) return false;
      const activeRequest = append
        ? messageLoadMoreRequestRef.current
        : topLevelSearchRequestRef.current;
      return activeRequest?.requestId === requestIdentity.requestId;
    };
    const singleWorkspaceSession = snapshot.singleWorkspaceId
      ? openPstSessions.find((session) => session.workspace.id === snapshot.singleWorkspaceId) ?? null
      : null;

    try {
      const result = snapshot.useMultiWorkspace
        ? await invoke<MultiMessagePageResult>("search_messages_multi", {
            workspaceIds: snapshot.workspaceIds,
            query: snapshot.query || null,
            searchFilters: backendSearchFilters(snapshot),
            sortOrder: snapshot.messageSort,
            limit: messagePageSize,
            offset,
            searchGeneration: request.generation,
            searchOperationId: requestIdentity.operationId,
            visibility: snapshot.visibility,
          })
        : await invoke<MessagePageResult>("list_messages", {
            workspaceId: snapshot.singleWorkspaceId,
            folderId: snapshot.folderId,
            query: snapshot.query || null,
            includeSubfolders: snapshot.includeSubfolders,
            searchFilters: backendSearchFilters(snapshot),
            sortOrder: snapshot.messageSort,
            limit: messagePageSize,
            cursor: requestCursor,
            searchGeneration: request.generation,
            searchOperationId: requestIdentity.operationId,
            visibility: snapshot.visibility,
      });
      if (!requestIsCurrent()) return;
      reconcileLocalTrashQueryHealth(result.localTrashStates);
      if (result.paginationMode !== paginationMode) {
        throw new Error("Search pagination mode did not match the active request.");
      }
      if (paginationMode === "cursor" && result.hasMore && !result.nextCursor) {
        throw new Error("Search pagination could not continue safely.");
      }

      const normalizedResult =
        !snapshot.useMultiWorkspace && snapshot.scope === "all_open" && singleWorkspaceSession
          ? {
              ...result,
              items: result.items.map((item) => ({
                ...item,
                workspaceId: singleWorkspaceSession.workspace.id,
                pstDisplayName: pstDisplayName(singleWorkspaceSession.workspace),
                workspacePath: singleWorkspaceSession.workspace.workspacePath,
              })),
            }
          : result;
      messagePaginationRef.current = settleMessagePagination(
        messagePaginationRef.current,
        searchGenerationRef.current,
        request.generation,
        result.paginationMode,
        result.hasMore,
        result.nextCursor,
      );
      setMessageHasMore(result.hasMore);
      setMessages((current) =>
        append
          ? appendUniqueByKey(current, normalizedResult.items, (item) =>
              `${item.workspaceId ?? snapshot.singleWorkspaceId ?? snapshot.activeWorkspaceId}:${item.id}`,
            )
          : normalizedResult.items,
      );
      setSearchError(null);
      if (!append) {
        setSelectedMessage((current) => {
          const currentWorkspaceId = current?.workspaceId ?? snapshot.activeWorkspaceId;
          const currentKey = current
            ? `${currentWorkspaceId ?? ""}:${current.id}`
            : null;
          if (
            !current ||
            preserveSelectedMessageRef.current === currentKey ||
            normalizedResult.items.some(
              (item) =>
                item.id === current.id &&
                (item.workspaceId ?? snapshot.singleWorkspaceId ?? snapshot.activeWorkspaceId) ===
                  (current.workspaceId ?? snapshot.activeWorkspaceId),
            )
          ) {
            return current;
          }
          setHtmlRender(null);
          setExportResults({});
          resetPrintableStatus();
          resetSourceEmlViewer();
          return null;
        });
      }
    } catch (err) {
      if (isSearchCancellationError(err)) return;
      if (requestIsCurrent()) {
        setSearchError({ generation: request.generation, message: getErrorMessage(err) });
      }
    } finally {
      if (!requestIsCurrent()) return;
      if (append) {
        messageLoadMoreRequestRef.current = null;
        setMessageLoadMoreRequest(null);
      } else {
        topLevelSearchRequestRef.current = null;
        setSearchLoading(null);
      }
      setInitializingWorkspaceId((current) =>
        current && queriedWorkspaceIds.includes(current) ? null : current,
      );
    }
  }

  async function loadMessageCount(request: AppliedSearchVersion) {
    const snapshot = request.snapshot;
    if (snapshot.listMode !== "messages") return;
    if (!snapshot.useMultiWorkspace && !snapshot.singleWorkspaceId) return;
    if (snapshot.useMultiWorkspace && snapshot.workspaceIds.length === 0) return;
    if (!isSearchGenerationCurrent(request.generation, searchGenerationRef.current)) return;
    if (messageCountRequestRef.current?.generation === request.generation) return;

    const requestId = messageCountRequestSequenceRef.current + 1;
    const requestIdentity: CountRequestIdentity = {
      ...createSearchOperationIdentity(request.generation, "message-count", requestId),
      mode: "messages",
      requestId,
    };
    messageCountRequestSequenceRef.current = requestId;
    messageCountRequestRef.current = requestIdentity;
    setMessageTotalCount(0);
    setWorkspaceSearchCounts([]);
    setMessageCountState(createSearchExactCountState(request.generation, "pending"));

    const requestIsCurrent = () =>
      isSearchGenerationCurrent(request.generation, searchGenerationRef.current) &&
      messageCountRequestRef.current?.requestId === requestIdentity.requestId;

    try {
      const result = snapshot.useMultiWorkspace
        ? await invoke<MultiMessageCountResult>("count_messages_multi", {
            workspaceIds: snapshot.workspaceIds,
            query: snapshot.query || null,
            searchFilters: backendSearchFilters(snapshot),
            searchGeneration: request.generation,
            searchOperationId: requestIdentity.operationId,
            visibility: snapshot.visibility,
          })
        : await invoke<MessageCountResult>("count_messages", {
            workspaceId: snapshot.singleWorkspaceId,
            folderId: snapshot.folderId,
            query: snapshot.query || null,
            includeSubfolders: snapshot.includeSubfolders,
            searchFilters: backendSearchFilters(snapshot),
            searchGeneration: request.generation,
            searchOperationId: requestIdentity.operationId,
            visibility: snapshot.visibility,
          });
      if (!requestIsCurrent()) return;
      setMessageTotalCount(result.totalCount);
      setWorkspaceSearchCounts(
        snapshot.useMultiWorkspace
          ? (result as MultiMessageCountResult).perWorkspaceCounts
          : [],
      );
      setMessageCountState((current) =>
        settleSearchExactCount(current, searchGenerationRef.current, request.generation, "ready"),
      );
    } catch (err) {
      if (!requestIsCurrent()) return;
      if (isSearchCancellationError(err)) {
        setMessageCountState(createSearchExactCountState(request.generation));
        return;
      }
      setMessageCountState((current) =>
        settleSearchExactCount(
          current,
          searchGenerationRef.current,
          request.generation,
          "unavailable",
        ),
      );
    } finally {
      if (requestIsCurrent()) {
        messageCountRequestRef.current = null;
      }
    }
  }

  async function loadConversationsPage(append: boolean, request: AppliedSearchVersion) {
    const snapshot = request.snapshot;
    if (snapshot.listMode !== "conversations" || !snapshot.conversationScopes.length) return;
    if (!isSearchGenerationCurrent(request.generation, searchGenerationRef.current)) return;

    const queriedWorkspaceIds = snapshot.conversationScopes.map((scope) => scope.workspaceId);
    const offset = append ? conversations.length : 0;
    if (
      !append &&
      topLevelSearchRequestRef.current?.generation === request.generation &&
      topLevelSearchRequestRef.current.mode === "conversations"
    ) {
      return;
    }
    let requestIdentity: SearchLoadingState | PageRequestIdentity;
    if (append) {
      const pending = conversationLoadMoreRequestRef.current;
      if (pending?.generation === request.generation) return;
      const requestId = conversationLoadMoreRequestSequenceRef.current + 1;
      requestIdentity = {
        ...createSearchOperationIdentity(
          request.generation,
          "conversation-load-more",
          requestId,
        ),
        offset,
        requestId,
      };
      conversationLoadMoreRequestSequenceRef.current = requestIdentity.requestId;
      conversationLoadMoreRequestRef.current = requestIdentity;
      setConversationLoadMoreRequest(requestIdentity);
    } else {
      const requestId = topLevelSearchRequestSequenceRef.current + 1;
      requestIdentity = {
        ...createSearchOperationIdentity(request.generation, "conversation-page", requestId),
        mode: "conversations",
        requestId,
      };
      topLevelSearchRequestSequenceRef.current = requestIdentity.requestId;
      topLevelSearchRequestRef.current = requestIdentity;
      setSearchLoading(requestIdentity);
      setSearchError(null);
      setExpandedConversations({});
    }

    const requestIsCurrent = () => {
      if (!isSearchGenerationCurrent(request.generation, searchGenerationRef.current)) return false;
      const activeRequest = append
        ? conversationLoadMoreRequestRef.current
        : topLevelSearchRequestRef.current;
      return activeRequest?.requestId === requestIdentity.requestId;
    };

    try {
      const result = await invoke<ConversationPageResult>("list_conversations", {
        scopes: snapshot.conversationScopes,
        query: snapshot.query || null,
        searchFilters: backendSearchFilters(snapshot),
        conversationSort: snapshot.conversationSort,
        limit: conversationPageSize,
        offset,
        searchGeneration: request.generation,
        searchOperationId: requestIdentity.operationId,
        visibility: snapshot.visibility,
      });
      if (!requestIsCurrent()) return;
      reconcileLocalTrashQueryHealth(result.localTrashStates);

      setConversationHasMore(result.hasMore);
      setConversationIndexedWorkspaceCount(result.indexedWorkspaceCount);
      setConversationWorkspaceIssues(result.unindexedWorkspaces);
      setConversations((current) =>
        append
          ? appendUniqueByKey(
              current,
              result.items,
              (item) => `${item.workspaceId}:${item.conversationId}`,
            )
          : result.items,
      );
      setSearchError(null);
    } catch (err) {
      if (isSearchCancellationError(err)) return;
      if (requestIsCurrent()) {
        setSearchError({ generation: request.generation, message: getErrorMessage(err) });
      }
    } finally {
      if (!requestIsCurrent()) return;
      if (append) {
        conversationLoadMoreRequestRef.current = null;
        setConversationLoadMoreRequest(null);
      } else {
        topLevelSearchRequestRef.current = null;
        setSearchLoading(null);
      }
      setInitializingWorkspaceId((current) =>
        current && queriedWorkspaceIds.includes(current) ? null : current,
      );
    }
  }

  async function loadConversationCount(request: AppliedSearchVersion) {
    const snapshot = request.snapshot;
    if (snapshot.listMode !== "conversations" || !snapshot.conversationScopes.length) return;
    if (!isSearchGenerationCurrent(request.generation, searchGenerationRef.current)) return;
    if (conversationCountRequestRef.current?.generation === request.generation) return;

    const requestId = conversationCountRequestSequenceRef.current + 1;
    const requestIdentity: CountRequestIdentity = {
      ...createSearchOperationIdentity(request.generation, "conversation-count", requestId),
      mode: "conversations",
      requestId,
    };
    conversationCountRequestSequenceRef.current = requestId;
    conversationCountRequestRef.current = requestIdentity;
    setConversationTotalCount(0);
    setConversationMatchingMessageCount(0);
    setConversationCountState(createSearchExactCountState(request.generation, "pending"));

    const requestIsCurrent = () =>
      isSearchGenerationCurrent(request.generation, searchGenerationRef.current) &&
      conversationCountRequestRef.current?.requestId === requestIdentity.requestId;

    try {
      const result = await invoke<ConversationCountResult>("count_conversations", {
        scopes: snapshot.conversationScopes,
        query: snapshot.query || null,
        searchFilters: backendSearchFilters(snapshot),
        searchGeneration: request.generation,
        searchOperationId: requestIdentity.operationId,
        visibility: snapshot.visibility,
      });
      if (!requestIsCurrent()) return;
      setConversationTotalCount(result.totalCount);
      setConversationMatchingMessageCount(result.matchingMessageCount);
      setConversationCountState((current) =>
        settleSearchExactCount(current, searchGenerationRef.current, request.generation, "ready"),
      );
    } catch (err) {
      if (!requestIsCurrent()) return;
      if (isSearchCancellationError(err)) {
        setConversationCountState(createSearchExactCountState(request.generation));
        return;
      }
      setConversationCountState((current) =>
        settleSearchExactCount(
          current,
          searchGenerationRef.current,
          request.generation,
          "unavailable",
        ),
      );
    } finally {
      if (requestIsCurrent()) {
        conversationCountRequestRef.current = null;
      }
    }
  }

  async function loadConversationMessages(
    conversation: ConversationSummary,
    showEntireConversation: boolean,
    append = false,
  ) {
    const request = appliedSearchVersion;
    const snapshot = request.snapshot;
    if (snapshot.listMode !== "conversations") return;
    if (!isSearchGenerationCurrent(request.generation, searchGenerationRef.current)) return;
    const key = conversationStateKey(conversation.workspaceId, conversation.conversationId);
    const current = expandedConversations[key];
    const scope = snapshot.conversationScopes.find(
      (candidate) => candidate.workspaceId === conversation.workspaceId,
    );
    if (!scope) return;
    const pending = expandedConversationRequestsRef.current.get(key);
    if (pending?.generation === request.generation) return;
    const requestId = expandedConversationRequestSequenceRef.current + 1;
    const requestIdentity = {
      ...createSearchOperationIdentity(request.generation, "expanded-conversation", requestId),
      requestId,
    };
    expandedConversationRequestSequenceRef.current = requestIdentity.requestId;
    expandedConversationRequestsRef.current.set(key, requestIdentity);

    setExpandedConversations((states) => ({
      ...states,
      [key]: {
        items: append ? states[key]?.items ?? [] : [],
        matchingMessageCount: states[key]?.matchingMessageCount ?? conversation.matchingMessageCount,
        totalMessageCount: states[key]?.totalMessageCount ?? conversation.totalMessageCount,
        showingEntireConversation: showEntireConversation,
        loading: true,
        error: null,
      },
    }));

    try {
      const result = await invoke<ConversationMessagesResult>("get_conversation_messages", {
        workspaceId: conversation.workspaceId,
        conversationId: conversation.conversationId,
        folderId: scope.folderId,
        includeSubfolders: scope.includeSubfolders,
        query: snapshot.query || null,
        searchFilters: backendSearchFilters(snapshot),
        showEntireConversation,
        limit: conversationMessagePageSize,
        offset: append ? current?.items.length ?? 0 : 0,
        searchGeneration: request.generation,
        searchOperationId: requestIdentity.operationId,
        visibility: snapshot.visibility,
      });
      const activeRequest = expandedConversationRequestsRef.current.get(key);
      if (
        !isExpandedConversationResponseCurrent(
          request.generation,
          searchGenerationRef.current,
          requestIdentity.requestId,
          activeRequest?.requestId,
          activeRequest != null,
        )
      ) {
        return;
      }
      expandedConversationRequestsRef.current.delete(key);
      setExpandedConversations((states) => ({
        ...states,
        ...(states[key]
          ? {
              [key]: {
                items: append
                  ? appendUniqueByKey(
                      states[key].items,
                      result.items,
                      (item) => `${conversation.workspaceId}:${item.id}`,
                    )
                  : result.items,
                matchingMessageCount: result.matchingMessageCount,
                totalMessageCount: result.totalMessageCount,
                showingEntireConversation: result.showingEntireConversation,
                loading: false,
                error: null,
              },
            }
          : {}),
      }));
    } catch (err) {
      if (isSearchCancellationError(err)) return;
      const activeRequest = expandedConversationRequestsRef.current.get(key);
      if (
        !isExpandedConversationResponseCurrent(
          request.generation,
          searchGenerationRef.current,
          requestIdentity.requestId,
          activeRequest?.requestId,
          activeRequest != null,
        )
      ) {
        return;
      }
      expandedConversationRequestsRef.current.delete(key);
      setExpandedConversations((states) => ({
        ...states,
        ...(states[key]
          ? {
              [key]: {
                ...states[key],
                loading: false,
                error: getErrorMessage(err),
              },
            }
          : {}),
      }));
    }
  }

  function toggleConversation(conversation: ConversationSummary) {
    const key = conversationStateKey(conversation.workspaceId, conversation.conversationId);
    if (expandedConversations[key]) {
      const pending = expandedConversationRequestsRef.current.get(key);
      if (pending) {
        const cancellation = cancellationForOperation(pending, searchGenerationRef.current);
        if (cancellation) cancelBackendSearch(cancellation);
      }
      expandedConversationRequestsRef.current.delete(key);
      setExpandedConversations((states) => {
        const next = { ...states };
        delete next[key];
        return next;
      });
      return;
    }
    void loadConversationMessages(conversation, false);
  }

  function clearWorkspaceState() {
    const currentSearch = searchApplicationRef.current;
    if (currentSearch) {
      invalidateSearchLifecycle(
        clearAppliedSearchSnapshot(currentSearch.applied.snapshot, searchScope),
      );
    }
    operationWorkspaceIdRef.current = null;
    activeWorkspaceIdRef.current = null;
    integrityOperationTokensRef.current.clear();
    repairConfirmReturnFocusRef.current = null;
    verificationReportReturnFocusRef.current = null;
    setForegroundOperationWorkspaceId(null);
    setForegroundImportAction(null);
    for (const workspaceId of workspaceOperationDismissTimersRef.current.keys()) {
      clearWorkspaceOperationDismissTimer(workspaceId);
    }
    workspaceOperationDismissGenerationsRef.current.clear();
    setWorkspace(null);
    setOpenPstSessions([]);
    openPstSessionsRef.current = [];
    setWorkspaceOperationStates({});
    setWorkspaceSize(null);
    setFolders([]);
    setSelectedFolderId(null);
    setMessageVisibility("active");
    setAllOpenFolderSelection({ workspaceId: null, folderId: null });
    setLocalTrashAllOpenWorkspaceId(null);
    setInitializingWorkspaceId(null);
    setMessages([]);
    setMessageTotalCount(0);
    setMessageHasMore(false);
    setMessageCountState(createSearchExactCountState(searchGenerationRef.current));
    setWorkspaceSearchCounts([]);
    setConversations([]);
    setConversationTotalCount(0);
    setConversationMatchingMessageCount(0);
    setConversationHasMore(false);
    setConversationCountState(createSearchExactCountState(searchGenerationRef.current));
    setConversationIndexedWorkspaceCount(0);
    setConversationWorkspaceIssues([]);
    setExpandedConversations({});
    setSelectedMessage(null);
    preserveSelectedMessageRef.current = null;
    activeLocalTrashOperationRef.current = null;
    activeLocalTrashResetRef.current = null;
    localTrashUndoMessageRef.current = null;
    localTrashSummaryRequestSequenceRef.current.clear();
    localTrashResetWorkspaceIdRef.current = null;
    setLocalTrashUndo(null);
    setLocalTrashStatus(null);
    setLocalTrashError(null);
    setLocalTrashMutationPending(false);
    setLocalTrashResetConfirmOpen(false);
    setSearch("");
    closeActivePopover();
    setSearchFilters(defaultAdvancedSearchFilters);
    setSortOrder("newest");
    setSearchLoading(null);
    setMessageLoadMoreRequest(null);
    setConversationLoadMoreRequest(null);
    setExportResults({});
    setExportingAttachmentId(null);
    resetPrintableStatus();
    resetSourceEmlViewer();
    setReaderMode("plain_text");
    setHtmlRender(null);
    setIsRenderingHtml(false);
    setIsImporting(false);
    setError(null);
    setNotice(null);
    setSetupCommand(null);
    setPendingOpenPlan(null);
    setPendingPreflightOpen(null);
    setPendingPlannedRepair(null);
    setPendingPlannedDelete(null);
    setPendingWorkspaceFlowOwnerId(null);
  }

  function addDeleteStatus(workspaceId: string, status: string) {
    setWorkspaceOperationStates((current) => {
      const operation = current[workspaceId] ?? emptyWorkspaceOperationState();
      return {
        ...current,
        [workspaceId]: {
          ...operation,
          deleteStatuses: [...operation.deleteStatuses, status],
        },
      };
    });
  }

  async function openPst() {
    if (localTrashMutationPending) return;
    setError(null);
    setNotice(null);
    setSetupCommand(null);
    setPendingPreflightOpen(null);
    setPendingOpenPlan(null);
    setPendingPlannedRepair(null);
    setPendingPlannedDelete(null);
    setPendingWorkspaceFlowOwnerId(null);
    setIsBusy(true);

    try {
      const pstPath = await invoke<string | null>("pick_pst_file");
      if (!pstPath) return;

      const plan = await invoke<PstOpenPlan>("plan_pst_open", {
        path: pstPath,
        workspaceLocationMode,
      });
      await handlePstOpenPlan(plan);
    } catch (err) {
      setError(getErrorMessage(err));
      setSetupCommand(getSetupCommand(err) ?? missingReadpstCommand);
    } finally {
      setIsBusy(false);
    }
  }

  async function openStandaloneMessage() {
    setError(null);
    setNotice(null);
    setPrintStatus(null);
    setSourceEmlStatus(null);
    setSourceEmlSaveResult(null);
    setPrintableSaveResult(null);
    setExportResults({});
    setIsBusy(true);

    try {
      const messageFile = await invoke<ExternalFileOpen | null>("pick_message_file");
      if (!messageFile) return;
      await openStandaloneMessageInline(messageFile.stableId);
    } catch (err) {
      setError(getErrorMessage(err));
      setSourceEmlStatus(null);
    } finally {
      setIsLoadingSourceEml(false);
      setIsBusy(false);
    }
  }

  async function openStandaloneMessageInline(stableId: string) {
    setIsLoadingSourceEml(true);
    setSourceEmlStatus("Opening standalone message.");
    try {
      const result = await invoke<SourceEmlView>("get_standalone_message_view", {
        stableId,
      });
      setSourceEmlView(result);
      setSourceEmlContext({ kind: "standalone", stableId });
      setSourceEmlMode("rendered");
      setSourceEmlOpen(true);
      setSourceEmlRawSearch("");
      setSourceEmlStatus(null);
    } finally {
      setIsLoadingSourceEml(false);
    }
  }

  async function handlePstOpenPlan(plan: PstOpenPlan, confirmNewImport = false) {
    setNotice(cacheLocationMessage(plan));

    const matchingExisting = plan.existingWorkspaces.find(
      (existing) => existing.workspacePath === plan.selectedWorkspacePath,
    );

    if (
      matchingExisting &&
      plan.existingWorkspaces.length === 1 &&
      matchingExisting.integrityClassification === "complete" &&
      matchingExisting.canOpen
    ) {
      await openPlannedPst(plan, matchingExisting, false, "open_existing");
    } else if (plan.existingWorkspaces.length > 0) {
      setPendingWorkspaceFlowOwnerId(activeWorkspaceIdRef.current);
      setPendingOpenPlan(plan);
    } else if (confirmNewImport) {
      setPendingWorkspaceFlowOwnerId(activeWorkspaceIdRef.current);
      setPendingPreflightOpen({
        plan,
        existingWorkspace: null,
        allowDuplicate: false,
        importAction: "import",
      });
    } else {
      await beginPlannedPstOpen(plan, null, false, "import");
    }
  }

  async function beginPlannedPstOpen(
    plan: PstOpenPlan,
    existingWorkspace: ExistingWorkspace | null,
    allowDuplicate: boolean,
    importAction: ImportAction = existingWorkspace ? "open_existing" : "import",
  ) {
    setError(null);
    setSetupCommand(null);
    setNotice(cacheLocationMessage(plan));

    if (importActionNeedsPreflight(importAction) && plan.preflight.warningRequired) {
      setPendingWorkspaceFlowOwnerId(activeWorkspaceIdRef.current);
      setPendingPreflightOpen({
        plan,
        existingWorkspace,
        allowDuplicate,
        importAction,
      });
      return;
    }

    await openPlannedPst(plan, existingWorkspace, allowDuplicate, importAction);
  }

  async function continuePendingPreflight() {
    if (!pendingPreflightOpen || !canContinueAfterPreflight(pendingPreflightOpen.plan.preflight)) {
      return;
    }

    const pending = pendingPreflightOpen;
    setPendingPreflightOpen(null);
    setPendingWorkspaceFlowOwnerId(null);
    await openPlannedPst(
      pending.plan,
      pending.existingWorkspace,
      pending.allowDuplicate,
      pending.importAction,
    );
  }

  async function chooseAppSupportForPendingPreflight() {
    if (!pendingPreflightOpen) return;

    const pending = pendingPreflightOpen;
    setError(null);
    setSetupCommand(null);
    setIsBusy(true);

    try {
      const appSupportPlan = await invoke<PstOpenPlan>("plan_pst_open", {
        path: pending.plan.pstPath,
        workspaceLocationMode: "app_support",
      });
      setWorkspaceLocationMode("app_support");
      setPendingPreflightOpen(null);
      setPendingWorkspaceFlowOwnerId(null);
      await handlePstOpenPlan(appSupportPlan);
    } catch (err) {
      setError(getErrorMessage(err));
      setSetupCommand(getSetupCommand(err) ?? missingReadpstCommand);
      finishExternalPstRequest(pending.plan.pstPath);
    } finally {
      setIsBusy(false);
    }
  }

  async function openPlannedPst(
    plan: PstOpenPlan,
    existingWorkspace: ExistingWorkspace | null,
    allowDuplicate: boolean,
    importAction: ImportAction = existingWorkspace ? "open_existing" : "import",
  ) {
    const activeWorkspaceChangeSequenceAtStart = activeWorkspaceChangeSequenceRef.current;
    setError(null);
    setSetupCommand(null);
    setIsBusy(true);
    const expectsProgress = importAction !== "open_existing";
    setForegroundImportAction(expectsProgress ? importAction : null);
    const operationId = createProgressOperationId("import");
    const targetWorkspaceId = existingWorkspace?.workspaceId ?? plan.workspaceId;
    clearWorkspaceOperationDismissTimer(targetWorkspaceId);
    operationWorkspaceIdRef.current = targetWorkspaceId;
    setForegroundOperationWorkspaceId(
      openWorkspaceIds.includes(targetWorkspaceId) ? null : targetWorkspaceId,
    );
    setIsImporting(expectsProgress);
    updateWorkspaceOperationState(targetWorkspaceId, {
      progress: null,
      progressOperationId: expectsProgress ? operationId : null,
      progressReceivedAtMonotonicMs: null,
      running: expectsProgress,
      notice: null,
      error: null,
      setupCommand: null,
      deleteResult: null,
      deleteStatuses: [],
      deleteConfirmOpen: false,
      ...(expectsProgress
        ? { verificationReport: null, verificationReportOpen: false }
        : {}),
    });

    try {
      const nextWorkspace = await invoke<WorkspaceSummary>("open_pst", {
        path: plan.pstPath,
        workspaceLocationMode: plan.selectedWorkspaceLocationMode,
        existingWorkspacePath: existingWorkspace?.workspacePath ?? null,
        allowDuplicate,
        importAction,
        operationId,
      });
      const activeWorkspaceChangedDuringOperation =
        activeWorkspaceChangeSequenceRef.current !== activeWorkspaceChangeSequenceAtStart;
      await refreshWorkspace(nextWorkspace, {
        onlyIfWorkspaceStillActive: activeWorkspaceChangedDuringOperation,
      });
      const currentWorkspaceId = activeWorkspaceIdRef.current;
      if (
        activeWorkspaceChangedDuringOperation &&
        currentWorkspaceId &&
        currentWorkspaceId !== nextWorkspace.id
      ) {
        restoringPlannedWorkspaceRef.current = true;
        setRestoringPlannedWorkspace(true);
        try {
          let restoreWorkspaceId = activeWorkspaceIdRef.current;
          let restoredStableWorkspace = false;
          for (let attempt = 0; attempt < 2; attempt += 1) {
            if (!restoreWorkspaceId || restoreWorkspaceId === nextWorkspace.id) {
              restoredStableWorkspace = true;
              break;
            }
            const restoreSequence = activeWorkspaceChangeSequenceRef.current;
            const restoredWorkspace = await invoke<WorkspaceSummary>("activate_workspace", {
              workspaceId: restoreWorkspaceId,
            });
            if (restoredWorkspace.id !== restoreWorkspaceId) {
              throw new Error(
                "The active workspace changed while repair results were being restored.",
              );
            }
            if (
              activeWorkspaceIdRef.current === restoreWorkspaceId &&
              activeWorkspaceChangeSequenceRef.current === restoreSequence
            ) {
              restoredStableWorkspace = true;
              break;
            }
            restoreWorkspaceId = activeWorkspaceIdRef.current;
          }
          if (!restoredStableWorkspace) {
            throw new Error(
              "The active workspace changed repeatedly while repair results were being restored.",
            );
          }
        } finally {
          restoringPlannedWorkspaceRef.current = false;
          setRestoringPlannedWorkspace(false);
        }
      }
      rememberRecentPst(nextWorkspace.pstPath);
      setPendingOpenPlan(null);
      setPendingPlannedRepair(null);
      setPendingPlannedDelete(null);
      setPendingWorkspaceFlowOwnerId(null);
      setReadpstStatus(await invoke<ReadpstStatus>("check_readpst"));
      if (nextWorkspace.id !== targetWorkspaceId) {
        clearWorkspaceOperationState(targetWorkspaceId);
      }
      setForegroundOperationWorkspaceId(null);
      completeWorkspaceOperation(
        nextWorkspace.id,
        importAction === "repair_index"
          ? "Repair Index completed. The verified replacement index is active."
          : nextWorkspace.reusedExisting
            ? "Opened existing workspace."
          : importAction === "resume_index"
            ? "Resume indexing completed."
            : nextWorkspace.integrityClassification === "complete"
              ? "Import completed and passed internal workspace verification."
              : "Import stopped before internal workspace verification completed.",
      );
    } catch (err) {
      const message = getErrorMessage(err);
      const operationSetupCommand = getSetupCommand(err);
      if (message.toLowerCase().includes("cancelled")) {
        completeWorkspaceOperation(
          targetWorkspaceId,
          importAction === "repair_index"
            ? "Repair Index cancelled. The previous active index remains available."
            : importAction === "resume_index"
              ? "Resume indexing cancelled."
              : "Import cancelled.",
        );
      } else if (operationSetupCommand) {
        clearWorkspaceOperationState(targetWorkspaceId);
        setForegroundOperationWorkspaceId(null);
        setError(message);
        setSetupCommand(operationSetupCommand);
      } else {
        failWorkspaceOperation(targetWorkspaceId, message);
      }
    } finally {
      if (operationWorkspaceIdRef.current === targetWorkspaceId) {
        operationWorkspaceIdRef.current = null;
      }
      setIsBusy(false);
      setIsImporting(false);
      setForegroundImportAction(null);
      finishExternalPstRequest(plan.pstPath);
    }
  }

  async function applyIntegrityWorkspaceSummary(
    operation: WorkspaceIntegrityOperationToken,
    nextWorkspace: WorkspaceSummary,
  ): Promise<boolean> {
    if (!workspaceIntegrityOperationIsCurrent(operation)) return false;
    if (nextWorkspace.id !== operation.workspaceId) {
      failWorkspaceIntegrityOperation(
        operation,
        "The workspace operation returned a mismatched workspace identity.",
      );
      return false;
    }
    const nextSession = await loadWorkspaceSession(nextWorkspace);
    if (!workspaceIntegrityOperationIsCurrent(operation)) return false;
    if (nextSession.workspace.id !== operation.workspaceId) {
      failWorkspaceIntegrityOperation(
        operation,
        "The refreshed workspace did not match the requested workspace.",
      );
      return false;
    }
    registerWorkspaceSession(nextSession);
    if (activeWorkspaceIdRef.current === operation.workspaceId) {
      applyActiveWorkspaceSession(nextSession, true);
    }
    return true;
  }

  async function verifyWorkspaceImport(
    workspaceId: string,
    returnFocus?: HTMLElement | null,
  ) {
    const operation = beginWorkspaceIntegrityOperation(
      workspaceId,
      "verify",
    );
    if (!operation) return;
    operationWorkspaceIdRef.current = workspaceId;

    try {
      const report = await invoke<VerificationReport>("verify_workspace_import", {
        workspaceId,
        operationId: operation.operationId,
        verificationLevel: "quick",
      });
      if (!workspaceIntegrityOperationIsCurrent(operation)) return;
      if (!isVerificationReportForOperation(report, operation)) {
        failWorkspaceIntegrityOperation(
          operation,
          "Verification returned a mismatched operation identity.",
        );
        return;
      }
      updateWorkspaceSummaryFromVerification(workspaceId, report);
      finishWorkspaceIntegrityOperation(operation, verificationHeadline(report), report);
      showVerificationReport(workspaceId, report, returnFocus);
    } catch (err) {
      failWorkspaceIntegrityOperation(operation, getErrorMessage(err));
    } finally {
      if (operationWorkspaceIdRef.current === workspaceId) {
        operationWorkspaceIdRef.current = null;
      }
    }
  }

  async function viewLastWorkspaceVerification(
    workspaceId: string,
    returnFocus?: HTMLElement | null,
  ) {
    const inMemoryReport = workspaceOperationStates[workspaceId]?.verificationReport ?? null;
    const operation = beginWorkspaceIntegrityOperation(
      workspaceId,
      "load_last_verification",
    );
    if (!operation) return;

    // Fail-closed verification results for malformed or future workspace
    // metadata are deliberately not written into that workspace. Keep the
    // current result viewable for this application session without claiming
    // that a persisted report exists.
    if (inMemoryReport) {
      if (!workspaceIntegrityOperationIsCurrent(operation)) return;
      if (inMemoryReport.workspaceId !== workspaceId) {
        failWorkspaceIntegrityOperation(
          operation,
          "The current verification report did not match this workspace.",
        );
        return;
      }
      finishWorkspaceIntegrityOperation(
        operation,
        "Loaded the current verification report.",
        inMemoryReport,
      );
      showVerificationReport(workspaceId, inMemoryReport, returnFocus);
      return;
    }

    try {
      const report = await invoke<VerificationReport | null>("get_last_workspace_verification", {
        workspaceId,
      });
      if (!workspaceIntegrityOperationIsCurrent(operation)) return;
      if (!report) {
        finishWorkspaceIntegrityOperation(operation, "No saved verification report is available.");
        return;
      }
      if (report.workspaceId !== workspaceId) {
        failWorkspaceIntegrityOperation(
          operation,
          "The saved verification report did not match this workspace.",
        );
        return;
      }
      finishWorkspaceIntegrityOperation(operation, "Loaded the last verification report.", report);
      showVerificationReport(workspaceId, report, returnFocus);
    } catch (err) {
      failWorkspaceIntegrityOperation(operation, getErrorMessage(err));
    }
  }

  async function resumeWorkspaceIndexing(workspaceId: string) {
    const operation = beginWorkspaceIntegrityOperation(
      workspaceId,
      "resume_indexing",
    );
    if (!operation) return;
    operationWorkspaceIdRef.current = workspaceId;

    try {
      const nextWorkspace = await invoke<WorkspaceSummary>("resume_workspace_indexing", {
        workspaceId,
        operationId: operation.operationId,
      });
      if (!workspaceIntegrityOperationIsCurrent(operation)) return;
      if (!(await applyIntegrityWorkspaceSummary(operation, nextWorkspace))) return;
      finishWorkspaceIntegrityOperation(
        operation,
        "Resume indexing completed. Existing canonical message IDs were preserved.",
      );
    } catch (err) {
      const message = getErrorMessage(err);
      if (message.toLocaleLowerCase().includes("cancelled")) {
        finishWorkspaceIntegrityOperation(operation, "Resume indexing cancelled.");
      } else {
        failWorkspaceIntegrityOperation(operation, message);
      }
    } finally {
      if (operationWorkspaceIdRef.current === workspaceId) {
        operationWorkspaceIdRef.current = null;
      }
    }
  }

  async function repairWorkspaceIndex(workspaceId: string) {
    const operation = beginWorkspaceIntegrityOperation(
      workspaceId,
      "repair_index",
    );
    if (!operation) return;
    operationWorkspaceIdRef.current = workspaceId;

    try {
      const nextWorkspace = await invoke<WorkspaceSummary>("repair_workspace_index", {
        workspaceId,
        operationId: operation.operationId,
      });
      if (!workspaceIntegrityOperationIsCurrent(operation)) return;
      if (!(await applyIntegrityWorkspaceSummary(operation, nextWorkspace))) return;
      finishWorkspaceIntegrityOperation(
        operation,
        "Repair Index completed. The verified replacement index is active.",
      );
    } catch (err) {
      const message = getErrorMessage(err);
      if (message.toLocaleLowerCase().includes("cancelled")) {
        finishWorkspaceIntegrityOperation(
          operation,
          "Repair Index cancelled. The previous active index remains available.",
        );
      } else {
        failWorkspaceIntegrityOperation(operation, message);
      }
    } finally {
      if (operationWorkspaceIdRef.current === workspaceId) {
        operationWorkspaceIdRef.current = null;
      }
    }
  }

  async function rebuildWorkspaceConversations(workspaceId: string) {
    const operation = beginWorkspaceIntegrityOperation(
      workspaceId,
      "rebuild_conversations",
    );
    if (!operation) return;
    operationWorkspaceIdRef.current = workspaceId;

    try {
      const nextWorkspace = await invoke<WorkspaceSummary>("rebuild_workspace_conversations", {
        workspaceId,
        operationId: operation.operationId,
      });
      if (!workspaceIntegrityOperationIsCurrent(operation)) return;
      if (!(await applyIntegrityWorkspaceSummary(operation, nextWorkspace))) return;
      finishWorkspaceIntegrityOperation(
        operation,
        "Conversation metadata was rebuilt without rewriting messages, folders, or attachments.",
      );
    } catch (err) {
      const message = getErrorMessage(err);
      if (message.toLocaleLowerCase().includes("cancelled")) {
        finishWorkspaceIntegrityOperation(operation, "Conversation rebuild cancelled.");
      } else {
        failWorkspaceIntegrityOperation(operation, message);
      }
    } finally {
      if (operationWorkspaceIdRef.current === workspaceId) {
        operationWorkspaceIdRef.current = null;
      }
    }
  }

  function requestRepairWorkspaceIndex(workspaceId: string, returnFocus: HTMLElement) {
    repairConfirmReturnFocusRef.current = returnFocus;
    updateWorkspaceOperationState(workspaceId, { repairConfirmOpen: true });
    window.requestAnimationFrame(() => repairConfirmCancelRef.current?.focus());
  }

  function requestPlannedWorkspaceRepair(
    plan: PstOpenPlan,
    existingWorkspace: ExistingWorkspace,
    returnFocus: HTMLElement,
  ) {
    repairConfirmReturnFocusRef.current = returnFocus;
    setPendingPlannedRepair({ plan, existingWorkspace });
    window.requestAnimationFrame(() => repairConfirmCancelRef.current?.focus());
  }

  function closeRepairWorkspaceConfirmation(workspaceId: string) {
    updateWorkspaceOperationState(workspaceId, { repairConfirmOpen: false });
    const returnFocus = repairConfirmReturnFocusRef.current;
    repairConfirmReturnFocusRef.current = null;
    window.requestAnimationFrame(() => {
      if (returnFocus?.isConnected) returnFocus.focus();
    });
  }

  function closePlannedWorkspaceRepairConfirmation() {
    setPendingPlannedRepair(null);
    const returnFocus = repairConfirmReturnFocusRef.current;
    repairConfirmReturnFocusRef.current = null;
    window.requestAnimationFrame(() => {
      if (returnFocus?.isConnected) returnFocus.focus();
    });
  }

  function confirmRepairWorkspaceIndex(workspaceId: string) {
    closeRepairWorkspaceConfirmation(workspaceId);
    void repairWorkspaceIndex(workspaceId);
  }

  function confirmPlannedWorkspaceRepair() {
    if (!visiblePendingPlannedRepair) return;
    const pending = visiblePendingPlannedRepair;
    setPendingPlannedRepair(null);
    repairConfirmReturnFocusRef.current = null;
    void openPlannedPst(
      pending.plan,
      pending.existingWorkspace,
      false,
      "repair_index",
    );
  }

  function closeVisibleRepairConfirmation() {
    if (visiblePendingPlannedRepair) {
      closePlannedWorkspaceRepairConfirmation();
    } else if (workspace && repairConfirmOpen) {
      closeRepairWorkspaceConfirmation(workspace.id);
    }
  }

  function confirmVisibleRepair() {
    if (visiblePendingPlannedRepair) {
      confirmPlannedWorkspaceRepair();
    } else if (workspace && repairConfirmOpen) {
      confirmRepairWorkspaceIndex(workspace.id);
    }
  }

  function handleRepairConfirmationKeyDown(event: ReactKeyboardEvent<HTMLElement>) {
    if (!workspace && !pendingPlannedRepair) return;
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeVisibleRepairConfirmation();
      return;
    }
    if (event.key !== "Tab") return;
    const dialog = repairConfirmDialogRef.current;
    if (!dialog) return;
    const focusable = Array.from(
      dialog.querySelectorAll<HTMLElement>("button:not(:disabled), [tabindex]:not([tabindex='-1'])"),
    );
    if (!focusable.length) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  async function reindexExistingEmls(targetWorkspaceId = workspace?.id) {
    if (!targetWorkspaceId) return;
    const operationId = createProgressOperationId("reindex");
    clearWorkspaceOperationDismissTimer(targetWorkspaceId);

    setError(null);
    setNotice(null);
    updateWorkspaceOperationState(targetWorkspaceId, {
      progress: null,
      progressOperationId: operationId,
      progressReceivedAtMonotonicMs: null,
      running: true,
      notice: null,
      error: null,
      setupCommand: null,
      deleteResult: null,
      deleteStatuses: [],
      deleteConfirmOpen: false,
      verificationReport: null,
      verificationReportOpen: false,
    });
    setHtmlRender(null);
    setExportResults({});
    resetPrintableStatus();
    resetSourceEmlViewer();
    setIsBusy(true);
    setIsImporting(true);
    operationWorkspaceIdRef.current = targetWorkspaceId;
    setForegroundOperationWorkspaceId(null);

    try {
      const nextWorkspace = await invoke<WorkspaceSummary>("reindex_existing_emls", {
        workspaceId: targetWorkspaceId,
        operationId,
      });
      await refreshWorkspace(nextWorkspace, {
        onlyIfWorkspaceStillActive: true,
      });
      const currentWorkspaceId = activeWorkspaceIdRef.current;
      if (currentWorkspaceId && currentWorkspaceId !== targetWorkspaceId) {
        await invoke<WorkspaceSummary>("activate_workspace", {
          workspaceId: currentWorkspaceId,
        });
      }
      completeWorkspaceOperation(
        targetWorkspaceId,
        "Reindexed existing EML files. HTML, attachment, and conversation metadata were refreshed.",
      );
    } catch (err) {
      const message = getErrorMessage(err);
      if (message.toLowerCase().includes("cancelled")) {
        completeWorkspaceOperation(
          targetWorkspaceId,
          "Reindex cancelled. Existing index was left in place.",
        );
      } else {
        failWorkspaceOperation(targetWorkspaceId, message);
      }
    } finally {
      if (operationWorkspaceIdRef.current === targetWorkspaceId) {
        operationWorkspaceIdRef.current = null;
      }
      setIsBusy(false);
      setIsImporting(false);
    }
  }

  async function refreshPendingOpenPlan(plan: PstOpenPlan) {
    const refreshed = await invoke<PstOpenPlan>("plan_pst_open", {
      path: plan.pstPath,
      workspaceLocationMode,
    });
    setPendingOpenPlan(refreshed);
    return refreshed;
  }

  async function openMessage(messageId: number, targetWorkspaceId = workspace?.id) {
    if (!targetWorkspaceId) return;

    preserveSelectedMessageRef.current = null;
    const messageListScrollTop = messageListRef.current?.scrollTop ?? null;
    try {
      if (workspace?.id !== targetWorkspaceId) {
        const nextWorkspace = await invoke<WorkspaceSummary>("activate_workspace", {
          workspaceId: targetWorkspaceId,
        });
        await refreshWorkspace(nextWorkspace, { resetMessageList: !isCrossPstSearch });
      }
      setExportResults({});
      resetPrintableStatus();
      resetSourceEmlViewer();
      setHtmlRender(null);
      setMessageDiagnostics(null);
      setMessageDiagnosticsError(null);
      const message = await invoke<MessageDetail>("get_message", {
        workspaceId: targetWorkspaceId,
        messageId,
      });
      const targetSession =
        openPstSessions.find((session) => session.workspace.id === targetWorkspaceId) ?? null;
      setSelectedMessage({
        ...message,
        workspaceId: targetWorkspaceId,
        pstDisplayName: targetSession ? pstDisplayName(targetSession.workspace) : message.pstDisplayName,
        workspacePath: targetSession?.workspace.workspacePath ?? message.workspacePath,
      });
      window.requestAnimationFrame(() => {
        if (messageListRef.current && messageListScrollTop != null) {
          messageListRef.current.scrollTop = messageListScrollTop;
        }
      });
    } catch (err) {
      showWorkspaceError(targetWorkspaceId, getErrorMessage(err));
    }
  }

  async function openInlineMessagePreview(messageId: number, targetWorkspaceId = workspace?.id) {
    if (!targetWorkspaceId) return;

    preserveSelectedMessageRef.current = null;
    const messageListScrollTop = messageListRef.current?.scrollTop ?? null;
    clearWorkspaceOperationMessages(targetWorkspaceId);
    setIsLoadingSourceEml(true);
    setSourceEmlStatus("Opening message preview.");

    try {
      if (workspace?.id !== targetWorkspaceId) {
        const nextWorkspace = await invoke<WorkspaceSummary>("activate_workspace", {
          workspaceId: targetWorkspaceId,
        });
        await refreshWorkspace(nextWorkspace, { resetMessageList: !isCrossPstSearch });
      }

      const [message, sourceView] = await Promise.all([
        invoke<MessageDetail>("get_message", {
          workspaceId: targetWorkspaceId,
          messageId,
        }),
        invoke<SourceEmlView>("get_source_eml_view", {
          workspaceId: targetWorkspaceId,
          messageId,
        }),
      ]);

      setExportResults({});
      resetPrintableStatus();
      setHtmlRender(null);
      setMessageDiagnostics(null);
      setMessageDiagnosticsError(null);
      const targetSession =
        openPstSessions.find((session) => session.workspace.id === targetWorkspaceId) ?? null;
      setSelectedMessage({
        ...message,
        workspaceId: targetWorkspaceId,
        pstDisplayName: targetSession ? pstDisplayName(targetSession.workspace) : message.pstDisplayName,
        workspacePath: targetSession?.workspace.workspacePath ?? message.workspacePath,
      });
      setSourceEmlView(sourceView);
      setSourceEmlContext({ kind: "workspace", workspaceId: targetWorkspaceId, messageId });
      setSourceEmlMode("rendered");
      setSourceEmlOpen(true);
      setSourceEmlStatus(null);
      setSourceEmlSaveResult(null);
      setSourceEmlRawSearch("");
      window.requestAnimationFrame(() => {
        if (messageListRef.current && messageListScrollTop != null) {
          messageListRef.current.scrollTop = messageListScrollTop;
        }
      });
    } catch (err) {
      showWorkspaceError(targetWorkspaceId, getErrorMessage(err));
      setSourceEmlStatus(null);
    } finally {
      setIsLoadingSourceEml(false);
    }
  }

  function localTrashSummaryForWorkspace(workspaceId: string): LocalTrashSummary | null {
    return (
      openPstSessionsRef.current.find(
        (session) => session.workspace.id === workspaceId,
      )?.localTrashSummary ?? null
    );
  }

  function replaceLocalTrashWorkspaceState(
    summary: LocalTrashSummary,
    refreshedFolders: Folder[] | null,
  ) {
    const nextSessions = openPstSessionsRef.current.map((session) =>
      session.workspace.id === summary.workspaceId
        ? {
            ...session,
            localTrashSummary: summary,
            folders: refreshedFolders ?? session.folders,
          }
        : session,
    );
    openPstSessionsRef.current = nextSessions;
    setOpenPstSessions(nextSessions);
    if (activeWorkspaceIdRef.current === summary.workspaceId && refreshedFolders) {
      setFolders(refreshedFolders);
    }
  }

  async function refreshLocalTrashSummary(workspaceId: string) {
    const requestSequence =
      (localTrashSummaryRequestSequenceRef.current.get(workspaceId) ?? 0) + 1;
    localTrashSummaryRequestSequenceRef.current.set(workspaceId, requestSequence);
    const [summary, refreshedFolders] = await Promise.all([
      invoke<LocalTrashSummary>("read_local_trash_summary", { workspaceId }).catch(() =>
        unavailableLocalTrashSummary(workspaceId),
      ),
      invoke<Folder[]>("list_folders", { workspaceId }).catch(() => null),
    ]);
    if (localTrashSummaryRequestSequenceRef.current.get(workspaceId) === requestSequence) {
      replaceLocalTrashWorkspaceState(summary, refreshedFolders);
    }
    return summary;
  }

  function reconcileLocalTrashQueryHealth(states: LocalTrashQueryHealth[]) {
    const reconciledWorkspaceIds = new Set<string>();
    for (const health of states) {
      if (
        reconciledWorkspaceIds.has(health.workspaceId) ||
        !openPstSessionsRef.current.some(
          (session) => session.workspace.id === health.workspaceId,
        )
      ) continue;
      reconciledWorkspaceIds.add(health.workspaceId);
      const summary = localTrashSummaryForWorkspace(health.workspaceId);
      const alreadyCurrent = Boolean(
        summary &&
          summary.available === health.available &&
          summary.canReset === health.canReset &&
          summary.sidecarPresent === health.sidecarPresent &&
          summary.schemaVersion === health.schemaVersion &&
          summary.stateRevision === health.stateRevision &&
          summary.fingerprintMatch === health.fingerprintMatch &&
          summary.lastErrorCode === health.lastErrorCode,
      );
      if (alreadyCurrent) continue;
      if (!health.available) {
        replaceLocalTrashWorkspaceState(
          {
            workspaceId: health.workspaceId,
            available: false,
            canReset: health.canReset,
            sidecarPresent: health.sidecarPresent,
            trashedMessageCount: 0,
            orphanedStateCount: 0,
            schemaVersion: health.schemaVersion,
            stateRevision: health.stateRevision,
            lastChangedAt: null,
            fingerprintMatch: health.fingerprintMatch,
            lastErrorCode: health.lastErrorCode ?? "LOCAL_TRASH_UNAVAILABLE",
          },
          null,
        );
        setLocalTrashUndo((current) =>
          current?.workspaceId === health.workspaceId ? null : current,
        );
        if (localTrashUndoMessageRef.current?.workspaceId === health.workspaceId) {
          localTrashUndoMessageRef.current = null;
        }
      }
      void refreshLocalTrashSummary(health.workspaceId);
    }
  }

  async function refreshLocalTrashPreviewState(workspaceId: string, messageId: number) {
    try {
      const refreshed = await invoke<MessageDetail>("get_message", {
        workspaceId,
        messageId,
      });
      setSelectedMessage((current) =>
        current &&
        current.id === messageId &&
        (current.workspaceId ?? workspace?.id) === workspaceId
          ? {
              ...current,
              inLocalTrash: refreshed.inLocalTrash,
              localTrashAvailable: refreshed.localTrashAvailable,
              localTrashErrorCode: refreshed.localTrashErrorCode,
            }
          : current,
      );
      setSourceEmlView((current) =>
        current &&
        current.messageId === messageId &&
        sourceEmlContext?.kind === "workspace" &&
        sourceEmlContext.workspaceId === workspaceId
          ? {
              ...current,
              inLocalTrash: refreshed.inLocalTrash,
              localTrashAvailable: refreshed.localTrashAvailable,
              localTrashErrorCode: refreshed.localTrashErrorCode,
            }
          : current,
      );
    } catch {
      setSelectedMessage((current) =>
        current &&
        current.id === messageId &&
        (current.workspaceId ?? workspace?.id) === workspaceId
          ? {
              ...current,
              localTrashAvailable: false,
              localTrashErrorCode: "LOCAL_TRASH_UNAVAILABLE",
            }
          : current,
      );
    }
  }

  function refreshMessagesAfterLocalTrashChange() {
    forceNextSearchApplication();
    setActiveSessionGeneration((current) => current + 1);
  }

  async function changeSelectedMessageLocalTrash(nextTrashed: boolean) {
    if (
      !selectedMessage ||
      localTrashMutationPending ||
      isBusy ||
      isImporting ||
      activeIntegrityOperationRunning
    ) return;
    const targetWorkspaceId = selectedMessage.workspaceId ?? workspace?.id;
    if (!targetWorkspaceId) return;
    const summary = localTrashSummaryForWorkspace(targetWorkspaceId);
    if (
      !summary?.available ||
      !summary.fingerprintMatch ||
      !selectedMessage.localTrashAvailable
    ) {
      setLocalTrashError(
        "Local Trash is unavailable for this workspace. The message remains visible in normal views.",
      );
      return;
    }

    localTrashOperationSequenceRef.current += 1;
    const token = createLocalTrashOperationToken(
      targetWorkspaceId,
      summary.stateRevision,
      localTrashOperationSequenceRef.current,
    );
    if (!token) return;
    activeLocalTrashOperationRef.current = token;
    setLocalTrashMutationPending(true);
    setLocalTrashError(null);
    setLocalTrashStatus(nextTrashed ? "Moving message to Local Trash." : "Restoring message.");

    try {
      const response = nextTrashed
        ? await invoke<LocalTrashMutationResult>("move_message_to_local_trash", {
            workspaceId: targetWorkspaceId,
            messageId: selectedMessage.id,
            expectedRevision: token.expectedRevision,
          })
        : await invoke<LocalTrashMutationResult>("restore_message_from_local_trash", {
            workspaceId: targetWorkspaceId,
            messageId: selectedMessage.id,
            expectedRevision: token.expectedRevision,
          });
      const currentRevision =
        localTrashSummaryForWorkspace(targetWorkspaceId)?.stateRevision ?? -1;
      if (
        !isLocalTrashMutationResponseCurrent({
          requestToken: token,
          activeToken: activeLocalTrashOperationRef.current,
          currentWorkspaceRevision: currentRevision,
          response,
        })
      ) {
        setLocalTrashStatus(null);
        setLocalTrashError(
          "Local Trash state changed while this request was running. The latest state was reloaded.",
        );
        await refreshLocalTrashSummary(targetWorkspaceId);
        refreshMessagesAfterLocalTrashChange();
        return;
      }

      const selectedKey = `${targetWorkspaceId}:${selectedMessage.id}`;
      preserveSelectedMessageRef.current = selectedKey;
      if (response.changedCount > 0) {
        setSelectedMessage((current) =>
          current &&
          current.id === selectedMessage.id &&
          (current.workspaceId ?? workspace?.id) === targetWorkspaceId
            ? { ...current, inLocalTrash: nextTrashed }
            : current,
        );
        setSourceEmlView((current) =>
          current?.messageId === selectedMessage.id &&
          sourceEmlContext?.kind === "workspace" &&
          sourceEmlContext.workspaceId === targetWorkspaceId
            ? { ...current, inLocalTrash: nextTrashed }
            : current,
        );
      }
      setLocalTrashUndo((current) =>
        reduceLocalTrashUndoState(current, {
          type: "mutation_succeeded",
          response,
        }),
      );
      if (response.result === "moved" && response.changedCount > 0) {
        localTrashUndoMessageRef.current = {
          workspaceId: targetWorkspaceId,
          messageId: selectedMessage.id,
        };
      } else if (response.changedCount > 0) {
        localTrashUndoMessageRef.current = null;
      }
      setLocalTrashStatus(
        response.result === "moved"
          ? "Moved to Local Trash."
          : response.result === "restored"
            ? "Message restored to normal views."
            : nextTrashed
              ? "Message is already in Local Trash."
              : "Message is already active.",
      );
      await refreshLocalTrashSummary(targetWorkspaceId);
      refreshMessagesAfterLocalTrashChange();
    } catch (err) {
      const indeterminate =
        getBackendErrorCode(err) === "LOCAL_TRASH_OPERATION_INDETERMINATE";
      if (indeterminate) {
        setLocalTrashUndo((current) =>
          current?.workspaceId === targetWorkspaceId ? null : current,
        );
        if (localTrashUndoMessageRef.current?.workspaceId === targetWorkspaceId) {
          localTrashUndoMessageRef.current = null;
        }
      }
      setLocalTrashStatus(null);
      setLocalTrashError(
        indeterminate
          ? "Local Trash operation needs reinspection. The latest state was reloaded; check the message status before trying again."
          : `Local Trash action could not be completed. ${errorSummary(getErrorMessage(err))}`,
      );
      await refreshLocalTrashSummary(targetWorkspaceId);
      if (indeterminate) {
        await refreshLocalTrashPreviewState(targetWorkspaceId, selectedMessage.id);
      }
      refreshMessagesAfterLocalTrashChange();
    } finally {
      if (activeLocalTrashOperationRef.current === token) {
        activeLocalTrashOperationRef.current = null;
        setLocalTrashMutationPending(false);
      }
    }
  }

  async function undoMostRecentLocalTrashMove() {
    if (
      !localTrashUndo ||
      localTrashMutationPending ||
      isBusy ||
      isImporting ||
      activeIntegrityOperationRunning
    ) return;
    const summary = localTrashSummaryForWorkspace(localTrashUndo.workspaceId);
    if (
      !summary ||
      !summary.available ||
      !summary.fingerprintMatch ||
      !isLocalTrashUndoCurrent(
        localTrashUndo,
        localTrashUndo.workspaceId,
        summary.stateRevision,
      )
    ) {
      setLocalTrashUndo(null);
      setLocalTrashError("Undo is no longer available because Local Trash state changed.");
      return;
    }

    localTrashOperationSequenceRef.current += 1;
    const token = createLocalTrashOperationToken(
      localTrashUndo.workspaceId,
      localTrashUndo.expectedRevision,
      localTrashOperationSequenceRef.current,
    );
    if (!token) return;
    activeLocalTrashOperationRef.current = token;
    setLocalTrashMutationPending(true);
    setLocalTrashError(null);
    setLocalTrashStatus("Undoing the most recent move.");
    try {
      const response = await invoke<LocalTrashMutationResult>("undo_local_trash_operation", {
        workspaceId: localTrashUndo.workspaceId,
        operationId: localTrashUndo.operationId,
        expectedRevision: localTrashUndo.expectedRevision,
      });
      const currentRevision =
        localTrashSummaryForWorkspace(localTrashUndo.workspaceId)?.stateRevision ?? -1;
      if (
        !isLocalTrashMutationResponseCurrent({
          requestToken: token,
          activeToken: activeLocalTrashOperationRef.current,
          currentWorkspaceRevision: currentRevision,
          response,
        })
      ) {
        setLocalTrashStatus(null);
        setLocalTrashError(
          "Undo was not applied because Local Trash state changed. The latest state was reloaded.",
        );
        await refreshLocalTrashSummary(localTrashUndo.workspaceId);
        refreshMessagesAfterLocalTrashChange();
        return;
      }
      const undoMessage = localTrashUndoMessageRef.current;
      setSelectedMessage((current) =>
        current &&
        undoMessage &&
        current.id === undoMessage.messageId &&
        (current.workspaceId ?? workspace?.id) === undoMessage.workspaceId
          ? { ...current, inLocalTrash: false }
          : current,
      );
      setSourceEmlView((current) =>
        current &&
        undoMessage &&
        sourceEmlContext?.kind === "workspace" &&
        sourceEmlContext.workspaceId === undoMessage.workspaceId &&
        current.messageId === undoMessage.messageId
          ? { ...current, inLocalTrash: false }
          : current,
      );
      setLocalTrashUndo((current) =>
        reduceLocalTrashUndoState(current, { type: "undo_succeeded", response }),
      );
      setLocalTrashStatus("Move undone. The message is visible in normal views.");
      localTrashUndoMessageRef.current = null;
      await refreshLocalTrashSummary(localTrashUndo.workspaceId);
      refreshMessagesAfterLocalTrashChange();
    } catch (err) {
      const indeterminate =
        getBackendErrorCode(err) === "LOCAL_TRASH_OPERATION_INDETERMINATE";
      const undoMessage = localTrashUndoMessageRef.current;
      if (indeterminate) {
        setLocalTrashUndo((current) =>
          current?.workspaceId === localTrashUndo.workspaceId ? null : current,
        );
        if (localTrashUndoMessageRef.current?.workspaceId === localTrashUndo.workspaceId) {
          localTrashUndoMessageRef.current = null;
        }
      }
      setLocalTrashStatus(null);
      setLocalTrashError(
        indeterminate
          ? "Undo outcome needs reinspection. The latest state was reloaded; check the message status before trying again."
          : `Undo failed; the message remains in Local Trash. ${errorSummary(getErrorMessage(err))}`,
      );
      await refreshLocalTrashSummary(localTrashUndo.workspaceId);
      if (indeterminate && undoMessage) {
        await refreshLocalTrashPreviewState(undoMessage.workspaceId, undoMessage.messageId);
      }
      refreshMessagesAfterLocalTrashChange();
    } finally {
      if (activeLocalTrashOperationRef.current === token) {
        activeLocalTrashOperationRef.current = null;
        setLocalTrashMutationPending(false);
      }
    }
  }

  function requestLocalTrashReset(returnFocus: HTMLElement) {
    if (
      !workspace ||
      isBusy ||
      isImporting ||
      activeIntegrityOperationRunning ||
      !canResetLocalTrashState(activeLocalTrashSummary)
    ) return;
    localTrashResetWorkspaceIdRef.current = workspace.id;
    localTrashResetReturnFocusRef.current = returnFocus;
    setLocalTrashResetConfirmOpen(true);
    window.requestAnimationFrame(() => localTrashResetCancelRef.current?.focus());
  }

  function closeLocalTrashResetConfirmation() {
    setLocalTrashResetConfirmOpen(false);
    localTrashResetWorkspaceIdRef.current = null;
    const returnFocus = localTrashResetReturnFocusRef.current;
    localTrashResetReturnFocusRef.current = null;
    window.requestAnimationFrame(() => {
      if (returnFocus?.isConnected) returnFocus.focus();
    });
  }

  async function confirmLocalTrashReset() {
    if (
      !workspace ||
      localTrashResetWorkspaceIdRef.current !== workspace.id ||
      isBusy ||
      isImporting ||
      activeIntegrityOperationRunning ||
      !canResetLocalTrashState(activeLocalTrashSummary)
    ) {
      closeLocalTrashResetConfirmation();
      return;
    }
    const targetWorkspaceId = workspace.id;
    localTrashOperationSequenceRef.current += 1;
    const token = createLocalTrashResetToken(
      targetWorkspaceId,
      localTrashOperationSequenceRef.current,
    );
    if (!token) {
      closeLocalTrashResetConfirmation();
      return;
    }
    activeLocalTrashResetRef.current = token;
    setLocalTrashResetConfirmOpen(false);
    localTrashResetReturnFocusRef.current = null;
    localTrashResetWorkspaceIdRef.current = null;
    setLocalTrashMutationPending(true);
    setLocalTrashError(null);
    setLocalTrashStatus("Resetting unhealthy Local Trash state.");
    try {
      const result = await invoke<LocalTrashResetResult>("reset_local_trash_state", {
        workspaceId: targetWorkspaceId,
      });
      if (
        !isLocalTrashResetResponseCurrent(
          token,
          activeLocalTrashResetRef.current,
          activeWorkspaceIdRef.current,
          result,
        )
      ) {
        setLocalTrashUndo((current) =>
          result.workspaceId === targetWorkspaceId &&
          current?.workspaceId === targetWorkspaceId
            ? null
            : current,
        );
        if (localTrashUndoMessageRef.current?.workspaceId === targetWorkspaceId) {
          localTrashUndoMessageRef.current = null;
        }
        await refreshLocalTrashSummary(targetWorkspaceId);
        return;
      }
      setLocalTrashUndo((current) =>
        current?.workspaceId === targetWorkspaceId ? null : current,
      );
      if (localTrashUndoMessageRef.current?.workspaceId === targetWorkspaceId) {
        localTrashUndoMessageRef.current = null;
      }
      preserveSelectedMessageRef.current = null;
      setMessageVisibility("active");
      setLocalTrashAllOpenWorkspaceId(null);
      setListMode(previousActiveListModeRef.current);
      const resetSessions = openPstSessionsRef.current.map((session) =>
        session.workspace.id === targetWorkspaceId
          ? { ...session, folderSelection: session.canonicalFolderSelection }
          : session,
      );
      openPstSessionsRef.current = resetSessions;
      setOpenPstSessions(resetSessions);
      setSelectedMessage((current) =>
        current && (current.workspaceId ?? targetWorkspaceId) === targetWorkspaceId
          ? { ...current, inLocalTrash: false, localTrashAvailable: true, localTrashErrorCode: null }
          : current,
      );
      setSourceEmlView((current) =>
        current &&
        sourceEmlContext?.kind === "workspace" &&
        sourceEmlContext.workspaceId === targetWorkspaceId
          ? {
              ...current,
              inLocalTrash: false,
              localTrashAvailable: true,
              localTrashErrorCode: null,
            }
          : current,
      );
      setLocalTrashStatus(
        result.reset
          ? "Local Trash state reset. Messages, the canonical index, and source PST were unchanged."
          : "Local Trash state did not require a reset.",
      );
      await refreshLocalTrashSummary(targetWorkspaceId);
      refreshMessagesAfterLocalTrashChange();
    } catch (err) {
      const indeterminate =
        getBackendErrorCode(err) === "LOCAL_TRASH_RESET_INDETERMINATE";
      const targetStillActive =
        activeLocalTrashResetRef.current === token &&
        activeWorkspaceIdRef.current === targetWorkspaceId;
      if (indeterminate) {
        setLocalTrashUndo((current) =>
          current?.workspaceId === targetWorkspaceId ? null : current,
        );
        if (localTrashUndoMessageRef.current?.workspaceId === targetWorkspaceId) {
          localTrashUndoMessageRef.current = null;
        }
      }
      if (targetStillActive) {
        setLocalTrashStatus(null);
        setLocalTrashError(
          indeterminate
            ? "Local Trash reset needs reinspection. The latest state was reloaded; check Local Trash details before trying again."
            : `Local Trash state could not be reset. ${errorSummary(getErrorMessage(err))}`,
        );
      }
      await refreshLocalTrashSummary(targetWorkspaceId);
      if (
        indeterminate &&
        selectedMessage &&
        (selectedMessage.workspaceId ?? workspace.id) === targetWorkspaceId
      ) {
        await refreshLocalTrashPreviewState(targetWorkspaceId, selectedMessage.id);
      }
      if (
        targetStillActive &&
        activeLocalTrashResetRef.current === token &&
        activeWorkspaceIdRef.current === targetWorkspaceId
      ) {
        refreshMessagesAfterLocalTrashChange();
      }
    } finally {
      if (activeLocalTrashResetRef.current === token) {
        activeLocalTrashResetRef.current = null;
        setLocalTrashMutationPending(false);
      }
    }
  }

  function handleLocalTrashResetKeyDown(event: ReactKeyboardEvent<HTMLElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeLocalTrashResetConfirmation();
      return;
    }
    if (event.key !== "Tab") return;
    const dialog = localTrashResetDialogRef.current;
    if (!dialog) return;
    const focusable = Array.from(
      dialog.querySelectorAll<HTMLElement>(
        "button:not(:disabled), [tabindex]:not([tabindex='-1'])",
      ),
    );
    if (!focusable.length) return;
    const first = focusable[0];
    const last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  async function openStandaloneMessagePreviewWindow(
    file: ExternalFileOpen,
  ): Promise<boolean> {
    try {
      const request: PreviewWindowRequest = {
        kind: "standalone",
        stableId: file.stableId,
      };
      await invoke<PreviewWindowOpenResult>("open_message_preview_window", {
        request,
      });
      return true;
    } catch {
      console.error("Could not open standalone message pop-out.");
      setError("Could not open a pop-out preview. Opened the message in the app instead.");
      try {
        await openStandaloneMessageInline(file.stableId);
      } catch (fallbackError) {
        setError(`Could not open the selected message: ${getErrorMessage(fallbackError)}`);
        return false;
      }
      return true;
    }
  }

  async function openMessagePreviewWindow(message: MessageListItem, targetWorkspaceId = workspace?.id) {
    if (!targetWorkspaceId) return;

    clearWorkspaceOperationMessages(targetWorkspaceId);

    try {
      const request: PreviewWindowRequest = {
        kind: "workspace",
        workspaceId: targetWorkspaceId,
        messageId: message.id,
      };
      await invoke<PreviewWindowOpenResult>("open_message_preview_window", {
        request,
      });
    } catch {
      console.error("Could not open pop-out message preview.");
      showWorkspaceError(
        targetWorkspaceId,
        "Could not open a pop-out preview. Opened the message in the app instead.",
      );
      await openInlineMessagePreview(message.id, targetWorkspaceId);
    }
  }

  async function loadMessageDiagnostics() {
    if (!workspace || !selectedMessage) return;
    if (messageDiagnostics?.messageId === selectedMessage.id) return;
    const messageWorkspaceId = selectedMessage.workspaceId ?? workspace.id;

    setMessageDiagnosticsError(null);
    setIsLoadingMessageDiagnostics(true);
    try {
      const result = await invoke<MessageDiagnostics>("get_message_diagnostics", {
        workspaceId: messageWorkspaceId,
        messageId: selectedMessage.id,
      });
      setMessageDiagnostics(result);
    } catch (err) {
      setMessageDiagnosticsError(getErrorMessage(err));
    } finally {
      setIsLoadingMessageDiagnostics(false);
    }
  }

  async function exportAttachment(attachmentId: number) {
    if (sourceEmlOpen && sourceEmlContext) {
      const sourceWorkspaceId =
        sourceEmlContext.kind === "workspace" ? sourceEmlContext.workspaceId : null;
      if (sourceWorkspaceId) clearWorkspaceOperationMessages(sourceWorkspaceId);
      else setError(null);
      setExportingAttachmentId(attachmentId);
      try {
        const result =
          sourceEmlContext.kind === "workspace"
            ? await invoke<ExportAttachmentResult>("export_attachment", {
                workspaceId: sourceEmlContext.workspaceId,
                messageId: sourceEmlContext.messageId,
                attachmentId,
              })
            : await invoke<ExportAttachmentResult>("export_standalone_message_attachment", {
                stableId: sourceEmlContext.stableId,
                attachmentId,
              });
        setExportResults((current) => ({ ...current, [attachmentId]: result }));
        if (result.exported) {
          if (sourceWorkspaceId) {
            showWorkspaceNotice(sourceWorkspaceId, `Exported attachment to ${result.outputPath}`);
          } else {
            setNotice(`Exported attachment to ${result.outputPath}`);
          }
        } else {
          const message = result.error ?? "Attachment export failed.";
          if (sourceWorkspaceId) showWorkspaceError(sourceWorkspaceId, message);
          else setError(message);
        }
      } catch (err) {
        if (sourceWorkspaceId) showWorkspaceError(sourceWorkspaceId, getErrorMessage(err));
        else setError(getErrorMessage(err));
      } finally {
        setExportingAttachmentId(null);
      }
      return;
    }

    if (!workspace || !selectedMessage) return;
    const messageWorkspaceId = selectedMessage.workspaceId ?? workspace.id;

    clearWorkspaceOperationMessages(messageWorkspaceId);
    setExportingAttachmentId(attachmentId);
    try {
      const result = await invoke<ExportAttachmentResult>("export_attachment", {
        workspaceId: messageWorkspaceId,
        messageId: selectedMessage.id,
        attachmentId,
      });
      setExportResults((current) => ({ ...current, [attachmentId]: result }));
      if (result.exported) {
        showWorkspaceNotice(messageWorkspaceId, `Exported attachment to ${result.outputPath}`);
      } else {
        showWorkspaceError(messageWorkspaceId, result.error ?? "Attachment export failed.");
      }
    } catch (err) {
      showWorkspaceError(messageWorkspaceId, getErrorMessage(err));
    } finally {
      setExportingAttachmentId(null);
    }
  }

  async function openAttachment(attachmentId: number) {
    if (sourceEmlOpen && sourceEmlContext) {
      const sourceWorkspaceId =
        sourceEmlContext.kind === "workspace" ? sourceEmlContext.workspaceId : null;
      if (sourceWorkspaceId) clearWorkspaceOperationMessages(sourceWorkspaceId);
      else setError(null);
      setOpeningAttachmentId(attachmentId);
      try {
        const result =
          sourceEmlContext.kind === "workspace"
            ? await invoke<ExportAttachmentResult>("open_attachment", {
                workspaceId: sourceEmlContext.workspaceId,
                messageId: sourceEmlContext.messageId,
                attachmentId,
              })
            : await invoke<ExportAttachmentResult>("open_standalone_message_attachment", {
                stableId: sourceEmlContext.stableId,
                attachmentId,
              });
        setExportResults((current) => ({ ...current, [attachmentId]: result }));
        if (result.exported) {
          if (sourceWorkspaceId) {
            showWorkspaceNotice(
              sourceWorkspaceId,
              `Opened exported attachment copy from ${result.outputPath}`,
            );
          } else {
            setNotice(`Opened exported attachment copy from ${result.outputPath}`);
          }
        } else {
          const message = result.error ?? "Attachment open failed.";
          if (sourceWorkspaceId) showWorkspaceError(sourceWorkspaceId, message);
          else setError(message);
        }
      } catch (err) {
        if (sourceWorkspaceId) showWorkspaceError(sourceWorkspaceId, getErrorMessage(err));
        else setError(getErrorMessage(err));
      } finally {
        setOpeningAttachmentId(null);
      }
      return;
    }

    if (!workspace || !selectedMessage) return;
    const messageWorkspaceId = selectedMessage.workspaceId ?? workspace.id;

    clearWorkspaceOperationMessages(messageWorkspaceId);
    setOpeningAttachmentId(attachmentId);
    try {
      const result = await invoke<ExportAttachmentResult>("open_attachment", {
        workspaceId: messageWorkspaceId,
        messageId: selectedMessage.id,
        attachmentId,
      });
      setExportResults((current) => ({ ...current, [attachmentId]: result }));
      if (result.exported) {
        showWorkspaceNotice(
          messageWorkspaceId,
          `Opened exported attachment copy from ${result.outputPath}`,
        );
      } else {
        showWorkspaceError(messageWorkspaceId, result.error ?? "Attachment open failed.");
      }
    } catch (err) {
      showWorkspaceError(messageWorkspaceId, getErrorMessage(err));
    } finally {
      setOpeningAttachmentId(null);
    }
  }

  async function openSourceEml() {
    if (!workspace || !selectedMessage) return;
    const messageWorkspaceId = selectedMessage.workspaceId ?? workspace.id;

    clearWorkspaceOperationMessages(messageWorkspaceId);
    setSourceEmlStatus("Opening source EML.");
    setSourceEmlSaveResult(null);
    setIsLoadingSourceEml(true);

    try {
      const result = await invoke<SourceEmlView>("get_source_eml_view", {
        workspaceId: messageWorkspaceId,
        messageId: selectedMessage.id,
      });
      setSourceEmlView(result);
      setSourceEmlContext({
        kind: "workspace",
        workspaceId: messageWorkspaceId,
        messageId: selectedMessage.id,
      });
      setSourceEmlOpen(true);
      setSourceEmlStatus(null);
    } catch (err) {
      showWorkspaceError(messageWorkspaceId, getErrorMessage(err));
      setSourceEmlStatus(null);
    } finally {
      setIsLoadingSourceEml(false);
    }
  }

  async function saveSourceEmlAs() {
    if (!sourceEmlContext) return;

    setError(null);
    setSourceEmlStatus("Choose where to save the source message.");
    setSourceEmlSaveResult(null);
    setPrintableSaveResult(null);
    setPrintStatus(null);
    try {
      const result =
        sourceEmlContext.kind === "workspace"
          ? await invoke<ExportOriginalEmlResult>("save_source_eml_as", {
              workspaceId: sourceEmlContext.workspaceId,
              messageId: sourceEmlContext.messageId,
            })
          : await invoke<ExportOriginalEmlResult>("save_standalone_source_message_as", {
              stableId: sourceEmlContext.stableId,
            });
      setSourceEmlSaveResult(result);
      if (result.exported) {
        setSourceEmlStatus(`Saved source message to ${result.outputPath}`);
      } else if (result.error) {
        setError(result.error);
        setSourceEmlStatus(null);
      } else {
        setSourceEmlStatus("Save Source Message As... cancelled.");
      }
    } catch (err) {
      setError(getErrorMessage(err));
      setSourceEmlStatus(null);
    }
  }

  function buildSourceEmlPrintablePreview(): PrintablePreview | null {
    if (!sourceEmlView) return null;

    let bodyHtml = "";
    let bodyModeLabel = "Rendered View";
    let note: string | null = null;

    if (sourceEmlMode === "raw_source") {
      bodyHtml = plainTextPrintBody(sourceEmlView.rawSource);
      bodyModeLabel = "Raw Source";
      note = rawSourcePrintNote(sourceEmlView);
    } else if (sourceEmlMode === "plain_text") {
      bodyHtml = plainTextPrintBody(formatBodyForDisplay(sourceEmlView.bodyText));
      bodyModeLabel = "Plain Text";
    } else if (sourceEmlView.sanitizedHtml.trim()) {
      bodyHtml = `<div class="html-body">${makeSanitizedHtmlPrintable(sourceEmlView.sanitizedHtml)}</div>`;
      bodyModeLabel = "Rendered View";
      if (sourceEmlView.remoteImagesBlocked) {
        note = "Remote images were blocked for this print.";
      }
    } else {
      bodyHtml = plainTextPrintBody(formatBodyForDisplay(sourceEmlView.bodyText));
      bodyModeLabel = "Rendered View fallback";
      note = "No sanitized HTML body was available. Printed converted plain text.";
    }

    const printDocument = buildPrintDocument(
      {
        subject: sourceEmlView.subject,
        sender: sourceEmlView.sender,
        recipients: sourceEmlView.recipients,
        date: sourceEmlView.date,
        attachmentCount: sourceEmlView.attachments.length,
        attachments: sourceEmlView.attachments,
        calendar: sourceEmlView.calendar,
      },
      bodyHtml,
      bodyModeLabel,
      note,
    );

    return {
      title: sourceEmlView.subject || "(No subject)",
      modeLabel: bodyModeLabel,
      defaultFilename: printableDefaultFilename(
        sourceEmlView.subject || "message",
        sourceEmlView.date,
        bodyModeLabel,
        sourceEmlView.messageId,
      ),
      html: printDocument,
    };
  }

  async function savePrintableHtmlAs(preview: PrintablePreview) {

    setError(null);
    setPrintStatus("Choose where to save the printable HTML.");
    setPrintableSaveResult(null);
    setSourceEmlSaveResult(null);
    setIsSavingPrintable(true);
    try {
      const result = await invoke<SavePrintableHtmlResult>("save_printable_html_as", {
        defaultFilename: preview.defaultFilename,
        html: preview.html,
      });
      setPrintableSaveResult(result);
      if (result.saved) {
        setPrintStatus(`Saved printable HTML to ${result.outputPath}`);
      } else if (result.error) {
        setError(result.error);
        setPrintStatus(null);
      } else {
        setPrintStatus("Save Printable HTML cancelled.");
      }
    } catch (err) {
      setError(getErrorMessage(err));
      setPrintStatus(null);
    } finally {
      setIsSavingPrintable(false);
    }
  }

  async function saveSourcePrintableHtml() {
    const preview = buildSourceEmlPrintablePreview();
    if (!preview) return;
    await savePrintableHtmlAs(preview);
  }

  async function revealSavedHtml(outputPath: string) {
    try {
      await invoke("reveal_saved_html", { outputPath });
      setPrintStatus("Revealed saved HTML in Finder.");
    } catch (err) {
      setError(getErrorMessage(err));
    }
  }

  async function revealSavedEml(outputPath: string) {
    try {
      await invoke("reveal_saved_eml", { outputPath });
      setSourceEmlStatus("Revealed saved source message in Finder.");
    } catch (err) {
      setError(getErrorMessage(err));
    }
  }

  async function revealExportedFile(outputPath: string) {
    let targetWorkspaceId: string | null = null;
    try {
      if (sourceEmlOpen && sourceEmlContext?.kind === "standalone") {
        await invoke("reveal_standalone_exported_file", { outputPath });
      } else {
        targetWorkspaceId =
          sourceEmlOpen && sourceEmlContext?.kind === "workspace"
            ? sourceEmlContext.workspaceId
            : selectedMessage?.workspaceId ?? workspace?.id ?? null;
        if (!targetWorkspaceId) {
          throw new Error("No workspace is available for this exported file.");
        }
        await invoke("reveal_exported_file_for_workspace", {
          workspaceId: targetWorkspaceId,
          outputPath,
        });
      }
      if (targetWorkspaceId) {
        showWorkspaceNotice(targetWorkspaceId, "Revealed exported file in Finder.");
      } else {
        setNotice("Revealed exported file in Finder.");
      }
    } catch (err) {
      if (targetWorkspaceId) showWorkspaceError(targetWorkspaceId, getErrorMessage(err));
      else setError(getErrorMessage(err));
    }
  }

  async function revealWorkspace() {
    if (!workspace) return;
    const workspaceId = workspace.id;
    try {
      await invoke("reveal_workspace_in_finder");
      showWorkspaceNotice(workspaceId, "Revealed workspace in Finder.");
    } catch (err) {
      showWorkspaceError(workspaceId, getErrorMessage(err));
    }
  }

  async function revealOriginalPst() {
    if (!workspace) return;
    const workspaceId = workspace.id;
    try {
      await invoke("reveal_original_pst_in_finder");
      showWorkspaceNotice(workspaceId, "Revealed original PST in Finder.");
    } catch (err) {
      showWorkspaceError(workspaceId, getErrorMessage(err));
    }
  }

  async function revealBothFolders() {
    if (!workspace) return;
    const workspaceId = workspace.id;
    try {
      await invoke("reveal_original_and_workspace_in_finder");
      showWorkspaceNotice(workspaceId, "Opened original PST and workspace locations in Finder.");
    } catch (err) {
      showWorkspaceError(workspaceId, getErrorMessage(err));
    }
  }

  async function revealImportLog() {
    if (!workspace) return;
    const workspaceId = workspace.id;
    try {
      await invoke("reveal_import_log");
      showWorkspaceNotice(workspaceId, "Revealed import log in Finder.");
    } catch (err) {
      showWorkspaceError(workspaceId, getErrorMessage(err));
    }
  }

  async function revealEml() {
    if (!workspace || !selectedMessage) return;
    const messageWorkspaceId = selectedMessage.workspaceId ?? workspace.id;
    try {
      await invoke("reveal_eml", {
        workspaceId: messageWorkspaceId,
        messageId: selectedMessage.id,
      });
    } catch (err) {
      showWorkspaceError(messageWorkspaceId, getErrorMessage(err));
    }
  }

  async function cancelImport(snapshot: ImportProgressSnapshot | null) {
    if (!snapshot || snapshot.status !== "active") return;
    const currentOperation = workspaceOperationStates[snapshot.workspaceId] ?? null;
    const currentSnapshot = currentOperation?.progress ?? null;
    if (
      !currentSnapshot ||
      currentSnapshot.status !== "active" ||
      currentOperation?.progressOperationId !== snapshot.operationId ||
      currentSnapshot.workspaceId !== snapshot.workspaceId ||
      currentSnapshot.operationId !== snapshot.operationId ||
      currentSnapshot.generation !== snapshot.generation ||
      currentSnapshot.stateRevision !== snapshot.stateRevision ||
      currentSnapshot.eventSequence !== snapshot.eventSequence
    ) {
      return;
    }
    const targetWorkspaceId = snapshot.workspaceId;
    const integrityOperation = targetWorkspaceId
      ? integrityOperationTokensRef.current.get(targetWorkspaceId) ?? null
      : null;
    if (targetWorkspaceId) {
      updateWorkspaceOperationState(targetWorkspaceId, {
        notice: integrityOperation
          ? `Cancel ${integrityOperationLabel(integrityOperation.kind).toLocaleLowerCase()} requested.`
          : "Cancel import requested.",
        error: null,
      });
    }
    try {
      const result = await invoke<CancelImportResult>("cancel_import", {
        workspaceId: snapshot.workspaceId,
        operationId: snapshot.operationId,
        generation: snapshot.generation,
      });
      if (integrityOperation && !workspaceIntegrityOperationIsCurrent(integrityOperation)) return;
      if (targetWorkspaceId) {
        updateWorkspaceOperationState(targetWorkspaceId, { notice: result.message });
      } else {
        setNotice(result.message);
      }
    } catch (err) {
      if (integrityOperation && !workspaceIntegrityOperationIsCurrent(integrityOperation)) return;
      if (targetWorkspaceId) {
        failWorkspaceOperation(targetWorkspaceId, getErrorMessage(err));
      } else {
        setError(getErrorMessage(err));
      }
    }
  }

  function requestPlannedWorkspaceDelete(
    plan: PstOpenPlan,
    existingWorkspace: ExistingWorkspace,
    returnFocus: HTMLElement,
  ) {
    deleteConfirmReturnFocusRef.current = returnFocus;
    setPendingPlannedDelete({ plan, existingWorkspace });
    window.requestAnimationFrame(() => deleteConfirmCancelRef.current?.focus());
  }

  function closeVisibleDeleteConfirmation() {
    if (visiblePendingPlannedDelete) setPendingPlannedDelete(null);
    else if (workspace && deleteConfirmOpen) cancelDeleteCurrentWorkspace();
    const returnFocus = deleteConfirmReturnFocusRef.current;
    deleteConfirmReturnFocusRef.current = null;
    window.requestAnimationFrame(() => {
      if (returnFocus?.isConnected) returnFocus.focus();
    });
  }

  function confirmVisibleDelete() {
    if (visiblePendingPlannedDelete) {
      const pending = visiblePendingPlannedDelete;
      setPendingPlannedDelete(null);
      deleteConfirmReturnFocusRef.current = null;
      void deletePlannedWorkspace(pending.plan, pending.existingWorkspace);
    } else if (workspace && deleteConfirmOpen) {
      deleteConfirmReturnFocusRef.current = null;
      void confirmDeleteCurrentWorkspace();
    }
  }

  function handleDeleteConfirmationKeyDown(event: ReactKeyboardEvent<HTMLElement>) {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      closeVisibleDeleteConfirmation();
      return;
    }
    if (event.key !== "Tab") return;
    const focusable = Array.from(
      deleteConfirmDialogRef.current?.querySelectorAll<HTMLElement>("button:not(:disabled)") ?? [],
    );
    if (!focusable.length) return;
    const first = focusable[0], last = focusable[focusable.length - 1];
    if (event.shiftKey && document.activeElement === first) {
      event.preventDefault();
      last.focus();
    } else if (!event.shiftKey && document.activeElement === last) {
      event.preventDefault();
      first.focus();
    }
  }

  async function deletePlannedWorkspace(plan: PstOpenPlan, existing: ExistingWorkspace) {
    setError(null);
    setNotice(null);
    setForegroundOperationWorkspaceId(existing.workspaceId);
    updateWorkspaceOperationState(existing.workspaceId, {
      progress: null,
      running: false,
      notice: null,
      error: null,
      setupCommand: null,
      deleteResult: null,
      deleteStatuses: [],
      deleteConfirmOpen: false,
    });
    setIsBusy(true);

    try {
      const result = await invoke<DeleteResult>("delete_planned_workspace", {
        pstPath: plan.pstPath,
        workspacePath: existing.workspacePath,
      });
      if (result.alreadyMissing) {
        await refreshPendingOpenPlan(plan);
        clearWorkspaceOperationState(existing.workspaceId);
        setForegroundOperationWorkspaceId(null);
        setWorkspaceDeleteToast({
          workspaceId: existing.workspaceId,
          message: "Workspace folder was already missing. Refreshed workspace list.",
          result,
        });
      } else if (result.deleted && !result.existsAfter) {
        await refreshPendingOpenPlan(plan);
        clearWorkspaceOperationState(existing.workspaceId);
        setForegroundOperationWorkspaceId(null);
        setWorkspaceDeleteToast({
          workspaceId: existing.workspaceId,
          message:
            "Deleted workspace cache and its Local Trash state. Original PST was not deleted.",
          result,
        });
      } else {
        updateWorkspaceOperationState(existing.workspaceId, {
          deleteResult: result,
          error: `Failed to delete workspace:\n${result.attemptedPath}\n${
            result.error ?? "Workspace still exists after deletion attempt."
          }`,
        });
      }
    } catch (err) {
      failWorkspaceOperation(existing.workspaceId, getErrorMessage(err));
    } finally {
      setIsBusy(false);
    }
  }

  async function deleteCurrentWorkspace(returnFocus: HTMLElement) {
    if (!workspace) return;
    const workspaceId = workspace.id;
    if (isImporting || activeIntegrityOperationRunning) {
      updateWorkspaceOperationState(workspaceId, {
        notice: "Cancel the current workspace operation before deleting this workspace.",
      });
      return;
    }

    setError(null);
    setNotice(null);
    updateWorkspaceOperationState(workspaceId, {
      notice: null,
      error: null,
      deleteResult: null,
      deleteStatuses: [],
      deleteConfirmOpen: true,
    });
    deleteConfirmReturnFocusRef.current = returnFocus;
    window.requestAnimationFrame(() => deleteConfirmCancelRef.current?.focus());
  }

  async function confirmDeleteCurrentWorkspace() {
    if (!workspace) return;
    const deletedWorkspaceId = workspace.id;
    if (isImporting || activeIntegrityOperationRunning) {
      updateWorkspaceOperationState(deletedWorkspaceId, {
        deleteConfirmOpen: false,
        notice: "Cancel the current workspace operation before deleting this workspace.",
      });
      return;
    }

    updateWorkspaceOperationState(deletedWorkspaceId, { deleteConfirmOpen: false });
    addDeleteStatus(deletedWorkspaceId, "Delete command started");
    setIsBusy(true);
    try {
      const result = await invoke<DeleteResult>("delete_workspace", {
        workspaceId: deletedWorkspaceId,
      });
      addDeleteStatus(deletedWorkspaceId, "Delete command returned");
      if (result.deleted && !result.existsAfter) {
        invalidateSearchLifecycle();
        clearWorkspaceOperationState(deletedWorkspaceId);
        setWorkspaceDeleteToast({
          workspaceId: deletedWorkspaceId,
          message:
            "Deleted workspace cache and its Local Trash state. Original PST was not deleted.",
          result,
        });
        const remainingSessions = openPstSessions.filter(
          (session) => session.workspace.id !== deletedWorkspaceId,
        );
        setOpenPstSessions(remainingSessions);
        if (allOpenFolderSelection.workspaceId === deletedWorkspaceId) {
          setAllOpenFolderSelection({ workspaceId: null, folderId: null });
        }
        if (localTrashAllOpenWorkspaceId === deletedWorkspaceId) {
          setLocalTrashAllOpenWorkspaceId(null);
        }
        if (remainingSessions.length > 0) {
          await activateWorkspaceTab(remainingSessions[0].workspace.id, { resetMessageList: true });
        } else {
          clearWorkspaceState();
        }
      } else {
        updateWorkspaceOperationState(deletedWorkspaceId, {
          deleteResult: result,
          error: `Failed to delete workspace:\n${result.attemptedPath}\n${
            result.error ?? "Workspace still exists after deletion attempt."
          }`,
        });
      }
    } catch (err) {
      const message = getErrorMessage(err);
      addDeleteStatus(deletedWorkspaceId, `Delete command failed: ${message}`);
      failWorkspaceOperation(deletedWorkspaceId, message);
    } finally {
      setIsBusy(false);
    }
  }

  function cancelDeleteCurrentWorkspace() {
    if (!workspace) return;
    updateWorkspaceOperationState(workspace.id, {
      deleteConfirmOpen: false,
      deleteStatuses: [],
    });
  }

  function clampPaneWidths(
    nextWidths: PaneWidths,
    containerWidth?: number,
    containerHeight?: number,
  ): PaneWidths {
    const availableWidth =
      containerWidth ?? paneLayoutRef.current?.getBoundingClientRect().width ?? window.innerWidth;
    const maxFolder = availableWidth - nextWidths.message - readerPaneMin - splitterTotalWidth;
    const folder = clamp(nextWidths.folder, folderPaneMin, Math.min(520, maxFolder));
    const maxMessage = availableWidth - folder - readerPaneMin - splitterTotalWidth;
    const message = clamp(nextWidths.message, messagePaneMin, Math.min(720, maxMessage));
    const availableHeight =
      containerHeight ?? paneLayoutRef.current?.getBoundingClientRect().height ?? window.innerHeight;
    const maxOutlookMessage = availableHeight - outlookReaderPaneMin - outlookSplitterHeight;
    const outlookMessage = clamp(
      nextWidths.outlookMessage,
      outlookMessagePaneMin,
      Math.min(900, maxOutlookMessage),
    );

    return { folder, message, outlookMessage };
  }

  function setFolderPaneWidth(width: number) {
    setPaneWidths((current) => clampPaneWidths({ ...current, folder: width }));
  }

  function setMessagePaneWidth(width: number) {
    setPaneWidths((current) => clampPaneWidths({ ...current, message: width }));
  }

  function setOutlookMessagePaneHeight(height: number) {
    setPaneWidths((current) => clampPaneWidths({ ...current, outlookMessage: height }));
  }

  function resetPaneLayout() {
    setPaneWidths(clampPaneWidths(defaultPaneWidths));
  }

  function toggleFolderCollapsed(folder: FolderNode, workspaceId = workspace?.id ?? "active") {
    const key = folderCollapseKey(folder, workspaceId);
    setCollapsedFolderPaths((current) => {
      const next = new Set(current);
      if (next.has(key)) {
        next.delete(key);
      } else {
        next.add(key);
      }
      return next;
    });
  }

  function collapseAllFolders() {
    if (searchScope === "all_open" && openPstSessions.length > 0) {
      const keys = openSessionFolderTrees.flatMap(({ session, tree }) => [
        workspaceRootCollapseKey(session.workspace.id),
        ...collectCollapsibleFolderKeys(tree, session.workspace.id),
      ]);
      setCollapsedFolderPaths(new Set(keys));
      return;
    }
    setCollapsedFolderPaths(new Set(collectCollapsibleFolderKeys(folderTree, workspace?.id ?? "active")));
  }

  function expandAllFolders() {
    setCollapsedFolderPaths(new Set());
  }

  function updateSearchFilter<K extends keyof AdvancedSearchFilters>(
    key: K,
    value: AdvancedSearchFilters[K],
  ) {
    setSearchFilters((current) => ({ ...current, [key]: value }));
  }

  function applySearchDraftToControls(nextDraft: SearchDraft) {
    const { query, ...filters } = nextDraft;
    setSearch(query);
    setSearchFilters(filters);
  }

  function removeSearchChip(id: AppliedFilterChipId) {
    const removal = removeAppliedFilter(draftSearchSnapshot, id);
    if (!removal.changed) return;

    forceNextSearchApplication();
    applySearchDraftToControls(removal.draft);
    if (removal.scope !== searchScope) {
      setSearchScope(removal.scope);
    }
    if (id === "folderScope") {
      setIncludeSubfolders(true);
      rememberIncludeSubfolders(true);
    }
    if (removal.clearFolderSelection) {
      if (draftSearchSnapshot.scope === "all_open") {
        if (messageVisibility === "active") {
          setAllOpenFolderSelection({ workspaceId: null, folderId: null });
        }
      } else {
        selectCurrentWorkspaceFolder(null);
      }
    }
  }

  function clearAllSearch() {
    const cleared = clearAllSearchDraft(draftSearchSnapshot);
    if (!cleared.changed) return;

    forceNextSearchApplication();
    applySearchDraftToControls(cleared.draft);
    setSortOrder(cleared.messageSort);
    setConversationSort(cleared.conversationSort);
    setIncludeSubfolders(true);
    rememberIncludeSubfolders(true);
    if (cleared.clearAllOpenFolderSelection && messageVisibility === "active") {
      setAllOpenFolderSelection({ workspaceId: null, folderId: null });
    }
  }

  function rememberWorkspaceFolderSelection(selection: WorkspaceFolderSelection) {
    setOpenPstSessions((current) =>
      current.map((session) =>
        session.workspace.id === selection.workspaceId
          ? {
              ...session,
              folderSelection: selection,
              canonicalFolderSelection: selection,
            }
          : session,
      ),
    );
  }

  function showActiveMessages() {
    if (messageVisibility === "active") {
      setLocalTrashAllOpenWorkspaceId(null);
      return;
    }
    const targetWorkspaceId =
      searchScope === "all_open" ? localTrashAllOpenWorkspaceId : workspace?.id ?? null;
    setMessageVisibility("active");
    setLocalTrashAllOpenWorkspaceId(null);
    setListMode(previousActiveListModeRef.current);
    setOpenPstSessions((current) =>
      current.map((session) =>
        targetWorkspaceId === session.workspace.id &&
        isLocalTrashFolderSelection(session.folderSelection)
          ? { ...session, folderSelection: session.canonicalFolderSelection }
          : session,
      ),
    );
  }

  function showLocalTrash(targetWorkspaceId: string | null) {
    if (messageVisibility === "active") previousActiveListModeRef.current = listMode;
    setMessageVisibility("trashed");
    setListMode("messages");
    setLocalTrashAllOpenWorkspaceId(
      searchScope === "all_open" ? targetWorkspaceId : null,
    );
    if (targetWorkspaceId) {
      setOpenPstSessions((current) =>
        current.map((session) =>
          session.workspace.id === targetWorkspaceId
            ? {
                ...session,
                folderSelection: localTrashWorkspaceFolderSelection(
                  targetWorkspaceId,
                  session.canonicalFolderSelection.includeSubfolders,
                ),
              }
            : session,
        ),
      );
    }
  }

  function selectCurrentWorkspaceFolder(folderId: number | null) {
    if (!workspace) return;
    showActiveMessages();
    const selection = resolveWorkspaceFolderSelection(workspace.id, folders, {
      workspaceId: workspace.id,
      folderId,
      virtualFolder: folderId == null ? "all_mail" : null,
      includeSubfolders,
    });
    setSelectedFolderId(selection.folderId);
    rememberWorkspaceFolderSelection(selection);
  }

  function selectAllOpenWorkspaceFolder(workspaceId: string, folderId: number | null) {
    const session = openPstSessions.find((candidate) => candidate.workspace.id === workspaceId);
    if (!session) return;
    showActiveMessages();
    const selection = resolveWorkspaceFolderSelection(workspaceId, session.folders, {
      workspaceId,
      folderId,
      virtualFolder: folderId == null ? "all_mail" : null,
      includeSubfolders: session.folderSelection.includeSubfolders,
    });
    setIncludeSubfolders(selection.includeSubfolders);
    setSearchFilters((current) => ({
      ...current,
      folderScope: selection.includeSubfolders ? "current_subfolders" : "current",
    }));
    setAllOpenFolderSelection({ workspaceId, folderId: selection.folderId });
    if (workspace?.id === workspaceId) {
      setSelectedFolderId(selection.folderId);
    }
    rememberWorkspaceFolderSelection(selection);
  }

  function rememberIncludeSubfolders(checked: boolean) {
    const workspaceId =
      searchScope === "all_open" ? allOpenFolderSelection.workspaceId : workspace?.id;
    if (!workspaceId) return;
    const session = openPstSessions.find((candidate) => candidate.workspace.id === workspaceId);
    if (!session) return;
    const folderId =
      searchScope === "all_open" ? allOpenFolderSelection.folderId : selectedFolderId;
    rememberWorkspaceFolderSelection(
      resolveWorkspaceFolderSelection(workspaceId, session.folders, {
        workspaceId,
        folderId,
        virtualFolder: folderId == null ? "all_mail" : null,
        includeSubfolders: checked,
      }),
    );
  }

  function updateFolderScope(folderScope: FolderScopeFilter) {
    setSearchFilters((current) => ({ ...current, folderScope }));
    if (folderScope === "current") {
      setIncludeSubfolders(false);
      rememberIncludeSubfolders(false);
    }
    if (folderScope === "current_subfolders") {
      setIncludeSubfolders(true);
      rememberIncludeSubfolders(true);
    }
  }

  function toggleIncludeSubfolders(checked: boolean) {
    setIncludeSubfolders(checked);
    rememberIncludeSubfolders(checked);
    setSearchFilters((current) => ({
      ...current,
      folderScope: checked ? "current_subfolders" : "current",
    }));
  }

  function startPaneResize(
    pane: "folder" | "message" | "outlookMessage",
    event: ReactPointerEvent<HTMLDivElement>,
  ) {
    event.preventDefault();
    const startX = event.clientX;
    const startY = event.clientY;
    const startWidths = paneWidths;
    const containerRect = paneLayoutRef.current?.getBoundingClientRect();
    const containerWidth = containerRect?.width ?? window.innerWidth;
    const containerHeight = containerRect?.height ?? window.innerHeight;

    const onPointerMove = (moveEvent: PointerEvent) => {
      const deltaX = moveEvent.clientX - startX;
      const deltaY = moveEvent.clientY - startY;
      const nextWidths =
        pane === "folder"
          ? { ...startWidths, folder: startWidths.folder + deltaX }
          : pane === "message"
            ? { ...startWidths, message: startWidths.message + deltaX }
            : { ...startWidths, outlookMessage: startWidths.outlookMessage + deltaY };
      setPaneWidths(clampPaneWidths(nextWidths, containerWidth, containerHeight));
    };

    const stopResize = () => {
      document.body.classList.remove("pane-resizing");
      window.removeEventListener("pointermove", onPointerMove);
      window.removeEventListener("pointerup", stopResize);
      window.removeEventListener("pointercancel", stopResize);
    };

    document.body.classList.add("pane-resizing");
    window.addEventListener("pointermove", onPointerMove);
    window.addEventListener("pointerup", stopResize);
    window.addEventListener("pointercancel", stopResize);
  }

  function renderWorkspaceChoice(plan: PstOpenPlan, existing: ExistingWorkspace) {
    if (existing.integrityClassification === "complete" && existing.canOpen) {
      return (
        <button
          type="button"
          key={existing.workspacePath}
          onClick={() => void openPlannedPst(plan, existing, false, "open_existing")}
          disabled={isBusy}
        >
          <span>
            Open {existing.workspaceLocationLabel} ({formatCount(existing.messageCount)})
          </span>
          <small>{existing.workspacePath}</small>
        </button>
      );
    }

    return (
      <div className="duplicate-workspace-choice" key={existing.workspacePath}>
        <strong>
          {existing.integrityClassification === "unsupported" ? "Unsupported" : "Incomplete"}{" "}
          {existing.workspaceLocationLabel} workspace -{" "}
          {workspaceIntegrityStateLabel(existing.integrityState)}
        </strong>
        {existing.resumeDetails ? (
          <>
            <span>
              {resumeProgressDescription(existing.resumeDetails, formatCount)}
            </span>
            <small>{existing.resumeDetails.reason}</small>
          </>
        ) : (
          <small>{existing.importStatus}</small>
        )}
        <details>
          <summary>Workspace location</summary>
          <small>{existing.workspacePath}</small>
        </details>
        <div className="workspace-safe-actions">
          {existing.canOpen ? (
            <button
              type="button"
              onClick={() => void openPlannedPst(plan, existing, false, "open_existing")}
              disabled={isBusy}
            >
              Open workspace
            </button>
          ) : null}
          <button
            type="button"
            onClick={() => void openPlannedPst(plan, existing, false, "resume_index")}
            disabled={isBusy || !existing.canResume}
            title={
              existing.canResume
                ? "Resume indexing extracted EML files in this workspace."
                : "Resume is unavailable because this workspace has no extracted EML folder."
            }
          >
            Resume Indexing
          </button>
          {existing.canRepairIndex ? (
            <button
              type="button"
              data-support-control="click"
              data-support-recovery="open"
              onClick={(event) =>
                requestPlannedWorkspaceRepair(plan, existing, event.currentTarget)
              }
              disabled={isBusy}
            >
              Repair Index
            </button>
          ) : null}
        </div>
        <div className="workspace-destructive-actions" aria-label="Destructive workspace options">
          <button
            type="button"
            onClick={() => void beginPlannedPstOpen(plan, existing, false, "reimport")}
            disabled={isBusy || !existing.canReimport}
            title={
              existing.canReimport
                ? "Rerun readpst into this workspace."
                : "Reimport is unavailable because this workspace folder is missing."
            }
          >
            Reimport from PST
          </button>
          <button
            type="button"
            className="danger-action"
            data-support-control="click"
            data-support-recovery="open"
            onClick={(event) => requestPlannedWorkspaceDelete(plan, existing, event.currentTarget)}
            disabled={isBusy || !existing.canReimport}
            title="Delete this workspace cache and its Local Trash state after confirmation. The original PST is not deleted."
          >
            Delete workspace
          </button>
        </div>
      </div>
    );
  }

  const selectedFolder = folders.find((folder) => folder.id === selectedFolderId);
  const folderDetailsSession =
    searchScope === "all_open"
      ? allOpenSelectedSession
      : openPstSessions.find((session) => session.workspace.id === workspace?.id) ?? null;
  const folderDetailsWorkspace =
    folderDetailsSession?.workspace ?? (searchScope === "current" ? workspace : null);
  const folderDetailsHierarchy =
    searchScope === "all_open" ? allOpenSelectedHierarchy : folderHierarchy;
  const folderDetailsFolderId =
    searchScope === "all_open" ? allOpenSelectedFolderId : selectedFolderId;
  const folderDetailsFolder =
    folderDetailsSession && folderDetailsFolderId != null
      ? folderDetailsSession.folders.find((folder) => folder.id === folderDetailsFolderId) ?? null
      : searchScope === "current"
        ? selectedFolder ?? null
        : null;
  const folderDetailsPresentationNode =
    folderDetailsHierarchy && folderDetailsFolderId != null
      ? folderDetailsHierarchy.flatNodes.find(
          (node) => node.canonicalFolderId === folderDetailsFolderId,
        ) ?? null
      : null;
  const folderDetailsPresentationSelection = folderDetailsHierarchy
    ? resolveFolderPresentationSelection(folderDetailsHierarchy, folderDetailsFolderId)
    : null;
  const folderDetailsWrappersHidden =
    folderHierarchyMode === "simplified" &&
    (folderDetailsPresentationSelection?.kind === "promoted_scope" ||
      folderDetailsPresentationNode?.ancestorWasPromoted === true);
  const displayedFolderName =
    appliedSearch.visibility === "trashed"
      ? appliedIsCrossPstSearch && appliedSelectedWorkspaceId
        ? `${pstDisplayName(appliedSelectedSession?.workspace ?? null)} - Local Trash`
        : "Local Trash"
      : appliedIsCrossPstSearch
      ? appliedSelectedWorkspaceId
        ? `${pstDisplayName(appliedSelectedSession?.workspace ?? null)} - ${
            appliedSearch.folderId == null
              ? "All Mail"
              : appliedSelectedFolder?.name ?? "Folder"
          }`
        : "All Open PSTs"
      : appliedSearch.folderScope === "all" || appliedSearch.folderId == null
      ? "All Mail"
      : appliedSelectedFolder?.name ?? selectedFolder?.name ?? "Messages";
  const progressSnapshots = Object.values(workspaceOperationStates).flatMap((operation) =>
    operation.progress ? [operation.progress] : [],
  );
  const progressIdentityPeers: ImportSourceIdentityPeer[] = [
    ...progressSnapshots.map(({ workspaceId, sourceBasename }) => ({
      workspaceId,
      sourceBasename,
    })),
    ...openPstSessions.map((session) => ({
      workspaceId: session.workspace.id,
      sourceBasename: pstDisplayName(session.workspace),
    })),
  ];
  const selectedWorkspaceProgress = workspace
    ? workspaceOperationStates[workspace.id]?.progress ?? null
    : null;
  const foregroundProgress = foregroundOperationWorkspaceId
    ? workspaceOperationStates[foregroundOperationWorkspaceId]?.progress ?? null
    : null;
  const activeProgress = progressSnapshots.find((candidate) => candidate.status === "active") ?? null;
  const latestTerminalProgress = progressSnapshots
    .filter((candidate) => candidate.status !== "active")
    .reduce<ImportProgressSnapshot | null>((latest, candidate) => {
      if (latest === null) return candidate;
      const latestReceived =
        workspaceOperationStates[latest.workspaceId]?.progressReceivedAtMonotonicMs ?? -1;
      const candidateReceived =
        workspaceOperationStates[candidate.workspaceId]?.progressReceivedAtMonotonicMs ?? -1;
      return candidateReceived > latestReceived ? candidate : latest;
    }, null);
  const progress =
    (selectedWorkspaceProgress?.status === "active" ? selectedWorkspaceProgress : null) ??
    (foregroundProgress?.status === "active" ? foregroundProgress : null) ??
    activeProgress ??
    latestTerminalProgress ??
    selectedWorkspaceProgress ??
    foregroundProgress ??
    null;
  useEffect(() => {
    if (openPopoverId !== "import_progress") return;
    activePopoverTriggerRef.current = null;
    setOpenPopoverId(null);
  }, [progress?.operationId]);
  const progressOperationState = progress
    ? workspaceOperationStates[progress.workspaceId] ?? null
    : null;
  const progressIsBackground = Boolean(
    progress && workspace && progress.workspaceId !== workspace.id,
  );
  const workspaceOperationNotice = visibleWorkspaceOperation?.notice ?? null;
  const workspaceOperationError = visibleWorkspaceOperation?.error ?? null;
  const displayedError = workspaceOperationError ?? activeSearchError ?? error;
  const displayedSetupCommand = workspaceOperationError
    ? visibleWorkspaceOperation?.setupCommand ?? null
    : activeSearchError
      ? null
      : setupCommand;
  const displayedNotice = workspaceOperationNotice ?? notice;
  const deleteResult = visibleWorkspaceOperation?.deleteResult ?? null;
  const deleteStatuses = activeWorkspaceOperation?.deleteStatuses ?? [];
  const deleteConfirmOpen = activeWorkspaceOperation?.deleteConfirmOpen ?? false;
  const activeWorkspaceOperationRunning = activeWorkspaceOperation?.running ?? false;
  const activeIntegrityOperationKind =
    activeWorkspaceOperation?.integrityOperationKind ?? null;
  const activeIntegrityOperationRunning =
    activeWorkspaceOperationRunning && activeIntegrityOperationKind != null;
  const activeIntegrityProgress =
    activeWorkspaceOperation?.progress?.status === "active" &&
    activeWorkspaceOperation.progressOperationId === activeWorkspaceOperation.progress.operationId
      ? activeWorkspaceOperation.progress
      : null;
  const activeVerificationReport = activeWorkspaceOperation?.verificationReport ?? null;
  const verificationReportOpen =
    activeWorkspaceOperation?.verificationReportOpen === true && activeVerificationReport != null;
  const repairConfirmOpen = activeWorkspaceOperation?.repairConfirmOpen ?? false;
  const integrityActions = workspace ? workspaceIntegrityActions(workspace) : [];
  const sourceAvailabilityMessage = workspace
    ? workspaceSourceAvailabilityMessage(workspace.sourceAvailability)
    : null;
  const progressIdentity = progress
    ? importIdentityPresentation(progress, progressIdentityPeers)
    : null;
  const progressTracker = progress ? importStageTracker(progress) : [];
  const progressSemantics = progress ? importProgressSemantics(progress) : null;
  const progressUnit = progress
    ? progress.stage === "indexing" &&
      progress.determinate &&
      progress.processedUnits != null &&
      progress.totalUnits != null &&
      progress.unitKind === "candidate_records"
      ? formatCandidateProgress({
          current: progress.processedUnits,
          total: progress.totalUnits,
        })
      : unitProgressText(progress)
    : null;
  const progressAllCounters = progress ? importCounterRows(progress, formatCount) : [];
  const progressStageCounters = progress
    ? progressAllCounters.filter((row) => importStageCounterKeys[progress.stage].includes(row.key))
    : [];
  const progressTerminal = progress
    ? terminalImportPresentation(progress, progressIdentityPeers)
    : null;
  const progressLiveNow = progressClockTick >= 0 ? monotonicMilliseconds() : 0;
  const progressLiveDelta =
    progress?.status === "active" && progressOperationState?.progressReceivedAtMonotonicMs != null
      ? Math.max(0, progressLiveNow - progressOperationState.progressReceivedAtMonotonicMs)
      : 0;
  const progressActiveElapsedMs = progress
    ? progress.activeElapsedMs +
      (["new_import", "resumed_import"].includes(progress.operationKind)
        ? progressLiveDelta
        : 0)
    : 0;
  const progressOperationElapsedMs = progress
    ? progress.operationElapsedMs + progressLiveDelta
    : 0;
  const progressStageElapsedMs = progress
    ? progress.stageElapsedMs + progressLiveDelta
    : 0;
  const progressDisplayedElapsedMs =
    progress?.operationKind === "repair" || progress?.operationKind === "verification"
      ? progressOperationElapsedMs
      : progressActiveElapsedMs;
  const progressLiveActivityAgeMs =
    progress?.lastActivityAgeMs == null
      ? null
      : progress.lastActivityAgeMs + progressLiveDelta;
  const progressEta = progress
    ? importEtaPresentation(progress, progressLiveActivityAgeMs)
    : null;
  const progressLastActivity = progress
    ? lastActivityText(progress, progressLiveActivityAgeMs)
    : null;
  const progressHealth = progress
    ? processHealthText(progress, progressLiveActivityAgeMs)
    : null;
  const progressExplanation = progress ? importStageExplanation(progress) : null;
  const progressFillPercent =
    progressSemantics?.kind === "determinate"
      ? Math.min(
          100,
          Math.max(
            0,
            (progressSemantics.ariaValueNow / progressSemantics.ariaValueMax) * 100,
          ),
        )
      : null;
  const progressWorkspaceIsOpen = Boolean(
    progress && openWorkspaceIds.includes(progress.workspaceId),
  );
  const visibleIntegrityOperationKind =
    progressOperationState?.integrityOperationKind ?? null;
  const progressCanCancel = Boolean(
    progress?.status === "active" &&
      progressOperationState?.running &&
      progressOperationState.progressOperationId === progress.operationId &&
      progressOperationState.progress?.generation === progress.generation &&
      progressOperationState.progress?.stateRevision === progress.stateRevision &&
      progressOperationState.progress?.eventSequence === progress.eventSequence &&
      (!visibleIntegrityOperationKind
        ? progress.operationKind !== "verification"
        : integrityOperationIsCancellable(visibleIntegrityOperationKind)),
  );
  const progressAnnouncementKey = progress
    ? materialImportAnnouncementKey(progress)
    : "no-import-progress";
  const progressAnnouncement = progress
    ? materialImportAnnouncementText(progress, progressIdentityPeers)
    : "";
  const progressCancelLabel =
    progressOperationState?.integrityOperationKind != null
      ? `Cancel ${integrityOperationLabel(progressOperationState.integrityOperationKind)}`
      : foregroundImportAction === "repair_index" || progress?.operationKind === "repair"
        ? "Cancel Repair"
        : progress?.operationKind === "resumed_import"
          ? "Cancel Resume"
          : "Cancel Import";
  const pendingWorkspaceFlowVisible = pendingWorkspaceFlowBelongsToActiveTab;
  const visiblePendingOpenPlan = pendingWorkspaceFlowVisible ? pendingOpenPlan : null;
  const visiblePendingPreflightOpen = pendingWorkspaceFlowVisible ? pendingPreflightOpen : null;
  const visiblePendingPlannedRepair = pendingWorkspaceFlowVisible
    ? pendingPlannedRepair
    : null;
  const visiblePendingPlannedDelete = pendingWorkspaceFlowVisible
    ? pendingPlannedDelete
    : null;
  const pendingSelectedWorkspace =
    visiblePendingOpenPlan?.existingWorkspaces.find(
      (existing) => existing.workspacePath === visiblePendingOpenPlan.selectedWorkspacePath,
    ) ?? null;
  const pendingOtherWorkspaces =
    visiblePendingOpenPlan?.existingWorkspaces.filter(
      (existing) => existing.workspacePath !== visiblePendingOpenPlan.selectedWorkspacePath,
    ) ?? [];
  const pendingCompleteWorkspaces = pendingOtherWorkspaces.filter((existing) => existing.isComplete);
  const pendingIncompleteWorkspaces = pendingOtherWorkspaces.filter(
    (existing) => !existing.isComplete,
  );
  const selectedBody = selectedMessage ? formatBodyForDisplay(selectedMessage.body) : "";
  const selectedMessageWorkspaceId = selectedMessage?.workspaceId ?? workspace?.id ?? null;
  const selectedMessageLocalTrashSummary = selectedMessageWorkspaceId
    ? openPstSessions.find(
        (session) => session.workspace.id === selectedMessageWorkspaceId,
      )?.localTrashSummary ?? null
    : null;
  const selectedMessageLocalTrashAvailable = Boolean(
    selectedMessage &&
      selectedMessage.localTrashAvailable &&
      isLocalTrashHealthy(selectedMessageLocalTrashSummary),
  );
  const localTrashUndoSummary = localTrashUndo
    ? openPstSessions.find(
        (session) => session.workspace.id === localTrashUndo.workspaceId,
      )?.localTrashSummary ?? null
    : null;
  const canUndoLocalTrashMove = Boolean(
    localTrashUndo &&
      localTrashUndoSummary &&
      isLocalTrashHealthy(localTrashUndoSummary) &&
      isLocalTrashUndoCurrent(
        localTrashUndo,
        localTrashUndo.workspaceId,
        localTrashUndoSummary.stateRevision,
      ),
  );
  const selectedBodyStatus = selectedMessage ? bodyStatusMessage(selectedMessage, selectedBody) : null;
  const selectedBodyIsRtfFallback = selectedMessage?.bodySource === "rtf_converted";
  const sourceEmlBody = sourceEmlView ? formatBodyForDisplay(sourceEmlView.bodyText) : "";
  const sourceEmlBodyStatus = sourceEmlView
    ? sourceBodyStatusMessage(sourceEmlView, sourceEmlBody)
    : null;
  const sourceEmlReconstructionWarnings = sourceEmlView
    ? sourceReconstructionWarnings(sourceEmlView)
    : [];
  const messageDiagnosticNotes = messageDiagnostics ? diagnosticNotes(messageDiagnostics) : [];
  const sourceEmlRawTerms = sourceEmlRawSearch.trim() ? [sourceEmlRawSearch.trim()] : [];
  const htmlMissingCanReindex =
    selectedMessage != null &&
    !selectedBodyIsRtfFallback &&
    htmlRender != null &&
    !htmlRender.htmlAvailable &&
    !selectedMessage.bodyHtmlAvailable &&
    selectedMessage.canReindexFromEml;
  const htmlRtfTextFallback =
    selectedMessage != null &&
    selectedBodyIsRtfFallback &&
    selectedBody.trim().length > 0 &&
    (!htmlRender || !htmlRender.htmlAvailable);
  const deleteResolved =
    deleteResult != null &&
    ((deleteResult.deleted && !deleteResult.existsAfter) || deleteResult.alreadyMissing);
  const activeFilterChips = buildAppliedFilterChips(appliedSearch, {
    workspaceCount: appliedSearch.workspaceIds.length,
    workspaceLabel:
      appliedIsCrossPstSearch && appliedSelectedWorkspaceId
        ? pstDisplayName(appliedSelectedSession?.workspace ?? null)
        : null,
    folderLabel:
      appliedSearch.visibility === "trashed"
        ? "Local Trash"
        : appliedSearch.folderId == null
        ? "All Mail"
        : appliedSelectedFolder?.name ?? "Selected folder",
  });
  const canClearAppliedSearch = canClearAll(appliedSearch);
  const activeMessageCountStatus =
    messageCountState.generation === appliedSearchVersion.generation
      ? messageCountState.status
      : "idle";
  const activeConversationCountStatus =
    conversationCountState.generation === appliedSearchVersion.generation
      ? conversationCountState.status
      : "idle";
  const hasMoreMessages = messageHasMore;
  const hasMoreConversations = conversationHasMore;
  const resultScopeSuffix = appliedIsCrossPstSearch
    ? ` across ${formatCount(appliedSearch.workspaceIds.length)} PST${
        appliedSearch.workspaceIds.length === 1 ? "" : "s"
      }`
    : "";
  const messageResultSummaryText =
    activeMessageCountStatus === "ready"
      ? messageTotalCount === 0
        ? `0 results${resultScopeSuffix}`
        : messages.length === messageTotalCount
          ? `Showing ${formatCount(messages.length)} of ${formatCount(messageTotalCount)} results${resultScopeSuffix}`
          : `Showing 1-${formatCount(messages.length)} of ${formatCount(messageTotalCount)} results${resultScopeSuffix}`
      : activeMessageCountStatus === "unavailable"
        ? `Showing ${formatCount(messages.length)} result${messages.length === 1 ? "" : "s"}${resultScopeSuffix}; exact count unavailable`
        : activeMessageCountStatus === "pending"
          ? messages.length
            ? `Showing ${formatCount(messages.length)} result${messages.length === 1 ? "" : "s"}${resultScopeSuffix}; counting total...`
            : `Showing results${resultScopeSuffix}; counting total...`
          : `Message results${resultScopeSuffix}`;
  const conversationResultSummaryText =
    activeConversationCountStatus === "ready"
      ? conversationTotalCount === 0
        ? `0 matching messages in 0 conversations${resultScopeSuffix}`
        : `${formatCount(conversationMatchingMessageCount)} matching message${
            conversationMatchingMessageCount === 1 ? "" : "s"
          } in ${formatCount(conversationTotalCount)} conversation${
            conversationTotalCount === 1 ? "" : "s"
          }${resultScopeSuffix}`
      : activeConversationCountStatus === "unavailable"
        ? `Showing ${formatCount(conversations.length)} conversation${
            conversations.length === 1 ? "" : "s"
          }${resultScopeSuffix}; exact count unavailable`
        : activeConversationCountStatus === "pending"
          ? conversations.length
            ? `Showing ${formatCount(conversations.length)} conversation${
                conversations.length === 1 ? "" : "s"
              }${resultScopeSuffix}; counting totals...`
            : `Showing conversations${resultScopeSuffix}; counting totals...`
          : `Conversation results${resultScopeSuffix}`;
  const resultSummaryText =
    appliedSearch.listMode === "conversations"
      ? conversationResultSummaryText
      : messageResultSummaryText;
  const isInitializingMessageList =
    workspace != null && initializingWorkspaceId === workspace.id;
  const isResultsBusy =
    isInitializingMessageList ||
    isSearching ||
    (appliedSearch.listMode === "messages"
      ? isLoadingMoreMessages
      : isLoadingMoreConversations);
  const searchEmptyState = classifySearchEmptyState({
    hasWorkspace: workspace != null,
    isSearchActive,
    isLoading: isInitializingMessageList || isSearching,
    resultCount:
      appliedSearch.listMode === "conversations" ? conversations.length : messages.length,
    listMode: appliedSearch.listMode,
    scope: appliedSearch.scope,
    activeFilterCount: activeAdvancedFilterCount,
    errorGeneration: searchError?.generation ?? null,
    currentGeneration: appliedSearchVersion.generation,
    visibility: appliedSearch.visibility,
  });
  const conversationReindexRequired =
    appliedSearch.listMode === "conversations" &&
    conversationWorkspaceIssues.length > 0 &&
    conversationIndexedWorkspaceCount === 0 &&
    searchEmptyState.kind !== "loading" &&
    searchEmptyState.kind !== "failed";
  const paneLayoutStyle = {
    "--folder-pane-width": `${paneWidths.folder}px`,
    "--message-pane-width": `${paneWidths.message}px`,
    "--outlook-message-height": `${paneWidths.outlookMessage}px`,
  } as CSSProperties;
  const messageResultsMeasurementIdentity = [
    appearance,
    layoutMode,
    messageListDisplayMode,
    foldersHidden ? "folders-hidden" : "folders-visible",
    paneWidths.folder,
    paneWidths.message,
    paneWidths.outlookMessage,
    appliedIsCrossPstSearch ? "cross-pst" : "single-pst",
    appliedSearch.visibility,
  ].join(":");
  const conversationResultsMeasurementIdentity = [
    appearance,
    layoutMode,
    foldersHidden ? "folders-hidden" : "folders-visible",
    paneWidths.folder,
    paneWidths.message,
    paneWidths.outlookMessage,
    appliedIsCrossPstSearch ? "cross-pst" : "single-pst",
  ].join(":");
  const workspaceSummaryText = workspace
    ? [
        workspaceSize?.workspaceLocationLabel ?? workspace.workspaceLocationLabel,
        workspaceSize ? formatBytes(workspaceSize.totalBytes) : null,
        openPstSessions.length
          ? `${formatCount(openPstSessions.length)} PST${openPstSessions.length === 1 ? "" : "s"} open`
          : null,
      ]
        .filter(Boolean)
        .join(" - ")
    : "";

  return (
    <>
    {dropOverlayVisible ? (
      <div className="file-drop-overlay" role="status" aria-live="polite">
        <div>Drop PST, EML, or MSG files to open</div>
      </div>
    ) : null}
    <RemoteSupportControl />
    <main className="app-shell">
      <header className="top-bar">
        <div className="brand-block">
          <h1>PST QuickView</h1>
          <p>v{appVersion} - read-only local PST viewer</p>
        </div>
        <div className="search-block">
          <div className="search-row">
            <input
              type="search"
              data-support-control="input"
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder='Search, or use from:adam subject:calendar "exact phrase"'
              disabled={!workspace || isBusy}
              aria-label="Search emails"
            />
            <button
              id={advancedSearchToggleId}
              data-support-control="click"
              type="button"
              className={
                advancedSearchOpen || activeAdvancedFilterCount > 0
                  ? "filter-toggle active"
                  : "filter-toggle"
              }
              onClick={(event) => togglePopover("advanced_search", event.currentTarget)}
              disabled={!workspace || isBusy}
              title="Show advanced search filters."
              aria-expanded={advancedSearchOpen}
              aria-controls={advancedSearchPanelId}
              aria-haspopup="dialog"
              aria-label={
                activeAdvancedFilterCount > 0
                  ? `Advanced search, ${activeAdvancedFilterCount} active filter${
                      activeAdvancedFilterCount === 1 ? "" : "s"
                    }`
                  : "Advanced search, no active filters"
              }
            >
              <span>Advanced</span>
              {activeAdvancedFilterCount > 0 ? (
                <span className="filter-count" aria-hidden="true">
                  {activeAdvancedFilterCount}
                </span>
              ) : null}
            </button>
            <label className="scope-control">
              <span>Scope</span>
              <select
                value={searchScope}
                data-support-control="input"
                onChange={(event) => {
                  setSearchScope(normalizeSearchScope(event.target.value));
                  setLocalTrashAllOpenWorkspaceId(null);
                }}
                disabled={!workspace || isBusy}
              >
                <option value="current">Current PST</option>
                <option value="all_open">All Open PSTs</option>
              </select>
            </label>
          </div>
          {advancedSearchOpen ? (
            <div
              id={advancedSearchPanelId}
              ref={advancedSearchPanelRef}
              className="advanced-search-panel"
              role="dialog"
              aria-modal="false"
              aria-labelledby={advancedSearchToggleId}
            >
              <label>
                <span>From</span>
                <input
                  value={searchFilters.from}
                  onChange={(event) => updateSearchFilter("from", event.target.value)}
                  disabled={!workspace || isBusy}
                />
              </label>
              <label>
                <span>To/Cc/Bcc</span>
                <input
                  value={searchFilters.recipients}
                  onChange={(event) => updateSearchFilter("recipients", event.target.value)}
                  disabled={!workspace || isBusy}
                />
              </label>
              <label>
                <span>Subject</span>
                <input
                  value={searchFilters.subject}
                  onChange={(event) => updateSearchFilter("subject", event.target.value)}
                  disabled={!workspace || isBusy}
                />
              </label>
              <label>
                <span>Body</span>
                <input
                  value={searchFilters.body}
                  onChange={(event) => updateSearchFilter("body", event.target.value)}
                  disabled={!workspace || isBusy}
                />
              </label>
              <label>
                <span>Attachment</span>
                <input
                  value={searchFilters.attachment}
                  onChange={(event) => updateSearchFilter("attachment", event.target.value)}
                  disabled={!workspace || isBusy}
                />
              </label>
              <label>
                <span>Has attachments</span>
                <select
                  value={searchFilters.hasAttachments}
                  onChange={(event) =>
                    updateSearchFilter(
                      "hasAttachments",
                      event.target.value as AdvancedSearchFilters["hasAttachments"],
                    )
                  }
                  disabled={!workspace || isBusy}
                >
                  <option value="any">Any</option>
                  <option value="yes">Yes</option>
                  <option value="no">No</option>
                </select>
              </label>
              <label>
                <span>Date from</span>
                <input
                  type="date"
                  value={searchFilters.dateFrom}
                  onChange={(event) => updateSearchFilter("dateFrom", event.target.value)}
                  disabled={!workspace || isBusy}
                />
              </label>
              <label>
                <span>Date to</span>
                <input
                  type="date"
                  value={searchFilters.dateTo}
                  onChange={(event) => updateSearchFilter("dateTo", event.target.value)}
                  disabled={!workspace || isBusy}
                />
              </label>
              <label>
                <span>Folder scope</span>
                <select
                  value={searchFilters.folderScope}
                  onChange={(event) => updateFolderScope(event.target.value as FolderScopeFilter)}
                  disabled={
                    !workspace || isBusy || isCrossPstSearch || messageVisibility === "trashed"
                  }
                  title={
                    messageVisibility === "trashed"
                      ? "Local Trash is not scoped by PST folders."
                      : isCrossPstSearch
                        ? "Cross-PST search uses All Mail in each open PST."
                        : undefined
                  }
                >
                  <option value="current">Current folder</option>
                  <option value="current_subfolders">Current folder + subfolders</option>
                  <option value="all">All Mail</option>
                </select>
              </label>
              <div className="advanced-search-actions">
                <button
                  type="button"
                  onClick={() => {
                    clearAllSearch();
                    closeActivePopover();
                  }}
                  disabled={!workspace || isBusy || !canClearAppliedSearch}
                  aria-label="Clear all search filters and sorting"
                >
                  Clear All
                </button>
              </div>
            </div>
          ) : null}
        </div>
        <label
          className="workspace-location-control"
          title="Choose where PST QuickView stores the searchable workspace/cache."
        >
          <span>Storage</span>
          <select
            value={workspaceLocationMode}
            onChange={(event) =>
              setWorkspaceLocationMode(event.target.value as WorkspaceLocationMode)
            }
            disabled={isBusy || localTrashMutationPending}
            title={
              workspaceLocationMode === "next_to_pst"
                ? "Next to PST stores the searchable cache beside the PST, useful for external drives."
                : "App Support stores the searchable cache on this Mac."
            }
          >
            <option value="next_to_pst">Next to PST</option>
            <option value="app_support">App Support</option>
          </select>
        </label>
        <div className="top-actions">
          <button
            type="button"
            className="primary-action"
            onClick={openPst}
            disabled={
              isBusy ||
              localTrashMutationPending ||
              anyWorkspaceIntegrityOperationRunning ||
              readpstStatus?.available !== true
            }
            title={
              readpstStatus?.available === false
                ? readpstStatus.setupCommand
                : "Choose a PST file to open."
            }
          >
            Open PST
          </button>
          <button
            type="button"
            onClick={() => void openStandaloneMessage()}
            disabled={isBusy}
            title="Open a standalone .eml or .msg file in the safe Print / Export viewer."
          >
            Open Message
          </button>
          {recentPsts.length ? (
            <div className="recent-menu">
              <button
                type="button"
                className="popover-trigger"
                title="Open a recently selected PST."
                onClick={(event) => togglePopover("recent", event.currentTarget)}
                aria-expanded={openPopoverId === "recent"}
                aria-controls={recentPopoverPanelId}
                aria-haspopup="dialog"
              >
                Recent
              </button>
              {openPopoverId === "recent" ? (
              <div
                id={recentPopoverPanelId}
                ref={recentPopoverPanelRef}
                className="recent-menu-panel"
                role="dialog"
                aria-modal="false"
                aria-label="Open Recent"
              >
                <div className="recent-menu-header">
                  <strong>Open Recent</strong>
                  <button
                    type="button"
                    onClick={() => {
                      clearRecentPsts();
                      closeActivePopover();
                    }}
                    disabled={
                      isBusy || localTrashMutationPending || anyWorkspaceIntegrityOperationRunning
                    }
                  >
                    Clear
                  </button>
                </div>
                <div className="recent-menu-items">
                  {recentPsts.map((recent) => (
                    <button
                      type="button"
                      key={recent.path}
                      onClick={() => {
                        closeActivePopover();
                        void openRecentPst(recent.path);
                      }}
                      disabled={
                        isBusy || localTrashMutationPending || anyWorkspaceIntegrityOperationRunning
                      }
                      title={pathParent(recent.path)}
                    >
                      <span>{pathFileName(recent.path)}</span>
                      <small>{pathParent(recent.path)}</small>
                    </button>
                  ))}
                </div>
              </div>
              ) : null}
            </div>
          ) : null}
          <button
            type="button"
            className="help-action"
            onClick={(event) => togglePopover("help", event.currentTarget)}
            title="About PST QuickView and safety notes."
            aria-expanded={aboutOpen}
            aria-controls={helpDialogId}
            aria-haspopup="dialog"
          >
            Help
          </button>
        </div>
      </header>

      {restorePromptVisible && savedSession?.entries.length ? (
        <section className="restore-session-prompt" role="dialog" aria-label="Restore previous PST session">
          <div>
            <strong>Restore {formatCount(savedSession.entries.length)} PST{savedSession.entries.length === 1 ? "" : "s"} from last session?</strong>
            <span>
              Opens saved local workspaces. Missing source volumes are reported without deleting local workspace data.
            </span>
            {restoreStatus ? <span>{restoreStatus}</span> : null}
          </div>
          <div className="restore-session-actions">
            <button
              type="button"
              className="primary-action"
              onClick={() => void restorePreviousSession()}
              disabled={isBusy || anyWorkspaceIntegrityOperationRunning}
            >
              Restore
            </button>
            <button
              type="button"
              onClick={startFreshPreviousSession}
              disabled={isBusy || anyWorkspaceIntegrityOperationRunning}
            >
              Start Fresh
            </button>
          </div>
        </section>
      ) : null}

      {readpstStatus?.available === false ? (
        <section className="notice-box" role="status">
          {readpstStatus.setupCommand}
          <button type="button" onClick={() => void checkReadpstStatus()} disabled={isBusy}>
            Check again
          </button>
        </section>
      ) : null}

      {workspace ? (
        <section className="workspace-chrome" aria-label="Open PSTs and workspace controls">
          {openPstSessions.length ? (
            <div className="open-pst-tabs" aria-label="Open PSTs">
              {openPstSessions.map((session) => {
                const active = workspace?.id === session.workspace.id;
                return (
                  <div key={session.workspace.id} className={`open-pst-tab ${active ? "active" : ""}`}>
                    <button
                      type="button"
                      className="open-pst-tab-main"
                      onClick={() => void activateWorkspaceTab(session.workspace.id)}
                      title={session.workspace.pstPath}
                      disabled={
                        localTrashMutationPending ||
                        (isBusy && !canSwitchTabsDuringOperation)
                      }
                    >
                      {pstDisplayName(session.workspace)} {formatCount(session.workspace.messageCount)}
                    </button>
                    <button
                      type="button"
                      className="open-pst-tab-close"
                      onClick={() => void closeWorkspaceTab(session.workspace.id)}
                      disabled={
                        isBusy ||
                        isImporting ||
                        localTrashMutationPending ||
                        workspaceOperationStates[session.workspace.id]?.running === true
                      }
                      title="Close this PST from the open set. This does not delete the workspace or original PST."
                    >
                      x
                    </button>
                  </div>
                );
              })}
            </div>
          ) : null}

          <div className="workspace-mini-controls">
            <span className="workspace-summary" title={workspaceSize?.workspacePath ?? workspace.workspacePath}>
              {workspaceSummaryText}
            </span>
            <div className="workspace-menu">
              <button
                type="button"
                className="popover-trigger"
                onClick={(event) => togglePopover("workspace", event.currentTarget)}
                aria-expanded={openPopoverId === "workspace"}
                aria-controls={workspacePopoverPanelId}
                aria-haspopup="dialog"
              >
                Workspace
              </button>
              {openPopoverId === "workspace" ? (
              <div
                id={workspacePopoverPanelId}
                ref={workspacePopoverPanelRef}
                className="workspace-menu-panel"
                role="dialog"
                aria-modal="false"
                aria-label="Workspace actions"
              >
                <button
                  type="button"
                  onClick={() => {
                    closeActivePopover();
                    void revealOriginalPst();
                  }}
                  disabled={!workspace}
                  title="Reveal the original PST file in Finder."
                >
                  Reveal PST
                </button>
                <button
                  type="button"
                  onClick={() => {
                    closeActivePopover();
                    void revealWorkspace();
                  }}
                  disabled={!workspace}
                  title="Reveal the current PST QuickView workspace/cache in Finder."
                >
                  Reveal Workspace
                </button>
                <button
                  type="button"
                  onClick={() => {
                    closeActivePopover();
                    void revealBothFolders();
                  }}
                  disabled={!workspace}
                  title="Open the original PST location and workspace location in Finder."
                >
                  Open Both
                </button>
                <button
                  type="button"
                  onClick={() => {
                    closeActivePopover();
                    void revealImportLog();
                  }}
                  title="Reveal the current import log in Finder."
                >
                  Import Log
                </button>
                <div className="workspace-destructive-actions" aria-label="Destructive workspace options">
                  <span>Destructive options</span>
                  <button
                    type="button"
                    className="danger-action"
                    data-support-control="click"
                    data-support-recovery="open"
                    onClick={(event) => {
                      const returnFocus = activePopoverTriggerRef.current ?? event.currentTarget;
                      closeActivePopover();
                      void deleteCurrentWorkspace(returnFocus);
                    }}
                    disabled={!workspace || isBusy || isImporting || activeIntegrityOperationRunning}
                    title={
                      isImporting || activeIntegrityOperationRunning
                        ? "Cancel the current workspace operation before deleting this workspace."
                        : "Delete the current PST QuickView workspace/cache, including Local Trash state. The original PST is not deleted."
                    }
                  >
                    Delete Workspace
                  </button>
                </div>
              </div>
              ) : null}
            </div>
            <div className="layout-controls">
              <button
                type="button"
                className="popover-trigger"
                onClick={(event) => togglePopover("layout", event.currentTarget)}
                data-support-control="click"
                aria-expanded={openPopoverId === "layout"}
                aria-controls={layoutPopoverPanelId}
                aria-haspopup="dialog"
              >
                Layout
              </button>
              {openPopoverId === "layout" ? (
              <div
                id={layoutPopoverPanelId}
                ref={layoutPopoverPanelRef}
                className="layout-control-grid"
                role="dialog"
                aria-modal="false"
                aria-label="Layout and folder presentation"
              >
                <label>
                  <span>View</span>
                  <select
                    value={listMode}
                    data-support-control="input"
                    onChange={(event) => {
                      if (
                        messageVisibility === "trashed" &&
                        event.target.value === "conversations"
                      ) return;
                      setListMode(event.target.value as ListMode);
                      closeActivePopover();
                    }}
                    aria-describedby={
                      messageVisibility === "trashed" ? "local-trash-messages-only" : undefined
                    }
                  >
                    <option value="messages">Messages</option>
                    <option value="conversations" disabled={messageVisibility === "trashed"}>
                      Conversations
                    </option>
                  </select>
                  {messageVisibility === "trashed" ? (
                    <small id="local-trash-messages-only">Local Trash uses Messages view.</small>
                  ) : null}
                </label>
                <label>
                  <span>Mode</span>
                  <select
                    value={layoutMode}
                    data-support-control="input"
                    onChange={(event) => {
                      setLayoutMode(event.target.value as LayoutMode);
                      closeActivePopover();
                    }}
                  >
                    <option value="three_column">Three Column</option>
                    <option value="outlook">Outlook Style</option>
                  </select>
                </label>
                <label>
                  <span>List</span>
                  <select
                    value={messageListDisplayMode}
                    data-support-control="input"
                    onChange={(event) => {
                      setMessageListDisplayMode(event.target.value as MessageListDisplayMode);
                      closeActivePopover();
                    }}
                  >
                    <option value="subject_first">Subject first</option>
                    <option value="sender_first">Sender first</option>
                  </select>
                </label>
                <fieldset className="folder-hierarchy-options">
                  <legend>Folder hierarchy</legend>
                  <button
                    type="button"
                    aria-pressed={folderHierarchyMode === "simplified"}
                    data-support-control="click"
                    onClick={() => {
                      setFolderHierarchyMode("simplified");
                      closeActivePopover();
                    }}
                  >
                    Simplified hierarchy
                  </button>
                  <button
                    type="button"
                    aria-pressed={folderHierarchyMode === "original"}
                    data-support-control="click"
                    onClick={() => {
                      setFolderHierarchyMode("original");
                      closeActivePopover();
                    }}
                  >
                    Original PST hierarchy
                  </button>
                </fieldset>
                <label>
                  <span>Folder</span>
                  <input
                    type="range"
                    min={folderPaneMin}
                    max={520}
                    value={paneWidths.folder}
                    onChange={(event) => setFolderPaneWidth(Number(event.target.value))}
                  />
                </label>
                <label>
                  <span>{layoutMode === "outlook" ? "Messages height" : "Messages"}</span>
                  <input
                    type="range"
                    min={layoutMode === "outlook" ? outlookMessagePaneMin : messagePaneMin}
                    max={layoutMode === "outlook" ? 900 : 720}
                    value={layoutMode === "outlook" ? paneWidths.outlookMessage : paneWidths.message}
                    onChange={(event) => {
                      const value = Number(event.target.value);
                      if (layoutMode === "outlook") {
                        setOutlookMessagePaneHeight(value);
                      } else {
                        setMessagePaneWidth(value);
                      }
                    }}
                  />
                </label>
                <button
                  type="button"
                  className="secondary-layout-action"
                  onClick={() => {
                    setFoldersHidden((current) => !current);
                    closeActivePopover();
                  }}
                >
                  {foldersHidden ? "Show Folders" : "Hide Folders"}
                </button>
                <button
                  type="button"
                  onClick={() => {
                    collapseAllFolders();
                    closeActivePopover();
                  }}
                  disabled={!workspace || !folderTree.length}
                >
                  Collapse All
                </button>
                <button
                  type="button"
                  onClick={() => {
                    expandAllFolders();
                    closeActivePopover();
                  }}
                  disabled={!workspace || !folderTree.length}
                >
                  Expand All
                </button>
                <button
                  type="button"
                  onClick={() => {
                    resetPaneLayout();
                    closeActivePopover();
                  }}
                  title="Reset folder and message list widths."
                >
                  Reset
                </button>
              </div>
              ) : null}
            </div>
            {workspaceSize ? (
              <div className="workspace-details">
                <button
                  type="button"
                  className="popover-trigger"
                  onClick={(event) => togglePopover("workspace_details", event.currentTarget)}
                  aria-expanded={openPopoverId === "workspace_details"}
                  aria-controls={workspaceDetailsPanelId}
                  aria-haspopup="dialog"
                >
                  Details
                </button>
                {openPopoverId === "workspace_details" ? (
                <div
                  id={workspaceDetailsPanelId}
                  ref={workspaceDetailsPanelRef}
                  className="workspace-details-panel"
                  role="dialog"
                  aria-modal="false"
                  aria-label="Workspace details"
                >
                  <dl>
                  <div>
                    <dt>Mode</dt>
                    <dd>{workspaceSize.workspaceLocationLabel}</dd>
                  </div>
                  <div>
                    <dt>Import state</dt>
                    <dd>{workspaceIntegrityStateLabel(workspace.integrityState)}</dd>
                  </div>
                  <div>
                    <dt>Source</dt>
                    <dd>{workspace.sourceAvailability}</dd>
                  </div>
                  <div>
                    <dt>Conversations</dt>
                    <dd>{workspace.conversationState.replace(/_/g, " ")}</dd>
                  </div>
                  <div>
                    <dt>Last verification</dt>
                    <dd>{workspace.hasLastVerification ? "Available" : "Not run"}</dd>
                  </div>
                  <div>
                    <dt>Source PST size</dt>
                    <dd>{formatBytes(workspace.pstSizeBytes)}</dd>
                  </div>
                  <div>
                    <dt>Completed import duration</dt>
                    <dd>{completedImportDurationText(
                      workspace.completedImportDurationMs,
                      workspace.integrityState === "complete",
                    )}</dd>
                  </div>
                  <div>
                    <dt>Active import elapsed</dt>
                    <dd>{formatImportDuration(workspace.activeImportElapsedMs)}</dd>
                  </div>
                  <div>
                    <dt>Current stage elapsed</dt>
                    <dd>{formatImportDuration(workspace.currentStageElapsedMs)}</dd>
                  </div>
                  <div>
                    <dt>Last import activity</dt>
                    <dd>{formatMessageDate(workspace.lastActivityAt) || "Not recorded"}</dd>
                  </div>
                  <div>
                    <dt>Original PST</dt>
                    <dd>{workspace?.pstPath ?? ""}</dd>
                  </div>
                  <div>
                    <dt>Workspace</dt>
                    <dd>{workspaceSize.workspacePath}</dd>
                  </div>
                  <div>
                    <dt>Total</dt>
                    <dd>{formatBytes(workspaceSize.totalBytes)}</dd>
                  </div>
                  <div>
                    <dt>EML</dt>
                    <dd>{formatBytes(workspaceSize.extractedEmlBytes)}</dd>
                  </div>
                  <div>
                    <dt>SQLite</dt>
                    <dd>{formatBytes(workspaceSize.sqliteIndexBytes)}</dd>
                  </div>
                  <div>
                    <dt>Logs</dt>
                    <dd>{formatBytes(workspaceSize.logsBytes)}</dd>
                  </div>
                  <div>
                    <dt>Attachments</dt>
                    <dd>{formatBytes(workspaceSize.attachmentsBytes)}</dd>
                  </div>
                  <div>
                    <dt>Local Trash available</dt>
                    <dd>{isLocalTrashHealthy(activeLocalTrashSummary) ? "Yes" : "No"}</dd>
                  </div>
                  <div>
                    <dt>Local Trash health</dt>
                    <dd>{localTrashHealthLabel(activeLocalTrashSummary)}</dd>
                  </div>
                  <div>
                    <dt>Canonical imported messages</dt>
                    <dd>{formatCount(workspace.messageCount)}</dd>
                  </div>
                  <div>
                    <dt>Visible active messages</dt>
                    <dd>
                      {formatCount(
                        activeOpenPstSession
                          ? workspaceActiveMessageCount(activeOpenPstSession)
                          : workspace.messageCount,
                      )}
                    </dd>
                  </div>
                  <div>
                    <dt>Messages in Local Trash</dt>
                    <dd>
                      {isLocalTrashHealthy(activeLocalTrashSummary)
                        ? formatCount(activeLocalTrashSummary!.trashedMessageCount)
                        : "Unavailable"}
                    </dd>
                  </div>
                  <div>
                    <dt>Orphaned local state</dt>
                    <dd>{formatCount(activeLocalTrashSummary?.orphanedStateCount ?? 0)}</dd>
                  </div>
                  <div>
                    <dt>Local-state schema</dt>
                    <dd>{activeLocalTrashSummary?.schemaVersion ?? "Not created"}</dd>
                  </div>
                  <div>
                    <dt>Local-state overlay</dt>
                    <dd>{activeLocalTrashSummary?.sidecarPresent ? "Present" : "Not created"}</dd>
                  </div>
                  <div>
                    <dt>Local-state revision</dt>
                    <dd>{activeLocalTrashSummary?.stateRevision ?? 0}</dd>
                  </div>
                  <div>
                    <dt>Last Local Trash change</dt>
                    <dd>
                      {formatMessageDate(activeLocalTrashSummary?.lastChangedAt ?? "") ||
                        "Not recorded"}
                    </dd>
                  </div>
                  <div>
                    <dt>Source/workspace fingerprint</dt>
                    <dd>
                      {activeLocalTrashSummary?.fingerprintMatch ? "Matches" : "Not verified"}
                    </dd>
                  </div>
                  <div>
                    <dt>Last local-state error code</dt>
                    <dd>{activeLocalTrashSummary?.lastErrorCode ?? "None"}</dd>
                  </div>
                  </dl>
                  {canResetLocalTrashState(activeLocalTrashSummary) ? (
                    <div className="workspace-local-trash-reset">
                      <p>{localTrashResetDisclosure}</p>
                      <button
                        type="button"
                        className="danger-action"
                        onClick={(event) => requestLocalTrashReset(event.currentTarget)}
                        disabled={
                          localTrashMutationPending ||
                          isBusy ||
                          isImporting ||
                          activeIntegrityOperationRunning
                        }
                      >
                        Reset Local Trash State
                      </button>
                    </div>
                  ) : !isLocalTrashHealthy(activeLocalTrashSummary) ? (
                    <div className="workspace-local-trash-reset" role="status">
                      <p>
                        Reset is unavailable because PST QuickView could not validate a safe,
                        exact Local Trash overlay target. Normal message visibility remains
                        fail-open.
                      </p>
                    </div>
                  ) : null}
                </div>
                ) : null}
              </div>
            ) : null}
          </div>
        </section>
      ) : null}

      {workspace ? (
        <section
          className={`workspace-integrity-banner workspace-integrity-${workspace.integrityClassification}`}
          aria-busy={activeIntegrityOperationRunning}
          aria-label="Import integrity"
        >
          <div className="workspace-integrity-summary">
            <strong>
              {workspace.resumeDetails?.pstBasename || pstDisplayName(workspace)} -{" "}
              {workspace.integrityClassification === "complete"
                ? "Import complete"
                : workspace.integrityClassification === "unsupported"
                  ? "Unsupported workspace state"
                  : "Import incomplete"}
            </strong>
            {workspace.resumeDetails ? (
              <span>{resumeProgressDescription(workspace.resumeDetails, formatCount)}</span>
            ) : (
              <span>State: {workspaceIntegrityStateLabel(workspace.integrityState)}</span>
            )}
            {sourceAvailabilityMessage ? <span>{sourceAvailabilityMessage}</span> : null}
            {activeIntegrityOperationRunning && activeIntegrityOperationKind ? (
              <span role="status" aria-live="polite" aria-atomic="true">
                {integrityOperationRunningLabel(activeIntegrityOperationKind)}
              </span>
            ) : null}
          </div>

          {workspace.resumeDetails ? (
            <dl className="workspace-resume-details">
              <div>
                <dt>Last completed stage</dt>
                <dd>{workspaceIntegrityStateLabel(workspace.resumeDetails.lastCompletedStage)}</dd>
              </div>
              <div>
                <dt>Interrupted stage</dt>
                <dd>{workspaceIntegrityStateLabel(workspace.resumeDetails.interruptedStage)}</dd>
              </div>
              <div>
                <dt>Discovered</dt>
                <dd>{formatCount(workspace.resumeDetails.discoveredCandidates)}</dd>
              </div>
              <div>
                <dt>Indexed</dt>
                <dd>{formatCount(workspace.resumeDetails.canonicalIndexed)}</dd>
              </div>
              <div>
                <dt>Remaining</dt>
                <dd>
                  {workspace.resumeDetails.remainingCandidates == null
                    ? "Unknown"
                    : formatCount(workspace.resumeDetails.remainingCandidates)}
                </dd>
              </div>
              <div>
                <dt>Last successful update</dt>
                <dd>{formatMessageDate(workspace.resumeDetails.lastSuccessfulUpdate) || "Unknown"}</dd>
              </div>
              <div>
                <dt>Elapsed</dt>
                <dd>{formatElapsedDuration(workspace.resumeDetails.elapsedDurationMs)}</dd>
              </div>
              <div>
                <dt>Interrupted stage elapsed</dt>
                <dd>{formatImportDuration(workspace.resumeDetails.stageElapsedDurationMs)}</dd>
              </div>
              <div>
                <dt>Last activity</dt>
                <dd>{formatMessageDate(workspace.resumeDetails.lastActivityAt) || "Not recorded"}</dd>
              </div>
              <div>
                <dt>Why incomplete</dt>
                <dd>{workspace.resumeDetails.reason}</dd>
              </div>
            </dl>
          ) : null}

          <div className="workspace-integrity-actions" role="group" aria-label="Import integrity actions">
            {integrityActions.includes("resume_indexing") ? (
              <button
                type="button"
                onClick={() => void resumeWorkspaceIndexing(workspace.id)}
                disabled={isBusy || isImporting || anyWorkspaceIntegrityOperationRunning}
              >
                {activeIntegrityOperationRunning && activeIntegrityOperationKind === "resume_indexing"
                  ? integrityOperationRunningLabel("resume_indexing")
                  : "Resume Indexing"}
              </button>
            ) : null}
            {integrityActions.includes("verify") ? (
              <button
                type="button"
                className={workspace.integrityClassification === "complete" ? "primary-action" : undefined}
                onClick={(event) => void verifyWorkspaceImport(workspace.id, event.currentTarget)}
                disabled={isBusy || isImporting || anyWorkspaceIntegrityOperationRunning}
              >
                {activeIntegrityOperationRunning && activeIntegrityOperationKind === "verify"
                  ? integrityOperationRunningLabel("verify")
                  : workspace.integrityClassification === "complete"
                    ? "Verify Import"
                    : "Verify"}
              </button>
            ) : null}
            {integrityActions.includes("view_last_verification") ? (
              <button
                type="button"
                onClick={(event) =>
                  void viewLastWorkspaceVerification(workspace.id, event.currentTarget)
                }
                disabled={isBusy || isImporting || anyWorkspaceIntegrityOperationRunning}
              >
                {activeIntegrityOperationRunning &&
                activeIntegrityOperationKind === "load_last_verification"
                  ? integrityOperationRunningLabel("load_last_verification")
                  : "View Last Verification"}
              </button>
            ) : null}
            {integrityActions.includes("repair_index") ? (
              <button
                type="button"
                data-support-control="click"
                data-support-recovery="open"
                aria-describedby={repairIndexDescriptionId}
                onClick={(event) => requestRepairWorkspaceIndex(workspace.id, event.currentTarget)}
                disabled={isBusy || isImporting || anyWorkspaceIntegrityOperationRunning}
              >
                {activeIntegrityOperationRunning && activeIntegrityOperationKind === "repair_index"
                  ? integrityOperationRunningLabel("repair_index")
                  : "Repair Index"}
              </button>
            ) : null}
            {integrityActions.includes("rebuild_conversations") ? (
              <button
                type="button"
                onClick={() => void rebuildWorkspaceConversations(workspace.id)}
                disabled={isBusy || isImporting || anyWorkspaceIntegrityOperationRunning}
              >
                {activeIntegrityOperationRunning &&
                activeIntegrityOperationKind === "rebuild_conversations"
                  ? integrityOperationRunningLabel("rebuild_conversations")
                  : "Rebuild Conversations"}
              </button>
            ) : null}
            {activeIntegrityOperationRunning &&
            activeIntegrityOperationKind &&
            integrityOperationIsCancellable(activeIntegrityOperationKind) ? (
              <button
                type="button"
                className="danger-action"
                onClick={() => void cancelImport(activeIntegrityProgress)}
                disabled={activeIntegrityProgress == null}
              >
                Cancel {integrityOperationLabel(activeIntegrityOperationKind)}
              </button>
            ) : null}
          </div>
          {workspace.integrityClassification === "incomplete" && workspace.canRepairIndex ? (
            <p id={repairIndexDescriptionId} className="workspace-integrity-safety-note">
              Repair Index reuses local extracted files. It does not rerun ReadPST, and the source PST remains read-only.
            </p>
          ) : null}
        </section>
      ) : null}

      {workspace && verificationReportOpen && activeVerificationReport ? (
        <section
          className="verification-report"
          role="region"
          aria-labelledby={verificationReportTitleId}
          onKeyDown={(event) => {
            if (event.key !== "Escape") return;
            event.preventDefault();
            closeVerificationReport(workspace.id);
          }}
        >
          <header>
            <div>
              <h2 id={verificationReportTitleId} ref={verificationReportHeadingRef} tabIndex={-1}>
                Import Verification Report
              </h2>
              <p role="status" aria-live="polite" aria-atomic="true">
                {verificationHeadline(activeVerificationReport)}
              </p>
            </div>
            <button type="button" onClick={() => closeVerificationReport(workspace.id)}>
              Close
            </button>
          </header>
          <dl className="verification-report-grid">
            <div><dt>Level</dt><dd>{verificationLevelLabel(activeVerificationReport.verificationLevel)}</dd></div>
            <div><dt>Result</dt><dd>{verificationResultLabel(activeVerificationReport.result)}</dd></div>
            <div><dt>Workspace state</dt><dd>{workspaceIntegrityStateLabel(activeVerificationReport.workspaceState)}</dd></div>
            <div><dt>Discovered candidates</dt><dd>{formatCount(activeVerificationReport.discoveredCandidates)}</dd></div>
            <div><dt>Canonical messages</dt><dd>{formatCount(activeVerificationReport.canonicalMessages)}</dd></div>
            <div><dt>Inserted messages</dt><dd>{formatCount(activeVerificationReport.insertedMessages)}</dd></div>
            <div><dt>Skipped existing</dt><dd>{formatCount(activeVerificationReport.skippedExistingMessages)}</dd></div>
            <div><dt>Parse failures</dt><dd>{formatCount(activeVerificationReport.parseFailures)}</dd></div>
            <div><dt>Attachments</dt><dd>{formatCount(activeVerificationReport.attachments)}</dd></div>
            <div><dt>Folders</dt><dd>{formatCount(activeVerificationReport.folders)}</dd></div>
            <div><dt>FTS rows</dt><dd>{formatCount(activeVerificationReport.ftsRows)}</dd></div>
            <div><dt>Conversations</dt><dd>{activeVerificationReport.conversationState.replace(/_/g, " ")}</dd></div>
            <div><dt>Missing canonical records</dt><dd>{formatCount(activeVerificationReport.missingCanonicalRecords)}</dd></div>
            <div><dt>Unexpected canonical records</dt><dd>{formatCount(activeVerificationReport.unexpectedCanonicalRecords)}</dd></div>
            <div><dt>Possible duplicate groups</dt><dd>{formatCount(activeVerificationReport.possibleDuplicateGroups)}</dd></div>
            <div><dt>Source integrity</dt><dd>{activeVerificationReport.sourceIntegrity.replace(/_/g, " ")}</dd></div>
            <div><dt>Started</dt><dd>{formatMessageDate(activeVerificationReport.startedAt) || "Unknown"}</dd></div>
            <div><dt>Completed</dt><dd>{formatMessageDate(activeVerificationReport.completedAt) || "Unknown"}</dd></div>
            <div><dt>Elapsed</dt><dd>{formatElapsedDuration(activeVerificationReport.elapsedDurationMs)}</dd></div>
            <div>
              <dt>Completed import duration</dt>
              <dd>{completedImportDurationText(
                activeVerificationReport.completedImportDurationMs,
                activeVerificationReport.workspaceState === "complete",
              )}</dd>
            </div>
          </dl>
          {possibleDuplicateSummary(activeVerificationReport.possibleDuplicateGroups) ? (
            <p className="verification-warning">
              {possibleDuplicateSummary(activeVerificationReport.possibleDuplicateGroups)} No messages were hidden, merged, suppressed, or deleted.
            </p>
          ) : null}
          {activeVerificationReport.warnings.length ? (
            <div className="verification-report-list">
              <h3>Warnings</h3>
              <ul>{activeVerificationReport.warnings.map((warning) => <li key={warning}>{warning}</li>)}</ul>
            </div>
          ) : null}
          {activeVerificationReport.recommendedActions.length ? (
            <div className="verification-report-list">
              <h3>Recommended actions</h3>
              <ul>{activeVerificationReport.recommendedActions.map((action) => <li key={action}>{action}</li>)}</ul>
            </div>
          ) : null}
          {activeVerificationReport.possibleDuplicateGroupDetails.length ? (
            <details className="verification-record-details">
              <summary>Opaque diagnostic details</summary>
              <p>These identifiers and signals do not expose message content or record paths.</p>
              {activeVerificationReport.possibleDuplicateGroupDetails.map((group) => (
                <div key={group.opaqueId}>
                  <strong>{group.opaqueId}</strong>
                  <span>{group.signals.join(", ")}</span>
                </div>
              ))}
            </details>
          ) : null}
        </section>
      ) : null}

      {(workspace && repairConfirmOpen) || visiblePendingPlannedRepair ? (
        <div className="repair-confirm-backdrop">
          <section
            className="repair-confirm-dialog"
            role="dialog"
            aria-modal="true"
            aria-labelledby={repairConfirmTitleId}
            aria-describedby={repairConfirmDescriptionId}
            ref={repairConfirmDialogRef}
            onKeyDown={handleRepairConfirmationKeyDown}
          >
            <h2 id={repairConfirmTitleId}>Repair this workspace index?</h2>
            <p id={repairConfirmDescriptionId}>
              Repair Index builds a new temporary database from this workspace's existing extracted files and verifies it before replacement. It does not rerun ReadPST or modify the source PST or extraction output. The current index remains active unless the verified replacement succeeds.
            </p>
            <div className="repair-confirm-actions">
              <button
                type="button"
                ref={repairConfirmCancelRef}
                data-support-control="click"
                data-support-recovery="cancel"
                onClick={closeVisibleRepairConfirmation}
              >
                Cancel
              </button>
              <button
                type="button"
                className="primary-action"
                data-support-recovery="confirm-local"
                onClick={(event) => {
                  if (event.nativeEvent.isTrusted) confirmVisibleRepair();
                }}
              >
                Repair Index
              </button>
            </div>
          </section>
        </div>
      ) : null}

      {isSearchActive ? (
        <section className="search-summary">
          <strong>{isSearching ? "Searching..." : resultSummaryText}</strong>
          <span className="visually-hidden" role="status" aria-live="polite" aria-atomic="true">
            {isSearching
              ? "Searching."
              : appliedSearch.listMode === "conversations"
                ? "Conversation results ready."
                : "Message results ready."}
          </span>
          <div className="filter-chips">
            {activeFilterChips.map((chip) => (
              <button
                type="button"
                className="filter-chip"
                key={chip.id}
                onClick={() => removeSearchChip(chip.id)}
                disabled={isBusy}
                aria-label={chip.removeLabel}
                title={chip.text}
              >
                <span className="filter-chip-text">{chip.text}</span>
                <span className="filter-chip-remove" aria-hidden="true">
                  x
                </span>
              </button>
            ))}
            {appliedIsCrossPstSearch
              ? workspaceSearchCounts
                  .filter((count) => count.count > 0)
                  .map((count) => (
                    <span className="result-count-chip" key={count.workspaceId}>
                      {count.pstDisplayName}: {formatCount(count.count)}
                    </span>
                  ))
              : null}
          </div>
          <div className="search-summary-actions">
            {canClearAppliedSearch && !advancedSearchOpen ? (
              <button
                type="button"
                onClick={clearAllSearch}
                disabled={isBusy}
                aria-label="Clear all search filters and sorting"
              >
                Clear All
              </button>
            ) : null}
          </div>
        </section>
      ) : null}

      {progress ? (
        <section
          className={`import-progress-panel${progressIsBackground ? " import-progress-background" : ""}${progress.status !== "active" ? ` import-progress-${progress.status}` : ""}`}
          aria-labelledby={`import-progress-heading-${progress.operationId}`}
          aria-busy={progress.status === "active"}
        >
          <span
            className="visually-hidden"
            role="status"
            aria-live="polite"
            aria-atomic="true"
            key={progressAnnouncementKey}
          >
            {progressAnnouncement}
          </span>

          <div className="import-progress-header">
            <div className="import-progress-identity">
              <h2 id={`import-progress-heading-${progress.operationId}`}>
                {importHeadline(progress, progressIsBackground, progressIdentityPeers)}
              </h2>
              {progressIdentity ? (
                <p>
                  {progressIdentity.sourceSize} · {progressIdentity.storageMode} ·{" "}
                  {progressIdentity.operation}
                </p>
              ) : null}
            </div>
            <div className="import-progress-actions">
              {progressIsBackground && progressWorkspaceIsOpen ? (
                <button
                  type="button"
                  onClick={() => void activateWorkspaceTab(progress.workspaceId)}
                  disabled={restoringPlannedWorkspace}
                >
                  View Workspace
                </button>
              ) : null}
              {progressCanCancel ? (
                <button
                  type="button"
                  className="danger-action"
                  onClick={() => void cancelImport(progress)}
                >
                  {progressCancelLabel}
                </button>
              ) : null}
              <div className="import-progress-details">
                <button
                  type="button"
                  className="popover-trigger"
                  onClick={(event) => togglePopover("import_progress", event.currentTarget)}
                  aria-expanded={openPopoverId === "import_progress"}
                  aria-controls={importProgressDetailsPanelId}
                  aria-haspopup="dialog"
                >
                  Details
                </button>
                {openPopoverId === "import_progress" ? (
                <div
                  id={importProgressDetailsPanelId}
                  ref={importProgressDetailsPanelRef}
                  className="import-progress-details-panel"
                  role="dialog"
                  aria-modal="false"
                  aria-label="Import progress details"
                >
                  <dl>
                  <div>
                    <dt>Source</dt>
                    <dd>{progressIdentity?.displayName ?? progress.sourceBasename}</dd>
                  </div>
                  <div>
                    <dt>Exact source size</dt>
                    <dd>{formatCount(progress.sourceSizeBytes)} bytes</dd>
                  </div>
                  <div>
                    <dt>Workspace mode</dt>
                    <dd>{progressIdentity?.storageMode ?? progress.workspaceStorageMode}</dd>
                  </div>
                  <div>
                    <dt>Operation</dt>
                    <dd>{progressIdentity?.operation ?? progress.operationKind}</dd>
                  </div>
                  <div>
                    <dt>Active import elapsed</dt>
                    <dd>{formatImportDuration(progressActiveElapsedMs)}</dd>
                  </div>
                  <div>
                    <dt>Current operation elapsed</dt>
                    <dd>{formatImportDuration(progressOperationElapsedMs)}</dd>
                  </div>
                  <div>
                    <dt>Stage elapsed</dt>
                    <dd>{formatImportDuration(progressStageElapsedMs)}</dd>
                  </div>
                  {progressAllCounters.map((row) => (
                    <div key={row.key}>
                      <dt>{row.label}</dt>
                      <dd>
                        {row.displayValue}
                        {row.key === "outputBytesObserved" || row.key === "workspaceBytesWritten"
                          ? ` (${formatCount(row.value)} bytes)`
                          : ""}
                      </dd>
                    </div>
                  ))}
                  </dl>
                </div>
                ) : null}
              </div>
            </div>
          </div>

          {progressIsBackground ? (
            <p className="import-progress-current-stage">
              <span aria-hidden="true">●</span>
              <span>{importStageLabel(progress.stage)}</span>
            </p>
          ) : (
            <ol className="import-stage-tracker" aria-label="Import stages">
              {progressTracker.map((item) => (
                <li key={item.stage} className={`import-stage-${item.state}`}>
                  <span className="import-stage-marker" aria-hidden="true">
                    {item.state === "completed"
                      ? "✓"
                      : item.state === "active"
                        ? "●"
                        : item.state === "interrupted"
                          ? "!"
                          : "○"}
                  </span>
                  <span aria-hidden="true">{item.label}</span>
                  <span className="visually-hidden">{item.accessibleText}</span>
                </li>
              ))}
            </ol>
          )}

          <div className="import-progress-body">
            <p className="import-progress-explanation">
              {progressTerminal?.detail ?? progressExplanation}
            </p>
            {progressHealth && progressHealth !== progressExplanation ? (
              <p className="import-progress-health">{progressHealth}</p>
            ) : null}

            {progressSemantics?.kind === "determinate" && progressFillPercent != null ? (
              <div
                className="import-stage-progress-track"
                role="progressbar"
                aria-label={progressSemantics.ariaLabel}
                aria-valuemin={progressSemantics.ariaValueMin}
                aria-valuemax={progressSemantics.ariaValueMax}
                aria-valuenow={progressSemantics.ariaValueNow}
              >
                <div
                  className="import-stage-progress-fill"
                  style={{ width: `${progressFillPercent}%` }}
                />
              </div>
            ) : progressSemantics?.kind === "indeterminate" ? (
              <div className="import-progress-indeterminate">
                <span className="import-progress-spinner" aria-hidden="true" />
                <span>{progressSemantics.ariaLabel}</span>
              </div>
            ) : null}

            {progressUnit || progressStageCounters.length ? (
              <div className="import-progress-counts">
                {progressUnit ? <strong>{progressUnit}</strong> : null}
                {progressStageCounters.map((row) => (
                  <span key={row.key}>
                    {row.label}: {row.displayValue}
                  </span>
                ))}
              </div>
            ) : null}

            {progress.warningCodes.length ? (
              <ul className="import-progress-warnings">
                {progress.warningCodes.map((warning) => (
                  <li key={warning}>{importWarningText(warning)}</li>
                ))}
              </ul>
            ) : null}

            {progress.status === "active" ? (
              <p className="import-progress-timing" aria-label="Import timing">
                <span>Elapsed: {formatImportDuration(progressDisplayedElapsedMs)}</span>
                <span>This stage: {formatImportDuration(progressStageElapsedMs)}</span>
                {progressLastActivity ? <span>{progressLastActivity}</span> : null}
                {progressEta?.text ? <span>{progressEta.text}</span> : null}
              </p>
            ) : null}
          </div>
        </section>
      ) : null}

      {workspace && activeWorkspaceOperationRunning && !activeIntegrityOperationRunning ? (
        <section className="notice-box" role="status">
          Cancel the running workspace operation before deleting this workspace.
        </section>
      ) : null}

      {displayedError ? (
        <section className="error-box" role="alert">
          <strong>{errorSummary(displayedError)}</strong>
          {displayedSetupCommand ? <code>{displayedSetupCommand}</code> : null}
          <div className="error-actions">
            <details>
              <summary>Details</summary>
              <pre>{displayedError}</pre>
            </details>
            <button type="button" onClick={() => void copyDisplayedError(displayedError)}>
              Copy Error
            </button>
          </div>
        </section>
      ) : null}

      {displayedNotice ? (
        <section className="notice-toast operation-notice-toast" role="status">
          <span>{displayedNotice}</span>
          <button
            type="button"
            aria-label="Dismiss status message"
            onClick={() => {
              if (workspaceOperationNotice && visibleWorkspaceOperation) {
                const targetWorkspaceId = activeWorkspaceOperation
                  ? workspace?.id
                  : foregroundOperationWorkspaceId;
                if (targetWorkspaceId) dismissWorkspaceOperation(targetWorkspaceId);
              } else {
                setNotice(null);
              }
            }}
          >
            Dismiss
          </button>
        </section>
      ) : null}

      {workspaceDeleteToast ? (
        <section className="notice-toast delete-success-toast" role="status">
          <div>
            <strong>{workspaceDeleteToast.message}</strong>
            <details>
              <summary>Details</summary>
              <dl>
                <div>
                  <dt>attempted_path</dt>
                  <dd>{workspaceDeleteToast.result.attemptedPath}</dd>
                </div>
                <div>
                  <dt>deleted</dt>
                  <dd>{String(workspaceDeleteToast.result.deleted)}</dd>
                </div>
                <div>
                  <dt>exists_after</dt>
                  <dd>{String(workspaceDeleteToast.result.existsAfter)}</dd>
                </div>
                <div>
                  <dt>removed_empty_parent</dt>
                  <dd>{String(workspaceDeleteToast.result.removedEmptyParent)}</dd>
                </div>
              </dl>
            </details>
          </div>
          <button
            type="button"
            aria-label="Dismiss workspace deletion result"
            onClick={() => setWorkspaceDeleteToast(null)}
          >
            Dismiss
          </button>
        </section>
      ) : null}

      {deleteStatuses.length && !deleteResult ? (
        <section className="delete-status-box" role="status">
          <strong>Delete status</strong>
          <ol>
            {deleteStatuses.map((status, index) => (
              <li key={`${status}-${index}`}>{status}</li>
            ))}
          </ol>
        </section>
      ) : null}

      {localTrashStatus || localTrashError || localTrashUndo ? (
        <section
          className={`local-trash-status ${localTrashError ? "local-trash-status-error" : ""}`}
          role="status"
          aria-live="polite"
          aria-atomic="true"
        >
          <div>
            <strong>{localTrashError ? "Local Trash action failed" : "Local Trash"}</strong>
            <span>{localTrashError ?? localTrashStatus ?? "A message was moved."}</span>
          </div>
          <div className="local-trash-status-actions">
            {localTrashUndo ? (
              <button
                type="button"
                onClick={() => void undoMostRecentLocalTrashMove()}
                disabled={
                  !canUndoLocalTrashMove ||
                  localTrashMutationPending ||
                  isBusy ||
                  isImporting ||
                  activeIntegrityOperationRunning
                }
                title={
                  canUndoLocalTrashMove
                    ? "Restore the most recently moved message."
                    : "Undo is no longer available because Local Trash state changed."
                }
              >
                Undo
              </button>
            ) : null}
            <button
              type="button"
              onClick={() => {
                setLocalTrashStatus(null);
                setLocalTrashError(null);
                setLocalTrashUndo(null);
                localTrashUndoMessageRef.current = null;
              }}
              disabled={localTrashMutationPending}
            >
              Dismiss
            </button>
          </div>
        </section>
      ) : null}

      {localTrashResetConfirmOpen && workspace ? (
        <div className="confirmation-backdrop">
          <section
            ref={localTrashResetDialogRef}
            className="confirmation-modal"
            role="dialog"
            aria-modal="true"
            aria-labelledby={localTrashResetTitleId}
            aria-describedby={localTrashResetDescriptionId}
            onKeyDown={handleLocalTrashResetKeyDown}
          >
            <h2 id={localTrashResetTitleId}>Reset unhealthy Local Trash state?</h2>
            <div id={localTrashResetDescriptionId}>
              <p>{localTrashResetDisclosure}</p>
              <p>
                This clears only this workspace&apos;s versioned local-state overlay and its Local
                Trash organization. It removes no message content and does not reclaim storage.
              </p>
            </div>
            <div className="confirmation-actions">
              <button
                ref={localTrashResetCancelRef}
                type="button"
                onClick={closeLocalTrashResetConfirmation}
                disabled={localTrashMutationPending}
              >
                Cancel
              </button>
              <button
                type="button"
                className="danger-action"
                onClick={() => void confirmLocalTrashReset()}
                disabled={
                  localTrashMutationPending ||
                  isBusy ||
                  isImporting ||
                  activeIntegrityOperationRunning
                }
              >
                Reset Local Trash State
              </button>
            </div>
          </section>
        </div>
      ) : null}

      {(deleteConfirmOpen && workspace) || visiblePendingPlannedDelete ? (
        <div className="repair-confirm-backdrop">
          <section
            className="repair-confirm-dialog delete-confirm-box"
            role="dialog"
            aria-modal="true"
            aria-label="Confirm workspace delete"
            ref={deleteConfirmDialogRef}
            onKeyDown={handleDeleteConfirmationKeyDown}
          >
            <div>
              <strong>Delete this workspace?</strong>
              <p>{visiblePendingPlannedDelete?.existingWorkspace.workspacePath ?? workspace?.workspacePath}</p>
              <p>
                The workspace cache, including its Local Trash sidecar and app-only organization,
                will be removed. The original PST will not be deleted.
              </p>
            </div>
            <div className="delete-confirm-actions">
              <button
                type="button"
                ref={deleteConfirmCancelRef}
                data-support-control="click"
                data-support-recovery="cancel"
                onClick={closeVisibleDeleteConfirmation}
                disabled={isBusy}
              >
                Cancel
              </button>
              <button
                type="button"
                className="danger-action"
                data-support-recovery="confirm-local"
                onClick={(event) => {
                  if (event.nativeEvent.isTrusted) confirmVisibleDelete();
                }}
                disabled={isBusy}
              >
                Yes, delete workspace
              </button>
            </div>
          </section>
        </div>
      ) : null}

      {deleteResult ? (
        <section
          className={`delete-result-box ${deleteResolved ? "delete-result-success" : "delete-result-failure"}`}
          role={deleteResolved ? "status" : "alert"}
        >
          <div className="delete-result-summary">
            <strong>
              {deleteResult.alreadyMissing
                ? "Workspace folder was already missing. Refreshed workspace list."
                : deleteResolved
                  ? "Deleted workspace cache and its Local Trash state. Original PST was not deleted."
                  : "Failed to delete workspace."}
            </strong>
            {!deleteResolved ? (
              <>
                <span>{deleteResult.attemptedPath}</span>
                <span>{deleteResult.error ?? "Workspace still exists after deletion attempt."}</span>
              </>
            ) : null}
          </div>
          <details>
            <summary>Details</summary>
            <dl>
              <div>
                <dt>attempted_path</dt>
                <dd>{deleteResult.attemptedPath}</dd>
              </div>
              <div>
                <dt>existed_before</dt>
                <dd>{String(deleteResult.existedBefore)}</dd>
              </div>
              <div>
                <dt>marker_existed</dt>
                <dd>{String(deleteResult.markerExisted)}</dd>
              </div>
              <div>
                <dt>deleted</dt>
                <dd>{String(deleteResult.deleted)}</dd>
              </div>
              <div>
                <dt>exists_after</dt>
                <dd>{String(deleteResult.existsAfter)}</dd>
              </div>
              <div>
                <dt>removed_empty_parent</dt>
                <dd>{String(deleteResult.removedEmptyParent)}</dd>
              </div>
              <div>
                <dt>parent_path</dt>
                <dd>{deleteResult.parentPath ?? ""}</dd>
              </div>
              <div>
                <dt>error</dt>
                <dd>{deleteResult.error ?? ""}</dd>
              </div>
            </dl>
            {deleteResult.remainingEntries.length ? (
              <div className="remaining-entries">
                <span>remaining_entries</span>
                <ul>
                  {deleteResult.remainingEntries.map((entry) => (
                    <li key={entry}>{entry}</li>
                  ))}
                </ul>
              </div>
            ) : null}
          </details>
        </section>
      ) : null}

      {visiblePendingPreflightOpen ? (
        <section className="preflight-box" role="dialog" aria-label="Workspace preflight warning">
          <div>
            <strong>Check workspace space before import</strong>
            <p>
              Importing creates a local searchable cache. The original PST remains read-only and
              will not be modified.
            </p>
          </div>
          {visiblePendingPreflightOpen.plan.preflight.warnings.length ? (
            <ul className="preflight-warnings">
              {visiblePendingPreflightOpen.plan.preflight.warnings.map((warning) => (
                <li key={warning}>{warning}</li>
              ))}
            </ul>
          ) : null}
          <dl className="preflight-details">
            <div>
              <dt>Original PST</dt>
              <dd>{visiblePendingPreflightOpen.plan.preflight.originalPstPath}</dd>
            </div>
            <div>
              <dt>Workspace/cache</dt>
              <dd>{visiblePendingPreflightOpen.plan.preflight.workspacePath}</dd>
            </div>
            <div>
              <dt>Mode</dt>
              <dd>{visiblePendingPreflightOpen.plan.preflight.workspaceLocationLabel}</dd>
            </div>
            <div>
              <dt>PST size</dt>
              <dd>{formatBytes(visiblePendingPreflightOpen.plan.preflight.pstSizeBytes)}</dd>
            </div>
            <div>
              <dt>Estimated required</dt>
              <dd>{formatBytes(visiblePendingPreflightOpen.plan.preflight.estimatedRequiredBytes)}</dd>
            </div>
            <div>
              <dt>Available</dt>
              <dd>{formatOptionalBytes(visiblePendingPreflightOpen.plan.preflight.availableDiskBytes)}</dd>
            </div>
          </dl>
          <div className="preflight-actions">
            <button
              type="button"
              className="primary-action"
              onClick={() => void continuePendingPreflight()}
              disabled={isBusy || !canContinueAfterPreflight(visiblePendingPreflightOpen.plan.preflight)}
              title={
                canContinueAfterPreflight(visiblePendingPreflightOpen.plan.preflight)
                  ? undefined
                  : "The original PST must be readable and the workspace parent must be writable before import can start."
              }
            >
              {preflightPrimaryLabel(visiblePendingPreflightOpen.plan.preflight)}
            </button>
            {shouldOfferAppSupportFallback(visiblePendingPreflightOpen) ? (
              <button
                type="button"
                onClick={() => void chooseAppSupportForPendingPreflight()}
                disabled={isBusy}
              >
                Choose Local Mac App Support
              </button>
            ) : null}
            <button
              type="button"
              onClick={() => cancelPendingPstOpen(visiblePendingPreflightOpen.plan.pstPath)}
              disabled={isBusy}
            >
              Cancel
            </button>
          </div>
        </section>
      ) : null}

      {visiblePendingOpenPlan ? (
        <section className="duplicate-box" role="dialog" aria-label="Existing workspace found">
          <div>
            <strong>{pendingOpenHeading(visiblePendingOpenPlan)}</strong>
            <p>
              Selected location: {visiblePendingOpenPlan.selectedWorkspaceLocationLabel} -{" "}
              {visiblePendingOpenPlan.selectedWorkspacePath}
            </p>
            {visiblePendingOpenPlan.fallbackWarning ? (
              <p>{visiblePendingOpenPlan.fallbackWarning}</p>
            ) : null}
          </div>
          <div className="duplicate-actions">
            {pendingSelectedWorkspace ? (
              <div className="duplicate-section">
                <strong>Selected location workspace</strong>
                {renderWorkspaceChoice(visiblePendingOpenPlan, pendingSelectedWorkspace)}
              </div>
            ) : null}
            {pendingCompleteWorkspaces.length ? (
              <div className="duplicate-section">
                <strong>Complete indexed workspaces</strong>
                {pendingCompleteWorkspaces.map((existing) =>
                  renderWorkspaceChoice(visiblePendingOpenPlan, existing),
                )}
              </div>
            ) : null}
            {pendingIncompleteWorkspaces.length ? (
              <div className="duplicate-section">
                <strong>Incomplete import workspaces</strong>
                {pendingIncompleteWorkspaces.map((existing) =>
                  renderWorkspaceChoice(visiblePendingOpenPlan, existing),
                )}
              </div>
            ) : null}
            <button
              type="button"
              onClick={() =>
                void beginPlannedPstOpen(visiblePendingOpenPlan, null, true, "import")
              }
              disabled={
                isBusy ||
                visiblePendingOpenPlan.existingWorkspaces.some(
                  (existing) =>
                    existing.workspacePath === visiblePendingOpenPlan.selectedWorkspacePath,
                )
              }
            >
              Create workspace in selected location
            </button>
            <button
              type="button"
              onClick={() => cancelPendingPstOpen(visiblePendingOpenPlan.pstPath)}
              disabled={isBusy}
            >
              Cancel
            </button>
          </div>
        </section>
      ) : null}

      <section className={paneLayoutClassName} ref={paneLayoutRef} style={paneLayoutStyle}>
        {!foldersHidden ? (
          <aside className="folders-pane">
            <div className="pane-heading folders-heading">
              <div className="folders-heading-title">
                <h2>Folders</h2>
                <button type="button" onClick={() => setFoldersHidden(true)} title="Hide folder pane.">
                  Hide
                </button>
              </div>
              <div className="folders-heading-controls">
                <label className="subfolder-toggle">
                  <input
                    type="checkbox"
                    checked={includeSubfolders}
                    onChange={(event) => toggleIncludeSubfolders(event.target.checked)}
                    disabled={!workspace || messageVisibility === "trashed"}
                  />
                  <span
                    title={
                      messageVisibility === "trashed"
                        ? "Local Trash is a reserved view and is not scoped by PST folders."
                        : "Include messages from child folders"
                    }
                  >
                    Subfolders
                  </span>
                </label>
                <button type="button" onClick={collapseAllFolders} disabled={!workspace}>
                  Collapse
                </button>
                <button type="button" onClick={expandAllFolders} disabled={!workspace}>
                  Expand
                </button>
              </div>
            </div>
            <div className="folder-tree" data-support-control="scroll">
              {workspace ? (
                searchScope === "all_open" && openPstSessions.length > 0 ? (
                  <>
                    <button
                      type="button"
                      className={`folder-row all-mail-row ${
                        messageVisibility === "active" &&
                        allOpenFolderSelection.workspaceId == null
                          ? "selected"
                          : ""
                      }`}
                      onClick={() => {
                        showActiveMessages();
                        setAllOpenFolderSelection({ workspaceId: null, folderId: null });
                      }}
                      title="All indexed messages across open PSTs"
                      aria-current={
                        messageVisibility === "active" &&
                        allOpenFolderSelection.workspaceId == null
                          ? "true"
                          : undefined
                      }
                    >
                      <span className="folder-name">All Open PSTs</span>
                      <span className="folder-count">
                        {formatCount(
                          openPstSessions.reduce(
                            (total, session) => total + workspaceActiveMessageCount(session),
                            0,
                          ),
                        )}
                      </span>
                    </button>
                    <button
                      type="button"
                      className={`folder-row local-trash-row ${
                        messageVisibility === "trashed" &&
                        localTrashAllOpenWorkspaceId == null
                          ? "selected"
                          : ""
                      }`}
                      onClick={() => showLocalTrash(null)}
                      title={localTrashStorageDisclosure}
                      aria-label={`Local Trash, app-only view across All Open PSTs, ${
                        allOpenLocalTrashSummary.available
                          ? `${formatCount(allOpenLocalTrashSummary.trashedMessageCount)} messages`
                          : "count unavailable"
                      }`}
                      aria-current={
                        messageVisibility === "trashed" &&
                        localTrashAllOpenWorkspaceId == null
                          ? "true"
                          : undefined
                      }
                    >
                      <span className="folder-name">Local Trash</span>
                      <span className="folder-count">
                        {allOpenLocalTrashSummary.available
                          ? formatCount(allOpenLocalTrashSummary.trashedMessageCount)
                          : "--"}
                      </span>
                    </button>
                    {openSessionFolderTrees.map(({ session, hierarchy, tree }) => {
                      const workspaceId = session.workspace.id;
                      const rootCollapsed = collapsedFolderPaths.has(
                        workspaceRootCollapseKey(workspaceId),
                      );
                      const rootSelected =
                        messageVisibility === "active" &&
                        allOpenFolderSelection.workspaceId === workspaceId &&
                        allOpenFolderSelection.folderId == null;
                      const localTrashSelected =
                        messageVisibility === "trashed" &&
                        localTrashAllOpenWorkspaceId === workspaceId;
                      return (
                        <div className="workspace-folder-root" key={workspaceId}>
                          <div
                            className={`folder-row folder-row-composite workspace-root-row ${
                              rootSelected ? "selected" : ""
                            }`}
                          >
                            <button
                              type="button"
                              className="folder-toggle-button"
                              onClick={() =>
                                setCollapsedFolderPaths((current) => {
                                  const next = new Set(current);
                                  const key = workspaceRootCollapseKey(workspaceId);
                                  if (next.has(key)) next.delete(key);
                                  else next.add(key);
                                  return next;
                                })
                              }
                              title={rootCollapsed ? "Expand PST folders" : "Collapse PST folders"}
                              aria-label={`${rootCollapsed ? "Expand" : "Collapse"} ${pstDisplayName(
                                session.workspace,
                              )}`}
                              aria-expanded={!rootCollapsed}
                            >
                              {rootCollapsed ? ">" : "v"}
                            </button>
                            <button
                              type="button"
                              className="folder-select-button"
                              onClick={() => selectAllOpenWorkspaceFolder(workspaceId, null)}
                              title={session.workspace.pstPath}
                              aria-current={rootSelected ? "true" : undefined}
                            >
                              <span className="folder-name">{pstDisplayName(session.workspace)}</span>
                              <span className="folder-count">
                                {formatCount(workspaceActiveMessageCount(session))}
                              </span>
                            </button>
                          </div>
                          {!rootCollapsed ? (
                            <>
                              <button
                                type="button"
                                className={`folder-row all-mail-row workspace-all-mail-row ${
                                  rootSelected ? "selected" : ""
                                }`}
                                onClick={() => selectAllOpenWorkspaceFolder(workspaceId, null)}
                                title={`All indexed messages in ${pstDisplayName(session.workspace)}`}
                                aria-current={rootSelected ? "true" : undefined}
                              >
                                <span className="folder-name">All Mail</span>
                                <span className="folder-count">
                                  {formatCount(workspaceActiveMessageCount(session))}
                                </span>
                              </button>
                              <button
                                type="button"
                                className={`folder-row local-trash-row workspace-local-trash-row ${
                                  localTrashSelected ? "selected" : ""
                                }`}
                                onClick={() => showLocalTrash(workspaceId)}
                                title={localTrashStorageDisclosure}
                                aria-label={`Local Trash, app-only view for ${pstDisplayName(
                                  session.workspace,
                                )}, ${
                                  isLocalTrashHealthy(session.localTrashSummary)
                                    ? `${formatCount(
                                        session.localTrashSummary.trashedMessageCount,
                                      )} messages`
                                    : "count unavailable"
                                }`}
                                aria-current={localTrashSelected ? "true" : undefined}
                              >
                                <span className="folder-name">Local Trash</span>
                                <span className="folder-count">
                                  {isLocalTrashHealthy(session.localTrashSummary)
                                    ? formatCount(session.localTrashSummary.trashedMessageCount)
                                    : "--"}
                                </span>
                              </button>
                              {hierarchy.status === "invalid" ? (
                                <div className="folder-presentation-notice" role="status">
                                  <strong>Folder hierarchy unavailable</strong>
                                  <span>All Mail remains available for this PST.</span>
                                </div>
                              ) : null}
                              {messageVisibility === "active" &&
                              allOpenFolderSelection.workspaceId === workspaceId &&
                              allOpenFolderPresentationSelection?.kind === "promoted_scope" ? (
                                <div className="folder-presentation-notice" role="status">
                                  <strong>Promoted technical root scope selected</strong>
                                  <span>
                                    Its canonical folder scope is preserved; no child was selected
                                    automatically.
                                  </span>
                                </div>
                              ) : null}
                              <FolderRows
                                nodes={tree}
                                workspaceId={workspaceId}
                                selectedWorkspaceId={
                                  messageVisibility === "active"
                                    ? allOpenFolderSelection.workspaceId
                                    : null
                                }
                                selectedFolderId={
                                  messageVisibility === "active"
                                    ? allOpenFolderSelection.folderId
                                    : null
                                }
                                includeSubfolders={includeSubfolders}
                                collapsedFolderPaths={collapsedFolderPaths}
                                onToggleCollapse={toggleFolderCollapsed}
                                onSelect={(folder, selectedWorkspaceId) =>
                                  selectAllOpenWorkspaceFolder(
                                    selectedWorkspaceId,
                                    folder.canonicalFolderId,
                                  )
                                }
                              />
                            </>
                          ) : null}
                        </div>
                      );
                    })}
                  </>
                ) : (
                  <>
                    <button
                      type="button"
                      className={`folder-row all-mail-row ${
                        messageVisibility === "active" && selectedFolderId == null
                          ? "selected"
                          : ""
                      }`}
                      onClick={() => selectCurrentWorkspaceFolder(null)}
                      title="All indexed messages in this PST"
                      aria-current={
                        messageVisibility === "active" && selectedFolderId == null
                          ? "true"
                          : undefined
                      }
                    >
                      <span className="folder-name">All Mail</span>
                      <span className="folder-count">
                        {formatCount(
                          activeOpenPstSession
                            ? workspaceActiveMessageCount(activeOpenPstSession)
                            : workspace.messageCount,
                        )}
                      </span>
                    </button>
                    <button
                      type="button"
                      className={`folder-row local-trash-row ${
                        messageVisibility === "trashed" ? "selected" : ""
                      }`}
                      onClick={() => showLocalTrash(workspace.id)}
                      title={localTrashStorageDisclosure}
                      aria-label={`Local Trash, app-only view, ${
                        isLocalTrashHealthy(activeLocalTrashSummary)
                          ? `${formatCount(activeLocalTrashSummary!.trashedMessageCount)} messages`
                          : "count unavailable"
                      }`}
                      aria-current={messageVisibility === "trashed" ? "true" : undefined}
                    >
                      <span className="folder-name">Local Trash</span>
                      <span className="folder-count">
                        {isLocalTrashHealthy(activeLocalTrashSummary)
                          ? formatCount(activeLocalTrashSummary!.trashedMessageCount)
                          : "--"}
                      </span>
                    </button>
                    {!isLocalTrashHealthy(activeLocalTrashSummary) ? (
                      <div className="folder-presentation-notice" role="status">
                        <strong>Local Trash unavailable</strong>
                        <span>Normal views remain fail-open and show active messages.</span>
                      </div>
                    ) : null}
                    {folderHierarchy.status === "invalid" ? (
                      <div className="folder-presentation-notice" role="status">
                        <strong>Folder hierarchy unavailable</strong>
                        <span>All Mail remains available for this PST.</span>
                      </div>
                    ) : null}
                    {currentFolderPresentationSelection.kind === "promoted_scope" ? (
                      <div className="folder-presentation-notice" role="status">
                        <strong>Promoted technical root scope selected</strong>
                        <span>
                          Its canonical folder scope is preserved; no child was selected
                          automatically.
                        </span>
                      </div>
                    ) : null}
                    <FolderRows
                      nodes={folderTree}
                      workspaceId={workspace.id}
                      selectedWorkspaceId={messageVisibility === "active" ? workspace.id : null}
                      selectedFolderId={messageVisibility === "active" ? selectedFolderId : null}
                      includeSubfolders={includeSubfolders}
                      collapsedFolderPaths={collapsedFolderPaths}
                      onToggleCollapse={toggleFolderCollapsed}
                      onSelect={(folder) =>
                        selectCurrentWorkspaceFolder(folder.canonicalFolderId)
                      }
                    />
                  </>
                )
              ) : (
                <p className="empty-state">No PST opened.</p>
              )}
            </div>
            {messageVisibility === "active" && folderDetailsFolder && folderDetailsWorkspace ? (
              <section className="folder-details" aria-label="Folder Details">
                <h3>Folder Details</h3>
                <dl>
                  <div>
                    <dt>Displayed folder</dt>
                    <dd>{folderDetailsFolder.name}</dd>
                  </div>
                  <div>
                    <dt>Original PST path</dt>
                    <dd>{folderDetailsFolder.path || "(PST root)"}</dd>
                  </div>
                  <div>
                    <dt>Presentation</dt>
                    <dd>
                      {folderDetailsWrappersHidden
                        ? folderDetailsPresentationSelection?.kind === "promoted_scope"
                          ? "Selected technical wrapper hidden; canonical scope preserved"
                          : "Proven technical ancestor hidden"
                        : "No technical wrappers hidden for this folder"}
                    </dd>
                  </div>
                  <div>
                    <dt>PST</dt>
                    <dd>{pstDisplayName(folderDetailsWorkspace)}</dd>
                  </div>
                  <div>
                    <dt>Workspace ID</dt>
                    <dd title={folderDetailsWorkspace.id}>
                      {folderDetailsWorkspace.id.slice(0, 12)}
                    </dd>
                  </div>
                  <div>
                    <dt>Canonical direct messages</dt>
                    <dd>{formatCount(folderDetailsFolder.directMessageCount)}</dd>
                  </div>
                  <div>
                    <dt>Canonical with descendants</dt>
                    <dd>{formatCount(folderDetailsFolder.messageCount)}</dd>
                  </div>
                  <div>
                    <dt>Active direct messages</dt>
                    <dd>{formatCount(folderDetailsFolder.activeDirectMessageCount)}</dd>
                  </div>
                  <div>
                    <dt>Active with descendants</dt>
                    <dd>{formatCount(folderDetailsFolder.activeMessageCount)}</dd>
                  </div>
                  <div>
                    <dt>Local Trash direct</dt>
                    <dd>{formatCount(folderDetailsFolder.localTrashDirectMessageCount)}</dd>
                  </div>
                  <div>
                    <dt>Local Trash with descendants</dt>
                    <dd>{formatCount(folderDetailsFolder.localTrashMessageCount)}</dd>
                  </div>
                </dl>
              </section>
            ) : null}
          </aside>
        ) : null}

        {foldersHidden ? (
          <div className="folder-restore-rail">
            <button
              type="button"
              className="rail-show-folders"
              onClick={() => setFoldersHidden(false)}
              title="Show folder pane."
            >
              Show Folders
            </button>
            <div className="rail-pst-list" aria-label="Open PSTs">
              {openPstSessions.map((session) => {
                const active = workspace?.id === session.workspace.id;
                return (
                  <button
                    type="button"
                    key={session.workspace.id}
                    className={active ? "active" : ""}
                    onClick={() => void activateWorkspaceTab(session.workspace.id)}
                    title={session.workspace.pstPath}
                    disabled={isBusy}
                  >
                    {pstDisplayName(session.workspace)}
                  </button>
                );
              })}
            </div>
          </div>
        ) : null}

        {!foldersHidden ? (
          <div
            className="pane-splitter folder-splitter"
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize folders pane"
            onPointerDown={(event) => startPaneResize("folder", event)}
          />
        ) : null}

        <section className="messages-pane">
          <div className="pane-heading message-pane-heading">
            <div className="message-heading-title">
              <h2>
                {appliedSearch.listMode === "conversations" ? "Conversations - " : ""}
                {isSearchActive ? `Search Results - ${displayedFolderName}` : displayedFolderName}
              </h2>
              <span title={resultSummaryText}>
                {isInitializingMessageList
                  ? "Loading messages..."
                  : isSearching
                    ? "Searching..."
                    : resultSummaryText}
              </span>
            </div>
            <div className="message-list-tools">
              {workspace ? (
                <label className="sort-control">
                  <span>Sort</span>
                  {listMode === "conversations" ? (
                    <select
                      value={conversationSort}
                      onChange={(event) =>
                        setConversationSort(event.target.value as ConversationSort)
                      }
                      disabled={isBusy}
                    >
                      <option value="newest">Newest activity</option>
                      <option value="oldest">Oldest activity</option>
                      <option value="subject">Subject A-Z</option>
                    </select>
                  ) : (
                    <>
                      <select
                        value={draftSearchSnapshot.messageSort}
                        onChange={(event) => {
                          const requested = event.target.value as SortOrder;
                          setSortOrder(
                            requested === "relevance" && !relevanceAvailability.available
                              ? "newest"
                              : requested,
                          );
                        }}
                        disabled={isBusy}
                        aria-describedby={relevanceSortHelpId}
                      >
                        <option value="newest">Newest first</option>
                        <option
                          value="relevance"
                          disabled={!relevanceAvailability.available}
                          title={relevanceAvailability.explanation}
                        >
                          Relevance
                        </option>
                        <option value="oldest">Oldest first</option>
                        <option value="sender_az">Sender A-Z</option>
                        <option value="subject_az">Subject A-Z</option>
                      </select>
                      <span className="visually-hidden" id={relevanceSortHelpId}>
                        {relevanceAvailability.explanation}
                      </span>
                    </>
                  )}
                </label>
              ) : null}
            </div>
          </div>
          {appliedSearch.visibility === "trashed" ? (
            <div
              className={`local-trash-view-note ${
                appliedLocalTrashAvailable ? "" : "local-trash-view-unavailable"
              }`}
              role="status"
            >
              <strong>Local Trash is app-only and reversible.</strong>
              <span>{localTrashStorageDisclosure}</span>
              {!appliedLocalTrashAvailable ? (
                <span>
                  Local Trash state is unavailable for part or all of this view. Normal views remain
                  fail-open; no messages are treated as deleted.
                </span>
              ) : null}
              <button
                type="button"
                onClick={showActiveMessages}
                disabled={localTrashMutationPending}
              >
                Back to active messages
              </button>
            </div>
          ) : null}
          <span
            className="visually-hidden"
            id={
              appliedSearch.listMode === "messages"
                ? messageResultsInstructionsId
                : conversationResultsInstructionsId
            }
          >
            {appliedSearch.listMode === "messages"
              ? "Use Up and Down Arrow keys to move through results. Press Enter to open a message."
              : "Use Up and Down Arrow keys to move. Use Right and Left Arrow keys to expand, collapse, and return to a conversation."}
          </span>
          <div
            className="message-list"
            ref={messageListRef}
            data-support-control="scroll"
            aria-busy={isResultsBusy}
            aria-describedby={
              appliedSearch.listMode === "messages"
                ? messageResultsInstructionsId
                : conversationResultsInstructionsId
            }
            aria-label={
              appliedSearch.listMode === "messages" ? "Message results" : "Conversation results"
            }
            role={appliedSearch.listMode === "messages" ? "list" : "tree"}
            tabIndex={-1}
          >
            {appliedSearch.listMode === "conversations"
              ? (
                <ConversationResultsList
                  activeNavigationKey={resultNavigation.conversations}
                  cleanSnippet={cleanMessageSnippet}
                  conversationReindexRequired={conversationReindexRequired}
                  conversations={conversations}
                  emptyState={searchEmptyState}
                  exactCountStatus={activeConversationCountStatus}
                  exactTotalCount={conversationTotalCount}
                  expandedConversations={expandedConversations}
                  formatCount={formatCount}
                  formatDate={formatMessageDate}
                  hasMore={hasMoreConversations}
                  highlightTerms={highlightTerms}
                  isBusy={isBusy || anyWorkspaceIntegrityOperationRunning}
                  isCrossPstSearch={appliedIsCrossPstSearch}
                  isLoadingMore={isLoadingMoreConversations}
                  isSearching={isSearching}
                  measurementIdentity={conversationResultsMeasurementIdentity}
                  onLoadMoreConversation={(conversation, showingEntireConversation) =>
                    void loadConversationMessages(conversation, showingEntireConversation, true)
                  }
                  onLoadMoreConversations={() =>
                    void loadConversationsPage(true, appliedSearchVersion)
                  }
                  onOpenMessage={openMessage}
                  onOpenPreview={openMessagePreviewWindow}
                  onActiveNavigationKeyChange={setConversationNavigationKey}
                  onReindexWorkspace={(workspaceId) => void reindexExistingEmls(workspaceId)}
                  onShowEntireConversation={(conversation) =>
                    void loadConversationMessages(conversation, true, false)
                  }
                  onToggleConversation={toggleConversation}
                  resetIdentity={appliedSearchVersion.generation}
                  resultSummaryText={conversationResultSummaryText}
                  scrollContainerRef={messageListRef}
                  selectedMessageId={selectedMessage?.id ?? null}
                  selectedWorkspaceId={selectedMessage?.workspaceId ?? workspace?.id ?? null}
                  workspaceIssues={conversationWorkspaceIssues}
                />
              )
              : (
                <MessageResultsList
                  activeNavigationKey={resultNavigation.messages}
                  activeWorkspaceId={workspace?.id ?? null}
                  cleanSnippet={cleanMessageSnippet}
                  displayMode={messageListDisplayMode}
                  emptyState={searchEmptyState}
                  exactCountStatus={activeMessageCountStatus}
                  exactTotalCount={messageTotalCount}
                  formatCount={formatCount}
                  formatDate={formatMessageDate}
                  hasMore={hasMoreMessages}
                  highlightTerms={highlightTerms}
                  isCrossPstSearch={appliedIsCrossPstSearch}
                  isLoadingMore={isLoadingMoreMessages}
                  isSearching={isSearching}
                  measurementIdentity={messageResultsMeasurementIdentity}
                  messages={messages}
                  onLoadMore={() => void loadMessagesPage(true, appliedSearchVersion)}
                  onActiveNavigationKeyChange={setMessageNavigationKey}
                  onOpenMessage={openMessage}
                  onOpenPreview={openMessagePreviewWindow}
                  resetIdentity={appliedSearchVersion.generation}
                  resultSummaryText={messageResultSummaryText}
                  scrollContainerRef={messageListRef}
                  selectedMessageId={selectedMessage?.id ?? null}
                  selectedWorkspaceId={selectedMessage?.workspaceId ?? workspace?.id ?? null}
                />
              )}
          </div>
        </section>

        {layoutMode === "three_column" ? (
          <div
            className="pane-splitter message-splitter"
            role="separator"
            aria-orientation="vertical"
            aria-label="Resize message list pane"
            onPointerDown={(event) => startPaneResize("message", event)}
          />
        ) : null}

        {layoutMode === "outlook" ? (
          <div
            className="pane-splitter outlook-splitter"
            role="separator"
            aria-orientation="horizontal"
            aria-label="Resize message list and reader panes"
            onPointerDown={(event) => startPaneResize("outlookMessage", event)}
          />
        ) : null}

        <article className="preview-pane">
          <div className="pane-heading">
            <h2>Preview</h2>
            {selectedMessage ? (
              <span title={selectedMessage.date || undefined}>
                {formatMessageDate(selectedMessage.date)}
              </span>
            ) : null}
          </div>

          {selectedMessage ? (
            <div className="message-preview" ref={previewScrollRef} data-support-control="scroll">
              <h3>{selectedMessage.subject || "(No subject)"}</h3>
              {selectedMessage.inLocalTrash ? (
                <span className="local-trash-badge">In Local Trash</span>
              ) : null}
              <dl className="headers">
                <div>
                  <dt>From</dt>
                  <dd>{selectedMessage.sender || "(No sender)"}</dd>
                </div>
                <div>
                  <dt>To/Cc/Bcc</dt>
                  <dd>{selectedMessage.recipients || "(No recipients)"}</dd>
                </div>
                <div>
                  <dt>Date</dt>
                  <dd title={selectedMessage.date || undefined}>
                    {formatMessageDate(selectedMessage.date) || "(No date)"}
                  </dd>
                </div>
              </dl>

              {selectedMessage.attachments.length ? (
                <section className="attachments">
                  <h4>Attachments ({formatCount(selectedMessage.attachmentCount)})</h4>
                  <p className="attachment-safety-note">
                    Opening attachments uses your Mac's default app after exporting a safe copy to
                    the workspace. The original PST is not modified.
                  </p>
                  <ul>
                    {selectedMessage.attachments.map((attachment) => {
                      const exportResult = exportResults[attachment.id];
                      return (
                        <li key={attachment.id}>
                          <div className="attachment-main">
                            <span>{attachment.filename || "(Unnamed attachment)"}</span>
                            <small>
                              {attachment.contentType || "unknown type"}
                              {attachment.sizeBytes != null
                                ? ` - ${formatBytes(attachment.sizeBytes)}`
                                : ""}
                            </small>
                          </div>
                          <div className="attachment-actions">
                            <button
                              type="button"
                              onClick={() => void openAttachment(attachment.id)}
                              disabled={
                                openingAttachmentId === attachment.id ||
                                exportingAttachmentId === attachment.id
                              }
                              title="Export a safe workspace copy, then open that copy with the Mac default app."
                            >
                              {openingAttachmentId === attachment.id ? "Opening" : "Open"}
                            </button>
                            <button
                              type="button"
                              onClick={() => void exportAttachment(attachment.id)}
                              disabled={
                                exportingAttachmentId === attachment.id ||
                                openingAttachmentId === attachment.id
                              }
                            >
                              {exportingAttachmentId === attachment.id ? "Exporting" : "Export"}
                            </button>
                            {exportResult?.exported && exportResult.outputPath ? (
                              <button
                                type="button"
                                onClick={() => void revealExportedFile(exportResult.outputPath!)}
                              >
                                Reveal Exported File in Finder
                              </button>
                            ) : null}
                          </div>
                          {exportResult ? (
                            <small
                              className={
                                exportResult.exported ? "attachment-export-ok" : "attachment-export-error"
                              }
                            >
                              {exportResult.exported
                                ? `Exported to ${exportResult.outputPath}`
                                : exportResult.error}
                            </small>
                          ) : null}
                        </li>
                      );
                    })}
                  </ul>
                </section>
              ) : null}

              <section className="body-section">
                <div className="reader-toolbar">
                  <div className="reader-mode-toggle" role="group" aria-label="Reader mode">
                    <button
                      type="button"
                      className={readerMode === "plain_text" ? "selected" : ""}
                      data-support-control="click"
                      onClick={() => setReaderMode("plain_text")}
                    >
                      Plain Text
                    </button>
                    <button
                      type="button"
                      className={readerMode === "sanitized_html" ? "selected" : ""}
                      data-support-control="click"
                      onClick={() => setReaderMode("sanitized_html")}
                    >
                      Sanitized HTML
                    </button>
                  </div>
                  <div className="reader-actions" role="toolbar" aria-label="Message actions">
                    <button
                      type="button"
                      onClick={() =>
                        void changeSelectedMessageLocalTrash(!selectedMessage.inLocalTrash)
                      }
                      disabled={
                        localTrashMutationPending ||
                        isBusy ||
                        isImporting ||
                        activeIntegrityOperationRunning ||
                        !selectedMessageLocalTrashAvailable
                      }
                      title={
                        selectedMessageLocalTrashAvailable
                          ? localTrashActionDisclosure
                          : "Local Trash is unavailable. Normal views remain fail-open."
                      }
                      aria-describedby="local-trash-action-disclosure"
                    >
                      {localTrashMutationPending
                        ? "Updating"
                        : selectedMessage.inLocalTrash
                          ? "Restore"
                          : "Move to Local Trash"}
                    </button>
                    <button
                      type="button"
                      onClick={() => void openSourceEml()}
                      disabled={isLoadingSourceEml}
                      title="Print, save HTML, save the original EML, or inspect raw source."
                    >
                      {isLoadingSourceEml ? "Opening" : "Print / Export"}
                    </button>
                    <button
                      type="button"
                      onClick={revealEml}
                      title="Reveal the extracted EML file inside the workspace cache."
                    >
                      Reveal Source
                    </button>
                  </div>
                </div>
                <p className="local-trash-disclosure" id="local-trash-action-disclosure">
                  {localTrashActionDisclosure} Local Trash removes no message content and does not
                  reclaim workspace storage.
                </p>
                {!selectedMessageLocalTrashAvailable ? (
                  <p className="body-status">
                    Local Trash actions are unavailable
                    {selectedMessage.localTrashErrorCode
                      ? ` (${selectedMessage.localTrashErrorCode})`
                      : ""}
                    . This message remains visible in normal views.
                  </p>
                ) : null}

                {readerMode === "plain_text" ? (
                  <>
                    {selectedBodyStatus ? <p className="body-status">{selectedBodyStatus}</p> : null}
                    {selectedBody.trim() ? (
                      <pre className="body-preview">
                        <HighlightedText text={selectedBody} terms={highlightTerms} />
                      </pre>
                    ) : null}
                  </>
                ) : (
                  <div className="html-reader-section">
                    {isRenderingHtml ? <p className="body-status">Rendering sanitized HTML.</p> : null}
                    {htmlRender?.error ? (
                      <p className="body-status">Body could not be parsed.</p>
                    ) : null}
                    {htmlRtfTextFallback ? (
                      <>
                        <p className="body-status">
                          Sanitized HTML is unavailable for this Outlook Rich Text message. Showing
                          converted plain text.
                        </p>
                        <pre className="body-preview">
                          <HighlightedText text={selectedBody} terms={highlightTerms} />
                        </pre>
                      </>
                    ) : htmlMissingCanReindex ? (
                      <div className="reindex-notice">
                        <span>
                          This workspace was indexed before HTML support. Reindex existing EMLs to
                          enable HTML view.
                        </span>
                        <button
                          type="button"
                          onClick={() => void reindexExistingEmls()}
                          disabled={isBusy || isImporting}
                        >
                          Reindex existing EMLs
                        </button>
                      </div>
                    ) : htmlRender && !htmlRender.htmlAvailable ? (
                      <p className="body-status">No HTML body available.</p>
                    ) : null}
                    {htmlRender?.remoteImagesBlocked ? (
                      <div className="remote-image-notice">
                        <span>{remoteImagesBlockedMessage}</span>
                      </div>
                    ) : null}
                    {htmlRender?.htmlAvailable && htmlRender.sanitizedHtml.trim() ? (
                      <div
                        className="html-preview"
                        onClick={(event) => {
                          if ((event.target as HTMLElement).closest("a")) {
                            event.preventDefault();
                          }
                        }}
                        onAuxClick={(event) => {
                          if ((event.target as HTMLElement).closest("a")) {
                            event.preventDefault();
                          }
                        }}
                        onMouseOver={(event) => {
                          const link = (event.target as HTMLElement).closest("a");
                          const href = link?.getAttribute("href");
                          if (link && href && !link.getAttribute("title")) {
                            link.setAttribute("title", href);
                          }
                        }}
                        dangerouslySetInnerHTML={{ __html: htmlRender.sanitizedHtml }}
                        data-support-untrusted="true"
                      />
                    ) : null}
                  </div>
                )}
              </section>

              <details
                className="message-diagnostics"
                onToggle={(event) => {
                  if (event.currentTarget.open) void loadMessageDiagnostics();
                }}
              >
                <summary>Message Diagnostics</summary>
                {isLoadingMessageDiagnostics ? (
                  <p className="body-status">Loading diagnostics.</p>
                ) : null}
                {messageDiagnosticsError ? (
                  <p className="body-status">Diagnostics failed: {messageDiagnosticsError}</p>
                ) : null}
                {messageDiagnostics ? (
                  <div className="diagnostics-content">
                    <dl className="diagnostics-grid">
                      <div>
                        <dt>Body source</dt>
                        <dd>{messageDiagnostics.bodySource || "(missing)"}</dd>
                      </div>
                      <div>
                        <dt>Plain text</dt>
                        <dd>{yesNo(messageDiagnostics.hasBodyText)}</dd>
                      </div>
                      <div>
                        <dt>Sanitized HTML source</dt>
                        <dd>{yesNo(messageDiagnostics.hasBodyHtml)}</dd>
                      </div>
                      <div>
                        <dt>Attachments</dt>
                        <dd>{formatCount(messageDiagnostics.attachmentCount)}</dd>
                      </div>
                      <div>
                        <dt>Remote images</dt>
                        <dd>{yesNo(messageDiagnostics.remoteImagesDetected)}</dd>
                      </div>
                      <div>
                        <dt>CID images</dt>
                        <dd>{yesNo(messageDiagnostics.cidImagesDetected)}</dd>
                      </div>
                      <div>
                        <dt>Body MIME part</dt>
                        <dd>{messageDiagnostics.detectedBodyMimePart || "(unknown)"}</dd>
                      </div>
                      <div>
                        <dt>Source EML</dt>
                        <dd title={messageDiagnostics.sourceEmlPath}>{messageDiagnostics.sourceEmlPath}</dd>
                      </div>
                      <div>
                        <dt>Message-ID</dt>
                        <dd>{messageDiagnostics.messageIdHeader || "(missing)"}</dd>
                      </div>
                      <div>
                        <dt>In-Reply-To</dt>
                        <dd>{messageDiagnostics.inReplyTo || "(missing)"}</dd>
                      </div>
                      <div>
                        <dt>References</dt>
                        <dd>{messageDiagnostics.referencesHeader || "(missing)"}</dd>
                      </div>
                      <div>
                        <dt>Normalized subject</dt>
                        <dd>{messageDiagnostics.normalizedSubject || "(missing)"}</dd>
                      </div>
                      <div>
                        <dt>Conversation</dt>
                        <dd>{messageDiagnostics.conversationId || "(not indexed)"}</dd>
                      </div>
                      <div>
                        <dt>Thread assignment</dt>
                        <dd>{threadAssignmentLabel(messageDiagnostics.threadAssignmentMethod)}</dd>
                      </div>
                      <div>
                        <dt>Detected parent</dt>
                        <dd>{messageDiagnostics.detectedParent || "(none)"}</dd>
                      </div>
                      <div>
                        <dt>Detected root</dt>
                        <dd>{messageDiagnostics.detectedRoot || "(none)"}</dd>
                      </div>
                    </dl>

                    {messageDiagnosticNotes.length ? (
                      <ul className="diagnostics-notes">
                        {messageDiagnosticNotes.map((note) => (
                          <li key={note}>{note}</li>
                        ))}
                      </ul>
                    ) : null}

                    {messageDiagnostics.attachments.length ? (
                      <div className="diagnostics-section">
                        <h4>Indexed Attachments</h4>
                        <ul className="diagnostics-attachments">
                          {messageDiagnostics.attachments.map((attachment, index) => (
                            <li key={`${attachment.filename}-${index}`}>
                              <span>{attachment.filename || "(Unnamed attachment)"}</span>
                              <small>{attachment.contentType || "unknown type"}</small>
                            </li>
                          ))}
                        </ul>
                      </div>
                    ) : null}

                    <div className="diagnostics-section">
                      <h4>MIME Parts</h4>
                      {messageDiagnostics.mimeParts.length ? (
                        <div className="diagnostics-table-wrap">
                          <table className="diagnostics-table">
                            <thead>
                              <tr>
                                <th>Part</th>
                                <th>Role</th>
                                <th>Type</th>
                                <th>Disposition</th>
                                <th>Filename</th>
                                <th>Content-ID</th>
                                <th>Size</th>
                              </tr>
                            </thead>
                            <tbody>
                              {messageDiagnostics.mimeParts.map((part) => (
                                <tr key={part.path}>
                                  <td>{part.path}</td>
                                  <td>{part.role}</td>
                                  <td>{part.contentType || "(unknown)"}</td>
                                  <td>{part.contentDisposition || "(none)"}</td>
                                  <td>{part.filename}</td>
                                  <td>{part.contentId}</td>
                                  <td>{formatMaybeBytes(part.sizeBytes)}</td>
                                </tr>
                              ))}
                            </tbody>
                          </table>
                        </div>
                      ) : (
                        <p className="body-status">No MIME summary available.</p>
                      )}
                    </div>
                  </div>
                ) : null}
              </details>
            </div>
          ) : (
            <p className="empty-state">Select a message to read it.</p>
          )}
        </article>
      </section>
    </main>

    {aboutOpen ? (
      <div className="about-backdrop">
        <section
          id={helpDialogId}
          ref={helpDialogRef}
          className="about-modal"
          role="dialog"
          aria-modal="false"
          aria-label="About PST QuickView"
        >
          <header className="about-header">
            <div>
              <h2>About PST QuickView</h2>
              <p>Read-only local PST viewer</p>
            </div>
            <button type="button" onClick={closeAbout} title="Close About.">
              Close
            </button>
          </header>
          <RemoteSupportPanel onApproved={finishSupportApproval} />
          <DevelopmentUpdatePanel />
          <dl className="about-details">
            <div>
              <dt>Version</dt>
              <dd>v{appVersion}</dd>
            </div>
            <div>
              <dt>Created by</dt>
              <dd>Kev P</dd>
            </div>
            <div>
              <dt>Appearance</dt>
              <dd>
                <label className="appearance-control">
                  <span className="visually-hidden">Application appearance</span>
                  <select
                    value={appearance}
                    data-support-control="input"
                    onChange={(event) => setAppearance(event.target.value as AppearanceMode)}
                  >
                    <option value="system">System</option>
                    <option value="light">Light</option>
                    <option value="dark">Dark</option>
                  </select>
                </label>
              </dd>
            </div>
            <div>
              <dt>Purpose</dt>
              <dd>Read-only local PST, EML, and MSG viewing, indexing, search, and export.</dd>
            </div>
            <div>
              <dt>Open files</dt>
              <dd>Open PST, EML, or MSG files from Finder, or drag them into the app.</dd>
            </div>
            <div>
              <dt>Safety</dt>
              <dd>
                Original PST, EML, and MSG files are never modified. Processing is local-only with
                no telemetry or cloud service. Deleting a workspace removes only the confirmed local
                cache; it does not delete an original source file.
              </dd>
            </div>
            <div>
              <dt>Local Trash</dt>
              <dd>
                Local Trash is reversible, app-only organization stored in a separate versioned
                overlay inside the local workspace. It hides messages from normal PST QuickView
                message, conversation, folder-count, and search views without changing the source
                PST, canonical index, extracted EML files, or attachments. Restore or the
                most-recent Move Undo makes a message active again. Local Trash removes no message
                content and does not reclaim storage. If its overlay is absent, all messages are
                active and the first Move initializes it. If the overlay is invalid, from a future
                schema, or belongs to a different workspace fingerprint, PST QuickView fails open:
                normal messages remain visible and Local Trash actions are unavailable.
              </dd>
            </div>
            <div>
              <dt>Folder hierarchy</dt>
              <dd>
                Simplified hierarchy is the default presentation and promotes only structurally
                proven technical wrappers. Uncertain folders, including real folders with
                Archive-like or Outlook-like names, remain visible. Original PST hierarchy is
                available from Layout. Switching presentations preserves canonical folder IDs,
                counts, selection, Include subfolders scope, and the original in-PST path shown in
                Folder Details; it never rewrites the PST, workspace, or index. The preference is
                stored locally and persists across workspace close and Start Fresh.
              </dd>
            </div>
            <div>
              <dt>Popover controls</dt>
              <dd>
                Advanced Search, Recent, Workspace, Layout, Details, and Help close when you press
                Escape or click outside. Escape returns focus to the opening control. Tab remains
                available for normal keyboard navigation, and opening one floating panel closes the
                previous panel.
              </dd>
            </div>
            <div>
              <dt>Import verification</dt>
              <dd>
                Quick Verify checks this workspace's marker, extracted-record manifest, database,
                search index, links, counters, and conversation state. A verified result means
                internally verified against this workspace's extracted source records. It does not
                prove parity with Exchange or Outlook, or prove that every historical message exists.
              </dd>
            </div>
            <div>
              <dt>Import progress</dt>
              <dd>
                Progress follows persisted stages from Preflight through internal verification.
                Extraction is indeterminate because ReadPST does not provide a trustworthy total;
                live stream/root observations and post-exit output aggregates are activity signals,
                not an Outlook parity count. Candidate records are discovered inputs, while
                canonical messages are durable indexed records.
                Import elapsed time accumulates only active app sessions across resumes. Repair and
                verification timing is kept separate from completed import history and can retain
                active time across an interrupted same-kind retry. Stage time can retain prior
                active sessions until a real stage transition.
                For an older workspace without trusted timing history, only active time recorded
                since the workspace entered Phase 3 is available.
                During a resumed import or repair, processed, inserted, and existing-record counts
                describe this pass; the canonical-message total remains the stable workspace count.
                ETA appears only for stable determinate work. Imports continue to identify their PST
                basename when another tab is selected.
              </dd>
            </div>
            <div>
              <dt>Resume and Repair</dt>
              <dd>
                Resume Indexing inserts missing canonical records from existing extraction output.
                Repair Index builds and verifies a temporary database from those local files, then
                replaces the active index only after success. Neither action writes to the source PST.
              </dd>
            </div>
            <div>
              <dt>Source availability</dt>
              <dd>
                A complete local workspace remains usable when its source volume is unavailable, but
                verification cannot make a source-comparison claim until that PST is available.
              </dd>
            </div>
            <div>
              <dt>Possible duplicates</dt>
              <dd>
                Possible duplicate source-record warnings are informational. Records are not hidden,
                merged, suppressed, or deleted automatically. Blank, no-subject, and empty-body
                messages remain visible; suppression remains fail-open and zero.
              </dd>
            </div>
            <div>
              <dt>Storage</dt>
              <dd>
                Next to PST keeps the searchable cache beside the PST in a Spotlight-safe .noindex
                folder, useful for external drives. App Support stores the cache on this Mac.
              </dd>
            </div>
            <div>
              <dt>Attachments</dt>
              <dd>Attachments are exported only when you click Export or Open.</dd>
            </div>
            <div>
              <dt>PST QuickView license</dt>
              <dd>GPL-3.0-or-later</dd>
            </div>
            <div>
              <dt>ReadPST/LibPST license</dt>
              <dd>GPL-2.0-or-later</dd>
            </div>
            <div>
              <dt>Warranty and third parties</dt>
              <dd>No warranty. Third-party components retain their own licenses.</dd>
            </div>
            <div>
              <dt>readpst</dt>
              <dd>
                {readpstStatus?.available
                  ? `${readpstStatus.sourceLabel}${readpstStatus.path ? ` - ${readpstStatus.path}` : ""}${readpstStatus.version ? ` (${readpstStatus.version})` : ""}`
                  : readpstStatus
                    ? `Missing - ${readpstStatus.setupCommand}`
                    : "Checking..."}
              </dd>
            </div>
          </dl>
          <div className="about-license-actions">
            <button type="button" onClick={() => void revealProjectLicense()}>
              Reveal Project License
            </button>
            <button type="button" onClick={() => void revealThirdPartyNotices()}>
              Reveal Third-Party Notices
            </button>
          </div>
          <details
            className="about-diagnostics"
            onToggle={(event) => {
              if (event.currentTarget.open && !appDiagnostics) {
                void loadAppDiagnostics();
              }
            }}
          >
            <summary>Diagnostics</summary>
            {isLoadingAppDiagnostics ? <p>Loading diagnostics.</p> : null}
            {appDiagnostics ? (
              <>
                <dl className="about-details">
                  <div>
                    <dt>App version</dt>
                    <dd>{appDiagnostics.appVersion}</dd>
                  </div>
                  <div>
                    <dt>macOS</dt>
                    <dd>{appDiagnostics.macosVersion}</dd>
                  </div>
                  <div>
                    <dt>CPU</dt>
                    <dd>{appDiagnostics.cpuArchitecture}</dd>
                  </div>
                  <div>
                    <dt>Executable</dt>
                    <dd>{appDiagnostics.executableArchitecture}</dd>
                  </div>
                  <div>
                    <dt>readpst source</dt>
                    <dd>{appDiagnostics.readpstSource}</dd>
                  </div>
                  <div>
                    <dt>readpst version</dt>
                    <dd>{appDiagnostics.readpstVersion}</dd>
                  </div>
                  <div>
                    <dt>Open PSTs</dt>
                    <dd>{formatCount(appDiagnostics.openPstCount)}</dd>
                  </div>
                  <div>
                    <dt>Workspace mode</dt>
                    <dd>{appDiagnostics.activeWorkspaceMode}</dd>
                  </div>
                  <div>
                    <dt>Workspace path</dt>
                    <dd>
                      {appDiagnostics.activeWorkspacePath ? (
                        <BreakableFilesystemPath path={appDiagnostics.activeWorkspacePath} />
                      ) : (
                        "(none)"
                      )}
                    </dd>
                  </div>
                  <div>
                    <dt>Database schema</dt>
                    <dd>{appDiagnostics.databaseSchemaVersion ?? "(unavailable)"}</dd>
                  </div>
                  <div>
                    <dt>Conversation data</dt>
                    <dd>{appDiagnostics.conversationDataStatus}</dd>
                  </div>
                </dl>
                <p className="diagnostics-privacy-warning">
                  Diagnostics can contain local file paths. Review them before sharing.
                </p>
                <div className="about-diagnostic-actions">
                  <button type="button" onClick={() => void copyAppDiagnostics()}>
                    Copy Diagnostics
                  </button>
                  <button type="button" onClick={() => void revealApplicationLogs()}>
                    Reveal Logs
                  </button>
                </div>
              </>
            ) : null}
            {aboutStatus ? <p className="message-action-status">{aboutStatus}</p> : null}
          </details>
        </section>
      </div>
    ) : null}

    {sourceEmlOpen && sourceEmlView ? (
      <div className="source-eml-backdrop">
        <section className="source-eml-modal" role="dialog" aria-modal="true" aria-label="Print / Export Message">
          <header className="source-eml-header">
            <div>
              <h2>Print / Export Message</h2>
              {sourceEmlView.inLocalTrash ? (
                <span className="local-trash-badge">In Local Trash</span>
              ) : null}
              <p title={sourceEmlView.sourcePath}>
                <strong>{sourceEmlView.sourceLabel}</strong> - {sourceEmlView.sourcePath}
              </p>
            </div>
            <button type="button" onClick={resetSourceEmlViewer} title="Close Print / Export Message.">
              Close
            </button>
          </header>

          <div className="source-eml-toolbar">
            <div className="reader-mode-toggle" role="group" aria-label="Print / Export view mode">
              <button
                type="button"
                className={sourceEmlMode === "rendered" ? "selected" : ""}
                onClick={() => setSourceEmlMode("rendered")}
              >
                Rendered View
              </button>
              <button
                type="button"
                className={sourceEmlMode === "plain_text" ? "selected" : ""}
                onClick={() => setSourceEmlMode("plain_text")}
              >
                Plain Text
              </button>
              <button
                type="button"
                className={sourceEmlMode === "raw_source" ? "selected" : ""}
                onClick={() => setSourceEmlMode("raw_source")}
              >
                Raw Source
              </button>
            </div>
            <div className="reader-actions">
              <button
                type="button"
                onClick={() => void saveSourcePrintableHtml()}
                disabled={isSavingPrintable}
              >
                {isSavingPrintable ? "Saving" : "Save Printable HTML"}
              </button>
              <button type="button" onClick={() => void saveSourceEmlAs()}>
                Save Source {sourceEmlView.sourceFormat === "msg" ? "MSG" : "EML"} As...
              </button>
              {printableSaveResult?.saved && printableSaveResult.outputPath ? (
                <button
                  type="button"
                  onClick={() => void revealSavedHtml(printableSaveResult.outputPath!)}
                >
                  Reveal Saved
                </button>
              ) : sourceEmlSaveResult?.exported && sourceEmlSaveResult.outputPath ? (
                <button
                  type="button"
                  onClick={() => void revealSavedEml(sourceEmlSaveResult.outputPath!)}
                >
                  Reveal Saved
                </button>
              ) : null}
            </div>
          </div>

          {sourceEmlStatus ? <p className="message-action-status">{sourceEmlStatus}</p> : null}
          {printStatus ? <p className="message-action-status">{printStatus}</p> : null}
          {printableSaveResult?.saved ? (
            <p className="message-action-status action-ok">
              Saved printable HTML to {printableSaveResult.outputPath}
            </p>
          ) : printableSaveResult?.error ? (
            <p className="message-action-status action-error">{printableSaveResult.error}</p>
          ) : null}
          {sourceEmlSaveResult?.exported ? (
            <p className="message-action-status action-ok">
              Saved source message to {sourceEmlSaveResult.outputPath}
            </p>
          ) : sourceEmlSaveResult?.error ? (
            <p className="message-action-status action-error">{sourceEmlSaveResult.error}</p>
          ) : null}

          {sourceEmlMode === "rendered" && sourceEmlReconstructionWarnings.length ? (
            <div className="message-action-status action-warning">
              {sourceEmlReconstructionWarnings.map((warning) => (
                <div key={warning}>{warning}</div>
              ))}
            </div>
          ) : null}

          {sourceEmlView.parseWarnings.length ? (
            <details className="message-reconstruction-details">
              <summary>Message reconstruction details</summary>
              <ul>
                {sourceEmlView.parseWarnings.map((warning) => (
                  <li key={warning}>{warning}</li>
                ))}
              </ul>
            </details>
          ) : null}

          <SourceMessageMetadata view={sourceEmlView} />

          {sourceEmlView.attachments.length ? (
            <section className="attachments source-eml-attachments">
              <h4>Attachments ({formatCount(sourceEmlView.attachments.length)})</h4>
              <p className="attachment-safety-note">
                {sourceEmlView.sourceKind === "standalone"
                  ? "Opening attachments uses your Mac's default app after exporting a safe copy to App Support. The source message is not modified."
                  : "Opening attachments uses your Mac's default app after exporting a safe copy to the workspace. The original PST is not modified."}
              </p>
              <ul>
                {sourceEmlView.attachments.map((attachment) => {
                  const exportResult = exportResults[attachment.id];
                  return (
                    <li key={attachment.id}>
                      <div className="attachment-main">
                        <span>{attachment.filename || "(Unnamed attachment)"}</span>
                        <small>
                          {attachmentContentLabel(attachment)}
                          {attachment.contentDisposition
                            ? ` - ${attachment.contentDisposition}`
                            : ""}
                          {attachment.sizeBytes != null ? ` - ${formatBytes(attachment.sizeBytes)}` : ""}
                        </small>
                      </div>
                      <div className="attachment-actions">
                        <button
                          type="button"
                          onClick={() => void openAttachment(attachment.id)}
                          disabled={
                            openingAttachmentId === attachment.id ||
                            exportingAttachmentId === attachment.id
                          }
                          title="Export a safe workspace copy, then open that copy with the Mac default app."
                        >
                          {openingAttachmentId === attachment.id ? "Opening" : "Open"}
                        </button>
                        <button
                          type="button"
                          onClick={() => void exportAttachment(attachment.id)}
                          disabled={
                            exportingAttachmentId === attachment.id ||
                            openingAttachmentId === attachment.id
                          }
                        >
                          {exportingAttachmentId === attachment.id ? "Exporting" : "Export"}
                        </button>
                        {exportResult?.exported && exportResult.outputPath ? (
                          <button
                            type="button"
                            onClick={() => void revealExportedFile(exportResult.outputPath!)}
                          >
                            Reveal Exported File in Finder
                          </button>
                        ) : null}
                      </div>
                      {exportResult ? (
                        <small
                          className={
                            exportResult.exported ? "attachment-export-ok" : "attachment-export-error"
                          }
                        >
                          {exportResult.exported
                            ? `Exported to ${exportResult.outputPath}`
                            : exportResult.error}
                        </small>
                      ) : null}
                    </li>
                  );
                })}
              </ul>
            </section>
          ) : null}

          {sourceEmlView.inlineResources.length ? (
            <details className="attachments inline-resources source-eml-attachments">
              <summary>
                Inline resources ({formatCount(sourceEmlView.inlineResources.length)})
              </summary>
              <p className="attachment-safety-note">
                These images were matched to exact Content-ID references in the message body.
              </p>
              <ul>
                {sourceEmlView.inlineResources.map((attachment) => {
                  const exportResult = exportResults[attachment.id];
                  return (
                    <li key={attachment.id}>
                      <div className="attachment-main">
                        <span>{attachment.filename || "(Unnamed inline resource)"}</span>
                        <small>
                          {attachment.contentType || "unknown type"}
                          {attachment.sizeBytes != null
                            ? ` - ${formatBytes(attachment.sizeBytes)}`
                            : ""}
                        </small>
                      </div>
                      <div className="attachment-actions">
                        <button
                          type="button"
                          onClick={() => void openAttachment(attachment.id)}
                          disabled={
                            openingAttachmentId === attachment.id ||
                            exportingAttachmentId === attachment.id
                          }
                          title="Export a safe copy, then open that copy with the Mac default app."
                        >
                          {openingAttachmentId === attachment.id ? "Opening" : "Open"}
                        </button>
                        <button
                          type="button"
                          onClick={() => void exportAttachment(attachment.id)}
                          disabled={
                            exportingAttachmentId === attachment.id ||
                            openingAttachmentId === attachment.id
                          }
                        >
                          {exportingAttachmentId === attachment.id ? "Exporting" : "Export"}
                        </button>
                        {exportResult?.exported && exportResult.outputPath ? (
                          <button
                            type="button"
                            onClick={() => void revealExportedFile(exportResult.outputPath!)}
                          >
                            Reveal Exported File in Finder
                          </button>
                        ) : null}
                      </div>
                      {exportResult ? (
                        <small
                          className={
                            exportResult.exported
                              ? "attachment-export-ok"
                              : "attachment-export-error"
                          }
                        >
                          {exportResult.exported
                            ? `Exported to ${exportResult.outputPath}`
                            : exportResult.error}
                        </small>
                      ) : null}
                    </li>
                  );
                })}
              </ul>
            </details>
          ) : null}

          <section className="source-eml-body">
            {sourceEmlMode === "rendered" ? (
              <>
                {sourceEmlView.remoteImagesBlocked ? (
                  <div className="remote-image-notice">
                    <span>{remoteImagesBlockedMessage}</span>
                  </div>
                ) : null}
                {sourceEmlView.sanitizedHtml.trim() ? (
                  <div
                    className="html-preview source-eml-html"
                    onClick={(event) => {
                      if ((event.target as HTMLElement).closest("a")) {
                        event.preventDefault();
                      }
                    }}
                    onAuxClick={(event) => {
                      if ((event.target as HTMLElement).closest("a")) {
                        event.preventDefault();
                      }
                    }}
                    onMouseOver={(event) => {
                      const link = (event.target as HTMLElement).closest("a");
                      const href = link?.getAttribute("href");
                      if (link && href && !link.getAttribute("title")) {
                        link.setAttribute("title", href);
                      }
                    }}
                    dangerouslySetInnerHTML={{ __html: sourceEmlView.sanitizedHtml }}
                    data-support-untrusted="true"
                  />
                ) : (
                  <>
                    <p className="body-status">
                      {sourceEmlBodyStatus ?? "No sanitized HTML body available. Showing plain text."}
                    </p>
                    {sourceEmlBody.trim() ? (
                      <pre className="body-preview">{sourceEmlBody}</pre>
                    ) : null}
                  </>
                )}
              </>
            ) : sourceEmlMode === "plain_text" ? (
              <>
                {sourceEmlBodyStatus ? <p className="body-status">{sourceEmlBodyStatus}</p> : null}
                {sourceEmlBody.trim() ? (
                  <pre className="body-preview">{sourceEmlBody}</pre>
                ) : null}
              </>
            ) : (
              <div className="source-eml-raw">
                  <p className="body-status">
                  {sourceEmlView.sourceFormat === "msg"
                    ? "Raw Source shows structured MSG properties and diagnostics. MSG files are binary and are not displayed as raw bytes."
                    : sourceEmlView.sourceKind === "standalone"
                      ? "Raw Source shows the original standalone .eml contents."
                      : "Raw Source shows the original extracted .eml contents."}
                </p>
                <input
                  type="search"
                  placeholder="Find in raw source"
                  value={sourceEmlRawSearch}
                  onChange={(event) => setSourceEmlRawSearch(event.target.value)}
                />
                <pre className="raw-source-preview">
                  <HighlightedText text={sourceEmlView.rawSource} terms={sourceEmlRawTerms} />
                </pre>
              </div>
            )}
          </section>
        </section>
      </div>
    ) : null}

    </>
  );
}

export default App;
