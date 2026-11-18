// 页面需要保留查询解析器省略的原文时，跟踪真实地址栏的搜索段。

import type { QueryPair, QueryWriteMode } from "@auction/web-shared/net/query";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { useRouter } from "@auction/web-shared/router";
import { useContext } from "solid-js";

import type { PublicBoardQuery } from "../query/board";
import {
  currentBrowserSearch,
  markBrowserLocationChanged,
  PublicLocationRevisionContext,
  readBrowserLocationRevision,
  readPublicQuery,
} from "../query/location";
import { queryPairsString } from "../query/serialization";

/** 读取地址栏原文，包含解析后查询状态无法区分的重复或非法条件。 */
export function usePublicQuerySearch(): () => string {
  const router = useRouter();
  const location = useContext(PublicLocationRevisionContext);
  return createValueMemo(() => {
    router.currentLocationString();
    location?.revision();
    readBrowserLocationRevision();
    return currentBrowserSearch();
  });
}

/** 只编码本次写入的值，保留其余 URL 键值对的原始字节和顺序。 */
export function writePublicQueryPatch(
  keys: readonly string[],
  pairs: readonly QueryPair[],
  mode: QueryWriteMode,
): boolean {
  if (typeof window === "undefined") {
    return false;
  }
  const raw = currentBrowserSearch().slice(1);
  const parts =
    raw === ""
      ? []
      : raw.split("&").filter((part) => {
          const separator = part.indexOf("=");
          const encodedKey = separator === -1 ? part : part.slice(0, separator);
          // 只解码键以识别等价编码，原始片段本身不经 URLSearchParams 重新序列化。
          const key = new URLSearchParams(`key=${encodedKey}`).get("key");
          return key === null || !keys.includes(key);
        });
  if (pairs.length !== 0) {
    parts.push(queryPairsString(pairs));
  }
  const search = parts.length === 0 ? "" : `?${parts.join("&")}`;
  try {
    const { pathname, hash } = window.location;
    const history = window.history;
    const url = `${pathname}${search}${hash}`;
    if (mode === "push") {
      history.pushState(history.state, "", url);
    } else {
      history.replaceState(history.state, "", url);
    }
  } catch {
    return false;
  }
  return true;
}

/** 局部写址成功才提交页内状态并通知其他读取者。 */
export function updatePublicQueryPairs(
  state: Cell<PublicBoardQuery>,
  keys: readonly string[],
  pairs: readonly QueryPair[],
  mode: QueryWriteMode,
): boolean {
  if (!writePublicQueryPatch(keys, pairs, mode)) {
    return false;
  }
  state.set(readPublicQuery());
  markBrowserLocationChanged();
  return true;
}
