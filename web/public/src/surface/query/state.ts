// Public 查询状态与地址栏的双向同步。

import {
  type QueryState,
  type QueryWriteMode,
  readQueryPairs,
  useQueryState,
} from "@auction/web-shared/net/query";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { useRouter } from "@auction/web-shared/router";
import { createEffect, useContext } from "solid-js";

import type { PublicBoardQuery } from "./board";
import {
  InitialPublicLocationContext,
  type InitialPublicSearch,
  markBrowserLocationChanged,
  PublicLocationRevisionContext,
  readBrowserLocationRevision,
  readPublicQuery,
} from "./location";
import { managedPublicQueryPairs, PUBLIC_QUERY_KEYS, queryPairsString } from "./serialization";

// 每个查询状态 cell 绑定自己的首次地址快照；更新入口按调用方传入的 state 取回同一份快照，
// 不用模块级“最近创建”的回退，多个宿主或重新挂载的实例不会互相串用。
const initialSearchByState = new WeakMap<Cell<PublicBoardQuery>, InitialPublicSearch>();

/** 挂载与地址栏双向同步的查询状态；写回只替换本 crate 管理的键。 */
export function usePublicQueryState(): QueryState<PublicBoardQuery> {
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
  const result: QueryState<PublicBoardQuery> = {
    state: query.state,
    set(value, mode) {
      if (writePublicQuery(value, mode)) {
        query.state.set(value);
      }
    },
    push(value) {
      if (writePublicQuery(value, "push")) {
        query.state.set(value);
      }
    },
    replace(value) {
      if (writePublicQuery(value, "replace")) {
        query.state.set(value);
      }
    },
  };
  const locationRevision = useContext(PublicLocationRevisionContext);
  const router = useRouter();
  createEffect(
    () => {
      // 完整位置读取订阅 Router，覆盖 Link push 与同路径 query 导航。
      if (locationRevision !== null) {
        locationRevision.revision();
      }
      // 本模块直接写 History 时不经过 Router，也不一定由发起写入的实例自己重读；
      // 每个挂载中的查询状态都订阅这一点，成功写入后统一从地址栏取最新值。
      return [
        router.currentLocationString(),
        locationRevision === null ? -1 : locationRevision.revision(),
        readBrowserLocationRevision(),
      ] as const;
    },
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
 * 返回是否真正写入；只有成功时才推进共享地址版本。 */
export function writePublicQuery(query: PublicBoardQuery, mode: QueryWriteMode): boolean {
  return writePublicQueryPairs(PUBLIC_QUERY_KEYS, managedPublicQueryPairs(query), mode);
}

function writePublicQueryPairs(
  keys: readonly string[],
  pairs: readonly (readonly [string, string])[],
  mode: QueryWriteMode,
): boolean {
  if (typeof window === "undefined") {
    return false;
  }
  let history: History;
  try {
    history = window.history;
  } catch {
    return false;
  }
  const merged: [string, string][] = [];
  for (const pair of readQueryPairs()) {
    if (!keys.includes(pair[0])) {
      merged.push([pair[0], pair[1]]);
    }
  }
  for (const pair of pairs) {
    merged.push([pair[0], pair[1]]);
  }
  const encoded = queryPairsString(merged);
  let pathname = "/";
  let hash = "";
  let state: unknown = null;
  try {
    pathname = window.location.pathname;
  } catch {
    pathname = "/";
  }
  try {
    hash = window.location.hash;
  } catch {
    hash = "";
  }
  try {
    state = history.state;
  } catch {
    state = null;
  }
  const url = encoded.length === 0 ? `${pathname}${hash}` : `${pathname}?${encoded}${hash}`;
  try {
    if (mode === "push") {
      history.pushState(state, "", url);
    } else {
      history.replaceState(state, "", url);
    }
    markBrowserLocationChanged();
  } catch {
    // 地址栏不可写时调用方必须保持原查询状态；失败不能推进地址版本或通知其他实例。
    return false;
  }
  return true;
}

/** 读取地址栏最新查询、应用变更并按模式写回（读-改-写），同步 hook 状态。 */
export function updatePublicQueryState(
  state: Cell<PublicBoardQuery>,
  update: (query: PublicBoardQuery) => void,
  mode: QueryWriteMode,
): boolean {
  const query = readPublicQuery(initialSearchByState.get(state) ?? null);
  update(query);
  if (!writePublicQuery(query, mode)) {
    return false;
  }
  state.set(query);
  return true;
}
