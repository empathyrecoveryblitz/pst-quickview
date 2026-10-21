import test from "node:test";
import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";

const inspector = fileURLToPath(new URL("../scripts/inspect-packaged-csp.py", import.meta.url));
const config = readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8");
const support = readFileSync(new URL("../src-tauri/src/support_session.rs", import.meta.url), "utf8");
const production = JSON.parse(config).app.security.csp;
const printable = "default-src 'none'; img-src data:; style-src 'unsafe-inline'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'; connect-src 'none'";
const headers = "Content-Security-Policy\0Content-Security-Policy\0" + "content-security-policy\0".repeat(4);
const baseline = `${headers}${production}\0${printable}`;
const runner = String.raw`
import importlib.util,json,sys
spec=importlib.util.spec_from_file_location("inspector",sys.argv[1])
module=importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)
value=json.load(sys.stdin)
try:
 literals=module.source_literals(value["config"].encode(),value["support"].encode())
 if value.get("operation")=="literals":
  print(json.dumps({name:{"sha256":module.fingerprint(data),"byteLength":len(data)} for name,data in literals.items()}))
 elif value.get("operation")=="main":
  module.inspect_app=lambda *_:{"schema":1,"diagnosticOnly":True,"slices":[module.inspect_slice(value["binary"].encode(),"arm64",literals)]}
  sys.exit(module.main(["inspect","config","support","binary"]))
 else:
  print(json.dumps(module.inspect_slice(value["binary"].encode(),value.get("architecture","arm64"),literals)))
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
  assert.equal(report.headerSpellings.canonical, 2);
  assert.equal(report.headerSpellings.lowercase, 4);
  assert.equal(report.sourceLiteralObservations.production.occurrences, 1);
  assert.equal(report.sourceLiteralObservations.support_fallback.occurrences, 0);
  assert.deepEqual(report.outsideKnownLiteralDirectiveHistogram, {});
  assert.equal(report.sourceLiteralObservations.production.locations[0].before, "nul");
});

test("recognized support literals remain observations and never approve an unreviewed policy", () => {
  const fallback = "default-src 'none'; script-src 'self'; style-src 'self'; frame-src 'none'; frame-ancestors 'none'; object-src 'none'; media-src 'none'; base-uri 'none'; form-action 'none'";
  const { report } = inspect(`${baseline}\0${fallback}\0connect-src ipc: http://ipc.localhost `);
  assert.equal(report.existingVerifier.passed, false);
  assert.equal(report.existingVerifier.failureCategory, "unreviewed_directive");
  assert.equal(report.sourceLiteralObservations.support_fallback.occurrences, 1);
  assert.equal(report.sourceLiteralObservations.support_connect_template.occurrences, 1);
  assert.equal(report.safe, undefined);
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
