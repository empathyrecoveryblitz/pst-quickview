function normalizedSender(sender: string): string {
  return sender.trim().replace(/^"|"$/g, "");
}

export function senderDisplayName(sender: string): string {
  const normalized = normalizedSender(sender);
  if (!normalized) return "(No sender)";

  const mailbox = normalized.match(/^\s*"?([^"<>]+?)"?\s*<[^<>]+>\s*$/);
  return mailbox?.[1]?.trim() || normalized;
}

export function conversationParticipantSummary(
  participants: string[],
  latestSender: string,
): string {
  const source = participants.length ? participants : latestSender ? [latestSender] : [];
  const seen = new Set<string>();
  const names = source
    .map(senderDisplayName)
    .filter((name) => {
      const key = name.toLocaleLowerCase();
      if (seen.has(key)) return false;
      seen.add(key);
      return true;
    });

  if (!names.length) return "(No sender)";
  if (names.length <= 2) return names.join(", ");
  return `${names.slice(0, 2).join(", ")} +${names.length - 2}`;
}
