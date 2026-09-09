# Local Trash

## Phase 6 baseline query boundary map

This map records the canonical read boundaries that existed before Local Trash
was added.

- `messages.eml_path` is the schema-v3 unique, binary canonical import identity.
  It uses `workspace-relative-eml-v1`; numeric `messages.id` values are transient
  UI/query handles and are not persistent organization identities.
- Message page, exact-count, FTS, relevance, contextual-snippet, folder-scope,
  single-workspace cursor, and multi-workspace offset paths share the backend
  message query source.
- Conversation summaries also read roots, participants, totals, attachment
  presence, and latest activity through independent subqueries. Expanded
  conversations have separate matching and entire-conversation paths.
- Folder direct and descendant counts come from canonical `folders` and
  `messages` rows. Simplified and Original hierarchy presentation consumes
  those canonical relationships; `All Mail` is a presentation scope, not a
  canonical folder.
- Import Resume, Verify, Re-verify, Rebuild Conversations, and Repair Index
  operate on the canonical index and extracted corpus. Repair atomically
  replaces only `index.sqlite`. Session restoration reopens an existing
  workspace. Marker-gated whole-workspace deletion is a separate destructive
  operation.
- Selected-message detail and pop-out previews read canonical rows and source
  views independently of the result page that selected them.

Local Trash integration must therefore apply visibility in every backend page,
count, conversation, expansion, and folder-count boundary. Filtering a fetched
page in React is not sufficient.

## Storage and identity design

Phase 6 stores reversible organization in a versioned local-state overlay inside
the validated workspace. The overlay uses two private SQLite slots
(`local-state.sqlite` and `local-state-alt.sqlite`) plus a small checksummed
control file. The control file selects the highest valid durable slot record;
writers build and synchronize the inactive slot through a held file descriptor
before synchronizing the next checksummed record. Fresh creation and promotion
of the legacy single-record empty state establish a two-record history before a
message mutation begins. During that one-time promotion, interruption after the
status-only write to the previously empty secondary record can leave the
selector invalid; the overlay then becomes unavailable and fails open, and its
empty local organization can be recovered with Reset. Once both valid history
records exist, a status-only next record leaves the prior slot selected. A
complete durable `PREPARING` record is authoritative and rolls forward after
interruption. A partially replaced or malformed newer control record makes the
overlay unavailable and fail-open instead of falling back to state that may be
stale. Local-state files are never renamed or unlinked, so path substitution
cannot turn cleanup into deletion of another workspace entry.

The overlay is separate from schema-v3 `index.sqlite` and is bound to the
workspace fingerprint, source fingerprint, and `workspace-relative-eml-v1`
identity version. SQLite slots contain canonical relative identities, revisions,
UTC change times, and opaque operation identifiers only. The control file holds
only format metadata, the same non-message scope, slot identity/digests, and a
monotonic selection sequence. No local-state component contains message text or
absolute paths.

The canonical PST, canonical message/folder/attachment/FTS rows, extracted EML
files, and extracted attachment data are never changed or deleted by a Local
Trash operation.

The hardened SQLite configuration continues to forbid database attachment.
For a query, PST QuickView validates and opens the local sidecar separately as
read-only, rejects unsupported schema objects or identity/fingerprint mismatch,
and copies only bounded validated identities into a temporary in-memory table
on an OS-level read-only canonical-index connection. SQL visibility predicates
then use that temporary table for pages and exact counts.

An absent overlay means every canonical message is active and Local Trash is
empty. A corrupt, unavailable, future-version, substituted, linked, oversized,
or fingerprint-mismatched component fails open: ordinary views show canonical
messages, Local Trash actions are unavailable, and no component is deleted
automatically.

The canonical index must also prove that every canonical relative identity is
unique before local state can apply. If a duplicate canonical identity is
detected, the query copies no identities, ordinary views show every canonical
row, Local Trash is unavailable, and Reset is not offered because resetting a
healthy overlay cannot repair an ambiguous canonical index.

Message and conversation page responses carry the Local Trash health observed
by the same validated materialization used for their visibility predicate. If a
query discovers newly invalid state, the frontend immediately marks the overlay
unavailable and clears transient Undo authority; it refreshes the complete
summary and folder overlay only when that reported health differs from the
cached state.

## Product contract

- Ordinary Messages and Conversations use `active` visibility.
- Local Trash uses `trashed` visibility and is a reserved presentation-only
  view outside the canonical PST hierarchy.
- Phase 6 Local Trash presents Messages only. Conversation mode remains an
  active-view feature.
- Move, Restore, and the most-recent Undo are transactional and idempotent.
  Undo restores only identities newly moved by its opaque operation.
- If descriptor-bound post-publication reconciliation cannot prove the complete
  control-and-slot namespace, Move, Restore, or Undo reports an indeterminate
  outcome. The frontend discards the transient Undo token, reloads message and
  preview state, and requires reinspection rather than claiming success or
  failure. Reset uses the same rule for an indeterminate recovery outcome.
- Local Trash survives restart, Resume, Verify, Re-verify, Rebuild
  Conversations, and Repair Index. Missing canonical identities remain orphaned
  local state and are never reassigned.
- Reset Local Trash State is recovery for unhealthy local state only. After
  explicit confirmation, it commits a fresh empty slot and may overwrite only
  the exact validated local-state components. It performs no pathname deletion.
- Whole-workspace deletion also removes workspace-local organization, including
  Local Trash, because it removes the validated workspace. It never changes the
  source PST.

Local Trash hides messages from normal PST QuickView views. It does not change
the source PST or reduce workspace storage. Phase 6 has no Empty Trash or
permanent-purge operation.
