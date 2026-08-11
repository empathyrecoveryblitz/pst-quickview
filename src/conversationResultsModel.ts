import type {
  ConversationMessageItem,
  ConversationSummary,
  ConversationWorkspaceIssue,
} from "./types";
import type { ResultNavigationEntry } from "./resultNavigation";

export type ExpandedConversationState = {
  items: ConversationMessageItem[];
  matchingMessageCount: number;
  totalMessageCount: number;
  showingEntireConversation: boolean;
  loading: boolean;
  error: string | null;
};

export const CONVERSATION_ENTRY_HEIGHT_ESTIMATES = {
  // Three compact text rows, right-side metadata, and the existing 9px vertical padding.
  conversationHeader: 78,
  // Two compact metadata rows plus the existing indented 7px vertical padding.
  expandedMessage: 52,
  // Existing count/error copy occupies one compact line with surrounding spacing.
  expandedStatus: 40,
  // One shared row retains both existing expanded-conversation action buttons.
  expandedActions: 48,
  // Button, summary text, gap, and the existing 12px vertical padding.
  conversationListLoadMore: 74,
  // Base warning copy plus one compact action row; additional issues add one row each.
  workspaceWarning: 76,
} as const;

type ConversationEntryBase = {
  key: string;
  logicalPosition: number;
  parentConversationKey: string | null;
  estimatedHeight: number;
  focusable: boolean;
};

export type ConversationHeaderEntry = ConversationEntryBase & {
  kind: "conversation-header";
  conversation: ConversationSummary;
  conversationPosition: number;
  expanded: boolean;
};

export type ExpandedMessageEntry = ConversationEntryBase & {
  kind: "expanded-message";
  conversation: ConversationSummary;
  message: ConversationMessageItem;
  expandedPosition: number;
  expandedSetSize: number;
};

export type ExpandedLoadingEntry = ConversationEntryBase & {
  kind: "expanded-loading";
  conversation: ConversationSummary;
};

export type ExpandedErrorEntry = ConversationEntryBase & {
  kind: "expanded-error";
  conversation: ConversationSummary;
  error: string;
};

export type ExpandedActionsEntry = ConversationEntryBase & {
  kind: "expanded-actions";
  conversation: ConversationSummary;
  showingEntireConversation: boolean;
  showEntireAvailable: boolean;
  loadMoreAvailable: boolean;
  disabled: boolean;
};

export type ConversationListLoadMoreEntry = ConversationEntryBase & {
  kind: "conversation-list-load-more";
  disabled: boolean;
};

export type ConversationWorkspaceWarningEntry = ConversationEntryBase & {
  kind: "conversation-workspace-warning";
  issues: readonly ConversationWorkspaceIssue[];
  disabled: boolean;
};

export type ConversationLogicalEntry =
  | ConversationHeaderEntry
  | ExpandedMessageEntry
  | ExpandedLoadingEntry
  | ExpandedErrorEntry
  | ExpandedActionsEntry
  | ConversationListLoadMoreEntry
  | ConversationWorkspaceWarningEntry;

type WithoutLogicalPosition<Entry> = Entry extends ConversationLogicalEntry
  ? Omit<Entry, "logicalPosition">
  : never;
type UnpositionedConversationLogicalEntry = WithoutLogicalPosition<ConversationLogicalEntry>;

export type ConversationResultsModelInput = {
  conversations: readonly ConversationSummary[];
  expandedConversations: Readonly<Record<string, ExpandedConversationState>>;
  workspaceIssues?: readonly ConversationWorkspaceIssue[];
  workspaceActionsDisabled?: boolean;
  hasMoreConversations?: boolean;
  topLevelLoadMoreDisabled?: boolean;
};

export function conversationStateKey(workspaceId: string, conversationId: string): string {
  return `${workspaceId}:${conversationId}`;
}

export function conversationHeaderEntryKey(
  workspaceId: string,
  conversationId: string,
): string {
  return `conversation-header:${conversationStateKey(workspaceId, conversationId)}`;
}

export function expandedMessageEntryKey(workspaceId: string, messageId: number): string {
  return `expanded-message:${workspaceId}:${messageId}`;
}

export function expandedLoadingEntryKey(parentConversationKey: string): string {
  return `expanded-loading:${parentConversationKey}`;
}

export function expandedErrorEntryKey(parentConversationKey: string): string {
  return `expanded-error:${parentConversationKey}`;
}

export function expandedActionsEntryKey(parentConversationKey: string): string {
  return `expanded-actions:${parentConversationKey}`;
}

export function conversationWorkspaceActionNavigationKey(workspaceId: string): string {
  return `conversation-workspace-action:${workspaceId}`;
}

export function expandedShowEntireNavigationKey(parentConversationKey: string): string {
  return `expanded-show-entire:${parentConversationKey}`;
}

export function expandedLoadMoreNavigationKey(parentConversationKey: string): string {
  return `expanded-load-more:${parentConversationKey}`;
}

export const conversationListLoadMoreNavigationKey = "conversation-list-load-more-action";

export function flattenConversationResults({
  conversations,
  expandedConversations,
  workspaceIssues = [],
  workspaceActionsDisabled = false,
  hasMoreConversations = false,
  topLevelLoadMoreDisabled = false,
}: ConversationResultsModelInput): ConversationLogicalEntry[] {
  const entries: ConversationLogicalEntry[] = [];
  const keys = new Set<string>();
  const push = (entry: UnpositionedConversationLogicalEntry) => {
    if (keys.has(entry.key)) return;
    keys.add(entry.key);
    entries.push({ ...entry, logicalPosition: entries.length + 1 } as ConversationLogicalEntry);
  };

  if (workspaceIssues.length > 0) {
    push({
      kind: "conversation-workspace-warning",
      key: "conversation-workspace-warning",
      parentConversationKey: null,
      estimatedHeight:
        CONVERSATION_ENTRY_HEIGHT_ESTIMATES.workspaceWarning +
        Math.max(0, workspaceIssues.length - 1) * 34,
      focusable: !workspaceActionsDisabled && workspaceIssues.some((issue) => issue.canReindex),
      issues: workspaceIssues,
      disabled: workspaceActionsDisabled,
    });
  }

  conversations.forEach((conversation, conversationIndex) => {
    const parentConversationKey = conversationStateKey(
      conversation.workspaceId,
      conversation.conversationId,
    );
    const expanded = expandedConversations[parentConversationKey];
    push({
      kind: "conversation-header",
      key: conversationHeaderEntryKey(conversation.workspaceId, conversation.conversationId),
      parentConversationKey,
      estimatedHeight: CONVERSATION_ENTRY_HEIGHT_ESTIMATES.conversationHeader,
      focusable: true,
      conversation,
      conversationPosition: conversationIndex + 1,
      expanded: Boolean(expanded),
    });

    if (!expanded) return;
    const expandedSetSize = Math.max(
      expanded.items.length,
      expanded.showingEntireConversation
        ? expanded.totalMessageCount
        : expanded.matchingMessageCount,
    );
    expanded.items.forEach((message, expandedIndex) => {
      push({
        kind: "expanded-message",
        key: expandedMessageEntryKey(conversation.workspaceId, message.id),
        parentConversationKey,
        estimatedHeight: CONVERSATION_ENTRY_HEIGHT_ESTIMATES.expandedMessage,
        focusable: true,
        conversation,
        message,
        expandedPosition: expandedIndex + 1,
        expandedSetSize,
      });
    });

    if (expanded.loading) {
      push({
        kind: "expanded-loading",
        key: expandedLoadingEntryKey(parentConversationKey),
        parentConversationKey,
        estimatedHeight: CONVERSATION_ENTRY_HEIGHT_ESTIMATES.expandedStatus,
        focusable: false,
        conversation,
      });
    }
    if (expanded.error) {
      push({
        kind: "expanded-error",
        key: expandedErrorEntryKey(parentConversationKey),
        parentConversationKey,
        estimatedHeight: CONVERSATION_ENTRY_HEIGHT_ESTIMATES.expandedStatus,
        focusable: false,
        conversation,
        error: expanded.error,
      });
    }

    const showEntireAvailable =
      !expanded.showingEntireConversation &&
      expanded.totalMessageCount > expanded.matchingMessageCount;
    const loadMoreAvailable = expanded.items.length < expandedSetSize;
    if (showEntireAvailable || loadMoreAvailable) {
      push({
        kind: "expanded-actions",
        key: expandedActionsEntryKey(parentConversationKey),
        parentConversationKey,
        estimatedHeight: CONVERSATION_ENTRY_HEIGHT_ESTIMATES.expandedActions,
        focusable: !expanded.loading,
        conversation,
        showingEntireConversation: expanded.showingEntireConversation,
        showEntireAvailable,
        loadMoreAvailable,
        disabled: expanded.loading,
      });
    }
  });

  if (hasMoreConversations) {
    push({
      kind: "conversation-list-load-more",
      key: "conversation-list-load-more",
      parentConversationKey: null,
      estimatedHeight: CONVERSATION_ENTRY_HEIGHT_ESTIMATES.conversationListLoadMore,
      focusable: !topLevelLoadMoreDisabled,
      disabled: topLevelLoadMoreDisabled,
    });
  }

  return entries;
}

export function buildConversationNavigationEntries(
  entries: readonly ConversationLogicalEntry[],
): ResultNavigationEntry[] {
  const headerKeyByParent = new Map<string, string>();
  for (const entry of entries) {
    if (entry.kind === "conversation-header" && entry.parentConversationKey) {
      headerKeyByParent.set(entry.parentConversationKey, entry.key);
    }
  }

  const navigationEntries: ResultNavigationEntry[] = [];
  const push = (
    key: string,
    entry: ConversationLogicalEntry,
    kind: string,
    parentKey: string | null = null,
  ) => {
    navigationEntries.push({
      key,
      logicalKey: entry.key,
      logicalIndex: entry.logicalPosition - 1,
      parentKey,
      kind,
    });
  };

  for (const entry of entries) {
    const parentKey = entry.parentConversationKey
      ? headerKeyByParent.get(entry.parentConversationKey) ?? null
      : null;
    switch (entry.kind) {
      case "conversation-workspace-warning":
        if (entry.disabled) break;
        for (const issue of entry.issues) {
          if (issue.canReindex) {
            push(
              conversationWorkspaceActionNavigationKey(issue.workspaceId),
              entry,
              "workspace-action",
            );
          }
        }
        break;
      case "conversation-header":
        push(entry.key, entry, "conversation-header");
        break;
      case "expanded-message":
        push(entry.key, entry, "expanded-message", parentKey);
        break;
      case "expanded-actions":
        if (entry.disabled) break;
        if (entry.showEntireAvailable) {
          push(
            expandedShowEntireNavigationKey(entry.parentConversationKey!),
            entry,
            "expanded-show-entire",
            parentKey,
          );
        }
        if (entry.loadMoreAvailable) {
          push(
            expandedLoadMoreNavigationKey(entry.parentConversationKey!),
            entry,
            "expanded-load-more",
            parentKey,
          );
        }
        break;
      case "conversation-list-load-more":
        if (!entry.disabled) {
          push(
            conversationListLoadMoreNavigationKey,
            entry,
            "conversation-list-load-more",
          );
        }
        break;
      case "expanded-loading":
      case "expanded-error":
        break;
    }
  }
  return navigationEntries;
}
