import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
const workflow = readFileSync(new URL('../.github/workflows/support-development.yml', import.meta.url), 'utf8');
test('development Mac build identity increments within the three significant bundle-version components', () => {
  // Apple ignores additional CFBundleVersion components; a fourth-component
  // increment would not identify a new build to the operating system.
  const config = JSON.parse(readFileSync(new URL('../src-tauri/tauri.conf.json', import.meta.url), 'utf8'));
  const bundleVersion = config.bundle.macOS.bundleVersion;
  assert.match(bundleVersion, /^\d+\.\d+\.\d+$/);
  assert.equal(bundleVersion, '0.2.302');
  assert.equal(config.version, '0.2.0-beta.3.support.2');
  assert.ok(Number(bundleVersion.split('.')[2]) > 0);
});
test('development workflow is Mac app-only, unsigned, and has no release or tag authority', () => {
  assert.match(workflow, /runs-on: macos-15/);
  assert.match(workflow, /branches: \[codex\/remote-support-development\]/);
  assert.match(workflow, /contents: read/);
  assert.match(workflow, /persist-credentials: false/);
  assert.match(workflow, /--target universal-apple-darwin --bundles app --no-sign --ci/);
  assert.match(workflow, /cargo test --locked --manifest-path scripts\/support-update-verifier\/Cargo.toml/);
  assert.match(workflow, /cargo clippy --locked --manifest-path scripts\/support-update-verifier\/Cargo.toml --all-targets -- -D warnings/);
  assert.match(workflow, /EXPECT_DMG=0 bash scripts\/verify-macos-release.sh/);
  assert.match(workflow, /rustc --edition=2021 scripts\/verify-update-bundle.rs/);
  assert.match(workflow, /pst-update-bundle-check.*PST QuickView\.app\/Contents\/MacOS\/pst-quickview/);
  assert.match(workflow, /ditto -c -k --sequesterRsrc --keepParent/);
  assert.match(workflow, /COPYFILE_DISABLE=1 tar -czf/);
  assert.doesNotMatch(workflow, /\.dmg|notarytool|altool|codesign |contents: write|gh release|git push|npm version/);
  const verification = workflow.indexOf('EXPECT_DMG=0');
  const artifact = workflow.indexOf('uses: actions/upload-artifact');
  assert.ok(verification > 0 && artifact > verification);
  assert.doesNotMatch(workflow, /continue-on-error|if: always\(\)/);
  assert.match(workflow, /if cargo fmt --check; then exit 0; fi\s+cargo fmt\s+git diff --no-ext-diff -- '\*\.rs'\s+exit 1/);
  const diagnostic = workflow.indexOf('python3 scripts/inspect-packaged-csp.py');
  assert.ok(diagnostic > verification && diagnostic < artifact);
  assert.match(workflow, /\|\| package_status=\$\?/);
  assert.match(workflow, /\|\| diagnostic_status=\$\?/);
  assert.match(workflow, /if \(\(package_status != 0 \|\| diagnostic_status != 0\)\); then exit 1; fi/);
});

test('failed security gates retain only the current executable as clearly labelled diagnostics', () => {
  const diagnostic = workflow.slice(workflow.indexOf('- name: Retain current executable'));
  assert.match(diagnostic, /if: \$\{\{ failure\(\) \}\}/);
  assert.match(diagnostic, /name: support-security-diagnostic-not-for-distribution/);
  assert.match(diagnostic, /path: src-tauri\/target\/universal-apple-darwin\/release\/bundle\/macos\/PST QuickView\.app\/Contents\/MacOS\/pst-quickview/);
  assert.match(diagnostic, /retention-days: 1/);
  assert.doesNotMatch(diagnostic, /\.app\.tar\.gz|\.zip|operator\.json|signing\.key/);
});
