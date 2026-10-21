import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, lstatSync, readFileSync, readdirSync } from "node:fs";
import { fileURLToPath } from "node:url";

const tauriRootUrl = new URL("../src-tauri/", import.meta.url);
const tauriConfigUrl = new URL("../src-tauri/tauri.conf.json", import.meta.url);
const devConfigUrl = new URL("../src-tauri/tauri.dev.conf.json", import.meta.url);
const capabilitiesUrl = new URL("../src-tauri/capabilities/", import.meta.url);
const capabilitiesPathUrl = new URL("../src-tauri/capabilities", import.meta.url);
const buildScriptUrl = new URL("../src-tauri/build.rs", import.meta.url);
const rustSourceUrl = new URL("../src-tauri/src/lib.rs", import.meta.url);
const importIntegrityPhase2Url = new URL(
  "../src-tauri/src/import_integrity_phase2.rs",
  import.meta.url,
);
const appSourceUrl = new URL("../src/App.tsx", import.meta.url);
const releaseVerifierUrl = new URL("../scripts/verify-macos-release.sh", import.meta.url);
const packagedCspVerifierUrl = new URL(
  "../scripts/verify-packaged-csp.py",
  import.meta.url,
);

const tauriConfig = JSON.parse(readFileSync(tauriConfigUrl, "utf8"));
const devConfig = JSON.parse(readFileSync(devConfigUrl, "utf8"));
const buildSource = readFileSync(buildScriptUrl, "utf8");
const rustSource = readFileSync(rustSourceUrl, "utf8");
const importIntegrityPhase2Source = readFileSync(importIntegrityPhase2Url, "utf8");
const appSource = readFileSync(appSourceUrl, "utf8");
const supportPanelSource = readFileSync(new URL("../src/RemoteSupportPanel.tsx", import.meta.url), "utf8");
const supportControlSource = readFileSync(new URL("../src/RemoteSupportControl.tsx", import.meta.url), "utf8");
const supportSessionSource = readFileSync(new URL("../public/support/session.js", import.meta.url), "utf8");
const supportRustSource = readFileSync(new URL("../src-tauri/src/support_session.rs", import.meta.url), "utf8");
const releaseVerifier = readFileSync(releaseVerifierUrl, "utf8");
const packagedCspVerifier = readFileSync(packagedCspVerifierUrl, "utf8");

function listCapabilityEntries(directoryUrl, prefix = "") {
  return readdirSync(directoryUrl, { withFileTypes: true }).flatMap((entry) => {
    const relativePath = prefix ? `${prefix}/${entry.name}` : entry.name;
    const kind = entry.isSymbolicLink()
      ? "symlink"
      : entry.isDirectory()
        ? "directory"
        : entry.isFile()
          ? "file"
          : "other";
    const current = [{ path: relativePath, kind }];
    if (!entry.isDirectory()) return current;
    return [
      ...current,
      ...listCapabilityEntries(
        new URL(`${encodeURIComponent(entry.name)}/`, directoryUrl),
        relativePath,
      ),
    ];
  });
}

const capabilityEntries = listCapabilityEntries(capabilitiesUrl).sort((left, right) =>
  left.path.localeCompare(right.path),
);
const capabilityFiles = capabilityEntries
  .filter((entry) => entry.kind === "file" && entry.path.endsWith(".json"))
  .map((entry) => entry.path);
const capabilities = new Map(
  capabilityFiles.map((file) => {
    const capability = JSON.parse(
      readFileSync(new URL(file, capabilitiesUrl), "utf8"),
    );
    return [capability.identifier, capability];
  }),
);

function parseCsp(csp) {
  const directives = {};
  for (const rawDirective of csp.split(";")) {
    const fields = rawDirective.trim().split(/\s+/).filter(Boolean);
    if (fields.length === 0) continue;
    const [name, ...values] = fields;
    assert.equal(directives[name], undefined, `duplicate CSP directive: ${name}`);
    directives[name] = values;
  }
  return directives;
}

const supportFallbackCsp = "default-src 'none'; script-src 'self'; style-src 'self'; frame-src 'none'; frame-ancestors 'none'; object-src 'none'; media-src 'none'; base-uri 'none'; form-action 'none'";
const supportConnectPrefix = "connect-src ipc: http://ipc.localhost ";
function supportLiteralInventory(arch = "arm64") {
  return `\0Content-Security-Policy${supportFallbackCsp}Cache-Control\0${arch === "x86_64" ? "\0" : "tauri"}${supportConnectPrefix}; \0`;
}

function verifySyntheticPackagedCsp(binary, arch = "arm64") {
  const runner = String.raw`
import importlib.util
import sys
from pathlib import Path

spec = importlib.util.spec_from_file_location("packaged_csp_verifier", sys.argv[1])
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
policies = module.reviewed_policies(Path(sys.argv[2]))
try:
    module.verify_slice(sys.stdin.buffer.read(), sys.argv[3], policies)
except module.VerificationError as error:
    print(error, file=sys.stderr)
    raise SystemExit(1)
`;
  return spawnSync(
    "python3",
    [
      "-c",
      runner,
      fileURLToPath(packagedCspVerifierUrl),
      fileURLToPath(tauriConfigUrl),
      arch,
    ],
    { input: binary },
  );
}

function sourceSection(source, startMarker, endMarker) {
  const start = source.indexOf(startMarker);
  assert.notEqual(start, -1, `missing source marker: ${startMarker}`);
  const remainderStart = start + startMarker.length;
  const relativeEnd = endMarker instanceof RegExp
    ? source.slice(remainderStart).search(endMarker)
    : source.indexOf(endMarker, remainderStart) - remainderStart;
  const end = relativeEnd < 0 ? -1 : remainderStart + relativeEnd;
  assert.notEqual(end, -1, `missing source marker: ${endMarker}`);
  return source.slice(start, end);
}

function extractBuildCommands(source) {
  const declaration = source.match(
    /const APP_COMMANDS:\s*&\[&str\]\s*=\s*&\[(?<body>[\s\S]*?)\];/,
  );
  assert.ok(declaration?.groups?.body, "build.rs must declare APP_COMMANDS");
  return [...declaration.groups.body.matchAll(/"([a-z][a-z0-9_]*)"/g)].map(
    (match) => match[1],
  );
}

function extractRegisteredCommands(source) {
  const handler = source.match(
    /\.invoke_handler\(tauri::generate_handler!\[(?<body>[\s\S]*?)\]\)/,
  );
  assert.ok(handler?.groups?.body, "lib.rs must register a Tauri command handler");
  const paths = handler.groups.body.split(",");
  if (paths.at(-1).trim() === "") paths.pop();
  return paths.map((path) => {
    const command = path.trim().match(/^(?:[a-z][a-z0-9_]*::)*([a-z][a-z0-9_]*)$/);
    assert.ok(command, "every registered command must be an exact Rust command path");
    return command[1];
  });
}

function appPermission(command) {
  return `allow-${command.replaceAll("_", "-")}`;
}

function extractFrontendCommands(source) {
  const invokeCalls = [...source.matchAll(/\binvoke(?:<[^>]+>)?\s*\(/g)];
  const literalCommands = [
    ...source.matchAll(/\binvoke(?:<[^>]+>)?\s*\(\s*(["'])([a-z][a-z0-9_]*)\1/g),
  ].map((match) => match[2]);
  assert.equal(
    literalCommands.length,
    invokeCalls.length,
    "every frontend invoke must use a statically classified command name",
  );
  return literalCommands;
}

function extractSupportSessionCommands(source) {
  const wrapper = [
    "const invoke = (command, args) => {",
    "  if (typeof window.__TAURI_INTERNALS__?.invoke !== 'function') return Promise.reject(new Error('native'));",
    "  return window.__TAURI_INTERNALS__.invoke(command, args);",
    "};",
  ].join("\n");
  assert.equal(source.split(wrapper).length, 2, "the session shell must contain exactly one reviewed invoke wrapper");
  return extractFrontendCommands(source.replace(wrapper, ""));
}

const mainFrontendCommands = [appSource, supportPanelSource, supportControlSource]
  .flatMap(extractFrontendCommands);
const supportFrontendCommands = extractSupportSessionCommands(supportSessionSource);

const mainCorePermissions = new Set([
  "core:event:allow-listen",
  "core:event:allow-unlisten",
]);
const previewCorePermissions = new Set(["core:window:allow-set-title"]);
const previewOnlyCommands = new Set(["get_preview_window_target"]);
const mainSupportCommands = new Set([
  "request_support_session", "support_service_status", "support_session_status", "publish_support_frame", "stop_support_session",
]);
const supportCommands = new Set([
  "bootstrap_support_session", "poll_support_frame", "support_submit_action", "stop_support_session",
]);
const capabilityKeys = ["$schema", "description", "identifier", "permissions", "windows"];
const previewCommands = new Set([
  "get_preview_window_target",
  "get_source_eml_view",
  "get_standalone_message_view",
  "save_source_eml_as",
  "save_standalone_source_message_as",
  "save_printable_html_as",
  "reveal_saved_html",
  "reveal_saved_eml",
  "export_attachment",
  "open_attachment",
  "export_standalone_message_attachment",
  "open_standalone_message_attachment",
  "reveal_exported_file_for_workspace",
  "reveal_standalone_exported_file",
]);
const previewSource = appSource.slice(
  appSource.indexOf("function MessagePreviewBootstrap"),
  appSource.indexOf("function App()"),
);
const permissionGeneratorStart = buildSource.indexOf("fn write_app_permissions");
const permissionGeneratorSource = buildSource.slice(permissionGeneratorStart);
const expectedPermissionGeneratorSha256 =
  "75fea422fd830395adb5e7564a49e012a3fd07726a0b089a737593f116f66211";

function assertExactCapabilityEntries(entries) {
  assert.deepEqual(entries, [
    { path: "main.json", kind: "file" },
    { path: "message-preview.json", kind: "file" },
    { path: "support-session.json", kind: "file" },
  ]);
}

function assertExactCapabilityScope(capability, windows, support = false) {
  assert.deepEqual(Object.keys(capability).sort(), support ? [...capabilityKeys, "local"].sort() : capabilityKeys);
  if (support) assert.equal(capability.local, true);
  assert.deepEqual(capability.windows, windows);
  assert.ok(
    capability.permissions.every((permission) => typeof permission === "string"),
    "capability permissions must be plain identifiers without scoped allow/deny objects",
  );
}

test("production CSP fails closed without remote images or active-content origins", () => {
  assert.deepEqual(Object.keys(tauriConfig.app?.security ?? {}).sort(), ["csp"]);
  for (const filename of [
    "Tauri.toml",
    "Tauri.macos.toml",
    "tauri.conf.json5",
    "tauri.macos.conf.json",
    "tauri.macos.conf.json5",
  ]) {
    assert.equal(
      existsSync(new URL(filename, tauriRootUrl)),
      false,
      `production Tauri config override must stay absent: ${filename}`,
    );
  }
  const csp = tauriConfig.app?.security?.csp;
  assert.equal(typeof csp, "string");
  const directives = parseCsp(csp);

  assert.deepEqual(directives, {
    "default-src": ["'none'"],
    "script-src": ["'self'"],
    "style-src": ["'self'", "'unsafe-inline'"],
    "img-src": ["'self'", "data:"],
    "font-src": ["'none'"],
    "connect-src": ["ipc:", "http://ipc.localhost"],
    "object-src": ["'none'"],
    "base-uri": ["'none'"],
    "form-action": ["'none'"],
    "frame-src": ["'none'"],
    "frame-ancestors": ["'none'"],
    "child-src": ["'none'"],
    "worker-src": ["'none'"],
    "media-src": ["'none'"],
    "manifest-src": ["'none'"],
  });

  const cspWithoutIpcTransport = csp
    .toLowerCase()
    .replaceAll("http://ipc.localhost", "");
  assert.doesNotMatch(
    cspWithoutIpcTransport,
    /(?:\*|'unsafe-eval'|https?:|wss?:|127\.0\.0\.1:1420|localhost:1420|@vite\/client)/,
  );
  assert.doesNotMatch(directives["img-src"].join(" "), /https?:|wss?:|\*/);
  assert.equal(devConfig.build?.devUrl, "http://127.0.0.1:1420");
  assert.equal(devConfig.app?.security?.csp, undefined);
});

test("application ACL enumerates exactly the registered Tauri commands", () => {
  const generatedCommands = extractBuildCommands(buildSource);
  const registeredCommands = extractRegisteredCommands(rustSource);
  const frontendCommands = [...mainFrontendCommands, ...supportFrontendCommands];

  assert.ok(generatedCommands.length > 0);
  assert.equal(new Set(generatedCommands).size, generatedCommands.length);
  assert.equal(new Set(registeredCommands).size, registeredCommands.length);
  assert.deepEqual([...generatedCommands].sort(), [...registeredCommands].sort());
  assert.equal((rustSource.match(/tauri::generate_context!/g) ?? []).length, 1);
  assert.match(rustSource, /let context = tauri::generate_context!\(\)/);
  assert.match(rustSource, /RuntimeProfileIdentity::from_config\(context\.config\(\)\)/);
  assert.match(rustSource, /\.build\(context\)/);
  assert.deepEqual(
    [...new Set(frontendCommands)].sort(),
    [...new Set(frontendCommands.filter((command) => generatedCommands.includes(command)))].sort(),
  );
  assert.match(buildSource, /env::var_os\("OUT_DIR"\)/);
  assert.match(
    buildSource,
    /AppManifest::new\(\)\.permissions_path_pattern\(permissions_pattern\)/,
  );
  assert.match(buildSource, /tauri_build::try_build\(/);
  assert.doesNotMatch(buildSource, /tauri_build::build\(\)/);
  assert.doesNotMatch(buildSource, /permissions\/autogenerated/);
  assert.notEqual(permissionGeneratorStart, -1);
  assert.equal(
    createHash("sha256").update(permissionGeneratorSource).digest("hex"),
    expectedPermissionGeneratorSha256,
    "application permission generation and registration must stay at the reviewed one-to-one implementation",
  );
});

test("main, preview, and support capabilities have exact constrained authority", () => {
  const capabilitiesDirectory = lstatSync(capabilitiesPathUrl);
  assert.equal(capabilitiesDirectory.isSymbolicLink(), false);
  assert.equal(capabilitiesDirectory.isDirectory(), true);
  assertExactCapabilityEntries(capabilityEntries);
  assert.deepEqual(capabilityFiles, ["main.json", "message-preview.json", "support-session.json"]);
  assert.deepEqual([...capabilities.keys()].sort(), ["main", "message-preview", "support-session"]);
  assert.equal(
    existsSync(new URL("default.json", capabilitiesUrl)),
    false,
    "the overlapping default capability must stay removed",
  );

  const main = capabilities.get("main");
  const preview = capabilities.get("message-preview");
  const support = capabilities.get("support-session");
  assertExactCapabilityScope(main, ["main"]);
  assertExactCapabilityScope(preview, ["message-preview-*"]);
  assertExactCapabilityScope(support, ["support-session-*"], true);

  const generatedCommands = extractBuildCommands(buildSource);
  const frontendCommands = new Set(mainFrontendCommands);
  const allFrontendCommands = new Set([...mainFrontendCommands, ...supportFrontendCommands]);
  const expectedMainPermissions = new Set([
    ...mainCorePermissions,
    ...[...frontendCommands]
      .filter((command) => !previewOnlyCommands.has(command))
      .map(appPermission),
  ]);
  const expectedPreviewPermissions = new Set([
    ...previewCorePermissions,
    ...[...previewCommands].map(appPermission),
  ]);
  assert.deepEqual(new Set(main.permissions), expectedMainPermissions);
  assert.deepEqual(new Set(preview.permissions), expectedPreviewPermissions);
  assert.deepEqual(new Set(support.permissions), new Set([...supportCommands].map(appPermission)));
  assert.equal(new Set(main.permissions).size, main.permissions.length);
  assert.equal(new Set(preview.permissions).size, preview.permissions.length);
  assert.equal(new Set(support.permissions).size, support.permissions.length);
  assert.deepEqual(new Set(supportFrontendCommands), supportCommands);
  assert.deepEqual(new Set([supportPanelSource, supportControlSource].flatMap(extractFrontendCommands)), mainSupportCommands);
  assert.deepEqual(
    [...generatedCommands.filter((command) => !allFrontendCommands.has(command))].sort(),
    ["export_original_eml", "reveal_exported_file", "start_support_session"],
    "registered but unused commands must not leak into a window capability",
  );
  assert.deepEqual(
    new Set(extractFrontendCommands(previewSource)),
    previewCommands,
    "preview command use and preview capability must stay in lockstep",
  );

  for (const capability of [main, preview, support]) {
    assert.equal(capability.permissions.includes("core:default"), false);
    assert.equal(capability.permissions.some((permission) => permission.includes("*")), false);
    assert.equal(
      capability.permissions.some(
        (permission) => permission.includes("shell") || permission.includes("fs:"),
      ),
      false,
    );
  }
  for (const permission of mainCorePermissions) {
    assert.equal(preview.permissions.includes(permission), false);
    assert.equal(support.permissions.includes(permission), false);
  }
  for (const command of supportCommands) {
    assert.equal(main.permissions.includes(appPermission(command)), command === "stop_support_session");
    assert.equal(preview.permissions.includes(appPermission(command)), false);
  }
  for (const command of mainSupportCommands) {
    assert.equal(preview.permissions.includes(appPermission(command)), false);
    assert.equal(support.permissions.includes(appPermission(command)), command === "stop_support_session");
  }
  assert.equal(
    preview.permissions.includes("core:webview:allow-create-webview-window"),
    false,
  );
  assert.equal(main.permissions.includes("core:webview:allow-create-webview-window"), false);
  for (const capability of [main, preview, support]) assert.equal(capability.permissions.includes("allow-start-support-session"), false);
  assert.equal(main.permissions.includes("allow-get-preview-window-target"), false);
  assert.equal(preview.permissions.includes("allow-open-message-preview-window"), false);
  assert.equal(main.permissions.includes("allow-open-message-preview-window"), true);
  assert.doesNotMatch(appSource, /new WebviewWindow|WebviewWindow\.getByLabel/);
  assert.match(rustSource, /WebviewWindowBuilder::new\(/);
  assert.match(rustSource, /WebviewUrl::App\(PathBuf::from\(PREVIEW_WINDOW_ROUTE\)\)/);
  assert.match(rustSource, /const PREVIEW_WINDOW_ROUTE: &str = "index\.html\?messagePreview=1"/);
  assert.match(rustSource, /if window\.label\(\) != "main"/);
  assert.match(rustSource, /const MAX_PREVIEW_WINDOWS: usize = 32/);
  assert.match(appSource, /kind: "standalone",\s*stableId: file\.stableId/);
  assert.doesNotMatch(appSource, /standaloneMessagePath=|previewWorkspaceId=/);
});

test("capability validation rejects alternate files and hidden authority scopes", () => {
  const main = capabilities.get("main");
  for (const path of ["nested/extra.json", "extra.toml", "extra.json5"]) {
    assert.throws(() =>
      assertExactCapabilityEntries([...capabilityEntries, { path, kind: "file" }]),
    );
  }
  assert.throws(() => assertExactCapabilityScope({ ...main, webviews: ["*"] }, ["main"]));
  assert.throws(() =>
    assertExactCapabilityScope(
      { ...main, remote: { urls: ["https://example.invalid"] } },
      ["main"],
    ),
  );
  assert.throws(() =>
    assertExactCapabilityScope(
      { ...main, permissions: [{ identifier: "core:window:allow-show", allow: [{}] }] },
      ["main"],
    ),
  );
  const support = capabilities.get("support-session");
  assert.throws(() => assertExactCapabilityScope({ ...support, local: false }, ["support-session-*"], true));
  assert.throws(() => assertExactCapabilityScope({ ...support, remote: { urls: ["https://example.invalid"] } }, ["support-session-*"], true));
});

test("module command paths and session invocation classification reject ambiguous syntax", () => {
  const handler = (body) => `.invoke_handler(tauri::generate_handler![${body}])`;
  assert.deepEqual(extractRegisteredCommands(handler("plain, support_session::bootstrap_support_session,")), ["plain", "bootstrap_support_session"]);
  for (const body of ["support_session::", "plain(),", "plain + other,", "plain,,other", "arbitrary!()"])
    assert.throws(() => extractRegisteredCommands(handler(body)));
  assert.throws(() => extractFrontendCommands('invoke(command, {})'));
  assert.throws(() => extractSupportSessionCommands(supportSessionSource.replace("invoke(command, args)", "invoke(command, arbitrary)")));
});

test("support CSP exception is restricted to the fixed local session document and verified origin", () => {
  const csp = sourceSection(supportRustSource, "fn support_csp(", "\nfn take_session(");
  assert.match(csp.replace(/\s+/g, " "), /if !\(allowed_origin\(origin\) \|\| \(allowed_queue_origin\(origin\) && pinned_origin == Some\(origin\)\)\)/);
  assert.match(csp, /let pinned_origin = service_config\(\)\.ok\(\)\.map\(\|config\| config\.origin\)/);
  assert.match(csp, /support_csp_for_service\(existing, origin, pinned_origin\.as_deref\(\)\)/);
  assert.match(csp, /directive\.split_whitespace\(\)\.next\(\) != Some\("connect-src"\)/);
  assert.match(csp, /directives\.push\(format!\("connect-src ipc: http:\/\/ipc\.localhost \{origin\}"\)\)/);
  assert.match(csp, /Some\(directives\.join\("; "\)\)/);
  const startMarker = ".on_web_resource_request(move |request, response| {";
  const endMarker = /^[ \t]*\}\)[ \t]*\r?\n[ \t]*\.build\(\);[ \t]*\r?$/m;
  const handler = sourceSection(supportRustSource, startMarker, endMarker);
  const fallbacks = [...handler.matchAll(/\.unwrap_or\(\s*("(?:[^"\\]|\\.)*")\s*\)/g)];
  assert.equal(fallbacks.length, 1);
  assert.equal(JSON.parse(fallbacks[0][1]), supportFallbackCsp);
  assert.ok(handler.indexOf("if !local_document(&url, &resource_id)") < handler.indexOf("support_csp(existing, &origin)"));
  for (const indent of ["    ", "\t"]) {
    const formatted = supportRustSource.replace(endMarker, `${indent}})\n${indent}.build();`);
    assert.equal(sourceSection(formatted, startMarker, endMarker), handler);
  }
  assert.throws(() => sourceSection(supportRustSource.replace(endMarker, "    })\n    .build(); arbitrary"), startMarker, endMarker));
  assert.match(supportRustSource, /const DOCUMENT: &str = "\/support\/session\.html"/);
  assert.match(supportRustSource, /strip_suffix\("\.run\.pinggy-free\.link"\)/);
  assert.match(supportRustSource, /\.on_navigation\(move \|url\| local_document\(url, &navigation_id\)\)/);
  assert.match(supportRustSource, /NewWindowResponse::Deny/);
  assert.doesNotMatch(supportRustSource.split("\n#[cfg(test)]")[0], /WebviewUrl::External|unsafe-eval|script-src \*/);
  assert.equal(capabilities.get("support-session").remote, undefined);
});

test("standalone message and export actions use backend-held file authority", () => {
  assert.match(
    rustSource,
    /fn get_standalone_message_view\(\s*state: State<AppState>,\s*stable_id: String/,
  );
  assert.match(rustSource, /fn authorized_standalone_path\(state: &AppState, stable_id: &str\)/);
  assert.match(rustSource, /struct AuthorizedStandaloneFile/);
  assert.match(rustSource, /struct ExportAuthorizationRegistry/);
  assert.match(rustSource, /fn authorized_export_path\(state: &AppState, requested_path: &str\)/);
  assert.match(rustSource, /\.create_new\(true\)/);
  assert.match(rustSource, /\.mode\(PRIVATE_FILE_MODE\)/);
  assert.match(appSource, /kind: "standalone"; stableId: string/);
  assert.match(appSource, /"get_standalone_message_view", \{\s*stableId/);
  assert.match(appSource, /"export_standalone_message_attachment", \{\s*stableId/);
  assert.match(appSource, /"open_standalone_message_attachment", \{\s*stableId/);
  assert.match(rustSource, /RunEvent::WebviewEvent/);
  assert.match(rustSource, /DragDropEvent::Drop \{ paths, \.\. \}/);
  assert.doesNotMatch(appSource, /prepare_external_file_opens|onDragDropEvent/);
  assert.doesNotMatch(buildSource, /prepare_external_file_opens/);
  assert.doesNotMatch(
    appSource,
    /"(?:get_standalone_message_view|export_standalone_message_attachment|open_standalone_message_attachment)", \{\s*path:/,
  );
});

test("feature progress probes harden every read-only SQLite connection before querying", () => {
  const countProbe = sourceSection(
    rustSource,
    "fn read_index_progress_counts(",
    "fn mark_workspace_status(",
  );
  const countConfigIndex = countProbe.indexOf("configure_sqlite_connection(&conn, true)");
  const countQueryIndex = countProbe.indexOf('.query_row("SELECT COUNT(*) FROM messages"');
  assert.notEqual(countConfigIndex, -1);
  assert.notEqual(countQueryIndex, -1);
  assert.ok(countConfigIndex < countQueryIndex);

  const legacyCandidateProbe = sourceSection(
    importIntegrityPhase2Source,
    "let discovered = if index_path.is_file() {",
    "// A legacy database status is not completion proof.",
  );
  const candidateConfigIndex = legacyCandidateProbe.indexOf(
    "configure_sqlite_connection(&conn, true)?",
  );
  const candidateQueryIndex = legacyCandidateProbe.indexOf(
    'metadata_value(&conn, "candidates_discovered")',
  );
  assert.notEqual(candidateConfigIndex, -1);
  assert.notEqual(candidateQueryIndex, -1);
  assert.ok(candidateConfigIndex < candidateQueryIndex);
});

test("ReadPST execution is bundled, isolated, bounded, and source preserving", () => {
  assert.match(rustSource, /fn find_readpst\(\) -> Option<ReadpstLocation> \{\s*find_bundled_readpst\(\)/);
  assert.doesNotMatch(rustSource, /\/usr\/local\/bin\/readpst|\/opt\/homebrew\/bin\/readpst/);
  assert.doesNotMatch(rustSource, /env::split_paths|ReadpstSource::System/);
  assert.match(rustSource, /READPST_X86_64_SHA256/);
  assert.match(rustSource, /READPST_ARM64_SHA256/);
  assert.match(rustSource, /READPST_UNIVERSAL_SHA256/);
  assert.match(rustSource, /is_verified_bundled_readpst_file/);
  assert.match(rustSource, /\.env_clear\(\)/);
  assert.match(rustSource, /\.process_group\(0\)/);
  assert.match(rustSource, /\.arg\("--"\)\s*\.arg\(pst_path\)/);
  assert.match(rustSource, /MAX_READPST_STREAM_BYTES/);
  assert.match(rustSource, /MAX_READPST_EXTRACTED_ENTRIES/);
  assert.match(rustSource, /MAX_READPST_EXTRACTED_FILE_BYTES/);
  assert.doesNotMatch(rustSource, /READPST_STALL_TIMEOUT/);
  assert.match(rustSource, /fn readpst_running_process_state/);
  assert.match(rustSource, /ProcessState::RunningNoRecentSignal/);
  assert.match(rustSource, /READPST_ABSOLUTE_TIMEOUT/);
  assert.match(rustSource, /fn inspect_readpst_output/);
  assert.match(rustSource, /fn remove_readpst_output_directory/);
  assert.match(rustSource, /let source_before = file_identity\(pst_path, "source PST"\)/);
  assert.match(rustSource, /fn verify_source_identity_unchanged/);
  assert.match(rustSource, /verify_source_identity_unchanged\(pst_path, &source_before\)/);
  assert.doesNotMatch(rustSource, /\.stdout\(Stdio::from\(stdout\)\)/);
});

test("release verification independently enforces the Tauri security boundary", () => {
  assert.match(releaseVerifier, /capabilities_dir\.is_symlink\(\)/);
  assert.match(releaseVerifier, /capabilities_dir\.rglob\("\*"\)/);
  assert.match(releaseVerifier, /expected_entries = \{/);
  assert.match(releaseVerifier, /expected_capability_keys = \{/);
  assert.match(releaseVerifier, /expected_permission_generator_sha256 =/);
  assert.match(releaseVerifier, /production Tauri config override is present/);
  assert.match(releaseVerifier, /TAURI_CONFIG build override is set/);
  assert.match(releaseVerifier, /reviewed merged Tauri context/);
  assert.match(releaseVerifier, /application-command ACL drifted/);
  assert.match(releaseVerifier, /production CSP is not the exact reviewed policy/);
  assert.match(releaseVerifier, /verify-packaged-csp\.py/);
  assert.match(releaseVerifier, /CFBundleIdentifier/);
  assert.match(packagedCspVerifier, /APP_ARCHITECTURES = \("x86_64", "arm64"\)/);
  assert.match(packagedCspVerifier, /TemporaryDirectory\(prefix="pst-quickview-csp-"\)/);
  assert.match(packagedCspVerifier, /PRINTABLE_CSP = \(/);
  assert.match(
    packagedCspVerifier,
    /default-src 'none'; img-src data:; style-src 'unsafe-inline'/,
  );
  assert.match(packagedCspVerifier, /CSP_DIRECTIVE_PATTERN = re\.compile/);
  assert.match(packagedCspVerifier, /CSP_HEADER_PATTERN\.finditer\(binary\)/);
  assert.match(releaseVerifier, /preview windows must not create webviews/);
  assert.doesNotMatch(releaseVerifier, /capabilities\/default\.json/);
});

test("release source gate independently accepts the exact three-window boundary", () => {
  const marker = 'if python3 - "${CAPABILITIES_DIR}" "${TAURI_CONFIG}" "${BUILD_SCRIPT}" "${RUST_SOURCE}" "${APP_SOURCE}"';
  const start = releaseVerifier.indexOf(marker);
  assert.notEqual(start, -1);
  const bodyStart = releaseVerifier.indexOf("<<'PY'\n", start) + "<<'PY'\n".length;
  const end = releaseVerifier.indexOf("\nPY\nthen", bodyStart);
  assert.ok(bodyStart > start && end > bodyStart);
  const body = releaseVerifier.slice(bodyStart, end);
  const args = [
    fileURLToPath(capabilitiesPathUrl), fileURLToPath(tauriConfigUrl), fileURLToPath(buildScriptUrl),
    fileURLToPath(rustSourceUrl), fileURLToPath(appSourceUrl)];
  const result = spawnSync("python3", ["-c", body, ...args], { encoding: "utf8" });
  assert.equal(result.status, 0, result.stderr);
  for (const changed of [
    supportRustSource.replace(supportFallbackCsp, supportFallbackCsp.replace("script-src 'self'", "script-src https:")),
    supportRustSource.replace('Some(directives.join("; "))', 'Some(directives.join(";"))'),
    supportRustSource.replace('directives.push(format!("connect-src ipc: http://ipc.localhost {origin}"))', 'directives.push(format!("connect-src * {origin}"))'),
    supportRustSource.replace("if !(allowed_origin(origin)", "if false && (allowed_origin(origin)"),
    supportRustSource.replace("pinned_origin == Some(origin)", "true"),
    supportRustSource.replace("pinned_origin.as_deref()", 'Some(origin)'),
    supportRustSource.replace("if !local_document(&url, &resource_id)", "if false"),
  ]) {
    const overridden = body.replace("support_csp_start =", `support_rust_source = ${JSON.stringify(changed)}\nsupport_csp_start =`);
    assert.notEqual(spawnSync("python3", ["-c", overridden, ...args], { encoding: "utf8" }).status, 0);
  }
});

test("packaged CSP inventory rejects missing, duplicate, and unreviewed policies", () => {
  const productionCsp = tauriConfig.app.security.csp;
  const printableCsp =
    "default-src 'none'; img-src data:; style-src 'unsafe-inline'; " +
    "object-src 'none'; base-uri 'none'; form-action 'none'; " +
    "frame-src 'none'; connect-src 'none'";
  const headers =
    "Content-Security-Policy\0Content-Security-Policy\0" +
    "content-security-policy\0content-security-policy\0" +
    "content-security-policy\0content-security-policy\0";
  const exact = Buffer.from(`${headers}${productionCsp}\0${printableCsp}${supportLiteralInventory()}`);

  assert.equal(verifySyntheticPackagedCsp(exact).status, 0);
  for (const rejected of [
    Buffer.from(`${headers}${printableCsp}${supportLiteralInventory()}`),
    Buffer.from(`${headers}${productionCsp}\0${productionCsp}\0${printableCsp}${supportLiteralInventory()}`),
    Buffer.from(`${headers}x${productionCsp}\0${printableCsp}${supportLiteralInventory()}`),
    Buffer.from(exact.toString().replace(printableCsp, `${printableCsp}; script-src-elem https:`)),
    Buffer.from(exact.toString().replace(printableCsp, `${printableCsp} https:`)),
    Buffer.from(exact.toString().replace(printableCsp, `${printableCsp}; future-directive value`)),
    Buffer.concat([exact, Buffer.from("\0navigate-to https:")]),
    Buffer.concat([exact, Buffer.from("\0default-src\t'none'")]),
    Buffer.concat([exact, Buffer.from("\0ScRiPt-SrC-ElEm https:")]),
    Buffer.concat([exact, Buffer.from("\0SaNdBoX;")]),
    Buffer.concat([exact, Buffer.from("\0SaNdBoX,")]),
    Buffer.concat([
      exact,
      Buffer.from("\0Content-Security-Policy\0future-directive value"),
    ]),
    Buffer.concat([
      exact,
      Buffer.from("\0content-security-policy\0FuTuRe-DiReCtIvE value"),
    ]),
  ]) {
    assert.notEqual(verifySyntheticPackagedCsp(rejected).status, 0);
  }
});

test("packaged CSP boundary retains only reviewed source prefixes for alternate bundles", () => {
  const productionCsp = tauriConfig.app.security.csp;
  const printableCsp =
    "default-src 'none'; img-src data:; style-src 'unsafe-inline'; " +
    "object-src 'none'; base-uri 'none'; form-action 'none'; " +
    "frame-src 'none'; connect-src 'none'";
  const headers =
    "Content-Security-Policy\0Content-Security-Policy\0" +
    "content-security-policy\0content-security-policy\0" +
    "content-security-policy\0content-security-policy\0";
  const reviewedPrefix = "PST QuickView";
  const observedAlternateSlice = Buffer.from(
    `${headers}${reviewedPrefix}${productionCsp}\0${printableCsp}${supportLiteralInventory("x86_64")}`,
  );
  const bundleNamePrefix = Buffer.from(
    `${headers}PST QuickView Beta4 Runtime Test${productionCsp}\0${printableCsp}${supportLiteralInventory("x86_64")}`,
  );

  assert.equal(
    verifySyntheticPackagedCsp(observedAlternateSlice, "x86_64").status,
    0,
  );
  assert.notEqual(
    verifySyntheticPackagedCsp(bundleNamePrefix, "x86_64").status,
    0,
  );
  assert.equal(
    verifySyntheticPackagedCsp(Buffer.from(observedAlternateSlice.toString().replace(supportLiteralInventory("x86_64"), supportLiteralInventory("arm64"))), "arm64").status,
    0,
  );
  const armEntryPointPrefix = Buffer.from(
    `${headers}index.html${productionCsp}\0${printableCsp}${supportLiteralInventory("arm64")}`,
  );
  assert.equal(
    verifySyntheticPackagedCsp(armEntryPointPrefix, "arm64").status,
    0,
  );
  assert.notEqual(
    verifySyntheticPackagedCsp(Buffer.from(bundleNamePrefix.toString().replace(supportLiteralInventory("x86_64"), supportLiteralInventory("arm64"))), "arm64").status,
    0,
  );
});

test("packaged support inventory pins exact literals, architecture neighbors, and token coverage", () => {
  const productionCsp = tauriConfig.app.security.csp;
  const printableCsp = "default-src 'none'; img-src data:; style-src 'unsafe-inline'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; connect-src 'none'";
  const headers = "Content-Security-Policy\0".repeat(2) + "content-security-policy\0".repeat(4);
  for (const arch of ["x86_64", "arm64"]) {
    const exact = `${headers}${productionCsp}\0${printableCsp}${supportLiteralInventory(arch)}`;
    assert.equal(verifySyntheticPackagedCsp(Buffer.from(exact), arch).status, 0);
    for (const rejected of [
      exact.replace(supportFallbackCsp, ""),
      `${exact}\0${supportFallbackCsp}`,
      exact.replace(supportFallbackCsp, supportFallbackCsp.replace("script-src 'self'", "script-src https:")),
      exact.replace(supportConnectPrefix, ""),
      `${exact}\0${supportConnectPrefix}`,
      exact.replace(supportConnectPrefix, "connect-src https://example.invalid "),
      exact.replace(`Content-Security-Policy${supportFallbackCsp}`, `Wrong-Neighbor${supportFallbackCsp}`),
      exact.replace(`${supportFallbackCsp}Cache-Control`, `${supportFallbackCsp}Wrong-Neighbor`),
      exact.replace(`${supportFallbackCsp}Cache-Control`, `${supportFallbackCsp} https://example.invalid Cache-Control`),
      exact.replace(`${supportConnectPrefix}; `, `${supportConnectPrefix}https://example.invalid; `),
      exact.replace(`${supportConnectPrefix}; `, `${supportConnectPrefix};`),
      arch === "arm64" ? exact.replace(`tauri${supportConnectPrefix}`, `wrong${supportConnectPrefix}`) : exact.replace(`\0${supportConnectPrefix}`, `x${supportConnectPrefix}`),
      `${exact}\0connect-src`,
      `${exact}\0embeddedconnect-srcword`,
      `${exact}\0CoNnEcT-SrC`,
      `${exact}\0script-src https:`,
      `${exact}\0ScRiPt-SrC-ElEm https:`,
      `${exact}\0Content-Security-Policy`,
      exact.replace("content-security-policy", "Content-Security-Policy"),
      exact.replace("content-security-policy", "CoNtEnT-SeCuRiTy-PoLiCy"),
    ]) assert.notEqual(verifySyntheticPackagedCsp(Buffer.from(rejected), arch).status, 0);
    assert.notEqual(verifySyntheticPackagedCsp(Buffer.from(exact), arch === "arm64" ? "x86_64" : "arm64").status, 0);
  }
});
