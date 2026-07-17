# PST QuickView Agent Rules

These rules apply to the entire repository. A narrower `AGENTS.md` may add
constraints for a subtree, but it must not weaken these safety requirements.

## Project

PST QuickView is a Tauri v2 desktop application with a React/TypeScript
frontend, a Rust backend, and SQLite indexes. It is a local-only viewer for PST,
EML, and MSG files. Original source files are always read-only.

## Required Safety

- Never modify, move, rename, truncate, replace, or delete a source PST, EML, or
  MSG file. Tests using real fixtures must verify that the source bytes remain
  unchanged.
- Never weaken HTML sanitization.
- Never enable remote images or other remote resources by default. Approval must
  remain explicit and scoped to the current message.
- Attachment **Open** must export a safe copy first. Never open attachment bytes
  directly from a PST, EML, or MSG source.
- Never delete a workspace unless the user explicitly instructs you to do so.
  Workspace deletion must remain marker-gated and must never target a source
  file.
- Never disable Gatekeeper globally or recommend doing so.
- Never introduce telemetry, analytics, cloud parsing, cloud storage, or other
  network processing of message data.
- Preserve all exact dependency pins. In particular, keep the Rust pins
  `time = "=0.3.51"`, `cfb = "=0.7.3"`, and
  `msg_parser = "=0.3.6"` unchanged.
- Do not commit, push, install, release, upload, sign, or notarize unless the
  user explicitly authorizes that exact action.
- Preserve unrelated user changes in a dirty worktree.

If a requested change conflicts with these rules, or product behavior is
ambiguous, stop and ask for human direction instead of guessing.

## Repository Layout

- `src/`: React/TypeScript UI.
- `src-tauri/src/`: Rust application logic, parsing, indexing, export, and Tauri
  commands.
- `src-tauri/capabilities/`: Tauri capability allowlists.
- `src-tauri/binaries/`: bundled `readpst` sidecars.
- `src-tauri/icons/`: application icon assets.
- `scripts/`: build, release-verification, and supervised-agent scripts.
- `docs/`: operational, release, logging, and agent-loop documentation.
- `TESTING.md`: manual internal-beta regression plan.
- `dist/`, `src-tauri/target/`, and `src-tauri/gen/`: generated output.

PST QuickView workspaces may exist next to a PST under
`.pst-quickview.noindex/` or `.pst-quickview/`, or under Application Support.
They are user data, not disposable build output.

## Build And Test Commands

Install dependencies only when the user explicitly asks:

```sh
npm install
```

Run the application:

```sh
npm run tauri dev
```

Run the standard checks:

```sh
git diff --check

cd src-tauri
cargo fmt --check
cargo check --locked
cargo test --locked

cd ..
npm run build
```

The repository acceptance command is:

```sh
scripts/agent-check.sh
```

## Optional Real MSG Fixtures

Real fixtures are external, optional, and read-only. Configure them with:

```sh
export PST_QUICKVIEW_RICH_MSG_FIXTURE="/absolute/path/to/rich-message-fixture.msg"
export PST_QUICKVIEW_LEGACY_MSG_FIXTURE="/absolute/path/to/legacy-message-fixture.msg"
# Optional identity pins; replace privately with known 64-character hashes:
# export PST_QUICKVIEW_RICH_MSG_EXPECTED_SHA256="<known SHA-256>"
# export PST_QUICKVIEW_LEGACY_MSG_EXPECTED_SHA256="<known SHA-256>"
```

Expected hashes are optional and must never be committed when they identify private fixtures.
When configured, tests require the pre- and post-parse hashes to match that value. Without one,
the tests still require the source bytes to remain unchanged but report that identity is unpinned.

Run the ignored fixture tests only when the corresponding path exists:

```sh
cd src-tauri

cargo test --locked verifies_rich_msg_fixture_reconstruction \
  -- --ignored --nocapture

cargo test --locked verifies_legacy_msg_fixture_reconstruction \
  -- --ignored --nocapture
```

Never copy these fixtures into the repository and never alter them.

## Universal Build And Release Verification

Install Rust targets only when the user explicitly authorizes toolchain changes:

```sh
rustup target add x86_64-apple-darwin aarch64-apple-darwin
```

Build the universal application:

```sh
npm run tauri build -- --target universal-apple-darwin
```

Verify an existing universal package:

```sh
bash scripts/verify-macos-release.sh
```

Release verification is read-only. It does not authorize signing, notarization,
upload, installation, quarantine changes, or Gatekeeper changes.

## Protected Files And Directories

Do not modify any of the following without explicit task authorization:

- Any `*.pst`, `*.eml`, or `*.msg` source or fixture, including external fixture
  paths supplied through environment variables.
- Any PST QuickView workspace, including `.pst-quickview.noindex/`,
  `.pst-quickview/`, and Application Support workspaces.
- `package.json`, `package-lock.json`, `src-tauri/Cargo.toml`, and
  `src-tauri/Cargo.lock`.
- `src-tauri/tauri.conf.json`, `src-tauri/Info.plist`, and
  `src-tauri/capabilities/`.
- `src-tauri/binaries/` and `src-tauri/icons/`.
- Generated or packaged output in `dist/`, `src-tauri/target/`, and
  `src-tauri/gen/`.
- Signing identities, certificates, provisioning material, notarization
  credentials, keychains, release upload configuration, and installed
  applications.
- `.git/` internals and existing `.agent-loop/` reports.
- Application feature code under `src/` and `src-tauri/src/` unless the task
  explicitly calls for a product change.

Never treat generated output, caches, workspaces, or untracked files as safe to
delete merely because Git does not track them.
