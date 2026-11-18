// Public 查询链接生成与历史导航。

import { useRouter } from "@auction/web-shared/router";
import { createEffect } from "solid-js";

import { type PublicBoardQuery, publicBoardQueryDefault } from "./board";
import { browserScrollState, currentBrowserSearch, markBrowserLocationChanged } from "./location";
import { publicQueryString } from "./serialization";

/** 原地写回完整查询；push 前先把离开的条目更新为当前滚动位置，与 Dioxus 历史一致。
 * 返回是否真正写入地址栏；失败时不推进共享地址版本。 */
export function setPublicQuery(query: PublicBoardQuery, replace: boolean): boolean {
  if (typeof window === "undefined") {
    return false;
  }
  let history: History;
  try {
    history = window.history;
  } catch {
    return false;
  }
  let pathname: string;
  try {
    pathname = window.location.pathname;
  } catch {
    pathname = "/";
  }
  const url = `${pathname}${publicQueryString(query)}`;
  const state = browserScrollState();
  if (replace) {
    try {
      history.replaceState(state, "", url);
      markBrowserLocationChanged();
      return true;
    } catch {
      // Rust 对历史写入失败不做回退；保持调用前的地址栏状态。
      return false;
    }
  }
  try {
    history.replaceState(state, "");
  } catch {
    // Rust 忽略离开条目写入滚动位置的失败，后续 push 仍照常尝试。
  }
  try {
    history.pushState(state, "", url);
    markBrowserLocationChanged();
    return true;
  } catch {
    // Rust 对 push 失败不做回退；保持调用前的地址栏状态。
    return false;
  }
}

export function publicHref(path: string, query: PublicBoardQuery): string {
  return `${path}${publicQueryString(query)}`;
}

/** 生成首页查询；`world` 是导航实际继承的地址值，`null` 表示省略并采用主世界缺省。
 * 只保留目标世界与本地数量偏好，避免从其他页面带回不属于首页的筛选和页码。 */
export function publicAuctionHomeQuery(world: string | null): PublicBoardQuery {
  return {
    ...publicBoardQueryDefault(),
    world,
  };
}

/** 把旧版公开端别名无历史记录地收敛到 canonical URL，并保留当前查询参数。 */
export function redirectPublicAlias(path: string): null {
  const router = useRouter();
  const target = `${path}${currentBrowserSearch()}`;
  createEffect(
    () => null,
    () => {
      router.replace(target);
    },
  );
  return null;
}
