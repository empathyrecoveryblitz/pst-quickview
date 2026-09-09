import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

import {
  aggregateLocalTrashSummaries,
  clearLocalTrashWorkspaceView,
  createLocalTrashOperationToken,
  createLocalTrashResetToken,
  createLocalTrashViewState,
  effectiveAllOpenLocalTrashQueryScope,
  effectiveLocalTrashQueryScope,
  enterLocalTrashView,
  initialLocalTrashUndoState,
  isLocalTrashMutationResponseCurrent,
  isLocalTrashResetResponseCurrent,
  isLocalTrashUndoCurrent,
  leaveLocalTrashView,
  localTrashActionDisclosure,
  localTrashResetDisclosure,
  localTrashStorageDisclosure,
  reduceLocalTrashUndoState,
  withCanonicalMessageViewScope,
} from "../src/localTrash.ts";

const localTrashSource = readFileSync(
  new URL("../src/localTrash.ts", import.meta.url),
  "utf8",
);
const appSource = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
const typesSource = readFileSync(new URL("../src/types.ts", import.meta.url), "utf8");
const backendSource = readFileSync(
  new URL("../src-tauri/src/lib.rs", import.meta.url),
  "utf8",
);
const buildSource = readFileSync(new URL("../src-tauri/build.rs", import.meta.url), "utf8");
const mainCapability = JSON.parse(
  readFileSync(new URL("../src-tauri/capabilities/main.json", import.meta.url), "utf8"),
);

function compactSource(source) {
  return source.replace(/\s+/g, " ").trim();
}

function sourceSection(source, startMarker, endMarker) {
  const start = source.indexOf(startMarker);
  assert.notEqual(start, -1, `missing source marker: ${startMarker}`);
  const end = source.indexOf(endMarker, start + startMarker.length);
  assert.notEqual(end, -1, `missing source marker: ${endMarker}`);
  return source.slice(start, end);
}

function literalInvokeCommands(source) {
  return Array.from(
    source.matchAll(/invoke(?:<[^>]+>)?\(\s*"([a-z0-9_]+)"/g),
    (match) => match[1],
  );
}

function summary(workspaceId, overrides = {}) {
  return {
    workspaceId,
    available: true,
    canReset: false,
    sidecarPresent: true,
    trashedMessageCount: 0,
    orphanedStateCount: 0,
    schemaVersion: 1,
    stateRevision: 0,
    lastChangedAt: null,
    fingerprintMatch: true,
    lastErrorCode: null,
    ...overrides,
  };
}

function mutation(workspaceId, overrides = {}) {
  return {
    workspaceId,
    result: "moved",
    changedCount: 1,
    stateRevision: 1,
    operationId: "operation-1",
    ...overrides,
  };
}

test("Current PST summary exposes available aggregate counts", () => {
  assert.deepEqual(
    aggregateLocalTrashSummaries(
      ["workspace-a"],
      [summary("workspace-a", { trashedMessageCount: 4, orphanedStateCount: 1 })],
    ),
    {
      workspaceCount: 1,
      availableWorkspaceCount: 1,
      unavailableWorkspaceCount: 0,
      sidecarPresentWorkspaceCount: 1,
      trashedMessageCount: 4,
      orphanedStateCount: 1,
      available: true,
    },
  );
});

test("All Open counts only available effective workspaces and reports unavailable ones", () => {
  const aggregate = aggregateLocalTrashSummaries(
    ["workspace-a", "workspace-b", "workspace-c", "workspace-a"],
    [
      summary("workspace-a", { trashedMessageCount: 3, orphanedStateCount: 2 }),
      summary("workspace-b", {
        available: false,
        trashedMessageCount: 99,
        orphanedStateCount: 99,
        lastErrorCode: "LOCAL_STATE_UNAVAILABLE",
      }),
      summary("workspace-not-open", { trashedMessageCount: 500 }),
    ],
  );

  assert.deepEqual(aggregate, {
    workspaceCount: 3,
    availableWorkspaceCount: 1,
    unavailableWorkspaceCount: 2,
    sidecarPresentWorkspaceCount: 2,
    trashedMessageCount: 3,
    orphanedStateCount: 2,
    available: false,
  });
});

test("wrong-fingerprint and duplicate workspace summaries remain unavailable", () => {
  assert.deepEqual(
    aggregateLocalTrashSummaries(
      ["workspace-a", "workspace-b"],
      [
        summary("workspace-a", { fingerprintMatch: false, trashedMessageCount: 8 }),
        summary("workspace-b", { trashedMessageCount: 2 }),
        summary("workspace-b", { trashedMessageCount: 2 }),
      ],
    ),
    {
      workspaceCount: 2,
      availableWorkspaceCount: 0,
      unavailableWorkspaceCount: 2,
      sidecarPresentWorkspaceCount: 1,
      trashedMessageCount: 0,
      orphanedStateCount: 0,
      available: false,
    },
  );
});

test("operation currentness rejects stale tokens and isolates workspace revisions", () => {
  const requestToken = createLocalTrashOperationToken("workspace-a", 4, 7);
  assert.ok(requestToken);
  const response = mutation("workspace-a", {
    stateRevision: 5,
    operationId: "operation-current",
  });
  const input = {
    requestToken,
    activeToken: requestToken,
    currentWorkspaceRevision: 4,
    response,
  };

  assert.equal(isLocalTrashMutationResponseCurrent(input), true);
  assert.equal(
    isLocalTrashMutationResponseCurrent({
      ...input,
      activeToken: createLocalTrashOperationToken("workspace-a", 4, 8),
    }),
    false,
    "a later request owns the operation lane",
  );
  assert.equal(
    isLocalTrashMutationResponseCurrent({ ...input, currentWorkspaceRevision: 5 }),
    false,
    "a changed revision makes the response stale",
  );
  assert.equal(
    isLocalTrashMutationResponseCurrent({
      ...input,
      response: mutation("workspace-b", { stateRevision: 5 }),
    }),
    false,
    "a response cannot cross workspace identity",
  );
  assert.equal(
    isLocalTrashMutationResponseCurrent({
      ...input,
      response: mutation("workspace-a", { stateRevision: 3 }),
    }),
    false,
    "a regressive revision is rejected",
  );
  assert.equal(
    isLocalTrashMutationResponseCurrent({
      ...input,
      response: mutation("workspace-a", { stateRevision: 6 }),
    }),
    false,
    "a forward revision leap is rejected",
  );
  assert.equal(
    isLocalTrashMutationResponseCurrent({
      ...input,
      response: mutation("workspace-a", {
        result: "already_trashed",
        changedCount: 0,
        stateRevision: 4,
        operationId: null,
      }),
    }),
    true,
    "an idempotent no-op must echo the exact expected revision",
  );
});

test("Reset responses are current only for the exact token, workspace, and backend target", () => {
  const token = createLocalTrashResetToken("workspace-a", 9);
  assert.ok(token);
  const response = { workspaceId: "workspace-a", reset: true };
  assert.equal(
    isLocalTrashResetResponseCurrent(token, token, "workspace-a", response),
    true,
  );
  assert.equal(
    isLocalTrashResetResponseCurrent(
      token,
      createLocalTrashResetToken("workspace-a", 10),
      "workspace-a",
      response,
    ),
    false,
  );
  assert.equal(
    isLocalTrashResetResponseCurrent(token, token, "workspace-b", response),
    false,
  );
  assert.equal(
    isLocalTrashResetResponseCurrent(token, token, "workspace-a", {
      workspaceId: "workspace-b",
      reset: true,
    }),
    false,
  );
});

test("Undo state retains exactly the most recent successful move and clears on restart", () => {
  let state = initialLocalTrashUndoState();
  assert.equal(state, null, "transient Undo does not restore after restart");

  state = reduceLocalTrashUndoState(state, {
    type: "mutation_succeeded",
    response: mutation("workspace-a", {
      stateRevision: 5,
      operationId: "operation-a",
    }),
  });
  assert.deepEqual(state, {
    workspaceId: "workspace-a",
    operationId: "operation-a",
    expectedRevision: 5,
  });
  assert.equal(isLocalTrashUndoCurrent(state, "workspace-a", 5), true);
  assert.equal(isLocalTrashUndoCurrent(state, "workspace-b", 5), false);
  assert.equal(isLocalTrashUndoCurrent(state, "workspace-a", 6), false);

  state = reduceLocalTrashUndoState(state, {
    type: "mutation_succeeded",
    response: mutation("workspace-b", {
      stateRevision: 9,
      operationId: "operation-b",
    }),
  });
  assert.deepEqual(state, {
    workspaceId: "workspace-b",
    operationId: "operation-b",
    expectedRevision: 9,
  });
  assert.equal(reduceLocalTrashUndoState(state, { type: "restart" }), null);
});

test("failed or mismatched Undo preserves state while exact success clears it", () => {
  const state = {
    workspaceId: "workspace-a",
    operationId: "operation-a",
    expectedRevision: 5,
  };
  const mismatched = reduceLocalTrashUndoState(state, {
    type: "undo_succeeded",
    response: mutation("workspace-b", {
      result: "undone",
      stateRevision: 6,
      operationId: "operation-a",
    }),
  });
  assert.equal(mismatched, state);

  const succeeded = reduceLocalTrashUndoState(state, {
    type: "undo_succeeded",
    response: mutation("workspace-a", {
      result: "undone",
      stateRevision: 6,
      operationId: "operation-a",
    }),
  });
  assert.equal(succeeded, null);
});

test("Local Trash view transitions preserve canonical folder scope and prior mode", () => {
  const canonicalScope = {
    selectedWorkspaceId: "workspace-a",
    folderId: 7,
    includeSubfolders: true,
  };
  const active = createLocalTrashViewState(canonicalScope, "conversations");
  const trashed = enterLocalTrashView(active);

  assert.equal(trashed.visibility, "trashed");
  assert.equal(trashed.listMode, "messages");
  assert.deepEqual(trashed.canonicalScope, canonicalScope);
  assert.deepEqual(effectiveLocalTrashQueryScope(trashed), {
    selectedWorkspaceId: "workspace-a",
    folderId: null,
    includeSubfolders: false,
  });

  const withChangedCanonicalScope = withCanonicalMessageViewScope(trashed, {
    selectedWorkspaceId: "workspace-a",
    folderId: 11,
    includeSubfolders: false,
  });
  const restored = leaveLocalTrashView(withChangedCanonicalScope);
  assert.equal(restored.visibility, "active");
  assert.equal(restored.listMode, "conversations");
  assert.deepEqual(effectiveLocalTrashQueryScope(restored), {
    selectedWorkspaceId: "workspace-a",
    folderId: 11,
    includeSubfolders: false,
  });

  assert.deepEqual(clearLocalTrashWorkspaceView(trashed), {
    visibility: "active",
    listMode: "conversations",
    previousActiveListMode: null,
    canonicalScope: {
      selectedWorkspaceId: null,
      folderId: null,
      includeSubfolders: false,
    },
  });
});

test("All Open Local Trash targets are independent of the restorable canonical scope", () => {
  const canonicalScope = {
    selectedWorkspaceId: "workspace-a",
    folderId: 17,
    includeSubfolders: true,
  };

  assert.deepEqual(
    effectiveAllOpenLocalTrashQueryScope(
      canonicalScope,
      "trashed",
      "workspace-b",
    ),
    {
      selectedWorkspaceId: "workspace-b",
      folderId: null,
      includeSubfolders: false,
    },
  );
  assert.deepEqual(canonicalScope, {
    selectedWorkspaceId: "workspace-a",
    folderId: 17,
    includeSubfolders: true,
  });
  assert.deepEqual(
    effectiveAllOpenLocalTrashQueryScope(canonicalScope, "active", null),
    canonicalScope,
  );
});

test("source contracts describe local-only visibility without destructive or storage claims", () => {
  assert.equal(
    localTrashActionDisclosure,
    "This affects only PST QuickView's local workspace. The source PST will not be changed.",
  );
  assert.equal(
    localTrashStorageDisclosure,
    "Local Trash hides messages from normal PST QuickView views. It does not change the source PST or reduce workspace storage.",
  );
  assert.match(localTrashResetDisclosure, /app-only organization/);
  assert.match(localTrashResetDisclosure, /does not change the source PST/);

  const sourceContracts = [
    localTrashActionDisclosure,
    localTrashStorageDisclosure,
    localTrashResetDisclosure,
  ].join(" ");
  assert.ok(
    (appSource.match(/title=\{localTrashStorageDisclosure\}/g) ?? []).length >= 3,
    "each virtual entry should expose the no-storage-reduction disclosure",
  );
  assert.match(appSource, /aria-describedby="local-trash-action-disclosure"/);
  assert.match(appSource, /\{localTrashActionDisclosure\} Local Trash removes no message content/);
  assert.match(appSource, /<p>\{localTrashResetDisclosure\}<\/p>/);
  const helpCopy = compactSource(
    sourceSection(appSource, "<dt>Local Trash</dt>", "<dt>Folder hierarchy</dt>"),
  );
  assert.match(
    helpCopy,
    /If its overlay is absent, all messages are active and the first Move initializes it\./,
  );
  assert.match(
    helpCopy,
    /If the overlay is invalid,[\s\S]*Local Trash actions are unavailable\./,
  );
  assert.doesNotMatch(helpCopy, /overlay is missing[\s\S]*actions are unavailable/i);
  assert.doesNotMatch(sourceContracts, /free(?:s|d)? (?:disk )?space|reclaim(?:s|ed)? storage/i);
  assert.doesNotMatch(localTrashSource, /Empty Local Trash|Empty Trash|permanentlyDelete|purgeLocalTrash/);
});

test("a successful restore invalidates an older Move Undo while an idempotent echo retains it", () => {
  const undo = {
    workspaceId: "workspace-fictional",
    operationId: "operation-fictional",
    expectedRevision: 8,
  };

  assert.equal(
    reduceLocalTrashUndoState(undo, {
      type: "mutation_succeeded",
      response: mutation("workspace-fictional", {
        result: "restored",
        stateRevision: 9,
        operationId: null,
      }),
    }),
    null,
  );
  assert.equal(
    reduceLocalTrashUndoState(undo, {
      type: "mutation_succeeded",
      response: mutation("workspace-fictional", {
        result: "already_active",
        changedCount: 0,
        stateRevision: 8,
        operationId: null,
      }),
    }),
    undo,
  );
});

test("the frontend invokes the five Local Trash commands literally and both Tauri allowlists agree", () => {
  const commands = [
    "move_message_to_local_trash",
    "read_local_trash_summary",
    "reset_local_trash_state",
    "restore_message_from_local_trash",
    "undo_local_trash_operation",
  ];
  const allowedPermissions = commands.map((command) => `allow-${command.replaceAll("_", "-")}`);

  assert.deepEqual(
    Array.from(new Set(literalInvokeCommands(appSource).filter((command) => command.includes("local_trash")))).sort(),
    commands,
  );
  assert.deepEqual(
    Array.from(buildSource.matchAll(/"([a-z0-9_]*local_trash[a-z0-9_]*)"/g), (match) => match[1]).sort(),
    commands,
  );
  assert.deepEqual(
    mainCapability.permissions.filter((permission) => permission.includes("local-trash")).sort(),
    allowedPermissions.sort(),
  );
});

test("typed and request contracts carry active or trashed visibility through every message path", () => {
  assert.match(typesSource, /export type MessageVisibility\s*=\s*"active"\s*\|\s*"trashed"/);
  assert.match(appSource, /useState<MessageVisibility>\("active"\)/);
  assert.match(appSource, /visibility:\s*messageVisibility/);

  const messageDetail = sourceSection(typesSource, "export type MessageDetail", "export type HtmlRenderResult");
  const sourceView = sourceSection(typesSource, "export type SourceEmlView", "export type ExternalFileOpen");
  for (const contract of [messageDetail, sourceView]) {
    assert.match(contract, /inLocalTrash:\s*boolean/);
    assert.match(contract, /localTrashAvailable:\s*boolean/);
    assert.match(contract, /localTrashErrorCode:\s*string\s*\|\s*null/);
  }

  for (const command of [
    "list_messages",
    "search_messages_multi",
    "count_messages",
    "count_messages_multi",
    "list_conversations",
    "count_conversations",
    "get_conversation_messages",
  ]) {
    const commandOffset = appSource.indexOf(`\"${command}\"`);
    assert.notEqual(commandOffset, -1, `missing frontend request: ${command}`);
    assert.match(
      appSource.slice(commandOffset, commandOffset + 1_200),
      /visibility:\s*snapshot\.visibility/,
      `${command} must use the applied visibility snapshot`,
    );
  }
});

test("Local Trash is a reserved virtual Messages view for Current PST and All Open PSTs", () => {
  const compactApp = compactSource(appSource);
  assert.match(compactApp, /<option value="current">Current PST<\/option>/);
  assert.match(compactApp, /<option value="all_open">All Open PSTs<\/option>/);
  assert.ok(
    (appSource.match(/<span className="folder-name">Local Trash<\/span>/g) ?? []).length >= 3,
    "Current, All Open aggregate, and per-workspace Local Trash entries must be present",
  );
  assert.match(compactApp, /onClick=\{\(\) => showLocalTrash\(null\)\}/);
  assert.match(compactApp, /onClick=\{\(\) => showLocalTrash\(workspaceId\)\}/);
  assert.match(compactApp, /onClick=\{\(\) => showLocalTrash\(workspace\.id\)\}/);
  assert.match(
    compactApp,
    /aggregateLocalTrashSummaries\( openWorkspaceIds, openPstSessions\.map\(\(session\) => session\.localTrashSummary\), \)/,
  );

  const viewTransitions = compactSource(
    sourceSection(appSource, "function showActiveMessages()", "function selectCurrentWorkspaceFolder"),
  );
  assert.match(viewTransitions, /setMessageVisibility\("trashed"\); setListMode\("messages"\)/);
  assert.match(
    viewTransitions,
    /setMessageVisibility\("active"\); setLocalTrashAllOpenWorkspaceId\(null\); setListMode\(previousActiveListModeRef\.current\)/,
  );
  assert.match(compactApp, /Local Trash uses Messages view\./);
  assert.match(
    compactApp,
    /<option value="conversations" disabled=\{messageVisibility === "trashed"\}> Conversations <\/option>/,
  );
});

test("Move, Restore, and Undo preserve selected preview state and reject stale responses", () => {
  const mutationSource = compactSource(
    sourceSection(
      appSource,
      "async function changeSelectedMessageLocalTrash",
      "async function undoMostRecentLocalTrashMove",
    ),
  );
  const undoSource = compactSource(
    sourceSection(
      appSource,
      "async function undoMostRecentLocalTrashMove",
      "function requestLocalTrashReset",
    ),
  );

  assert.match(mutationSource, /nextTrashed \? await invoke<LocalTrashMutationResult>\("move_message_to_local_trash"/);
  assert.match(mutationSource, /: await invoke<LocalTrashMutationResult>\("restore_message_from_local_trash"/);
  assert.match(mutationSource, /expectedRevision: token\.expectedRevision/);
  assert.match(mutationSource, /!isLocalTrashMutationResponseCurrent\(/);
  assert.match(mutationSource, /Local Trash state changed while this request was running\. The latest state was reloaded\./);
  assert.match(mutationSource, /preserveSelectedMessageRef\.current = selectedKey/);
  assert.match(mutationSource, /\? \{ \.\.\.current, inLocalTrash: nextTrashed \} : current/);
  assert.match(mutationSource, /Moved to Local Trash\./);
  assert.match(mutationSource, /Message restored to normal views\./);
  assert.match(mutationSource, /Local Trash action could not be completed\./);
  assert.match(mutationSource, /LOCAL_TRASH_OPERATION_INDETERMINATE/);
  assert.match(mutationSource, /Local Trash operation needs reinspection\./);
  assert.match(mutationSource, /refreshLocalTrashPreviewState\(targetWorkspaceId, selectedMessage\.id\)/);
  assert.match(mutationSource, /await refreshLocalTrashSummary\(targetWorkspaceId\); refreshMessagesAfterLocalTrashChange\(\)/);

  assert.match(undoSource, /invoke<LocalTrashMutationResult>\("undo_local_trash_operation"/);
  assert.match(undoSource, /operationId: localTrashUndo\.operationId/);
  assert.match(undoSource, /!isLocalTrashMutationResponseCurrent\(/);
  assert.match(undoSource, /\? \{ \.\.\.current, inLocalTrash: false \} : current/);
  assert.match(undoSource, /Move undone\. The message is visible in normal views\./);
  assert.match(undoSource, /Undo failed; the message remains in Local Trash\./);
  assert.match(undoSource, /LOCAL_TRASH_OPERATION_INDETERMINATE/);
  assert.match(undoSource, /Undo outcome needs reinspection\./);
  assert.match(undoSource, /current\?\.workspaceId === localTrashUndo\.workspaceId \? null : current/);
});

test("query health reconciles changed overlays without adding an unconditional summary rescan", () => {
  const refreshSource = compactSource(
    sourceSection(appSource, "async function refreshLocalTrashSummary", "function refreshMessagesAfterLocalTrashChange"),
  );
  assert.match(refreshSource, /Promise\.all\(\[/);
  assert.match(refreshSource, /invoke<LocalTrashSummary>\("read_local_trash_summary", \{ workspaceId \}\)/);
  assert.match(refreshSource, /invoke<Folder\[\]>\("list_folders", \{ workspaceId \}\)/);
  assert.match(refreshSource, /localTrashSummaryRequestSequenceRef\.current\.get\(workspaceId\) === requestSequence/);
  assert.match(refreshSource, /replaceLocalTrashWorkspaceState\(summary, refreshedFolders\)/);

  assert.match(typesSource, /export type LocalTrashQueryHealth = \{/);
  assert.ok(
    (typesSource.match(/localTrashStates:\s*LocalTrashQueryHealth\[\]/g) ?? []).length >= 2,
    "message and conversation page contracts should carry query-observed Local Trash health",
  );
  assert.ok(
    (backendSource.match(/local_trash_states:\s*Vec<LocalTrashQueryHealth>/g) ?? []).length >= 3,
    "single, multi-workspace, and conversation pages should return query-observed Local Trash health",
  );
  assert.ok(
    (appSource.match(/reconcileLocalTrashQueryHealth\(result\.localTrashStates\)/g) ?? []).length >= 2,
    "current message and conversation results should reconcile the health observed by their query",
  );

  const reconcileSource = compactSource(
    sourceSection(
      appSource,
      "function reconcileLocalTrashQueryHealth",
      "async function refreshLocalTrashPreviewState",
    ),
  );
  assert.match(reconcileSource, /const alreadyCurrent = Boolean\(/);
  assert.match(reconcileSource, /if \(alreadyCurrent\) continue/);
  assert.match(reconcileSource, /if \(!health\.available\) \{ replaceLocalTrashWorkspaceState\(/);
  assert.match(reconcileSource, /current\?\.workspaceId === health\.workspaceId \? null : current/);
  assert.match(reconcileSource, /void refreshLocalTrashSummary\(health\.workspaceId\)/);
  assert.doesNotMatch(appSource, /refreshLocalTrashSummariesAfterQuery/);

  for (const field of [
    "available",
    "canReset",
    "sidecarPresent",
    "schemaVersion",
    "stateRevision",
    "fingerprintMatch",
    "lastErrorCode",
  ]) {
    assert.match(reconcileSource, new RegExp(`summary\\.${field} === health\\.${field}`));
  }

  assert.match(appSource, /Canonical imported messages/);
  assert.match(appSource, /Local-state overlay/);
});

test("details panel reports both canonical and local state", () => {
  for (const field of [
    "activeMessageCount",
    "activeDirectMessageCount",
    "localTrashMessageCount",
    "localTrashDirectMessageCount",
  ]) {
    assert.match(typesSource, new RegExp(`${field}:\\s*number`));
  }
  for (const label of [
    "Canonical imported messages",
    "Visible active messages",
    "Messages in Local Trash",
    "Orphaned local state",
    "Local-state schema",
    "Local-state overlay",
    "Local-state revision",
    "Last Local Trash change",
    "Source/workspace fingerprint",
    "Last local-state error code",
    "Canonical direct messages",
    "Canonical with descendants",
    "Active direct messages",
    "Active with descendants",
    "Local Trash direct",
    "Local Trash with descendants",
  ]) {
    assert.match(appSource, new RegExp(label));
  }
  const folderDetails = sourceSection(
    appSource,
    '<section className="folder-details" aria-label="Folder Details">',
    "</section>",
  );
  assert.equal(
    (folderDetails.match(/formatCount\(folderDetailsFolder\.directMessageCount\)/g) ?? []).length,
    1,
  );
  assert.equal(
    (folderDetails.match(/formatCount\(folderDetailsFolder\.messageCount\)/g) ?? []).length,
    1,
  );
  assert.doesNotMatch(folderDetails, /Imported canonical count/);
});

test("preview badges, result preservation, and reset focus intent remain wired", () => {
  assert.ok(
    (appSource.match(/className="local-trash-badge">In Local Trash/g) ?? []).length >= 3,
    "main preview, pop-out preview, and source view should identify Local Trash state",
  );
  assert.match(appSource, /preserveSelectedMessageRef\.current === currentKey/);
  assert.match(appSource, /preserveSelectedMessageRef\.current = selectedKey/);

  const resetSource = compactSource(
    sourceSection(appSource, "function requestLocalTrashReset", "async function openStandaloneMessagePreviewWindow"),
  );
  assert.match(resetSource, /localTrashResetReturnFocusRef\.current = returnFocus/);
  assert.match(resetSource, /requestAnimationFrame\(\(\) => localTrashResetCancelRef\.current\?\.focus\(\)\)/);
  assert.match(resetSource, /if \(returnFocus\?\.isConnected\) returnFocus\.focus\(\)/);
  assert.match(resetSource, /if \(event\.key === "Escape"\)/);
  assert.match(resetSource, /if \(event\.key !== "Tab"\) return/);
  assert.match(resetSource, /last\.focus\(\)/);
  assert.match(resetSource, /first\.focus\(\)/);
});

test("reset is constrained to unhealthy state and has an explicit accessible confirmation", () => {
  const compactApp = compactSource(appSource);
  const resetEligibility = compactSource(
    sourceSection(appSource, "function canResetLocalTrashState", "function errorSummary"),
  );
  assert.match(typesSource, /canReset:\s*boolean/);
  assert.match(resetEligibility, /summary\?\.canReset === true/);
  assert.doesNotMatch(resetEligibility, /LOCAL_TRASH_[A-Z_]+/);
  assert.match(compactApp, /role="dialog" aria-modal="true" aria-labelledby=\{localTrashResetTitleId\} aria-describedby=\{localTrashResetDescriptionId\}/);
  assert.match(compactApp, /Reset unhealthy Local Trash state\?/);
  assert.match(compactApp, /This clears only this workspace&apos;s versioned local-state overlay/);
  assert.match(compactApp, /It removes no message content and does not reclaim storage\./);
  assert.match(compactApp, /invoke<LocalTrashResetResult>\("reset_local_trash_state", \{ workspaceId: targetWorkspaceId, \}\)/);
  assert.match(compactApp, /isLocalTrashResetResponseCurrent\(/);
  assert.match(compactApp, /setMessageVisibility\("active"\)/);
  assert.match(compactApp, /folderSelection: session\.canonicalFolderSelection/);
  assert.match(compactApp, /LOCAL_TRASH_RESET_INDETERMINATE/);
  assert.match(compactApp, /Local Trash reset needs reinspection\./);
});

test("Local Trash status and failure UI expose polite live semantics without a purge action", () => {
  const compactApp = compactSource(appSource);
  assert.match(
    compactApp,
    /className=\{`local-trash-status[^`]*`\} role="status" aria-live="polite" aria-atomic="true"/,
  );
  assert.match(compactApp, /Local Trash action failed/);
  assert.match(compactApp, /Undo is no longer available because Local Trash state changed\./);
  assert.match(compactApp, /Local Trash is app-only and reversible\./);
  assert.match(compactApp, /Normal views remain fail-open; no messages are treated as deleted\./);

  const allLocalTrashContracts = [
    appSource,
    localTrashSource,
    typesSource,
    buildSource,
    JSON.stringify(mainCapability),
  ].join("\n");
  assert.doesNotMatch(allLocalTrashContracts, /Empty Local Trash|Empty Trash|purge[_A-Za-z]*LocalTrash|purge_local_trash/i);
  const declaredCommands = Array.from(
    buildSource.matchAll(/"([a-z0-9_]*local_trash[a-z0-9_]*)"/g),
    (match) => match[1],
  );
  assert.equal(declaredCommands.some((command) => /delete|empty|purge/.test(command)), false);
});
