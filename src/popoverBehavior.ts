export type PopoverId = string;

export type PopoverDismissReason =
  | "outside_pointer"
  | "escape"
  | "window_blur"
  | "window_resize";

export type PopoverEventTarget = Pick<Document, "addEventListener" | "removeEventListener">;
export type PopoverWindowTarget = Pick<
  Window,
  "addEventListener" | "removeEventListener" | "requestAnimationFrame"
>;

export type DismissiblePopoverOptions = {
  documentTarget: PopoverEventTarget;
  windowTarget: PopoverWindowTarget;
  trigger: HTMLElement | null;
  panel: HTMLElement | null;
  onDismiss: (reason: PopoverDismissReason) => void;
};

export type DismissiblePopoverController = {
  attach: () => () => void;
  detach: () => void;
  isAttached: () => boolean;
};

/**
 * Returns the sole open popover after activating `requestedId`.
 * Activating the already-open popover toggles it closed; activating another
 * popover replaces the previous one without an intermediate multi-open state.
 */
export function toggleOpenPopover(
  currentOpenId: PopoverId | null,
  requestedId: PopoverId,
): PopoverId | null {
  return currentOpenId === requestedId ? null : requestedId;
}

/** Closes only the requested popover, leaving a different open popover alone. */
export function closeOpenPopover(
  currentOpenId: PopoverId | null,
  requestedId: PopoverId,
): PopoverId | null {
  return currentOpenId === requestedId ? null : currentOpenId;
}

function targetIsInside(container: HTMLElement | null, target: EventTarget | null): boolean {
  if (!container || !target) return false;
  if (container === target) return true;
  try {
    return container.contains(target as Node);
  } catch {
    return false;
  }
}

function restoreConnectedTriggerFocus(
  windowTarget: PopoverWindowTarget,
  trigger: HTMLElement | null,
): void {
  windowTarget.requestAnimationFrame(() => {
    if (!trigger || trigger.isConnected === false || typeof trigger.focus !== "function") return;
    try {
      trigger.focus({ preventScroll: true });
    } catch {
      trigger.focus();
    }
  });
}

/**
 * Creates the listener lifecycle for one open custom popover.
 *
 * The controller deliberately observes only pointerdown, keydown, blur, and
 * resize. It never prevents an outside pointer event, never moves focus for an
 * outside/window dismissal, and does not listen for drag or selection events.
 */
export function createDismissiblePopoverController(
  options: DismissiblePopoverOptions,
): DismissiblePopoverController {
  let attached = false;
  let dismissed = false;
  let capturedTrigger: HTMLElement | null = options.trigger;

  const removeListeners = () => {
    if (!attached) return;
    attached = false;
    options.documentTarget.removeEventListener("pointerdown", onPointerDown, true);
    options.documentTarget.removeEventListener("keydown", onKeyDown);
    options.windowTarget.removeEventListener("blur", onWindowBlur);
    options.windowTarget.removeEventListener("resize", onWindowResize);
  };

  const dismiss = (reason: PopoverDismissReason, restoreFocus: boolean) => {
    if (!attached || dismissed) return;
    dismissed = true;
    removeListeners();
    options.onDismiss(reason);
    if (restoreFocus) {
      restoreConnectedTriggerFocus(options.windowTarget, capturedTrigger);
    }
  };

  function onPointerDown(event: Event) {
    if (
      targetIsInside(options.trigger, event.target) ||
      targetIsInside(options.panel, event.target)
    ) {
      return;
    }
    dismiss("outside_pointer", false);
  }

  function onKeyDown(event: Event) {
    const keyboardEvent = event as KeyboardEvent;
    if (keyboardEvent.key !== "Escape") return;
    keyboardEvent.preventDefault();
    keyboardEvent.stopPropagation();
    dismiss("escape", true);
  }

  function onWindowBlur() {
    dismiss("window_blur", false);
  }

  function onWindowResize() {
    dismiss("window_resize", false);
  }

  const detach = () => {
    dismissed = true;
    removeListeners();
  };

  const attach = () => {
    if (attached) return detach;
    dismissed = false;
    capturedTrigger = options.trigger;
    attached = true;
    options.documentTarget.addEventListener("pointerdown", onPointerDown, true);
    options.documentTarget.addEventListener("keydown", onKeyDown);
    options.windowTarget.addEventListener("blur", onWindowBlur);
    options.windowTarget.addEventListener("resize", onWindowResize);
    return detach;
  };

  return {
    attach,
    detach,
    isAttached: () => attached,
  };
}
