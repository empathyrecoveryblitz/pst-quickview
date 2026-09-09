# PST QuickView Security Architecture

PST QuickView is a local macOS viewer for PST, EML, and MSG files. This document
describes the security boundary being evaluated for the `0.2.0-beta.4`
hardening gate. It does not retroactively change the published
`0.2.0-beta.3` build.

Security statements below use two labels:

- **Implemented baseline** describes a control present in the current source.
- **Beta.4 gate requirement** describes a control that must be verified or
  completed before the security gate can pass.

A passing gate would establish only the tested properties of the reviewed
version. It would not prove the absence of unknown vulnerabilities or zero-day
flaws.

## Security goals

- Treat every PST, EML, MSG, MIME part, message field, attachment name, HTML
  fragment, workspace database, and sidecar output as hostile.
- Keep original PST, EML, and MSG sources read-only and byte-for-byte unchanged.
- Perform parsing, indexing, search, and preview locally, without telemetry,
  analytics, cloud parsing, cloud storage, or background update traffic.
- Render message content without script execution or access to Tauri authority.
- Require a new, validated export copy before an attachment can be opened by
  another application.
- Confine writes and deletion to positively identified PST QuickView workspace,
  temporary, and export roots.
- Fail closed when a boundary cannot be classified or tested adequately.

PST QuickView does not claim perfect Outlook rendering, complete recovery from
damaged archives, malware detection, or safety of an exported attachment in the
application that opens it.

## Trust boundaries

| Boundary | Untrusted input | Trusted component and intended control |
| --- | --- | --- |
| Source selection | Paths from dialogs, Finder, drag and drop, Recent, and file associations | Rust validates the supported type, canonicalizes the source, and opens it for reading. Source paths must never become write or delete targets. |
| PST extraction | PST bytes, names, folder trees, and ReadPST output | A separate ReadPST process extracts into one assigned workspace. Its arguments, environment, output, cancellation, and filesystem effects require independent confinement. |
| EML/MSG parsing | MIME, CFB/MAPI, RTF, calendar data, Unicode, lengths, and nesting | Rust parsers must reject or degrade malformed content without a panic, process-wide crash, unbounded allocation, or source modification. |
| Workspace | Extracted EML, metadata, logs, SQLite/WAL files, and old/tampered workspaces | Marker, identity, direct-child, schema, and root checks distinguish cache data from source data. A workspace remains user data, not disposable build output. |
| SQLite/search | Tampered databases, query text, filters, sort values, cursors, and FTS terms | Values are bound, grammar and sort fragments are allowlisted, reads use read-only connections, and cancellation interrupts SQLite work. Schema objects remain untrusted. |
| Message HTML | Active markup, URLs, CSS, CID content, tracking pixels, and malformed encodings | Backend sanitization emits a narrow HTML subset. The production CSP and WebView authority must provide independent defense in depth. |
| Tauri IPC | Calls from main and preview WebViews | Capabilities and backend command validation must grant each window only the authority it needs. A remote document must never receive local IPC authority. |
| Exports and macOS Open/Reveal | Hostile filenames, payloads, substitutions, links, and frontend paths | Exports must be newly and exclusively created beneath an app-controlled root. Open/Reveal must accept only backend-produced, revalidated paths. |
| Network | URLs embedded in mail, redirects, DNS results, and local/private destinations | Network access is disabled for message content until a mediated design can enforce destination and privacy policy. |
| Diagnostics and logs | Errors and operational metadata derived from private mail and paths | Logging must be bounded and redacted. Diagnostics are user initiated and must be reviewed before sharing. |
| Distribution | App, nested ReadPST binary, DMG, signatures, and notarization | Release verification checks exact artifacts. Signing and notarization are separate distribution-trust requirements. |

## Local-only and read-only source model

The application is designed to open source PST, EML, and MSG files for reading.
PST identity uses canonical path and file metadata plus a content fingerprint;
standalone EML and MSG inputs are canonicalized and format checked before use.
The index, extracted EML, logs, and exports are written elsewhere.

**Implemented baseline:** source parsing uses read operations, selected
standalone files are held behind bounded backend identity records, and optional
external MSG fixture tests compare a source SHA-256 before and after parsing.
PST workspace deletion requires an exact structured marker, validates any index
read-only, and isolates the validated directory inode before removing only the
cache.

**Beta.4 gate requirement:** the hostile-input and failure matrix must also
prove that source bytes and relevant metadata remain unchanged across success,
cancellation, parser failure, disk-full conditions, temporary-directory
failure, and sidecar termination. Path substitution, symlink, and hardlink
races must fail closed rather than relying only on an earlier canonicalization.

## Workspace storage

Workspaces may be stored:

- beside a PST under `.pst-quickview.noindex/<fingerprint>/`;
- under the legacy `.pst-quickview/<fingerprint>/` location; or
- under `~/Library/Application Support/PST QuickView/workspaces/`.

The last location is an explicit compatibility rule for the exact production
identifier `com.local.pstquickview`; it keeps existing beta.3 workspaces and
session references discoverable without moving, copying, or opening the normal
profile during validation. Every other packaged identifier uses Tauri's
identifier-scoped application-data, log, cache, and configuration roots. Its
application-owned workspaces and standalone exports descend from that alternate
data root, with no fallback to the production directory. Next-to-PST roots are
selected and validated independently of the packaged identity.

A workspace contains a marker, metadata, extracted EML, a SQLite index and WAL
files, logs, and exports. Standalone EML/MSG exports use separate Application
Support roots. These directories contain private mailbox-derived data and must
not be uploaded, included in public fixtures, or treated as disposable solely
because Git does not track them.

**Implemented baseline:** workspace IDs are constrained, expected workspace
locations are direct children of known roots, stored sessions are checked
against their marker and source identity, and missing or contradictory markers
cannot authorize deletion. New marker and workspace files use private modes,
and symlink, hardlink, and inode substitution checks fail closed.

**Beta.4 gate requirement:** creation and deletion must be no-follow,
race-resistant operations with restrictive permissions. The final gate must
cover symlinked parents, replacement after validation, hardlinks, case and
Unicode-normalization collisions, file/directory substitution, disappeared
mounts, read-only and network volumes, and partial deletion. No recursive
operation may escape the validated workspace root.

## HTML rendering and remote content

### Sanitization baseline

Message HTML is sanitized in the Rust backend before React inserts it into a
preview. The sanitizer uses an allowlist of document and table elements,
bounded legacy layout attributes, CID images that are converted to bounded
`data:image` URLs, and safe image data URLs. It removes generic styling and
event-handler attributes and drops active elements including scripts, frames,
objects, embeds, forms, metadata, and linked resources.

This sanitizer is a primary boundary, but it is not the only boundary. The
production CSP must continue to deny remote scripts, frames, plugins, form
submission, base-URL changes, and development-server origins. Sanitized message
content must never create a WebView or invoke Tauri IPC.

### Remote-content policy

**Beta.4 policy: message-triggered network loading is disabled until a mediated
image-fetch design is implemented and independently reviewed.** A UI approval
must not directly expose a mailbox-supplied URL to the WebView.

The published beta.3 baseline can permit direct `http:` and `https:` image URLs
after a per-message action. The beta.4 hardening branch removes those schemes
from production `img-src` and from the sanitizer allowlist, removes the backend
approval flags, and removes the frontend approval actions. Runtime network
observation still requires final validation. Direct WebView loading cannot
reliably enforce DNS rebinding, redirect, loopback, private, link-local,
multicast, credential-bearing URL, cookie, referrer, or tracking constraints;
restoring it is not an accepted residual risk.

Sanitized HTML and printable exports retain only resolved local CID images and
allowlisted inert data images. Printable exports execute outside the app CSP,
so the backend treats the complete frontend document as untrusted, applies the
same mail sanitizer again, discards supplied scripts, styles, links, forms,
frames, and remote resources, and rebuilds the result inside a fixed offline
document with its own restrictive CSP. Printable input is capped at 128 MiB.

Before remote images can be restored, a mediator must at minimum:

- accept only explicitly approved image requests for the current message;
- allow only the required HTTPS image formats and bounded response sizes;
- reject credentials and unsafe URL forms;
- resolve and reject loopback, private, link-local, multicast, and otherwise
  non-public destinations for IPv4 and IPv6;
- revalidate every redirect and DNS result;
- send no cookies, authorization, mailbox-derived headers, or referrer;
- avoid adding message data to query parameters;
- return inert image bytes rather than scriptable or framed content; and
- keep remote documents outside all Tauri capability and IPC scopes.

Until those properties are implemented and tested, production CSP must not
allow remote image schemes and the approval action must remain unavailable.

## CSP and Tauri capabilities

The beta.4 hardening branch changes production CSP to `default-src 'none'`,
limits scripts to local application code, limits images to local and `data:`
content, limits connection endpoints to Tauri IPC, and denies objects, base
elements, forms, frames/ancestors, child content, workers, media, fonts, and
manifests. Inline style remains necessary for the local application UI; inline
script and `unsafe-eval` are not allowed. Development-server configuration is
kept in `tauri.dev.conf.json` and does not appear in the production CSP.

The hardening branch also replaces the shared capability with separate `main`
and `message-preview-*` capabilities. Build-time generated allow permissions
make registered application commands deny-by-default unless a window
capability names them. The preview capability omits picker, import, indexing,
search, diagnostics, workspace deletion, and WebView-creation authority. No
capability declares broad shell or filesystem plugin permission.

The frontend has no generic WebView/window-construction permission. A
main-window-only backend command validates a workspace/message identity or a
previously authorized standalone-file identifier, chooses an opaque bounded
window label, and creates only the fixed local preview route. Preview windows
resolve their typed target from backend state associated with their own label;
they cannot choose a URL, initialization script, capability, or child window.

The beta.4 review must still:

- verify the exact packaged CSP, not only the source JSON;
- bind the packaged effective capability set to the reviewed source configuration;
- prove that sanitized content cannot trigger arbitrary top-level navigation;
- verify the packaged app contains no generic frontend WebView-creation
  permission and retains the fixed backend preview route;
- verify capability generation fails closed when the registered command list
  and allowlist drift;
- confirm preview capabilities cannot union into main-window authority;
- verify every custom Tauri command validates identifiers and backend-owned
  paths instead of accepting arbitrary frontend paths; and
- prove that no remote WebView origin or message document can access IPC.

Passing a JSON shape check alone is not evidence that the runtime authority
model is correct. Tauri can merge build-time configuration supplied outside the
repository, so source allowlists and an embedded-CSP check do not by themselves
prove the effective capability set in an already-built package. Release evidence
must constrain the build invocation or independently inspect the packaged ACL.

## Attachment export and external applications

Attachment **Open** follows export-first behavior: the backend locates the
attachment by workspace/message/attachment identity, sanitizes its filename,
chooses a unique name beneath an export root, writes a separate copy, checks
that the result is under that root, and only then asks macOS to open it.
Standalone EML and MSG attachments use format-specific export roots. OLE object
attachments are not opened automatically.

An export remains untrusted content. PST QuickView does not scan it, and the
receiving application becomes a separate security boundary.

Exports use exclusive no-overwrite creation, restrictive modes, collision-safe
names, regular single-link file checks, and bounded backend records containing
the created file's identity and digest. Open and Reveal reject arbitrary,
changed, disappeared, or substituted paths. File-dialog, Finder-open, and
backend drag/drop selection similarly mint bounded standalone-message records;
standalone preview and export commands accept only their opaque stable IDs.
License, notices, logs, and workspace actions remain fixed-resource or
backend-derived operations.

## ReadPST sidecar

PST extraction uses the bundled LibPST/ReadPST 0.6.76 sidecar when available.
The published source, local patch, per-architecture binaries, and universal
binary have provenance and byte/architecture verification tooling. The local
patch disables ReadPST's unused MSG output mode.

**Implemented baseline:** ReadPST is launched with `Command::new`, not a shell,
from a fixed bundled sidecar location. There is no ambient `PATH` fallback.
Options and canonical source/output paths are separate arguments and `--`
terminates option parsing. The child runs in its own process group with a
cleared, minimal environment and private temporary directory. Cancellation,
stall, absolute-time, stream-output, extracted-entry, per-file, and total-byte
limits terminate the complete process group. Stdout and stderr are drained into
bounded in-memory tails and only byte counts are logged.

Extraction first lands in a private `extracting` directory. Every output entry
is inspected without following links; symlinks, hardlinks, nonregular files,
cross-device cleanup roots, and oversized output fail closed. Successful output
is renamed into place only after validation. Failure and cancellation remove
partial output from the exact assigned root. The source PST is hashed and its
device, inode, size, modification time, and link count are revalidated after the
child exits.

**Residual beta.4 gate work:** ReadPST is still native code and is not an OS
sandbox. The final gate must record external sanitizer coverage where supported
and retain the packaged architecture, dependency, provenance, and corresponding
source checks.

ReadPST is native code and is not an OS sandbox. A supported external
ASan/UBSan build from the verified source must exercise synthetic malformed
inputs. Reproducing a native memory-safety flaw blocks release until the source
patch, rebuilt binaries, corresponding source, and regression evidence are all
complete.

## SQLite, FTS, and cursors

**Implemented baseline:** search values use SQLite parameters. Search grammar,
FTS terms, and sort choices are compiled from bounded allowlists rather than
accepting raw SQL or raw FTS syntax. Query paths open the database read-only,
set `query_only`, retain WAL visibility, verify schema version and required
columns, and use SQLite interrupt handles for cancellation. Page sizes are
bounded.

Single-workspace cursors are fixed-size opaque values containing hashes rather
than cleartext search text, paths, or subjects. A per-process secret integrity
tag binds cursor version, sort, search generation, workspace, criteria, index
generation, and message position. Malformed, tampered, wrong-context, and stale
cursors are rejected.

The beta.4 gate must additionally prove and, where compatible, enable:

- `PRAGMA trusted_schema=OFF` on every connection;
- SQLite defensive mode and disabled extension loading;
- page, database, expression, result, and query-progress limits;
- fail-closed handling of malicious schema objects;
- rejection of future schemas on every write/migration path without lowering
  `user_version`; and
- compatibility with existing valid schema-v3 workspaces and active WAL reads.

The baseline read path rejects old and future schemas without migrating, but
that does not by itself validate every upgrade and writer path.

## Resource limits and cancellation

The application bounds page sizes, offsets, cursor size, search grammar
complexity, printable HTML, message source/text size, MIME depth, parts,
headers, boundaries, attachments and recipients, standalone attachment export
size, embedded image size, queued external file opens, open workspaces,
multi-workspace request vectors, preview windows, mounted result rows, and
cancellation registry lifetime. At most 64 workspaces can be open or targeted
by one request, and offset-based fallback requests reject offsets above
5,000,000 rows. Combined counts use checked arithmetic.

ReadPST has absolute-time, stall, stream, per-file, entry-count, and total-byte
limits. Search operations register SQLite interrupt handles. Deterministic
tests cover parser boundaries, malformed cursor storms, repeated cancellation,
workspace capacity, temporary-output failure, disk-space preflight, sidecar
output floods, and 100,000 logical frontend entries. The ignored Search 2.0
stress gate uses synthetic 10,000, 100,000, and 250,000-message workspaces.
Diagnostic timings may be reported, but fragile wall-clock thresholds are not
mandatory CI assertions.

## Privacy, logging, and diagnostics

The product policy forbids telemetry, analytics, cloud processing, and
background network requests. Message bodies, HTML, headers, subjects,
participants, attachment payloads, search text, remote URLs, SQL, credentials,
and tokens must not be written to logs.

Application and workspace logs rotate with bounded backup counts, use private
directory/file modes, and reject symlinked, hard-linked, or nonregular log
paths. Import logs omit source/workspace/helper paths, fingerprints, extracted
filenames, and parser error text. Export logs identify operations by local
numeric IDs rather than attachment filenames or export paths. Application logs
record only whether an error occurred, not the backend error text. ReadPST
output is drained into bounded in-memory tails and only byte counts and
truncation state are logged.

Diagnostics and Finder reveal actions are user initiated. Diagnostics may show
private paths only after the user opens the Diagnostics section, and the UI
warns the user to review the content before explicitly copying it. Cancellation
is treated as expected lifecycle control rather than a logged application
failure.

## Security reporting

Do not publish a suspected vulnerability or attach real mail to a public issue.
Use GitHub Private Vulnerability Reporting as documented in
[`SECURITY.md`](../SECURITY.md). Reports should use synthetic reproduction data
and non-sensitive diagnostics. Never include real PST, OST, EML, MSG, mailbox,
workspace, log, credential, or token material without prior authorization.

## Distribution trust

The Tauri configuration enables the macOS hardened runtime and declares no
custom entitlements. The application and nested ReadPST executable are designed
to be signed in the correct inside-out order before notarization.

The published beta.3 is unsigned and unnotarized. Developer ID identity and
notarization-credential availability are environment/operator facts and are not
application-code vulnerabilities. A future release still requires nested-code
signing, app and DMG verification, notarization, stapling, Gatekeeper
assessment, and clean Intel and Apple Silicon testing. See
[`MACOS_SIGNING.md`](MACOS_SIGNING.md). Never disable Gatekeeper globally.

## Known residual risks and release blockers

- PST, MSG/CFB, EML/MIME, RTF, calendar, HTML, SQLite, and ReadPST parsers
  process complex hostile formats; bounded testing cannot prove all inputs safe.
- Remote image loading must remain disabled until mediation and private-network
  enforcement exist.
- Remote-resource stripping, printable-export behavior, and constrained preview
  creation still require packaged runtime validation.
- The effective capability set in a packaged application still needs evidence
  that rules out build-time configuration overrides; source-shape checks alone
  are insufficient.
- Sidecar environment, output, link, and resource confinement require the
  beta.4 controls described above.
- Exported attachments remain untrusted and may exploit another application.
- Workspace databases and extracted messages are local plaintext caches subject
  to the user's filesystem and account protections.
- Logging/path redaction and restrictive permission checks have deterministic
  regression coverage; packaged runtime observation remains release-gate
  evidence.
- Rust CodeQL coverage, public dependency-alert state, native sanitizer runs,
  network observation, and the complete malformed/fuzz matrix are release-gate
  evidence, not assumptions.
- Signing and notarization remain future distribution-trust work.

Any unresolved high, critical, unclassified, or release-relevant finding
requires `HOLD — UNRESOLVED SECURITY FINDINGS`.

## Future OST requirements

OST support is not implemented by this hardening work. Any future OST alpha
must:

- isolate the native parser in a subprocess boundary;
- provide cancellation and explicit memory, CPU, time, output, and disk limits;
- handle damaged, encrypted, and locked files without changing the source;
- disclose incomplete-cache behavior clearly; and
- never promise complete mailbox recovery.

## Future Windows requirements

Windows support is not implemented by this hardening work. Any future Windows
version must address:

- the WebView2 content and IPC boundary;
- restrictive AppData workspace, log, temporary, and export permissions;
- safe Explorer file associations and frontend-independent path validation;
- Outlook source-file lock detection;
- sidecar integrity and architecture verification;
- rollback-safe installer upgrades;
- Authenticode signing and SmartScreen reputation; and
- process invocation with separate arguments and no shell command construction.
