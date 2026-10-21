import { createSupportPeer } from './peer.js';
import { createEphemeralKeyPair, verifyClaim, deriveClaimConfig, validateServiceConfig } from './queue-protocol.js';

const $ = id => document.getElementById(id);
const canvas = $('view'), context = canvas.getContext('2d');
let capability, service, active, rows = [], ready = false, queueBusy = false, ended = false, generation = 0;
let queueTimer, heartbeatTimer, frameTimer, lastWheel = 0;
const removed = new Map();
const requestLabel = id => 'Request ' + id.slice(0, 6).toUpperCase();
const status = (text, kind = '') => { $('status').textContent = text; $('status').className = 'status ' + kind; };

async function api(route, body, { keepalive = false } = {}) {
  const response = await fetch('/api/' + route, {
    method: body === undefined ? 'GET' : 'POST', headers: { Authorization: 'Bearer ' + capability, ...(body === undefined ? {} : { 'Content-Type': 'application/json' }) },
    ...(body === undefined ? {} : { body: JSON.stringify(body) }), credentials: 'omit', redirect: 'error', cache: 'no-store',
    signal: AbortSignal.timeout(8000), keepalive,
  });
  if (!response.ok) throw new Error('unavailable');
  return response.json();
}
function selected() { return active?.frame?.targets.find(target => target.id === $('target').value); }
function controls() {
  const target = selected(), usable = active?.connected && active.frame;
  $('target').disabled = !usable || !active.frame.targets.length;
  $('activate').disabled = !usable || target?.kind !== 'click';
  $('input').disabled = !usable || target?.kind !== 'input';
  $('send-input').disabled = !usable || target?.kind !== 'input';
  $('scroll-up').disabled = !usable || target?.kind !== 'scroll';
  $('scroll-down').disabled = !usable || target?.kind !== 'scroll';
}
function clearView() {
  clearTimeout(frameTimer); if (active) active.frame = undefined;
  context.clearRect(0, 0, canvas.width, canvas.height);
  $('target').replaceChildren(); $('input').value = ''; $('controls').hidden = true; $('placeholder').hidden = false; controls();
}
function selectTarget(id) {
  const target = active?.frame?.targets.find(target => target.id === id); if (!target) return;
  $('target').value = id; $('input').value = target.kind === 'input' ? target.value || '' : ''; controls();
}
function draw(frame, record) {
  if (active !== record || !record.connected || ended) return;
  const selection = $('target').value; record.frame = frame;
  canvas.width = frame.width; canvas.height = frame.height;
  context.fillStyle = '#fff'; context.fillRect(0, 0, frame.width, frame.height);
  for (const item of frame.items) {
    context.save(); context.beginPath(); context.rect(item.x, item.y, item.w, item.h); context.clip();
    if (item.kind === 'box') { context.fillStyle = item.fill; context.fillRect(item.x, item.y, item.w, item.h); }
    else {
      context.fillStyle = item.color; context.font = `${item.weight} ${item.size}px system-ui,sans-serif`; context.textBaseline = 'top';
      const lines = item.text.split('\n');
      for (let index = 0; index < lines.length; index++) context.fillText(lines[index], item.x, item.y + index * item.size * 1.25);
    }
    context.restore();
  }
  $('target').replaceChildren(...frame.targets.map((target, index) => { const option = document.createElement('option'); option.value = target.id; option.textContent = `${target.label || 'Shared control ' + (index + 1)} (${target.kind})`; return option; }));
  if (frame.targets.some(target => target.id === selection)) $('target').value = selection;
  else if (frame.targets.length) selectTarget(frame.targets[0].id);
  $('controls').hidden = !frame.targets.length; $('placeholder').hidden = true; controls();
  status(frame.truncated ? 'Connected. Some app content is omitted by the display limit.' : 'Connected. The owner-approved app window is shared.', 'connected');
  clearTimeout(frameTimer);
  frameTimer = setTimeout(() => { if (active === record && record.frame?.id === frame.id) { clearView(); status('Connected. Waiting for a fresh app view; old content and controls were cleared.'); } }, 3000);
}
function renderQueue() {
  $('queue').replaceChildren(...rows.map(row => {
    const item = document.createElement('li'), dot = document.createElement('span'), details = document.createElement('div'), title = document.createElement('span'), note = document.createElement('small'), button = document.createElement('button');
    dot.className = 'dot' + (row.status === 'claimed' ? ' claimed' : ''); dot.setAttribute('aria-hidden', 'true');
    details.className = 'request-details'; title.textContent = requestLabel(row.id); note.textContent = active?.id === row.id ? active.connected ? 'Connected on this laptop' : 'Connecting on this laptop' : row.status === 'waiting' ? 'Owner approved · waiting' : 'Unavailable · already claimed';
    details.append(title, note); button.type = 'button'; button.textContent = 'Connect';
    button.disabled = !ready || !!active || row.status !== 'waiting' || !$('consent').checked || row.expires <= Date.now();
    button.setAttribute('aria-label', 'Connect to ' + requestLabel(row.id)); button.onclick = () => { void connect(row); };
    item.append(dot, details, button); return item;
  }));
  $('queue-count').textContent = String(rows.filter(row => row.status === 'waiting').length);
}
async function stop(reason = 'stopped', keepalive = false) {
  const record = active;
  generation++; clearInterval(heartbeatTimer); clearView(); active = undefined;
  record?.peer?.close(); $('stop').disabled = true; $('session-heading').textContent = 'No active session';
  if (record) {
    removed.set(record.id, Date.now() + 30 * 60 * 1000);
    while (removed.size > 256) removed.delete(removed.keys().next().value);
    rows = rows.filter(row => row.id !== record.id);
  }
  renderQueue();
  if (reason === 'failed') status('The support connection ended or the service is unavailable. Access is closed.', 'failure');
  else if (reason === 'expired') status('The approved session expired. Access is closed.');
  else status('Support stopped. Access is closed.');
  if (record) try { await api('stop', { id: record.id }, { keepalive }); } catch { /* Lease expiry also removes abandoned requests. */ }
}
async function connect(row) {
  if (!ready || ended || active || !$('consent').checked || row.status !== 'waiting' || row.expires <= Date.now()) return;
  const current = ++generation, record = { id: row.id, connected: false, frame: undefined, peer: undefined };
  active = record; $('consent').checked = false; $('stop').disabled = false; $('session-heading').textContent = requestLabel(row.id); renderQueue();
  status('Checking the private route and connecting to the approved app…');
  try {
    const pair = await createEphemeralKeyPair();
    if (active !== record || current !== generation) return;
    const result = await api('claim', { id: row.id, helperPublicKey: pair.publicKey });
    if (active !== record || current !== generation) { try { await api('stop', { id: row.id }); } catch {} return; }
    const claim = await verifyClaim(result.claim, service);
    if (claim.payload.id !== row.id || claim.payload.guestPublicKey !== row.guestPublicKey || claim.payload.expires !== row.expires || claim.payload.helperPublicKey !== pair.publicKey) throw new Error('claim');
    const config = await deriveClaimConfig(claim.payload, claim.signature, pair.privateKey, 'controller', result.token, service.origin);
    if (active !== record || current !== generation) return;
    record.peer = await createSupportPeer({ config, queue: { origin: service.origin, requestId: row.id },
      onFrame: frame => draw(frame, record), onAction: () => { if (active === record) void stop('failed'); },
      onState(state) {
        if (active !== record || ended) return;
        if (state === 'connected') { record.connected = true; renderQueue(); controls(); status('Connected. Waiting for the owner’s app view…', 'connected'); }
        else if (state.startsWith('failed:')) void stop('failed');
        else if (state === 'negotiating') status('Both sides approved. Establishing a direct encrypted connection…');
        else if (state === 'waiting' || state === 'approved') status('Waiting for the approved app to establish its direct connection…');
      }, onClose: reason => { if (active === record) void stop(reason === 'expired' ? 'expired' : 'stopped'); },
    });
    if (active !== record || current !== generation) { record.peer.close(); return; }
    heartbeatTimer = setInterval(() => { if (active === record) void api('heartbeat', { id: row.id }).catch(() => { if (active === record) void stop('failed'); }); }, 10000);
    await api('heartbeat', { id: row.id });
    if (active === record) await record.peer.approve();
  } catch { if (active === record) await stop('failed'); }
}
async function refreshQueue() {
  if (!ready || ended || queueBusy) return;
  queueBusy = true;
  const startedGeneration = generation, startedActive = active;
  try {
    const result = await api('queue'); if (ended || generation !== startedGeneration || active !== startedActive) return;
    for (const [id, expires] of removed) if (expires <= Date.now()) removed.delete(id);
    rows = result.requests.filter(row => !removed.has(row.id));
    if (active && result.activeId !== active.id && active.peer) { await stop('stopped'); return; }
    renderQueue(); $('queue-status').textContent = rows.length ? 'Requests are current. Connect when you are ready.' : 'No requests yet. Owners can request help from PST QuickView.';
  } catch {
    if (ended || generation !== startedGeneration || active !== startedActive) return;
    rows = []; renderQueue(); $('queue-status').textContent = 'Support service unavailable. The waiting list and shared view are cleared.'; if (active) await stop('failed');
  }
  finally { queueBusy = false; }
}
async function action(target, extra = {}) {
  const record = active, frame = record?.frame;
  if (!record?.connected || !frame || !target || !frame.targets.some(value => value.id === target.id && value.kind === target.kind)) return;
  try { await record.peer.sendAction({ frame: frame.id, target: target.id, kind: target.kind, ...extra }); }
  catch { if (active === record) await stop('failed'); }
}
function hit(event, kind) {
  const frame = active?.frame; if (!active?.connected || !frame) return;
  const rect = canvas.getBoundingClientRect(), scale = Math.min(rect.width / frame.width, rect.height / frame.height);
  const x = (event.clientX - rect.left) / scale, y = (event.clientY - rect.top) / scale;
  return [...frame.targets].reverse().find(target => (!kind || target.kind === kind) && x >= target.x && y >= target.y && x < target.x + target.w && y < target.y + target.h);
}
$('consent').onchange = renderQueue; $('stop').onclick = () => { void stop(); };
$('target').onchange = () => selectTarget($('target').value);
$('activate').onclick = () => { void action(selected()); };
$('send-input').onclick = () => { const target = selected(); if (target?.kind === 'input') void action(target, { value: $('input').value }); };
$('scroll-up').onclick = () => { const target = selected(); if (target?.kind === 'scroll') void action(target, { deltaY: -500 }); };
$('scroll-down').onclick = () => { const target = selected(); if (target?.kind === 'scroll') void action(target, { deltaY: 500 }); };
canvas.onclick = event => { const target = hit(event); if (!target) return; selectTarget(target.id); if (target.kind === 'click') void action(target); else if (target.kind === 'input') $('input').focus(); };
canvas.addEventListener('wheel', event => { const target = hit(event, 'scroll'); if (!target) return; event.preventDefault(); const now = performance.now(); if (now - lastWheel < 150) return; lastWheel = now; const multiplier = event.deltaMode === 1 ? 20 : event.deltaMode === 2 ? 500 : 1; const deltaY = Math.max(-700, Math.min(700, Math.round(event.deltaY * multiplier))); if (deltaY) void action(target, { deltaY }); }, { passive: false });
addEventListener('pagehide', () => { ended = true; ready = false; clearInterval(queueTimer); void stop('stopped', true); capability = undefined; });
try {
  const token = location.hash.slice(1); history.replaceState(null, '', location.pathname);
  if (location.pathname !== '/' || location.search || !/^[a-f0-9]{64}$/.test(token) || !/^http:\/\/127\.0\.0\.1:\d{4,5}$/.test(location.origin) || !context) throw new Error('setup');
  capability = token; service = validateServiceConfig((await api('service')).service);
  $('operator-label').textContent = service.operatorLabel; ready = true; $('consent').disabled = false;
  await refreshQueue(); queueTimer = setInterval(() => { void refreshQueue(); }, 3000);
} catch { ready = false; rows = []; renderQueue(); clearView(); $('queue-status').textContent = 'Dashboard unavailable. Start it from the configured laptop launcher.'; status('No support access is active.', 'failure'); }
