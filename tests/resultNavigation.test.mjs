import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  ResultNavigationModel,
  createPendingResultFocus,
  createResultNavigationState,
  didResultNavigationContextChange,
  resetResultNavigationMode,
  resolvePendingResultFocus,
  setResultNavigationActiveKey,
  shouldPublishResolvedNavigationKey,
} from "../src/resultNavigation.ts";

const source = readFileSync(new URL("../src/resultNavigation.ts", import.meta.url), "utf8");

function entries() {
  return [
    { key: "message:workspace-a:1", logicalKey: "row-1", logicalIndex: 0 },
    {
      key: "status:workspace-a:1",
      logicalKey: "row-2",
      logicalIndex: 1,
      focusable: false,
    },
    { key: "message:workspace-a:2", logicalKey: "row-3", logicalIndex: 2 },
    {
      key: "message:workspace-a:3",
      logicalKey: "row-6",
      logicalIndex: 5,
      parentKey: "message:workspace-a:1",
    },
    { key: "message:workspace-a:4", logicalKey: "row-9", logicalIndex: 8 },
  ];
}

function snapshot(overrides = {}) {
  return {
    query: "",
    from: "",
    recipients: "",
    subject: "",
    body: "",
    attachment: "",
    hasAttachments: "any",
    dateFrom: "",
    dateTo: "",
    folderScope: "current_subfolders",
    scope: "current",
    activeWorkspaceId: "workspace-a",
    workspaceIds: ["workspace-a"],
    selectedWorkspaceId: "workspace-a",
    useMultiWorkspace: false,
    singleWorkspaceId: "workspace-a",
    folderId: 1,
    includeSubfolders: true,
    conversationScopes: [
      { workspaceId: "workspace-a", folderId: 1, includeSubfolders: true },
    ],
    listMode: "messages",
    messageSort: "newest",
    conversationSort: "newest",
    sessionGeneration: 1,
    ...overrides,
  };
}

test("empty and one-entry models resolve safe navigation boundaries", () => {
  const empty = new ResultNavigationModel();
  assert.equal(empty.firstKey(), null);
  assert.equal(empty.lastKey(), null);
  assert.equal(empty.nextKey("stale"), null);
  assert.equal(empty.resolveActiveKey("stale"), null);

  const one = new ResultNavigationModel([entries()[0]]);
  assert.equal(one.firstKey(), "message:workspace-a:1");
  assert.equal(one.lastKey(), "message:workspace-a:1");
  assert.equal(one.nextKey(one.firstKey()), null);
  assert.equal(one.previousKey(one.firstKey()), null);
});

test("next, previous, Home, and End skip disabled logical entries", () => {
  const model = new ResultNavigationModel(entries());
  assert.equal(model.size, 4);
  assert.equal(model.nextKey("message:workspace-a:1"), "message:workspace-a:2");
  assert.equal(model.previousKey("message:workspace-a:3"), "message:workspace-a:2");
  assert.equal(model.firstKey(), "message:workspace-a:1");
  assert.equal(model.lastKey(), "message:workspace-a:4");
  assert.equal(model.has("status:workspace-a:1"), false);
});

test("Page Up and Page Down choose nearest focusable logical destinations", () => {
  const model = new ResultNavigationModel(entries());
  assert.equal(
    model.pageKey("message:workspace-a:1", 6, 1),
    "message:workspace-a:4",
  );
  assert.equal(
    model.pageKey("message:workspace-a:4", 4, -1),
    "message:workspace-a:2",
  );
  assert.equal(
    model.pageKey("message:workspace-a:2", 2, 1),
    "message:workspace-a:3",
  );
});

test("stale active keys prefer selection, then the nearest surviving entry", () => {
  const model = new ResultNavigationModel(entries());
  assert.equal(
    model.resolveActiveKey("removed", "message:workspace-a:3", 7),
    "message:workspace-a:3",
  );
  assert.equal(model.resolveActiveKey("removed", null, 4), "message:workspace-a:3");
  assert.equal(model.resolveActiveKey("removed"), "message:workspace-a:1");
});

test("append preserves an active key and exposes the appended final entry", () => {
  const before = new ResultNavigationModel(entries().slice(0, 3));
  const active = before.resolveActiveKey("message:workspace-a:2");
  const after = new ResultNavigationModel(entries());
  assert.equal(after.resolveActiveKey(active), "message:workspace-a:2");
  assert.equal(after.lastKey(), "message:workspace-a:4");
});

test("parent lookup supports conversation child-to-header navigation", () => {
  const model = new ResultNavigationModel(entries());
  assert.equal(
    model.firstChildKey("message:workspace-a:1"),
    "message:workspace-a:3",
  );
  assert.equal(
    model.entry("message:workspace-a:3")?.parentKey,
    "message:workspace-a:1",
  );
});

test("duplicate and malformed keys do not create duplicate active entries", () => {
  const first = entries()[0];
  const model = new ResultNavigationModel([
    first,
    { ...first, logicalIndex: 99 },
    { key: "", logicalKey: "row-x", logicalIndex: 1 },
    { key: "bad", logicalKey: "", logicalIndex: 1 },
    { key: "negative", logicalKey: "row-y", logicalIndex: -1 },
  ]);
  assert.equal(model.size, 1);
  assert.equal(model.entry(first.key)?.logicalIndex, 0);
});

test("100,000 entries retain constant-time adjacent lookup structures", () => {
  const large = new ResultNavigationModel(
    Array.from({ length: 100_000 }, (_, index) => ({
      key: `message:workspace-a:${index}`,
      logicalKey: `row-${index}`,
      logicalIndex: index,
    })),
  );
  assert.equal(large.size, 100_000);
  assert.equal(large.nextKey("message:workspace-a:50000"), "message:workspace-a:50001");
  assert.equal(large.previousKey("message:workspace-a:50000"), "message:workspace-a:49999");
  const adjacentMethods = source.slice(
    source.indexOf("nextKey("),
    source.indexOf("resolveActiveKey("),
  );
  assert.doesNotMatch(adjacentMethods, /\bfor\b|\.find(?:Index)?\(/);
  assert.match(source, /private readonly focusablePositionByKey = new Map/);
});

test("pending focus waits for an unmounted target and resolves after mount", () => {
  const model = new ResultNavigationModel(entries());
  const pending = createPendingResultFocus(model, "message:workspace-a:3", 7);
  assert.deepEqual(pending, { key: "message:workspace-a:3", resetIdentity: 7 });
  assert.equal(resolvePendingResultFocus(pending, model, new Set(), 7), "wait");
  assert.equal(
    resolvePendingResultFocus(pending, model, new Set(["message:workspace-a:3"]), 7),
    "focus",
  );
});

test("pending focus is cancelled by reset or target removal", () => {
  const model = new ResultNavigationModel(entries());
  const pending = createPendingResultFocus(model, "message:workspace-a:3", 7);
  assert.equal(resolvePendingResultFocus(pending, model, new Set(), 8), "cancel");
  assert.equal(
    resolvePendingResultFocus(pending, new ResultNavigationModel(), new Set(), 7),
    "cancel",
  );
  assert.equal(createPendingResultFocus(model, "stale", 7), null);
});

test("navigation identity remains opaque and contains no display or mailbox fields", () => {
  const model = new ResultNavigationModel(entries());
  const pending = createPendingResultFocus(model, model.firstKey(), 3);
  assert.deepEqual(Object.keys(pending).sort(), ["key", "resetIdentity"]);
  assert.doesNotMatch(JSON.stringify(pending), /query|subject|sender|recipient|body|path/i);
});

test("Messages and Conversations retain independent active keys across mode switches", () => {
  let state = createResultNavigationState();
  state = setResultNavigationActiveKey(state, "messages", "message:workspace-a:1");
  state = setResultNavigationActiveKey(
    state,
    "conversations",
    "conversation-header:workspace-a:1",
  );
  assert.deepEqual(state, {
    messages: "message:workspace-a:1",
    conversations: "conversation-header:workspace-a:1",
  });
  assert.equal(
    setResultNavigationActiveKey(state, "messages", state.messages),
    state,
  );
});

test("a transient empty list retains its mode key until replacement rows arrive", () => {
  assert.equal(
    shouldPublishResolvedNavigationKey("message:workspace-a:1", null, 0),
    false,
  );
  assert.equal(
    shouldPublishResolvedNavigationKey("removed", "message:workspace-a:2", 3),
    true,
  );
  assert.equal(shouldPublishResolvedNavigationKey(null, null, 0), false);
});

test("mode reset, workspace closure, and final closure clear only intended state", () => {
  let state = {
    messages: "message:workspace-a:1",
    conversations: "conversation-header:workspace-a:1",
  };
  state = resetResultNavigationMode(state, "messages");
  assert.equal(state.messages, null);
  assert.notEqual(state.conversations, null);
  state = resetResultNavigationMode(state, "conversations");
  assert.deepEqual(state, createResultNavigationState());
});

test("a pure list-mode switch preserves navigation context", () => {
  const previous = snapshot({ listMode: "messages" });
  const next = snapshot({ listMode: "conversations" });
  assert.equal(didResultNavigationContextChange(previous, next, "messages"), false);
  assert.equal(didResultNavigationContextChange(previous, next, "conversations"), false);
});

test("search, sort, scope, and workspace changes invalidate affected navigation", () => {
  const previous = snapshot();
  assert.equal(
    didResultNavigationContextChange(previous, snapshot({ query: "synthetic" }), "messages"),
    true,
  );
  assert.equal(
    didResultNavigationContextChange(previous, snapshot({ messageSort: "oldest" }), "messages"),
    true,
  );
  assert.equal(
    didResultNavigationContextChange(previous, snapshot({ messageSort: "oldest" }), "conversations"),
    false,
  );
  assert.equal(
    didResultNavigationContextChange(
      previous,
      snapshot({ activeWorkspaceId: null, workspaceIds: [], singleWorkspaceId: null }),
      "messages",
    ),
    true,
  );
});
