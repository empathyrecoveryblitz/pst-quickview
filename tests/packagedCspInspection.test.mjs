import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { createHash } from "node:crypto";

const inspector = fileURLToPath(new URL("../scripts/inspect-packaged-csp.py", import.meta.url));
const config = readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8");
const support = readFileSync(new URL("../src-tauri/src/support_session.rs", import.meta.url), "utf8");
const production = JSON.parse(config).app.security.csp;
const printable = "default-src 'none'; img-src data:; style-src 'unsafe-inline'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; connect-src 'none'";
const headers = "Content-Security-Policy\0Content-Security-Policy\0" + "content-security-policy\0".repeat(4);
const legacyBaseline = `${headers}${production}\0${printable}`;
const fallback = "default-src 'none'; script-src 'self'; style-src 'self'; frame-src 'none'; frame-ancestors 'none'; object-src 'none'; media-src 'none'; base-uri 'none'; form-action 'none'";
const runtimePrefix = "connect-src ipc: http://ipc.localhost ";
const baseline = `${legacyBaseline}\0Content-Security-Policy${fallback}Cache-Control\0tauri${runtimePrefix}; \0`;
const sha256 = (value) => createHash("sha256").update(value).digest("hex");
const runner = String.raw`
import importlib.util,json,sys
spec=importlib.util.spec_from_file_location("inspector",sys.argv[1])
module=importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
value=json.load(sys.stdin)
try:
 literals=module.source_literals(value["config"].encode(),value["support"].encode())
 sources=[(name,data.encode()) for name,data in value.get("adjacencySources",{"support_session.rs":value["support"]}).items()]
 catalog=module.source_adjacency_catalog(sources,literals)
 if value.get("operation")=="literals":
  print(json.dumps({name:{"sha256":module.fingerprint(data),"byteLength":len(data)} for name,data in literals.items()}))
 elif value.get("operation")=="main":
  module.inspect_app=lambda *_:{"schema":1,"diagnosticOnly":True,"slices":[module.inspect_slice(value["binary"].encode(),"arm64",literals,catalog)]}
  sys.exit(module.main(["inspect","config","support","binary"]))
 else:
  print(json.dumps(module.inspect_slice(value["binary"].encode(),value.get("architecture","arm64"),literals,catalog)))
except module.DiagnosticError as error:
 print(json.dumps({"errorCategory":error.args[0]}))
 sys.exit(2)
`;
function inspect(binary = baseline, options = {}) {
  const result = spawnSync("python3", ["-B", "-c", runner, inspector], {
    input: JSON.stringify({ config, support, binary, ...options }), encoding: "utf8",
  });
  assert.equal(result.stderr, "");
  return { status: result.status, report: JSON.parse(result.stdout), output: result.stdout };
}

test("CSP inspection derives static literals from the current reviewed sources", () => {
  const { status, report } = inspect(baseline, { operation: "literals" });
  assert.equal(status, 0);
  assert.deepEqual(Object.keys(report).sort(), ["printable", "production", "support_connect_template", "support_directive_token", "support_fallback"]);
  assert.equal(report.production.byteLength, Buffer.byteLength(production));
  for (const value of Object.values(report)) assert.match(value.sha256, /^[a-f0-9]{64}$/);
  assert.equal(inspect(baseline, { support: "unsupported" }).report.errorCategory, "source_literal_unavailable");
});

test("CSP inspection reports counts and boundaries while retaining the existing verdict", () => {
  const { status, report } = inspect();
  assert.equal(status, 0);
  assert.deepEqual(report.existingVerifier, { passed: true, failureCategory: "none" });
  assert.equal(report.headerSpellings.canonical, 3);
  assert.equal(report.headerSpellings.lowercase, 4);
  assert.equal(report.sourceLiteralObservations.production.occurrences, 1);
  assert.equal(report.sourceLiteralObservations.support_fallback.occurrences, 1);
  assert.deepEqual(report.outsideKnownLiteralDirectiveHistogram, {});
  assert.equal(report.sourceLiteralObservations.production.locations[0].before, "nul");
});

test("recognized support literals remain observations and never approve an unreviewed policy", () => {
  const { report } = inspect(`${legacyBaseline}\0${fallback}\0connect-src ipc: http://ipc.localhost `);
  assert.equal(report.existingVerifier.passed, false);
  assert.equal(report.sourceLiteralObservations.support_fallback.occurrences, 1);
  assert.equal(report.sourceLiteralObservations.support_connect_template.occurrences, 1);
  assert.equal(report.safe, undefined);
});

test("adjacent complete source literals are fingerprinted longest-first without changing the verdict", () => {
  const before = "Content-Security-Policy";
  const after = "Cache-Control";
  const { report, output } = inspect(`${legacyBaseline}\0${before}${fallback}${after}\0tauri${runtimePrefix}; `);
  const location = report.sourceLiteralObservations.support_fallback.locations[0];
  assert.equal(location.adjacency.before.classification, "source_literal_match");
  assert.deepEqual(location.adjacency.before.matches[0], { sha256: sha256(before), byteLength: before.length, kinds: ["rust_string"] });
  assert.equal(location.adjacency.after.matches[0].sha256, sha256(after));
  assert.equal(report.sourceLiteralObservations.support_connect_template.locations[0].adjacency.after.matches[0].sha256, sha256("; "));
  assert.match(report.adjacencySourceCatalog.sourceSha256, /^[a-f0-9]{64}$/);
  assert.equal(report.adjacencySourceCatalog.sourceFileCount, 1);
  assert.equal(report.existingVerifier.passed, true);
  assert.equal(output.includes(before), false);
  assert.equal(output.includes(after), false);
});

test("changed or arbitrary adjacency stays unclassified with only bounded context fingerprints", () => {
  const before = "SYNTHETIC_UNCLASSIFIED~".repeat(3);
  const after = "~SYNTHETIC_UNCLASSIFIED".repeat(3);
  const { report, output } = inspect(`${legacyBaseline}\0${before}${fallback}${after}`);
  const adjacency = report.sourceLiteralObservations.support_fallback.locations[0].adjacency;
  for (const side of Object.values(adjacency)) {
    assert.equal(side.classification, "unclassified");
    assert.deepEqual(side.matches, []);
    assert.equal(side.contextByteLength, 32);
  }
  assert.equal(adjacency.before.contextSha256, sha256(before.slice(-32)));
  assert.equal(adjacency.after.contextSha256, sha256(after.slice(0, 32)));
  assert.equal(report.existingVerifier.passed, false);
  assert.equal(output.includes("SYNTHETIC_UNCLASSIFIED"), false);
  const changed = inspect(`${legacyBaseline}\0Content-Security-Polic~${fallback}~Cache-Control`);
  const changedAdjacency = changed.report.sourceLiteralObservations.support_fallback.locations[0].adjacency;
  assert.equal(changedAdjacency.before.classification, "unclassified");
  assert.equal(changedAdjacency.after.classification, "unclassified");
});

test("own-crate source set is pinned while comments, raw strings, and test-only literals are excluded", () => {
  const neighbor = "SYNTHETIC_OTHER_MODULE_LITERAL";
  const ignored = "SYNTHETIC_COMMENT_LITERAL";
  const raw = "SYNTHETIC_RAW_LITERAL";
  const testOnly = "SYNTHETIC_TEST_LITERAL";
  const extra = `// \"${ignored}\"\nconst RAW: &str = r#\"${raw}\"#;\nconst OWN: &str = \"${neighbor}\";\n#[cfg(test)]\nmod tests { const TEST: &str = \"${testOnly}\"; }`;
  const options = { adjacencySources: { "support_session.rs": support, "other.rs": extra } };
  const { report, output } = inspect(`${legacyBaseline}\0${neighbor}${fallback}`, options);
  assert.equal(report.adjacencySourceCatalog.sourceFileCount, 2);
  assert.equal(report.sourceLiteralObservations.support_fallback.locations[0].adjacency.before.matches[0].sha256, sha256(neighbor));
  for (const value of [ignored, raw, testOnly]) {
    const observation = inspect(`${legacyBaseline}\0${value}${fallback}`, options).report;
    assert.equal(observation.sourceLiteralObservations.support_fallback.locations[0].adjacency.before.classification, "unclassified");
  }
  const changed = inspect(baseline, { adjacencySources: { ...options.adjacencySources, "other.rs": `${extra}\n// changed` } }).report;
  assert.notEqual(changed.adjacencySourceCatalog.sourceSha256, report.adjacencySourceCatalog.sourceSha256);
  assert.equal(output.includes(neighbor), false);
  assert.equal(output.includes("other.rs"), false);
});

test("adjacency scanner skips continued byte strings without truncating later production literals", () => {
  const neighbor = "SYNTHETIC_PRODUCTION_AFTER_TEST_ATTRIBUTE";
  const extra = '#[cfg(test)]\nfn test_helper() {}\nconst BYTES: &[u8] = b"ignored\\\ncontinued";\nconst QUOTE: char = \'"\';\n' + `const OWN: &str = "${neighbor}";`;
  const { report } = inspect(`${legacyBaseline}\0${neighbor}${fallback}`, { adjacencySources: { "other.rs": extra } });
  assert.equal(report.sourceLiteralObservations.support_fallback.locations[0].adjacency.before.matches[0].sha256, sha256(neighbor));
  assert.equal(report.existingVerifier.passed, false);
});

test("adjacency candidate inventory and reported matches have explicit limits", () => {
  const sources = Object.fromEntries(Array.from({ length: 129 }, (_, index) => [`${index}.rs`, 'const A: &str = "bounded";']));
  assert.equal(inspect(baseline, { adjacencySources: sources }).report.errorCategory, "source_adjacency_limit");
  const values = ["abcdefgh", "bcdefgh", "cdefgh", "defgh", "efgh", "fgh"];
  const extra = values.map((value, index) => `const A${index}: &str = "${value}";`).join("\n");
  const { report } = inspect(`${legacyBaseline}\0${values[0]}${fallback}`, { adjacencySources: { "other.rs": extra } });
  const side = report.sourceLiteralObservations.support_fallback.locations[0].adjacency.before;
  assert.equal(side.matches.length, 4);
  assert.equal(side.matchesTruncated, true);
  assert.deepEqual(side.matches.map((match) => match.byteLength), [8, 7, 6, 5]);
});

test("unknown static content is fingerprinted without dumping strings, paths, or addresses", () => {
  const secret = "SYNTHETIC_PRIVATE_SENTINEL";
  const { output, report } = inspect(`${baseline}\0script-src-elem https://example.invalid/${secret}\0CoNtEnT-SeCuRiTy-PoLiCy`);
  assert.equal(report.existingVerifier.passed, false);
  assert.equal(report.headerSpellings.otherCase, 1);
  assert.equal(report.outsideKnownLiteralDirectiveHistogram["script-src-elem"], 1);
  assert.match(report.outsideKnownLiteralSamples[0].contextSha256, /^[a-f0-9]{64}$/);
  assert.equal(output.includes(secret), false);
  assert.equal(output.includes("example.invalid"), false);
  assert.equal(output.includes(production), false);
});

test("inspection output is bounded and preserves only fixed enum categories", () => {
  const { report } = inspect(`${baseline}\0${"script-src-elem https:\0".repeat(40)}`);
  assert.equal(report.outsideKnownLiteralSamples.length, 16);
  assert.equal(report.samplesTruncated, true);
  assert.equal(report.outsideKnownLiteralDirectiveHistogram["script-src-elem"], 40);
  const repeated = inspect(`${baseline}\0${`${production}\0`.repeat(12)}`).report;
  assert.equal(repeated.sourceLiteralObservations.production.locations.length, 8);
  assert.equal(repeated.sourceLiteralObservations.production.locationsTruncated, true);
  assert.equal(repeated.existingVerifier.passed, false);
  assert.equal(inspect(baseline, { architecture: "unclassified" }).report.errorCategory, "slice_limit_or_architecture");
});

test("CLI failures are categorical and never echo supplied paths", () => {
  const sentinel = "SYNTHETIC_PRIVATE_SENTINEL";
  const result = spawnSync("python3", ["-B", inspector, `/missing/${sentinel}`, "/missing/support", "/missing/binary"], { encoding: "utf8" });
  assert.equal(result.status, 2);
  assert.equal(result.stderr, "");
  assert.equal(result.stdout.includes(sentinel), false);
  assert.deepEqual(JSON.parse(result.stdout), { schema: 1, diagnosticOnly: true, errorCategory: "source_unavailable" });
});

test("CLI observation does not override failure from the existing verifier", () => {
  const passed = inspect(baseline, { operation: "main" });
  assert.equal(passed.status, 0);
  assert.equal(passed.report.diagnosticOnly, true);
  const failed = inspect(`${baseline}\0script-src-elem https:`, { operation: "main" });
  assert.equal(failed.status, 1);
  assert.equal(failed.report.slices[0].existingVerifier.passed, false);
});

test("source-derived observations never approve changed support literal semantics", () => {
  const changedFallback = fallback.replace("script-src 'self'", "script-src https:");
  const { report } = inspect(baseline.replace(fallback, changedFallback), { support: support.replace(fallback, changedFallback) });
  assert.equal(report.sourceLiteralObservations.support_fallback.occurrences, 1);
  assert.equal(report.existingVerifier.passed, false);
  assert.equal(inspect(legacyBaseline).report.existingVerifier.passed, false);
});
