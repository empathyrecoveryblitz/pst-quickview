# Public GitHub readiness audit

Audit date: 2026-07-17. Scope: tracked files and all commits reachable from local branches and tags. The audit used targeted string searches, email/credential patterns, tracked-extension checks, and per-commit Git searches. Sensitive content is intentionally not reproduced here.

## Decision

The sanitized current tree can become a public-beta candidate after the unresolved items below receive human review. **The existing Git history is not safe to publish.** Reachable commits contain private external-drive paths, fixture names/identifiers, and message-derived names or text. Examples include commits `c3ba791225831b90f32c3d0e156a540c81aea94a`, `d94cbc192f6265f1ec99d6de4fe8533e4f64e0b3`, `885ff75104d08f66e4cdfa0cf60cfbbcaa985181`, and `72239a9fce46cab7a6cd3cd2b86c79622139dc04`, affecting historical versions of `README.md`, `AGENTS.md`, tests, scripts, and `src-tauri/src/lib.rs`.

Use a fresh repository created from a reviewed, sanitized export. Keep this repository private as the development archive. Do not publish or rewrite this history without separate backup, review, and explicit approval.

## Current-tree findings and remediation

- No tracked PST, OST, EML, MSG, SQLite workspace, log, DMG, private key, or obvious high-confidence token was found.
- Private external fixture paths, names, subjects, content checks, and identifying test names were removed or generalized.
- Neutral `PST_QUICKVIEW_RICH_MSG_FIXTURE` and `PST_QUICKVIEW_LEGACY_MSG_FIXTURE` variables replace identifying names. Deprecated aliases remain temporarily for private developer compatibility.
- Test identities now use synthetic `example.com` data.
- `.gitignore`, the offline audit script, and the history-free export procedure reduce recurrence risk.
- The public repository URL is recorded as
  `https://github.com/empathyrecoveryblitz/pst-quickview` in package metadata.
- The new application icon is original project artwork owned by Kev P; its provenance and project
  license treatment are recorded in `THIRD_PARTY_NOTICES.md`.

## Licensing status

Resolved:

- The owner selected `GPL-3.0-or-later` for PST QuickView's original source code.
- The root `LICENSE` and `LICENSES/GPL-3.0-or-later.txt` contain byte-identical official GNU GPL
  version 3 text.
- `LICENSES/GPL-2.0-or-later.txt` contains the official GNU GPL version 2 text applicable to the
  separately identified ReadPST/LibPST component.
- Project metadata, contributor terms, packaged-resource configuration, and automated license
  checks use the selected SPDX identifier.
- The project repository URL and original application-icon provenance are documented.

Bundled ReadPST/LibPST 0.6.76 remains independently licensed under `GPL-2.0-or-later`. The binary
links dynamically only to macOS system libraries and contains statically linked libpst objects.
The exact upstream archive and checksum, explicit local patch, toolchain assumptions, build
commands, sidecar hashes, x86_64 macOS 10.13 and arm64 macOS 11.0 deployment
targets, offline companion preparation script, and strict public-release
verifier are now recorded. The accidental macOS 15.0 sidecar minimum was
corrected through a verified source rebuild. Technical Corresponding Source
companion preparation is complete. Public delivery is still
incomplete until the regenerated companion archive and checksum are uploaded
beside the DMG at the intended `0.2.0-beta.2` release location. Preparation and
release verification now reject a stale companion URL. An upstream URL alone remains
insufficient; see
`docs/READPST_CORRESPONDING_SOURCE.md`.

## Remaining publication work

- Upload the approved DMG and matching ReadPST Corresponding Source archive/checksum together.
- Verify the recorded public download URLs after upload.
- Complete a clean Intel Mac test without Homebrew or system ReadPST.
- Complete a clean Apple Silicon Mac test without Homebrew or system ReadPST.
- Enable and verify public-repository private vulnerability reporting.
- Add a private code-of-conduct reporting contact and enforcement procedure before community
  contributions open.
- Review and approve synthetic screenshots.
- Make an explicit signing/notarization decision; this candidate remains unsigned and unnotarized.
- Obtain explicit final publication approval before any push, release, or upload.
