# Import Integrity

This document defines PST QuickView Import Integrity Phases 1 through 3 and the
canonical-data boundaries that later local presentation and organization phases
must preserve. Phase 2 keeps the Phase 1 canonical source-record identity and
fail-open candidate policy, adds a persistent revisioned workspace state
machine, and provides internal verification and non-destructive repair
workflows. Phase 3 presents that state truthfully with stage-specific progress,
source identity, active timing, activity health, aggregate counters, and
cautious estimates. Phase 5 changes folder presentation only. Phase 6 adds a
separate reversible local-state overlay. Source PST files and ReadPST extraction
output remain read-only during import, verification, resume, index repair, and
Local Trash operations.

## State transitions before Phase 2

Before Phase 2, import status exposed a coarse lifecycle:

1. **Pending** was written before an import worker started.
2. **Running** covered ReadPST extraction, extracted-file discovery, batch
   indexing, and conversation construction without distinguishing those
   stages.
3. **Complete** was written after indexing, conversation construction,
   metadata finalization, and temporary-index promotion. It did not mean that
   an internal verification pass had succeeded.
4. Cancellation changed Running to **Cancelled**; other ordinary errors changed
   it to **Failed**. An uncertain index-promotion result was left fail-closed
   rather than overwritten with a misleading terminal state.
5. Resume and reindex reused Running and Complete, so reopening an interrupted
   workspace could not report the precise last completed or interrupted stage.

Missing marker, metadata, extraction, or index files were handled as missing
workspace components. A source fingerprint or extracted-candidate disagreement
was treated as stale or mismatched input and failed closed, but stale was not a
durable workspace state. The mere presence of index.sqlite was never sufficient
evidence that extraction, indexing, conversations, and verification were all
complete.

## Progress presentation before Phase 3

Before Phase 3, the backend emitted a generic stage label, optional current and
total values, and a free-form message. Workspace and operation identifiers were
optional. The frontend also created an initial progress value before the first
backend event, selected either the foreground operation or the selected
workspace's operation for display, and reused one horizontal bar for every
stage. This event flow was not the persistent Phase 2 state machine and could
not establish which durable stage had completed.

Concretely, an import worker emitted the generic Tauri `import-progress`
payload; the frontend listener accepted it while a global progress-events flag
was enabled, resolved the optional workspace identity into its in-memory
operation map or foreground/selected-workspace fallback, selected one visible
operation, and computed the bar from `current / total`. Phase 2 state, manifest,
and SQLite checkpoints were written through their separate durable paths. The
generic payload did not carry the operation generation and state revision needed
to reconcile those two flows or reject every stale update deterministically.

The old visual presentation was misleading for all of these independently
confirmed reasons:

- an operation with no measurable numerator and denominator began with a fixed
  35-percent-filled bar, so indeterminate work looked partly complete;
- the same bar was reused for unrelated stages, so a new stage could reset its
  fill or reinterpret the previous stage's units;
- ReadPST normally supplies no verified total, so the bar could remain still
  throughout a healthy extraction even while process output or extracted files
  changed;
- workspace identity on progress events was optional and the fallback depended
  on foreground or selected-tab state, so a hidden, missing, or stale event
  could disappear or appear associated with the wrong PST;
- any nonzero `current`/`total` pair was rendered as a percentage without a
  typed unit or proof that the denominator was complete and trustworthy; and
- free-form stage names and frontend-created initial values could diverge from
  the persisted state revision, especially across cancellation, interruption,
  resume, verification correction, or repair.

Phase 3 removes that generic overall bar. Visual continuity never overrides
truth: an indeterminate stage remains indeterminate, and a later stage does not
inherit or reset a percentage from an earlier one.

## Persistent Phase 2 workspace state

Phase 2 persists one of these states:

- **preflight** — the workspace and read-only source checks are in progress;
- **extracting** — ReadPST is producing local extraction output;
- **extraction_complete** — ReadPST completed successfully and extraction
  provenance was sealed;
- **discovering** — canonical extracted candidates are being enumerated;
- **indexing** — canonical candidates are being inserted or skipped;
- **building_conversations** — conversation metadata is being derived from
  canonical message rows;
- **verifying** — internal consistency checks are running;
- **complete** — an internal Quick or Full Verify succeeded for the committed
  workspace revision;
- **cancelled** — a user-requested cancellation stopped the operation;
- **incomplete** — prior work is usable but one or more required stages did not
  finish; or
- **failed** — the operation failed for a reason other than cancellation.

Each transition carries a monotonically increasing state revision, a canonical
UTC update timestamp, last completed and interrupted stages, privacy-safe
reason codes, aggregate counters, and accumulated timing. The marker-gated
private state sidecar is authoritative and each transition is committed by an
atomic temporary-file replacement with file synchronization and
parent-directory synchronization. SQLite `import_metadata` remains the
transactional authority for index counters and the compatibility import status;
it is not described as part of an impossible cross-file transaction. A state or
counter contradiction fails closed instead of guessing.

Reopening extracting, discovering, indexing, building_conversations, or
verifying classifies the workspace as incomplete and, when its guards pass,
resumable. Cancelled remains distinct from failed. Complete can be reached only
from verifying after verification succeeds. A missing source volume does not
rewrite an otherwise valid local workspace as failed or corrupt. Unknown
future state names and versions are unsupported and fail closed.

State records contain only stages, revisions, counts, durations, timestamps,
opaque workspace identifiers, and privacy-safe reason categories. An absolute
source path is allowed only in the existing private workspace metadata that
already needs it; it is not copied into state transitions, verification reports,
resume banners, or ordinary logs. Existing explicit workspace-location Details
remain separate from the normal integrity result.

## Phase 3 stage presentation

The persistent Phase 2 state and its monotonically increasing revision are the
source of truth for Phase 3. The user-facing tracker has seven stages:

1. **Preflight**
2. **Extracting from PST**
3. **Discovering message records**
4. **Indexing messages**
5. **Building conversations**
6. **Verifying import**
7. **Complete**

`extraction_complete` is a durable checkpoint between Extracting from PST and
Discovering message records. It completes the extraction tracker item but is
not displayed as an eighth user-facing stage. No stage is inferred only because
a directory, extracted file, database, table, or manifest exists. Complete is
displayed only after the Phase 2 verification transition succeeds for the
committed workspace revision.

Completed tracker items retain their completed state, the active item is
identified in text as well as visually, and later items remain pending. Within
one operation generation, an accepted event cannot move the tracker backward.
Every event is checked against its opaque workspace identifier, operation
identifier and generation, persisted state revision, and bounded event
sequence before it may replace displayed state. Ordering is lexicographic by
generation, state revision, then event sequence. A lower tuple, a different
workspace, or an event for a closed workspace is stale and is ignored
deterministically. `completedStages` is the ordered subset justified by the
persisted `lastCompletedStage` lineage; the active stage's rank is never used
by itself to fabricate checkmarks. Gaps are allowed when only some completion
evidence is durable, but every completed entry must precede the active or
interrupted stage.

Cancellation, incompleteness, and failure are interrupted terminal
presentations, not completed stages. They retain the last truthfully completed
stage and name the interrupted stage without marking later stages complete.
Resume starts a new operation generation from persisted state and canonical
counters. It deliberately replays candidate discovery before continuing
interrupted indexing, conversation building, or verification; that recheck is
named explicitly, and completion evidence is clipped to stages before
Discovering so replayed later stages never appear completed. Repair and
standalone verification likewise carry forward only persisted prerequisites
below their active stage. A conversation-only repair marks reused indexing
prerequisites only after accepting the Phase 2 manifest and opening the
canonical index. Recovery that proves a replacement is already active marks
reused prerequisites before final verification. If verification discovers that
an earlier stage must be revisited, the current operation ends with an explicit
verification correction and reason; the tracker never silently animates
backward.

### Determinate and indeterminate work

Phase 3 has no overall import percentage. A stage-specific progress bar is
shown only when both its typed numerator and denominator are trustworthy. The
rules are:

| Stage | Presentation and eligible measurements |
| --- | --- |
| Preflight | Normally indeterminate or a finite checklist. It never receives a time-based percentage. |
| Extracting from PST | Indeterminate unless the verified ReadPST contract supplies a measurable total. Process liveness, bounded stream-byte changes, and coarse output-root metadata changes are activity signals, not a percentage of PST completion. Aggregate output file/byte counts appear only after the bounded post-exit validation pass. |
| Discovering message records | Shows candidates discovered. It is determinate only when an independently trustworthy discovery total is known; the count observed so far is not itself that total. |
| Indexing messages | Determinate when the sealed discovery total is known. The numerator is candidate records processed and the denominator is discovered candidate records. Inserted, skipped-existing, parse-failure, attachment, and canonical-total counts remain separate. |
| Building conversations | Determinate only when a reliable total number of messages or groups to process is fixed for the pass. Otherwise it remains indeterminate. |
| Verifying import | Uses a checklist or exact completed-check/total-check count. Message count is never used as a verification percentage. |
| Complete | Has no active progress bar. It shows the persisted completion duration and qualified verification result. |

An indeterminate progress element has no numeric accessibility value. A
determinate progress bar uses the stage's real typed units and clamps only for
display safety; contradictory counters fail closed rather than being forced
into a plausible percentage. Candidate records, canonical messages, files, and
bytes are never treated as interchangeable units.

### Active PST and operation identity

Every ordinary progress presentation identifies:

- the source PST basename, never its full path;
- the formatted source size, with exact bytes available only in an explicit
  local Details view;
- the workspace storage mode, such as Next to PST or App Support; and
- the operation kind: new import, resumed import, repair, or verification.

The backend contract carries an opaque workspace identifier in addition to the
basename. Same-named PSTs remain distinct internally. When basename, size, and
storage mode are still ambiguous in the UI, a bounded privacy-safe workspace
disambiguator derived from the opaque identity may be shown; a source path,
fingerprint, canonical relative identity, or mailbox-derived value may not.

If the user selects another PST while work continues, the compact presentation
uses wording such as “Importing 1998–2017 Archive.pst in the background.” The
background item keeps its own stage, counters, timing, activity, and controls;
it cannot borrow the selected tab's details. Consistent with the established
tab behavior, Close remains unavailable while that workspace operation is
active; a guarded close attempt neither hides nor cancels it. Existing explicit
cancellation remains the only normal UI request to stop it. That request carries
the exact opaque workspace ID, operation ID, and generation; a delayed request
for an older operation is rejected without affecting current work.

### Backend progress contract

The serialized contract is `formatVersion: 1` and uses these bounded camel-case
fields:

- `operationId`, `generation`, persisted `stateRevision`, bounded
  `eventSequence`, and opaque `workspaceId`;
- `sourceBasename`, `sourceSizeBytes`, `workspaceStorageMode`, and
  `operationKind`;
- `stage`, `completedStages`, and terminal-or-active `status`;
- `determinate`, optional `processedUnits` and `totalUnits`, and `unitKind`;
- the fixed nullable aggregate `counters` record;
- `activeElapsedMs`, logical-operation `operationElapsedMs`, `stageElapsedMs`,
  `lastActivityUtc`, and locally useful `lastActivityAgeMs`;
- `processState`, optional bounded `eta`, `statusCode`, bounded
  `warningCodes`, and optional `stageCorrection`.

The fixed stage values are `preflight`, `extracting`, `discovering`,
`indexing`, `building_conversations`, `verifying`, and `complete`. Operation
kinds are `new_import`, `resumed_import`, `repair`, and `verification`; storage
modes are `app_support` and `next_to_pst`; statuses are `active`, `complete`,
`cancelled`, `incomplete`, and `failed`. Unit kinds remain explicit:
`files`, `bytes`, `candidate_records`, `canonical_messages`,
`conversation_records`, or `verification_checks`.

The event never contains a full path, message text, mailbox field, attachment
name, search term, parser error, raw ReadPST output, or canonical relative
identity. Lists and strings have fixed bounds, numeric values must be finite and
within the safe serialized range, and unknown future stage, status, unit, or
health values fail safely. Display timer ticks are derived locally and do not
create backend events or database writes.

### Active elapsed time and stage time

Import duration is active processing time, not wall-clock time between the
first and last UTC timestamp. The backend uses a monotonic clock for live
duration deltas and persists:

- active time accumulated before the current application session;
- the canonical UTC start of the active session for history;
- the active stage and its canonical UTC start;
- accumulated active time for the current stage;
- the most recent privacy-safe activity timestamp; and
- the final active completion duration when verification succeeds.

Live total elapsed time is the persisted accumulated duration plus the current
session's monotonic delta. Repair and standalone-verification timing uses a
separate, validated record keyed by operation kind, persisted stage, and
generation, so it can never rewrite canonical import duration. A new logical
repair or verification starts at zero. After interruption, a retry of the same
kind resumes its saved active operation time; a same-stage retry also resumes
stage time. Changing operation kind or completing successfully clears that
lineage. Operation generation still orders events and rejects stale updates; it
is not the elapsed-time reset boundary. New and resumed imports expose their
accepted generation's operation time separately from accumulated import time. A
display may update once per second, but the timer is not a progress signal and
is excluded from live-region announcements.

Timing is checkpointed at material state transitions, existing durable batch or
check checkpoints, terminal outcomes, and a coalesced low-frequency safety
checkpoint. It is never written merely to animate the display. On quit, cancel,
failure, or ordinary interruption, the last accumulated active duration is
retained. After restart, downtime is not reconstructed by subtracting UTC
timestamps and is not added to elapsed time. A backwards or forwards system
clock change cannot create a negative duration because durations use monotonic
deltas; UTC remains history metadata only.

The successful terminal timing checkpoint stores the actual active import
duration, not an ETA. Workspace Details and the latest verification report
retain that duration independently of any temporary completion banner. The
exact monotonic duration is first saved as a pending completion value at the
final, cancellation-insensitive verification boundary and is folded into the
same durable state transition that marks the import Complete. A crash before
that boundary remains incomplete; a crash after the Complete transition
recovers the exact saved duration instead of relabeling an earlier coalesced
checkpoint as the final value.

Timing provenance is explicit. A Phase 3 import started through Start Fresh has
complete active-time history. A legacy workspace without Phase 3 timing
provenance begins recording active time only when it is adopted or resumed; its
older downtime cannot be reconstructed from UTC timestamps. That partial
history is retained across interruption, but it is never presented or persisted
as the whole completed import duration. Progress carries the fixed
`prior_timing_unavailable` warning until a new Start Fresh lineage establishes
complete timing history. A repair or verification of an already Complete
workspace still reports its separately measured current-operation duration.
Display values omit milliseconds, for example `Elapsed: 12m 41s`, `This stage:
3m 08s`, and `Completed in 18m 32s`.

### Last activity and process health

“Last activity” means one of these real, privacy-safe observations:

- ReadPST child-process start or exit;
- a change in bounded stdout/stderr byte counters;
- a coarsely and safely observed extraction-output root metadata change;
- candidate-discovery progress;
- an indexing batch commit;
- a conversation batch completion; or
- a verification-check completion.

Moving the elapsed timer is not activity. While ReadPST is running, Phase 3
does not recursively rescan the extraction tree: it observes bounded process
streams and root metadata, then performs one full safety validation after the
child exits. `processState` distinguishes
`not_applicable`, `starting`, `running_recent_activity`,
`running_no_recent_signal`, `exited_success`, `exited_failure`,
`cancellation_requested`, `stalled_evidence`, and `unknown_after_restart`.
Only evidence-backed health checks may select `stalled_evidence`; a quiet but
live process is not called frozen or terminated merely because the bounded
signals stay unchanged. The separate 48-hour absolute process safety limit
still applies. Wording such as “No measurable progress
signal for 45 seconds; the process is still running” reports only the evidence
available. A recent observation may be displayed as `Last activity: 3 seconds
ago`; a live extraction may say `ReadPST is running; this stage does not provide
a percentage.` Relative age calculations saturate at zero if the wall clock
moves backward and never manufacture a new activity event.

The current logging policy does not permit raw ReadPST output in ordinary logs
or progress events. A Details view may therefore show bounded aggregate stream
byte counts, truncation state, process state, and output file/byte observations,
but not raw process lines or message-derived output.

### Cautious stage ETA

ETA is optional and stage-specific. It is eligible only for a determinate,
unfinished stage with a reliable total after at least four cumulative samples,
ten completed units, and ten seconds of active processing. Progress must be
positive, the most recent measurable activity must be no more than 30 seconds
old, and the resulting finite range must not exceed seven days. Indexing is the
usual eligible stage. Extraction has no ETA unless ReadPST supplies a verified
measurable total, and unknown later stages are never summed into an overall
estimate.

The estimator applies an exponentially weighted moving average with alpha
`0.35` to bounded recent active-progress intervals rather than using the last
item's instantaneous time. Resume discards the old live estimator samples and
warms a new rate from new active-session progress, so application downtime
cannot appear as processing time. Before the sample threshold it may show
`Estimating…`. It suppresses the estimate for zero or out-of-order progress,
indeterminate work, nonpositive or non-finite rates, coefficient of variation
above `0.35`, stale activity, contradictory counters, or an unbounded result.
It never displays a negative, infinite, zero-time estimate for unfinished work
or false second-level precision. User-facing results are rounded to forms such
as “Less than a minute remaining,” “About 8 minutes remaining,” or “Estimated
1–2 hours remaining.”

### Resume and restart presentation

On reopen, persisted state, revision, manifest, and canonical counters are
reconciled before a progress presentation begins. Resume preserves prior active
elapsed time and stable discovery/canonical totals, establishes a new session
and last-activity baseline, and does not restore an old ETA. Resume visibly
rechecks discovery before continuing the interrupted durable stage; completion
evidence from later persisted stages is withheld until the replay reaches it.
Candidate outcomes that are recomputed by a resumed indexing or repair pass are
explicitly labeled “this pass”; they do not replace the stable canonical
database total. A source-unavailable result is shown without changing a valid
local workspace to failed.

The prior canonical last-activity timestamp remains history; it is not
described as current process activity after restart. Starting the new operation
generation establishes a new session baseline, and a new child-process,
discovery, batch, conversation, or verification observation establishes its
measurable activity signal.

Start Fresh remains a separate marker-gated destructive workflow. It first
closes any crash-stale active timing session, then resets state lineage, pass
counters, elapsed history, and the prior verification report before
compatibility status becomes Pending. The report invalidation is limited to the
exact bounded regular sidecar and refuses links. Start Fresh does not seed the
new operation from the prior manifest's parse-failure count, and starts a new
operation generation only after the existing confirmation path succeeds. Old
progress events cannot belong to the new generation.

A standalone verification presents and announces the fresh report from that
operation. An older saved report cannot replace an Unsupported or Incomplete
current result. If an operational failure interrupts a prior Complete
workspace, the prior completion evidence remains intact while the verification
operation itself ends as cancelled or failed; a newly incomplete workspace is
terminalized accordingly.

### Accessibility semantics

Stage changes, completion, cancellation, and failure use a restrained
`role="status"` or polite live region. The tracker exposes meaningful stage
names and completed, active, and pending text. A `progressbar` role is present
only for a truly determinate stage and then has valid `aria-valuemin`,
`aria-valuemax`, and `aria-valuenow` values from the typed stage units.
Indeterminate stages expose explanatory text without a fake numeric value.

Second-by-second elapsed and last-activity text is outside the announcing live
region. Stage updates do not replace focused controls, Cancel remains a normal
keyboard-reachable button, and Details remains reachable without a focus trap.
The compact background presentation retains the same operation name,
accessible stage summary, and any material source or verification warning. A
terminal result remains associated with its workspace when another tab is
selected, and live terminal announcements include the privacy-safe PST name.

## Additive schema-v3 metadata and sidecars

Phase 2 preserves valid schema-v3 workspaces. It does not change SQLite
user_version, destructively rewrite message rows, or weaken future-schema
rejection. Revisioned workspace state, manifest references, verification
summaries, and repair bookkeeping use format-versioned private workspace
sidecars. Existing additive `import_metadata` keys remain the transactional
authority for index counters and the coarse compatibility import status.

The workspace marker remains the authority for workspace identity and safe
workspace operations. The extraction/index manifest and the most recent
verification report are local private metadata, not public diagnostics. Each
has its own format version; malformed data and unknown future versions are
reported as unsupported rather than adopted. A failed additive metadata update
leaves valid schema-v3 message data unchanged; SQLite counter and status writes
remain transactionally coupled within their database transaction.

## Canonical source-record identity

The canonical identity is **workspace-relative-eml-v1**: the exact normalized
path of an extracted EML relative to that workspace's extracted directory. The
identity is scoped to the source workspace and importer generation
**readpst-separate-eml-v1**. It contains no absolute source or workspace path and
does not depend on the subject, body, Message-ID, attachment content, or a
complete content hash.

Every relative component must be valid UTF-8. An unrepresentable component is
rejected rather than converted lossily, because two distinct filesystem names
must never collapse to one canonical identity.

This identity keeps two legitimate messages distinct even when their content
is identical. It also preserves distinct same-named PST folders. ReadPST can add
a numeric suffix when two source folders have the same display name, so folder
suffixes must never be removed or treated as proof that one folder is a
duplicate of another.

The existing messages.eml_path unique constraint enforces this identity in
schema v3. Reprocessing an existing identity leaves its message row, numeric
message ID, folder assignment, attachment rows, and full-text-search row
unchanged. Attachments and search data are created only when the canonical
message row is newly inserted. A workspace in which messages.eml_path is not
uniquely constrained by one nonpartial, binary-collated key is rejected with a
fail-closed error rather than silently migrated or repaired.

## Canonical folders and presentation hierarchy

Phase 5 adds a folder presentation choice without changing the canonical
imported hierarchy. **Simplified hierarchy** is the default and promotes only
technical wrappers established by deterministic structural or importer
provenance. **Original PST hierarchy** remains available and presents every
canonical imported folder in its recorded parent/child relationship, subject to
the existing application-root handling.

Simplification is fail open. A visible label such as PST, Outlook Data File,
Archive, Archive Mailbox, Personal Folders, Mailbox, or Root is never sufficient
evidence that a folder is technical. A folder with uncertain provenance, a real
user folder, or a proven wrapper that unexpectedly contains direct messages
remains visible. Promoted children retain their stable canonical folder and
workspace identities, original canonical parent and in-PST path, direct and
roll-up counts, and canonical descendant scope. Same-named folders remain
distinct and All Mail remains outside the imported folder hierarchy.

The current canonical folder record does not persist a general structural-role
or importer-provenance field. The only production classification presently
provable is the importer-generated empty-path root (and it remains visible if it
contains direct messages or its counts are inconsistent). Nonempty PST store,
root-container, and similar layers therefore remain visible unless a future
canonical importer supplies deterministic provenance; names are not used as a
substitute. The pure presentation model accepts explicit trusted structural
provenance for synthetic coverage and future canonical inputs without changing
today's schema.

The presentation does not rewrite a PST, extraction output, workspace metadata,
folder row, message assignment, SQLite schema or data, manifest, or verification
report. Changing hierarchy mode cannot alter canonical folder IDs, counts,
search inputs, or verification results. Include subfolders is always evaluated
from canonical descendants rather than presentation ancestry. Selection remains
a canonical scope; hiding a selected technical wrapper must not silently choose
one promoted child. Expansion state is maintained independently for Simplified
and Original hierarchy so switching modes does not discard the other view's
state.

Folder Details may show the displayed name, original canonical in-PST folder
path, workspace/PST identity, direct and descendant counts, and whether proven
technical ancestors are hidden in the current presentation. The in-PST path is
not the source filesystem path. The hierarchy preference is frontend-only local
application state and does not participate in import integrity or workspace
verification.

## Phase 6 local-state overlay and Local Trash

Phase 6 stores reversible message-visibility state in a versioned private
workspace overlay. Two fixed SQLite slots and a checksummed two-record control
file provide descriptor-bound, crash-recoverable publication without renaming
or unlinking a workspace entry. The selected SQLite slot is separate from the
canonical schema-v3 index: it never changes canonical message or folder rows,
attachments, FTS data, extraction output, import metadata, the manifest, or a
verification report. Its records are bound to the workspace/source fingerprint
and keyed by the stable **workspace-relative-eml-v1** identity. A transient
numeric database ID may resolve an operation request, but it is not durable
local-state identity.

The highest valid checksummed control record is authoritative, including a
fully synchronized `PREPARING` record. Fresh creation and promotion of the
legacy single-record empty state establish two valid history records before a
message mutation begins. During that one-time promotion, interruption after the
status-only write to the previously empty secondary record can invalidate the
selector; the overlay then becomes unavailable and fails open, with no message
mutation from that operation started. Once dual history is established, a
status-only next record keeps the prior selector. A partially replaced or
malformed newer record disables the overlay and fails open rather than reviving
possibly stale visibility state, while a complete synchronized `PREPARING`
record rolls forward.

Absence from the sidecar's trash set means active. Ordinary Messages and
Conversations use active visibility; expanded conversations also exclude
trashed messages. Local Trash is deliberately a Messages-only virtual view in
this phase and is not a canonical or presentation folder. Move to Local Trash,
Restore, and Undo change only this overlay. Operations are idempotent and
transactional. Undo restores exactly the identities changed by its successful
move operation and never restores an identity that was already trashed before
that operation.

Local visibility counts are distinct from import-integrity truth. Normal folder
and All Mail presentation counts describe active messages, and Local Trash
counts describe overlay entries that still match canonical messages. Canonical
message, folder, attachment, and FTS totals remain unchanged. Quick Verify,
Full Verify, possible-duplicate diagnostics, manifests, repair acceptance, and
all other import-integrity accounting continue to use the complete canonical
index. Moving or restoring a message therefore cannot make verification pass or
fail and cannot reduce the canonical imported count.

The overlay persists through application restart, session restore, repeated
import Resume, Verify, Re-verify, Rebuild Conversations, and Phase 2 Repair
Index atomic replacement. Repair reconnects surviving state only by exact
canonical identity. A sidecar identity missing from the repaired canonical
index becomes an orphaned local-state entry; it is counted only in private
Workspace/Local State details, never reassigned, and never hides an unrelated
message. A workspace/source fingerprint mismatch likewise applies no old state.
The canonical index must independently prove uniqueness of every canonical
identity. A duplicate canonical identity disables local-state application,
copies no identity into the query overlay, hides no canonical row, and does not
offer Reset because an overlay reset cannot repair the canonical ambiguity.

An absent overlay fails open: ordinary views show every canonical message and
Local Trash is empty until an operation creates valid state. A corrupt,
unavailable, unsupported-future, or fingerprint-mismatched component also fails
open for ordinary views, hides no messages, disables Local Trash actions, and
surfaces only a privacy-safe availability/reset warning. It is never
automatically deleted, replaced, or treated as permission to change canonical
data. The only Phase 6 recovery action for unusable local state is an explicitly
confirmed **Reset Local Trash State** operation. That operation validates the
exact overlay components, commits a fresh empty slot, and may overwrite only
those validated local-state files; it restores canonical messages to ordinary
visibility without changing the source, extraction output, canonical index, or
attachments. Start Fresh or Delete Workspace may remove local state only as
part of their existing explicitly confirmed whole-workspace behavior.

After publication, Move, Restore, Undo, and Reset revalidate the pinned control
file and both fixed SQLite slots. If that reconciliation cannot prove the exact
published namespace and selector, the command reports a fixed indeterminate
outcome. The frontend clears transient Undo authority and reloads Local Trash,
message, and selected-preview state so the user reinspects the durable result;
it does not guess whether the operation took effect.

Local Trash is not deletion or storage reclamation. It never removes source PST,
EML, or MSG bytes, extracted candidates, canonical rows, attachments, or search
data, and it does not reduce workspace storage. This phase has no Empty Trash,
permanent-delete, secure-erasure, or purge operation. Any future permanent local
purge requires a separate design and review; Local Trash state provides no
authorization for one.

## Extraction and index manifest

The private manifest records enough provenance to reconcile extraction,
indexing, and verification without storing message content. Its versioned
fields include, when applicable:

- workspace and source fingerprints;
- canonical-identity and importer generations;
- bundled ReadPST version and invocation mode that affects output semantics;
- extraction start and completion timestamps and completion result;
- the ordered set of normalized workspace-relative EML identities;
- discovered candidate count and aggregate candidate size;
- inserted, skipped-existing, parse-failure, and attachment counts;
- canonical message, folder, and FTS row totals;
- conversation-assignment state;
- source size and modification time observed before and after extraction; and
- a source hash when one is already available without imposing an unreasonable
  additional import delay.

Discovery retains the Phase 1 deterministic SHA-256 aggregate over ordered,
length-delimited relative identities, bounded file sizes, and candidate content
digests as a candidate-set freshness guard. It is not a message identity or
deduplication key. Modification time alone is never canonical identity.

Canonical relative identities are private local provenance. They are not
serialized in normal verification results or ordinary logs, which contain only
aggregate counts and opaque identifiers. A future separately invoked local
Details operation may expose validated relative identities without changing
that default boundary. Quick Verify does not require a fresh full-content hash
of every extracted message.

## Resume behavior

Indexing may rescan the extracted tree after cancellation, interruption,
reopen, or another resume. Each candidate has one of two canonical outcomes:

- a new canonical message is inserted; or
- an existing canonical identity is skipped.

A parse failure is a diagnostic subset of inserted candidates: the candidate
is retained as the existing privacy-safe parse-failure placeholder. Thus
`processed = inserted + skipped`; parse failures overlap inserted placeholders
rather than forming a third canonical outcome.

Committed canonical rows remain valid if a later batch fails. A retry scans the
same candidates, skips committed identities, and inserts only missing rows.
Folder creation, message insertion, attachment insertion, full-text-search
insertion, and counter updates remain transactionally coupled for each committed
batch. The authoritative state sidecar is checkpointed after that commit.
Reopen classification derives resume counts from the usable SQLite file set
without changing message or index rows. A valid Phase 2 state sidecar is not
rewritten by that summary; a legacy workspace with no state sidecar receives an
additive initial incomplete-state record from marker and SQLite aggregate truth.
That adoption path does not walk the extraction tree, does not infer extraction
completion merely from files or a database, and does not invent pre-adoption
active time. A strictly validated prior Complete verification report may seed a
durable completion-lineage marker in either a missing legacy sidecar or an
existing pre-field sidecar before repair or reverification chooses its separate
timer. A malformed report fails closed without changing the sidecar; database
status alone never provides completion proof. Before the next Verify, Resume,
Repair, or conversation rebuild, a persisted transient state is reconciled to
incomplete from SQLite database truth. Existing message IDs are preserved.

When a missing legacy manifest must be created for an authorized resume,
repair, verification, or conversation rebuild, candidate discovery uses the
same bounded progress and cancellation checkpoints as ordinary discovery. The
manifest is written atomically only after the complete candidate set validates;
a cancelled or contradictory scan leaves no partial manifest. Retrying the same
unchanged synthetic corpus therefore produces the same manifest digest without
changing the source or extraction output.

Resume depends on the original extracted tree and partial SQLite file set.
Operators must not remove a partial index or its SQLite sidecars and must not
alter the extracted directory. Resume fails closed if the workspace identity,
source fingerprint, candidate count or identity manifest, canonical-identity
version, or importer generation does not match. The chosen target and every
already indexed relative identity are validated read-only before schema or
message writes. A side-by-side partial target cannot replace the intact main
index unless the partial index completes and passes verification.

Pre-Phase-1 schema-v3 workspaces lack the original additive provenance keys.
They use an explicit compatibility path only when the stored workspace ID,
source fingerprint, candidate count, exact existing-identity subset, and
canonical unique key all validate. Adoption is recorded transactionally. The
original ReadPST path and version remain provenance; resume records separately
that ReadPST was not invoked.

Conversation assignment is rebuilt from canonical message rows. It updates
those rows rather than inserting conversation rows, so rebuilding an unchanged
corpus is idempotent. A newly discovered earlier parent can legitimately change
thread roots once; repeated rebuilding after the corpus is complete must remain
stable.

### Resume details shown to the user

An incomplete or resumable workspace summary shows the PST basename, last
completed stage, interrupted stage, discovered candidate count, canonical
indexed count, known remaining count, last successful UTC update, accumulated
duration, and a privacy-safe reason that the workspace is not complete. It does
not show the full source path by default.

For example: “Indexing was interrupted after 34,281 of 79,354 discovered
records.”

Resume Indexing resumes at canonical candidate boundaries. It does not claim
that ReadPST extraction resumes at a byte offset.

## Quick Verify

Quick Verify is non-destructive. Except for persisting the privacy-safe report
and truthful verification state revisions, it does not change messages,
folders, attachments, extraction output, or the source PST. It checks:

- the workspace marker and workspace/source fingerprint relationship;
- supported state, metadata, manifest, and canonical-identity versions;
- source identity when the source is mounted;
- existence of the extraction root and every expected canonical relative
  record;
- absence of duplicate canonical identities;
- SQLite integrity;
- required tables, relationships, and indexes;
- canonical message count;
- folder-link and attachment-link integrity;
- FTS row parity;
- conversation-assignment completeness;
- reconciliation of import counters; and
- contradictions among persisted state, manifest, and database data.

When all applicable checks pass, the UI uses this exact statement:

> Internally verified against this workspace’s extracted source records.

The completion summary may say “Import completed in 18m 32s and passed
internal verification.” “Internal verification” has exactly the qualified
meaning below; the UI never says that everything in Outlook or Exchange was
imported.

This means the local index is internally consistent with the recorded
extraction corpus and provenance for that workspace revision. It does not
establish parity with a live Exchange mailbox, an Outlook profile, server
retention history, or every message ever present in a source system. It is also
not a tamper-evident forensic authenticity proof against coordinated alteration
of the workspace and its private metadata. It is not authorization to remove
the original PST.

If the source is unavailable, local workspace checks may still succeed, but
the result must say that source integrity is unavailable and omit any claim
that requires a current source comparison.

### Full Verify

Phase 2's backend also supports a bounded Full Verify. It performs every Quick
Verify check and rereads each extracted candidate to recompute its size and
SHA-256 freshness guard before comparing the complete canonical identity set.
It does not mutate the active database or reparse message content. The restrained
Workspace UI exposes Quick Verify by default; Full Verify remains available to
future UI or diagnostic integration. It has the same proof boundary and does
not claim historic or live mailbox parity.

## Verification result contract

A verification report is versioned and privacy-safe. It includes:

- verification level;
- result category: verified, incomplete, repairable, source unavailable,
  source changed, extraction inconsistent, database inconsistent, unsupported,
  or failed;
- current workspace state;
- discovered candidates, canonical messages, inserted messages,
  skipped-existing messages, parse failures, attachments, folders, and FTS
  rows;
- conversation-assignment state;
- missing and unexpected canonical-record counts;
- possible duplicate-group count and opaque group identifiers;
- source-integrity result;
- canonical UTC verification start and completion timestamps and verification
  elapsed duration;
- the persisted completed active import duration, when available;
- privacy-safe warnings; and
- recommended actions.

Normal results do not contain message text, message-derived fields, attachment
names, search terms, absolute paths, or relative canonical identities. Detailed
relative identities remain private manifest provenance and are not part of the
Phase 2 report wire contract.

## Possible duplicate source records

Phase 2 may issue this informational warning:

> Possible duplicate source records detected.

Signals may include identical valid Message-ID values, identical freshness or
content digests at different canonical relative paths, matching normalized
header/body digests, or repeated extraction provenance. These are diagnostic
signals, not proof of corruption. The normal report exposes only group counts
and opaque group identifiers. The current local Details view shows only opaque
identifiers and privacy-safe signal names. A future separately invoked local
details operation may expose validated workspace-relative identities, but never
message content or an absolute source path.

Possible duplicates are not hidden, merged, suppressed, or deleted. Legitimate
identical messages at different canonical paths remain separate indexed
messages. Counts remain canonical database counts. A duplicate warning never
authorizes an automatic repair or changes the canonical identity policy.

## Verify and non-destructive repair actions

The Workspace UI distinguishes these actions:

- **Verify** runs Quick Verify and creates a report without repairing data.
- **Resume Indexing** reuses the existing extraction output, applies the Phase 1
  canonical identity, inserts only missing canonical records, skips existing
  records, preserves existing message IDs, and updates counters
  transactionally.
- **Repair Index** (Rebuild Index From Extracted Files) does not invoke ReadPST.
  It discovers and indexes the existing extraction output into a new temporary
  database, verifies that database, and promotes it only after verification
  succeeds.
- **Rebuild Conversations** changes only conversation metadata derived from
  existing canonical messages. It is idempotent and does not rewrite messages,
  folders, attachments, or extraction output.
- **Re-verify** reruns Quick Verify after an action.

Repair Index leaves the active index usable while the temporary replacement is
built when safe to do so. The active database is never deleted before its
replacement passes verification. Promotion checkpoints and synchronizes the
replacement, retains the previous active database as a rollback backup through
the replacement operation, atomically swaps the verified database into place,
and removes the backup only after the new active database is durably installed
and its post-promotion invariants pass. A build, verification, checkpoint,
or pre-promotion cancellation failure leaves or restores the original active
database. Cancellation is checked before the promotion commit boundary; once
the atomic swap begins, completion wins so the code can finish checked
verification and backup handling without falsely reporting that the old index
remains active. An uncertain promotion result fails closed for inspection
instead of guessing which database is authoritative.

Start Fresh and Delete Workspace remain separate destructive options with their
existing marker and confirmation protections. Verify or repair approval does
not authorize either destructive action. Verify, Resume Indexing, Repair Index,
Rebuild Conversations, and Re-verify do not modify the source PST or extraction
output. Start Fresh and Delete Workspace may change marker-validated workspace
data only; they never modify the source PST.

## Source availability and change

Source comparison is read-only and records only privacy-safe result categories
outside existing private workspace metadata:

- **Mounted and unchanged:** size, modification time, and the recorded source
  fingerprint are consistent. The manifest retains an already available source
  hash as provenance; Quick Verify does not reread a large PST solely to
  recompute it.
- **Unavailable:** the local workspace remains usable and is not automatically
  invalidated. Verification cannot make a source-comparison claim.
- **Moved:** a missing original location is treated as unavailable until the
  user identifies the moved PST. The selected file is compared read-only
  against recorded identity; matching provenance may restore source-integrity
  comparison without exposing its full path by default.
- **Changed:** a size, modification-time, fingerprint, or applicable hash
  mismatch produces a clear source-changed warning. It never deletes or
  replaces the local database.
- **Workspace volume disappeared:** the operation stops without promoting a
  replacement. It does not recreate the missing workspace merely to record a
  new state; the last state already persisted on that volume remains the
  durable record if the volume returns. Disappearance is never treated as
  permission to recreate or delete user data.

The source PST is never opened writable. Verification, resume, and repair must
leave its bytes, size, and modification time unchanged. Missing or mismatched
source identity never causes automatic database or extraction deletion.

## Structurally empty ReadPST candidates

Phase 2 retains Phase 1's fail-open policy. It does not suppress any extracted EML
because it has a blank subject, no subject, an empty body, a
generated-looking filename, or an exact LibPST empty envelope. Current evidence
does not provide deterministic extractor provenance that can distinguish a
legitimate completely blank source message, a field-extraction failure, and a
non-message auxiliary output. Treating any of those cases as a proven artifact
could hide mail.

Therefore:

- every newly discovered relative identity remains visible and indexed,
  including exact empty envelopes and auxiliary-looking filenames;
- malformed candidates use the existing privacy-safe parse-failure placeholder;
- no per-record suppression reason or source-record diagnostic is emitted; and
- structurally_empty_artifacts_suppressed remains reserved, fail-open, and zero.

The suppression count remains zero pending deterministic extractor provenance.

Future suppression requires a deterministic signal tied to the bundled
ReadPST version, invocation, and extractor provenance that proves an output is
auxiliary rather than a source message. It must preserve legitimate blank,
no-subject, and empty-body messages and fail open for unknown versions,
templates, names, MIME shapes, and provenance. See
[LOGGING.md](LOGGING.md) for logging restrictions.

## Import counters

Import, resume, repair, and verification accounting distinguishes:

- extraction output files and bytes observed by the bounded post-exit safety
  validation;
- source PST size; `workspaceBytesWritten` remains unavailable unless a future
  bounded measurement covers the whole documented workspace aggregate;
- candidates discovered and processed;
- canonical messages currently indexed;
- canonical messages inserted during the pass;
- existing canonical messages skipped during the pass;
- parse failures;
- attachments indexed for newly inserted messages;
- folder and FTS row totals;
- conversation records or groups processed and their reliable total, when one
  exists;
- verification checks completed and the exact check total;
- missing and unexpected canonical records;
- possible duplicate-group counts; and
- the reserved structurally empty artifact suppression counter, which remains
  zero.

The canonical indexed count is database truth, not the number of loop
iterations. Processed equals inserted plus skipped, with parse failures also
counted among inserted placeholders. The suppression counter is not a third
outcome. Counters are numeric operational metadata and never contain
message-derived values.

Extraction file and byte values are coarse observations, not proof of a
percentage. Candidate records are extracted records considered for indexing;
they are not a claim of Outlook, Exchange, or source-server message parity.
Canonical messages are rows established under `workspace-relative-eml-v1`.
The result-list page size, mounted-row count, and search-result count are never
used as import counters. Human-readable byte values may be shown in the compact
presentation, while exact aggregate bytes remain available in local Details.

Persisted discovery and canonical totals survive resume and are never added to
the display twice. Phase 1 and Phase 2 candidate-processing, insertion,
skip-existing, and newly indexed attachment counters begin a separately
identified and labeled pass when resume or repair begins; they do not replace
or reduce the stable canonical total. Each committed batch stores rows and
counter snapshots in the same SQLite transaction. The state revision is a
separate atomic sidecar checkpoint. Read-only reopen summaries derive observed
counts from
`canonical_messages_total` or `SELECT COUNT(*) FROM messages`; the next
integrity operation persists interrupted-state reconciliation before it begins.

## Cause boundary and known limits

The schema-v3 database already has a unique messages.eml_path key. An ordinary
repeated insert of the same relative path fails rather than creating another
row, and Phase 1's conflict-safe insert turns that replay into an explicit
skip. Before Phase 1, Resume deleted and rebuilt the temporary index from the
extracted tree instead of appending to that partial database. That discarded
partial progress and changed message IDs, but the deterministic same-path
interruption lifecycle did not double rows.

The synthetic pre-fix corpus contains five ordinary messages and five distinct
auxiliary-looking empty-envelope paths, so the pre-fix scanner indexes ten
rows. The observed external doubling mechanism is not proven by that
reproduction. Regenerated or auxiliary outputs with different relative paths
remain possible explanations, but are hypotheses rather than a root-cause
finding. Conversation building is update-only and cannot add message rows.

Phase 2 verifies internal consistency; it cannot prove that two different
relative paths represent the same original PST record or that the extractor
materialized every record a server or Outlook installation ever held. Subject,
body, Message-ID, attachment content, or digest similarity is not sufficient
evidence for deletion. Unknown extractor provenance stays visible and
fail-open.
