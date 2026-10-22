import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';

const source = readFileSync(new URL('../src/App.tsx', import.meta.url), 'utf8');
const tree = ts.createSourceFile('App.tsx', source, ts.ScriptTarget.Latest, true, ts.ScriptKind.TSX);
const app = tree.statements.find(node => ts.isFunctionDeclaration(node) && node.name?.text === 'App');
const helpers = new Map(app.body.statements.filter(ts.isFunctionDeclaration).map(node => [node.name.text, node]));
function helper(name) {
  assert.ok(helpers.has(name), `Actual App handler exists: ${name}`);
  return helpers.get(name).getText(tree);
}
const existing = {
  workspaceId: 'fictional-workspace', workspacePath: '/fictional/workspace',
  workspaceLocationLabel: 'Fictional cache', integrityClassification: 'incomplete',
  integrityState: 'failed', canRepairIndex: true, canReimport: true, canResume: true,
  canOpen: false, resumeDetails: null, importStatus: 'interrupted',
};
const plan = { pstPath: '/fictional/source.pst' };

function fixture() {
  const queued = [], operations = [];
  const state = { repair: null, deletion: null, focusReturns: 0, cancelFocused: 0, pendingFlowVisible: true };
  const origin = { isConnected: true, focus() { state.focusReturns++; } };
  const context = {
    React: { createElement(type, props, ...children) { return { type, props: props || {}, children }; } },
    isBusy: false, pendingPlannedRepair: null, pendingPlannedDelete: null, workspace: null,
    repairConfirmOpen: false, deleteConfirmOpen: false,
    get visiblePendingPlannedRepair() { return state.pendingFlowVisible ? state.repair : null; },
    get visiblePendingPlannedDelete() { return state.pendingFlowVisible ? state.deletion : null; },
    repairConfirmReturnFocusRef: { current: null }, deleteConfirmReturnFocusRef: { current: null },
    repairConfirmCancelRef: { current: { focus() { state.cancelFocused++; } } },
    deleteConfirmCancelRef: { current: { focus() { state.cancelFocused++; } } },
    setPendingPlannedRepair(value) { state.repair = value; context.pendingPlannedRepair = value; },
    setPendingPlannedDelete(value) { state.deletion = value; context.pendingPlannedDelete = value; },
    window: { requestAnimationFrame(callback) { queued.push(callback); }, confirm() { throw Error('Native dialogs forbidden in the test'); } },
    workspaceIntegrityStateLabel: value => value,
    formatCount: String,
    openPlannedPst(...args) { operations.push(['open', ...args]); },
    beginPlannedPstOpen(...args) { operations.push(['reimport', ...args]); },
    deletePlannedWorkspace(...args) { operations.push(['delete', ...args]); },
    cancelDeleteCurrentWorkspace() { operations.push(['cancel-current-delete', context.workspace?.id]); },
    confirmDeleteCurrentWorkspace() { operations.push(['delete-current', context.workspace?.id]); },
    closeRepairWorkspaceConfirmation(id) { operations.push(['cancel-current-repair', id]); },
    confirmRepairWorkspaceIndex(id) { operations.push(['repair-current', id]); },
  };
  const names = ['renderWorkspaceChoice', 'requestPlannedWorkspaceRepair', 'closePlannedWorkspaceRepairConfirmation',
    'requestPlannedWorkspaceDelete', 'closeVisibleDeleteConfirmation', 'confirmVisibleDelete',
    'confirmVisibleRepair', 'confirmPlannedWorkspaceRepair', 'closeVisibleRepairConfirmation'];
  const compiled = ts.transpileModule(names.map(helper).join('\n'), {
    compilerOptions: { target: ts.ScriptTarget.ES2022, jsx: ts.JsxEmit.React },
  }).outputText;
  vm.createContext(context); vm.runInContext(compiled, context);
  const buttons = [];
  function walk(node) {
    if (!node || typeof node !== 'object') return;
    if (node.type === 'button') buttons.push(node);
    for (const child of node.children.flat(Infinity)) walk(child);
  }
  walk(context.renderWorkspaceChoice(plan, existing));
  return { state, operations, origin, context,
    button(label) { return buttons.find(node => node.children.flat(Infinity).includes(label)); },
    flush() { while (queued.length) queued.shift()(); },
  };
}

test('actual incomplete-workspace first clicks only open app dialogs and never invoke a workspace operation', () => {
  const f = fixture();
  const repair = f.button('Repair Index');
  assert.equal(repair.props['data-support-control'], 'click');
  repair.props.onClick({ currentTarget: f.origin }); f.flush();
  assert.equal(f.state.repair.existingWorkspace, existing);
  assert.equal(f.state.cancelFocused, 1);
  assert.deepEqual(f.operations, []);
  f.context.closePlannedWorkspaceRepairConfirmation(); f.flush();
  assert.equal(f.state.repair, null);
  assert.equal(f.state.focusReturns, 1);

  const deletion = f.button('Delete workspace');
  assert.equal(deletion.props['data-support-control'], 'click');
  deletion.props.onClick({ currentTarget: f.origin }); f.flush();
  assert.equal(f.state.deletion.existingWorkspace, existing);
  assert.equal(f.state.cancelFocused, 2);
  assert.deepEqual(f.operations, []);
  f.context.closeVisibleDeleteConfirmation(); f.flush();
  assert.equal(f.state.deletion, null);
  assert.equal(f.state.focusReturns, 2);
  assert.deepEqual(f.operations, []);
});

test('a hidden planned recovery never takes over confirmation or cancellation for another active workspace', () => {
  const f = fixture();
  f.button('Repair Index').props.onClick({ currentTarget: f.origin });
  f.button('Delete workspace').props.onClick({ currentTarget: f.origin });
  f.state.pendingFlowVisible = false;
  f.context.workspace = { id: 'another-fictional-workspace' };
  f.context.confirmVisibleRepair(); f.context.confirmVisibleDelete();
  assert.deepEqual(f.operations, []);
  f.context.repairConfirmOpen = true; f.context.deleteConfirmOpen = true;
  f.context.confirmVisibleRepair(); f.context.confirmVisibleDelete();
  f.context.closeVisibleRepairConfirmation(); f.context.closeVisibleDeleteConfirmation();
  assert.deepEqual(f.operations, [
    ['repair-current', 'another-fictional-workspace'], ['delete-current', 'another-fictional-workspace'],
    ['cancel-current-repair', 'another-fictional-workspace'], ['cancel-current-delete', 'another-fictional-workspace'],
  ]);
  assert.equal(f.state.repair.existingWorkspace, existing);
  assert.equal(f.state.deletion.existingWorkspace, existing);
});

test('incomplete-workspace operations remain unavailable to remote controls', () => {
  const f = fixture();
  for (const label of ['Resume Indexing', 'Reimport from PST']) {
    assert.equal(f.button(label).props['data-support-control'], undefined);
  }
  assert.doesNotMatch(helper('deletePlannedWorkspace'), /window\.confirm/);
  const confirmations = [];
  function walk(node) {
    if (ts.isJsxOpeningElement(node) && node.tagName.getText(tree) === 'button' &&
        node.attributes.properties.some(prop => ts.isJsxAttribute(prop) && prop.name.text === 'data-support-recovery' && prop.initializer?.text === 'confirm-local')) {
      confirmations.push(node.getText(tree));
    }
    ts.forEachChild(node, walk);
  }
  walk(tree);
  assert.equal(confirmations.length, 2);
  for (const confirmation of confirmations) {
    assert.doesNotMatch(confirmation, /data-support-control/);
    assert.match(confirmation, /if \(event\.nativeEvent\.isTrusted\) confirmVisible(?:Repair|Delete)\(\)/);
  }
});
