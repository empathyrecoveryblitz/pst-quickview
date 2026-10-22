// Fixed local categories only. Native/DOM errors can contain private data, so
// never display, log, or transmit their text as a support diagnostic.
export function supportSharingFailureNotice(stage: "capture" | "publish", error: unknown): string {
  const code = stage === "capture" ? "S01: view capture failed"
    : error === "The shared application frame exceeds its permitted limits." ? "S02: frame rejected"
    : error === "This support session is unavailable or has stopped." ? "S03: session ended"
    : "S04: frame publication failed";
  return `Support sharing stopped (${code}). Open Help to start a new approved session.`;
}
