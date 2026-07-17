import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import { extractCssBlock } from "./contrast.mjs";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const css = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");

test("About licensing text remains explicit and unchanged", () => {
  assert.match(app, /<dt>PST QuickView license<\/dt>\s*<dd>GPL-3\.0-or-later<\/dd>/);
  assert.match(app, /<dt>ReadPST\/LibPST license<\/dt>\s*<dd>GPL-2\.0-or-later<\/dd>/);
  assert.match(app, /No warranty\. Third-party components retain their own licenses\./);
});

test("About definition rows keep responsive columns and a visible gap", () => {
  const row = extractCssBlock(css, ".about-details > div");
  assert.match(row, /grid-template-columns:\s*var\(--about-label-column\)\s+minmax\(0, 1fr\)/);
  assert.match(row, /column-gap:\s*16px/);

  const details = extractCssBlock(css, ".about-details");
  assert.match(details, /--about-label-column:\s*clamp\(124px, 34%, 168px\)/);
});

test("About paths break at separators without arbitrary anywhere wrapping", () => {
  const value = extractCssBlock(css, ".about-details > div > dd");
  assert.match(value, /overflow-wrap:\s*break-word/);
  assert.match(value, /word-break:\s*normal/);
  assert.doesNotMatch(value, /overflow-wrap:\s*anywhere/);
  assert.match(value, /user-select:\s*text/);

  const path = extractCssBlock(css, ".about-filesystem-path");
  assert.match(path, /ui-monospace/);
  assert.match(path, /overflow-wrap:\s*break-word/);
  assert.match(app, /className="about-filesystem-path" title=\{path\}/);
  assert.match(app, /<wbr \/>/);
});
