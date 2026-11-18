// Public 查询状态与地址栏的双向同步。

import {
  parseQuerySearch,
  type QueryState,
  type QueryWriteMode,
  useQueryState,
} from "@auction/web-shared/net/query";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { useRouter, writeLocationPreservingScroll } from "@auction/web-shared/router";
import { validUuid } from "@auction/web-shared/ui/validation";
import { createEffect, useContext } from "solid-js";

import type { PublicBoardQuery } from "./board";
import {
  currentBrowserHash,
  currentBrowserSearch,
  currentPublicPathname,
  InitialPublicLocationContext,
  type InitialPublicSearch,
  PublicLocationRevisionContext,
  readPublicQuery,
  updatePublicQueryPairs,
} from "./location";
import { parsePublicQuery } from "./parse";
import {
  managedPublicQueryPairs,
  PUBLIC_QUERY_KEYS,
  publicQueryString,
  queryPairsString,
} from "./serialization";

/** 可写查询不携带非法原文；省略场次字段表示沿用地址中的场次。 */
export type PublicQueryWrite = Omit<PublicBoardQuery, "auctionId" | "invalidAuction"> & {
  readonly auctionId?: string | null;
};

/** 读出完整状态，写入时可省略场次。 */
export type PublicQueryState = Omit<QueryState<PublicQueryWrite>, "state"> & {
  readonly state: Cell<PublicBoardQuery>;
};

// 每个查询状态 cell 绑定自己的首次地址快照；更新入口按调用方传入的 state 取回同一份快照，
// 不用模块级“最近创建”的回退，多个宿主或重新挂载的实例不会互相串用。
const initialSearchByState = new WeakMap<Cell<PublicBoardQuery>, InitialPublicSearch>();

/** 挂载与地址栏双向同步的查询状态；写回只替换本 crate 管理的键。 */
export function usePublicQueryState(): PublicQueryState {
  const initial = useContext(InitialPublicLocationContext);
  const query = useQueryState(
    PUBLIC_QUERY_KEYS,
    () => readPublicQuery(initial),
    managedPublicQueryPairs,
  );
  if (initial !== null) {
    initialSearchByState.set(query.state, initial);
  }
  // 共享 QueryState 的写回使用 URLSearchParams；公开端契约要求与 Rust form_urlencoded
  // 的百分号编码保持一致，状态句柄的三个写入口复用本模块的编码器，避免两套序列化漂移。
  const result: PublicQueryState = {
    state: query.state,
    set(value, mode) {
      commitQuery(query.state, value, mode);
    },
    push(value) {
      commitQuery(query.state, value, "push");
    },
    replace(value) {
      commitQuery(query.state, value, "replace");
    },
  };
  const locationRevision = useContext(PublicLocationRevisionContext);
  const router = useRouter();
  createEffect(
    // 同路径查询成功写入也会更新快照，各挂载实例从同一地址读取已应用查询。
    () => [router.currentLocationString(), locationRevision?.revision()] as const,
    () => {
      // 地址栏读取含首次 query 恢复，属于 effect 的副作用阶段。
      const current = readPublicQuery(initial);
      if (!structuralEquals(query.state.peek(), current)) {
        query.state.set(current);
      }
    },
  );
  return result;
}

/** 把完整查询状态按既有序列化规则写回地址栏（只替换本 crate 管理的键）。
 * 返回是否真正写入；路由只发布成功写入的地址。 */
export function writePublicQuery(query: PublicQueryWrite, mode: QueryWriteMode): boolean {
  return persistQuery(query, mode, false) !== null;
}

/** 原地替换整份查询，供首页重置等同时清除非受管字段的导航使用。 */
export function setPublicQuery(query: PublicQueryWrite, replace: boolean): boolean {
  return persistQuery(query, replace ? "replace" : "push", true) !== null;
}

function withAuctionWrite(query: PublicQueryWrite, current: PublicBoardQuery): PublicBoardQuery {
  // 提供 auctionId（包括 null）就替换整条场次状态，丢弃旧非法原文；省略才沿用当前地址。
  if (!Object.hasOwn(query, "auctionId")) {
    return { ...query, auctionId: current.auctionId, invalidAuction: current.invalidAuction };
  }
  const auctionId = query.auctionId ?? null;
  return auctionId !== null && !validUuid(auctionId)
    ? { ...query, auctionId: null, invalidAuction: [auctionId] }
    : { ...query, auctionId, invalidAuction: null };
}

function persistQuery(
  query: PublicQueryWrite,
  mode: QueryWriteMode,
  replaceAll: boolean,
): PublicBoardQuery | null {
  if (typeof window === "undefined") {
    return null;
  }
  const pathname = currentPublicPathname();
  const search = currentBrowserSearch();
  const current = parsePublicQuery(pathname, search);
  const next = withAuctionWrite(query, current);
  // 重复应用当前筛选不创建可返回的条目，也不把省略的默认值写入地址。
  if (!replaceAll && mode === "push" && structuralEquals(next, current)) {
    return null;
  }
  let suffix: string;
  if (replaceAll) {
    suffix = publicQueryString(next);
  } else {
    const retained = [...parseQuerySearch(search)].filter(
      ([key]) => !PUBLIC_QUERY_KEYS.includes(key),
    );
    const encoded = queryPairsString([...retained, ...managedPublicQueryPairs(next)]);
    suffix = `${encoded === "" ? "" : `?${encoded}`}${currentBrowserHash()}`;
  }
  if (!writeLocationPreservingScroll(`${pathname}${suffix}`, mode)) {
    // 地址栏不可写时不能提交查询或唤醒其他实例。
    return null;
  }
  return next;
}

function commitQuery(
  state: Cell<PublicBoardQuery>,
  query: PublicQueryWrite,
  mode: QueryWriteMode,
): boolean {
  const next = persistQuery(query, mode, false);
  if (next === null) {
    return false;
  }
  state.set(next);
  return true;
}

function changedAccountPageKeys(current: PublicBoardQuery, next: PublicBoardQuery): string[] {
  const { bidPage, proxyPage, ...previous } = current;
  const { bidPage: nextBidPage, proxyPage: nextProxyPage, ...rest } = next;
  if (!structuralEquals(previous, rest)) {
    return [];
  }
  const keys: string[] = [];
  if (bidPage !== nextBidPage) {
    keys.push("bid_page");
  }
  if (proxyPage !== nextProxyPage) {
    keys.push("proxy_page");
  }
  return keys;
}

/** 读取地址栏最新查询、应用变更并按模式写回（读-改-写），同步 hook 状态。 */
export function updatePublicQueryState(
  state: Cell<PublicBoardQuery>,
  update: (query: PublicBoardQuery) => void,
  mode: QueryWriteMode,
): boolean {
  const query = readPublicQuery(initialSearchByState.get(state) ?? null);
  // 非法态的可见 ID 也是 null，读-改-写须在赋值时记录调用方是否提供场次。
  let auctionId = query.auctionId;
  let wroteAuction = false;
  const draft: PublicBoardQuery = {
    ...query,
    get auctionId() {
      return auctionId;
    },
    set auctionId(value) {
      auctionId = value;
      wroteAuction = true;
    },
  };
  update(draft);
  if (!wroteAuction && currentPublicPathname() === "/me") {
    const keys = changedAccountPageKeys(query, draft);
    if (keys.length !== 0) {
      // 账户翻页只提交改动的表；另一张表与其余筛选仍使用各自的原始查询片段。
      const pairs = managedPublicQueryPairs(draft).filter(([key]) => keys.includes(key));
      return updatePublicQueryPairs(state, keys, pairs, mode);
    }
  }
  const { auctionId: selected, invalidAuction: _invalidAuction, ...fields } = draft;
  return commitQuery(state, wroteAuction ? { ...fields, auctionId: selected } : fields, mode);
}
