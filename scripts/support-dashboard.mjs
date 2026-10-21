#!/usr/bin/env node
import http from 'node:http';
import crypto from 'node:crypto';
import fs from 'node:fs/promises';
import { constants as fsConstants } from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { publicIPv4 } from '../public/support/protocol.js';
import { createClaim, verifyClaim, validateServiceConfig } from '../public/support/queue-protocol.js';

const DIRECTORY = path.dirname(fileURLToPath(import.meta.url));
const ID = /^[a-f0-9]{32}$/;
const TOKEN = /^[a-f0-9]{64}$/;
const PUBLIC_KEY = /^[A-Za-z0-9_-]{87}$/;
const MAX_BODY = 4096;
const PRIVATE_MODE = 0o077;
const exact = (value, keys) => value && typeof value === 'object' && !Array.isArray(value)
  && Object.keys(value).length === keys.length && keys.every(key => Object.hasOwn(value, key));
const validPublicKey = value => typeof value === 'string' && PUBLIC_KEY.test(value)
  && Buffer.from(value, 'base64url').length === 65 && Buffer.from(value, 'base64url')[0] === 4
  && Buffer.from(value, 'base64url').toString('base64url') === value;

export function validateDashboardService(value) {
  try { return validateServiceConfig(value); } catch { throw new Error('service_not_configured'); }
}

export async function loadOperatorIdentity(configPath, service) {
  if (!path.isAbsolute(configPath)) throw new Error('private_config_required');
  const directory = await fs.lstat(path.dirname(configPath));
  if (!directory.isDirectory() || directory.isSymbolicLink() || (directory.mode & PRIVATE_MODE)
    || (process.getuid && directory.uid !== process.getuid())) throw new Error('private_config_required');
  const handle = await fs.open(configPath, fsConstants.O_RDONLY | fsConstants.O_NOFOLLOW);
  let identity;
  try {
    const stat = await handle.stat();
    if (!stat.isFile() || (stat.mode & PRIVATE_MODE) || stat.size > 8192
      || (process.getuid && stat.uid !== process.getuid())) throw new Error('private_config_required');
    identity = JSON.parse(await handle.readFile('utf8'));
  } finally { await handle.close(); }
  if (!exact(identity, ['v', 'adminToken', 'operatorSigningJwk']) || identity.v !== 3 || !TOKEN.test(identity.adminToken)) throw new Error('private_config_invalid');
  const jwk = identity.operatorSigningJwk;
  if (!jwk || jwk.kty !== 'EC' || jwk.crv !== 'P-256'
    || !['x', 'y', 'd'].every(key => typeof jwk[key] === 'string' && /^[A-Za-z0-9_-]{43}$/.test(jwk[key])
      && Buffer.from(jwk[key], 'base64url').length === 32 && Buffer.from(jwk[key], 'base64url').toString('base64url') === jwk[key])) throw new Error('private_config_invalid');
  const publicKey = Buffer.concat([Buffer.from([4]), Buffer.from(jwk.x, 'base64url'), Buffer.from(jwk.y, 'base64url')]);
  const pinned = Buffer.from(service.operatorPublicKey, 'base64url');
  if (publicKey.length !== pinned.length || !crypto.timingSafeEqual(publicKey, pinned)) throw new Error('operator_key_mismatch');
  const operatorSigningKey = await crypto.webcrypto.subtle.importKey('jwk', jwk, { name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign']);
  return { adminToken: identity.adminToken, operatorSigningKey };
}

async function readJson(response, maximum = 32 * 1024) {
  if (!response.ok || !response.body) throw new Error('service_unavailable');
  const reader = response.body.getReader();
  let total = 0;
  const chunks = [];
  try {
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      total += value.byteLength;
      if (total > maximum) throw new Error('service_unavailable');
      chunks.push(value);
    }
  } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
  return JSON.parse(Buffer.concat(chunks).toString('utf8'));
}

function queueRows(value, now) {
  if (!exact(value, ['requests']) || !Array.isArray(value.requests) || value.requests.length > 64) throw new Error('service_unavailable');
  const ids = new Set();
  return value.requests.map(row => {
    if (!exact(row, ['v', 'id', 'guestPublicKey', 'expires', 'status']) || row.v !== 3 || !ID.test(row.id)
      || ids.has(row.id) || !validPublicKey(row.guestPublicKey) || !['waiting', 'claimed'].includes(row.status)
      || !Number.isSafeInteger(row.expires) || row.expires > now + 30 * 60 * 1000) throw new Error('service_unavailable');
    ids.add(row.id); return { ...row };
  }).filter(row => row.expires > now);
}

/** Local authenticated proxy only. No frame, action, or mailbox endpoint exists. */
export async function createOperatorDashboard({ service: rawService, adminToken, operatorSigningKey, assets,
  fetchImpl = fetch, now = Date.now, onStop = () => {} } = {}) {
  const service = validateDashboardService(rawService);
  if (!TOKEN.test(adminToken) || operatorSigningKey?.type !== 'private' || operatorSigningKey.algorithm?.name !== 'ECDSA'
    || operatorSigningKey.algorithm.namedCurve !== 'P-256'
    || !exact(assets, ['html', 'dashboard', 'css', 'protocol', 'peer', 'queueProtocol'])
    || Object.values(assets).some(value => typeof value !== 'string')) throw new Error('dashboard_configuration_invalid');
  const capability = crypto.randomBytes(32).toString('hex');
  let origin, active = null, claiming = false, pendingId = null, cancelledClaim = false, closing = false;
  const sockets = new Set();
  const fetchJson = async (route, token, method = 'GET', body, maximum) => readJson(await fetchImpl(service.origin + route, {
    method, headers: { Authorization: 'Bearer ' + token, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }), credentials: 'omit', redirect: 'error',
    cache: 'no-store', signal: AbortSignal.timeout(6000),
  }), maximum);
  const stopRecord = async record => {
    if (!record) return;
    try { await fetchJson(`/v3/requests/${record.id}/stop`, record.token, 'POST', {}); } catch { /* The remote lease expires independently. */ }
  };
  const clearActive = async (expected = active) => { if (active !== expected) return; const old = active; active = null; await stopRecord(old); };
  const queue = async () => queueRows(await fetchJson('/v3/admin/queue', adminToken), now());
  const reply = (response, code, body, type = 'application/json; charset=utf-8') => {
    if (response.destroyed) return;
    response.writeHead(code, {
      'Content-Type': type, 'Cache-Control': 'no-store', 'Referrer-Policy': 'no-referrer',
      'X-Content-Type-Options': 'nosniff', 'Cross-Origin-Resource-Policy': 'same-origin',
      'Permissions-Policy': 'camera=(), microphone=(), display-capture=(), geolocation=(), usb=(), serial=(), hid=()',
      'Content-Security-Policy': `default-src 'none'; script-src 'self'; style-src 'self'; connect-src 'self' ${service.origin}; img-src 'none'; font-src 'none'; frame-src 'none'; frame-ancestors 'none'; object-src 'none'; base-uri 'none'; form-action 'none'`,
    });
    response.end(type.startsWith('application/json') ? JSON.stringify(body) : body);
  };
  const authenticated = request => {
    const authorization = request.headers.authorization;
    if (typeof authorization !== 'string' || !authorization.startsWith('Bearer ')) return false;
    const token = Buffer.from(authorization.slice(7));
    return token.length === capability.length && crypto.timingSafeEqual(token, Buffer.from(capability));
  };
  const bodyJson = async request => {
    if (request.headers['content-type'] !== 'application/json' || Number(request.headers['content-length']) > MAX_BODY) throw new Error('invalid_request');
    let size = 0; const chunks = [];
    for await (const chunk of request) { size += chunk.length; if (size > MAX_BODY) throw new Error('invalid_request'); chunks.push(chunk); }
    return JSON.parse(Buffer.concat(chunks).toString('utf8'));
  };
  const server = http.createServer(async (request, response) => {
    const address = request.socket.remoteAddress;
    if (closing || !['127.0.0.1', '::ffff:127.0.0.1'].includes(address) || request.headers.host !== origin?.slice(7)) { reply(response, 403, { error: 'unavailable' }); return; }
    let url;
    try { url = new URL(request.url, origin); } catch { reply(response, 400, { error: 'invalid_request' }); return; }
    if (url.origin !== origin || url.search || url.hash || request.url !== url.pathname) { reply(response, 400, { error: 'invalid_request' }); return; }
    const asset = {
      '/': ['html', 'text/html; charset=utf-8'], '/dashboard.js': ['dashboard', 'text/javascript; charset=utf-8'],
      '/dashboard.css': ['css', 'text/css; charset=utf-8'], '/protocol.js': ['protocol', 'text/javascript; charset=utf-8'],
      '/peer.js': ['peer', 'text/javascript; charset=utf-8'], '/queue-protocol.js': ['queueProtocol', 'text/javascript; charset=utf-8'],
    }[url.pathname];
    if (asset && request.method === 'GET') { reply(response, 200, assets[asset[0]], asset[1]); return; }
    if (!url.pathname.startsWith('/api/')) { reply(response, 404, { error: 'unavailable' }); return; }
    const validOrigin = request.headers.origin === origin || (request.method === 'GET' && !request.headers.origin && request.headers['sec-fetch-site'] === 'same-origin');
    if (!validOrigin || !authenticated(request)) { reply(response, 403, { error: 'unavailable' }); return; }
    const beganActive = active;
    try {
      if (request.method === 'GET' && url.pathname === '/api/service') { reply(response, 200, { service }); return; }
      if (request.method === 'GET' && url.pathname === '/api/queue') {
        const requests = await queue();
        if (active && active === beganActive && (!requests.some(row => row.id === active.id && row.status === 'claimed') || active.expires <= now())) await clearActive(beganActive);
        reply(response, 200, { requests, activeId: active?.id ?? null }); return;
      }
      if (request.method === 'POST' && url.pathname === '/api/claim') {
        const input = await bodyJson(request);
        if (!exact(input, ['id', 'helperPublicKey']) || !ID.test(input.id) || !validPublicKey(input.helperPublicKey)) { reply(response, 400, { error: 'invalid_request' }); return; }
        if (active || claiming) { reply(response, 409, { error: 'one_active_session' }); return; }
        claiming = true; pendingId = input.id; cancelledClaim = false;
        try {
          const row = (await queue()).find(row => row.id === input.id && row.status === 'waiting');
          if (!row) { reply(response, 409, { error: 'request_unavailable' }); return; }
          const route = await readJson(await fetchImpl('https://am.i.mullvad.net/json', { redirect: 'error', cache: 'no-store', signal: AbortSignal.timeout(6000) }), 8192);
          if (route.mullvad_exit_ip !== true || !publicIPv4(route.ip)) throw new Error('vpn_not_verified');
          const claim = await createClaim(row, input.helperPublicKey, route.ip, operatorSigningKey);
          const result = await fetchJson(`/v3/admin/requests/${input.id}/claim`, adminToken, 'POST', claim, 8192);
          if (!exact(result, ['token', 'claim']) || !TOKEN.test(result.token)) throw new Error('service_unavailable');
          const verified = await verifyClaim(result.claim, service);
          if (JSON.stringify(verified) !== JSON.stringify(claim)) throw new Error('service_unavailable');
          if (closing || cancelledClaim || response.destroyed) { await stopRecord({ id: row.id, token: result.token }); reply(response, 409, { error: 'request_unavailable' }); return; }
          active = { id: row.id, token: result.token, expires: claim.payload.expires };
          reply(response, 200, result); return;
        } finally { claiming = false; pendingId = null; cancelledClaim = false; }
      }
      if (request.method === 'POST' && ['/api/heartbeat', '/api/stop'].includes(url.pathname)) {
        const input = await bodyJson(request);
        if (!exact(input, ['id']) || !ID.test(input.id)) { reply(response, 400, { error: 'invalid_request' }); return; }
        if (url.pathname === '/api/stop' && pendingId === input.id) { cancelledClaim = true; reply(response, 200, { stopped: true }); return; }
        if (!active || active.id !== input.id) { reply(response, 409, { error: 'request_unavailable' }); return; }
        if (url.pathname === '/api/stop') { await clearActive(); reply(response, 200, { stopped: true }); return; }
        if (active.expires <= now()) throw new Error('session_expired');
        const record = active;
        await fetchJson(`/v3/requests/${record.id}/heartbeat`, record.token, 'POST', {});
        if (active !== record) { reply(response, 409, { error: 'request_unavailable' }); return; }
        reply(response, 200, { active: true }); return;
      }
      reply(response, 404, { error: 'unavailable' });
    } catch (error) {
      if (pendingId) cancelledClaim = true;
      await clearActive(beganActive);
      reply(response, 503, { error: error.message === 'vpn_not_verified' ? 'vpn_not_verified' : 'service_unavailable' });
    }
  });
  server.requestTimeout = 8000; server.headersTimeout = 8000; server.keepAliveTimeout = 1000;
  server.on('connection', socket => { sockets.add(socket); socket.once('close', () => sockets.delete(socket)); });
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  origin = `http://127.0.0.1:${server.address().port}`;
  return {
    origin, capability,
    async close() {
      if (closing) return;
      closing = true; cancelledClaim = true; await clearActive();
      await new Promise(resolve => { server.close(resolve); for (const socket of sockets) socket.destroy(); });
      onStop();
    },
  };
}

export async function launchOperatorDashboard({ configPath, servicePath = path.join(DIRECTORY, '../public/support/service.json') } = {}) {
  const service = validateDashboardService(JSON.parse(await fs.readFile(servicePath, 'utf8')));
  const identity = await loadOperatorIdentity(configPath, service);
  const names = { html: 'dashboard.html', dashboard: 'dashboard.js', css: 'dashboard.css', protocol: 'protocol.js', peer: 'peer.js', queueProtocol: 'queue-protocol.js' };
  const assets = Object.fromEntries(await Promise.all(Object.entries(names).map(async ([key, name]) => [key, await fs.readFile(path.join(DIRECTORY, '../public/support', name), 'utf8')])));
  const dashboard = await createOperatorDashboard({ service, ...identity, assets });
  let stateDir, stateFile;
  try {
    stateDir = await fs.mkdtemp(path.join(os.tmpdir(), 'pst-support-dashboard-')); await fs.chmod(stateDir, 0o700);
    stateFile = path.join(stateDir, 'dashboard.json');
    await fs.writeFile(stateFile, JSON.stringify({ pid: process.pid, dashboardURL: `${dashboard.origin}/#${dashboard.capability}` }), { mode: 0o600, flag: 'wx' });
  } catch { await dashboard.close(); if (stateFile) await fs.unlink(stateFile).catch(() => {}); if (stateDir) await fs.rmdir(stateDir).catch(() => {}); throw new Error('private_state_unavailable'); }
  return { stateFile, async close() { await dashboard.close(); await fs.unlink(stateFile).catch(() => {}); await fs.rmdir(stateDir).catch(() => {}); } };
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const args = process.argv.slice(2);
  if (args.length === 1 && args[0] === '--help') {
    process.stdout.write('Usage: node scripts/support-dashboard.mjs [--config /absolute/private/operator.json]\nStarts the local operator dashboard using an already configured support service and private operator identity. No account, key, service, tunnel, or identity is created.\n');
  } else {
    const defaultRoot = process.env.XDG_CONFIG_HOME || path.join(os.homedir(), '.config');
    const configPath = args.length === 2 && args[0] === '--config' ? args[1] : args.length === 0 ? path.join(defaultRoot, 'pst-quickview-support/operator.json') : undefined;
    let running, stopping = false;
    const stop = async () => { if (stopping) return; stopping = true; await running?.close(); };
    process.once('SIGINT', () => { void stop(); }); process.once('SIGTERM', () => { void stop(); });
    (async () => {
      if (!configPath || !path.isAbsolute(configPath)) throw new Error('private_config_required');
      running = await launchOperatorDashboard({ configPath });
      if (stopping) { await running.close(); return; }
      process.stdout.write(JSON.stringify({ event: 'dashboard_ready', stateFile: running.stateFile }) + '\n');
    })().catch(async error => {
      const reasons = new Set(['service_not_configured', 'private_config_required', 'private_config_invalid', 'operator_key_mismatch', 'private_state_unavailable']);
      process.stderr.write(JSON.stringify({ event: 'dashboard_unavailable', reason: reasons.has(error.message) ? error.message : 'setup_required' }) + '\n');
      process.exitCode = 1; await stop();
    });
  }
}
