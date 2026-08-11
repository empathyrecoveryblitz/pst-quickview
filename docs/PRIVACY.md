# Privacy and local data

PST QuickView is built for local mail inspection. Parsing, PST conversion, indexing, search, and message preview occur on the Mac rather than through a hosted mail-processing service.

## At a glance

- No telemetry or analytics.
- No account system.
- No cloud parsing or cloud storage.
- No automatic upload of message data.
- Original PST, EML, and MSG source files remain read-only.
- Search indexes, extracted messages, logs, preferences, and exports are stored locally.

## Network behavior

PST QuickView does not send messages to a cloud service and does not include a background telemetry or analytics client.

Remote resources referenced by message HTML are blocked by default. The interface can explicitly load remote images for the current message. If that action is approved, the Mac requests the image URLs named in the message, which can disclose the Mac's public IP address and any message- or recipient-specific identifiers embedded in those URLs. Approval applies to that message and does not enable remote resources globally. HTML sanitization remains active.

## Source file handling

PST, EML, and MSG sources are opened for reading and are not modified, replaced, renamed, or used as attachment-export destinations.

For a PST, the bundled ReadPST / LibPST 0.6.76 component reads the archive and writes extracted EML copies into a PST QuickView workspace. Indexing and later browsing operate on that workspace. Standalone EML and MSG files are parsed locally without changing the source file.

## Local indexes and workspaces

A PST workspace is a persistent local cache, not a temporary file that disappears when the app closes. It can contain:

- extracted EML copies produced during PST conversion;
- `index.sqlite` and its SQLite support files;
- FTS5 search data for subject, body text, sender, recipients, and attachment names;
- folder, message, attachment, and conversation metadata;
- import and export logs; and
- message or attachment copies exported within that workspace.

Workspaces can be stored under `~/Library/Application Support/PST QuickView/workspaces/` or beside a PST under `.pst-quickview.noindex/`. Older next-to-PST workspaces may use `.pst-quickview/`.

Because workspaces contain searchable copies and metadata derived from mail, protect them with the same care as the source archive.

## Local preferences and logs

The app stores interface preferences, recent PST locations, optional previous-session information, the selected search Scope, and related workspace paths locally. These values support session restoration and do not leave the Mac through an application service.

Operational logs are also local. Application logs can remain under `~/Library/Application Support/PST QuickView/logs/`, while workspace logs are stored inside their workspace. Logs can contain local paths and operational details, so review them before sharing. See [Logging](LOGGING.md) for the recorded fields and retention behavior.

## HTML messages and remote content

HTML messages are sanitized before display. Active and non-allowlisted markup is removed, and remote resources are blocked unless the user explicitly approves remote images for the current message. Plain-text rendering remains available when HTML fidelity is unnecessary or incomplete.

Blocking remote resources avoids automatic contact with servers referenced by a message. It does not prove that the message itself or any exported file is trustworthy.

## Attachment export and Open

PST QuickView does not execute an attachment directly from a PST, EML, or MSG source. **Export** writes a separate local copy. **Open** first creates such a copy and then asks macOS to open that exported path with the configured application.

Export-first handling separates the opened file from the source container, but it is not malware scanning or content validation. Treat every exported attachment as untrusted. PST-workspace attachment copies remain until the user removes them or explicitly deletes their containing workspace.

Standalone EML and MSG attachment exports are stored separately under the corresponding local export directories within `~/Library/Application Support/PST QuickView/`. They are outside PST workspaces and remain until the user removes them.

## Removing local data

Use the in-app **Delete Workspace** action when possible. It validates that the target is an expected PST QuickView workspace for the selected PST, including workspace marker and identity checks; when available, the indexed PST fingerprint is also compared. It does not delete the original PST.

Deleting one PST workspace does not remove:

- the original PST, EML, or MSG file;
- another PST workspace;
- standalone EML or MSG attachment exports;
- application-level logs and local preferences; or
- files exported to another folder selected by the user.

Uninstalling the application also does not automatically remove these items. Review the locations in the [installation and uninstall guide](INSTALLATION.md), remove only data you recognize, and never treat a source mail file as workspace data.

## Sharing reports

Do not post real mail, source archives, attachments, workspace databases, logs, private file paths, or unreviewed diagnostics in a public issue. Use synthetic data for bug reports and follow [SECURITY.md](../SECURITY.md) for vulnerability reporting.
