import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  IMPORT_COUNTER_KEYS,
  IMPORT_OPERATION_KINDS,
  IMPORT_PROCESS_STATES,
  IMPORT_PROGRESS_STATUSES,
  IMPORT_STAGES,
  IMPORT_STATUS_CODES,
  IMPORT_UNIT_KINDS,
  IMPORT_WARNING_CODES,
  completedImportDurationText,
  formatByteCount,
  formatCompactDuration,
  importCounterRows,
  importEtaPresentation,
  importHeadline,
  importIdentityPresentation,
  importProgressSemantics,
  importStageExplanation,
  importStageTracker,
  lastActivityText,
  materialImportAnnouncementKey,
  materialImportAnnouncementText,
  operationKindLabel,
  parseImportProgressSnapshot,
  privacySafeWorkspaceDisambiguator,
  processHealthText,
  reduceImportProgress,
  selectImportPresentations,
  sourceDisplayName,
  stageCorrectionExplanation,
  terminalImportPresentation,
  unitProgressText,
  workspaceStorageModeLabel,
} from "../src/importProgress.ts";

const appSource = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const typeSource = readFileSync(new URL("../src/types.ts", import.meta.url), "utf8");
const styleSource = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");
const rustSource = readFileSync(new URL("../src-tauri/src/lib.rs", import.meta.url), "utf8");
const phase2RustSource = readFileSync(
  new URL("../src-tauri/src/import_integrity_phase2.rs", import.meta.url),
  "utf8",
);

function emptyCounters(overrides = {}) {
  return { ...Object.fromEntries(IMPORT_COUNTER_KEYS.map((key) => [key, null])), ...overrides };
}

function snapshot(overrides = {}) {
  return {
    formatVersion: 1,
    operationId: "import-fictional-001",
    generation: 1,
    stateRevision: 1,
    eventSequence: 1,
    workspaceId: "workspace-fictional-a",
    sourceBasename: "Fictional Archive.pst",
    sourceSizeBytes: 54_975_581_389,
    workspaceStorageMode: "next_to_pst",
    operationKind: "new_import",
    stage: "extracting",
    completedStages: ["preflight"],
    status: "active",
    determinate: false,
    processedUnits: null,
    totalUnits: null,
    unitKind: null,
    counters: emptyCounters(),
    activeElapsedMs: 761_000,
    operationElapsedMs: 761_000,
    stageElapsedMs: 188_000,
    lastActivityUtc: "2026-08-23T16:00:00.000Z",
    lastActivityAgeMs: 3_000,
    processState: "running_recent_activity",
    eta: null,
    statusCode: "readpst_running",
    warningCodes: [],
    stageCorrection: null,
    ...overrides,
  };
}

function indexing(overrides = {}) {
  return snapshot({
    stateRevision: 4,
    stage: "indexing",
    completedStages: ["preflight", "extracting", "discovering"],
    determinate: true,
    processedUnits: 40,
    totalUnits: 100,
    unitKind: "candidate_records",
    counters: emptyCounters({
      candidatesDiscovered: 100,
      candidatesProcessed: 40,
      canonicalMessages: 37,
      insertedMessages: 35,
      skippedExistingMessages: 2,
      parseFailures: 3,
      attachmentsIndexed: 8,
    }),
    activeElapsedMs: 121_000,
    operationElapsedMs: 121_000,
    stageElapsedMs: 61_000,
    processState: "not_applicable",
    statusCode: "indexing_candidates",
    ...overrides,
  });
}

function complete(overrides = {}) {
  return indexing({
    stateRevision: 9,
    stage: "complete",
    completedStages: [...IMPORT_STAGES],
    status: "complete",
    determinate: false,
    processedUnits: null,
    totalUnits: null,
    unitKind: null,
    counters: emptyCounters({
      candidatesDiscovered: 100,
      candidatesProcessed: 100,
      canonicalMessages: 97,
      insertedMessages: 95,
      skippedExistingMessages: 2,
      parseFailures: 3,
      attachmentsIndexed: 18,
      verificationChecksCompleted: 6,
      verificationChecksTotal: 6,
      missingCanonicalRecords: 0,
      unexpectedCanonicalRecords: 0,
      possibleDuplicateGroups: 1,
    }),
    activeElapsedMs: 1_112_000,
    operationElapsedMs: 1_112_000,
    stageElapsedMs: 12_000,
    processState: "not_applicable",
    eta: null,
    statusCode: "import_complete",
    ...overrides,
  });
}

function parsed(input) {
  const result = parseImportProgressSnapshot(input);
  assert.equal(result.ok, true, result.ok ? undefined : result.error);
  return result.value;
}

test("the fixed aggregate-only wire contract parses without exposing free-form content", () => {
  const result = parseImportProgressSnapshot(snapshot());
  assert.equal(result.ok, true);
  assert.equal(result.value.sourceBasename, "Fictional Archive.pst");
  assert.equal(result.value.workspaceId, "workspace-fictional-a");
  assert.equal(result.value.eventSequence, 1);
  assert.equal(result.value.operationElapsedMs, 761_000);
  assert.deepEqual(Object.keys(result.value.counters), IMPORT_COUNTER_KEYS);
  assert.equal("sourcePath" in result.value, false);
  assert.equal("message" in result.value, false);
  assert.equal("subject" in result.value, false);
});

test("unknown format, fields, stages, operations, units, process states, statuses, and codes fail closed", () => {
  const mutations = [
    { formatVersion: 2 },
    { futureField: "future" },
    { stage: "future_stage" },
    { operationKind: "future_operation" },
    { unitKind: "future_units", processedUnits: 2 },
    { processState: "future_process_state" },
    { status: "future_status" },
    { statusCode: "future_status_code" },
    { warningCodes: ["future_warning"] },
    { stageCorrection: "future_correction", statusCode: "stage_corrected_by_verification" },
  ];
  for (const mutation of mutations) {
    assert.equal(parseImportProgressSnapshot(snapshot(mutation)).ok, false, JSON.stringify(mutation));
  }

  for (const field of ["generation", "stateRevision", "eventSequence"]) {
    assert.equal(parseImportProgressSnapshot(snapshot({ [field]: 0 })).ok, false);
  }
  for (const requiredField of ["eventSequence", "operationElapsedMs"]) {
    const missing = snapshot();
    delete missing[requiredField];
    assert.equal(parseImportProgressSnapshot(missing).ok, false);
  }
  assert.equal(parseImportProgressSnapshot(snapshot({ operationElapsedMs: -1 })).ok, false);
  assert.equal(
    parseImportProgressSnapshot(snapshot({ eventSequence: Number.MAX_SAFE_INTEGER + 1 })).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(snapshot({ operationElapsedMs: Number.MAX_SAFE_INTEGER + 1 })).ok,
    false,
  );
});

test("all exact supported enum values are deliberately enumerated", () => {
  assert.deepEqual(IMPORT_STAGES, [
    "preflight",
    "extracting",
    "discovering",
    "indexing",
    "building_conversations",
    "verifying",
    "complete",
  ]);
  assert.deepEqual(IMPORT_OPERATION_KINDS, [
    "new_import",
    "resumed_import",
    "repair",
    "verification",
  ]);
  assert.deepEqual(IMPORT_UNIT_KINDS, [
    "files",
    "bytes",
    "candidate_records",
    "canonical_messages",
    "conversation_records",
    "verification_checks",
  ]);
  assert.equal(IMPORT_PROCESS_STATES.length, 9);
  assert.deepEqual(IMPORT_PROGRESS_STATUSES, [
    "active",
    "complete",
    "cancelled",
    "incomplete",
    "failed",
  ]);
  assert.equal(IMPORT_STATUS_CODES.includes("stage_corrected_by_verification"), true);
  assert.equal(IMPORT_WARNING_CODES.includes("progress_signal_unavailable"), true);
  assert.equal(IMPORT_WARNING_CODES.includes("prior_timing_unavailable"), true);
});

test("the parser requires the fixed counter shape and safe aggregate values", () => {
  const missingCounter = emptyCounters();
  delete missingCounter.parseFailures;
  assert.equal(parseImportProgressSnapshot(snapshot({ counters: missingCounter })).ok, false);
  assert.equal(
    parseImportProgressSnapshot(
      snapshot({ counters: emptyCounters({ mailboxSubject: 1 }) }),
    ).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(
      snapshot({ counters: emptyCounters({ candidatesDiscovered: -1 }) }),
    ).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(
      snapshot({ sourceSizeBytes: Number.MAX_SAFE_INTEGER + 1 }),
    ).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(
      indexing({
        counters: emptyCounters({ candidatesDiscovered: 20, candidatesProcessed: 21 }),
      }),
    ).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(
      indexing({
        counters: emptyCounters({
          candidatesDiscovered: 100,
          candidatesProcessed: 40,
          insertedMessages: 39,
          skippedExistingMessages: 2,
        }),
      }),
    ).ok,
    false,
  );
});

test("source identity is basename-only and opaque IDs cannot smuggle paths", () => {
  for (const unsafeName of [
    "/Users/example/Private Archive.pst",
    "C:\\Private\\Archive.pst",
    "../Archive.pst",
    "folder/Archive.pst",
    "Archive\nPrivate.pst",
  ]) {
    assert.equal(parseImportProgressSnapshot(snapshot({ sourceBasename: unsafeName })).ok, false);
  }
  assert.equal(
    parseImportProgressSnapshot(snapshot({ workspaceId: "/private/workspace" })).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(snapshot({ operationId: "import/../../private" })).ok,
    false,
  );
  assert.equal(parseImportProgressSnapshot(snapshot({ operationId: "import:fictional:1" })).ok, true);
});

test("completed stages are canonical persisted subsets and terminal states are not treated as complete", () => {
  assert.equal(
    parseImportProgressSnapshot(snapshot({ completedStages: ["extracting", "preflight"] })).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(snapshot({ completedStages: ["preflight", "preflight"] })).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(snapshot({ completedStages: ["preflight", "extracting"] })).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(
      snapshot({ stage: "preflight", completedStages: ["preflight"], statusCode: "preflight_running" }),
    ).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(snapshot({ completedStages: ["discovering"] })).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(snapshot({ completedStages: ["preflight", "complete"] })).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(
      indexing({ completedStages: ["preflight", "discovering"] }),
    ).ok,
    true,
  );
  assert.equal(parseImportProgressSnapshot(indexing({ completedStages: [] })).ok, true);
  assert.equal(
    parseImportProgressSnapshot(
      indexing({
        status: "incomplete",
        statusCode: "import_incomplete",
        determinate: false,
        processedUnits: null,
        totalUnits: null,
        unitKind: null,
        completedStages: ["indexing"],
      }),
    ).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(snapshot({ stage: "complete", status: "active" })).ok,
    false,
  );
  assert.equal(parseImportProgressSnapshot(complete()).ok, true);
  assert.equal(
    parseImportProgressSnapshot(complete({ completedStages: IMPORT_STAGES.slice(0, -1) })).ok,
    false,
  );
});

test("determinate progress requires a real numerator and denominator", () => {
  assert.equal(parseImportProgressSnapshot(indexing()).ok, true);
  assert.equal(
    parseImportProgressSnapshot(indexing({ processedUnits: null })).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(indexing({ totalUnits: 0 })).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(indexing({ processedUnits: 101 })).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(snapshot({ totalUnits: 10 })).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(
      snapshot({ processedUnits: 3, unitKind: "files", counters: emptyCounters({ outputFilesObserved: 3 }) }),
    ).ok,
    true,
  );
});

test("reducer rejects closed, cross-workspace, cross-operation, older-generation, and stale events", () => {
  const current = parsed(indexing({ generation: 2 }));
  assert.equal(
    reduceImportProgress(null, snapshot(), { acceptedWorkspaceIds: [] }).reason,
    "closed_workspace",
  );
  assert.equal(
    reduceImportProgress(current, indexing({ workspaceId: "workspace-fictional-b", generation: 2, stateRevision: 5 }), {
      acceptedWorkspaceIds: ["workspace-fictional-a", "workspace-fictional-b"],
    }).reason,
    "workspace_mismatch",
  );
  assert.equal(
    reduceImportProgress(current, indexing({ operationId: "import-fictional-002", generation: 2, stateRevision: 5 }), {
      acceptedWorkspaceIds: ["workspace-fictional-a"],
    }).reason,
    "operation_mismatch",
  );
  assert.equal(
    reduceImportProgress(current, indexing({ generation: 1, stateRevision: 5 }), {
      acceptedWorkspaceIds: ["workspace-fictional-a"],
    }).reason,
    "generation_mismatch",
  );
  assert.equal(
    reduceImportProgress(current, indexing({ generation: 2, stateRevision: 3 }), {
      acceptedWorkspaceIds: ["workspace-fictional-a"],
    }).reason,
    "stale_revision",
  );
});

test("event ordering uses revision then sequence and exact duplicates are idempotent", () => {
  const current = parsed(indexing());
  const duplicate = reduceImportProgress(current, indexing(), {
    acceptedWorkspaceIds: new Set(["workspace-fictional-a"]),
  });
  assert.equal(duplicate.accepted, true);
  assert.equal(duplicate.changed, false);
  assert.equal(duplicate.snapshot, current);

  const conflict = reduceImportProgress(
    current,
    indexing({ activeElapsedMs: current.activeElapsedMs + 1 }),
    { acceptedWorkspaceIds: ["workspace-fictional-a"] },
  );
  assert.equal(conflict.accepted, false);
  assert.equal(conflict.reason, "event_sequence_conflict");

  const laterEvent = reduceImportProgress(
    current,
    indexing({
      eventSequence: 2,
      activeElapsedMs: current.activeElapsedMs + 1,
      operationElapsedMs: current.operationElapsedMs + 1,
      stageElapsedMs: current.stageElapsedMs + 1,
    }),
    { acceptedWorkspaceIds: ["workspace-fictional-a"] },
  );
  assert.equal(laterEvent.accepted, true);
  assert.equal(laterEvent.changed, true);

  assert.equal(
    reduceImportProgress(
      parsed(indexing({ eventSequence: 2 })),
      indexing({ eventSequence: 1 }),
      { acceptedWorkspaceIds: ["workspace-fictional-a"] },
    ).reason,
    "stale_event_sequence",
  );

  const newerRevision = reduceImportProgress(
    parsed(indexing({ eventSequence: 50 })),
    indexing({ stateRevision: 5, eventSequence: 1 }),
    { acceptedWorkspaceIds: ["workspace-fictional-a"] },
  );
  assert.equal(newerRevision.accepted, true);

  const evidenceGrowth = reduceImportProgress(
    parsed(indexing({ completedStages: ["preflight"], eventSequence: 1 })),
    indexing({ completedStages: ["preflight", "discovering"], eventSequence: 2 }),
    { acceptedWorkspaceIds: ["workspace-fictional-a"] },
  );
  assert.equal(evidenceGrowth.accepted, true);

  const unpersistedStageChange = reduceImportProgress(
    current,
    indexing({
      eventSequence: 2,
      stage: "building_conversations",
      completedStages: [...current.completedStages, "indexing"],
      determinate: false,
      processedUnits: null,
      totalUnits: null,
      unitKind: null,
      statusCode: "building_conversations",
    }),
    { acceptedWorkspaceIds: ["workspace-fictional-a"] },
  );
  assert.equal(unpersistedStageChange.accepted, false);
  assert.equal(unpersistedStageChange.reason, "stage_revision_mismatch");
});

test("stage, counter, elapsed, completed evidence, and unit regressions are rejected", () => {
  const current = parsed(indexing());
  const context = { acceptedWorkspaceIds: ["workspace-fictional-a"] };
  const cases = [
    [
      indexing({
        stateRevision: 5,
        stage: "discovering",
        completedStages: ["preflight", "extracting"],
        determinate: false,
        totalUnits: null,
        statusCode: "discovering_candidates",
      }),
      "stage_regression",
    ],
    [
      indexing({
        stateRevision: 5,
        counters: emptyCounters({
          candidatesDiscovered: 100,
          candidatesProcessed: 39,
          canonicalMessages: 37,
          insertedMessages: 35,
          skippedExistingMessages: 2,
          parseFailures: 3,
          attachmentsIndexed: 8,
        }),
      }),
      "counter_regression",
    ],
    [indexing({ stateRevision: 5, activeElapsedMs: 120_999 }), "elapsed_regression"],
    [
      indexing({ stateRevision: 5, operationElapsedMs: 120_999 }),
      "operation_elapsed_regression",
    ],
    [indexing({ stateRevision: 5, stageElapsedMs: 60_999 }), "stage_elapsed_regression"],
    [indexing({ stateRevision: 5, processedUnits: 39 }), "unit_regression"],
    [
      indexing({ stateRevision: 5, completedStages: ["preflight", "discovering"] }),
      "completed_stage_regression",
    ],
  ];
  for (const [candidate, expected] of cases) {
    const result = reduceImportProgress(current, candidate, context);
    assert.equal(result.accepted, false, expected);
    assert.equal(result.reason, expected);
  }
});

test("a real forward stage transition resets only stage time, not total active time", () => {
  const current = parsed(indexing());
  const candidate = indexing({
    stateRevision: 5,
    stage: "building_conversations",
    completedStages: ["preflight", "extracting", "discovering", "indexing"],
    determinate: false,
    processedUnits: null,
    totalUnits: null,
    unitKind: null,
    counters: {
      ...current.counters,
      candidatesProcessed: 100,
      canonicalMessages: 97,
      insertedMessages: 95,
      parseFailures: 3,
    },
    activeElapsedMs: 180_000,
    operationElapsedMs: 180_000,
    stageElapsedMs: 400,
    statusCode: "building_conversations",
  });
  const result = reduceImportProgress(current, candidate, {
    acceptedWorkspaceIds: ["workspace-fictional-a"],
  });
  assert.equal(result.accepted, true);
  assert.equal(result.snapshot.stageElapsedMs, 400);
  assert.equal(result.snapshot.activeElapsedMs, 180_000);
  assert.equal(result.snapshot.operationElapsedMs, 180_000);
});

test("a newer generation cannot discard or lower an observed durable counter", () => {
  const current = parsed(indexing());
  const candidate = indexing({
    operationId: "import-fictional-002",
    generation: 2,
    stateRevision: 1,
    activeElapsedMs: 130_000,
    operationElapsedMs: 1_000,
    counters: { ...current.counters, candidatesProcessed: 39 },
  });
  const result = reduceImportProgress(current, candidate, {
    acceptedWorkspaceIds: ["workspace-fictional-a"],
  });
  assert.equal(result.accepted, false);
  assert.equal(result.reason, "counter_regression");
});

test("a resumed generation resets operation time but preserves same-stage elapsed history", () => {
  const current = parsed(indexing());
  const resumed = indexing({
    operationId: "resume-fictional-002",
    operationKind: "resumed_import",
    generation: 2,
    stateRevision: 4,
    eventSequence: 1,
    operationElapsedMs: 1_000,
    stageElapsedMs: current.stageElapsedMs,
  });
  const accepted = reduceImportProgress(current, resumed, {
    acceptedWorkspaceIds: ["workspace-fictional-a"],
  });
  assert.equal(accepted.accepted, true);
  assert.equal(accepted.snapshot.operationElapsedMs, 1_000);
  assert.equal(accepted.snapshot.stageElapsedMs, current.stageElapsedMs);

  const regressed = reduceImportProgress(
    current,
    { ...resumed, stageElapsedMs: current.stageElapsedMs - 1 },
    { acceptedWorkspaceIds: ["workspace-fictional-a"] },
  );
  assert.equal(regressed.accepted, false);
  assert.equal(regressed.reason, "stage_elapsed_regression");
});

test("verification correction is accepted only as an explicit newer generation", () => {
  const current = parsed(indexing());
  const corrected = snapshot({
    operationId: "verify-fictional-002",
    generation: 2,
    stateRevision: 1,
    stage: "discovering",
    completedStages: ["preflight", "extracting"],
    counters: current.counters,
    activeElapsedMs: 130_000,
    operationElapsedMs: 1_000,
    stageElapsedMs: 1_000,
    processState: "not_applicable",
    statusCode: "stage_corrected_by_verification",
    warningCodes: ["verification_incomplete"],
    stageCorrection: "verification_reclassified",
  });
  const result = reduceImportProgress(current, corrected, {
    acceptedWorkspaceIds: ["workspace-fictional-a"],
  });
  assert.equal(result.accepted, true);
  assert.match(result.correctionExplanation, /Verification reclassified/);
  assert.match(stageCorrectionExplanation(result.snapshot), /last verified stage/);

  const sameGeneration = reduceImportProgress(
    current,
    {
      ...corrected,
      operationId: current.operationId,
      generation: current.generation,
      stateRevision: current.stateRevision + 1,
    },
    { acceptedWorkspaceIds: ["workspace-fictional-a"] },
  );
  assert.equal(sameGeneration.accepted, false);
  assert.equal(sameGeneration.reason, "generation_mismatch");

  assert.equal(
    parseImportProgressSnapshot(
      snapshot({ statusCode: "stage_corrected_by_verification", stageCorrection: null }),
    ).ok,
    false,
  );
});

test("a terminal operation cannot be overwritten by later ordinary progress", () => {
  const current = parsed(complete());
  const result = reduceImportProgress(current, complete({ stateRevision: 10, activeElapsedMs: 1_113_000 }), {
    acceptedWorkspaceIds: ["workspace-fictional-a"],
  });
  assert.equal(result.accepted, false);
  assert.equal(result.reason, "terminal_state");
});

test("stage tracker retains checks, identifies one current stage, and never completes interruption", () => {
  const active = importStageTracker(parsed(indexing()));
  assert.deepEqual(
    active.map((item) => item.state),
    ["completed", "completed", "completed", "active", "pending", "pending", "pending"],
  );
  assert.equal(active[3].accessibleText, "Indexing messages: current stage");

  const evidenceWithGap = importStageTracker(
    parsed(indexing({ completedStages: ["preflight", "discovering"] })),
  );
  assert.deepEqual(
    evidenceWithGap.map((item) => item.state),
    ["completed", "pending", "completed", "active", "pending", "pending", "pending"],
  );

  const cancelled = importStageTracker(
    parsed(
      indexing({
        status: "cancelled",
        statusCode: "import_cancelled",
        determinate: false,
        totalUnits: null,
        eta: null,
      }),
    ),
  );
  assert.equal(cancelled[3].state, "interrupted");
  assert.equal(cancelled.at(-1).state, "pending");

  const done = importStageTracker(parsed(complete()));
  assert.equal(done.every((item) => item.state === "completed"), true);
});

test("foreground and background selection uses opaque workspace identity, not PST name", () => {
  const first = parsed(indexing({ sourceBasename: "Shared Name.pst" }));
  const second = parsed(
    indexing({
      workspaceId: "workspace-fictional-b",
      operationId: "import-fictional-002",
      sourceBasename: "Shared Name.pst",
      generation: 2,
    }),
  );
  const selection = selectImportPresentations([first, second], "workspace-fictional-b");
  assert.equal(selection.foreground.workspaceId, "workspace-fictional-b");
  assert.deepEqual(selection.background.map((item) => item.workspaceId), ["workspace-fictional-a"]);
  assert.match(importHeadline(first, true, [first, second]), /^Importing Shared Name\.pst · [0-9A-F]{6} in the background\.$/);
  assert.doesNotMatch(importHeadline(first, true, [first, second]), /workspace-fictional-a/);

  const laterSameRevision = parsed(
    indexing({
      sourceBasename: "Shared Name.pst",
      eventSequence: 2,
      processedUnits: 41,
      counters: { ...first.counters, candidatesProcessed: 41 },
    }),
  );
  assert.equal(
    selectImportPresentations([laterSameRevision, first], first.workspaceId).foreground
      .eventSequence,
    2,
  );
});

test("same-basename UI disambiguation is stable, derived, and privacy-safe", () => {
  const first = parsed(snapshot({ sourceBasename: "Archive.pst" }));
  const second = parsed(
    snapshot({
      workspaceId: "workspace-fictional-b",
      operationId: "import-fictional-002",
      sourceBasename: "archive.pst",
    }),
  );
  const display = sourceDisplayName(first, [first, second]);
  const code = privacySafeWorkspaceDisambiguator(first.workspaceId);
  assert.equal(display, `Archive.pst · ${code}`);
  assert.match(code, /^[0-9A-F]{6}$/);
  assert.doesNotMatch(display, /workspace-fictional/);
  assert.equal(sourceDisplayName(first, [first]), "Archive.pst");
});

test("sole background progress disambiguates against an idle open PST with the same basename", () => {
  const background = parsed(
    indexing({
      workspaceId: "workspace-fictional-background",
      operationId: "import-fictional-background",
      sourceBasename: "Archive.pst",
    }),
  );
  const idleSelectedPeer = {
    workspaceId: "workspace-fictional-idle",
    sourceBasename: "Archive.pst",
  };
  const peers = [idleSelectedPeer, background];
  const code = privacySafeWorkspaceDisambiguator(background.workspaceId);

  assert.equal(sourceDisplayName(background, peers), `Archive.pst · ${code}`);
  assert.equal(
    importHeadline(background, true, peers),
    `Importing Archive.pst · ${code} in the background.`,
  );
  assert.doesNotMatch(importHeadline(background, true, peers), /workspace-fictional/);
  assert.match(
    appSource,
    /openPstSessions\.map\(\(session\) => \(\{\s*workspaceId: session\.workspace\.id,\s*sourceBasename: pstDisplayName\(session\.workspace\),/,
  );
  assert.match(appSource, /importHeadline\(progress, progressIsBackground, progressIdentityPeers\)/);
});

test("identity presentation includes basename, source size, storage mode, and operation", () => {
  const value = importIdentityPresentation(parsed(snapshot()));
  assert.deepEqual(value, {
    displayName: "Fictional Archive.pst",
    sourceSize: "51.2 GB",
    exactSourceBytes: 54_975_581_389,
    storageMode: "Next to PST",
    operation: "New import",
  });
  assert.equal(operationKindLabel("resumed_import"), "Resumed import");
  assert.equal(operationKindLabel("repair"), "Repair");
  assert.equal(workspaceStorageModeLabel("app_support"), "App Support");
});

test("compact timing and byte formatting avoid milliseconds and false precision", () => {
  assert.equal(formatCompactDuration(0), "0s");
  assert.equal(formatCompactDuration(-1), "0s");
  assert.equal(formatCompactDuration(Number.NaN), "0s");
  assert.equal(formatCompactDuration(3_999), "3s");
  assert.equal(formatCompactDuration(761_999), "12m 41s");
  assert.equal(formatCompactDuration(3_912_000), "1h 05m");
  assert.equal(formatByteCount(0), "0 B");
  assert.equal(formatByteCount(1_536), "1.5 KB");
  assert.equal(formatByteCount(54_975_581_389), "51.2 GB");
});

test("missing duration history distinguishes Complete imports from unfinished imports", () => {
  assert.equal(completedImportDurationText(null, true), "Not recorded");
  assert.equal(completedImportDurationText(null, false), "Not completed");
  assert.equal(completedImportDurationText(1_112_000, true), "18m 32s");
  assert.match(
    appSource,
    /completedImportDurationText\(\s*workspace\.completedImportDurationMs,\s*workspace\.integrityState === "complete",/,
  );
  assert.match(
    appSource,
    /completedImportDurationText\(\s*activeVerificationReport\.completedImportDurationMs,\s*activeVerificationReport\.workspaceState === "complete",/,
  );
});

test("last activity is a real observed signal and elapsed timer movement is separate", () => {
  const value = parsed(snapshot());
  assert.equal(lastActivityText(value), "Last activity: just now");
  assert.equal(lastActivityText(value, 3_000), "Last activity: just now");
  assert.equal(lastActivityText(value, 45_000), "Last activity: 45 seconds ago");
  assert.equal(lastActivityText(value, 125_000), "Last activity: 2 minutes ago");
  assert.equal(lastActivityText(parsed(snapshot({ lastActivityUtc: null, lastActivityAgeMs: null }))), null);
  assert.equal(
    lastActivityText(parsed(snapshot({ lastActivityAgeMs: null }))),
    "Last activity time is being re-established.",
  );
  assert.equal(
    parseImportProgressSnapshot(snapshot({ lastActivityUtc: null, lastActivityAgeMs: 1_000 })).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(
      snapshot({
        operationKind: "resumed_import",
        operationElapsedMs: 1_000,
        stageElapsedMs: 761_001,
      }),
    ).ok,
    true,
  );
});

test("process health wording distinguishes activity, silence, exit, cancellation, and evidence", () => {
  assert.equal(
    processHealthText(parsed(snapshot())),
    "ReadPST is running; this stage does not provide a percentage.",
  );
  assert.equal(
    processHealthText(
      parsed(
        snapshot({
          processState: "running_no_recent_signal",
          warningCodes: ["no_recent_activity_signal"],
          lastActivityAgeMs: 45_000,
        }),
      ),
    ),
    "No measurable progress signal for 45s; the process is still running.",
  );
  assert.match(processHealthText(parsed(snapshot({ processState: "exited_success" }))), /exited successfully/);
  assert.match(
    processHealthText(
      parsed(
        snapshot({
          processState: "cancellation_requested",
          statusCode: "cancellation_requested",
        }),
      ),
    ),
    /Cancellation requested/,
  );
  assert.doesNotMatch(
    processHealthText(parsed(snapshot({ processState: "stalled_evidence" }))),
    /frozen/i,
  );
  assert.match(
    processHealthText(parsed(snapshot({ processState: "unknown_after_restart" }))),
    /re-established after restart/,
  );
});

test("aggregate counters distinguish candidates from canonical messages", () => {
  const value = parsed(indexing());
  const rows = importCounterRows(value);
  assert.deepEqual(
    rows.map((row) => row.label),
    [
      "Candidate records discovered",
      "Candidate records processed",
      "Canonical messages",
      "Messages inserted",
      "Existing canonical messages skipped",
      "Candidate parse failures",
      "Attachments indexed this pass",
    ],
  );
  assert.equal(rows[0].displayValue, "100");
  assert.equal(unitProgressText(value), "40 of 100 candidate records processed");
  assert.doesNotMatch(JSON.stringify(rows), /Outlook|Exchange|parity/i);
});

test("resume and repair mutation counters are explicitly scoped to this pass", () => {
  const labels = importCounterRows(
    parsed(indexing({ operationKind: "resumed_import" })),
  ).map((row) => row.label);
  assert.ok(labels.includes("Candidate records processed this pass"));
  assert.ok(labels.includes("Messages inserted this pass"));
  assert.ok(labels.includes("Existing canonical messages encountered this pass"));
  assert.ok(labels.includes("Canonical messages"));
});

test("extraction is indeterminate and exposes no fake progressbar value or overall percentage", () => {
  const semantics = importProgressSemantics(parsed(snapshot()));
  assert.deepEqual(semantics, {
    kind: "indeterminate",
    role: "status",
    ariaLabel: "Extracting from PST is active; a trustworthy percentage is not available.",
  });
  assert.equal("ariaValueNow" in semantics, false);
  assert.equal("ariaValueMin" in semantics, false);
  assert.equal("ariaValueMax" in semantics, false);
  assert.equal("overallPercentage" in semantics, false);
});

test("indexing exposes a valid determinate stage progressbar without an overall percentage", () => {
  const semantics = importProgressSemantics(parsed(indexing()));
  assert.deepEqual(semantics, {
    kind: "determinate",
    role: "progressbar",
    ariaLabel: "Indexing messages: 40 of 100 candidate records processed",
    ariaValueMin: 0,
    ariaValueMax: 100,
    ariaValueNow: 40,
  });
  assert.equal("overallPercentage" in semantics, false);
});

test("terminal progress has status semantics and no active progress bar", () => {
  const semantics = importProgressSemantics(parsed(complete()));
  assert.equal(semantics.kind, "terminal");
  assert.equal(semantics.role, "status");
  assert.equal("ariaValueNow" in semantics, false);
});

test("ETA estimates are cautious, rounded, finite, and stage-specific", () => {
  const estimating = importEtaPresentation(
    parsed(indexing({ processedUnits: 9, stageElapsedMs: 9_000 })),
  );
  assert.deepEqual(estimating, { kind: "estimating", text: "Estimating…" });

  const stable = importEtaPresentation(
    parsed(
      indexing({
        eta: { lowerBoundMs: 7 * 60_000, upperBoundMs: 9 * 60_000 },
      }),
    ),
  );
  assert.deepEqual(stable, { kind: "estimate", text: "About 8 minutes remaining" });

  const underMinute = importEtaPresentation(
    parsed(indexing({ eta: { lowerBoundMs: 25_000, upperBoundMs: 55_000 } })),
  );
  assert.equal(underMinute.text, "Less than a minute remaining");

  const hours = importEtaPresentation(
    parsed(
      indexing({
        eta: { lowerBoundMs: 70 * 60_000, upperBoundMs: 110 * 60_000 },
      }),
    ),
  );
  assert.equal(hours.text, "Estimated 1–2 hours remaining");
});

test("a previously emitted ETA hides when monotonic live activity age crosses the stall limit", () => {
  const emitted = parsed(
    indexing({
      lastActivityAgeMs: 5_000,
      eta: { lowerBoundMs: 7 * 60_000, upperBoundMs: 9 * 60_000 },
    }),
  );

  assert.equal(importEtaPresentation(emitted, 30_000).kind, "estimate");
  assert.deepEqual(importEtaPresentation(emitted, 30_001), { kind: "hidden", text: null });
  assert.equal(emitted.lastActivityAgeMs, 5_000);
  assert.match(
    appSource,
    /importEtaPresentation\(progress, progressLiveActivityAgeMs\)/,
  );
});

test("ETA disappears when confidence, activity, or stage eligibility is insufficient", () => {
  assert.deepEqual(importEtaPresentation(parsed(indexing({ eta: null }))), {
    kind: "hidden",
    text: null,
  });
  assert.deepEqual(
    importEtaPresentation(
      parsed(
        indexing({
          eta: null,
          processState: "running_no_recent_signal",
          warningCodes: ["no_recent_activity_signal"],
        }),
      ),
    ),
    { kind: "hidden", text: null },
  );
  assert.deepEqual(importEtaPresentation(parsed(snapshot())), { kind: "hidden", text: null });
  assert.equal(
    parseImportProgressSnapshot(
      indexing({ eta: { lowerBoundMs: 0, upperBoundMs: 60_000 } }),
    ).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(
      indexing({ eta: { lowerBoundMs: 120_000, upperBoundMs: 60_000 } }),
    ).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(
      indexing({ eta: { lowerBoundMs: 60_000, upperBoundMs: Number.POSITIVE_INFINITY } }),
    ).ok,
    false,
  );
  assert.equal(
    parseImportProgressSnapshot(
      indexing({ eta: { lowerBoundMs: 60_000, upperBoundMs: 7 * 24 * 60 * 60_000 + 1 } }),
    ).ok,
    false,
  );
});

test("active stage explanations are privacy-safe and stage-specific", () => {
  assert.match(importStageExplanation(parsed(snapshot({ stage: "preflight", completedStages: [], statusCode: "preflight_running" }))), /source identity/);
  assert.match(importStageExplanation(parsed(snapshot())), /ReadPST is running/);
  assert.match(
    importStageExplanation(
      parsed(
        snapshot({
          operationKind: "resumed_import",
          stage: "discovering",
          completedStages: ["preflight", "extracting"],
          processState: "not_applicable",
          statusCode: "discovering_candidates",
        }),
      ),
    ),
    /Rechecking extracted candidate records/,
  );
  assert.match(importStageExplanation(parsed(indexing())), /canonical workspace messages/);
  assert.doesNotMatch(importStageExplanation(parsed(indexing())), /\/Users\/|subject|address/i);
});

test("completion, cancellation, incomplete, and failure have qualified terminal wording", () => {
  const completed = terminalImportPresentation(parsed(complete()));
  assert.equal(
    completed.detail,
    "Import completed in 18m 32s and passed internal verification.",
  );

  const cases = [
    ["cancelled", "import_cancelled", /cancelled after/],
    ["incomplete", "import_incomplete", /did not pass internal verification/],
    ["failed", "import_failed", /did not pass internal verification/],
  ];
  for (const [status, statusCode, expected] of cases) {
    const terminal = terminalImportPresentation(
      parsed(indexing({ status, statusCode, determinate: false, totalUnits: null, eta: null })),
    );
    assert.equal(terminal.kind, status);
    assert.match(terminal.detail, expected);
    assert.doesNotMatch(terminal.detail, /Everything in Outlook|Exchange parity/i);
  }

  assert.equal(
    terminalImportPresentation(
      parsed(complete({ operationKind: "verification", operationElapsedMs: 60_000 })),
    ).detail,
    "Verification completed in 1m 00s and passed internal workspace checks.",
  );
  assert.equal(
    terminalImportPresentation(parsed(complete({ operationKind: "repair" }))).headline,
    "Repair completed for Fictional Archive.pst.",
  );
});

test("legacy timing terminal copy does not present partial import timing as full duration", () => {
  const warningCodes = ["prior_timing_unavailable"];
  const completed = parsed(
    complete({ operationKind: "resumed_import", warningCodes }),
  );
  const completedPresentation = terminalImportPresentation(completed);
  assert.equal(
    completedPresentation.detail,
    "Import completed and passed internal verification; earlier active timing was not recorded.",
  );
  assert.doesNotMatch(completedPresentation.detail, /completed in|18m 32s/i);
  assert.match(
    materialImportAnnouncementText(completed),
    /earlier active timing was not recorded/i,
  );

  const interruptedCases = [
    ["cancelled", "import_cancelled", /completed stages and counts were preserved/],
    ["incomplete", "import_incomplete", /did not pass internal verification/],
    ["failed", "import_failed", /did not pass internal verification/],
  ];
  for (const [status, statusCode, expected] of interruptedCases) {
    const terminal = terminalImportPresentation(
      parsed(
        indexing({
          operationKind: "resumed_import",
          status,
          statusCode,
          determinate: false,
          totalUnits: null,
          eta: null,
          warningCodes,
        }),
      ),
    );
    assert.match(terminal.detail, /Recorded active time: 2m 01s/);
    assert.doesNotMatch(terminal.detail, /this session/i);
    assert.match(terminal.detail, /Earlier active timing was not recorded/);
    assert.match(terminal.detail, expected);
    assert.doesNotMatch(terminal.detail, /(?:cancelled|incomplete|failed) after/i);
  }

  for (const operationKind of ["repair", "verification"]) {
    const operation = parsed(
      complete({
        operationKind,
        operationElapsedMs: 60_000,
        warningCodes,
      }),
    );
    const terminal = terminalImportPresentation(operation);
    assert.match(terminal.detail, /completed in 1m 00s/);
    assert.match(terminal.detail, /Earlier active import timing was not recorded/);
    assert.match(
      materialImportAnnouncementText(operation),
      /Earlier active import timing was not recorded/,
    );
  }
});

test("operation headlines identify new, resumed, repair, verification, and background work", () => {
  assert.equal(importHeadline(parsed(snapshot()), false), "Importing Fictional Archive.pst");
  assert.equal(
    importHeadline(parsed(snapshot({ operationKind: "resumed_import" })), true),
    "Importing Fictional Archive.pst in the background.",
  );
  assert.equal(
    importHeadline(parsed(snapshot({ operationKind: "repair" })), true),
    "Repairing Fictional Archive.pst in the background.",
  );
  assert.equal(
    importHeadline(parsed(snapshot({ operationKind: "verification" })), false),
    "Verifying Fictional Archive.pst",
  );
});

test("announcement keys ignore timer, counter, activity, ETA, and revision-only ticks", () => {
  const before = parsed(indexing());
  const after = parsed(
    indexing({
      stateRevision: 5,
      eventSequence: 2,
      processedUnits: 41,
      counters: { ...before.counters, candidatesProcessed: 41 },
      activeElapsedMs: before.activeElapsedMs + 1_000,
      operationElapsedMs: before.operationElapsedMs + 1_000,
      stageElapsedMs: before.stageElapsedMs + 1_000,
      lastActivityUtc: "2026-08-23T16:00:01.000Z",
      lastActivityAgeMs: 1_000,
      eta: { lowerBoundMs: 7 * 60_000, upperBoundMs: 9 * 60_000 },
    }),
  );
  assert.equal(materialImportAnnouncementKey(before), materialImportAnnouncementKey(after));
  assert.equal(materialImportAnnouncementText(before), "Indexing messages started for Fictional Archive.pst.");

  const cancellation = parsed(
    indexing({
      stateRevision: 6,
      processState: "cancellation_requested",
      statusCode: "cancellation_requested",
    }),
  );
  assert.notEqual(
    materialImportAnnouncementKey(before),
    materialImportAnnouncementKey(cancellation),
  );
  assert.match(materialImportAnnouncementText(cancellation), /Cancellation requested/);

  const unavailable = parsed(
    indexing({
      stateRevision: 7,
      warningCodes: ["source_unavailable"],
    }),
  );
  assert.match(materialImportAnnouncementText(unavailable), /Source PST unavailable/);
  assert.match(materialImportAnnouncementText(unavailable), /Fictional Archive\.pst/);
  assert.match(materialImportAnnouncementText(parsed(complete())), /Fictional Archive\.pst/);
  assert.match(
    materialImportAnnouncementText(
      parsed(indexing({ stateRevision: 8, processState: "exited_failure" })),
    ),
    /exited with an error/,
  );
  assert.match(
    materialImportAnnouncementText(
      parsed(indexing({ stateRevision: 9, processState: "stalled_evidence" })),
    ),
    /bounded health checks/,
  );

  const priorTimingUnavailable = parsed(
    indexing({
      stateRevision: 10,
      warningCodes: ["prior_timing_unavailable"],
    }),
  );
  assert.notEqual(
    materialImportAnnouncementKey(before),
    materialImportAnnouncementKey(priorTimingUnavailable),
  );
  assert.match(
    materialImportAnnouncementText(priorTimingUnavailable),
    /Only active time recorded since the workspace/,
  );
  assert.match(materialImportAnnouncementText(priorTimingUnavailable), /Fictional Archive\.pst/);
  assert.match(
    appSource,
    /case "prior_timing_unavailable":\s*return "Only active time recorded since this workspace entered Phase 3 is available\."/,
  );
});

test("all generated presentation text remains aggregate-only and path-free", () => {
  const value = parsed(indexing());
  const presentation = {
    identity: importIdentityPresentation(value),
    headline: importHeadline(value, true),
    counters: importCounterRows(value),
    progress: importProgressSemantics(value),
    activity: lastActivityText(value),
    health: processHealthText(value),
    eta: importEtaPresentation(value),
    announcement: materialImportAnnouncementText(value),
  };
  const serialized = JSON.stringify(presentation);
  assert.doesNotMatch(serialized, /\/Users\/|C:\\|subject|address|attachment name|search term/i);
  assert.doesNotMatch(serialized, /Everything in Outlook|Exchange parity/i);
});

test("App accepts progress only through strict parsing, expected identity, and monotonic reduction", () => {
  assert.match(appSource, /listen<unknown>\("import-progress"/);
  assert.match(appSource, /parseImportProgressSnapshot\(event\.payload\)/);
  assert.match(
    appSource,
    /operation\.progressOperationId !== incoming\.operationId/,
  );
  assert.match(appSource, /reduceImportProgress\(operation\.progress, incoming/);
  assert.match(appSource, /acceptedWorkspaceIds: Object\.keys\(current\)/);
  assert.doesNotMatch(appSource, /payload\.workspaceId \?\? operationWorkspaceIdRef/);
  assert.doesNotMatch(appSource, /progressEventsEnabledRef/);
});

test("invoke completion preserves the typed progress slot for a late terminal event", () => {
  assert.match(
    appSource,
    /const preserveProgressSlot =\s*operation\.progressOperationId !== null &&\s*\(operation\.progress === null \|\| operation\.progress\.status === "active"\)/,
  );
  assert.match(
    appSource,
    /progressOperationId: preserveProgressSlot \? operation\.progressOperationId : null/,
  );
  assert.match(
    appSource,
    /state\.progressOperationId === operation\.operationId &&\s*\(state\.progress === null \|\| state\.progress\.status === "active"\)/,
  );
});

test("the newest background terminal result remains visible across tab selection", () => {
  assert.match(appSource, /const latestTerminalProgress = progressSnapshots/);
  assert.match(appSource, /candidateReceived > latestReceived \? candidate : latest/);
  assert.match(
    appSource,
    /activeProgress \?\?\s*latestTerminalProgress \?\?\s*selectedWorkspaceProgress/,
  );
});

test("dismiss timers are generation-scoped and cannot erase a new active operation", () => {
  assert.match(appSource, /workspaceOperationDismissGenerationsRef/);
  assert.match(
    appSource,
    /workspaceOperationDismissGenerationsRef\.current\.get\(workspaceId\) !== dismissGeneration/,
  );
  assert.match(
    appSource,
    /state\?\.running === true \|\| state\?\.progress\?\.status === "active"/,
  );
  assert.match(
    appSource,
    /const targetWorkspaceId = existingWorkspace\?\.workspaceId \?\? plan\.workspaceId;\s*clearWorkspaceOperationDismissTimer\(targetWorkspaceId\)/,
  );
  assert.match(
    appSource,
    /const operationId = createProgressOperationId\("reindex"\);\s*clearWorkspaceOperationDismissTimer\(targetWorkspaceId\)/,
  );
});

test("active workspace operations cannot be hidden through the tab-close handler", () => {
  const closeStart = appSource.indexOf("async function closeWorkspaceTab");
  const closeEnd = appSource.indexOf("async function loadMessagesPage", closeStart);
  assert.notEqual(closeStart, -1);
  assert.notEqual(closeEnd, -1);
  const closeRegion = appSource.slice(closeStart, closeEnd);
  assert.match(
    closeRegion,
    /if \(workspaceOperationStates\[workspaceId\]\?\.running === true \|\| isImporting\) \{\s*setError\("Cancel or finish the active workspace operation before closing its tab\."\);\s*return;/,
  );
  assert.doesNotMatch(closeRegion, /integrityOperationTokensRef\.current\.delete\(workspaceId\)/);
});

test("all proven committed import changes ignore late cancellation", () => {
  const importStart = rustSource.indexOf("fn import_workspace_from_pst");
  const resumeFlowStart = rustSource.indexOf("fn resume_index_existing_workspace");
  const repairFlowStart = rustSource.indexOf("fn reindex_existing_emls_blocking");
  const conversationsStart = rustSource.indexOf("fn rebuild_workspace_conversations_blocking");
  const repairStart = rustSource.indexOf("fn recover_interrupted_repair_layout");
  const resumeStart = rustSource.indexOf("fn recover_interrupted_resume_layout");
  const resumeEnd = rustSource.indexOf("fn reindex_existing_emls_blocking", resumeStart);
  assert.notEqual(importStart, -1);
  assert.notEqual(resumeFlowStart, -1);
  assert.notEqual(repairFlowStart, -1);
  assert.notEqual(conversationsStart, -1);
  assert.notEqual(repairStart, -1);
  assert.notEqual(resumeStart, -1);
  assert.notEqual(resumeEnd, -1);
  const importRegion = rustSource.slice(importStart, resumeFlowStart);
  const resumeFlowRegion = rustSource.slice(resumeFlowStart, repairStart);
  const repairFlowRegion = rustSource.slice(repairFlowStart, conversationsStart);
  const conversationsRegion = rustSource.slice(
    conversationsStart,
    rustSource.indexOf("fn read_stable_message_ids", conversationsStart),
  );
  const repairRegion = rustSource.slice(repairStart, resumeStart);
  const resumeRegion = rustSource.slice(resumeStart, resumeEnd);
  assert.match(importRegion, /verify_active_workspace_after_commit/);
  assert.match(resumeFlowRegion, /verify_active_workspace_after_commit/);
  assert.match(
    resumeFlowRegion,
    /else \{[\s\S]*?verify_replacement_index_with_operation_progress\([\s\S]*?false,[\s\S]*?\)\?;[\s\S]*?verify_active_workspace_after_commit/,
  );
  assert.match(repairFlowRegion, /verify_active_workspace_after_commit/);
  assert.match(conversationsRegion, /verify_active_workspace_after_commit/);
  assert.match(
    repairRegion,
    /if active_matches_staged \{[\s\S]*?verify_active_workspace_after_commit/,
  );
  assert.match(
    resumeRegion,
    /if active_matches_staged \{[\s\S]*?verify_active_workspace_after_commit/,
  );
  assert.match(
    resumeRegion,
    /Some\("complete"\) if active_matches_current => \{[\s\S]*?verify_active_workspace_after_commit/,
  );
});

test("backend lifecycle gates current verification, recovery evidence, and operation generation", () => {
  const verifyStart = rustSource.indexOf("async fn verify_workspace_import");
  const verifyEnd = rustSource.indexOf("fn get_last_workspace_verification", verifyStart);
  const verifyRegion = rustSource.slice(verifyStart, verifyEnd);
  assert.match(verifyRegion, /terminalize_standalone_verification_error/);
  assert.match(
    verifyRegion,
    /report\.workspace_state !=[\s\S]*?ProgressStatus::Incomplete/,
  );
  assert.match(verifyRegion, /Ok\(report\)/);
  assert.doesNotMatch(verifyRegion, /get_last_verification/);

  const repairStart = rustSource.indexOf("fn reindex_existing_emls_blocking");
  const repairEnd = rustSource.indexOf("fn rebuild_workspace_conversations_blocking", repairStart);
  const repairRegion = rustSource.slice(repairStart, repairEnd);
  assert.ok(
    repairRegion.indexOf("ensure_state_for_workspace") <
      repairRegion.indexOf("recover_phase2_workspace_state"),
  );
  assert.doesNotMatch(repairRegion, /legacy_repair_adoption/);
  assert.ok(
    repairRegion.indexOf("recover_interrupted_repair_layout") <
      repairRegion.indexOf("WorkspaceState::Discovering"),
  );
  assert.ok(
    repairRegion.indexOf("ensure_manifest_for_existing_extraction") <
      repairRegion.indexOf("import_integrity_phase2::WorkspaceState::Discovering"),
  );

  const importStart = rustSource.indexOf("fn import_workspace_from_pst");
  const importEnd = rustSource.indexOf("fn resume_index_existing_workspace", importStart);
  const importRegion = rustSource.slice(importStart, importEnd);
  assert.match(
    importRegion,
    /if force_reimport \{\s*recover_phase2_workspace_state\(&progress_active\)\?;\s*import_integrity_phase2::restart_import_state/,
  );
  assert.ok(
    importRegion.indexOf("restart_import_state") <
      importRegion.indexOf("mark_workspace_status_file_only"),
  );
  assert.ok(
    (rustSource.match(/operation_generation != scope\.generation/g) ?? []).length >= 3,
  );
});

test("recovery stage revisions and terminal lineage remain reducer-safe", () => {
  const prepareStart = rustSource.indexOf("fn prepare_recovery_verification_progress");
  const prepareEnd = rustSource.indexOf("fn emit_progress_observation", prepareStart);
  const prepareRegion = rustSource.slice(prepareStart, prepareEnd);
  assert.match(
    prepareRegion,
    /transition_state\([\s\S]*?WorkspaceState::Verifying[\s\S]*?recovery_verification_running/,
  );
  assert.ok(
    prepareRegion.indexOf("transition_state(") < prepareRegion.indexOf("scope.stage ="),
  );
  assert.match(prepareRegion, /scope\.state_revision = verifying\.revision/);

  const verifyStart = rustSource.indexOf("fn terminalize_standalone_verification_error");
  const verifyEnd = rustSource.indexOf("fn get_last_workspace_verification", verifyStart);
  const verifyRegion = rustSource.slice(verifyStart, verifyEnd);
  assert.match(verifyRegion, /state_has_complete_lineage\(verification_start_state\)/);

  const finishStart = rustSource.indexOf("fn finish_integrity_operation_scope");
  const finishEnd = rustSource.indexOf("#[cfg(test)]\nmod tests", finishStart);
  const finishRegion = rustSource.slice(finishStart, finishEnd);
  assert.match(finishRegion, /restored_prior_complete_after_interrupted_verification/);
  assert.match(
    finishRegion,
    /retain\(\|stage\| stage\.rank\(\) < import_progress::ProgressStage::Verifying\.rank\(\)\)/,
  );
  assert.match(finishRegion, /should_record_import_completion/);
  assert.match(finishRegion, /terminal_process_state\(scope\.stage, scope\.process_state\)/);
});

test("durable completion timing is captured after metadata and ignores late cancellation", () => {
  const wrapperStart = phase2RustSource.indexOf("fn verify_active_workspace_with_policy");
  const coreStart = phase2RustSource.indexOf("fn verify_active_workspace_core", wrapperStart);
  const wrapperRegion = phase2RustSource.slice(wrapperStart, coreStart);
  assert.match(
    wrapperRegion,
    /checkpoint_integrity_operation_with_policy\(app, true, false, true\)/,
  );

  const coreEnd = phase2RustSource.indexOf("fn source_availability_for_snapshot", coreStart);
  const coreRegion = phase2RustSource.slice(coreStart, coreEnd);
  const metadataWrite = coreRegion.indexOf("mark_existing_workspace_complete_file_only(");
  const durationBoundary = coreRegion.indexOf("on_completion_boundary()?;");
  const completeTransition = coreRegion.indexOf("WorkspaceState::Complete", durationBoundary);
  assert.ok(metadataWrite >= 0 && metadataWrite < durationBoundary);
  assert.ok(durationBoundary < completeTransition);
});

test("verification and terminal events do not relabel historical candidate work", () => {
  const verifyProgressStart = rustSource.indexOf("fn emit_verification_progress");
  const verifyProgressEnd = rustSource.indexOf("enum InsertMessageOutcome", verifyProgressStart);
  const verifyProgressRegion = rustSource.slice(verifyProgressStart, verifyProgressEnd);
  assert.doesNotMatch(verifyProgressRegion, /inserted_messages:/);
  assert.doesNotMatch(verifyProgressRegion, /skipped_existing_messages:/);
  assert.doesNotMatch(verifyProgressRegion, /parse_failures:/);

  assert.doesNotMatch(rustSource, /"Import and indexing complete and internally verified\."/);
  assert.doesNotMatch(rustSource, /"Resume indexing complete and internally verified\."/);
  assert.doesNotMatch(rustSource, /"Verification limited"/);
  assert.doesNotMatch(
    rustSource,
    /emit_progress\([\s\S]{0,240}Some\(stats\.processed\)[\s\S]{0,120}Some\(stats\.discovered\)/,
  );
});

test("App supplies opaque workspace and operation identity to import and reindex commands", () => {
  assert.match(typeSource, /export type PstOpenPlan = \{[\s\S]*?workspaceId: string;/);
  assert.match(
    appSource,
    /const targetWorkspaceId = existingWorkspace\?\.workspaceId \?\? plan\.workspaceId;/,
  );
  assert.match(
    appSource,
    /invoke<WorkspaceSummary>\("open_pst", \{[\s\S]*?operationId,/,
  );
  assert.match(
    appSource,
    /invoke<WorkspaceSummary>\("reindex_existing_emls", \{[\s\S]*?workspaceId: targetWorkspaceId,[\s\S]*?operationId,/,
  );
  assert.doesNotMatch(appSource, /existingWorkspace\?\.workspaceId \?\? plan\.fingerprint/);
});

test("cancellation is scoped to the exact active snapshot", () => {
  assert.match(
    appSource,
    /invoke<CancelImportResult>\("cancel_import", \{[\s\S]*?workspaceId: snapshot\.workspaceId,[\s\S]*?operationId: snapshot\.operationId,[\s\S]*?generation: snapshot\.generation,/,
  );
  assert.match(appSource, /currentSnapshot\.status !== "active"/);
  assert.match(appSource, /currentSnapshot\.stateRevision !== snapshot\.stateRevision/);
  assert.match(appSource, /currentSnapshot\.eventSequence !== snapshot\.eventSequence/);
  assert.match(appSource, /cancelImport\(progress\)/);
  assert.match(appSource, /cancelImport\(activeIntegrityProgress\)/);
  assert.match(appSource, /disabled=\{activeIntegrityProgress == null\}/);
});

test("the old generic midway bar and fake initial progress are removed from App", () => {
  assert.doesNotMatch(appSource, /progressPercent|progressCount|progress-strip/);
  assert.doesNotMatch(appSource, /width: `\$\{[^}]*\?\?[^}]*35/);
  assert.doesNotMatch(appSource, /stage: "Checking PST"|stage: "Scanning extracted EML files"/);
  assert.doesNotMatch(appSource, /Workspace import completed\./);
});

test("live presentation renders a stage tracker and a determinate-only stage bar", () => {
  assert.match(appSource, /className="import-stage-tracker" aria-label="Import stages"/);
  assert.match(appSource, /progressSemantics\?\.kind === "determinate"/);
  assert.match(appSource, /role="progressbar"/);
  assert.match(appSource, /aria-valuemin=\{progressSemantics\.ariaValueMin\}/);
  assert.match(appSource, /aria-valuemax=\{progressSemantics\.ariaValueMax\}/);
  assert.match(appSource, /aria-valuenow=\{progressSemantics\.ariaValueNow\}/);
  assert.match(appSource, /progressSemantics\?\.kind === "indeterminate"/);
  assert.equal("overallPercentage" in importProgressSemantics(parsed(indexing())), false);
});

test("progress header and compact background mode use basename-only identity", () => {
  const start = appSource.indexOf("className={`import-progress-panel");
  const end = appSource.indexOf(
    "{workspace && activeWorkspaceOperationRunning",
    start,
  );
  assert.notEqual(start, -1);
  assert.notEqual(end, -1);
  const region = appSource.slice(start, end);
  assert.match(region, /importHeadline\(progress, progressIsBackground, progressIdentityPeers\)/);
  assert.match(region, /progressIdentity\.sourceSize/);
  assert.match(region, /progressIdentity\.storageMode/);
  assert.match(region, /progressIdentity\.operation/);
  assert.match(region, /import-progress-background/);
  assert.doesNotMatch(region, /\.pstPath|\.workspacePath|message\.subject|sender|recipient|search/i);
});

test("elapsed and activity ticks use a monotonic UI clock outside the live region", () => {
  assert.match(appSource, /performance\.now\(\)/);
  assert.match(appSource, /\["new_import", "resumed_import"\]\.includes\(progress\.operationKind\)/);
  assert.match(appSource, /progress\.operationElapsedMs \+ progressLiveDelta/);
  assert.match(appSource, /progress\.stageElapsedMs \+ progressLiveDelta/);
  assert.match(
    appSource,
    /progress\?\.operationKind === "repair" \|\| progress\?\.operationKind === "verification"/,
  );
  assert.match(appSource, /progress\.lastActivityAgeMs \+ progressLiveDelta/);
  assert.match(appSource, /key=\{progressAnnouncementKey\}/);
  assert.match(appSource, /aria-live="polite"/);
  assert.doesNotMatch(
    appSource,
    /className="import-progress-timing"[^>]*aria-live/,
  );
});

test("workspace and verification details retain completed import timing history", () => {
  for (const field of [
    "pstSizeBytes: number",
    "completedImportDurationMs: number | null",
    "lastActivityAt: string | null",
    "activeImportElapsedMs: number",
    "currentStageElapsedMs: number",
    "stageElapsedDurationMs: number",
  ]) {
    assert.match(typeSource, new RegExp(field.replace(/[|]/g, "\\|")));
  }
  assert.match(appSource, /Completed import duration/);
  assert.match(appSource, /Interrupted stage elapsed/);
  assert.match(appSource, /Last import activity/);
});

test("progress layout supports theme variables, reduced motion, and the 980px boundary", () => {
  assert.match(styleSource, /\.import-progress-panel/);
  assert.match(styleSource, /var\(--color-progress-track\)/);
  assert.match(styleSource, /var\(--color-progress-fill\)/);
  assert.match(styleSource, /@media \(prefers-reduced-motion: reduce\)/);
  assert.match(styleSource, /@media \(max-width: 980px\)[\s\S]*?\.import-stage-tracker/);
  assert.doesNotMatch(styleSource, /\.progress-strip|\.progress-fill/);
});

test("Help explains truthful counts, elapsed timing, qualified ETA, and source safety", () => {
  assert.match(appSource, /Extraction is indeterminate because ReadPST does not provide a trustworthy total/);
  assert.match(appSource, /Candidate[\s\S]*canonical messages are durable indexed records/);
  assert.match(appSource, /Import elapsed time accumulates only active app sessions across resumes/);
  assert.match(appSource, /older workspace without trusted timing history/);
  assert.match(appSource, /only active time recorded[\s\S]*since the workspace entered Phase 3 is available/);
  assert.doesNotMatch(appSource, /elapsed time starts with this session/i);
  assert.match(
    appSource,
    /Repair and[\s\S]*verification timing is kept separate from completed import history[\s\S]*interrupted same-kind retry/,
  );
  assert.match(appSource, /ETA appears only for stable determinate work/);
  assert.match(appSource, /Neither action writes to the source PST/);
});
