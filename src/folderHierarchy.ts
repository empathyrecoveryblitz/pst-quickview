export type FolderHierarchyMode = "simplified" | "original";

export const defaultFolderHierarchyMode: FolderHierarchyMode = "simplified";
export const folderHierarchyStorageKey = "pstQuickView.folderHierarchy";

export type FolderStructuralRole =
  | "application_root"
  | "pst_store_wrapper"
  | "root_container"
  | "technical_wrapper"
  | "user_folder"
  | "unknown";

export type FolderProvenance =
  | "application"
  | "readpst"
  | "canonical_importer"
  | "user"
  | "unknown";

export type CanonicalFolderRecord = {
  id: number;
  parentId: number | null;
  path: string;
  name: string;
  messageCount: number;
  directMessageCount: number;
  activeMessageCount?: number;
  activeDirectMessageCount?: number;
  localTrashMessageCount?: number;
  localTrashDirectMessageCount?: number;
  structuralRole?: FolderStructuralRole;
  provenance?: FolderProvenance;
};

export type CompactCanonicalFolderScope = {
  workspaceId: string;
  folderId: number;
  includeSubfolders: true;
  pathIdentity: string;
};

export type PromotedAncestorEvidence = {
  count: number;
  nearestCanonicalIdentity: string | null;
  outermostCanonicalIdentity: string | null;
};

export type PresentationFolderNode = {
  presentationId: string;
  canonicalIdentity: string;
  canonicalFolderId: number;
  canonicalParentId: number | null;
  canonicalParentIdentity: string | null;
  presentationParentIdentity: string | null;
  displayName: string;
  originalPath: string;
  directMessageCount: number;
  descendantMessageCount: number;
  canonicalDirectMessageCount: number;
  canonicalDescendantMessageCount: number;
  localTrashDirectMessageCount: number;
  localTrashDescendantMessageCount: number;
  workspaceId: string;
  structuralRole: FolderStructuralRole;
  provenance: FolderProvenance;
  ancestorWasPromoted: boolean;
  promotedAncestorEvidence: PromotedAncestorEvidence;
  canonicalDescendantScope: CompactCanonicalFolderScope;
  children: readonly PresentationFolderNode[];
};

export type FolderHierarchyDiagnosticCode =
  | "invalid_workspace_identity"
  | "invalid_folder_id"
  | "duplicate_folder_id"
  | "invalid_parent_id"
  | "missing_parent"
  | "self_parent"
  | "cycle"
  | "invalid_count"
  | "count_mismatch"
  | "technical_wrapper_has_direct_messages"
  | "technical_wrapper_provenance_unavailable";

export type FolderHierarchyDiagnostic = {
  code: FolderHierarchyDiagnosticCode;
  severity: "warning" | "fatal";
  canonicalIdentity: string | null;
  observedCount?: number;
  expectedCount?: number;
};

export type CanonicalFolderDisposition = {
  canonicalFolderId: number;
  canonicalIdentity: string;
  visible: boolean;
  presentationId: string | null;
  structuralRole: FolderStructuralRole;
  provenance: FolderProvenance;
  canonicalDescendantScope: CompactCanonicalFolderScope;
};

export type FolderHierarchyResult = {
  status: "ok" | "recovered" | "invalid";
  mode: FolderHierarchyMode;
  workspaceId: string;
  roots: readonly PresentationFolderNode[];
  flatNodes: readonly PresentationFolderNode[];
  dispositions: readonly CanonicalFolderDisposition[];
  diagnostics: readonly FolderHierarchyDiagnostic[];
};

export type FolderPresentationSelection =
  | {
      kind: "all_mail";
      canonicalScope: null;
      presentationId: null;
    }
  | {
      kind: "visible";
      canonicalScope: CompactCanonicalFolderScope;
      presentationId: string;
    }
  | {
      kind: "promoted_scope";
      canonicalScope: CompactCanonicalFolderScope;
      presentationId: null;
    }
  | {
      kind: "missing";
      canonicalScope: null;
      presentationId: null;
    };

export type CanonicalFolderSearchScope = {
  workspaceId: string;
  folderId: number | null;
  includeSubfolders: boolean;
  pathIdentity: string | null;
};

export type FolderHierarchyExpansionState = {
  simplified: readonly string[];
  original: readonly string[];
};

type WorkingFolder = {
  input: CanonicalFolderRecord;
  id: number;
  rawParentId: number | null;
  safeParentId: number | null;
  canonicalIdentity: string;
  presentationId: string;
  role: FolderStructuralRole;
  provenance: FolderProvenance;
  directCount: number;
  rollupCount: number;
  activeDirectCount: number;
  activeRollupCount: number;
  localTrashDirectCount: number;
  localTrashRollupCount: number;
  children: WorkingFolder[];
  selfUnsafe: boolean;
  branchUnsafe: boolean;
  subtreeUnsafe: boolean;
  hidden: boolean;
};

type MutablePresentationNode = Omit<PresentationFolderNode, "children"> & {
  children: MutablePresentationNode[] | readonly PresentationFolderNode[];
};

const technicalRoles = new Set<FolderStructuralRole>([
  "application_root",
  "pst_store_wrapper",
  "root_container",
  "technical_wrapper",
]);

function validCount(value: unknown): value is number {
  return Number.isSafeInteger(value) && Number(value) >= 0;
}

function localCountOverlay(input: CanonicalFolderRecord): {
  activeDirect: number;
  activeRollup: number;
  trashDirect: number;
  trashRollup: number;
} {
  const activeDirect = input.activeDirectMessageCount;
  const activeRollup = input.activeMessageCount;
  const trashDirect = input.localTrashDirectMessageCount;
  const trashRollup = input.localTrashMessageCount;
  if (
    validCount(input.directMessageCount) &&
    validCount(input.messageCount) &&
    validCount(activeDirect) &&
    validCount(activeRollup) &&
    validCount(trashDirect) &&
    validCount(trashRollup) &&
    activeDirect + trashDirect === input.directMessageCount &&
    activeRollup + trashRollup === input.messageCount
  ) {
    return { activeDirect, activeRollup, trashDirect, trashRollup };
  }
  return {
    activeDirect: input.directMessageCount,
    activeRollup: input.messageCount,
    trashDirect: 0,
    trashRollup: 0,
  };
}

export function normalizeFolderHierarchyMode(value: unknown): FolderHierarchyMode {
  return value === "simplified" || value === "original"
    ? value
    : defaultFolderHierarchyMode;
}

export function storedFolderHierarchyMode(
  storage: Pick<Storage, "getItem"> | null,
): FolderHierarchyMode {
  if (!storage) return defaultFolderHierarchyMode;
  try {
    return normalizeFolderHierarchyMode(storage.getItem(folderHierarchyStorageKey));
  } catch {
    return defaultFolderHierarchyMode;
  }
}

export function saveFolderHierarchyMode(
  storage: Pick<Storage, "setItem"> | null,
  mode: unknown,
): void {
  if (!storage) return;
  try {
    storage.setItem(folderHierarchyStorageKey, normalizeFolderHierarchyMode(mode));
  } catch {
    // Keep the in-memory preference when local storage is unavailable.
  }
}

export function canonicalFolderIdentity(workspaceId: string, folderId: number): string {
  return `folder:${workspaceId.length}:${workspaceId}:${folderId}`;
}

export function presentationFolderIdentity(workspaceId: string, folderId: number): string {
  return `presented:${canonicalFolderIdentity(workspaceId, folderId)}`;
}

function compareText(left: string, right: string): number {
  if (left === right) return 0;
  return left < right ? -1 : 1;
}

function compareFolders(left: WorkingFolder, right: WorkingFolder): number {
  return (
    left.input.name.localeCompare(right.input.name) ||
    left.input.path.localeCompare(right.input.path) ||
    left.id - right.id
  );
}

function comparePresentationFolders(
  left: PresentationFolderNode,
  right: PresentationFolderNode,
): number {
  return (
    left.displayName.localeCompare(right.displayName) ||
    left.originalPath.localeCompare(right.originalPath) ||
    left.canonicalFolderId - right.canonicalFolderId
  );
}

function deterministicRole(folder: CanonicalFolderRecord): {
  role: FolderStructuralRole;
  provenance: FolderProvenance;
} {
  if (folder.parentId === null && folder.path === "") {
    return { role: "application_root", provenance: "application" };
  }
  return {
    role: folder.structuralRole ?? "unknown",
    provenance: folder.provenance ?? "unknown",
  };
}

function hasDeterministicTechnicalProvenance(folder: WorkingFolder): boolean {
  if (folder.role === "application_root" && folder.provenance === "application") return true;
  if (!technicalRoles.has(folder.role)) return false;
  return folder.provenance === "readpst" || folder.provenance === "canonical_importer";
}

function compactScope(folder: WorkingFolder, workspaceId: string): CompactCanonicalFolderScope {
  return Object.freeze({
    workspaceId,
    folderId: folder.id,
    includeSubfolders: true as const,
    pathIdentity: folder.input.path,
  });
}

function fatalResult(
  mode: FolderHierarchyMode,
  workspaceId: string,
  diagnostics: FolderHierarchyDiagnostic[],
): FolderHierarchyResult {
  return Object.freeze({
    status: "invalid" as const,
    mode,
    workspaceId,
    roots: Object.freeze([]),
    flatNodes: Object.freeze([]),
    dispositions: Object.freeze([]),
    diagnostics: Object.freeze(diagnostics),
  });
}

export function buildFolderHierarchy(
  canonicalFolders: readonly CanonicalFolderRecord[],
  workspaceId: string,
  requestedMode: FolderHierarchyMode,
): FolderHierarchyResult {
  const mode = normalizeFolderHierarchyMode(requestedMode);
  const diagnostics: FolderHierarchyDiagnostic[] = [];
  if (!workspaceId) {
    diagnostics.push({
      code: "invalid_workspace_identity",
      severity: "fatal",
      canonicalIdentity: null,
    });
    return fatalResult(mode, workspaceId, diagnostics);
  }

  const byId = new Map<number, WorkingFolder>();
  const ordered: WorkingFolder[] = [];
  for (const input of canonicalFolders) {
    if (!Number.isSafeInteger(input.id) || input.id <= 0) {
      diagnostics.push({
        code: "invalid_folder_id",
        severity: "fatal",
        canonicalIdentity: null,
      });
      continue;
    }
    const identity = canonicalFolderIdentity(workspaceId, input.id);
    if (byId.has(input.id)) {
      diagnostics.push({
        code: "duplicate_folder_id",
        severity: "fatal",
        canonicalIdentity: identity,
      });
      continue;
    }
    const { role, provenance } = deterministicRole(input);
    const overlay = localCountOverlay(input);
    const working: WorkingFolder = {
      input,
      id: input.id,
      rawParentId: input.parentId,
      safeParentId: input.parentId,
      canonicalIdentity: identity,
      presentationId: presentationFolderIdentity(workspaceId, input.id),
      role,
      provenance,
      directCount: input.directMessageCount,
      rollupCount: input.messageCount,
      activeDirectCount: overlay.activeDirect,
      activeRollupCount: overlay.activeRollup,
      localTrashDirectCount: overlay.trashDirect,
      localTrashRollupCount: overlay.trashRollup,
      children: [],
      selfUnsafe: false,
      branchUnsafe: false,
      subtreeUnsafe: false,
      hidden: false,
    };
    byId.set(input.id, working);
    ordered.push(working);
  }
  if (diagnostics.some((diagnostic) => diagnostic.severity === "fatal")) {
    return fatalResult(mode, workspaceId, diagnostics);
  }

  for (const folder of ordered) {
    const parentId = folder.rawParentId;
    if (parentId === null) continue;
    if (!Number.isSafeInteger(parentId) || parentId <= 0) {
      folder.safeParentId = null;
      folder.selfUnsafe = true;
      diagnostics.push({
        code: "invalid_parent_id",
        severity: "warning",
        canonicalIdentity: folder.canonicalIdentity,
      });
    } else if (!byId.has(parentId)) {
      folder.safeParentId = null;
      folder.selfUnsafe = true;
      diagnostics.push({
        code: "missing_parent",
        severity: "warning",
        canonicalIdentity: folder.canonicalIdentity,
      });
    } else if (parentId === folder.id) {
      folder.safeParentId = null;
      folder.selfUnsafe = true;
      diagnostics.push({
        code: "self_parent",
        severity: "warning",
        canonicalIdentity: folder.canonicalIdentity,
      });
    }
  }

  const completed = new Set<number>();
  for (const start of ordered) {
    if (completed.has(start.id)) continue;
    const chain: number[] = [];
    const positions = new Map<number, number>();
    let cursor: number | null = start.id;
    while (cursor !== null && !completed.has(cursor)) {
      const cycleStart = positions.get(cursor);
      if (cycleStart !== undefined) {
        for (const cycleId of chain.slice(cycleStart)) {
          const cycleFolder = byId.get(cycleId)!;
          cycleFolder.safeParentId = null;
          cycleFolder.selfUnsafe = true;
          diagnostics.push({
            code: "cycle",
            severity: "warning",
            canonicalIdentity: cycleFolder.canonicalIdentity,
          });
        }
        break;
      }
      positions.set(cursor, chain.length);
      chain.push(cursor);
      cursor = byId.get(cursor)!.safeParentId;
    }
    for (const id of chain) completed.add(id);
  }

  const canonicalRoots: WorkingFolder[] = [];
  for (const folder of ordered) folder.children = [];
  for (const folder of ordered) {
    const parent = folder.safeParentId === null ? null : byId.get(folder.safeParentId) ?? null;
    if (parent) parent.children.push(folder);
    else canonicalRoots.push(folder);
  }
  for (const folder of ordered) folder.children.sort(compareFolders);
  canonicalRoots.sort(compareFolders);

  const canonicalOrder: WorkingFolder[] = [];
  const orderStack = [...canonicalRoots].reverse();
  while (orderStack.length) {
    const folder = orderStack.pop()!;
    canonicalOrder.push(folder);
    for (let index = folder.children.length - 1; index >= 0; index -= 1) {
      orderStack.push(folder.children[index]);
    }
  }

  for (const folder of canonicalOrder) {
    if (folder.safeParentId !== null && byId.get(folder.safeParentId)?.branchUnsafe) {
      folder.branchUnsafe = true;
    }
    folder.branchUnsafe ||= folder.selfUnsafe;
    for (const [value, code] of [
      [folder.directCount, "direct"],
      [folder.rollupCount, "rollup"],
    ] as const) {
      if (!Number.isSafeInteger(value) || value < 0) {
        folder.selfUnsafe = true;
        folder.branchUnsafe = true;
        diagnostics.push({
          code: "invalid_count",
          severity: "warning",
          canonicalIdentity: folder.canonicalIdentity,
          observedCount: value,
          expectedCount: code === "direct" ? 0 : folder.directCount,
        });
      }
    }
  }

  for (let index = canonicalOrder.length - 1; index >= 0; index -= 1) {
    const folder = canonicalOrder[index];
    const expected = folder.children.reduce(
      (total, child) => total + child.rollupCount,
      folder.directCount,
    );
    if (
      Number.isSafeInteger(folder.directCount) &&
      folder.directCount >= 0 &&
      Number.isSafeInteger(folder.rollupCount) &&
      folder.rollupCount >= 0 &&
      expected !== folder.rollupCount
    ) {
      folder.selfUnsafe = true;
      diagnostics.push({
        code: "count_mismatch",
        severity: "warning",
        canonicalIdentity: folder.canonicalIdentity,
        observedCount: folder.rollupCount,
        expectedCount: expected,
      });
    }
    folder.subtreeUnsafe =
      folder.selfUnsafe ||
      folder.branchUnsafe ||
      folder.children.some((child) => child.subtreeUnsafe);
  }

  for (const folder of canonicalOrder) {
    const isTechnical = technicalRoles.has(folder.role);
    const deterministicProvenance = hasDeterministicTechnicalProvenance(folder);
    if (isTechnical && folder.directCount > 0) {
      diagnostics.push({
        code: "technical_wrapper_has_direct_messages",
        severity: "warning",
        canonicalIdentity: folder.canonicalIdentity,
        observedCount: folder.directCount,
      });
    }
    if (isTechnical && !deterministicProvenance) {
      diagnostics.push({
        code: "technical_wrapper_provenance_unavailable",
        severity: "warning",
        canonicalIdentity: folder.canonicalIdentity,
      });
    }
    folder.hidden =
      mode === "simplified" &&
      isTechnical &&
      deterministicProvenance &&
      folder.directCount === 0 &&
      !folder.subtreeUnsafe;
  }

  const presentationRoots: MutablePresentationNode[] = [];
  const flatNodes: MutablePresentationNode[] = [];
  const dispositions: CanonicalFolderDisposition[] = [];
  const traversalStack: Array<{
    folder: WorkingFolder;
    presentationParent: MutablePresentationNode | null;
    promoted: PromotedAncestorEvidence;
  }> = canonicalRoots
    .slice()
    .reverse()
    .map((folder) => ({
      folder,
      presentationParent: null,
      promoted: {
        count: 0,
        nearestCanonicalIdentity: null,
        outermostCanonicalIdentity: null,
      },
    }));

  while (traversalStack.length) {
    const { folder, presentationParent, promoted } = traversalStack.pop()!;
    const scope = compactScope(folder, workspaceId);
    dispositions.push(
      Object.freeze({
        canonicalFolderId: folder.id,
        canonicalIdentity: folder.canonicalIdentity,
        visible: !folder.hidden,
        presentationId: folder.hidden ? null : folder.presentationId,
        structuralRole: folder.role,
        provenance: folder.provenance,
        canonicalDescendantScope: scope,
      }),
    );

    if (folder.hidden) {
      const nextPromoted: PromotedAncestorEvidence = {
        count: promoted.count + 1,
        nearestCanonicalIdentity: folder.canonicalIdentity,
        outermostCanonicalIdentity:
          promoted.outermostCanonicalIdentity ?? folder.canonicalIdentity,
      };
      for (let index = folder.children.length - 1; index >= 0; index -= 1) {
        traversalStack.push({
          folder: folder.children[index],
          presentationParent,
          promoted: nextPromoted,
        });
      }
      continue;
    }

    const evidence = Object.freeze({ ...promoted });
    const node: MutablePresentationNode = {
      presentationId: folder.presentationId,
      canonicalIdentity: folder.canonicalIdentity,
      canonicalFolderId: folder.id,
      canonicalParentId: folder.rawParentId,
      canonicalParentIdentity:
        folder.rawParentId !== null &&
        Number.isSafeInteger(folder.rawParentId) &&
        folder.rawParentId > 0
          ? canonicalFolderIdentity(workspaceId, folder.rawParentId)
          : null,
      presentationParentIdentity: presentationParent?.presentationId ?? null,
      displayName: folder.input.name,
      originalPath: folder.input.path,
      directMessageCount: folder.activeDirectCount,
      descendantMessageCount: folder.activeRollupCount,
      canonicalDirectMessageCount: folder.directCount,
      canonicalDescendantMessageCount: folder.rollupCount,
      localTrashDirectMessageCount: folder.localTrashDirectCount,
      localTrashDescendantMessageCount: folder.localTrashRollupCount,
      workspaceId,
      structuralRole: folder.role,
      provenance: folder.provenance,
      ancestorWasPromoted: evidence.count > 0,
      promotedAncestorEvidence: evidence,
      canonicalDescendantScope: scope,
      children: [],
    };
    if (presentationParent) {
      (presentationParent.children as MutablePresentationNode[]).push(node);
    } else {
      presentationRoots.push(node);
    }
    flatNodes.push(node);
    for (let index = folder.children.length - 1; index >= 0; index -= 1) {
      traversalStack.push({
        folder: folder.children[index],
        presentationParent: node,
        promoted: evidence,
      });
    }
  }

  presentationRoots.sort(comparePresentationFolders);
  for (const node of flatNodes) {
    (node.children as MutablePresentationNode[]).sort(comparePresentationFolders);
  }
  const sortedFlatNodes: MutablePresentationNode[] = [];
  const presentationStack = [...presentationRoots].reverse();
  while (presentationStack.length) {
    const node = presentationStack.pop()!;
    sortedFlatNodes.push(node);
    const children = node.children as MutablePresentationNode[];
    for (let index = children.length - 1; index >= 0; index -= 1) {
      presentationStack.push(children[index]);
    }
  }

  for (let index = sortedFlatNodes.length - 1; index >= 0; index -= 1) {
    const node = sortedFlatNodes[index];
    node.children = Object.freeze([...(node.children as MutablePresentationNode[])]);
    Object.freeze(node);
  }

  return Object.freeze({
    status: diagnostics.length ? ("recovered" as const) : ("ok" as const),
    mode,
    workspaceId,
    roots: Object.freeze([...presentationRoots]) as readonly PresentationFolderNode[],
    flatNodes: Object.freeze([...sortedFlatNodes]) as readonly PresentationFolderNode[],
    dispositions: Object.freeze(dispositions),
    diagnostics: Object.freeze(diagnostics),
  });
}

export function resolveFolderPresentationSelection(
  hierarchy: FolderHierarchyResult,
  selectedFolderId: number | null,
): FolderPresentationSelection {
  if (selectedFolderId === null) {
    return { kind: "all_mail", canonicalScope: null, presentationId: null };
  }
  const disposition = hierarchy.dispositions.find(
    (candidate) => candidate.canonicalFolderId === selectedFolderId,
  );
  if (!disposition) {
    return { kind: "missing", canonicalScope: null, presentationId: null };
  }
  if (!disposition.visible) {
    return {
      kind: "promoted_scope",
      canonicalScope: disposition.canonicalDescendantScope,
      presentationId: null,
    };
  }
  return {
    kind: "visible",
    canonicalScope: disposition.canonicalDescendantScope,
    presentationId: disposition.presentationId!,
  };
}

export function canonicalFolderSearchScopeForSelection(
  hierarchy: FolderHierarchyResult,
  selectedFolderId: number | null,
  includeSubfolders: boolean,
): CanonicalFolderSearchScope | null {
  if (selectedFolderId === null) {
    return Object.freeze({
      workspaceId: hierarchy.workspaceId,
      folderId: null,
      includeSubfolders,
      pathIdentity: null,
    });
  }
  const disposition = hierarchy.dispositions.find(
    (candidate) => candidate.canonicalFolderId === selectedFolderId,
  );
  if (!disposition) return null;
  return Object.freeze({
    workspaceId: hierarchy.workspaceId,
    folderId: disposition.canonicalFolderId,
    includeSubfolders,
    pathIdentity: disposition.canonicalDescendantScope.pathIdentity,
  });
}

export function canonicalFolderSearchScopeKey(scope: CanonicalFolderSearchScope): string {
  const path = scope.pathIdentity ?? "";
  const folder = scope.folderId === null ? "all" : String(scope.folderId);
  return `scope:${scope.workspaceId.length}:${scope.workspaceId}:${folder}:${
    scope.includeSubfolders ? "descendants" : "direct"
  }:${path.length}:${path}`;
}

export function emptyFolderHierarchyExpansionState(): FolderHierarchyExpansionState {
  return Object.freeze({
    simplified: Object.freeze([]),
    original: Object.freeze([]),
  });
}

function normalizedIdentityList(value: unknown): readonly string[] {
  if (!Array.isArray(value)) return Object.freeze([]);
  return Object.freeze(
    Array.from(new Set(value.filter((item): item is string => typeof item === "string"))).sort(
      compareText,
    ),
  );
}

export function normalizeFolderHierarchyExpansionState(
  value: unknown,
): FolderHierarchyExpansionState {
  if (!value || typeof value !== "object") return emptyFolderHierarchyExpansionState();
  const candidate = value as Partial<Record<FolderHierarchyMode, unknown>>;
  return Object.freeze({
    simplified: normalizedIdentityList(candidate.simplified),
    original: normalizedIdentityList(candidate.original),
  });
}

export function updateFolderHierarchyExpansionState(
  state: FolderHierarchyExpansionState,
  mode: FolderHierarchyMode,
  canonicalIdentity: string,
  expanded: boolean,
): FolderHierarchyExpansionState {
  const current = new Set(state[mode]);
  if (expanded) current.add(canonicalIdentity);
  else current.delete(canonicalIdentity);
  return Object.freeze({
    simplified:
      mode === "simplified"
        ? Object.freeze([...current].sort(compareText))
        : Object.freeze([...state.simplified]),
    original:
      mode === "original"
        ? Object.freeze([...current].sort(compareText))
        : Object.freeze([...state.original]),
  });
}
