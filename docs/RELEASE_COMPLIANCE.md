# Release compliance checklist

- [x] Owner selected `GPL-3.0-or-later`; root license and package metadata updated.
- [x] Official GPL version 3 and GPL version 2 texts included without project commentary.
- [x] Project copyright notice and contribution licensing terms documented.
- [ ] Complete lockfile-derived third-party inventory and all required dependency notices reviewed.
- [x] ReadPST companion preparation tooling records the exact version, source archive, SHA-256, every local patch or
  generated replacement, configure/build commands, sidecar build script, license text, copyright
  notices, sidecar hashes, per-architecture deployment targets, complete manifest, and intended
  public download location.
- [x] The verified ReadPST Corresponding Source archive and checksum are uploaded and available
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
- [x] Signing/notarization status is accurate; this beta is unsigned and unnotarized.
- [x] Release artifact SHA-256 hashes are recorded and independently checked: DMG
  `b29ed3295e0bbbdcad4bd88621972a609c260601bda34808974c234a8785efad` and ReadPST
  Corresponding Source
  `a858ea017bb80516b42b14da8b624530968c70a6daf5bfc7fad628a631a88787`.
- [x] Security reporting and supported-beta statements reviewed.

`PUBLIC_RELEASE=1 READPST_CORRESPONDING_SOURCE_DIR=/absolute/path/to/readpst-corresponding-source-0.6.76 bash
scripts/verify-macos-release.sh` is required for public binary publication. It must fail while the
ReadPST companion is absent, internally inconsistent, or not tied to the bundled sidecars. Passing
the local gate does not prove public delivery; release operators must also confirm that the archive
is available beside the DMG. That confirmation was completed for `v0.2.0-beta.3`, including public
download and hash verification of the DMG, source archive, and `SHA256SUMS.txt`. See
[READPST_CORRESPONDING_SOURCE.md](READPST_CORRESPONDING_SOURCE.md).

Unchecked items remain explicit follow-up work and must not be silently treated as complete.
