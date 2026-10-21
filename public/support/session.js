import { createSupportPeer } from './peer.js';

const $ = id => document.getElementById(id);
let sessionId, peer, connected = false, ending = false, pollTimer, expiryTimer, lastFrame;
const invoke = (command, args) => {
  if (typeof window.__TAURI_INTERNALS__?.invoke !== 'function') return Promise.reject(new Error('native'));
  return window.__TAURI_INTERNALS__.invoke(command, args);
};
async function stop(reason = 'stopped') {
  if (ending) return;
  ending = true; connected = false; lastFrame = undefined;
  clearTimeout(pollTimer); clearTimeout(expiryTimer);
  peer?.close();
  $('stop').disabled = true;
  $('status').textContent = reason === 'expired' ? 'The invitation expired. Support is closed.' : 'Support stopped. Access is closed.';
  if (sessionId) try { await invoke('stop_support_session', { sessionId }); } catch { /* Native broker enforces expiry independently. */ }
}
async function poll() {
  if (ending) return;
  try {
    if (connected) {
      const frame = await invoke('poll_support_frame', { sessionId });
      if (ending) return;
      if (frame && frame.id !== lastFrame) { await peer.sendFrame(frame); lastFrame = frame.id; }
    }
  } catch { await stop('failed'); return; }
  pollTimer = setTimeout(() => { void poll(); }, 500);
}
$('stop').onclick = () => { void stop(); };
addEventListener('pagehide', () => { void stop(); });
try {
  const query = new URLSearchParams(location.search);
  sessionId = query.get('session');
  if ([...query.keys()].length !== 1 || !/^[a-f0-9]{32}$/.test(sessionId || '')) throw new Error('session');
  const config = await invoke('bootstrap_support_session', { sessionId });
  if (ending || config.role !== 'guest') throw new Error('session');
  $('expiry').textContent = 'Session ends by ' + new Date(config.expires).toLocaleTimeString() + '.';
  peer = await createSupportPeer({
    config, onFrame: () => { void stop('failed'); },
    onAction: action => { if (!ending && connected) void invoke('support_submit_action', { sessionId, action }).catch(() => stop('failed')); },
    onState(state) {
      if (ending) return;
      if (state === 'connected') { connected = true; $('status').textContent = 'Connected. The helper can view and use the shared PST QuickView controls.'; }
      else if (state.startsWith('failed:')) void stop('failed');
      else if (state === 'negotiating') $('status').textContent = 'Both sides approved. Establishing the direct encrypted connection…';
      else if (state === 'approved' || state === 'waiting') $('status').textContent = 'Approved. Waiting for the helper to start support…';
    },
    onClose: reason => { void stop(reason); },
  });
  if (ending) peer.close();
  else {
    // The main application records local consent before creating this isolated window.
    await peer.approve();
    if (!ending) { expiryTimer = setTimeout(() => { void stop('expired'); }, Math.max(1, config.expires - Date.now())); void poll(); }
  }
} catch {
  await stop('failed');
  $('status').textContent = 'This support session could not start. Return to PST QuickView and request a fresh invitation.';
}
