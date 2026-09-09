import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  createWorkspaceIntegrityOperationToken,
  formatElapsedDuration,
  integrityClassificationForState,
  integrityOperationIsCancellable,
  isSupportedWorkspaceIntegrityState,
  isVerificationReportForOperation,
  isWorkspaceIntegrityOperationCurrent,
  possibleDuplicateSummary,
  resumeProgressDescription,
  verificationHeadline,
  verificationLevelLabel,
  workspaceIntegrityActions,
  workspaceIntegrityStateLabel,
  workspaceSourceAvailabilityMessage,
  workspaceSummaryWithVerification,
} from "../src/workspaceIntegrity.ts";

const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const styles = readFileSync(new URL("../src/styles.css", import.meta.url), "utf8");
const types = readFileSync(new URL("../src/types.ts", import.meta.url), "utf8");

const supportedStates = [
  "preflight",
  "extracting",
  "extraction_complete",
  "discovering",
  "indexing",
  "building_conversations",
  "verifying",
  "complete",
  "cancelled",
  "incomplete",
  "failed",
];

function workspace(overrides = {}) {
  return {
    id: "workspace-a",
    pstPath: "/private/source/example.pst",
    workspacePath: "/private/workspace/cache",
    indexPath: "/private/workspace/cache/index.sqlite",
    extractedPath: "/private/workspace/cache/extracted",
    reusedExisting: true,
    messageCount: 10,
    attachmentCount: 2,
    folderCount: 3,
    fingerprint: "opaque-workspace-fingerprint",
    fingerprintStrategy: "workspace-relative-eml-v1",
    workspaceLocationMode: "app_support",
    workspaceLocationLabel: "App Support",
    integrityState: "incomplete",
    integrityClassification: "incomplete",
    sourceAvailability: "available",
    canResumeIndexing: true,
    canRepairIndex: true,
    canRebuildConversations: true,
    resumeDetails: null,
    hasLastVerification: false,
    conversationState: "incomplete",
    ...overrides,
  };
}

function report(overrides = {}) {
  return {
    operationId: "integrity-1",
    workspaceId: "workspace-a",
    verificationLevel: "quick",
    result: "verified",
    workspaceState: "complete",
    discoveredCandidates: 10,
    canonicalMessages: 10,
    insertedMessages: 10,
    skippedExistingMessages: 0,
    parseFailures: 0,
    attachments: 2,
    folders: 3,
    ftsRows: 10,
    conversationState: "complete",
    missingCanonicalRecords: 0,
    unexpectedCanonicalRecords: 0,
    possibleDuplicateGroups: 0,
    possibleDuplicateGroupDetails: [],
    sourceIntegrity: "passed",
    startedAt: "2026-08-22T12:00:00.000Z",
    completedAt: "2026-08-22T12:00:01.000Z",
    elapsedDurationMs: 1_000,
    warnings: [],
    recommendedActions: [],
    ...overrides,
  };
}

test("workspace integrity states classify complete narrowly and future states fail closed", () => {
  for (const state of supportedStates) {
    assert.equal(isSupportedWorkspaceIntegrityState(state), true);
    assert.equal(
      integrityClassificationForState(state),
      state === "complete" ? "complete" : "incomplete",
    );
  }

  assert.equal(isSupportedWorkspaceIntegrityState("future_phase"), false);
  assert.equal(integrityClassificationForState("future_phase"), "unsupported");

  const actions = workspaceIntegrityActions(
    workspace({
      integrityClassification: "unsupported",
      canResumeIndexing: true,
      canRepairIndex: true,
      canRebuildConversations: true,
      hasLastVerification: true,
    }),
  );
  assert.deepEqual(actions, ["verify", "view_last_verification"]);
  assert.doesNotMatch(actions.join(" "), /resume|repair|rebuild/);
});

test("complete and incomplete workspaces expose only their intended actions", () => {
  assert.deepEqual(
    workspaceIntegrityActions(
      workspace({ integrityClassification: "complete", hasLastVerification: true }),
    ),
    ["verify", "view_last_verification"],
  );
  assert.deepEqual(workspaceIntegrityActions(workspace()), [
    "verify",
    "resume_indexing",
    "repair_index",
    "rebuild_conversations",
  ]);
  assert.deepEqual(
    workspaceIntegrityActions(
      workspace({
        canResumeIndexing: false,
        canRepairIndex: false,
        canRebuildConversations: true,
      }),
    ),
    ["verify", "rebuild_conversations"],
  );
});

test("resume descriptions expose structured counts without a full source path", () => {
  const details = {
    pstBasename: "fictional-mail.pst",
    lastCompletedStage: "discovering",
    interruptedStage: "indexing",
    discoveredCandidates: 79_354,
    canonicalIndexed: 34_281,
    remainingCandidates: 45_073,
    lastSuccessfulUpdate: "2026-08-22T12:00:00.000Z",
    elapsedDurationMs: 5_460_000,
    reason: "The application stopped before verification completed.",
  };
  const description = resumeProgressDescription(
    details,
    (value) => new Intl.NumberFormat("en-US").format(value),
  );
  assert.equal(
    description,
    "Indexing was interrupted after 34,281 of 79,354 discovered records.",
  );
  assert.doesNotMatch(description, /private|\/Users|workspace/i);
  assert.equal(formatElapsedDuration(details.elapsedDurationMs), "1 hour, 31 minutes");
  assert.equal(formatElapsedDuration(1_000), "Less than a minute");
  assert.equal(workspaceIntegrityStateLabel("none"), "None yet");
});

test("operation tokens and reports reject stale or cross-workspace responses", () => {
  const current = createWorkspaceIntegrityOperationToken("workspace-a", "repair_index", 3);
  const stale = createWorkspaceIntegrityOperationToken("workspace-a", "repair_index", 2);
  const otherWorkspace = createWorkspaceIntegrityOperationToken(
    "workspace-b",
    "repair_index",
    3,
  );

  assert.equal(isWorkspaceIntegrityOperationCurrent(current, current), true);
  assert.equal(isWorkspaceIntegrityOperationCurrent(current, stale), false);
  assert.equal(isWorkspaceIntegrityOperationCurrent(current, otherWorkspace), false);
  assert.equal(isVerificationReportForOperation(report({ operationId: "integrity-3" }), current), true);
  assert.equal(isVerificationReportForOperation(report(), current), false);
  assert.match(current.operationId, /^integrity-\d+$/);
  assert.doesNotMatch(JSON.stringify(current), /source|subject|address|\/Users/i);
});

test("verification summaries distinguish internal proof and source availability", () => {
  assert.equal(verificationLevelLabel("quick"), "Quick Verify");
  assert.equal(verificationLevelLabel("full"), "Full Verify");
  assert.equal(
    verificationHeadline(report()),
    "Internally verified against this workspace's extracted source records.",
  );
  assert.match(
    verificationHeadline(report({ result: "source_unavailable" })),
    /Source comparison is unavailable/,
  );
  assert.match(verificationHeadline(report({ result: "source_changed" })), /differs/);
  assert.equal(possibleDuplicateSummary(0), null);
  assert.equal(possibleDuplicateSummary(2), "Possible duplicate source records detected.");
  assert.match(workspaceSourceAvailabilityMessage("unavailable"), /workspace remains usable/);
  assert.match(workspaceSourceAvailabilityMessage("changed"), /No files or indexes were deleted/);
  assert.equal(workspaceSourceAvailabilityMessage("available"), null);
});

test("saved verification updates aggregate state and disables unsupported repairs", () => {
  const verified = workspaceSummaryWithVerification(workspace(), report());
  assert.equal(verified.integrityClassification, "complete");
  assert.equal(verified.sourceAvailability, "available");
  assert.equal(verified.hasLastVerification, true);
  assert.equal(verified.canResumeIndexing, false);
  assert.equal(verified.canRepairIndex, false);
  assert.equal(verified.canRebuildConversations, false);

  const unavailable = workspaceSummaryWithVerification(
    workspace(),
    report({
      result: "source_unavailable",
      workspaceState: "complete",
      sourceIntegrity: "unavailable",
    }),
  );
  assert.equal(unavailable.integrityClassification, "complete");
  assert.equal(unavailable.sourceAvailability, "unavailable");

  const unsupported = workspaceSummaryWithVerification(
    workspace(),
    report({ result: "unsupported", workspaceState: "incomplete" }),
  );
  assert.equal(unsupported.integrityClassification, "unsupported");
  assert.equal(unsupported.canResumeIndexing, false);
  assert.equal(unsupported.canRepairIndex, false);
  assert.equal(unsupported.canRebuildConversations, false);

  const repairable = workspaceSummaryWithVerification(
    workspace({
      integrityClassification: "complete",
      canResumeIndexing: false,
      canRepairIndex: false,
      canRebuildConversations: false,
    }),
    report({
      result: "repairable",
      workspaceState: "incomplete",
      missingCanonicalRecords: 1,
      conversationState: "incomplete",
    }),
  );
  assert.equal(repairable.integrityClassification, "incomplete");
  assert.equal(repairable.canResumeIndexing, true);
  assert.equal(repairable.canRepairIndex, true);
  assert.equal(repairable.canRebuildConversations, true);

  const contradictory = workspaceSummaryWithVerification(
    workspace({ integrityClassification: "complete" }),
    report({ result: "database_inconsistent", workspaceState: "complete" }),
  );
  assert.equal(contradictory.integrityClassification, "incomplete");
  assert.equal(contradictory.canRepairIndex, true);
});

test("only supported mutating integrity operations are cancellable", () => {
  assert.equal(integrityOperationIsCancellable("verify"), false);
  assert.equal(integrityOperationIsCancellable("load_last_verification"), false);
  assert.equal(integrityOperationIsCancellable("resume_indexing"), true);
  assert.equal(integrityOperationIsCancellable("repair_index"), true);
  assert.equal(integrityOperationIsCancellable("rebuild_conversations"), true);
});

test("App binds the workspace-scoped verification and non-destructive repair commands", () => {
  const integrityFunctions = app.slice(
    app.indexOf("async function applyIntegrityWorkspaceSummary"),
    app.indexOf("async function reindexExistingEmls"),
  );
  for (const command of [
    "verify_workspace_import",
    "get_last_workspace_verification",
    "resume_workspace_indexing",
    "repair_workspace_index",
    "rebuild_workspace_conversations",
  ]) {
    assert.match(integrityFunctions, new RegExp(`invoke<[^>]+>\\(\"${command}\"`));
  }
  assert.match(integrityFunctions, /workspaceId,/);
  assert.match(integrityFunctions, /operationId: operation\.operationId/);
  assert.match(integrityFunctions, /verificationLevel: "quick"/);
  assert.doesNotMatch(integrityFunctions, /activate_workspace|setIsBusy|setIsImporting/);
  assert.match(app, /invoke<CancelImportResult>\("cancel_import",\s*\{\s*workspaceId:\s*snapshot\.workspaceId,\s*operationId:\s*snapshot\.operationId,\s*generation:\s*snapshot\.generation,?\s*\}\s*\)/s);
});

test("App guards response and progress mutations with workspace operation identity", () => {
  const savedReportViewer = app.slice(
    app.indexOf("async function viewLastWorkspaceVerification"),
    app.indexOf("async function resumeWorkspaceIndexing"),
  );
  assert.ok((app.match(/workspaceIntegrityOperationIsCurrent\(operation\)/g) ?? []).length >= 9);
  assert.match(app, /event\.payload\.operationId !== integrityOperation\.operationId/);
  assert.match(app, /integrityOperationTokensRef\.current\.delete\(workspaceId\)/);
  assert.match(
    app,
    /if \(!workspaceIntegrityOperationIsCurrent\(operation\)\) return false;[\s\S]*await loadWorkspaceSession[\s\S]*if \(!workspaceIntegrityOperationIsCurrent\(operation\)\) return false;/,
  );
  assert.match(app, /if \(nextWorkspace\.id !== operation\.workspaceId\)/);
  assert.match(app, /if \(nextSession\.workspace\.id !== operation\.workspaceId\)/);
  assert.match(app, /if \(activeWorkspaceIdRef\.current === operation\.workspaceId\)/);
  assert.match(app, /if \(integrityOperationTokensRef\.current\.size > 0\) return null/);
  assert.ok((app.match(/if \(!operation\) return;/g) ?? []).length >= 5);
  assert.match(
    savedReportViewer,
    /const inMemoryReport = workspaceOperationStates\[workspaceId\]\?\.verificationReport \?\? null/,
  );
  assert.match(
    savedReportViewer,
    /if \(inMemoryReport\)[\s\S]*inMemoryReport\.workspaceId !== workspaceId[\s\S]*showVerificationReport\(workspaceId, inMemoryReport, returnFocus\)[\s\S]*return;/,
  );
  assert.ok(
    savedReportViewer.indexOf("if (inMemoryReport)") <
      savedReportViewer.indexOf('invoke<VerificationReport | null>("get_last_workspace_verification"'),
  );
  const integrityOperationStart = app.slice(
    app.indexOf("function beginWorkspaceIntegrityOperation"),
    app.indexOf("function workspaceIntegrityOperationIsCurrent"),
  );
  assert.match(
    integrityOperationStart,
    /kind === "load_last_verification" \? \{\} : \{ verificationReport: null \}/,
  );
  assert.match(
    app.slice(app.indexOf("async function openPlannedPst"), app.indexOf("function requestPlannedWorkspaceRepair")),
    /expectsProgress[\s\S]*verificationReport: null, verificationReportOpen: false/,
  );
  assert.doesNotMatch(savedReportViewer, /updateWorkspaceSummaryFromVerification/);
});

test("integrity UI separates complete and incomplete actions and stays operation-specific", () => {
  assert.match(app, /workspace-integrity-banner/);
  assert.match(app, /workspace\.integrityClassification === "complete"[\s\S]*"Verify Import"/);
  assert.match(app, /"View Last Verification"/);
  assert.match(app, /"Resume Indexing"/);
  assert.match(app, /"Repair Index"/);
  assert.match(app, /"Rebuild Conversations"/);
  assert.match(app, /aria-busy=\{activeIntegrityOperationRunning\}/);
  assert.match(app, /integrityOperationRunningLabel\(activeIntegrityOperationKind\)/);
  assert.match(app, /Cancel \{integrityOperationLabel\(activeIntegrityOperationKind\)\}/);
  assert.match(app, /!visibleIntegrityOperationKind/);
  assert.match(
    app,
    /existing\.canRepairIndex[\s\S]*requestPlannedWorkspaceRepair\(plan, existing, event\.currentTarget\)/,
  );
  assert.match(app, /pending\.existingWorkspace,[\s\S]*"repair_index"/);
  assert.match(app, /foregroundImportAction === "repair_index"[\s\S]*"Cancel Repair"/);
  assert.match(
    app,
    /importAction === "repair_index"[\s\S]*"Repair Index cancelled\. The previous active index remains available\."/,
  );
  const plannedOpen = app.slice(
    app.indexOf("async function openPlannedPst"),
    app.indexOf("function requestPlannedWorkspaceRepair"),
  );
  assert.match(
    plannedOpen,
    /activeWorkspaceChangeSequenceAtStart[\s\S]*activeWorkspaceChangedDuringOperation[\s\S]*onlyIfWorkspaceStillActive: activeWorkspaceChangedDuringOperation/,
  );
  assert.match(
    plannedOpen,
    /currentWorkspaceId !== nextWorkspace\.id[\s\S]*invoke<WorkspaceSummary>\("activate_workspace",[\s\S]*workspaceId: restoreWorkspaceId/,
  );
  assert.match(
    plannedOpen,
    /restoringPlannedWorkspaceRef\.current = true[\s\S]*restoreSequence[\s\S]*activeWorkspaceChangeSequenceRef\.current === restoreSequence[\s\S]*restoringPlannedWorkspaceRef\.current = false/,
  );
  assert.match(app, /if \(restoringPlannedWorkspaceRef\.current\) return;/);
  assert.match(
    app,
    /const canSwitchTabsDuringOperation =[\s\S]*!restoringPlannedWorkspace/,
  );
});

test("repair confirmation is explicit, non-destructive, and keyboard accessible", () => {
  const repairMarkup = app.slice(
    app.indexOf('className="repair-confirm-backdrop"'),
    app.indexOf("{isSearchActive ?"),
  );
  assert.match(repairMarkup, /role="dialog"/);
  assert.match(repairMarkup, /aria-modal="true"/);
  assert.match(repairMarkup, /aria-labelledby=\{repairConfirmTitleId\}/);
  assert.match(repairMarkup, /aria-describedby=\{repairConfirmDescriptionId\}/);
  assert.match(repairMarkup, /new temporary database/);
  assert.match(repairMarkup, /verifies it before replacement/);
  assert.match(repairMarkup, /does not rerun ReadPST or modify the source PST or extraction output/);
  assert.match(repairMarkup, /current index remains active unless the verified replacement succeeds/i);
  assert.match(app, /event\.key === "Escape"/);
  assert.match(app, /repairConfirmCancelRef\.current\?\.focus\(\)/);
  const repairCloseHelpers = app.slice(
    app.indexOf("function closeRepairWorkspaceConfirmation"),
    app.indexOf("function confirmRepairWorkspaceIndex"),
  );
  assert.equal((repairCloseHelpers.match(/returnFocus\?\.isConnected/g) ?? []).length, 2);
  assert.equal((repairCloseHelpers.match(/returnFocus\.focus\(\)/g) ?? []).length, 2);
});

test("verification report is aggregate and duplicate details remain opaque", () => {
  const reportMarkup = app.slice(
    app.indexOf('className="verification-report"'),
    app.indexOf("{(workspace && repairConfirmOpen) || visiblePendingPlannedRepair ?"),
  );
  for (const label of [
    "Canonical messages",
    "Missing canonical records",
    "Unexpected canonical records",
    "Possible duplicate groups",
    "Source integrity",
  ]) {
    assert.match(reportMarkup, new RegExp(label));
  }
  assert.match(reportMarkup, /<details className="verification-record-details">/);
  assert.match(reportMarkup, /Opaque diagnostic details/);
  assert.match(reportMarkup, /do not expose message content or record paths/);
  assert.doesNotMatch(reportMarkup, /relativeWorkspaceIdentities|Relative workspace identities/);
  assert.doesNotMatch(
    reportMarkup,
    /activeVerificationReport\.(?:pstPath|workspacePath|subject|body|sender|recipients)/,
  );
  assert.match(reportMarkup, /No messages were hidden, merged, suppressed, or deleted/);
  assert.match(reportMarkup, /verificationReportHeadingRef/);
});

test("Help and safety copy state the proof boundary and preserve fail-open visibility", () => {
  assert.match(app, /internally verified against this workspace's extracted source records/i);
  assert.match(app, /does not\s+prove parity with Exchange or Outlook/i);
  assert.match(app, /source PST remains read-only/i);
  assert.match(app, /Possible duplicate source-record warnings/);
  assert.match(app, /Records are not hidden,[\s\S]*merged, suppressed, or deleted automatically/);
  assert.match(app, /Blank, no-subject, and empty-body[\s\S]*messages remain visible/);
  assert.match(app, /suppression remains fail-open and zero/);
  assert.doesNotMatch(app, /Safe to delete the original mailbox/i);
  assert.doesNotMatch(app, /Duplicate messages deleted/i);
  assert.match(app, /\{selectedMessage\.subject \|\| "\(No subject\)"\}/);
});

test("integrity controls have restrained responsive styling and destructive separation", () => {
  for (const selector of [
    ".workspace-integrity-banner",
    ".workspace-integrity-complete",
    ".workspace-integrity-incomplete",
    ".workspace-integrity-unsupported",
    ".verification-report",
    ".repair-confirm-dialog",
    ".workspace-destructive-actions",
  ]) {
    assert.match(styles, new RegExp(selector.replaceAll(".", "\\.")));
  }
  assert.match(styles, /\.workspace-destructive-actions[\s\S]*border-top/);
  assert.match(styles, /\.verification-report[\s\S]*max-height/);
  assert.match(styles, /@media \(max-width: 760px\)[\s\S]*\.workspace-integrity-actions button/);
});

test("frontend types preserve the exact snake_case integrity wire contract", () => {
  for (const value of [
    "extraction_complete",
    "building_conversations",
    "source_unavailable",
    "source_changed",
    "extraction_inconsistent",
    "database_inconsistent",
  ]) {
    assert.match(types, new RegExp(`\"${value}\"`));
  }
  assert.match(types, /sourceIntegrity: "passed" \| "unavailable" \| "changed"/);
});
