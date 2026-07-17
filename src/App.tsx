import {
  Fragment,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { WebviewWindow } from "@tauri-apps/api/webviewWindow";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type {
  AppDiagnostics,
  Attachment,
  BackendError,
  CancelImportResult,
  CalendarItemDetails,
  ConversationListResult,
  ConversationMessageItem,
  ConversationMessagesResult,
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
  ImportProgress,
  MessageDiagnostics,
  MessageDetail,
  MessageListItem,
  MessageListResult,
  MultiMessageListResult,
  PstOpenPlan,
  ReadpstStatus,
  SavePrintableHtmlResult,
  SearchFilters,
  SourceEmlView,
  WorkspaceLocationMode,
  WorkspacePreflight,
  WorkspaceSearchCount,
  WorkspaceSize,
  WorkspaceSummary,
} from "./types";
import packageInfo from "../package.json";
import { conversationParticipantSummary } from "./conversationDisplay";
import {
  appearanceStorageKey,
  applyAppearance,
  applyStoredAppearance,
  storedAppearance,
  type AppearanceMode,
} from "./appearance";
import {
  defaultWorkspaceFolderSelection,
  normalizeStoredFolderSelection,
  resolveWorkspaceFolderSelection,
  type WorkspaceFolderSelection,
} from "./sessionRestore";

type FolderNode = Folder & { children: FolderNode[] };
type ImportAction = "import" | "open_existing" | "resume_index" | "reimport";
type ReaderMode = "plain_text" | "sanitized_html";
type SourceEmlMode = "rendered" | "plain_text" | "raw_source";
type FolderScopeFilter = "current" | "current_subfolders" | "all";
type SortOrder = "newest" | "oldest" | "sender_az" | "subject_az";
type SearchScope = "current" | "all_open";
type LayoutMode = "three_column" | "outlook";
type MessageListDisplayMode = "subject_first" | "sender_first";
type ListMode = "messages" | "conversations";
type ConversationSort = "newest" | "oldest" | "subject";
type AllOpenFolderSelection = {
  workspaceId: string | null;
  folderId: number | null;
};
type OpenPstSession = {
  workspace: WorkspaceSummary;
  workspaceSize: WorkspaceSize | null;
  folders: Folder[];
  folderSelection: WorkspaceFolderSelection;
  status: "complete" | "importing" | "error";
};
type WorkspaceOperationState = {
  progress: ImportProgress | null;
  running: boolean;
  notice: string | null;
  error: string | null;
  setupCommand: string | null;
  deleteResult: DeleteResult | null;
  deleteStatuses: string[];
  deleteConfirmOpen: boolean;
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

const emptyWorkspaceOperationState = (): WorkspaceOperationState => ({
  progress: null,
  running: false,
  notice: null,
  error: null,
  setupCommand: null,
  deleteResult: null,
  deleteStatuses: [],
  deleteConfirmOpen: false,
});
type PrintablePreview = {
  title: string;
  modeLabel: string;
  defaultFilename: string;
  html: string;
};
type ExpandedConversationState = {
  items: ConversationMessageItem[];
  matchingMessageCount: number;
  totalMessageCount: number;
  showingEntireConversation: boolean;
  loading: boolean;
  error: string | null;
};
type SourceEmlContext =
  | { kind: "workspace"; workspaceId: string; messageId: number }
  | { kind: "standalone"; messagePath: string };
type PreviewWindowTarget =
  | { kind: "workspace"; workspaceId: string; messageId: number }
  | { kind: "standalone"; messagePath: string };
type AdvancedSearchFilters = {
  from: string;
  recipients: string;
  subject: string;
  body: string;
  attachment: string;
  hasAttachments: "any" | "yes" | "no";
  dateFrom: string;
  dateTo: string;
  folderScope: FolderScopeFilter;
};

const missingReadpstCommand = "brew install libpst";
const workspaceLocationStorageKey = "pstQuickView.workspaceLocationMode";
const paneWidthStorageKey = "pstQuickView.paneWidths";
const folderHiddenStorageKey = "pstQuickView.folderPaneHidden";
const collapsedFolderPathsStorageKey = "pstQuickView.collapsedFolderPaths";
const layoutModeStorageKey = "pstQuickView.layoutMode";
const messageListDisplayStorageKey = "pstQuickView.messageListDisplayMode";
const listModeStorageKey = "pstQuickView.listMode";
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
const defaultAdvancedSearchFilters: AdvancedSearchFilters = {
  from: "",
  recipients: "",
  subject: "",
  body: "",
  attachment: "",
  hasAttachments: "any",
  dateFrom: "",
  dateTo: "",
  folderScope: "current_subfolders",
};

type PaneWidths = typeof defaultPaneWidths;

function getErrorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    return String((error as BackendError).message);
  }
  return "An unknown error occurred.";
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

function initialCollapsedFolderPaths(): string[] {
  if (typeof window === "undefined") return [];
  try {
    const stored = JSON.parse(window.localStorage.getItem(collapsedFolderPathsStorageKey) ?? "[]");
    return Array.isArray(stored)
      ? stored.filter((item): item is string => typeof item === "string")
      : [];
  } catch {
    return [];
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

function buildFolderTree(folders: Folder[]): FolderNode[] {
  const nodes = new Map<number, FolderNode>();
  for (const folder of folders) {
    nodes.set(folder.id, { ...folder, children: [] });
  }

  const roots: FolderNode[] = [];
  for (const node of nodes.values()) {
    if (node.parentId && nodes.has(node.parentId)) {
      nodes.get(node.parentId)!.children.push(node);
    } else {
      roots.push(node);
    }
  }

  const sortNodes = (items: FolderNode[]) => {
    items.sort((a, b) => a.name.localeCompare(b.name));
    items.forEach((item) => sortNodes(item.children));
  };
  sortNodes(roots);

  return roots;
}

function folderCollapseKey(folder: Folder, workspaceId: string | null): string {
  return `${workspaceId ?? "active"}:${folder.path || `id:${folder.id}`}`;
}

function workspaceRootCollapseKey(workspaceId: string): string {
  return `workspace:${workspaceId}`;
}

function collectCollapsibleFolderKeys(nodes: FolderNode[], workspaceId: string | null): string[] {
  const keys: string[] = [];
  const visit = (items: FolderNode[]) => {
    for (const item of items) {
      if (item.children.length) keys.push(folderCollapseKey(item, workspaceId));
      visit(item.children);
    }
  };
  visit(nodes);
  return keys;
}

function formatCount(count: number): string {
  return new Intl.NumberFormat().format(count);
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

function hasAdvancedFilterValues(filters: AdvancedSearchFilters): boolean {
  return Boolean(
    filters.from.trim() ||
      filters.recipients.trim() ||
      filters.subject.trim() ||
      filters.body.trim() ||
      filters.attachment.trim() ||
      filters.hasAttachments !== "any" ||
      filters.dateFrom ||
      filters.dateTo ||
      filters.folderScope !== "current_subfolders",
  );
}

function backendSearchFilters(filters: AdvancedSearchFilters): SearchFilters {
  const valueOrNull = (value: string) => value.trim() || null;
  return {
    from: valueOrNull(filters.from),
    recipients: valueOrNull(filters.recipients),
    subject: valueOrNull(filters.subject),
    body: valueOrNull(filters.body),
    attachment: valueOrNull(filters.attachment),
    hasAttachments: filters.hasAttachments,
    dateFrom: filters.dateFrom || null,
    dateTo: filters.dateTo || null,
  };
}

function escapeRegex(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function queryHighlightTerms(query: string, filters: AdvancedSearchFilters): string[] {
  const terms = new Set<string>();
  const pushTerms = (value: string) => {
    for (const match of value.match(/"([^"]+)"|[^\s:]+:[^\s]+|[^\s]+/g) ?? []) {
      const typedValue = match.includes(":") && !match.startsWith('"') ? match.split(":").slice(1).join(":") : match;
      const cleaned = typedValue.replace(/^"|"$/g, "").trim();
      if (
        cleaned &&
        !/^(has|before|after)$/i.test(match.split(":")[0] ?? "") &&
        !/^\d{4}-\d{2}-\d{2}$/.test(cleaned)
      ) {
        terms.add(cleaned);
      }
    }
  };

  pushTerms(query);
  [
    filters.from,
    filters.recipients,
    filters.subject,
    filters.body,
    filters.attachment,
  ].forEach(pushTerms);

  return Array.from(terms)
    .flatMap((term) => (term.length > 24 ? term.split(/[^\w@.-]+/) : [term]))
    .map((term) => term.trim())
    .filter((term) => term.length >= 2 && term.length <= 80)
    .slice(0, 12);
}

function HighlightedText({ text, terms }: { text: string; terms: string[] }): ReactNode {
  if (!text || !terms.length) return text;
  const pattern = terms
    .map(escapeRegex)
    .sort((a, b) => b.length - a.length)
    .join("|");
  if (!pattern) return text;

  const matcher = new RegExp(`(${pattern})`, "gi");
  const parts = text.split(matcher);
  return parts.map((part, index) =>
    index % 2 === 1 ? (
      <mark className="match-highlight" key={`${part}-${index}`}>
        {part}
      </mark>
    ) : (
      part
    ),
  );
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
    notes.push("Remote images detected; they remain blocked unless loaded for this message.");
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
  nodes: FolderNode[];
  workspaceId: string;
  selectedWorkspaceId: string | null;
  selectedFolderId: number | null;
  includeSubfolders: boolean;
  collapsedFolderPaths: Set<string>;
  onToggleCollapse: (folder: FolderNode, workspaceId: string) => void;
  onSelect: (folder: Folder, workspaceId: string) => void;
  level?: number;
}) {
  return (
    <>
      {nodes.map((node) => {
        const isCollapsed = collapsedFolderPaths.has(folderCollapseKey(node, workspaceId));
        const isSelected = selectedWorkspaceId === workspaceId && selectedFolderId === node.id;
        return (
          <div key={node.id}>
            <div
              className={`folder-row folder-row-composite ${isSelected ? "selected" : ""}`}
              style={{ paddingLeft: `${12 + level * 14}px` }}
            >
              <button
                type="button"
                className="folder-toggle-button"
                onClick={() => onToggleCollapse(node, workspaceId)}
                disabled={!node.children.length}
                title={isCollapsed ? "Expand folder" : "Collapse folder"}
                aria-label={`${isCollapsed ? "Expand" : "Collapse"} ${node.name}`}
              >
                {node.children.length ? (isCollapsed ? ">" : "v") : ""}
              </button>
              <button
                type="button"
                className="folder-select-button"
                onClick={() => onSelect(node, workspaceId)}
                title={node.path || "Root folder"}
              >
                <span className="folder-name">{node.name}</span>
                <span className="folder-count">
                  {includeSubfolders ? node.messageCount : node.directMessageCount}
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

function previewWindowParams(): PreviewWindowTarget | null {
  if (typeof window === "undefined") return null;
  const params = new URLSearchParams(window.location.search);
  const standaloneMessagePath = params.get("standaloneMessagePath");
  if (standaloneMessagePath) {
    return { kind: "standalone", messagePath: standaloneMessagePath };
  }
  const workspaceId = params.get("previewWorkspaceId");
  const messageId = Number(params.get("previewMessageId"));
  if (!workspaceId || !Number.isFinite(messageId)) return null;
  return { kind: "workspace", workspaceId, messageId };
}

function previewWindowLabel(workspaceId: string, messageId: number): string {
  const safeWorkspaceId = workspaceId.replace(/[^a-zA-Z0-9:/_-]/g, "_");
  return `message-preview-${safeWorkspaceId}-${messageId}`;
}

function standalonePreviewWindowLabel(stableId: string): string {
  return `message-preview-file-${stableId.replace(/[^a-zA-Z0-9_-]/g, "_")}`;
}

function stringifyWindowError(error: unknown): string {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "payload" in error) {
    return String((error as { payload: unknown }).payload);
  }
  return getErrorMessage(error);
}

function MessagePreviewWindow({ target }: { target: PreviewWindowTarget }) {
  const [sourceEmlView, setSourceEmlView] = useState<SourceEmlView | null>(null);
  const [sourceEmlMode, setSourceEmlMode] = useState<SourceEmlMode>("rendered");
  const [sourceEmlRemoteAllowed, setSourceEmlRemoteAllowed] = useState(false);
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

  async function loadSourceEml(allowRemoteResources = false) {
    setError(null);
    setSourceEmlStatus(
      allowRemoteResources ? "Loading remote images for this EML." : "Loading message preview.",
    );
    try {
      const result =
        target.kind === "workspace"
          ? await invoke<SourceEmlView>("get_source_eml_view", {
              workspaceId: target.workspaceId,
              messageId: target.messageId,
              allowRemoteResources,
            })
          : await invoke<SourceEmlView>("get_standalone_message_view", {
              path: target.messagePath,
              allowRemoteResources,
            });
      setSourceEmlView(result);
      setSourceEmlRemoteAllowed(allowRemoteResources);
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
    void loadSourceEml(false);
  }, [target.kind, target.kind === "workspace" ? target.workspaceId : target.messagePath]);

  function buildPreviewPrintable(): PrintablePreview | null {
    if (!sourceEmlView) return null;

    let bodyHtml = "";
    let bodyModeLabel = "Rendered View";
    let note: string | null = null;

    if (sourceEmlMode === "raw_source") {
      bodyHtml = plainTextPrintBody(sourceEmlView.rawSource);
      bodyModeLabel = "Raw Source";
      note = "Printed raw EML source text. No remote resources were loaded.";
    } else if (sourceEmlMode === "plain_text") {
      bodyHtml = plainTextPrintBody(formatBodyForDisplay(sourceEmlView.bodyText));
      bodyModeLabel = "Plain Text";
    } else if (sourceEmlView.sanitizedHtml.trim()) {
      bodyHtml = `<div class="html-body">${makeSanitizedHtmlPrintable(sourceEmlView.sanitizedHtml)}</div>`;
      bodyModeLabel = "Rendered View";
      if (sourceEmlView.remoteImagesBlocked && !sourceEmlRemoteAllowed) {
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
    setError(null);
    setSourceEmlStatus("Choose where to save the source EML.");
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
              path: target.messagePath,
            });
      setSourceEmlSaveResult(result);
      if (result.exported) {
        setSourceEmlStatus(`Saved source EML to ${result.outputPath}`);
      } else if (result.error) {
        setError(result.error);
        setSourceEmlStatus(null);
      } else {
        setSourceEmlStatus("Save Source EML As... cancelled.");
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
              path: target.messagePath,
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
              path: target.messagePath,
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
      setSourceEmlStatus("Revealed saved source EML in Finder.");
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
            <p title={sourceEmlView?.sourcePath}>{sourceEmlView?.sourcePath ?? "Loading source EML."}</p>
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
                  {sourceEmlView.remoteImagesBlocked && !sourceEmlRemoteAllowed ? (
                    <div className="remote-image-notice">
                      <span>Remote resources are blocked.</span>
                      <button type="button" onClick={() => void loadSourceEml(true)}>
                        Load remote resources for this EML
                      </button>
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
                    placeholder="Search raw source"
                    aria-label="Search raw EML source"
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
  const previewParams = previewWindowParams();
  if (previewParams) {
    return <MessagePreviewWindow target={previewParams} />;
  }

  const [readpstStatus, setReadpstStatus] = useState<ReadpstStatus | null>(null);
  const [workspace, setWorkspace] = useState<WorkspaceSummary | null>(null);
  const [openPstSessions, setOpenPstSessions] = useState<OpenPstSession[]>([]);
  const [workspaceSize, setWorkspaceSize] = useState<WorkspaceSize | null>(null);
  const [folders, setFolders] = useState<Folder[]>([]);
  const [selectedFolderId, setSelectedFolderId] = useState<number | null>(null);
  const [allOpenFolderSelection, setAllOpenFolderSelection] = useState<AllOpenFolderSelection>({
    workspaceId: null,
    folderId: null,
  });
  const [includeSubfolders, setIncludeSubfolders] = useState(true);
  const [foldersHidden, setFoldersHidden] = useState(() =>
    initialBooleanPreference(folderHiddenStorageKey, false),
  );
  const [collapsedFolderPaths, setCollapsedFolderPaths] = useState<Set<string>>(
    () => new Set(initialCollapsedFolderPaths()),
  );
  const [messages, setMessages] = useState<MessageListItem[]>([]);
  const [messageTotalCount, setMessageTotalCount] = useState(0);
  const [listMode, setListMode] = useState<ListMode>(initialListMode);
  const [activeSessionGeneration, setActiveSessionGeneration] = useState(0);
  const [initializingWorkspaceId, setInitializingWorkspaceId] = useState<string | null>(null);
  const [conversationSort, setConversationSort] = useState<ConversationSort>("newest");
  const [conversations, setConversations] = useState<ConversationSummary[]>([]);
  const [conversationTotalCount, setConversationTotalCount] = useState(0);
  const [conversationMatchingMessageCount, setConversationMatchingMessageCount] = useState(0);
  const [conversationIndexedWorkspaceCount, setConversationIndexedWorkspaceCount] = useState(0);
  const [conversationWorkspaceIssues, setConversationWorkspaceIssues] = useState<
    ConversationWorkspaceIssue[]
  >([]);
  const [expandedConversations, setExpandedConversations] = useState<
    Record<string, ExpandedConversationState>
  >({});
  const [selectedMessage, setSelectedMessage] = useState<MessageDetail | null>(null);
  const [search, setSearch] = useState("");
  const [debouncedSearch, setDebouncedSearch] = useState("");
  const [advancedSearchOpen, setAdvancedSearchOpen] = useState(false);
  const [searchFilters, setSearchFilters] = useState<AdvancedSearchFilters>(
    defaultAdvancedSearchFilters,
  );
  const [sortOrder, setSortOrder] = useState<SortOrder>("newest");
  const [searchScope, setSearchScope] = useState<SearchScope>("current");
  const [isSearching, setIsSearching] = useState(false);
  const [isLoadingMoreMessages, setIsLoadingMoreMessages] = useState(false);
  const [isLoadingMoreConversations, setIsLoadingMoreConversations] = useState(false);
  const [workspaceSearchCounts, setWorkspaceSearchCounts] = useState<WorkspaceSearchCount[]>([]);
  const [workspaceOperationStates, setWorkspaceOperationStates] = useState<
    Record<string, WorkspaceOperationState>
  >({});
  const [isBusy, setIsBusy] = useState(false);
  const [isImporting, setIsImporting] = useState(false);
  const [aboutOpen, setAboutOpen] = useState(false);
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
  const [sourceEmlRemoteAllowed, setSourceEmlRemoteAllowed] = useState(false);
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
  const [remoteImagesAllowedMessageId, setRemoteImagesAllowedMessageId] = useState<number | null>(
    null,
  );
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
  const progressEventsEnabledRef = useRef(false);
  const operationWorkspaceIdRef = useRef<string | null>(null);
  const activeWorkspaceIdRef = useRef<string | null>(null);
  const workspaceOperationDismissTimersRef = useRef<Map<string, number>>(new Map());
  const paneLayoutRef = useRef<HTMLElement | null>(null);
  const messageListRef = useRef<HTMLDivElement | null>(null);
  const previewScrollRef = useRef<HTMLDivElement | null>(null);
  const messageSearchRequestIdRef = useRef(0);
  const conversationSearchRequestIdRef = useRef(0);
  const externalFileOpenChainRef = useRef<Promise<void>>(Promise.resolve());
  const externalPstQueueRef = useRef<string[]>([]);
  const externalPstProcessingRef = useRef(false);
  const activeExternalPstPathRef = useRef<string | null>(null);
  const externalBatchEnqueueRef = useRef<(batch: ExternalFileOpenBatch) => void>(() => undefined);
  const restorePromptTimerRef = useRef<number | null>(null);

  useEffect(() => {
    applyAppearance(appearance, document.documentElement);
    try {
      window.localStorage.setItem(appearanceStorageKey, appearance);
    } catch {
      // Appearance remains active for this session when storage is unavailable.
    }
  }, [appearance]);

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

  function clearWorkspaceOperationDismissTimer(workspaceId: string) {
    const timeout = workspaceOperationDismissTimersRef.current.get(workspaceId);
    if (timeout != null) {
      window.clearTimeout(timeout);
      workspaceOperationDismissTimersRef.current.delete(workspaceId);
    }
  }

  function clearWorkspaceOperationState(workspaceId: string) {
    clearWorkspaceOperationDismissTimer(workspaceId);
    setWorkspaceOperationStates((current) => {
      if (!current[workspaceId]) return current;
      const next = { ...current };
      delete next[workspaceId];
      return next;
    });
  }

  function scheduleWorkspaceOperationDismiss(workspaceId: string) {
    clearWorkspaceOperationDismissTimer(workspaceId);
    const timeout = window.setTimeout(() => {
      updateWorkspaceOperationState(workspaceId, {
        progress: null,
        notice: null,
      });
      setForegroundOperationWorkspaceId((current) =>
        current === workspaceId ? null : current,
      );
      workspaceOperationDismissTimersRef.current.delete(workspaceId);
    }, 5200);
    workspaceOperationDismissTimersRef.current.set(workspaceId, timeout);
  }

  function completeWorkspaceOperation(workspaceId: string, message: string) {
    updateWorkspaceOperationState(workspaceId, {
      progress: null,
      running: false,
      notice: message,
      error: null,
      setupCommand: null,
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

  function failWorkspaceOperation(
    workspaceId: string,
    message: string,
    operationSetupCommand: string | null = null,
  ) {
    clearWorkspaceOperationDismissTimer(workspaceId);
    updateWorkspaceOperationState(workspaceId, {
      running: false,
      error: message,
      setupCommand: operationSetupCommand,
      notice: null,
    });
  }

  function dismissWorkspaceOperation(workspaceId: string) {
    clearWorkspaceOperationDismissTimer(workspaceId);
    updateWorkspaceOperationState(workspaceId, {
      progress: null,
      notice: null,
    });
  }

  const folderTree = useMemo(() => buildFolderTree(folders), [folders]);
  const openSessionFolderTrees = useMemo(
    () =>
      openPstSessions.map((session) => ({
        session,
        tree: buildFolderTree(session.folders),
      })),
    [openPstSessions],
  );
  const hasActiveAdvancedFilters = hasAdvancedFilterValues(searchFilters);
  const isSearchActive = Boolean(search.trim() || hasActiveAdvancedFilters);
  const openWorkspaceIds = useMemo(
    () => openPstSessions.map((session) => session.workspace.id),
    [openPstSessions],
  );
  const activeWorkspaceOperation = workspace
    ? workspaceOperationStates[workspace.id] ?? null
    : null;
  const operationWorkspaceId = operationWorkspaceIdRef.current;
  const operationWorkspaceIsOpen = operationWorkspaceId
    ? openWorkspaceIds.includes(operationWorkspaceId)
    : false;
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
    isImporting && operationWorkspaceId != null && operationWorkspaceIsOpen;
  const allOpenSelectedWorkspaceId =
    searchScope === "all_open" ? allOpenFolderSelection.workspaceId : null;
  const allOpenSelectedFolderId =
    searchScope === "all_open" ? allOpenFolderSelection.folderId : null;
  const allOpenSelectedSession = allOpenSelectedWorkspaceId
    ? openPstSessions.find((session) => session.workspace.id === allOpenSelectedWorkspaceId) ?? null
    : null;
  const allOpenSelectedFolder =
    allOpenSelectedSession && allOpenSelectedFolderId != null
      ? allOpenSelectedSession.folders.find((folder) => folder.id === allOpenSelectedFolderId) ?? null
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
  const shouldUseMultiWorkspaceSearch = searchScope === "all_open" && allOpenSelectedWorkspaceId == null;
  const singleSearchWorkspaceId =
    searchScope === "all_open" ? allOpenSelectedWorkspaceId : (workspace?.id ?? null);
  const effectiveFolderId =
    searchScope === "all_open"
      ? allOpenSelectedFolderId
      : searchFilters.folderScope === "all"
        ? null
        : selectedFolderId;
  const effectiveIncludeSubfolders =
    searchScope === "all_open"
      ? includeSubfolders
      : searchFilters.folderScope === "current_subfolders";
  const conversationScopes = useMemo<ConversationWorkspaceScope[]>(() => {
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
  const messageListContextKey =
    searchScope === "all_open"
      ? `${activeSearchWorkspaceKey}:${allOpenSelectedFolderId ?? "all"}`
      : (workspace?.id ?? "");
  const highlightTerms = useMemo(
    () => queryHighlightTerms(search, searchFilters),
    [search, searchFilters],
  );
  const collapsedFolderPathList = useMemo(
    () => Array.from(collapsedFolderPaths).sort(),
    [collapsedFolderPaths],
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
    setSourceEmlRemoteAllowed(false);
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
        unlistenImport = await listen<ImportProgress>("import-progress", (event) => {
          const workspaceId = operationWorkspaceIdRef.current;
          if (!progressEventsEnabledRef.current || !workspaceId) return;

          const stage = event.payload.stage.toLowerCase();
          if (stage === "complete") {
            completeWorkspaceOperation(workspaceId, event.payload.message);
          } else if (stage === "cancelled") {
            completeWorkspaceOperation(workspaceId, event.payload.message);
          } else {
            updateWorkspaceOperationState(workspaceId, {
              progress: event.payload,
              running: true,
              notice: null,
              error: null,
            });
          }
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
    void getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "enter" || event.payload.type === "over") {
          setDropOverlayVisible(true);
          return;
        }
        setDropOverlayVisible(false);
        if (event.payload.type === "drop") {
          void handleDroppedFiles(event.payload.paths);
        }
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

  async function handleDroppedFiles(paths: string[]) {
    setDropOverlayVisible(false);
    try {
      const batch = await invoke<ExternalFileOpenBatch>("prepare_external_file_opens", { paths });
      externalBatchEnqueueRef.current(batch);
    } catch (err) {
      appendVisibleError(`Could not process dropped files: ${getErrorMessage(err)}`);
    }
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
    setAboutOpen(false);
    setAppDiagnostics(null);
    setAboutStatus(null);
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
    setError(null);
    setNotice(null);
    setSetupCommand(null);
    setPendingPreflightOpen(null);
    setPendingOpenPlan(null);
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
      setSearch("");
      setDebouncedSearch("");
      setAdvancedSearchOpen(false);
      setSearchFilters(defaultAdvancedSearchFilters);
      setSearchScope("current");
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
    activeWorkspaceIdRef.current = workspace?.id ?? null;
  }, [workspace?.id]);

  useEffect(() => {
    window.localStorage.setItem(paneWidthStorageKey, JSON.stringify(paneWidths));
  }, [paneWidths]);

  useEffect(() => {
    window.localStorage.setItem(folderHiddenStorageKey, String(foldersHidden));
  }, [foldersHidden]);

  useEffect(() => {
    window.localStorage.setItem(
      collapsedFolderPathsStorageKey,
      JSON.stringify(collapsedFolderPathList),
    );
  }, [collapsedFolderPathList]);

  useEffect(() => {
    window.localStorage.setItem(layoutModeStorageKey, layoutMode);
  }, [layoutMode]);

  useEffect(() => {
    window.localStorage.setItem(messageListDisplayStorageKey, messageListDisplayMode);
  }, [messageListDisplayMode]);

  useEffect(() => {
    window.localStorage.setItem(listModeStorageKey, listMode);
  }, [listMode]);

  useEffect(() => {
    window.localStorage.setItem(recentPstsStorageKey, JSON.stringify(recentPsts));
  }, [recentPsts]);

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

  useEffect(() => {
    const timeout = window.setTimeout(() => setDebouncedSearch(search), searchDebounceMs);
    return () => window.clearTimeout(timeout);
  }, [search]);

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
      for (const timeout of workspaceOperationDismissTimersRef.current.values()) {
        window.clearTimeout(timeout);
      }
      workspaceOperationDismissTimersRef.current.clear();
    },
    [],
  );

  useEffect(() => {
    if (!workspace && activeSearchWorkspaceIds.length === 0) {
      setMessages([]);
      setMessageTotalCount(0);
      setWorkspaceSearchCounts([]);
      setConversations([]);
      setConversationTotalCount(0);
      setConversationMatchingMessageCount(0);
      setConversationWorkspaceIssues([]);
      setExpandedConversations({});
      return;
    }

    if (listMode === "conversations") {
      void loadConversationsPage(false);
    } else {
      void loadMessagesPage(false);
    }
  }, [
    activeSessionGeneration,
    listMode,
    messageListContextKey,
    conversationScopeKey,
    searchScope,
    effectiveFolderId,
    effectiveIncludeSubfolders,
    debouncedSearch,
    searchFilters,
    sortOrder,
    conversationSort,
  ]);

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
    const allowRemoteImages = remoteImagesAllowedMessageId === selectedMessage.id;
    const messageWorkspaceId = selectedMessage.workspaceId ?? workspace.id;
    setIsRenderingHtml(true);
    invoke<HtmlRenderResult>("render_message_html", {
      workspaceId: messageWorkspaceId,
      messageId: selectedMessage.id,
      allowRemoteImages,
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
  }, [workspace, selectedMessage, readerMode, remoteImagesAllowedMessageId]);

  async function loadWorkspaceSession(
    nextWorkspace: WorkspaceSummary,
    requestedSelection?: WorkspaceFolderSelection,
  ): Promise<OpenPstSession> {
    const [nextFolders, nextWorkspaceSize] = await Promise.all([
      invoke<Folder[]>("list_folders", {
        workspaceId: nextWorkspace.id,
      }),
      invoke<WorkspaceSize>("get_workspace_size", {
        workspaceId: nextWorkspace.id,
      }),
    ]);

    const existingSelection = openPstSessions.find(
      (session) => session.workspace.id === nextWorkspace.id,
    )?.folderSelection;
    const activeSelection =
      workspace?.id === nextWorkspace.id
        ? {
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

    return {
      workspace: nextWorkspace,
      workspaceSize: nextWorkspaceSize,
      folders: nextFolders,
      folderSelection,
      status: "complete",
    };
  }

  function registerWorkspaceSession(nextSession: OpenPstSession) {
    setOpenPstSessions((current) => {
      const existingIndex = current.findIndex(
        (session) => session.workspace.id === nextSession.workspace.id,
      );
      if (existingIndex === -1) return [...current, nextSession];
      return current.map((session, index) => (index === existingIndex ? nextSession : session));
    });
  }

  function applyActiveWorkspaceSession(nextSession: OpenPstSession, resetMessageList: boolean) {
    const { workspace: nextWorkspace, workspaceSize: nextWorkspaceSize, folders: nextFolders } =
      nextSession;
    const selection = nextSession.folderSelection;

    activeWorkspaceIdRef.current = nextWorkspace.id;
    setWorkspace(nextWorkspace);
    setWorkspaceSize(nextWorkspaceSize);
    setFolders(nextFolders);
    setSelectedFolderId(selection.folderId);
    setIncludeSubfolders(selection.includeSubfolders);
    setSearchFilters((current) => ({
      ...current,
      folderScope: selection.includeSubfolders ? "current_subfolders" : "current",
    }));
    setInitializingWorkspaceId(nextWorkspace.id);
    setActiveSessionGeneration((current) => current + 1);

    if (resetMessageList) {
      setMessages([]);
      setMessageTotalCount(0);
      setWorkspaceSearchCounts([]);
      setConversations([]);
      setConversationTotalCount(0);
      setConversationMatchingMessageCount(0);
      setConversationWorkspaceIssues([]);
      setExpandedConversations({});
      setSelectedMessage(null);
      setExportResults({});
      resetPrintableStatus();
      resetSourceEmlViewer();
      setHtmlRender(null);
      setRemoteImagesAllowedMessageId(null);
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
    if (workspace?.id === workspaceId) return;

    const previousWorkspaceId = activeWorkspaceIdRef.current;
    activeWorkspaceIdRef.current = workspaceId;
    setError(null);
    try {
      const nextWorkspace = await invoke<WorkspaceSummary>("activate_workspace", { workspaceId });
      await refreshWorkspace(nextWorkspace, {
        resetMessageList: options.resetMessageList ?? !isCrossPstSearch,
      });
    } catch (err) {
      activeWorkspaceIdRef.current = previousWorkspaceId;
      setError(getErrorMessage(err));
    }
  }

  async function closeWorkspaceTab(workspaceId: string) {
    setError(null);
    try {
      const remainingSessions = openPstSessions.filter(
        (session) => session.workspace.id !== workspaceId,
      );
      const nextActive = await invoke<WorkspaceSummary | null>("close_workspace", { workspaceId });
      setOpenPstSessions(remainingSessions);
      clearWorkspaceOperationState(workspaceId);
      if (pendingWorkspaceFlowOwnerId === workspaceId) {
        const pendingPstPath = pendingOpenPlan?.pstPath ?? pendingPreflightOpen?.plan.pstPath;
        setPendingOpenPlan(null);
        setPendingPreflightOpen(null);
        setPendingWorkspaceFlowOwnerId(null);
        if (pendingPstPath) finishExternalPstRequest(pendingPstPath);
      }
      if (allOpenFolderSelection.workspaceId === workspaceId) {
        setAllOpenFolderSelection({ workspaceId: null, folderId: null });
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

  async function loadMessagesPage(append: boolean) {
    if (!shouldUseMultiWorkspaceSearch && !singleSearchWorkspaceId) return;
    if (shouldUseMultiWorkspaceSearch && activeSearchWorkspaceIds.length === 0) return;

    const requestId = messageSearchRequestIdRef.current + 1;
    messageSearchRequestIdRef.current = requestId;
    const queriedWorkspaceIds = shouldUseMultiWorkspaceSearch
      ? activeSearchWorkspaceIds
      : singleSearchWorkspaceId
        ? [singleSearchWorkspaceId]
        : [];
    const offset = append ? messages.length : 0;
    if (append) {
      setIsLoadingMoreMessages(true);
    } else {
      setIsSearching(true);
    }

    try {
      const result = shouldUseMultiWorkspaceSearch
        ? await invoke<MultiMessageListResult>("search_messages_multi", {
            workspaceIds: activeSearchWorkspaceIds,
            query: debouncedSearch.trim() || null,
            searchFilters: backendSearchFilters(searchFilters),
            sortOrder,
            limit: messagePageSize,
            offset,
          })
        : await invoke<MessageListResult>("list_messages", {
            workspaceId: singleSearchWorkspaceId,
            folderId: effectiveFolderId,
            query: debouncedSearch.trim() || null,
            includeSubfolders: effectiveIncludeSubfolders,
            searchFilters: backendSearchFilters(searchFilters),
            sortOrder,
            limit: messagePageSize,
            offset,
          });

      if (messageSearchRequestIdRef.current !== requestId) return;

      const singleWorkspaceSession =
        singleSearchWorkspaceId != null
          ? openPstSessions.find((session) => session.workspace.id === singleSearchWorkspaceId) ?? null
          : null;
      const normalizedResult =
        !shouldUseMultiWorkspaceSearch && searchScope === "all_open" && singleWorkspaceSession
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
      const multiResult = shouldUseMultiWorkspaceSearch ? (result as MultiMessageListResult) : null;
      setMessageTotalCount(result.totalCount);
      setWorkspaceSearchCounts(multiResult?.perWorkspaceCounts ?? []);
      setMessages((current) =>
        append ? [...current, ...normalizedResult.items] : normalizedResult.items,
      );
      if (!append) {
        setSelectedMessage((current) => {
          if (
            !current ||
            normalizedResult.items.some(
              (item) =>
                item.id === current.id &&
                (item.workspaceId ?? singleSearchWorkspaceId ?? workspace?.id) ===
                  (current.workspaceId ?? workspace?.id),
            )
          ) {
            return current;
          }
          setHtmlRender(null);
          setRemoteImagesAllowedMessageId(null);
          setExportResults({});
          resetPrintableStatus();
          resetSourceEmlViewer();
          return null;
        });
      }
    } catch (err) {
      if (messageSearchRequestIdRef.current === requestId) setError(getErrorMessage(err));
    } finally {
      if (messageSearchRequestIdRef.current === requestId) {
        setIsSearching(false);
        setIsLoadingMoreMessages(false);
        setInitializingWorkspaceId((current) =>
          current && queriedWorkspaceIds.includes(current) ? null : current,
        );
      }
    }
  }

  async function loadConversationsPage(append: boolean) {
    if (!conversationScopes.length) return;

    const requestId = conversationSearchRequestIdRef.current + 1;
    conversationSearchRequestIdRef.current = requestId;
    const queriedWorkspaceIds = conversationScopes.map((scope) => scope.workspaceId);
    const offset = append ? conversations.length : 0;
    if (append) {
      setIsLoadingMoreConversations(true);
    } else {
      setIsSearching(true);
      setExpandedConversations({});
    }

    try {
      const result = await invoke<ConversationListResult>("list_conversations", {
        scopes: conversationScopes,
        query: debouncedSearch.trim() || null,
        searchFilters: backendSearchFilters(searchFilters),
        conversationSort,
        limit: conversationPageSize,
        offset,
      });
      if (conversationSearchRequestIdRef.current !== requestId) return;

      setConversationTotalCount(result.totalCount);
      setConversationMatchingMessageCount(result.matchingMessageCount);
      setConversationIndexedWorkspaceCount(result.indexedWorkspaceCount);
      setConversationWorkspaceIssues(result.unindexedWorkspaces);
      setConversations((current) => (append ? [...current, ...result.items] : result.items));
    } catch (err) {
      if (conversationSearchRequestIdRef.current === requestId) setError(getErrorMessage(err));
    } finally {
      if (conversationSearchRequestIdRef.current === requestId) {
        setIsSearching(false);
        setIsLoadingMoreConversations(false);
        setInitializingWorkspaceId((current) =>
          current && queriedWorkspaceIds.includes(current) ? null : current,
        );
      }
    }
  }

  function conversationKey(workspaceId: string, conversationId: string): string {
    return `${workspaceId}:${conversationId}`;
  }

  async function loadConversationMessages(
    conversation: ConversationSummary,
    showEntireConversation: boolean,
    append = false,
  ) {
    const key = conversationKey(conversation.workspaceId, conversation.conversationId);
    const current = expandedConversations[key];
    const scope = conversationScopes.find(
      (candidate) => candidate.workspaceId === conversation.workspaceId,
    );
    if (!scope) return;

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
        query: debouncedSearch.trim() || null,
        searchFilters: backendSearchFilters(searchFilters),
        showEntireConversation,
        limit: conversationMessagePageSize,
        offset: append ? current?.items.length ?? 0 : 0,
      });
      setExpandedConversations((states) => ({
        ...states,
        [key]: {
          items: append ? [...(states[key]?.items ?? []), ...result.items] : result.items,
          matchingMessageCount: result.matchingMessageCount,
          totalMessageCount: result.totalMessageCount,
          showingEntireConversation: result.showingEntireConversation,
          loading: false,
          error: null,
        },
      }));
    } catch (err) {
      setExpandedConversations((states) => ({
        ...states,
        [key]: {
          ...(states[key] ?? {
            items: [],
            matchingMessageCount: conversation.matchingMessageCount,
            totalMessageCount: conversation.totalMessageCount,
            showingEntireConversation: showEntireConversation,
          }),
          loading: false,
          error: getErrorMessage(err),
        },
      }));
    }
  }

  function toggleConversation(conversation: ConversationSummary) {
    const key = conversationKey(conversation.workspaceId, conversation.conversationId);
    if (expandedConversations[key]) {
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
    progressEventsEnabledRef.current = false;
    operationWorkspaceIdRef.current = null;
    activeWorkspaceIdRef.current = null;
    setForegroundOperationWorkspaceId(null);
    for (const workspaceId of workspaceOperationDismissTimersRef.current.keys()) {
      clearWorkspaceOperationDismissTimer(workspaceId);
    }
    setWorkspace(null);
    setOpenPstSessions([]);
    setWorkspaceOperationStates({});
    setWorkspaceSize(null);
    setFolders([]);
    setSelectedFolderId(null);
    setAllOpenFolderSelection({ workspaceId: null, folderId: null });
    setInitializingWorkspaceId(null);
    setMessages([]);
    setMessageTotalCount(0);
    setWorkspaceSearchCounts([]);
    setConversations([]);
    setConversationTotalCount(0);
    setConversationMatchingMessageCount(0);
    setConversationIndexedWorkspaceCount(0);
    setConversationWorkspaceIssues([]);
    setExpandedConversations({});
    setSelectedMessage(null);
    setSearch("");
    setDebouncedSearch("");
    setAdvancedSearchOpen(false);
    setSearchFilters(defaultAdvancedSearchFilters);
    setSortOrder("newest");
    setSearchScope("current");
    setIsSearching(false);
    setIsLoadingMoreMessages(false);
    setIsLoadingMoreConversations(false);
    setExportResults({});
    setExportingAttachmentId(null);
    resetPrintableStatus();
    resetSourceEmlViewer();
    setReaderMode("plain_text");
    setHtmlRender(null);
    setIsRenderingHtml(false);
    setRemoteImagesAllowedMessageId(null);
    setIsImporting(false);
    setError(null);
    setNotice(null);
    setSetupCommand(null);
    setPendingOpenPlan(null);
    setPendingPreflightOpen(null);
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
    setError(null);
    setNotice(null);
    setSetupCommand(null);
    setPendingPreflightOpen(null);
    setPendingOpenPlan(null);
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
      const messagePath = await invoke<string | null>("pick_message_file");
      if (!messagePath) return;
      await openStandaloneMessageInline(messagePath);
    } catch (err) {
      setError(getErrorMessage(err));
      setSourceEmlStatus(null);
    } finally {
      setIsLoadingSourceEml(false);
      setIsBusy(false);
    }
  }

  async function openStandaloneMessageInline(messagePath: string) {
    setIsLoadingSourceEml(true);
    setSourceEmlStatus("Opening standalone message.");
    try {
      const result = await invoke<SourceEmlView>("get_standalone_message_view", {
        path: messagePath,
        allowRemoteResources: false,
      });
      setSourceEmlView(result);
      setSourceEmlContext({ kind: "standalone", messagePath: result.sourcePath });
      setSourceEmlMode("rendered");
      setSourceEmlOpen(true);
      setSourceEmlRemoteAllowed(false);
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

    if (matchingExisting && plan.existingWorkspaces.length === 1 && matchingExisting.isComplete) {
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
    setError(null);
    setSetupCommand(null);
    setIsBusy(true);
    const expectsProgress = importAction !== "open_existing";
    const targetWorkspaceId = existingWorkspace?.workspaceId ?? plan.fingerprint;
    operationWorkspaceIdRef.current = targetWorkspaceId;
    setForegroundOperationWorkspaceId(
      openWorkspaceIds.includes(targetWorkspaceId) ? null : targetWorkspaceId,
    );
    progressEventsEnabledRef.current = expectsProgress;
    setIsImporting(expectsProgress);
    updateWorkspaceOperationState(targetWorkspaceId, {
      progress: expectsProgress
        ? {
            stage: "Checking PST",
            current: null,
            total: null,
            message: "Checking PST and preparing the workspace.",
          }
        : null,
      running: expectsProgress,
      notice: null,
      error: null,
      setupCommand: null,
      deleteResult: null,
      deleteStatuses: [],
      deleteConfirmOpen: false,
    });

    try {
      const nextWorkspace = await invoke<WorkspaceSummary>("open_pst", {
        path: plan.pstPath,
        workspaceLocationMode: plan.selectedWorkspaceLocationMode,
        existingWorkspacePath: existingWorkspace?.workspacePath ?? null,
        allowDuplicate,
        importAction,
      });
      await refreshWorkspace(nextWorkspace);
      rememberRecentPst(nextWorkspace.pstPath);
      setPendingOpenPlan(null);
      setPendingWorkspaceFlowOwnerId(null);
      setReadpstStatus(await invoke<ReadpstStatus>("check_readpst"));
      if (nextWorkspace.id !== targetWorkspaceId) {
        clearWorkspaceOperationState(targetWorkspaceId);
      }
      setForegroundOperationWorkspaceId(null);
      completeWorkspaceOperation(
        nextWorkspace.id,
        nextWorkspace.reusedExisting
          ? "Opened existing workspace."
          : importAction === "resume_index"
            ? "Resume indexing completed."
            : "Workspace import completed.",
      );
    } catch (err) {
      const message = getErrorMessage(err);
      const operationSetupCommand = getSetupCommand(err);
      if (message.toLowerCase().includes("cancelled")) {
        completeWorkspaceOperation(targetWorkspaceId, "Import cancelled.");
      } else if (operationSetupCommand) {
        clearWorkspaceOperationState(targetWorkspaceId);
        setForegroundOperationWorkspaceId(null);
        setError(message);
        setSetupCommand(operationSetupCommand);
      } else {
        failWorkspaceOperation(targetWorkspaceId, message);
      }
    } finally {
      progressEventsEnabledRef.current = false;
      if (operationWorkspaceIdRef.current === targetWorkspaceId) {
        operationWorkspaceIdRef.current = null;
      }
      setIsBusy(false);
      setIsImporting(false);
      finishExternalPstRequest(plan.pstPath);
    }
  }

  async function reindexExistingEmls(targetWorkspaceId = workspace?.id) {
    if (!targetWorkspaceId) return;

    setError(null);
    setNotice(null);
    updateWorkspaceOperationState(targetWorkspaceId, {
      progress: {
        stage: "Scanning extracted EML files",
        current: null,
        total: null,
        message: "Scanning existing extracted EML files.",
      },
      running: true,
      notice: null,
      error: null,
      setupCommand: null,
      deleteResult: null,
      deleteStatuses: [],
      deleteConfirmOpen: false,
    });
    setHtmlRender(null);
    setRemoteImagesAllowedMessageId(null);
    setExportResults({});
    resetPrintableStatus();
    resetSourceEmlViewer();
    setIsBusy(true);
    setIsImporting(true);
    operationWorkspaceIdRef.current = targetWorkspaceId;
    setForegroundOperationWorkspaceId(null);
    progressEventsEnabledRef.current = true;

    try {
      const nextWorkspace = await invoke<WorkspaceSummary>("reindex_existing_emls", {
        workspaceId: targetWorkspaceId,
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
      progressEventsEnabledRef.current = false;
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
      setRemoteImagesAllowedMessageId(null);
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
          allowRemoteResources: false,
        }),
      ]);

      setExportResults({});
      resetPrintableStatus();
      setHtmlRender(null);
      setMessageDiagnostics(null);
      setMessageDiagnosticsError(null);
      setRemoteImagesAllowedMessageId(null);
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
      setSourceEmlRemoteAllowed(false);
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

  async function focusPreviewWindow(previewWindow: WebviewWindow) {
    try {
      await previewWindow.unminimize();
    } catch (err) {
      console.warn("Could not unminimize preview window.", err);
    }
    try {
      await previewWindow.show();
    } catch (err) {
      console.warn("Could not show preview window.", err);
    }
    try {
      await previewWindow.setFocus();
    } catch (err) {
      console.warn("Could not focus preview window.", err);
    }
  }

  async function waitForPreviewWindowCreated(previewWindow: WebviewWindow, label: string) {
    await new Promise<void>((resolve, reject) => {
      let settled = false;
      let cleanupCreated: (() => void) | null = null;
      let cleanupError: (() => void) | null = null;
      const settle = (callback: () => void) => {
        if (settled) return;
        settled = true;
        cleanupCreated?.();
        cleanupError?.();
        callback();
      };
      const timeout = window.setTimeout(async () => {
        const existing = await WebviewWindow.getByLabel(label).catch(() => null);
        if (existing) {
          settle(resolve);
        } else {
          settle(() => reject(new Error("Timed out waiting for preview window creation.")));
        }
      }, 5000);

      previewWindow
        .once("tauri://created", () => {
          window.clearTimeout(timeout);
          settle(resolve);
        })
        .then((unlisten) => {
          cleanupCreated = unlisten;
        })
        .catch((error) => {
          window.clearTimeout(timeout);
          settle(() => reject(error));
        });

      previewWindow
        .once("tauri://error", (event) => {
          window.clearTimeout(timeout);
          settle(() => reject(new Error(stringifyWindowError(event))));
        })
        .then((unlisten) => {
          cleanupError = unlisten;
        })
        .catch((error) => {
          window.clearTimeout(timeout);
          settle(() => reject(error));
        });
    });
  }

  async function openStandaloneMessagePreviewWindow(
    file: ExternalFileOpen,
  ): Promise<boolean> {
    const label = standalonePreviewWindowLabel(file.stableId);
    const title = file.path.split("/").pop() || "Message Preview";
    const url = `/?standaloneMessagePath=${encodeURIComponent(file.path)}`;

    try {
      const existing = await WebviewWindow.getByLabel(label);
      if (existing) {
        await focusPreviewWindow(existing);
        return true;
      }

      const previewWindow = new WebviewWindow(label, {
        url,
        title,
        width: 900,
        height: 700,
        minWidth: 720,
        minHeight: 520,
        resizable: true,
        focus: true,
      });
      await waitForPreviewWindowCreated(previewWindow, label);
      await focusPreviewWindow(previewWindow);
      return true;
    } catch (err) {
      const errorMessage = stringifyWindowError(err);
      console.error("Could not open standalone message pop-out.", err);
      setError(
        `Could not open pop-out window for ${file.path}: ${errorMessage}. Opened in-app preview instead.`,
      );
      try {
        await openStandaloneMessageInline(file.path);
      } catch (fallbackError) {
        setError(
          `Could not open ${file.path}: ${getErrorMessage(fallbackError)}. Pop-out error: ${errorMessage}`,
        );
        return false;
      }
      return true;
    }
  }

  async function openMessagePreviewWindow(message: MessageListItem, targetWorkspaceId = workspace?.id) {
    if (!targetWorkspaceId) return;

    clearWorkspaceOperationMessages(targetWorkspaceId);
    const label = previewWindowLabel(targetWorkspaceId, message.id);
    const title = message.subject || "Message Preview";
    const url = `/?previewWorkspaceId=${encodeURIComponent(targetWorkspaceId)}&previewMessageId=${encodeURIComponent(
      String(message.id),
    )}`;

    try {
      const existing = await WebviewWindow.getByLabel(label);
      if (existing) {
        await focusPreviewWindow(existing);
        return;
      }

      const previewWindow = new WebviewWindow(label, {
        url,
        title,
        width: 900,
        height: 700,
        minWidth: 720,
        minHeight: 520,
        resizable: true,
        focus: true,
      });
      await waitForPreviewWindowCreated(previewWindow, label);
      await focusPreviewWindow(previewWindow);
    } catch (err) {
      const errorMessage = stringifyWindowError(err);
      console.error("Could not open pop-out message preview.", err);
      showWorkspaceError(
        targetWorkspaceId,
        `Could not open pop-out window: ${errorMessage}. Opened in-app preview instead.`,
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
                path: sourceEmlContext.messagePath,
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
                path: sourceEmlContext.messagePath,
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

  async function openSourceEml(allowRemoteResources = false) {
    if (!workspace || !selectedMessage) return;
    const messageWorkspaceId = selectedMessage.workspaceId ?? workspace.id;

    clearWorkspaceOperationMessages(messageWorkspaceId);
    setSourceEmlStatus(
      allowRemoteResources ? "Loading remote images for this EML." : "Opening source EML.",
    );
    setSourceEmlSaveResult(null);
    setIsLoadingSourceEml(true);

    try {
      const result = await invoke<SourceEmlView>("get_source_eml_view", {
        workspaceId: messageWorkspaceId,
        messageId: selectedMessage.id,
        allowRemoteResources,
      });
      setSourceEmlView(result);
      setSourceEmlContext({
        kind: "workspace",
        workspaceId: messageWorkspaceId,
        messageId: selectedMessage.id,
      });
      setSourceEmlOpen(true);
      setSourceEmlRemoteAllowed(allowRemoteResources);
      setSourceEmlStatus(null);
    } catch (err) {
      showWorkspaceError(messageWorkspaceId, getErrorMessage(err));
      setSourceEmlStatus(null);
    } finally {
      setIsLoadingSourceEml(false);
    }
  }

  async function loadSourceEmlForCurrentContext(allowRemoteResources = false) {
    if (!sourceEmlContext) return;

    if (sourceEmlContext.kind === "workspace") {
      clearWorkspaceOperationMessages(sourceEmlContext.workspaceId);
      setSourceEmlStatus(
        allowRemoteResources ? "Loading remote images for this EML." : "Opening source EML.",
      );
      setIsLoadingSourceEml(true);
      try {
        const result = await invoke<SourceEmlView>("get_source_eml_view", {
          workspaceId: sourceEmlContext.workspaceId,
          messageId: sourceEmlContext.messageId,
          allowRemoteResources,
        });
        setSourceEmlView(result);
        setSourceEmlRemoteAllowed(allowRemoteResources);
        setSourceEmlStatus(null);
      } catch (err) {
        showWorkspaceError(sourceEmlContext.workspaceId, getErrorMessage(err));
        setSourceEmlStatus(null);
      } finally {
        setIsLoadingSourceEml(false);
      }
      return;
    }

    setError(null);
    setSourceEmlStatus(
      allowRemoteResources ? "Loading remote images for this message." : "Opening standalone message.",
    );
    setIsLoadingSourceEml(true);
    try {
      const result = await invoke<SourceEmlView>("get_standalone_message_view", {
        path: sourceEmlContext.messagePath,
        allowRemoteResources,
      });
      setSourceEmlView(result);
      setSourceEmlRemoteAllowed(allowRemoteResources);
      setSourceEmlStatus(null);
    } catch (err) {
      setError(getErrorMessage(err));
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
              path: sourceEmlContext.messagePath,
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
      note = "Printed raw EML source text. No remote resources were loaded.";
    } else if (sourceEmlMode === "plain_text") {
      bodyHtml = plainTextPrintBody(formatBodyForDisplay(sourceEmlView.bodyText));
      bodyModeLabel = "Plain Text";
    } else if (sourceEmlView.sanitizedHtml.trim()) {
      bodyHtml = `<div class="html-body">${makeSanitizedHtmlPrintable(sourceEmlView.sanitizedHtml)}</div>`;
      bodyModeLabel = "Rendered View";
      if (sourceEmlView.remoteImagesBlocked && !sourceEmlRemoteAllowed) {
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
      setSourceEmlStatus("Revealed saved source EML in Finder.");
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

  async function cancelImport() {
    const targetWorkspaceId = operationWorkspaceIdRef.current;
    if (targetWorkspaceId) {
      updateWorkspaceOperationState(targetWorkspaceId, {
        notice: "Cancel import requested.",
        error: null,
      });
    }
    try {
      const result = await invoke<CancelImportResult>("cancel_import");
      if (targetWorkspaceId) {
        updateWorkspaceOperationState(targetWorkspaceId, { notice: result.message });
      } else {
        setNotice(result.message);
      }
    } catch (err) {
      if (targetWorkspaceId) {
        failWorkspaceOperation(targetWorkspaceId, getErrorMessage(err));
      } else {
        setError(getErrorMessage(err));
      }
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
          message: "Deleted workspace. Original PST was not deleted.",
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

  async function deleteCurrentWorkspace() {
    if (!workspace) return;
    const workspaceId = workspace.id;
    if (isImporting) {
      updateWorkspaceOperationState(workspaceId, {
        notice: "Cancel import before deleting this workspace.",
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
  }

  async function confirmDeleteCurrentWorkspace() {
    if (!workspace) return;
    const deletedWorkspaceId = workspace.id;
    if (isImporting) {
      updateWorkspaceOperationState(deletedWorkspaceId, {
        deleteConfirmOpen: false,
        notice: "Cancel import before deleting this workspace.",
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
        clearWorkspaceOperationState(deletedWorkspaceId);
        setWorkspaceDeleteToast({
          workspaceId: deletedWorkspaceId,
          message: "Deleted workspace. Original PST was not deleted.",
          result,
        });
        const remainingSessions = openPstSessions.filter(
          (session) => session.workspace.id !== deletedWorkspaceId,
        );
        setOpenPstSessions(remainingSessions);
        if (allOpenFolderSelection.workspaceId === deletedWorkspaceId) {
          setAllOpenFolderSelection({ workspaceId: null, folderId: null });
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
    if (searchScope === "all_open" && openPstSessions.length > 1) {
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

  function rememberWorkspaceFolderSelection(selection: WorkspaceFolderSelection) {
    setOpenPstSessions((current) =>
      current.map((session) =>
        session.workspace.id === selection.workspaceId
          ? { ...session, folderSelection: selection }
          : session,
      ),
    );
  }

  function selectCurrentWorkspaceFolder(folderId: number | null) {
    if (!workspace) return;
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

  function clearSearchText() {
    setSearch("");
    setDebouncedSearch("");
  }

  function clearSearchFilters() {
    setSearchFilters({
      ...defaultAdvancedSearchFilters,
      folderScope: includeSubfolders ? "current_subfolders" : "current",
    });
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
    if (existing.isComplete) {
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
          Incomplete {existing.workspaceLocationLabel} workspace - {existing.importStatus}
        </strong>
        <small>{existing.workspacePath}</small>
        <div>
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
            Resume indexing existing extracted EMLs
          </button>
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
            onClick={() => void deletePlannedWorkspace(plan, existing)}
            disabled={isBusy || !existing.canReimport}
          >
            Delete workspace
          </button>
        </div>
      </div>
    );
  }

  const selectedFolder = folders.find((folder) => folder.id === selectedFolderId);
  const displayedFolderName =
    searchScope === "all_open"
      ? allOpenSelectedWorkspaceId
        ? `${pstDisplayName(allOpenSelectedSession?.workspace ?? null)} - ${
            allOpenSelectedFolderId == null
              ? "All Mail"
              : allOpenSelectedFolder?.name ?? "Folder"
          }`
        : "All Open PSTs"
      : searchFilters.folderScope === "all" || selectedFolderId == null
      ? "All Mail"
      : selectedFolder?.name ?? "Messages";
  const progress = visibleWorkspaceOperation?.progress ?? null;
  const workspaceOperationNotice = visibleWorkspaceOperation?.notice ?? null;
  const workspaceOperationError = visibleWorkspaceOperation?.error ?? null;
  const displayedError = workspaceOperationError ?? error;
  const displayedSetupCommand = workspaceOperationError
    ? visibleWorkspaceOperation?.setupCommand ?? null
    : setupCommand;
  const displayedNotice = workspaceOperationNotice ?? notice;
  const deleteResult = visibleWorkspaceOperation?.deleteResult ?? null;
  const deleteStatuses = activeWorkspaceOperation?.deleteStatuses ?? [];
  const deleteConfirmOpen = activeWorkspaceOperation?.deleteConfirmOpen ?? false;
  const activeWorkspaceOperationRunning = activeWorkspaceOperation?.running ?? false;
  const progressPercent =
    progress?.current != null && progress.total && progress.total > 0
      ? Math.round((progress.current / progress.total) * 100)
      : null;
  const progressCount =
    progress?.current != null && progress.total != null
      ? `Indexed ${formatCount(progress.current)} of ${formatCount(progress.total)} discovered EML files`
      : null;
  const pendingWorkspaceFlowVisible = pendingWorkspaceFlowBelongsToActiveTab;
  const visiblePendingOpenPlan = pendingWorkspaceFlowVisible ? pendingOpenPlan : null;
  const visiblePendingPreflightOpen = pendingWorkspaceFlowVisible ? pendingPreflightOpen : null;
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
  const remoteImagesAllowedForSelected =
    selectedMessage != null && remoteImagesAllowedMessageId === selectedMessage.id;
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
  const activeFilterChips = [
    search.trim() ? `Search: ${search.trim()}` : null,
    searchScope === "all_open"
      ? `PST scope: All Open PSTs (${formatCount(openPstSessions.length)})`
      : null,
    searchFilters.from.trim() ? `From: ${searchFilters.from.trim()}` : null,
    searchFilters.recipients.trim() ? `To/Cc/Bcc: ${searchFilters.recipients.trim()}` : null,
    searchFilters.subject.trim() ? `Subject: ${searchFilters.subject.trim()}` : null,
    searchFilters.body.trim() ? `Body: ${searchFilters.body.trim()}` : null,
    searchFilters.attachment.trim() ? `Attachment: ${searchFilters.attachment.trim()}` : null,
    searchFilters.hasAttachments === "yes"
      ? "Has attachments"
      : searchFilters.hasAttachments === "no"
        ? "No attachments"
        : null,
    searchFilters.dateFrom || searchFilters.dateTo
      ? `Date: ${searchFilters.dateFrom || "Any"} - ${searchFilters.dateTo || "Any"}`
      : null,
    searchScope === "all_open"
      ? allOpenSelectedWorkspaceId
        ? `Folder: ${pstDisplayName(allOpenSelectedSession?.workspace ?? null)} / ${
            allOpenSelectedFolderId == null
              ? "All Mail"
              : allOpenSelectedFolder?.name ?? "Selected folder"
          }${allOpenSelectedFolderId != null && includeSubfolders ? " + subfolders" : ""}`
        : "Folder scope: All Mail in each open PST"
      : searchFilters.folderScope === "all"
      ? "Scope: All Mail"
      : searchFilters.folderScope === "current"
        ? "Scope: Current folder"
        : selectedFolderId != null
          ? "Scope: Current folder + subfolders"
          : null,
  ].filter((chip): chip is string => Boolean(chip));
  const hasMoreMessages = messages.length < messageTotalCount;
  const hasMoreConversations = conversations.length < conversationTotalCount;
  const resultScopeSuffix = searchScope === "all_open"
    ? ` across ${formatCount(activeSearchWorkspaceIds.length)} PST${
        activeSearchWorkspaceIds.length === 1 ? "" : "s"
      }`
    : "";
  const messageResultSummaryText =
    messageTotalCount === 0
      ? `0 results${resultScopeSuffix}`
      : messages.length === messageTotalCount
        ? `Showing ${formatCount(messages.length)} of ${formatCount(messageTotalCount)} results${resultScopeSuffix}`
        : `Showing 1-${formatCount(messages.length)} of ${formatCount(messageTotalCount)} results${resultScopeSuffix}`;
  const conversationResultSummaryText =
    conversationTotalCount === 0
      ? `0 matching messages in 0 conversations${resultScopeSuffix}`
      : `${formatCount(conversationMatchingMessageCount)} matching message${
          conversationMatchingMessageCount === 1 ? "" : "s"
        } in ${formatCount(conversationTotalCount)} conversation${
          conversationTotalCount === 1 ? "" : "s"
        }${resultScopeSuffix}`;
  const resultSummaryText =
    listMode === "conversations" ? conversationResultSummaryText : messageResultSummaryText;
  const isInitializingMessageList =
    workspace != null && initializingWorkspaceId === workspace.id;
  const paneLayoutStyle = {
    "--folder-pane-width": `${paneWidths.folder}px`,
    "--message-pane-width": `${paneWidths.message}px`,
    "--outlook-message-height": `${paneWidths.outlookMessage}px`,
  } as CSSProperties;
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
              value={search}
              onChange={(event) => setSearch(event.target.value)}
              placeholder='Search, or use from:adam subject:calendar "exact phrase"'
              disabled={!workspace || isBusy}
              aria-label="Search emails"
            />
            <button
              type="button"
              className={advancedSearchOpen || hasActiveAdvancedFilters ? "filter-toggle active" : "filter-toggle"}
              onClick={() => setAdvancedSearchOpen((open) => !open)}
              disabled={!workspace || isBusy}
              title="Show advanced search filters."
            >
              Advanced
            </button>
            <label className="scope-control">
              <span>Scope</span>
              <select
                value={searchScope}
                onChange={(event) => setSearchScope(event.target.value as SearchScope)}
                disabled={!workspace || isBusy}
              >
                <option value="current">Current PST</option>
                <option value="all_open">All Open PSTs</option>
              </select>
            </label>
          </div>
          {advancedSearchOpen ? (
            <div className="advanced-search-panel" role="group" aria-label="Advanced search filters">
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
                  disabled={!workspace || isBusy || isCrossPstSearch}
                  title={
                    isCrossPstSearch ? "Cross-PST search uses All Mail in each open PST." : undefined
                  }
                >
                  <option value="current">Current folder</option>
                  <option value="current_subfolders">Current folder + subfolders</option>
                  <option value="all">All Mail</option>
                </select>
              </label>
              <div className="advanced-search-actions">
                <button type="button" onClick={clearSearchFilters} disabled={!workspace || isBusy}>
                  Clear Filters
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
            disabled={isBusy}
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
            disabled={isBusy || readpstStatus?.available !== true}
            title={
              readpstStatus?.available === false
                ? `Install readpst first: ${readpstStatus.setupCommand}`
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
            <details className="recent-menu">
              <summary title="Open a recently selected PST.">Recent</summary>
              <div className="recent-menu-panel">
                <div className="recent-menu-header">
                  <strong>Open Recent</strong>
                  <button type="button" onClick={clearRecentPsts} disabled={isBusy}>
                    Clear
                  </button>
                </div>
                <div className="recent-menu-items">
                  {recentPsts.map((recent) => (
                    <button
                      type="button"
                      key={recent.path}
                      onClick={() => void openRecentPst(recent.path)}
                      disabled={isBusy}
                      title={pathParent(recent.path)}
                    >
                      <span>{pathFileName(recent.path)}</span>
                      <small>{pathParent(recent.path)}</small>
                    </button>
                  ))}
                </div>
              </div>
            </details>
          ) : null}
          <button
            type="button"
            className="help-action"
            onClick={() => setAboutOpen(true)}
            title="About PST QuickView and safety notes."
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
              Opens existing complete workspaces only. Missing external drives are skipped.
            </span>
            {restoreStatus ? <span>{restoreStatus}</span> : null}
          </div>
          <div className="restore-session-actions">
            <button
              type="button"
              className="primary-action"
              onClick={() => void restorePreviousSession()}
              disabled={isBusy}
            >
              Restore
            </button>
            <button type="button" onClick={startFreshPreviousSession} disabled={isBusy}>
              Start Fresh
            </button>
          </div>
        </section>
      ) : null}

      {readpstStatus?.available === false ? (
        <section className="notice-box" role="status">
          Install libpst before opening a PST: <code>{readpstStatus.setupCommand}</code>
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
                      disabled={isBusy && !canSwitchTabsDuringOperation}
                    >
                      {pstDisplayName(session.workspace)} {formatCount(session.workspace.messageCount)}
                    </button>
                    <button
                      type="button"
                      className="open-pst-tab-close"
                      onClick={() => void closeWorkspaceTab(session.workspace.id)}
                      disabled={isBusy || isImporting}
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
            <details className="workspace-menu">
              <summary>Workspace</summary>
              <div className="workspace-menu-panel">
                <button
                  type="button"
                  onClick={revealOriginalPst}
                  disabled={!workspace}
                  title="Reveal the original PST file in Finder."
                >
                  Reveal PST
                </button>
                <button
                  type="button"
                  onClick={revealWorkspace}
                  disabled={!workspace}
                  title="Reveal the current PST QuickView workspace/cache in Finder."
                >
                  Reveal Workspace
                </button>
                <button
                  type="button"
                  onClick={revealBothFolders}
                  disabled={!workspace}
                  title="Open the original PST location and workspace location in Finder."
                >
                  Open Both
                </button>
                <button type="button" onClick={revealImportLog} title="Reveal the current import log in Finder.">
                  Import Log
                </button>
                <button
                  type="button"
                  className="danger-action"
                  onClick={deleteCurrentWorkspace}
                  disabled={!workspace || isBusy || isImporting}
                  title={
                    isImporting
                      ? "Cancel import before deleting this workspace."
                      : "Delete the current PST QuickView workspace/cache. The original PST is not deleted."
                  }
                >
                  Delete Workspace
                </button>
              </div>
            </details>
            <details className="layout-controls">
              <summary>Layout</summary>
              <div className="layout-control-grid">
                <label>
                  <span>View</span>
                  <select
                    value={listMode}
                    onChange={(event) => setListMode(event.target.value as ListMode)}
                  >
                    <option value="messages">Messages</option>
                    <option value="conversations">Conversations</option>
                  </select>
                </label>
                <label>
                  <span>Mode</span>
                  <select
                    value={layoutMode}
                    onChange={(event) => setLayoutMode(event.target.value as LayoutMode)}
                  >
                    <option value="three_column">Three Column</option>
                    <option value="outlook">Outlook Style</option>
                  </select>
                </label>
                <label>
                  <span>List</span>
                  <select
                    value={messageListDisplayMode}
                    onChange={(event) =>
                      setMessageListDisplayMode(event.target.value as MessageListDisplayMode)
                    }
                  >
                    <option value="subject_first">Subject first</option>
                    <option value="sender_first">Sender first</option>
                  </select>
                </label>
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
                  onClick={() => setFoldersHidden((current) => !current)}
                >
                  {foldersHidden ? "Show Folders" : "Hide Folders"}
                </button>
                <button type="button" onClick={collapseAllFolders} disabled={!workspace || !folderTree.length}>
                  Collapse All
                </button>
                <button type="button" onClick={expandAllFolders} disabled={!workspace || !folderTree.length}>
                  Expand All
                </button>
                <button type="button" onClick={resetPaneLayout} title="Reset folder and message list widths.">
                  Reset
                </button>
              </div>
            </details>
            {workspaceSize ? (
              <details className="workspace-details">
                <summary>Details</summary>
                <dl>
                  <div>
                    <dt>Mode</dt>
                    <dd>{workspaceSize.workspaceLocationLabel}</dd>
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
                </dl>
              </details>
            ) : null}
          </div>
        </section>
      ) : null}

      {isSearchActive ? (
        <section className="search-summary" aria-live="polite">
          <strong>
            {isSearching ? "Searching..." : resultSummaryText}
          </strong>
          <div className="filter-chips">
            {activeFilterChips.map((chip) => (
              <span key={chip}>{chip}</span>
            ))}
            {isCrossPstSearch
              ? workspaceSearchCounts
                  .filter((count) => count.count > 0)
                  .map((count) => (
                    <span key={count.workspaceId}>
                      {count.pstDisplayName}: {formatCount(count.count)}
                    </span>
                  ))
              : null}
          </div>
          <div className="search-summary-actions">
            {search.trim() ? (
              <button type="button" onClick={clearSearchText} disabled={isBusy}>
                Clear Search
              </button>
            ) : null}
            {hasActiveAdvancedFilters ? (
              <button type="button" onClick={clearSearchFilters} disabled={isBusy}>
                Clear Filters
              </button>
            ) : null}
          </div>
        </section>
      ) : null}

      {progress ? (
        <section className="progress-strip">
          <div className="progress-label">
            <span>{progress.stage}</span>
            <span>{progressPercent != null ? `${progressPercent}%` : progress.message}</span>
          </div>
          <div className="progress-track">
            <div
              className="progress-fill"
              style={{ width: `${progressPercent ?? (isBusy ? 35 : 100)}%` }}
            />
          </div>
          <div className="progress-detail-row">
            <p>{progressCount ?? progress.message}</p>
            {visibleWorkspaceOperation?.running ? (
              <button
                type="button"
                className="danger-action"
                onClick={() => void cancelImport()}
              >
                Cancel Import
              </button>
            ) : null}
          </div>
        </section>
      ) : null}

      {workspace && activeWorkspaceOperationRunning ? (
        <section className="notice-box" role="status">
          Cancel import before deleting this workspace.
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

      {deleteConfirmOpen && workspace ? (
        <section className="delete-confirm-box" role="dialog" aria-label="Confirm workspace delete">
          <div>
            <strong>Delete current workspace?</strong>
            <p>{workspace.workspacePath}</p>
            <p>The original PST will not be deleted.</p>
          </div>
          <div className="delete-confirm-actions">
            <button type="button" onClick={cancelDeleteCurrentWorkspace} disabled={isBusy}>
              Cancel
            </button>
            <button
              type="button"
              className="danger-action"
              onClick={() => void confirmDeleteCurrentWorkspace()}
              disabled={isBusy}
            >
              Yes, delete workspace
            </button>
          </div>
        </section>
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
                  ? "Deleted workspace. Original PST was not deleted."
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
                    disabled={!workspace}
                  />
                  <span title="Include messages from child folders">Subfolders</span>
                </label>
                <button type="button" onClick={collapseAllFolders} disabled={!workspace}>
                  Collapse
                </button>
                <button type="button" onClick={expandAllFolders} disabled={!workspace}>
                  Expand
                </button>
              </div>
            </div>
            <div className="folder-tree">
              {workspace ? (
                searchScope === "all_open" && openPstSessions.length > 0 ? (
                  <>
                    <button
                      type="button"
                      className={`folder-row all-mail-row ${
                        allOpenFolderSelection.workspaceId == null ? "selected" : ""
                      }`}
                      onClick={() => setAllOpenFolderSelection({ workspaceId: null, folderId: null })}
                      title="All indexed messages across open PSTs"
                    >
                      <span className="folder-name">All Open PSTs</span>
                      <span className="folder-count">
                        {formatCount(
                          openPstSessions.reduce(
                            (total, session) => total + session.workspace.messageCount,
                            0,
                          ),
                        )}
                      </span>
                    </button>
                    {openSessionFolderTrees.map(({ session, tree }) => {
                      const workspaceId = session.workspace.id;
                      const rootCollapsed = collapsedFolderPaths.has(
                        workspaceRootCollapseKey(workspaceId),
                      );
                      const rootSelected =
                        allOpenFolderSelection.workspaceId === workspaceId &&
                        allOpenFolderSelection.folderId == null;
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
                            >
                              {rootCollapsed ? ">" : "v"}
                            </button>
                            <button
                              type="button"
                              className="folder-select-button"
                              onClick={() => selectAllOpenWorkspaceFolder(workspaceId, null)}
                              title={session.workspace.pstPath}
                            >
                              <span className="folder-name">{pstDisplayName(session.workspace)}</span>
                              <span className="folder-count">
                                {formatCount(session.workspace.messageCount)}
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
                              >
                                <span className="folder-name">All Mail</span>
                                <span className="folder-count">
                                  {formatCount(session.workspace.messageCount)}
                                </span>
                              </button>
                              <FolderRows
                                nodes={tree}
                                workspaceId={workspaceId}
                                selectedWorkspaceId={allOpenFolderSelection.workspaceId}
                                selectedFolderId={allOpenFolderSelection.folderId}
                                includeSubfolders={includeSubfolders}
                                collapsedFolderPaths={collapsedFolderPaths}
                                onToggleCollapse={toggleFolderCollapsed}
                                onSelect={(folder, selectedWorkspaceId) =>
                                  selectAllOpenWorkspaceFolder(selectedWorkspaceId, folder.id)
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
                      className={`folder-row all-mail-row ${selectedFolderId == null ? "selected" : ""}`}
                      onClick={() => selectCurrentWorkspaceFolder(null)}
                      title="All indexed messages in this PST"
                    >
                      <span className="folder-name">All Mail</span>
                      <span className="folder-count">{workspace.messageCount}</span>
                    </button>
                    <FolderRows
                      nodes={folderTree}
                      workspaceId={workspace.id}
                      selectedWorkspaceId={workspace.id}
                      selectedFolderId={selectedFolderId}
                      includeSubfolders={includeSubfolders}
                      collapsedFolderPaths={collapsedFolderPaths}
                      onToggleCollapse={toggleFolderCollapsed}
                      onSelect={(folder) => selectCurrentWorkspaceFolder(folder.id)}
                    />
                  </>
                )
              ) : (
                <p className="empty-state">No PST opened.</p>
              )}
            </div>
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
                {listMode === "conversations" ? "Conversations - " : ""}
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
                    <select
                      value={sortOrder}
                      onChange={(event) => setSortOrder(event.target.value as SortOrder)}
                      disabled={isBusy}
                    >
                      <option value="newest">Newest first</option>
                      <option value="oldest">Oldest first</option>
                      <option value="sender_az">Sender A-Z</option>
                      <option value="subject_az">Subject A-Z</option>
                    </select>
                  )}
                </label>
              ) : null}
            </div>
          </div>
          <div className="message-list" ref={messageListRef}>
            {listMode === "conversations" && conversationWorkspaceIssues.length ? (
              <div className="conversation-index-warning">
                <strong>Conversation data is not indexed for this workspace.</strong>
                {conversationWorkspaceIssues.map((issue) => (
                  <div key={issue.workspaceId}>
                    <span title={issue.workspacePath}>{issue.pstDisplayName}</span>
                    <button
                      type="button"
                      onClick={() => void reindexExistingEmls(issue.workspaceId)}
                      disabled={!issue.canReindex || isBusy}
                    >
                      Reindex Existing EMLs
                    </button>
                  </div>
                ))}
              </div>
            ) : null}
            {listMode === "conversations"
              ? conversations.map((conversation) => {
                  const key = conversationKey(
                    conversation.workspaceId,
                    conversation.conversationId,
                  );
                  const participantSummary = conversationParticipantSummary(
                    conversation.participants,
                    conversation.latestSender,
                  );
                  const participantTitle =
                    conversation.participants.length > 0
                      ? conversation.participants.join(", ")
                      : conversation.latestSender || "(No sender)";
                  const expanded = expandedConversations[key];
                  const expectedExpandedCount = expanded?.showingEntireConversation
                    ? expanded.totalMessageCount
                    : expanded?.matchingMessageCount;
                  return (
                    <div className="conversation-group" key={key}>
                      <button
                        type="button"
                        className={`conversation-row ${expanded ? "expanded" : ""}`}
                        onClick={() => toggleConversation(conversation)}
                        aria-expanded={Boolean(expanded)}
                      >
                        <span className="conversation-toggle" aria-hidden="true">
                          {expanded ? "v" : ">"}
                        </span>
                        <span className="conversation-main">
                          <span className="conversation-subject-line">
                            <span className="conversation-subject">
                              {conversation.subject || "(No subject)"}
                            </span>
                            {isCrossPstSearch ? (
                              <span className="message-workspace" title={conversation.workspacePath}>
                                {conversation.pstDisplayName}
                              </span>
                            ) : null}
                          </span>
                          <span
                            className="conversation-participants"
                            title={participantTitle}
                            aria-label={`Participants: ${participantTitle}`}
                          >
                            {participantSummary}
                          </span>
                          <span className="conversation-snippet">
                            <HighlightedText
                              text={cleanMessageSnippet(conversation.snippet)}
                              terms={highlightTerms}
                            />
                          </span>
                        </span>
                        <span className="conversation-meta">
                          <span title={conversation.latestDate || undefined}>
                            {formatMessageDate(conversation.latestDate)}
                          </span>
                          <span>
                            {formatCount(conversation.matchingMessageCount)} match
                            {conversation.matchingMessageCount === 1 ? "" : "es"} / {formatCount(
                              conversation.totalMessageCount,
                            )}
                          </span>
                          {conversation.hasAttachments ? <span>Attachments</span> : null}
                        </span>
                      </button>
                      {expanded ? (
                        <div className="conversation-expanded">
                          {expanded.items.map((message) => {
                            const isSelectedRow =
                              selectedMessage?.id === message.id &&
                              (selectedMessage.workspaceId ?? workspace?.id) ===
                                conversation.workspaceId;
                            return (
                              <button
                                type="button"
                                key={`${conversation.workspaceId}-${message.id}`}
                                className={`conversation-message-row ${
                                  isSelectedRow ? "selected" : ""
                                } ${message.matchesScope ? "" : "context-message"}`}
                                onClick={() => void openMessage(message.id, conversation.workspaceId)}
                                onDoubleClick={() =>
                                  void openMessagePreviewWindow(message, conversation.workspaceId)
                                }
                              >
                                <span className="conversation-message-sender">
                                  {message.sender || "(No sender)"}
                                </span>
                                <span className="conversation-message-date" title={message.date}>
                                  {formatMessageDate(message.date)}
                                </span>
                                <span className="conversation-message-folder">
                                  {message.folderPath || message.folderName}
                                  {!message.matchesScope ? " - context" : ""}
                                </span>
                                {message.attachmentCount > 0 ? (
                                  <span className="conversation-message-attachments">
                                    {formatCount(message.attachmentCount)} attachment
                                    {message.attachmentCount === 1 ? "" : "s"}
                                  </span>
                                ) : null}
                              </button>
                            );
                          })}
                          {expanded.loading ? <p className="list-count-note">Loading messages...</p> : null}
                          {expanded.error ? (
                            <p className="conversation-error">{expanded.error}</p>
                          ) : null}
                          <div className="conversation-actions">
                            {!expanded.showingEntireConversation &&
                            expanded.totalMessageCount > expanded.matchingMessageCount ? (
                              <button
                                type="button"
                                onClick={() =>
                                  void loadConversationMessages(conversation, true, false)
                                }
                                disabled={expanded.loading}
                              >
                                Show Entire Conversation
                              </button>
                            ) : null}
                            {expanded.items.length < (expectedExpandedCount ?? 0) ? (
                              <button
                                type="button"
                                onClick={() =>
                                  void loadConversationMessages(
                                    conversation,
                                    expanded.showingEntireConversation,
                                    true,
                                  )
                                }
                                disabled={expanded.loading}
                              >
                                Load More Messages
                              </button>
                            ) : null}
                          </div>
                        </div>
                      ) : null}
                    </div>
                  );
                })
              : messages.map((message) => {
              const displayDate = formatMessageDate(message.date);
              const displaySnippet = cleanMessageSnippet(message.snippet);
              const rowWorkspaceId = message.workspaceId ?? workspace?.id;
              const isSelectedRow =
                selectedMessage?.id === message.id &&
                (rowWorkspaceId ?? null) === (selectedMessage.workspaceId ?? workspace?.id ?? null);
              const primaryText =
                messageListDisplayMode === "sender_first"
                  ? message.sender || "(No sender)"
                  : message.subject || "(No subject)";
              const secondaryText =
                messageListDisplayMode === "sender_first"
                  ? message.subject || "(No subject)"
                  : message.sender || "(No sender)";
              return (
                <button
                  type="button"
                  key={`${rowWorkspaceId ?? "workspace"}-${message.id}`}
                  className={`message-row ${isSelectedRow ? "selected" : ""}`}
                  onClick={() => void openMessage(message.id, rowWorkspaceId)}
                  onDoubleClick={() => void openMessagePreviewWindow(message, rowWorkspaceId)}
                >
                  <span className="message-subject">
                    {primaryText}
                    {message.attachmentCount > 0 ? (
                      <span className="attachment-dot">📎 {formatCount(message.attachmentCount)}</span>
                    ) : null}
                  </span>
                  {isCrossPstSearch && message.pstDisplayName ? (
                    <span className="message-workspace" title={message.workspacePath || undefined}>
                      {message.pstDisplayName}
                      {message.folderPath ? ` - ${message.folderPath}` : ""}
                    </span>
                  ) : null}
                  <span className="message-meta">{secondaryText}</span>
                  <span className="message-snippet" title={message.snippet || undefined}>
                    <HighlightedText text={displaySnippet} terms={highlightTerms} />
                  </span>
                  <span className="message-date" title={message.date || undefined}>
                    {displayDate}
                  </span>
                </button>
              );
            })}
            {listMode === "conversations" && !conversations.length ? (
              <p className="empty-state">
                {isInitializingMessageList
                  ? "Loading messages..."
                  : isSearching
                    ? "Searching..."
                  : conversationWorkspaceIssues.length && conversationIndexedWorkspaceCount === 0
                    ? "Reindex existing EMLs to enable Conversation View."
                    : isSearchActive
                      ? "No conversations match this search."
                      : "No conversations to show."}
              </p>
            ) : null}
            {listMode === "messages" && !messages.length ? (
              <p className="empty-state">
                {isInitializingMessageList
                  ? "Loading messages..."
                  : isSearching
                    ? "Searching..."
                  : isSearchActive
                    ? "No results match this search."
                    : "No messages to show."}
              </p>
            ) : null}
            {listMode === "conversations" && hasMoreConversations ? (
              <div className="load-more-row">
                <button
                  type="button"
                  onClick={() => void loadConversationsPage(true)}
                  disabled={isSearching || isLoadingMoreConversations}
                >
                  {isLoadingMoreConversations
                    ? "Loading..."
                    : `Load More (${formatCount(
                        conversationTotalCount - conversations.length,
                      )} remaining)`}
                </button>
                <span>{conversationResultSummaryText}</span>
              </div>
            ) : listMode === "conversations" && conversations.length ? (
              <p className="list-count-note">{conversationResultSummaryText}</p>
            ) : listMode === "messages" && hasMoreMessages ? (
              <div className="load-more-row">
                <button
                  type="button"
                  onClick={() => void loadMessagesPage(true)}
                  disabled={isSearching || isLoadingMoreMessages}
                >
                  {isLoadingMoreMessages
                    ? "Loading..."
                    : `Load More (${formatCount(messageTotalCount - messages.length)} remaining)`}
                </button>
                <span>{resultSummaryText}</span>
              </div>
            ) : listMode === "messages" && messages.length ? (
              <p className="list-count-note">{messageResultSummaryText}</p>
            ) : null}
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
            <div className="message-preview" ref={previewScrollRef}>
              <h3>{selectedMessage.subject || "(No subject)"}</h3>
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
                      onClick={() => setReaderMode("plain_text")}
                    >
                      Plain Text
                    </button>
                    <button
                      type="button"
                      className={readerMode === "sanitized_html" ? "selected" : ""}
                      onClick={() => setReaderMode("sanitized_html")}
                    >
                      Sanitized HTML
                    </button>
                  </div>
                  <div className="reader-actions" role="toolbar" aria-label="Message actions">
                    <button
                      type="button"
                      onClick={() => void openSourceEml(false)}
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
                    {htmlRender?.remoteImagesBlocked && !remoteImagesAllowedForSelected ? (
                      <div className="remote-image-notice">
                        <span>Remote images blocked.</span>
                        <button
                          type="button"
                          onClick={() => setRemoteImagesAllowedMessageId(selectedMessage.id)}
                        >
                          Load remote images for this message
                        </button>
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
        <section className="about-modal" role="dialog" aria-modal="true" aria-label="About PST QuickView">
          <header className="about-header">
            <div>
              <h2>About PST QuickView</h2>
              <p>Read-only local PST viewer</p>
            </div>
            <button type="button" onClick={closeAbout} title="Close About.">
              Close
            </button>
          </header>
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
                no telemetry or cloud service. Workspaces are local caches and can be deleted safely.
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
                {sourceEmlView.remoteImagesBlocked && !sourceEmlRemoteAllowed ? (
                  <div className="remote-image-notice">
                    <span>Remote resources are blocked.</span>
                    <button type="button" onClick={() => void loadSourceEmlForCurrentContext(true)}>
                      Load remote resources for this message
                    </button>
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
