// 长文本浮层的悬停宽限、触控固定与 DOM 事件清理；不持有业务内容。

import { focusClosestControl } from "../../../platform/a11y";
import { sleep } from "../../../platform/timer";
import type { Cell } from "../../../reactive/cell";
import type { TaskScope } from "../../../reactive/tasks";
import type { TooltipPosition } from ".";
import { hideTooltip } from "./tooltip";

/** 浮层显示条件与关闭竞态所需的信号。 */
export interface TooltipInteractionState {
  readonly position: Cell<TooltipPosition | null>;
  readonly focused: Cell<boolean>;
  readonly hovered: Cell<boolean>;
  readonly popupHovered: Cell<boolean>;
  readonly popupFocused: Cell<boolean>;
  readonly pinned: Cell<boolean>;
  readonly dismissed: Cell<boolean>;
  readonly closeRevision: Cell<number>;
  readonly portal: Cell<HTMLElement | null>;
}

/** 带有显式关闭与延迟关闭能力的浮层交互状态。 */
export interface TooltipInteraction extends TooltipInteractionState {
  close(): void;
  dismiss(previousParent?: Element | null): boolean;
  deferClose(): void;
  setAnchorTracking(stop: (() => void) | null): void;
}

/** 由既有信号创建浮层交互；关闭会清空固定与悬停状态并标记本次拒绝。 */
export function createTooltipInteraction(
  state: TooltipInteractionState,
  tasks: TaskScope,
  tooltipId: string,
): TooltipInteraction {
  let stopTracking: (() => void) | null = null;
  const setAnchorTracking = (stop: (() => void) | null): void => {
    stopTracking?.();
    stopTracking = stop;
  };
  const close = (): void => {
    setAnchorTracking(null);
    state.position.set(null);
    state.pinned.set(false);
    state.popupHovered.set(false);
    state.popupFocused.set(false);
    state.dismissed.set(true);
    hideTooltip(state.position, state.portal);
  };
  const dismiss = (previousParent: Element | null = null): boolean => {
    if (state.position.peek() === null) return false;
    // 只有浮层持有焦点才还焦；锚点移除后只尝试仍连接的原入口容器。
    const active = globalThis.document?.activeElement ?? null;
    const trigger =
      active !== null && document.getElementById(tooltipId)?.contains(active)
        ? (document.getElementById(`${tooltipId}-trigger`) ??
          (previousParent?.isConnected ? previousParent : null))
        : null;
    close();
    if (trigger !== null) focusClosestControl(trigger);
    return true;
  };
  const deferClose = (): void => {
    const revision = Math.min(state.closeRevision.peek() + 1, Number.MAX_SAFE_INTEGER);
    state.closeRevision.set(revision);
    tasks.spawn(async (context) => {
      if (!(await sleep(180, context.signal)) || !context.alive()) {
        return;
      }
      if (
        state.closeRevision.peek() === revision &&
        !state.focused.peek() &&
        !state.hovered.peek() &&
        !state.popupHovered.peek() &&
        !state.popupFocused.peek() &&
        !state.pinned.peek()
      ) {
        close();
      }
    });
  };
  return { ...state, close, dismiss, deferClose, setAnchorTracking };
}

const POPUP_EVENTS: readonly string[] = ["mouseenter", "mouseleave", "focusin", "focusout"];

/** 浮层事件监听的生命周期守卫。 */
export interface PopupListeners {
  dispose(): void;
}

/**
 * 同时支持 Solid 内联浮层和挂到 body 的 portal；关闭时成对清理监听器。
 * 延迟关闭经由组件创建的任务作用域，不能在原生 DOM 回调中直接创建任务。
 */
export function attachPopupListeners(
  element: Element,
  triggerId: string,
  state: TooltipInteraction,
  deferClose: () => void,
): PopupListeners | null {
  if (typeof document === "undefined") {
    return null;
  }
  const documentNode = document;
  const callback = (event: Event): void => {
    if (event.type === "mouseenter") {
      state.popupHovered.set(true);
    } else if (event.type === "mouseleave") {
      state.popupHovered.set(false);
      deferClose();
    } else if (event.type === "focusin") {
      state.popupFocused.set(true);
    } else if (event.type === "focusout") {
      state.popupFocused.set(false);
      deferClose();
    }
  };
  const outside = (event: Event): void => {
    const target = event.target instanceof Node ? event.target : null;
    if (target === null) {
      return;
    }
    const trigger = documentNode.getElementById(triggerId);
    const insideTrigger = trigger?.contains(target) ?? false;
    if (!insideTrigger && !element.contains(target)) {
      state.close();
    }
  };
  const dispose = (): void => {
    for (const event of POPUP_EVENTS) {
      element.removeEventListener(event, callback);
    }
    documentNode.removeEventListener("pointerdown", outside);
  };
  try {
    for (const event of POPUP_EVENTS) {
      element.addEventListener(event, callback);
    }
    documentNode.addEventListener("pointerdown", outside);
  } catch {
    dispose();
    return null;
  }
  return { dispose };
}
