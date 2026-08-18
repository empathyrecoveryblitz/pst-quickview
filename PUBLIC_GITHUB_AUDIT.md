# Public GitHub readiness audit

Audit refreshed: 2026-08-11. Scope: tracked files and all commits reachable from local branches and tags. The audit used targeted string searches, email/credential patterns, tracked-extension checks, and per-commit Git searches. Sensitive content is intentionally not reproduced here.

## Decision

The sanitized beta.3 tree was exported into fresh public history and published as a prerelease. **The private development history is not safe to publish.** Reachable private commits contain external-drive paths, fixture names/identifiers, and message-derived names or text. Examples include commits `c3ba791225831b90f32c3d0e156a540c81aea94a`, `d94cbc192f6265f1ec99d6de4fe8533e4f64e0b3`, `885ff75104d08f66e4cdfa0cf60cfbbcaa985181`, and `72239a9fce46cab7a6cd3cd2b86c79622139dc04`, affecting historical documentation, tests, scripts, and backend files.

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
preparation and public delivery are complete for `v0.2.0-beta.3`. The published
prerelease provides the universal DMG, the exact ReadPST Corresponding Source
archive, and `SHA256SUMS.txt` together. Public access and the recorded hashes were
verified after publication: the DMG is
`b29ed3295e0bbbdcad4bd88621972a609c260601bda34808974c234a8785efad`, and the
source archive is
`a858ea017bb80516b42b14da8b624530968c70a6daf5bfc7fad628a631a88787`.
Preparation and release verification reject a stale companion URL. An upstream
URL alone remains insufficient; see `docs/READPST_CORRESPONDING_SOURCE.md`.
The beta.2 tag, release, and assets remain preserved.

## Security and conduct reporting status

Resolved:

- GitHub Private Vulnerability Reporting is enabled for the public repository, and the issue
  template links to the repository's private advisory form.
- `SECURITY.md` uses Private Vulnerability Reporting without publishing a contact email.
- `CODE_OF_CONDUCT.md` directs sensitive conduct concerns to GitHub's platform-level reporting
  controls and records an enforcement procedure, non-retaliation, and conflict handling for a
  report involving the sole maintainer.

## Remaining post-publication validation

- Complete a clean Intel Mac test without Homebrew or system ReadPST.
- Complete a clean Apple Silicon Mac test without Homebrew or system ReadPST.
- Review and approve synthetic screenshots.
- Revisit signing and notarization for a future release; beta.3 remains unsigned and unnotarized.
