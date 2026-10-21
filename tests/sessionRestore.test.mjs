import test from "node:test";
import assert from "node:assert/strict";

import {
  allMailVirtualFolder,
  defaultWorkspaceFolderSelection,
  isLocalTrashFolderSelection,
  localTrashVirtualFolder,
  localTrashWorkspaceFolderSelection,
  normalizeStoredFolderSelection,
  resolveWorkspaceFolderSelection,
} from "../src/sessionRestore.ts";
import {
  conversationParticipantSummary,
  senderDisplayName,
} from "../src/conversationDisplay.ts";
import {
  appearanceStorageKey,
  applyAppearance,
  applyStoredAppearance,
  normalizeAppearance,
  storedAppearance,
} from "../src/appearance.ts";

const folders = [{ id: 1 }, { id: 7 }];

test("restored workspace keeps an existing scoped folder", () => {
  const restored = resolveWorkspaceFolderSelection("workspace-a", folders, {
    workspaceId: "workspace-a",
    folderId: 7,
    virtualFolder: null,
    includeSubfolders: false,
  });
  assert.deepEqual(restored, {
    workspaceId: "workspace-a",
    folderId: 7,
    virtualFolder: null,
    includeSubfolders: false,
  });
});

test("missing saved folder falls back to the workspace All Mail view", () => {
  const restored = resolveWorkspaceFolderSelection("workspace-a", folders, {
    workspaceId: "workspace-a",
    folderId: 99,
    virtualFolder: null,
    includeSubfolders: true,
  });
  assert.equal(restored.folderId, null);
  assert.equal(restored.virtualFolder, allMailVirtualFolder);
});

test("folder IDs from another workspace cannot collide", () => {
  const restored = resolveWorkspaceFolderSelection("workspace-b", folders, {
    workspaceId: "workspace-a",
    folderId: 7,
    virtualFolder: null,
    includeSubfolders: true,
  });
  assert.deepEqual(restored, defaultWorkspaceFolderSelection("workspace-b"));
});

test("an existing zero-count folder remains a valid selection", () => {
  const restored = resolveWorkspaceFolderSelection(
    "workspace-a",
    [{ id: 7, directCount: 0, rollupCount: 0 }],
    {
      workspaceId: "workspace-a",
      folderId: 7,
      virtualFolder: null,
      includeSubfolders: true,
    },
  );
  assert.equal(restored.folderId, 7);
});

test("legacy saved sessions without scoped selection default to All Mail", () => {
  assert.deepEqual(
    normalizeStoredFolderSelection("workspace-a", { folderId: 7 }),
    defaultWorkspaceFolderSelection("workspace-a"),
  );
});

test("Local Trash restores as a reserved virtual view outside numeric folder IDs", () => {
  const stored = localTrashWorkspaceFolderSelection("workspace-a", false);
  assert.equal(stored.folderId, null);
  assert.equal(stored.virtualFolder, localTrashVirtualFolder);
  assert.equal(typeof stored.virtualFolder, "string");
  assert.equal(isLocalTrashFolderSelection(stored), true);

  assert.deepEqual(
    normalizeStoredFolderSelection("workspace-a", stored),
    stored,
  );
  assert.deepEqual(
    resolveWorkspaceFolderSelection("workspace-a", folders, stored),
    stored,
  );
});

test("Local Trash cannot collide with canonical folders or cross workspaces", () => {
  const canonical = normalizeStoredFolderSelection("workspace-a", {
    workspaceId: "workspace-a",
    folderId: 7,
    virtualFolder: localTrashVirtualFolder,
    includeSubfolders: true,
  });
  assert.deepEqual(canonical, {
    workspaceId: "workspace-a",
    folderId: 7,
    virtualFolder: null,
    includeSubfolders: true,
  });

  const crossWorkspace = resolveWorkspaceFolderSelection(
    "workspace-b",
    folders,
    localTrashWorkspaceFolderSelection("workspace-a"),
  );
  assert.deepEqual(crossWorkspace, defaultWorkspaceFolderSelection("workspace-b"));
});

test("conversation participant display favors names and stays compact", () => {
  assert.equal(senderDisplayName('"Alex Rivera" <alex@example.com>'), "Alex Rivera");
  assert.equal(
    conversationParticipantSummary(
      [
        '"Alex Rivera" <alex@example.com>',
        "Morgan Lee <morgan@example.com>",
        "Taylor Smith <taylor@example.com>",
      ],
      "Alex Rivera <alex@example.com>",
    ),
    "Alex Rivera, Morgan Lee +1",
  );
});

test("appearance preference defaults safely and persists valid choices", () => {
  assert.equal(normalizeAppearance(null), "system");
  assert.equal(normalizeAppearance("sepia"), "system");
  assert.equal(storedAppearance({ getItem: (key) => key === appearanceStorageKey ? "dark" : null }), "dark");
});

test("appearance applies explicit themes and restores system mode", () => {
  const attributes = new Map();
  const root = {
    style: { colorScheme: "" },
    setAttribute: (name, value) => attributes.set(name, value),
    removeAttribute: (name) => attributes.delete(name),
  };
  applyAppearance("light", root);
  assert.equal(attributes.get("data-appearance"), "light");
  assert.equal(root.style.colorScheme, "light");
  applyAppearance("system", root);
  assert.equal(attributes.has("data-appearance"), false);
  assert.equal(root.style.colorScheme, "light dark");
});

test("explicit Light overrides system dark and explicit Dark overrides system light", () => {
  const attributes = new Map([["data-system-appearance", "dark"]]);
  const root = {
    style: { colorScheme: "" },
    setAttribute: (name, value) => attributes.set(name, value),
    removeAttribute: (name) => attributes.delete(name),
  };
  applyAppearance("light", root);
  assert.equal(attributes.get("data-appearance"), "light");
  assert.equal(root.style.colorScheme, "light");
  attributes.set("data-system-appearance", "light");
  applyAppearance("dark", root);
  assert.equal(attributes.get("data-appearance"), "dark");
  assert.equal(root.style.colorScheme, "dark");
});

test("pop-out appearance initialization reads the shared stored preference", () => {
  const attributes = new Map();
  const root = {
    style: { colorScheme: "" },
    setAttribute: (name, value) => attributes.set(name, value),
    removeAttribute: (name) => attributes.delete(name),
  };
  const mode = applyStoredAppearance({ getItem: () => "dark" }, root);
  assert.equal(mode, "dark");
  assert.equal(attributes.get("data-appearance"), "dark");
  assert.equal(root.style.colorScheme, "dark");
});
