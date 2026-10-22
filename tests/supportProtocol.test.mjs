import test from 'node:test';
import assert from 'node:assert/strict';
import { webcrypto } from 'node:crypto';

globalThis.crypto ??= webcrypto;
const { publicIPv4, sanitizeCandidate, validateCandidate, validateDescription, createCipher, validateConfig, validateFrame, validateAction, validateDataMessage } = await import('../public/support/protocol.js');
const key = Buffer.alloc(32, 0x5a).toString('base64url');
const sid = '0123456789abcdef0123456789abcdef';
const exit = '8.8.8.8';
const rawCandidate = `candidate:123 1 UDP 1686052607 ${exit} 40000 typ srflx raddr 192.168.1.9 rport 50000 generation 0 network-cost 999`;
const description = setup => [
  'v=0', 'o=- 123456789 2 IN IP4 127.0.0.1', 's=-', 't=0 0',
  'a=group:BUNDLE 0', 'a=extmap-allow-mixed', 'a=msid-semantic: WMS',
  'm=application 9 UDP/DTLS/SCTP webrtc-datachannel', 'c=IN IP4 0.0.0.0',
  'a=ice-ufrag:abcd', 'a=ice-pwd:abcdefghijklmnopqrstuv', 'a=ice-options:trickle',
  `a=fingerprint:sha-256 ${Array(32).fill('AB').join(':')}`, `a=setup:${setup}`,
  'a=mid:0', 'a=sctp-port:5000', 'a=max-message-size:262144',
].join('\r\n') + '\r\n';
const pair = async (overrides = {}) => {
  const options = { key, sid, expires: Date.now() + 60000, ...overrides };
  return Promise.all([
    createCipher({ ...options, role: 'controller' }),
    createCipher({ ...options, role: 'guest' }),
  ]);
};

async function within(promise, label, describe = () => '') {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error(`Synthetic peer timed out: ${label}. ${describe()}`)), 5000);
    })]);
  } finally { clearTimeout(timer); }
}

test('only canonical public IPv4 UDP srflx candidates leave the browser', () => {
  const clean = sanitizeCandidate(rawCandidate, { role: 'controller', expectedExit: exit });
  assert.deepEqual(clean, {
    candidate: `candidate:123 1 udp 1686052607 ${exit} 40000 typ srflx raddr 0.0.0.0 rport 0`,
    sdpMid: '0', sdpMLineIndex: 0,
  });
  assert.deepEqual(validateCandidate(clean, { sourceRole: 'controller', expectedExit: exit }), clean);
  for (const address of ['0.0.0.0', '10.1.2.3', '127.0.0.1', '100.64.0.1', '169.254.1.1', '172.16.0.1', '192.168.1.1', '192.0.2.1', '198.18.0.1', '198.51.100.1', '203.0.113.1', '224.0.0.1', '255.255.255.255', '08.8.8.8', '256.1.1.1', '::1']) {
    assert.equal(publicIPv4(address), false);
    assert.equal(sanitizeCandidate(rawCandidate.replace(exit, address), { role: 'guest' }), null);
  }
  for (const modification of [
    text => text.replace(' srflx ', ' relay '), text => text.replace(' srflx ', ' host '),
    text => text.replace(' 1 UDP ', ' 2 UDP '), text => text.replace(' UDP ', ' TCP '),
    text => text.replace(' 40000 ', ' 65536 '), text => `${text}\r\na=candidate:injection`,
  ]) assert.equal(sanitizeCandidate(modification(rawCandidate), { role: 'guest' }), null);
  assert.equal(sanitizeCandidate(rawCandidate, { role: 'controller', expectedExit: '1.1.1.1' }), null);
  assert.throws(() => validateCandidate({ ...clean, usernameFragment: 'abcd' }, { sourceRole: 'guest' }));
  assert.throws(() => validateCandidate({ ...clean, candidate: rawCandidate }, { sourceRole: 'guest' }));
  assert.throws(() => validateCandidate(clean, { sourceRole: 'controller', expectedExit: '1.1.1.1' }));
});

test('description validation preserves initial data-only SDP and requires trickle', () => {
  const offer = description('actpass');
  assert.deepEqual(validateDescription(offer, 'offer'), { type: 'offer', sdp: offer });
  assert.equal(validateDescription(description('active'), 'answer').type, 'answer');
  for (const rejected of [
    offer.replace('webrtc-datachannel', 'audio'),
    `${offer}m=video 9 UDP/TLS/RTP/SAVPF 96\r\n`,
    offer.replace('a=ice-options:trickle\r\n', ''),
    offer.replace('a=ice-options:trickle', 'a=ice-options:renomination'),
    offer.replace('a=fingerprint:sha-256', 'a=fingerprint:sha-1'),
    offer.replace('c=IN IP4 0.0.0.0', 'c=IN IP4 192.168.1.2'),
    offer.replace('IN IP4 127.0.0.1', 'IN IP4 8.8.8.8'),
    `${offer}a=candidate:123 1 udp 1 8.8.8.8 1 typ srflx\r\n`,
    `${offer}a=end-of-candidates\r\n`, `${offer}a=msid:/private/example\r\n`,
    offer.replace('v=0\r\n', 'v=0\n'),
  ]) assert.throws(() => validateDescription(rejected, 'offer'));
});

test('cipher roundtrips both directions, queues concurrent sends, and rejects replay', async () => {
  const [controller, guest] = await pair();
  const messages = await Promise.all([
    controller.seal('approved', { accepted: true }),
    controller.seal('offer', validateDescription(description('actpass'), 'offer')),
  ]);
  assert.deepEqual(messages.map(message => message.seq), [1, 2]);
  assert.deepEqual(await guest.open(messages[0]), { kind: 'approved', data: { accepted: true } });
  assert.equal((await guest.open(messages[1])).kind, 'offer');
  const answer = await guest.seal('answer', validateDescription(description('active'), 'answer'));
  assert.equal((await controller.open(answer)).kind, 'answer');
  await assert.rejects(guest.open(messages[0]));
  const next = await controller.seal('end', null);
  const concurrent = await Promise.allSettled([guest.open(next), guest.open(next)]);
  assert.deepEqual(concurrent.map(result => result.status), ['fulfilled', 'rejected']);
});

test('cipher rejects tampering, wrong direction, session, expiry and skipped sequence', async () => {
  const [controller, guest] = await pair();
  const message = await controller.seal('diagnostic', { attempts: 1 });
  const changed = message.ct[0] === 'A' ? 'B' : 'A';
  for (const invalid of [
    { ...message, ct: changed + message.ct.slice(1) },
    { ...message, from: 'guest' }, { ...message, sid: 'f'.repeat(32) },
    { ...message, expires: message.expires + 1 }, { ...message, seq: 2 },
    { ...message, iv: message.iv + '=' }, { ...message, ct: message.ct + '=' },
    { ...message, extra: true }, { ...message, v: '1' },
  ]) await assert.rejects(guest.open(invalid));
  await assert.rejects(controller.open(message));
  const wrongSession = await createCipher({ key, sid: 'f'.repeat(32), role: 'guest', expires: message.expires });
  await assert.rejects(wrongSession.open(message));
  const wrongKey = await createCipher({ key: Buffer.alloc(32, 1).toString('base64url'), sid, role: 'guest', expires: message.expires });
  await assert.rejects(wrongKey.open(message));
  assert.equal((await guest.open(message)).kind, 'diagnostic');
  await assert.rejects(createCipher({ key, sid, role: 'guest', expires: Date.now() - 1 }));
});

test('cipher enforces expiry on both operations and bounded payloads and sequences', async () => {
  const [controller, guest] = await pair();
  await assert.rejects(controller.seal('unknown', null));
  await assert.rejects(controller.seal('diagnostic', { invalid: undefined }));
  await assert.rejects(controller.seal('diagnostic', 'a'.repeat(14001)));
  await assert.rejects(controller.seal('diagnostic', { first: 'a'.repeat(14000), second: 'b'.repeat(14000) }));
  const cycle = {}; cycle.self = cycle;
  await assert.rejects(controller.seal('diagnostic', cycle));
  for (let i = 1; i <= 128; i++) {
    const message = await controller.seal('candidate', { number: i });
    assert.equal(message.seq, i);
    assert.equal((await guest.open(message)).data.number, i);
  }
  await assert.rejects(controller.seal('stop', null));
  const [freshController, freshGuest] = await pair();
  const pending = await freshController.seal('stop', null);
  const originalNow = Date.now;
  try {
    Date.now = () => pending.expires;
    await assert.rejects(freshController.seal('stop', null), /expired/);
    await assert.rejects(freshGuest.open(pending), /expired/);
  } finally { Date.now = originalNow; }
});

const config = role => ({
  v: 2, sid, key, token: 'a'.repeat(64), expires: Date.now() + 60000, expectedExit: exit, role,
  origin: role === 'controller' ? 'http://127.0.0.1:43210' : 'https://example.run.pinggy-free.link',
});
const frame = () => ({
  v: 1, id: '1'.repeat(32), width: 1400, height: 900, truncated: false,
  items: [
    { kind: 'box', x: 0, y: 0, w: 1400, h: 900, fill: '#fff' },
    { kind: 'text', x: 10, y: 10, w: 100, h: 20, text: 'Synthetic label', color: 'rgb(10, 20, 30)', size: 14, weight: 400 },
  ],
  targets: [{ id: '2'.repeat(32), x: 20, y: 40, w: 100, h: 30, kind: 'input', label: 'Synthetic input', value: '' }],
});
const action = kind => ({ frame: '1'.repeat(32), target: '2'.repeat(32), kind,
  ...(kind === 'input' ? { value: 'Synthetic value' } : kind === 'scroll' ? { deltaY: 300 } : {}),
});

test('support invitations restrict origin, role, key and session lifetime', () => {
  assert.equal(validateConfig(config('guest')).origin, 'https://example.run.pinggy-free.link');
  assert.equal(validateConfig(config('controller')).origin, 'http://127.0.0.1:43210');
  for (const invalid of [
    { ...config('guest'), origin: 'http://127.0.0.1:43210' },
    { ...config('guest'), origin: 'https://example.run.pinggy-free.link.attacker.invalid' },
    { ...config('guest'), origin: 'https://nested.example.run.pinggy-free.link' },
    { ...config('guest'), origin: 'https://example.run.pinggy-free.link/path' },
    { ...config('controller'), origin: 'http://localhost:43210' },
    { ...config('controller'), origin: 'http://127.0.0.1:80' },
    { ...config('controller'), origin: 'http://127.0.0.1:65536' },
    { ...config('guest'), key: key.slice(0, -1) + '=' },
    { ...config('guest'), expires: Date.now() + 31 * 60 * 1000 },
    { ...config('guest'), expectedExit: '192.168.1.2' },
  ]) assert.throws(() => validateConfig(invalid));
});

test('queue transport accepts only the exact explicitly pinned Worker origin', () => {
  const origin = 'https://support.synthetic.workers.dev';
  const value = { ...config('guest'), origin };
  assert.throws(() => validateConfig(value));
  assert.equal(validateConfig(value, { serviceOrigin: origin }).origin, origin);
  for (const candidate of [origin + '/', origin + '/path', origin + ':443',
    'http://support.synthetic.workers.dev',
    'https://support.synthetic.workers.dev.attacker.invalid', 'https://workers.dev',
    'https://*.workers.dev', 'https://user:pass@support.synthetic.workers.dev']) {
    assert.throws(() => validateConfig({ ...value, origin: candidate }, { serviceOrigin: origin }));
    assert.throws(() => validateConfig({ ...value, origin: candidate }, { serviceOrigin: candidate }));
  }
  assert.throws(() => validateConfig({ ...value, origin: 'https://other.synthetic.workers.dev' }, { serviceOrigin: origin }));
  assert.throws(() => validateConfig(config('guest'), { serviceOrigin: origin }));
});

test('queue peers route only encrypted setup to a bound request, without the legacy tunnel header', async () => {
  const previousRTC = globalThis.RTCPeerConnection, previousFetch = globalThis.fetch;
  const origin = 'https://support.synthetic.workers.dev', id = 'c'.repeat(32);
  const value = { ...config('guest'), origin };
  let peer;
  const urls = [];
  const envelopes = [];
  let signalStop;
  const stopSent = new Promise(resolve => { signalStop = resolve; });
  const remoteCipher = await createCipher({ ...value, role: 'controller' });
  try {
    globalThis.RTCPeerConnection = class { constructor() { throw Error('Must not create a peer before mutual approval'); } };
    globalThis.fetch = async (url, options) => {
      urls.push(String(url));
      assert.equal(options.headers['X-Pinggy-No-Screen'], undefined);
      assert.equal(options.headers.Authorization, 'Bearer ' + value.token);
      assert.equal(options.credentials, 'omit');
      assert.equal(options.redirect, 'error');
      if (options.method === 'POST') {
        const envelope = JSON.parse(options.body);
        assert.equal(envelope.from, 'guest'); assert.equal(typeof envelope.ct, 'string');
        assert.equal(Object.hasOwn(envelope, 'key'), false);
        const opened = await remoteCipher.open(envelope); envelopes.push(opened.kind);
        if (opened.kind === 'stop') signalStop();
        return new Response('{}');
      }
      return new Promise((_, reject) => options.signal.addEventListener('abort', () => reject(Error('closed')), { once: true }));
    };
    const { createSupportPeer } = await import('../public/support/peer.js');
    for (const queue of [{ origin, requestId: '../other' }, { origin: origin + '/path', requestId: id },
      { origin, requestId: id, extra: true }, { origin: 'https://other.synthetic.workers.dev', requestId: id }]) {
      await assert.rejects(createSupportPeer({ config: value, queue }));
    }
    assert.equal(urls.length, 0);
    peer = await createSupportPeer({ config: value, queue: { origin, requestId: id } });
    assert.equal(urls.length, 0);
    assert.equal(await peer.approve(), true);
    assert.ok(urls.includes(`${origin}/v3/requests/${id}/api/send`));
    assert.ok(urls.includes(`${origin}/v3/requests/${id}/api/poll?after=0`));
    peer.close();
    await Promise.race([stopSent, new Promise((_, reject) => { const timer = setTimeout(() => reject(Error('Stop setup was not delivered')), 1500); timer.unref(); })]);
    assert.deepEqual(envelopes, ['approved', 'stop']);
    assert.ok(urls.every(url => url.startsWith(`${origin}/v3/requests/${id}/api/`)));
    assert.equal(peer.sendFrame(frame()), false);
  } finally { peer?.close(); globalThis.RTCPeerConnection = previousRTC; globalThis.fetch = previousFetch; }
});

test('data-only WebKit SDP variants preserve the original bytes', () => {
  const webkit = description('actpass').replace('IN IP4 127.0.0.1', 'IN IP4 0.0.0.0').replace('a=msid-semantic: WMS', 'a=msid-semantic: WMS *');
  assert.equal(validateDescription(webkit, 'offer').sdp, webkit);
  const legacy = webkit.replace('UDP/DTLS/SCTP webrtc-datachannel', 'DTLS/SCTP 5000').replace('a=sctp-port:5000', 'a=sctpmap:5000 webrtc-datachannel 1024');
  assert.equal(validateDescription(legacy, 'offer').sdp, legacy);
  assert.throws(() => validateDescription(webkit.replace('o=- ', 'o=private-user '), 'offer'));
});

test('strict app frames contain only bounded drawing primitives and control targets', () => {
  assert.equal(validateFrame(frame()).items.length, 2);
  for (const mutate of [
    value => { value.html = '<script>example</script>'; },
    value => { value.items[0].fill = 'url(https://example.invalid)'; },
    value => { value.items[0].fill = 'rgb(999, 0, 0)'; },
    value => { value.items[1].kind = 'html'; },
    value => { value.items[1].size = Infinity; },
    value => { value.items[1].text = 'x'.repeat(1001); },
    value => { value.items[0].x = -1; },
    value => { value.targets.push({ ...value.targets[0] }); },
    value => { value.targets[0].value = 'x'.repeat(2001); },
    value => { value.targets[0].onclick = 'arbitrary'; },
    value => { value.width = 4097; },
    value => { value.truncated = 'false'; },
    value => { value.items = Array.from({ length: 601 }, () => ({ ...value.items[0] })); },
    value => { value.items = Array.from({ length: 13 }, () => ({ ...value.items[1], text: 'x'.repeat(1000) })); },
    value => { value.targets = Array.from({ length: 80 }, (_, index) => ({ ...value.targets[0], id: index.toString(16).padStart(32, '0'), value: 'x'.repeat(2000) })); },
  ]) {
    const value = frame(); mutate(value);
    assert.throws(() => validateFrame(value));
  }
});

test('actions are exact kind-specific messages bound to frame and target identifiers', () => {
  for (const kind of ['click', 'input', 'scroll']) assert.equal(validateAction(action(kind)).kind, kind);
  for (const value of [
    { ...action('click'), value: 'unexpected' }, { ...action('input'), deltaY: 1 },
    { ...action('scroll'), deltaY: 701 }, { ...action('scroll'), deltaY: NaN },
    { ...action('input'), value: 'x'.repeat(2001) }, { ...action('input'), value: 'x\u0000' },
    { ...action('click'), kind: 'execute' }, { ...action('click'), target: '../arbitrary' },
    { ...action('click'), frame: 'expired' },
  ]) assert.throws(() => validateAction(value));
  assert.equal(validateDataMessage({ type: 'frame', frame: frame() }, { sid, sourceRole: 'guest' }).type, 'frame');
  assert.equal(validateDataMessage({ type: 'action', action: action('click') }, { sid, sourceRole: 'controller' }).type, 'action');
  assert.throws(() => validateDataMessage({ type: 'frame', frame: frame() }, { sid, sourceRole: 'controller' }));
  assert.throws(() => validateDataMessage({ type: 'action', action: action('click') }, { sid, sourceRole: 'guest' }));
  assert.throws(() => validateDataMessage({ type: 'hello', v: 1, sid: 'f'.repeat(32) }, { sid, sourceRole: 'guest' }));
  assert.throws(() => validateDataMessage({ type: 'heartbeat', action: action('click') }, { sid, sourceRole: 'guest' }));
});

const updateHandoff = (overrides = {}) => ({
  v: 1, parentId: '1'.repeat(32), parentSid: sid, nonce: '2'.repeat(32), targetVersion: '0.2.0-beta.4',
  artifactSha256: '3'.repeat(64), nativeSessionId: '4'.repeat(32), originalExpires: Date.now() + 60000,
  resumeDeadline: Date.now() + 45000, resumeTicketSha256: '5'.repeat(64), ...overrides,
});
test('update handoff is bounded, exact, guest-only and bound to the current direct SID', () => {
  const packet = { type: 'update-handoff', handoff: updateHandoff(), resumeAuth: '6'.repeat(64) };
  assert.deepEqual(validateDataMessage(packet, { sid, sourceRole: 'guest' }), packet);
  const ack = { type: 'update-handoff-ack', nonce: packet.handoff.nonce, proof: '7'.repeat(64) };
  assert.deepEqual(validateDataMessage(ack, { sid, sourceRole: 'controller' }), ack);
  assert.throws(() => validateDataMessage(packet, { sid, sourceRole: 'controller' }));
  assert.throws(() => validateDataMessage(ack, { sid, sourceRole: 'guest' }));
  for (const changes of [
    { parentSid: '0'.repeat(32) }, { nonce: '../arbitrary' }, { targetVersion: 'x'.repeat(129) },
    { targetVersion: 'https://untrusted.invalid' }, { artifactSha256: 'short' },
    { resumeDeadline: Date.now() - 1 }, { resumeDeadline: Date.now() + 91000 },
    { originalExpires: Date.now() + 30 * 60 * 1000 + 1000 }, { extra: true },
  ]) assert.throws(() => validateDataMessage({ ...packet, handoff: { ...packet.handoff, ...changes } }, { sid, sourceRole: 'guest' }));
  for (const invalid of [{ ...packet, resumeTicket: 'secret' }, { ...packet, resumeAuth: 'bad' }, { ...ack, proof: 'short' }, { ...ack, action: action('click') }]) {
    assert.throws(() => validateDataMessage(invalid, { sid, sourceRole: invalid.type === 'update-handoff' ? 'guest' : 'controller' }));
  }
});

test('peer starts STUN only after local and remote approval and Stop closes immediately', async () => {
  const originalRTC = globalThis.RTCPeerConnection, originalFetch = globalThis.fetch;
  const cfg = config('controller');
  const remote = await createCipher({ ...cfg, role: 'guest' });
  const approval = await remote.seal('approved', null);
  let peers = 0, closures = 0, releasePoll, negotiating, stopped;
  const negotiation = new Promise(resolve => { negotiating = resolve; });
  const stopDelivered = new Promise(resolve => { stopped = resolve; });
  const pendingPoll = new Promise(resolve => { releasePoll = resolve; });
  const makeResponse = value => new Response(JSON.stringify(value), { status: 200, headers: { 'Content-Type': 'application/json' } });
  class FakePeer {
    constructor(options) {
      peers++;
      assert.equal(options.iceCandidatePoolSize, 0);
      assert.ok(options.iceServers.every(server => server.urls.startsWith('stun:')));
    }
    createDataChannel(label, options) {
      return { label, ...options, maxRetransmits: null, maxPacketLifeTime: null, readyState: 'connecting', bufferedAmount: 0, close() {} };
    }
    async createOffer() { return { type: 'offer', sdp: description('actpass') }; }
    async setLocalDescription(value) { this.localDescription = value; }
    close() { closures++; }
  }
  let peer;
  try {
    globalThis.RTCPeerConnection = FakePeer;
    globalThis.fetch = async (url, options) => {
      if (String(url).includes('/api/poll')) return pendingPoll;
      const message = await remote.open(JSON.parse(options.body));
      if (message.kind === 'stop') stopped();
      return makeResponse({});
    };
    const { createSupportPeer } = await import('../public/support/peer.js');
    const states = [], reasons = [];
    peer = await createSupportPeer({ config: cfg, onState: state => { states.push(state); if (state === 'negotiating') negotiating(); }, onClose: reason => reasons.push(reason) });
    assert.equal(peers, 0);
    assert.equal(await peer.approve(), true);
    assert.equal(peers, 0);
    releasePoll(makeResponse({ messages: [approval], expiresAt: cfg.expires }));
    await within(negotiation, 'negotiation', () => JSON.stringify(reasons));
    assert.equal(peers, 1);
    peer.close();
    assert.equal(closures, 1);
    assert.deepEqual(reasons, ['stopped']);
    assert.equal(peer.sendFrame(frame()), false);
    assert.equal(peer.sendAction(action('click')), false);
    assert.equal(await peer.approve(), false);
    peer.close();
    assert.deepEqual(reasons, ['stopped']);
    await within(stopDelivered, 'negotiation Stop delivery');
  } finally {
    peer?.close();
    globalThis.RTCPeerConnection = originalRTC;
    globalThis.fetch = originalFetch;
  }
});

test('queued trickle candidates stay ordered and delayed Stop completes before teardown', async () => {
  const originalRTC = globalThis.RTCPeerConnection, originalFetch = globalThis.fetch;
  const originalEncrypt = crypto.subtle.encrypt;
  const cfg = config('controller'), remote = await createCipher({ ...cfg, role: 'guest' });
  const approval = await remote.seal('approved', null);
  const kinds = [];
  let pc, peer, firstCandidate, releaseCandidate, finished, stopped;
  const firstDelivery = new Promise(resolve => { firstCandidate = resolve; });
  const blockedDelivery = new Promise(resolve => { releaseCandidate = resolve; });
  const complete = new Promise(resolve => { finished = resolve; });
  const stopDelivered = new Promise(resolve => { stopped = resolve; });
  const response = value => new Response(JSON.stringify(value), { status: 200 });
  class FakePeer {
    constructor() { pc = this; }
    createDataChannel(label, options) { return { label, ...options, maxRetransmits: null, maxPacketLifeTime: null, close() {} }; }
    async createOffer() { return { type: 'offer', sdp: description('actpass') }; }
    async setLocalDescription(value) {
      this.localDescription = value;
      this.onicecandidate({ candidate: { candidate: rawCandidate } });
      this.onicecandidate({ candidate: { candidate: rawCandidate.replace(' 40000 ', ' 40001 ') } });
    }
    close() {}
  }
  try {
    // A slow WebCrypto operation used to outlive the fixed 5ms teardown sleep
    // and send this test's Stop into the next test's replacement fetch mock.
    crypto.subtle.encrypt = async function(...args) {
      if (JSON.parse(new TextDecoder().decode(args[2])).kind === 'stop') {
        await new Promise(resolve => setTimeout(resolve, 30));
      }
      return originalEncrypt.apply(this, args);
    };
    globalThis.RTCPeerConnection = FakePeer;
    globalThis.fetch = async (url, options) => {
      assert.equal(options.headers['X-Pinggy-No-Screen'], undefined);
      if (String(url).includes('/api/poll')) return response({ messages: [approval], expiresAt: cfg.expires });
      const message = await remote.open(JSON.parse(options.body));
      kinds.push(message.kind);
      if (message.kind === 'candidate' && kinds.filter(kind => kind === 'candidate').length === 1) {
        firstCandidate(); await blockedDelivery;
      }
      if (message.kind === 'end') finished();
      if (message.kind === 'stop') stopped();
      return response({});
    };
    const { createSupportPeer } = await import('../public/support/peer.js');
    peer = await createSupportPeer({ config: cfg });
    await peer.approve();
    await within(firstDelivery, 'first trickle delivery');
    pc.onicecandidate({ candidate: null });
    releaseCandidate();
    await within(complete, 'end of trickle delivery');
    assert.deepEqual(kinds, ['approved', 'offer', 'candidate', 'candidate', 'end']);
    peer.close();
    await within(stopDelivered, 'delayed trickle Stop delivery');
    assert.deepEqual([...kinds], ['approved', 'offer', 'candidate', 'candidate', 'end', 'stop']);
  } finally {
    releaseCandidate?.(); peer?.close();
    try {
      if (kinds.includes('approved')) await within(stopDelivered, 'trickle teardown Stop delivery');
    } finally {
      crypto.subtle.encrypt = originalEncrypt;
      globalThis.RTCPeerConnection = originalRTC; globalThis.fetch = originalFetch;
    }
  }
});

for (const mode of ['stop', 'update', 'forged ACK']) test(mode === 'forged ACK' ? 'paired queue peer rejects a forged update ACK and cannot suspend' : mode === 'update' ? 'paired queue peers freeze sharing, authenticate the update ACK and suspend without stale Stop' : 'paired peers verify direct transport, carry direction-bound app data and revoke on Stop', async () => {
  const updating = mode !== 'stop';
  const originalRTC = globalThis.RTCPeerConnection, originalFetch = globalThis.fetch;
  const controllerConfig = { ...config('controller'), ...(updating ? { origin: 'https://synthetic.example.workers.dev' } : {}) };
  const guestConfig = { ...controllerConfig, role: 'guest', token: 'b'.repeat(64), origin: updating ? controllerConfig.origin : 'https://example.run.pinggy-free.link' };
  const queue = updating ? { origin: controllerConfig.origin, requestId: '1'.repeat(32) } : undefined;
  const queues = { controller: [], guest: [] }, pcs = [], frames = [], actions = [], reasons = [];
  const stopSignals = {}, approvedRoles = new Set();
  const stopDeliveries = Object.fromEntries(['controller', 'guest'].map(role => [role, new Promise(resolve => { stopSignals[role] = resolve; })]));
  const observers = {
    controller: await createCipher({ ...controllerConfig, role: 'guest' }),
    guest: await createCipher({ ...guestConfig, role: 'controller' }),
  };
  const handoffs = [], acks = [];
  let controller, guest, controllerConnected, guestConnected, frameReceived, actionReceived;
  let handoffReceived, ackReceived, updateRejected;
  const connected = [new Promise(resolve => { controllerConnected = resolve; }), new Promise(resolve => { guestConnected = resolve; })];
  const nextFrame = new Promise(resolve => { frameReceived = resolve; });
  const nextAction = new Promise(resolve => { actionReceived = resolve; });
  const nextHandoff = new Promise(resolve => { handoffReceived = resolve; });
  const nextAck = new Promise(resolve => { ackReceived = resolve; });
  const rejectedUpdate = new Promise(resolve => { updateRejected = resolve; });
  const response = value => new Response(JSON.stringify(value), { status: 200 });
  class FakeChannel {
    constructor(label, options) {
      Object.assign(this, { label, ...options, maxRetransmits: null, maxPacketLifeTime: null, readyState: 'connecting', bufferedAmount: 0 });
    }
    send(text) { queueMicrotask(() => { if (this.other.readyState === 'open') this.other.onmessage?.({ data: text }); }); }
    close() {
      if (this.readyState === 'closed') return;
      this.readyState = 'closed';
      this.onclose?.();
      if (this.other && this.other.readyState !== 'closed') { this.other.readyState = 'closed'; this.other.onclose?.(); }
    }
  }
  class FakePeer {
    constructor() { this.role = pcs.length ? 'guest' : 'controller'; pcs.push(this); }
    createDataChannel(label, options) { this.channel = new FakeChannel(label, options); return this.channel; }
    async createOffer() { return { type: 'offer', sdp: description('actpass') }; }
    async createAnswer() { return { type: 'answer', sdp: description('active') }; }
    async setLocalDescription(value) { this.localDescription = value; }
    async setRemoteDescription(value) {
      this.remoteDescription = value;
      if (value.type === 'offer') {
        const first = pcs[0].channel;
        this.channel = new FakeChannel(first.label, { protocol: first.protocol, ordered: true });
        this.channel.other = first; first.other = this.channel;
        this.ondatachannel({ channel: this.channel });
      } else {
        for (const peer of pcs) peer.channel.readyState = 'open';
        for (const peer of pcs) queueMicrotask(() => { void peer.channel.onopen?.(); });
      }
    }
    async addIceCandidate() {}
    async getStats() {
      return new Map([
        ['transport', { type: 'transport', selectedCandidatePairId: 'pair', dtlsState: 'connected' }],
        ['pair', { type: 'candidate-pair', state: 'succeeded', nominated: true, localCandidateId: 'local', remoteCandidateId: 'remote' }],
        ['local', { type: 'local-candidate', candidateType: 'srflx', protocol: 'udp', address: this.role === 'controller' ? exit : '9.9.9.9' }],
        ['remote', { type: 'remote-candidate', candidateType: 'srflx', protocol: 'udp', address: this.role === 'guest' ? exit : '9.9.9.9' }],
      ]);
    }
    close() { this.channel?.close(); }
  }
  try {
    globalThis.RTCPeerConnection = FakePeer;
    globalThis.fetch = async (url, options) => {
      const role = options.headers.Authorization === `Bearer ${controllerConfig.token}` ? 'controller' : 'guest';
      assert.equal(options.headers['X-Pinggy-No-Screen'], role === 'guest' && !updating ? '1' : undefined);
      assert.ok(String(url).startsWith((role === 'guest' ? guestConfig.origin : controllerConfig.origin) + (updating ? '/v3/requests/' + queue.requestId : '') + '/api/'));
      if (options.method === 'POST') {
        const envelope = JSON.parse(options.body);
        assert.equal(envelope.from, role);
        if (!queues[role].some(message => message.seq === envelope.seq)) {
          const opened = await observers[role].open(envelope);
          queues[role].push(envelope);
          if (opened.kind === 'stop') stopSignals[role]();
        }
        return response({});
      }
      const after = Number(new URL(url).searchParams.get('after'));
      return response({ messages: queues[role === 'controller' ? 'guest' : 'controller'].filter(message => message.seq > after), expiresAt: controllerConfig.expires });
    };
    const { createSupportPeer } = await import('../public/support/peer.js');
    controller = await createSupportPeer({ config: controllerConfig, queue, onUpdateHandoff: value => { handoffs.push(value); handoffReceived(); }, onState: state => { if (state === 'connected') controllerConnected(); }, onFrame: value => { frames.push(value); frameReceived(); }, onClose: reason => reasons.push(['controller', reason]) });
    guest = await createSupportPeer({ config: guestConfig, queue, onUpdateHandoffAck: value => { acks.push(value); ackReceived(); }, onState: state => { if (state === 'connected') guestConnected(); }, onAction: value => { actions.push(value); actionReceived(); }, onClose: reason => { reasons.push(['guest', reason]); if (reason === 'update') updateRejected(); } });
    assert.equal(await controller.approve(), true); approvedRoles.add('controller');
    assert.equal(await guest.approve(), true); approvedRoles.add('guest');
    await within(Promise.all(connected), 'connected', () => JSON.stringify(reasons));
    assert.equal(controller.sendFrame(frame()), false);
    assert.equal(guest.sendAction(action('click')), false);
    assert.equal(guest.sendFrame(frame()), true);
    assert.equal(guest.sendFrame({ ...frame(), id: '3'.repeat(32) }), false);
    await within(nextFrame, 'frame', () => JSON.stringify(reasons));
    assert.deepEqual(frames, [frame()]);
    assert.equal(controller.sendAction(action('input')), true);
    await within(nextAction, 'action', () => JSON.stringify(reasons));
    assert.deepEqual(actions, [action('input')]);
    assert.equal(controller.suspendForUpdate(), false); assert.equal(guest.suspendForUpdate(), false);
    if (updating) {
      const packet = { handoff: updateHandoff({ originalExpires: controllerConfig.expires }), resumeAuth: '6'.repeat(64) };
      const { ackProof } = await import('../public/support/queue-protocol.js');
      assert.equal(controller.sendUpdateHandoff(packet), false);
      assert.equal(guest.sendUpdateHandoff({ ...packet, handoff: { ...packet.handoff, parentId: '0'.repeat(32) } }), false);
      assert.equal(guest.sendUpdateHandoff(packet), true);
      assert.equal(guest.sendFrame(frame()), false);
      await within(nextHandoff, 'update handoff', () => JSON.stringify(reasons));
      assert.deepEqual(handoffs, [packet]); assert.equal(controller.sendAction(action('click')), false);
      assert.equal(guest.suspendForUpdate(), false);
      if (mode === 'forged ACK') {
        pcs[0].channel.send(JSON.stringify({ type: 'update-handoff-ack', nonce: packet.handoff.nonce, proof: '0'.repeat(64) }));
        await within(rejectedUpdate, 'forged update rejection', () => JSON.stringify(reasons));
        assert.deepEqual(acks, []); assert.equal(guest.suspendForUpdate(), false);
        assert.ok(reasons.some(([role, reason]) => role === 'guest' && reason === 'update'));
        assert.equal(controller.sendAction(action('click')), false); return;
      }
      assert.equal(await controller.sendUpdateHandoffAck({ nonce: packet.handoff.nonce, proof: '0'.repeat(64) }), false);
      const ack = { nonce: packet.handoff.nonce, proof: await ackProof(packet.resumeAuth, packet.handoff) };
      assert.equal(await controller.sendUpdateHandoffAck(ack), true);
      await within(nextAck, 'authenticated update ACK', () => JSON.stringify(reasons));
      assert.deepEqual(acks, [ack]); assert.equal(await controller.sendUpdateHandoffAck(ack), false);
      const setupCounts = [queues.controller.length, queues.guest.length];
      assert.equal(guest.suspendForUpdate(), true);
      assert.equal(guest.suspendForUpdate(), false); assert.equal(controller.sendAction(action('click')), false);
      controller.close(); guest.close();
      await new Promise(resolve => setTimeout(resolve, 5));
      assert.deepEqual(reasons, []); assert.deepEqual([queues.controller.length, queues.guest.length], setupCounts);
      assert.equal(JSON.stringify(queues).includes(packet.resumeAuth), false);
      return;
    }
    // A validated action queued before Stop must not reach a revoked receiver.
    assert.equal(controller.sendAction(action('click')), true);
    guest.close();
    await within(stopDeliveries.guest, 'paired guest Stop delivery');
    assert.equal(actions.length, 1);
    assert.equal(guest.sendFrame(frame()), false);
    assert.equal(controller.sendAction(action('click')), false);
    assert.ok(reasons.some(([role, reason]) => role === 'guest' && reason === 'stopped'));
    assert.ok(reasons.some(([role, reason]) => role === 'controller' && reason === 'disconnected'));
  } finally {
    controller?.close(); guest?.close();
    try {
      const locallyStopped = reasons.filter(([role, reason]) => reason === 'stopped' && approvedRoles.has(role));
      await within(Promise.all(locallyStopped.map(([role]) => stopDeliveries[role])), 'paired teardown Stop delivery');
    } finally {
      globalThis.RTCPeerConnection = originalRTC; globalThis.fetch = originalFetch;
    }
  }
});

async function setupDeliveryFailure(t, kind, connect) {
  const originalRTC = globalThis.RTCPeerConnection, originalFetch = globalThis.fetch, originalTimeout = globalThis.setTimeout;
  const controllerConfig = config('controller');
  const guestConfig = { ...controllerConfig, role: 'guest', token: 'b'.repeat(64), origin: 'https://example.run.pinggy-free.link' };
  const queues = { controller: [], guest: [] }, pcs = [], gathering = [], states = [], reasons = [], received = [];
  const observed = { controller: new Map(), guest: new Map() };
  const stopSignals = {}, approvedRoles = new Set();
  const stopDeliveries = Object.fromEntries(['controller', 'guest'].map(role => [role, new Promise(resolve => { stopSignals[role] = resolve; })]));
  const observers = {
    controller: await createCipher({ ...controllerConfig, role: 'guest' }),
    guest: await createCipher({ ...guestConfig, role: 'controller' }),
  };
  let controller, guest, releaseFailure, markPending, markFinished, markReady, markSeed, markFrame, markAction;
  let controllerConnected, guestConnected, attempts = 0;
  const pending = new Promise(resolve => { markPending = resolve; });
  const finished = new Promise(resolve => { markFinished = resolve; });
  const ready = new Promise(resolve => { markReady = resolve; });
  const seed = new Promise(resolve => { markSeed = resolve; });
  const blocked = new Promise(resolve => { releaseFailure = resolve; });
  const connected = [new Promise(resolve => { controllerConnected = resolve; }), new Promise(resolve => { guestConnected = resolve; })];
  const response = value => new Response(JSON.stringify(value), { status: 200 });
  const tick = () => new Promise(resolve => originalTimeout(resolve, 0));
  let guard;
  const timed = promise => Promise.race([promise, new Promise((_, reject) => {
    guard = originalTimeout(() => reject(new Error('Synthetic setup did not finish.')), 5000);
  })]).finally(() => clearTimeout(guard));
  t.after(async () => {
    releaseFailure(); controller?.close(); guest?.close();
    try {
      const locallyStopped = reasons.filter(([role, reason]) => reason === 'stopped' && approvedRoles.has(role));
      await within(Promise.all(locallyStopped.map(([role]) => stopDeliveries[role])), 'setup-failure teardown Stop delivery');
    } finally {
      globalThis.RTCPeerConnection = originalRTC; globalThis.fetch = originalFetch; globalThis.setTimeout = originalTimeout;
    }
  });
  class FakeChannel {
    constructor(label, options) { Object.assign(this, { label, ...options, maxRetransmits: null, maxPacketLifeTime: null, readyState: 'connecting', bufferedAmount: 0 }); }
    send(text) { queueMicrotask(() => { if (this.other.readyState === 'open') this.other.onmessage?.({ data: text }); }); }
    close() {
      if (this.readyState === 'closed') return;
      this.readyState = 'closed'; this.onclose?.();
      if (this.other && this.other.readyState !== 'closed') { this.other.readyState = 'closed'; this.other.onclose?.(); }
    }
  }
  class FakePeer {
    constructor() { this.role = pcs.length ? 'guest' : 'controller'; pcs.push(this); }
    createDataChannel(label, options) { this.channel = new FakeChannel(label, options); return this.channel; }
    async createOffer() { return { type: 'offer', sdp: description('actpass') }; }
    async createAnswer() { return { type: 'answer', sdp: description('active') }; }
    async setLocalDescription(value) { this.localDescription = value; }
    async setRemoteDescription(value) {
      this.remoteDescription = value;
      if (value.type === 'offer') {
        const first = pcs[0].channel;
        this.channel = new FakeChannel(first.label, { protocol: first.protocol, ordered: true });
        this.channel.other = first; first.other = this.channel;
        this.ondatachannel({ channel: this.channel });
      } else markReady();
    }
    async addIceCandidate() {}
    async getStats() {
      return new Map([
        ['transport', { type: 'transport', selectedCandidatePairId: 'pair', dtlsState: 'connected' }],
        ['pair', { type: 'candidate-pair', state: 'succeeded', nominated: true, localCandidateId: 'local', remoteCandidateId: 'remote' }],
        ['local', { type: 'local-candidate', candidateType: 'srflx', protocol: 'udp', address: this.role === 'controller' ? exit : '9.9.9.9' }],
        ['remote', { type: 'remote-candidate', candidateType: 'srflx', protocol: 'udp', address: this.role === 'guest' ? exit : '9.9.9.9' }],
      ]);
    }
    close() { this.channel?.close(); }
  }
  globalThis.RTCPeerConnection = FakePeer;
  globalThis.setTimeout = (callback, delay, ...args) => {
    if (delay === 15000) { gathering.push(callback); return 0; }
    return originalTimeout(callback, delay, ...args);
  };
  globalThis.fetch = async (url, options) => {
    const role = options.headers.Authorization === `Bearer ${controllerConfig.token}` ? 'controller' : 'guest';
    if (options.method === 'POST') {
      const envelope = JSON.parse(options.body);
      if (!observed[role].has(envelope.seq)) observed[role].set(envelope.seq, await observers[role].open(envelope));
      const message = observed[role].get(envelope.seq);
      if (message.kind === 'stop') stopSignals[role]();
      if (role === 'guest' && message.kind === (kind === 'candidate' ? 'candidate' : 'end')) {
        attempts++; markPending(); await blocked;
        if (attempts === 3) markFinished();
        return new Response('{}', { status: 503 });
      }
      if (!queues[role].some(item => item.seq === envelope.seq)) queues[role].push(envelope);
      if (role === 'guest' && message.kind === 'candidate') markSeed();
      return response({});
    }
    const after = Number(new URL(url).searchParams.get('after'));
    return response({ messages: queues[role === 'controller' ? 'guest' : 'controller'].filter(item => item.seq > after), expiresAt: controllerConfig.expires });
  };
  const { createSupportPeer } = await import('../public/support/peer.js');
  controller = await createSupportPeer({ config: controllerConfig,
    onState: state => { states.push(['controller', state]); if (state === 'connected') controllerConnected(); },
    onClose: reason => reasons.push(['controller', reason]), onFrame: value => { received.push(value); markFrame?.(); },
  });
  guest = await createSupportPeer({ config: guestConfig,
    onState: state => { states.push(['guest', state]); if (state === 'connected') guestConnected(); },
    onClose: reason => reasons.push(['guest', reason]), onAction: () => markAction?.(),
  });
  assert.equal(await controller.approve(), true); approvedRoles.add('controller');
  assert.equal(await guest.approve(), true); approvedRoles.add('guest');
  await timed(ready); await tick();
  const candidate = { candidate: rawCandidate.replace(exit, '9.9.9.9') };
  pcs[1].onicecandidate({ candidate });
  if (kind !== 'candidate') {
    await timed(seed);
    if (kind === 'timeout') gathering[1]();
    else pcs[1].onicecandidate({ candidate: null });
  }
  await timed(pending);
  if (connect) {
    for (const pc of pcs) pc.channel.readyState = 'open';
    for (const pc of pcs) queueMicrotask(() => { void pc.channel.onopen?.(); });
    await timed(Promise.all(connected));
    const delivered = new Promise(resolve => { markFrame = resolve; });
    assert.equal(guest.sendFrame(frame()), true); await timed(delivered);
  }
  releaseFailure(); await timed(finished); await tick();
  assert.equal(attempts, 3);
  if (connect) {
    assert.deepEqual(reasons, []);
    assert.equal(states.some(([, state]) => state.startsWith('failed:')), false);
    const delivered = new Promise(resolve => { markFrame = resolve; });
    assert.equal(guest.sendFrame({ ...frame(), id: '3'.repeat(32) }), true); await timed(delivered);
    const applied = new Promise(resolve => { markAction = resolve; });
    assert.equal(controller.sendAction(action('input')), true); await timed(applied);
    assert.equal(received.length, 2);
    guest.close(); await tick();
    assert.equal(controller.sendAction(action('click')), false);
    assert.ok(reasons.some(([role, reason]) => role === 'guest' && reason === 'stopped'));
  } else {
    assert.ok(reasons.some(([role, reason]) => role === 'guest' && reason === 'signaling'));
    assert.equal(states.some(([, state]) => state === 'connected'), false);
    assert.equal(guest.sendFrame(frame()), false);
  }
}

for (const kind of ['candidate', 'end', 'timeout']) {
  test(`late ${kind} setup delivery failure does not revoke a verified direct connection`, async t => {
    await setupDeliveryFailure(t, kind, true);
  });
}
test('candidate setup delivery failure before direct verification remains fail-closed', async t => {
  await setupDeliveryFailure(t, 'candidate', false);
});
