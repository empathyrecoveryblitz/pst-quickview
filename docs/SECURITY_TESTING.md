# PST QuickView Security Testing

This document defines the fail-closed security evidence required for the
`0.2.0-beta.4` hardening gate. It is a test plan, not a claim that every command
has run or every requirement has passed. Final evidence must identify the exact
commit, tool versions, command results, skipped tests, environment limitations,
and any remaining finding.

Do not put real mail, private paths, fixture hashes, mailbox data, credentials,
tokens, audit JSON, packet captures, or fuzz reproducers in the repository or a
public report.

## Test-data rules

- Prefer deterministic synthetic PST, EML, MSG, HTML, SQLite, and filename
  inputs containing no real person or mailbox data.
- Keep optional real MSG fixtures external and read-only. Configure their paths
  and optional SHA-256 pins privately; never copy or commit them.
- Record source size, modification metadata, and SHA-256 before and after any
  real-fixture test. A mismatch is a release blocker.
- Create temporary fuzz seeds, sanitizer profiles, databases, captures, logs,
  and native-tool builds outside the repository.
- Minimize a reproducible failure privately. Commit only a synthetic regression
  that does not reveal exploit instructions for an unresolved flaw.
- Delete temporary evidence only after its owning process exits. Never delete a
  PST QuickView workspace or source file as test cleanup.

## Required tool and dependency checks

Run from the repository root unless a command changes directory:

```sh
git diff --check

npm ci
npm audit --omit=dev --json
npm audit --json
npm ls --all
npm audit signatures
npm run test:frontend
npm run build

cd src-tauri
cargo fmt --check
cargo check --locked
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo tree --locked
cargo tree --duplicates
cargo audit
cd ..
```

Requirements:

- production and development npm audits report zero unresolved advisories;
- package signatures verify;
- `npm ci` is deterministic from the reviewed lockfile;
- RustSec reports zero vulnerabilities; and
- unmaintained crates or duplicate security-sensitive crates are classified
  explicitly rather than hidden by a broad update.

Do not use `npm audit fix`, `--force`, a broad `cargo update`, or an unreviewed
major upgrade. The production bundle must be inspected to confirm build-only
packages such as Vite, PostCSS, Nanoid, and the React Vite plugin are not shipped
as application runtime dependencies.

If `cargo-audit` is unavailable, record that limitation. Install it only when a
task explicitly authorizes the toolchain change; use a task-owned temporary tool
root outside the repository, use the current RustSec advisory database, record
its version, and remove that root during final cleanup.

## GitHub security controls

Read-only review uses the public repository
`empathyrecoveryblitz/pst-quickview`. Do not print authentication material.
Relevant endpoints include:

```text
GET /repos/empathyrecoveryblitz/pst-quickview/vulnerability-alerts
GET /repos/empathyrecoveryblitz/pst-quickview/automated-security-fixes
GET /repos/empathyrecoveryblitz/pst-quickview/private-vulnerability-reporting
GET /repos/empathyrecoveryblitz/pst-quickview/dependabot/alerts?state=open
GET /repos/empathyrecoveryblitz/pst-quickview/code-scanning/default-setup
GET /repos/empathyrecoveryblitz/pst-quickview/code-scanning/analyses
GET /repos/empathyrecoveryblitz/pst-quickview/code-scanning/alerts?state=open
GET /repos/empathyrecoveryblitz/pst-quickview/secret-scanning/alerts?state=open
```

Verify Dependabot alerts and security updates, Private Vulnerability Reporting,
secret scanning, push protection where available, and CodeQL
security-extended analysis for Rust and JavaScript/TypeScript. Inspect every
open Dependabot, CodeQL, and secret-scanning alert. Do not dismiss an alert for
convenience.

At the August 12, 2026 start of this beta.4 review, public main reported open
PostCSS Dependabot alerts, and CodeQL default setup had successful Actions and
JavaScript/TypeScript analyses but no Rust database. Treat that as dated
historical evidence: the current gate still requires a fresh readback showing
zero open alerts and a successful Rust scan. Private branch fixes do not
silently change the preserved public beta.3 release or its alerts.

## Baseline and release verification

Run the repository acceptance and release-support checks:

```sh
scripts/agent-check.sh
bash scripts/test-macos-dylib-validation.sh
bash scripts/verify-macos-readpst-bundle.sh
scripts/audit-public-repo.sh
scripts/audit-public-repo.sh --history
```

With no argument, the ReadPST verifier checks the three committed sidecars and
does not infer a packaged app from old build output. Pass the exact current
candidate `.app` path to verify a package. `--source-only` makes the source-only
intent explicit.

`scripts/agent-check.sh` runs formatting, build, frontend/Rust tests, shell
syntax, public-audit regressions, and the public-repository audit. Optional real
MSG tests are skipped unless configured. A skipped external fixture is not a
failure, but it is also not native parser validation.

Do not build a beta.4 DMG during this gate. Verification of an already existing
package does not authorize signing, notarization, upload, installation, or
quarantine changes.

The focused deterministic security gate combines the hardened frontend source
assertions, non-ignored Rust regression tests, Clippy, and bundled ReadPST
verification without using optional real-message fixtures:

```sh
scripts/security-regression-gate.sh
```

Set `PST_QUICKVIEW_SECURITY_STRESS=1` to include the explicit 10,000 through
250,000-message synthetic Search 2.0 stress gate. Any other value is rejected.
The stress databases are temporary and contain no real mail.

## Search, SQLite, FTS, and cursor tests

Run the deterministic ignored Search 2.0 gate separately:

```sh
cd src-tauri
cargo test --locked search_2_stabilization_stress_gate -- --ignored --nocapture
cd ..
```

The gate exercises synthetic 10,000, 100,000, and 250,000-message workspaces,
cursor pagination, multi-workspace behavior, conversations, WAL-visible reads,
and cancellation. Timings are diagnostic; fragile wall-clock thresholds are
not mandatory assertions.

Security regression coverage must additionally prove:

- all user text and filter values remain bound parameters;
- raw FTS operators, punctuation, quoting, Unicode, and injection-like input
  cannot escape the search compiler;
- sort and SQL fragments are selected only from allowlists;
- malformed, truncated, oversized, wrong-version, wrong-context, tampered, and
  stale cursors never panic and fail with typed errors;
- cursors contain no cleartext query, path, subject, sender, or recipient data;
- read-only queries never migrate or alter `user_version`;
- old schemas require the normal upgrade path, future schemas fail closed, and
  no path can lower a future `user_version`;
- active WAL changes are visible to read-only connections;
- malicious views, triggers, indexes, virtual tables, and schema text cannot
  redirect trusted execution;
- extension loading is disabled, `trusted_schema=OFF` is active, and defensive
  mode is applied where supported; and
- database, page, expression, result, and progress limits terminate excessive
  work with a recoverable error.

Use tampered synthetic workspace databases only. Keep schema-v3 compatibility
tests so hardening does not break valid existing workspaces.

## HTML sanitizer and CSP tests

Backend and integration tests must cover hostile synthetic HTML classes,
including active elements, inline handlers, executable URL schemes, SVG and
MathML, frames and embedded documents, forms, refresh/navigation elements,
plugins, base elements, CSS imports and URLs, malformed/mixed-case/encoded
schemes, protocol-relative URLs, local-file URLs, credential-bearing URLs, and
remote images aimed at public and non-public address classes.

Expected properties:

- no script, handler, active SVG/MathML, frame, form, plugin, refresh, or base
  behavior survives;
- message HTML cannot invoke Tauri APIs or create privileged WebViews;
- CSS cannot initiate a hidden request;
- CID and safe data images remain functional within configured byte limits;
- raw HTML and sanitizer markers never appear in contextual search snippets;
- no message-triggered request occurs during rendering or printable export; and
- no frontend action or backend flag can approve a mailbox-supplied network
  resource.

Verify the source and packaged production CSP. It must have no development
origin, wildcard, `unsafe-eval`, remote script, remote frame, object/plugin,
arbitrary navigation, or remote image allowance. Local sanitized rendering must
continue to work.

The existing narrow sanitizer and remote-wording tests are baseline coverage;
they are not substitutes for this adversarial matrix.

## Tauri capability tests

Inspect every capability file, window label, registered command, and dynamic
preview-window construction. Verify the effective permission union, not merely
individual JSON files.

Tests must show that:

- main and `message-preview-*` windows receive only required permissions;
- previews cannot obtain main-window-only authority;
- main-window preview creation cannot select a remote URL, arbitrary label, or
  unreviewed WebView configuration;
- no window has arbitrary filesystem or shell/process permission;
- Open/Reveal cannot target arbitrary frontend-supplied paths;
- remote origins and message documents have no IPC authority; and
- diagnostics, license/notices, file associations, and window focus continue to
  work through their narrow backend paths.

`bash scripts/verify-macos-release.sh` includes a baseline capability shape
check and rejects a visible `TAURI_CONFIG` override during verification. That
does not prove how an older package was built: release evidence must constrain
the build invocation or independently inspect the packaged effective ACL.
Runtime IPC and overlapping-capability tests remain required.

## Path, temporary-file, workspace, and export tests

Use synthetic directory trees outside the repository. Exercise traversal,
absolute paths, symlinked parents, replacement after validation, hardlinks,
case and Unicode-normalization collisions, existing targets, directory/file
substitution, mount disappearance, read-only and network-volume errors,
restrictive permissions, disk-full simulation, and temporary-directory
failure.

Expected properties:

- workspace, extraction, temporary, and export writes remain beneath the exact
  validated root;
- no existing file is silently overwritten;
- creation is exclusive and produces a regular file with restrictive mode;
- no operation follows a symlink or hardlink across the trusted root;
- marker-gated deletion cannot leave the workspace or target a source file;
- source PST, EML, and MSG bytes and metadata remain unchanged;
- attachment Open uses only the just-created backend-validated copy; and
- license, notice, diagnostics, and Finder actions use fixed or backend-derived
  allowlisted paths.

Every verified defect needs a synthetic regression test. Tests must include
races at both validation-to-open and validation-to-delete boundaries where the
platform permits deterministic coordination.

## ReadPST process and output tests

Review the launch implementation and use a synthetic sidecar fixture to record
arguments and environment without processing mail. Verify `Command::new`, no
shell, separate arguments, an explicit integrity-verified sidecar path, minimal
cleared environment, no ambient `PATH`, safe leading-dash handling, isolated
output, bounded capture, cancellation escalation, and safe partial cleanup.

A flood fixture must produce output beyond the configured limit without
unbounded memory or log growth. Output discovery must reject links, hardlinks,
devices, sockets, FIFOs, non-regular files, and anything outside the extraction
root before indexing.

Build an ASan/UBSan ReadPST outside the repository from the verified LibPST
0.6.76 source and local patch when the toolchain supports it. Exercise
deterministic truncation, random bytes, and structured mutations. Record tool
and compiler versions and the unsupported reason if sanitizers cannot run. Do
not replace committed sidecars unless a native flaw is reproduced, remediated,
rebuilt with corresponding source, and fully retested.

## Parser and malformed-input tests

Exercise PST, EML, MSG/CFB, RTF, calendar, and sidecar-output boundaries with
synthetic:

- truncation, random bytes, misleading extensions, and invalid lengths;
- invalid Unicode, embedded NULs, and extreme headers/fields;
- large bodies, many attachments, and deeply nested MIME structures;
- path separators, leading dashes, duplicate names, and reserved names;
- corrupt folder trees and malformed RTF/calendar properties; and
- repeated cancellation and partial failure.

Each case must terminate, return a bounded recoverable error, keep the app
usable, keep temporary disk and memory bounded, avoid corrupting a valid
workspace, and preserve the source. A panic, process-wide crash, indefinite
hang, escape, leak, or native sanitizer finding is a release blocker.

Optional real MSG fixtures are run only when their external paths exist:

```sh
cd src-tauri
cargo test --locked verifies_rich_msg_fixture_reconstruction \
  -- --ignored --nocapture
cargo test --locked verifies_legacy_msg_fixture_reconstruction \
  -- --ignored --nocapture
cd ..
```

Private expected hashes may be supplied through the documented environment
variables. Successful output must not print them.

## Deterministic fuzzing

Run a temporary, bounded harness outside the repository against:

- search tokenization and FTS compilation;
- cursor decoding;
- HTML sanitization;
- EML and MSG/CFB parsing boundaries;
- calendar and RTF parsing;
- filename sanitization and attachment collision handling; and
- sidecar-output discovery.

Use a recorded non-private seed, fixed iteration count, per-case size limit,
and per-case timeout. Include random bytes, truncation, structured mutation,
invalid lengths and Unicode, duplicated sections, and bounded nesting. Do not
make nondeterministic wall-clock performance a CI pass condition. A reproducer
stays private until the flaw is fixed and disclosure is appropriate.

## Resource-exhaustion tests

Beyond the Search 2.0 gate, exercise long bodies, broad queries, large result
counts, many attachments and workspaces, repeated Load More, cursor storms,
many expanded conversations, repeated cancellation, sidecar output floods,
disk-full behavior, and temporary-directory failure.

Verify bounded logs, child output, mounted DOM, result pages, cursor length,
cancellation registry, parser allocation, and temporary storage. Errors must be
recoverable; the UI must not remain permanently locked; source integrity must
hold. Diagnostic latency and peak-resource numbers belong in the private gate
report.

## Privacy and logging tests

Use unique synthetic canary values for a body, subject, participant, attachment
content/name, search text, remote URL, SQL-like text, credential-like value,
source path, and workspace path. After success and failure paths, scan every app
and workspace log and user-visible non-Diagnostics error.

No body, HTML, subject, participant, attachment content, search text, remote
URL, SQL, credential, token, or raw source/workspace path may appear outside an
explicitly requested Diagnostics view. Verify log directories and files have
restrictive permissions, rotation retains only the documented bounded backups,
and a sidecar flood cannot bypass the bound. Cancellation must not be recorded
as a failure.

The current baseline import log contains raw source/workspace/ReadPST paths and
direct child output. That known finding must be remediated and covered by a
regression before beta.4 can pass.

## Network observation

Use only synthetic content and an isolated test profile. Observe process and
WebView sockets during startup, ordinary viewing, search, HTML preview,
standalone preview, diagnostics, cancellation, and malformed-input handling.

The beta.4 policy expects zero message-triggered sockets both before and after
the legacy remote-image action because remote loading remains disabled until a
mediator exists. Also verify no telemetry, analytics, updater, DNS lookup, or
background request. Test public, loopback, IPv4/IPv6 private, link-local,
multicast, credential-bearing, redirecting, and DNS-rebinding destinations
without contacting real third parties.

If mediation is implemented in a future milestone, add tests proving that only
an explicitly approved synthetic public image is returned; every DNS answer
and redirect is revalidated; non-public destinations are rejected; response
type and size are bounded; and no cookies, authorization, mailbox-derived
header/query data, or referrer is sent.

Delete captures and test profiles after review. Public documentation should
report only the result, not private paths or full packet contents.

## Source-integrity test

For each read-only real fixture:

1. Record SHA-256, size, modification time, and stable file identity before the
   operation.
2. Run import/parse, cancellation, malformed-neighbor, export, and failure paths
   without granting write access to the source.
3. Record the same values afterward.
4. Require byte-for-byte hash equality and unchanged metadata.

For large PSTs, the release gate must still use a full before/after SHA-256; the
application's faster workspace fingerprint is not a substitute for evidence
that the source remained unchanged.

## Security regression inventory

For every verified defect, the final report must identify:

- the affected boundary and severity;
- a private minimal reproducer;
- the correcting commit;
- the synthetic regression test and command;
- the complete validation that passed; and
- whether the public beta.3 remains exposed and what later public remediation
  is required.

Mechanical test counts are not enough. Independent review must confirm that the
test reaches the vulnerable boundary and fails on the pre-fix behavior.

## Signing and notarization readiness

Do not sign during this security gate. Read-only readiness checks may report:

- whether a Developer ID Application identity is available, without exposing
  certificate material;
- whether hardened runtime is enabled;
- whether the nested ReadPST binary is ready for inside-out signing;
- the effective entitlement set; and
- whether a named notarization credential profile is available, without
  exposing credentials.

Missing signing or notarization is distribution-trust work, not an
application-code vulnerability. The remaining procedure is documented in
[`MACOS_SIGNING.md`](MACOS_SIGNING.md).

## Final verdict

Return `SECURITY GATE PASSED` only when all required evidence is complete and
there are zero unresolved production npm vulnerabilities, RustSec
vulnerabilities, GitHub security alerts, reproduced injection/escape/overwrite
or source-modification defects, sanitizer bypasses, message-triggered network
requests, unclassified SSRF/private-network findings, tested parser
panics/crashes/hangs, private mailbox data in logs, and unresolved high,
critical, unclassified, or release-relevant findings.

Otherwise return `HOLD — UNRESOLVED SECURITY FINDINGS`, list each unresolved
item with evidence, and do not prepare a release.

## Cleanup

After every test process has exited, remove only the task-owned external tools,
synthetic mail, fuzz roots, databases, logs, profiles, screenshots, network
captures, audit JSON, and temporary build roots. Confirm the Git worktree state
separately. Never use broad cleanup, and never delete a source file, workspace,
repository, published tag, or release asset.
