import type { SearchScope } from "./searchScopePreference";
import type { SearchFilters } from "./types";

export type SearchListMode = "messages" | "conversations";
export type MessageSearchSort =
  | "newest"
  | "oldest"
  | "sender_az"
  | "subject_az"
  | "relevance";
export type ConversationSearchSort = "newest" | "oldest" | "subject";
export type SearchFolderScope = "current" | "current_subfolders" | "all";
export type HasAttachmentsFilter = "any" | "yes" | "no";

export type SearchFilterDraft = {
  from: string;
  recipients: string;
  subject: string;
  body: string;
  attachment: string;
  hasAttachments: HasAttachmentsFilter;
  dateFrom: string;
  dateTo: string;
  folderScope: SearchFolderScope;
};

export type SearchDraft = SearchFilterDraft & {
  query: string;
};

export type SearchConversationScope = {
  workspaceId: string;
  folderId: number | null;
  includeSubfolders: boolean;
};

export type SearchSnapshotContext = {
  scope: SearchScope;
  activeWorkspaceId: string | null;
  workspaceIds: string[];
  selectedWorkspaceId: string | null;
  useMultiWorkspace: boolean;
  singleWorkspaceId: string | null;
  folderId: number | null;
  includeSubfolders: boolean;
  conversationScopes: SearchConversationScope[];
  listMode: SearchListMode;
  messageSort: MessageSearchSort;
  conversationSort: ConversationSearchSort;
  sessionGeneration: number;
};

export type AppliedSearchSnapshot = SearchDraft & SearchSnapshotContext;

export type AppliedSearchVersion = {
  snapshot: AppliedSearchSnapshot;
  generation: number;
};

export type SearchOperationLane =
  | "message-page"
  | "message-count"
  | "message-load-more"
  | "conversation-page"
  | "conversation-count"
  | "conversation-load-more"
  | "expanded-conversation";

export type SearchOperationIdentity = {
  generation: number;
  operationId: string;
  lane: SearchOperationLane;
};

export type SearchCancellationRequest = {
  generation: number;
  operationId: string | null;
};

export const searchCancelledErrorCode = "SEARCH_CANCELLED";

export type SearchExactCountStatus = "idle" | "pending" | "ready" | "unavailable";

export type SearchExactCountState = {
  generation: number;
  status: SearchExactCountStatus;
};

export type MessagePaginationMode = "cursor" | "offset";

export type MessagePaginationState = {
  generation: number;
  mode: MessagePaginationMode;
  nextCursor: string | null;
};

export function getMessagePaginationMode(
  snapshot: AppliedSearchSnapshot,
): MessagePaginationMode {
  const effectiveWorkspaceCount = snapshot.useMultiWorkspace
    ? uniqueWorkspaceIds(snapshot.workspaceIds).length
    : snapshot.singleWorkspaceId
      ? 1
      : 0;
  return effectiveWorkspaceCount === 1 ? "cursor" : "offset";
}

export function createMessagePaginationState(
  snapshot: AppliedSearchSnapshot,
  generation: number,
): MessagePaginationState {
  return {
    generation,
    mode: getMessagePaginationMode(snapshot),
    nextCursor: null,
  };
}

export function cursorForMessagePage(
  state: MessagePaginationState,
  requestGeneration: number,
  append: boolean,
): string | null {
  if (
    !append ||
    state.generation !== requestGeneration ||
    state.mode !== "cursor"
  ) {
    return null;
  }
  return state.nextCursor;
}

export function settleMessagePagination(
  current: MessagePaginationState,
  currentGeneration: number,
  requestGeneration: number,
  responseMode: MessagePaginationMode,
  hasMore: boolean,
  nextCursor: string | null,
): MessagePaginationState {
  if (
    currentGeneration !== requestGeneration ||
    current.generation !== requestGeneration ||
    current.mode !== responseMode
  ) {
    return current;
  }
  return {
    generation: requestGeneration,
    mode: responseMode,
    nextCursor: responseMode === "cursor" && hasMore ? nextCursor : null,
  };
}

export function createSearchExactCountState(
  generation: number,
  status: SearchExactCountStatus = "idle",
): SearchExactCountState {
  return { generation, status };
}

export function settleSearchExactCount(
  current: SearchExactCountState,
  currentGeneration: number,
  requestGeneration: number,
  status: Exclude<SearchExactCountStatus, "idle" | "pending">,
): SearchExactCountState {
  if (requestGeneration !== currentGeneration || current.generation !== requestGeneration) {
    return current;
  }
  return { generation: requestGeneration, status };
}

export type RelevanceUnavailableReason =
  | "no_workspace"
  | "requires_text"
  | "multiple_workspaces"
  | "conversations";

export type RelevanceAvailability = {
  available: boolean;
  reason: RelevanceUnavailableReason | null;
  explanation: string;
  effectiveWorkspaceCount: number;
};

export type AppliedFilterChipId =
  | "query"
  | "scope"
  | "from"
  | "recipients"
  | "subject"
  | "body"
  | "attachment"
  | "hasAttachments"
  | "dateFrom"
  | "dateTo"
  | "folderScope"
  | "folderSelection";

export type AppliedFilterChip = {
  id: AppliedFilterChipId;
  label: string;
  value: string;
  text: string;
  removeLabel: string;
};

export type AppliedFilterChipContext = {
  workspaceCount?: number;
  workspaceLabel?: string | null;
  folderLabel?: string | null;
};

export type SearchFilterRemoval = {
  changed: boolean;
  draft: SearchDraft;
  scope: SearchScope;
  clearFolderSelection: boolean;
};

export type ClearAllSearchResult = {
  changed: boolean;
  draft: SearchDraft;
  scope: SearchScope;
  messageSort: MessageSearchSort;
  conversationSort: ConversationSearchSort;
  clearAllOpenFolderSelection: boolean;
};

export type SearchEmptyStateKind =
  | "none"
  | "no_workspace"
  | "inactive_empty"
  | "loading"
  | "no_matches"
  | "failed";

export type SearchEmptyState = {
  kind: SearchEmptyStateKind;
  title: string;
  detail: string | null;
};

export type SearchEmptyStateInput = {
  hasWorkspace: boolean;
  isSearchActive: boolean;
  isLoading: boolean;
  resultCount: number;
  listMode: SearchListMode;
  scope: SearchScope;
  activeFilterCount: number;
  errorGeneration: number | null;
  currentGeneration: number;
};

type PendingSearchSnapshot = {
  token: number;
  snapshot: AppliedSearchSnapshot;
};

export type SearchApplicationState = {
  applied: AppliedSearchVersion;
  pending: PendingSearchSnapshot | null;
  nextToken: number;
};

export type QueuedSearchSnapshot = {
  state: SearchApplicationState;
  token: number | null;
};

export type CommittedSearchSnapshot = {
  state: SearchApplicationState;
  applied: boolean;
};

const textKeys = ["query", "from", "recipients", "subject", "body", "attachment"] as const;

export const defaultSearchFilterDraft: SearchFilterDraft = {
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

export function emptySearchDraft(
  folderScope: SearchFolderScope = "current_subfolders",
): SearchDraft {
  return {
    query: "",
    ...defaultSearchFilterDraft,
    folderScope,
  };
}

function normalizeDraft(draft: SearchDraft): SearchDraft {
  return {
    query: draft.query.trim(),
    from: draft.from.trim(),
    recipients: draft.recipients.trim(),
    subject: draft.subject.trim(),
    body: draft.body.trim(),
    attachment: draft.attachment.trim(),
    hasAttachments: draft.hasAttachments,
    dateFrom: draft.dateFrom.trim(),
    dateTo: draft.dateTo.trim(),
    folderScope: draft.folderScope,
  };
}

function uniqueWorkspaceIds(workspaceIds: string[]): string[] {
  return Array.from(new Set(workspaceIds.filter((workspaceId) => workspaceId.trim())));
}

type SearchTextToken = {
  value: string;
  quoted: boolean;
};

const textSearchFieldNames = new Set([
  "from",
  "to",
  "cc",
  "bcc",
  "recipient",
  "recipients",
  "subject",
  "subj",
  "body",
  "text",
  "attachment",
  "attach",
  "filename",
]);
const nonTextSearchFieldNames = new Set(["has", "after", "before"]);

function tokenizeSearchText(value: string): SearchTextToken[] {
  const tokens: SearchTextToken[] = [];
  let current = "";
  let inQuote = false;
  let currentQuoted = false;

  const pushCurrent = () => {
    const token = current.trim();
    if (token) tokens.push({ value: token, quoted: currentQuoted });
    current = "";
    currentQuoted = false;
  };

  for (const character of value) {
    if (character === '"') {
      if (inQuote) {
        pushCurrent();
        inQuote = false;
      } else {
        pushCurrent();
        inQuote = true;
        currentQuoted = true;
      }
    } else if (/\s/u.test(character) && !inQuote) {
      pushCurrent();
    } else {
      current += character;
    }
  }
  pushCurrent();
  return tokens;
}

function hasSearchableScalar(value: string): boolean {
  return /[\p{L}\p{N}]/u.test(value);
}

function splitInlineTypedToken(value: string): [string, string] | null {
  const separator = value.indexOf(":");
  if (separator <= 0 || separator === value.length - 1) return null;
  const key = value.slice(0, separator);
  if (!/^[A-Za-z]+$/.test(key)) return null;
  return [key.toLowerCase(), value.slice(separator + 1)];
}

function queryProducesFtsText(query: string): boolean {
  const tokens = tokenizeSearchText(query);
  for (let index = 0; index < tokens.length; index += 1) {
    const token = tokens[index];
    if (!token.quoted && token.value.endsWith(":")) {
      const next = tokens[index + 1];
      if (next) {
        const key = token.value.slice(0, -1).toLowerCase();
        index += 1;
        if (nonTextSearchFieldNames.has(key)) continue;
        if (textSearchFieldNames.has(key)) {
          if (hasSearchableScalar(next.value)) return true;
          continue;
        }
        if (hasSearchableScalar(`${key}:${next.value}`)) return true;
        continue;
      }
    }

    if (!token.quoted) {
      const typed = splitInlineTypedToken(token.value);
      if (typed) {
        const [key, value] = typed;
        if (nonTextSearchFieldNames.has(key)) continue;
        if (textSearchFieldNames.has(key)) {
          if (hasSearchableScalar(value)) return true;
          continue;
        }
        // Unknown typed fields retain the current backend behavior and become FTS text.
        return true;
      }
    }

    if (hasSearchableScalar(token.value)) return true;
  }
  return false;
}

export function hasAppliedFtsText(snapshot: AppliedSearchSnapshot): boolean {
  return (
    queryProducesFtsText(snapshot.query) ||
    [snapshot.from, snapshot.recipients, snapshot.subject, snapshot.body, snapshot.attachment].some(
      hasSearchableScalar,
    )
  );
}

export function getRelevanceAvailability(
  snapshot: AppliedSearchSnapshot,
): RelevanceAvailability {
  const effectiveWorkspaceCount = snapshot.useMultiWorkspace
    ? uniqueWorkspaceIds(snapshot.workspaceIds).length
    : snapshot.singleWorkspaceId
      ? 1
      : 0;

  if (snapshot.listMode === "conversations") {
    return {
      available: false,
      reason: "conversations",
      explanation: "Relevance is not available in Conversations.",
      effectiveWorkspaceCount,
    };
  }
  if (effectiveWorkspaceCount === 0) {
    return {
      available: false,
      reason: "no_workspace",
      explanation: "Relevance requires an open PST.",
      effectiveWorkspaceCount,
    };
  }
  if (effectiveWorkspaceCount !== 1) {
    return {
      available: false,
      reason: "multiple_workspaces",
      explanation: "Relevance is available only when searching one PST.",
      effectiveWorkspaceCount,
    };
  }
  if (!hasAppliedFtsText(snapshot)) {
    return {
      available: false,
      reason: "requires_text",
      explanation: "Relevance requires a text search.",
      effectiveWorkspaceCount,
    };
  }
  return {
    available: true,
    reason: null,
    explanation: "Relevance ranks text matches within this PST.",
    effectiveWorkspaceCount,
  };
}

export function normalizeRelevanceSort(
  snapshot: AppliedSearchSnapshot,
): AppliedSearchSnapshot {
  if (snapshot.messageSort !== "relevance" || getRelevanceAvailability(snapshot).available) {
    return snapshot;
  }
  return { ...snapshot, messageSort: "newest" };
}

export function createAppliedSearchSnapshot(
  draft: SearchDraft,
  context: SearchSnapshotContext,
): AppliedSearchSnapshot {
  return normalizeRelevanceSort({
    ...normalizeDraft(draft),
    ...context,
    workspaceIds: uniqueWorkspaceIds(context.workspaceIds),
    conversationScopes: context.conversationScopes.map((scope) => ({ ...scope })),
  });
}

export function replaceAppliedSearchDraft(
  snapshot: AppliedSearchSnapshot,
  draft: SearchDraft,
): AppliedSearchSnapshot {
  return normalizeRelevanceSort({
    ...snapshot,
    ...normalizeDraft(draft),
  });
}

export function clearAppliedSearchSnapshot(
  snapshot: AppliedSearchSnapshot,
  scope: SearchScope = snapshot.scope,
): AppliedSearchSnapshot {
  return normalizeRelevanceSort({
    ...snapshot,
    ...emptySearchDraft(),
    scope,
    activeWorkspaceId: null,
    workspaceIds: [],
    selectedWorkspaceId: null,
    useMultiWorkspace: false,
    singleWorkspaceId: null,
    folderId: null,
    includeSubfolders: false,
    conversationScopes: [],
  });
}

export function appliedSearchSnapshotKey(snapshot: AppliedSearchSnapshot): string {
  return JSON.stringify(snapshot);
}

export function appliedSearchTextKey(snapshot: AppliedSearchSnapshot): string {
  return JSON.stringify(textKeys.map((key) => snapshot[key]));
}

export function appliedSearchNonTextKey(snapshot: AppliedSearchSnapshot): string {
  const {
    query: _query,
    from: _from,
    recipients: _recipients,
    subject: _subject,
    body: _body,
    attachment: _attachment,
    ...nonText
  } = snapshot;
  return JSON.stringify(nonText);
}

export function hasAdvancedSearchValues(filters: SearchFilterDraft): boolean {
  return Boolean(
    filters.from.trim() ||
      filters.recipients.trim() ||
      filters.subject.trim() ||
      filters.body.trim() ||
      filters.attachment.trim() ||
      filters.hasAttachments !== "any" ||
      filters.dateFrom.trim() ||
      filters.dateTo.trim() ||
      filters.folderScope !== "current_subfolders",
  );
}

export function isAppliedSearchActive(snapshot: AppliedSearchSnapshot): boolean {
  return Boolean(snapshot.query || hasAdvancedSearchValues(snapshot));
}

export function backendSearchFilters(snapshot: AppliedSearchSnapshot): SearchFilters {
  const valueOrNull = (value: string) => value || null;
  return {
    from: valueOrNull(snapshot.from),
    recipients: valueOrNull(snapshot.recipients),
    subject: valueOrNull(snapshot.subject),
    body: valueOrNull(snapshot.body),
    attachment: valueOrNull(snapshot.attachment),
    hasAttachments: snapshot.hasAttachments,
    dateFrom: valueOrNull(snapshot.dateFrom),
    dateTo: valueOrNull(snapshot.dateTo),
  };
}

function filterChip(
  id: AppliedFilterChipId,
  label: string,
  value: string,
): AppliedFilterChip {
  return {
    id,
    label,
    value,
    text: `${label}: ${value}`,
    removeLabel: `Remove ${label} filter ${value}`,
  };
}

export function getActiveFilterCount(snapshot: AppliedSearchSnapshot): number {
  let count = 0;
  for (const value of [
    snapshot.from,
    snapshot.recipients,
    snapshot.subject,
    snapshot.body,
    snapshot.attachment,
  ]) {
    if (value) count += 1;
  }
  if (snapshot.hasAttachments !== "any") count += 1;
  if (snapshot.dateFrom) count += 1;
  if (snapshot.dateTo) count += 1;
  if (snapshot.scope === "current" && snapshot.folderScope !== "current_subfolders") {
    count += 1;
  }
  return count;
}

export function buildAppliedFilterChips(
  snapshot: AppliedSearchSnapshot,
  context: AppliedFilterChipContext = {},
): AppliedFilterChip[] {
  const chips: AppliedFilterChip[] = [];
  const addTextChip = (id: AppliedFilterChipId, label: string, value: string) => {
    if (value) chips.push(filterChip(id, label, value));
  };

  addTextChip("query", "Search", snapshot.query);
  if (snapshot.scope === "all_open") {
    const workspaceCount = context.workspaceCount ?? snapshot.workspaceIds.length;
    const countSuffix = workspaceCount > 0 ? ` (${workspaceCount})` : "";
    chips.push(filterChip("scope", "PST scope", `All Open PSTs${countSuffix}`));
  }
  addTextChip("from", "From", snapshot.from);
  addTextChip("recipients", "To/Cc/Bcc", snapshot.recipients);
  addTextChip("subject", "Subject", snapshot.subject);
  addTextChip("body", "Body", snapshot.body);
  addTextChip("attachment", "Attachment", snapshot.attachment);

  if (snapshot.hasAttachments === "yes") {
    chips.push(filterChip("hasAttachments", "Attachments", "Has attachments"));
  } else if (snapshot.hasAttachments === "no") {
    chips.push(filterChip("hasAttachments", "Attachments", "No attachments"));
  }
  addTextChip("dateFrom", "Date from", snapshot.dateFrom);
  addTextChip("dateTo", "Date to", snapshot.dateTo);

  if (snapshot.scope === "current" && snapshot.folderScope !== "current_subfolders") {
    chips.push(
      filterChip(
        "folderScope",
        "Folder scope",
        snapshot.folderScope === "all" ? "All Mail" : "Current folder",
      ),
    );
  }

  const hasSelectedFolder =
    snapshot.folderId != null ||
    (snapshot.scope === "all_open" && snapshot.selectedWorkspaceId != null);
  if (hasSelectedFolder) {
    const folderLabel =
      context.folderLabel ?? (snapshot.folderId == null ? "All Mail" : "Selected folder");
    const workspacePrefix = context.workspaceLabel ? `${context.workspaceLabel} / ` : "";
    const subtreeSuffix =
      snapshot.folderId != null && snapshot.includeSubfolders ? " + subfolders" : "";
    chips.push(
      filterChip(
        "folderSelection",
        "Folder",
        `${workspacePrefix}${folderLabel}${subtreeSuffix}`,
      ),
    );
  }

  return chips;
}

export function removeAppliedFilter(
  snapshot: AppliedSearchSnapshot,
  id: AppliedFilterChipId,
): SearchFilterRemoval {
  const draft = normalizeDraft(snapshot);
  let scope = snapshot.scope;
  let clearFolderSelection = false;

  switch (id) {
    case "query":
    case "from":
    case "recipients":
    case "subject":
    case "body":
    case "attachment":
    case "dateFrom":
    case "dateTo":
      draft[id] = "";
      break;
    case "hasAttachments":
      draft.hasAttachments = "any";
      break;
    case "folderScope":
      draft.folderScope = "current_subfolders";
      break;
    case "scope":
      scope = "current";
      break;
    case "folderSelection":
      clearFolderSelection = true;
      break;
  }

  const changed =
    clearFolderSelection ||
    scope !== snapshot.scope ||
    JSON.stringify(draft) !== JSON.stringify(normalizeDraft(snapshot));
  return { changed, draft, scope, clearFolderSelection };
}

export function canClearAll(snapshot: AppliedSearchSnapshot): boolean {
  return Boolean(
    snapshot.query ||
      getActiveFilterCount(snapshot) > 0 ||
      snapshot.folderScope !== "current_subfolders" ||
      snapshot.messageSort !== "newest" ||
      snapshot.conversationSort !== "newest" ||
      (snapshot.scope === "all_open" && snapshot.selectedWorkspaceId != null),
  );
}

export function clearAllSearchDraft(snapshot: AppliedSearchSnapshot): ClearAllSearchResult {
  return {
    changed: canClearAll(snapshot),
    draft: emptySearchDraft(),
    scope: snapshot.scope,
    messageSort: "newest",
    conversationSort: "newest",
    clearAllOpenFolderSelection:
      snapshot.scope === "all_open" && snapshot.selectedWorkspaceId != null,
  };
}

export function classifySearchEmptyState(input: SearchEmptyStateInput): SearchEmptyState {
  const itemLabel = input.listMode === "conversations" ? "conversations" : "messages";
  if (!input.hasWorkspace) {
    return {
      kind: "no_workspace",
      title: "Open a PST to search messages.",
      detail: "Use Open PST or drop a PST file into the app.",
    };
  }
  if (input.resultCount > 0) {
    return { kind: "none", title: "", detail: null };
  }
  if (input.isLoading) {
    return {
      kind: "loading",
      title: input.isSearchActive ? "Searching..." : `Loading ${itemLabel}...`,
      detail: null,
    };
  }
  if (
    input.errorGeneration != null &&
    input.errorGeneration === input.currentGeneration
  ) {
    return {
      kind: "failed",
      title: "Search could not be completed.",
      detail: "Review the error details, then adjust the search and try again.",
    };
  }
  if (input.isSearchActive) {
    const scopeLabel = input.scope === "all_open" ? "All Open PSTs" : "Current PST";
    const filterText =
      input.activeFilterCount > 0
        ? ` ${input.activeFilterCount} advanced filter${input.activeFilterCount === 1 ? " is" : "s are"} active.`
        : "";
    return {
      kind: "no_matches",
      title: `No ${itemLabel} matched this search.`,
      detail: `Scope: ${scopeLabel}.${filterText} Remove a filter or use Clear All.`,
    };
  }
  return {
    kind: "inactive_empty",
    title: `No ${itemLabel} in this folder.`,
    detail: "Select another folder to continue browsing.",
  };
}

export function highlightTermsForSnapshot(snapshot: AppliedSearchSnapshot): string[] {
  const terms = new Set<string>();
  const pushTerms = (value: string) => {
    for (const match of value.match(/"([^"]+)"|[^\s:]+:[^\s]+|[^\s]+/g) ?? []) {
      const typedValue =
        match.includes(":") && !match.startsWith('"')
          ? match.split(":").slice(1).join(":")
          : match;
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

  pushTerms(snapshot.query);
  [snapshot.from, snapshot.recipients, snapshot.subject, snapshot.body, snapshot.attachment].forEach(
    pushTerms,
  );

  return Array.from(terms)
    .flatMap((term) => (term.length > 24 ? term.split(/[^\w@.-]+/) : [term]))
    .map((term) => term.trim())
    .filter((term) => term.length >= 2 && term.length <= 80)
    .slice(0, 12);
}

export function createSearchApplicationState(
  snapshot: AppliedSearchSnapshot,
): SearchApplicationState {
  return {
    applied: { snapshot, generation: 0 },
    pending: null,
    nextToken: 0,
  };
}

export function queueTextSearchSnapshot(
  state: SearchApplicationState,
  snapshot: AppliedSearchSnapshot,
): QueuedSearchSnapshot {
  const token = state.nextToken + 1;
  if (appliedSearchSnapshotKey(snapshot) === appliedSearchSnapshotKey(state.applied.snapshot)) {
    return {
      state: { ...state, pending: null, nextToken: token },
      token: null,
    };
  }
  return {
    state: {
      ...state,
      pending: { token, snapshot },
      nextToken: token,
    },
    token,
  };
}

export function applySearchSnapshotImmediately(
  state: SearchApplicationState,
  snapshot: AppliedSearchSnapshot,
): CommittedSearchSnapshot {
  const changed =
    appliedSearchSnapshotKey(snapshot) !== appliedSearchSnapshotKey(state.applied.snapshot);
  return {
    state: {
      applied: changed
        ? { snapshot, generation: state.applied.generation + 1 }
        : state.applied,
      pending: null,
      nextToken: state.nextToken + 1,
    },
    applied: changed,
  };
}

export function commitQueuedSearchSnapshot(
  state: SearchApplicationState,
  token: number,
): CommittedSearchSnapshot {
  if (!state.pending || state.pending.token !== token) {
    return { state, applied: false };
  }
  const changed =
    appliedSearchSnapshotKey(state.pending.snapshot) !==
    appliedSearchSnapshotKey(state.applied.snapshot);
  return {
    state: {
      applied: changed
        ? {
            snapshot: state.pending.snapshot,
            generation: state.applied.generation + 1,
          }
        : state.applied,
      pending: null,
      nextToken: state.nextToken,
    },
    applied: changed,
  };
}

export function invalidateSearchApplication(
  state: SearchApplicationState,
  snapshot: AppliedSearchSnapshot = state.applied.snapshot,
): SearchApplicationState {
  return {
    applied: {
      snapshot,
      generation: state.applied.generation + 1,
    },
    pending: null,
    nextToken: state.nextToken + 1,
  };
}

export function isSearchGenerationCurrent(
  requestGeneration: number,
  currentGeneration: number,
): boolean {
  return requestGeneration === currentGeneration;
}

export function createSearchOperationIdentity(
  generation: number,
  lane: SearchOperationLane,
  sequence: number,
): SearchOperationIdentity {
  const safeGeneration = Number.isSafeInteger(generation) && generation >= 0 ? generation : 0;
  const safeSequence = Number.isSafeInteger(sequence) && sequence > 0 ? sequence : 1;
  return {
    generation: safeGeneration,
    operationId: `${lane}-${safeSequence}`,
    lane,
  };
}

export function cancellationForGeneration(generation: number): SearchCancellationRequest {
  return {
    generation: Number.isSafeInteger(generation) && generation >= 0 ? generation : 0,
    operationId: null,
  };
}

export function cancellationForOperation(
  operation: SearchOperationIdentity,
  currentGeneration: number,
): SearchCancellationRequest | null {
  if (!isSearchGenerationCurrent(operation.generation, currentGeneration)) return null;
  return { generation: operation.generation, operationId: operation.operationId };
}

export function isSearchCancellationError(error: unknown): boolean {
  return Boolean(
    error &&
      typeof error === "object" &&
      "code" in error &&
      (error as { code?: unknown }).code === searchCancelledErrorCode,
  );
}

export function isExpandedConversationResponseCurrent(
  requestGeneration: number,
  currentGeneration: number,
  requestId: number,
  activeRequestId: number | undefined,
  isExpanded: boolean,
): boolean {
  return (
    isExpanded &&
    requestId === activeRequestId &&
    isSearchGenerationCurrent(requestGeneration, currentGeneration)
  );
}

export function appendUniqueByKey<T>(
  current: T[],
  incoming: T[],
  keyFor: (item: T) => string,
): T[] {
  const seen = new Set(current.map(keyFor));
  const next = [...current];
  for (const item of incoming) {
    const key = keyFor(item);
    if (seen.has(key)) continue;
    seen.add(key);
    next.push(item);
  }
  return next;
}
