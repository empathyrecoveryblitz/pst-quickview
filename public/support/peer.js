import {
  publicIPv4, sanitizeCandidate, validateCandidate, validateDescription, createCipher,
  validateConfig, validateFrame, validateAction, validateDataMessage,
} from './protocol.js';

const CHANNEL = 'pst-quickview-app-support';
const PROTOCOL = 'pqv-app-support-v1';
const ICE = [{ urls: 'stun:stun.cloudflare.com:3478' }, { urls: 'stun:stun.l.google.com:19302' }];
const encoder = new TextEncoder();
const wait = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds));

export async function createSupportPeer({ config, queue, onState, onFrame, onAction, onClose } = {}) {
  if (queue !== undefined && (!queue || typeof queue !== 'object' || Array.isArray(queue)
    || Object.keys(queue).length !== 2 || !Object.hasOwn(queue, 'origin') || !Object.hasOwn(queue, 'requestId')
    || typeof queue.requestId !== 'string' || !/^[a-f0-9]{32}$/.test(queue.requestId))) {
    throw new Error('Invalid support queue.');
  }
  const cfg = validateConfig(config, queue === undefined ? {} : { serviceOrigin: queue.origin });
  const signalingPrefix = queue === undefined ? '' : `/v3/requests/${queue.requestId}`;
  if (typeof globalThis.RTCPeerConnection !== 'function' || typeof globalThis.fetch !== 'function') {
    throw new Error('This browser does not support direct app support.');
  }
  let cipher = await createCipher(cfg);
  const sourceRole = cfg.role === 'controller' ? 'guest' : 'controller';
  let pc, channel, ended = false, connected = false, approved = false, peerApproved = false;
  let offerStarted = false, descriptionSent = false, localEnded = false, remoteEnded = false;
  let localVerified = false, peerHello = false, helloSent = false, routeBusy = false;
  let remoteSequence = 0, remoteCandidates = 0, lastPeerAt = 0, lastFrameAt = -Infinity;
  let sendChain = Promise.resolve(), pendingLocal = [], pendingRemote = [];
  let negotiationTimer, gatheringTimer, heartbeatTimer;
  const localCandidates = new Set(), requests = new Set();
  let receiveWindow = Date.now(), receivedInWindow = 0, actionWindow = Date.now(), actionsInWindow = 0;
  const emit = state => { try { onState?.(state); } catch {} };
  const notifyClose = reason => { try { onClose?.(reason); } catch {} };
  const expiryTimer = setTimeout(() => fail('expired'), Math.max(1, cfg.expires - Date.now()));

  function shutdown(reason) {
    if (ended) return;
    ended = true;
    connected = false;
    clearTimeout(expiryTimer); clearTimeout(negotiationTimer); clearTimeout(gatheringTimer); clearInterval(heartbeatTimer);
    for (const request of requests) request.abort();
    requests.clear();
    if (channel) {
      channel.onmessage = null; channel.onopen = null; channel.onclose = null; channel.onerror = null;
      try { channel.close(); } catch {}
    }
    if (pc) {
      pc.onicecandidate = null; pc.ondatachannel = null;
      pc.onconnectionstatechange = null; pc.oniceconnectionstatechange = null;
      try { pc.close(); } catch {}
    }
    pendingLocal = []; pendingRemote = []; localCandidates.clear();
    cipher = null; cfg.key = ''; cfg.token = '';
    notifyClose(reason);
  }

  function fail(phase) {
    if (ended) return;
    emit(`failed:${phase}`);
    shutdown(phase);
  }

  async function readJSON(response) {
    if (!response.body?.getReader) throw new Error('signaling');
    const reader = response.body.getReader();
    const chunks = [];
    let total = 0;
    try {
      while (true) {
        const { done, value } = await reader.read();
        if (done) break;
        total += value.byteLength;
        if (total > 512 * 1024) throw new Error('signaling');
        chunks.push(value);
      }
    } catch (error) {
      try { await reader.cancel(); } catch {}
      throw error;
    } finally { reader.releaseLock(); }
    const bytes = new Uint8Array(total);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.byteLength; }
    return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
  }

  async function request(path, method = 'GET', value) {
    for (let attempt = 0; attempt < 3; attempt++) {
      if (ended || Date.now() >= cfg.expires) throw new Error('stopped');
      const controller = new AbortController();
      requests.add(controller);
      const timeout = setTimeout(() => controller.abort(), 6000);
      try {
        const response = await fetch(cfg.origin + signalingPrefix + path, {
          method, mode: 'cors', credentials: 'omit', cache: 'no-store', redirect: 'error', referrerPolicy: 'no-referrer',
          headers: { Authorization: `Bearer ${cfg.token}`, ...(cfg.role === 'guest' && queue === undefined ? { 'X-Pinggy-No-Screen': '1' } : {}), ...(method === 'POST' ? { 'Content-Type': 'application/json' } : {}) },
          ...(value === undefined ? {} : { body: JSON.stringify(value) }), signal: controller.signal,
        });
        if (!response.ok) throw new Error('signaling');
        return await readJSON(response);
      } catch {
        if (attempt === 2 || ended) throw new Error('signaling');
      } finally { clearTimeout(timeout); requests.delete(controller); }
      await wait(400 * (attempt + 1));
    }
    throw new Error('signaling');
  }

  function send(kind, data = null) {
    const operation = sendChain.then(async () => {
      if (ended || !cipher) throw new Error('stopped');
      const envelope = await cipher.seal(kind, data);
      await request('/api/send', 'POST', envelope);
    });
    sendChain = operation;
    return operation;
  }

  async function finishGathering() {
    if (ended || localEnded || connected) return;
    localEnded = true;
    clearTimeout(gatheringTimer);
    if (!localCandidates.size) { fail('discovery'); return; }
    if (descriptionSent) await send('end'); else pendingLocal.push(null);
  }

  async function localCandidate(raw) {
    if (ended || localEnded || connected) return;
    if (!raw) { await finishGathering(); return; }
    const clean = sanitizeCandidate(raw, { role: cfg.role, expectedExit: cfg.expectedExit });
    if (!clean) return;
    const identity = JSON.stringify(clean);
    if (localCandidates.has(identity) || localCandidates.size >= 8) return;
    localCandidates.add(identity);
    if (descriptionSent) await send('candidate', clean); else pendingLocal.push(clean);
  }

  async function flushLocal() {
    descriptionSent = true;
    const pending = pendingLocal;
    pendingLocal = [];
    const deliveries = pending.map(candidate => send(candidate ? 'candidate' : 'end', candidate));
    await Promise.all(deliveries);
  }

  async function flushRemote() {
    const pending = pendingRemote;
    pendingRemote = [];
    for (const candidate of pending) await pc.addIceCandidate(candidate);
  }

  function startPeer() {
    if (pc || ended || !approved || !peerApproved) throw new Error('state');
    emit('negotiating');
    pc = new RTCPeerConnection({ iceServers: ICE, iceCandidatePoolSize: 0 });
    // An in-flight setup delivery can become obsolete after direct verification.
    pc.onicecandidate = event => { void localCandidate(event.candidate).catch(() => { if (!ended && !connected) fail('signaling'); }); };
    pc.ondatachannel = event => wire(event.channel);
    pc.onconnectionstatechange = () => { if (pc.connectionState === 'failed') fail('connectivity'); };
    pc.oniceconnectionstatechange = () => { if (pc.iceConnectionState === 'failed') fail('connectivity'); };
    gatheringTimer = setTimeout(() => { void finishGathering().catch(() => { if (!ended && !connected) fail('signaling'); }); }, 15000);
    negotiationTimer = setTimeout(() => fail('connectivity'), 65000);
  }

  async function makeOffer() {
    if (offerStarted || ended || !approved || !peerApproved) return;
    offerStarted = true;
    startPeer();
    wire(pc.createDataChannel(CHANNEL, { protocol: PROTOCOL, ordered: true }));
    let description;
    try { description = validateDescription((await pc.createOffer()).sdp, 'offer'); }
    catch { fail('description'); return; }
    await pc.setLocalDescription(description);
    await send('offer', description);
    await flushLocal();
  }

  async function receiveOffer(value) {
    if (cfg.role !== 'guest' || pc || !approved || !peerApproved || value?.type !== 'offer') throw new Error('state');
    let description;
    try { description = validateDescription(value?.sdp, 'offer'); }
    catch { fail('description'); return; }
    startPeer();
    await pc.setRemoteDescription(description);
    await flushRemote();
    let answer;
    try { answer = validateDescription((await pc.createAnswer()).sdp, 'answer'); }
    catch { fail('description'); return; }
    await pc.setLocalDescription(answer);
    await send('answer', answer);
    await flushLocal();
  }

  async function receiveAnswer(value) {
    if (cfg.role !== 'controller' || !pc || pc.remoteDescription || value?.type !== 'answer') throw new Error('state');
    let description;
    try { description = validateDescription(value?.sdp, 'answer'); }
    catch { fail('description'); return; }
    await pc.setRemoteDescription(description);
    await flushRemote();
  }

  async function verifyRoute() {
    const stats = await pc.getStats();
    let transport, pair;
    stats.forEach(row => { if (row.type === 'transport' && row.selectedCandidatePairId) transport = row; });
    if (transport) pair = stats.get(transport.selectedCandidatePairId);
    if (!pair) stats.forEach(row => { if (row.type === 'candidate-pair' && row.state === 'succeeded' && row.nominated) pair = row; });
    const local = pair && stats.get(pair.localCandidateId);
    const remote = pair && stats.get(pair.remoteCandidateId);
    const remoteAddress = remote?.address || remote?.ip;
    if (!local || !remote || pair.state !== 'succeeded' || local.candidateType === 'relay' || remote.candidateType === 'relay' ||
        local.protocol !== 'udp' || remote.protocol !== 'udp' || !publicIPv4(remoteAddress) || transport?.dtlsState !== 'connected' ||
        (cfg.role === 'guest' && remoteAddress !== cfg.expectedExit)) throw new Error('route');
  }

  function becomeConnected() {
    if (ended || connected || !localVerified || !peerHello || !helloSent) return;
    connected = true;
    clearTimeout(negotiationTimer); clearTimeout(gatheringTimer);
    emit('connected');
  }

  function dataSend(message) {
    if (ended || channel?.readyState !== 'open' || channel.bufferedAmount > 256 * 1024) return false;
    const text = JSON.stringify(message);
    const bytes = encoder.encode(text).length;
    if (bytes > 50 * 1024) throw new Error('Invalid support data.');
    if (channel.bufferedAmount + bytes > 256 * 1024) return false;
    try { channel.send(text); return true; }
    catch { fail('data'); return false; }
  }

  async function heartbeat() {
    if (ended) return;
    if (Date.now() - lastPeerAt >= 12000) { fail('disconnected'); return; }
    if (!connected || routeBusy) return;
    routeBusy = true;
    try { await verifyRoute(); dataSend({ type: 'heartbeat' }); }
    catch { fail('verification'); }
    finally { routeBusy = false; }
  }

  function wire(ch) {
    if (channel || ch.label !== CHANNEL || ch.protocol !== PROTOCOL || ch.ordered !== true || ch.maxRetransmits != null || ch.maxPacketLifeTime != null) {
      try { ch.close(); } catch {}
      fail('protocol');
      return;
    }
    channel = ch;
    ch.onopen = async () => {
      if (ended) { ch.close(); return; }
      lastPeerAt = Date.now();
      try {
        let verified = false;
        for (let attempt = 0; attempt < 3 && !ended; attempt++) {
          try { await verifyRoute(); verified = true; break; }
          catch { if (attempt < 2) await wait(200); }
        }
        if (!verified || ended) { if (!ended) fail('verification'); return; }
        localVerified = true;
        helloSent = dataSend({ type: 'hello', v: 1, sid: cfg.sid });
        if (!helloSent) { fail('data'); return; }
        becomeConnected();
        heartbeatTimer = setInterval(() => { void heartbeat(); }, 3000);
      } catch { fail('verification'); }
    };
    ch.onmessage = event => {
      if (ended) return;
      try {
        const now = Date.now();
        if (now - receiveWindow >= 1000) { receiveWindow = now; receivedInWindow = 0; }
        if (++receivedInWindow > 40 || typeof event.data !== 'string' || event.data.length > 50 * 1024 ||
            encoder.encode(event.data).length > 50 * 1024) throw new Error('data');
        const message = validateDataMessage(JSON.parse(event.data), { sid: cfg.sid, sourceRole });
        lastPeerAt = now;
        if (message.type === 'hello') {
          if (peerHello) throw new Error('hello');
          peerHello = true;
          becomeConnected();
          return;
        }
        if (!connected) throw new Error('state');
        if (message.type === 'frame') { try { onFrame?.(message.frame); } catch {} }
        else if (message.type === 'action') { try { onAction?.(message.action); } catch {} }
      } catch { fail('protocol'); }
    };
    ch.onclose = () => fail('disconnected');
    ch.onerror = () => fail('data');
  }

  async function receive({ kind, data }) {
    if (ended) return;
    if (kind === 'approved') {
      if (peerApproved || data !== null) throw new Error('state');
      peerApproved = true;
      if (cfg.role === 'controller') await makeOffer();
      return;
    }
    if (kind === 'stop') {
      if (data !== null) throw new Error('state');
      emit('stopped'); shutdown('peer_stopped'); return;
    }
    if (!approved || !peerApproved) throw new Error('state');
    if (kind === 'offer') { await receiveOffer(data); return; }
    if (kind === 'answer') { await receiveAnswer(data); return; }
    if (kind === 'candidate') {
      if (remoteEnded || remoteCandidates >= 8) throw new Error('candidate');
      const clean = validateCandidate(data, { sourceRole, expectedExit: cfg.expectedExit });
      remoteCandidates++;
      if (pc?.remoteDescription) await pc.addIceCandidate(clean); else pendingRemote.push(clean);
      return;
    }
    if (kind === 'end') {
      if (data !== null || remoteEnded || !pc?.remoteDescription) throw new Error('state');
      remoteEnded = true;
      await pc.addIceCandidate(null);
      return;
    }
    if (kind === 'diagnostic' && data && ['discovery', 'description', 'connectivity', 'verification', 'signaling', 'data', 'protocol', 'expired'].includes(data.stage)) {
      fail('peer_failure'); return;
    }
    throw new Error('protocol');
  }

  async function poll() {
    try {
      while (!ended && !connected) {
        const batch = await request(`/api/poll?after=${remoteSequence}`);
        if (!Array.isArray(batch.messages) || batch.messages.length > 128 || batch.expiresAt !== cfg.expires) throw new Error('signaling');
        for (const envelope of batch.messages) {
          const message = await cipher.open(envelope);
          remoteSequence = envelope.seq;
          await receive(message);
          if (ended) break;
        }
        if (!ended && !connected) await wait(batch.messages.length ? 80 : 650);
      }
    } catch { if (!ended && !connected) fail('signaling'); }
  }

  emit('ready');
  return Object.freeze({
    async approve() {
      if (ended || approved) return false;
      approved = true;
      emit('approved');
      try {
        await send('approved');
        if (ended) return false;
        emit('waiting');
        void poll();
        return true;
      } catch { fail('signaling'); return false; }
    },
    sendFrame(frame) {
      if (ended || !connected || cfg.role !== 'guest') return false;
      validateFrame(frame);
      const now = performance.now();
      if (now - lastFrameAt < 500) return false;
      if (!dataSend({ type: 'frame', frame })) return false;
      lastFrameAt = now;
      return true;
    },
    sendAction(action) {
      if (ended || !connected || cfg.role !== 'controller') return false;
      validateAction(action);
      const now = Date.now();
      if (now - actionWindow >= 1000) { actionWindow = now; actionsInWindow = 0; }
      if (actionsInWindow >= 20) return false;
      if (!dataSend({ type: 'action', action })) return false;
      actionsInWindow++;
      return true;
    },
    close() {
      if (ended) return;
      const finalCipher = cipher, token = cfg.token, origin = cfg.origin;
      const finalMessage = approved && finalCipher ? finalCipher.seal('stop', null) : null;
      emit('stopped');
      shutdown('stopped');
      if (finalMessage) void finalMessage.then(async envelope => {
        const controller = new AbortController();
        const timeout = setTimeout(() => controller.abort(), 1500);
        try {
          await fetch(origin + signalingPrefix + '/api/send', {
            method: 'POST', mode: 'cors', credentials: 'omit', cache: 'no-store', redirect: 'error', referrerPolicy: 'no-referrer',
            headers: { Authorization: `Bearer ${token}`, ...(cfg.role === 'guest' && queue === undefined ? { 'X-Pinggy-No-Screen': '1' } : {}), 'Content-Type': 'application/json' },
            body: JSON.stringify(envelope), signal: controller.signal,
          });
        } catch {} finally { clearTimeout(timeout); }
      }).catch(() => {});
    },
  });
}
