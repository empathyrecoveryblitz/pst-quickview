import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
const source = readFileSync(new URL('../src/DevelopmentUpdatePanel.tsx', import.meta.url), 'utf8');
const control = readFileSync(new URL('../src/RemoteSupportControl.tsx', import.meta.url), 'utf8');
function fixture({ activeSupport = true } = {}) {
  const hooks = [], calls = [], responses = []; let cursor = 0;
  const react = { createElement: (type, props, ...children) => ({ type, props: props || {}, children }),
    useState(initial) { const index = cursor++; if (!(index in hooks)) hooks[index] = initial; return [hooks[index], value => { hooks[index] = value; }]; },
    useRef(initial) { const index = cursor++; if (!(index in hooks)) hooks[index] = { current: initial }; return hooks[index]; } };
  const exports = {};
  vm.runInNewContext(ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.React } }).outputText,
    { exports, React: react, Date, require(name) { if (name === 'react') return react; if (name === '@tauri-apps/api/core') return { invoke(command, args) { calls.push({ command, args }); if (command === 'support_session_status') return Promise.resolve(activeSupport ? { sessionId: 'b'.repeat(32), expiresAt: Date.now() + 60000, updating: false } : null); const response = responses.shift(); return response instanceof Error ? Promise.reject(response) : Promise.resolve(response); } }; throw Error('unexpected import'); } });
  const render = () => { cursor = 0; return exports.DevelopmentUpdatePanel(); };
  const find = (node, type) => !node || typeof node !== 'object' ? undefined : node.type === type ? node : node.children?.flat(Infinity).map(child => find(child, type)).find(Boolean);
  const buttons = node => !node || typeof node !== 'object' ? [] : [...(node.type === 'button' ? [node] : []), ...(node.children?.flat(Infinity).flatMap(buttons) || [])];
  return { calls, responses, render, check: () => buttons(render())[0].props.onClick(), install: () => buttons(render())[1]?.props.onClick(),
    resumeInput: () => find(render(), 'input'),
    enableResume: () => find(render(), 'input').props.onChange({ target: { checked: true } }),
    disableResume: () => find(render(), 'input').props.onChange({ target: { checked: false } }), flush: () => new Promise(resolve => setImmediate(resolve)) };
}
test('update UI performs no automatic check and never exposes remotely actionable approval controls', () => {
  const f = fixture(); f.render(); assert.equal(f.calls.length, 0);
  assert.match(source, /data-support-private="true"/); assert.doesNotMatch(source, /data-support-control|fetch\(|localStorage|sessionStorage|useEffect|console\./);
  assert.match(source, /"approve_verified_update", \{ pendingId: update.pendingId, resume \}/);
  assert.doesNotMatch(source, /invoke\([^\n]*(?:url:|signature:|version:|path:)/);
});
test('only a valid verified candidate and a second deliberate click can request native approval', async () => {
  const f = fixture(); f.responses.push({ available: true, pendingId: 'a'.repeat(32), version: '0.2.0-beta.3.support.2' });
  f.check(); f.check(); await f.flush(); assert.equal(f.calls.filter(call => call.command === 'check_verified_update').length, 1); assert.equal(f.calls[0].command, 'check_verified_update');
  f.responses.push({ available: false }); f.install(); await f.flush();
  assert.equal(f.calls[2].command, 'approve_verified_update'); assert.deepEqual(JSON.parse(JSON.stringify(f.calls[2].args)), { pendingId: 'a'.repeat(32), resume: false });
});
test('malformed or failed check never grants install authority and the owner can explicitly decline resume', async () => {
  for (const candidate of [null, { available: true, pendingId: '../wrong', version: '1.0.0' }, { available: true, pendingId: 'a'.repeat(32), version: '<untrusted>' }, new Error('synthetic')]) {
    const f = fixture(); f.responses.push(candidate); f.check(); await f.flush(); f.install(); assert.equal(f.calls.length, 1);
  }
  const f = fixture(); f.responses.push({ available: true, pendingId: 'a'.repeat(32), version: '0.2.0-beta.3.support.2' }); f.check(); await f.flush();
  f.disableResume(); f.responses.push({ available: false }); f.install(); await f.flush(); assert.equal(f.calls[2].args.resume, false);
});
test('ordinary updates default to no resumption when no support session is active', async () => {
  const f = fixture({ activeSupport: false }); f.responses.push({ available: true, pendingId: 'a'.repeat(32), version: '0.2.0-beta.3.support.2' });
  f.check(); await f.flush(); f.responses.push({ available: false }); f.install(); await f.flush();
  assert.equal(f.calls.find(call => call.command === 'approve_verified_update').args.resume, false);
});
test('an unexpired waiting request never silently selects continuation or blocks the ordinary update path', async () => {
  const f = fixture({ activeSupport: true });
  f.responses.push({ available: true, pendingId: 'a'.repeat(32), version: '0.2.0-beta.3.support.2' });
  f.check(); await f.flush();
  assert.equal(f.resumeInput().props.checked, false);
  assert.match(source, /only if the separate support window says Connected/);
  assert.match(source, /Waiting requests cannot resume/);
  f.responses.push({ available: false }); f.install(); await f.flush();
  assert.equal(f.calls.find(call => call.command === 'approve_verified_update').args.resume, false);
});
test('continuation requires a deliberate additional opt-in and resets for each verified candidate', async () => {
  const f = fixture({ activeSupport: true });
  f.responses.push({ available: true, pendingId: 'a'.repeat(32), version: '0.2.0-beta.3.support.2' });
  f.check(); await f.flush(); assert.equal(f.resumeInput().props.checked, false);
  f.enableResume(); assert.equal(f.resumeInput().props.checked, true);
  f.responses.push({ available: true, pendingId: 'c'.repeat(32), version: '0.2.0-beta.3.support.3' });
  f.check(); await f.flush(); assert.equal(f.resumeInput().props.checked, false);
  f.enableResume(); f.responses.push({ available: false }); f.install(); await f.flush();
  const approval = f.calls.find(call => call.command === 'approve_verified_update');
  assert.equal(approval.args.pendingId, 'c'.repeat(32)); assert.equal(approval.args.resume, true);
});
test('resume bootstrap mounts Stop first, suppresses paused capture and fences late completion after Stop', () => {
  assert.match(control, /support_resume_status/); assert.match(control, /resume_support_after_update/);
  assert.match(control, /if \(!resumePending \|\| resumeStarted.current\) return/);
  assert.match(control, /generation !== resumeGeneration.current/);
  assert.match(control, /current.updating \|\| !capture/);
  assert.match(control, /view.current = next.updating \? null : createSupportView/);
  assert.match(control, /active.current\?\.sessionId \|\| resumeSessionId.current/);
  assert.match(control, /session \|\| resumePending/); assert.match(control, /Stop support<\/button>/);
});
