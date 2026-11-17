// 地址栏查询读取、首次地址快照与位置变化版本上下文。

import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createContext } from "solid-js";

import { type PublicBoardQuery, publicBoardQueryDefault } from "./board";
import { parsePublicQuery } from "./parse";

/** 可提供首次 query 的地址快照；路由首次挂载后该快照会失效。 */
export interface InitialPublicSearch {
  searchFor(pathname: string): string | null;
}

// 查询状态直接改 History API 时 Router 不会收到通知；登录返回地址等依赖地址栏的响应式
// 计算需要一个覆盖这些写入的版本信号，不能只依赖 Router 的当前位置。
const browserLocationRevision = createCell(0);

/** 标记浏览器地址已由本模块直接改写；Router 导航另有自己的位置信号。 */
export function markBrowserLocationChanged(): void {
  browserLocationRevision.update((revision) => Math.min(revision + 1, Number.MAX_SAFE_INTEGER));
}

/** 响应式读取地址栏直接写入的版本；在 memo、effect 或 JSX 中调用会订阅后续变化。 */
export function readBrowserLocationRevision(): number {
  return browserLocationRevision();
}

/** 读取当前地址栏查询；首次挂载时可恢复 Router 尚未声明的 query。
 *
 * 恢复只在 `replaceState` 成功后才采用初始 search：写地址失败时解析到的仍是当前空查询，
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
    if (initialSearch !== null) {
      // 恢复原 query 时保留当前片段与 history.state，避免破坏锚点和滚动位置。
      try {
        const history = window.history;
        let state: unknown = null;
        try {
          state = history.state;
        } catch {
          // state 读取失败时 Rust 使用 null，但仍继续恢复 query。
        }
        let hash: string;
        try {
          hash = window.location.hash;
        } catch {
          hash = "";
        }
        history.replaceState(state, "", `${pathname}${initialSearch}${hash}`);
        search = initialSearch;
      } catch {
        // 地址栏不可写时保持当前空 query，与 Rust 忽略 replace 失败一致。
      }
    }
  }
  return parsePublicQuery(pathname, search);
}

/** Router 首次挂载前的公开端地址快照，用于恢复未在路由表中声明的查询参数。 */
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
    let hash: string;
    try {
      hash = location.hash;
    } catch {
      hash = "";
    }
    const url = `${this.pathname}${hash}`;
    try {
      const history = window.history;
      let state: unknown = null;
      try {
        state = history.state;
      } catch {
        // state 读取失败时 Rust 使用 null，但仍清理地址栏。
      }
      history.replaceState(state, "", url);
    } catch {
      // Rust 明确忽略清理 Router 空 query 标记时的地址栏写入失败。
    }
  }
}

/** 在 Router 规范化未声明的 query 前保存首次 pathname 与 search。 */
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

/** 读取当前滚动位置作为历史状态；任一轴读取失败时 Rust 使用 0。 */
export function browserScrollState(): [number, number] {
  let scrollX = 0;
  let scrollY = 0;
  try {
    scrollX = window.scrollX;
  } catch {
    // Rust 对 X 滚动位置读取失败使用 0，不影响历史写入。
  }
  try {
    scrollY = window.scrollY;
  } catch {
    // Y 滚动位置读取失败同样使用 0。
  }
  return [scrollX, scrollY];
}
