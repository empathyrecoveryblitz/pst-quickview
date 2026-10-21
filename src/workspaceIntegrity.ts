import type {
  ResumeDetails,
  VerificationLevel,
  VerificationReport,
  WorkspaceIntegrityClassification,
  WorkspaceIntegrityState,
  WorkspaceSourceAvailability,
  WorkspaceSummary,
} from "./types";

export type WorkspaceIntegrityAction =
  | "verify"
  | "view_last_verification"
  | "resume_indexing"
  | "repair_index"
  | "rebuild_conversations";

export type WorkspaceIntegrityOperationKind = Exclude<
  WorkspaceIntegrityAction,
  "view_last_verification"
> | "load_last_verification";

export type WorkspaceIntegrityOperationToken = {
  workspaceId: string;
  operationId: string;
  kind: WorkspaceIntegrityOperationKind;
};

const supportedStates = new Set<WorkspaceIntegrityState>([
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
]);

export function isSupportedWorkspaceIntegrityState(value: string): value is WorkspaceIntegrityState {
  return supportedStates.has(value as WorkspaceIntegrityState);
}

export function integrityClassificationForState(
  state: string,
): WorkspaceIntegrityClassification {
  if (!isSupportedWorkspaceIntegrityState(state)) return "unsupported";
  return state === "complete" ? "complete" : "incomplete";
}

export function workspaceIntegrityActions(
  workspace: Pick<
    WorkspaceSummary,
    | "integrityClassification"
    | "canResumeIndexing"
    | "canRepairIndex"
    | "canRebuildConversations"
    | "hasLastVerification"
  >,
): WorkspaceIntegrityAction[] {
  const actions: WorkspaceIntegrityAction[] = ["verify"];
  if (workspace.hasLastVerification) actions.push("view_last_verification");
  if (workspace.integrityClassification !== "incomplete") return actions;
  if (workspace.canResumeIndexing) actions.push("resume_indexing");
  if (workspace.canRepairIndex) actions.push("repair_index");
  if (workspace.canRebuildConversations) actions.push("rebuild_conversations");
  return actions;
}

export function createWorkspaceIntegrityOperationToken(
  workspaceId: string,
  kind: WorkspaceIntegrityOperationKind,
  sequence: number,
): WorkspaceIntegrityOperationToken {
  return {
    workspaceId,
    operationId: `integrity-${sequence}`,
    kind,
  };
}

export function isWorkspaceIntegrityOperationCurrent(
  expected: WorkspaceIntegrityOperationToken | null | undefined,
  candidate: WorkspaceIntegrityOperationToken,
): boolean {
  return Boolean(
    expected &&
      expected.workspaceId === candidate.workspaceId &&
      expected.operationId === candidate.operationId &&
      expected.kind === candidate.kind,
  );
}

export function isVerificationReportForOperation(
  report: VerificationReport,
  operation: WorkspaceIntegrityOperationToken,
): boolean {
  return (
    report.workspaceId === operation.workspaceId &&
    report.operationId === operation.operationId
  );
}

function titleCaseState(value: string): string {
  return value
    .replace(/_/g, " ")
    .replace(/\b\w/g, (character: string) => character.toLocaleUpperCase());
}

export function workspaceIntegrityStateLabel(state: string): string {
  if (state === "none") return "None yet";
  return isSupportedWorkspaceIntegrityState(state) ? titleCaseState(state) : "Unsupported";
}

export function formatElapsedDuration(durationMs: number): string {
  if (!Number.isFinite(durationMs) || durationMs <= 0) return "Less than a minute";
  const totalMinutes = Math.floor(durationMs / 60_000);
  if (totalMinutes < 1) return "Less than a minute";
  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (hours === 0) return `${totalMinutes} minute${totalMinutes === 1 ? "" : "s"}`;
  if (minutes === 0) return `${hours} hour${hours === 1 ? "" : "s"}`;
  return `${hours} hour${hours === 1 ? "" : "s"}, ${minutes} minute${minutes === 1 ? "" : "s"}`;
}

export function resumeProgressDescription(
  details: ResumeDetails,
  formatCount: (value: number) => string = (value) => String(value),
): string {
  const stage = titleCaseState(details.interruptedStage || "import");
  if (details.discoveredCandidates > 0) {
    return `${stage} was interrupted after ${formatCount(details.canonicalIndexed)} of ${formatCount(details.discoveredCandidates)} discovered records.`;
  }
  return `${stage} was interrupted after ${formatCount(details.canonicalIndexed)} canonical records were indexed.`;
}

export function workspaceSourceAvailabilityMessage(
  sourceAvailability: WorkspaceSourceAvailability,
): string | null {
  if (sourceAvailability === "unavailable") {
    return "The local workspace remains usable, but source comparison is unavailable until the PST is mounted.";
  }
  if (sourceAvailability === "changed") {
    return "The available PST no longer matches this workspace's recorded source identity. No files or indexes were deleted.";
  }
  return null;
}

export function verificationResultLabel(result: VerificationReport["result"]): string {
  return titleCaseState(result);
}

export function verificationLevelLabel(level: VerificationLevel): string {
  return level === "quick" ? "Quick Verify" : "Full Verify";
}

export function verificationHeadline(report: VerificationReport): string {
  switch (report.result) {
    case "verified":
      return "Internally verified against this workspace's extracted source records.";
    case "source_unavailable":
      return "Source comparison is unavailable; the local workspace was not discarded.";
    case "source_changed":
      return "The available PST differs from the source identity recorded for this workspace.";
    case "repairable":
      return "The workspace has repairable internal inconsistencies.";
    case "incomplete":
      return "Verification found that this workspace is incomplete.";
    case "extraction_inconsistent":
      return "Verification found inconsistent extracted source records.";
    case "database_inconsistent":
      return "Verification found an inconsistent workspace database.";
    case "unsupported":
      return "This workspace uses unsupported integrity metadata.";
    case "failed":
      return "Verification could not be completed.";
  }
}

export function possibleDuplicateSummary(groupCount: number): string | null {
  return groupCount > 0 ? "Possible duplicate source records detected." : null;
}

export function integrityOperationLabel(kind: WorkspaceIntegrityOperationKind): string {
  switch (kind) {
    case "verify":
      return "Verify";
    case "load_last_verification":
      return "Load verification";
    case "resume_indexing":
      return "Resume indexing";
    case "repair_index":
      return "Repair index";
    case "rebuild_conversations":
      return "Rebuild conversations";
  }
}

export function integrityOperationRunningLabel(kind: WorkspaceIntegrityOperationKind): string {
  switch (kind) {
    case "verify":
      return "Verifying...";
    case "load_last_verification":
      return "Loading verification...";
    case "resume_indexing":
      return "Resuming...";
    case "repair_index":
      return "Repairing...";
    case "rebuild_conversations":
      return "Rebuilding conversations...";
  }
}

export function integrityOperationIsCancellable(kind: WorkspaceIntegrityOperationKind): boolean {
  return kind === "resume_indexing" || kind === "repair_index" || kind === "rebuild_conversations";
}

function sourceAvailabilityFromReport(
  report: VerificationReport,
  fallback: WorkspaceSourceAvailability,
): WorkspaceSourceAvailability {
  if (report.result === "source_unavailable" || report.sourceIntegrity === "unavailable") {
    return "unavailable";
  }
  if (report.result === "source_changed" || report.sourceIntegrity === "changed") return "changed";
  if (report.sourceIntegrity === "passed") return "available";
  return fallback;
}

export function workspaceSummaryWithVerification(
  workspace: WorkspaceSummary,
  report: VerificationReport,
): WorkspaceSummary {
  const integrityClassification =
    report.result === "unsupported"
      ? "unsupported"
      : report.result === "verified" ||
          report.result === "source_unavailable" ||
          report.result === "source_changed"
        ? integrityClassificationForState(report.workspaceState)
        : "incomplete";
  const repairableIndex =
    report.result === "repairable" || report.result === "database_inconsistent";
  return {
    ...workspace,
    integrityState: report.workspaceState,
    integrityClassification,
    sourceAvailability: sourceAvailabilityFromReport(report, workspace.sourceAvailability),
    canResumeIndexing:
      integrityClassification === "incomplete" &&
      (workspace.canResumeIndexing || report.result === "repairable"),
    canRepairIndex:
      integrityClassification === "incomplete" &&
      (workspace.canRepairIndex || repairableIndex),
    canRebuildConversations:
      integrityClassification === "incomplete" &&
      (workspace.canRebuildConversations || report.conversationState !== "complete"),
    hasLastVerification: true,
    conversationState: report.conversationState,
  };
}
