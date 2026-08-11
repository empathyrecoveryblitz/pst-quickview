import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  createSearchExactCountState,
  createSearchOperationIdentity,
  settleSearchExactCount,
} from "../src/searchRequest.ts";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const messageResults = readFileSync(
  new URL("../src/MessageResultsList.tsx", import.meta.url),
  "utf8",
);
const conversationResults = readFileSync(
  new URL("../src/ConversationResultsList.tsx", import.meta.url),
  "utf8",
);

function functionSource(name, nextName) {
  const start = app.indexOf(`async function ${name}`);
  const end = app.indexOf(`async function ${nextName}`, start + 1);
  assert.notEqual(start, -1, `${name} should exist`);
  assert.notEqual(end, -1, `${nextName} should follow ${name}`);
  return app.slice(start, end);
}

test("page and count operations use distinct opaque lanes in one generation", () => {
  const page = createSearchOperationIdentity(12, "message-page", 4);
  const count = createSearchOperationIdentity(12, "message-count", 4);
  const conversationPage = createSearchOperationIdentity(12, "conversation-page", 5);
  const conversationCount = createSearchOperationIdentity(12, "conversation-count", 5);

  assert.equal(page.generation, count.generation);
  assert.notEqual(page.operationId, count.operationId);
  assert.notEqual(conversationPage.operationId, conversationCount.operationId);
  assert.doesNotMatch(JSON.stringify([page, count]), /query|filter|workspace|path/);
});

test("current exact count settles independently while stale count completion is ignored", () => {
  const pending = createSearchExactCountState(8, "pending");
  const ready = settleSearchExactCount(pending, 8, 8, "ready");
  assert.deepEqual(ready, { generation: 8, status: "ready" });

  const newer = createSearchExactCountState(9, "pending");
  assert.strictEqual(settleSearchExactCount(newer, 9, 8, "ready"), newer);
  assert.deepEqual(settleSearchExactCount(newer, 9, 9, "unavailable"), {
    generation: 9,
    status: "unavailable",
  });
});

test("first page and exact count launch independently for both list modes", () => {
  assert.match(
    app,
    /void loadConversationsPage\(false, request\);\s*void loadConversationCount\(request\);/,
  );
  assert.match(app, /void loadMessagesPage\(false, request\);\s*void loadMessageCount\(request\);/);
  assert.match(app, /createSearchOperationIdentity\(request\.generation, "message-count"/);
  assert.match(app, /createSearchOperationIdentity\(request\.generation, "conversation-count"/);
});

test("message page publishes rows and has-more without waiting for exact count", () => {
  const pageSource = functionSource("loadMessagesPage", "loadMessageCount");
  assert.match(pageSource, /setMessageHasMore\(result\.hasMore\)/);
  assert.match(pageSource, /setMessages\(\(current\) =>/);
  assert.match(pageSource, /setSearchLoading\(null\)/);
  assert.doesNotMatch(pageSource, /count_messages/);
  assert.doesNotMatch(pageSource, /setMessageTotalCount\(result\.totalCount\)/);
});

test("conversation page publishes rows and has-more without waiting for exact count", () => {
  const pageSource = functionSource("loadConversationsPage", "loadConversationCount");
  assert.match(pageSource, /setConversationHasMore\(result\.hasMore\)/);
  assert.match(pageSource, /setConversations\(\(current\) =>/);
  assert.match(pageSource, /setSearchLoading\(null\)/);
  assert.doesNotMatch(pageSource, /count_conversations/);
  assert.doesNotMatch(pageSource, /setConversationTotalCount\(result\.totalCount\)/);
});

test("count failure is isolated from rows and the page error channel", () => {
  const messageCount = functionSource("loadMessageCount", "loadConversationsPage");
  const conversationCount = functionSource("loadConversationCount", "loadConversationMessages");
  for (const source of [messageCount, conversationCount]) {
    assert.match(source, /"unavailable"/);
    assert.doesNotMatch(source, /setSearchError/);
    assert.doesNotMatch(source, /setMessages\(\[\]\)|setConversations\(\[\]\)/);
  }
});

test("current count cancellation clears pending state without surfacing a search error", () => {
  const messageCount = functionSource("loadMessageCount", "loadConversationsPage");
  const conversationCount = functionSource("loadConversationCount", "loadConversationMessages");
  assert.match(
    messageCount,
    /isSearchCancellationError\(err\)[\s\S]*createSearchExactCountState\(request\.generation\)/,
  );
  assert.match(
    conversationCount,
    /isSearchCancellationError\(err\)[\s\S]*createSearchExactCountState\(request\.generation\)/,
  );
});

test("Load More is page-derived and does not launch another exact count", () => {
  assert.match(app, /const hasMoreMessages = messageHasMore;/);
  assert.match(app, /const hasMoreConversations = conversationHasMore;/);
  assert.match(app, /onLoadMore=\{\(\) => void loadMessagesPage\(true, appliedSearchVersion\)\}/);
  assert.match(messageResults, /onClick=\{onLoadMore\}/);
  assert.match(
    app,
    /onLoadMoreConversations=\{\(\) =>\s*void loadConversationsPage\(true, appliedSearchVersion\)\s*\}/,
  );
  assert.match(conversationResults, /onLoadMoreConversations\(\);/);
  assert.doesNotMatch(app, /loadMessageCount\([^\n]+true/);
  assert.doesNotMatch(app, /loadConversationCount\([^\n]+true/);
});

test("count wording never fabricates an exact total", () => {
  assert.match(app, /counting total\.\.\./);
  assert.match(app, /counting totals\.\.\./);
  assert.match(app, /exact count unavailable/);
  assert.match(app, /activeMessageCountStatus === "ready"/);
  assert.match(app, /activeConversationCountStatus === "ready"/);
  assert.match(app, /activeMessageCountStatus === "pending"/);
  assert.match(app, /activeConversationCountStatus === "pending"/);
  assert.match(app, /`Message results\$\{resultScopeSuffix\}`/);
  assert.match(app, /`Conversation results\$\{resultScopeSuffix\}`/);
});

test("page readiness owns busy state and count changes are not repeatedly announced", () => {
  assert.match(app, /const isSearching =\s*searchLoading\?\.generation/);
  assert.doesNotMatch(app, /const isSearching =[\s\S]{0,180}CountState/);
  assert.match(app, /<strong>\{isSearching \? "Searching\.\.\." : resultSummaryText\}<\/strong>/);
  assert.match(app, /className="visually-hidden" role="status" aria-live="polite"/);
  assert.match(app, /"Message results ready\."/);
  assert.match(app, /"Conversation results ready\."/);
});

test("generation reset clears page and count metadata together", () => {
  assert.match(app, /messageCountRequestRef\.current = null;/);
  assert.match(app, /conversationCountRequestRef\.current = null;/);
  assert.match(app, /setMessageHasMore\(false\)/);
  assert.match(app, /setConversationHasMore\(false\)/);
  assert.match(app, /createSearchExactCountState\(next\.applied\.generation\)/);
});
