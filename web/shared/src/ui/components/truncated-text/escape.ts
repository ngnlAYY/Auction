// 每份文档的可见截断提示共用一条 Escape 监听。

import { listenDocumentEscape, topModal } from "../../../platform/a11y/escape";

interface OpenTooltips {
  readonly dismissals: Map<string, () => boolean>;
  readonly unlisten: () => void;
}

const openByDocument = new WeakMap<Document, OpenTooltips>();

/** 在捕获阶段优先关闭同一前景范围最近打开的提示。 */
export function registerTooltipEscape(
  document: Document,
  tooltipId: string,
  dismiss: () => boolean,
): () => void {
  let open = openByDocument.get(document);
  if (open === undefined) {
    const dismissals = new Map<string, () => boolean>();
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
  dismissals: ReadonlyMap<string, () => boolean>,
): boolean {
  const modal = topModal(document);
  if (modal !== null && (!(event.target instanceof Element) || !modal.contains(event.target))) {
    return false;
  }
  let current: (() => boolean) | null = null;
  for (const [id, dismiss] of dismissals) {
    const popup = document.getElementById(id);
    const trigger = document.getElementById(`${id}-trigger`);
    if (
      popup !== null &&
      trigger !== null &&
      (modal === null || (modal.contains(popup) && modal.contains(trigger)))
    ) {
      current = dismiss;
    }
  }
  return current?.() ?? false;
}
