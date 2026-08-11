import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  matchedFieldLabel,
  matchedFieldLabelsForResult,
  normalizeHighlightRanges,
  splitHighlightedText,
} from "../src/searchHighlight.ts";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const messageResults = readFileSync(
  new URL("../src/MessageResultsList.tsx", import.meta.url),
  "utf8",
);
const helperSource = readFileSync(
  new URL("../src/searchHighlight.ts", import.meta.url),
  "utf8",
);

function reconstruct(segments) {
  return segments.map((segment) => segment.text).join("");
}

test("no ranges preserves the exact plain text", () => {
  const text = "No match here";
  assert.deepEqual(normalizeHighlightRanges(text, []), []);
  const segments = splitHighlightedText(text, undefined);
  assert.deepEqual(segments, [{ text, highlighted: false }]);
  assert.equal(reconstruct(segments), text);
});

test("one and multiple valid ranges split text structurally", () => {
  assert.deepEqual(splitHighlightedText("alpha beta", [{ start: 0, end: 5 }]), [
    { text: "alpha", highlighted: true },
    { text: " beta", highlighted: false },
  ]);
  assert.deepEqual(
    splitHighlightedText("alpha beta gamma", [
      { start: 0, end: 5 },
      { start: 11, end: 16 },
    ]),
    [
      { text: "alpha", highlighted: true },
      { text: " beta ", highlighted: false },
      { text: "gamma", highlighted: true },
    ],
  );
});

test("ranges are sorted and overlapping adjacent or duplicate ranges merge", () => {
  assert.deepEqual(
    normalizeHighlightRanges("abcdefghij", [
      { start: 5, end: 8 },
      { start: 0, end: 3 },
      { start: 2, end: 5 },
      { start: 0, end: 3 },
      { start: 8, end: 10 },
    ]),
    [{ start: 0, end: 10 }],
  );
});

test("invalid and out-of-bounds ranges are ignored", () => {
  const text = "abcdef";
  assert.deepEqual(
    normalizeHighlightRanges(text, [
      { start: -1, end: 2 },
      { start: 3, end: 2 },
      { start: 2, end: 2 },
      { start: 0, end: 20 },
      { start: 1.5, end: 3 },
      { start: 1, end: 4 },
    ]),
    [{ start: 1, end: 4 }],
  );
});

test("frontend range validation caps highlighted regions at eight", () => {
  const text = "a b c d e f g h i j";
  const ranges = Array.from({ length: 10 }, (_, index) => ({
    start: index * 2,
    end: index * 2 + 1,
  }));
  assert.equal(normalizeHighlightRanges(text, ranges).length, 8);
});

test("UTF-16 ranges preserve emoji, CJK, and RTL text exactly", () => {
  for (const [text, range, expected] of [
    ["A😀B", { start: 1, end: 3 }, "😀"],
    ["前東京後", { start: 1, end: 3 }, "東京"],
    ["قبل العربية بعد", { start: 4, end: 11 }, "العربية"],
  ]) {
    const segments = splitHighlightedText(text, [range]);
    assert.equal(reconstruct(segments), text);
    assert.equal(
      segments.filter((segment) => segment.highlighted).map((segment) => segment.text).join(""),
      expected,
    );
  }
});

test("matched-field labels use the fixed allowlist and remove duplicates", () => {
  assert.equal(matchedFieldLabel("subject"), "Subject");
  assert.equal(matchedFieldLabel("sender"), "Sender");
  assert.equal(matchedFieldLabel("recipients"), "Recipients");
  assert.equal(matchedFieldLabel("body"), "Body");
  assert.equal(matchedFieldLabel("attachment"), "Attachment");
  assert.equal(matchedFieldLabel("unknown"), null);
  assert.deepEqual(
    matchedFieldLabelsForResult([
      "subject",
      "unknown",
      "body",
      "subject",
      "attachment",
    ]),
    ["Subject", "Body", "Attachment"],
  );
});

test("search-result integration uses backend ranges and retains heuristic fallback", () => {
  assert.doesNotMatch(helperSource, /dangerouslySetInnerHTML/);
  assert.match(app, /<MessageResultsList/);
  assert.match(messageResults, /const matchContext = message\.searchMatchContext/);
  assert.match(
    messageResults,
    /<BackendHighlightedText[\s\S]*ranges=\{matchContext\.highlightRanges\}/,
  );
  assert.match(
    messageResults,
    /:\s*\(\s*<HighlightedText text=\{displaySnippet\} terms=\{highlightTerms\}/,
  );
  assert.match(
    messageResults,
    /matchedFieldLabelsForResult\(matchContext\?\.matchedFields\)/,
  );
});
