// 文档级 Escape 与最前面的模态范围，供浮层保留正确的关闭顺序。

import { type Accessor, createEffect } from "solid-js";
import { createValueMemo } from "../../reactive/memo";

const ARIA_MODAL = '[role="dialog"][aria-modal="true"]';
const NATIVE_MODAL = "dialog:modal";
const OPEN_DIALOG = "dialog[open]";

function withNativeModalSelector<T>(find: (selector: string) => T): T {
  try {
    return find(NATIVE_MODAL);
  } catch {
    // 旧浏览器无法识别 :modal 时，保守地将打开的原生 dialog 视为模态。
    return find(OPEN_DIALOG);
  }
}

/** 找到触发控件所属的模态，使浮层留在相同的焦点范围。 */
export function containingModal(element: Element): Element | null {
  return withNativeModalSelector((selector) => element.closest(`${selector},${ARIA_MODAL}`));
}

/** 原生模态限定顶层范围；其中的应用对话框仍可占用最前面一层。 */
export function topModal(document: Document): Element | null {
  const native = withNativeModalSelector((selector) => {
    const dialogs = document.querySelectorAll(selector);
    // 多个原生 dialog 的打开顺序可能与 DOM 顺序不同，优先取持有焦点的一层。
    return dialogs.length === 0
      ? null
      : (document.activeElement?.closest(selector) ?? dialogs.item(dialogs.length - 1));
  });
  const dialogs = (native ?? document).querySelectorAll(ARIA_MODAL);
  return dialogs.length === 0 ? native : dialogs.item(dialogs.length - 1);
}

/** 只消费由当前浮层接手的 Escape，保留输入法与前景控件已处理的按键。 */
export function listenDocumentEscape(
  document: Document,
  handle: (event: KeyboardEvent) => boolean,
  capture = false,
): () => void {
  const listener = (event: KeyboardEvent) => {
    if (event.key === "Escape" && !event.isComposing && !event.defaultPrevented && handle(event)) {
      event.preventDefault();
      event.stopPropagation();
    }
  };
  document.addEventListener("keydown", listener, capture);
  return () => document.removeEventListener("keydown", listener, capture);
}

interface PagePanel {
  readonly triggerId: string;
  readonly panelId: string;
  readonly dismiss: () => void;
}

interface OpenPagePanels {
  readonly panels: Set<PagePanel>;
  readonly unlisten: () => void;
}

const openPanelsByDocument = new WeakMap<Document, OpenPagePanels>();

function dismissPagePanel(document: Document, panels: ReadonlySet<PagePanel>): boolean {
  if (topModal(document) !== null) return false;
  const focus = document.activeElement;
  let target: PagePanel | null = null;
  for (const panel of panels) {
    target = panel;
    if (
      focus !== null &&
      (document.getElementById(panel.triggerId)?.contains(focus) ||
        document.getElementById(panel.panelId)?.contains(focus))
    ) {
      break;
    }
  }
  if (target === null) return false;
  target.dismiss();
  return true;
}

/** 展开的页面面板共用一条 Escape 监听，优先关闭持有焦点的一层，再按打开顺序选择。 */
export function usePagePanelEscape(open: Accessor<boolean>, panel: PagePanel): void {
  const opened = createValueMemo(open);
  createEffect(opened, (visible) => {
    if (!visible || typeof document === "undefined") return;
    const owner = document;
    let registered = openPanelsByDocument.get(owner);
    if (registered === undefined) {
      const panels = new Set<PagePanel>();
      registered = {
        panels,
        unlisten: listenDocumentEscape(owner, () => dismissPagePanel(owner, panels)),
      };
      openPanelsByDocument.set(owner, registered);
    }
    const { panels, unlisten } = registered;
    panels.add(panel);
    return () => {
      if (panels.delete(panel) && panels.size === 0) {
        unlisten();
        openPanelsByDocument.delete(owner);
      }
    };
  });
}
