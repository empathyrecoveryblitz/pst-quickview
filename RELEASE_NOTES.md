# PST QuickView Release Notes

## 0.2.0-beta.3

Unsigned and unnotarized macOS public-beta candidate.

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
