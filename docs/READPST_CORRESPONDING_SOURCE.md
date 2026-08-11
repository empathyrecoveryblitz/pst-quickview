# ReadPST Corresponding Source release gate

**Technical preparation: COMPLETE**

**Public delivery beside the DMG: PENDING AND RELEASE-BLOCKING**

PST QuickView bundles ReadPST/LibPST under `GPL-2.0-or-later`. Every public DMG
must provide equivalent access to the exact Corresponding Source for its bundled
ReadPST sidecars. An upstream project link alone is not sufficient.

## Verified release inputs

- LibPST version: `0.6.76`
- Authoritative archive:
  `https://www.five-ten-sg.com/libpst/packages/libpst-0.6.76.tar.gz`
- Archive SHA-256:
  `3d291beebbdb48d2b934608bc06195b641da63d2a8f5e0d386f2e9d6d05a0b42`
- Local patch: `scripts/readpst-patches/0001-disable-msg-output.patch`
- Patch SHA-256:
  `73c319f11c42618707476f3cffaaf3238a667f48b6b8e32945665257b953a6b0`
- Compiler: Apple clang 17.0.0 (`clang-1700.0.13.5`)
- macOS SDK: 15.5
- Intel deployment target: macOS 10.13
- Apple Silicon deployment target: macOS 11.0
- Universal assembly: `lipo -create` over the independently built x86_64 and
  arm64 sidecars

The patch disables only ReadPST's optional MSG writer. PST QuickView uses
ReadPST for EML extraction. No other local source patches or generated source
replacements are used.

The previously committed sidecars could not be tied to retained exact source
bytes: the source archive and build log were absent, and the old build command
depended on an undocumented ambient `-liconv` input. The sidecars were therefore
rebuilt from the verified archive with the explicit patch and build environment
above. `src-tauri/binaries/README.md` records the resulting sidecar hashes.
The first provenance rebuild accidentally inherited macOS 15.0 from the host
SDK. The corrected build explicitly supplies macOS 10.13 to every x86_64
configure, compile, and link operation and macOS 11.0 to every arm64 operation.
The universal sidecar preserves those different per-slice minimums.

## Offline preparation

The preparation script never downloads, uploads, publishes, or reads Git
history. Supply the authoritative archive, a new destination, and the intended
HTTPS asset location explicitly:

```sh
scripts/prepare-readpst-corresponding-source.sh \
  /absolute/path/to/libpst-0.6.76.tar.gz \
  /absolute/output/readpst-corresponding-source-0.6.76 \
  https://github.com/empathyrecoveryblitz/pst-quickview/releases/download/v0.2.0-beta.3/readpst-corresponding-source-0.6.76.tar.gz
```

This is the intended `0.2.0-beta.3` release asset URL. It is recorded metadata,
not proof that the asset has been published; public delivery remains pending
until the archive is uploaded and independently accessible. The script refuses
existing output, verifies the exact intended URL plus the source and patch inputs, copies exact
build/verification scripts, records the bundled sidecar hashes, produces a
complete `MANIFEST.sha256`, creates a normalized deterministic `.tar.gz`, writes
its SHA-256 sidecar, and runs strict local verification.

The generated directory contains:

```text
readpst-corresponding-source-0.6.76/
├── README.md
├── MANIFEST.sha256
├── SOURCE_URL.txt
├── SOURCE_SHA256.txt
├── PUBLIC_DOWNLOAD_LOCATION.txt
├── DEPLOYMENT_TARGETS.txt
├── COPYRIGHT_NOTICES.md
├── LICENSE
├── SIDECAR_SHA256.txt
├── libpst-0.6.76.tar.gz
├── patches/
│   ├── README.md
│   └── 0001-disable-msg-output.patch
├── build/
    ├── BUILD_INSTRUCTIONS.md
    ├── build-readpst-sidecars.sh
    ├── verify-macos-readpst-bundle.sh
    └── verify-readpst-corresponding-source.sh
└── scripts/
    ├── macos-dylib-validation.sh
    └── test-macos-dylib-validation.sh
```

The adjacent files are:

```text
readpst-corresponding-source-0.6.76.tar.gz
readpst-corresponding-source-0.6.76.tar.gz.sha256
```

## Public-release verification

Before publishing a binary release, run:

```sh
PUBLIC_RELEASE=1 \
READPST_CORRESPONDING_SOURCE_DIR=/absolute/path/to/readpst-corresponding-source-0.6.76 \
bash scripts/verify-macos-release.sh
```

The gate validates the authoritative source hash, exact patch, license and
notices, exact build and dylib-verification scripts, complete manifest,
absence of private/message files, exact bundled-sidecar hashes and per-slice deployment targets,
deterministic archive contents, archive SHA-256, and concrete HTTPS publication
location. For this beta, the location must be:

```text
https://github.com/empathyrecoveryblitz/pst-quickview/releases/download/v0.2.0-beta.3/readpst-corresponding-source-0.6.76.tar.gz
```

After verification, upload the companion archive and checksum beside the DMG at
the recorded public location. Keep that source available on the terms and for
the period required by the applicable license. Technical preparation is not
public delivery; do not mark the release compliant until the source archive is
actually available to every binary recipient.

The load-command targets are packaging compatibility metadata, not a substitute
for clean-machine tests on supported Intel and Apple Silicon macOS versions.
