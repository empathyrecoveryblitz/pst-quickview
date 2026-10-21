import { validateConfig } from './protocol.js';
import { createSupportPeer } from './peer.js';

const $ = id => document.getElementById(id);
const canvas = $('view');
const context = canvas.getContext('2d');
let config, peer, frame, connected = false, ending = false, expiryTimer, frameTimer, lastWheel = 0;

function status(text, state = '') {
  $('status').textContent = text;
  $('status').className = state;
}
function clearView() {
  clearTimeout(frameTimer);
  frame = undefined;
  context.clearRect(0, 0, canvas.width, canvas.height);
  $('target').replaceChildren();
  $('input').value = '';
  $('controls').hidden = true;
  controls();
}
async function stopService(keepalive = false) {
  if (!config) return;
  try {
    await fetch(config.origin + '/api/stop', {
      method: 'POST', headers: { Authorization: 'Bearer ' + config.token, 'Content-Type': 'application/json' },
      body: '{}', credentials: 'omit', redirect: 'error', cache: 'no-store', keepalive,
      signal: AbortSignal.timeout(2000),
    });
  } catch { /* Closing the launcher also closes the bounded support service. */ }
}
function close(reason = 'stopped') {
  if (ending) return;
  ending = true;
  connected = false;
  clearTimeout(expiryTimer);
  clearView();
  $('approve').disabled = true;
  $('consent').disabled = true;
  $('copy').disabled = true;
  $('invitation').value = '';
  $('invitation-panel').hidden = true;
  peer?.close();
  if (reason === 'expired') status('This support invitation expired. Access is closed.');
  else if (reason === 'failed') status('The direct support connection could not be completed. Access is closed.', 'failure');
  else status('Support stopped. Access is closed.');
  void stopService();
}
function selected() { return frame?.targets.find(target => target.id === $('target').value); }
function controls() {
  const target = selected();
  $('target').disabled = !connected || !frame?.targets.length;
  $('activate').disabled = !connected || target?.kind !== 'click';
  $('send-input').disabled = !connected || target?.kind !== 'input';
  $('input').disabled = !connected || target?.kind !== 'input';
  $('scroll-up').disabled = !connected || target?.kind !== 'scroll';
  $('scroll-down').disabled = !connected || target?.kind !== 'scroll';
}
function selectTarget(id) {
  const target = frame?.targets.find(value => value.id === id);
  if (!target) return;
  $('target').value = id;
  $('input').value = target.kind === 'input' ? target.value || '' : '';
  controls();
}
function draw(next) {
  if (!connected || ending) return;
  const priorSelection = $('target').value;
  frame = next;
  canvas.width = frame.width;
  canvas.height = frame.height;
  context.fillStyle = '#fff';
  context.fillRect(0, 0, frame.width, frame.height);
  for (const item of frame.items) {
    context.save();
    context.beginPath(); context.rect(item.x, item.y, item.w, item.h); context.clip();
    if (item.kind === 'box') {
      context.fillStyle = item.fill;
      context.fillRect(item.x, item.y, item.w, item.h);
    } else {
      context.fillStyle = item.color;
      context.font = `${item.weight} ${item.size}px system-ui,sans-serif`;
      context.textBaseline = 'top';
      const lines = item.text.split('\n');
      for (let index = 0; index < lines.length; index++) context.fillText(lines[index], item.x, item.y + index * item.size * 1.25);
    }
    context.restore();
  }
  const options = frame.targets.map((target, index) => {
    const option = document.createElement('option');
    option.value = target.id;
    option.textContent = `${target.label || 'Shared control ' + (index + 1)} (${target.kind})`;
    return option;
  });
  $('target').replaceChildren(...options);
  if (frame.targets.some(target => target.id === priorSelection)) $('target').value = priorSelection;
  else if (frame.targets.length) selectTarget(frame.targets[0].id);
  $('controls').hidden = frame.targets.length === 0;
  controls();
  status(frame.truncated ? 'Connected. This view has reached its display limit; some app content is omitted.' : 'Connected. The approved PST QuickView window is shared.', 'connected');
  clearTimeout(frameTimer);
  const renderedId = frame.id;
  frameTimer = setTimeout(() => {
    if (!ending && connected && frame?.id === renderedId) {
      clearView();
      status('Connected. Waiting for a fresh app view; the previous view and controls have been cleared.');
    }
  }, 3000);
}
function hit(event, kind) {
  if (!frame || !connected || ending) return;
  const bounds = canvas.getBoundingClientRect();
  const scale = Math.min(bounds.width / frame.width, bounds.height / frame.height);
  const x = (event.clientX - bounds.left) / scale, y = (event.clientY - bounds.top) / scale;
  return [...frame.targets].reverse().find(target => (!kind || target.kind === kind)
    && x >= target.x && y >= target.y && x < target.x + target.w && y < target.y + target.h);
}
async function action(target, extra = {}) {
  if (!connected || ending || !frame || !target || !frame.targets.some(value => value.id === target.id && value.kind === target.kind)) return;
  try { await peer.sendAction({ frame: frame.id, target: target.id, kind: target.kind, ...extra }); }
  catch { close('failed'); }
}
canvas.addEventListener('click', event => {
  const target = hit(event);
  if (!target) return;
  selectTarget(target.id);
  if (target.kind === 'click') void action(target);
  else if (target.kind === 'input') $('input').focus();
});
canvas.addEventListener('wheel', event => {
  const target = hit(event, 'scroll');
  if (!target) return;
  event.preventDefault();
  const now = performance.now();
  if (now - lastWheel < 150) return;
  lastWheel = now;
  const multiplier = event.deltaMode === 1 ? 20 : event.deltaMode === 2 ? 500 : 1;
  const deltaY = Math.max(-700, Math.min(700, Math.round(event.deltaY * multiplier)));
  if (deltaY) void action(target, { deltaY });
}, { passive: false });
$('target').addEventListener('change', () => selectTarget($('target').value));
$('activate').onclick = () => { void action(selected()); };
$('send-input').onclick = () => { const target = selected(); if (target?.kind === 'input') void action(target, { value: $('input').value }); };
$('scroll-up').onclick = () => { const target = selected(); if (target?.kind === 'scroll') void action(target, { deltaY: -500 }); };
$('scroll-down').onclick = () => { const target = selected(); if (target?.kind === 'scroll') void action(target, { deltaY: 500 }); };
$('stop').onclick = () => close();
$('copy').onclick = async () => {
  if (ending) return;
  try { await navigator.clipboard.writeText($('invitation').value); }
  catch { $('invitation').focus(); $('invitation').select(); }
};
$('approve').onclick = async () => {
  if (ending || !$('consent').checked) { status('Check the approval box when you are ready.'); return; }
  $('approve').disabled = true;
  $('consent').disabled = true;
  try { await peer.approve(); } catch { close('failed'); }
};
addEventListener('pagehide', () => {
  if (ending) return;
  ending = true; connected = false; clearTimeout(expiryTimer); clearView(); peer?.close(); void stopService(true);
});

try {
  if (location.hash.length < 2 || location.hash.length > 12000) throw new Error('invitation');
  const encoded = location.hash.slice(1);
  const raw = JSON.parse(atob(encoded.replaceAll('-', '+').replaceAll('_', '/')));
  history.replaceState(null, '', location.pathname);
  config = validateConfig(raw);
  if (config.role !== 'controller' || config.origin !== location.origin || location.pathname !== '/') throw new Error('invitation');
  if (typeof raw.invite !== 'string' || raw.invite.length > 6000) throw new Error('invitation');
  const invitation = new URL(raw.invite);
  if (invitation.pathname !== '/join' || invitation.search || invitation.username || invitation.password || !invitation.hash) throw new Error('invitation');
  const guest = validateConfig(JSON.parse(atob(invitation.hash.slice(1).replaceAll('-', '+').replaceAll('_', '/'))));
  if (guest.role !== 'guest' || guest.origin !== invitation.origin || guest.sid !== config.sid || guest.key !== config.key
    || guest.expires !== config.expires || guest.expectedExit !== config.expectedExit || guest.token === config.token) throw new Error('invitation');
  $('invitation').value = raw.invite;
  $('invitation-panel').hidden = false;
  $('expiry').textContent = 'Session ends by ' + new Date(config.expires).toLocaleTimeString() + '.';
  peer = await createSupportPeer({
    config, onFrame: draw, onAction: () => close('failed'),
    onState(state) {
      if (ending) return;
      if (state === 'connected') { connected = true; controls(); status('Connected. Waiting for the shared app window…', 'connected'); }
      else if (state.startsWith('failed:')) { status('The direct support connection could not be established. Access is closed.', 'failure'); close('failed'); }
      else if (state === 'negotiating') status('Both sides approved. Connecting directly…');
      else if (state === 'approved' || state === 'waiting') status('Approved. Waiting for the other computer…');
    },
    onClose: reason => close(reason),
  });
  if (ending) peer.close();
  else {
    $('approve').disabled = false;
    status('Ready. Share the invitation and start support when both sides are available.');
    expiryTimer = setTimeout(() => close('expired'), Math.max(1, config.expires - Date.now()));
  }
} catch {
  close('invalid');
  status('This invitation is missing, expired, or unsupported. Start a fresh support session.', 'failure');
}
