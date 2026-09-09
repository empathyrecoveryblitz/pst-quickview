import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  buildFolderHierarchy,
  canonicalFolderIdentity,
  canonicalFolderSearchScopeForSelection,
  canonicalFolderSearchScopeKey,
  defaultFolderHierarchyMode,
  emptyFolderHierarchyExpansionState,
  folderHierarchyStorageKey,
  normalizeFolderHierarchyExpansionState,
  normalizeFolderHierarchyMode,
  presentationFolderIdentity,
  resolveFolderPresentationSelection,
  saveFolderHierarchyMode,
  storedFolderHierarchyMode,
  updateFolderHierarchyExpansionState,
} from "../src/folderHierarchy.ts";

const moduleSource = readFileSync(
  new URL("../src/folderHierarchy.ts", import.meta.url),
  "utf8",
);
const appSource = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const searchRequestSource = readFileSync(
  new URL("../src/searchRequest.ts", import.meta.url),
  "utf8",
);
const frontendTypesSource = readFileSync(new URL("../src/types.ts", import.meta.url), "utf8");
const backendSource = readFileSync(
  new URL("../src-tauri/src/lib.rs", import.meta.url),
  "utf8",
);

function sourceSection(source, startMarker, endMarker) {
  const start = source.indexOf(startMarker);
  assert.notEqual(start, -1, `missing source marker: ${startMarker}`);
  const end = source.indexOf(endMarker, start + startMarker.length);
  assert.notEqual(end, -1, `missing source marker: ${endMarker}`);
  return source.slice(start, end);
}

function folder({
  id,
  parentId = null,
  name = `Synthetic ${id}`,
  path = `synthetic/${id}`,
  direct = 0,
  rollup = direct,
  structuralRole,
  provenance,
}) {
  return {
    id,
    parentId,
    name,
    path,
    directMessageCount: direct,
    messageCount: rollup,
    ...(structuralRole === undefined ? {} : { structuralRole }),
    ...(provenance === undefined ? {} : { provenance }),
  };
}

function technicalFolder(options) {
  return folder({
    structuralRole: "technical_wrapper",
    provenance: "readpst",
    ...options,
  });
}

function memoryStorage(initial = {}, failures = {}) {
  const values = new Map(Object.entries(initial));
  return {
    getItem(key) {
      if (failures.get) throw new Error("synthetic storage read failure");
      return values.get(key) ?? null;
    },
    setItem(key, value) {
      if (failures.set) throw new Error("synthetic storage write failure");
      values.set(key, value);
    },
    removeItem(key) {
      values.delete(key);
    },
  };
}

function nodeById(hierarchy, id) {
  return hierarchy.flatNodes.find((node) => node.canonicalFolderId === id);
}

function diagnosticCodes(hierarchy) {
  return hierarchy.diagnostics.map((diagnostic) => diagnostic.code);
}

test("empty canonical hierarchies produce empty simplified and original trees", () => {
  for (const mode of ["simplified", "original"]) {
    const hierarchy = buildFolderHierarchy([], "workspace-empty", mode);
    assert.equal(hierarchy.status, "ok");
    assert.deepEqual(hierarchy.roots, []);
    assert.deepEqual(hierarchy.flatNodes, []);
    assert.deepEqual(hierarchy.dispositions, []);
  }
});

test("one normal folder remains visible with its canonical details", () => {
  const input = folder({ id: 7, name: "Leaf A", path: "branch/leaf-a", direct: 3 });
  const hierarchy = buildFolderHierarchy([input], "workspace-one", "simplified");
  const node = hierarchy.roots[0];

  assert.equal(hierarchy.status, "ok");
  assert.equal(hierarchy.roots.length, 1);
  assert.equal(node.canonicalFolderId, 7);
  assert.equal(node.canonicalParentId, null);
  assert.equal(node.presentationParentIdentity, null);
  assert.equal(node.originalPath, "branch/leaf-a");
  assert.equal(node.directMessageCount, 3);
  assert.equal(node.descendantMessageCount, 3);
  assert.equal(node.canonicalDirectMessageCount, 3);
  assert.equal(node.canonicalDescendantMessageCount, 3);
  assert.equal(node.localTrashDirectMessageCount, 0);
  assert.equal(node.localTrashDescendantMessageCount, 0);
});

test("local count overlays change presentation counts without changing canonical structure", () => {
  const input = {
    ...folder({ id: 8, name: "Leaf B", path: "branch/leaf-b", direct: 7 }),
    activeDirectMessageCount: 4,
    activeMessageCount: 4,
    localTrashDirectMessageCount: 3,
    localTrashMessageCount: 3,
  };
  const hierarchy = buildFolderHierarchy([input], "workspace-overlay", "simplified");
  const node = hierarchy.roots[0];

  assert.equal(hierarchy.status, "ok");
  assert.equal(node.directMessageCount, 4);
  assert.equal(node.descendantMessageCount, 4);
  assert.equal(node.canonicalDirectMessageCount, 7);
  assert.equal(node.canonicalDescendantMessageCount, 7);
  assert.equal(node.localTrashDirectMessageCount, 3);
  assert.equal(node.localTrashDescendantMessageCount, 3);
  assert.equal(node.canonicalFolderId, 8);
  assert.equal(node.canonicalDescendantScope.folderId, 8);
});

test("invalid local count overlays fail open to canonical presentation counts", () => {
  const input = {
    ...folder({ id: 9, name: "Leaf C", path: "branch/leaf-c", direct: 5 }),
    activeDirectMessageCount: 1,
    activeMessageCount: 1,
    localTrashDirectMessageCount: 1,
    localTrashMessageCount: 1,
  };
  const node = buildFolderHierarchy([input], "workspace-overlay-invalid", "original").roots[0];

  assert.equal(node.directMessageCount, 5);
  assert.equal(node.descendantMessageCount, 5);
  assert.equal(node.localTrashDirectMessageCount, 0);
  assert.equal(node.localTrashDescendantMessageCount, 0);
});

test("the canonical empty-path application root is omitted only in simplified mode", () => {
  const records = [
    folder({ id: 1, name: "Application Node", path: "", direct: 0, rollup: 4 }),
    folder({ id: 2, parentId: 1, name: "Leaf A", direct: 4 }),
  ];
  const simplified = buildFolderHierarchy(records, "workspace-root", "simplified");
  const original = buildFolderHierarchy(records, "workspace-root", "original");

  assert.deepEqual(simplified.flatNodes.map((node) => node.canonicalFolderId), [2]);
  assert.equal(simplified.roots[0].ancestorWasPromoted, true);
  assert.equal(original.roots[0].canonicalFolderId, 1);
  assert.equal(original.roots[0].children[0].canonicalFolderId, 2);
  assert.equal(original.flatNodes.length, 2);
});

test("one proven technical wrapper promotes its child without changing identity", () => {
  const records = [
    technicalFolder({ id: 10, name: "Container A", rollup: 2 }),
    folder({ id: 11, parentId: 10, name: "Leaf A", direct: 2 }),
  ];
  const hierarchy = buildFolderHierarchy(records, "workspace-wrapper", "simplified");
  const promoted = hierarchy.roots[0];

  assert.deepEqual(hierarchy.flatNodes.map((node) => node.canonicalFolderId), [11]);
  assert.equal(promoted.canonicalIdentity, canonicalFolderIdentity("workspace-wrapper", 11));
  assert.equal(promoted.presentationParentIdentity, null);
  assert.equal(promoted.canonicalParentId, 10);
  assert.equal(promoted.ancestorWasPromoted, true);
  assert.equal(promoted.promotedAncestorEvidence.count, 1);
});

test("several proven technical wrapper levels collapse with compact evidence", () => {
  const records = [
    technicalFolder({ id: 20, name: "Container A", rollup: 5 }),
    technicalFolder({ id: 21, parentId: 20, name: "Container B", rollup: 5 }),
    technicalFolder({ id: 22, parentId: 21, name: "Container C", rollup: 5 }),
    folder({ id: 23, parentId: 22, name: "Leaf A", direct: 5 }),
  ];
  const hierarchy = buildFolderHierarchy(records, "workspace-chain", "simplified");
  const promoted = hierarchy.roots[0];

  assert.equal(promoted.canonicalFolderId, 23);
  assert.equal(promoted.promotedAncestorEvidence.count, 3);
  assert.equal(
    promoted.promotedAncestorEvidence.nearestCanonicalIdentity,
    canonicalFolderIdentity("workspace-chain", 22),
  );
  assert.equal(
    promoted.promotedAncestorEvidence.outermostCanonicalIdentity,
    canonicalFolderIdentity("workspace-chain", 20),
  );
});

test("a proven technical wrapper promotes multiple children independently", () => {
  const records = [
    technicalFolder({ id: 30, name: "Container A", rollup: 3 }),
    folder({ id: 31, parentId: 30, name: "Leaf Z", direct: 1 }),
    folder({ id: 32, parentId: 30, name: "Leaf A", direct: 2 }),
  ];
  const hierarchy = buildFolderHierarchy(records, "workspace-many", "simplified");

  assert.deepEqual(hierarchy.roots.map((node) => node.displayName), ["Leaf A", "Leaf Z"]);
  assert.deepEqual(
    new Set(hierarchy.roots.map((node) => node.canonicalFolderId)),
    new Set([31, 32]),
  );
  assert.equal(hierarchy.dispositions.filter((item) => item.visible).length, 2);
});

test("a technical wrapper containing direct messages remains visible", () => {
  const records = [
    technicalFolder({ id: 40, name: "Container A", direct: 2, rollup: 5 }),
    folder({ id: 41, parentId: 40, name: "Leaf A", direct: 3 }),
  ];
  const hierarchy = buildFolderHierarchy(records, "workspace-direct", "simplified");

  assert.equal(hierarchy.roots[0].canonicalFolderId, 40);
  assert.equal(hierarchy.roots[0].directMessageCount, 2);
  assert.ok(diagnosticCodes(hierarchy).includes("technical_wrapper_has_direct_messages"));
});

test("a technical wrapper with a count mismatch fails open", () => {
  const records = [
    technicalFolder({ id: 45, name: "Container A", rollup: 4 }),
    folder({ id: 46, parentId: 45, name: "Leaf A", direct: 3 }),
  ];
  const hierarchy = buildFolderHierarchy(records, "workspace-mismatch", "simplified");

  assert.equal(hierarchy.roots[0].canonicalFolderId, 45);
  assert.ok(diagnosticCodes(hierarchy).includes("count_mismatch"));
});

test("an uncertain PST-named folder remains visible", () => {
  const hierarchy = buildFolderHierarchy(
    [folder({ id: 50, name: "PST", path: "named/pst" })],
    "workspace-name-pst",
    "simplified",
  );
  assert.equal(hierarchy.roots[0].canonicalFolderId, 50);
});

test("a legitimate Archive folder remains visible", () => {
  const hierarchy = buildFolderHierarchy(
    [folder({ id: 51, name: "Archive", direct: 1 })],
    "workspace-name-archive",
    "simplified",
  );
  assert.equal(hierarchy.roots[0].canonicalFolderId, 51);
});

test("an Outlook Data File-named folder without provenance remains visible", () => {
  const hierarchy = buildFolderHierarchy(
    [folder({ id: 52, name: "Outlook Data File" })],
    "workspace-name-data-file",
    "simplified",
  );
  assert.equal(hierarchy.roots[0].canonicalFolderId, 52);
});

test("an explicitly technical node without importer provenance remains visible", () => {
  const hierarchy = buildFolderHierarchy(
    [
      folder({
        id: 53,
        name: "Container A",
        structuralRole: "technical_wrapper",
        provenance: "unknown",
      }),
    ],
    "workspace-no-provenance",
    "simplified",
  );

  assert.equal(hierarchy.roots[0].canonicalFolderId, 53);
  assert.ok(diagnosticCodes(hierarchy).includes("technical_wrapper_provenance_unavailable"));
});

test("same-named siblings remain distinct and deterministically ordered", () => {
  const records = [
    folder({ id: 60, name: "Parent A", rollup: 2 }),
    folder({ id: 62, parentId: 60, name: "Repeated", path: "b/repeated", direct: 1 }),
    folder({ id: 61, parentId: 60, name: "Repeated", path: "a/repeated", direct: 1 }),
  ];
  const hierarchy = buildFolderHierarchy(records, "workspace-siblings", "simplified");
  const siblings = hierarchy.roots[0].children;

  assert.deepEqual(siblings.map((node) => node.canonicalFolderId), [61, 62]);
  assert.notEqual(siblings[0].presentationId, siblings[1].presentationId);
});

test("same-named promoted folders remain distinct", () => {
  const records = [
    technicalFolder({ id: 70, name: "Container A", rollup: 2 }),
    folder({ id: 71, parentId: 70, name: "Repeated", path: "a/repeated", direct: 1 }),
    folder({ id: 72, parentId: 70, name: "Repeated", path: "b/repeated", direct: 1 }),
  ];
  const hierarchy = buildFolderHierarchy(records, "workspace-promoted-twins", "simplified");

  assert.deepEqual(hierarchy.roots.map((node) => node.canonicalFolderId), [71, 72]);
  assert.equal(new Set(hierarchy.roots.map((node) => node.presentationId)).size, 2);
});

test("children promoted from separate wrappers are re-sorted as visible siblings", () => {
  const records = [
    technicalFolder({ id: 38, name: "A wrapper", path: "technical/a", rollup: 1 }),
    folder({ id: 39, parentId: 38, name: "Z child", path: "technical/a/z", direct: 1 }),
    technicalFolder({ id: 40, name: "B wrapper", path: "technical/b", rollup: 1 }),
    folder({ id: 41, parentId: 40, name: "A child", path: "technical/b/a", direct: 1 }),
  ];

  const simplified = buildFolderHierarchy(records, "workspace-promoted-order", "simplified");

  assert.deepEqual(
    simplified.roots.map((node) => node.displayName),
    ["A child", "Z child"],
  );
  assert.deepEqual(
    simplified.flatNodes.map((node) => node.canonicalFolderId),
    [41, 39],
  );
});

test("direct and roll-up counts survive promotion without double counting", () => {
  const records = [
    technicalFolder({ id: 80, name: "Container A", rollup: 5 }),
    folder({ id: 81, parentId: 80, name: "Branch A", direct: 1, rollup: 3 }),
    folder({ id: 82, parentId: 81, name: "Leaf A", direct: 2 }),
    folder({ id: 83, parentId: 80, name: "Leaf B", direct: 2 }),
  ];
  const hierarchy = buildFolderHierarchy(records, "workspace-counts", "simplified");

  assert.equal(nodeById(hierarchy, 81).directMessageCount, 1);
  assert.equal(nodeById(hierarchy, 81).descendantMessageCount, 3);
  assert.equal(nodeById(hierarchy, 82).directMessageCount, 2);
  assert.equal(nodeById(hierarchy, 83).descendantMessageCount, 2);
  assert.equal(hierarchy.roots.reduce((total, node) => total + node.descendantMessageCount, 0), 5);
  assert.equal(new Set(hierarchy.flatNodes.map((node) => node.canonicalFolderId)).size, 3);
});

test("every node and hidden disposition retains canonical descendant scope", () => {
  const records = [
    technicalFolder({ id: 90, name: "Container A", path: "technical/a", rollup: 1 }),
    folder({ id: 91, parentId: 90, name: "Leaf A", path: "technical/a/leaf", direct: 1 }),
  ];
  const hierarchy = buildFolderHierarchy(records, "workspace-scope", "simplified");
  const visibleScope = hierarchy.roots[0].canonicalDescendantScope;
  const hiddenScope = hierarchy.dispositions.find((item) => item.canonicalFolderId === 90)
    .canonicalDescendantScope;

  assert.deepEqual(visibleScope, {
    workspaceId: "workspace-scope",
    folderId: 91,
    includeSubfolders: true,
    pathIdentity: "technical/a/leaf",
  });
  assert.equal(hiddenScope.folderId, 90);
  assert.equal(hiddenScope.includeSubfolders, true);
  assert.equal(hiddenScope.pathIdentity, "technical/a");
});

test("canonical and presentation identities are stable and workspace-qualified", () => {
  const record = folder({ id: 100, name: "Leaf A", direct: 1 });
  const simplified = buildFolderHierarchy([record], "workspace-a", "simplified");
  const original = buildFolderHierarchy([record], "workspace-a", "original");
  const otherWorkspace = buildFolderHierarchy([record], "workspace-b", "simplified");

  assert.equal(simplified.roots[0].canonicalIdentity, original.roots[0].canonicalIdentity);
  assert.equal(simplified.roots[0].presentationId, original.roots[0].presentationId);
  assert.notEqual(simplified.roots[0].canonicalIdentity, otherWorkspace.roots[0].canonicalIdentity);
  assert.notEqual(simplified.roots[0].presentationId, otherWorkspace.roots[0].presentationId);
  assert.notEqual(canonicalFolderIdentity("a:2", 3), canonicalFolderIdentity("a", 2));
  assert.equal(
    simplified.roots[0].presentationId,
    presentationFolderIdentity("workspace-a", 100),
  );
});

test("a malformed parent identity is detached safely", () => {
  const hierarchy = buildFolderHierarchy(
    [folder({ id: 110, parentId: 0, name: "Leaf A", direct: 1 })],
    "workspace-invalid-parent",
    "simplified",
  );

  assert.equal(hierarchy.roots[0].canonicalFolderId, 110);
  assert.ok(diagnosticCodes(hierarchy).includes("invalid_parent_id"));
});

test("a missing parent is detached safely while its raw parent is retained", () => {
  const hierarchy = buildFolderHierarchy(
    [folder({ id: 111, parentId: 999, name: "Leaf A", direct: 1 })],
    "workspace-missing-parent",
    "simplified",
  );
  const node = hierarchy.roots[0];

  assert.equal(node.canonicalParentId, 999);
  assert.equal(node.presentationParentIdentity, null);
  assert.ok(diagnosticCodes(hierarchy).includes("missing_parent"));
});

test("a self-parent is detached and cannot enter the presentation tree", () => {
  const hierarchy = buildFolderHierarchy(
    [folder({ id: 112, parentId: 112, name: "Leaf A", direct: 1 })],
    "workspace-self-parent",
    "simplified",
  );

  assert.equal(hierarchy.roots.length, 1);
  assert.equal(hierarchy.flatNodes.length, 1);
  assert.ok(diagnosticCodes(hierarchy).includes("self_parent"));
});

test("cycles are detached without duplication or loss", () => {
  const hierarchy = buildFolderHierarchy(
    [
      folder({ id: 120, parentId: 121, name: "Leaf A", direct: 1 }),
      folder({ id: 121, parentId: 120, name: "Leaf B", direct: 1 }),
      folder({ id: 122, parentId: 121, name: "Leaf C", direct: 1 }),
    ],
    "workspace-cycle",
    "original",
  );

  assert.equal(hierarchy.flatNodes.length, 3);
  assert.equal(new Set(hierarchy.flatNodes.map((node) => node.canonicalFolderId)).size, 3);
  assert.ok(diagnosticCodes(hierarchy).includes("cycle"));
});

test("duplicate canonical IDs cause a fatal empty fallback", () => {
  const hierarchy = buildFolderHierarchy(
    [folder({ id: 130, name: "Leaf A" }), folder({ id: 130, name: "Leaf B" })],
    "workspace-duplicate",
    "simplified",
  );

  assert.equal(hierarchy.status, "invalid");
  assert.deepEqual(hierarchy.roots, []);
  assert.deepEqual(hierarchy.flatNodes, []);
  assert.ok(diagnosticCodes(hierarchy).includes("duplicate_folder_id"));
});

test("invalid workspace identity and invalid canonical IDs fail safely", () => {
  const noWorkspace = buildFolderHierarchy([folder({ id: 140 })], "", "simplified");
  const badId = buildFolderHierarchy([folder({ id: -1 })], "workspace-bad-id", "simplified");

  assert.equal(noWorkspace.status, "invalid");
  assert.ok(diagnosticCodes(noWorkspace).includes("invalid_workspace_identity"));
  assert.equal(badId.status, "invalid");
  assert.ok(diagnosticCodes(badId).includes("invalid_folder_id"));
});

test("switching simplified and original modes does not mutate canonical records", () => {
  const records = Object.freeze([
    Object.freeze(technicalFolder({ id: 150, name: "Container A", rollup: 1 })),
    Object.freeze(folder({ id: 151, parentId: 150, name: "Leaf A", direct: 1 })),
  ]);
  const before = JSON.stringify(records);
  const simplified = buildFolderHierarchy(records, "workspace-switch", "simplified");
  const original = buildFolderHierarchy(records, "workspace-switch", "original");
  const simplifiedAgain = buildFolderHierarchy(records, "workspace-switch", "simplified");

  assert.equal(JSON.stringify(records), before);
  assert.deepEqual(simplified.flatNodes.map((node) => node.canonicalFolderId), [151]);
  assert.deepEqual(original.flatNodes.map((node) => node.canonicalFolderId), [150, 151]);
  assert.deepEqual(
    simplifiedAgain.flatNodes.map((node) => node.presentationId),
    simplified.flatNodes.map((node) => node.presentationId),
  );
});

test("selection of a hidden technical wrapper preserves its canonical scope", () => {
  const records = [
    technicalFolder({ id: 160, name: "Container A", path: "technical/a", rollup: 1 }),
    folder({ id: 161, parentId: 160, name: "Leaf A", direct: 1 }),
  ];
  const simplified = buildFolderHierarchy(records, "workspace-selection", "simplified");
  const original = buildFolderHierarchy(records, "workspace-selection", "original");
  const simplifiedSelection = resolveFolderPresentationSelection(simplified, 160);
  const originalSelection = resolveFolderPresentationSelection(original, 160);

  assert.equal(simplifiedSelection.kind, "promoted_scope");
  assert.equal(simplifiedSelection.presentationId, null);
  assert.equal(simplifiedSelection.canonicalScope.folderId, 160);
  assert.equal(simplifiedSelection.canonicalScope.includeSubfolders, true);
  assert.equal(originalSelection.kind, "visible");
  assert.deepEqual(originalSelection.canonicalScope, simplifiedSelection.canonicalScope);
});

test("selection of a meaningful folder and All Mail remain stable", () => {
  const hierarchy = buildFolderHierarchy(
    [folder({ id: 170, name: "Leaf A", direct: 1 })],
    "workspace-selection-visible",
    "simplified",
  );
  const selected = resolveFolderPresentationSelection(hierarchy, 170);

  assert.equal(selected.kind, "visible");
  assert.equal(selected.canonicalScope.folderId, 170);
  assert.equal(selected.presentationId, presentationFolderIdentity("workspace-selection-visible", 170));
  assert.deepEqual(resolveFolderPresentationSelection(hierarchy, null), {
    kind: "all_mail",
    canonicalScope: null,
    presentationId: null,
  });
  assert.equal(resolveFolderPresentationSelection(hierarchy, 999).kind, "missing");
});

test("canonical search scope is independent of presentation mode and ancestry", () => {
  const records = [
    technicalFolder({ id: 180, name: "Container A", rollup: 1 }),
    folder({ id: 181, parentId: 180, name: "Leaf A", path: "technical/a/leaf", direct: 1 }),
  ];
  const simplified = buildFolderHierarchy(records, "workspace-search", "simplified");
  const original = buildFolderHierarchy(records, "workspace-search", "original");
  const simpleScope = canonicalFolderSearchScopeForSelection(simplified, 181, true);
  const originalScope = canonicalFolderSearchScopeForSelection(original, 181, true);

  assert.deepEqual(simpleScope, originalScope);
  assert.equal(canonicalFolderSearchScopeKey(simpleScope), canonicalFolderSearchScopeKey(originalScope));
  assert.equal(simpleScope.folderId, 181);
  assert.equal(simpleScope.pathIdentity, "technical/a/leaf");
});

test("a real canonical search-scope change has a different invalidation key", () => {
  const hierarchy = buildFolderHierarchy(
    [
      folder({ id: 190, name: "Leaf A", direct: 1 }),
      folder({ id: 191, name: "Leaf B", direct: 1 }),
    ],
    "workspace-search-change",
    "simplified",
  );
  const first = canonicalFolderSearchScopeForSelection(hierarchy, 190, true);
  const second = canonicalFolderSearchScopeForSelection(hierarchy, 191, true);
  const directOnly = canonicalFolderSearchScopeForSelection(hierarchy, 190, false);

  assert.notEqual(canonicalFolderSearchScopeKey(first), canonicalFolderSearchScopeKey(second));
  assert.notEqual(canonicalFolderSearchScopeKey(first), canonicalFolderSearchScopeKey(directOnly));
  assert.equal(canonicalFolderSearchScopeForSelection(hierarchy, 999, true), null);
});

test("separate canonical expansion state is retained for each hierarchy mode", () => {
  const empty = emptyFolderHierarchyExpansionState();
  const simpleIdentity = canonicalFolderIdentity("workspace-expand", 200);
  const originalIdentity = canonicalFolderIdentity("workspace-expand", 201);
  const simplifiedExpanded = updateFolderHierarchyExpansionState(
    empty,
    "simplified",
    simpleIdentity,
    true,
  );
  const bothExpanded = updateFolderHierarchyExpansionState(
    simplifiedExpanded,
    "original",
    originalIdentity,
    true,
  );
  const simpleCollapsed = updateFolderHierarchyExpansionState(
    bothExpanded,
    "simplified",
    simpleIdentity,
    false,
  );

  assert.deepEqual(bothExpanded.simplified, [simpleIdentity]);
  assert.deepEqual(bothExpanded.original, [originalIdentity]);
  assert.deepEqual(simpleCollapsed.simplified, []);
  assert.deepEqual(simpleCollapsed.original, [originalIdentity]);
  assert.deepEqual(empty, { simplified: [], original: [] });
});

test("malformed expansion state is normalized, deduplicated, and sorted", () => {
  const normalized = normalizeFolderHierarchyExpansionState({
    simplified: ["z", 3, "a", "z"],
    original: "not-an-array",
  });

  assert.deepEqual(normalized, { simplified: ["a", "z"], original: [] });
  assert.deepEqual(normalizeFolderHierarchyExpansionState(null), {
    simplified: [],
    original: [],
  });
});

test("hierarchy preference defaults to Simplified and restores valid values", () => {
  const storage = memoryStorage();

  assert.equal(defaultFolderHierarchyMode, "simplified");
  assert.equal(storedFolderHierarchyMode(storage), "simplified");
  saveFolderHierarchyMode(storage, "original");
  assert.equal(storage.getItem(folderHierarchyStorageKey), "original");
  assert.equal(storedFolderHierarchyMode(storage), "original");
  saveFolderHierarchyMode(storage, "simplified");
  assert.equal(storedFolderHierarchyMode(storage), "simplified");
});

test("malformed preferences and unavailable storage fail back to Simplified", () => {
  assert.equal(normalizeFolderHierarchyMode("advanced"), "simplified");
  assert.equal(normalizeFolderHierarchyMode(null), "simplified");
  assert.equal(
    storedFolderHierarchyMode(memoryStorage({ [folderHierarchyStorageKey]: "malformed" })),
    "simplified",
  );
  assert.equal(storedFolderHierarchyMode(memoryStorage({}, { get: true })), "simplified");
  assert.doesNotThrow(() => saveFolderHierarchyMode(memoryStorage({}, { set: true }), "original"));
  assert.doesNotThrow(() => saveFolderHierarchyMode(null, "original"));
});

test("Start Fresh and workspace close preserve the independent hierarchy preference", () => {
  const sessionKey = "pstQuickView.previousSession";
  const workspaceSelectionKey = "pstQuickView.workspaceSelection.synthetic";
  const storage = memoryStorage({
    [sessionKey]: "synthetic-session",
    [workspaceSelectionKey]: "synthetic-selection",
  });
  saveFolderHierarchyMode(storage, "original");

  storage.removeItem(sessionKey);
  assert.equal(storedFolderHierarchyMode(storage), "original");
  storage.removeItem(workspaceSelectionKey);
  assert.equal(storedFolderHierarchyMode(storage), "original");
});

test("presentation output is deterministic across canonical input order", () => {
  const records = [
    folder({ id: 210, name: "Parent A", rollup: 3 }),
    folder({ id: 213, parentId: 210, name: "Leaf B", path: "b", direct: 1 }),
    folder({ id: 212, parentId: 210, name: "Leaf A", path: "z", direct: 1 }),
    folder({ id: 211, parentId: 210, name: "Leaf A", path: "a", direct: 1 }),
  ];
  const forward = buildFolderHierarchy(records, "workspace-order", "simplified");
  const reverse = buildFolderHierarchy([...records].reverse(), "workspace-order", "simplified");

  assert.deepEqual(
    forward.flatNodes.map((node) => node.canonicalFolderId),
    [210, 211, 212, 213],
  );
  assert.deepEqual(
    reverse.flatNodes.map((node) => node.canonicalFolderId),
    forward.flatNodes.map((node) => node.canonicalFolderId),
  );
});

test("folder names retain the existing locale-aware ordering with deterministic ties", () => {
  const records = [
    folder({ id: 224, name: "a", path: "synthetic/a" }),
    folder({ id: 222, name: "B", path: "synthetic/b" }),
    folder({ id: 223, name: "á", path: "synthetic/accent" }),
    folder({ id: 221, name: "A", path: "synthetic/capital" }),
  ];
  const expected = [...records]
    .sort(
      (left, right) =>
        left.name.localeCompare(right.name) ||
        left.path.localeCompare(right.path) ||
        left.id - right.id,
    )
    .map(({ id }) => id);

  const forward = buildFolderHierarchy(records, "workspace-locale-order", "original");
  const reverse = buildFolderHierarchy(
    [...records].reverse(),
    "workspace-locale-order",
    "original",
  );

  assert.deepEqual(forward.flatNodes.map((node) => node.canonicalFolderId), expected);
  assert.deepEqual(reverse.flatNodes.map((node) => node.canonicalFolderId), expected);
});

test("App renders the pure presentation tree but keeps selection and backend scope canonical", () => {
  assert.match(
    appSource,
    /import \{[\s\S]*?buildFolderHierarchy[\s\S]*?\} from "\.\/folderHierarchy";/,
  );
  assert.match(
    appSource,
    /buildFolderHierarchy\(folders, workspace\?\.id \?\? "inactive", folderHierarchyMode\)/,
  );
  assert.match(
    appSource,
    /buildFolderHierarchy\(\s*session\.folders,\s*session\.workspace\.id,\s*folderHierarchyMode,\s*\)/,
  );
  assert.match(
    appSource,
    /onSelect=\{\(folder\) =>\s*selectCurrentWorkspaceFolder\(folder\.canonicalFolderId\)/,
  );
  assert.match(
    appSource,
    /selectAllOpenWorkspaceFolder\(\s*selectedWorkspaceId,\s*folder\.canonicalFolderId,\s*\)/,
  );

  const snapshotIntegration = sourceSection(
    appSource,
    "const draftSearchSnapshot = useMemo(",
    "const draftSearchTextKey = appliedSearchTextKey(draftSearchSnapshot);",
  );
  assert.match(snapshotIntegration, /folderId: effectiveFolderId/);
  assert.match(snapshotIntegration, /includeSubfolders: effectiveIncludeSubfolders/);
  assert.doesNotMatch(snapshotIntegration, /folderHierarchyMode|presentationId|presentationParent/);

  assert.match(
    appSource,
    /invoke<MessagePageResult>\("list_messages", \{[\s\S]*?folderId: snapshot\.folderId,[\s\S]*?includeSubfolders: snapshot\.includeSubfolders,/,
  );
  assert.match(
    appSource,
    /invoke<MessageCountResult>\("count_messages", \{[\s\S]*?folderId: snapshot\.folderId,[\s\S]*?includeSubfolders: snapshot\.includeSubfolders,/,
  );
});

test("the stored hierarchy preference initializes synchronously and survives App lifecycle resets", () => {
  assert.match(
    appSource,
    /useState<FolderHierarchyMode>\(\(\) =>\s*storedFolderHierarchyMode\(typeof window === "undefined" \? null : window\.localStorage\)/,
  );
  assert.match(
    appSource,
    /useEffect\(\(\) => \{\s*saveFolderHierarchyMode\(window\.localStorage, folderHierarchyMode\);\s*\}, \[folderHierarchyMode\]\);/,
  );

  const startFresh = sourceSection(
    appSource,
    "function startFreshPreviousSession()",
    "useEffect(() => {\n    window.localStorage.setItem(workspaceLocationStorageKey",
  );
  assert.match(startFresh, /removeItem\(previousSessionStorageKey\)/);
  assert.doesNotMatch(startFresh, /folderHierarchy|setFolderHierarchyMode/);

  const workspaceClose = sourceSection(
    appSource,
    "async function closeWorkspaceTab(workspaceId: string)",
    "async function loadMessagesPage(append: boolean, request: AppliedSearchVersion)",
  );
  assert.match(workspaceClose, /invoke<WorkspaceSummary \| null>\("close_workspace"/);
  assert.doesNotMatch(workspaceClose, /folderHierarchy|setFolderHierarchyMode|localStorage/);

  assert.equal(
    [...appSource.matchAll(/\bsetFolderHierarchyMode\(/g)].length,
    2,
    "only the two explicit Layout choices may change the hierarchy preference",
  );
});

test("hierarchy mode stays outside search snapshots and real canonical scope changes apply immediately", () => {
  assert.doesNotMatch(
    searchRequestSource,
    /folderHierarchy|FolderHierarchy|presentationId|presentationParent/,
  );
  assert.match(
    searchRequestSource,
    /export type SearchSnapshotContext = \{[\s\S]*?folderId: number \| null;[\s\S]*?includeSubfolders: boolean;[\s\S]*?\};/,
  );
  assert.match(
    searchRequestSource,
    /export function appliedSearchNonTextKey[\s\S]*?\.\.\.nonText[\s\S]*?JSON\.stringify\(nonText\)/,
  );

  const searchApplication = sourceSection(
    appSource,
    "useLayoutEffect(() => {\n    clearSearchDebounceTimer();",
    "useEffect(() => {\n    if (!notice) return;",
  );
  assert.match(
    searchApplication,
    /appliedSearchNonTextKey\(current\.applied\.snapshot\) !== draftSearchNonTextKey/,
  );
  assert.match(
    searchApplication,
    /applySearchSnapshotImmediately\(current, draftSearchSnapshot\)/,
  );
  assert.doesNotMatch(searchApplication, /folderHierarchyMode/);
});

test("Folder Details and rows expose canonical paths and both canonical count forms", () => {
  const rows = sourceSection(appSource, "function FolderRows({", "function isMessagePreviewRoute()");
  assert.match(rows, /key=\{node\.presentationId\}/);
  assert.match(rows, /selectedFolderId === node\.canonicalFolderId/);
  assert.match(rows, /title=\{node\.originalPath \|\| "PST root"\}/);
  assert.match(
    rows,
    /includeSubfolders \? node\.descendantMessageCount : node\.directMessageCount/,
  );

  const details = sourceSection(
    appSource,
    '<section className="folder-details" aria-label="Folder Details">',
    "</section>",
  );
  for (const label of [
    "Displayed folder",
    "Original PST path",
    "Presentation",
    "PST",
    "Workspace ID",
    "Canonical direct messages",
    "Canonical with descendants",
  ]) {
    assert.match(details, new RegExp(`<dt>${label}</dt>`));
  }
  assert.match(details, /folderDetailsFolder\.path \|\| "\(PST root\)"/);
  assert.equal(
    (details.match(/formatCount\(folderDetailsFolder\.directMessageCount\)/g) ?? []).length,
    1,
  );
  assert.equal(
    (details.match(/formatCount\(folderDetailsFolder\.messageCount\)/g) ?? []).length,
    1,
  );
  assert.doesNotMatch(details, /Imported canonical count/);
});

test("folder hierarchy adds no mutation command or canonical presentation schema", () => {
  const controls = sourceSection(
    appSource,
    '<fieldset className="folder-hierarchy-options">',
    "</fieldset>",
  );
  assert.doesNotMatch(controls, /\binvoke\s*\(|delete|rename|move/i);
  assert.doesNotMatch(
    appSource,
    /\binvoke(?:<[^>]+>)?\s*\(\s*["'][^"']*(?:folder_hierarchy|hierarchy_folder|folder_presentation)[^"']*["']/i,
  );

  const frontendFolder = frontendTypesSource.match(/export type Folder = \{([\s\S]*?)\n\};/);
  assert.ok(frontendFolder, "canonical frontend Folder type should exist");
  assert.deepEqual(
    [...frontendFolder[1].matchAll(/^\s*(\w+):/gm)].map((match) => match[1]),
    [
      "id",
      "parentId",
      "path",
      "name",
      "messageCount",
      "directMessageCount",
      "activeMessageCount",
      "activeDirectMessageCount",
      "localTrashMessageCount",
      "localTrashDirectMessageCount",
    ],
  );

  const backendFolder = backendSource.match(/struct Folder \{([\s\S]*?)\n\}/);
  assert.ok(backendFolder, "canonical backend Folder struct should exist");
  assert.deepEqual(
    [...backendFolder[1].matchAll(/^\s*(\w+):/gm)].map((match) => match[1]),
    [
      "id",
      "parent_id",
      "path",
      "name",
      "message_count",
      "direct_message_count",
      "active_message_count",
      "active_direct_message_count",
      "local_trash_message_count",
      "local_trash_direct_message_count",
    ],
  );

  const folderTable = backendSource.match(
    /CREATE TABLE IF NOT EXISTS folders \(([\s\S]*?)\n\s*\);/,
  );
  assert.ok(folderTable, "canonical folders table should exist");
  assert.match(
    folderTable[1],
    /^\s*id INTEGER PRIMARY KEY,\s*parent_id INTEGER REFERENCES folders\(id\) ON DELETE CASCADE,\s*path TEXT NOT NULL UNIQUE,\s*name TEXT NOT NULL\s*$/,
  );
  assert.doesNotMatch(
    `${searchRequestSource}\n${backendFolder[1]}\n${folderTable[1]}`,
    /folderHierarchy|folder_hierarchy|presentationParent|presentation_parent|structuralRole|structural_role/,
  );
  assert.match(backendSource, /const SQLITE_SCHEMA_VERSION_CURRENT: i64 = 3;/);
});

test("the pure model introduces no command, database, or name-only mutation path", () => {
  assert.doesNotMatch(moduleSource, /\binvoke\s*\(/);
  assert.doesNotMatch(moduleSource, /\b(?:CREATE|ALTER|DROP)\s+TABLE\b/i);
  assert.doesNotMatch(moduleSource, /\b(?:INSERT|UPDATE|DELETE)\s+(?:INTO|FROM|folders)\b/i);
  assert.doesNotMatch(moduleSource, /\.name\.(?:includes|startsWith|endsWith|match|test)\s*\(/);
  assert.doesNotMatch(
    moduleSource,
    /["'`](?:PST|Outlook Data File|Archive|Archive Mailbox|Personal Folders|Mailbox|Root)["'`]/,
  );
});

test("100,000 synthetic canonical nodes are modeled iteratively without loss", () => {
  const total = 100_000;
  const records = new Array(total);
  for (let index = 0; index < total; index += 1) {
    const id = index + 1;
    records[index] = folder({
      id,
      parentId: id === 1 ? null : id - 1,
      name: `Synthetic ${String(id).padStart(6, "0")}`,
      path: id === 1 ? "" : `synthetic/${id}`,
      direct: id === total ? 1 : 0,
      rollup: 1,
    });
  }

  const hierarchy = buildFolderHierarchy(records, "workspace-large-synthetic", "original");

  assert.equal(hierarchy.status, "ok");
  assert.equal(hierarchy.roots.length, 1);
  assert.equal(hierarchy.flatNodes.length, total);
  assert.equal(hierarchy.dispositions.length, total);
  assert.equal(hierarchy.flatNodes[0].canonicalFolderId, 1);
  assert.equal(hierarchy.flatNodes.at(-1).canonicalFolderId, total);
  assert.equal(new Set(hierarchy.flatNodes.map((node) => node.canonicalIdentity)).size, total);
});
