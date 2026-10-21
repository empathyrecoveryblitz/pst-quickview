# Logging Policy

PST QuickView logs privacy-safe operational metadata for troubleshooting. Logs
describe operations, state, aggregate counts, durations, and opaque identifiers;
they do not describe mailbox content.

## Application log

Path:

    ~/Library/Application Support/PST QuickView/logs/application.log

That exact location is retained only for the production identifier
`com.local.pstquickview`, preserving the beta.3 profile without a migration.
Other packaged identifiers resolve their log root through Tauri's
identifier-scoped application-log directory and never fall back to the
production profile. File logging begins only after the merged packaged
identity and its complete runtime-profile paths have been validated.

The application log may record:

- canonical UTC timestamps;
- allowlisted operation names such as import, resume, verify, repair index,
  rebuild conversations, re-verify, cancel, export, move to local trash,
  restore, undo, and reset local trash;
- workspace state and stage names, monotonically increasing state revisions,
  and privacy-safe result or failure categories;
- aggregate numeric counts, active elapsed durations, stage durations, and
  completed import durations;
- privacy-safe process-health, activity, status-message, and warning codes; and
- opaque workspace or operation identifiers when available.

Opaque identifiers must not be an absolute path, source fingerprint, mailbox
field, extracted filename, or message-derived digest. The application log
records only whether an unexpected backend error occurred; it does not copy
backend error text.

## Folder hierarchy preference and diagnostics

The Simplified/Original folder hierarchy preference is stored only as a
frontend `localStorage` value. It is not written to the source PST, extraction
output, workspace metadata, SQLite database, import manifest, verification
report, application log, or workspace import log. Only the supported
`simplified` and `original` values are accepted; an unavailable, malformed, or
unsupported value falls back to Simplified hierarchy.

If development or synthetic tests observe a proven technical wrapper with an
unexpected direct-message count, any diagnostic must remain a fixed,
privacy-safe category with bounded aggregate counts. It must not include folder
names, original in-PST paths, source or workspace paths, canonical relative
identities, message identifiers, or message-derived data. No per-folder wrapper
diagnostic is added to ordinary logs or verification reports.

## Local Trash state and logging

The dual-slot Local Trash overlay is private workspace operational state, not a
mailbox log or part of the canonical message index. Its selected SQLite slot may
store the minimum validated workspace-relative canonical identity, identity
generation, opaque operation identifier, revision, and timestamps required for
reversible Local Trash behavior. Its checksummed control file stores only
format, scope, slot identity/digest, and selection metadata. No component may
store message text, mailbox fields, attachment names, search terms, source
contents, or absolute paths. Canonical identities in this overlay are private
implementation data and are never copied to ordinary logs, frontend console
output, or routine diagnostics.

Ordinary application logs may record fixed operation/result codes for Move to
Local Trash, Restore, Undo, and Reset Local Trash State; a validated opaque
operation identifier; the local-state revision or schema version; and bounded
aggregate attempted, changed, already-active, already-trashed, failed, visible,
trashed, or orphan counts. They may record a fixed availability or
fingerprint-match category, but never the fingerprint itself. They must not
record a numeric message ID, canonical identity, folder name or path, per-message
timestamp, subject, sender, recipient, body, attachment value, or search value.

Missing, corrupt, unavailable, unsupported-future, fingerprint-mismatched, and
unsafe-file conditions are mapped to allowlisted privacy-safe error categories.
An orphan diagnostic is an aggregate count only. No error record embeds SQLite
text, a path, a filename, a canonical identity, or sidecar bytes. Invalid state
is never automatically deleted. Reset Local Trash State is a separate,
explicitly confirmed operation whose log records only its aggregate outcome;
the overlay is otherwise retained across restart, resume, verification,
conversation rebuild, and index repair, and is removed with an explicitly
deleted whole workspace.

Post-publication reconciliation failures use only the fixed
`LOCAL_TRASH_OPERATION_INDETERMINATE` or `LOCAL_TRASH_RESET_INDETERMINATE`
category, and canonical identity ambiguity uses only
`LOCAL_TRASH_CANONICAL_IDENTITY_AMBIGUOUS`. The UI responds by reloading state;
these categories never authorize logging an underlying path, SQLite error, or
identity value.

Local Trash operations do not change canonical import counters, verification
results, or import-log accounting. Logs must not describe Move, Restore, or Undo
as source deletion, permanent deletion, secure erasure, storage reclamation, or
modification of Outlook or Exchange.

## Import progress event contract

Import progress events are transient typed application data, not an additional
mailbox log. The flat camel-case `formatVersion: 1` contract contains only:

- `operationId`, `generation`, persisted `stateRevision`, bounded
  `eventSequence`, and opaque `workspaceId`;
- `sourceBasename`, `sourceSizeBytes`, `workspaceStorageMode`, and
  `operationKind` needed for the local progress UI;
- `stage`, `completedStages`, and `status`;
- `determinate`, optional `processedUnits` and `totalUnits`, and `unitKind`;
- the fixed nullable aggregate `counters` record;
- `activeElapsedMs`, logical-operation `operationElapsedMs`, `stageElapsedMs`,
  `lastActivityUtc`, and `lastActivityAgeMs`;
- `processState`, optional `eta`, `statusCode`, bounded `warningCodes`, and
  optional `stageCorrection`.

`eta` is either null or `{ lowerBoundMs, upperBoundMs }` with finite, ordered,
bounded values. `stageCorrection` is either null or
`verification_reclassified`; it is not free-form text.

The source basename is used only in the local progress presentation. It is not
copied into application.log, import.log, a verification report, a diagnostic
export, or a failure message. Full source/workspace paths, source fingerprints,
canonical relative identities, and message-derived values are never progress
event fields.

Each event string, list, and numeric value has a fixed implementation bound.
Operation identifiers use the existing validated opaque-token grammar;
workspace identifiers use the existing opaque workspace-identity contract;
completed stages cannot exceed the seven user-facing stages; counters and
durations cannot exceed the safe serialized integer range; ETA values must be
finite, nonnegative, ordered, and bounded. An unknown stage, unit, operation,
health, status, or warning code fails safely instead of being logged or shown as
a successful known state.

`stateRevision` is the durable Phase 2 sidecar revision; it is not an animation
counter. Multiple bounded observations under that revision use
`eventSequence`, and frontend ordering compares generation, revision, then
sequence. `completedStages` is a canonical-order subset supported by persisted
completion evidence, not a prefix inferred from the active stage rank. Gaps are
permitted, but every entry must precede the active or interrupted stage.

The fixed `stage` values are `preflight`, `extracting`, `discovering`,
`indexing`, `building_conversations`, `verifying`, and `complete`.
`operationKind` is `new_import`, `resumed_import`, `repair`, or `verification`;
`workspaceStorageMode` is `app_support` or `next_to_pst`; `status` is `active`,
`complete`, `cancelled`, `incomplete`, or `failed`; and `unitKind` is `files`,
`bytes`, `candidate_records`, `canonical_messages`, `conversation_records`, or
`verification_checks`.

The fixed `statusCode` values are `preflight_running`, `readpst_running`,
`discovering_candidates`, `indexing_candidates`, `building_conversations`,
`verifying_workspace`, `import_complete`, `cancellation_requested`,
`import_cancelled`, `import_incomplete`, `import_failed`, and
`stage_corrected_by_verification`.

The fixed `warningCodes` values are `no_recent_activity_signal`,
`source_unavailable`, `source_changed`, `possible_duplicate_groups`,
`verification_incomplete`, `progress_signal_unavailable`, and
`prior_timing_unavailable`. The last value means that active timing began only
when a legacy workspace entered the Phase 3 lineage; it does not expose when or
where that workspace was created. The fixed `processState` values are
`not_applicable`, `starting`,
`running_recent_activity`, `running_no_recent_signal`, `exited_success`,
`exited_failure`, `cancellation_requested`, `stalled_evidence`, and
`unknown_after_restart`. Codes never embed error text, paths, filenames,
mailbox fields, or process output.

The nullable aggregate counter keys are `outputFilesObserved`,
`outputBytesObserved`, `workspaceBytesWritten`, `candidatesDiscovered`,
`candidatesProcessed`, `canonicalMessages`, `insertedMessages`,
`skippedExistingMessages`, `parseFailures`, `attachmentsIndexed`,
`conversationRecordsProcessed`, `conversationRecordsTotal`,
`verificationChecksCompleted`, `verificationChecksTotal`,
`missingCanonicalRecords`, `unexpectedCanonicalRecords`, and
`possibleDuplicateGroups`. Null means unavailable; it must not be rendered as a
measured zero. In particular, `workspaceBytesWritten` remains null unless the
whole documented workspace aggregate can be measured with a bounded method;
extraction bytes are not relabeled as workspace bytes.

Frontend display ticks derive elapsed and “ago” text locally. They are not
logged and do not trigger progress events, sidecar writes, or database writes.
Only material stage/activity/counter changes and coalesced checkpoints may
produce a new backend event.

The application log rotates at 1 MiB and retains two backups:

    application.log
    application.log.1
    application.log.2

## Workspace import log

Path:

    <workspace>/logs/import.log

The import log records allowlisted operation and stage names, aggregate numeric
counters, active and completed operation durations, cancellation, and
privacy-safe result categories. An operation identifier may be logged only when
it is an opaque validated token. Revisioned transitions, operation generation,
active-session timing, stage timing, and last-activity history remain in the
private state sidecar; the bundled ReadPST version, invocation provenance, and
manifest format version remain in private workspace metadata rather than
ordinary logs.

The private sidecar timing history stores accumulated active import
milliseconds from completed application sessions, accumulated milliseconds for
the current stage, canonical UTC session and stage anchors, the last real
activity UTC timestamp, explicit timing-version/history-completeness provenance,
an optional pending duration at the final verification boundary, and the
optional final active completion duration. A separate bounded record stores the
operation kind, persisted stage, generation, active operation time, and stage
time for an interrupted repair or standalone verification of prior Complete
lineage. That record permits a same-kind retry without changing canonical import
history and is cleared by an unrelated operation or successful terminal result.
Live deltas use a monotonic clock and are folded into the accumulated values
only at material transitions, durable batch/check checkpoints, terminal
outcomes, and a coalesced low-frequency safety checkpoint. Restart establishes
new monotonic anchors, so application downtime is never inferred from UTC
timestamps or counted as active work. Clock rollback saturates a live delta at
zero, a real stage transition alone resets stage elapsed time, and completion
freezes the actual active duration. A pending completion value is folded into
the durable Complete transition, so recovery cannot substitute a lower
coalesced checkpoint for the exact final duration. Legacy timing with
incomplete provenance remains explicitly partial and cannot mint a whole-import
completion duration. ETA samples and prior ETA guesses are not persisted as
history.

Start Fresh recovers a crash-stale active-session marker before resetting the
timing lineage and safely invalidating the prior bounded verification-report
sidecar. Its new pass does not inherit pass-scoped parse-failure or mutation
counters from the prior manifest. Repair, resume, and verification progress
reject a state record whose operation generation differs from the scoped
operation, even when its state name and revision otherwise look usable.

Import-integrity counters may include candidates discovered and processed,
canonical messages inserted, existing canonical messages skipped, canonical
message totals, parse failures, attachments, missing and unexpected record
counts, possible duplicate-group counts, extraction output files and bytes
observed, aggregate workspace bytes when cheaply available, conversation units,
and verification checks. Source size is separate event identity metadata.
Canonical totals are database row counts rather than indexing-loop iteration or
result-list pagination counts. Candidate, canonical-message, file, byte,
conversation-unit, and verification-check labels remain distinct.

Verification completion entries record the verification level and result,
aggregate canonical, missing, unexpected, and possible-duplicate counts, and
verification elapsed duration. They may also record the persisted completed
active import duration as a separate aggregate value. Repair entries may
additionally record allowlisted lifecycle outcomes such as successful
replacement or checked rollback. The private state sidecar and verification
report retain:

- verification level and result category;
- source-integrity category such as passed, changed, or unavailable;
- current state, completed stage, interrupted stage, and state revision;
- aggregate consistency counts, verification elapsed duration, and completed
  active import duration when available.

Detailed replacement verification, promotion, and rollback conditions are
enforced during the operation. They are represented afterward by the current
state and privacy-safe reason plus any marker-gated recovery files that must be
retained; they are not separate fields in the saved verification report.

Possible duplicate diagnostics emit only the informational warning category,
aggregate group count, and opaque group identifiers. Ordinary logs never emit
the compared Message-ID, digest material, normalized headers, or canonical
relative identities. The warning does not imply suppression, merging, or
deletion.

The reserved structurally_empty_artifacts_suppressed counter remains fail-open
and zero pending deterministic extractor provenance. Blank, no-subject, and
empty-body candidates remain indexed. No per-record artifact-suppression diagnostic
or reason code is emitted. A future suppression policy requires
deterministic extractor provenance and a separate privacy review before any
new diagnostic may be logged.

App-written import and index entries rotate at 10 MiB. Starting a new import
also rotates the prior log. Two backups are retained:

    import.log
    import.log.1
    import.log.2

ReadPST does not write directly to the active log. Its stdout and stderr are
drained through bounded in-memory tails; their contents are not copied into a
log. Only stream byte counts and whether the bounded tail was truncated may be
recorded. A progress Details view may show those aggregate byte counts,
truncation state, process health, and bounded extraction file/byte observations;
it does not show raw tail contents. App-written checkpoints remain subject to
the rotation limit even when the helper emits excessive output.

## Prohibited ordinary log data

Application, import, verification, repair, conversation, and export logs must
not contain:

- people or mailbox names;
- email or other addresses;
- subjects;
- message bodies, sanitized or raw HTML, or raw source contents;
- attachment names or payloads;
- search terms;
- header names or values;
- parser error text or ReadPST output;
- separate raw source/workspace fingerprint fields or digest material (the
  required opaque workspace handle is allowed as an identifier);
- extracted filenames;
- absolute source, workspace, export, or temporary paths; or
- workspace-relative canonical identities.

These restrictions also apply to warnings, cancellation records, caught error
messages, debug output, and test diagnostics. Callers must map errors to
allowlisted privacy-safe categories before logging; truncation or newline
replacement alone does not make private text safe.

## Private verification and manifest sidecars

The extraction/index manifest and verification reports are private workspace
metadata, not ordinary logs. They are format-versioned and contain the minimum
local provenance and aggregate result data required by Import Integrity Phase
2. Normal report data follows the same mailbox-content and path restrictions as
ordinary logs. It includes the opaque workspace handle required to reject stale
or cross-workspace responses, but no separate fingerprint field.

Normal verification reports and their sidecars do not serialize canonical
workspace-relative identities. A future separately invoked local Details
operation may read validated identities from the private manifest, but those
identities must not be copied to application.log, import.log, frontend console
output, or routine diagnostics. Sidecars never contain message text, names,
addresses, subjects, bodies, attachment names, search terms, or absolute source
paths.

Private sidecars are created beneath a marker-validated workspace, written by
atomic temporary-file replacement, and protected with file mode 0600 inside
directories with mode 0700. Linked or nonregular targets are rejected rather
than followed or overwritten. Sidecars are retained with the workspace and are
removed only through the existing explicitly confirmed, marker-gated workspace
deletion workflow.

## Workspace export log

Path:

    <workspace>/logs/exports.log

The export log records operation type, local numeric message or attachment IDs,
sizes, timestamps, durations, and a generic failure category. It intentionally
omits backend error text, attachment filenames, and exported output paths. It
rotates at 2 MiB and retains two backups.

## Permissions, retention, and diagnostics

Log directories use mode 0700 and log files use mode 0600. Existing linked or
nonregular log paths are rejected rather than followed or overwritten.

- Application logs are retained until rotated or manually removed.
- Workspace logs are deleted when that workspace is safely deleted.
- Logs can be revealed from Help > Diagnostics > Reveal Logs.
- Diagnostics can contain local paths only in an explicit user-requested
  diagnostic export and must warn the user to review them before sharing.
- Removing log files or private report sidecars does not modify source PST, EML,
  or MSG files.
