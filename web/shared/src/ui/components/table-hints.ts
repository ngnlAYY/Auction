// 表格单元格文本溢出时按需补充原生 title 提示。

import { onSettled } from "solid-js";

interface TableHintsGuard {
  cleanup(): void;
}

const EXCLUDED_TARGET =
  ".shared-truncated-text, .ui-truncated-text, .admin-users-history-trigger, [role=dialog]";
const EXCLUDED_CONTROL = "input, textarea, [role=dialog]";
const PREFERRED_CONTROL = "button, a, summary";

/** 在组件挂载后安装全局委托，在卸载时撤销监听并清理本模块添加的提示。 */
export function useTableHints(): void {
  onSettled(() => {
    const guard = installTableHints();
    if (guard === null) {
      return undefined;
    }
    return () => guard.cleanup();
  });
}

function installTableHints(): TableHintsGuard | null {
  if (typeof document === "undefined") {
    return null;
  }
  const body = document.body;
  if (body === null) {
    return null;
  }
  let active: Element | null = null;

  const onTableEvent = (event: Event): void => {
    const target = event.target;
    if (!(target instanceof Element)) {
      return;
    }
    const cell = closestOrNull(target, "th, td");
    if (cell === null) {
      active = null;
      return;
    }
    // 避免与已有完整文本浮层竞争，也不把表单输入或一次性凭据复制进 title。
    if (
      closestOrNull(target, EXCLUDED_TARGET) !== null ||
      querySelectorOrNull(cell, EXCLUDED_CONTROL) !== null
    ) {
      clearOwnedHint(cell);
      if (active !== null) {
        clearOwnedHint(active);
      }
      active = null;
      return;
    }
    const control = closestOrNull(target, PREFERRED_CONTROL);
    const hintTarget = control !== null && cell.contains(control) ? control : cell;
    refreshHint(hintTarget);
    active = hintTarget;
  };

  // 不观察属性：更新 title 不会触发自身，实时快照文字变化才刷新当前提示。
  let observer: MutationObserver;
  try {
    observer = new MutationObserver((records) => {
      const current = active;
      if (current === null) {
        return;
      }
      if (!current.isConnected) {
        active = null;
        return;
      }
      if (records.some((record) => current.contains(record.target))) {
        refreshHint(current);
      }
    });
  } catch {
    return null;
  }

  try {
    observer.observe(body, {
      childList: true,
      characterData: true,
      subtree: true,
    });
  } catch {
    return null;
  }

  try {
    document.addEventListener("mouseover", onTableEvent);
    document.addEventListener("focusin", onTableEvent);
  } catch {
    observer.disconnect();
    document.removeEventListener("mouseover", onTableEvent);
    document.removeEventListener("focusin", onTableEvent);
    return null;
  }

  let cleaned = false;
  return {
    cleanup() {
      if (cleaned) {
        return;
      }
      cleaned = true;
      observer.disconnect();
      document.removeEventListener("mouseover", onTableEvent);
      document.removeEventListener("focusin", onTableEvent);
      try {
        for (const element of document.querySelectorAll("[data-table-hint]")) {
          clearOwnedHint(element);
        }
      } catch {
        // 源码忽略查询失败，仍保留已撤销的监听与观察器。
      }
    },
  };
}

function refreshHint(element: Element): void {
  if (
    closestOrNull(element, "[role=dialog]") !== null ||
    querySelectorOrNull(element, EXCLUDED_CONTROL) !== null
  ) {
    clearOwnedHint(element);
    return;
  }
  if (element.hasAttribute("title") && !element.hasAttribute("data-table-hint")) {
    return;
  }
  if (!(element instanceof HTMLElement)) {
    return;
  }
  // biome-ignore lint/performance/useDomNodeTextContent: 源码读取 innerText，title 必须跟随渲染文本。
  const text = rustTrim(element.innerText);
  if (text.length === 0) {
    clearOwnedHint(element);
    return;
  }
  if (element.getAttribute("title") !== text) {
    element.setAttribute("title", text);
    element.setAttribute("data-table-hint", "true");
  }
}

function clearOwnedHint(element: Element): void {
  if (element.hasAttribute("data-table-hint")) {
    element.removeAttribute("title");
    element.removeAttribute("data-table-hint");
  }
}

function closestOrNull(element: Element, selector: string): Element | null {
  try {
    return element.closest(selector);
  } catch {
    return null;
  }
}

function querySelectorOrNull(element: Element, selector: string): Element | null {
  try {
    return element.querySelector(selector);
  } catch {
    return null;
  }
}

function rustTrim(value: string): string {
  let start = 0;
  while (start < value.length && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  let end = value.length;
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

function isRustWhitespace(code: number): boolean {
  return (
    (code >= 0x0009 && code <= 0x000d) ||
    code === 0x0020 ||
    code === 0x0085 ||
    code === 0x00a0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}
