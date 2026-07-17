import type { Folder } from "./types";

export const allMailVirtualFolder = "all_mail" as const;

export type WorkspaceFolderSelection = {
  workspaceId: string;
  folderId: number | null;
  virtualFolder: typeof allMailVirtualFolder | null;
  includeSubfolders: boolean;
};

export function defaultWorkspaceFolderSelection(
  workspaceId: string,
  includeSubfolders = true,
): WorkspaceFolderSelection {
  return {
    workspaceId,
    folderId: null,
    virtualFolder: allMailVirtualFolder,
    includeSubfolders,
  };
}

export function normalizeStoredFolderSelection(
  workspaceId: string,
  value: unknown,
): WorkspaceFolderSelection {
  if (!value || typeof value !== "object") {
    return defaultWorkspaceFolderSelection(workspaceId);
  }

  const stored = value as Partial<WorkspaceFolderSelection>;
  if (stored.workspaceId !== workspaceId || typeof stored.includeSubfolders !== "boolean") {
    return defaultWorkspaceFolderSelection(workspaceId);
  }

  if (Number.isInteger(stored.folderId) && Number(stored.folderId) > 0) {
    return {
      workspaceId,
      folderId: Number(stored.folderId),
      virtualFolder: null,
      includeSubfolders: stored.includeSubfolders,
    };
  }

  if (stored.folderId === null && stored.virtualFolder === allMailVirtualFolder) {
    return defaultWorkspaceFolderSelection(workspaceId, stored.includeSubfolders);
  }

  return defaultWorkspaceFolderSelection(workspaceId);
}

export function resolveWorkspaceFolderSelection(
  workspaceId: string,
  folders: Pick<Folder, "id">[],
  requested: WorkspaceFolderSelection | null | undefined,
): WorkspaceFolderSelection {
  if (!requested || requested.workspaceId !== workspaceId) {
    return defaultWorkspaceFolderSelection(workspaceId);
  }

  if (requested.folderId != null && folders.some((folder) => folder.id === requested.folderId)) {
    return {
      workspaceId,
      folderId: requested.folderId,
      virtualFolder: null,
      includeSubfolders: requested.includeSubfolders,
    };
  }

  return defaultWorkspaceFolderSelection(workspaceId, requested.includeSubfolders);
}
