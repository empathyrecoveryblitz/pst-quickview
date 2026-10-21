import http from 'node:http';
import { randomBytes, timingSafeEqual } from 'node:crypto';

const MAX_BODY = 24 * 1024;
const MAX_MESSAGES = 128;
const MAX_CIPHERTEXT = 256 * 1024;
const MAX_REQUESTS = 12000;
const APP_ORIGIN = 'tauri://localhost';
const PUBLIC_ORIGIN = /^https:\/\/[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.run\.pinggy-free\.link$/;
const HEADERS = Object.freeze({
  'Content-Security-Policy': "default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'self'; img-src 'none'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'; object-src 'none'",
  'Referrer-Policy': 'no-referrer',
  'Cache-Control': 'no-store',
  Pragma: 'no-cache',
  'X-Content-Type-Options': 'nosniff',
  'X-Frame-Options': 'DENY',
  'Permissions-Policy': 'camera=(), microphone=(), display-capture=(), geolocation=(), clipboard-read=(), usb=(), serial=(), hid=(), payment=()',
});
const JOIN_PAGE = '<!doctype html><meta charset="utf-8"><title>PST QuickView support invitation</title><h1>PST QuickView support invitation</h1><p>Open PST QuickView on the computer receiving support, paste the complete invitation in Remote Support, and approve the session there.</p><p>This page does not connect to your computer. Keep the complete invitation private.</p>';

export function validSupportOrigin(origin) {
  return typeof origin === 'string' && PUBLIC_ORIGIN.test(origin);
}

function exactKeys(value, keys) {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
    && Object.keys(value).length === keys.length && keys.every(key => Object.hasOwn(value, key));
}
function canonical64(value, min, max) {
  return typeof value === 'string' && value.length >= min && value.length <= max
    && /^[A-Za-z0-9_-]+$/.test(value)
    && Buffer.from(value, 'base64url').toString('base64url') === value;
}
function authorized(header, token) {
  return typeof header === 'string' && /^Bearer [0-9a-f]{64}$/.test(header)
    && timingSafeEqual(Buffer.from(header.slice(7)), Buffer.from(token));
}
async function readJSON(req) {
  if (req.headers['content-type'] !== 'application/json') { req.resume(); throw 415; }
  const length = req.headers['content-length'];
  if (length !== undefined && (!/^\d+$/.test(length) || Number(length) > MAX_BODY)) { req.resume(); throw 413; }
  let size = 0;
  let oversized = false;
  const chunks = [];
  for await (const chunk of req) {
    size += chunk.length;
    if (size > MAX_BODY) { oversized = true; chunks.length = 0; }
    else if (!oversized) chunks.push(chunk);
  }
  if (oversized) throw 413;
  try { return JSON.parse(Buffer.concat(chunks).toString('utf8')); } catch { throw 400; }
}
function listen(server) {
  return new Promise((resolve, reject) => {
    server.once('error', reject);
    server.listen(0, '127.0.0.1', () => {
      server.off('error', reject);
      resolve(server.address().port);
    });
  });
}

/** Only encrypted connection setup enters these queues; frames/actions use WebRTC. */
export async function createSupportServer({ assets, ttlMs = 30 * 60 * 1000, onStop = () => {} }) {
  if (!assets || ['html', 'controller', 'protocol', 'peer', 'css'].some(key => typeof assets[key] !== 'string')) {
    throw new TypeError('Support assets are required');
  }
  if (!Number.isSafeInteger(ttlMs) || ttlMs < 1 || ttlMs > 30 * 60 * 1000) throw new TypeError('Invalid lifetime');
  const sid = randomBytes(16).toString('hex');
  const controllerToken = randomBytes(32).toString('hex');
  const guestToken = randomBytes(32).toString('hex');
  const expiresAt = Date.now() + ttlMs;
  const queues = { controller: [], guest: [] };
  const servers = [];
  let guestOrigin, controllerPort, guestPort, timer;
  let bytes = 0, requests = 0, closed = false;

  async function close() {
    if (closed) return;
    closed = true;
    clearTimeout(timer);
    queues.controller.length = 0;
    queues.guest.length = 0;
    await Promise.all(servers.map(server => new Promise(resolve => {
      server.close(resolve);
      server.closeAllConnections();
    })));
  }
  async function stop(reason) {
    if (closed) return;
    await close();
    try { await onStop({ reason }); } catch { /* No raw callback diagnostics. */ }
  }
  function listener(role) {
    const server = http.createServer(async (req, res) => {
      let cors = {};
      const respond = (status, body, type = 'application/json; charset=utf-8') => {
        if (res.destroyed || res.writableEnded) return;
        const value = typeof body === 'string' ? body : JSON.stringify(body);
        res.writeHead(status, { ...HEADERS, ...cors, 'Content-Type': type, 'Content-Length': Buffer.byteLength(value) });
        res.end(value);
      };
      const reject = status => respond(status, { error: status });
      try {
        if (closed || Date.now() >= expiresAt) return reject(410);
        if (++requests > MAX_REQUESTS) return reject(429);
        const origin = role === 'controller' ? `http://127.0.0.1:${controllerPort}` : guestOrigin;
        if (!origin || req.headers.host !== new URL(origin).host) return reject(421);
        const url = new URL(req.url, origin);
        if (url.origin !== origin || url.pathname + url.search !== req.url) return reject(400);
        const source = req.headers.origin;
        const allowedOrigin = source === origin || (role === 'guest' && source === APP_ORIGIN);
        if (source !== undefined && !allowedOrigin) return reject(403);
        if (role === 'guest' && allowedOrigin) cors = { 'Access-Control-Allow-Origin': source, Vary: 'Origin' };

        if (req.method === 'GET' && url.search === '') {
          if (role === 'guest' && url.pathname === '/join') return respond(200, JOIN_PAGE, 'text/html; charset=utf-8');
          const files = {
            '/': [assets.html, 'text/html; charset=utf-8'],
            '/support/controller.js': [assets.controller, 'application/javascript; charset=utf-8'],
            '/support/protocol.js': [assets.protocol, 'application/javascript; charset=utf-8'],
            '/support/peer.js': [assets.peer, 'application/javascript; charset=utf-8'],
            '/support/support.css': [assets.css, 'text/css; charset=utf-8'],
          };
          if (role === 'controller' && Object.hasOwn(files, url.pathname)) {
            return respond(200, files[url.pathname][0], files[url.pathname][1]);
          }
        }
        const route = url.pathname;
        const api = route === '/api/poll' || route === '/api/send' || (role === 'controller' && route === '/api/stop');
        if (!api) return reject(404);
        if (req.method === 'OPTIONS') {
          if (role !== 'guest' || !allowedOrigin) return reject(403);
          const method = req.headers['access-control-request-method'];
          const requested = (req.headers['access-control-request-headers'] || '').toLowerCase().split(',').map(value => value.trim()).filter(Boolean);
          if (!['GET', 'POST'].includes(method) || requested.some(header => !['authorization', 'content-type', 'x-pinggy-no-screen'].includes(header))) return reject(403);
          cors = { ...cors, 'Access-Control-Allow-Methods': 'GET, POST', 'Access-Control-Allow-Headers': 'Authorization, Content-Type, X-Pinggy-No-Screen', 'Access-Control-Max-Age': '300' };
          return respond(204, '');
        }
        if (source === undefined && role === 'guest') return reject(403);
        if (req.headers['sec-fetch-site'] === 'cross-site' && !(role === 'guest' && source === APP_ORIGIN)) return reject(403);
        if (!authorized(req.headers.authorization, role === 'controller' ? controllerToken : guestToken)) return reject(401);
        if (route === '/api/poll') {
          if (req.method !== 'GET') return reject(405);
          if ([...url.searchParams.keys()].length !== 1 || !url.searchParams.has('after') || !/^(0|[1-9]\d{0,2})$/.test(url.searchParams.get('after'))) return reject(400);
          const after = Number(url.searchParams.get('after'));
          if (after > MAX_MESSAGES) return reject(400);
          return respond(200, { messages: queues[role === 'guest' ? 'controller' : 'guest'].filter(item => item.seq > after), expiresAt });
        }
        if (req.method !== 'POST' || url.search !== '') return reject(405);
        if (!allowedOrigin) return reject(403);
        const body = await readJSON(req);
        if (closed || Date.now() >= expiresAt) return reject(410);
        if (route === '/api/stop') {
          if (!exactKeys(body, [])) return reject(400);
          respond(200, { accepted: true });
          setImmediate(() => { void stop('controller_stopped'); });
          return;
        }
        if (!exactKeys(body, ['v', 'sid', 'from', 'seq', 'expires', 'iv', 'ct']) || body.v !== 1 || body.sid !== sid
          || body.from !== role || body.expires !== expiresAt || !Number.isSafeInteger(body.seq) || body.seq < 1 || body.seq > MAX_MESSAGES
          || !canonical64(body.iv, 16, 16) || !canonical64(body.ct, 22, 22000)) return reject(400);
        const queue = queues[role];
        if (body.seq <= queue.length) {
          if (Object.keys(body).some(key => body[key] !== queue[body.seq - 1][key])) return reject(409);
          return respond(200, { accepted: true });
        }
        if (body.seq !== queue.length + 1) return reject(409);
        const size = Buffer.from(body.ct, 'base64url').length;
        if (bytes + size > MAX_CIPHERTEXT) return reject(413);
        bytes += size;
        queue.push(body);
        respond(200, { accepted: true });
      } catch (error) {
        reject(Number.isInteger(error) && error >= 400 && error <= 499 ? error : 400);
      }
    });
    server.requestTimeout = 15000;
    server.headersTimeout = 10000;
    server.keepAliveTimeout = 1000;
    server.timeout = 5000;
    server.maxConnections = 16;
    server.maxHeadersCount = 32;
    server.on('clientError', (_error, socket) => socket.destroy());
    servers.push(server);
    return server;
  }
  try {
    controllerPort = await listen(listener('controller'));
    guestPort = await listen(listener('guest'));
  } catch (error) { await close(); throw error; }
  timer = setTimeout(() => { void stop('expired'); }, Math.max(1, expiresAt - Date.now()));
  return {
    sid, controllerToken, guestToken, expiresAt, controllerPort, guestPort, close,
    setGuestOrigin(origin) {
      if (!validSupportOrigin(origin)) throw new TypeError('Unsupported support origin');
      if (guestOrigin && guestOrigin !== origin) throw new Error('Origin already configured');
      guestOrigin = origin;
    },
  };
}
