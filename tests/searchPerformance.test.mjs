import assert from "node:assert/strict";
import { performance } from "node:perf_hooks";
import test from "node:test";

import {
  flattenConversationResults,
} from "../src/conversationResultsModel.ts";
import { ResultNavigationModel } from "../src/resultNavigation.ts";
import { appendUniqueByKey } from "../src/searchRequest.ts";
import { VariableHeightWindow } from "../src/variableHeightWindow.ts";

function percentile(samples, fraction) {
  const sorted = [...samples].sort((left, right) => left - right);
  const index = Math.min(
    sorted.length - 1,
    Math.max(0, Math.ceil(sorted.length * fraction) - 1),
  );
  return sorted[index];
}

function measure(iterations, operation) {
  const samples = [];
  for (let index = 0; index < iterations; index += 1) {
    const started = performance.now();
    operation(index);
    samples.push(performance.now() - started);
  }
  return {
    p50: percentile(samples, 0.5),
    p95: percentile(samples, 0.95),
  };
}

function diagnostic(t, metric, datasetSize, iterations, result) {
  t.diagnostic(
    `SEARCH_FRONTEND dataset_rows=${datasetSize} metric=${metric} iterations=${iterations} ` +
      `p50_ms=${result.p50.toFixed(4)} p95_ms=${result.p95.toFixed(4)}`,
  );
}

function conversation(index) {
  return {
    conversationId: `conversation-${index}`,
    conversationRootId: index,
    subject: `Synthetic subject ${index % 101}`,
    latestSender: `sender-${index % 31}`,
    participants: [`participant-${index % 17}`],
    latestDate: "2026-01-01T00:00:00+00:00",
    snippet: `Synthetic snippet ${index}`,
    matchingMessageCount: 1,
    totalMessageCount: 1,
    hasAttachments: index % 19 === 0,
    latestMessageId: index,
    assignmentMethod: "synthetic",
    workspaceId: `workspace-${index % 3}`,
    pstDisplayName: "Synthetic workspace",
    workspacePath: "",
  };
}

test("Search 2.0 frontend models expose deterministic scalability diagnostics", (t) => {
  const itemCount = 100_000;
  const keys = Array.from({ length: itemCount }, (_, index) => `workspace:${index}`);

  const construction = measure(5, () => {
    const model = new VariableHeightWindow(keys, 96);
    assert.equal(model.itemCount, itemCount);
  });
  diagnostic(t, "window_model_build", itemCount, 5, construction);

  const windowModel = new VariableHeightWindow(keys, 96);
  const visibleRange = measure(1_000, (iteration) => {
    const result = windowModel.calculateWindow({
      scrollTop: (iteration * 7_919) % (windowModel.totalHeight() - 720),
      viewportHeight: 720,
    });
    assert.ok(result.renderedRowCount > 0);
    assert.ok(result.renderedRowCount <= 80);
  });
  diagnostic(t, "visible_range", itemCount, 1_000, visibleRange);

  const measurementUpdate = measure(5_000, (iteration) => {
    const key = keys[iteration % keys.length];
    assert.equal(windowModel.updateMeasuredHeight(key, 72 + (iteration % 13)), true);
  });
  diagnostic(t, "measurement_update", itemCount, 5_000, measurementUpdate);

  const pageDestination = measure(5_000, (iteration) => {
    const key = keys[(iteration * 37) % keys.length];
    const destination = windowModel.pageIndexForKey(
      key,
      iteration % 2 === 0 ? 1 : -1,
      720,
    );
    assert.ok(destination >= 0 && destination < itemCount);
  });
  diagnostic(t, "page_destination", itemCount, 5_000, pageDestination);

  const navigationEntries = keys.map((key, logicalIndex) => ({
    key,
    logicalKey: key,
    logicalIndex,
  }));
  const navigationBuild = measure(5, () => {
    const model = new ResultNavigationModel(navigationEntries);
    assert.equal(model.size, itemCount);
  });
  diagnostic(t, "navigation_model_build", itemCount, 5, navigationBuild);

  const navigationModel = new ResultNavigationModel(navigationEntries);
  const adjacentNavigation = measure(10_000, (iteration) => {
    const key = keys[iteration % (keys.length - 1)];
    assert.equal(navigationModel.nextKey(key), keys[(iteration % (keys.length - 1)) + 1]);
  });
  diagnostic(t, "navigation_next", itemCount, 10_000, adjacentNavigation);

  const navigationPage = measure(5_000, (iteration) => {
    const position = (iteration * 41) % (keys.length - 100);
    const key = keys[position];
    const destination = navigationModel.pageKey(key, position + 50, 1);
    assert.equal(destination, keys[position + 50]);
  });
  diagnostic(t, "navigation_page", itemCount, 5_000, navigationPage);

  const conversations = Array.from({ length: itemCount }, (_, index) => conversation(index + 1));
  const conversationFlatten = measure(3, () => {
    const entries = flattenConversationResults({ conversations, expandedConversations: {} });
    assert.equal(entries.length, itemCount);
    assert.equal(new Set(entries.map((entry) => entry.key)).size, itemCount);
  });
  diagnostic(t, "conversation_flatten", itemCount, 3, conversationFlatten);

  const retained = Array.from({ length: 50_000 }, (_, index) => ({ id: index }));
  const incoming = Array.from({ length: 50_000 }, (_, index) => ({ id: index + 25_000 }));
  const deduplication = measure(10, () => {
    const merged = appendUniqueByKey(retained, incoming, (item) => String(item.id));
    assert.equal(merged.length, 75_000);
  });
  diagnostic(t, "result_deduplication", 75_000, 10, deduplication);
});
