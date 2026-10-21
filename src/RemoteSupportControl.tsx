import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { createSupportView } from "./supportView";
export type SupportSession = { sessionId: string; expiresAt: number };
export const SUPPORT_SESSION_CHANGED = "pst-support-session-changed";

// Always mounted outside About: closing Help cannot hide Stop.
export function RemoteSupportControl() {
  const [session, setSession] = useState<SupportSession | null>(null);
  const [notice, setNotice] = useState("");
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
        if (!next || next.expiresAt <= Date.now() || revoked.current.has(next.sessionId)) { clear(); return; }
        if (next.sessionId !== active.current?.sessionId) { view.current?.close(); view.current = createSupportView(document); active.current = next; setSession(next); setNotice(""); }
      } catch { revoke(active.current?.sessionId); } finally { busy = false; }
    }
    async function publish() {
      const current = active.current, capture = view.current;
      if (!current || !capture || publishing || document.hidden || stopping.current || !listenerReady || !healthy) return; publishing = true;
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
  async function stop() {
    if (stopping.current) return; stopping.current = true;
    const id = active.current?.sessionId;
    if (id) revoked.current.add(id);
    // Revoke the DOM map synchronously, before waiting for native shutdown.
    view.current?.close(); view.current = null; active.current = null; setSession(null);
    try { if (id) await invoke("stop_support_session", { sessionId: id }); setNotice("Remote support stopped."); }
    catch { setNotice("Sharing and control stopped. Quit PST QuickView to close any remaining connection."); }
    finally { stopping.current = false; }
  }
  return <>
    {session ? <section className="support-live-banner" data-support-private="true" aria-label="Active remote support">
      <span><strong>Remote support approved.</strong> Visible app content may be shared. Only marked app controls are available; no desktop or file access.</span>
      <button type="button" onClick={() => void stop()}>Stop support</button>
    </section> : null}
    {notice ? <p className="support-stop-notice" data-support-private="true" role="status">{notice}</p> : null}
  </>;
}
