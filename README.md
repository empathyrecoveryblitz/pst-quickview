<p align="center">
  <img src="src-tauri/icons/128x128.png" alt="PST QuickView application icon" width="128" height="128">
</p>

<h1 align="center">PST QuickView</h1>

<p align="center">
  A local-only macOS viewer and search tool for PST, EML, and MSG files.<br>
  <strong>Current source: v0.2.0-beta.3 release candidate</strong>
</p>

<p align="center">
  <a href="https://github.com/empathyrecoveryblitz/pst-quickview/releases"><img src="https://img.shields.io/github/v/release/empathyrecoveryblitz/pst-quickview?include_prereleases&amp;sort=semver&amp;display_name=tag&amp;label=release" alt="Latest GitHub release"></a>
  <a href="https://github.com/empathyrecoveryblitz/pst-quickview/actions/workflows/ci.yml"><img src="https://github.com/empathyrecoveryblitz/pst-quickview/actions/workflows/ci.yml/badge.svg?branch=main" alt="CI status"></a>
  <a href="docs/INSTALLATION.md"><img src="https://img.shields.io/badge/platform-macOS-000000?logo=apple&amp;logoColor=white" alt="macOS"></a>
  <a href="docs/INSTALLATION.md"><img src="https://img.shields.io/badge/binary-Universal%20macOS-555555" alt="Universal macOS binary"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-GPL--3.0--or--later-blue.svg" alt="GPL-3.0-or-later license"></a>
  <a href="docs/PRIVACY.md"><img src="https://img.shields.io/badge/data-local--only%20%7C%20no%20telemetry-2f855a" alt="Local-only operation with no telemetry"></a>
</p>

<p align="center">
  <a href="https://github.com/empathyrecoveryblitz/pst-quickview/releases"><strong>Download latest published beta</strong></a>
  ·
  <a href="https://github.com/empathyrecoveryblitz/pst-quickview/releases">View releases</a>
  ·
  <a href="https://github.com/empathyrecoveryblitz/pst-quickview/issues/new?template=bug_report.yml">Report a bug</a>
  ·
  <a href="SECURITY.md">Security policy</a>
</p>

## Overview

PST QuickView provides a focused desktop interface for browsing and searching Outlook PST archives and standalone EML or MSG messages without sending mail data to a cloud service. Original source files remain read-only. PST archives are converted locally into persistent workspaces by the bundled ReadPST / LibPST 0.6.76 tooling, then indexed with SQLite and FTS5 for navigation and search.

This source version is a beta release candidate. It is unsigned and unnotarized, and it does not claim complete Outlook rendering, RTF, or calendar compatibility.

## Screenshots

Approved screenshots using synthetic mail content will be added at these stable paths:

- **Main window** — `assets/screenshots/main-window.png`
- **Search results** — `assets/screenshots/search-results.png`
- **Message preview** — `assets/screenshots/message-preview.png`

See the [public screenshot requirements](assets/screenshots/README.md) before adding or replacing any image.

## Key features

### View and organize

- Open PST archives and standalone EML and MSG files.
- Browse mail folders or use the **All Mail** view.
- Read individual messages in a focused preview.
- Group related messages in Conversation view when threading data is available.
- Optionally restore the previous PST session and its selected folders.

### Search and navigate

- Search subject, body text, sender, recipients, and attachment names with ordinary terms, quoted phrases, typed fields, and inclusive date filters.
- Search the current PST or all open PSTs with a persistent **Scope** preference, folder/subtree filtering, removable filter chips, and **Clear All**.
- See match-centered snippets, backend-derived highlighting, and badges identifying matched fields.
- Sort single-PST text searches by FTS5 Relevance; Relevance is not compared across separate PST databases.
- Receive the first result page independently of the exact count, while superseded SQLite page and count work is physically cancelled.
- Use stable cursor pagination for single-PST Messages searches. True multi-PST Messages searches and Conversations retain deterministic offset pagination.
- Keep large loaded result sets usable through variable-height DOM virtualization for Messages, Conversations, and expanded conversation messages.
- Navigate virtualized results with a roving Tab stop plus Arrow, Home, End, Page Up, and Page Down keys; Conversations also support Left/Right expand, collapse, child, and parent movement.
- Move between folder, message, and conversation views without leaving the desktop app.

### Privacy and safety behavior

- Keep original PST, EML, and MSG source files read-only.
- Process and index mail locally, with no telemetry, analytics, account system, or cloud upload.
- Sanitize HTML messages and block remote message resources by default.
- Export an attachment to a separate local copy before asking macOS to open it.

### Local workspaces

- Convert PST contents locally with bundled ReadPST / LibPST 0.6.76.
- Store extracted EML files, a SQLite/FTS5 index, and workspace logs on the Mac.
- Place a PST workspace beside its source archive or under macOS Application Support.
- Keep workspaces available between sessions until they are explicitly deleted.

## Privacy and data handling

PST QuickView performs parsing, indexing, search, and preview on the Mac. It has no telemetry, analytics, cloud parser, cloud storage, account system, or automatic upload path. Source PST, EML, and MSG files are opened for reading and are not modified.

PST processing creates a local workspace containing extracted message copies and a searchable SQLite index. These workspaces may contain sensitive mail data and persist after the app is closed. HTML is sanitized before display, and remote resources are blocked by default. If a user explicitly loads remote images for the current message, the remote host can receive a normal image request from the Mac; that permission is not enabled globally.

Attachments are exported to a separate local file before Open is requested. Exporting does not scan or establish trust in an attachment. See [Privacy and local data](docs/PRIVACY.md) for storage locations, retention, and removal guidance.

## Installation

1. Download the current published DMG from the project [Releases](https://github.com/empathyrecoveryblitz/pst-quickview/releases) page.
2. Open the DMG and drag **PST QuickView** into **Applications**.
3. In Finder, open **Applications**, right-click **PST QuickView**, and choose **Open**.
4. Confirm the per-app macOS prompt.

The beta is unsigned and unnotarized, so a normal double-click may be blocked on first launch. Do not disable Gatekeeper globally. See the full [installation, upgrade, and uninstall guide](docs/INSTALLATION.md).

## Supported files and platforms

- **PST:** locally converted by the bundled ReadPST / LibPST 0.6.76 component and stored in a searchable workspace.
- **EML:** opened and parsed as a standalone email message.
- **MSG:** opened as a standalone Outlook message, with best-effort support for Outlook-specific content.
- **macOS:** distributed as a Universal application for Intel (`x86_64`) and Apple silicon (`arm64`) Macs. The Intel slice targets macOS 10.13 and the Apple silicon slice targets macOS 11.0; these deployment targets do not replace clean-machine testing.

The packaged beta includes ReadPST; Homebrew is not required to open PST files with the released application.

## Known beta limitations

- The current beta is unsigned and unnotarized. First launch may require explicit per-app approval from macOS.
- This is beta-quality software and may contain defects or incomplete workflows.
- Message rendering prioritizes readable plain text and sanitized HTML. Outlook HTML, Word/RTF layouts, calendar data, and uncommon MAPI properties may be simplified or incomplete.
- Calendar and meeting MSG previews are best effort. Recurrence or time-zone details may be incomplete, and real exported calendar or meeting messages have not yet been manually validated.
- Encrypted, damaged, unusual, or newer PST/MSG content may be unsupported.
- Relevance is available only for text searches that resolve to exactly one PST. True multi-PST searches do not compare BM25 scores, and Conversations do not use Relevance.
- Single-PST Messages use cursor pagination. Multi-PST Messages and Conversations remain offset-based.
- Virtualization bounds mounted DOM rows, but loaded result objects remain in memory until the search resets; page-data eviction is not implemented.
- Keyboard navigation has source and local testing but is not presented as VoiceOver certification.
- Broader clean-machine and user testing remain important for this beta candidate.
- PST conversion requires temporary local processing space and leaves a persistent local workspace for extracted EML files and the SQLite index until it is explicitly deleted. Large archives can require significant disk space and initial processing time.
- Exported attachments remain untrusted files and should be handled with the same care as attachments from any other source.
- Attachment export is one item at a time, and conversation quality depends on available Message-ID, References, In-Reply-To, participant, and subject data.

See the [release notes](RELEASE_NOTES.md) for release-specific details.

## How it works

1. The app opens the selected PST, EML, or MSG source for reading.
2. For PST files, bundled ReadPST converts messages into EML files inside a local workspace. Standalone EML and MSG files are parsed directly.
3. PST workspace metadata and searchable content are indexed in local SQLite and FTS5 tables.
4. The Tauri desktop interface queries the local index for folders, messages, conversations, and search results.
5. HTML is sanitized before rendering, remote resources remain blocked unless approved for the current message, and attachment Open operates on an exported copy.

## Building from source

Development requires macOS, Node.js/npm, Rust, and the Tauri v2 platform prerequisites. The established contributor workflow is:

```sh
npm ci
scripts/agent-check.sh
npm run tauri dev
```

Read [CONTRIBUTING.md](CONTRIBUTING.md) before making changes. Release checks and bundled ReadPST source obligations are documented in [Release compliance](docs/RELEASE_COMPLIANCE.md) and [ReadPST Corresponding Source](docs/READPST_CORRESPONDING_SOURCE.md).

## Security

PST QuickView is beta software. Do not attach real mail, archives, message bodies, attachments, logs, workspaces, or unreviewed diagnostics to a public issue. Follow [SECURITY.md](SECURITY.md) for vulnerability reporting and data-handling guidance.

## Contributing

Contributions should use synthetic test data and preserve the project's local-processing, read-only-source, sanitization, remote-resource, and export-first behavior. See [CONTRIBUTING.md](CONTRIBUTING.md) and the [Code of Conduct](CODE_OF_CONDUCT.md).

## Licensing

PST QuickView's original source code and application artwork are licensed under [GPL-3.0-or-later](LICENSE). The bundled ReadPST / LibPST 0.6.76 component is independently licensed under GPL-2.0-or-later. Other dependencies retain their own terms.

See the [copyright notice](COPYRIGHT.md), [third-party notices](THIRD_PARTY_NOTICES.md), included [license texts](LICENSES/), and [ReadPST Corresponding Source documentation](docs/READPST_CORRESPONDING_SOURCE.md) for details.

Every distributed DMG must provide equivalent access to the exact Corresponding Source used for its bundled ReadPST binary; an upstream URL alone is not sufficient.

---

PST QuickView is an independent open-source project and is not affiliated with Microsoft.
