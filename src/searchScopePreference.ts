export type SearchScope = "current" | "all_open";

export const defaultSearchScope: SearchScope = "current";
export const searchScopeStorageKey = "pstQuickView.searchScope";

export function normalizeSearchScope(value: unknown): SearchScope {
  return value === "current" || value === "all_open" ? value : defaultSearchScope;
}

export function storedSearchScope(
  storage: Pick<Storage, "getItem"> | null,
): SearchScope {
  if (!storage) return defaultSearchScope;
  try {
    return normalizeSearchScope(storage.getItem(searchScopeStorageKey));
  } catch {
    return defaultSearchScope;
  }
}

export function saveSearchScope(
  storage: Pick<Storage, "setItem"> | null,
  scope: SearchScope,
): void {
  if (!storage) return;
  try {
    storage.setItem(searchScopeStorageKey, scope);
  } catch {
    // Keep the in-memory preference when local storage is unavailable.
  }
}
