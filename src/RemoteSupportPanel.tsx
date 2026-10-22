import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { SUPPORT_SESSION_CHANGED, type SupportSession } from "./RemoteSupportControl";
type SupportService = { configured: boolean; operatorLabel: string };

export function RemoteSupportPanel({ onApproved }: { onApproved: () => void }) {
  const [service, setService] = useState<SupportService>({ configured: false, operatorLabel: "" });
  const [approved, setApproved] = useState(false);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("");
  const pending = useRef(false);
  useEffect(() => {
    let disposed = false;
    // This is local compiled configuration only. It performs no network request.
    void invoke<SupportService>("support_service_status").then(value => {
      if (disposed) return;
      if (value?.configured === true && typeof value.operatorLabel === "string" && value.operatorLabel.trim() === value.operatorLabel
        && value.operatorLabel.length > 0 && value.operatorLabel.length <= 80 && !/[\u0000-\u001f\u007f]/.test(value.operatorLabel)) {
        setService(value);
      }
    }).catch(() => { if (!disposed) setStatus("Remote Support is unavailable in this build."); });
    return () => { disposed = true; };
  }, []);
  async function start() {
    if (!approved || pending.current || !service.configured) return;
    pending.current = true; setBusy(true); setStatus("");
    let created: SupportSession | undefined;
    try {
      const session = await invoke<SupportSession>("request_support_session");
      created = session;
      if (!session || typeof session.sessionId !== "string" || !/^[a-f0-9]{32}$/.test(session.sessionId) || !Number.isSafeInteger(session.expiresAt)
        || session.expiresAt <= Date.now() || session.expiresAt > Date.now() + 30 * 60 * 1000
        || session.operatorLabel !== service.operatorLabel) throw Error();
      setApproved(false); window.dispatchEvent(new Event(SUPPORT_SESSION_CHANGED));
      setStatus("Support requested. Waiting for your approved helper; connection is not verified until it says Connected. Stop support remains available in the main window.");
      onApproved();
    } catch {
      if (typeof created?.sessionId === "string" && /^[a-f0-9]{32}$/.test(created.sessionId)) {
        void invoke("stop_support_session", { sessionId: created.sessionId }).catch(() => {});
      }
      setStatus("Could not request support. Stop any existing session before trying again. No remote access was verified.");
    }
    finally { pending.current = false; setBusy(false); }
  }
  return <details className="remote-support" data-support-private="true">
    <summary>Remote Support</summary>
    <div className="remote-support-content">
      <p>Get help inside PST QuickView. No account or separate support app is needed on this Mac.</p>
      <p>{service.configured ? <>Your approved helper is <strong>{service.operatorLabel}</strong>. Request support here; your helper can accept it from their dashboard.</> : "Remote Support is not configured in this application build. No request can be sent."}</p>
      <p>A direct encrypted connection is required; there is no data-relay fallback. Cloudflare handles the temporary request and encrypted connection setup, without receiving app content.</p>
      <label className="support-approval"><input type="checkbox" checked={approved} disabled={busy || !service.configured} onChange={event => setApproved(event.target.checked)} />
        I trust {service.configured ? service.operatorLabel : "the configured helper"} and approve sharing visible PST QuickView text and layout, including visible messages, and control of marked navigation/search controls and opening or cancelling recovery dialogs for this session (up to 30 minutes). Only I can confirm a repair or deletion on this Mac. I understand my helper may use an AI assistant to inspect the shared view.
      </label>
      <p>No desktop, terminal, arbitrary files, attachments, native dialogs, audio, camera, or microphone access. The app view is a live text/layout mirror, not a pixel-perfect screen recording. Mail images and separate preview windows are not shared. No mailbox data goes through the setup service or is recorded by this app.</p>
      <p>Only share mail you are authorized to disclose. If your helper uses an AI assistant, displayed content may be processed by that assistant; keeping mail out of GitHub does not prevent that processing.</p>
      <button type="button" disabled={!approved || !service.configured || busy} onClick={() => void start()}>{busy ? "Requesting…" : "Approve and request support"}</button>
      <p className="message-action-status" role="status" aria-live="polite">{status}</p>
    </div>
  </details>;
}
