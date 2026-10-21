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
const queueProtocol = await import('../public/support/queue-protocol.js');
const sessionSource = read('public/support/session.js');

test('customer request UI is bound to the bundled helper and accepts no invitation or origin', () => {
  assert.match(panel, /invoke<SupportSession>\("request_support_session"\)/);
  assert.match(panel, /invoke<SupportService>\("support_service_status"\)/);
  assert.doesNotMatch(panel, /<textarea|setInvitation|start_support_session/);
  assert.match(panel, /Approve and request support/);
  assert.match(panel, /session.operatorLabel !== service.operatorLabel/);
});

// Execute the actual TSX approval handler with only React/native boundaries
// stubbed. No mail, browser connection, or native session is created here.
function approvalFixture(service = { configured: true, operatorLabel: 'Fictional helper' }) {
  const hooks = [];
  let cursor = 0, resolveSession, rejectSession;
  let openPopover = 'help', callbacks = 0;
  const invocations = [], events = [], stopped = [];
  let serviceReads = 0;
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
    useEffect(effect) {
      const index = cursor++;
      if (!(index in hooks)) { hooks[index] = true; effect(); }
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
      if (name === '@tauri-apps/api/core') return { invoke(command, args) {
        if (command === 'support_service_status') { serviceReads++; return Promise.resolve(service); }
        if (command === 'stop_support_session') { stopped.push(args.sessionId); return Promise.resolve(); }
        if (command !== 'request_support_session') throw new Error('Unexpected native command');
        invocations.push({ command, args }); return pending;
      } };
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
    invocations, events, stopped,
    get serviceReads() { return serviceReads; },
    get callbacks() { return callbacks; }, get openPopover() { return openPopover; },
    setPopover(value) { openPopover = value; },
    approve() {
      find(render(), node => node.type === 'input' && node.props.type === 'checkbox').props.onChange({ target: { checked: true } });
    },
    async load() { render(); await new Promise(resolve => setImmediate(resolve)); },
    start() { button().props.onClick(); },
    resolve(value = { sessionId: 'a'.repeat(32), expiresAt: Date.now() + 60000, operatorLabel: 'Fictional helper' }) { resolveSession(value); },
    reject() { rejectSession(new Error('synthetic failure')); },
    flush: () => new Promise(resolve => setImmediate(resolve)),
  };
}

test('successful local support approval dismisses Help once and exposes the shared app', async () => {
  const fixture = approvalFixture();
  await fixture.load();
  fixture.start();
  assert.equal(fixture.invocations.length, 0);
  fixture.approve(); fixture.start(); fixture.start();
  assert.equal(fixture.invocations.length, 1);
  assert.equal(fixture.invocations[0].command, 'request_support_session');
  assert.equal(fixture.invocations[0].args, undefined);
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
    .map(expiresAt => ({ sessionId: 'a'.repeat(32), expiresAt, operatorLabel: 'Fictional helper' }));
  for (const response of ['reject', null, { sessionId: 'invalid', expiresAt: Date.now() + 60000, operatorLabel: 'Fictional helper' }, { sessionId: ['a'.repeat(32)], expiresAt: Date.now() + 60000, operatorLabel: 'Fictional helper' }, ...invalidExpiry]) {
    const fixture = approvalFixture(); await fixture.load(); fixture.approve(); fixture.start();
    if (response === 'reject') fixture.reject(); else fixture.resolve(response);
    await fixture.flush();
    assert.equal(fixture.callbacks, 0);
    assert.equal(fixture.openPopover, 'help');
    assert.deepEqual(fixture.events, []);
  }
});
test('delayed support approval does not close a different current popover', async () => {
  const fixture = approvalFixture(); await fixture.load(); fixture.approve(); fixture.start();
  fixture.setPopover('layout'); fixture.resolve(); await fixture.flush();
  assert.equal(fixture.callbacks, 1);
  assert.equal(fixture.openPopover, 'layout');
});

test('configuration lookup alone never requests support and an unconfigured build fails closed', async () => {
  for (const service of [{ configured: false, operatorLabel: '' }, { configured: true, operatorLabel: '' }, { configured: true, operatorLabel: 'untrusted\nlabel' }]) {
    const fixture = approvalFixture(service); await fixture.load();
    assert.equal(fixture.serviceReads, 1);
    fixture.approve(); fixture.start(); await fixture.flush();
    assert.equal(fixture.invocations.length, 0);
    assert.equal(fixture.callbacks, 0);
  }
});

test('a native response naming a different helper is revoked before Help completion', async () => {
  const fixture = approvalFixture(); await fixture.load(); fixture.approve(); fixture.start();
  fixture.resolve({ sessionId: 'a'.repeat(32), expiresAt: Date.now() + 60000, operatorLabel: 'Different helper' });
  await fixture.flush();
  assert.equal(fixture.callbacks, 0);
  assert.deepEqual(fixture.stopped, ['a'.repeat(32)]);
  assert.equal(fixture.openPopover, 'help');
});

test('session creation requires a deliberate local approval and is never automatic', () => {
  assert.match(panel, /if \(!approved \|\| pending.current/);
  assert.match(panel, /invoke<SupportSession>\("request_support_session"\)/);
  assert.match(panel, /onClick=\{\(\) => void start\(\)\}/);
  assert.doesNotMatch(panel, /localStorage|sessionStorage|console\.|codex:|open_remote_support_destination/);
  const configurationEffect = panel.slice(panel.indexOf('useEffect(()'), panel.indexOf('async function start()'));
  assert.doesNotMatch(configurationEffect, /request_support_session|start_support_session|start\(/);
  assert.match(panel, /setApproved\(false\)/);
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

async function queueWindowFixture({ claimed = false, wrongSigner = false, holdRegistration = false } = {}) {
  const operator = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign', 'verify']);
  const service = { v: 3, origin: 'https://synthetic.synthetic.workers.dev', operatorLabel: 'Fictional helper',
    operatorPublicKey: queueProtocol.encodeQueueBytes(new Uint8Array(await crypto.subtle.exportKey('raw', operator.publicKey))) };
  const config = { ...service, role: 'guest', expires: Date.now() + 60000 };
  const elements = new Map(), timers = [], intervals = [], events = new Map(), calls = [], peers = [];
  let registration, releaseRegistration, registered, claimedValue, closed = 0, approvals = 0;
  const registering = new Promise(resolve => { registered = resolve; });
  const held = new Promise(resolve => { releaseRegistration = resolve; });
  const element = id => {
    if (!elements.has(id)) elements.set(id, { textContent: '', disabled: false });
    return elements.get(id);
  };
  const response = body => new Response(JSON.stringify(body), { status: 200 });
  const context = {
    console: undefined, crypto, URLSearchParams, Date, JSON, Object, Uint8Array, TextDecoder, AbortController,
    location: { search: '?session=' + 'c'.repeat(32) },
    document: { getElementById: element },
    addEventListener(name, callback) { events.set(name, callback); },
    setTimeout(callback, delay) { const timer = { callback, delay, cleared: false }; timers.push(timer); return timer; },
    clearTimeout(timer) { if (timer) timer.cleared = true; },
    setInterval(callback, delay) { const timer = { callback, delay, cleared: false }; intervals.push(timer); return timer; },
    clearInterval(timer) { if (timer) timer.cleared = true; },
    window: { __TAURI_INTERNALS__: { async invoke(command) {
      calls.push({ category: 'native', command });
      if (command === 'bootstrap_support_session') return config;
      if (command === 'stop_support_session') return;
      if (command === 'poll_support_frame') return null;
      throw new Error('Unexpected native command');
    } } },
    validateServiceConfig(value) { return queueProtocol.validateServiceConfig(JSON.parse(JSON.stringify(value))); },
    decodeQueueBytes: queueProtocol.decodeQueueBytes, createRequest: queueProtocol.createRequest,
    acceptClaim: queueProtocol.acceptClaim, deriveClaimConfig: queueProtocol.deriveClaimConfig,
    async createSupportPeer(options) {
      peers.push(options);
      return { async approve() { approvals++; options.onState('connected'); return true; }, sendFrame() { return true; }, close() { closed++; } };
    },
    async fetch(url, options = {}) {
      assert.ok(String(url).startsWith(service.origin + '/v3/requests'));
      assert.equal(options.credentials, 'omit');
      assert.equal(options.redirect, 'error');
      const pathname = new URL(url).pathname;
      calls.push({ category: 'network', pathname, method: options.method ?? 'GET', keepalive: options.keepalive === true });
      if (pathname === '/v3/requests') {
        registration = JSON.parse(options.body); registered();
        if (holdRegistration) await held;
        return response({ v: 3, id: registration.id, expires: registration.expires });
      }
      if (pathname.endsWith('/stop') || pathname.endsWith('/heartbeat')) return response({ accepted: true });
      if (pathname === '/v3/requests/' + registration.id) {
        if (claimed && !claimedValue) {
          const helper = await queueProtocol.createEphemeralKeyPair();
          const signer = wrongSigner ? await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign', 'verify']) : operator;
          claimedValue = await queueProtocol.createClaim(registration, helper.publicKey, '8.8.8.8', signer.privateKey);
        }
        return response({ v: 3, id: registration.id, expires: registration.expires, claim: claimed ? claimedValue : null });
      }
      throw new Error('Unexpected service route');
    },
  };
  const executable = sessionSource.replace(/^import .*;\n/gm, '');
  const ready = vm.runInNewContext(`(async () => {\n${executable}\n})()`, context);
  return { ready, registering, releaseRegistration, calls, peers, intervals, timers, events,
    get approvals() { return approvals; }, get closed() { return closed; },
    stop() { element('stop').onclick(); },
    get status() { return element('status').textContent; },
    flush: () => new Promise(resolve => setImmediate(resolve)),
  };
}

test('isolated request waits without WebRTC and Stop revokes the queue without automatic restart', async () => {
  const fixture = await queueWindowFixture(); await fixture.ready;
  assert.equal(fixture.peers.length, 0);
  assert.match(fixture.status, /Waiting for your approved helper/);
  assert.ok(fixture.intervals.some(timer => timer.delay === 10000 && !timer.cleared));
  fixture.events.get('pqv-support-revoked')(); await fixture.flush();
  assert.ok(fixture.calls.some(call => call.category === 'network' && call.pathname.endsWith('/stop') && call.keepalive));
  assert.ok(fixture.calls.some(call => call.command === 'stop_support_session'));
  assert.ok(fixture.intervals.every(timer => timer.cleared));
  assert.ok(fixture.timers.every(timer => timer.cleared));
  for (const timer of fixture.timers.filter(timer => timer.delay === 3000)) await timer.callback();
  assert.equal(fixture.peers.length, 0);
});

test('only a signature from the pinned helper starts a direct peer and queue heartbeat remains active', async () => {
  const fixture = await queueWindowFixture({ claimed: true }); await fixture.ready;
  assert.equal(fixture.peers.length, 1);
  assert.equal(fixture.approvals, 1);
  assert.equal(fixture.peers[0].config.role, 'guest');
  assert.equal(fixture.peers[0].config.v, 2);
  assert.equal(fixture.peers[0].queue.origin, 'https://synthetic.synthetic.workers.dev');
  assert.match(fixture.peers[0].queue.requestId, /^[a-f0-9]{32}$/);
  assert.ok(fixture.intervals.some(timer => timer.delay === 10000 && !timer.cleared));
  fixture.stop(); await fixture.flush(); assert.equal(fixture.closed, 1);
  const wrong = await queueWindowFixture({ claimed: true, wrongSigner: true }); await wrong.ready;
  assert.equal(wrong.peers.length, 0);
  assert.equal(wrong.approvals, 0);
  assert.ok(wrong.calls.some(call => call.command === 'stop_support_session'));
});

test('registration finishing after Stop gets a second bounded removal and never connects', async () => {
  const fixture = await queueWindowFixture({ holdRegistration: true }); await fixture.registering;
  fixture.stop(); await fixture.flush();
  fixture.releaseRegistration(); await fixture.ready; await fixture.flush();
  assert.ok(fixture.calls.filter(call => call.category === 'network' && call.pathname.endsWith('/stop')).length >= 2);
  assert.equal(fixture.peers.length, 0);
  assert.equal(fixture.approvals, 0);
  assert.ok(fixture.calls.filter(call => call.command === 'stop_support_session').length === 1);
});
