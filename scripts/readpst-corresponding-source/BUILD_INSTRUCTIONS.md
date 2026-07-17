# Build ReadPST 0.6.76 macOS sidecars

These instructions reproduce the source and command inputs used for PST
QuickView's bundled ReadPST sidecars. Mach-O UUIDs may differ across Apple
toolchain releases; release sidecars must therefore be rebuilt with the exact
recorded toolchain unless a new companion records the replacement environment.

## Recorded build host

- macOS host tested: 15.7.3 on x86_64
- Xcode Command Line Tools with macOS SDK 15.5
- Apple clang 17.0.0 (`clang-1700.0.13.5`)
- Apple linker recorded in the resulting Mach-O load command: tool version 1167.5
- Base-system `make`, `patch`, `tar`, `lipo`, `otool`, `file`, `shasum`, and
  `install`

Homebrew is not required. The build script restricts `PATH` to Apple/base-system
locations after validating the toolchain. It statically links LibPST objects and
dynamically links only macOS system `libz`, `libiconv`, `libc++`, and `libSystem`.

## Inputs

From the companion root, verify the source and patch:

```sh
shasum -a 256 libpst-0.6.76.tar.gz
shasum -a 256 patches/0001-disable-msg-output.patch
```

Expected values:

```text
3d291beebbdb48d2b934608bc06195b641da63d2a8f5e0d386f2e9d6d05a0b42  libpst-0.6.76.tar.gz
73c319f11c42618707476f3cffaaf3238a667f48b6b8e32945665257b953a6b0  patches/0001-disable-msg-output.patch
```

The script performs those checks again before compilation. For manual source
inspection, extract and patch with:

```sh
mkdir libpst-source
tar -xzf libpst-0.6.76.tar.gz -C libpst-source --strip-components=1
patch --batch --forward -d libpst-source -p1 \
  < patches/0001-disable-msg-output.patch
```

## Exact automated build

Run from the companion root:

```sh
READPST_SOURCE_ARCHIVE="$PWD/libpst-0.6.76.tar.gz" \
READPST_PATCH_FILE="$PWD/patches/0001-disable-msg-output.patch" \
READPST_OUTPUT_DIR="$PWD/out" \
BUILD_ROOT="/tmp/pst-quickview-readpst-0.6.76" \
READPST_X86_64_DEPLOYMENT_TARGET="10.13" \
READPST_ARM64_DEPLOYMENT_TARGET="11.0" \
bash build/build-readpst-sidecars.sh
```

The script configures each architecture with the following compiler inputs:

```text
x86_64:
  MACOSX_DEPLOYMENT_TARGET=10.13
  CC="clang -arch x86_64 -mmacosx-version-min=10.13"
  CXX="clang++ -arch x86_64 -mmacosx-version-min=10.13"
  CPPFLAGS="-mmacosx-version-min=10.13"
  CFLAGS="-O2 -mmacosx-version-min=10.13"
  CXXFLAGS="-O2 -mmacosx-version-min=10.13"
  LDFLAGS="-mmacosx-version-min=10.13"

arm64:
  MACOSX_DEPLOYMENT_TARGET=11.0
  CC="clang -arch arm64 -mmacosx-version-min=11.0"
  CXX="clang++ -arch arm64 -mmacosx-version-min=11.0"
  CPPFLAGS="-mmacosx-version-min=11.0"
  CFLAGS="-O2 -mmacosx-version-min=11.0"
  CXXFLAGS="-O2 -mmacosx-version-min=11.0"
  LDFLAGS="-mmacosx-version-min=11.0"

LIBS=-liconv
```

It uses these configure options:

```text
--host=x86_64-apple-darwin or --host=aarch64-apple-darwin
--build=<host-architecture>-apple-darwin
--disable-dependency-tracking
--disable-python
--disable-nls
--enable-static-tools
--disable-libpst-shared
--disable-shared
GSF_CFLAGS=' '
GSF_LIBS=' '
ZLIB_CFLAGS=' '
ZLIB_LIBS='-lz'
```

Autoconf cache values used for cross-compilation are listed directly in the
exact build script. The build repeats all architecture-specific deployment
inputs so generated makefiles cannot discard the intended target. The x86_64
invocation is equivalent to:

```sh
MACOSX_DEPLOYMENT_TARGET=10.13 \
make -C src -j"$(sysctl -n hw.ncpu)" readpst \
  CC="clang -arch x86_64 -mmacosx-version-min=10.13" \
  CXX="clang++ -arch x86_64 -mmacosx-version-min=10.13" \
  CPPFLAGS="-mmacosx-version-min=10.13" \
  CFLAGS="-O2 -mmacosx-version-min=10.13" \
  CXXFLAGS="-O2 -mmacosx-version-min=10.13" \
  LDFLAGS="-mmacosx-version-min=10.13" \
  LIBS='-liconv'
```

The arm64 invocation is identical except for `-arch arm64` and deployment
target `11.0`. No post-link load-command patching is used.

The universal binary is created with:

```sh
lipo -create \
  out/readpst-x86_64-apple-darwin \
  out/readpst-aarch64-apple-darwin \
  -output out/readpst-universal-apple-darwin
```

`lipo` preserves the deployment metadata in each input slice: the universal
x86_64 slice remains `10.13`, while the universal arm64 slice remains `11.0`.

## Verification

The build script performs these checks before copying staged outputs:

```sh
file out/readpst-*
lipo -archs out/readpst-x86_64-apple-darwin
lipo -archs out/readpst-aarch64-apple-darwin
lipo -archs out/readpst-universal-apple-darwin
otool -L out/readpst-*
xcrun vtool -arch x86_64 -show-build \
  out/readpst-x86_64-apple-darwin
xcrun vtool -arch arm64 -show-build \
  out/readpst-aarch64-apple-darwin
xcrun vtool -arch x86_64 -show-build \
  out/readpst-universal-apple-darwin
xcrun vtool -arch arm64 -show-build \
  out/readpst-universal-apple-darwin
out/readpst-universal-apple-darwin -V
```

Expected version: `ReadPST / LibPST v0.6.76`.

The x86_64 sidecar must contain only `x86_64`, the arm sidecar only `arm64`,
and the universal sidecar both. Dynamic dependencies must be under `/usr/lib`
or `/System/Library/Frameworks`; Homebrew and user-specific paths are rejected.
The standalone and universal x86_64 load commands must report macOS `10.13`;
the standalone and universal arm64 load commands must report macOS `11.0`.

Compare the resulting hashes with `SIDECAR_SHA256.txt`. Exact hashes require
the recorded toolchain and inputs. The source and build material remain valid
Corresponding Source even where Mach-O UUID generation prevents bit-for-bit
identity on a different Apple toolchain.

## Repository mapping

After all checks pass, release maintainers map the outputs as follows:

```text
out/readpst-x86_64-apple-darwin    -> src-tauri/binaries/readpst-x86_64-apple-darwin
out/readpst-aarch64-apple-darwin   -> src-tauri/binaries/readpst-aarch64-apple-darwin
out/readpst-universal-apple-darwin -> src-tauri/binaries/readpst-universal-apple-darwin
```

Do not copy partial or unverified outputs. Do not sign, package, or publish as
part of this source build.

## Known limitations

- The previous sidecars accidentally inherited a macOS 15.0 minimum from the
  host SDK. The corrected build asserts macOS 10.13 for x86_64 and macOS 11.0
  for arm64 at configure, compile, and link time.
- A Mach-O deployment target is a compatibility declaration, not proof that
  every supported workflow works on that OS. Clean-machine testing is still
  required on Intel and Apple Silicon Macs.
- The arm64 sidecar is cross-compiled on the recorded Intel host. Its version
  markers, architecture, and linkage are checked there; execute it on Apple
  Silicon during clean-machine release testing.
- ReadPST's optional MSG output is intentionally disabled. PST QuickView uses
  EML extraction.
- A real read-only PST import smoke test remains a separate application-level
  release check and must confirm the PST hash is unchanged.
