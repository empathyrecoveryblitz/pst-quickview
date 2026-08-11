# Release compliance checklist

- [x] Owner selected `GPL-3.0-or-later`; root license and package metadata updated.
- [x] Official GPL version 3 and GPL version 2 texts included without project commentary.
- [x] Project copyright notice and contribution licensing terms documented.
- [ ] Complete lockfile-derived third-party inventory and all required dependency notices reviewed.
- [x] ReadPST companion preparation tooling records the exact version, source archive, SHA-256, every local patch or
  generated replacement, configure/build commands, sidecar build script, license text, copyright
  notices, sidecar hashes, per-architecture deployment targets, complete manifest, and intended
  public download location.
- [ ] The verified ReadPST Corresponding Source archive and checksum are uploaded and available
  next to the DMG at the recorded public location:
  `https://github.com/empathyrecoveryblitz/pst-quickview/releases/download/v0.2.0-beta.3/readpst-corresponding-source-0.6.76.tar.gz`.
- [ ] Distribution obligations and notices for the statically linked `GPL-2.0-or-later` LibPST
  component receive final release review.
- [x] Original application icon ownership is documented; copied assets still require their normal
  dependency/source notice review.
- [x] Bundled ReadPST binaries match recorded version, architecture, checksum, provenance, patch,
  x86_64 macOS 10.13 / arm64 macOS 11.0 deployment targets, and system-only linkage checks.
- [ ] ReadPST is exercised on clean Intel and Apple Silicon Macs at release-supported OS versions;
  Mach-O deployment metadata alone is not a compatibility test.
- [ ] Privacy statement accurately describes local-only processing, no telemetry, and remote-content defaults.
- [ ] Public export and synthetic screenshots pass privacy review.
- [ ] Signing/notarization status is accurate; this beta is currently unsigned and unnotarized.
- [ ] Release artifact SHA-256 hashes recorded and independently checked.
- [ ] Security reporting and supported-beta statements reviewed.

`PUBLIC_RELEASE=1 READPST_CORRESPONDING_SOURCE_DIR=/absolute/path/to/readpst-corresponding-source-0.6.76 bash
scripts/verify-macos-release.sh` is required for public binary publication. It must fail while the
ReadPST companion is absent, internally inconsistent, or not tied to the bundled sidecars. Passing
the local gate does not prove public delivery; release operators must also confirm that the archive
is available beside the DMG. See [READPST_CORRESPONDING_SOURCE.md](READPST_CORRESPONDING_SOURCE.md).

Unresolved compliance items are publication blockers, not automated failures to bypass.
