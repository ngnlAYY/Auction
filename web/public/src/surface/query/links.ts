// Public 查询链接生成与历史导航。

import { useRouter } from "@auction/web-shared/router";
import { createEffect } from "solid-js";

import { type PublicBoardQuery, publicBoardQueryDefault } from "./board";
import { browserScrollState, currentBrowserSearch, markBrowserLocationChanged } from "./location";
import { publicQueryString } from "./serialization";

/** 原地写回完整查询；push 前先把离开的条目更新为当前滚动位置，与 Dioxus 历史一致。 */
export function setPublicQuery(query: PublicBoardQuery, replace: boolean): void {
  if (typeof window === "undefined") {
    return;
  }
  let history: History;
  try {
    history = window.history;
  } catch {
    return;
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
    } catch {
      // Rust 对历史写入失败不做回退；保持调用前的地址栏状态。
    }
    return;
  }
  try {
    history.replaceState(state, "");
  } catch {
    // Rust 忽略离开条目写入滚动位置的失败，后续 push 仍照常尝试。
  }
  try {
    history.pushState(state, "", url);
    markBrowserLocationChanged();
  } catch {
    // Rust 对 push 失败不做回退；保持调用前的地址栏状态。
  }
}

export function publicHref(path: string, query: PublicBoardQuery): string {
  return `${path}${publicQueryString(query)}`;
}

/** 首页入口不冻结数量：导航时读取最新本地偏好，避免外壳旧链接覆盖刚保存的选择。 */
export function publicAuctionHomeHref(world: string): string {
  return world === "isekai" ? "/?world=isekai" : "/";
}

/** 生成当前世界的拍卖首页查询；同路由导航可直接用它替换历史视图状态。 */
export function publicAuctionHomeQuery(world: string): PublicBoardQuery {
  // 只保留目标世界与本地数量偏好，避免从其他页面带回不属于首页的筛选和页码。
  return {
    ...publicBoardQueryDefault(),
    world: world === "isekai" ? "isekai" : null,
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
