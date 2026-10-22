import React from 'react';
import { createRoot } from 'react-dom/client';
import { mockIPC, mockWindows } from '@tauri-apps/api/mocks';
import App from '../../src/App';
import { createSupportView } from '../../src/supportView';
import type { PstOpenPlan } from '../../src/types';
import '../../src/styles.css';

// The real App is mounted, but every native boundary is replaced. No source
// file, workspace, peer, or real support session exists in this fixture.
const plan: PstOpenPlan = {
  pstPath: '/fictional/source.pst', fingerprint: 'fictional', workspaceId: 'fictional-workspace',
  selectedWorkspacePath: '/fictional/workspace', selectedWorkspaceLocationMode: 'next_to_pst',
  selectedWorkspaceLocationLabel: 'Fictional cache', fallbackWarning: null,
  preflight: {
    originalPstPath: '/fictional/source.pst', originalPstExists: true, originalPstReadable: true,
    pstSizeBytes: 100, workspacePath: '/fictional/workspace', workspaceParentPath: '/fictional',
    workspaceLocationMode: 'next_to_pst', workspaceLocationLabel: 'Fictional cache',
    workspaceParentWritable: true, workspaceParentWriteError: null, availableDiskBytes: 100000,
    estimatedRequiredBytes: 1000, hasEnoughSpace: true, spaceWarning: false, warningRequired: false, warnings: [],
  },
  existingWorkspaces: [{
    workspaceId: 'fictional-workspace', workspacePath: '/fictional/workspace',
    workspaceLocationMode: 'next_to_pst', workspaceLocationLabel: 'Fictional cache',
    importStatus: 'interrupted', isComplete: false, canResume: true, canReimport: true,
    messageCount: 0, canOpen: false, integrityState: 'failed', integrityClassification: 'incomplete',
    canRepairIndex: true, canRebuildConversations: false, resumeDetails: null, hasLastVerification: false,
  }],
};
let mutationCalls = 0, nativeDialogs = 0, errors = 0, run = 0;
const commandNames = new Set<string>();
window.addEventListener('error', () => { errors++; });
window.addEventListener('unhandledrejection', () => { errors++; });
window.confirm = () => { nativeDialogs++; throw Error('Native confirmation was invoked'); };
mockWindows('main');
mockIPC((command) => {
  commandNames.add(command);
  if (command === 'pick_pst_file') return plan.pstPath;
  if (command === 'plan_pst_open') return plan;
  if (command === 'check_readpst') return { available: true, sourceLabel: 'Fictional test parser' };
  if (command === 'frontend_ready_for_file_opens') return { externalOpenReceived: false, batches: [] };
  if (command === 'support_session_status') return null;
  if (command === 'support_resume_status') return { available: false };
  if (command === 'cancel_search_operation') return null;
  if (command === 'open_pst' || command === 'delete_planned_workspace' || command === 'repair_workspace_index' || command === 'delete_workspace') {
    mutationCalls++;
    throw Error('Fictional operation stopped at the native boundary');
  }
  throw Error('Unexpected fictional test command');
}, { shouldMockEvents: true });
const root = createRoot(document.getElementById('root')!);
const result = document.getElementById('result')!;
const settle = () => new Promise<void>(resolve => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));
const button = (label: string, container: ParentNode = document.getElementById('root')!) => {
  const found = Array.from(container.querySelectorAll<HTMLButtonElement>('button')).find(item => item.textContent?.trim() === label);
  if (!found) throw Error('Expected app control is missing');
  return found;
};
const assert = (value: unknown, category: string) => { if (!value) throw Error(category); };
const view = createSupportView(document);
async function remoteClick(element: HTMLButtonElement) {
  element.scrollIntoView({ block: 'center' }); await settle();
  const frame = view.capture(), rect = element.getBoundingClientRect();
  const target = frame.targets.find(item => Math.abs(item.x - rect.x) < 2 && Math.abs(item.y - rect.y) < 2);
  assert(target?.kind === 'click', 'remote target is missing');
  assert(view.apply({ frame: frame.id, target: target!.id, kind: 'click' }), 'remote click rejected');
  await settle();
}
function assertLocalConfirmation(element: HTMLButtonElement) {
  const frame = view.capture(), rect = element.getBoundingClientRect();
  assert(!frame.targets.some(item => Math.abs(item.x - rect.x) < 2 && Math.abs(item.y - rect.y) < 2), 'confirmation was remotely exposed');
  element.click();
}
root.render(<App />);
document.getElementById('run')!.onclick = async () => {
  const checks: Record<string, boolean> = {};
  result.textContent = 'Running fictional recovery checks';
  try {
    mutationCalls = 0; nativeDialogs = 0; errors = 0;
    root.render(<App key={++run} />); await settle();
    button('Open PST').click(); await settle();
    assert(document.body.textContent?.includes('Incomplete import workspace found.'), 'recovery prompt is missing');
    checks.realRecoveryPromptMounted = true;
    const repair = button('Repair Index');
    await remoteClick(repair);
    const repairDialog = document.querySelector<HTMLElement>('[role="dialog"][aria-modal="true"]');
    assert(repairDialog?.textContent?.includes('Repair this workspace index?'), 'repair first click failed');
    checks.remoteRepairFirstClick = true;
    assertLocalConfirmation(button('Repair Index', repairDialog!)); await settle();
    assert(mutationCalls === 0 && repairDialog?.isConnected, 'synthetic repair confirmation escaped');
    checks.repairConfirmationLocalOnly = true;
    await remoteClick(button('Cancel', repairDialog!));
    assert(!repairDialog?.isConnected && document.activeElement === repair, 'repair cancel did not restore focus');
    checks.remoteRepairCancelAndFocus = true;
    const deletion = button('Delete workspace');
    await remoteClick(deletion);
    const deleteDialog = document.querySelector<HTMLElement>('[aria-label="Confirm workspace delete"]');
    assert(deleteDialog, 'delete first click failed');
    checks.remoteDeleteFirstClick = true;
    assertLocalConfirmation(button('Yes, delete workspace', deleteDialog!)); await settle();
    assert(mutationCalls === 0 && deleteDialog?.isConnected, 'synthetic delete confirmation escaped');
    checks.deleteConfirmationLocalOnly = true;
    await remoteClick(button('Cancel', deleteDialog!));
    assert(!deleteDialog?.isConnected && document.activeElement === deletion, 'delete cancel did not restore focus');
    checks.remoteDeleteCancelAndFocus = true;
    assert(mutationCalls === 0, 'workspace operation was invoked');
    assert(nativeDialogs === 0, 'native dialog was invoked');
    assert(errors === 0, 'render or promise error occurred');
    checks.zeroWorkspaceOperations = true; checks.zeroNativeDialogs = true; checks.noRenderOrPromiseErrors = true;
    result.textContent = JSON.stringify({ passed: true, ...checks }, null, 2);
  } catch (error) {
    result.textContent = JSON.stringify({ passed: false, ...checks, error: error instanceof Error ? error.message : 'Fictional check failed', mutationCalls, nativeDialogs, errors }, null, 2);
  }
};
