import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const backend = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const previewStart = app.indexOf("function MessagePreviewWindow");
const previewEnd = app.indexOf("function App()");
const previewWindow = app.slice(previewStart, previewEnd);

test("standalone EML and MSG previews explain permanent remote-image blocking", () => {
  assert.match(app, /Remote images are blocked for privacy and security\./);
  assert.match(previewWindow, /invoke<SourceEmlView>\("get_standalone_message_view"/);
  assert.doesNotMatch(app, /(?:Load|Loading) remote (?:resources|images)/i);
});

test("remote resources have no frontend approval or backend allow flag", () => {
  for (const source of [app, backend]) {
    assert.doesNotMatch(source, /allowRemote(?:Images|Resources)|allow_remote_(?:images|resources)/);
  }
  assert.doesNotMatch(app, /sourceEmlRemoteAllowed|remoteImagesAllowedMessageId/);
  assert.doesNotMatch(app, /loadSourceEml\(true\)|setRemoteImagesAllowedMessageId/);
  assert.match(backend, /let url_schemes = HashSet::from\(\["cid", "data"\]\);/);
  assert.doesNotMatch(backend, /HashSet::from\(\["cid", "data", "http", "https"\]\)/);
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

test("PST-derived and standalone rendering use the same fail-closed sanitizer", () => {
  assert.match(previewWindow, /target\.kind === "workspace"/);
  assert.match(previewWindow, /invoke<SourceEmlView>\("get_source_eml_view"/);
  assert.match(app, /invoke<HtmlRenderResult>\("render_message_html"/);
  assert.match(backend, /sanitize_email_html\(&body_html, cid_images\)/);
  assert.match(backend, /sanitize_email_html\(&message\.body_html, message\.cid_images\.clone\(\)\)/);
  assert.equal((app.match(/dangerouslySetInnerHTML/g) ?? []).length, 3);
  assert.equal(
    (app.match(/dangerouslySetInnerHTML=\{\{ __html: sourceEmlView\.sanitizedHtml \}\}/g) ?? [])
      .length,
    2,
  );
  assert.doesNotMatch(app, /fetch\(|XMLHttpRequest|remoteImagesAllowed\s*=\s*true/);
  assert.match(
    backend,
    /fn save_printable_html_as[\s\S]*sanitize_printable_html_document\(&html\)\?/,
  );
  assert.match(backend, /Printable HTML exceeds the supported size limit\./);
  assert.match(backend, /default-src 'none'; img-src data:/);
});
