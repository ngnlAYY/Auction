// URL query 参数与组件状态的双向同步。
// 只读写当前地址的 query 段，不接管路由。多键共存靠「声明各自管理的键」：写回只替换本
// hook 声明的键，其余键（其他 hook 或路由持有）原样保留。状态形状与解析/序列化规则由
// 调用方注入，本模块不含业务键名。

import { getOwner, onCleanup, onSettled, runWithOwner } from "solid-js";

import { type Cell, createCell } from "../../reactive/cell";
import { structuralEquals } from "../../reactive/equality";
import { type LocationWriteOptions, writeLocationPreservingScroll } from "../../router/history";
import { watchLocationWritten } from "../../router/location-notice";

/** 写回地址栏时创建还是替换浏览器历史项。 */
export type QueryWriteMode = "push" | "replace";

/** 一个 URL query 键值对；重复键按出现顺序保留。 */
export type QueryPair = readonly [key: string, value: string];

/** 解析完整 search；第一个问号是分隔符，其余问号属于参数。 */
export function parseQuerySearch(search: string): URLSearchParams {
  return new URLSearchParams(search);
}

function queryWindow(): Window | null {
  return typeof window === "undefined" || window.document === undefined ? null : window;
}

/** 读取当前地址栏的全部 query 键值对；浏览器不可用时为空。 */
export function readQueryPairs(): readonly QueryPair[] {
  const browser = queryWindow();
  if (browser === null) {
    return [];
  }
  return [...parseQuerySearch(browser.location.search)];
}

/** 查询状态的历史写入与地址恢复选项。 */
export interface QueryStateOptions extends LocationWriteOptions {
  /** 调用方已有同页地址订阅时传入 `false`，只保留共享的原生历史恢复。 */
  readonly watchRouteWrites?: boolean;
}

// 历史层同步广播成功写入。查询写回照常更新路由快照，但不需要按站内导航重新解析。
let queryWriteDepth = 0;

/** 把 `pairs` 合并写回当前地址的 query 段；`keys` 声明的旧值整体替换，其余键原样保留。
 * 返回地址栏写入是否成功；没有浏览器环境时返回 `false`。 */
export function writeQueryPairs(
  keys: readonly string[],
  pairs: readonly QueryPair[],
  mode: QueryWriteMode,
  options?: LocationWriteOptions,
): boolean {
  const browser = queryWindow();
  if (browser === null) {
    return false;
  }
  const location = browser.location;
  let rawSearch: string;
  let pathname: string;
  let hash: string;
  try {
    rawSearch = location.search;
    pathname = location.pathname;
    hash = location.hash;
  } catch {
    return false;
  }
  const raw = rawSearch.startsWith("?") ? rawSearch.slice(1) : rawSearch;
  const retained =
    raw === ""
      ? []
      : raw.split("&").filter((segment) => {
          const separator = segment.indexOf("=");
          const encodedKey = separator === -1 ? segment : segment.slice(0, separator);
          const key = new URLSearchParams(`key=${encodedKey}`).get("key");
          return key === null || !keys.includes(key);
        });
  const replacement = new URLSearchParams();
  for (const [key, value] of pairs) {
    replacement.append(key, value);
  }
  const encoded = replacement.toString();
  if (encoded.length !== 0) {
    retained.push(encoded);
  }
  const query = retained.length === 0 ? "" : `?${retained.join("&")}`;
  const url = `${pathname}${query}${hash}`;
  queryWriteDepth += 1;
  try {
    return writeLocationPreservingScroll(url, mode, options);
  } finally {
    queryWriteDepth -= 1;
  }
}

/** 状态 → 写回键值对的序列化器（省略等于默认值的键）。 */
export type QuerySerializer<T> = (value: T) => readonly QueryPair[];

/** 全部 query 键值对 → 状态的解析器（挂载初值与导航恢复共用）。 */
export type QueryParser<T> = (pairs: readonly QueryPair[]) => T;

/** 随地址栏双向同步的状态句柄。 */
export interface QueryState<T> {
  /** 当前状态；挂载、同页导航与浏览器前进/后退时取自地址栏。 */
  readonly state: Cell<T>;
  /** 更新状态并按 `mode` 写回地址栏（只替换本 hook 声明管理的键）。 */
  set(value: T, mode: QueryWriteMode): void;
  /** 用户主动变更：更新状态并创建可返回的浏览器历史项。 */
  push(value: T): void;
  /** 程序化纠偏：更新状态并替换当前历史项。 */
  replace(value: T): void;
}

/** 每个 hook 只安装一次 popstate 监听；组件卸载时移除。 */
function usePopstateListener(onPopstate: () => void): void {
  // 原生事件不会进入 Solid 的作用域；解析器可能读取 Context，必须在创建时记住宿主。
  const owner = getOwner();
  onSettled(() => {
    const browser = queryWindow();
    if (browser === null) {
      return;
    }
    const listener = () => runWithOwner(owner, onPopstate);
    browser.addEventListener("popstate", listener);
    return () => browser.removeEventListener("popstate", listener);
  });
}

function currentQueryAddress(): { readonly pathname: string; readonly search: string } | null {
  const browser = queryWindow();
  if (browser === null) {
    return null;
  }
  const { pathname, search } = browser.location;
  return { pathname, search };
}

function useQueryNavigation(restore: (pairs: readonly QueryPair[]) => void): void {
  const owner = getOwner();
  let observed = currentQueryAddress();
  const pathname = observed?.pathname;
  const restoreFromLocation = (): void => {
    const next = currentQueryAddress();
    const previous = observed;
    observed = next;
    // 站内导航与前进后退共用路径门禁，离页实例不能解析目标页的参数。
    if (
      next === null ||
      next.pathname !== pathname ||
      queryWriteDepth !== 0 ||
      (previous?.pathname === next.pathname && previous.search === next.search)
    ) {
      return;
    }
    restore([...parseQuerySearch(next.search)]);
  };
  if (queryWindow() !== null) {
    // 初始化分页也会写回地址，订阅须在首次 effect 前就能记录成功写入。
    onCleanup(watchLocationWritten(() => runWithOwner(owner, restoreFromLocation)));
  }
  usePopstateListener(restoreFromLocation);
}

/** 挂载与 URL query 双向同步的状态；组件卸载时注销位置监听。 */
export function useQueryState<T>(
  keys: readonly string[],
  parse: QueryParser<T>,
  serialize: QuerySerializer<T>,
  options?: QueryStateOptions,
): QueryState<T> {
  const state = createCell(parse(readQueryPairs()));
  const restore = (pairs: readonly QueryPair[]): void => {
    const value = parse(pairs);
    if (!structuralEquals(value, state.peek())) {
      state.set(value);
    }
  };
  if (options?.watchRouteWrites === false) {
    usePopstateListener(() => restore(readQueryPairs()));
  } else {
    useQueryNavigation(restore);
  }

  const set = (value: T, mode: QueryWriteMode): void => {
    writeQueryPairs(keys, serialize(value), mode, options);
    state.set(value);
  };
  return {
    state,
    set,
    push: (value) => set(value, "push"),
    replace: (value) => set(value, "replace"),
  };
}

/** 浏览器前进/后退时递增调用方持有的位置版本；组件卸载时移除监听。 */
export function usePopstateRevision(revision: Cell<number>): void {
  usePopstateListener(() => {
    const next = Math.min(revision.peek() + 1, Number.MAX_SAFE_INTEGER);
    revision.set(next);
  });
}
