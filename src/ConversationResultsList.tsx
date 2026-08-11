import {
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
import { conversationParticipantSummary } from "./conversationDisplay";
import {
  CONVERSATION_ENTRY_HEIGHT_ESTIMATES,
  buildConversationNavigationEntries,
  conversationListLoadMoreNavigationKey,
  conversationWorkspaceActionNavigationKey,
  expandedLoadMoreNavigationKey,
  expandedMessageEntryKey,
  expandedShowEntireNavigationKey,
  flattenConversationResults,
  type ConversationLogicalEntry,
  type ExpandedConversationState,
} from "./conversationResultsModel";
import { HighlightedText } from "./MessageResultsList";
import type {
  ConversationMessageItem,
  ConversationSummary,
  ConversationWorkspaceIssue,
} from "./types";
import type { SearchEmptyState, SearchExactCountStatus } from "./searchRequest";
import {
  ResultNavigationModel,
  createPendingResultFocus,
  resolvePendingResultFocus,
  shouldPublishResolvedNavigationKey,
  type PendingResultFocus,
} from "./resultNavigation";
import {
  VariableHeightWindow,
  type ScrollAnchor,
  type VariableHeightItemDefinition,
} from "./variableHeightWindow";

type ConversationResultsListProps = {
  conversations: readonly ConversationSummary[];
  expandedConversations: Readonly<Record<string, ExpandedConversationState>>;
  workspaceIssues: readonly ConversationWorkspaceIssue[];
  conversationReindexRequired: boolean;
  selectedMessageId: number | null;
  selectedWorkspaceId: string | null;
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
  isBusy: boolean;
  resultSummaryText: string;
  emptyState: SearchEmptyState;
  formatCount: (count: number) => string;
  formatDate: (value: string | null | undefined) => string;
  cleanSnippet: (value: string | null | undefined) => string;
  onToggleConversation: (conversation: ConversationSummary) => void;
  onOpenMessage: (messageId: number, workspaceId?: string) => void | Promise<void>;
  onOpenPreview: (message: ConversationMessageItem, workspaceId?: string) => void | Promise<void>;
  onShowEntireConversation: (conversation: ConversationSummary) => void;
  onLoadMoreConversation: (
    conversation: ConversationSummary,
    showingEntireConversation: boolean,
  ) => void;
  onLoadMoreConversations: () => void;
  onReindexWorkspace: (workspaceId: string) => void | Promise<void>;
};

type ViewportState = {
  scrollTop: number;
  viewportHeight: number;
  measurementRevision: number;
};

function entryDefinitions(
  entries: readonly ConversationLogicalEntry[],
): VariableHeightItemDefinition[] {
  return entries.map((entry) => ({ key: entry.key, estimatedHeight: entry.estimatedHeight }));
}

function entryDefinitionsEqual(
  left: readonly ConversationLogicalEntry[],
  right: readonly ConversationLogicalEntry[],
): boolean {
  return (
    left.length === right.length &&
    left.every(
      (entry, index) =>
        entry.key === right[index].key && entry.estimatedHeight === right[index].estimatedHeight,
    )
  );
}

function resolveStructureAnchor(
  anchor: ScrollAnchor | null,
  previousEntries: readonly ConversationLogicalEntry[],
  nextEntries: readonly ConversationLogicalEntry[],
): ScrollAnchor | null {
  if (!anchor) return null;
  const nextKeys = new Set(nextEntries.map((entry) => entry.key));
  if (nextKeys.has(anchor.key)) return anchor;

  const previousIndex = previousEntries.findIndex((entry) => entry.key === anchor.key);
  if (previousIndex < 0) return null;
  const previousEntry = previousEntries[previousIndex];
  if (previousEntry.parentConversationKey) {
    const parentHeader = nextEntries.find(
      (entry) =>
        entry.kind === "conversation-header" &&
        entry.parentConversationKey === previousEntry.parentConversationKey,
    );
    if (parentHeader) return { ...anchor, key: parentHeader.key };
  }

  for (let distance = 1; distance < previousEntries.length; distance += 1) {
    const after = previousEntries[previousIndex + distance];
    if (after && nextKeys.has(after.key)) return { ...anchor, key: after.key };
    const before = previousEntries[previousIndex - distance];
    if (before && nextKeys.has(before.key)) return { ...anchor, key: before.key };
  }
  return null;
}

export function ConversationResultsList({
  conversations,
  expandedConversations,
  workspaceIssues,
  conversationReindexRequired,
  selectedMessageId,
  selectedWorkspaceId,
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
  isBusy,
  resultSummaryText,
  emptyState,
  formatCount,
  formatDate,
  cleanSnippet,
  onToggleConversation,
  onOpenMessage,
  onOpenPreview,
  onShowEntireConversation,
  onLoadMoreConversation,
  onLoadMoreConversations,
  onReindexWorkspace,
}: ConversationResultsListProps) {
  const logicalEntries = useMemo(
    () =>
      flattenConversationResults({
        conversations,
        expandedConversations,
        workspaceIssues,
        workspaceActionsDisabled: isBusy,
        hasMoreConversations: hasMore,
        topLevelLoadMoreDisabled: isSearching || isLoadingMore,
      }),
    [
      conversations,
      expandedConversations,
      hasMore,
      isBusy,
      isLoadingMore,
      isSearching,
      workspaceIssues,
    ],
  );
  const definitions = useMemo(() => entryDefinitions(logicalEntries), [logicalEntries]);
  const navigationEntries = useMemo(
    () => buildConversationNavigationEntries(logicalEntries),
    [logicalEntries],
  );
  const navigationModel = useMemo(
    () => new ResultNavigationModel(navigationEntries),
    [navigationEntries],
  );
  const modelRef = useRef<VariableHeightWindow | null>(null);
  const priorEntriesRef = useRef<readonly ConversationLogicalEntry[]>([]);
  const resetIdentityRef = useRef(resetIdentity);
  const resetScrollPendingRef = useRef(true);
  const priorActiveLogicalIndexRef = useRef<number | undefined>(undefined);
  const restoreRemovedFocusRef = useRef(false);
  const entryElementsRef = useRef(new Map<string, HTMLDivElement>());
  const entryRefCallbacksRef = useRef(
    new Map<string, (element: HTMLDivElement | null) => void>(),
  );
  const navigationButtonsRef = useRef(new Map<string, HTMLButtonElement>());
  const navigationButtonRefCallbacksRef = useRef(
    new Map<string, (element: HTMLButtonElement | null) => void>(),
  );
  const entryResizeObserverRef = useRef<ResizeObserver | null>(null);
  const scrollFrameRef = useRef<number | null>(null);
  const measureFrameRef = useRef<number | null>(null);
  const pendingScrollAnchorRef = useRef<ScrollAnchor | null>(null);
  const [pendingFocus, setPendingFocus] = useState<PendingResultFocus | null>(null);
  const [viewport, setViewport] = useState<ViewportState>({
    scrollTop: 0,
    viewportHeight: 0,
    measurementRevision: 0,
  });

  if (modelRef.current == null) {
    modelRef.current = new VariableHeightWindow(
      [],
      CONVERSATION_ENTRY_HEIGHT_ESTIMATES.conversationHeader,
    );
    modelRef.current.resetItems(definitions);
  } else if (resetIdentityRef.current !== resetIdentity) {
    modelRef.current.resetItems(definitions);
    resetIdentityRef.current = resetIdentity;
    resetScrollPendingRef.current = true;
    pendingScrollAnchorRef.current = null;
  } else if (priorEntriesRef.current !== logicalEntries) {
    if (!entryDefinitionsEqual(priorEntriesRef.current, logicalEntries)) {
      const container = scrollContainerRef.current;
      const anchor = container
        ? modelRef.current.captureScrollAnchor(container.scrollTop)
        : null;
      pendingScrollAnchorRef.current = resolveStructureAnchor(
        anchor,
        priorEntriesRef.current,
        logicalEntries,
      );
    }
    modelRef.current.syncItems(definitions);
  }
  priorEntriesRef.current = logicalEntries;
  const windowModel = modelRef.current;

  const selectedKey = useMemo(() => {
    if (selectedMessageId == null || !selectedWorkspaceId) return null;
    const key = expandedMessageEntryKey(selectedWorkspaceId, selectedMessageId);
    return windowModel.indexForKey(key) >= 0 ? key : null;
  }, [logicalEntries, selectedMessageId, selectedWorkspaceId, windowModel]);
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

  const measureMountedEntries = useCallback(() => {
    const container = scrollContainerRef.current;
    if (!container) return;
    const anchor = windowModel.captureScrollAnchor(container.scrollTop);
    let changed = false;
    for (const [key, element] of entryElementsRef.current) {
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
      measureMountedEntries();
    });
  }, [measureMountedEntries]);

  const setEntryElement = useCallback(
    (key: string, element: HTMLDivElement | null) => {
      const previous = entryElementsRef.current.get(key);
      if (previous && previous !== element) entryResizeObserverRef.current?.unobserve(previous);
      if (!element) {
        entryElementsRef.current.delete(key);
        return;
      }
      entryElementsRef.current.set(key, element);
      entryResizeObserverRef.current?.observe(element);
      scheduleMeasurement();
    },
    [scheduleMeasurement],
  );

  const entryRefForKey = useCallback(
    (key: string) => {
      const existing = entryRefCallbacksRef.current.get(key);
      if (existing) return existing;
      const callback = (element: HTMLDivElement | null) => setEntryElement(key, element);
      entryRefCallbacksRef.current.set(key, callback);
      return callback;
    },
    [setEntryElement],
  );

  const navigationButtonRefForKey = useCallback((key: string) => {
    const existing = navigationButtonRefCallbacksRef.current.get(key);
    if (existing) return existing;
    const callback = (element: HTMLButtonElement | null) => {
      if (element) navigationButtonsRef.current.set(key, element);
      else navigationButtonsRef.current.delete(key);
    };
    navigationButtonRefCallbacksRef.current.set(key, callback);
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
  }, [
    exactCountStatus,
    exactTotalCount,
    isLoadingMore,
    logicalEntries,
    measurementIdentity,
    resetIdentity,
    resultSummaryText,
    scheduleMeasurement,
    scrollContainerRef,
  ]);

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
      entryResizeObserverRef.current = new ResizeObserver(() => scheduleMeasurement());
      for (const element of entryElementsRef.current.values()) {
        entryResizeObserverRef.current.observe(element);
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
      entryResizeObserverRef.current?.disconnect();
      entryResizeObserverRef.current = null;
      containerResizeObserver?.disconnect();
    };
  }, [scheduleMeasurement, scheduleViewportUpdate, scrollContainerRef]);

  useEffect(() => {
    const currentEntries = new Map(logicalEntries.map((entry) => [entry.key, entry]));
    for (const key of entryRefCallbacksRef.current.keys()) {
      if (!currentEntries.has(key)) entryRefCallbacksRef.current.delete(key);
    }
    const currentNavigationKeys = new Set(navigationEntries.map((entry) => entry.key));
    for (const key of navigationButtonRefCallbacksRef.current.keys()) {
      if (!currentNavigationKeys.has(key)) navigationButtonRefCallbacksRef.current.delete(key);
    }
    const pendingResolution = resolvePendingResultFocus(
      pendingFocus,
      navigationModel,
      new Set(navigationButtonsRef.current.keys()),
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
    logicalEntries,
    navigationEntries,
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
    setViewport((current) => ({
      ...current,
      scrollTop: container.scrollTop,
      viewportHeight: container.clientHeight,
    }));
  }, [logicalEntries, scrollContainerRef, viewport.measurementRevision, windowModel]);

  useLayoutEffect(() => {
    const container = scrollContainerRef.current;
    const pendingResolution = resolvePendingResultFocus(
      pendingFocus,
      navigationModel,
      new Set(navigationButtonsRef.current.keys()),
      resetIdentity,
    );
    const focusKey =
      pendingResolution === "focus"
        ? pendingFocus?.key ?? null
        : document.activeElement === container || restoreRemovedFocusRef.current
          ? resolvedActiveKey
          : null;
    if (!focusKey) return;
    const button = navigationButtonsRef.current.get(focusKey);
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

  const handleEntryNavigation = useCallback(
    (
      event: KeyboardEvent<HTMLButtonElement>,
      navigationKey: string,
      logicalEntry: ConversationLogicalEntry,
    ) => {
      let targetKey: string | null = null;
      switch (event.key) {
        case "ArrowDown":
          targetKey = navigationModel.nextKey(navigationKey);
          break;
        case "ArrowUp":
          targetKey = navigationModel.previousKey(navigationKey);
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
            logicalEntry.key,
            direction,
            container?.clientHeight ?? 0,
          );
          targetKey = navigationModel.pageKey(navigationKey, targetIndex, direction);
          break;
        }
        case "ArrowRight":
          if (logicalEntry.kind === "conversation-header") {
            if (!logicalEntry.expanded) {
              event.preventDefault();
              requestFocusByKey(navigationKey);
              onToggleConversation(logicalEntry.conversation);
              return;
            }
            targetKey = navigationModel.firstChildKey(navigationKey);
          } else if (navigationModel.entry(navigationKey)?.parentKey) {
            event.preventDefault();
            return;
          } else {
            return;
          }
          break;
        case "ArrowLeft":
          if (logicalEntry.kind === "conversation-header") {
            event.preventDefault();
            if (logicalEntry.expanded) {
              requestFocusByKey(navigationKey);
              onToggleConversation(logicalEntry.conversation);
            }
            return;
          }
          targetKey = navigationModel.entry(navigationKey)?.parentKey ?? null;
          if (!targetKey) return;
          break;
        default:
          return;
      }
      event.preventDefault();
      if (targetKey) requestFocusByKey(targetKey);
    },
    [navigationModel, onToggleConversation, requestFocusByKey, scrollContainerRef, windowModel],
  );

  const preserveFocusBeforeAction = useCallback(
    (navigationKey: string, preferredKey: string | null = null) => {
      const fallbackKey =
        (preferredKey && navigationModel.has(preferredKey) ? preferredKey : null) ??
        navigationModel.previousKey(navigationKey) ??
        navigationModel.nextKey(navigationKey);
      if (fallbackKey) {
        requestFocusByKey(fallbackKey);
        return;
      }
      const container = scrollContainerRef.current;
      try {
        container?.focus({ preventScroll: true });
      } catch {
        container?.focus();
      }
    },
    [navigationModel, requestFocusByKey, scrollContainerRef],
  );

  const conversationAriaSetSize = exactCountStatus === "ready" ? exactTotalCount : -1;

  const renderEntry = (entry: ConversationLogicalEntry): ReactNode => {
    const shared = {
      "data-conversation-entry-key": entry.key,
      ref: entryRefForKey(entry.key),
    };

    if (entry.kind === "conversation-workspace-warning") {
      return (
        <div {...shared} className="conversation-window-item" key={entry.key} role="none">
          <div className="conversation-index-warning">
            <strong>Conversation data is not indexed for this workspace.</strong>
            {entry.issues.map((issue) => {
              const navigationKey = conversationWorkspaceActionNavigationKey(issue.workspaceId);
              return (
                <div key={issue.workspaceId}>
                  <span title={issue.workspacePath}>{issue.pstDisplayName}</span>
                  <button
                    data-result-navigation-key={navigationKey}
                    disabled={!issue.canReindex || entry.disabled}
                    onClick={() => {
                      preserveFocusBeforeAction(navigationKey);
                      void onReindexWorkspace(issue.workspaceId);
                    }}
                    onFocus={() => onActiveNavigationKeyChange(navigationKey)}
                    onKeyDown={(event) =>
                      handleEntryNavigation(event, navigationKey, entry)
                    }
                    ref={navigationButtonRefForKey(navigationKey)}
                    tabIndex={navigationKey === resolvedActiveKey ? 0 : -1}
                    type="button"
                  >
                    Reindex Existing EMLs
                  </button>
                </div>
              );
            })}
          </div>
        </div>
      );
    }

    if (entry.kind === "conversation-header") {
      const conversation = entry.conversation;
      const participantSummary = conversationParticipantSummary(
        conversation.participants,
        conversation.latestSender,
      );
      const participantTitle =
        conversation.participants.length > 0
          ? conversation.participants.join(", ")
          : conversation.latestSender || "(No sender)";
      return (
        // Flat tree levels keep every expanded child independently windowable.
        <div
          {...shared}
          aria-expanded={entry.expanded}
          aria-level={1}
          aria-posinset={entry.conversationPosition}
          aria-setsize={conversationAriaSetSize}
          className="conversation-window-item conversation-window-header"
          key={entry.key}
          role="treeitem"
        >
          <button
            aria-expanded={entry.expanded}
            className={`conversation-row ${entry.expanded ? "expanded" : ""}`}
            data-result-navigation-key={entry.key}
            onClick={() => {
              requestFocusByKey(entry.key);
              onToggleConversation(conversation);
            }}
            onFocus={() => onActiveNavigationKeyChange(entry.key)}
            onKeyDown={(event) => handleEntryNavigation(event, entry.key, entry)}
            ref={navigationButtonRefForKey(entry.key)}
            tabIndex={entry.key === resolvedActiveKey ? 0 : -1}
            type="button"
          >
            <span aria-hidden="true" className="conversation-toggle">
              {entry.expanded ? "v" : ">"}
            </span>
            <span className="conversation-main">
              <span className="conversation-subject-line">
                <span className="conversation-subject">
                  {conversation.subject || "(No subject)"}
                </span>
                {isCrossPstSearch ? (
                  <span className="message-workspace" title={conversation.workspacePath}>
                    {conversation.pstDisplayName}
                  </span>
                ) : null}
              </span>
              <span
                aria-label={`Participants: ${participantTitle}`}
                className="conversation-participants"
                title={participantTitle}
              >
                {participantSummary}
              </span>
              <span className="conversation-snippet">
                <HighlightedText
                  terms={highlightTerms}
                  text={cleanSnippet(conversation.snippet)}
                />
              </span>
            </span>
            <span className="conversation-meta">
              <span title={conversation.latestDate || undefined}>
                {formatDate(conversation.latestDate)}
              </span>
              <span>
                {formatCount(conversation.matchingMessageCount)} match
                {conversation.matchingMessageCount === 1 ? "" : "es"} / {formatCount(
                  conversation.totalMessageCount,
                )}
              </span>
              {conversation.hasAttachments ? <span>Attachments</span> : null}
            </span>
          </button>
        </div>
      );
    }

    if (entry.kind === "expanded-message") {
      const message = entry.message;
      const isSelectedRow =
        selectedMessageId === message.id && selectedWorkspaceId === entry.conversation.workspaceId;
      return (
        <div
          {...shared}
          aria-level={2}
          aria-posinset={entry.expandedPosition}
          aria-setsize={entry.expandedSetSize}
          className="conversation-window-item conversation-window-expanded-entry"
          key={entry.key}
          role="treeitem"
        >
          <button
            aria-current={isSelectedRow ? "true" : undefined}
            className={`conversation-message-row ${isSelectedRow ? "selected" : ""} ${
              message.matchesScope ? "" : "context-message"
            }`}
            data-result-navigation-key={entry.key}
            onClick={() => void onOpenMessage(message.id, entry.conversation.workspaceId)}
            onDoubleClick={() => void onOpenPreview(message, entry.conversation.workspaceId)}
            onFocus={() => onActiveNavigationKeyChange(entry.key)}
            onKeyDown={(event) => handleEntryNavigation(event, entry.key, entry)}
            ref={navigationButtonRefForKey(entry.key)}
            tabIndex={entry.key === resolvedActiveKey ? 0 : -1}
            type="button"
          >
            <span className="conversation-message-sender">
              {message.sender || "(No sender)"}
            </span>
            <span className="conversation-message-date" title={message.date}>
              {formatDate(message.date)}
            </span>
            <span className="conversation-message-folder">
              {message.folderPath || message.folderName}
              {!message.matchesScope ? " - context" : ""}
            </span>
            {message.attachmentCount > 0 ? (
              <span className="conversation-message-attachments">
                {formatCount(message.attachmentCount)} attachment
                {message.attachmentCount === 1 ? "" : "s"}
              </span>
            ) : null}
          </button>
        </div>
      );
    }

    if (entry.kind === "expanded-loading") {
      return (
        <div
          {...shared}
          className="conversation-window-item conversation-window-expanded-entry"
          key={entry.key}
          role="none"
        >
          <p className="list-count-note">Loading messages...</p>
        </div>
      );
    }

    if (entry.kind === "expanded-error") {
      return (
        <div
          {...shared}
          className="conversation-window-item conversation-window-expanded-entry"
          key={entry.key}
          role="none"
        >
          <p className="conversation-error">{entry.error}</p>
        </div>
      );
    }

    if (entry.kind === "expanded-actions") {
      const showEntireNavigationKey = expandedShowEntireNavigationKey(
        entry.parentConversationKey!,
      );
      const loadMoreNavigationKey = expandedLoadMoreNavigationKey(
        entry.parentConversationKey!,
      );
      return (
        <div
          {...shared}
          className="conversation-window-item conversation-window-expanded-entry"
          key={entry.key}
          role="none"
        >
          <div className="conversation-actions">
            {entry.showEntireAvailable ? (
              <button
                data-result-navigation-key={showEntireNavigationKey}
                disabled={entry.disabled}
                onClick={() => {
                  preserveFocusBeforeAction(
                    showEntireNavigationKey,
                    navigationModel.entry(showEntireNavigationKey)?.parentKey ?? null,
                  );
                  onShowEntireConversation(entry.conversation);
                }}
                onFocus={() => onActiveNavigationKeyChange(showEntireNavigationKey)}
                onKeyDown={(event) =>
                  handleEntryNavigation(event, showEntireNavigationKey, entry)
                }
                ref={navigationButtonRefForKey(showEntireNavigationKey)}
                tabIndex={showEntireNavigationKey === resolvedActiveKey ? 0 : -1}
                type="button"
              >
                Show Entire Conversation
              </button>
            ) : null}
            {entry.loadMoreAvailable ? (
              <button
                data-result-navigation-key={loadMoreNavigationKey}
                disabled={entry.disabled}
                onClick={() => {
                  preserveFocusBeforeAction(
                    loadMoreNavigationKey,
                    navigationModel.entry(loadMoreNavigationKey)?.parentKey ?? null,
                  );
                  onLoadMoreConversation(
                    entry.conversation,
                    entry.showingEntireConversation,
                  );
                }}
                onFocus={() => onActiveNavigationKeyChange(loadMoreNavigationKey)}
                onKeyDown={(event) =>
                  handleEntryNavigation(event, loadMoreNavigationKey, entry)
                }
                ref={navigationButtonRefForKey(loadMoreNavigationKey)}
                tabIndex={loadMoreNavigationKey === resolvedActiveKey ? 0 : -1}
                type="button"
              >
                Load More Messages
              </button>
            ) : null}
          </div>
        </div>
      );
    }

    return (
      <div {...shared} className="conversation-window-item" key={entry.key} role="none">
        <div className="load-more-row">
          <button
            data-result-navigation-key={conversationListLoadMoreNavigationKey}
            disabled={entry.disabled}
            onClick={() => {
              preserveFocusBeforeAction(conversationListLoadMoreNavigationKey);
              onLoadMoreConversations();
            }}
            onFocus={() =>
              onActiveNavigationKeyChange(conversationListLoadMoreNavigationKey)
            }
            onKeyDown={(event) =>
              handleEntryNavigation(event, conversationListLoadMoreNavigationKey, entry)
            }
            ref={navigationButtonRefForKey(conversationListLoadMoreNavigationKey)}
            tabIndex={conversationListLoadMoreNavigationKey === resolvedActiveKey ? 0 : -1}
            type="button"
          >
            {isLoadingMore
              ? "Loading..."
              : exactCountStatus === "ready"
                ? `Load More (${formatCount(
                    Math.max(0, exactTotalCount - conversations.length),
                  )} remaining)`
                : "Load More"}
          </button>
          <span>{resultSummaryText}</span>
        </div>
      </div>
    );
  };

  return (
    <>
      {windowResult.runs.flatMap((run): ReactNode[] => {
        if (run.kind === "spacer") {
          return [
            <div
              aria-hidden="true"
              className="conversation-window-spacer"
              key={`conversation-spacer:${run.startIndex}:${run.endIndex}`}
              role="presentation"
              style={{ height: `${run.height}px` }}
            />,
          ];
        }

        const rows: ReactNode[] = [];
        for (let index = run.startIndex; index < run.endIndex; index += 1) {
          rows.push(renderEntry(logicalEntries[index]));
        }
        return rows;
      })}

      {conversations.length === 0 ? (
        <div
          aria-live={emptyState.kind === "loading" ? "polite" : undefined}
          className="empty-state search-empty-state"
          role={emptyState.kind === "loading" ? "status" : undefined}
        >
          <strong>
            {conversationReindexRequired
              ? "Reindex existing EMLs to enable Conversation View."
              : emptyState.title}
          </strong>
          {!conversationReindexRequired && emptyState.detail ? (
            <span>{emptyState.detail}</span>
          ) : null}
        </div>
      ) : null}

      {!hasMore && conversations.length ? (
        <p className="list-count-note">{resultSummaryText}</p>
      ) : null}
    </>
  );
}
