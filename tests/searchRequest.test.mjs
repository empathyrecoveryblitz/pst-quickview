import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  appendUniqueByKey,
  appliedSearchSnapshotKey,
  applySearchSnapshotImmediately,
  backendSearchFilters,
  buildAppliedFilterChips,
  canClearAll,
  classifySearchEmptyState,
  clearAllSearchDraft,
  clearAppliedSearchSnapshot,
  commitQueuedSearchSnapshot,
  createAppliedSearchSnapshot,
  createSearchApplicationState,
  emptySearchDraft,
  getActiveFilterCount,
  invalidateSearchApplication,
  isAppliedSearchActive,
  isExpandedConversationResponseCurrent,
  isSearchGenerationCurrent,
  queueTextSearchSnapshot,
  removeAppliedFilter,
  replaceAppliedSearchDraft,
} from "../src/searchRequest.ts";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const styles = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");

function context(overrides = {}) {
  return {
    scope: "current",
    activeWorkspaceId: "workspace-a",
    workspaceIds: ["workspace-a"],
    selectedWorkspaceId: "workspace-a",
    useMultiWorkspace: false,
    singleWorkspaceId: "workspace-a",
    folderId: 7,
    includeSubfolders: true,
    conversationScopes: [
      { workspaceId: "workspace-a", folderId: 7, includeSubfolders: true },
    ],
    listMode: "messages",
    messageSort: "newest",
    conversationSort: "newest",
    sessionGeneration: 1,
    ...overrides,
  };
}

function draft(overrides = {}) {
  return { ...emptySearchDraft(), ...overrides };
}

function snapshot(draftOverrides = {}, contextOverrides = {}) {
  return createAppliedSearchSnapshot(draft(draftOverrides), context(contextOverrides));
}

test("blank and whitespace-only drafts normalize to the same inactive snapshot", () => {
  const blank = snapshot();
  const whitespace = snapshot({
    query: "  ",
    from: " \n",
    recipients: "\t",
    subject: " ",
    body: " ",
    attachment: " ",
  });

  assert.equal(appliedSearchSnapshotKey(blank), appliedSearchSnapshotKey(whitespace));
  assert.equal(isAppliedSearchActive(blank), false);
  assert.deepEqual(backendSearchFilters(blank), {
    from: null,
    recipients: null,
    subject: null,
    body: null,
    attachment: null,
    hasAttachments: "any",
    dateFrom: null,
    dateTo: null,
  });
});

test("snapshot includes normalized text, filters, scope, folder identity, mode, and sorting", () => {
  const applied = snapshot(
    {
      query: "  calendar  ",
      from: "  Adam  ",
      recipients: " kev@example.com ",
      subject: " schedule ",
      body: " outlook ",
      attachment: " PDF ",
      hasAttachments: "yes",
      dateFrom: " 2026-03-01 ",
      dateTo: " 2026-06-30 ",
      folderScope: "current",
    },
    {
      scope: "all_open",
      activeWorkspaceId: "workspace-b",
      workspaceIds: ["workspace-a", "workspace-b", "workspace-a"],
      selectedWorkspaceId: "workspace-b",
      useMultiWorkspace: true,
      singleWorkspaceId: null,
      folderId: 12,
      includeSubfolders: false,
      listMode: "conversations",
      messageSort: "sender_az",
      conversationSort: "subject",
    },
  );

  assert.equal(applied.query, "calendar");
  assert.equal(applied.from, "Adam");
  assert.equal(applied.recipients, "kev@example.com");
  assert.equal(applied.subject, "schedule");
  assert.equal(applied.body, "outlook");
  assert.equal(applied.attachment, "PDF");
  assert.equal(applied.hasAttachments, "yes");
  assert.equal(applied.dateFrom, "2026-03-01");
  assert.equal(applied.dateTo, "2026-06-30");
  assert.deepEqual(applied.workspaceIds, ["workspace-a", "workspace-b"]);
  assert.equal(applied.scope, "all_open");
  assert.equal(applied.folderId, 12);
  assert.equal(applied.listMode, "conversations");
  assert.equal(applied.messageSort, "sender_az");
  assert.equal(applied.conversationSort, "subject");
  assert.equal(isAppliedSearchActive(applied), true);
  assert.deepEqual(
    buildAppliedFilterChips(applied, {
      workspaceCount: 2,
      workspaceLabel: "backup.pst",
      folderLabel: "Inbox",
    }).map(({ id, text }) => ({ id, text })),
    [
      { id: "query", text: "Search: calendar" },
      { id: "scope", text: "PST scope: All Open PSTs (2)" },
      { id: "from", text: "From: Adam" },
      { id: "recipients", text: "To/Cc/Bcc: kev@example.com" },
      { id: "subject", text: "Subject: schedule" },
      { id: "body", text: "Body: outlook" },
      { id: "attachment", text: "Attachment: PDF" },
      { id: "hasAttachments", text: "Attachments: Has attachments" },
      { id: "dateFrom", text: "Date from: 2026-03-01" },
      { id: "dateTo", text: "Date to: 2026-06-30" },
      { id: "folderSelection", text: "Folder: backup.pst / Inbox" },
    ],
  );
});

test("active advanced filter count excludes the main query and default controls", () => {
  assert.equal(getActiveFilterCount(snapshot()), 0);
  assert.equal(getActiveFilterCount(snapshot({ query: "calendar" })), 0);
  assert.equal(
    getActiveFilterCount(snapshot({}, { scope: "all_open", workspaceIds: ["workspace-a", "workspace-b"] })),
    0,
  );
  assert.equal(getActiveFilterCount(snapshot({}, { messageSort: "oldest" })), 0);

  for (const [key, value] of [
    ["from", "Adam"],
    ["recipients", "kev@example.com"],
    ["subject", "Calendar"],
    ["body", "Outlook"],
    ["attachment", "pdf"],
    ["hasAttachments", "yes"],
    ["dateFrom", "2026-03-01"],
    ["dateTo", "2026-06-30"],
    ["folderScope", "current"],
  ]) {
    assert.equal(getActiveFilterCount(snapshot({ [key]: value })), 1, `${key} counts once`);
  }

  assert.equal(
    getActiveFilterCount(
      snapshot({
        from: "Adam",
        subject: "Calendar",
        hasAttachments: "yes",
        dateFrom: "2026-03-01",
        dateTo: "2026-06-30",
      }),
    ),
    5,
  );
});

test("chips have stable ids, normalized values, accessible removal labels, and omit defaults", () => {
  assert.deepEqual(buildAppliedFilterChips(snapshot({}, { folderId: null })), []);

  const longValue = `Avery ${"Chen ".repeat(40)}`;
  const chips = buildAppliedFilterChips(
    snapshot({ query: "  calendar  ", from: ` ${longValue} `, folderScope: "all" }),
  );
  assert.deepEqual(chips.map((chip) => chip.id), ["query", "from", "folderScope", "folderSelection"]);
  assert.equal(chips[0].value, "calendar");
  assert.equal(chips[1].value, longValue.trim());
  assert.equal(chips[1].removeLabel, `Remove From filter ${longValue.trim()}`);
  assert.equal(chips[2].text, "Folder scope: All Mail");
});

test("each removable filter clears only itself and preserves the other applied values", () => {
  const original = snapshot({
    query: "calendar",
    from: "Adam",
    recipients: "kev@example.com",
    subject: "Schedule",
    body: "Outlook",
    attachment: "pdf",
    hasAttachments: "yes",
    dateFrom: "2026-03-01",
    dateTo: "2026-06-30",
    folderScope: "current",
  });

  for (const id of [
    "query",
    "from",
    "recipients",
    "subject",
    "body",
    "attachment",
    "dateFrom",
    "dateTo",
  ]) {
    const removed = removeAppliedFilter(original, id);
    assert.equal(removed.changed, true);
    assert.equal(removed.draft[id], "");
    assert.equal(removed.draft.hasAttachments, "yes");
    if (id !== "query") assert.equal(removed.draft.query, "calendar");
  }

  const attachmentsRemoved = removeAppliedFilter(original, "hasAttachments");
  assert.equal(attachmentsRemoved.draft.hasAttachments, "any");
  assert.equal(attachmentsRemoved.draft.subject, "Schedule");

  const folderScopeRemoved = removeAppliedFilter(original, "folderScope");
  assert.equal(folderScopeRemoved.draft.folderScope, "current_subfolders");
  assert.equal(folderScopeRemoved.clearFolderSelection, false);

  const scopeRemoved = removeAppliedFilter(
    snapshot({ query: "calendar" }, { scope: "all_open" }),
    "scope",
  );
  assert.equal(scopeRemoved.scope, "current");
  assert.equal(scopeRemoved.draft.query, "calendar");

  const folderRemoved = removeAppliedFilter(original, "folderSelection");
  assert.equal(folderRemoved.clearFolderSelection, true);
  assert.equal(folderRemoved.draft.folderScope, "current");
});

test("immediate chip removal invalidates an obsolete text debounce token", () => {
  let state = createSearchApplicationState(snapshot({ from: "Adam" }));
  const queued = queueTextSearchSnapshot(state, snapshot({ from: "Avery" }));
  state = queued.state;

  const removal = removeAppliedFilter(queued.state.pending.snapshot, "from");
  const removedSnapshot = replaceAppliedSearchDraft(queued.state.pending.snapshot, removal.draft);
  state = applySearchSnapshotImmediately(state, removedSnapshot).state;

  assert.equal(state.applied.snapshot.from, "");
  assert.equal(commitQueuedSearchSnapshot(state, queued.token).applied, false);
});

test("Clear All clears search constraints, restores sorts, and preserves Scope", () => {
  const original = snapshot(
    {
      query: "calendar",
      from: "Adam",
      recipients: "kev@example.com",
      subject: "Schedule",
      body: "Outlook",
      attachment: "pdf",
      hasAttachments: "yes",
      dateFrom: "2026-03-01",
      dateTo: "2026-06-30",
      folderScope: "current",
    },
    {
      scope: "all_open",
      selectedWorkspaceId: "workspace-a",
      workspaceIds: ["workspace-a", "workspace-b"],
      messageSort: "subject_az",
      conversationSort: "subject",
    },
  );
  const cleared = clearAllSearchDraft(original);

  assert.equal(cleared.changed, true);
  assert.deepEqual(cleared.draft, emptySearchDraft());
  assert.equal(cleared.scope, "all_open");
  assert.equal(cleared.messageSort, "newest");
  assert.equal(cleared.conversationSort, "newest");
  assert.equal(cleared.clearAllOpenFolderSelection, true);

  let state = createSearchApplicationState(original);
  const oldGeneration = state.applied.generation;
  state = applySearchSnapshotImmediately(
    state,
    replaceAppliedSearchDraft(original, cleared.draft),
  ).state;
  assert.equal(isSearchGenerationCurrent(oldGeneration, state.applied.generation), false);
});

test("Clear All is a no-op for defaults and does not treat persisted Scope as clearable", () => {
  assert.equal(canClearAll(snapshot()), false);
  assert.equal(clearAllSearchDraft(snapshot()).changed, false);
  const allOpenDefault = snapshot({}, {
    scope: "all_open",
    selectedWorkspaceId: null,
    workspaceIds: ["workspace-a", "workspace-b"],
  });
  assert.equal(canClearAll(allOpenDefault), false);
  assert.equal(clearAllSearchDraft(allOpenDefault).scope, "all_open");

  const allOpenWithStoredFolderScope = snapshot(
    { folderScope: "current" },
    {
      scope: "all_open",
      selectedWorkspaceId: null,
      workspaceIds: ["workspace-a", "workspace-b"],
    },
  );
  assert.equal(canClearAll(allOpenWithStoredFolderScope), true);
  assert.equal(
    clearAllSearchDraft(allOpenWithStoredFolderScope).draft.folderScope,
    "current_subfolders",
  );
});

test("empty-state classification distinguishes workspace, loading, matches, failure, and mode", () => {
  const base = {
    hasWorkspace: true,
    isSearchActive: true,
    isLoading: false,
    resultCount: 0,
    listMode: "messages",
    scope: "current",
    activeFilterCount: 0,
    errorGeneration: null,
    currentGeneration: 4,
  };

  assert.equal(
    classifySearchEmptyState({ ...base, hasWorkspace: false }).kind,
    "no_workspace",
  );
  assert.equal(
    classifySearchEmptyState({ ...base, isSearchActive: false }).kind,
    "inactive_empty",
  );
  assert.equal(classifySearchEmptyState({ ...base, isLoading: true }).kind, "loading");
  assert.equal(classifySearchEmptyState(base).kind, "no_matches");
  assert.match(classifySearchEmptyState(base).title, /No messages matched/);
  assert.match(
    classifySearchEmptyState({
      ...base,
      listMode: "conversations",
      scope: "all_open",
      activeFilterCount: 2,
    }).detail,
    /All Open PSTs.*2 advanced filters/s,
  );
  assert.match(
    classifySearchEmptyState({ ...base, listMode: "conversations" }).title,
    /No conversations matched/,
  );
  assert.equal(
    classifySearchEmptyState({ ...base, errorGeneration: 4 }).kind,
    "failed",
  );
  assert.equal(
    classifySearchEmptyState({ ...base, errorGeneration: 3 }).kind,
    "no_matches",
    "a stale error does not replace the current empty state",
  );
  assert.equal(classifySearchEmptyState({ ...base, resultCount: 1 }).kind, "none");
});

test("equivalent drafts share a key and each meaningful backend input changes it", () => {
  const base = snapshot({ query: " calendar " });
  assert.equal(appliedSearchSnapshotKey(base), appliedSearchSnapshotKey(snapshot({ query: "calendar" })));

  for (const changed of [
    snapshot({ query: "meeting" }),
    snapshot({ query: "calendar", from: "Adam" }),
    snapshot({ query: "calendar", recipients: "kev@example.com" }),
    snapshot({ query: "calendar", subject: "schedule" }),
    snapshot({ query: "calendar", body: "outlook" }),
    snapshot({ query: "calendar", attachment: "pdf" }),
    snapshot({ query: "calendar", hasAttachments: "yes" }),
    snapshot({ query: "calendar", dateFrom: "2026-01-01" }),
    snapshot({ query: "calendar", dateTo: "2026-12-31" }),
    snapshot({ query: "calendar", folderScope: "all" }),
    snapshot({ query: "calendar" }, { scope: "all_open" }),
    snapshot({ query: "calendar" }, { activeWorkspaceId: "workspace-b" }),
    snapshot({ query: "calendar" }, { workspaceIds: ["workspace-a", "workspace-b"] }),
    snapshot({ query: "calendar" }, { selectedWorkspaceId: "workspace-b" }),
    snapshot({ query: "calendar" }, { useMultiWorkspace: true }),
    snapshot({ query: "calendar" }, { singleWorkspaceId: null }),
    snapshot({ query: "calendar" }, { folderId: 8 }),
    snapshot({ query: "calendar" }, { includeSubfolders: false }),
    snapshot(
      { query: "calendar" },
      {
        conversationScopes: [
          { workspaceId: "workspace-a", folderId: 8, includeSubfolders: false },
        ],
      },
    ),
    snapshot({ query: "calendar" }, { listMode: "conversations" }),
    snapshot({ query: "calendar" }, { messageSort: "oldest" }),
    snapshot({ query: "calendar" }, { conversationSort: "subject" }),
    snapshot({ query: "calendar" }, { sessionGeneration: 2 }),
  ]) {
    assert.notEqual(appliedSearchSnapshotKey(base), appliedSearchSnapshotKey(changed));
  }
});

test("rapid textual changes commit only the final debounced snapshot", () => {
  let state = createSearchApplicationState(snapshot());
  const first = queueTextSearchSnapshot(state, snapshot({ query: "c" }));
  state = first.state;
  const second = queueTextSearchSnapshot(state, snapshot({ query: "ca", from: "A" }));
  state = second.state;
  const final = queueTextSearchSnapshot(
    state,
    snapshot({ query: "calendar", from: "Adam", body: "outlook" }),
  );
  state = final.state;

  assert.equal(commitQueuedSearchSnapshot(state, first.token).applied, false);
  assert.equal(commitQueuedSearchSnapshot(state, second.token).applied, false);
  const committed = commitQueuedSearchSnapshot(state, final.token);
  assert.equal(committed.applied, true);
  assert.equal(committed.state.applied.snapshot.query, "calendar");
  assert.equal(committed.state.applied.snapshot.from, "Adam");
  assert.equal(committed.state.applied.snapshot.body, "outlook");
});

test("non-text changes apply latest text immediately and invalidate an older timer", () => {
  let state = createSearchApplicationState(snapshot());
  const queued = queueTextSearchSnapshot(state, snapshot({ query: "cal" }));
  state = queued.state;

  const immediate = applySearchSnapshotImmediately(
    state,
    snapshot({ query: "calendar", from: "Adam", hasAttachments: "yes" }),
  );
  state = immediate.state;

  assert.equal(immediate.applied, true);
  assert.equal(state.applied.snapshot.query, "calendar");
  assert.equal(state.applied.snapshot.from, "Adam");
  assert.equal(state.applied.snapshot.hasAttachments, "yes");
  assert.equal(commitQueuedSearchSnapshot(state, queued.token).applied, false);
});

test("clearing textual search is debounced and produces an inactive applied snapshot", () => {
  let state = createSearchApplicationState(snapshot({ query: "calendar", subject: "schedule" }));
  const queued = queueTextSearchSnapshot(state, snapshot());
  state = queued.state;
  assert.equal(state.applied.snapshot.query, "calendar");

  const committed = commitQueuedSearchSnapshot(state, queued.token);
  assert.equal(committed.applied, true);
  assert.equal(committed.state.applied.snapshot.query, "");
  assert.equal(isAppliedSearchActive(committed.state.applied.snapshot), false);
});

test("one generation rejects stale rows, counts, errors, loading completions, and pagination", () => {
  let state = createSearchApplicationState(snapshot({ query: "old" }));
  const oldGeneration = state.applied.generation;
  state = applySearchSnapshotImmediately(state, snapshot({ query: "new" })).state;
  const currentGeneration = state.applied.generation;

  for (const responseKind of [
    "message result",
    "conversation result",
    "count",
    "error",
    "loading completion",
    "Load More result",
  ]) {
    assert.equal(
      isSearchGenerationCurrent(oldGeneration, currentGeneration),
      false,
      `${responseKind} must be rejected`,
    );
  }
  assert.equal(isSearchGenerationCurrent(currentGeneration, currentGeneration), true);
});

test("mode changes and workspace closure advance the same generation", () => {
  let state = createSearchApplicationState(snapshot({ query: "calendar" }));
  const messageGeneration = state.applied.generation;
  state = applySearchSnapshotImmediately(
    state,
    snapshot({ query: "calendar" }, { listMode: "conversations" }),
  ).state;
  assert.equal(isSearchGenerationCurrent(messageGeneration, state.applied.generation), false);

  const conversationGeneration = state.applied.generation;
  state = invalidateSearchApplication(state);
  assert.equal(isSearchGenerationCurrent(conversationGeneration, state.applied.generation), false);
});

test("expanded conversation responses require generation, request identity, and expansion", () => {
  assert.equal(isExpandedConversationResponseCurrent(3, 3, 9, 9, true), true);
  assert.equal(isExpandedConversationResponseCurrent(2, 3, 9, 9, true), false);
  assert.equal(isExpandedConversationResponseCurrent(3, 3, 8, 9, true), false);
  assert.equal(isExpandedConversationResponseCurrent(3, 3, 9, 9, false), false);
});

test("duplicate pagination rows are not appended", () => {
  const merged = appendUniqueByKey(
    [{ id: 1 }, { id: 2 }],
    [{ id: 2 }, { id: 3 }, { id: 3 }],
    (item) => String(item.id),
  );
  assert.deepEqual(merged, [{ id: 1 }, { id: 2 }, { id: 3 }]);
});

test("Load More responses are rejected after the applied search changes", () => {
  let state = createSearchApplicationState(snapshot({ query: "calendar" }));
  const loadMoreGeneration = state.applied.generation;
  state = applySearchSnapshotImmediately(state, snapshot({ query: "meeting" })).state;

  assert.equal(
    isSearchGenerationCurrent(loadMoreGeneration, state.applied.generation),
    false,
  );
});

test("Scope survives Start Fresh, session reset, and final workspace close", () => {
  const allOpen = snapshot(
    { query: "calendar", from: "Adam", hasAttachments: "yes" },
    { scope: "all_open", workspaceIds: ["workspace-a", "workspace-b"] },
  );
  let state = createSearchApplicationState(allOpen);

  state = invalidateSearchApplication(state);
  assert.equal(state.applied.snapshot.scope, "all_open", "Start Fresh keeps Scope");

  const restored = replaceAppliedSearchDraft(state.applied.snapshot, emptySearchDraft());
  assert.equal(restored.scope, "all_open", "session restoration keeps Scope");
  assert.equal(restored.query, "");
  assert.equal(restored.from, "");
  assert.equal(restored.hasAttachments, "any");

  const preCloseGeneration = state.applied.generation;
  state = invalidateSearchApplication(state, clearAppliedSearchSnapshot(restored));
  const closed = state.applied.snapshot;
  assert.equal(closed.scope, "all_open", "final workspace close keeps Scope");
  assert.deepEqual(closed.workspaceIds, []);
  assert.equal(closed.activeWorkspaceId, null);
  assert.equal(closed.query, "");
  assert.equal(
    isSearchGenerationCurrent(preCloseGeneration, state.applied.generation),
    false,
    "a result from before the final close cannot restore rows",
  );
});

test("workspace switching preserves textual and structured filters", () => {
  const searchDraft = draft({
    query: "calendar",
    from: "Adam",
    body: "outlook",
    hasAttachments: "yes",
  });
  const first = createAppliedSearchSnapshot(searchDraft, context());
  const second = createAppliedSearchSnapshot(
    searchDraft,
    context({
      activeWorkspaceId: "workspace-b",
      workspaceIds: ["workspace-b"],
      selectedWorkspaceId: "workspace-b",
      singleWorkspaceId: "workspace-b",
      conversationScopes: [
        { workspaceId: "workspace-b", folderId: 4, includeSubfolders: true },
      ],
      folderId: 4,
    }),
  );

  assert.equal(second.query, first.query);
  assert.equal(second.from, first.from);
  assert.equal(second.body, first.body);
  assert.equal(second.hasAttachments, first.hasAttachments);
  assert.notEqual(appliedSearchSnapshotKey(second), appliedSearchSnapshotKey(first));
});

test("App integrates one generation across results, pagination, and expanded conversations", () => {
  assert.match(app, /const searchGenerationRef = useRef\(appliedSearchVersion\.generation\)/);
  assert.match(app, /loadMessagesPage\(false, request\)/);
  assert.match(app, /loadConversationsPage\(false, request\)/);
  assert.match(app, /expandedConversationRequestsRef\.current\.clear\(\)/);
  assert.match(app, /aria-busy=\{isResultsBusy\}/);
  assert.match(app, /if \(restoreStatus\) return;/);
  assert.doesNotMatch(app, /messageSearchRequestIdRef|conversationSearchRequestIdRef/);
  assert.doesNotMatch(app, /const \[debouncedSearch,/);
});

test("App exposes Advanced Search and removable chips with keyboard semantics", () => {
  assert.match(app, /const advancedSearchPanelId = "advanced-search-panel"/);
  assert.match(app, /aria-expanded=\{advancedSearchOpen\}/);
  assert.match(app, /aria-controls=\{advancedSearchPanelId\}/);
  assert.match(app, /id=\{advancedSearchPanelId\}/);
  assert.match(app, /aria-labelledby=\{advancedSearchToggleId\}/);
  assert.match(app, /onKeyDown=\{handleAdvancedSearchKeyDown\}/);
  assert.match(app, /advancedSearchToggleRef\.current\?\.focus\(\)/);
  assert.match(app, /className="filter-chip"/);
  assert.match(app, /aria-label=\{chip\.removeLabel\}/);
  assert.match(app, /aria-busy=\{isResultsBusy\}/);
  assert.match(app, /aria-live="polite"/);
  assert.doesNotMatch(app, />\s*Clear Search\s*</);
  assert.doesNotMatch(app, />\s*Clear Filters\s*</);
});

test("chip and filter-count styles use theme tokens and retain visible focus handling", () => {
  assert.match(styles, /\.filter-chip[\s\S]*min-height:\s*28px/);
  assert.match(styles, /\.filter-chip-text[\s\S]*text-overflow:\s*ellipsis/);
  assert.match(styles, /\.filter-count[\s\S]*var\(--color-badge-background\)/);
  assert.match(styles, /:focus-visible[\s\S]*var\(--color-focus-ring\)/);
});
