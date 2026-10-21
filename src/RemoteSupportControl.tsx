import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createSupportView } from "./supportView";
export type SupportSession = { sessionId: string; expiresAt: number; operatorLabel: string; updating?: boolean };
export const SUPPORT_SESSION_CHANGED = "pst-support-session-changed";

// Always mounted outside About: closing Help cannot hide Stop.
export function RemoteSupportControl() {
  const [session, setSession] = useState<SupportSession | null>(null);
  const [notice, setNotice] = useState("");
  const [resumePending, setResumePending] = useState(false);
  const resumeStarted = useRef(false);
  const resumeGeneration = useRef(0);
  const resumeSessionId = useRef<string | null>(null);
  const active = useRef<SupportSession | null>(null);
  const view = useRef<ReturnType<typeof createSupportView> | null>(null);
  const stopping = useRef(false);
  const revoked = useRef(new Set<string>());
  useEffect(() => {
    let disposed = false, busy = false, publishing = false, listenerReady = false, healthy = true;
    const clear = () => { active.current = null; view.current?.close(); view.current = null; if (!disposed) setSession(null); };
    const revoke = (id: string | undefined) => { if (id) { revoked.current.add(id); void invoke("stop_support_session", { sessionId: id }).catch(() => {}); } clear(); };
    async function status() {
      if (busy || disposed) return; busy = true;
      try {
        const next = await invoke<SupportSession | null>("support_session_status");
        if (disposed) return;
        if (!healthy) { revoke(next?.sessionId); return; }
        if (!next || typeof next.sessionId !== "string" || !/^[a-f0-9]{32}$/.test(next.sessionId) || !Number.isSafeInteger(next.expiresAt)
          || next.expiresAt <= Date.now() || typeof next.operatorLabel !== "string" || !next.operatorLabel || next.operatorLabel.length > 80
          || revoked.current.has(next.sessionId)) { clear(); return; }
        if (next.sessionId !== active.current?.sessionId || next.updating !== active.current?.updating) {
          view.current?.close(); view.current = next.updating ? null : createSupportView(document);
          active.current = next; setSession(next); setNotice("");
        }
      } catch { revoke(active.current?.sessionId); } finally { busy = false; }
    }
    async function publish() {
      const current = active.current, capture = view.current;
      if (!current || current.updating || !capture || publishing || document.hidden || stopping.current || !listenerReady || !healthy) return; publishing = true;
      try { await invoke("publish_support_frame", { sessionId: current.sessionId, frame: capture.capture() }); }
      catch { revoked.current.add(current.sessionId); clear(); void invoke("stop_support_session", { sessionId: current.sessionId }).catch(() => {}); setNotice("Support sharing stopped. Open Help to start a new approved session."); }
      finally { publishing = false; }
    }
    const statusTimer = window.setInterval(() => void status(), 1000), frameTimer = window.setInterval(() => void publish(), 550);
    const onChange = () => void status(); window.addEventListener(SUPPORT_SESSION_CHANGED, onChange);
    let unlisten: (() => void) | undefined;
    void listen<{ sessionId: string; action: unknown }>("support-action", event => {
      if (!stopping.current && event.payload.sessionId === active.current?.sessionId && Date.now() < active.current.expiresAt) view.current?.apply(event.payload.action);
    }).then(fn => { if (disposed) fn(); else { unlisten = fn; listenerReady = true; } }).catch(() => { healthy = false; revoke(active.current?.sessionId); });
    void status();
    return () => {
      disposed = true; clearInterval(statusTimer); clearInterval(frameTimer); window.removeEventListener(SUPPORT_SESSION_CHANGED, onChange); unlisten?.();
      const id = active.current?.sessionId; clear(); if (id) void invoke("stop_support_session", { sessionId: id }).catch(() => {});
    };
  }, []);
  useEffect(() => {
    let disposed = false;
    // Local one-use Keychain grant only; this lookup performs no update check.
    void invoke<{ available: boolean; sessionId?: string; expiresAt?: number }>("support_resume_status").then(value => {
      if (!disposed && value?.available === true && /^[a-f0-9]{32}$/.test(value.sessionId || "") && Number.isSafeInteger(value.expiresAt) && value.expiresAt! > Date.now()) {
        resumeSessionId.current = value.sessionId!; setResumePending(true);
      }
    }).catch(() => {});
    return () => { disposed = true; };
  }, []);
  useEffect(() => {
    if (!resumePending || resumeStarted.current) return;
    resumeStarted.current = true;
    const generation = resumeGeneration.current;
    // This effect runs only after the visible reconnect banner and Stop exist.
    void invoke<SupportSession>("resume_support_after_update").then(value => {
      if (generation !== resumeGeneration.current) {
        void invoke("stop_support_session", { sessionId: value?.sessionId }).catch(() => {}); return;
      }
      window.dispatchEvent(new Event(SUPPORT_SESSION_CHANGED));
    }).catch(() => { if (generation === resumeGeneration.current) setNotice("Support could not safely resume. Open Help to approve a new request."); })
      .finally(() => { if (generation === resumeGeneration.current) { resumeSessionId.current = null; setResumePending(false); } });
  }, [resumePending]);
  async function stop() {
    if (stopping.current) return; stopping.current = true;
    resumeGeneration.current++; resumeStarted.current = true; setResumePending(false);
    const id = active.current?.sessionId || resumeSessionId.current;
    resumeSessionId.current = null;
    if (id) revoked.current.add(id);
    // Revoke the DOM map synchronously, before waiting for native shutdown.
    view.current?.close(); view.current = null; active.current = null; setSession(null);
    try { if (id) await invoke("stop_support_session", { sessionId: id }); setNotice("Remote support stopped."); }
    catch { setNotice("Sharing and control stopped. Quit PST QuickView to close any remaining connection."); }
    finally { stopping.current = false; }
  }
  return <>
    {session || resumePending ? <section className="support-live-banner" data-support-private="true" aria-label="Active remote support">
      <span>{resumePending ? <strong>Reconnecting to the same approved helper after your update.</strong> : session?.updating ? <strong>Updating. Sharing and control are paused; Stop cancels resumption.</strong> : <><strong>Support approved for {session?.operatorLabel}.</strong> Waiting or connected: visible app content may be shared with this helper. Only marked app controls are available; no desktop or file access.</>}</span>
      <button type="button" onClick={() => void stop()}>Stop support</button>
    </section> : null}
    {notice ? <p className="support-stop-notice" data-support-private="true" role="status">{notice}</p> : null}
  </>;
}
