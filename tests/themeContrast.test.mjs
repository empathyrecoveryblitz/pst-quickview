import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  contrastRatio,
  extractColorTokens,
  extractCssBlock,
} from "./contrast.mjs";

const css = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");
const light = extractColorTokens(extractCssBlock(css, ":root"));
const dark = {
  ...light,
  ...extractColorTokens(extractCssBlock(css, ':root[data-appearance="dark"]')),
};

const aaPairs = [
  ["primary text / panel", "--color-text-primary", "--color-panel-background"],
  ["secondary text / panel", "--color-text-secondary", "--color-panel-background"],
  ["muted text / panel", "--color-text-muted", "--color-panel-background"],
  ["input text / input", "--color-input-text", "--color-input-background"],
  ["selected text / selected background", "--color-selected-text", "--color-selected-background"],
];

function assertContrast(tokens, label, foregroundToken, backgroundToken, minimum = 4.5) {
  const ratio = contrastRatio(tokens[foregroundToken], tokens[backgroundToken]);
  assert.ok(
    ratio >= minimum,
    `${label} contrast ${ratio.toFixed(2)} is below ${minimum}:1`,
  );
  return ratio;
}

test("light appearance token pairs meet WCAG AA text contrast", (context) => {
  for (const [label, foreground, background] of aaPairs) {
    const ratio = assertContrast(light, label, foreground, background);
    context.diagnostic(`${label}: ${ratio.toFixed(2)}:1`);
  }
});

test("dark appearance token pairs meet WCAG AA text contrast", (context) => {
  for (const [label, foreground, background] of aaPairs) {
    const ratio = assertContrast(dark, label, foreground, background);
    context.diagnostic(`${label}: ${ratio.toFixed(2)}:1`);
  }
  const reader = assertContrast(
    dark,
    "reader text / reader background",
    "--color-reader-text",
    "--color-reader-background",
  );
  const popover = assertContrast(
    dark,
    "popover text / popover background",
    "--color-text-primary",
    "--color-popover-background",
  );
  context.diagnostic(`reader text / reader background: ${reader.toFixed(2)}:1`);
  context.diagnostic(`popover text / popover background: ${popover.toFixed(2)}:1`);
});

test("disabled controls use semantic colors without fading their contents", () => {
  const disabledRule = extractCssBlock(css, "button:disabled,\ninput:disabled,\nselect:disabled,\ntextarea:disabled");
  assert.match(disabledRule, /--color-button-disabled-background/);
  assert.match(disabledRule, /--color-text-disabled/);
  assert.doesNotMatch(disabledRule, /\bopacity\s*:/);
  assertContrast(
    light,
    "light disabled text / disabled background",
    "--color-text-disabled",
    "--color-button-disabled-background",
    3,
  );
  assertContrast(
    dark,
    "dark disabled text / disabled background",
    "--color-text-disabled",
    "--color-button-disabled-background",
    3,
  );
});

test("reader modes keep application text separate from the document canvas", () => {
  assert.match(extractCssBlock(css, ".body-preview"), /color:\s*var\(--color-reader-text\)/);
  const htmlPreview = extractCssBlock(css, ".html-preview");
  assert.match(htmlPreview, /background:\s*var\(--color-document-canvas-background\)/);
  assert.match(htmlPreview, /color:\s*var\(--color-document-text\)/);
  assert.equal(dark["--color-document-canvas-background"], "#ffffff");
  assertContrast(
    dark,
    "document text / intentional light canvas",
    "--color-document-text",
    "--color-document-canvas-background",
  );
});

test("inactive tabs remain legible and content regions are not opacity-faded", () => {
  const inactiveTab = extractCssBlock(css, ".open-pst-tab");
  assert.match(inactiveTab, /color:\s*var\(--color-text-primary\)/);
  assert.doesNotMatch(inactiveTab, /\bopacity\s*:/);

  const opacityRules = [...css.matchAll(/([^{}]+)\{([^{}]*\bopacity\s*:\s*([0-9.]+)[^{}]*)\}/g)]
    .map(([, selector, , opacity]) => ({ selector: selector.trim(), opacity: Number(opacity) }))
    .filter(({ opacity }) => opacity < 0.6);
  assert.deepEqual(opacityRules, []);
});
