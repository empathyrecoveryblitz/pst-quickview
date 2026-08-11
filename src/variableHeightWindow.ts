export const DEFAULT_MESSAGE_ROW_ESTIMATE = 96;
export const MAX_MEASURED_ROW_HEIGHT = 4096;

export type VariableHeightWindowOptions = {
  minimumOverscanRows?: number;
  maximumOverscanRows?: number;
  maximumOrdinaryRows?: number;
};

export type VariableHeightItemDefinition = {
  key: string;
  estimatedHeight?: number;
};

export type ScrollAnchor = {
  key: string;
  viewportOffset: number;
};

export type WindowItemRun = {
  kind: "items";
  startIndex: number;
  endIndex: number;
};

export type WindowSpacerRun = {
  kind: "spacer";
  startIndex: number;
  endIndex: number;
  height: number;
};

export type WindowRun = WindowItemRun | WindowSpacerRun;

export type VariableHeightWindowResult = {
  visibleStartIndex: number;
  visibleEndIndex: number;
  overscanStartIndex: number;
  overscanEndIndex: number;
  renderedIndices: number[];
  runs: WindowRun[];
  totalHeight: number;
  renderedRowCount: number;
};

type WindowCalculation = {
  scrollTop: number;
  viewportHeight: number;
  pinnedIndices?: readonly number[];
  pinnedKeys?: readonly (string | null | undefined)[];
};

const DEFAULT_MINIMUM_OVERSCAN_ROWS = 6;
const DEFAULT_MAXIMUM_OVERSCAN_ROWS = 20;
const DEFAULT_MAXIMUM_ORDINARY_ROWS = 80;
const MEASUREMENT_EPSILON = 0.25;

function finiteNonnegative(value: number): number {
  return Number.isFinite(value) && value > 0 ? value : 0;
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(maximum, Math.max(minimum, value));
}

function integerOption(value: number | undefined, fallback: number, minimum: number): number {
  if (!Number.isFinite(value)) return fallback;
  return Math.max(minimum, Math.floor(value ?? fallback));
}

class FenwickHeightIndex {
  private tree: number[] = [0];

  reset(values: readonly number[]): void {
    this.tree = new Array(values.length + 1).fill(0);
    for (let index = 1; index <= values.length; index += 1) {
      this.tree[index] += values[index - 1];
      const parent = index + (index & -index);
      if (parent <= values.length) this.tree[parent] += this.tree[index];
    }
  }

  update(index: number, delta: number): void {
    for (let cursor = index + 1; cursor < this.tree.length; cursor += cursor & -cursor) {
      this.tree[cursor] += delta;
    }
  }

  prefixHeight(endIndex: number): number {
    const itemCount = this.tree.length - 1;
    let cursor = clamp(Math.floor(endIndex), 0, itemCount);
    let total = 0;
    while (cursor > 0) {
      total += this.tree[cursor];
      cursor -= cursor & -cursor;
    }
    return total;
  }

  totalHeight(): number {
    return this.prefixHeight(this.tree.length - 1);
  }

  indexAtOffset(offset: number): number {
    const itemCount = this.tree.length - 1;
    if (itemCount === 0) return -1;

    const total = this.totalHeight();
    const target = clamp(Number.isFinite(offset) ? offset : 0, 0, Math.max(0, total - 0.001));
    let index = 0;
    let accumulated = 0;
    let bit = 1;
    while (bit * 2 <= itemCount) bit *= 2;

    while (bit > 0) {
      const next = index + bit;
      if (next <= itemCount && accumulated + this.tree[next] <= target) {
        index = next;
        accumulated += this.tree[next];
      }
      bit = Math.floor(bit / 2);
    }

    return Math.min(index, itemCount - 1);
  }
}

/**
 * Prefix-sum window state for variable-height rows. Row measurements and offset
 * lookups are O(log n); work performed per window is proportional to mounted
 * rows plus the selected/focused pins.
 */
export class VariableHeightWindow {
  private keys: string[] = [];
  private heights: number[] = [];
  private estimatedHeights: number[] = [];
  private measuredHeights = new Map<string, number>();
  private keyIndices = new Map<string, number>();
  private heightIndex = new FenwickHeightIndex();
  readonly estimatedRowHeight: number;
  readonly minimumOverscanRows: number;
  readonly maximumOverscanRows: number;
  readonly maximumOrdinaryRows: number;

  constructor(
    keys: readonly string[] = [],
    estimatedRowHeight = DEFAULT_MESSAGE_ROW_ESTIMATE,
    options: VariableHeightWindowOptions = {},
  ) {
    this.estimatedRowHeight =
      Number.isFinite(estimatedRowHeight) && estimatedRowHeight > 0
        ? estimatedRowHeight
        : DEFAULT_MESSAGE_ROW_ESTIMATE;
    this.minimumOverscanRows = integerOption(
      options.minimumOverscanRows,
      DEFAULT_MINIMUM_OVERSCAN_ROWS,
      0,
    );
    this.maximumOverscanRows = Math.max(
      this.minimumOverscanRows,
      integerOption(options.maximumOverscanRows, DEFAULT_MAXIMUM_OVERSCAN_ROWS, 0),
    );
    this.maximumOrdinaryRows = integerOption(
      options.maximumOrdinaryRows,
      DEFAULT_MAXIMUM_ORDINARY_ROWS,
      1,
    );
    this.reset(keys);
  }

  get itemCount(): number {
    return this.keys.length;
  }

  keyAt(index: number): string | null {
    return Number.isInteger(index) && index >= 0 && index < this.keys.length
      ? this.keys[index]
      : null;
  }

  indexForKey(key: string | null | undefined): number {
    if (!key) return -1;
    return this.keyIndices.get(key) ?? -1;
  }

  reset(keys: readonly string[] = []): void {
    this.resetItems(keys.map((key) => ({ key })));
  }

  resetItems(items: readonly VariableHeightItemDefinition[] = []): void {
    this.measuredHeights.clear();
    this.rebuild(items);
  }

  syncKeys(keys: readonly string[]): boolean {
    return this.syncItems(keys.map((key) => ({ key })));
  }

  syncItems(items: readonly VariableHeightItemDefinition[]): boolean {
    const normalizedEstimates = items.map((item) => this.normalizeEstimate(item.estimatedHeight));
    if (
      items.length === this.keys.length &&
      items.every(
        (item, index) =>
          item.key === this.keys[index] &&
          normalizedEstimates[index] === this.estimatedHeights[index],
      )
    ) {
      return false;
    }
    this.rebuild(items, normalizedEstimates);
    return true;
  }

  appendKeys(keys: readonly string[]): boolean {
    if (keys.length === 0) return false;
    return this.syncItems([
      ...this.keys.map((key, index) => ({
        key,
        estimatedHeight: this.estimatedHeights[index],
      })),
      ...keys.map((key) => ({ key })),
    ]);
  }

  appendItems(items: readonly VariableHeightItemDefinition[]): boolean {
    if (items.length === 0) return false;
    return this.syncItems([
      ...this.keys.map((key, index) => ({
        key,
        estimatedHeight: this.estimatedHeights[index],
      })),
      ...items,
    ]);
  }

  private normalizeEstimate(estimatedHeight: number | undefined): number {
    return Number.isFinite(estimatedHeight) && (estimatedHeight ?? 0) > 0
      ? estimatedHeight!
      : this.estimatedRowHeight;
  }

  private rebuild(
    items: readonly VariableHeightItemDefinition[],
    normalizedEstimates = items.map((item) => this.normalizeEstimate(item.estimatedHeight)),
  ): void {
    this.keys = items.map((item) => item.key);
    const activeKeys = new Set(this.keys);
    for (const key of this.measuredHeights.keys()) {
      if (!activeKeys.has(key)) this.measuredHeights.delete(key);
    }
    this.estimatedHeights = normalizedEstimates;
    this.keyIndices = new Map(this.keys.map((key, index) => [key, index]));
    this.heights = this.keys.map(
      (key, index) => this.measuredHeights.get(key) ?? this.estimatedHeights[index],
    );
    this.heightIndex.reset(this.heights);
  }

  updateMeasuredHeight(key: string, measuredHeight: number): boolean {
    const index = this.indexForKey(key);
    if (
      index < 0 ||
      !Number.isFinite(measuredHeight) ||
      measuredHeight <= 0 ||
      measuredHeight > MAX_MEASURED_ROW_HEIGHT
    ) {
      return false;
    }

    const previous = this.heights[index];
    if (Math.abs(previous - measuredHeight) < MEASUREMENT_EPSILON) return false;
    this.heights[index] = measuredHeight;
    this.measuredHeights.set(key, measuredHeight);
    this.heightIndex.update(index, measuredHeight - previous);
    return true;
  }

  heightAt(index: number): number {
    return Number.isInteger(index) && index >= 0 && index < this.heights.length
      ? this.heights[index]
      : 0;
  }

  offsetForIndex(index: number): number {
    return this.heightIndex.prefixHeight(index);
  }

  heightBetween(startIndex: number, endIndex: number): number {
    const start = clamp(Math.floor(startIndex), 0, this.itemCount);
    const end = clamp(Math.floor(endIndex), start, this.itemCount);
    return this.heightIndex.prefixHeight(end) - this.heightIndex.prefixHeight(start);
  }

  totalHeight(): number {
    return this.heightIndex.totalHeight();
  }

  indexAtOffset(offset: number): number {
    return this.heightIndex.indexAtOffset(offset);
  }

  captureScrollAnchor(scrollTop: number): ScrollAnchor | null {
    if (this.itemCount === 0) return null;
    const safeScrollTop = clamp(
      Number.isFinite(scrollTop) ? scrollTop : 0,
      0,
      Math.max(0, this.totalHeight() - 0.001),
    );
    const index = this.indexAtOffset(safeScrollTop);
    const key = this.keyAt(index);
    if (!key) return null;
    return {
      key,
      viewportOffset: this.offsetForIndex(index) - safeScrollTop,
    };
  }

  restoreScrollAnchor(anchor: ScrollAnchor | null, viewportHeight = 0): number | null {
    if (!anchor) return null;
    const index = this.indexForKey(anchor.key);
    if (index < 0 || !Number.isFinite(anchor.viewportOffset)) return null;
    const maximum = Math.max(0, this.totalHeight() - finiteNonnegative(viewportHeight));
    return clamp(this.offsetForIndex(index) - anchor.viewportOffset, 0, maximum);
  }

  scrollTopForIndex(
    index: number,
    currentScrollTop: number,
    viewportHeight: number,
  ): number {
    if (!Number.isInteger(index) || index < 0 || index >= this.itemCount) {
      return finiteNonnegative(currentScrollTop);
    }
    const safeViewportHeight = finiteNonnegative(viewportHeight);
    const maximum = Math.max(0, this.totalHeight() - safeViewportHeight);
    const current = clamp(
      Number.isFinite(currentScrollTop) ? currentScrollTop : 0,
      0,
      maximum,
    );
    const rowStart = this.offsetForIndex(index);
    const rowEnd = rowStart + this.heightAt(index);
    if (rowStart < current) return clamp(rowStart, 0, maximum);
    if (rowEnd > current + safeViewportHeight) {
      return clamp(rowEnd - safeViewportHeight, 0, maximum);
    }
    return current;
  }

  pageIndexForKey(
    key: string | null | undefined,
    direction: -1 | 1,
    viewportHeight: number,
  ): number {
    const index = this.indexForKey(key);
    if (index < 0) return -1;
    const pageHeight = finiteNonnegative(viewportHeight);
    if (pageHeight === 0) return index;
    const targetOffset = this.offsetForIndex(index) + pageHeight * direction;
    let targetIndex = this.indexAtOffset(targetOffset);
    if (targetIndex === index) {
      targetIndex = clamp(index + direction, 0, this.itemCount - 1);
    }
    return targetIndex;
  }

  calculateWindow({
    scrollTop,
    viewportHeight,
    pinnedIndices = [],
    pinnedKeys = [],
  }: WindowCalculation): VariableHeightWindowResult {
    const totalHeight = this.totalHeight();
    if (this.itemCount === 0) {
      return {
        visibleStartIndex: 0,
        visibleEndIndex: 0,
        overscanStartIndex: 0,
        overscanEndIndex: 0,
        renderedIndices: [],
        runs: [],
        totalHeight: 0,
        renderedRowCount: 0,
      };
    }

    const safeViewportHeight = finiteNonnegative(viewportHeight);
    const maximumScrollTop = Math.max(
      0,
      totalHeight - (safeViewportHeight > 0 ? safeViewportHeight : 0.001),
    );
    const safeScrollTop = clamp(
      Number.isFinite(scrollTop) ? scrollTop : 0,
      0,
      maximumScrollTop,
    );
    const visibleStartIndex = this.indexAtOffset(safeScrollTop);
    const visibleEndOffset = Math.min(
      totalHeight - 0.001,
      safeScrollTop + Math.max(1, safeViewportHeight) - 0.001,
    );
    const visibleEndIndex = this.indexAtOffset(visibleEndOffset) + 1;

    const viewportBeforeIndex = this.indexAtOffset(
      Math.max(0, safeScrollTop - safeViewportHeight),
    );
    const viewportAfterIndex =
      this.indexAtOffset(
        Math.min(totalHeight - 0.001, safeScrollTop + safeViewportHeight * 2),
      ) + 1;
    const desiredBefore = Math.min(
      visibleStartIndex,
      clamp(
        Math.max(
          this.minimumOverscanRows,
          visibleStartIndex - viewportBeforeIndex,
        ),
        0,
        this.maximumOverscanRows,
      ),
    );
    const desiredAfter = Math.min(
      this.itemCount - visibleEndIndex,
      clamp(
        Math.max(
          this.minimumOverscanRows,
          viewportAfterIndex - visibleEndIndex,
        ),
        0,
        this.maximumOverscanRows,
      ),
    );

    const visibleCount = visibleEndIndex - visibleStartIndex;
    let remainingBudget = Math.max(0, this.maximumOrdinaryRows - visibleCount);
    let beforeCount = Math.min(desiredBefore, Math.floor(remainingBudget / 2));
    let afterCount = Math.min(desiredAfter, remainingBudget - beforeCount);
    remainingBudget -= beforeCount + afterCount;
    if (remainingBudget > 0) {
      const extraBefore = Math.min(desiredBefore - beforeCount, remainingBudget);
      beforeCount += extraBefore;
      remainingBudget -= extraBefore;
    }
    if (remainingBudget > 0) {
      afterCount += Math.min(desiredAfter - afterCount, remainingBudget);
    }

    const overscanStartIndex = visibleStartIndex - beforeCount;
    const overscanEndIndex = visibleEndIndex + afterCount;
    const rendered = new Set<number>();
    for (let index = overscanStartIndex; index < overscanEndIndex; index += 1) {
      rendered.add(index);
    }
    for (const index of pinnedIndices) {
      if (Number.isInteger(index) && index >= 0 && index < this.itemCount) rendered.add(index);
    }
    for (const key of pinnedKeys) {
      const index = this.indexForKey(key);
      if (index >= 0) rendered.add(index);
    }

    const renderedIndices = [...rendered].sort((left, right) => left - right);
    const runs = this.buildRuns(renderedIndices);
    return {
      visibleStartIndex,
      visibleEndIndex,
      overscanStartIndex,
      overscanEndIndex,
      renderedIndices,
      runs,
      totalHeight,
      renderedRowCount: renderedIndices.length,
    };
  }

  private buildRuns(renderedIndices: readonly number[]): WindowRun[] {
    if (this.itemCount === 0) return [];
    if (renderedIndices.length === 0) {
      return [
        {
          kind: "spacer",
          startIndex: 0,
          endIndex: this.itemCount,
          height: this.totalHeight(),
        },
      ];
    }

    const runs: WindowRun[] = [];
    let cursor = 0;
    let position = 0;
    while (position < renderedIndices.length) {
      const itemStart = renderedIndices[position];
      if (itemStart > cursor) {
        runs.push({
          kind: "spacer",
          startIndex: cursor,
          endIndex: itemStart,
          height: this.heightBetween(cursor, itemStart),
        });
      }

      let itemEnd = itemStart + 1;
      position += 1;
      while (
        position < renderedIndices.length &&
        renderedIndices[position] === itemEnd
      ) {
        itemEnd += 1;
        position += 1;
      }
      runs.push({ kind: "items", startIndex: itemStart, endIndex: itemEnd });
      cursor = itemEnd;
    }

    if (cursor < this.itemCount) {
      runs.push({
        kind: "spacer",
        startIndex: cursor,
        endIndex: this.itemCount,
        height: this.heightBetween(cursor, this.itemCount),
      });
    }
    return runs;
  }
}
