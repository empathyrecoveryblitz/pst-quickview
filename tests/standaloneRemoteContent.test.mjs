import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const previewStart = app.indexOf("function MessagePreviewWindow");
const previewEnd = app.indexOf("function App()");
const previewWindow = app.slice(previewStart, previewEnd);

test("standalone EML and MSG previews share accurate remote-image wording", () => {
  assert.match(
    app,
    /const loadRemoteImagesActionLabel = "Load remote images for this message";/,
  );
  assert.match(
    app,
    /const loadingRemoteImagesStatus = "Loading remote images for this message\.";/,
  );
  assert.match(previewWindow, /invoke<SourceEmlView>\("get_standalone_message_view"/);
  assert.doesNotMatch(previewWindow, /(?:Load|Loading) remote (?:resources|images).*EML/i);
  assert.doesNotMatch(app, /Load remote (?:resources|images) for this EML/i);
});

test("remote-image visible, accessible, and tooltip labels agree", () => {
  const ariaLabels = app.match(/aria-label=\{loadRemoteImagesActionLabel\}/g) ?? [];
  const titles = app.match(/title=\{loadRemoteImagesActionLabel\}/g) ?? [];
  const visibleLabels = app.match(/>\s*\{loadRemoteImagesActionLabel\}\s*<\/button>/g) ?? [];

  assert.equal(ariaLabels.length, 3);
  assert.equal(titles.length, 3);
  assert.equal(visibleLabels.length, 3);
});

test("remote images remain blocked until explicit per-message approval", () => {
  assert.match(
    previewWindow,
    /useState\(false\);[\s\S]*sourceEmlRemoteAllowed/,
  );
  assert.match(previewWindow, /void loadSourceEml\(false\);/);
  assert.match(
    previewWindow,
    /sourceEmlView\.remoteImagesBlocked && !sourceEmlRemoteAllowed/,
  );
  assert.match(previewWindow, /onClick=\{\(\) => void loadSourceEml\(true\)\}/);
  assert.equal((previewWindow.match(/loadSourceEml\(true\)/g) ?? []).length, 1);
  assert.match(previewWindow, /allowRemoteResources,\s*\}\);/);
  assert.match(previewWindow, /setSourceEmlRemoteAllowed\(allowRemoteResources\)/);
});

test("standalone source-format copy is MSG-aware without weakening EML wording", () => {
  assert.match(
    app,
    /function sourceMessageFormatLabel[\s\S]*sourceFormat === "msg" \? "MSG" : "EML"/,
  );
  assert.match(
    app,
    /Printed structured MSG properties and diagnostics\. No remote resources were loaded\./,
  );
  assert.match(app, /Printed raw EML source text\. No remote resources were loaded\./);
  assert.match(app, /sourceFormat === "msg"[\s\S]*"Search structured MSG properties"/);
  assert.match(app, /"Search raw EML source"/);
  assert.match(previewWindow, /Loading message source\./);
  assert.doesNotMatch(previewWindow, /Choose where to save the source EML\./);
  assert.doesNotMatch(previewWindow, /Revealed saved source EML/);
});

test("PST-derived rendering and sanitized HTML integration remain unchanged", () => {
  assert.match(previewWindow, /target\.kind === "workspace"/);
  assert.match(previewWindow, /invoke<SourceEmlView>\("get_source_eml_view"/);
  assert.match(app, /setRemoteImagesAllowedMessageId\(selectedMessage\.id\)/);
  assert.equal((app.match(/dangerouslySetInnerHTML/g) ?? []).length, 3);
  assert.equal(
    (app.match(/dangerouslySetInnerHTML=\{\{ __html: sourceEmlView\.sanitizedHtml \}\}/g) ?? [])
      .length,
    2,
  );
  assert.doesNotMatch(previewWindow, /fetch\(|XMLHttpRequest|remoteImagesAllowed\s*=\s*true/);
});
