# PST QuickView Internal Beta Test Plan

Version under test: `0.2.0-beta.3`

Record the Mac model, macOS version, CPU architecture, app path, DMG filename, and whether
Homebrew is installed before testing. Use disposable copies of test files where possible and
record SHA-256 hashes for source-integrity checks.

## Installation

- Copy `PST QuickView.app` to `/Applications`.
- For this unsigned internal beta, use Finder's right-click **Open** action for the first launch.
- Never disable Gatekeeper globally.
- If the approved internal workflow requires removing quarantine, remove it only from
  `/Applications/PST QuickView.app`.
- Confirm About shows `0.2.0-beta.3`.
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

## Messages

- Open a plain-text message.
- Open sanitized HTML and confirm remote content is blocked by default.
- Load remote images for one message and confirm approval is not global.
- Open an EML with CID images.
- Open an RTF-body message and confirm readable body recovery.
- Confirm `rtf-body.rtf` is hidden only when promoted as the body.
- Export, reveal, and open an attachment; confirm export-first behavior and unique filenames.
- Double-click a message and confirm a separate pop-out preview opens.
- Save Printable HTML.
- Save Source EML As and compare the source and copy hashes.
- Save Source MSG As and compare the source and copy hashes.
- Confirm Message Diagnostics remain read-only.

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
- Verify remote resources are blocked by default.
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
