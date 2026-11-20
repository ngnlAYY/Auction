// URL query 参数与组件状态的双向同步。
// 只读写当前地址的 query 段，不接管路由。多键共存靠「声明各自管理的键」：写回只替换本
// hook 声明的键，其余键（其他 hook 或路由持有）原样保留。状态形状与解析/序列化规则由
// 调用方注入，本模块不含业务键名。

import { getOwner, onSettled, runWithOwner } from "solid-js";

import { type Cell, createCell } from "../../reactive/cell";
import { structuralEquals } from "../../reactive/equality";
import { type LocationWriteOptions, writeLocationPreservingScroll } from "../../router/history";

/** 写回地址栏时创建还是替换浏览器历史项。 */
export type QueryWriteMode = "push" | "replace";

/** 一个 URL query 键值对；重复键按出现顺序保留。 */
export type QueryPair = readonly [key: string, value: string];

/** 解析完整 search；第一个问号是分隔符，其余问号属于参数。 */
export function parseQuerySearch(search: string): URLSearchParams {
  return new URLSearchParams(search);
}

/** 读取当前地址栏的全部 query 键值对；浏览器不可用时为空。 */
export function readQueryPairs(): readonly QueryPair[] {
  if (typeof window === "undefined") {
    return [];
  }
  return [...parseQuerySearch(window.location.search)];
}

/** 查询状态写回地址栏的选项；`sameAddress` 缺省 `"skip"`，与共享历史层一致。 */
export type QueryStateOptions = LocationWriteOptions;

/** 把 `pairs` 合并写回当前地址的 query 段；`keys` 声明的旧值整体替换，其余键原样保留。
 * 返回地址栏写入是否成功；没有浏览器环境时返回 `false`。 */
export function writeQueryPairs(
  keys: readonly string[],
  pairs: readonly QueryPair[],
  mode: QueryWriteMode,
  options?: QueryStateOptions,
): boolean {
  if (typeof window === "undefined") {
    return false;
  }
  const location = window.location;
  const parameters = new URLSearchParams();
  for (const [key, value] of readQueryPairs()) {
    if (!keys.includes(key)) {
      parameters.append(key, value);
    }
  }
  for (const [key, value] of pairs) {
    parameters.append(key, value);
  }
  const encoded = parameters.toString();
  let pathname = "/";
  let hash = "";
  try {
    pathname = location.pathname;
  } catch {
    pathname = "/";
  }
  try {
    hash = location.hash;
  } catch {
    hash = "";
  }
  const url = encoded.length === 0 ? `${pathname}${hash}` : `${pathname}?${encoded}${hash}`;
  return writeLocationPreservingScroll(url, mode, options);
}

/** 状态 → 写回键值对的序列化器（省略等于默认值的键）。 */
export type QuerySerializer<T> = (value: T) => readonly QueryPair[];

/** 全部 query 键值对 → 状态的解析器（挂载初值与 popstate 恢复共用）。 */
export type QueryParser<T> = (pairs: readonly QueryPair[]) => T;

/** 随地址栏双向同步的状态句柄。 */
export interface QueryState<T> {
  /** 当前状态；挂载时取自地址栏初值，浏览器前进/后退时自动恢复。 */
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
    if (typeof window === "undefined") {
      return;
    }
    const listener = () => runWithOwner(owner, onPopstate);
    window.addEventListener("popstate", listener);
    return () => window.removeEventListener("popstate", listener);
  });
}

/** 挂载与 URL query 双向同步的状态；组件卸载时移除 popstate 监听。 */
export function useQueryState<T>(
  keys: readonly string[],
  parse: QueryParser<T>,
  serialize: QuerySerializer<T>,
  options?: QueryStateOptions,
): QueryState<T> {
  const state = createCell(parse(readQueryPairs()));
  usePopstateListener(() => {
    const value = parse(readQueryPairs());
    if (!structuralEquals(value, state.peek())) {
      state.set(value);
    }
  });

  return {
    state,
    set(value, mode) {
      writeQueryPairs(keys, serialize(value), mode, options);
      state.set(value);
    },
    push(value) {
      writeQueryPairs(keys, serialize(value), "push", options);
      state.set(value);
    },
    replace(value) {
      writeQueryPairs(keys, serialize(value), "replace", options);
      state.set(value);
    },
  };
}

/** 浏览器前进/后退时递增调用方持有的位置版本；组件卸载时移除监听。 */
export function usePopstateRevision(revision: Cell<number>): void {
  usePopstateListener(() => {
    const next = Math.min(revision.peek() + 1, Number.MAX_SAFE_INTEGER);
    revision.set(next);
  });
}
