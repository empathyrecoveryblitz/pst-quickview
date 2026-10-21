const ROLES = new Set(['controller', 'guest']);
const KINDS = new Set(['approved', 'offer', 'answer', 'candidate', 'end', 'diagnostic', 'stop']);
const MAX_CIPHERTEXT = 20 * 1024;
const MAX_PLAINTEXT = MAX_CIPHERTEXT - 16;
const MAX_SEQUENCE = 128;
const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: true });

function plainObject(value) {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}

function exactKeys(value, keys) {
  if (!plainObject(value)) return false;
  const actual = Reflect.ownKeys(value);
  return actual.length === keys.length && actual.every(key =>
    typeof key === 'string' && keys.includes(key) &&
    Object.getOwnPropertyDescriptor(value, key)?.get === undefined &&
    Object.getOwnPropertyDescriptor(value, key)?.set === undefined);
}

export function publicIPv4(value) {
  if (typeof value !== 'string' || !/^(?:0|[1-9]\d{0,2})(?:\.(?:0|[1-9]\d{0,2})){3}$/.test(value)) return false;
  const octets = value.split('.').map(Number);
  if (octets.some(octet => octet > 255)) return false;
  const [a, b, c] = octets;
  return !(
    a === 0 || a === 10 || a === 127 || a >= 224 ||
    (a === 100 && b >= 64 && b <= 127) ||
    (a === 169 && b === 254) ||
    (a === 172 && b >= 16 && b <= 31) ||
    (a === 192 && b === 168) ||
    (a === 192 && b === 0 && (c === 0 || c === 2)) ||
    (a === 192 && b === 88 && c === 99) ||
    (a === 198 && (b === 18 || b === 19)) ||
    (a === 198 && b === 51 && c === 100) ||
    (a === 203 && b === 0 && c === 113)
  );
}

export function sanitizeCandidate(raw, { role, expectedExit } = {}) {
  if (!ROLES.has(role) || (role === 'controller' && !publicIPv4(expectedExit))) return null;
  let candidate;
  if (typeof raw === 'string') candidate = raw;
  else if (raw && typeof raw === 'object') {
    if (raw.sdpMid != null && raw.sdpMid !== '0') return null;
    if (raw.sdpMLineIndex != null && raw.sdpMLineIndex !== 0) return null;
    candidate = raw.candidate;
  }
  if (typeof candidate !== 'string' || candidate.length > 1024 || /[^\x20-\x7e]/.test(candidate)) return null;
  const fields = candidate.trim().split(/ +/);
  if (fields.length < 8 || (fields.length - 8) % 2 !== 0) return null;
  const [foundation, component, protocol, priority, address, port, marker, type] = fields;
  if (!/^candidate:[A-Za-z0-9+/]{1,32}$/.test(foundation) || component !== '1' ||
      protocol.toLowerCase() !== 'udp' || !/^[1-9]\d{0,9}$/.test(priority) ||
      Number(priority) > 0xffffffff || !publicIPv4(address) || !/^[1-9]\d{0,4}$/.test(port) ||
      Number(port) > 65535 || marker !== 'typ' || type !== 'srflx') return null;
  if (role === 'controller' && address !== expectedExit) return null;
  const seen = new Set();
  for (let i = 8; i < fields.length; i += 2) {
    if (!/^[A-Za-z][A-Za-z0-9-]{0,31}$/.test(fields[i]) ||
        !/^[A-Za-z0-9+/:._-]{1,256}$/.test(fields[i + 1]) || seen.has(fields[i])) return null;
    seen.add(fields[i]);
  }
  return {
    candidate: `${foundation} 1 udp ${priority} ${address} ${port} typ srflx raddr 0.0.0.0 rport 0`,
    sdpMid: '0',
    sdpMLineIndex: 0,
  };
}

export function validateCandidate(raw, { sourceRole, expectedExit } = {}) {
  const canonical = sanitizeCandidate(raw, { role: sourceRole, expectedExit });
  if (!canonical || !exactKeys(raw, ['candidate', 'sdpMid', 'sdpMLineIndex']) ||
      raw.candidate !== canonical.candidate || raw.sdpMid !== '0' || raw.sdpMLineIndex !== 0) {
    throw new Error('Invalid direct candidate.');
  }
  return canonical;
}

export function validateDescription(sdp, type) {
  const invalid = () => { throw new Error('Invalid data-only session description.'); };
  if (!['offer', 'answer'].includes(type) || typeof sdp !== 'string' || sdp.length > 14000 ||
      !sdp.endsWith('\r\n') || /[^\x20-\x7e\r\n]/.test(sdp) || /(?:^|[^\r])\n|\r(?!\n)/.test(sdp)) invalid();
  const lines = sdp.slice(0, -2).split('\r\n');
  if (lines.length > 64 || lines.some(line => !line || line.length > 4096)) invalid();
  const counts = new Map();
  const count = key => counts.set(key, (counts.get(key) ?? 0) + 1);
  for (const line of lines) {
    if (/^a=(?:candidate|end-of-candidates)(?::|$)/i.test(line)) invalid();
    if (line === 'v=0') count('version');
    else if (/^o=- \d{1,20} \d{1,20} IN (?:IP4 (?:0\.0\.0\.0|127\.0\.0\.1)|IP6 ::)$/.test(line)) count('origin');
    else if (line === 's=-') count('session');
    else if (line === 't=0 0') count('time');
    else if (/^m=application (?:0|[1-9]\d{0,4}) (?:UDP\/DTLS\/SCTP webrtc-datachannel|DTLS\/SCTP 5000)$/.test(line)) {
      if (Number(line.split(' ')[1]) > 65535) invalid();
      count('media');
    } else if (/^c=IN (?:IP4 (?:0\.0\.0\.0|127\.0\.0\.1)|IP6 ::)$/.test(line)) count('connection');
    else if (line === 'a=group:BUNDLE 0') count('bundle');
    else if (line === 'a=extmap-allow-mixed') count('extmap');
    else if (/^a=msid-semantic: *WMS(?: \*)?$/.test(line)) count('msid');
    else if (/^a=ice-ufrag:[A-Za-z0-9+/]{4,256}$/.test(line)) count('ufrag');
    else if (/^a=ice-pwd:[A-Za-z0-9+/]{22,256}$/.test(line)) count('password');
    else if (/^a=ice-options:[a-z0-9-]+(?: [a-z0-9-]+)*$/.test(line)) {
      if (!line.slice(14).split(' ').includes('trickle')) invalid();
      count('options');
    } else if (/^a=fingerprint:sha-256 (?:[A-Fa-f0-9]{2}:){31}[A-Fa-f0-9]{2}$/.test(line)) count('fingerprint');
    else if (line === `a=setup:${type === 'offer' ? 'actpass' : 'active'}` || (type === 'answer' && line === 'a=setup:passive')) count('setup');
    else if (line === 'a=mid:0') count('mid');
    else if (/^a=sctp-port:[1-9]\d{0,4}$/.test(line)) {
      if (Number(line.slice(12)) > 65535) invalid();
      count('sctp');
    } else if (line === 'a=sctpmap:5000 webrtc-datachannel 1024') {
      count('sctp');
    } else if (/^a=max-message-size:(?:0|[1-9]\d{0,9})$/.test(line)) {
      if (Number(line.slice(19)) > 0xffffffff) invalid();
      count('maximum');
    } else if (/^a=sctp-init:[A-Za-z0-9+/]{1,4000}={0,2}$/.test(line)) count('init');
    else invalid();
  }
  for (const required of ['version', 'origin', 'session', 'time', 'media', 'connection', 'bundle', 'ufrag', 'password', 'options', 'fingerprint', 'setup', 'mid', 'sctp']) {
    if (counts.get(required) !== 1) invalid();
  }
  if ([...counts.values()].some(value => value !== 1) || lines[0] !== 'v=0') invalid();
  return { type, sdp };
}

function base64url(bytes) {
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}

function decode64(value, minimum, maximum) {
  if (typeof value !== 'string' || value.length < 1 || value.length > Math.ceil(maximum * 4 / 3) ||
      !/^[A-Za-z0-9_-]+$/.test(value) || value.length % 4 === 1) throw new Error('Invalid encrypted session message.');
  let binary;
  try { binary = atob(value.replace(/-/g, '+').replace(/_/g, '/') + '='.repeat((4 - value.length % 4) % 4)); }
  catch { throw new Error('Invalid encrypted session message.'); }
  const bytes = Uint8Array.from(binary, character => character.charCodeAt(0));
  if (bytes.length < minimum || bytes.length > maximum || base64url(bytes) !== value) throw new Error('Invalid encrypted session message.');
  return bytes;
}

function jsonData(value, depth = 0, seen = new Set()) {
  if (depth > 8) return false;
  if (value === null || typeof value === 'boolean') return true;
  if (typeof value === 'number') return Number.isFinite(value);
  if (typeof value === 'string') return value.length <= 14000;
  if (typeof value !== 'object' || seen.has(value)) return false;
  seen.add(value);
  let valid;
  if (Array.isArray(value)) {
    valid = value.length <= 128 && Reflect.ownKeys(value).length === value.length + 1 &&
      value.every(item => jsonData(item, depth + 1, seen));
  } else {
    valid = plainObject(value) && Reflect.ownKeys(value).length <= 64 && Reflect.ownKeys(value).every(key => {
      const descriptor = Object.getOwnPropertyDescriptor(value, key);
      return typeof key === 'string' && key.length <= 64 && !['__proto__', 'constructor', 'prototype'].includes(key) &&
        descriptor?.get === undefined && descriptor?.set === undefined && descriptor?.enumerable === true &&
        jsonData(descriptor.value, depth + 1, seen);
    });
  }
  seen.delete(value);
  return valid;
}

export async function createCipher({ key, sid, role, expires } = {}) {
  const invalid = () => { throw new Error('Invalid encrypted session message.'); };
  if (!ROLES.has(role) || typeof sid !== 'string' || !/^[a-f0-9]{32}$/.test(sid) ||
      !Number.isSafeInteger(expires) || expires <= Date.now() || expires > Date.now() + 30 * 60 * 1000 ||
      !globalThis.crypto?.subtle || !globalThis.crypto?.getRandomValues) invalid();
  const rawKey = decode64(key, 32, 32);
  const source = await crypto.subtle.importKey('raw', rawKey, 'HKDF', false, ['deriveKey']);
  rawKey.fill(0);
  const peer = role === 'controller' ? 'guest' : 'controller';
  const salt = encoder.encode(`pst-quickview-app-support-v1:${sid}`);
  const derive = async direction => crypto.subtle.deriveKey({
    name: 'HKDF', hash: 'SHA-256', salt, info: encoder.encode(direction),
  }, source, { name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt']);
  const [sendKey, receiveKey] = await Promise.all([derive(`${role}->${peer}`), derive(`${peer}->${role}`)]);
  const prefix = crypto.getRandomValues(new Uint8Array(8));
  let sendSequence = 0;
  let receiveSequence = 0;
  let receivePrefix;
  let sendTail = Promise.resolve();
  let receiveTail = Promise.resolve();
  const alive = () => { if (Date.now() >= expires) throw new Error('Session expired.'); };
  const aad = (from, sequence) => encoder.encode(JSON.stringify([1, sid, from, sequence, expires]));
  return Object.freeze({
    seal(kind, data) {
      const result = sendTail.then(async () => {
        alive();
        if (!KINDS.has(kind) || !jsonData(data) || sendSequence >= MAX_SEQUENCE) invalid();
        const plaintext = encoder.encode(JSON.stringify({ kind, data }));
        if (plaintext.length > MAX_PLAINTEXT) invalid();
        const sequence = ++sendSequence;
        const iv = new Uint8Array(12);
        iv.set(prefix);
        new DataView(iv.buffer).setUint32(8, sequence, false);
        const ciphertext = await crypto.subtle.encrypt({ name: 'AES-GCM', iv, additionalData: aad(role, sequence), tagLength: 128 }, sendKey, plaintext);
        alive();
        return { v: 1, sid, from: role, seq: sequence, expires, iv: base64url(iv), ct: base64url(new Uint8Array(ciphertext)) };
      });
      sendTail = result.catch(() => {});
      return result;
    },
    open(envelope) {
      const result = receiveTail.then(async () => {
        alive();
        if (!exactKeys(envelope, ['v', 'sid', 'from', 'seq', 'expires', 'iv', 'ct']) ||
            envelope.v !== 1 || envelope.sid !== sid || envelope.from !== peer || envelope.expires !== expires ||
            !Number.isSafeInteger(envelope.seq) || envelope.seq !== receiveSequence + 1 || envelope.seq > MAX_SEQUENCE) invalid();
        const sequence = envelope.seq;
        const iv = decode64(envelope.iv, 12, 12);
        const ciphertext = decode64(envelope.ct, 16, MAX_CIPHERTEXT);
        if (new DataView(iv.buffer, iv.byteOffset, iv.byteLength).getUint32(8, false) !== sequence ||
            (receivePrefix && !receivePrefix.every((byte, index) => iv[index] === byte))) invalid();
        let payload;
        try {
          const plaintext = await crypto.subtle.decrypt({ name: 'AES-GCM', iv, additionalData: aad(peer, sequence), tagLength: 128 }, receiveKey, ciphertext);
          if (plaintext.byteLength > MAX_PLAINTEXT) invalid();
          payload = JSON.parse(decoder.decode(plaintext));
        } catch { invalid(); }
        alive();
        if (!exactKeys(payload, ['kind', 'data']) || !KINDS.has(payload.kind) || !jsonData(payload.data)) invalid();
        receiveSequence = sequence;
        receivePrefix ??= iv.slice(0, 8);
        return payload;
      });
      receiveTail = result.catch(() => {});
      return result;
    },
  });
}

export function validateConfig(raw, { serviceOrigin } = {}) {
  if (!plainObject(raw) || raw.v !== 2 || !ROLES.has(raw.role) ||
      !/^[a-f0-9]{32}$/.test(raw.sid) || !/^[a-f0-9]{64}$/.test(raw.token) ||
      !/^[A-Za-z0-9_-]{43}$/.test(raw.key) || !publicIPv4(raw.expectedExit) ||
      !Number.isSafeInteger(raw.expires) || raw.expires <= Date.now() ||
      raw.expires > Date.now() + 30 * 60 * 1000 || typeof raw.origin !== 'string') {
    throw new Error('Invalid support invitation.');
  }
  const publicOrigin = /^https:\/\/[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.run\.pinggy-free\.link$/.test(raw.origin);
  const loopback = /^http:\/\/127\.0\.0\.1:([1-9]\d{3,4})$/.exec(raw.origin);
  // The automatic queue uses an exact deployment pin supplied by trusted local
  // bootstrap, never a wildcard learned from an invitation or service response.
  const pinned = serviceOrigin !== undefined;
  const canonicalWorker = typeof serviceOrigin === 'string' && /^https:\/\/[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.workers\.dev$/.test(serviceOrigin);
  if (pinned ? (!canonicalWorker || raw.origin !== serviceOrigin)
    : (!publicOrigin && !(raw.role === 'controller' && loopback && Number(loopback[1]) >= 1024 && Number(loopback[1]) <= 65535))) {
    throw new Error('Invalid support invitation.');
  }
  decode64(raw.key, 32, 32);
  return {
    v: 2, role: raw.role, sid: raw.sid, token: raw.token, key: raw.key,
    expires: raw.expires, expectedExit: raw.expectedExit, origin: raw.origin,
  };
}

function boundedText(value, maximum) {
  return typeof value === 'string' && value.length <= maximum && !/[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/.test(value);
}

function identifier(value) {
  return typeof value === 'string' && /^[a-f0-9]{32}$/.test(value);
}

function finiteRange(value, minimum, maximum) {
  return typeof value === 'number' && Number.isFinite(value) && value >= minimum && value <= maximum;
}

function rectangle(value) {
  return finiteRange(value.x, 0, 4096) && finiteRange(value.y, 0, 2160) &&
    finiteRange(value.w, 0, 4096) && finiteRange(value.h, 0, 2160);
}

function color(value) {
  if (typeof value !== 'string' || value.length > 64) return false;
  if (/^#(?:[a-f0-9]{3}|[a-f0-9]{4}|[a-f0-9]{6}|[a-f0-9]{8})$/i.test(value)) return true;
  const match = /^(rgb|rgba)\(\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})(?:\s*,\s*(0(?:\.\d{1,4})?|1(?:\.0{1,4})?|\.\d{1,4}))?\s*\)$/.exec(value);
  return Boolean(match && Number(match[2]) <= 255 && Number(match[3]) <= 255 && Number(match[4]) <= 255 &&
    ((match[1] === 'rgb' && match[5] === undefined) || (match[1] === 'rgba' && match[5] !== undefined)));
}

export function validateAction(raw) {
  const invalid = () => { throw new Error('Invalid support action.'); };
  if (!plainObject(raw) || !identifier(raw.frame) || !identifier(raw.target)) invalid();
  if (raw.kind === 'click') {
    if (!exactKeys(raw, ['frame', 'target', 'kind'])) invalid();
  } else if (raw.kind === 'input') {
    if (!exactKeys(raw, ['frame', 'target', 'kind', 'value']) || !boundedText(raw.value, 2000)) invalid();
  } else if (raw.kind === 'scroll') {
    if (!exactKeys(raw, ['frame', 'target', 'kind', 'deltaY']) || !finiteRange(raw.deltaY, -700, 700)) invalid();
  } else invalid();
  return raw;
}

export function validateFrame(raw) {
  const invalid = () => { throw new Error('Invalid support frame.'); };
  if (!exactKeys(raw, ['v', 'id', 'width', 'height', 'items', 'targets', 'truncated']) || raw.v !== 1 ||
      !identifier(raw.id) || !finiteRange(raw.width, 1, 4096) || !finiteRange(raw.height, 1, 2160) ||
      typeof raw.truncated !== 'boolean' || !Array.isArray(raw.items) || raw.items.length > 600 ||
      !Array.isArray(raw.targets) || raw.targets.length > 80) invalid();
  let textLength = 0;
  for (const item of raw.items) {
    if (!plainObject(item) || !rectangle(item)) invalid();
    if (item.kind === 'box') {
      if (!exactKeys(item, ['kind', 'x', 'y', 'w', 'h', 'fill']) || !color(item.fill)) invalid();
    } else if (item.kind === 'text') {
      if (!exactKeys(item, ['kind', 'x', 'y', 'w', 'h', 'text', 'color', 'size', 'weight']) ||
          !boundedText(item.text, 1000) || !color(item.color) || !finiteRange(item.size, 8, 40) ||
          !finiteRange(item.weight, 100, 900)) invalid();
      textLength += item.text.length;
      if (textLength > 12000) invalid();
    } else invalid();
  }
  const targets = new Set();
  for (const target of raw.targets) {
    const keys = ['id', 'x', 'y', 'w', 'h', 'kind', 'label'];
    if (target?.value !== undefined) keys.push('value');
    if (!exactKeys(target, keys) || !identifier(target.id) || targets.has(target.id) || !rectangle(target) ||
        !['click', 'input', 'scroll'].includes(target.kind) || !boundedText(target.label, 100) ||
        (target.value !== undefined && (target.kind !== 'input' || !boundedText(target.value, 2000)))) invalid();
    targets.add(target.id);
  }
  if (encoder.encode(JSON.stringify(raw)).length > 48 * 1024) invalid();
  return raw;
}

export function validateDataMessage(raw, { sid, sourceRole } = {}) {
  if (!plainObject(raw) || !ROLES.has(sourceRole) || !identifier(sid)) throw new Error('Invalid support data.');
  if (raw.type === 'hello' && exactKeys(raw, ['type', 'v', 'sid']) && raw.v === 1 && raw.sid === sid) return raw;
  if (raw.type === 'heartbeat' && exactKeys(raw, ['type'])) return raw;
  if (raw.type === 'frame' && sourceRole === 'guest' && exactKeys(raw, ['type', 'frame'])) {
    validateFrame(raw.frame);
    return raw;
  }
  if (raw.type === 'action' && sourceRole === 'controller' && exactKeys(raw, ['type', 'action'])) {
    validateAction(raw.action);
    return raw;
  }
  if (raw.type === 'update-handoff' && sourceRole === 'guest' && exactKeys(raw, ['type', 'handoff', 'resumeAuth'])) {
    const handoff = raw.handoff, now = Date.now(), hex64 = value => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
    if (exactKeys(handoff, ['v', 'parentId', 'parentSid', 'nonce', 'targetVersion', 'artifactSha256', 'nativeSessionId', 'originalExpires', 'resumeDeadline', 'resumeTicketSha256'])
      && handoff.v === 1 && identifier(handoff.parentId) && handoff.parentSid === sid && identifier(handoff.nonce)
      && typeof handoff.targetVersion === 'string' && handoff.targetVersion.length <= 128
      && /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9]\d*|\d*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.test(handoff.targetVersion)
      && hex64(handoff.artifactSha256) && identifier(handoff.nativeSessionId) && hex64(handoff.resumeTicketSha256) && hex64(raw.resumeAuth)
      && Number.isSafeInteger(handoff.originalExpires) && handoff.originalExpires > now && handoff.originalExpires <= now + 30 * 60 * 1000
      && Number.isSafeInteger(handoff.resumeDeadline) && handoff.resumeDeadline > now && handoff.resumeDeadline <= handoff.originalExpires && handoff.resumeDeadline <= now + 90000) return raw;
  }
  if (raw.type === 'update-handoff-ack' && sourceRole === 'controller' && exactKeys(raw, ['type', 'nonce', 'proof'])
    && identifier(raw.nonce) && typeof raw.proof === 'string' && /^[a-f0-9]{64}$/.test(raw.proof)) return raw;
  throw new Error('Invalid support data.');
}
