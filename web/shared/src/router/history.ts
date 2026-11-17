// 浏览器 History API 适配：与 Dioxus WebHistory::new(None, true) 保持相同的滚动位置协议。
// 路由匹配不在这里解析；本模块只负责读取当前地址、写入条目，并在页面提交后恢复 popstate 的位置。

import { onCleanup } from "solid-js";

/** 浏览器滚动位置；history state 仍按 Dioxus 的 `[x, y]` 数组保存。 */
export interface ScrollPosition {
  readonly x: number;
  readonly y: number;
}

/** 浏览器历史适配器。 */
export interface WebHistory {
  /** 当前地址的 pathname + search + hash，不含 origin。 */
  currentRoute(): string;
  /** 先保存当前滚动位置，再新增一条历史记录；地址相同时不重复写入。 */
  push(route: string): void;
  /** 用当前滚动位置替换当前条目。 */
  replace(route: string): void;
  /** 返回上一条历史记录；实际路由更新由随后的 popstate 完成。 */
  goBack(): void;
  /** 在目标页面提交后的下一帧恢复 popstate 保存的位置；导航已变化或路由已卸载时跳过。 */
  restoreScrollAfterCommit(position: ScrollPosition, isCurrent: () => boolean): void;
}

/** 创建历史适配器所需的回调。 */
export interface WebHistoryOptions {
  /** popstate 已经发生、浏览器地址已是目标地址时调用，并传入该条目保存的位置。 */
  readonly onPopState: (position: ScrollPosition | null) => void;
}

function currentWindowRoute(): string {
  const pathname = window.location.pathname || "/";
  return `${pathname}${window.location.search}${window.location.hash}`;
}

function currentScrollPosition(): ScrollPosition {
  return { x: window.scrollX, y: window.scrollY };
}

function positionState(position: ScrollPosition): [number, number] {
  return [position.x, position.y];
}

function replaceState(position: ScrollPosition, url: string | null): boolean {
  try {
    if (url === null) {
      window.history.replaceState(positionState(position), "");
    } else {
      window.history.replaceState(positionState(position), "", url);
    }
    return true;
  } catch {
    // Dioxus 忽略 History API 的失败；失败时保留原地址与状态。
    return false;
  }
}

function pushState(position: ScrollPosition, url: string): boolean {
  try {
    window.history.pushState(positionState(position), "", url);
    return true;
  } catch {
    // 跨源等非法地址不能写入历史条目；调用方随后按当前地址重新解析。
    return false;
  }
}

function updateCurrentScroll(): void {
  void replaceState(currentScrollPosition(), null);
}

function savedScrollPosition(): ScrollPosition | null {
  const state: unknown = window.history.state;
  if (!Array.isArray(state) || state.length < 2) {
    return null;
  }
  const x: unknown = state[0];
  const y: unknown = state[1];
  if (typeof x !== "number" || typeof y !== "number") {
    return null;
  }
  return { x, y };
}

/**
 * 创建与 Dioxus WebHistory 对齐的浏览器历史。
 * 初始化时接管滚动恢复并给当前条目写入滚动 state；之后的 push/replace 写入
 * `[scrollX, scrollY]` 数组。popstate 只把位置交给路由；路由要等目标页面提交后再请求恢复，
 * 避免异步页面尚未挂载时把位置滚动到一个错误的文档高度；路由卸载会取消尚未执行的恢复帧。
 */
export function createWebHistory(options: WebHistoryOptions): WebHistory {
  if ("scrollRestoration" in window.history) {
    window.history.scrollRestoration = "manual";
  }

  const currentUrl = currentWindowRoute();
  void replaceState(currentScrollPosition(), currentUrl);

  const handlePopState = (): void => {
    options.onPopState(savedScrollPosition());
  };
  let restoreFrame: number | null = null;
  const cancelPendingRestore = (): void => {
    if (restoreFrame !== null) {
      window.cancelAnimationFrame(restoreFrame);
      restoreFrame = null;
    }
  };
  window.addEventListener("popstate", handlePopState);
  onCleanup(() => {
    window.removeEventListener("popstate", handlePopState);
    cancelPendingRestore();
  });

  return {
    currentRoute: currentWindowRoute,
    push(route) {
      if (route === currentWindowRoute()) {
        return;
      }
      updateCurrentScroll();
      if (pushState(currentScrollPosition(), route)) {
        window.scrollTo(0, 0);
      }
    },
    replace(route) {
      if (replaceState(currentScrollPosition(), route)) {
        window.scrollTo(0, 0);
      }
    },
    goBack() {
      window.history.back();
    },
    restoreScrollAfterCommit(position, isCurrent) {
      cancelPendingRestore();
      restoreFrame = window.requestAnimationFrame(() => {
        restoreFrame = null;
        if (isCurrent()) {
          window.scrollTo(position.x, position.y);
        }
      });
    },
  };
}
