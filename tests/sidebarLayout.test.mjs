import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { extractCssBlock } from "./contrast.mjs";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const css = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");

test("folder heading stays inset and stacks controls within narrow panes", () => {
  const heading = extractCssBlock(css, ".folders-heading");
  const title = extractCssBlock(css, ".folders-heading-title");
  const controls = extractCssBlock(css, ".folders-heading-controls");
  const headingText = extractCssBlock(css, ".folders-heading h2");

  const inlinePadding = /padding:\s*\d+(?:\.\d+)?px\s+(\d+(?:\.\d+)?)px/.exec(heading);
  assert.ok(inlinePadding, "folder heading must declare vertical and horizontal padding");
  assert.ok(Number(inlinePadding[1]) >= 8, "folder heading needs a durable left inset");
  assert.match(heading, /flex-direction:\s*column/);
  assert.match(heading, /width:\s*100%/);
  assert.match(heading, /min-width:\s*0/);
  assert.match(title, /width:\s*100%/);
  assert.match(title, /min-width:\s*0/);
  assert.match(controls, /width:\s*100%/);
  assert.match(controls, /flex-wrap:\s*wrap/);
  assert.match(headingText, /overflow:\s*visible/);
  assert.doesNotMatch(headingText, /text-overflow:\s*ellipsis/);

  for (const [label, block] of [
    ["folder heading", heading],
    ["folder heading title", title],
    ["folder heading text", headingText],
  ]) {
    assert.doesNotMatch(block, /margin-left:\s*-/i, `${label} must not use a negative left margin`);
    assert.doesNotMatch(
      block,
      /transform:\s*translate(?:X|3d)?\(\s*-/i,
      `${label} must not translate left`,
    );
  }
});

test("folder splitters occupy separate grid tracks without overlay positioning", () => {
  const threeColumnLayout = extractCssBlock(css, ".pane-layout-three");
  const outlookLayout = extractCssBlock(css, ".pane-layout-outlook");
  const splitter = extractCssBlock(css, ".pane-splitter");

  assert.match(
    threeColumnLayout,
    /minmax\(180px, var\(--folder-pane-width, 260px\)\)\s*8px/,
  );
  assert.match(
    outlookLayout,
    /minmax\(180px, var\(--folder-pane-width, 260px\)\)\s+8px/,
  );
  assert.doesNotMatch(splitter, /position:\s*(?:absolute|fixed)/);
  assert.doesNotMatch(splitter, /margin-left:\s*-/);
  assert.doesNotMatch(splitter, /transform:\s*translate(?:X|3d)?\(\s*-/i);
});

test("stored and resized folder widths remain clamped to the sidebar minimum", () => {
  assert.match(app, /const folderPaneMin = 180;/);
  assert.match(app, /folder:\s*clamp\(folder, folderPaneMin, 520\)/);
  assert.match(
    app,
    /const folder = clamp\(nextWidths\.folder, folderPaneMin, Math\.min\(520, maxFolder\)\)/,
  );
});
