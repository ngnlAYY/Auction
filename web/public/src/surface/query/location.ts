// 地址栏查询读写、首次地址快照与位置变化版本上下文。

import type { QueryPair, QueryWriteMode } from "@auction/web-shared/net/query";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { writeLocationPreservingScroll } from "@auction/web-shared/router";
import { createContext } from "solid-js";

import { type PublicBoardQuery, publicBoardQueryDefault } from "./board";
import { parsePublicQuery } from "./parse";
import { queryPairsString } from "./serialization";

/** 可提供首次 query 的地址快照；路由首次挂载后该快照会失效。 */
export interface InitialPublicSearch {
  searchFor(pathname: string): string | null;
}

/** 读取当前地址栏查询；首次规范化期间查询暂时为空时可恢复快照。
 *
 * 恢复只在地址替换成功后才采用初始 search：写地址失败时解析到的仍是当前空查询，
 * 不能把未真正恢复的参数当成地址栏状态。首次地址快照由调用方在自己的组件上下文取得并显式
 * 传入；模块级“最近创建的快照”会让多个宿主或重新挂载的实例互相串用。
 */
export function readPublicQuery(initial: InitialPublicSearch | null = null): PublicBoardQuery {
  if (typeof window === "undefined") {
    return publicBoardQueryDefault();
  }
  let pathname: string;
  try {
    pathname = window.location.pathname;
  } catch {
    pathname = "";
  }
  let search: string;
  try {
    search = window.location.search;
  } catch {
    search = "";
  }
  if (search === "") {
    const initialSearch = initial?.searchFor(pathname) ?? null;
    if (
      initialSearch !== null &&
      writeLocationPreservingScroll(`${pathname}${initialSearch}${currentBrowserHash()}`, "replace")
    ) {
      search = initialSearch;
    }
  }
  return parsePublicQuery(pathname, search);
}

/** Router 首次规范化期间的公开端查询回退。 */
export class InitialPublicLocation {
  private readonly pathname: string;
  private readonly search: string;
  private readonly hadQueryMarker: boolean;
  private readonly available: Cell<boolean>;

  constructor(
    pathname: string,
    search: string,
    hadQueryMarker: boolean,
    available: Cell<boolean> = createCell(true),
  ) {
    this.pathname = pathname;
    this.search = search;
    this.hadQueryMarker = hadQueryMarker;
    this.available = available;
  }

  searchFor(pathname: string): string | null {
    const available = this.available.peek();
    this.available.set(false);
    if (!available || this.pathname !== pathname || this.search === "") {
      return null;
    }
    return this.search;
  }

  expire(): void {
    this.removeRouterEmptyQuery();
    this.available.set(false);
  }

  /** 只在原始 URL 没有 `?`、pathname/search 仍未变化时无历史记录地移除 Router 空标记。 */
  private removeRouterEmptyQuery(): void {
    if (this.search !== "" || this.hadQueryMarker || typeof window === "undefined") {
      return;
    }
    const location = window.location;
    let pathname: string;
    try {
      pathname = location.pathname;
    } catch {
      return;
    }
    if (pathname !== this.pathname) {
      return;
    }
    try {
      if (location.search !== "") {
        return;
      }
    } catch {
      // search 读取失败时 Rust 继续检查 href，不能把 Router 空标记当成有效 query。
    }
    let href: string;
    try {
      href = location.href;
    } catch {
      return;
    }
    const beforeHash = href.split("#")[0] ?? "";
    if (!beforeHash.endsWith("?")) {
      return;
    }
    writeLocationPreservingScroll(`${this.pathname}${currentBrowserHash()}`, "replace");
  }
}

/** 保存首次 pathname 与 search，供 Router 首次规范化短暂回退。 */
export function initialPublicLocation(): InitialPublicLocation {
  if (typeof window === "undefined") {
    return new InitialPublicLocation("/", "", false);
  }
  const location = window.location;
  let pathname: string;
  try {
    pathname = location.pathname;
  } catch {
    pathname = "/";
  }
  let search: string;
  try {
    search = location.search;
  } catch {
    search = "";
  }
  let hadQueryMarker = false;
  try {
    hadQueryMarker = (location.href.split("#")[0] ?? "").includes("?");
  } catch {
    // 读取 href 失败时与 Rust 一样按原始地址没有 query 标记处理。
  }
  return new InitialPublicLocation(pathname, search, hadQueryMarker);
}

/** 路由入口提供；查询 hook 与登录返回地址在 Router 改写地址前读取它。 */
export const InitialPublicLocationContext = createContext<InitialPublicLocation | null>(null);

/** 浏览器 pathname/query 变化版本；查询驱动页面通过独立类型订阅。 */
export interface PublicLocationRevision {
  readonly revision: Cell<number>;
}

/** 地址栏变化版本上下文；没有提供方时查询 hook 只依赖 Router 的完整位置读取。 */
export const PublicLocationRevisionContext = createContext<PublicLocationRevision | null>(null);

/** 当前地址栏 pathname；非浏览器环境恒为 `/`。 */
export function currentPublicPathname(): string {
  if (typeof window === "undefined") {
    return "/";
  }
  try {
    return window.location.pathname;
  } catch {
    return "/";
  }
}

export function currentBrowserSearch(): string {
  if (typeof window === "undefined") {
    return "";
  }
  try {
    return window.location.search;
  } catch {
    return "";
  }
}

/** 当前地址片段；读取失败时按无片段处理。 */
export function currentBrowserHash(): string {
  if (typeof window === "undefined") {
    return "";
  }
  try {
    return window.location.hash;
  } catch {
    return "";
  }
}

/** 只编码本次写入的键值对，保留其余 URL 查询片段的原始字节和顺序。 */
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
          // 只解码键来识别等价编码，未触及的片段不经重新序列化。
          const key = new URLSearchParams(`key=${encodedKey}`).get("key");
          return key === null || !keys.includes(key);
        });
  if (pairs.length !== 0) {
    parts.push(queryPairsString(pairs));
  }
  const search = parts.length === 0 ? "" : `?${parts.join("&")}`;
  try {
    const { pathname, hash } = window.location;
    // 原始查询串直接交给路由；条目标识与快照只随成功写入更新。
    return writeLocationPreservingScroll(`${pathname}${search}${hash}`, mode);
  } catch {
    return false;
  }
}

/** 按键写回并同步已应用查询；地址写入成功才让页面读取新状态。 */
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
  return true;
}
