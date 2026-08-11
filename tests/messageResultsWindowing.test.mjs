import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const component = readFileSync(
  new URL("../src/MessageResultsList.tsx", import.meta.url),
  "utf8",
);
const model = readFileSync(
  new URL("../src/variableHeightWindow.ts", import.meta.url),
  "utf8",
);
const packageJson = readFileSync(new URL("../package.json", import.meta.url), "utf8");

test("App windows Messages through MessageResultsList independently of Conversations", () => {
  assert.match(app, /<MessageResultsList[\s\S]*messages=\{messages\}/);
  assert.match(app, /<ConversationResultsList[\s\S]*conversations=\{conversations\}/);
  assert.doesNotMatch(app, /:\s*messages\.map\(\(message\) =>/);
  assert.match(app, /role=\{appliedSearch\.listMode === "messages" \? "list" : "tree"\}/);
});

test("all loaded message data remains in memory without a hard cap or eviction", () => {
  assert.match(app, /useState<MessageListItem\[\]>\(\[\]\)/);
  assert.match(app, /messages=\{messages\}/);
  assert.match(component, /messages\.map\(\(message\) => messageResultKey/);
  assert.doesNotMatch(app + component, /setMessages\([^\n]*(?:slice|splice)\(/);
  assert.doesNotMatch(component, /MAX_(?:MESSAGES|ROWS|ITEMS)|evict|retainedPages/i);
});

test("cursor and offset pagination contracts remain in App and Load More stays explicit", () => {
  assert.match(app, /cursorForMessagePage\(/);
  assert.match(app, /paginationMode === "cursor"/);
  assert.match(app, /invoke<MultiMessagePageResult>\("search_messages_multi"/);
  assert.match(app, /offset,\s*searchGeneration:/);
  assert.match(app, /invoke<MessagePageResult>\("list_messages"/);
  assert.match(app, /cursor: requestCursor/);
  assert.match(app, /onLoadMore=\{\(\) => void loadMessagesPage\(true, appliedSearchVersion\)\}/);
  assert.match(component, /:\s*"Load More"\}/);
  assert.doesNotMatch(component, /IntersectionObserver|onScroll[^\n]*load/i);
});

test("windowing preserves busy state and positional list semantics", () => {
  assert.match(app, /aria-busy=\{isResultsBusy\}/);
  assert.match(component, /role="listitem"/);
  assert.match(component, /aria-posinset=\{index \+ 1\}/);
  assert.match(component, /aria-setsize=\{ariaSetSize\}/);
  assert.match(component, /aria-current=\{isSelectedRow \? "true" : undefined\}/);
  assert.match(component, /exactCountStatus === "ready" \? exactTotalCount : -1/);
});

test("selected, active, and pending rows are bounded key-based pins", () => {
  assert.match(component, /pinnedKeys: \[selectedKey, activeLogicalKey, pendingLogicalKey\]/);
  assert.match(component, /ResultNavigationModel/);
  assert.match(component, /createPendingResultFocus/);
  assert.match(component, /tabIndex=\{key === resolvedActiveKey \? 0 : -1\}/);
  assert.match(component, /case "ArrowDown"/);
  assert.match(component, /case "PageUp"/);
  assert.match(component, /scrollTopForIndex\(/);
  assert.match(component, /button\.focus\(\{ preventScroll: true \}\)/);
});

test("ResizeObserver measurement has layout-effect and resize fallbacks", () => {
  assert.match(component, /typeof ResizeObserver === "function"/);
  assert.match(component, /getBoundingClientRect\(\)\.height/);
  assert.match(component, /element\.offsetHeight/);
  assert.match(component, /useLayoutEffect\(\(\) =>/);
  assert.match(component, /window\.addEventListener\("resize", onWindowResize\)/);
  assert.match(component, /captureScrollAnchor\(/);
  assert.match(component, /restoreScrollAnchor\(/);
});

test("spacers are flow-layout, hidden from accessibility, and pointer inert", () => {
  assert.match(component, /className="message-window-spacer"/);
  assert.match(component, /aria-hidden="true"/);
  assert.match(component, /role="presentation"/);
  assert.match(component, /style=\{\{ height: `\$\{run\.height\}px` \}\}/);
  assert.doesNotMatch(component, /position:\s*["']absolute/);
});

test("the implementation is dependency-free, frontend-only, and structurally safe", () => {
  assert.doesNotMatch(component + model, /dangerouslySetInnerHTML/);
  assert.doesNotMatch(component, /src-tauri|invoke\(|@tauri-apps/);
  assert.doesNotMatch(component, /react-window|react-virtualized|tanstack|virtuoso/i);
  assert.doesNotMatch(packageJson, /react-window|react-virtualized|tanstack|virtuoso/i);
  assert.match(model, /class FenwickHeightIndex/);
  assert.match(model, /maximumOrdinaryRows/);
});

test("windowing does not impose a fixed message-row height or truncate row content", () => {
  assert.match(component, /DEFAULT_MESSAGE_ROW_ESTIMATE/);
  assert.match(component, /updateMeasuredHeight/);
  assert.doesNotMatch(component, /className="message-window-item"[\s\S]{0,180}height:/);
  assert.doesNotMatch(component, /lineClamp|WebkitLineClamp/);
});
