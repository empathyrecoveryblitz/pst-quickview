import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  closeOpenPopover,
  createDismissiblePopoverController,
  toggleOpenPopover,
} from "../src/popoverBehavior.ts";

const appSource = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const stylesSource = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");

function elementSource(source, marker, tagName) {
  const markerIndex = source.indexOf(marker);
  assert.notEqual(markerIndex, -1, `missing source marker: ${marker}`);
  const start = source.lastIndexOf(`<${tagName}`, markerIndex);
  const end = source.indexOf(`</${tagName}>`, markerIndex);
  assert.notEqual(start, -1, `missing opening <${tagName}> for ${marker}`);
  assert.notEqual(end, -1, `missing closing </${tagName}> for ${marker}`);
  return source.slice(start, end + tagName.length + 3);
}

function cssRule(source, selector) {
  const selectorIndex = source.indexOf(selector);
  assert.notEqual(selectorIndex, -1, `missing CSS selector: ${selector}`);
  const start = source.indexOf("{", selectorIndex);
  const end = source.indexOf("}", start);
  assert.notEqual(start, -1, `missing CSS rule start: ${selector}`);
  assert.notEqual(end, -1, `missing CSS rule end: ${selector}`);
  return source.slice(start + 1, end);
}

class FakeEventTarget {
  listeners = new Map();
  additions = [];
  removals = [];

  addEventListener(type, listener, options) {
    const entries = this.listeners.get(type) ?? [];
    entries.push({ listener, options });
    this.listeners.set(type, entries);
    this.additions.push({ type, listener, options });
  }

  removeEventListener(type, listener, options) {
    const entries = this.listeners.get(type) ?? [];
    const index = entries.findIndex(
      (entry) => entry.listener === listener && entry.options === options,
    );
    if (index >= 0) entries.splice(index, 1);
    this.listeners.set(type, entries);
    this.removals.push({ type, listener, options });
  }

  dispatch(type, event = {}) {
    event.type = type;
    for (const { listener } of [...(this.listeners.get(type) ?? [])]) {
      listener(event);
    }
    return event;
  }

  listenerCount(type) {
    return (this.listeners.get(type) ?? []).length;
  }
}

class FakeWindow extends FakeEventTarget {
  frames = [];

  requestAnimationFrame(callback) {
    this.frames.push(callback);
    return this.frames.length;
  }

  flushFrames() {
    const frames = this.frames.splice(0);
    for (const callback of frames) callback(0);
  }
}

function element(name, children = []) {
  const descendants = new Set(children);
  return {
    name,
    isConnected: true,
    focusCalls: [],
    contains(target) {
      return descendants.has(target);
    },
    focus(options) {
      this.focusCalls.push(options);
    },
  };
}

function keyboardEvent(key, target = null) {
  return {
    key,
    target,
    defaultPrevented: false,
    propagationStopped: false,
    preventDefault() {
      this.defaultPrevented = true;
    },
    stopPropagation() {
      this.propagationStopped = true;
    },
  };
}

function pointerEvent(target) {
  return {
    target,
    defaultPrevented: false,
    propagationStopped: false,
    preventDefault() {
      this.defaultPrevented = true;
    },
    stopPropagation() {
      this.propagationStopped = true;
    },
  };
}

function fixture(overrides = {}) {
  const documentTarget = new FakeEventTarget();
  const windowTarget = new FakeWindow();
  const triggerChild = { name: "trigger-child" };
  const panelChild = { name: "panel-child" };
  const trigger = element("trigger", [triggerChild]);
  const panel = element("panel", [panelChild]);
  const dismissals = [];
  const controller = createDismissiblePopoverController({
    documentTarget,
    windowTarget,
    trigger,
    panel,
    onDismiss: (reason) => dismissals.push(reason),
    ...overrides,
  });
  return {
    controller,
    dismissals,
    documentTarget,
    panel,
    panelChild,
    trigger,
    triggerChild,
    windowTarget,
  };
}

test("toggle helper closes the same popover and atomically replaces another", () => {
  assert.equal(toggleOpenPopover(null, "layout"), "layout");
  assert.equal(toggleOpenPopover("layout", "layout"), null);
  assert.equal(toggleOpenPopover("layout", "workspace"), "workspace");
  assert.equal(closeOpenPopover("workspace", "layout"), "workspace");
  assert.equal(closeOpenPopover("workspace", "workspace"), null);
});

test("inside trigger and panel pointer presses do not dismiss", () => {
  const f = fixture();
  f.controller.attach();

  for (const target of [f.trigger, f.triggerChild, f.panel, f.panelChild]) {
    const event = pointerEvent(target);
    f.documentTarget.dispatch("pointerdown", event);
    assert.equal(event.defaultPrevented, false);
    assert.equal(event.propagationStopped, false);
  }

  assert.deepEqual(f.dismissals, []);
  assert.equal(f.controller.isAttached(), true);
});

test("outside pointer dismisses without preventing default, stopping propagation, or moving focus", () => {
  const f = fixture();
  f.controller.attach();
  const event = pointerEvent({ name: "outside" });

  f.documentTarget.dispatch("pointerdown", event);

  assert.deepEqual(f.dismissals, ["outside_pointer"]);
  assert.equal(event.defaultPrevented, false);
  assert.equal(event.propagationStopped, false);
  assert.deepEqual(f.trigger.focusCalls, []);
  assert.equal(f.controller.isAttached(), false);
});

test("Escape dismisses, consumes only Escape, and restores connected trigger focus on a frame", () => {
  const f = fixture();
  f.controller.attach();
  const event = keyboardEvent("Escape", f.panelChild);

  f.documentTarget.dispatch("keydown", event);

  assert.deepEqual(f.dismissals, ["escape"]);
  assert.equal(event.defaultPrevented, true);
  assert.equal(event.propagationStopped, true);
  assert.deepEqual(f.trigger.focusCalls, []);
  f.windowTarget.flushFrames();
  assert.deepEqual(f.trigger.focusCalls, [{ preventScroll: true }]);
});

test("Tab and other keys remain untouched and do not create a focus trap", () => {
  const f = fixture();
  f.controller.attach();

  for (const key of ["Tab", "Enter", " ", "ArrowDown"]) {
    const event = keyboardEvent(key, f.panelChild);
    f.documentTarget.dispatch("keydown", event);
    assert.equal(event.defaultPrevented, false, `${key} default must remain available`);
    assert.equal(event.propagationStopped, false, `${key} propagation must remain available`);
  }

  assert.deepEqual(f.dismissals, []);
  assert.equal(f.controller.isAttached(), true);
});

test("removed trigger makes scheduled Escape focus restoration a safe no-op", () => {
  const f = fixture();
  f.controller.attach();
  f.documentTarget.dispatch("keydown", keyboardEvent("Escape", f.panel));
  f.trigger.isConnected = false;

  f.windowTarget.flushFrames();

  assert.deepEqual(f.dismissals, ["escape"]);
  assert.deepEqual(f.trigger.focusCalls, []);
});

test("attachment and cleanup are idempotent with no duplicate listeners", () => {
  const f = fixture();
  const firstCleanup = f.controller.attach();
  const secondCleanup = f.controller.attach();

  assert.strictEqual(firstCleanup, secondCleanup);
  assert.equal(f.documentTarget.listenerCount("pointerdown"), 1);
  assert.equal(f.documentTarget.listenerCount("keydown"), 1);
  assert.equal(f.windowTarget.listenerCount("blur"), 1);
  assert.equal(f.windowTarget.listenerCount("resize"), 1);

  firstCleanup();
  secondCleanup();
  f.controller.detach();
  assert.equal(f.documentTarget.listenerCount("pointerdown"), 0);
  assert.equal(f.documentTarget.listenerCount("keydown"), 0);
  assert.equal(f.windowTarget.listenerCount("blur"), 0);
  assert.equal(f.windowTarget.listenerCount("resize"), 0);
  assert.equal(f.documentTarget.removals.length, 2);
  assert.equal(f.windowTarget.removals.length, 2);
});

test("window blur and resize dismiss safely without focus restoration", () => {
  for (const [eventName, reason] of [
    ["blur", "window_blur"],
    ["resize", "window_resize"],
  ]) {
    const f = fixture();
    f.controller.attach();
    f.windowTarget.dispatch(eventName, { target: f.windowTarget });
    f.windowTarget.flushFrames();

    assert.deepEqual(f.dismissals, [reason]);
    assert.deepEqual(f.trigger.focusCalls, []);
    assert.equal(f.controller.isAttached(), false);
  }
});

test("a dismissal is delivered once even if later lifecycle events arrive", () => {
  const f = fixture();
  f.controller.attach();
  f.documentTarget.dispatch("pointerdown", pointerEvent({ name: "outside" }));
  f.windowTarget.dispatch("blur", {});
  f.windowTarget.dispatch("resize", {});
  assert.deepEqual(f.dismissals, ["outside_pointer"]);
});

test("primitive does not register click, drag, selection, or text-input listeners", () => {
  const f = fixture();
  f.controller.attach();
  const types = [...f.documentTarget.additions, ...f.windowTarget.additions].map(
    ({ type }) => type,
  );
  assert.deepEqual(types.sort(), ["blur", "keydown", "pointerdown", "resize"]);
  for (const forbidden of [
    "click",
    "mousedown",
    "dragstart",
    "dragover",
    "drop",
    "selectstart",
    "selectionchange",
    "input",
  ]) {
    assert.equal(types.includes(forbidden), false);
  }
});

test("source remains dependency-free and contains no global click or focus-trap behavior", () => {
  const source = readFileSync(new URL("../src/popoverBehavior.ts", import.meta.url), "utf8");
  assert.doesNotMatch(source, /^import\s/m);
  assert.doesNotMatch(source, /addEventListener\("click"/);
  assert.doesNotMatch(source, /dragstart|dragover|selectstart|selectionchange/);
  assert.doesNotMatch(source, /querySelectorAll|firstFocusable|lastFocusable/);
});

test("App uses one shared coordinator for every custom popover and clears stale state", () => {
  const customPopoverType = appSource.match(/type CustomPopoverId =([\s\S]*?);/)?.[1] ?? "";
  const ids = [
    "advanced_search",
    "recent",
    "workspace",
    "layout",
    "workspace_details",
    "import_progress",
    "help",
  ];

  for (const id of ids) {
    assert.match(customPopoverType, new RegExp(`\\|?\\s*"${id}"`), `${id} must be coordinated`);
    assert.match(appSource, new RegExp(`case "${id}":`), `${id} must resolve through one panel map`);
    assert.match(
      appSource,
      new RegExp(`togglePopover\\("${id}", event\\.currentTarget\\)`),
      `${id} must open through the shared toggle`,
    );
  }

  assert.equal(
    (appSource.match(/createDismissiblePopoverController\(\{/g) ?? []).length,
    1,
    "App must create exactly one controller for the currently open custom popover",
  );
  assert.match(
    appSource,
    /function togglePopover[\s\S]*?toggleOpenPopover\(openPopoverId, id\)[\s\S]*?activePopoverTriggerRef\.current = trigger;[\s\S]*?setOpenPopoverId\(next\);/,
  );
  assert.match(appSource, /return controller\.attach\(\);/);
  assert.match(
    appSource,
    /useEffect\(\(\) => \{\s*if \(openPopoverId === "help"\) clearHelpState\(\);\s*activePopoverTriggerRef\.current = null;\s*setOpenPopoverId\(null\);\s*\}, \[workspace\?\.id\]\);/,
    "workspace closure or replacement must clear the open popover and trigger",
  );
  assert.match(
    appSource,
    /if \(openPopoverId !== "import_progress"\) return;\s*activePopoverTriggerRef\.current = null;\s*setOpenPopoverId\(null\);\s*\}, \[progress\?\.operationId\]\);/,
    "an import operation change must not retain a stale details panel",
  );
});

test("custom popover triggers and panels have stable valid dialog associations", () => {
  const bindings = [
    ["advancedSearchPanelId", "div"],
    ["recentPopoverPanelId", "div"],
    ["workspacePopoverPanelId", "div"],
    ["layoutPopoverPanelId", "div"],
    ["workspaceDetailsPanelId", "div"],
    ["importProgressDetailsPanelId", "div"],
    ["helpDialogId", "section"],
  ];
  const stableIds = [];

  for (const [binding, panelTag] of bindings) {
    const declaration = appSource.match(new RegExp(`const ${binding} = "([^"]+)";`));
    assert.ok(declaration, `${binding} must be a stable string constant`);
    stableIds.push(declaration[1]);

    const trigger = elementSource(appSource, `aria-controls={${binding}}`, "button");
    assert.match(trigger, /aria-expanded=\{/);
    assert.match(trigger, /aria-haspopup="dialog"/);
    assert.doesNotMatch(trigger, /role="menuitem"/);

    const panel = elementSource(appSource, `id={${binding}}`, panelTag);
    assert.match(panel, /role="dialog"/);
    assert.match(panel, /aria-modal="false"/);
    assert.match(panel, /aria-(?:label|labelledby)=/);
    assert.doesNotMatch(panel, /role="menu"/);
  }

  assert.equal(new Set(stableIds).size, bindings.length, "panel IDs must not collide");

  for (const binding of ["workspaceDetailsPanelId", "importProgressDetailsPanelId"]) {
    const panel = elementSource(appSource, `id={${binding}}`, "div");
    assert.match(panel, /<dl>/);
    assert.doesNotMatch(panel, /<dl[^>]*role=/);
  }
});

test("Layout and hierarchy choices apply once, close immediately, and retain native selects", () => {
  for (const [valueMarker, setter] of [
    ["value={listMode}", "setListMode"],
    ["value={layoutMode}", "setLayoutMode"],
    ["value={messageListDisplayMode}", "setMessageListDisplayMode"],
  ]) {
    const select = elementSource(appSource, valueMarker, "select");
    assert.match(select, new RegExp(`${setter}\\(event\\.target\\.value`));
    assert.match(select, /closeActivePopover\(\);/);
    assert.doesNotMatch(select, /role="(?:listbox|menu)/);
  }

  for (const mode of ["simplified", "original"]) {
    const button = elementSource(appSource, `setFolderHierarchyMode("${mode}")`, "button");
    assert.match(button, /aria-pressed=\{/);
    assert.match(button, /closeActivePopover\(\);/);
  }

  const appearance = elementSource(appSource, "value={appearance}", "select");
  assert.match(appearance, /<option value="system">System<\/option>/);
  assert.match(appearance, /<option value="light">Light<\/option>/);
  assert.match(appearance, /<option value="dark">Dark<\/option>/);

  assert.match(
    appSource,
    /function togglePopover[\s\S]*?toggleOpenPopover\(openPopoverId, id\)/,
    "a trigger's second activation must use the same close-on-toggle transition as the unit test",
  );
  assert.match(appSource, /\{openPopoverId === "layout" \? \(/);
  assert.doesNotMatch(appSource, /<details[^>]*className="layout-controls"/);
  assert.doesNotMatch(appSource, /<summary[^>]*>\s*Layout\s*<\/summary>/);
});

test("selected state, focus, theme tokens, and bounded responsive panels remain visible", () => {
  const focusRule = cssRule(stylesSource, ":focus-visible {");
  assert.match(focusRule, /outline: 3px solid var\(--color-focus-ring\);/);
  assert.match(focusRule, /outline-offset: 2px;/);

  const selectedRule = cssRule(
    stylesSource,
    '.folder-hierarchy-options button[aria-pressed="true"] {',
  );
  assert.match(selectedRule, /background: var\(--color-selected-background\);/);
  assert.match(selectedRule, /color: var\(--color-selected-text\);/);
  assert.match(selectedRule, /border-color: var\(--color-focus-ring\);/);

  for (const themeSelector of [
    ":root {",
    ':root[data-appearance="dark"] {',
    ":root:not([data-appearance]) {",
  ]) {
    assert.match(cssRule(stylesSource, themeSelector), /--color-popover-background:/);
  }

  const boundedPanels = [
    ["\n.recent-menu-panel {", "100vw", "100vh"],
    ["\n.layout-control-grid {", "100vw", "100vh"],
    ["\n.workspace-details-panel {\n  position: absolute;", "100vw", "100vh"],
    ["\n.import-progress-details-panel {\n  position: absolute;", "100vw", "100vh"],
    ["\n.about-modal {", "100vw", "100vh"],
  ];
  for (const [selector, widthBound, heightBound] of boundedPanels) {
    const rule = cssRule(stylesSource, selector);
    assert.match(rule, new RegExp(widthBound));
    assert.match(rule, new RegExp(heightBound));
    assert.match(rule, /background: var\(--color-(?:popover|modal)-background\);/);
    assert.doesNotMatch(rule, /background:\s*#[0-9a-f]/i);
  }

  for (const selector of [
    "\n.advanced-search-panel {",
    "\n.recent-menu-panel {",
    "\n.workspace-menu-panel {",
    "\n.layout-control-grid {",
    "\n.workspace-details-panel {\n  position: absolute;",
    "\n.import-progress-details-panel {\n  position: absolute;",
  ]) {
    const rule = cssRule(stylesSource, selector);
    assert.match(rule, /(?:top|left|right):/);
    assert.match(rule, /background: var\(--color-popover-background\);/);
  }

  const narrowStart = stylesSource.lastIndexOf("@media (max-width: 980px)");
  assert.notEqual(narrowStart, -1);
  const narrowRules = stylesSource.slice(narrowStart);
  assert.match(narrowRules, /\.layout-control-grid,[\s\S]*?grid-template-columns: 1fr;/);
  assert.match(narrowRules, /\.folder-hierarchy-options \{\s*grid-template-columns: 1fr;/);
  assert.match(
    narrowRules,
    /\.pane-layout \{\s*grid-template-columns: 1fr;\s*grid-template-rows: none;\s*overflow: auto;/,
  );
  assert.match(
    narrowRules,
    /\.pane-layout \.folder-restore-rail,[\s\S]*?grid-column: 1;\s*grid-row: auto;/,
  );
  assert.match(
    narrowRules,
    /\.pane-layout-three\.folders-hidden,\s*\.pane-layout-outlook\.folders-hidden \{\s*grid-template-columns: 1fr;\s*grid-template-rows: none;/,
    "higher-specificity hidden-pane grids must not retain desktop columns",
  );
  assert.match(
    narrowRules,
    /\.pane-layout-three\.folders-hidden \.messages-pane,[\s\S]*?\.pane-layout-outlook\.folders-hidden \.outlook-splitter \{\s*grid-column: 1;\s*grid-row: auto;/,
    "hidden-pane children must not create implicit narrow columns",
  );
  assert.match(
    narrowRules,
    /\.folders-pane \{\s*min-height: 500px;/,
    "the stacked folder tree and Folder Details must remain reachable",
  );
});
