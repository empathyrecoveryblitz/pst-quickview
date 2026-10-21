import test from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import ts from 'typescript';
import { closeOpenPopover } from '../src/popoverBehavior.ts';
import { SupportQueue } from '../cloudflare/support/worker.mjs';
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
  assert.deepEqual(new Set(supportCapability.permissions), new Set(['allow-bootstrap-support-session', 'allow-poll-support-frame', 'allow-support-submit-action', 'allow-stop-support-session', 'allow-poll-support-update-handoff', 'allow-complete-support-update-handoff', 'allow-support-confirm-update-resume']));
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
      if (command === 'poll_support_update_handoff') return null;
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

// Execute the actual isolated session against the real queue handler and real
// cryptography. Only native IPC, the peer interface, DOM and timers are mocked.
// Every datum is synthetic; no public server, mailbox or native update is used.
const plain = value => JSON.parse(JSON.stringify(value));
const deferred = () => { let resolve; const promise = new Promise(value => { resolve = value; }); return { promise, resolve }; };
async function until(predicate) {
  // Real WebCrypto uses an asynchronous worker pool. Counting immediate turns
  // can exhaust the budget in milliseconds under the full parallel test run.
  // Use a bounded monotonic deadline without changing any production timer.
  const deadline = performance.now() + 5000;
  do {
    if (await predicate()) return;
    await new Promise(resolve => setTimeout(resolve, 1));
  } while (performance.now() < deadline);
  assert.fail('Synthetic session did not reach the expected categorical state');
}
class UpdateQueueStorage {
  constructor() { this.data = new Map(); this.alarm = null; }
  async get(key) { return structuredClone(this.data.get(key)); }
  async put(key, value) { this.data.set(key, structuredClone(value)); }
  async delete(keys) { for (const key of Array.isArray(keys) ? keys : [keys]) this.data.delete(key); }
  async list({ prefix = '', limit = 10000 } = {}) {
    return new Map([...this.data].filter(([key]) => key.startsWith(prefix)).sort(([a], [b]) => a.localeCompare(b)).slice(0, limit).map(([key, value]) => [key, structuredClone(value)]));
  }
  async transaction(operation) { const before = structuredClone(this.data); try { return await operation(this); } catch (error) { this.data = before; throw error; } }
  async getAlarm() { return this.alarm; }
  async setAlarm(value) { this.alarm = value; }
  async deleteAlarm() { this.alarm = null; }
}

async function updateWindowFixture({ resumed = false, holdHandoff = false, holdComplete = false, holdConfirm = false,
  confirmResult = true, completeResult = true, wrongResumeProof = false, expiredResume = false, autoConnect = true, badAck = false,
  executableSource = sessionSource } = {}) {
  const operator = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign', 'verify']);
  const service = { v: 3, origin: 'https://synthetic.synthetic.workers.dev', operatorLabel: 'Fictional helper',
    operatorPublicKey: queueProtocol.encodeQueueBytes(new Uint8Array(await crypto.subtle.exportKey('raw', operator.publicKey))) };
  let now = Date.now();
  class ClockDate extends Date { constructor(...args) { super(...(args.length ? args : [now])); } static now() { return now; } }
  const config = { ...service, role: 'guest', expires: now + 120000 };
  const storage = new UpdateQueueStorage(), env = { SERVICE_ORIGIN: service.origin, OPERATOR_PUBLIC_KEY: service.operatorPublicKey,
    OPERATOR_LABEL: service.operatorLabel, ADMIN_TOKEN: 'a'.repeat(64) };
  const queue = new SupportQueue({ storage }, env, { now: () => now });
  const nativeId = 'c'.repeat(32), resumeAuth = '4'.repeat(64);
  const calls = [], peers = [], packets = [], frames = [], proofChecks = [], peerCreationWasVerified = [], elements = new Map(), timers = [], intervals = [], events = new Map();
  const handoffResponse = deferred(), completeResponse = deferred(), confirmResponse = deferred();
  let registration, original, helperClaim, issued, pendingUpdate, persisted = 0, suspended = 0, closed = 0, approvals = 0;
  const admin = async (path, body) => {
    const response = await queue.fetch(new Request(service.origin + path, { method: body === undefined ? 'GET' : 'POST',
      headers: { Authorization: `Bearer ${env.ADMIN_TOKEN}`, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
      ...(body === undefined ? {} : { body: JSON.stringify(body) }) }));
    assert.equal(response.status, 200); return response.json();
  };
  const post = async (path, token, body) => {
    const response = await queue.fetch(new Request(service.origin + path, { method: 'POST',
      headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json', Origin: 'tauri://localhost' }, body: JSON.stringify(body) }));
    assert.equal(response.status, 200); return response.json();
  };
  if (resumed) {
    const parent = await queueProtocol.createRequest({ expires: config.expires });
    original = parent.request;
    await post('/v3/requests', original.token, original);
    const helper = await queueProtocol.createEphemeralKeyPair();
    const claim = await queueProtocol.createClaim(original, helper.publicKey, '8.8.8.8', operator.privateKey);
    await admin(`/v3/admin/requests/${original.id}/claim`, claim);
    issued = await post(`/v3/requests/${original.id}/handoff`, original.token, { v: 1, parentId: original.id, parentSid: claim.payload.sid,
      nonce: '1'.repeat(32), targetVersion: '0.2.0-beta.4', artifactSha256: '2'.repeat(64), nativeSessionId: nativeId,
      originalExpires: config.expires, resumeDeadline: now + 60000 });
    config.resume = { ...plain(issued), resumeAuth };
    if (expiredResume) config.resume.handoff.resumeDeadline = now - 1;
  }
  const element = id => {
    if (!elements.has(id)) elements.set(id, { textContent: '', disabled: false });
    return elements.get(id);
  };
  const context = {
    console: undefined, crypto, URLSearchParams, Date: ClockDate, JSON, Object, Uint8Array, TextDecoder, AbortController, AbortSignal,
    location: { search: '?session=' + nativeId }, document: { getElementById: element },
    addEventListener(name, callback) { events.set(name, callback); },
    setTimeout(callback, delay) { const timer = { callback, delay, cleared: false }; timers.push(timer); return timer; },
    clearTimeout(timer) { if (timer) timer.cleared = true; },
    setInterval(callback, delay) { const timer = { callback, delay, cleared: false }; intervals.push(timer); return timer; },
    clearInterval(timer) { if (timer) timer.cleared = true; },
    window: { __TAURI_INTERNALS__: { async invoke(command, args) {
      calls.push({ category: 'native', command, args: args && plain(args) });
      if (command === 'bootstrap_support_session') return config;
      if (command === 'stop_support_session') return;
      if (command === 'poll_support_update_handoff') return pendingUpdate ?? null;
      if (command === 'poll_support_frame') return { id: 'e'.repeat(32) };
      if (command === 'complete_support_update_handoff') {
        assert.equal(args.sessionId, nativeId);
        assert.equal(await queueProtocol.queueTokenSha256(args.resumeTicket), args.handoff.resumeTicketSha256);
        assert.equal(args.proof, await queueProtocol.ackProof(resumeAuth, plain(args.handoff)));
        persisted++;
        if (holdComplete) await completeResponse.promise;
        return completeResult;
      }
      if (command === 'support_confirm_update_resume') {
        assert.equal(args.sessionId, nativeId);
        if (holdConfirm) await confirmResponse.promise;
        return confirmResult;
      }
      throw new Error('Unexpected native command');
    } } },
    validateServiceConfig: value => queueProtocol.validateServiceConfig(plain(value)),
    validateUpdateHandoff: (value, options = {}) => queueProtocol.validateUpdateHandoff(plain(value), { ...options, now }),
    decodeQueueBytes: queueProtocol.decodeQueueBytes, createRequest: queueProtocol.createRequest,
    createResumedRequest: value => queueProtocol.createResumedRequest(plain(value)),
    acceptClaim: queueProtocol.acceptClaim, deriveClaimConfig: queueProtocol.deriveClaimConfig,
    queueTokenSha256: queueProtocol.queueTokenSha256,
    ackProof: (secret, handoff) => queueProtocol.ackProof(secret, plain(handoff)),
    resumedGuestProof: (secret, handoff, key, hash) => queueProtocol.resumedGuestProof(secret, plain(handoff), key, hash),
    resumedClaimProof: (secret, handoff, claim) => queueProtocol.resumedClaimProof(secret, plain(handoff), claim),
    verifyUpdateProof(expected, actual) { const accepted = queueProtocol.verifyUpdateProof(expected, actual); proofChecks.push(accepted); return accepted; },
    async createSupportPeer(options) {
      peerCreationWasVerified.push(!resumed || proofChecks.includes(true));
      peers.push(options);
      return {
        async approve() { approvals++; if (autoConnect) options.onState('connected'); return true; },
        sendFrame(frame) { frames.push(frame); return true; }, close() { closed++; },
        sendUpdateHandoff(packet) {
          packets.push(plain(packet));
          void queueProtocol.ackProof(packet.resumeAuth, plain(packet.handoff)).then(proof => options.onUpdateHandoffAck({
            nonce: packet.handoff.nonce, proof: badAck ? 'f'.repeat(64) : proof,
          }));
          return true;
        },
        suspendForUpdate() { suspended++; return true; },
      };
    },
    async fetch(url, options = {}) {
      assert.ok(String(url).startsWith(service.origin + '/v3/requests'));
      assert.equal(options.credentials, 'omit'); assert.equal(options.redirect, 'error');
      const pathname = new URL(url).pathname, body = options.body === undefined ? undefined : JSON.parse(options.body);
      const token = options.headers.Authorization.slice(7);
      const call = { category: 'network', pathname, method: options.method ?? 'GET', token, body, keepalive: options.keepalive === true };
      calls.push(call);
      if (pathname === '/v3/requests' || pathname.endsWith('/resume')) registration = body;
      const record = await storage.get(`request:${registration?.id ?? original?.id}`);
      if (options.method === 'GET' && record && !record.claim && (!record.handoff || record.handoffState === 'resuming')) {
        const helper = await queueProtocol.createEphemeralKeyPair();
        helperClaim = await queueProtocol.createClaim({ v: 3, id: record.id, guestPublicKey: record.guestPublicKey, expires: record.expires }, helper.publicKey, '8.8.8.8', operator.privateKey);
        if (record.handoff) helperClaim.resumeProof = await queueProtocol.resumedClaimProof(wrongResumeProof ? '5'.repeat(64) : resumeAuth, record.handoff, helperClaim);
        await admin(`/v3/admin/requests/${record.id}/claim`, helperClaim);
      }
      const response = await queue.fetch(new Request(String(url), { method: options.method ?? 'GET', headers: {
        ...options.headers, Origin: 'tauri://localhost',
      }, ...(options.body === undefined ? {} : { body: options.body }) }));
      call.status = response.status;
      if (pathname.endsWith('/handoff') && response.ok) {
        issued = await response.clone().json();
        if (holdHandoff) await handoffResponse.promise;
      }
      return response;
    },
  };
  const executable = executableSource.replace(/^import .*;\n/gm, '');
  const ready = vm.runInNewContext(`(async () => {\n${executable}\n})()`, context);
  const fixture = { ready, calls, peers, packets, frames, proofChecks, peerCreationWasVerified, storage, timers, intervals, events,
    get persisted() { return persisted; }, get suspended() { return suspended; }, get closed() { return closed; }, get approvals() { return approvals; },
    get status() { return element('status').textContent; }, get registration() { return registration; }, get original() { return original; },
    get issued() { return issued; }, get claim() { return helperClaim; }, get config() { return config; },
    stop() { element('stop').onclick(); }, event(name) { events.get(name)?.(); },
    releaseHandoff() { handoffResponse.resolve(); }, releaseComplete() { completeResponse.resolve(); }, releaseConfirm() { confirmResponse.resolve(); },
    advance(value) { now += value; },
    async poll() {
      await new Promise(resolve => setImmediate(resolve));
      const timer = timers.findLast(timer => timer.delay === 500 && !timer.cleared);
      assert.ok(timer); timer.cleared = true; timer.callback();
      await new Promise(resolve => setImmediate(resolve));
    },
    async beginHandoff() {
      await ready;
      pendingUpdate = { v: 1, nonce: '1'.repeat(32), targetVersion: '0.2.0-beta.4', artifactSha256: '2'.repeat(64), nativeSessionId: nativeId,
        originalExpires: config.expires, resumeDeadline: now + 60000, resumeAuth };
      await fixture.poll();
    },
    async cleanup() {
      fixture.stop(); handoffResponse.resolve(); completeResponse.resolve(); confirmResponse.resolve();
      await ready; await new Promise(resolve => setImmediate(resolve));
    },
  };
  return fixture;
}

async function successfulHandoffScenario(source = sessionSource, earlyRestart = false) {
  const f = await updateWindowFixture({ executableSource: source, holdComplete: earlyRestart });
  try {
    await f.beginHandoff(); await until(() => f.persisted === 1);
    assert.equal(f.packets.length, 1);
    assert.equal(f.packets[0].handoff.parentSid, f.peers[0].config.sid);
    assert.equal(f.proofChecks.includes(true), true);
    if (earlyRestart) { f.event('pqv-support-update-restart'); f.event('pagehide'); f.releaseComplete(); }
    await until(() => f.suspended === 1 || f.calls.some(call => call.command === 'stop_support_session'));
    assert.equal(f.suspended, 1);
    f.event('pqv-support-update-restart'); f.event('pagehide');
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(f.suspended, 1);
    assert.equal(f.calls.some(call => call.command === 'stop_support_session'), false);
    assert.equal(f.calls.some(call => call.pathname?.endsWith('/stop')), false);
    assert.equal((await f.storage.get(`request:${f.registration.id}`)).handoffState, 'updating');
    assert.match(f.status, /Sharing and control are paused/);
  } finally { await f.cleanup(); }
}
test('actual session persists an acknowledged handoff and preserves it across pagehide', async () => {
  await successfulHandoffScenario();
});
test('native restart before a held completion reply suspends idempotently without revoking the grant', async () => {
  await successfulHandoffScenario(sessionSource, true);
});

async function stoppedHandoffScenario(source = sessionSource) {
  const f = await updateWindowFixture({ executableSource: source, holdHandoff: true });
  try {
    await f.beginHandoff(); await until(() => f.issued !== undefined);
    assert.equal((await f.storage.get(`request:${f.registration.id}`)).handoffState, 'updating');
    f.stop(); await until(() => f.calls.some(call => call.command === 'stop_support_session'));
    await until(() => f.calls.some(call => call.pathname?.endsWith('/handoff/stop') && call.status !== undefined));
    assert.equal(Boolean(await f.storage.get(`request:${f.registration.id}`)), false);
    assert.ok(f.calls.some(call => call.pathname?.endsWith('/handoff/stop') && call.token === f.registration.token));
    f.releaseHandoff();
    await until(() => f.calls.some(call => call.pathname?.endsWith('/handoff/stop') && call.token === f.issued.resumeTicket));
    assert.equal(f.persisted, 0); assert.equal(f.packets.length, 0); assert.equal(f.suspended, 0);
    assert.equal(Boolean(await f.storage.get(`request:${f.registration.id}`)), false);
  } finally { await f.cleanup(); }
}
test('Stop during a held handoff reply removes the pending grant and cleans the late ticket', async () => {
  await stoppedHandoffScenario();
});

test('bad handoff acknowledgement or failed native persistence revokes instead of restarting support', async () => {
  for (const options of [{ badAck: true }, { completeResult: false }]) {
    const f = await updateWindowFixture(options);
    try {
      await f.beginHandoff(); await until(() => f.calls.some(call => call.command === 'stop_support_session'));
      await until(async () => !(await f.storage.get(`request:${f.registration.id}`)));
      assert.equal(f.persisted, options.badAck ? 0 : 1);
      assert.equal(f.suspended, 0);
      assert.equal(f.status.startsWith('Connected.'), false);
      assert.ok(f.intervals.every(timer => timer.cleared));
    } finally { await f.cleanup(); }
  }
});

test('resumed actual session rotates guest credentials and verifies helper continuity before creating a peer', async () => {
  const f = await updateWindowFixture({ resumed: true, holdConfirm: true });
  try {
    await f.ready; await until(() => f.calls.some(call => call.command === 'support_confirm_update_resume'));
    assert.equal(f.peers.length, 1); assert.equal(f.approvals, 1); assert.deepEqual(f.proofChecks, [true]);
    assert.deepEqual(f.peerCreationWasVerified, [true]);
    assert.equal(f.registration.id, undefined); // Resume wire has no authority to choose another request ID.
    assert.notEqual(f.registration.token, f.original.token);
    assert.notEqual(f.registration.guestPublicKey, f.original.guestPublicKey);
    assert.equal(f.peers[0].queue.requestId, f.original.id);
    assert.notEqual(f.peers[0].config.sid, f.issued.handoff.parentSid);
    assert.equal(f.status.startsWith('Connected.'), false); assert.equal(f.frames.length, 0);
    f.releaseConfirm(); await until(() => f.status.startsWith('Connected.'));
    await f.poll(); assert.equal(f.frames.length, 1);
    const before = f.calls.length; f.stop(); await until(() => f.calls.some(call => call.command === 'stop_support_session'));
    assert.equal(f.calls.slice(before).some(call => call.pathname?.endsWith('/handoff/stop')), false);
    await until(async () => !(await f.storage.get(`request:${f.original.id}`)));
    assert.equal(Boolean(await f.storage.get(`request:${f.original.id}`)), false);
  } finally { await f.cleanup(); }
});

async function rejectedResumeScenario(options = {}, source = sessionSource) {
    const f = await updateWindowFixture({ resumed: true, ...options, executableSource: source });
    try {
      await f.ready;
      assert.equal(f.peers.length, 0); assert.equal(f.approvals, 0); assert.equal(f.frames.length, 0);
      assert.equal(f.calls.some(call => call.command === 'support_confirm_update_resume'), false);
      assert.ok(f.calls.some(call => call.command === 'stop_support_session'));
      await until(async () => !(await f.storage.get(`request:${f.original.id}`)));
      assert.equal(Boolean(await f.storage.get(`request:${f.original.id}`)), false);
    } finally { await f.cleanup(); }
}
test('invalid or expired helper continuity never creates a peer or resumes sharing', async () => {
  for (const options of [{ wrongResumeProof: true }, { expiredResume: true }]) await rejectedResumeScenario(options);
});

async function failedConfirmationScenario(mode, source = sessionSource) {
    const f = await updateWindowFixture({ resumed: true, holdConfirm: mode !== 'rejected', confirmResult: mode !== 'rejected', executableSource: source });
    try {
      await f.ready; await until(() => f.calls.some(call => call.command === 'support_confirm_update_resume'));
      if (mode === 'stop') f.stop();
      if (mode === 'deadline') {
        f.advance(60001);
        const timer = f.timers.find(timer => timer.delay > 10000 && timer.delay <= 60000 && !timer.cleared);
        assert.ok(timer); timer.callback();
      }
      f.releaseConfirm(); await until(() => f.calls.some(call => call.command === 'stop_support_session'));
      f.peers[0].onState('connected');
      await new Promise(resolve => setImmediate(resolve));
      assert.equal(f.status.startsWith('Connected.'), false); assert.equal(f.frames.length, 0);
      assert.ok(f.intervals.every(timer => timer.cleared));
    } finally { await f.cleanup(); }
}
test('Stop, the continuation deadline, or failed native confirmation cannot silently enable sharing', async () => {
  for (const mode of ['stop', 'deadline', 'rejected']) await failedConfirmationScenario(mode);
});

test('known regression mutants are rejected by actual-session handoff and continuity scenarios', async () => {
  const noAttemptStop = sessionSource.replace('if (service && handoffAttempt) bestEffortHandoffStop(service.origin, handoffAttempt.parentId, handoffAttempt.nonce, handoffAttempt.token);', '/* synthetic mutant: missing in-flight grant Stop */');
  const noRestartEvent = sessionSource.replace("addEventListener('pqv-support-update-restart', () => { if (!suspendApprovedUpdate()) void stop('failed'); });", '/* synthetic mutant: missing native restart event */');
  assert.notEqual(noAttemptStop, sessionSource); assert.notEqual(noRestartEvent, sessionSource);
  await assert.rejects(stoppedHandoffScenario(noAttemptStop), { name: 'AssertionError' });
  await assert.rejects(successfulHandoffScenario(noRestartEvent, true), { name: 'AssertionError' });
  const noProofCheck = sessionSource.replace("if (!verifyUpdateProof(proof, result.claim.resumeProof)) throw Error('continuity');", '/* synthetic mutant: unchecked continuity proof */');
  assert.notEqual(noProofCheck, sessionSource);
  await assert.rejects(rejectedResumeScenario({ wrongResumeProof: true }, noProofCheck), { name: 'AssertionError' });
  const noConfirmationGate = sessionSource.replace('if (ending || confirmed !== true || !resumedGrant || Date.now() >= resumedGrant.handoff.resumeDeadline)',
    'if (ending || !resumedGrant || Date.now() >= resumedGrant.handoff.resumeDeadline)');
  const noResumeTimer = sessionSource.replace("resumeTimer = setTimeout(() => { if (resumedGrant) void stop('expired'); }, Math.max(1, handoff.resumeDeadline - Date.now()));", '/* synthetic mutant: missing continuation deadline */');
  const noLateTicketCleanup = sessionSource.replace("if (/^[a-f0-9]{64}$/.test(issued?.resumeTicket || '')) bestEffortHandoffStop(service.origin, raw.parentId, raw.nonce, issued.resumeTicket);", '/* synthetic mutant: missing late ticket cleanup */');
  for (const mutant of [noConfirmationGate, noResumeTimer, noLateTicketCleanup]) assert.notEqual(mutant, sessionSource);
  await assert.rejects(failedConfirmationScenario('rejected', noConfirmationGate), { name: 'AssertionError' });
  await assert.rejects(failedConfirmationScenario('deadline', noResumeTimer), { name: 'AssertionError' });
  await assert.rejects(stoppedHandoffScenario(noLateTicketCleanup), { name: 'AssertionError' });
});
