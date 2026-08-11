import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const messages = readFileSync(
  new URL("../src/MessageResultsList.tsx", import.meta.url),
  "utf8",
);
const conversations = readFileSync(
  new URL("../src/ConversationResultsList.tsx", import.meta.url),
  "utf8",
);
const navigation = readFileSync(
  new URL("../src/resultNavigation.ts", import.meta.url),
  "utf8",
);
const packageJson = readFileSync(new URL("../package.json", import.meta.url), "utf8");

test("Messages and Conversations use one controlled roving result target", () => {
  assert.match(messages, /tabIndex=\{key === resolvedActiveKey \? 0 : -1\}/);
  assert.match(conversations, /tabIndex=\{entry\.key === resolvedActiveKey \? 0 : -1\}/);
  assert.match(conversations, /tabIndex=\{\w+NavigationKey === resolvedActiveKey \? 0 : -1\}/);
  assert.match(app, /activeNavigationKey=\{resultNavigation\.messages\}/);
  assert.match(app, /activeNavigationKey=\{resultNavigation\.conversations\}/);
  assert.doesNotMatch(messages + conversations, /event\.key !== "Tab"|handle(?:Row|Entry)Tab/);
});

test("keyboard handlers cover arrows, boundaries, and viewport paging", () => {
  for (const key of ["ArrowDown", "ArrowUp", "Home", "End", "PageDown", "PageUp"]) {
    assert.match(messages, new RegExp(`case "${key}"`));
    assert.match(conversations, new RegExp(`case "${key}"`));
  }
  assert.doesNotMatch(messages, /case "Arrow(?:Left|Right)"/);
  assert.match(conversations, /case "ArrowRight"/);
  assert.match(conversations, /case "ArrowLeft"/);
  assert.match(conversations, /firstChildKey\(navigationKey\)/);
  assert.match(conversations, /parentKey \?\? null/);
  assert.match(messages + conversations, /pageIndexForKey\(/);
  assert.match(messages + conversations, /pageKey\(/);
});

test("Enter and Space remain native button activation without duplicate handlers", () => {
  assert.doesNotMatch(messages + conversations, /case "Enter"|case " "|case "Space"/);
  assert.match(messages, /onClick=\{\(\) => void onOpenMessage/);
  assert.match(conversations, /onToggleConversation\(conversation\);/);
  assert.match(conversations, /onClick=\{\(\) => void onOpenMessage/);
});

test("virtual boundaries pin and restore pending focus by stable key", () => {
  assert.match(messages + conversations, /createPendingResultFocus\(/);
  assert.match(messages + conversations, /resolvePendingResultFocus\(/);
  assert.match(messages, /pinnedKeys: \[selectedKey, activeLogicalKey, pendingLogicalKey\]/);
  assert.match(conversations, /pinnedKeys: \[selectedKey, activeLogicalKey, pendingLogicalKey\]/);
  assert.match(messages, /windowResult\.runs\.flatMap/);
  assert.match(conversations, /windowResult\.runs\.flatMap/);
  assert.doesNotMatch(messages + conversations, /Fragment key=\{`(?:conversation-)?items:/);
  assert.match(messages + conversations, /focus\(\{ preventScroll: true \}\)/);
  assert.match(navigation, /private readonly focusablePositionByKey = new Map/);
});

test("result instructions are stable, hidden, and associated once per active list", () => {
  assert.match(app, /id=\{[\s\S]*messageResultsInstructionsId/);
  assert.match(app, /aria-describedby=\{[\s\S]*messageResultsInstructionsId/);
  assert.match(app, /Use Up and Down Arrow keys to move through results/);
  assert.match(app, /Use Right and Left Arrow keys to expand, collapse/);
  assert.match(app, /className="visually-hidden"/);
  assert.match(app, /aria-busy=\{isResultsBusy\}/);
  assert.match(app, /role=\{appliedSearch\.listMode === "messages" \? "list" : "tree"\}/);
});

test("Load More and conversation actions remain explicit reachable buttons", () => {
  assert.match(messages, /<button[\s\S]*onClick=\{onLoadMore\}/);
  assert.match(conversations, /conversationListLoadMoreNavigationKey/);
  assert.match(conversations, /expandedShowEntireNavigationKey/);
  assert.match(conversations, /expandedLoadMoreNavigationKey/);
  assert.match(conversations, /conversationWorkspaceActionNavigationKey/);
  assert.match(conversations, /preserveFocusBeforeAction/);
});

test("navigation remains dependency-free, frontend-only, and structurally safe", () => {
  assert.doesNotMatch(messages + conversations + navigation, /dangerouslySetInnerHTML/);
  assert.doesNotMatch(messages + conversations + navigation, /src-tauri|invoke\(|@tauri-apps\//);
  assert.doesNotMatch(packageJson, /react-window|react-virtualized|tanstack|virtuoso|focus-trap/i);
  assert.doesNotMatch(messages + conversations, /onKeyDown[\s\S]{0,100}Tab/);
});
