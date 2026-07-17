export type AppearanceMode = "system" | "light" | "dark";

export const appearanceStorageKey = "pstQuickView.appearance";

export function normalizeAppearance(value: unknown): AppearanceMode {
  return value === "light" || value === "dark" || value === "system" ? value : "system";
}

export function storedAppearance(storage: Pick<Storage, "getItem"> | null): AppearanceMode {
  if (!storage) return "system";
  try {
    return normalizeAppearance(storage.getItem(appearanceStorageKey));
  } catch {
    return "system";
  }
}

export function applyAppearance(mode: AppearanceMode, root: HTMLElement): void {
  if (mode === "system") root.removeAttribute("data-appearance");
  else root.setAttribute("data-appearance", mode);
  root.style.colorScheme = mode === "system" ? "light dark" : mode;
}

export function applyStoredAppearance(
  storage: Pick<Storage, "getItem"> | null,
  root: HTMLElement,
): AppearanceMode {
  const mode = storedAppearance(storage);
  applyAppearance(mode, root);
  return mode;
}
