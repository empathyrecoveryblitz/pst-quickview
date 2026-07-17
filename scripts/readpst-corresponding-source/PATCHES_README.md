# Local patches

Exactly one local source patch is used for the bundled ReadPST 0.6.76 sidecars:

- `0001-disable-msg-output.patch`
- SHA-256: `73c319f11c42618707476f3cffaaf3238a667f48b6b8e32945665257b953a6b0`
- Apply from the extracted `libpst-0.6.76` root with:
  `patch --batch --forward -p1 < ../patches/0001-disable-msg-output.patch`

The patch compiles a small `write_msg_email` stub instead of the upstream
libgsf-based MSG writer. PST QuickView does not request MSG output from ReadPST;
it uses EML extraction only. The stub preserves that scope and emits a clear
error if MSG output is unexpectedly requested.

No other source patch, generated replacement, vendored library, or binary edit
is used. The upstream source archive already includes its generated `configure`,
`Makefile.in`, and supporting build files. Architecture-specific `config.h`,
Makefiles, objects, and binaries are generated normally during the documented
build and are not source inputs.
