# PST QuickView Release Notes

## 0.2.0-beta.4-hotfix.1

Unsigned and unnotarized macOS testing prerelease prepared for publication. Client import
success and duplicate resolution are not yet confirmed. Check GitHub Releases for availability.

### Large Message Import Recovery

- Discovery, identity hashing, and backend Full Verify stream complete extracted EML files
  without retaining their complete contents in memory. Hashes still cover all source bytes.
- EML files above the 256 MiB full-preview budget retain a canonical message row, available
  searchable headers, and an explicit limited-preview notice. Their body text and individual
  attachments are not parsed or indexed. Headers that are malformed or exceed the separate
  header budget may be unavailable; the message remains represented by a limited placeholder.
- **Save Source As** exports the complete original EML, including attachments. Original PST,
  EML, and MSG files remain read-only. This does not provide unlimited rendering or expand
  standalone MSG parsing limits.
- ReadPST output validation no longer rejects an individual extracted file solely because it
  exceeds 512 MiB. The source-scaled aggregate extraction budget and other process, file-type,
  entry-count, and filesystem safety checks remain active.
- Import completion and verification preserve notices for messages with limited previews;
  internal consistency is not proof that every body or attachment was parsed.

### Retrying An Existing Import

Keep the existing workspace and extracted EML files. After upgrading, reopen the same PST and
use **Resume Indexing** or **Repair Index** if offered. These paths reuse retained extraction
output. Preserve the original PST, current index, and workspace sidecars; do not remove them to
retry. This recovery does not require starting a fresh import or deleting a workspace.

### Known Limitations

- A successful retry of the reported large PST and historical duplicate resolution remain
  client-testing items. No end-to-end client result is claimed by this release preparation.
- A message over 256 MiB has limited header-based support, not full body search, full preview,
  or individual attachment access. Export the complete original EML for access in another viewer.
- Exact dependency resolutions are preserved. The development-only Browserslist 4.28.4
  dependency remains affected by [GHSA-73wf-gq98-2v4g](https://github.com/advisories/GHSA-73wf-gq98-2v4g),
  fixed in 4.28.7. That source-build advisory is unresolved; exposure through message viewing
  in a prebuilt Mac installer has not been demonstrated.
- The beta.4 limits below remain applicable, including incomplete clean-machine, Finder
  drag-and-drop, and packaged synthetic MSG coverage. Verify checks internal workspace
  consistency, not Outlook/Exchange parity. Suspected duplicates are never deleted automatically.

## 0.2.0-beta.4

Published unsigned and unnotarized macOS testing prerelease. Its existing release and assets
remain unchanged. Historical duplicate-import resolution is not confirmed; client testing
remains necessary.

### Security And Privacy Hardening

- JavaScript and Rust advisory remediation retains the exact reviewed dependency pins, including
  `time = "=0.3.51"`, `cfb = "=0.7.3"`, and `msg_parser = "=0.3.6"`.
- A hardened offline CSP, split main/preview capabilities, and constrained preview-window authority
  keep remote message resources disabled. Beta.4 provides no remote-content approval action.
- Filesystem and workspace checks preserve source PST, EML, and MSG files as read-only inputs.
  Attachment Open remains export-first and workspace deletion remains explicit and marker-gated.
- ReadPST execution is isolated and bounded. Unicode, SQLite, malformed-message, resource-limit,
  private-build-path, privacy-safe logging, and runtime-profile isolation defenses are retained.

### Import Integrity And Repair

- Resume is idempotent, uses stable message identities, and preserves a persistent incomplete
  state so interrupted work can continue without intentionally duplicating already indexed mail.
- Quick/Full Verify report internal workspace consistency. Repair Index and Rebuild Conversations
  use staged atomic replacement with rollback on failure.
- Possible-duplicate diagnostics are conservative. Suspected duplicates are never deleted
  automatically, and Repair does not modify the source PST or extracted EML/output files.

### Import Experience

- Import progress now distinguishes Preflight, Extracting from PST, Discovering message records,
  Indexing messages, Building conversations, Verifying import, and Complete.
- The display includes the PST basename and size, real counters, elapsed and per-stage time, last
  activity, qualified ETA, and background-import identity without inventing unsupported progress.

### Navigation And Folder Presentation

- Simplified hierarchy conservatively hides only proven technical wrappers; Original hierarchy is
  always available and Details retains the original folder path.
- Folder popovers dismiss correctly while preserving keyboard and focus behavior. These views are
  presentation only: they do not move or rename folders, and uncertain folders remain visible.

### Reversible Local Trash

- Messages can be moved to Local Trash, undone, or restored. Trashed messages are excluded from
  normal folder, search, and count views, and the state persists across restart and index repair.
- Local Trash is a reversible workspace-local overlay. It never changes the PST or canonical
  messages, does not reclaim storage, has no permanent purge, and remains limited to Messages.

### Existing Limited RTF Fallback

Beta.4 retains PST QuickView's existing limited RTF body fallback for certain PST-derived items and
standalone EML/MSG previews. It can extract readable plain text and, for some Outlook Rich Text
content containing embedded HTML, recover HTML that is passed through PST QuickView's normal
message sanitizer. This is not comprehensive RTF rendering. Formatting, embedded images, objects,
fields, links, and other rich-content fidelity may be omitted or reduced, and content absent from
the source cannot be reconstructed.

For PST-derived messages, the derived plain text is stored in the local workspace index and is
searchable. Standalone message previews are not indexed. Ordinary RTF attachments remain available
through the existing export-first Export/Open workflow. When a body-like RTF part is selected as
the message body, that part is hidden from the attachment list; the complete original EML or MSG
remains available through Save Source As.

The separately developed enhanced RTF converter prototype is not included in beta.4.

### Known Limitations

- The source lockfile retains the development-only Browserslist 4.28.4 dependency affected by
  [GHSA-73wf-gq98-2v4g](https://github.com/advisories/GHSA-73wf-gq98-2v4g) (fixed in 4.28.7).
  Malicious custom statistics can crash build tools or alter a temporary object's prototype.
  This remains unresolved for builds from source. No application import or custom statistics
  file was found in the reviewed tree; exposure through message viewing in the prebuilt Mac
  installer has not been demonstrated. The prepared installer and exact dependency locks are unchanged.
- The exact cause of historical duplicated import in a live workspace remains unproven. Beta.4
  adds idempotent Resume, verification, possible-duplicate diagnostics, and Repair, but never
  deletes suspected duplicates automatically.
- Blank or no-subject items fail open: filename alone never suppresses a message. They may be moved
  to Local Trash manually.
- Verify proves internal workspace consistency; it does not prove Exchange or Outlook historical
  parity and does not establish that it is safe to delete the original mailbox or PST.
- Local Trash is reversible and local only. It changes neither PST nor canonical messages, reclaims
  no storage, and provides no permanent purge.
- Folder hierarchy changes presentation only. No folder is moved or renamed, uncertain folders
  remain visible, and Original hierarchy is available.
- The inherited RTF fallback is limited, the enhanced converter prototype is excluded, formatting
  and rich-content fidelity may be reduced, and absent source content cannot be recreated.
- The proposed compact/collapsed attachment-section redesign is not included.
- OST and Windows are unsupported. This candidate is unsigned and unnotarized.
- Finder drag-and-drop non-interference remains a manual QA item.
- Packaged synthetic MSG runtime smoke is deferred when no valid fictional generator exists.
- The Linux zero-byte LibPST LSan case passed, but malformed-input testing is not exhaustive.
- PST QuickView does not claim full Outlook fidelity, complete calendar compatibility, Exchange or
  Outlook historical parity, or VoiceOver certification. Clean Intel and Apple Silicon user
  testing without Homebrew remains required before wider distribution.

## 0.2.0-beta.3

Published unsigned and unnotarized macOS public beta.

### Search Correctness And Safety

- Search parsing and compilation now live in a dedicated bounded Rust module. Query length,
  clause count, token, phrase, and structured-filter limits are validated before SQLite work.
- Date-only ranges use canonical UTC boundaries: the starting date is inclusive from midnight,
  and the ending date is represented by the exclusive start of the following day.
- Ordinary list, count, conversation, and search reads use validated read-only query connections
  instead of entering schema-migration transactions. Future, older, or incompatible workspace
  schemas are rejected without rewriting `PRAGMA user_version`.

### Request Lifecycle And Responsiveness

- One applied-search snapshot owns the main query, advanced text filters, Scope, folder, sort,
  mode, and workspace identity. Main and advanced text inputs share the existing 350 ms debounce.
- One search generation rejects stale message rows, conversations, expanded messages, counts,
  errors, loading state, and pagination updates.
- Superseded SQLite page, count, Load More, and expanded-conversation operations are physically
  interrupted. Cancellation is an expected lifecycle result and is not shown as a search error.
- Result pages and exact counts run independently, so first-page rows can appear before the exact
  total. Load More uses page-level `has_more` metadata and does not restart the count.

### Search Experience

- Applied filters appear as individually removable chips. Advanced Search reports its active
  filter count, and Clear All resets search constraints while preserving the global Scope
  preference.
- Empty, loading, no-result, and count-pending states distinguish active searches from normal
  unfiltered message browsing.
- SQLite-derived contextual snippets are centered around matches and use structural highlight
  ranges rendered as React text and `mark` elements. Compact badges identify actual Subject,
  Sender, Recipients, Body, and Attachment matches.

### Search Quality And Scaling

- Relevance sorting uses weighted SQLite FTS5 BM25 for text searches that resolve to exactly one
  PST. Newest remains the default and deterministic date/message-ID ties preserve pagination.
- Single-workspace Messages use stable opaque cursor pagination. True multi-PST Messages searches
  and Conversations retain deterministic offset pagination; raw BM25 scores are never compared
  across PST databases.
- Variable-height DOM windowing bounds mounted Messages, Conversations, and expanded-conversation
  rows while retaining loaded result objects in memory. Context snippets, badges, expansion, and
  selected message preview remain available.
- Result lists use one roving tab stop with Arrow, Home, End, Page Up, and Page Down navigation.
  Conversation headers and children also support Left/Right expand, collapse, child, and parent
  movement.

### Stabilization Evidence

- Deterministic synthetic SQLite datasets covered 10,000, 100,000, and 250,000 messages plus
  multiple workspaces, Unicode content, duplicate sort keys, folders, attachments, conversations,
  and predictable field-weight differences.
- Automated coverage exercises cancellation, cursor traversal, date boundaries, relevance,
  page-first/count-later behavior, contextual metadata, virtualization, and focus restoration.
- Bounded page-data eviction is deferred: measured retained-data growth remained reasonable for
  repeated explicit Load More operations, while mounted DOM stays bounded. Clean-machine and
  broader user testing remain important before wider distribution.

### Known Limitations

- Relevance applies only to an active text search targeting exactly one PST. It is unavailable for
  true multi-PST searches, non-text-only filters, and Conversations.
- True multi-PST Messages searches remain offset-based. Conversations are offset-based and do not
  use Relevance.
- Loaded result objects remain retained in memory even though mounted result DOM is bounded.
- This beta is unsigned and not notarized.
- PST QuickView does not claim full Outlook fidelity, complete calendar compatibility, complete
  legacy RTF reconstruction, or VoiceOver certification.
- Clean Intel and Apple Silicon systems without Homebrew still require broader release testing.

## 0.2.0-beta.2

Unsigned and unnotarized macOS public-beta candidate.

### Changes In Beta.2

- Modern macOS-oriented interface with compact desktop toolbars, multi-PST tabs, independent pane
  scrolling, adjustable layouts, and Outlook-style navigation options.
- System, Light, and Dark appearance modes, including contrast corrections across toolbars, tabs,
  folders, readers, popovers, form controls, dialogs, and standalone message windows.
- Restored PST sessions now resolve their saved or fallback folder and load Messages or
  Conversations immediately without requiring a folder click.
- Conversation rows show latest senders and participant summaries prominently beneath the subject.
- New original PST QuickView application icon.
- PST QuickView project licensing finalized as `GPL-3.0-or-later`; packaged applications include
  the project license, applicable GPL texts, and third-party notices.
- Bundled ReadPST/LibPST 0.6.76 provenance is tied to the exact verified source archive and local
  patch through an auditable Corresponding Source companion.
- Corrected ReadPST deployment targets: macOS 10.13 for x86_64 and macOS 11.0 for arm64, with the
  universal sidecar retaining each slice's separate minimum.

### Licensing

PST QuickView source code is licensed under `GPL-3.0-or-later`. Bundled ReadPST/LibPST remains
independently licensed under `GPL-2.0-or-later`, and other dependencies retain their own licenses.
See `LICENSE`, `LICENSES/`, and `THIRD_PARTY_NOTICES.md`. A public binary release additionally
requires the verified ReadPST Corresponding Source companion and checksum to be uploaded next to
the DMG. Technical companion preparation is complete; public delivery remains pending.

### Highlights

- Local-only, read-only PST browsing and SQLite FTS search.
- Bundled universal ReadPST/LibPST 0.6.76 built from checksum-verified source; Homebrew is not
  required by the packaged beta.
- Next-to-PST Spotlight-safe workspaces and App Support fallback.
- Multiple open PST tabs, folder roots, and cross-PST search.
- Messages and optional Conversation View.
- Safe standalone EML and Outlook MSG viewing.
- Legacy Outlook RTF body recovery and CID inline-image reconstruction.
- Best-effort Outlook appointment and meeting MSG preview.
- Finder Open With support for PST, EML, and MSG.
- Drag and drop for PST, EML, and MSG.
- Export-first attachment safety. Source PST, EML, and MSG files are never opened as writable
  export targets.
- Recent PSTs and optional previous-session restore.
- Print/Export viewer with printable HTML and byte-for-byte source message copying.
- Release diagnostics for version, architecture, ReadPST, workspace, schema, and conversation data.

### Conversation View

Older complete workspaces can open in Messages mode immediately. Conversation data requires
**Reindex Existing EMLs** when the workspace predates threading fields. Reindexing uses the
existing extracted EML files and does not run `readpst`.

### Calendar MSG Status

Outlook calendar and meeting MSG previews are supported on a best-effort basis. The feature is
covered by synthetic and unit tests but has not yet been manually validated with a real exported
appointment or meeting-request MSG. Time-zone and recurrence data may be incomplete for some
Outlook items.

### Known Limitations

- This beta is unsigned and not notarized.
- This beta does not claim full Outlook rendering fidelity or complete calendar compatibility.
- Outlook HTML and legacy Word/RTF layout reconstruction is intentionally simplified.
- Remote images are blocked unless approved for the current message.
- Attachment export remains one-at-a-time.
- Conversation quality depends on available Message-ID, References, In-Reply-To, participant, and
  subject data.
- Detailed Outlook recurrence expansion and reliable ICS generation are not implemented.
- Some binary Outlook time-zone structures are detected but not fully decoded.
- Real calendar/meeting MSG fixtures are not currently available for manual validation.
- A clean Intel Mac and clean Apple Silicon Mac still require final no-Homebrew installation tests.

### Installation

Copy `PST QuickView.app` to `/Applications`. For this unsigned internal beta, right-click the app
and choose **Open** for the first launch. Do not disable Gatekeeper globally.
