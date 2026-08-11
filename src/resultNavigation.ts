import type {
  AppliedSearchSnapshot,
  SearchListMode,
} from "./searchRequest";

export type ResultNavigationDirection = -1 | 1;

export type ResultNavigationEntry = {
  key: string;
  logicalKey: string;
  logicalIndex: number;
  focusable?: boolean;
  parentKey?: string | null;
  kind?: string;
};

export type PendingResultFocus = {
  key: string;
  resetIdentity: number;
};

export type PendingResultFocusResolution = "wait" | "focus" | "cancel";

export type ResultNavigationState = Record<SearchListMode, string | null>;

function lowerBound(values: readonly number[], target: number): number {
  let low = 0;
  let high = values.length;
  while (low < high) {
    const middle = low + Math.floor((high - low) / 2);
    if (values[middle] < target) low = middle + 1;
    else high = middle;
  }
  return low;
}

/**
 * Immutable lookup index for result-list focus navigation. Building the model
 * is O(n); ordinary adjacent navigation and key lookup are O(1), while page
 * destinations are O(log n).
 */
export class ResultNavigationModel {
  private readonly entriesByKey = new Map<string, ResultNavigationEntry>();
  private readonly focusableEntries: ResultNavigationEntry[] = [];
  private readonly focusablePositionByKey = new Map<string, number>();
  private readonly logicalIndices: number[] = [];
  private readonly firstChildByParent = new Map<string, string>();

  constructor(entries: readonly ResultNavigationEntry[] = []) {
    for (const entry of entries) {
      if (
        !entry.key ||
        !entry.logicalKey ||
        !Number.isInteger(entry.logicalIndex) ||
        entry.logicalIndex < 0 ||
        this.entriesByKey.has(entry.key)
      ) {
        continue;
      }
      this.entriesByKey.set(entry.key, entry);
      if (entry.focusable === false) continue;
      const position = this.focusableEntries.length;
      this.focusableEntries.push(entry);
      this.focusablePositionByKey.set(entry.key, position);
      this.logicalIndices.push(entry.logicalIndex);
      if (entry.parentKey && !this.firstChildByParent.has(entry.parentKey)) {
        this.firstChildByParent.set(entry.parentKey, entry.key);
      }
    }
  }

  get size(): number {
    return this.focusableEntries.length;
  }

  has(key: string | null | undefined): boolean {
    return Boolean(key && this.focusablePositionByKey.has(key));
  }

  entry(key: string | null | undefined): ResultNavigationEntry | null {
    if (!key || !this.focusablePositionByKey.has(key)) return null;
    return this.entriesByKey.get(key) ?? null;
  }

  firstKey(): string | null {
    return this.focusableEntries[0]?.key ?? null;
  }

  lastKey(): string | null {
    return this.focusableEntries[this.focusableEntries.length - 1]?.key ?? null;
  }

  nextKey(key: string | null | undefined): string | null {
    if (!key) return this.firstKey();
    const position = this.focusablePositionByKey.get(key);
    if (position == null) return this.firstKey();
    return this.focusableEntries[position + 1]?.key ?? null;
  }

  previousKey(key: string | null | undefined): string | null {
    if (!key) return this.lastKey();
    const position = this.focusablePositionByKey.get(key);
    if (position == null) return this.lastKey();
    return this.focusableEntries[position - 1]?.key ?? null;
  }

  resolveActiveKey(
    activeKey: string | null | undefined,
    preferredKey: string | null | undefined = null,
    previousLogicalIndex?: number,
  ): string | null {
    if (this.has(activeKey)) return activeKey!;
    if (this.has(preferredKey)) return preferredKey!;
    if (Number.isFinite(previousLogicalIndex) && this.focusableEntries.length > 0) {
      const position = Math.min(
        lowerBound(this.logicalIndices, Math.max(0, Math.floor(previousLogicalIndex!))),
        this.focusableEntries.length - 1,
      );
      return this.focusableEntries[position].key;
    }
    return this.firstKey();
  }

  pageKey(
    activeKey: string | null | undefined,
    targetLogicalIndex: number,
    direction: ResultNavigationDirection,
  ): string | null {
    if (this.focusableEntries.length === 0) return null;
    if (!this.has(activeKey)) return direction > 0 ? this.firstKey() : this.lastKey();

    const safeTarget = Number.isFinite(targetLogicalIndex)
      ? Math.max(0, Math.floor(targetLogicalIndex))
      : this.entry(activeKey)?.logicalIndex ?? 0;
    let position = lowerBound(this.logicalIndices, safeTarget);
    if (direction < 0 && (position >= this.logicalIndices.length || this.logicalIndices[position] > safeTarget)) {
      position -= 1;
    }
    position = Math.min(Math.max(position, 0), this.focusableEntries.length - 1);

    const currentPosition = this.focusablePositionByKey.get(activeKey!)!;
    if (position === currentPosition) {
      position = Math.min(
        Math.max(currentPosition + direction, 0),
        this.focusableEntries.length - 1,
      );
    }
    return this.focusableEntries[position].key;
  }

  firstChildKey(parentKey: string | null | undefined): string | null {
    if (!parentKey) return null;
    return this.firstChildByParent.get(parentKey) ?? null;
  }
}

export function createPendingResultFocus(
  model: ResultNavigationModel,
  key: string | null | undefined,
  resetIdentity: number,
): PendingResultFocus | null {
  if (!model.has(key) || !Number.isSafeInteger(resetIdentity)) return null;
  return { key: key!, resetIdentity };
}

export function resolvePendingResultFocus(
  pending: PendingResultFocus | null,
  model: ResultNavigationModel,
  mountedKeys: ReadonlySet<string>,
  resetIdentity: number,
): PendingResultFocusResolution {
  if (!pending || pending.resetIdentity !== resetIdentity || !model.has(pending.key)) {
    return "cancel";
  }
  return mountedKeys.has(pending.key) ? "focus" : "wait";
}

export function shouldPublishResolvedNavigationKey(
  activeKey: string | null,
  resolvedKey: string | null,
  focusableEntryCount: number,
): boolean {
  if (activeKey === resolvedKey) return false;
  // Mode switches briefly clear rows while refetching; retain that mode's key
  // until replacement rows establish whether it is still valid.
  return !(focusableEntryCount === 0 && activeKey != null);
}

export function createResultNavigationState(): ResultNavigationState {
  return { messages: null, conversations: null };
}

export function setResultNavigationActiveKey(
  state: ResultNavigationState,
  mode: SearchListMode,
  key: string | null,
): ResultNavigationState {
  if (state[mode] === key) return state;
  return { ...state, [mode]: key };
}

export function resetResultNavigationMode(
  state: ResultNavigationState,
  mode: SearchListMode,
): ResultNavigationState {
  return setResultNavigationActiveKey(state, mode, null);
}

function sameStrings(left: readonly string[], right: readonly string[]): boolean {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

function sameConversationScopes(
  left: AppliedSearchSnapshot["conversationScopes"],
  right: AppliedSearchSnapshot["conversationScopes"],
): boolean {
  return (
    left.length === right.length &&
    left.every((scope, index) => {
      const other = right[index];
      return (
        scope.workspaceId === other.workspaceId &&
        scope.folderId === other.folderId &&
        scope.includeSubfolders === other.includeSubfolders
      );
    })
  );
}

/** List-mode switches retain each mode's last active key; data-affecting changes do not. */
export function didResultNavigationContextChange(
  previous: AppliedSearchSnapshot,
  next: AppliedSearchSnapshot,
  mode: SearchListMode,
): boolean {
  return (
    previous.query !== next.query ||
    previous.from !== next.from ||
    previous.recipients !== next.recipients ||
    previous.subject !== next.subject ||
    previous.body !== next.body ||
    previous.attachment !== next.attachment ||
    previous.hasAttachments !== next.hasAttachments ||
    previous.dateFrom !== next.dateFrom ||
    previous.dateTo !== next.dateTo ||
    previous.folderScope !== next.folderScope ||
    previous.scope !== next.scope ||
    previous.activeWorkspaceId !== next.activeWorkspaceId ||
    !sameStrings(previous.workspaceIds, next.workspaceIds) ||
    previous.selectedWorkspaceId !== next.selectedWorkspaceId ||
    previous.useMultiWorkspace !== next.useMultiWorkspace ||
    previous.singleWorkspaceId !== next.singleWorkspaceId ||
    previous.folderId !== next.folderId ||
    previous.includeSubfolders !== next.includeSubfolders ||
    !sameConversationScopes(previous.conversationScopes, next.conversationScopes) ||
    previous.sessionGeneration !== next.sessionGeneration ||
    (mode === "messages"
      ? previous.messageSort !== next.messageSort
      : previous.conversationSort !== next.conversationSort)
  );
}
