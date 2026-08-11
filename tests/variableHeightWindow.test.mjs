import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  DEFAULT_MESSAGE_ROW_ESTIMATE,
  VariableHeightWindow,
} from "../src/variableHeightWindow.ts";

const source = readFileSync(
  new URL("../src/variableHeightWindow.ts", import.meta.url),
  "utf8",
);

function keys(count, prefix = "row") {
  return Array.from({ length: count }, (_, index) => `${prefix}-${index}`);
}

function itemIndices(result) {
  return result.runs.flatMap((run) =>
    run.kind === "items"
      ? Array.from(
          { length: run.endIndex - run.startIndex },
          (_, offset) => run.startIndex + offset,
        )
      : [],
  );
}

function spacerHeight(result) {
  return result.runs.reduce(
    (total, run) => total + (run.kind === "spacer" ? run.height : 0),
    0,
  );
}

function renderedHeight(model, result) {
  return result.renderedIndices.reduce(
    (total, index) => total + model.heightAt(index),
    0,
  );
}

test("empty list produces no rows, ranges, or spacers", () => {
  const model = new VariableHeightWindow();
  assert.deepEqual(model.calculateWindow({ scrollTop: 0, viewportHeight: 500 }), {
    visibleStartIndex: 0,
    visibleEndIndex: 0,
    overscanStartIndex: 0,
    overscanEndIndex: 0,
    renderedIndices: [],
    runs: [],
    totalHeight: 0,
    renderedRowCount: 0,
  });
});

test("one row and a short list below viewport render without spacers", () => {
  const one = new VariableHeightWindow(["only"], 100);
  assert.deepEqual(
    one.calculateWindow({ scrollTop: 0, viewportHeight: 500 }).renderedIndices,
    [0],
  );

  const short = new VariableHeightWindow(keys(4), 100);
  const result = short.calculateWindow({ scrollTop: 0, viewportHeight: 500 });
  assert.deepEqual(result.renderedIndices, [0, 1, 2, 3]);
  assert.deepEqual(result.runs, [{ kind: "items", startIndex: 0, endIndex: 4 }]);
});

test("ordinary visible ranges follow scroll positions near start, middle, and end", () => {
  const model = new VariableHeightWindow(keys(100), 100, {
    minimumOverscanRows: 0,
    maximumOverscanRows: 0,
  });
  assert.deepEqual(
    model.calculateWindow({ scrollTop: 0, viewportHeight: 300 }).renderedIndices,
    [0, 1, 2],
  );
  assert.deepEqual(
    model.calculateWindow({ scrollTop: 4_950, viewportHeight: 300 }).renderedIndices,
    [49, 50, 51, 52],
  );
  assert.deepEqual(
    model.calculateWindow({ scrollTop: 99_999, viewportHeight: 300 }).renderedIndices,
    [97, 98, 99],
  );
});

test("overscan honors the six-row minimum and twenty-row maximum", () => {
  const model = new VariableHeightWindow(keys(200), 100);
  const minimum = model.calculateWindow({ scrollTop: 10_000, viewportHeight: 100 });
  assert.equal(minimum.visibleEndIndex - minimum.visibleStartIndex, 1);
  assert.equal(minimum.visibleStartIndex - minimum.overscanStartIndex, 6);
  assert.equal(minimum.overscanEndIndex - minimum.visibleEndIndex, 6);

  const maximum = model.calculateWindow({ scrollTop: 8_000, viewportHeight: 3_000 });
  assert.equal(maximum.visibleEndIndex - maximum.visibleStartIndex, 30);
  assert.equal(maximum.visibleStartIndex - maximum.overscanStartIndex, 20);
  assert.equal(maximum.overscanEndIndex - maximum.visibleEndIndex, 20);
  assert.ok(maximum.renderedRowCount <= 80);
});

test("estimated heights initialize totals and measured heights replace estimates", () => {
  const model = new VariableHeightWindow(keys(3));
  assert.equal(model.totalHeight(), DEFAULT_MESSAGE_ROW_ESTIMATE * 3);
  assert.equal(model.updateMeasuredHeight("row-1", 144), true);
  assert.equal(model.heightAt(1), 144);
  assert.equal(model.totalHeight(), DEFAULT_MESSAGE_ROW_ESTIMATE * 2 + 144);
  assert.equal(model.updateMeasuredHeight("row-1", 144), false);
});

test("measurement above the viewport preserves the first visible scroll anchor", () => {
  const model = new VariableHeightWindow(keys(20), 100);
  const anchor = model.captureScrollAnchor(525);
  assert.deepEqual(anchor, { key: "row-5", viewportOffset: -25 });
  assert.equal(model.updateMeasuredHeight("row-1", 180), true);
  assert.equal(model.restoreScrollAnchor(anchor, 300), 605);
});

test("measurement inside the anchored row does not move its top edge", () => {
  const model = new VariableHeightWindow(keys(20), 100);
  const anchor = model.captureScrollAnchor(525);
  assert.equal(model.updateMeasuredHeight("row-5", 180), true);
  assert.equal(model.restoreScrollAnchor(anchor, 300), 525);
});

test("append preserves existing key measurements and grows estimated rows", () => {
  const model = new VariableHeightWindow(keys(3), 100);
  model.updateMeasuredHeight("row-1", 175);
  assert.equal(model.appendKeys(["row-3", "row-4"]), true);
  assert.equal(model.itemCount, 5);
  assert.equal(model.heightAt(1), 175);
  assert.equal(model.heightAt(4), 100);
  assert.equal(model.totalHeight(), 575);
});

test("per-item estimates support mixed logical entry kinds and append", () => {
  const model = new VariableHeightWindow([], 100);
  model.resetItems([
    { key: "header", estimatedHeight: 78 },
    { key: "message", estimatedHeight: 52 },
    { key: "actions", estimatedHeight: 48 },
  ]);
  assert.equal(model.totalHeight(), 178);
  model.updateMeasuredHeight("message", 61);
  model.appendItems([{ key: "load-more", estimatedHeight: 74 }]);
  assert.equal(model.heightAt(1), 61);
  assert.equal(model.heightAt(3), 74);
  assert.equal(model.totalHeight(), 261);
});

test("per-item estimate changes preserve measurements and reset discards them", () => {
  const model = new VariableHeightWindow([], 100);
  model.resetItems([{ key: "row", estimatedHeight: 52 }]);
  model.updateMeasuredHeight("row", 65);
  assert.equal(model.syncItems([{ key: "row", estimatedHeight: 78 }]), true);
  assert.equal(model.heightAt(0), 65);
  model.resetItems([{ key: "row", estimatedHeight: 78 }]);
  assert.equal(model.heightAt(0), 78);
});

test("removing a key prunes its stale measurement before a later reinsert", () => {
  const model = new VariableHeightWindow([], 100);
  model.resetItems([
    { key: "header", estimatedHeight: 78 },
    { key: "expanded-message", estimatedHeight: 52 },
  ]);
  model.updateMeasuredHeight("expanded-message", 91);
  model.syncItems([{ key: "header", estimatedHeight: 78 }]);
  model.syncItems([
    { key: "header", estimatedHeight: 78 },
    { key: "expanded-message", estimatedHeight: 52 },
  ]);
  assert.equal(model.heightAt(1), 52);
});

test("complete reset discards prior measurements", () => {
  const model = new VariableHeightWindow(keys(3), 100);
  model.updateMeasuredHeight("row-1", 175);
  model.reset(keys(3, "fresh"));
  assert.equal(model.itemCount, 3);
  assert.equal(model.totalHeight(), 300);
  assert.equal(model.indexForKey("row-1"), -1);
});

test("100,000 loaded rows retain a bounded ordinary render window", () => {
  const model = new VariableHeightWindow(keys(100_000), 96);
  const result = model.calculateWindow({
    scrollTop: 4_800_000,
    viewportHeight: 900,
  });
  assert.equal(model.itemCount, 100_000);
  assert.ok(result.renderedRowCount > 0);
  assert.ok(result.renderedRowCount <= 80);
  assert.ok(result.visibleStartIndex > 49_000);
  assert.ok(result.visibleEndIndex < 51_000);
});

test("invalid scroll and viewport values clamp safely", () => {
  const model = new VariableHeightWindow(keys(10), 100);
  assert.equal(
    model.calculateWindow({ scrollTop: -500, viewportHeight: Number.NaN })
      .visibleStartIndex,
    0,
  );
  const end = model.calculateWindow({
    scrollTop: Number.POSITIVE_INFINITY,
    viewportHeight: -20,
  });
  assert.equal(end.visibleStartIndex, 0);
  assert.ok(end.renderedRowCount > 0);
});

test("invalid measurements are ignored", () => {
  const model = new VariableHeightWindow(["row"], 100);
  for (const value of [0, -1, Number.NaN, Number.POSITIVE_INFINITY, 5000]) {
    assert.equal(model.updateMeasuredHeight("row", value), false);
  }
  assert.equal(model.updateMeasuredHeight("missing", 120), false);
  assert.equal(model.heightAt(0), 100);
});

test("selected and focused pins remain mounted above and below the viewport", () => {
  const model = new VariableHeightWindow(keys(100), 100, {
    minimumOverscanRows: 1,
    maximumOverscanRows: 1,
  });
  const result = model.calculateWindow({
    scrollTop: 4_900,
    viewportHeight: 300,
    pinnedKeys: ["row-2", "row-90"],
  });
  assert.ok(result.renderedIndices.includes(2));
  assert.ok(result.renderedIndices.includes(90));
  assert.ok(result.renderedIndices.includes(49));
});

test("duplicate selected and focused pins produce one mounted row", () => {
  const model = new VariableHeightWindow(keys(30), 100);
  const result = model.calculateWindow({
    scrollTop: 1_500,
    viewportHeight: 200,
    pinnedIndices: [2, 2],
    pinnedKeys: ["row-2", "row-2"],
  });
  assert.equal(result.renderedIndices.filter((index) => index === 2).length, 1);
});

test("pins create ordered disjoint item and spacer runs", () => {
  const model = new VariableHeightWindow(keys(20), 100, {
    minimumOverscanRows: 0,
    maximumOverscanRows: 0,
  });
  const result = model.calculateWindow({
    scrollTop: 800,
    viewportHeight: 200,
    pinnedIndices: [1, 17],
  });
  assert.deepEqual(itemIndices(result), [1, 8, 9, 17]);
  assert.equal(result.runs.filter((run) => run.kind === "items").length, 3);
  assert.equal(result.runs.filter((run) => run.kind === "spacer").length, 4);
});

test("spacers plus rendered rows reproduce the complete estimated height", () => {
  const model = new VariableHeightWindow(keys(50), 100, {
    minimumOverscanRows: 2,
    maximumOverscanRows: 2,
  });
  model.updateMeasuredHeight("row-3", 160);
  model.updateMeasuredHeight("row-40", 75);
  const result = model.calculateWindow({
    scrollTop: 2_000,
    viewportHeight: 400,
    pinnedKeys: ["row-3", "row-40"],
  });
  assert.equal(spacerHeight(result) + renderedHeight(model, result), result.totalHeight);
  assert.deepEqual(itemIndices(result), result.renderedIndices);
});

test("scroll-to-index and resize calculations keep rows in view", () => {
  const model = new VariableHeightWindow(keys(50), 100);
  assert.equal(model.scrollTopForIndex(20, 0, 300), 1_800);
  assert.equal(model.scrollTopForIndex(20, 1_900, 500), 1_900);
  assert.equal(model.scrollTopForIndex(2, 1_900, 500), 200);

  const wide = model.calculateWindow({ scrollTop: 1_900, viewportHeight: 500 });
  const narrow = model.calculateWindow({ scrollTop: 1_900, viewportHeight: 200 });
  assert.ok(wide.visibleEndIndex > narrow.visibleEndIndex);
  assert.equal(wide.visibleStartIndex, narrow.visibleStartIndex);
});

test("Page Up and Page Down use measured prefix heights", () => {
  const model = new VariableHeightWindow(keys(20), 100);
  model.updateMeasuredHeight("row-5", 250);
  assert.equal(model.pageIndexForKey("row-4", 1, 350), 6);
  assert.equal(model.pageIndexForKey("row-8", -1, 350), 5);
  assert.equal(model.pageIndexForKey("missing", 1, 350), -1);
  assert.equal(model.pageIndexForKey("row-4", 1, 0), 4);
});

test("stable key remapping preserves measurements at new indices", () => {
  const model = new VariableHeightWindow(["a", "b", "c"], 100);
  model.updateMeasuredHeight("b", 180);
  assert.equal(model.syncKeys(["c", "b", "a", "d"]), true);
  assert.equal(model.indexForKey("b"), 1);
  assert.equal(model.heightAt(1), 180);
  assert.equal(model.heightAt(3), 100);
  assert.equal(model.syncKeys(["c", "b", "a", "d"]), false);
});

test("range lookup uses the Fenwick prefix index rather than a full row scan", () => {
  assert.match(source, /class FenwickHeightIndex/);
  assert.match(source, /indexAtOffset\(offset: number\)/);
  assert.match(source, /cursor \+= cursor & -cursor/);
  assert.doesNotMatch(
    source.slice(source.indexOf("calculateWindow("), source.indexOf("private buildRuns")),
    /this\.heights\.(?:find|findIndex|reduce)/,
  );
});
