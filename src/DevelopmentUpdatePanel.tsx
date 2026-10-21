import { useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
type VerifiedUpdate = { available: boolean; pendingId?: string; version?: string };

// Owner-only UI: none of these controls can be targeted by Remote Support.
export function DevelopmentUpdatePanel() {
  const [update, setUpdate] = useState<VerifiedUpdate | null>(null);
  const [busy, setBusy] = useState(false);
  const [resume, setResume] = useState(false);
  const [canResume, setCanResume] = useState(false);
  const [status, setStatus] = useState("");
  const pending = useRef(false);
  async function check() {
    if (pending.current) return;
    pending.current = true; setBusy(true); setUpdate(null);
    setStatus("Checking and verifying the development update. No installation has been approved.");
    try {
      const next = await invoke<VerifiedUpdate>("check_verified_update");
      if (next?.available === false) { setStatus("This development build is up to date."); return; }
      if (next?.available !== true || typeof next.pendingId !== "string" || !/^[a-f0-9]{32}$/.test(next.pendingId)
        || typeof next.version !== "string" || next.version.length > 128 || !/^[0-9A-Za-z.+-]+$/.test(next.version)) throw Error();
      const session = await invoke<{ sessionId: string; expiresAt: number; updating?: boolean } | null>("support_session_status").catch(() => null);
      const allowed = !!session && /^[a-f0-9]{32}$/.test(session.sessionId) && Number.isSafeInteger(session.expiresAt) && session.expiresAt > Date.now() && session.updating !== true;
      setCanResume(allowed); setResume(allowed); setUpdate(next);
      setStatus("A signed development update is verified and ready. Nothing will be installed until you approve it on this Mac.");
    } catch { setStatus("No verified update is available. The installed app has not been changed."); }
    finally { pending.current = false; setBusy(false); }
  }
  async function install() {
    if (pending.current || !update?.available || !update.pendingId) return;
    pending.current = true; setBusy(true);
    setStatus("Confirm the update on this Mac. The helper cannot approve this confirmation for you.");
    try {
      await invoke("approve_verified_update", { pendingId: update.pendingId, resume });
      setStatus("The approved update is restarting PST QuickView. Support can resume only with the same helper, within the original session limit.");
    } catch {
      setUpdate(null);
      setStatus("The update did not complete or its approval was cancelled. Automatic support resumption is closed. Check the app before starting a new support request.");
    } finally { pending.current = false; setBusy(false); }
  }
  return <details className="remote-support" data-support-private="true">
    <summary>Development updates</summary>
    <div className="remote-support-content">
      <p>Install a verified correction without downloading a replacement app by hand. Checks happen only when you choose them.</p>
      <button type="button" disabled={busy} onClick={() => void check()}>{busy ? "Working…" : "Check for a development update"}</button>
      {update?.available ? <>
        <p>Verified development version: <strong>{update.version}</strong>.</p>
        <label className="support-approval"><input type="checkbox" checked={resume} disabled={busy || !canResume} onChange={event => setResume(event.target.checked)} />
          Resume the currently approved, connected support session after the update (same helper only, within 90 seconds and the original session expiry).
        </label>
        <p>A restart is required. Stop cancels support resumption. macOS may ask for local permission; no protection is bypassed.</p>
        <button type="button" disabled={busy} onClick={() => void install()}>{resume ? "Update and resume support" : "Update and restart without support"}</button>
      </> : null}
      <p className="message-action-status" role="status" aria-live="polite">{status}</p>
    </div>
  </details>;
}
