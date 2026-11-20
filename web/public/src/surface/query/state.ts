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
import { createEffect, useContext } from "solid-js";

import { type AuctionParameter, auctionParameter } from "./auction";
import type { PublicBoardQuery } from "./board";
import {
  bindPublicQueryRouter,
  currentBrowserHash,
  currentBrowserSearch,
  currentPublicPathname,
  InitialPublicLocationContext,
  type InitialPublicSearch,
  PublicLocationRevisionContext,
  publicQueryPage,
  readPublicQuery,
  readPublicQueryForState,
  updatePublicQueryPairs,
} from "./location";
import {
  managedPublicQueryPairs,
  PUBLIC_QUERY_KEYS,
  publicQueryString,
  queryPairsString,
} from "./serialization";

type PublicQueryFields = Omit<PublicBoardQuery, "auction">;

/** 完整写入可替换场次参数；省略时沿用最新地址的原场次。 */
export type PublicQueryWrite = PublicQueryFields & {
  readonly auction?: AuctionParameter;
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
  const router = useRouter();
  const query = useQueryState(
    PUBLIC_QUERY_KEYS,
    () => readPublicQuery(initial, router),
    managedPublicQueryPairs,
    // 页内查询写回由下方已有的 Router 订阅恢复，避免共享层再解析一次。
    { watchRouteWrites: false },
  );
  bindPublicQueryRouter(query.state, router, initial);
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
  createEffect(
    // 同路径查询成功写入也会更新快照，各挂载实例从同一地址读取已应用查询。
    () => [router.current(), router.currentLocationString(), locationRevision?.revision()] as const,
    () => {
      // 地址栏读取含首次 query 恢复，属于 effect 的副作用阶段。
      const current = readPublicQuery(initial, router);
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

function persistQuery(
  query: PublicQueryWrite,
  mode: QueryWriteMode,
  replaceAll: boolean,
  state?: Cell<PublicBoardQuery>,
): PublicBoardQuery | null {
  if (typeof window === "undefined") {
    return null;
  }
  const pathname = currentPublicPathname();
  const search = currentBrowserSearch();
  const current = state === undefined ? readPublicQuery() : readPublicQueryForState(state);
  const next = { ...query, auction: query.auction ?? current.auction };
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
  const next = persistQuery(query, mode, false, state);
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

/** 读取地址栏最新查询、更新场次以外的筛选；只有写入成功才同步查询。 */
export function updatePublicQueryState(
  state: Cell<PublicBoardQuery>,
  update: (query: PublicQueryFields) => void,
  mode: QueryWriteMode,
): boolean {
  return updateFullQuery(state, update, mode);
}

/** 提交场次选择及同一次操作负责的筛选或分页，拒写时保留原查询。
 * 自动选择只用 replace，不能覆盖尚待玩家修复的非法场次。 */
export function commitAuctionSelection(
  state: Cell<PublicBoardQuery>,
  id: string | null,
  mode: QueryWriteMode,
  change: readonly string[] | ((query: PublicQueryFields) => void),
): boolean {
  const auction = auctionParameter(id);
  if (
    auction.kind === "invalid" ||
    (mode === "replace" && readPublicQueryForState(state).auction.kind === "invalid")
  ) {
    return false;
  }
  if (typeof change === "function") {
    return updateFullQuery(state, change, mode, auction);
  }
  return updatePublicQueryPairs(
    state,
    ["auction", ...change],
    auction.kind === "selected" ? [["auction", auction.id]] : [],
    mode,
  );
}

function updateFullQuery(
  state: Cell<PublicBoardQuery>,
  update: (query: PublicQueryFields) => void,
  mode: QueryWriteMode,
  selection?: AuctionParameter,
): boolean {
  const current = readPublicQueryForState(state, initialSearchByState.get(state) ?? null);
  const { auction, ...draft } = current;
  update(draft);
  if (selection === undefined && publicQueryPage(state) === "Me") {
    const next = { ...draft, auction };
    const keys = changedAccountPageKeys(current, next);
    if (keys.length !== 0) {
      // 账户翻页只提交改动的表；另一张表与其余筛选仍使用各自的原始查询片段。
      const pairs = managedPublicQueryPairs(next).filter(([key]) => keys.includes(key));
      return updatePublicQueryPairs(state, keys, pairs, mode);
    }
  }
  return commitQuery(
    state,
    selection === undefined ? draft : { ...draft, auction: selection },
    mode,
  );
}
