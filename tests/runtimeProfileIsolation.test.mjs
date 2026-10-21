import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync } from "node:fs";
import { extname } from "node:path";

const backendUrl = new URL("../src-tauri/src/lib.rs", import.meta.url);
const profileUrl = new URL("../src-tauri/src/runtime_profile.rs", import.meta.url);
const rustRootUrl = new URL("../src-tauri/src/", import.meta.url);
const frontendUrl = new URL("../src/App.tsx", import.meta.url);
const localTrashUrl = new URL("../src-tauri/src/local_trash.rs", import.meta.url);
const packageUrl = new URL("../package.json", import.meta.url);
const tauriConfigUrl = new URL("../src-tauri/tauri.conf.json", import.meta.url);
const cargoTomlUrl = new URL("../src-tauri/Cargo.toml", import.meta.url);

const backend = readFileSync(backendUrl, "utf8");
const profile = readFileSync(profileUrl, "utf8");
const backendImplementation = backend.slice(0, backend.lastIndexOf("#[cfg(test)]\nmod tests {"));
const profileImplementation = profile.slice(0, profile.indexOf("#[cfg(test)]\nmod tests {"));
const frontend = readFileSync(frontendUrl, "utf8");
const localTrash = readFileSync(localTrashUrl, "utf8");
const packageJson = JSON.parse(readFileSync(packageUrl, "utf8"));
const tauriConfig = JSON.parse(readFileSync(tauriConfigUrl, "utf8"));
const cargoToml = readFileSync(cargoTomlUrl, "utf8");

function rustSources(directoryUrl, prefix = "") {
  return readdirSync(directoryUrl, { withFileTypes: true }).flatMap((entry) => {
    const relativePath = prefix ? `${prefix}/${entry.name}` : entry.name;
    const url = new URL(`${encodeURIComponent(entry.name)}${entry.isDirectory() ? "/" : ""}`, directoryUrl);
    if (entry.isDirectory()) return rustSources(url, relativePath);
    if (extname(entry.name) !== ".rs") return [];
    return [{ path: relativePath, source: readFileSync(url, "utf8") }];
  });
}

function sourceSection(source, startMarker, endMarker) {
  const start = source.indexOf(startMarker);
  assert.notEqual(start, -1, `missing source marker: ${startMarker}`);
  const end = source.indexOf(endMarker, start + startMarker.length);
  assert.notEqual(end, -1, `missing source marker: ${endMarker}`);
  return source.slice(start, end);
}

test("runtime profile identity and paths have one merged-config authority", () => {
  assert.match(profile, /struct RuntimeProfileIdentity/);
  assert.match(profile, /struct RuntimeProfilePaths/);
  assert.match(profile, /struct RuntimeProfileState\(OnceLock<RuntimeProfile>\)/);
  assert.match(profile, /RuntimeProfileIdentity::from_config\(app\.config\(\)\)/);
  assert.match(
    profile,
    /pst-quickview-runtime-identity-v1:[\s\S]*TAURI_ANDROID_PACKAGE_NAME_PREFIX[\s\S]*TAURI_ANDROID_PACKAGE_NAME_APP_NAME/,
  );
  for (const resolver of [
    "app_data_dir",
    "app_log_dir",
    "app_cache_dir",
    "app_config_dir",
  ]) {
    assert.match(profile, new RegExp(`\\.${resolver}\\(\\)`));
  }
  assert.match(profile, /PRODUCTION_BUNDLE_IDENTIFIER: &str = "com\.local\.pstquickview"/);
  assert.match(profile, /profile_layout_generation/);
  assert.match(profile, /RuntimeProfileFlavor::ProductionLegacy/);
  assert.match(profile, /RuntimeProfileFlavor::Alternate/);
  assert.match(profile, /LEGACY_PRODUCTION_PROFILE_DIRECTORY: &str = "PST QuickView"/);
  assert.match(profile, /AlternateUsesProductionPath/);
});

test("profile roots cannot be reconstructed outside the approved module", () => {
  assert.doesNotMatch(backendImplementation, /fn app_support_dir\s*\(/);
  assert.doesNotMatch(backendImplementation, /env::var_os\("HOME"\)/);
  assert.doesNotMatch(backendImplementation, /\.join\("Application Support"\)/);
  assert.doesNotMatch(backendImplementation, /\.join\((?:APP_SUPPORT_NAME|PRODUCT_DISPLAY_NAME)\)/);

  const forbiddenResolvers = /\.app_(?:data|log|cache|config)_dir\s*\(\s*\)/;
  for (const source of rustSources(rustRootUrl)) {
    if (source.path === "runtime_profile.rs") continue;
    const testModule = source.source.indexOf("#[cfg(test)]\nmod tests {");
    const implementation =
      testModule === -1 ? source.source : source.source.slice(0, testModule);
    assert.doesNotMatch(
      implementation,
      forbiddenResolvers,
      `${source.path} independently resolves a runtime profile root`,
    );
    assert.doesNotMatch(
      implementation,
      /env::(?:var|var_os)\("HOME"\)|\.join\("Application Support"\)|Library\/Application Support\/PST QuickView|\.join\("PST QuickView"\)|\.join\((?:APP_SUPPORT_NAME|PRODUCT_DISPLAY_NAME)\)/,
      `${source.path} reconstructs the production profile outside the approved module`,
    );
  }

  assert.match(backend, /fn workspaces_dir\(profile_paths: &RuntimeProfilePaths\)/);
  assert.match(backend, /fn standalone_exports_dir\([\s\S]*profile_paths: &RuntimeProfilePaths/);
  assert.match(backend, /fn append_application_log\(\s*profile_paths: &RuntimeProfilePaths/);
});

test("startup validates merged identity and initializes state before file logging", () => {
  const run = sourceSection(backend, "pub fn run()", "#[tauri::command]\nfn check_readpst");
  const context = run.indexOf("let context = tauri::generate_context!()");
  const prevalidation = run.indexOf("RuntimeProfileIdentity::from_config(context.config())");
  const builder = run.indexOf("tauri::Builder::default()");
  const setup = run.indexOf(".setup(move |app|");
  const resolution = run.indexOf("RuntimeProfile::resolve(app, &expected_identity)", setup);
  const initialization = run.indexOf("initialize_runtime_profile(profile)", resolution);
  const logging = run.indexOf("append_application_log(profile.paths()", initialization);

  assert.ok(context >= 0 && context < prevalidation);
  assert.ok(prevalidation < builder && builder < setup);
  assert.ok(setup < resolution && resolution < initialization && initialization < logging);
  assert.doesNotMatch(run.slice(0, setup), /append_application_log\s*\(/);
  assert.match(run, /\.build\(context\)/);
  assert.doesNotMatch(profileImplementation, /std::env|var_os|var\(|profile.*override/i);
});

test("registry, session validation, exports, and logs use the resolved profile", () => {
  assert.match(profile, /workspace_registry_session_root: application_data_root\.join\(WORKSPACES_DIRECTORY\)/);
  assert.match(profile, /standalone_eml_export_root: application_data_root\.join\(EML_EXPORTS_DIRECTORY\)/);
  assert.match(profile, /standalone_msg_export_root: application_data_root\.join\(MSG_EXPORTS_DIRECTORY\)/);
  assert.match(profile, /diagnostics_root: log_root\.clone\(\)/);

  const restore = sourceSection(
    backend,
    "fn open_existing_workspace_from_session(",
    "#[tauri::command]\nfn activate_workspace",
  );
  assert.match(restore, /state\.runtime_profile\(\)\?\.paths\(\)/);
  assert.match(restore, /validate_session_workspace_path/);
  assert.ok(
    restore.indexOf("validate_session_workspace_scope_before_access") <
      restore.indexOf(".canonicalize()"),
  );
  const validation = sourceSection(
    backend,
    "fn validate_session_workspace_path(",
    "fn validate_session_workspace_scope_before_access(",
  );
  assert.ok(
    validation.indexOf("outside the PST QuickView workspace root") <
      validation.indexOf("read_workspace_marker"),
  );
  const workspaceRoot = sourceSection(
    backend,
    "fn workspaces_dir(",
    "fn standalone_exports_dir(",
  );
  assert.ok(
    workspaceRoot.indexOf("application_data_root()") <
      workspaceRoot.indexOf("workspace_registry_session_root()"),
  );
  assert.doesNotMatch(backend, /fn (?:list_recent|get_recent|get_saved_session|list_sessions)\b/);
  assert.match(frontend, /window\.localStorage/);
});

test("frontend and runtime environment cannot choose a backend profile path", () => {
  assert.doesNotMatch(frontend, /(?:profilePath|appDataPath|logRoot|cacheRoot|configRoot)/);
  assert.doesNotMatch(profile, /(?:profile_path|app_data_path|log_root_override|cache_root_override)/i);
  assert.doesNotMatch(profile, /PathBuf::from\([^)]*(?:product_name|bundle_identifier)/);

  const commands = [...backend.matchAll(/#\[tauri::command\][\s\S]*?\nfn\s+([a-z0-9_]+)\s*\(([^)]*)\)/g)];
  for (const [, name, parameters] of commands) {
    assert.doesNotMatch(
      parameters,
      /(?:profile|app_data|log_root|cache_root|config_root).*String/i,
      `${name} exposes profile routing to the frontend`,
    );
  }
});

test("next-to-PST and Local Trash remain workspace-local", () => {
  assert.match(backend, /NEXT_TO_PST_WORKSPACE_DIR: &str = "\.pst-quickview\.noindex"/);
  assert.match(backend, /LEGACY_NEXT_TO_PST_WORKSPACE_DIR: &str = "\.pst-quickview"/);
  assert.match(backend, /canonical_path[\s\S]*\.parent\(\)[\s\S]*NEXT_TO_PST_WORKSPACE_DIR/);
  assert.doesNotMatch(profile, /NEXT_TO_PST_WORKSPACE_DIR|\.pst-quickview\.noindex/);
  assert.match(localTrash, /workspace\.join\(LOCAL_TRASH_FILE_NAME\)/);
  assert.doesNotMatch(localTrash, /Application Support|app_data_dir|app_log_dir/);
});

test("runtime-profile fix preserves beta.3 version and source contracts", () => {
  assert.equal(packageJson.version, "0.2.0-beta.3");
  assert.equal(tauriConfig.version, "0.2.0-beta.3");
  assert.equal(tauriConfig.identifier, "com.local.pstquickview");
  assert.equal(tauriConfig.productName, "PST QuickView");
  assert.match(cargoToml, /^version = "0\.2\.0-beta\.3"$/m);
  assert.doesNotMatch(profileImplementation, /["'][^"']*\.(?:pst|eml|msg)["']/);
  assert.doesNotMatch(profileImplementation, /fs::(?:remove_file|remove_dir|rename)\s*\(/);
});
