// 浏览器端通用无障碍交互。
// 不含 surface 业务语义，只处理所有 `aria-modal` 弹窗都需要的键盘焦点、关闭后恢复和
// 背景滚动边界。监听在 surface 根壳层挂载一次，始终把最内层弹窗视为当前焦点范围。
// 另提供按元素 ID 移动焦点的原语，供菜单收起、校验失败等场景把焦点交给明确的控件。

import { type Accessor, createEffect, onSettled } from "solid-js";

import { createValueMemo } from "../../reactive/memo";
import { createTaskScope } from "../../reactive/tasks";
import { sleep } from "../timer";

const FOCUSABLE_SELECTOR =
  'a[href],button:not([disabled]),input:not([disabled]),select:not([disabled]),textarea:not([disabled]),[tabindex]:not([tabindex="-1"]),[contenteditable="true"]';

/** SPA 路由真正变化后把键盘焦点移入主内容；首次挂载与同路由数据刷新不抢占用户焦点。 */
export function useRouteFocus(routeToken: Accessor<string>, targetId: string): void {
  let mounted = false;
  const route = createValueMemo(routeToken);
  createEffect(route, () => {
    // 首次挂载保留浏览器自然的文档起始焦点。
    if (!mounted) {
      mounted = true;
      return;
    }
    focusElementById(targetId);
  });
}

/** 把键盘焦点移到指定 ID 的元素；元素不存在或不是 HTML 元素时保持当前焦点。 */
export function focusElementById(elementId: string): void {
  if (typeof document === "undefined") {
    return;
  }
  const element = document.getElementById(elementId);
  if (element instanceof HTMLElement) {
    focusElement(element);
  }
}

/**
 * 在组件体里取得延迟聚焦入口；返回的函数可在事件回调中调用。
 * 让出一个宏任务后再按 ID 聚焦，确保事件回调里新渲染的节点已经写入 DOM；
 * 任务随创建组件卸载而取消，事件的临时作用域无法继承该寿命。
 */
export function useFocusElementByIdAfterRender(): (elementId: string) => void {
  const tasks = createTaskScope();
  return (elementId) => {
    tasks.spawn(async (context) => {
      if (await sleep(0, context.signal)) {
        if (context.alive()) {
          focusElementById(elementId);
        }
      }
    });
  };
}

/** 聚焦失败只影响焦点位置，不改变任何状态，无需上报。 */
function focusElement(element: HTMLElement): void {
  try {
    element.focus();
  } catch {
    // 不可聚焦节点不是错误；保持当前焦点。
  }
}

/** 聚焦触发文本本身或承载它的可聚焦链接、按钮。 */
export function focusClosestControl(trigger: Element): void {
  const control = trigger.closest(FOCUSABLE_SELECTOR);
  if (control instanceof HTMLElement && control.isConnected) {
    focusElement(control);
  }
}

/** 匹配失败按不匹配处理，避免选择器兼容差异中断焦点循环。 */
function matches(element: HTMLElement, selector: string): boolean {
  try {
    return element.matches(selector);
  } catch {
    return false;
  }
}

/** 最内层、当前可见的模态对话框。 */
function topDialog(document: Document): Element | null {
  try {
    const dialogs = document.querySelectorAll('[role="dialog"][aria-modal="true"]');
    if (dialogs.length === 0) {
      return null;
    }
    return dialogs.item(dialogs.length - 1);
  } catch {
    return null;
  }
}

/** 当前焦点元素并确认它仍可聚焦。 */
function currentFocusable(document: Document): HTMLElement | null {
  const element = document.activeElement;
  if (element instanceof HTMLElement && isFocusable(element)) {
    return element;
  }
  return null;
}

/** 手机折叠菜单、关闭的 details 与隐藏祖先不能成为焦点循环的首尾控件。 */
function hasHiddenAncestor(element: HTMLElement): boolean {
  try {
    return element.closest('[hidden], [inert], [aria-hidden="true"]') !== null;
  } catch {
    return false;
  }
}

function isFocusable(element: HTMLElement): boolean {
  if (!matches(element, FOCUSABLE_SELECTOR) || matches(element, ":disabled")) {
    return false;
  }
  if (!element.isConnected || hasHiddenAncestor(element) || element.getClientRects().length === 0) {
    return false;
  }
  if (typeof window === "undefined") {
    return true;
  }
  try {
    const visibility = window.getComputedStyle(element).getPropertyValue("visibility");
    return visibility !== "hidden" && visibility !== "collapse";
  } catch {
    return true;
  }
}

/** 对话框内优先 autofocus，再取首个可聚焦项；没有子控件时允许面板自身接收焦点。 */
function preferredDialogFocus(dialog: Element): HTMLElement | null {
  try {
    const autofocus = dialog.querySelector("[autofocus]");
    if (autofocus instanceof HTMLElement && isFocusable(autofocus)) {
      return autofocus;
    }
  } catch {
    // 选择器失败按无 autofocus 处理，继续寻找首个可聚焦控件。
  }
  let controls: NodeListOf<Element>;
  try {
    controls = dialog.querySelectorAll(FOCUSABLE_SELECTOR);
  } catch {
    return null;
  }
  for (let index = 0; index < controls.length; index += 1) {
    const control = controls.item(index);
    if (control instanceof HTMLElement && isFocusable(control)) {
      return control;
    }
  }
  return dialog instanceof HTMLElement ? dialog : null;
}

interface DialogFocusTrapState {
  activeDialog: Element | null;
  opener: HTMLElement | null;
  previousOverflow: string | null;
}

/** 同步最内层对话框、背景滚动与关闭后的触发控件恢复。 */
function synchronizeDialogState(
  document: Document,
  state: DialogFocusTrapState,
  lockBackgroundScroll: boolean,
  fallbackFocusId?: string,
): void {
  const nextDialog = topDialog(document);
  const unchanged =
    (state.activeDialog === null && nextDialog === null) ||
    (state.activeDialog !== null &&
      nextDialog !== null &&
      state.activeDialog.isSameNode(nextDialog));
  if (unchanged) {
    if (nextDialog !== null) {
      const active = document.activeElement;
      const focusStillValid =
        active !== null &&
        nextDialog.contains(active) &&
        (active.isSameNode(nextDialog) || (active instanceof HTMLElement && isFocusable(active)));
      if (!focusStillValid) {
        const target = preferredDialogFocus(nextDialog);
        if (target !== null) {
          focusElement(target);
        }
      }
    }
    return;
  }

  // 不能只记录“是否打开”：创建成功后可能在同一轮 DOM 更新中替换顶层弹窗；
  // 此时继续保留最初 opener，但必须把焦点移入新的最顶层 dialog。
  const hadDialog = state.activeDialog !== null;
  state.activeDialog = nextDialog;
  const body = document.body;
  if (body === null) {
    return;
  }
  if (nextDialog !== null) {
    const active = currentFocusable(document);
    const activeInside = active !== null && nextDialog.contains(active);
    if (!activeInside) {
      // opener 已由 focusin 或捕获阶段 click 记录；这里不能用当前焦点覆盖它。
      const target = preferredDialogFocus(nextDialog);
      if (target !== null) {
        focusElement(target);
      }
    }
    if (!hadDialog && lockBackgroundScroll) {
      try {
        state.previousOverflow = body.style.getPropertyValue("overflow");
      } catch {
        state.previousOverflow = "";
      }
      try {
        body.style.setProperty("overflow", "hidden");
      } catch {
        // 滚动锁定失败不改变焦点与 opener 处理。
      }
    }
    return;
  }

  if (state.previousOverflow !== null) {
    const previous = state.previousOverflow;
    state.previousOverflow = null;
    try {
      if (previous.length === 0) {
        body.style.removeProperty("overflow");
      } else {
        body.style.setProperty("overflow", previous);
      }
    } catch {
      // 恢复失败只影响背景滚动，不阻塞关闭后的焦点恢复。
    }
  }
  // 先快照 opener，再聚焦；focusin 监听器可以安全写回同一字段。
  const opener = state.opener;
  if (opener !== null && isFocusable(opener)) {
    focusElement(opener);
  } else if (fallbackFocusId !== undefined) {
    // 目录行可能在弹窗打开时被刷新移除，焦点仍须回到有效的页面入口。
    focusElementById(fallbackFocusId);
  }
}

/** 管理当前 surface 的模态焦点循环、触发控件恢复和可选的背景滚动锁定。 */
export function useDialogFocusTrap(lockBackgroundScroll: boolean, fallbackFocusId?: string): void {
  onSettled(() => {
    if (typeof document === "undefined") {
      return;
    }
    const documentElement = document;
    const state: DialogFocusTrapState = {
      activeDialog: null,
      opener: currentFocusable(documentElement),
      previousOverflow: null,
    };

    const onFocusIn = (event: FocusEvent) => {
      const dialog = topDialog(documentElement);
      if (dialog !== null) {
        const target = event.target;
        const inside = target instanceof Element && dialog.contains(target);
        if (!inside) {
          const preferred = preferredDialogFocus(dialog);
          if (preferred !== null) {
            focusElement(preferred);
          }
        }
        return;
      }
      const target = event.target;
      if (target instanceof HTMLElement && isFocusable(target)) {
        state.opener = target;
      }
    };

    const onClick = (event: MouseEvent) => {
      if (topDialog(documentElement) !== null) {
        return;
      }
      const target = event.target;
      if (!(target instanceof Element)) {
        return;
      }
      let control: Element | null;
      try {
        control = target.closest(FOCUSABLE_SELECTOR);
      } catch {
        return;
      }
      if (control instanceof HTMLElement && isFocusable(control)) {
        // 捕获阶段先于打开 dialog，程序化 click 也能保存真实触发控件；
        // dialog 内关闭按钮不会覆盖该 opener。
        state.opener = control;
      }
    };

    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Tab" || event.defaultPrevented) {
        return;
      }
      const dialog = topDialog(documentElement);
      if (dialog === null) {
        return;
      }
      let controls: NodeListOf<Element>;
      try {
        controls = dialog.querySelectorAll(FOCUSABLE_SELECTOR);
      } catch {
        return;
      }
      let first: HTMLElement | null = null;
      let last: HTMLElement | null = null;
      for (let index = 0; index < controls.length; index += 1) {
        const control = controls.item(index);
        if (!(control instanceof HTMLElement) || !isFocusable(control)) {
          continue;
        }
        if (first === null) {
          first = control;
        }
        last = control;
      }
      if (first === null || last === null) {
        // busy 弹窗可能没有任何可 Tab 子控件，焦点必须留在面板而非背景。
        event.preventDefault();
        if (dialog instanceof HTMLElement) {
          focusElement(dialog);
        }
        return;
      }
      const active = documentElement.activeElement;
      const inside = active !== null && dialog.contains(active);
      const atFirst = active?.isSameNode(first) === true;
      const atLast = active?.isSameNode(last) === true;
      // busy 结束后焦点可能仍在 tabindex=-1 的面板上，反向 Tab 应从末项进入。
      const atPanel = active?.isSameNode(dialog) === true;
      if (inside && !atPanel && !((event.shiftKey && atFirst) || (!event.shiftKey && atLast))) {
        return;
      }
      event.preventDefault();
      focusElement(event.shiftKey ? last : first);
    };

    documentElement.addEventListener("focusin", onFocusIn);
    documentElement.addEventListener("click", onClick, true);
    documentElement.addEventListener("keydown", onKeyDown);
    synchronizeDialogState(documentElement, state, lockBackgroundScroll, fallbackFocusId);

    let observer: MutationObserver | null = null;
    try {
      observer = new MutationObserver(() => {
        synchronizeDialogState(documentElement, state, lockBackgroundScroll, fallbackFocusId);
      });
    } catch {
      observer = null;
    }
    const root = documentElement.documentElement;
    if (observer !== null && root !== null) {
      try {
        observer.observe(root, {
          childList: true,
          subtree: true,
          attributes: true,
          attributeFilter: ["disabled", "hidden", "inert"],
        });
      } catch {
        // 观察失败时保留事件监听与现有焦点；下一次 DOM 事件仍可重新收敛。
      }
    }

    let cleaned = false;
    return () => {
      if (cleaned) {
        return;
      }
      cleaned = true;
      observer?.disconnect();
      documentElement.removeEventListener("focusin", onFocusIn);
      documentElement.removeEventListener("click", onClick, true);
      documentElement.removeEventListener("keydown", onKeyDown);
      const body = documentElement.body;
      if (body !== null && state.previousOverflow !== null) {
        const previous = state.previousOverflow;
        state.previousOverflow = null;
        try {
          if (previous.length === 0) {
            body.style.removeProperty("overflow");
          } else {
            body.style.setProperty("overflow", previous);
          }
        } catch {
          // 卸载清理不能因样式恢复失败而跳过焦点恢复。
        }
      }
      const opener = state.opener;
      if (opener !== null && isFocusable(opener)) {
        focusElement(opener);
      } else if (fallbackFocusId !== undefined) {
        focusElementById(fallbackFocusId);
      }
    };
  });
}
