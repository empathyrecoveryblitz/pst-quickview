# PST QuickView Internal Beta Test Plan

Version under test: `0.2.0-beta.5`

Record the Mac model, macOS version, CPU architecture, app path, DMG filename, and whether
Homebrew is installed before testing. Use disposable copies of test files where possible and
record SHA-256 hashes for source-integrity checks.

## Installation

- Copy `PST QuickView.app` to `/Applications`.
- For this unsigned internal beta, use Finder's right-click **Open** action for the first launch.
- Never disable Gatekeeper globally.
- If the approved internal workflow requires removing quarantine, remove it only from
  `/Applications/PST QuickView.app`.
- Confirm About shows `0.2.0-beta.5`.
- Expand About > Diagnostics and record macOS, CPU, executable architecture, and readpst source.

## PST

- Open a PST from the toolbar.
- Open a PST with Finder > Open With > PST QuickView.
- Drag a PST onto the main window.
- Import a new small PST.
- Reopen a complete workspace without running `readpst` again.
- Open an incomplete workspace and test Resume, Reimport, Delete, and Cancel.
- Exercise disk-space preflight, including Continue Anyway and App Support fallback.
- Confirm Diagnostics reports a bundled readpst source.
- Test an older ANSI PST when a safe fixture is available.
- Confirm invalid or truncated PST files show a readable error with technical Details.

## Beta.5 Large EML Recovery

This matrix is a test plan, not a claim of completed client or packaged-app testing. Use an
isolated synthetic corpus and retain before/after source hashes. Never remove an important
workspace or source file merely to obtain a test result.

- Include extracted EMLs below, at, and above 256 MiB, plus one above 512 MiB. Use synthetic
  headers and content; confirm complete-file hashes and byte lengths remain unchanged.
- Confirm discovery and backend Full Verify stream large files with bounded working memory,
  activity updates, and responsive cancellation. Verify identity hashes cover complete files.
- Resume an incomplete synthetic workspace containing a large EML. Confirm canonical rows are
  retained and repeated Resume does not add rows for the same extracted message identities.
- Repair an existing synthetic index from its retained extraction output. Confirm the original
  extraction remains unchanged and the previous active index survives any failed replacement.
- For EMLs above 256 MiB, confirm available Subject, From, To/Cc/Bcc, and Date fields appear and
  header searches can find them. Confirm an explicit limited-preview notice remains visible;
  body and attachment contents must not be presented as parsed or indexed.
- Exercise malformed, unterminated, and excessive large-message headers. Confirm a retained
  canonical placeholder and clear notice rather than silent message loss or an unbounded read.
- Use **Save Source As** on a large-message preview and compare the exported EML SHA-256 and
  length to its original. Confirm the exported file includes all original bytes and attachments.
- Confirm standalone MSG parsing limits, HTML sanitization, remote-content blocking, and
  export-first attachment handling remain unchanged.
- Confirm ReadPST accepts an individual synthetic output above 512 MiB within the permitted
  aggregate budget, while over-budget aggregate output and unsafe file types still fail closed.
- Confirm verification/completion reports mention retained large messages and limited previews.
  A completed index must not imply full body or attachment parsing or Outlook/Exchange parity.
- Client retry: keep the existing workspace, index sidecars, and extracted files; reopen the same
  PST and use **Resume Indexing** or **Repair Index** if offered. Record only aggregate result,
  counts, warnings, and source-integrity evidence. Do not recommend starting over or deleting
  the workspace as a prerequisite. Client success and duplicate resolution remain unconfirmed
  until this run supplies evidence.

## Import Progress Phase 3

Use only a synthetic fictional PST/workspace or an isolated copy of
non-sensitive test metadata. Never exercise this matrix against an important
real PST merely to obtain progress observations. Record the source size category
and pre-test SHA-256, byte length, and modification time; after every destructive
workspace action, confirm the source values are unchanged. Report only the stage
sequence, aggregate counters, elapsed/ETA/activity behavior, completion result,
and source-integrity result—never paths or mailbox-derived data.

- Run a normal new import and observe Preflight, Extracting from PST,
  Discovering message records, Indexing messages, Building conversations,
  Verifying import, and Complete. Confirm completed stages remain checked,
  waiting stages remain pending, the sequence never silently moves backward or
  resets, and Complete appears only after qualified internal verification.
- During preflight and extraction, confirm there is no percentage or determinate
  `progressbar` unless a trustworthy total actually exists. A live ReadPST
  process must say that the stage does not provide a percentage. Confirm live
  health uses bounded stream/root observations, the recursive safety scan runs
  only after child exit, and output file/byte aggregates appear only after that
  bounded validation.
- During discovery, confirm candidates discovered are labeled candidate records.
  During determinate indexing, verify processed/total candidates plus inserted,
  skipped-existing, parse-failure, attachment, and canonical-message counts
  against the synthetic canonical state. Confirm result-list pagination is not
  used as an import count and candidate count is never presented as
  Outlook/Exchange parity. On resume/repair, confirm pass-scoped processed,
  inserted, and existing-candidate labels do not replace the stable canonical
  total.
- During conversation building and verification, expose determinate progress
  only when the record/group total or exact check total is reliable. Verification
  must use check progress, not message count; verify missing, unexpected, and
  possible-duplicate aggregates where the synthetic fixture supplies them.
- Exercise a stable determinate indexing run through `Estimating…` to a coarsely
  rounded stage ETA. Then separately test too few samples, zero progress,
  unstable rate, stale progress, interruption/resume, and indeterminate stages;
  confirm ETA is suppressed and no overall or extraction ETA is invented.
- Confirm Elapsed and This stage omit milliseconds. Interrupt and resume after a
  measured period with the app not running: prior active time must be retained,
  downtime excluded, and stage time reset only after a real stage transition.
  Simulate a system-clock change and confirm no negative duration. Verify the
  final active completion duration remains stable in Workspace Details and the
  latest verification report, independent of verification-check duration. A
  standalone repair or verification must show its current-operation elapsed
  value rather than relabeling the full import history.
- Cancel and fail a repair and a standalone reverification of a synthetic prior
  Complete workspace, close/reopen, and retry the same kind and stage. Confirm
  prior active operation/stage time resumes, downtime is excluded, a real stage
  transition resets only stage time, and the canonical completed import duration
  never changes. Starting the other operation kind must not inherit that timing.
- Open a synthetic legacy workspace that has no Phase 3 timing provenance.
  Confirm adoption does not walk extraction output merely to summarize it,
  elapsed time begins with the current active session, and the UI qualifies the
  partial history instead of calling it the whole completed duration. Cancel a
  required legacy-manifest discovery and confirm no partial manifest appears;
  retry the unchanged corpus and confirm the same aggregate identity digest.
- Remove only a copied synthetic state sidecar from a workspace with a valid
  prior Complete verification report. Confirm repair/reverification adopts
  sticky Complete lineage before timing starts, uses the separate logical-
  operation timer, and retains that lineage after repair stages overwrite the
  older `lastCompletedStage` field. Database `complete` status without a valid
  report must not provide this proof.
- Complete a synthetic import, choose Start Fresh, and confirm the prior
  verification report is invalidated before the new lineage begins. Run the
  full Preflight-through-Verification sequence again and require a fresh
  Complete report rather than a false database inconsistency. Replace the
  copied report with a symlink in a hostile fixture and confirm Start Fresh
  refuses it without changing state or the linked target.
- Exercise real last-activity signals for child start/exit, bounded stream-byte
  activity, extraction output observation, discovery, indexing batch,
  conversation batch, and verification check. A timer tick alone must not update
  Last activity. For a quiet live child, confirm the UI says no recent measurable
  signal while still identifying the process as running; it must not claim the
  process is frozen without evidence. Details must expose only bounded aggregate
  stream/process information, never raw output.
- At the narrow 980px minimum-width window and a normal wider window, exercise
  System, Light, and Dark appearances. Confirm the tracker remains restrained
  and does not hide search, folders, results, Verify, Repair, Cancel Import, or
  Details/Workspace access, and that stage/counter updates do not shift keyboard
  focus.
- Open two workspaces, switch tabs during one import, and confirm the selected tab
  never inherits the background operation. The compact item must identify the
  importing PST by basename, formatted size, storage mode, and operation kind and
  use wording equivalent to `Importing 1998–2017 Archive.pst in the background.`
  Test same-named synthetic PSTs and confirm the opaque workspace identity, plus
  only a privacy-safe disambiguator when necessary, keeps them distinct. No
  ordinary banner or progress event may expose a full path.
- Confirm the established close control remains unavailable while that
  workspace's operation is active, including when another tab is selected; a
  guarded close attempt must neither hide nor cancel the import. After a terminal
  result, close and reopen the workspace and confirm stale events for the closed
  generation are rejected and persisted identity, stage, counts, active time,
  and completion duration are restored truthfully.
- While a background operation reaches Complete, Cancelled, Incomplete, or
  Failed, keep another PST tab selected. Confirm the newest terminal result and
  material warnings remain visible with the correct privacy-safe PST name.
  Start a new operation before the old dismissal delay expires and confirm the
  stale timer cannot remove the new banner or Cancel control.
- Test Cancel Import during extraction and indexing, including a child that exits
  from the requested terminate signal. Delay a synthetic cancellation request
  until another generation is active and confirm the stale workspace/operation/
  generation token is rejected without cancelling it. Also test a simulated crash or forced
  interruption, ordinary app quit, source unavailable, resumed indexing, resumed
  conversation building, and resumed verification. Confirm cancellation,
  incomplete, source-unavailable, and failed presentations are distinct from
  Complete; accumulated time and stable counts survive; last activity is safely
  re-established; old ETA samples are not restored; and no counters or canonical
  messages are duplicated.
- Use Start Fresh only on a disposable marker-validated synthetic workspace and
  follow the existing confirmation workflow. Confirm it creates a new operation
  generation, recovers a simulated crash-stale active-session marker before the
  reset, does not inherit the prior pass's parse-failure count, old events cannot
  update it, and the source PST remains read-only.
  If verification reclassifies a stage, require an explicit
  `stage_corrected_by_verification` explanation rather than a silent regression.
- Inspect accessibility semantics with keyboard and an accessibility inspector.
  Stage transitions and terminal outcomes may use a restrained `role="status"`
  or polite live region; once-per-second timer changes must not be announced. A
  `progressbar` is allowed only for determinate stage units with valid minimum,
  maximum, and current values. Confirm Cancel is keyboard accessible, Details
  has no focus trap, tracker text identifies completed/active/pending stages, and
  focus survives updates.
- Confirm the successful wording is qualified, for example `Import completed in
  18m 32s and passed internal verification.` It must never claim that everything
  in Outlook or Exchange was imported. Inspect progress payloads and diagnostics
  for synthetic runs and confirm they contain aggregate counters and allowlisted
  codes only—no message content, names, addresses, subjects, attachment names,
  search terms, raw ReadPST output, or full private paths.
- Remove only this test run's disposable synthetic workspaces, copied metadata,
  logs, timer fixtures, and profiles. Never treat an existing PST QuickView
  workspace as disposable test output.

## Workspace Safety

- Create a Next-to-PST workspace under `.pst-quickview.noindex/<fingerprint>/`.
- Open a legacy `.pst-quickview/<fingerprint>/` workspace if available.
- Create or open an App Support workspace.
- Reveal the workspace and verify its displayed path.
- Delete a workspace and verify only the active marked workspace is removed.
- Verify an empty `.pst-quickview.noindex` parent is removed.
- Verify the original PST remains and its SHA-256 is unchanged.
- Confirm workspace errors do not recommend deletion as the first remedy.

## Multi-PST

- Open two PSTs and switch tabs.
- Use the All Open PSTs folder tree and select folders under each PST root.
- Run Current PST and All Open PSTs searches.
- Select a cross-PST result and confirm the correct tab/message activates.
- Hide folders and use the PST-name rail to switch workspaces.
- Quit and restore the previous session.
- Confirm the restored active folder loads its Messages or Conversations immediately without a
  folder click or misleading zero-results state.
- Open a recent PST and clear Recent.
- Reindex one PST while viewing another and confirm progress remains workspace-specific.
- Delete one workspace and confirm another tab does not show its deletion details.

## Folder Hierarchy and Popovers — Phase 5

Use only synthetic fictional folder trees and aggregate observations. Include a
structurally proven technical wrapper, an uncertain wrapper-like folder, a
legitimate Archive-like folder, same-named siblings, a wrapper with direct
messages, a deep hierarchy, and multiple workspaces. Do not record real mailbox
folder names, in-PST paths, source paths, or message data.

- With no saved preference, confirm **Simplified hierarchy** is selected without
  a visible startup flash. Restart the app after choosing each hierarchy and
  confirm the valid preference returns; malformed storage must fall back to
  Simplified. Start Fresh, workspace close, and Current PST/All Open PSTs changes
  must not reset the preference.
- In Simplified hierarchy, confirm only wrappers proven by structural or importer
  provenance are promoted. An uncertain folder, an Archive-like user folder, and
  any wrapper containing direct messages must remain visible; labels alone must
  never hide a folder.
- Switch to **Original PST hierarchy** and confirm every canonical folder returns
  to its recorded relationship. Switching either way must not rewrite the source
  PST, workspace metadata, SQLite database, folder records, verification report,
  or canonical counts.
- Confirm Folder Details shows the displayed name and original in-PST folder
  path, clearly distinct from a source filesystem path, along with direct and
  descendant counts and whether proven wrappers are hidden in the presentation.
- Select meaningful folders, a promoted wrapper scope, and All Mail before
  switching modes. Preserve the logical canonical selection without selecting an
  arbitrary promoted child. Include subfolders must continue to use canonical
  descendants, and a mode switch must not restart search unless the effective
  canonical scope changes. An actual scope change must reject stale results.
- Confirm direct and roll-up counts are unchanged, promoted children appear once,
  same-named folders remain distinct, and Collapse All/Expand All work. Expansion
  state for Simplified and Original must survive switching without overwriting
  the other mode's state.
- For each custom popover, confirm its trigger toggles it, an outside pointer
  press closes it, and a pointer press inside does not. Escape must close and
  restore focus to the connected trigger; selecting a Layout or hierarchy action
  must apply it and close the panel.
- Open each custom popover while another is open and confirm only one remains.
  Tab and Shift-Tab must move normally through and beyond the panel without a
  focus trap. Trigger removal, workspace close, and layout changes must leave no
  stale panel or duplicate document listener.
- Confirm outside dismissal does not steal focus or interfere with text
  selection, message preview interaction, file pickers, or drag and drop. Native
  selects must retain native behavior.
- Exercise approximately 1400x900, 1100x750, and the 980px minimum width in
  Three Column, Outlook Style, and responsive stacked layouts, with the folder
  pane shown and hidden. Repeat in System, Light, and Dark with long names, deep
  Original hierarchy, same-named promoted roots, and several promoted roots.
  Require visible focus, no clipped Folders heading, no overlapping rows or page
  overflow, and no preventable popover clipping beyond the usable viewport.

## Messages

- Open a plain-text message.
- Open sanitized HTML and confirm remote content is blocked.
- Confirm no action can approve or load remote message resources.
- Open an EML with CID images.
- Open an RTF-body message and confirm readable body recovery.
- Confirm `rtf-body.rtf` is hidden only when promoted as the body.
- Export, reveal, and open an attachment; confirm export-first behavior and unique filenames.
- Double-click a message and confirm a separate pop-out preview opens.
- Save Printable HTML.
- Save Source EML As and compare the source and copy hashes.
- Save Source MSG As and compare the source and copy hashes.
- Confirm Message Diagnostics remain read-only.

## Local Trash — Phase 6

Use only synthetic fictional messages and disposable, marker-validated test
workspaces. Record the source PST and representative extracted/canonical file
hashes, sizes, and modification times before the matrix. Report only aggregate
counts and fixed result categories; never record message-derived data, canonical
relative identities, fingerprints, or private paths.

- With no Local Trash overlay components, confirm ordinary Messages and
  Conversations show every canonical message, Local Trash shows zero, and no
  error appears until a trash operation is requested. Creating the two private
  SQLite slots and checksummed control file must not change canonical schema v3,
  canonical rows, folders, attachments, FTS data, or import metadata.
- Move one and a bounded synthetic set to Local Trash. Confirm each disappears
  from ordinary Messages, remains available in the Messages-only Local Trash
  view, and repeated Move reports already-trashed state without duplication.
  Repeated Restore must similarly report already active.
- After a successful move, use Undo and confirm it restores exactly the
  identities changed by that operation, not messages that were already trashed.
  Dismissing the notification must not restore anything. After restart the
  transient Undo offer may be gone, but Restore must remain available.
- Move the selected message and confirm its row leaves the active list while its
  loaded preview remains stable, is marked In Local Trash, and exposes Restore.
  Restoring from Local Trash removes its trash row without forcing navigation to
  its original folder or selecting an unrelated message. Verify keyboard focus
  moves only to a mounted safe target.
- Confirm Local Trash is clearly restricted to Messages. Ordinary Conversations
  and expanded conversations must exclude trashed messages, and a conversation
  with no active messages must disappear. No Trash conversation grouping or
  canonical Local Trash folder may appear.
- Exercise ordinary terms, typed fields, date and attachment filters, Relevance,
  contextual snippets/badges, folder/subfolder scope, exact counts, and Load More
  in both active and trashed visibility. Counts must be backend-exact and
  independent of mounted pages. A cursor, offset sequence, count, or cancelled
  response from one visibility must never populate the other.
- Exercise Current PST and All Open PSTs with identical synthetic canonical
  relative identities in two workspaces. State must remain workspace/fingerprint
  scoped, ordering deterministic, and Move, Restore, or stale Undo in one
  workspace must not affect the other.
- In Simplified and Original hierarchy, confirm normal direct, descendant, and
  All Mail counts describe active messages while Details distinguish canonical,
  active, and Local Trash aggregates. Include subfolders must combine canonical
  scope with visibility without double subtraction. Import and possible-
  duplicate counts remain canonical.
- Restart the application, restore the session, repeatedly Resume import, run
  Verify and Re-verify, Rebuild Conversations, and complete a Phase 2 Repair
  Index atomic replacement. Exact surviving canonical identities must retain
  their Local Trash state, while canonical verification counts and results stay
  unchanged.
- Remove a synthetic canonical identity during a disposable repair fixture and
  confirm its state becomes an aggregate orphan only. It must not hide or attach
  to another message, even when content, numeric ID, or filename-like values are
  similar. No identity is reassigned automatically.
- Create a disposable canonical index with the same canonical relative identity
  on two rows. Confirm local state becomes unavailable without Reset permission,
  the active query returns both rows, the trash query returns none, and a Move
  request is rejected without changing the overlay.
- Test a missing sidecar, wrong workspace/source fingerprint, corrupt database,
  unsupported future schema, unsafe permissions, symlink, hardlink, nonregular
  file, and post-validation substitution. Ordinary views must fail open and hide
  no canonical messages. Invalid state must disable Local Trash actions and show
  only a privacy-safe availability/reset warning; it must never be deleted or
  repaired automatically.
- Exercise selector cut points with synthetic control data. A status-only write
  to the empty secondary record during initial single-record promotion must make
  the overlay unavailable and ordinary visibility fail open; no message
  mutation may have started. After dual history exists, a status-only next
  record must retain the prior selected slot, a complete synchronized
  `PREPARING` record must roll forward, and a partial or malformed replacement
  must make the overlay unavailable rather than revive stale state.
- Substitute the control, selected slot, and inactive slot independently after
  durable operation and Reset publication. Each mismatch must report the fixed
  operation/reset-indeterminate category. The UI must clear transient Undo,
  reload summary, message, and selected-preview state, and require reinspection
  instead of claiming a definite result.
- When invalid state is present, exercise the explicitly confirmed **Reset Local
  Trash State** recovery. Confirm it validates the exact overlay components,
  commits a fresh empty slot without any pathname deletion, restores all
  canonical messages to active visibility, and does not alter source files,
  extraction output, the canonical index, attachments, verification data, or
  another workspace. Do not show Reset while state is healthy.
- Inspect application and operation output for aggregate counts, opaque operation
  IDs, bounded revisions, and fixed result/error codes only. Confirm logs and
  diagnostics contain no message IDs, canonical identities, fingerprints,
  names, addresses, subjects, bodies, attachment names, search terms, SQLite
  error text, or private paths.
- Confirm the UI states that Local Trash hides messages only in PST QuickView and
  does not modify the source PST or reduce workspace storage. There must be no
  Empty Trash, permanent-delete, purge, secure-erasure, or storage-reclamation
  command or claim.
- After the matrix, verify the source PST and extracted/canonical artifacts retain
  their original hashes, sizes, and modification times. Remove only disposable
  synthetic state through the explicit Reset or marker-gated workspace-deletion
  workflow.

## Search 2.0

- Search with ordinary terms, quoted phrases, typed fields, advanced text filters, attachment
  presence, inclusive date bounds, folder scope, and subtree scope.
- Confirm removable filter chips, the Advanced Search filter count, and Clear All preserve the
  global Current PST or All Open PSTs Scope preference.
- Confirm contextual snippets, matched-field badges, and highlighted ranges identify the actual
  match without raw marker characters or rendered HTML.
- Confirm Relevance is available only for a text search resolving to one PST and falls back to
  Newest when the query, mode, or effective workspace selection becomes ineligible.
- Confirm first-page rows appear without waiting for the exact count and that superseded searches
  do not restore stale rows, counts, errors, or loading state.
- Exercise explicit Load More for single-PST cursor pagination and true multi-PST offset
  pagination; confirm no duplicates, missing rows, or unexpected scroll reset.
- In Messages and Conversations, exercise Arrow keys, Home, End, Page Up, Page Down, and the
  documented conversation Left/Right behavior across virtualized boundaries.
- Confirm mounted result rows remain bounded after repeated Load More while selected message
  preview and loaded result data remain available.

## Conversations

- Switch between Messages and Conversations.
- Open a pre-Conversation workspace and confirm schema migration succeeds.
- Confirm Messages mode works before conversation reindex.
- Run Reindex Existing EMLs and verify `readpst` is not executed.
- Test folder-scoped conversations with and without subfolders.
- Use Show Entire Conversation.
- Search while in Conversations mode.
- Confirm conversation rows show the latest sender or participant summary prominently beneath the
  subject and preserve the full participant list in the tooltip.
- Open two PSTs containing duplicate Message-ID values and confirm threads remain isolated by
  workspace.

## Standalone Files

- Open EML from the toolbar, Finder, and drag and drop.
- Open MSG from the toolbar, Finder, and drag and drop.
- Verify remote resources remain blocked with no approval action.
- Verify attachment export/open uses `eml-exports` or `msg-exports`.
- Run the rich-content external MSG regression fixture when configured.
- Run the legacy-RTF external MSG regression fixture when configured.
- Optionally set `PST_QUICKVIEW_RICH_MSG_EXPECTED_SHA256` and
  `PST_QUICKVIEW_LEGACY_MSG_EXPECTED_SHA256` privately to pin fixture identity. Never commit the
  values. Without them, the tests still prove that source bytes remain unchanged and report that
  fixture identity was not pinned.
- Verify both source MSG SHA-256 values remain unchanged.
- Test a malformed MSG and confirm the app remains usable.

## Calendar MSG

Calendar and meeting MSG preview is best-effort for this beta.

- Run the synthetic/unit tests for appointment, request, response, cancellation, all-day,
  recurrence, missing time zone, and malformed properties.
- If a real appointment or meeting MSG becomes available, verify the dedicated calendar layout,
  organizer, attendees, dates, location, status, notes, and attachments.
- Confirm no real calendar fixture is currently claimed as manually validated.
- Confirm uncertain time zones are labeled.
- Confirm recurrence falls back to `Recurring meeting` rather than guessing.

## Layout

- Resize all panes in Three Column mode.
- Resize the message/reader split in Outlook Style.
- Hide and show folders.
- Collapse and expand the folder tree.
- Quit and reopen to verify pane/layout persistence.
- Confirm folders, message list, and reader scroll independently.
- Confirm message selection does not scroll the whole page.

## Appearance

- Select System, Light, and Dark appearances and verify each applies immediately.
- In System mode, switch macOS between Light and Dark and verify the app follows it.
- Check toolbar, tabs, folders, Messages, Conversations, Plain Text, Sanitized HTML, Layout,
  Advanced Search, About, Recent, restore prompts, and pop-out windows in Light and Dark.
- Confirm Plain Text follows the application theme while Sanitized HTML remains on its intentional
  light document canvas.
- Confirm selected, inactive, and disabled controls remain legible and keyboard focus is visible.

## Architecture Matrix

Run the following on both machines:

- Clean Intel Mac.
- Clean Apple Silicon Mac.
- No Homebrew installed.
- No system `readpst` installed.
- App copied to `/Applications`.
- Bundled readpst detected and a small PST imports successfully.
- Finder Open With lists PST QuickView for PST, EML, and MSG.
- Main app, pop-out preview, attachment export, and workspace deletion work.

## Release Artifacts

```sh
bash scripts/verify-macos-release.sh

plutil -p \
  "src-tauri/target/universal-apple-darwin/release/bundle/macos/PST QuickView.app/Contents/Info.plist"

lipo -info \
  "src-tauri/target/universal-apple-darwin/release/bundle/macos/PST QuickView.app/Contents/MacOS/pst-quickview"
```
