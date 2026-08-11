import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  clearAllSearchDraft,
  createAppliedSearchSnapshot,
  createMessagePaginationState,
  cursorForMessagePage,
  emptySearchDraft,
  getMessagePaginationMode,
  settleMessagePagination,
} from "../src/searchRequest.ts";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");

function snapshot(overrides = {}, contextOverrides = {}) {
  return createAppliedSearchSnapshot(
    { ...emptySearchDraft(), ...overrides },
    {
      scope: "current",
      activeWorkspaceId: "workspace-a",
      workspaceIds: ["workspace-a"],
      selectedWorkspaceId: "workspace-a",
      useMultiWorkspace: false,
      singleWorkspaceId: "workspace-a",
      folderId: null,
      includeSubfolders: true,
      conversationScopes: [
        { workspaceId: "workspace-a", folderId: null, includeSubfolders: true },
      ],
      listMode: "messages",
      messageSort: "newest",
      conversationSort: "newest",
      sessionGeneration: 1,
      ...contextOverrides,
    },
  );
}

test("single-workspace first page has no cursor and Load More uses the returned cursor", () => {
  const search = snapshot({ query: "term" });
  let state = createMessagePaginationState(search, 4);
  assert.equal(state.mode, "cursor");
  assert.equal(cursorForMessagePage(state, 4, false), null);

  state = settleMessagePagination(state, 4, 4, "cursor", true, "opaque-next");
  assert.equal(cursorForMessagePage(state, 4, true), "opaque-next");
});

test("final cursor page clears continuation and removes Load More eligibility", () => {
  const search = snapshot({ query: "term" });
  let state = createMessagePaginationState(search, 4);
  state = settleMessagePagination(state, 4, 4, "cursor", true, "opaque-next");
  state = settleMessagePagination(state, 4, 4, "cursor", false, null);
  assert.equal(state.nextCursor, null);
  assert.equal(cursorForMessagePage(state, 4, true), null);
});

test("every new backend-relevant generation starts with no cursor", () => {
  const transitions = [
    snapshot({ query: "replacement" }),
    snapshot({ from: "sender" }),
    snapshot({}, { folderId: 8 }),
    snapshot(
      {},
      {
        scope: "all_open",
        workspaceIds: ["workspace-a", "workspace-b"],
        selectedWorkspaceId: null,
        useMultiWorkspace: true,
        singleWorkspaceId: null,
      },
    ),
    snapshot({}, { messageSort: "oldest" }),
    snapshot({}, { listMode: "conversations" }),
    snapshot(
      {},
      {
        activeWorkspaceId: "workspace-b",
        workspaceIds: ["workspace-b"],
        selectedWorkspaceId: "workspace-b",
        singleWorkspaceId: "workspace-b",
      },
    ),
  ];

  for (const [index, next] of transitions.entries()) {
    const state = createMessagePaginationState(next, index + 1);
    assert.equal(state.nextCursor, null);
  }
});

test("Clear All and relevance fallback begin fresh cursor sequences", () => {
  const relevant = snapshot({ query: "term" }, { messageSort: "relevance" });
  const cleared = clearAllSearchDraft(relevant);
  const clearedSnapshot = snapshot(cleared.draft, {
    messageSort: cleared.messageSort,
    conversationSort: cleared.conversationSort,
  });
  assert.equal(clearedSnapshot.messageSort, "newest");
  assert.equal(createMessagePaginationState(clearedSnapshot, 9).nextCursor, null);

  const relevanceFallback = snapshot({}, { messageSort: "relevance" });
  assert.equal(relevanceFallback.messageSort, "newest");
  assert.equal(createMessagePaginationState(relevanceFallback, 10).nextCursor, null);
});

test("cancelled or failed Load More retains retry cursor and stale responses cannot advance it", () => {
  const search = snapshot({ query: "term" });
  const retryable = settleMessagePagination(
    createMessagePaginationState(search, 5),
    5,
    5,
    "cursor",
    true,
    "retry-cursor",
  );

  // Cancellation and failure do not call the settlement helper.
  assert.equal(cursorForMessagePage(retryable, 5, true), "retry-cursor");
  const stale = settleMessagePagination(
    retryable,
    6,
    5,
    "cursor",
    true,
    "stale-cursor",
  );
  assert.strictEqual(stale, retryable);
  assert.equal(stale.nextCursor, "retry-cursor");
});

test("true multi-workspace uses offsets while All Open narrowed to one PST uses a cursor", () => {
  const multi = snapshot(
    { query: "term" },
    {
      scope: "all_open",
      workspaceIds: ["workspace-a", "workspace-b"],
      selectedWorkspaceId: null,
      useMultiWorkspace: true,
      singleWorkspaceId: null,
    },
  );
  assert.equal(getMessagePaginationMode(multi), "offset");
  assert.equal(createMessagePaginationState(multi, 2).nextCursor, null);

  const narrowed = snapshot(
    { query: "term" },
    {
      scope: "all_open",
      workspaceIds: ["workspace-a"],
      selectedWorkspaceId: "workspace-a",
      useMultiWorkspace: false,
      singleWorkspaceId: "workspace-a",
    },
  );
  assert.equal(getMessagePaginationMode(narrowed), "cursor");

  const onlyOpenWorkspace = snapshot(
    { query: "term" },
    {
      scope: "all_open",
      workspaceIds: ["workspace-a"],
      selectedWorkspaceId: null,
      useMultiWorkspace: false,
      singleWorkspaceId: "workspace-a",
    },
  );
  assert.equal(getMessagePaginationMode(onlyOpenWorkspace), "cursor");
  assert.match(
    app,
    /openWorkspaceIds\.length > 1[\s\S]*openWorkspaceIds\.length === 1/,
  );
});

test("session restoration never restores a cursor", () => {
  const restored = snapshot(
    { query: "" },
    {
      sessionGeneration: 22,
      folderId: 4,
    },
  );
  assert.deepEqual(createMessagePaginationState(restored, 12), {
    generation: 12,
    mode: "cursor",
    nextCursor: null,
  });
});

test("App guards repeated Load More and advances cursors only after current responses", () => {
  assert.match(
    app,
    /if \(pending\?\.generation === request\.generation\) return;/,
  );
  assert.match(app, /if \(!requestIsCurrent\(\)\) return;/);
  assert.match(
    app,
    /messagePaginationRef\.current = settleMessagePagination\(/,
  );
  assert.match(
    app,
    /if \(append && paginationMode === "cursor" && !requestCursor\) return;/,
  );
});

test("App sends cursor only to the single-workspace command and offset only to multi-workspace", () => {
  const multiStart = app.indexOf('invoke<MultiMessagePageResult>("search_messages_multi"');
  const singleStart = app.indexOf('invoke<MessagePageResult>("list_messages"');
  const resultGuard = app.indexOf("if (!requestIsCurrent()) return;", singleStart);
  assert.notEqual(multiStart, -1);
  assert.notEqual(singleStart, -1);
  const multiInvoke = app.slice(multiStart, singleStart);
  const singleInvoke = app.slice(singleStart, resultGuard);
  assert.match(multiInvoke, /\boffset,\s*searchGeneration:/);
  assert.doesNotMatch(multiInvoke, /\bcursor:/);
  assert.match(singleInvoke, /cursor: requestCursor/);
  assert.doesNotMatch(singleInvoke, /\boffset,/);
});

test("exact-count lifecycle remains independent of cursor state", () => {
  const countStart = app.indexOf("async function loadMessageCount");
  const countEnd = app.indexOf("async function loadConversationsPage", countStart);
  const countSource = app.slice(countStart, countEnd);
  assert.doesNotMatch(countSource, /nextCursor|messagePaginationRef|cursor:/);
  assert.match(countSource, /count_messages_multi/);
  assert.match(countSource, /count_messages/);
});
