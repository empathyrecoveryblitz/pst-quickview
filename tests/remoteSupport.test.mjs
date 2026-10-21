import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
import { closeOpenPopover } from '../src/popoverBehavior.ts';
const read = name => readFileSync(new URL('../' + name, import.meta.url), 'utf8');
const panel = read('src/RemoteSupportPanel.tsx');
const control = read('src/RemoteSupportControl.tsx');
const native = read('src-tauri/src/support_session.rs');
const peer = read('public/support/peer.js');
const app = read('src/App.tsx');
const supportCapability = JSON.parse(read('src-tauri/capabilities/support-session.json'));

// Execute the actual TSX approval handler with only React/native boundaries
// stubbed. No mail, browser connection, or native session is created here.
function approvalFixture() {
  const hooks = [];
  let cursor = 0, resolveSession, rejectSession;
  let openPopover = 'help', callbacks = 0;
  const invocations = [], events = [];
  const pending = new Promise((resolve, reject) => { resolveSession = resolve; rejectSession = reject; });
  const react = {
    createElement: (type, props, ...children) => ({ type, props: props || {}, children }),
    useState(initial) {
      const index = cursor++;
      if (!(index in hooks)) hooks[index] = initial;
      return [hooks[index], value => { hooks[index] = typeof value === 'function' ? value(hooks[index]) : value; }];
    },
    useRef(initial) {
      const index = cursor++;
      if (!(index in hooks)) hooks[index] = { current: initial };
      return hooks[index];
    },
  };
  const exports = {};
  const compiled = ts.transpileModule(panel, { compilerOptions: {
    target: ts.ScriptTarget.ES2022, module: ts.ModuleKind.CommonJS, jsx: ts.JsxEmit.React,
  } });
  vm.runInNewContext(compiled.outputText, {
    exports, React: react, Event, Date,
    window: { dispatchEvent(event) { events.push(event.type); } },
    require(name) {
      if (name === 'react') return react;
      if (name === '@tauri-apps/api/core') return { invoke(command, args) { invocations.push({ command, args }); return pending; } };
      if (name === './RemoteSupportControl') return { SUPPORT_SESSION_CHANGED: 'pst-support-session-changed' };
      throw new Error('Unexpected test import');
    },
  });
  function render() {
    cursor = 0;
    return exports.RemoteSupportPanel({ onApproved() {
      callbacks++;
      openPopover = closeOpenPopover(openPopover, 'help');
    } });
  }
  const find = (node, predicate) => {
    if (!node || typeof node !== 'object') return undefined;
    if (predicate(node)) return node;
    return node.children?.flat(Infinity).map(child => find(child, predicate)).find(Boolean);
  };
  const button = () => find(render(), node => node.type === 'button');
  return {
    invocations, events,
    get callbacks() { return callbacks; }, get openPopover() { return openPopover; },
    setPopover(value) { openPopover = value; },
    approve() {
      find(render(), node => node.type === 'textarea').props.onChange({ target: { value: 'fictional invitation' } });
      find(render(), node => node.type === 'input' && node.props.type === 'checkbox').props.onChange({ target: { checked: true } });
    },
    start() { button().props.onClick(); },
    resolve(value = { sessionId: 'a'.repeat(32), expiresAt: Date.now() + 60000 }) { resolveSession(value); },
    reject() { rejectSession(new Error('synthetic failure')); },
    flush: () => new Promise(resolve => setImmediate(resolve)),
  };
}

test('successful local support approval dismisses Help once and exposes the shared app', async () => {
  const fixture = approvalFixture();
  fixture.start();
  assert.equal(fixture.invocations.length, 0);
  fixture.approve(); fixture.start(); fixture.start();
  assert.equal(fixture.invocations.length, 1);
  assert.equal(fixture.openPopover, 'help');
  fixture.resolve(); await fixture.flush();
  assert.equal(fixture.callbacks, 1);
  assert.equal(fixture.openPopover, null);
  assert.deepEqual(fixture.events, ['pst-support-session-changed']);
  assert.match(app, /<RemoteSupportPanel onApproved=\{finishSupportApproval\} \/>/);
  const finish = app.slice(app.indexOf('function finishSupportApproval()'), app.indexOf('async function copyDisplayedError'));
  assert.match(finish, /closeOpenPopover\(current, "help"\)/);
  assert.match(finish, /clearHelpState\(\)/);
});
test('failed, expired, or malformed support startup keeps Help open for recovery', async () => {
  const invalidExpiry = [undefined, null, NaN, Infinity, 'future', '999999999999999', 1, Date.now() + 31 * 60000]
    .map(expiresAt => ({ sessionId: 'a'.repeat(32), expiresAt }));
  for (const response of ['reject', null, { sessionId: 'invalid', expiresAt: Date.now() + 60000 }, { sessionId: ['a'.repeat(32)], expiresAt: Date.now() + 60000 }, ...invalidExpiry]) {
    const fixture = approvalFixture(); fixture.approve(); fixture.start();
    if (response === 'reject') fixture.reject(); else fixture.resolve(response);
    await fixture.flush();
    assert.equal(fixture.callbacks, 0);
    assert.equal(fixture.openPopover, 'help');
    assert.deepEqual(fixture.events, []);
  }
});
test('delayed support approval does not close a different current popover', async () => {
  const fixture = approvalFixture(); fixture.approve(); fixture.start();
  fixture.setPopover('layout'); fixture.resolve(); await fixture.flush();
  assert.equal(fixture.callbacks, 1);
  assert.equal(fixture.openPopover, 'layout');
});

test('session creation requires a deliberate local approval and is never automatic', () => {
  assert.match(panel, /if \(!approved \|\| pending.current/);
  assert.match(panel, /invoke<SupportSession>\("start_support_session", \{ invitation:/);
  assert.match(panel, /onClick=\{\(\) => void start\(\)\}/);
  assert.doesNotMatch(panel, /useEffect|localStorage|sessionStorage|console\.|codex:|open_remote_support_destination/);
  assert.match(panel, /setInvitation\(""\); setApproved\(false\)/);
  assert.match(panel, /including visible messages/);
  assert.match(panel, /not verified until it says Connected/);
});
test('Stop stays outside Help and synchronously revokes the action map first', () => {
  assert.match(app, /<RemoteSupportControl \/>/);
  assert.ok(app.indexOf('<RemoteSupportControl />') < app.indexOf('<main className="app-shell">'));
  assert.match(control, />Stop support<\/button>/);
  const stop = control.slice(control.indexOf('async function stop()'));
  assert.ok(stop.indexOf('view.current?.close()') < stop.indexOf('await invoke("stop_support_session"'));
  assert.match(stop, /revoked.current.add\(id\)/);
  assert.match(control, /revoked.current.has\(next.sessionId\)/);
  assert.match(control, /sessionId === active.current\?\.sessionId/);
});
test('native listener failure and publish/status failure revoke rather than silently resume', () => {
  assert.match(control, /healthy = false; revoke\(active.current\?\.sessionId\)/);
  assert.match(control, /if \(!healthy\) \{ revoke\(next\?\.sessionId\)/);
  assert.match(control, /catch \{ revoke\(active.current\?\.sessionId\)/);
  assert.match(control, /!listenerReady \|\| !healthy/);
  assert.match(control, /document.hidden/);
});
test('networking is in an isolated support window with no message/file/native authority', () => {
  assert.deepEqual(supportCapability.windows, ['support-session-*']);
  assert.equal(supportCapability.remote, undefined);
  assert.deepEqual(new Set(supportCapability.permissions), new Set(['allow-bootstrap-support-session', 'allow-poll-support-frame', 'allow-support-submit-action', 'allow-stop-support-session']));
  assert.match(native, /on_navigation\(move \|url\| local_document/);
  assert.match(native, /NewWindowResponse::Deny/);
  assert.match(native, /\.on_download\(\|_, _\| false\)/);
  assert.match(native, /HEARTBEAT_TIMEOUT: Duration = Duration::from_secs\(5\)/);
  assert.doesNotMatch(native, /Command::new|std::process|\.write\(|fs::write|println!|eprintln!|log::/);
  assert.doesNotMatch(panel + control, /\bfetch\(|WebSocket|RTCPeerConnection|dangerouslySetInnerHTML/);
});
test('only explicit app controls are marked, never exports, destructive actions or support approval', () => {
  assert.doesNotMatch(panel + control, /data-support-control/);
  assert.match(app, /type="search"\s+data-support-control="input"/);
  assert.match(app, /dangerouslySetInnerHTML=\{\{ __html: htmlRender.sanitizedHtml \}\}\s+data-support-untrusted="true"/);
  const snippets = [...app.matchAll(/data-support-control="(?:click|input|scroll)"/g)].map(match => app.slice(Math.max(0, app.lastIndexOf('<', match.index)), app.indexOf('>', match.index)));
  for (const snippet of snippets) assert.doesNotMatch(snippet, /export_|openAttachment|deleteWorkspace|openPst|openStandalone|resetLocalTrash|changeSelectedMessageLocalTrash/);
});
test('support transports no app data through signaling and has no TURN or media access', () => {
  assert.doesNotMatch(peer, /turn:|turns:|getUserMedia|getDisplayMedia|console\.|localStorage|sessionStorage|eval\(/);
  assert.match(peer, /local\.candidateType === 'relay' \|\| remote\.candidateType === 'relay'/);
  assert.match(peer, /transport\?\.dtlsState !== 'connected'/);
  assert.match(peer, /cfg.role === 'guest' && remoteAddress !== cfg.expectedExit/);
  assert.match(peer, /channel\.send\(text\)/);
  assert.match(peer, /shutdown\('stopped'\)/);
});
