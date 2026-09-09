/**
 * Dependency-free presentation and validation helpers for Import Integrity
 * Phase 3. This module deliberately accepts only the bounded, privacy-safe
 * progress contract. Unknown wire values fail closed instead of being guessed.
 */

export const IMPORT_PROGRESS_FORMAT_VERSION = 1 as const;

export const IMPORT_STAGES = [
  "preflight",
  "extracting",
  "discovering",
  "indexing",
  "building_conversations",
  "verifying",
  "complete",
] as const;

export const IMPORT_OPERATION_KINDS = [
  "new_import",
  "resumed_import",
  "repair",
  "verification",
] as const;

export const IMPORT_WORKSPACE_STORAGE_MODES = ["app_support", "next_to_pst"] as const;

export const IMPORT_UNIT_KINDS = [
  "files",
  "bytes",
  "candidate_records",
  "canonical_messages",
  "conversation_records",
  "verification_checks",
] as const;

export const IMPORT_PROCESS_STATES = [
  "not_applicable",
  "starting",
  "running_recent_activity",
  "running_no_recent_signal",
  "exited_success",
  "exited_failure",
  "cancellation_requested",
  "stalled_evidence",
  "unknown_after_restart",
] as const;

export const IMPORT_PROGRESS_STATUSES = [
  "active",
  "complete",
  "cancelled",
  "incomplete",
  "failed",
] as const;

export const IMPORT_STATUS_CODES = [
  "preflight_running",
  "readpst_running",
  "discovering_candidates",
  "indexing_candidates",
  "building_conversations",
  "verifying_workspace",
  "import_complete",
  "cancellation_requested",
  "import_cancelled",
  "import_incomplete",
  "import_failed",
  "stage_corrected_by_verification",
] as const;

export const IMPORT_WARNING_CODES = [
  "no_recent_activity_signal",
  "source_unavailable",
  "source_changed",
  "possible_duplicate_groups",
  "verification_incomplete",
  "progress_signal_unavailable",
  "prior_timing_unavailable",
] as const;

export const IMPORT_STAGE_CORRECTIONS = ["verification_reclassified"] as const;

export const IMPORT_COUNTER_KEYS = [
  "outputFilesObserved",
  "outputBytesObserved",
  "workspaceBytesWritten",
  "candidatesDiscovered",
  "candidatesProcessed",
  "canonicalMessages",
  "insertedMessages",
  "skippedExistingMessages",
  "parseFailures",
  "attachmentsIndexed",
  "conversationRecordsProcessed",
  "conversationRecordsTotal",
  "verificationChecksCompleted",
  "verificationChecksTotal",
  "missingCanonicalRecords",
  "unexpectedCanonicalRecords",
  "possibleDuplicateGroups",
] as const;

export type ImportStage = (typeof IMPORT_STAGES)[number];
export type ImportOperationKind = (typeof IMPORT_OPERATION_KINDS)[number];
export type ImportWorkspaceStorageMode = (typeof IMPORT_WORKSPACE_STORAGE_MODES)[number];
export type ImportUnitKind = (typeof IMPORT_UNIT_KINDS)[number];
export type ImportProcessState = (typeof IMPORT_PROCESS_STATES)[number];
export type ImportProgressStatus = (typeof IMPORT_PROGRESS_STATUSES)[number];
export type ImportStatusCode = (typeof IMPORT_STATUS_CODES)[number];
export type ImportWarningCode = (typeof IMPORT_WARNING_CODES)[number];
export type ImportStageCorrection = (typeof IMPORT_STAGE_CORRECTIONS)[number];
export type ImportCounterKey = (typeof IMPORT_COUNTER_KEYS)[number];

export type ImportProgressCounters = Record<ImportCounterKey, number | null>;

export type ImportEtaRange = {
  lowerBoundMs: number;
  upperBoundMs: number;
};

export type ImportProgressSnapshot = {
  formatVersion: typeof IMPORT_PROGRESS_FORMAT_VERSION;
  operationId: string;
  generation: number;
  stateRevision: number;
  eventSequence: number;
  workspaceId: string;
  sourceBasename: string;
  sourceSizeBytes: number;
  workspaceStorageMode: ImportWorkspaceStorageMode;
  operationKind: ImportOperationKind;
  stage: ImportStage;
  completedStages: ImportStage[];
  status: ImportProgressStatus;
  determinate: boolean;
  processedUnits: number | null;
  totalUnits: number | null;
  unitKind: ImportUnitKind | null;
  counters: ImportProgressCounters;
  activeElapsedMs: number;
  operationElapsedMs: number;
  stageElapsedMs: number;
  lastActivityUtc: string | null;
  lastActivityAgeMs: number | null;
  processState: ImportProcessState;
  eta: ImportEtaRange | null;
  statusCode: ImportStatusCode;
  warningCodes: ImportWarningCode[];
  stageCorrection: ImportStageCorrection | null;
};

export type ImportSourceIdentityPeer = Pick<
  ImportProgressSnapshot,
  "workspaceId" | "sourceBasename"
>;

export type ImportProgressParseResult =
  | { ok: true; value: ImportProgressSnapshot }
  | { ok: false; error: string };

const STAGE_LABELS: Record<ImportStage, string> = {
  preflight: "Preflight",
  extracting: "Extracting from PST",
  discovering: "Discovering message records",
  indexing: "Indexing messages",
  building_conversations: "Building conversations",
  verifying: "Verifying import",
  complete: "Complete",
};

const OPERATION_LABELS: Record<ImportOperationKind, string> = {
  new_import: "New import",
  resumed_import: "Resumed import",
  repair: "Repair",
  verification: "Verification",
};

const STORAGE_LABELS: Record<ImportWorkspaceStorageMode, string> = {
  app_support: "App Support",
  next_to_pst: "Next to PST",
};

const UNIT_LABELS: Record<ImportUnitKind, { singular: string; plural: string }> = {
  files: { singular: "output file", plural: "output files" },
  bytes: { singular: "byte", plural: "bytes" },
  candidate_records: { singular: "candidate record", plural: "candidate records" },
  canonical_messages: { singular: "canonical message", plural: "canonical messages" },
  conversation_records: {
    singular: "conversation record",
    plural: "conversation records",
  },
  verification_checks: { singular: "verification check", plural: "verification checks" },
};

const STATUS_CODE_SET = new Set<string>(IMPORT_STATUS_CODES);
const STAGE_SET = new Set<string>(IMPORT_STAGES);
const OPERATION_KIND_SET = new Set<string>(IMPORT_OPERATION_KINDS);
const STORAGE_MODE_SET = new Set<string>(IMPORT_WORKSPACE_STORAGE_MODES);
const UNIT_KIND_SET = new Set<string>(IMPORT_UNIT_KINDS);
const PROCESS_STATE_SET = new Set<string>(IMPORT_PROCESS_STATES);
const STATUS_SET = new Set<string>(IMPORT_PROGRESS_STATUSES);
const WARNING_CODE_SET = new Set<string>(IMPORT_WARNING_CODES);
const STAGE_CORRECTION_SET = new Set<string>(IMPORT_STAGE_CORRECTIONS);
const COUNTER_KEY_SET = new Set<string>(IMPORT_COUNTER_KEYS);

const ROOT_KEYS = new Set([
  "formatVersion",
  "operationId",
  "generation",
  "stateRevision",
  "eventSequence",
  "workspaceId",
  "sourceBasename",
  "sourceSizeBytes",
  "workspaceStorageMode",
  "operationKind",
  "stage",
  "completedStages",
  "status",
  "determinate",
  "processedUnits",
  "totalUnits",
  "unitKind",
  "counters",
  "activeElapsedMs",
  "operationElapsedMs",
  "stageElapsedMs",
  "lastActivityUtc",
  "lastActivityAgeMs",
  "processState",
  "eta",
  "statusCode",
  "warningCodes",
  "stageCorrection",
]);

const ETA_KEYS = new Set(["lowerBoundMs", "upperBoundMs"]);
const OPAQUE_ID_PATTERN = /^[A-Za-z0-9][A-Za-z0-9._:-]*$/;
const UTC_TIMESTAMP_PATTERN = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/;
const MAX_ETA_MS = 7 * 24 * 60 * 60 * 1_000;
const ETA_SAMPLE_UNITS = 10;
const ETA_SAMPLE_ACTIVE_MS = 10_000;
const ETA_STALL_AFTER_MS = 30_000;
const ACTIVE_STAGE_STATUS_CODES: Record<ImportStage, ImportStatusCode> = {
  preflight: "preflight_running",
  extracting: "readpst_running",
  discovering: "discovering_candidates",
  indexing: "indexing_candidates",
  building_conversations: "building_conversations",
  verifying: "verifying_workspace",
  complete: "import_complete",
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function assertExactKeys(
  value: Record<string, unknown>,
  expected: ReadonlySet<string>,
  label: string,
): void {
  const keys = Object.keys(value);
  for (const key of keys) {
    if (!expected.has(key)) throw new Error(`${label} contains unsupported field ${key}`);
  }
  for (const key of expected) {
    if (!Object.prototype.hasOwnProperty.call(value, key)) {
      throw new Error(`${label} is missing required field ${key}`);
    }
  }
}

function requireString(value: unknown, label: string, maximumLength = 255): string {
  if (typeof value !== "string" || value.length === 0 || value.length > maximumLength) {
    throw new Error(`${label} must be a non-empty bounded string`);
  }
  return value;
}

function requireOpaqueId(value: unknown, label: string, maximumLength: number): string {
  const parsed = requireString(value, label, maximumLength);
  if (!OPAQUE_ID_PATTERN.test(parsed)) throw new Error(`${label} is not an opaque identifier`);
  return parsed;
}

function requireBasename(value: unknown): string {
  const parsed = requireString(value, "sourceBasename", 510);
  const characters = Array.from(parsed);
  if (
    characters.length > 255 ||
    new TextEncoder().encode(parsed).length > 1_024 ||
    parsed === "." ||
    parsed === ".." ||
    parsed.includes("/") ||
    parsed.includes("\\") ||
    characters.some((character) => {
      const codePoint = character.codePointAt(0) ?? 0;
      return codePoint <= 0x1f || (codePoint >= 0x7f && codePoint <= 0x9f);
    })
  ) {
    throw new Error("sourceBasename must not contain a path");
  }
  return parsed;
}

function requireSafeInteger(value: unknown, label: string, minimum = 0): number {
  if (!Number.isSafeInteger(value) || (value as number) < minimum) {
    throw new Error(`${label} must be a safe integer greater than or equal to ${minimum}`);
  }
  return value as number;
}

function requireNullableSafeInteger(value: unknown, label: string): number | null {
  return value === null ? null : requireSafeInteger(value, label);
}

function requireEnum<T extends string>(
  value: unknown,
  allowed: ReadonlySet<string>,
  label: string,
): T {
  if (typeof value !== "string" || !allowed.has(value)) {
    throw new Error(`${label} has an unsupported value`);
  }
  return value as T;
}

function requireUtcTimestamp(value: unknown): string | null {
  if (value === null) return null;
  const parsed = requireString(value, "lastActivityUtc", 40);
  if (
    !UTC_TIMESTAMP_PATTERN.test(parsed) ||
    !Number.isFinite(Date.parse(parsed)) ||
    new Date(parsed).toISOString() !== parsed
  ) {
    throw new Error("lastActivityUtc must be a canonical UTC timestamp");
  }
  return parsed;
}

function parseCounters(value: unknown): ImportProgressCounters {
  if (!isRecord(value)) throw new Error("counters must be an object");
  assertExactKeys(value, COUNTER_KEY_SET, "counters");
  const counters = {} as ImportProgressCounters;
  for (const key of IMPORT_COUNTER_KEYS) {
    counters[key] = requireNullableSafeInteger(value[key], `counters.${key}`);
  }
  return counters;
}

function parseEta(value: unknown): ImportEtaRange | null {
  if (value === null) return null;
  if (!isRecord(value)) throw new Error("eta must be an object or null");
  assertExactKeys(value, ETA_KEYS, "eta");
  const lowerBoundMs = requireSafeInteger(value.lowerBoundMs, "eta.lowerBoundMs", 1);
  const upperBoundMs = requireSafeInteger(value.upperBoundMs, "eta.upperBoundMs", 1);
  if (lowerBoundMs > upperBoundMs) throw new Error("eta bounds are reversed");
  if (upperBoundMs > MAX_ETA_MS) throw new Error("eta exceeds the bounded display range");
  return { lowerBoundMs, upperBoundMs };
}

function parseCompletedStages(value: unknown): ImportStage[] {
  if (!Array.isArray(value) || value.length > IMPORT_STAGES.length) {
    throw new Error("completedStages must be a bounded array");
  }
  const parsed = value.map((stage, index) =>
    requireEnum<ImportStage>(stage, STAGE_SET, `completedStages[${index}]`),
  );
  let previousRank = -1;
  for (const stage of parsed) {
    const rank = stageRank(stage);
    if (rank <= previousRank) {
      throw new Error("completedStages must be unique and in canonical order");
    }
    previousRank = rank;
  }
  return parsed;
}

function parseWarnings(value: unknown): ImportWarningCode[] {
  if (!Array.isArray(value) || value.length > IMPORT_WARNING_CODES.length) {
    throw new Error("warningCodes must be a bounded array");
  }
  const warnings = value.map((warning, index) =>
    requireEnum<ImportWarningCode>(warning, WARNING_CODE_SET, `warningCodes[${index}]`),
  );
  if (new Set(warnings).size !== warnings.length) {
    throw new Error("warningCodes must not contain duplicates");
  }
  return warnings;
}

function validateSnapshotSemantics(snapshot: ImportProgressSnapshot): void {
  if (snapshot.status === "complete") {
    if (snapshot.stage !== "complete" || snapshot.statusCode !== "import_complete") {
      throw new Error("complete status requires the verified complete stage");
    }
    if (
      snapshot.completedStages.length !== IMPORT_STAGES.length ||
      !IMPORT_STAGES.every((stage, index) => snapshot.completedStages[index] === stage)
    ) {
      throw new Error("verified completion requires every completed stage");
    }
  } else {
    if (snapshot.stage === "complete") {
      throw new Error("the complete stage is reserved for verified completion");
    }
    if (
      snapshot.completedStages.includes("complete") ||
      snapshot.completedStages.some((stage) => stageRank(stage) >= stageRank(snapshot.stage))
    ) {
      throw new Error("completed stage evidence must precede the current stage");
    }
  }

  const terminalStatusCodes: Partial<Record<ImportProgressStatus, ImportStatusCode>> = {
    cancelled: "import_cancelled",
    incomplete: "import_incomplete",
    failed: "import_failed",
  };
  const expectedTerminalCode = terminalStatusCodes[snapshot.status];
  if (expectedTerminalCode && snapshot.statusCode !== expectedTerminalCode) {
    throw new Error(`${snapshot.status} status has an inconsistent statusCode`);
  }
  if (
    snapshot.status === "active" &&
    ["import_complete", "import_cancelled", "import_incomplete", "import_failed"].includes(
      snapshot.statusCode,
    )
  ) {
    throw new Error("active status cannot carry a terminal statusCode");
  }
  if (
    snapshot.status === "active" &&
    snapshot.stageCorrection === null &&
    snapshot.statusCode !== "cancellation_requested" &&
    snapshot.statusCode !== ACTIVE_STAGE_STATUS_CODES[snapshot.stage]
  ) {
    throw new Error("active statusCode does not match the current stage");
  }

  if (snapshot.stageCorrection === null) {
    if (snapshot.statusCode === "stage_corrected_by_verification") {
      throw new Error("a stage correction explanation is required");
    }
  } else if (snapshot.statusCode !== "stage_corrected_by_verification") {
    throw new Error("stageCorrection requires the verification correction statusCode");
  }

  if (snapshot.determinate) {
    if (
      snapshot.status !== "active" ||
      snapshot.processedUnits === null ||
      snapshot.totalUnits === null ||
      snapshot.unitKind === null ||
      snapshot.totalUnits <= 0 ||
      snapshot.processedUnits > snapshot.totalUnits
    ) {
      throw new Error("determinate progress requires a trustworthy numerator and denominator");
    }
  } else {
    if (snapshot.totalUnits !== null || snapshot.eta !== null) {
      throw new Error("indeterminate progress cannot expose a total or ETA");
    }
    if ((snapshot.processedUnits === null) !== (snapshot.unitKind === null)) {
      throw new Error("observed units and their aggregate unit kind must appear together");
    }
  }

  if (snapshot.processedUnits === null && snapshot.totalUnits !== null) {
    throw new Error("totalUnits cannot appear without processedUnits");
  }
  if (snapshot.eta !== null) {
    if (
      snapshot.status !== "active" ||
      !snapshot.determinate ||
      snapshot.processedUnits === snapshot.totalUnits ||
      !["indexing", "building_conversations"].includes(snapshot.stage)
    ) {
      throw new Error("ETA is not eligible for this progress state");
    }
  }

  if (snapshot.status !== "active" && snapshot.eta !== null) {
    throw new Error("terminal progress cannot retain an ETA");
  }
  if (
    snapshot.status === "complete" &&
    (snapshot.processedUnits !== null || snapshot.totalUnits !== null || snapshot.unitKind !== null)
  ) {
    throw new Error("verified completion cannot retain active stage units");
  }
  if (snapshot.lastActivityAgeMs !== null && snapshot.lastActivityUtc === null) {
    throw new Error("last activity age requires a canonical activity timestamp");
  }

  const boundedCounterPairs: Array<[number | null, number | null]> = [
    [snapshot.counters.candidatesProcessed, snapshot.counters.candidatesDiscovered],
    [
      snapshot.counters.conversationRecordsProcessed,
      snapshot.counters.conversationRecordsTotal,
    ],
    [
      snapshot.counters.verificationChecksCompleted,
      snapshot.counters.verificationChecksTotal,
    ],
  ];
  if (
    boundedCounterPairs.some(
      ([processed, total]) => processed !== null && total !== null && processed > total,
    )
  ) {
    throw new Error("aggregate processed counters cannot exceed their known totals");
  }
  if (
    snapshot.counters.insertedMessages !== null &&
    snapshot.counters.skippedExistingMessages !== null &&
    snapshot.counters.candidatesProcessed !== null &&
    snapshot.counters.insertedMessages + snapshot.counters.skippedExistingMessages >
      snapshot.counters.candidatesProcessed
  ) {
    throw new Error("inserted and skipped counters cannot exceed processed candidates");
  }
}

/** Parse an untrusted Tauri event without accepting future or unbounded fields. */
export function parseImportProgressSnapshot(input: unknown): ImportProgressParseResult {
  try {
    if (!isRecord(input)) throw new Error("progress snapshot must be an object");
    assertExactKeys(input, ROOT_KEYS, "progress snapshot");

    if (input.formatVersion !== IMPORT_PROGRESS_FORMAT_VERSION) {
      throw new Error("unsupported progress formatVersion");
    }

    const stage = requireEnum<ImportStage>(input.stage, STAGE_SET, "stage");
    const status = requireEnum<ImportProgressStatus>(input.status, STATUS_SET, "status");
    const determinate = input.determinate;
    if (typeof determinate !== "boolean") throw new Error("determinate must be boolean");

    let unitKind: ImportUnitKind | null = null;
    if (input.unitKind !== null) {
      unitKind = requireEnum<ImportUnitKind>(input.unitKind, UNIT_KIND_SET, "unitKind");
    }

    let stageCorrection: ImportStageCorrection | null = null;
    if (input.stageCorrection !== null) {
      stageCorrection = requireEnum<ImportStageCorrection>(
        input.stageCorrection,
        STAGE_CORRECTION_SET,
        "stageCorrection",
      );
    }

    const snapshot: ImportProgressSnapshot = {
      formatVersion: IMPORT_PROGRESS_FORMAT_VERSION,
      operationId: requireOpaqueId(input.operationId, "operationId", 128),
      generation: requireSafeInteger(input.generation, "generation", 1),
      stateRevision: requireSafeInteger(input.stateRevision, "stateRevision", 1),
      eventSequence: requireSafeInteger(input.eventSequence, "eventSequence", 1),
      workspaceId: requireOpaqueId(input.workspaceId, "workspaceId", 256),
      sourceBasename: requireBasename(input.sourceBasename),
      sourceSizeBytes: requireSafeInteger(input.sourceSizeBytes, "sourceSizeBytes"),
      workspaceStorageMode: requireEnum<ImportWorkspaceStorageMode>(
        input.workspaceStorageMode,
        STORAGE_MODE_SET,
        "workspaceStorageMode",
      ),
      operationKind: requireEnum<ImportOperationKind>(
        input.operationKind,
        OPERATION_KIND_SET,
        "operationKind",
      ),
      stage,
      completedStages: parseCompletedStages(input.completedStages),
      status,
      determinate,
      processedUnits: requireNullableSafeInteger(input.processedUnits, "processedUnits"),
      totalUnits: requireNullableSafeInteger(input.totalUnits, "totalUnits"),
      unitKind,
      counters: parseCounters(input.counters),
      activeElapsedMs: requireSafeInteger(input.activeElapsedMs, "activeElapsedMs"),
      operationElapsedMs: requireSafeInteger(input.operationElapsedMs, "operationElapsedMs"),
      stageElapsedMs: requireSafeInteger(input.stageElapsedMs, "stageElapsedMs"),
      lastActivityUtc: requireUtcTimestamp(input.lastActivityUtc),
      lastActivityAgeMs: requireNullableSafeInteger(
        input.lastActivityAgeMs,
        "lastActivityAgeMs",
      ),
      processState: requireEnum<ImportProcessState>(
        input.processState,
        PROCESS_STATE_SET,
        "processState",
      ),
      eta: parseEta(input.eta),
      statusCode: requireEnum<ImportStatusCode>(input.statusCode, STATUS_CODE_SET, "statusCode"),
      warningCodes: parseWarnings(input.warningCodes),
      stageCorrection,
    };

    validateSnapshotSemantics(snapshot);
    return { ok: true, value: snapshot };
  } catch (error) {
    return { ok: false, error: error instanceof Error ? error.message : "invalid progress snapshot" };
  }
}

export function stageRank(stage: ImportStage): number {
  return IMPORT_STAGES.indexOf(stage);
}

export function stageLabel(stage: ImportStage): string {
  return STAGE_LABELS[stage];
}

export function operationKindLabel(kind: ImportOperationKind): string {
  return OPERATION_LABELS[kind];
}

export function workspaceStorageModeLabel(mode: ImportWorkspaceStorageMode): string {
  return STORAGE_LABELS[mode];
}

export type ImportProgressRejectionReason =
  | "invalid_contract"
  | "closed_workspace"
  | "workspace_mismatch"
  | "operation_mismatch"
  | "generation_mismatch"
  | "stale_revision"
  | "stale_event_sequence"
  | "event_sequence_conflict"
  | "terminal_state"
  | "stage_regression"
  | "stage_revision_mismatch"
  | "completed_stage_regression"
  | "counter_regression"
  | "elapsed_regression"
  | "operation_elapsed_regression"
  | "stage_elapsed_regression"
  | "unit_regression"
  | "source_identity_changed";

export type ImportProgressReducerContext = {
  /** Includes open tabs and intentionally registered background operations. */
  acceptedWorkspaceIds: ReadonlySet<string> | readonly string[];
};

export type ImportProgressReducerResult =
  | {
      accepted: true;
      changed: boolean;
      snapshot: ImportProgressSnapshot;
      correctionExplanation: string | null;
    }
  | {
      accepted: false;
      reason: ImportProgressRejectionReason;
      snapshot: ImportProgressSnapshot | null;
      error?: string;
    };

function rejected(
  snapshot: ImportProgressSnapshot | null,
  reason: ImportProgressRejectionReason,
  error?: string,
): ImportProgressReducerResult {
  return error === undefined
    ? { accepted: false, reason, snapshot }
    : { accepted: false, reason, snapshot, error };
}

function hasAcceptedWorkspace(
  accepted: ReadonlySet<string> | readonly string[],
  workspaceId: string,
): boolean {
  const setLike = accepted as ReadonlySet<string>;
  if (typeof setLike.has === "function") return setLike.has(workspaceId);
  return (accepted as readonly string[]).some((candidate) => candidate === workspaceId);
}

function isExplainedCorrection(snapshot: ImportProgressSnapshot): boolean {
  return (
    snapshot.stageCorrection === "verification_reclassified" &&
    snapshot.statusCode === "stage_corrected_by_verification"
  );
}

function completedStageSetIsMonotonic(
  previous: ImportProgressSnapshot,
  candidate: ImportProgressSnapshot,
): boolean {
  return previous.completedStages.every((stage) => candidate.completedStages.includes(stage));
}

function snapshotsAreIdentical(
  left: ImportProgressSnapshot,
  right: ImportProgressSnapshot,
): boolean {
  // Both values have already been parsed into the same fixed, bounded shape.
  return JSON.stringify(left) === JSON.stringify(right);
}

/**
 * Apply one parsed-or-untrusted event to one operation generation. The caller
 * starts a fresh reducer slot for a new operation/generation. Ordering is
 * generation, then persisted state revision, then per-event sequence. An
 * exact duplicate tuple and payload is accepted without causing a render.
 */
export function reduceImportProgress(
  current: ImportProgressSnapshot | null,
  candidateInput: unknown,
  context: ImportProgressReducerContext,
): ImportProgressReducerResult {
  const parsed = parseImportProgressSnapshot(candidateInput);
  if (!parsed.ok) return rejected(current, "invalid_contract", parsed.error);
  const candidate = parsed.value;

  if (!hasAcceptedWorkspace(context.acceptedWorkspaceIds, candidate.workspaceId)) {
    return rejected(current, "closed_workspace");
  }
  if (current === null) {
    return {
      accepted: true,
      changed: true,
      snapshot: candidate,
      correctionExplanation: stageCorrectionExplanation(candidate),
    };
  }

  if (candidate.workspaceId !== current.workspaceId) return rejected(current, "workspace_mismatch");
  if (
    candidate.sourceBasename !== current.sourceBasename ||
    candidate.sourceSizeBytes !== current.sourceSizeBytes ||
    candidate.workspaceStorageMode !== current.workspaceStorageMode
  ) {
    return rejected(current, "source_identity_changed");
  }
  if (candidate.generation < current.generation) return rejected(current, "generation_mismatch");
  const sameGeneration = candidate.generation === current.generation;
  if (sameGeneration) {
    if (candidate.operationId !== current.operationId) return rejected(current, "operation_mismatch");
    if (candidate.operationKind !== current.operationKind) {
      return rejected(current, "source_identity_changed");
    }
    if (candidate.stateRevision < current.stateRevision) {
      return rejected(current, "stale_revision");
    }
    if (candidate.stateRevision === current.stateRevision) {
      if (candidate.eventSequence < current.eventSequence) {
        return rejected(current, "stale_event_sequence");
      }
      if (candidate.eventSequence === current.eventSequence) {
        if (!snapshotsAreIdentical(current, candidate)) {
          return rejected(current, "event_sequence_conflict");
        }
        return {
          accepted: true,
          changed: false,
          snapshot: current,
          correctionExplanation: stageCorrectionExplanation(current),
        };
      }
      if (candidate.stage !== current.stage) {
        return rejected(current, "stage_revision_mismatch");
      }
    }
    if (candidate.stageCorrection !== null) return rejected(current, "generation_mismatch");
  }

  for (const key of IMPORT_COUNTER_KEYS) {
    const before = current.counters[key];
    const after = candidate.counters[key];
    if (before !== null && (after === null || after < before)) {
      return rejected(current, "counter_regression");
    }
  }
  if (candidate.activeElapsedMs < current.activeElapsedMs) {
    return rejected(current, "elapsed_regression");
  }
  if (!sameGeneration) {
    if (stageRank(candidate.stage) < stageRank(current.stage) && !isExplainedCorrection(candidate)) {
      return rejected(current, "stage_regression");
    }
    if (candidate.stage === current.stage && candidate.stageElapsedMs < current.stageElapsedMs) {
      return rejected(current, "stage_elapsed_regression");
    }
    return {
      accepted: true,
      changed: true,
      snapshot: candidate,
      correctionExplanation: stageCorrectionExplanation(candidate),
    };
  }

  if (current.status !== "active") return rejected(current, "terminal_state");
  if (stageRank(candidate.stage) < stageRank(current.stage)) {
    return rejected(current, "stage_regression");
  }
  if (!completedStageSetIsMonotonic(current, candidate)) {
    return rejected(current, "completed_stage_regression");
  }
  if (candidate.operationElapsedMs < current.operationElapsedMs) {
    return rejected(current, "operation_elapsed_regression");
  }

  if (candidate.stage === current.stage && candidate.stageElapsedMs < current.stageElapsedMs) {
    return rejected(current, "stage_elapsed_regression");
  }

  if (candidate.stage === current.stage && candidate.unitKind === current.unitKind) {
    if (
      current.processedUnits !== null &&
      (candidate.processedUnits === null || candidate.processedUnits < current.processedUnits)
    ) {
      return rejected(current, "unit_regression");
    }
    if (
      current.totalUnits !== null &&
      (candidate.totalUnits === null || candidate.totalUnits !== current.totalUnits)
    ) {
      return rejected(current, "unit_regression");
    }
  }

  return {
    accepted: true,
    changed: true,
    snapshot: candidate,
    correctionExplanation: stageCorrectionExplanation(candidate),
  };
}

export type ImportStageTrackerItem = {
  stage: ImportStage;
  label: string;
  state: "completed" | "active" | "interrupted" | "pending";
  accessibleText: string;
};

export function importStageTracker(snapshot: ImportProgressSnapshot): ImportStageTrackerItem[] {
  return IMPORT_STAGES.map((stage) => {
    let state: ImportStageTrackerItem["state"] = "pending";
    if (stage === snapshot.stage && snapshot.status !== "complete") {
      state = snapshot.status === "active" ? "active" : "interrupted";
    } else if (snapshot.status === "complete" || snapshot.completedStages.includes(stage)) {
      state = "completed";
    }
    const accessibleState =
      state === "completed"
        ? "completed"
        : state === "active"
          ? "current stage"
          : state === "interrupted"
            ? `${snapshot.status} at this stage`
            : "pending";
    return {
      stage,
      label: stageLabel(stage),
      state,
      accessibleText: `${stageLabel(stage)}: ${accessibleState}`,
    };
  });
}

export function stageCorrectionExplanation(snapshot: ImportProgressSnapshot): string | null {
  if (!isExplainedCorrection(snapshot)) return null;
  return "Verification reclassified the saved import state; progress is shown from the last verified stage.";
}

export function importStageExplanation(snapshot: ImportProgressSnapshot): string {
  if (snapshot.status !== "active") {
    return terminalImportPresentation(snapshot)?.detail ?? "The import operation stopped.";
  }
  if (snapshot.statusCode === "cancellation_requested") {
    return "Cancellation was requested; completed stages and durable counts will be preserved.";
  }
  const correction = stageCorrectionExplanation(snapshot);
  if (correction !== null) return correction;
  switch (snapshot.stage) {
    case "preflight":
      return "Checking the source identity, workspace, and available storage.";
    case "extracting":
      return processHealthText(snapshot) ?? "ReadPST is extracting local records from the source PST.";
    case "discovering":
      return snapshot.operationKind === "resumed_import"
        ? "Rechecking extracted candidate records before continuing the interrupted import."
        : "Discovering candidate message records in the extracted output.";
    case "indexing":
      return "Indexing discovered candidate records into canonical workspace messages.";
    case "building_conversations":
      return "Building local conversation assignments from canonical messages.";
    case "verifying":
      return "Running the known internal workspace verification checks.";
    case "complete":
      return "Internal verification completed successfully.";
  }
}

function hashOpaqueId(value: string): string {
  let hash = 0x811c9dc5;
  for (let index = 0; index < value.length; index += 1) {
    hash ^= value.charCodeAt(index);
    hash = Math.imul(hash, 0x01000193);
  }
  return (hash >>> 0).toString(16).toUpperCase().padStart(8, "0").slice(0, 6);
}

/** Return a derived code, never the opaque workspace identity itself. */
export function privacySafeWorkspaceDisambiguator(workspaceId: string): string {
  if (!OPAQUE_ID_PATTERN.test(workspaceId)) return "UNKNOWN";
  return hashOpaqueId(workspaceId);
}

export function sourceDisplayName(
  snapshot: ImportProgressSnapshot,
  peers: readonly ImportSourceIdentityPeer[] = [snapshot],
): string {
  const sameNameWorkspaceIds = new Set(
    peers
      .filter(
        (peer) =>
          peer.sourceBasename.localeCompare(snapshot.sourceBasename, undefined, {
            sensitivity: "accent",
          }) === 0,
      )
      .map((peer) => peer.workspaceId),
  );
  if (sameNameWorkspaceIds.size <= 1) return snapshot.sourceBasename;
  return `${snapshot.sourceBasename} · ${privacySafeWorkspaceDisambiguator(snapshot.workspaceId)}`;
}

export type ImportIdentityPresentation = {
  displayName: string;
  sourceSize: string;
  exactSourceBytes: number;
  storageMode: string;
  operation: string;
};

export function importIdentityPresentation(
  snapshot: ImportProgressSnapshot,
  peers: readonly ImportSourceIdentityPeer[] = [snapshot],
): ImportIdentityPresentation {
  return {
    displayName: sourceDisplayName(snapshot, peers),
    sourceSize: formatByteCount(snapshot.sourceSizeBytes),
    exactSourceBytes: snapshot.sourceSizeBytes,
    storageMode: workspaceStorageModeLabel(snapshot.workspaceStorageMode),
    operation: operationKindLabel(snapshot.operationKind),
  };
}

function activeVerb(kind: ImportOperationKind): string {
  if (kind === "repair") return "Repairing";
  if (kind === "verification") return "Verifying";
  return "Importing";
}

export function importHeadline(
  snapshot: ImportProgressSnapshot,
  background: boolean,
  peers: readonly ImportSourceIdentityPeer[] = [snapshot],
): string {
  const name = sourceDisplayName(snapshot, peers);
  if (snapshot.status !== "active") {
    const terminal = terminalImportPresentation(snapshot, peers);
    return terminal?.headline ?? `${name} import status changed.`;
  }
  const headline = `${activeVerb(snapshot.operationKind)} ${name}`;
  return background ? `${headline} in the background.` : headline;
}

export type ImportPresentationSelection = {
  foreground: ImportProgressSnapshot | null;
  background: ImportProgressSnapshot[];
};

function newerSnapshot(
  left: ImportProgressSnapshot | undefined,
  right: ImportProgressSnapshot,
): ImportProgressSnapshot {
  if (left === undefined) return right;
  if (right.generation !== left.generation) {
    return right.generation > left.generation ? right : left;
  }
  if (right.stateRevision !== left.stateRevision) {
    return right.stateRevision > left.stateRevision ? right : left;
  }
  return right.eventSequence > left.eventSequence ? right : left;
}

/** Select by opaque workspace identity, never by basename or selected path. */
export function selectImportPresentations(
  snapshots: readonly ImportProgressSnapshot[],
  selectedWorkspaceId: string | null,
): ImportPresentationSelection {
  const latestByWorkspace = new Map<string, ImportProgressSnapshot>();
  for (const snapshot of snapshots) {
    latestByWorkspace.set(
      snapshot.workspaceId,
      newerSnapshot(latestByWorkspace.get(snapshot.workspaceId), snapshot),
    );
  }
  const foreground =
    selectedWorkspaceId === null ? null : (latestByWorkspace.get(selectedWorkspaceId) ?? null);
  const background = [...latestByWorkspace.values()]
    .filter(
      (snapshot) => snapshot.status === "active" && snapshot.workspaceId !== selectedWorkspaceId,
    )
    .sort((left, right) => left.sourceBasename.localeCompare(right.sourceBasename));
  return { foreground, background };
}

export type TerminalImportPresentation = {
  kind: "complete" | "cancelled" | "incomplete" | "failed";
  headline: string;
  detail: string;
};

export function terminalImportPresentation(
  snapshot: ImportProgressSnapshot,
  peers: readonly ImportSourceIdentityPeer[] = [snapshot],
): TerminalImportPresentation | null {
  if (snapshot.status === "active") return null;
  const name = sourceDisplayName(snapshot, peers);
  const usesCurrentOperationTimer =
    snapshot.operationKind === "repair" || snapshot.operationKind === "verification";
  const priorTimingUnavailable = snapshot.warningCodes.includes("prior_timing_unavailable");
  const duration = formatCompactDuration(
    usesCurrentOperationTimer ? snapshot.operationElapsedMs : snapshot.activeElapsedMs,
  );
  const operation =
    snapshot.operationKind === "repair"
      ? "Repair"
      : snapshot.operationKind === "verification"
        ? "Verification"
        : "Import";
  const appendPriorImportTimingWarning = (detail: string): string =>
    priorTimingUnavailable && usesCurrentOperationTimer
      ? `${detail} Earlier active import timing was not recorded.`
      : detail;
  if (snapshot.status === "complete") {
    const detail = priorTimingUnavailable && !usesCurrentOperationTimer
      ? "Import completed and passed internal verification; earlier active timing was not recorded."
      : appendPriorImportTimingWarning(
          snapshot.operationKind === "verification"
            ? `Verification completed in ${duration} and passed internal workspace checks.`
            : `${operation} completed in ${duration} and passed internal verification.`,
        );
    return {
      kind: "complete",
      headline: `${operation} completed for ${name}.`,
      detail,
    };
  }
  if (snapshot.status === "cancelled") {
    return {
      kind: "cancelled",
      headline: `${operation} cancelled for ${name}.`,
      detail:
        priorTimingUnavailable && !usesCurrentOperationTimer
          ? `Import cancelled. Recorded active time: ${duration}. Earlier active timing was not recorded; completed stages and counts were preserved.`
          : appendPriorImportTimingWarning(
              `${operation} cancelled after ${duration}; completed stages and counts were preserved.`,
            ),
    };
  }
  const interruptedStage = `at the "${stageLabel(snapshot.stage)}" stage`;
  const verificationOutcome = snapshot.stage === "verifying"
    ? "it did not pass internal verification"
    : "internal verification was not completed";
  if (snapshot.status === "incomplete") {
    return {
      kind: "incomplete",
      headline: `${operation} incomplete for ${name}.`,
      detail:
        priorTimingUnavailable && !usesCurrentOperationTimer
          ? `Import stopped incomplete ${interruptedStage}. Recorded active time: ${duration}. Earlier active timing was not recorded; ${verificationOutcome}.`
          : appendPriorImportTimingWarning(
              `${operation} stopped incomplete after ${duration} ${interruptedStage}; ${verificationOutcome}.`,
            ),
    };
  }
  return {
    kind: "failed",
    headline: `${operation} failed for ${name}.`,
    detail:
      priorTimingUnavailable && !usesCurrentOperationTimer
        ? `Import failed ${interruptedStage}. Recorded active time: ${duration}. Earlier active timing was not recorded; ${verificationOutcome}.`
        : appendPriorImportTimingWarning(
            `${operation} failed after ${duration} ${interruptedStage}; ${verificationOutcome}.`,
          ),
  };
}

export function formatCompactDuration(durationMs: number): string {
  if (!Number.isFinite(durationMs) || durationMs <= 0) return "0s";
  const totalSeconds = Math.floor(durationMs / 1_000);
  if (totalSeconds < 60) return `${totalSeconds}s`;
  const totalMinutes = Math.floor(totalSeconds / 60);
  const seconds = totalSeconds % 60;
  if (totalMinutes < 60) return `${totalMinutes}m ${String(seconds).padStart(2, "0")}s`;
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  return `${hours}h ${String(minutes).padStart(2, "0")}m`;
}

export function completedImportDurationText(
  durationMs: number | null,
  importStateIsComplete: boolean,
): string {
  if (durationMs === null) return importStateIsComplete ? "Not recorded" : "Not completed";
  return formatCompactDuration(durationMs);
}

export function formatByteCount(bytes: number): string {
  if (!Number.isSafeInteger(bytes) || bytes < 0) return "Unknown size";
  const units = ["B", "KB", "MB", "GB", "TB"] as const;
  let value = bytes;
  let unitIndex = 0;
  while (value >= 1_024 && unitIndex < units.length - 1) {
    value /= 1_024;
    unitIndex += 1;
  }
  if (unitIndex === 0) return `${bytes} B`;
  const digits = value >= 100 ? 0 : 1;
  return `${value.toFixed(digits)} ${units[unitIndex]}`;
}

function relativeAge(ageMs: number): string {
  const seconds = Math.max(0, Math.floor(ageMs / 1_000));
  if (seconds < 5) return "just now";
  if (seconds < 60) return `${seconds} second${seconds === 1 ? "" : "s"} ago`;
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} minute${minutes === 1 ? "" : "s"} ago`;
  const hours = Math.floor(minutes / 60);
  return `${hours} hour${hours === 1 ? "" : "s"} ago`;
}

/** liveAgeMs can be advanced from a monotonic UI clock without backend writes. */
export function lastActivityText(
  snapshot: ImportProgressSnapshot,
  liveAgeMs: number | null = snapshot.lastActivityAgeMs,
): string | null {
  if (snapshot.lastActivityUtc === null && liveAgeMs === null) return null;
  if (liveAgeMs === null || !Number.isFinite(liveAgeMs) || liveAgeMs < 0) {
    return "Last activity time is being re-established.";
  }
  return `Last activity: ${relativeAge(liveAgeMs)}`;
}

export function processHealthText(
  snapshot: ImportProgressSnapshot,
  liveAgeMs: number | null = snapshot.lastActivityAgeMs,
): string | null {
  const age = liveAgeMs === null || !Number.isFinite(liveAgeMs) ? 0 : Math.max(0, liveAgeMs);
  const processName = snapshot.stage === "extracting" ? "ReadPST" : "The local process";
  switch (snapshot.processState) {
    case "not_applicable":
      return null;
    case "starting":
      return `${processName} is starting.`;
    case "running_recent_activity":
      return snapshot.stage === "extracting" && !snapshot.determinate
        ? "ReadPST is running; this stage does not provide a percentage."
        : `${processName} is running with recent activity.`;
    case "running_no_recent_signal":
      return `No measurable progress signal for ${formatCompactDuration(age)}; the process is still running.`;
    case "exited_success":
      return `${processName} exited successfully.`;
    case "exited_failure":
      return `${processName} exited with an error.`;
    case "cancellation_requested":
      return "Cancellation requested; waiting for the local operation to stop safely.";
    case "stalled_evidence":
      return "The process has not responded to bounded health checks; review Details.";
    case "unknown_after_restart":
      return "Process activity has not yet been re-established after restart.";
  }
}

export type ImportCounterRow = {
  key: ImportCounterKey;
  label: string;
  value: number;
  displayValue: string;
};

const COUNTER_LABELS: Record<ImportCounterKey, string> = {
  outputFilesObserved: "Output files observed",
  outputBytesObserved: "Output bytes observed",
  workspaceBytesWritten: "Workspace bytes written",
  candidatesDiscovered: "Candidate records discovered",
  candidatesProcessed: "Candidate records processed",
  canonicalMessages: "Canonical messages",
  insertedMessages: "Messages inserted",
  skippedExistingMessages: "Existing canonical messages skipped",
  parseFailures: "Candidate parse failures",
  attachmentsIndexed: "Attachments indexed this pass",
  conversationRecordsProcessed: "Conversation records processed",
  conversationRecordsTotal: "Conversation records total",
  verificationChecksCompleted: "Verification checks completed",
  verificationChecksTotal: "Verification checks total",
  missingCanonicalRecords: "Missing canonical records",
  unexpectedCanonicalRecords: "Unexpected canonical records",
  possibleDuplicateGroups: "Possible duplicate groups",
};

export function importCounterRows(
  snapshot: ImportProgressSnapshot,
  countFormatter: (value: number) => string = (value) => value.toLocaleString("en-US"),
): ImportCounterRow[] {
  return IMPORT_COUNTER_KEYS.flatMap((key) => {
    const value = snapshot.counters[key];
    if (value === null) return [];
    const displayValue =
      key === "outputBytesObserved" || key === "workspaceBytesWritten"
        ? formatByteCount(value)
        : countFormatter(value);
    const passScoped =
      snapshot.operationKind === "resumed_import" || snapshot.operationKind === "repair";
    const label =
      passScoped && key === "candidatesProcessed"
        ? "Candidate records processed this pass"
        : passScoped && key === "insertedMessages"
          ? "Messages inserted this pass"
          : passScoped && key === "skippedExistingMessages"
            ? "Existing canonical messages encountered this pass"
            : COUNTER_LABELS[key];
    return [{ key, label, value, displayValue }];
  });
}

export function unitProgressText(snapshot: ImportProgressSnapshot): string | null {
  if (snapshot.processedUnits === null || snapshot.unitKind === null) return null;
  const unit = UNIT_LABELS[snapshot.unitKind];
  const processed = snapshot.processedUnits.toLocaleString("en-US");
  if (snapshot.totalUnits === null) {
    const label = snapshot.processedUnits === 1 ? unit.singular : unit.plural;
    return `${processed} ${label} observed`;
  }
  const total = snapshot.totalUnits.toLocaleString("en-US");
  return `${processed} of ${total} ${unit.plural} processed`;
}

export type ImportProgressSemantics =
  | {
      kind: "determinate";
      role: "progressbar";
      ariaLabel: string;
      ariaValueMin: 0;
      ariaValueMax: number;
      ariaValueNow: number;
    }
  | {
      kind: "indeterminate";
      role: "status";
      ariaLabel: string;
    }
  | {
      kind: "terminal";
      role: "status";
      ariaLabel: string;
    };

/** There is intentionally no overall percentage in this model. */
export function importProgressSemantics(snapshot: ImportProgressSnapshot): ImportProgressSemantics {
  if (snapshot.status !== "active") {
    return {
      kind: "terminal",
      role: "status",
      ariaLabel: terminalImportPresentation(snapshot)?.detail ?? "Import status changed.",
    };
  }
  if (
    snapshot.determinate &&
    snapshot.processedUnits !== null &&
    snapshot.totalUnits !== null &&
    snapshot.unitKind !== null
  ) {
    return {
      kind: "determinate",
      role: "progressbar",
      ariaLabel: `${stageLabel(snapshot.stage)}: ${unitProgressText(snapshot) ?? "progress"}`,
      ariaValueMin: 0,
      ariaValueMax: snapshot.totalUnits,
      ariaValueNow: snapshot.processedUnits,
    };
  }
  return {
    kind: "indeterminate",
    role: "status",
    ariaLabel: `${stageLabel(snapshot.stage)} is active; a trustworthy percentage is not available.`,
  };
}

function usefulMinutes(value: number): number {
  if (value <= 10) return Math.max(1, Math.round(value));
  if (value <= 60) return Math.max(5, Math.round(value / 5) * 5);
  return Math.round(value / 15) * 15;
}

function etaRangeText(eta: ImportEtaRange): string {
  if (eta.upperBoundMs <= 60_000) return "Less than a minute remaining";
  if (eta.upperBoundMs < 60 * 60_000) {
    const midpointMinutes = usefulMinutes((eta.lowerBoundMs + eta.upperBoundMs) / 120_000);
    const spreadMinutes = (eta.upperBoundMs - eta.lowerBoundMs) / 60_000;
    if (spreadMinutes <= Math.max(2, midpointMinutes * 0.4)) {
      return `About ${midpointMinutes} minute${midpointMinutes === 1 ? "" : "s"} remaining`;
    }
    const lowerMinutes = usefulMinutes(eta.lowerBoundMs / 60_000);
    const upperMinutes = Math.max(lowerMinutes, usefulMinutes(eta.upperBoundMs / 60_000));
    return `Estimated ${lowerMinutes}–${upperMinutes} minutes remaining`;
  }
  const lowerHours = Math.max(1, Math.floor(eta.lowerBoundMs / (60 * 60_000)));
  const upperHours = Math.max(lowerHours, Math.ceil(eta.upperBoundMs / (60 * 60_000)));
  if (lowerHours === upperHours) {
    return `About ${upperHours} hour${upperHours === 1 ? "" : "s"} remaining`;
  }
  return `Estimated ${lowerHours}–${upperHours} hours remaining`;
}

export type ImportEtaPresentation =
  | { kind: "estimate"; text: string }
  | { kind: "estimating"; text: "Estimating…" }
  | { kind: "hidden"; text: null };

export function importEtaPresentation(
  snapshot: ImportProgressSnapshot,
  liveActivityAgeMs: number | null = snapshot.lastActivityAgeMs,
): ImportEtaPresentation {
  const suitableStage = ["indexing", "building_conversations"].includes(snapshot.stage);
  const activitySupportsEta = ![
    "running_no_recent_signal",
    "cancellation_requested",
    "stalled_evidence",
    "exited_failure",
  ].includes(snapshot.processState);
  const warningsSupportEta = !snapshot.warningCodes.some((warning) =>
    ["no_recent_activity_signal", "progress_signal_unavailable"].includes(warning),
  );
  const liveActivitySupportsEta =
    liveActivityAgeMs === null ||
    (Number.isFinite(liveActivityAgeMs) &&
      liveActivityAgeMs >= 0 &&
      liveActivityAgeMs <= ETA_STALL_AFTER_MS);
  if (
    snapshot.status !== "active" ||
    !snapshot.determinate ||
    !suitableStage ||
    snapshot.processedUnits === null ||
    snapshot.totalUnits === null ||
    snapshot.processedUnits >= snapshot.totalUnits ||
    !activitySupportsEta ||
    !warningsSupportEta ||
    !liveActivitySupportsEta
  ) {
    return { kind: "hidden", text: null };
  }
  if (snapshot.eta !== null) return { kind: "estimate", text: etaRangeText(snapshot.eta) };
  if (
    snapshot.processedUnits < ETA_SAMPLE_UNITS ||
    snapshot.stageElapsedMs < ETA_SAMPLE_ACTIVE_MS
  ) {
    return { kind: "estimating", text: "Estimating…" };
  }
  return { kind: "hidden", text: null };
}

const MATERIAL_WARNING_CODES = new Set<ImportWarningCode>([
  "source_unavailable",
  "source_changed",
  "verification_incomplete",
  "prior_timing_unavailable",
]);
const MATERIAL_PROCESS_STATES = new Set<ImportProcessState>([
  "exited_failure",
  "cancellation_requested",
  "stalled_evidence",
]);

/** Stable across timer, activity-age, counter, ETA, and ordinary revision ticks. */
export function materialImportAnnouncementKey(snapshot: ImportProgressSnapshot): string {
  const warnings = snapshot.warningCodes.filter((warning) => MATERIAL_WARNING_CODES.has(warning));
  const process = MATERIAL_PROCESS_STATES.has(snapshot.processState) ? snapshot.processState : "";
  return [
    snapshot.workspaceId,
    snapshot.operationId,
    snapshot.generation,
    snapshot.operationKind,
    snapshot.stage,
    snapshot.status,
    snapshot.statusCode,
    snapshot.stageCorrection ?? "",
    warnings.join(","),
    process,
  ].join(":");
}

export function materialImportAnnouncementText(
  snapshot: ImportProgressSnapshot,
  peers: readonly ImportSourceIdentityPeer[] = [snapshot],
): string {
  const terminal = terminalImportPresentation(snapshot, peers);
  const source = sourceDisplayName(snapshot, peers);
  if (terminal !== null) return `${source}: ${terminal.detail}`;
  const correction = stageCorrectionExplanation(snapshot);
  if (correction !== null) return correction;
  if (
    snapshot.statusCode === "cancellation_requested" ||
    snapshot.processState === "cancellation_requested"
  ) {
    return `Cancellation requested for ${source}.`;
  }
  if (snapshot.processState === "exited_failure") {
    return `The local process exited with an error for ${source}.`;
  }
  if (snapshot.processState === "stalled_evidence") {
    return `The local process has not responded to bounded health checks for ${source}.`;
  }
  if (snapshot.warningCodes.includes("source_unavailable")) {
    return `Source PST unavailable for ${source}; current source comparison is not available.`;
  }
  if (snapshot.warningCodes.includes("source_changed")) {
    return `Source identity changed for ${source}; the local workspace result remains qualified.`;
  }
  if (snapshot.warningCodes.includes("verification_incomplete")) {
    return `Internal verification is incomplete for ${source}.`;
  }
  if (snapshot.warningCodes.includes("prior_timing_unavailable")) {
    return `Only active time recorded since the workspace for ${source} entered Phase 3 is available.`;
  }
  return `${stageLabel(snapshot.stage)} started for ${source}.`;
}
