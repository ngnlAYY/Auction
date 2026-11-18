// 每份文档的可见截断提示共用一条 Escape 监听。

import { focusClosestControl } from "../../../platform/a11y";
import { listenDocumentEscape, topModal } from "../../../platform/a11y/escape";

interface OpenTooltips {
  readonly dismissals: Map<string, () => void>;
  readonly unlisten: () => void;
}

const openByDocument = new WeakMap<Document, OpenTooltips>();

/** 在捕获阶段优先关闭同一前景范围最近打开的提示。 */
export function registerTooltipEscape(
  document: Document,
  tooltipId: string,
  dismiss: () => void,
): () => void {
  let open = openByDocument.get(document);
  if (open === undefined) {
    const dismissals = new Map<string, () => void>();
    open = {
      dismissals,
      unlisten: listenDocumentEscape(
        document,
        (event) => dismissTopTooltip(document, event, dismissals),
        true,
      ),
    };
    openByDocument.set(document, open);
  }
  const { dismissals, unlisten } = open;
  dismissals.set(tooltipId, dismiss);
  return () => {
    if (dismissals.delete(tooltipId) && dismissals.size === 0) {
      unlisten();
      openByDocument.delete(document);
    }
  };
}

function dismissTopTooltip(
  document: Document,
  event: KeyboardEvent,
  dismissals: ReadonlyMap<string, () => void>,
): boolean {
  const modal = topModal(document);
  if (modal !== null && (!(event.target instanceof Element) || !modal.contains(event.target))) {
    return false;
  }
  let current: { dismiss: () => void; popup: Element; trigger: Element } | null = null;
  for (const [id, dismiss] of dismissals) {
    const popup = document.getElementById(id);
    const trigger = document.getElementById(`${id}-trigger`);
    if (
      popup !== null &&
      trigger !== null &&
      (modal === null || (modal.contains(popup) && modal.contains(trigger)))
    ) {
      current = { dismiss, popup, trigger };
    }
  }
  if (current === null) return false;
  // 仅浮层拿走焦点时归还，悬停提示不能抢走页面上其他控件的焦点。
  const restoreFocus = current.popup.contains(document.activeElement);
  current.dismiss();
  if (restoreFocus) focusClosestControl(current.trigger);
  return true;
}
