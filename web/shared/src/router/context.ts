// 路由对象与上下文：维护当前匹配和实际地址快照，并提供 push/replace/goBack。
// 当前路由只在初始化、push、replace 与 popstate 时更新；绕过 Router 的 History 写入
// 只能经 notifyLocationWritten() 刷新地址快照，不能改变当前匹配。

import { createContext, onCleanup, useContext } from "solid-js";

import { createCell } from "../reactive/cell";
import type { ScrollPosition } from "./history";
import { createWebHistory } from "./history";
import { watchLocationWritten } from "./location-notice";
import type { RouteDefinition, RouteMatch } from "./pattern";
import { compileRoutes, isInternalHref, resolveRoute } from "./pattern";

/** 客户端路由对象。 */
export interface Router {
  /** 响应式读取当前匹配；未命中时返回 not-found 与解码后的路径段。 */
  current(): RouteMatch;
  /**
   * 响应式读取地址栏当前快照，内容为 pathname + search + hash。
   * 路由初始化、push、replace、popstate 与直接写 History 后调用 notifyLocationWritten()
   * 都会刷新本值；直接写 History 却不通知时保持旧快照。
   */
  currentLocationString(): string;
  /** 新增历史记录并同步当前路由。 */
  push(href: string): void;
  /** 替换当前历史记录并同步当前路由。 */
  replace(href: string): void;
  /** 返回上一条历史记录；路由在浏览器发出 popstate 时更新。 */
  goBack(): void;
  /** 页面提交后调用；恢复仍属于当前导航的 popstate 滚动位置。 */
  restoreScrollAfterCommit(): void;
}

/** 创建路由对象的路由表。 */
export interface RouterOptions {
  readonly routes: readonly RouteDefinition[];
}

/** 路由对象所在上下文；没有提供方时 useContext 会直接报错。 */
export const RouterContext = createContext<Router>();

function navigateExternal(href: string): void {
  try {
    window.location.assign(href);
  } catch {
    // Dioxus 对外部导航失败同样只返回 false；函数签名没有错误通道。
    return;
  }
}

/**
 * 创建路由对象。
 *
 * 初始化按 Dioxus 的规则解析当前地址；规范路由串与地址不同时执行一次 replace，
 * 因此未声明的查询参数与 hash 会在初始化时被规范化掉，应用层需要自行保存初始查询快照。
 * 当前匹配之后只在 push/replace/popstate 时重新解析；地址快照还可由
 * notifyLocationWritten() 单独刷新，但不会因此重新匹配。
 */
export function createRouter(options: RouterOptions): Router {
  const routes = compileRoutes(options.routes);
  let navigationVersion = 0;
  let pendingScrollRestore: {
    readonly version: number;
    readonly position: ScrollPosition;
  } | null = null;
  const browserHistory = createWebHistory({
    onPopState: (position) => {
      updateFromLocation();
      if (position !== null) {
        pendingScrollRestore = { version: navigationVersion, position };
      }
    },
  });
  const initial = resolveRoute(routes, browserHistory.currentRoute());
  const currentMatch = createCell(initial.match);
  const currentLocation = createCell(browserHistory.currentRoute());
  // 应用层写查询参数不经过路由匹配；通知只同步地址快照，让 Link 的当前页判定跟随地址栏。
  const stopWatchingLocation = watchLocationWritten(() => {
    currentLocation.set(browserHistory.currentRoute());
  });
  onCleanup(stopWatchingLocation);

  const updateFromLocation = (): void => {
    navigationVersion += 1;
    pendingScrollRestore = null;
    const resolved = resolveRoute(routes, browserHistory.currentRoute());
    currentMatch.set(resolved.match);
    currentLocation.set(browserHistory.currentRoute());
  };

  if (initial.routeString !== browserHistory.currentRoute()) {
    browserHistory.replace(initial.routeString);
    updateFromLocation();
  }

  const push = (href: string): void => {
    if (!isInternalHref(href)) {
      navigateExternal(href);
      return;
    }
    browserHistory.push(href);
    updateFromLocation();
  };

  const replace = (href: string): void => {
    if (!isInternalHref(href)) {
      navigateExternal(href);
      return;
    }
    browserHistory.replace(href);
    updateFromLocation();
  };

  const restoreScrollAfterCommit = (): void => {
    const pending = pendingScrollRestore;
    if (pending === null) {
      return;
    }
    pendingScrollRestore = null;
    browserHistory.restoreScrollAfterCommit(
      pending.position,
      () => pending.version === navigationVersion,
    );
  };

  return {
    current: () => currentMatch(),
    currentLocationString: () => currentLocation(),
    push,
    replace,
    goBack: () => browserHistory.goBack(),
    restoreScrollAfterCommit,
  };
}

/** 读取最近提供方提供的路由对象。 */
export function useRouter(): Router {
  return useContext(RouterContext);
}
