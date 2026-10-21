import { createSupportPeer } from './peer.js';
import { createEphemeralKeyPair, verifyClaim, deriveClaimConfig, validateServiceConfig, validateUpdateHandoff, canonicalUpdateHandoff, ackProof, resumedGuestProof, resumedClaimProof, verifyUpdateProof } from './queue-protocol.js';

const $ = id => document.getElementById(id);
const canvas = $('view'), context = canvas.getContext('2d');
let capability, service, active, rows = [], ready = false, queueBusy = false, ended = false, generation = 0;
let queueTimer, heartbeatTimer, frameTimer, updateTimer, lastWheel = 0, lastQueueRender;
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
  const target = selected(), usable = active?.connected && active.phase === 'claimed' && active.frame;
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
  if (active !== record || !record.connected || record.phase !== 'claimed' || ended) return;
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
  const signature = JSON.stringify([ready, active?.id ?? null, active?.connected === true, active?.phase, $('consent').checked,
    rows.map(row => [row.id, row.guestPublicKey, row.status, row.expires, row.expires <= Date.now()])]);
  // Polling must not detach an unchanged keyboard target. On a real list change,
  // restore focus to the same request rather than to its previous list position.
  if (signature === lastQueueRender) return;
  lastQueueRender = signature;
  const focusedRequest = document.activeElement?.dataset?.requestId;
  let focusButton;
  $('queue').replaceChildren(...rows.map(row => {
    const item = document.createElement('li'), dot = document.createElement('span'), details = document.createElement('div'), title = document.createElement('span'), note = document.createElement('small'), button = document.createElement('button');
    dot.className = 'dot' + (row.status === 'claimed' ? ' claimed' : ''); dot.setAttribute('aria-hidden', 'true');
    details.className = 'request-details'; title.textContent = requestLabel(row.id); note.textContent = active?.id === row.id ? ['updating', 'resuming'].includes(active.phase) ? 'Updating · reconnecting to this laptop' : active.connected ? 'Connected on this laptop' : 'Connecting on this laptop' : row.status === 'waiting' ? 'Owner approved · waiting' : 'Unavailable · already claimed';
    details.append(title, note); button.type = 'button'; button.textContent = 'Connect';
    button.dataset.requestId = row.id;
    button.disabled = !ready || !!active || row.status !== 'waiting' || !$('consent').checked || row.expires <= Date.now();
    button.setAttribute('aria-label', 'Connect to ' + requestLabel(row.id)); button.onclick = () => { void connect(row); };
    if (row.id === focusedRequest) focusButton = button;
    item.append(dot, details, button); return item;
  }));
  $('queue-count').textContent = String(rows.filter(row => row.status === 'waiting').length);
  if (focusedRequest) {
    if (focusButton && !focusButton.disabled) focusButton.focus({ preventScroll: true });
    else if (active && !$('stop').disabled) $('stop').focus({ preventScroll: true });
    else $('consent').focus({ preventScroll: true });
  }
}
function clearGrant(record) {
  clearTimeout(updateTimer);
  if (record?.grant) record.grant.resumeAuth = '';
  if (record) record.grant = undefined;
}
function startHeartbeat(record) {
  clearInterval(heartbeatTimer);
  heartbeatTimer = setInterval(() => {
    if (active !== record) return;
    const phase = record.phase;
    void api('heartbeat', { id: record.id }).catch(() => { if (active === record && record.phase === phase) void stop('failed'); });
  }, 10000);
}
async function acceptUpdate(packet, record) {
  if (active !== record || ended || record.grant || record.phase !== 'claimed') return;
  try {
    const handoff = validateUpdateHandoff(packet.handoff);
    if (!/^[a-f0-9]{64}$/.test(packet.resumeAuth) || handoff.parentId !== record.id || handoff.parentSid !== record.sid || handoff.originalExpires !== record.expires) throw new Error('update');
    const grant = { handoff, resumeAuth: packet.resumeAuth };
    record.grant = grant; record.phase = 'updating'; record.connected = false; generation++;
    clearView(); renderQueue(); status('Updating the app. Sharing and controls are paused; waiting for its secure restart…');
    updateTimer = setTimeout(() => { if (active === record && record.grant === grant) void stop('expired'); }, Math.max(1, handoff.resumeDeadline - Date.now()));
    const result = await api('handoff', { id: record.id, handoff, resumeAuth: grant.resumeAuth });
    if (active !== record || record.grant !== grant || ended) return;
    if (result.accepted !== true) throw new Error('update');
    startHeartbeat(record);
    const proof = await ackProof(grant.resumeAuth, handoff);
    if (active !== record || record.grant !== grant || ended) return;
    if (!await record.peer.sendUpdateHandoffAck({ nonce: handoff.nonce, proof })) throw new Error('update');
  } catch { if (active === record) await stop('failed'); }
}
async function stop(reason = 'stopped', keepalive = false) {
  const record = active;
  generation++; clearInterval(heartbeatTimer); clearView(); active = undefined;
  clearGrant(record);
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
  const current = ++generation, record = { id: row.id, phase: 'claimed', connected: false, frame: undefined, peer: undefined };
  active = record; $('consent').checked = false; $('stop').disabled = false; $('session-heading').textContent = requestLabel(row.id); renderQueue();
  status('Checking the private route and connecting to the approved app…');
  await connectPeer(row, record, current, false);
}
async function resume(row, old) {
  if (active !== old || old.phase !== 'updating' || !old.grant || ended) return;
  const grant = old.grant;
  try {
    if (canonicalUpdateHandoff(row.handoff) !== canonicalUpdateHandoff(grant.handoff)
      || !verifyUpdateProof(await resumedGuestProof(grant.resumeAuth, grant.handoff, row.guestPublicKey, row.guestTokenSha256), row.resumeGuestProof)) throw new Error('resume');
    if (active !== old || old.grant !== grant || ended) return;
    old.peer.suspendForUpdate();
    const current = ++generation, record = { id: old.id, phase: 'resuming', connected: false, frame: undefined, peer: undefined, grant };
    old.grant = undefined; clearInterval(heartbeatTimer); clearView(); active = record; renderQueue();
    // Keep the original, short restart deadline even while fresh keys negotiate.
    clearTimeout(updateTimer);
    updateTimer = setTimeout(() => { if (active === record && record.grant === grant) void stop('expired'); }, Math.max(1, grant.handoff.resumeDeadline - Date.now()));
    status('The updated app returned. Verifying fresh keys before sharing resumes…');
    await connectPeer(row, record, current, true);
  } catch { if (active === old) await stop('failed'); }
}
async function connectPeer(row, record, current, resuming) {
  try {
    const pair = await createEphemeralKeyPair();
    if (active !== record || current !== generation) return;
    const result = await api(resuming ? 'resume' : 'claim', { id: row.id, helperPublicKey: pair.publicKey, ...(resuming ? { resumeAuth: record.grant.resumeAuth } : {}) });
    if (active !== record || current !== generation) { try { await api('stop', { id: row.id }); } catch {} return; }
    const claim = await verifyClaim(result.claim, service);
    if (claim.payload.id !== row.id || claim.payload.guestPublicKey !== row.guestPublicKey || claim.payload.expires !== row.expires || claim.payload.helperPublicKey !== pair.publicKey) throw new Error('claim');
    if (resuming && (!record.grant || !verifyUpdateProof(await resumedClaimProof(record.grant.resumeAuth, record.grant.handoff, claim), claim.resumeProof))) throw new Error('resume');
    const config = await deriveClaimConfig(claim.payload, claim.signature, pair.privateKey, 'controller', result.token, service.origin);
    if (active !== record || current !== generation) return;
    record.sid = config.sid; record.expires = config.expires;
    record.peer = await createSupportPeer({ config, queue: { origin: service.origin, requestId: row.id },
      onFrame: frame => draw(frame, record), onAction: () => { if (active === record) void stop('failed'); },
      onUpdateHandoff: packet => { if (active === record) void acceptUpdate(packet, record); },
      onState(state) {
        if (active !== record || ended) return;
        if (state === 'connected') {
          const finish = async () => {
            if (resuming) { clearGrant(record); if ((await api('resumed', { id: record.id })).accepted !== true) throw new Error('resume'); }
            if (active !== record || ended) return;
            record.phase = 'claimed'; record.connected = true; renderQueue(); controls(); status('Connected. Waiting for the owner’s app view…', 'connected');
          };
          void finish().catch(() => { if (active === record) void stop('failed'); });
        }
        else if (state === 'updating') { record.connected = false; clearView(); controls(); status('Updating the app. Sharing and controls are paused.'); }
        else if (state.startsWith('failed:')) void stop('failed');
        else if (state === 'negotiating') status('Both sides approved. Establishing a direct encrypted connection…');
        else if (state === 'waiting' || state === 'approved') status('Waiting for the approved app to establish its direct connection…');
      }, onClose: reason => { if (active === record) void stop(reason === 'expired' ? 'expired' : 'stopped'); },
    });
    if (active !== record || current !== generation) { record.peer.close(); return; }
    startHeartbeat(record);
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
    if (active?.phase === 'updating' && active.grant) {
      const row = rows.find(row => row.id === active.id && row.status === 'resuming');
      if (row) await resume(row, active);
    }
  } catch {
    if (ended || generation !== startedGeneration || active !== startedActive) return;
    rows = []; renderQueue(); $('queue-status').textContent = 'Support service unavailable. The waiting list and shared view are cleared.'; if (active) await stop('failed');
  }
  finally { queueBusy = false; }
}
async function action(target, extra = {}) {
  const record = active, frame = record?.frame;
  if (!record?.connected || record.phase !== 'claimed' || !frame || !target || !frame.targets.some(value => value.id === target.id && value.kind === target.kind)) return;
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
