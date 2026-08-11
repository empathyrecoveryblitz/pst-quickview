import type { SearchHighlightRange, SearchMatchedField } from "./types";

export type SearchHighlightSegment = {
  text: string;
  highlighted: boolean;
};

const maximumHighlightRanges = 8;
const maximumInputRanges = 64;

export function normalizeHighlightRanges(
  text: string,
  ranges: readonly SearchHighlightRange[] | null | undefined,
): SearchHighlightRange[] {
  if (!text || !ranges?.length) return [];

  const valid = ranges
    .slice(0, maximumInputRanges)
    .filter(
      (range) =>
        Number.isInteger(range.start) &&
        Number.isInteger(range.end) &&
        range.start >= 0 &&
        range.end > range.start &&
        range.end <= text.length,
    )
    .map((range) => ({ start: range.start, end: range.end }))
    .sort((left, right) => left.start - right.start || left.end - right.end);

  const merged: SearchHighlightRange[] = [];
  for (const range of valid) {
    const previous = merged[merged.length - 1];
    if (previous && range.start <= previous.end) {
      previous.end = Math.max(previous.end, range.end);
      continue;
    }
    if (merged.length >= maximumHighlightRanges) break;
    merged.push(range);
  }
  return merged;
}

export function splitHighlightedText(
  text: string,
  ranges: readonly SearchHighlightRange[] | null | undefined,
): SearchHighlightSegment[] {
  const normalized = normalizeHighlightRanges(text, ranges);
  if (!normalized.length) {
    return text ? [{ text, highlighted: false }] : [];
  }

  const segments: SearchHighlightSegment[] = [];
  let cursor = 0;
  for (const range of normalized) {
    if (range.start > cursor) {
      segments.push({ text: text.slice(cursor, range.start), highlighted: false });
    }
    segments.push({ text: text.slice(range.start, range.end), highlighted: true });
    cursor = range.end;
  }
  if (cursor < text.length) {
    segments.push({ text: text.slice(cursor), highlighted: false });
  }
  return segments;
}

const matchedFieldLabels: Record<SearchMatchedField, string> = {
  subject: "Subject",
  sender: "Sender",
  recipients: "Recipients",
  body: "Body",
  attachment: "Attachment",
};

export function matchedFieldLabel(field: string): string | null {
  return Object.prototype.hasOwnProperty.call(matchedFieldLabels, field)
    ? matchedFieldLabels[field as SearchMatchedField]
    : null;
}

export function matchedFieldLabelsForResult(
  fields: readonly string[] | null | undefined,
): string[] {
  const labels: string[] = [];
  for (const field of fields ?? []) {
    const label = matchedFieldLabel(field);
    if (label && !labels.includes(label)) labels.push(label);
    if (labels.length === 5) break;
  }
  return labels;
}
