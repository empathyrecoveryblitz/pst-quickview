# Third-party notices

PST QuickView incorporates third-party software. PST QuickView's original source code is licensed
under `GPL-3.0-or-later`; this file does not replace or alter any third-party license.

## ReadPST / LibPST

- Packaged version: 0.6.76 (verified with the bundled executable's `-V` output)
- License: GPL-2.0-or-later
- Copyright/attribution: David Smith, Joe Nahmias, 510 Software Group, and other libpst contributors; see upstream source notices
- Upstream: https://www.five-ten-sg.com/libpst/
- Exact source archive: `libpst-0.6.76.tar.gz`, SHA-256 `3d291beebbdb48d2b934608bc06195b641da63d2a8f5e0d386f2e9d6d05a0b42`

The included macOS ReadPST binaries contain statically linked LibPST objects and dynamically link
only to macOS system libraries (`libiconv`, `libz`, `libc++`, and `libSystem`). The sidecars were
rebuilt from the exact archive above using Apple clang 17.0.0 (`clang-1700.0.13.5`), macOS SDK
15.5, and the explicit patch `scripts/readpst-patches/0001-disable-msg-output.patch`. That patch
disables the optional MSG writer; PST QuickView uses ReadPST only for EML extraction. No other
local source patches or generated replacements are used. Exact commands and toolchain checks are
in `scripts/build-readpst-sidecars.sh` and the generated Corresponding Source companion.

The x86_64 sidecar is built with a macOS 10.13 deployment target and the arm64
sidecar with macOS 11.0. The universal binary retains those separate per-slice
minimums. An earlier macOS 15.0 minimum inherited from the host SDK was
corrected through a source rebuild. Deployment metadata does not replace clean
machine compatibility testing.

Every distributed DMG must provide equivalent access to the exact Corresponding Source used for
the bundled ReadPST binary. The release companion must include the exact libpst version, exact
source archive, archive SHA-256, all local patches or generated replacements, configure/build
commands, the exact sidecar build script, the applicable license text, upstream copyright notices,
and a clear source download location next to the DMG. An upstream URL alone does not complete this
requirement.

The official GPL version 2 text is included at `LICENSES/GPL-2.0-or-later.txt`. Offline companion
preparation and verification tooling is complete. The `v0.2.0-beta.3` prerelease publishes the exact
generated companion archive beside the DMG, with both hashes recorded in `SHA256SUMS.txt`. Public
access to the archive and its SHA-256,
`a858ea017bb80516b42b14da8b624530968c70a6daf5bfc7fad628a631a88787`, was verified after
publication. Future binary releases must repeat the documented `PUBLIC_RELEASE=1` verification and
provide equivalent source access. See `docs/READPST_CORRESPONDING_SOURCE.md`.

## Application dependencies

Rust and npm dependency versions are pinned by `src-tauri/Cargo.lock` and `package-lock.json`.
License identifiers must be taken from verified package metadata and bundled license files; Cargo.lock
does not record them. Key runtime components include Tauri (Apache-2.0 OR MIT), React/React DOM
(MIT), Ammonia (Apache-2.0 OR MIT), rusqlite, mailparse, cfb, and msg_parser. Release review must
generate and retain a complete lockfile-derived inventory plus verified license metadata and copies
of required license texts; do not infer license terms from package names.

## Assets

The PST QuickView application icon is original project artwork owned by Kev P,
Copyright (C) 2026 Kev P, and distributed with the project under
`GPL-3.0-or-later`. Interface icons are local inline SVG/CSS only; no remote
icon or font service is used.
