import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  appendUniqueByKey,
  applySearchSnapshotImmediately,
  cancellationForGeneration,
  cancellationForOperation,
  clearAppliedSearchSnapshot,
  createAppliedSearchSnapshot,
  createSearchApplicationState,
  createSearchOperationIdentity,
  emptySearchDraft,
  invalidateSearchApplication,
  isSearchCancellationError,
  searchCancelledErrorCode,
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

test("generation invalidation produces an opaque generation cancellation request", () => {
  let state = createSearchApplicationState(snapshot({ query: "private-search-value" }));
  const oldGeneration = state.applied.generation;
  state = applySearchSnapshotImmediately(state, snapshot({ query: "replacement" })).state;

  const cancellation = cancellationForGeneration(oldGeneration);
  assert.deepEqual(cancellation, { generation: oldGeneration, operationId: null });
  assert.equal(state.applied.generation, oldGeneration + 1);
  assert.doesNotMatch(JSON.stringify(cancellation), /private-search-value|replacement/);
});

test("operation identities are lane-specific and contain no query or filter content", () => {
  const messages = createSearchOperationIdentity(4, "message-page", 8);
  const messageCount = createSearchOperationIdentity(4, "message-count", 8);
  const conversations = createSearchOperationIdentity(4, "conversation-page", 8);
  const conversationCount = createSearchOperationIdentity(4, "conversation-count", 8);
  const expanded = createSearchOperationIdentity(4, "expanded-conversation", 9);

  assert.deepEqual(messages, {
    generation: 4,
    operationId: "message-page-8",
    lane: "message-page",
  });
  assert.notEqual(messages.operationId, messageCount.operationId);
  assert.notEqual(messages.operationId, conversations.operationId);
  assert.notEqual(conversations.operationId, conversationCount.operationId);
  assert.notEqual(conversations.operationId, expanded.operationId);
  assert.doesNotMatch(
    JSON.stringify([messages, messageCount, conversations, conversationCount, expanded]),
    /query|sender|subject/,
  );
});

test("exact operation cancellation applies only to the current generation", () => {
  const operation = createSearchOperationIdentity(7, "expanded-conversation", 3);
  assert.deepEqual(cancellationForOperation(operation, 7), {
    generation: 7,
    operationId: "expanded-conversation-3",
  });
  assert.equal(cancellationForOperation(operation, 8), null);
});

test("typed cancellation is silent while actual failures remain visible candidates", () => {
  assert.equal(
    isSearchCancellationError({ message: "Search cancelled.", code: searchCancelledErrorCode }),
    true,
  );
  assert.equal(isSearchCancellationError({ message: "database disk image is malformed" }), false);
  assert.equal(isSearchCancellationError("Search cancelled."), false);
});

test("mode, Scope, restore, Start Fresh, and workspace reset invalidations advance generation", () => {
  let state = createSearchApplicationState(snapshot({ query: "term" }));
  const transitions = [
    snapshot({ query: "term" }, { listMode: "conversations" }),
    snapshot(
      { query: "term" },
      {
        scope: "all_open",
        workspaceIds: ["workspace-a", "workspace-b"],
        selectedWorkspaceId: null,
        useMultiWorkspace: true,
        singleWorkspaceId: null,
      },
    ),
    clearAppliedSearchSnapshot(snapshot({ query: "term" })),
    clearAppliedSearchSnapshot(
      snapshot(
        { query: "term" },
        {
          activeWorkspaceId: null,
          workspaceIds: [],
          selectedWorkspaceId: null,
          singleWorkspaceId: null,
          conversationScopes: [],
        },
      ),
    ),
  ];

  for (const next of transitions) {
    const previous = state.applied.generation;
    state = invalidateSearchApplication(state, next);
    assert.equal(state.applied.generation, previous + 1);
    assert.deepEqual(cancellationForGeneration(previous), {
      generation: previous,
      operationId: null,
    });
  }
});

test("cancelling Load More does not mutate already loaded rows", () => {
  const existing = [{ id: 1 }, { id: 2 }];
  const operation = createSearchOperationIdentity(2, "message-load-more", 1);
  const cancellation = cancellationForOperation(operation, 2);

  assert.ok(cancellation);
  assert.deepEqual(existing, [{ id: 1 }, { id: 2 }]);
  assert.deepEqual(appendUniqueByKey(existing, [], (item) => item.id), existing);
});

test("App sends lifecycle identity on every search invoke and cancels without waiting", () => {
  assert.match(app, /function cancelBackendSearch\(request: SearchCancellationRequest\)/);
  assert.match(app, /void invoke\("cancel_search_operation"/);
  assert.match(app, /\.catch\(\(\) => undefined\)/);
  assert.match(
    app,
    /cancelBackendSearch\(cancellationForGeneration\(previous\.applied\.generation\)\)/,
  );
  assert.ok((app.match(/searchGeneration: request\.generation/g) ?? []).length >= 6);
  assert.ok((app.match(/searchOperationId: requestIdentity\.operationId/g) ?? []).length >= 6);
});

test("App ignores cancellation errors and preserves current real-error handling", () => {
  assert.ok((app.match(/if \(isSearchCancellationError\(err\)\) return;/g) ?? []).length >= 3);
  assert.match(app, /setSearchError\(\{ generation: request\.generation, message: getErrorMessage\(err\) \}\)/);
  assert.doesNotMatch(app, /setError\([^\n]*Search cancelled/);
});

test("expanded collapse cancels only its operation and generation changes cancel all lanes", () => {
  assert.match(app, /cancellationForOperation\(pending, searchGenerationRef\.current\)/);
  assert.match(app, /if \(cancellation\) cancelBackendSearch\(cancellation\)/);
  assert.match(app, /expandedConversationRequestsRef\.current\.delete\(key\)/);
  assert.match(app, /expandedConversationRequestsRef\.current\.clear\(\)/);
});

test("Relevance fallback and final workspace close retain the unified invalidation path", () => {
  assert.match(app, /setSortOrder\(draftSearchSnapshot\.messageSort\)/);
  assert.match(app, /function invalidateSearchLifecycle/);
  assert.match(app, /invalidateSearchLifecycle\([\s\S]*clearAppliedSearchSnapshot/);
  assert.match(
    app,
    /cancelBackendSearch\(cancellationForGeneration\(searchGenerationRef\.current\)\)/,
  );
});
