import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  applySearchSnapshotImmediately,
  clearAllSearchDraft,
  clearAppliedSearchSnapshot,
  commitQueuedSearchSnapshot,
  createAppliedSearchSnapshot,
  createSearchApplicationState,
  emptySearchDraft,
  getRelevanceAvailability,
  queueTextSearchSnapshot,
} from "../src/searchRequest.ts";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");

function context(overrides = {}) {
  return {
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
    ...overrides,
  };
}

function snapshot(draftOverrides = {}, contextOverrides = {}) {
  return createAppliedSearchSnapshot(
    { ...emptySearchDraft(), ...draftOverrides },
    context(contextOverrides),
  );
}

test("relevance is available for one-workspace message text searches", () => {
  assert.equal(getRelevanceAvailability(snapshot({ query: "calendar" })).available, true);

  for (const key of ["from", "recipients", "subject", "body", "attachment"]) {
    const availability = getRelevanceAvailability(snapshot({ [key]: "calendar" }));
    assert.equal(availability.available, true, `${key} creates an FTS text search`);
    assert.equal(availability.effectiveWorkspaceCount, 1);
  }

  const globalAllOpenWithOneWorkspace = snapshot(
    { query: "calendar" },
    {
      scope: "all_open",
      workspaceIds: ["workspace-a"],
      selectedWorkspaceId: null,
      useMultiWorkspace: true,
      singleWorkspaceId: null,
    },
  );
  assert.equal(getRelevanceAvailability(globalAllOpenWithOneWorkspace).available, true);
});

test("effective workspace targeting controls relevance rather than Scope alone", () => {
  const multiple = snapshot(
    { query: "calendar" },
    {
      scope: "all_open",
      workspaceIds: ["workspace-a", "workspace-b", "workspace-a"],
      selectedWorkspaceId: null,
      useMultiWorkspace: true,
      singleWorkspaceId: null,
    },
  );
  assert.deepEqual(getRelevanceAvailability(multiple), {
    available: false,
    reason: "multiple_workspaces",
    explanation: "Relevance is available only when searching one PST.",
    effectiveWorkspaceCount: 2,
  });

  const narrowed = snapshot(
    { query: "calendar" },
    {
      scope: "all_open",
      workspaceIds: ["workspace-b"],
      selectedWorkspaceId: "workspace-b",
      useMultiWorkspace: false,
      singleWorkspaceId: "workspace-b",
    },
  );
  assert.equal(getRelevanceAvailability(narrowed).available, true);
  assert.equal(getRelevanceAvailability(narrowed).effectiveWorkspaceCount, 1);
});

test("relevance rejects non-text, punctuation-only, conversation, and no-workspace searches", () => {
  for (const candidate of [
    snapshot(),
    snapshot({ dateFrom: "2026-01-01" }),
    snapshot({ hasAttachments: "yes" }),
    snapshot({ query: "--- !!!" }),
    snapshot({ query: "has:yes" }),
    snapshot({ query: "before:2026-01-01" }),
    snapshot({ from: "  ", subject: "\t" }),
  ]) {
    assert.equal(getRelevanceAvailability(candidate).reason, "requires_text");
  }

  assert.equal(
    getRelevanceAvailability(snapshot({ query: "calendar" }, { listMode: "conversations" }))
      .reason,
    "conversations",
  );
  assert.equal(
    getRelevanceAvailability(
      snapshot(
        { query: "calendar" },
        {
          activeWorkspaceId: null,
          workspaceIds: [],
          selectedWorkspaceId: null,
          singleWorkspaceId: null,
          conversationScopes: [],
        },
      ),
    ).reason,
    "no_workspace",
  );
});

test("frontend FTS detection follows typed fields and punctuation behavior", () => {
  for (const query of [
    "subject:calendar",
    "from:calendar",
    "recipient:calendar",
    "body:calendar",
    "attachment:calendar",
    'subject:"calendar planning"',
    "unknown:calendar",
    "café",
    "東京",
  ]) {
    assert.equal(
      getRelevanceAvailability(snapshot({ query })).available,
      true,
      `${query} produces FTS text`,
    );
  }
  for (const query of ["subject:---", 'body:"---"', "has:no", "after:2026-01-01"]) {
    assert.equal(
      getRelevanceAvailability(snapshot({ query })).available,
      false,
      `${query} produces no FTS text`,
    );
  }
});

test("eligible relevance stays selected and every ineligible transition falls back to newest", () => {
  const selected = snapshot({ query: "calendar" }, { messageSort: "relevance" });
  assert.equal(selected.messageSort, "relevance");

  for (const next of [
    snapshot({}, { messageSort: "relevance" }),
    snapshot(
      { query: "calendar" },
      {
        scope: "all_open",
        workspaceIds: ["workspace-a", "workspace-b"],
        selectedWorkspaceId: null,
        useMultiWorkspace: true,
        singleWorkspaceId: null,
        messageSort: "relevance",
      },
    ),
    snapshot(
      { query: "calendar" },
      { listMode: "conversations", messageSort: "relevance" },
    ),
    snapshot(
      { query: "calendar" },
      {
        activeWorkspaceId: null,
        workspaceIds: [],
        selectedWorkspaceId: null,
        singleWorkspaceId: null,
        conversationScopes: [],
        messageSort: "relevance",
      },
    ),
  ]) {
    assert.equal(next.messageSort, "newest");
  }

  assert.equal(clearAllSearchDraft(selected).messageSort, "newest");
  assert.equal(clearAppliedSearchSnapshot(selected).messageSort, "newest");
});

test("a stale debounce token cannot restore relevance after eligibility is lost", () => {
  const selected = snapshot({ query: "calendar" }, { messageSort: "relevance" });
  let state = createSearchApplicationState(selected);
  const queued = queueTextSearchSnapshot(
    state,
    snapshot({ query: "calendar plan" }, { messageSort: "relevance" }),
  );
  state = queued.state;

  const immediatelyCleared = snapshot({}, { messageSort: "relevance" });
  assert.equal(immediatelyCleared.messageSort, "newest");
  state = applySearchSnapshotImmediately(state, immediatelyCleared).state;

  assert.equal(state.applied.snapshot.messageSort, "newest");
  assert.equal(commitQueuedSearchSnapshot(state, queued.token).applied, false);
});

test("App exposes an explained relevance option and only sends normalized message sorts", () => {
  assert.match(app, /<option[\s\S]*value="relevance"[\s\S]*disabled=\{!relevanceAvailability\.available\}/);
  assert.match(app, /aria-describedby=\{relevanceSortHelpId\}/);
  assert.match(app, /\{relevanceAvailability\.explanation\}/);
  assert.match(app, /sortOrder:\s*snapshot\.messageSort/);
  assert.match(app, /conversationSort:\s*snapshot\.conversationSort/);
  assert.match(app, /if \(sortOrder === draftSearchSnapshot\.messageSort\) return;/);
  assert.match(app, /setSortOrder\(draftSearchSnapshot\.messageSort\)/);
  assert.doesNotMatch(app, /conversationSort:\s*snapshot\.messageSort/);
});
