import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const backend = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const rustTests = readFileSync(
  new URL("../src-tauri/src/import_integrity_tests.rs", import.meta.url),
  "utf8",
);
const policy = readFileSync(new URL("../docs/IMPORT_INTEGRITY.md", import.meta.url), "utf8");
const loggingPolicy = readFileSync(new URL("../docs/LOGGING.md", import.meta.url), "utf8");

test("import progress reports processed candidates rather than canonical rows", () => {
  assert.match(
    app,
    /Processed \$\{formatCount\(progress\.current\)\} of \$\{formatCount\(progress\.total\)\} extracted message candidates/,
  );
  assert.doesNotMatch(app, /Indexed .* discovered EML files/);
  assert.match(backend, /processed_units: u64::try_from\(stats\.processed\)\.ok\(\)/);
  assert.match(backend, /total_units: u64::try_from\(stats\.discovered\)\.ok\(\)/);
  assert.match(backend, /unit_kind: Some\(import_progress::ProgressUnitKind::CandidateRecords\)/);
  assert.match(backend, /candidates_processed: u64::try_from\(stats\.processed\)\.ok\(\)/);
  assert.match(backend, /candidates_discovered: u64::try_from\(stats\.discovered\)\.ok\(\)/);
});

test("canonical insertion and fail-open empty-candidate policy stay explicit", () => {
  assert.match(backend, /ON CONFLICT\(eml_path\) DO NOTHING/);
  assert.match(backend, /preflight_resume_index/);
  assert.match(backend, /candidate_identity_manifest/);
  assert.match(policy, /workspace-relative-eml-v1/);
  assert.match(policy, /processed = inserted \+ skipped/);
  assert.match(policy, /does not suppress any extracted EML/);
  assert.match(policy, /remains zero pending deterministic extractor provenance/);
  assert.match(loggingPolicy, /No per-record artifact-suppression diagnostic/);
  assert.match(backend, /Artifact suppression is disabled until extractor provenance/);
  assert.match(rustTests, /blank_no_subject_and_empty_body_candidates_all_fail_open/);
  assert.match(rustTests, /uncertain blank candidates must remain indexed and visible/);
  assert.doesNotMatch(policy, /structurally_empty_readpst_eml_v1/);
  assert.doesNotMatch(loggingPolicy, /structurally_empty_readpst_eml_v1/);
  assert.doesNotMatch(backend, /EMPTY_ARTIFACT_REASON|exact_readpst_empty_envelope/);
});
