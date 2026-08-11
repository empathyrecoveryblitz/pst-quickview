import {
  Fragment,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
} from "react";
import type { MessageListItem, SearchHighlightRange } from "./types";
import type { SearchEmptyState, SearchExactCountStatus } from "./searchRequest";
import {
  matchedFieldLabelsForResult,
  splitHighlightedText,
} from "./searchHighlight";
import {
  ResultNavigationModel,
  createPendingResultFocus,
  resolvePendingResultFocus,
  shouldPublishResolvedNavigationKey,
  type PendingResultFocus,
} from "./resultNavigation";
import {
  DEFAULT_MESSAGE_ROW_ESTIMATE,
  VariableHeightWindow,
  type ScrollAnchor,
} from "./variableHeightWindow";

type MessageListDisplayMode = "subject_first" | "sender_first";

type MessageResultsListProps = {
  messages: readonly MessageListItem[];
  activeWorkspaceId: string | null;
  selectedMessageId: number | null;
  selectedWorkspaceId: string | null;
  displayMode: MessageListDisplayMode;
  isCrossPstSearch: boolean;
  highlightTerms: readonly string[];
  resetIdentity: number;
  measurementIdentity: string;
  activeNavigationKey: string | null;
  onActiveNavigationKeyChange: (key: string | null) => void;
  scrollContainerRef: RefObject<HTMLDivElement | null>;
  exactCountStatus: SearchExactCountStatus;
  exactTotalCount: number;
  hasMore: boolean;
  isSearching: boolean;
  isLoadingMore: boolean;
  resultSummaryText: string;
  emptyState: SearchEmptyState;
  formatCount: (count: number) => string;
  formatDate: (value: string | null | undefined) => string;
  cleanSnippet: (value: string | null | undefined) => string;
  onOpenMessage: (messageId: number, workspaceId?: string) => void | Promise<void>;
  onOpenPreview: (message: MessageListItem, workspaceId?: string) => void | Promise<void>;
  onLoadMore: () => void;
};

type ViewportState = {
  scrollTop: number;
  viewportHeight: number;
  measurementRevision: number;
};

function escapeRegex(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export function HighlightedText({
  text,
  terms,
}: {
  text: string;
  terms: readonly string[];
}): ReactNode {
  if (!text || !terms.length) return text;
  const pattern = terms
    .map(escapeRegex)
    .sort((left, right) => right.length - left.length)
    .join("|");
  if (!pattern) return text;

  const matcher = new RegExp(`(${pattern})`, "gi");
  return text.split(matcher).map((part, index) =>
    index % 2 === 1 ? (
      <mark className="match-highlight" key={`${part}-${index}`}>
        {part}
      </mark>
    ) : (
      part
    ),
  );
}

export function BackendHighlightedText({
  text,
  ranges,
}: {
  text: string;
  ranges: readonly SearchHighlightRange[];
}): ReactNode {
  return splitHighlightedText(text, ranges).map((segment, index) =>
    segment.highlighted ? (
      <mark className="match-highlight" key={`${segment.text}-${index}`}>
        {segment.text}
      </mark>
    ) : (
      <Fragment key={`${segment.text}-${index}`}>{segment.text}</Fragment>
    ),
  );
}

export function messageResultKey(
  message: MessageListItem,
  activeWorkspaceId: string | null,
): string {
  return `${message.workspaceId ?? activeWorkspaceId ?? "workspace"}:${message.id}`;
}

export function MessageResultsList({
  messages,
  activeWorkspaceId,
  selectedMessageId,
  selectedWorkspaceId,
  displayMode,
  isCrossPstSearch,
  highlightTerms,
  resetIdentity,
  measurementIdentity,
  activeNavigationKey,
  onActiveNavigationKeyChange,
  scrollContainerRef,
  exactCountStatus,
  exactTotalCount,
  hasMore,
  isSearching,
  isLoadingMore,
  resultSummaryText,
  emptyState,
  formatCount,
  formatDate,
  cleanSnippet,
  onOpenMessage,
  onOpenPreview,
  onLoadMore,
}: MessageResultsListProps) {
  // The current four-line row plus padding is about 90px; 96px leaves room for
  // borders and common badge/workspace rows until ResizeObserver measures it.
  const modelRef = useRef<VariableHeightWindow | null>(null);
  const priorMessagesRef = useRef<readonly MessageListItem[] | null>(null);
  const priorKeysRef = useRef<readonly string[]>([]);
  const resetIdentityRef = useRef(resetIdentity);
  const resetScrollPendingRef = useRef(true);
  const priorActiveLogicalIndexRef = useRef<number | undefined>(undefined);
  const restoreRemovedFocusRef = useRef(false);
  const rowElementsRef = useRef(new Map<string, HTMLDivElement>());
  const rowButtonsRef = useRef(new Map<string, HTMLButtonElement>());
  const rowRefCallbacksRef = useRef(
    new Map<string, (element: HTMLDivElement | null) => void>(),
  );
  const buttonRefCallbacksRef = useRef(
    new Map<string, (element: HTMLButtonElement | null) => void>(),
  );
  const rowResizeObserverRef = useRef<ResizeObserver | null>(null);
  const scrollFrameRef = useRef<number | null>(null);
  const measureFrameRef = useRef<number | null>(null);
  const pendingScrollAnchorRef = useRef<ScrollAnchor | null>(null);
  const [pendingFocus, setPendingFocus] = useState<PendingResultFocus | null>(null);
  const [viewport, setViewport] = useState<ViewportState>({
    scrollTop: 0,
    viewportHeight: 0,
    measurementRevision: 0,
  });

  const messageKeys = useMemo(
    () => messages.map((message) => messageResultKey(message, activeWorkspaceId)),
    [activeWorkspaceId, messages],
  );
  if (modelRef.current == null) {
    modelRef.current = new VariableHeightWindow(messageKeys, DEFAULT_MESSAGE_ROW_ESTIMATE);
  } else if (resetIdentityRef.current !== resetIdentity) {
    modelRef.current.reset(messageKeys);
    resetIdentityRef.current = resetIdentity;
    resetScrollPendingRef.current = true;
  } else if (priorMessagesRef.current !== messages || priorKeysRef.current !== messageKeys) {
    modelRef.current.syncKeys(messageKeys);
  }
  priorMessagesRef.current = messages;
  priorKeysRef.current = messageKeys;
  const windowModel = modelRef.current;

  const selectedKey = useMemo(() => {
    if (selectedMessageId == null) return null;
    const workspaceId = selectedWorkspaceId ?? activeWorkspaceId;
    const key = `${workspaceId ?? "workspace"}:${selectedMessageId}`;
    return windowModel.indexForKey(key) >= 0 ? key : null;
  }, [activeWorkspaceId, selectedMessageId, selectedWorkspaceId, windowModel, messages]);
  const navigationModel = useMemo(
    () =>
      new ResultNavigationModel(
        messageKeys.map((key, logicalIndex) => ({ key, logicalKey: key, logicalIndex })),
      ),
    [messageKeys],
  );
  const resolvedActiveKey = navigationModel.resolveActiveKey(
    activeNavigationKey,
    selectedKey,
    priorActiveLogicalIndexRef.current,
  );
  const resolvedActiveEntry = navigationModel.entry(resolvedActiveKey);
  if (activeNavigationKey && !navigationModel.has(activeNavigationKey)) {
    const container = scrollContainerRef.current;
    if (container?.contains(document.activeElement)) restoreRemovedFocusRef.current = true;
  }
  if (resolvedActiveEntry) {
    priorActiveLogicalIndexRef.current = resolvedActiveEntry.logicalIndex;
  } else if (navigationModel.size === 0 && !activeNavigationKey) {
    priorActiveLogicalIndexRef.current = undefined;
  }
  const activeLogicalKey = resolvedActiveEntry?.logicalKey ?? null;
  const pendingLogicalKey = navigationModel.entry(pendingFocus?.key)?.logicalKey ?? null;

  const scheduleViewportUpdate = useCallback(() => {
    if (scrollFrameRef.current != null) return;
    scrollFrameRef.current = window.requestAnimationFrame(() => {
      scrollFrameRef.current = null;
      const container = scrollContainerRef.current;
      if (!container) return;
      setViewport((current) => {
        const nextScrollTop = container.scrollTop;
        const nextViewportHeight = container.clientHeight;
        if (
          current.scrollTop === nextScrollTop &&
          current.viewportHeight === nextViewportHeight
        ) {
          return current;
        }
        return {
          ...current,
          scrollTop: nextScrollTop,
          viewportHeight: nextViewportHeight,
        };
      });
    });
  }, [scrollContainerRef]);

  const measureMountedRows = useCallback(() => {
    const container = scrollContainerRef.current;
    if (!container) return;
    const anchor = windowModel.captureScrollAnchor(container.scrollTop);
    let changed = false;
    for (const [key, element] of rowElementsRef.current) {
      const rectangleHeight = element.getBoundingClientRect().height;
      const measuredHeight = rectangleHeight > 0 ? rectangleHeight : element.offsetHeight;
      if (windowModel.updateMeasuredHeight(key, measuredHeight)) changed = true;
    }
    if (!changed) {
      scheduleViewportUpdate();
      return;
    }

    pendingScrollAnchorRef.current = anchor;
    setViewport((current) => ({
      scrollTop: container.scrollTop,
      viewportHeight: container.clientHeight,
      measurementRevision: current.measurementRevision + 1,
    }));
  }, [scheduleViewportUpdate, scrollContainerRef, windowModel]);

  const scheduleMeasurement = useCallback(() => {
    if (measureFrameRef.current != null) return;
    measureFrameRef.current = window.requestAnimationFrame(() => {
      measureFrameRef.current = null;
      measureMountedRows();
    });
  }, [measureMountedRows]);

  const setRowElement = useCallback(
    (key: string, element: HTMLDivElement | null) => {
      const previous = rowElementsRef.current.get(key);
      if (previous && previous !== element) rowResizeObserverRef.current?.unobserve(previous);
      if (!element) {
        rowElementsRef.current.delete(key);
        return;
      }
      rowElementsRef.current.set(key, element);
      rowResizeObserverRef.current?.observe(element);
      scheduleMeasurement();
    },
    [scheduleMeasurement],
  );

  const rowRefForKey = useCallback(
    (key: string) => {
      const existing = rowRefCallbacksRef.current.get(key);
      if (existing) return existing;
      const callback = (element: HTMLDivElement | null) => setRowElement(key, element);
      rowRefCallbacksRef.current.set(key, callback);
      return callback;
    },
    [setRowElement],
  );

  const buttonRefForKey = useCallback((key: string) => {
    const existing = buttonRefCallbacksRef.current.get(key);
    if (existing) return existing;
    const callback = (element: HTMLButtonElement | null) => {
      if (element) rowButtonsRef.current.set(key, element);
      else rowButtonsRef.current.delete(key);
    };
    buttonRefCallbacksRef.current.set(key, callback);
    return callback;
  }, []);

  useLayoutEffect(() => {
    const container = scrollContainerRef.current;
    if (!container) return;
    if (resetScrollPendingRef.current) {
      resetScrollPendingRef.current = false;
      container.scrollTop = 0;
      pendingScrollAnchorRef.current = null;
      setPendingFocus(null);
    }
    setViewport((current) => ({
      scrollTop: container.scrollTop,
      viewportHeight: container.clientHeight,
      measurementRevision: current.measurementRevision,
    }));
    scheduleMeasurement();
  }, [measurementIdentity, messages, resetIdentity, scheduleMeasurement, scrollContainerRef]);

  useEffect(() => {
    const container = scrollContainerRef.current;
    if (!container) return;
    const onScroll = () => scheduleViewportUpdate();
    const onWindowResize = () => {
      scheduleViewportUpdate();
      scheduleMeasurement();
    };
    container.addEventListener("scroll", onScroll, { passive: true });
    window.addEventListener("resize", onWindowResize);

    let containerResizeObserver: ResizeObserver | null = null;
    if (typeof ResizeObserver === "function") {
      rowResizeObserverRef.current = new ResizeObserver(() => scheduleMeasurement());
      for (const element of rowElementsRef.current.values()) {
        rowResizeObserverRef.current.observe(element);
      }
      containerResizeObserver = new ResizeObserver(() => {
        scheduleViewportUpdate();
        scheduleMeasurement();
      });
      containerResizeObserver.observe(container);
    }

    scheduleViewportUpdate();
    scheduleMeasurement();
    return () => {
      container.removeEventListener("scroll", onScroll);
      window.removeEventListener("resize", onWindowResize);
      rowResizeObserverRef.current?.disconnect();
      rowResizeObserverRef.current = null;
      containerResizeObserver?.disconnect();
    };
  }, [scheduleMeasurement, scheduleViewportUpdate, scrollContainerRef]);

  useEffect(() => {
    const currentKeys = new Set(messageKeys);
    for (const key of rowRefCallbacksRef.current.keys()) {
      if (!currentKeys.has(key)) rowRefCallbacksRef.current.delete(key);
    }
    for (const key of buttonRefCallbacksRef.current.keys()) {
      if (!currentKeys.has(key)) buttonRefCallbacksRef.current.delete(key);
    }
    const pendingResolution = resolvePendingResultFocus(
      pendingFocus,
      navigationModel,
      new Set(rowButtonsRef.current.keys()),
      resetIdentity,
    );
    if (pendingFocus && pendingResolution === "cancel") setPendingFocus(null);
    if (
      shouldPublishResolvedNavigationKey(
        activeNavigationKey,
        resolvedActiveKey,
        navigationModel.size,
      )
    ) {
      onActiveNavigationKeyChange(resolvedActiveKey);
    }
  }, [
    activeNavigationKey,
    messageKeys,
    navigationModel,
    onActiveNavigationKeyChange,
    pendingFocus,
    resetIdentity,
    resolvedActiveKey,
  ]);

  useEffect(
    () => () => {
      for (const frame of [scrollFrameRef, measureFrameRef]) {
        if (frame.current != null) window.cancelAnimationFrame(frame.current);
        frame.current = null;
      }
    },
    [],
  );

  const windowResult = windowModel.calculateWindow({
    scrollTop: viewport.scrollTop,
    viewportHeight: viewport.viewportHeight,
    pinnedKeys: [selectedKey, activeLogicalKey, pendingLogicalKey],
  });

  useLayoutEffect(() => {
    const anchor = pendingScrollAnchorRef.current;
    const container = scrollContainerRef.current;
    if (!anchor || !container) return;
    pendingScrollAnchorRef.current = null;
    const anchoredScrollTop = windowModel.restoreScrollAnchor(anchor, container.clientHeight);
    if (anchoredScrollTop == null) return;
    if (Math.abs(container.scrollTop - anchoredScrollTop) >= 0.25) {
      container.scrollTop = anchoredScrollTop;
    }
    setViewport((current) => {
      if (
        current.scrollTop === container.scrollTop &&
        current.viewportHeight === container.clientHeight
      ) {
        return current;
      }
      return {
        ...current,
        scrollTop: container.scrollTop,
        viewportHeight: container.clientHeight,
      };
    });
  }, [scrollContainerRef, viewport.measurementRevision, windowModel]);

  useLayoutEffect(() => {
    const container = scrollContainerRef.current;
    const pendingResolution = resolvePendingResultFocus(
      pendingFocus,
      navigationModel,
      new Set(rowButtonsRef.current.keys()),
      resetIdentity,
    );
    const focusKey =
      pendingResolution === "focus"
        ? pendingFocus?.key ?? null
        : document.activeElement === container || restoreRemovedFocusRef.current
          ? resolvedActiveKey
          : null;
    if (!focusKey) return;
    const button = rowButtonsRef.current.get(focusKey);
    if (!button) return;
    restoreRemovedFocusRef.current = false;
    setPendingFocus(null);
    try {
      button.focus({ preventScroll: true });
    } catch {
      button.focus();
    }
  }, [
    navigationModel,
    pendingFocus,
    resetIdentity,
    resolvedActiveKey,
    scrollContainerRef,
    viewport.measurementRevision,
    viewport.scrollTop,
    windowResult.renderedRowCount,
  ]);

  const requestFocusByKey = useCallback(
    (key: string | null) => {
      const pending = createPendingResultFocus(navigationModel, key, resetIdentity);
      const entry = navigationModel.entry(key);
      if (!pending || !entry) return;
      onActiveNavigationKeyChange(entry.key);
      setPendingFocus(pending);
      const container = scrollContainerRef.current;
      if (!container) return;
      const nextScrollTop = windowModel.scrollTopForIndex(
        entry.logicalIndex,
        container.scrollTop,
        container.clientHeight,
      );
      if (Math.abs(container.scrollTop - nextScrollTop) >= 0.25) {
        container.scrollTop = nextScrollTop;
      }
      setViewport((current) => ({
        ...current,
        scrollTop: container.scrollTop,
        viewportHeight: container.clientHeight,
      }));
    },
    [
      navigationModel,
      onActiveNavigationKeyChange,
      resetIdentity,
      scrollContainerRef,
      windowModel,
    ],
  );

  const handleRowNavigation = useCallback(
    (event: KeyboardEvent<HTMLButtonElement>, key: string) => {
      let targetKey: string | null = null;
      switch (event.key) {
        case "ArrowDown":
          targetKey = navigationModel.nextKey(key);
          break;
        case "ArrowUp":
          targetKey = navigationModel.previousKey(key);
          break;
        case "Home":
          targetKey = navigationModel.firstKey();
          break;
        case "End":
          targetKey = navigationModel.lastKey();
          break;
        case "PageDown":
        case "PageUp": {
          const direction = event.key === "PageDown" ? 1 : -1;
          const container = scrollContainerRef.current;
          const targetIndex = windowModel.pageIndexForKey(
            key,
            direction,
            container?.clientHeight ?? 0,
          );
          targetKey = navigationModel.pageKey(key, targetIndex, direction);
          break;
        }
        default:
          return;
      }
      event.preventDefault();
      if (targetKey) requestFocusByKey(targetKey);
    },
    [navigationModel, requestFocusByKey, scrollContainerRef, windowModel],
  );

  const ariaSetSize = exactCountStatus === "ready" ? exactTotalCount : -1;

  return (
    <>
      {windowResult.runs.flatMap((run): ReactNode[] => {
        if (run.kind === "spacer") {
          return [
            <div
              aria-hidden="true"
              className="message-window-spacer"
              key={`spacer:${run.startIndex}:${run.endIndex}`}
              role="presentation"
              style={{ height: `${run.height}px` }}
            />,
          ];
        }

        const rows: ReactNode[] = [];
        for (let index = run.startIndex; index < run.endIndex; index += 1) {
          const message = messages[index];
          const key = messageKeys[index];
          const rowWorkspaceId = message.workspaceId ?? activeWorkspaceId ?? undefined;
          const displayDate = formatDate(message.date);
          const matchContext = message.searchMatchContext;
          const displaySnippet = matchContext?.snippetText || cleanSnippet(message.snippet);
          const matchedFields = matchedFieldLabelsForResult(matchContext?.matchedFields);
          const isSelectedRow =
            selectedMessageId === message.id &&
            (rowWorkspaceId ?? null) === (selectedWorkspaceId ?? activeWorkspaceId ?? null);
          const primaryText =
            displayMode === "sender_first"
              ? message.sender || "(No sender)"
              : message.subject || "(No subject)";
          const secondaryText =
            displayMode === "sender_first"
              ? message.subject || "(No subject)"
              : message.sender || "(No sender)";

          rows.push(
            <div
              aria-posinset={index + 1}
              aria-setsize={ariaSetSize}
              className="message-window-item"
              key={key}
              ref={rowRefForKey(key)}
              role="listitem"
            >
              <button
                aria-current={isSelectedRow ? "true" : undefined}
                className={`message-row ${isSelectedRow ? "selected" : ""}`}
                data-message-result-key={key}
                data-result-navigation-key={key}
                onClick={() => void onOpenMessage(message.id, rowWorkspaceId)}
                onDoubleClick={() => void onOpenPreview(message, rowWorkspaceId)}
                onFocus={() => onActiveNavigationKeyChange(key)}
                onKeyDown={(event) => handleRowNavigation(event, key)}
                ref={buttonRefForKey(key)}
                tabIndex={key === resolvedActiveKey ? 0 : -1}
                type="button"
              >
                <span className="message-subject">
                  {primaryText}
                  {message.attachmentCount > 0 ? (
                    <span className="attachment-dot">
                      {"\u{1F4CE} "}
                      {formatCount(message.attachmentCount)}
                    </span>
                  ) : null}
                </span>
                {isCrossPstSearch && message.pstDisplayName ? (
                  <span className="message-workspace" title={message.workspacePath || undefined}>
                    {message.pstDisplayName}
                    {message.folderPath ? ` - ${message.folderPath}` : ""}
                  </span>
                ) : null}
                <span className="message-meta">{secondaryText}</span>
                <span className="message-snippet" title={displaySnippet || undefined}>
                  {matchContext ? (
                    <BackendHighlightedText
                      ranges={matchContext.highlightRanges}
                      text={displaySnippet}
                    />
                  ) : (
                    <HighlightedText text={displaySnippet} terms={highlightTerms} />
                  )}
                </span>
                {matchedFields.length ? (
                  <span
                    aria-label={`Matched fields: ${matchedFields.join(", ")}`}
                    className="message-match-fields"
                  >
                    {matchedFields.map((field) => (
                      <span className="message-match-field" key={field}>
                        {field}
                      </span>
                    ))}
                  </span>
                ) : null}
                <span className="message-date" title={message.date || undefined}>
                  {displayDate}
                </span>
              </button>
            </div>,
          );
        }
        return rows;
      })}

      {messages.length === 0 ? (
        <div
          aria-live={emptyState.kind === "loading" ? "polite" : undefined}
          className="empty-state search-empty-state"
          role={emptyState.kind === "loading" ? "status" : undefined}
        >
          <strong>{emptyState.title}</strong>
          {emptyState.detail ? <span>{emptyState.detail}</span> : null}
        </div>
      ) : null}

      {hasMore ? (
        <div className="load-more-row">
          <button
            disabled={isSearching || isLoadingMore}
            onClick={onLoadMore}
            type="button"
          >
            {isLoadingMore
              ? "Loading..."
              : exactCountStatus === "ready"
                ? `Load More (${formatCount(
                    Math.max(0, exactTotalCount - messages.length),
                  )} remaining)`
                : "Load More"}
          </button>
          <span>{resultSummaryText}</span>
        </div>
      ) : messages.length ? (
        <p className="list-count-note">{resultSummaryText}</p>
      ) : null}
    </>
  );
}
