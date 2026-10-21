import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { SUPPORT_SESSION_CHANGED, type SupportSession } from "./RemoteSupportControl";

export function RemoteSupportPanel({ onApproved }: { onApproved: () => void }) {
  const [invitation, setInvitation] = useState("");
  const [approved, setApproved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("");
  const pending = useRef(false);
  async function start() {
    if (!approved || pending.current || invitation.length > 5000) return;
    pending.current = true; setBusy(true); setStatus("");
    try {
      const session = await invoke<SupportSession>("start_support_session", { invitation: invitation.trim() });
      if (!session || typeof session.sessionId !== "string" || !/^[a-f0-9]{32}$/.test(session.sessionId) || !Number.isSafeInteger(session.expiresAt)
        || session.expiresAt <= Date.now() || session.expiresAt > Date.now() + 30 * 60 * 1000) throw Error();
      setInvitation(""); setApproved(false); window.dispatchEvent(new Event(SUPPORT_SESSION_CHANGED));
      setStatus("Session approved. Connecting in the support window; connection is not verified until it says Connected. Stop support remains available in the main window.");
      onApproved();
    } catch { setStatus("Could not start support. Ask your technician for a fresh invitation, or stop the existing session before trying again. No remote access was verified."); }
    finally { pending.current = false; setBusy(false); }
  }
  return <details className="remote-support" data-support-private="true">
    <summary>Remote Support</summary>
    <div className="remote-support-content">
      <p>Get help inside PST QuickView. No account or separate support app is needed on this Mac.</p>
      <p>Your technician first creates a private invitation on their laptop. Paste it below. A direct encrypted connection is required; there is no data-relay fallback.</p>
      <label>Private session invitation
        <textarea rows={3} value={invitation} maxLength={5000} spellCheck={false} autoComplete="off" onChange={event => setInvitation(event.target.value)} disabled={busy} />
      </label>
      <label className="support-approval"><input type="checkbox" checked={approved} disabled={busy} onChange={event => setApproved(event.target.checked)} />
        I trust this technician and approve sharing visible PST QuickView text and layout, including visible messages, and control of marked navigation/search controls for this session (up to 30 minutes).
      </label>
      <p>No desktop, terminal, arbitrary files, attachments, native dialogs, audio, camera, or microphone access. The app view is a live text/layout mirror, not a pixel-perfect screen recording. Mail images and separate preview windows are not shared. No mailbox data goes through the setup service or is recorded by this app.</p>
      <button type="button" disabled={!approved || !invitation.trim() || busy} onClick={() => void start()}>{busy ? "Starting…" : "Approve and connect"}</button>
      <p className="message-action-status" role="status" aria-live="polite">{status}</p>
    </div>
  </details>;
}
