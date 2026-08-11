import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const component = readFileSync(
  new URL("../src/ConversationResultsList.tsx", import.meta.url),
  "utf8",
);
const model = readFileSync(
  new URL("../src/conversationResultsModel.ts", import.meta.url),
  "utf8",
);
const windowModel = readFileSync(
  new URL("../src/variableHeightWindow.ts", import.meta.url),
  "utf8",
);
const packageJson = readFileSync(new URL("../package.json", import.meta.url), "utf8");

test("App delegates each result mode to its bounded rendering component", () => {
  assert.match(app, /<ConversationResultsList[\s\S]*conversations=\{conversations\}/);
  assert.match(app, /<MessageResultsList[\s\S]*messages=\{messages\}/);
  assert.doesNotMatch(app, /conversations\.map\(\(conversation\) =>/);
  assert.doesNotMatch(app, /expanded\.items\.map\(\(message\) =>/);
  assert.match(app, /role=\{appliedSearch\.listMode === "messages" \? "list" : "tree"\}/);
});

test("expanded messages and state rows are independent logical entries", () => {
  assert.match(model, /kind: "conversation-header"/);
  assert.match(model, /kind: "expanded-message"/);
  assert.match(model, /kind: "expanded-loading"/);
  assert.match(model, /kind: "expanded-error"/);
  assert.match(model, /kind: "expanded-actions"/);
  assert.match(model, /kind: "conversation-list-load-more"/);
  assert.match(model, /expanded\.items\.forEach\(\(message, expandedIndex\) =>/);
  assert.doesNotMatch(component, /expanded\.items\.map/);
});

test("all loaded conversation and expanded-message data remains retained", () => {
  assert.match(app, /useState<ConversationSummary\[\]>\(\[\]\)/);
  assert.match(app, /Record<string, ExpandedConversationState>/);
  assert.match(app, /conversations=\{conversations\}/);
  assert.match(app, /expandedConversations=\{expandedConversations\}/);
  assert.doesNotMatch(app + component + model, /setConversations\([^\n]*(?:slice|splice)\(/);
  assert.doesNotMatch(component + model, /evict|retainedPages|MAX_CONVERSATIONS/i);
});

test("conversation offsets, message cursors, counts, and cancellation remain in App", () => {
  assert.match(app, /invoke<ConversationPageResult>\("list_conversations"/);
  assert.match(app, /offset,\s*searchGeneration:/);
  assert.match(app, /invoke<ConversationCountResult>\("count_conversations"/);
  assert.match(app, /invoke<ConversationMessagesResult>\("get_conversation_messages"/);
  assert.match(app, /offset: append \? current\?\.items\.length \?\? 0 : 0/);
  assert.match(app, /cursorForMessagePage\(/);
  assert.match(app, /cancelBackendSearch\(cancellation\)/);
  assert.match(app, /isExpandedConversationResponseCurrent\(/);
});

test("top-level and expanded Load More controls remain explicit", () => {
  assert.match(component, /onLoadMoreConversations/);
  assert.match(component, />\s*Load More Messages\s*</);
  assert.match(component, />\s*Show Entire Conversation\s*</);
  assert.doesNotMatch(component, /IntersectionObserver|onScroll[^\n]*load/i);
});

test("conversation measurement uses the shared variable-height engine and fallbacks", () => {
  assert.match(component, /new VariableHeightWindow\(/);
  assert.match(component, /syncItems\(definitions\)/);
  assert.match(component, /typeof ResizeObserver === "function"/);
  assert.match(component, /getBoundingClientRect\(\)\.height/);
  assert.match(component, /element\.offsetHeight/);
  assert.match(component, /window\.addEventListener\("resize", onWindowResize\)/);
  assert.match(component, /captureScrollAnchor\(/);
  assert.match(component, /restoreScrollAnchor\(/);
  assert.match(windowModel, /class FenwickHeightIndex/);
});

test("selected, active, and pending logical entries are bounded pins", () => {
  assert.match(component, /pinnedKeys: \[selectedKey, activeLogicalKey, pendingLogicalKey\]/);
  assert.match(component, /expandedMessageEntryKey\(selectedWorkspaceId, selectedMessageId\)/);
  assert.match(component, /ResultNavigationModel/);
  assert.match(component, /buildConversationNavigationEntries/);
  assert.match(component, /tabIndex=\{entry\.key === resolvedActiveKey \? 0 : -1\}/);
  assert.match(component, /button\.focus\(\{ preventScroll: true \}\)/);
});

test("tree semantics distinguish top-level conversations from expanded messages", () => {
  assert.match(component, /role="treeitem"/);
  assert.match(component, /aria-level=\{1\}/);
  assert.match(component, /aria-level=\{2\}/);
  assert.match(component, /aria-posinset=\{entry\.conversationPosition\}/);
  assert.match(component, /aria-posinset=\{entry\.expandedPosition\}/);
  assert.match(component, /aria-expanded=\{entry\.expanded\}/);
  assert.match(component, /aria-current=\{isSelectedRow \? "true" : undefined\}/);
});

test("spacers are flow-layout, hidden, finite-model output, and pointer inert", () => {
  assert.match(component, /className="conversation-window-spacer"/);
  assert.match(component, /aria-hidden="true"/);
  assert.match(component, /role="presentation"/);
  assert.match(component, /style=\{\{ height: `\$\{run\.height\}px` \}\}/);
  assert.doesNotMatch(component, /position:\s*["']absolute/);
});

test("implementation stays frontend-only, dependency-free, and structurally safe", () => {
  assert.doesNotMatch(component + model, /dangerouslySetInnerHTML/);
  assert.doesNotMatch(component + model, /src-tauri|invoke\(|@tauri-apps\//);
  assert.doesNotMatch(component + model, /react-window|react-virtualized|tanstack|virtuoso/i);
  assert.doesNotMatch(packageJson, /react-window|react-virtualized|tanstack|virtuoso/i);
  assert.doesNotMatch(component, /lineClamp|WebkitLineClamp|content-visibility/);
});
