import type { SearchListMode } from "./searchRequest";
import type {
  LocalTrashMutationResult,
  LocalTrashResetResult,
  LocalTrashSummary,
  LocalTrashUndoState,
  MessageVisibility,
} from "./types";

export const localTrashActionDisclosure =
  "This affects only PST QuickView's local workspace. The source PST will not be changed.";

export const localTrashStorageDisclosure =
  "Local Trash hides messages from normal PST QuickView views. It does not change the source PST or reduce workspace storage.";

export const localTrashResetDisclosure =
  "Reset Local Trash State clears PST QuickView's app-only organization. It does not change the source PST, canonical index, extracted messages, or attachments.";

export type LocalTrashAggregateSummary = {
  workspaceCount: number;
  availableWorkspaceCount: number;
  unavailableWorkspaceCount: number;
  sidecarPresentWorkspaceCount: number;
  trashedMessageCount: number;
  orphanedStateCount: number;
  available: boolean;
};

function isNonNegativeSafeInteger(value: number): boolean {
  return Number.isSafeInteger(value) && value >= 0;
}

function addBoundedCount(left: number, right: number): number {
  return Math.min(Number.MAX_SAFE_INTEGER, left + right);
}

function isUsableSummary(summary: LocalTrashSummary): boolean {
  return (
    summary.available &&
    summary.fingerprintMatch &&
    isNonNegativeSafeInteger(summary.trashedMessageCount) &&
    isNonNegativeSafeInteger(summary.orphanedStateCount) &&
    isNonNegativeSafeInteger(summary.stateRevision)
  );
}

/** Aggregate only the effective open workspace IDs; duplicate or malformed summaries stay unavailable. */
export function aggregateLocalTrashSummaries(
  effectiveWorkspaceIds: readonly string[],
  summaries: readonly LocalTrashSummary[],
): LocalTrashAggregateSummary {
  const workspaceIds = Array.from(
    new Set(effectiveWorkspaceIds.filter((workspaceId) => workspaceId.trim().length > 0)),
  );
  const summariesByWorkspace = new Map<string, LocalTrashSummary | null>();
  for (const summary of summaries) {
    if (!workspaceIds.includes(summary.workspaceId)) continue;
    summariesByWorkspace.set(
      summary.workspaceId,
      summariesByWorkspace.has(summary.workspaceId) ? null : summary,
    );
  }

  let availableWorkspaceCount = 0;
  let unavailableWorkspaceCount = 0;
  let sidecarPresentWorkspaceCount = 0;
  let trashedMessageCount = 0;
  let orphanedStateCount = 0;

  for (const workspaceId of workspaceIds) {
    const summary = summariesByWorkspace.get(workspaceId);
    if (summary?.sidecarPresent) sidecarPresentWorkspaceCount += 1;
    if (!summary || !isUsableSummary(summary)) {
      unavailableWorkspaceCount += 1;
      continue;
    }
    availableWorkspaceCount += 1;
    trashedMessageCount = addBoundedCount(
      trashedMessageCount,
      summary.trashedMessageCount,
    );
    orphanedStateCount = addBoundedCount(orphanedStateCount, summary.orphanedStateCount);
  }

  return {
    workspaceCount: workspaceIds.length,
    availableWorkspaceCount,
    unavailableWorkspaceCount,
    sidecarPresentWorkspaceCount,
    trashedMessageCount,
    orphanedStateCount,
    available: workspaceIds.length > 0 && unavailableWorkspaceCount === 0,
  };
}

export type LocalTrashOperationToken = {
  workspaceId: string;
  expectedRevision: number;
  sequence: number;
};

export type LocalTrashResetToken = {
  workspaceId: string;
  sequence: number;
};

export function createLocalTrashResetToken(
  workspaceId: string,
  sequence: number,
): LocalTrashResetToken | null {
  if (
    workspaceId.trim().length === 0 ||
    !Number.isSafeInteger(sequence) ||
    sequence <= 0
  ) {
    return null;
  }
  return { workspaceId, sequence };
}

export function isLocalTrashResetResponseCurrent(
  requestToken: LocalTrashResetToken,
  activeToken: LocalTrashResetToken | null,
  activeWorkspaceId: string | null,
  response: LocalTrashResetResult,
): boolean {
  return (
    activeToken === requestToken &&
    activeWorkspaceId === requestToken.workspaceId &&
    response.workspaceId === requestToken.workspaceId
  );
}

export function createLocalTrashOperationToken(
  workspaceId: string,
  expectedRevision: number,
  sequence: number,
): LocalTrashOperationToken | null {
  if (
    workspaceId.trim().length === 0 ||
    !isNonNegativeSafeInteger(expectedRevision) ||
    !Number.isSafeInteger(sequence) ||
    sequence <= 0
  ) {
    return null;
  }
  return { workspaceId, expectedRevision, sequence };
}

export function isSameLocalTrashOperationToken(
  left: LocalTrashOperationToken | null,
  right: LocalTrashOperationToken | null,
): boolean {
  return Boolean(
    left &&
      right &&
      left.workspaceId === right.workspaceId &&
      left.expectedRevision === right.expectedRevision &&
      left.sequence === right.sequence,
  );
}

export type LocalTrashMutationCurrentness = {
  requestToken: LocalTrashOperationToken;
  activeToken: LocalTrashOperationToken | null;
  currentWorkspaceRevision: number;
  response: LocalTrashMutationResult;
};

export function isLocalTrashMutationResponseCurrent({
  requestToken,
  activeToken,
  currentWorkspaceRevision,
  response,
}: LocalTrashMutationCurrentness): boolean {
  return (
    isSameLocalTrashOperationToken(requestToken, activeToken) &&
    response.workspaceId === requestToken.workspaceId &&
    currentWorkspaceRevision === requestToken.expectedRevision &&
    isNonNegativeSafeInteger(response.changedCount) &&
    response.changedCount <= 1 &&
    isNonNegativeSafeInteger(response.stateRevision) &&
    response.stateRevision === requestToken.expectedRevision + response.changedCount
  );
}

function isOpaqueOperationId(value: string | null): value is string {
  return Boolean(
    value &&
      value.length <= 256 &&
      value.trim() === value &&
      !/[\u0000-\u001f\u007f]/.test(value),
  );
}

export type LocalTrashUndoAction =
  | { type: "mutation_succeeded"; response: LocalTrashMutationResult }
  | { type: "undo_succeeded"; response: LocalTrashMutationResult }
  | { type: "dismissed" }
  | { type: "restart" };

export function initialLocalTrashUndoState(): LocalTrashUndoState | null {
  return null;
}

export function isLocalTrashUndoCurrent(
  state: LocalTrashUndoState | null,
  workspaceId: string,
  stateRevision: number,
): boolean {
  return Boolean(
    state &&
      state.workspaceId === workspaceId &&
      state.expectedRevision === stateRevision,
  );
}

/** Keep exactly one transient Undo target; failed operations should not dispatch an action. */
export function reduceLocalTrashUndoState(
  state: LocalTrashUndoState | null,
  action: LocalTrashUndoAction,
): LocalTrashUndoState | null {
  if (action.type === "dismissed" || action.type === "restart") return null;

  const { response } = action;
  if (action.type === "undo_succeeded") {
    if (
      state &&
      response.result === "undone" &&
      response.changedCount > 0 &&
      response.workspaceId === state.workspaceId &&
      response.operationId === state.operationId &&
      response.stateRevision >= state.expectedRevision
    ) {
      return null;
    }
    return state;
  }

  if (
    response.result === "moved" &&
    response.changedCount > 0 &&
    isOpaqueOperationId(response.operationId)
  ) {
    return {
      workspaceId: response.workspaceId,
      operationId: response.operationId,
      expectedRevision: response.stateRevision,
    };
  }

  if (
    state &&
    response.workspaceId === state.workspaceId &&
    response.stateRevision !== state.expectedRevision
  ) {
    return null;
  }
  return state;
}

export type CanonicalMessageViewScope = {
  selectedWorkspaceId: string | null;
  folderId: number | null;
  includeSubfolders: boolean;
};

export type LocalTrashViewState = {
  visibility: MessageVisibility;
  listMode: SearchListMode;
  previousActiveListMode: SearchListMode | null;
  canonicalScope: CanonicalMessageViewScope;
};

export function createLocalTrashViewState(
  canonicalScope: CanonicalMessageViewScope,
  listMode: SearchListMode,
  visibility: MessageVisibility = "active",
): LocalTrashViewState {
  return {
    visibility,
    listMode: visibility === "trashed" ? "messages" : listMode,
    previousActiveListMode: visibility === "trashed" ? listMode : null,
    canonicalScope: { ...canonicalScope },
  };
}

export function enterLocalTrashView(state: LocalTrashViewState): LocalTrashViewState {
  if (state.visibility === "trashed") return state;
  return {
    ...state,
    visibility: "trashed",
    listMode: "messages",
    previousActiveListMode: state.listMode,
  };
}

export function leaveLocalTrashView(state: LocalTrashViewState): LocalTrashViewState {
  if (state.visibility === "active") return state;
  return {
    ...state,
    visibility: "active",
    listMode: state.previousActiveListMode ?? "messages",
    previousActiveListMode: null,
  };
}

export function withCanonicalMessageViewScope(
  state: LocalTrashViewState,
  canonicalScope: CanonicalMessageViewScope,
): LocalTrashViewState {
  return { ...state, canonicalScope: { ...canonicalScope } };
}

export function effectiveLocalTrashQueryScope(
  state: LocalTrashViewState,
): CanonicalMessageViewScope {
  if (state.visibility === "trashed") {
    return {
      selectedWorkspaceId: state.canonicalScope.selectedWorkspaceId,
      folderId: null,
      includeSubfolders: false,
    };
  }
  return { ...state.canonicalScope };
}

/** Keep the canonical All Open selection intact while Trash targets a separate workspace. */
export function effectiveAllOpenLocalTrashQueryScope(
  canonicalScope: CanonicalMessageViewScope,
  visibility: MessageVisibility,
  localTrashWorkspaceId: string | null,
): CanonicalMessageViewScope {
  if (visibility === "trashed") {
    return {
      selectedWorkspaceId: localTrashWorkspaceId,
      folderId: null,
      includeSubfolders: false,
    };
  }
  return { ...canonicalScope };
}

export function clearLocalTrashWorkspaceView(
  state: LocalTrashViewState,
): LocalTrashViewState {
  return {
    visibility: "active",
    listMode: state.previousActiveListMode ?? state.listMode,
    previousActiveListMode: null,
    canonicalScope: {
      selectedWorkspaceId: null,
      folderId: null,
      includeSubfolders: false,
    },
  };
}
