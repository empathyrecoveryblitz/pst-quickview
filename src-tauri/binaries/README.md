# Bundled readpst binaries

Tauri looks for sidecar binaries named:

- `readpst-x86_64-apple-darwin`
- `readpst-aarch64-apple-darwin`
- `readpst-universal-apple-darwin`

The current sidecars are built from the authoritative `libpst 0.6.76` archive:

- URL: `https://www.five-ten-sg.com/libpst/packages/libpst-0.6.76.tar.gz`
- SHA-256: `3d291beebbdb48d2b934608bc06195b641da63d2a8f5e0d386f2e9d6d05a0b42`

The build uses:

- static libpst objects
- Native Language Support disabled
- Python disabled
- `scripts/readpst-patches/0001-disable-msg-output.patch` to disable ReadPST
  `.msg` output and avoid `libgsf`/`glib`
- Apple clang 17.0.0 (`clang-1700.0.13.5`) and macOS SDK 15.5
- explicit deployment targets: macOS 10.13 for x86_64 and macOS 11.0 for arm64
- base-system tools only after toolchain validation

PST QuickView uses readpst for EML extraction, not `.msg` output. The sidecars should only link to macOS system libraries such as `/usr/lib/libiconv.2.dylib`, `/usr/lib/libz.1.dylib`, `/usr/lib/libc++.1.dylib`, and `/usr/lib/libSystem.B.dylib`.

Current sidecar SHA-256 values:

```text
500857ad0bcce39e8353cbd32cccd4789d6e84e8d082f188b7ba5da0f75f069b  readpst-x86_64-apple-darwin
31a374e4d08ceb34149222d3eebb08642cb65d88015250b0931abd5c9a8839fe  readpst-aarch64-apple-darwin
95b378b108b17c7fd26f697f59a17e3915f9ea6a7435ced1ed26197c872aaae4  readpst-universal-apple-darwin
```

The universal binary retains macOS 10.13 in its x86_64 slice and macOS 11.0 in
its arm64 slice. The earlier macOS 15.0 load-command minimum was accidental and
has been corrected by rebuilding, not by post-processing the binaries. These
targets do not replace clean-machine compatibility testing.

The build script does not download source. Rebuild sidecars by supplying the
verified archive explicitly:

```sh
READPST_SOURCE_ARCHIVE=/absolute/path/to/libpst-0.6.76.tar.gz \
scripts/build-readpst-sidecars.sh
```

Verify packaged app sidecar linkage with:

```sh
scripts/verify-macos-readpst-bundle.sh
```

Prepare the public Corresponding Source companion with
`scripts/prepare-readpst-corresponding-source.sh`. See
`docs/READPST_CORRESPONDING_SOURCE.md`; uploading the verified companion next
to a DMG remains a separate release step.
