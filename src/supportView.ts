// The peer receives inert drawing instructions, never HTML, script, selectors,
// native input events, file paths, or application command names.
export type SupportAction = { frame: string; target: string; kind: "click" | "input" | "scroll"; value?: string; deltaY?: number };
type Box = { x: number; y: number; w: number; h: number };
type DrawItem = (Box & { kind: "box"; fill: string }) | (Box & { kind: "text"; text: string; color: string; size: number; weight: number });
type Target = Box & { id: string; kind: SupportAction["kind"]; label: string; value?: string };
export type SupportFrame = { v: 1; id: string; width: number; height: number; items: DrawItem[]; targets: Target[]; truncated: boolean };
const ID = /^[a-f0-9]{32}$/;
const FORBIDDEN = "[data-support-private],[data-support-untrusted],script,style,svg,img,canvas,iframe,video,audio";

export function parseSupportAction(value: unknown): SupportAction | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null;
  const a = value as Record<string, unknown>;
  if (typeof a.frame !== "string" || !ID.test(a.frame) || typeof a.target !== "string" || !ID.test(a.target)) return null;
  let keys = ["frame", "target", "kind"];
  if (a.kind === "input") {
    if (typeof a.value !== "string" || a.value.length > 2000 || /[\u0000-\u0008\u000b\u000c\u000e-\u001f]/.test(a.value)) return null;
    keys = [...keys, "value"];
  } else if (a.kind === "scroll") {
    if (typeof a.deltaY !== "number" || !Number.isFinite(a.deltaY) || Math.abs(a.deltaY) > 700) return null;
    keys = [...keys, "deltaY"];
  } else if (a.kind !== "click") return null;
  if (Object.keys(a).length !== keys.length || keys.some(key => !Object.prototype.hasOwnProperty.call(a, key))) return null;
  return a as SupportAction;
}
function identifier(): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(16)), b => b.toString(16).padStart(2, "0")).join("");
}
function intersect(a: Box, b: Box): Box | null {
  const x = Math.max(a.x, b.x), y = Math.max(a.y, b.y);
  const w = Math.min(a.x + a.w, b.x + b.w) - x, h = Math.min(a.y + a.h, b.y + b.h) - y;
  return w > 0 && h > 0 ? { x, y, w, h } : null;
}
function bounds(rect: DOMRect): Box { return { x: rect.x, y: rect.y, w: rect.width, h: rect.height }; }
function color(value: string): string | null {
  if (value.length > 64) return null;
  if (/^#(?:[a-f\d]{3}|[a-f\d]{4}|[a-f\d]{6}|[a-f\d]{8})$/i.test(value)) return value;
  const match = /^(rgb|rgba)\(\s*(\d{1,3})\s*,\s*(\d{1,3})\s*,\s*(\d{1,3})(?:\s*,\s*(0(?:\.\d{1,4})?|1(?:\.0{1,4})?|\.\d{1,4}))?\s*\)$/.exec(value);
  return match && Number(match[2]) <= 255 && Number(match[3]) <= 255 && Number(match[4]) <= 255 &&
    ((match[1] === "rgb" && match[5] === undefined) || (match[1] === "rgba" && match[5] !== undefined)) ? value : null;
}
function invisibleColor(value: string): boolean {
  return value === "transparent" || /rgba\([^)]*,\s*(?:0(?:\.0+)?|\.0+)\s*\)$/.test(value) || /^#(?:[a-f\d]{3}0|[a-f\d]{6}00)$/i.test(value);
}
function viewportDimension(value: number | undefined, maximum: number, fallback: number): number {
  return value !== undefined && Number.isFinite(value) && value > 0 ? Math.min(maximum, Math.max(1, value)) : fallback;
}
// Repair only the outgoing mirror copy, never the source DOM. Limits remain
// UTF-16 units, but neither truncation nor replacement emits a lone surrogate.
function safeText(value: string, maximum: number): string {
  let copy = "";
  for (const point of value) {
    const code = point.codePointAt(0)!;
    const character = (code >= 0xd800 && code <= 0xdfff) || code <= 8 || code === 11 || code === 12 || (code >= 14 && code <= 31) || code === 127 ? "\ufffd" : point;
    if (copy.length + character.length > maximum) break;
    copy += character;
  }
  return copy;
}
function allowedKind(element: Element): SupportAction["kind"] | null {
  if (element.closest(FORBIDDEN) || element.matches(":disabled,[aria-disabled=true]")) return null;
  for (let parent: Element | null = element; parent; parent = parent.parentElement) {
    const style = getComputedStyle(parent);
    if (style.visibility !== "visible" || style.display === "none" || Number(style.opacity) === 0 || style.pointerEvents === "none") return null;
  }
  if (element instanceof HTMLButtonElement && element.dataset.supportControl === "click" && !element.disabled) return "click";
  if (element instanceof HTMLInputElement && element.dataset.supportControl === "input" && ["search", "text"].includes(element.type) && !element.disabled && !element.readOnly) return "input";
  if (element instanceof HTMLSelectElement && element.dataset.supportControl === "input" && !element.disabled) return "input";
  if (element instanceof HTMLElement && element.dataset.supportControl === "scroll" && element.scrollHeight > element.clientHeight) return "scroll";
  return null;
}
/** One instance per locally approved session. No recording or persistent cache. */
export function createSupportView(document: Document, now = () => performance.now()) {
  let active = true;
  const tokens = new WeakMap<Element, string>();
  const frames = new Map<string, { at: number; targets: Map<string, { element: Element; rect: Box; kind: SupportAction["kind"] }> }>();
  function capture(): SupportFrame {
    if (!active) throw Error("Support stopped.");
    const viewportWidth = document.defaultView?.innerWidth, viewportHeight = document.defaultView?.innerHeight;
    const width = viewportDimension(viewportWidth, 4096, 1280), height = viewportDimension(viewportHeight, 2160, 820);
    const viewport: Box = {
      x: 0, y: 0,
      w: viewportWidth !== undefined && Number.isFinite(viewportWidth) && viewportWidth > 0 ? Math.min(width, viewportWidth) : 0,
      h: viewportHeight !== undefined && Number.isFinite(viewportHeight) && viewportHeight > 0 ? Math.min(height, viewportHeight) : 0,
    };
    const frame: SupportFrame = { v: 1, id: identifier(), width, height, items: [], targets: [], truncated: false };
    const controls = new Map<string, { element: Element; rect: Box; kind: SupportAction["kind"] }>();
    let glyphs = 0, visited = 0, textUnits = 0;
    const append = (item: DrawItem) => {
      if (frame.items.length >= 590) { frame.truncated = true; return; }
      if (item.kind === "text") {
        const text = safeText(item.text, Math.min(1000, 12000 - textUnits));
        if (text.length < item.text.length) frame.truncated = true;
        if (!text) return;
        textUnits += text.length;
        item = { ...item, text };
      }
      frame.items.push(item);
    };
    const walk = (element: Element, clip: Box, depth: number) => {
      if (++visited > 2400 || depth > 45 || frame.items.length >= 590) { frame.truncated = true; return; }
      if (element.matches("[data-support-private],script,style,svg,canvas,iframe,video,audio,input[type=password],input[type=file],[hidden]")) return;
      const style = getComputedStyle(element);
      if (style.display === "none" || style.visibility !== "visible" || Number(style.opacity) === 0) return;
      const rect = bounds(element.getBoundingClientRect()), visible = intersect(rect, clip);
      if (!visible && style.overflow !== "visible") return;
      const background = color(style.backgroundColor), foreground = color(style.color) || "";
      const textHidden = !foreground || invisibleColor(foreground) || invisibleColor(style.getPropertyValue("-webkit-text-fill-color")) || (style.clipPath && style.clipPath !== "none");
      if (visible && background && !invisibleColor(background)) append({ kind: "box", ...visible, fill: background });
      let nextClip = /hidden|auto|scroll|clip/.test(`${style.overflowX} ${style.overflowY}`) ? visible : clip;
      if (nextClip && style.textOverflow === "ellipsis" && element instanceof HTMLElement && element.scrollWidth > element.clientWidth) {
        nextClip = { ...nextClip, w: Math.max(0, nextClip.w - (parseFloat(style.fontSize) || 14) * 2) };
      }
      if (!nextClip) return;
      if (visible && !element.closest("[data-support-untrusted]")) {
        const kind = allowedKind(element);
        if (kind && frame.targets.length < 80) {
          let id = tokens.get(element); if (!id) { id = identifier(); tokens.set(element, id); }
          const label = `${kind} control`;
          const target: Target = { ...visible, id, kind, label };
          if (element instanceof HTMLSelectElement) target.value = safeText(element.value, 2000);
          frame.targets.push(target); controls.set(id, { element, rect: visible, kind });
        }
      }
      if (visible && (element instanceof HTMLInputElement || element instanceof HTMLSelectElement)) {
        const top = document.elementFromPoint(visible.x + visible.w / 2, visible.y + visible.h / 2);
        if (top !== element || textHidden) return;
        // Native inputs cannot provide clipped text ranges. Never export their
        // values/prefixes: horizontal scrolling could make those bytes hidden.
        const text = element instanceof HTMLSelectElement ? element.selectedOptions[0]?.textContent ?? "" : "Search input (technician can enter a query)";
        const size = Math.min(40, Math.max(8, parseFloat(style.fontSize) || 13));
        append({ kind: "text", ...visible, text: safeText(text, Math.min(1000, Math.floor(visible.w / size))), color: foreground, size, weight: 400 });
        return;
      }
      for (const node of element.childNodes) {
        if (node.nodeType === Node.ELEMENT_NODE) { walk(node as Element, nextClip, depth + 1); continue; }
        if (node.nodeType !== Node.TEXT_NODE || !node.textContent?.trim()) continue;
        // An unknown computed color may contain an alpha channel (Color 4,
        // for example). Do not turn potentially invisible text opaque merely
        // because the bounded drawing protocol cannot represent that color.
        if (textHidden) continue;
        // Character ranges omit off-screen lines: a visible paragraph must not
        // accidentally disclose the remainder of a long message or raw source.
        const range = document.createRange(); let line: (Box & { text: string }) | null = null;
        const flush = () => {
          if (line?.text.trim()) append({ kind: "text", ...line, color: foreground, size: Math.min(40, Math.max(8, parseFloat(style.fontSize) || 13)), weight: Math.min(900, Math.max(100, parseInt(style.fontWeight) || 400)) });
          line = null;
        };
        const text = node.textContent;
        for (let offset = 0; offset < text.length;) {
          const point = String.fromCodePoint(text.codePointAt(offset)!);
          if (glyphs + point.length > 12000 || frame.items.length >= 590) { frame.truncated = true; break; }
          glyphs += point.length;
          range.setStart(node, offset); range.setEnd(node, offset + point.length);
          offset += point.length;
          const character = safeText(point, point.length);
          const charRect = bounds(range.getBoundingClientRect()), shown = intersect(charRect, nextClip);
          if (!shown || shown.h < charRect.h * 0.8 || shown.w < charRect.w * 0.8) { flush(); continue; }
          const top = document.elementFromPoint(shown.x + shown.w / 2, shown.y + shown.h / 2);
          if (top !== element) { flush(); continue; }
          if (line && Math.abs(line.y - shown.y) < 2 && Math.abs(line.x + line.w - shown.x) < 3 && line.text.length + character.length <= 1000) {
            const x = Math.min(line.x, shown.x), y = Math.min(line.y, shown.y);
            line = { x, y, w: Math.max(line.x + line.w, shown.x + shown.w) - x, h: Math.max(line.y + line.h, shown.y + shown.h) - y, text: line.text + character };
          }
          else { flush(); line = { ...shown, text: character }; }
        }
        flush();
      }
    };
    walk(document.body, viewport, 0);
    for (const target of frame.targets) {
      const visibleLabels = frame.items.filter((item): item is Extract<DrawItem, { kind: "text" }> => item.kind === "text" && item.x >= target.x && item.y >= target.y && item.x < target.x + target.w && item.y < target.y + target.h);
      target.label = safeText(visibleLabels.map(item => item.text).join(" "), 100) || target.label;
    }
    while (new TextEncoder().encode(JSON.stringify(frame)).byteLength > 48 * 1024) { frame.truncated = true; if (frame.items.length) frame.items.pop(); else if (frame.targets.length) frame.targets.pop(); else break; }
    const retained = new Set(frame.targets.map(t => t.id));
    for (const id of controls.keys()) if (!retained.has(id)) controls.delete(id);
    for (const [id, old] of frames) if (now() - old.at > 2000) frames.delete(id);
    while (frames.size >= 3) frames.delete(frames.keys().next().value!);
    frames.set(frame.id, { at: now(), targets: controls }); return frame;
  }
  function apply(value: unknown): boolean {
    const action = parseSupportAction(value);
    if (!active || !action || document.hidden) return false;
    const frame = frames.get(action.frame), target = frame?.targets.get(action.target);
    if (!frame || now() - frame.at > 2000 || !target || target.kind !== action.kind || !target.element.isConnected || allowedKind(target.element) !== action.kind) return false;
    const rect = bounds(target.element.getBoundingClientRect());
    if (Math.abs(rect.x - target.rect.x) > 3 || Math.abs(rect.y - target.rect.y) > 3) return false;
    const hit = document.elementFromPoint(target.rect.x + target.rect.w / 2, target.rect.y + target.rect.h / 2);
    if (!hit || (hit !== target.element && !target.element.contains(hit))) return false;
    if (action.kind === "click") (target.element as HTMLButtonElement).click();
    else if (action.kind === "scroll") (target.element as HTMLElement).scrollTop += action.deltaY!;
    else {
      const element = target.element as HTMLInputElement | HTMLSelectElement;
      if (element instanceof HTMLSelectElement && !Array.from(element.options).some(o => !o.disabled && o.value === action.value)) return false;
      const prototype = element instanceof HTMLSelectElement ? HTMLSelectElement.prototype : HTMLInputElement.prototype;
      Object.getOwnPropertyDescriptor(prototype, "value")!.set!.call(element, action.value);
      element.dispatchEvent(new Event("input", { bubbles: true })); element.dispatchEvent(new Event("change", { bubbles: true }));
    }
    return true;
  }
  return { capture, apply, close() { active = false; frames.clear(); } };
}
