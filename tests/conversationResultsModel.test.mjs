import test from "node:test";
import assert from "node:assert/strict";

import {
  buildConversationNavigationEntries,
  conversationHeaderEntryKey,
  conversationStateKey,
  expandedActionsEntryKey,
  expandedMessageEntryKey,
  flattenConversationResults,
} from "../src/conversationResultsModel.ts";

function conversation(id, workspaceId = "workspace-a") {
  return {
    conversationId: `conversation-${id}`,
    conversationRootId: id,
    subject: `Synthetic subject ${id}`,
    latestSender: `Synthetic sender ${id}`,
    participants: [`Participant ${id}`],
    latestDate: "2026-01-01T00:00:00.000Z",
    snippet: `Synthetic snippet ${id}`,
    matchingMessageCount: 2,
    totalMessageCount: 3,
    hasAttachments: false,
    latestMessageId: id * 10,
    assignmentMethod: "synthetic",
    workspaceId,
    pstDisplayName: `Workspace ${workspaceId}`,
    workspacePath: `/synthetic/${workspaceId}`,
  };
}

function message(id, workspaceId = "workspace-a") {
  return {
    id,
    folderId: 1,
    folderPath: "Synthetic/Inbox",
    folderName: "Inbox",
    subject: `Synthetic message ${id}`,
    sender: `Synthetic sender ${id}`,
    recipients: "Synthetic recipient",
    date: "2026-01-01T00:00:00.000Z",
    snippet: `Synthetic message snippet ${id}`,
    hasAttachments: false,
    attachmentCount: 0,
    workspaceId,
    matchesScope: true,
  };
}

function expanded(items, overrides = {}) {
  return {
    items,
    matchingMessageCount: items.length,
    totalMessageCount: items.length,
    showingEntireConversation: false,
    loading: false,
    error: null,
    ...overrides,
  };
}

test("empty conversation state produces no logical entries", () => {
  assert.deepEqual(
    flattenConversationResults({ conversations: [], expandedConversations: {} }),
    [],
  );
});

test("collapsed conversation produces one stable header entry", () => {
  const item = conversation(1);
  const entries = flattenConversationResults({
    conversations: [item],
    expandedConversations: {},
  });
  assert.equal(entries.length, 1);
  assert.equal(entries[0].kind, "conversation-header");
  assert.equal(entries[0].key, conversationHeaderEntryKey(item.workspaceId, item.conversationId));
  assert.equal(entries[0].parentConversationKey, conversationStateKey(item.workspaceId, item.conversationId));
  assert.equal(entries[0].logicalPosition, 1);
});

test("one expanded conversation flattens every message independently", () => {
  const item = conversation(1);
  const parent = conversationStateKey(item.workspaceId, item.conversationId);
  const entries = flattenConversationResults({
    conversations: [item],
    expandedConversations: {
      [parent]: expanded([message(10), message(11)]),
    },
  });
  assert.deepEqual(entries.map((entry) => entry.kind), [
    "conversation-header",
    "expanded-message",
    "expanded-message",
  ]);
  assert.equal(entries[1].key, expandedMessageEntryKey(item.workspaceId, 10));
  assert.equal(entries[2].key, expandedMessageEntryKey(item.workspaceId, 11));
});

test("several expanded conversations retain deterministic parent relationships", () => {
  const first = conversation(1, "workspace-a");
  const second = conversation(2, "workspace-b");
  const firstParent = conversationStateKey(first.workspaceId, first.conversationId);
  const secondParent = conversationStateKey(second.workspaceId, second.conversationId);
  const entries = flattenConversationResults({
    conversations: [first, second],
    expandedConversations: {
      [firstParent]: expanded([message(10, "workspace-a")]),
      [secondParent]: expanded([message(10, "workspace-b")]),
    },
  });
  assert.deepEqual(entries.map((entry) => entry.parentConversationKey), [
    firstParent,
    firstParent,
    secondParent,
    secondParent,
  ]);
  assert.equal(new Set(entries.map((entry) => entry.key)).size, entries.length);
});

test("loading, error, expanded actions, and top-level Load More keep their order", () => {
  const item = conversation(1);
  const parent = conversationStateKey(item.workspaceId, item.conversationId);
  const entries = flattenConversationResults({
    conversations: [item],
    expandedConversations: {
      [parent]: expanded([message(10)], {
        matchingMessageCount: 3,
        totalMessageCount: 5,
        loading: true,
        error: "Synthetic failure",
      }),
    },
    hasMoreConversations: true,
    topLevelLoadMoreDisabled: true,
  });
  assert.deepEqual(entries.map((entry) => entry.kind), [
    "conversation-header",
    "expanded-message",
    "expanded-loading",
    "expanded-error",
    "expanded-actions",
    "conversation-list-load-more",
  ]);
  const actions = entries[4];
  assert.equal(actions.key, expandedActionsEntryKey(parent));
  assert.equal(actions.showEntireAvailable, true);
  assert.equal(actions.loadMoreAvailable, true);
  assert.equal(actions.focusable, false);
  assert.equal(entries[5].focusable, false);
});

test("workspace warning is stable, measured, and scoped outside conversations", () => {
  const entries = flattenConversationResults({
    conversations: [conversation(1)],
    expandedConversations: {},
    workspaceIssues: [
      {
        workspaceId: "workspace-a",
        pstDisplayName: "Synthetic workspace",
        workspacePath: "/synthetic/workspace-a",
        canReindex: true,
      },
    ],
  });
  assert.equal(entries[0].kind, "conversation-workspace-warning");
  assert.equal(entries[0].parentConversationKey, null);
  assert.equal(entries[0].focusable, true);
  assert.ok(entries[0].estimatedHeight > 0);
  assert.equal(entries[1].kind, "conversation-header");
});

test("collapse removes children and reopening restores deterministic keys", () => {
  const item = conversation(1);
  const parent = conversationStateKey(item.workspaceId, item.conversationId);
  const state = { [parent]: expanded([message(10), message(11)]) };
  const first = flattenConversationResults({ conversations: [item], expandedConversations: state });
  const collapsed = flattenConversationResults({
    conversations: [item],
    expandedConversations: {},
  });
  const reopened = flattenConversationResults({ conversations: [item], expandedConversations: state });
  assert.equal(collapsed.length, 1);
  assert.deepEqual(reopened.map((entry) => entry.key), first.map((entry) => entry.key));
});

test("expanded page append preserves earlier keys and inserts the new message before actions", () => {
  const item = conversation(1);
  const parent = conversationStateKey(item.workspaceId, item.conversationId);
  const initial = flattenConversationResults({
    conversations: [item],
    expandedConversations: {
      [parent]: expanded([message(10)], { matchingMessageCount: 3, totalMessageCount: 3 }),
    },
  });
  const appended = flattenConversationResults({
    conversations: [item],
    expandedConversations: {
      [parent]: expanded([message(10), message(11)], {
        matchingMessageCount: 3,
        totalMessageCount: 3,
      }),
    },
  });
  assert.equal(appended[0].key, initial[0].key);
  assert.equal(appended[1].key, initial[1].key);
  assert.equal(appended[2].key, expandedMessageEntryKey(item.workspaceId, 11));
  assert.equal(appended.at(-1).kind, "expanded-actions");
});

test("selected expanded-message identity is workspace scoped", () => {
  assert.notEqual(
    expandedMessageEntryKey("workspace-a", 10),
    expandedMessageEntryKey("workspace-b", 10),
  );
});

test("duplicate source identities never produce duplicate logical keys", () => {
  const duplicate = conversation(1);
  const entries = flattenConversationResults({
    conversations: [duplicate, duplicate],
    expandedConversations: {},
  });
  assert.equal(entries.length, 1);
  assert.equal(new Set(entries.map((entry) => entry.key)).size, entries.length);
});

test("navigation entries skip status rows and preserve independent shared actions", () => {
  const item = conversation(1);
  const parent = conversationStateKey(item.workspaceId, item.conversationId);
  const entries = flattenConversationResults({
    conversations: [item, conversation(2)],
    expandedConversations: {
      [parent]: expanded([], {
        matchingMessageCount: 1,
        totalMessageCount: 2,
        error: "Synthetic failure",
      }),
    },
  });
  assert.deepEqual(entries.map((entry) => entry.kind), [
    "conversation-header",
    "expanded-error",
    "expanded-actions",
    "conversation-header",
  ]);
  const navigationEntries = buildConversationNavigationEntries(entries);
  assert.deepEqual(navigationEntries.map((entry) => entry.kind), [
    "conversation-header",
    "expanded-show-entire",
    "expanded-load-more",
    "conversation-header",
  ]);
  assert.equal(navigationEntries[1].logicalKey, entries[2].key);
  assert.equal(navigationEntries[2].logicalKey, entries[2].key);
  assert.equal(navigationEntries[1].parentKey, entries[0].key);
});

test("mode or generation reset can replace a populated model with an empty result", () => {
  const populated = flattenConversationResults({
    conversations: [conversation(1)],
    expandedConversations: {},
  });
  const reset = flattenConversationResults({ conversations: [], expandedConversations: {} });
  assert.equal(populated.length, 1);
  assert.deepEqual(reset, []);
});

test("100,000 synthetic conversations retain stable positions and unique keys", () => {
  const conversations = Array.from({ length: 100_000 }, (_, index) => conversation(index + 1));
  const entries = flattenConversationResults({ conversations, expandedConversations: {} });
  assert.equal(entries.length, 100_000);
  assert.equal(entries[0].logicalPosition, 1);
  assert.equal(entries[99_999].logicalPosition, 100_000);
  assert.equal(entries[99_999].conversationPosition, 100_000);
  assert.equal(new Set(entries.map((entry) => entry.key)).size, 100_000);
});
