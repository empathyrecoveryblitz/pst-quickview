# PST QuickView

PST QuickView is a macOS public-beta candidate for fast, read-only inspection of Outlook PST archives and standalone EML/MSG messages. It indexes locally for search and keeps original source files unchanged.

> Version 0.2.0-beta.2 is an unsigned, unnotarized beta. macOS may require the normal per-app approval flow. Never disable Gatekeeper globally.

Repository: https://github.com/empathyrecoveryblitz/pst-quickview

## Features

- PST, EML, and MSG viewing with Finder Open With and drag and drop
- Multiple open PSTs, session restore, hidden-folder rail, Three Column and Outlook Style layouts
- Fast local SQLite indexing and search, including `from:`, `to:`, `subject:`, `body:`, `attachment:`, `has:attachment`, date filters, and quoted phrases
- Conversation View with prominent participants and expandable message threads
- Plain Text and Sanitized HTML reader modes; remote resources blocked unless explicitly approved for the current message
- Standalone EML/MSG pop-out previews, legacy RTF/CID reconstruction, inline-resource details, and raw-source diagnostics
- Best-effort calendar MSG preview; this is not fully validated Outlook compatibility
- Attachment export-first Open behavior, printable HTML export, and source reveal actions
- Bundled universal ReadPST/LibPST 0.6.76 sidecar for PST extraction

PST QuickView does not claim perfect Outlook fidelity. Encrypted, damaged, unusual, or newly introduced MAPI content may be incomplete or unsupported.

## Safety and privacy

Original PST, EML, and MSG files are read-only. Parsing, indexing, search, and preview happen locally. The app has no telemetry, cloud parsing, cloud storage, or background network service. HTML is sanitized, remote content is blocked by default, and attachment Open exports a separate safe copy before asking macOS to open it.

Workspaces are caches, not source archives. They may live beside a PST in `.pst-quickview.noindex/` (or legacy `.pst-quickview/`) or under macOS Application Support. Workspace deletion is explicit and marker-gated and never targets a source message/archive.

## Screenshots

Public screenshots are pending and must use synthetic mail only. See [docs/SCREENSHOTS.md](docs/SCREENSHOTS.md).

## Install the beta

Public installation instructions will accompany a reviewed artifact. The current beta is unsigned and unnotarized. Verify its published SHA-256 hash before opening it and retain the original download for comparison.

## Build and test

Prerequisites are macOS, Node/npm, the Rust toolchain, and Tauri v2 platform requirements.

```sh
npm ci
npm run test:frontend
npm run build

cd src-tauri
cargo fmt --check
cargo check --locked
cargo test --locked
cd ..

scripts/agent-check.sh
npm run tauri dev
```

Universal release-candidate verification is documented in `docs/RELEASE_COMPLIANCE.md` and performed with:

```sh
npm run tauri build -- --target universal-apple-darwin
bash scripts/verify-macos-release.sh
```

The bundled ReadPST sidecars are rebuilt from an explicitly supplied,
checksum-verified LibPST 0.6.76 archive. Intel sidecars target macOS 10.13;
Apple Silicon sidecars target macOS 11.0; the universal binary preserves each
slice's own minimum. The build script performs no source download:

```sh
READPST_SOURCE_ARCHIVE=/absolute/path/to/libpst-0.6.76.tar.gz \
scripts/build-readpst-sidecars.sh
```

`scripts/prepare-readpst-corresponding-source.sh` assembles the offline source
companion, manifest, and deterministic archive. Public binary publication must
also upload that verified archive and checksum beside the DMG; local preparation
alone does not complete delivery.

Those Mach-O deployment targets do not by themselves prove runtime support on
every matching macOS release; clean Intel and Apple Silicon testing remains a
release requirement.

Optional real MSG regression fixtures must remain external and read-only:

```sh
export PST_QUICKVIEW_RICH_MSG_FIXTURE="/absolute/path/to/rich-message-fixture.msg"
export PST_QUICKVIEW_LEGACY_MSG_FIXTURE="/absolute/path/to/legacy-message-fixture.msg"
```

Never copy fixtures into this repository. Synthetic fixtures are preferred.

## Known limitations

- Calendar MSG rendering is best effort and not fully validated.
- Outlook-specific rendering, encrypted archives, and uncommon MAPI properties may differ or be unsupported.
- Large archives require local extraction/index storage and initial processing time.
- Remote images remain blocked unless approved per message.
- The beta is currently unsigned and unnotarized.

## Contributing and security

Read [CONTRIBUTING.md](CONTRIBUTING.md) before proposing a change. Never attach real mail, archives, message bodies, attachments, logs, workspaces, or unreviewed Diagnostics to a public issue. Security reports must use the private process in [SECURITY.md](SECURITY.md).

## Licensing and third parties

PST QuickView source code is licensed under `GPL-3.0-or-later`. The complete project license is in [LICENSE](LICENSE), and the copyright notice is in [COPYRIGHT.md](COPYRIGHT.md).

The bundled ReadPST/LibPST component remains independently licensed under `GPL-2.0-or-later`. Other dependencies retain their own licenses; the project license does not replace or alter those terms. Component notices, provenance, and release-compliance requirements are in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). [LICENSES/](LICENSES/) contains the applicable full GPL license texts.

Every public binary distribution must provide equivalent access to the exact Corresponding Source used for its bundled ReadPST binary. An upstream URL alone is not sufficient; see [docs/READPST_CORRESPONDING_SOURCE.md](docs/READPST_CORRESPONDING_SOURCE.md).
