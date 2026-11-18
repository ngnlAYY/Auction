// 路由对象与上下文：维护当前匹配和实际地址快照，并提供 push/replace/goBack。
// 查询写入同步实际地址快照，路由导航和历史遍历另行更新路由匹配。

import { createContext, onCleanup, useContext } from "solid-js";

import { createCell } from "../reactive/cell";
import { createWebHistory, writeLocationPreservingScroll } from "./history";
import { watchLocationWritten } from "./location-notice";
import type { RouteDefinition, RouteMatch } from "./pattern";
import { compileRoutes, isInternalHref, resolveRoute } from "./pattern";

/** 客户端路由对象。 */
export interface Router {
  /** 响应式读取当前匹配；未命中时返回 not-found 与解码后的路径段。 */
  current(): RouteMatch;
  /**
   * 响应式读取地址栏当前快照，内容为 pathname + search + hash。
   * 路由初始化、浏览器导航以及成功的路由和查询写入都会刷新本值。
   */
  currentLocationString(): string;
  /** 新增历史记录并同步当前路由。 */
  push(href: string): void;
  /** 替换当前历史记录并同步当前路由。 */
  replace(href: string): void;
  /** 返回上一条历史记录；路由在浏览器发出 popstate 时更新。 */
  goBack(): void;
  /** 页面提交后调用；定位仍属于当前导航的片段或历史滚动位置。 */
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
 * 只规范化已匹配路由的 pathname，原查询和片段随地址保留。
 * 路由导航和历史遍历才需要重新匹配页面；同页查询写入只刷新地址快照。
 */
export function createRouter(options: RouterOptions): Router {
  const routes = compileRoutes(options.routes);
  const browserHistory = createWebHistory({
    onLocationChanged: () => updateFromLocation(),
  });
  const canonicalLocation = (href: string): string => {
    const resolved = resolveRoute(routes, href);
    // 协议相对地址或含路径分隔符的解码结果不能被改写成另一种导航。
    return resolved.match.kind === "matched" &&
      !href.startsWith("//") &&
      !resolved.routeString.startsWith("//")
      ? resolved.routeString
      : href;
  };
  const initial = browserHistory.currentRoute();
  const canonical = canonicalLocation(initial);
  if (canonical !== initial) {
    writeLocationPreservingScroll(canonical, "replace");
  }
  const currentMatch = createCell(resolveRoute(routes, browserHistory.currentRoute()).match);
  const currentLocation = createCell(browserHistory.currentRoute());
  // 路由和查询共用写入通知，查询变化时可保留已经挂载的页面。
  const stopWatchingLocation = watchLocationWritten(() => {
    currentLocation.set(browserHistory.currentRoute());
  });
  onCleanup(stopWatchingLocation);

  const updateFromLocation = (): void => {
    const location = browserHistory.currentRoute();
    currentMatch.set(resolveRoute(routes, location).match);
    currentLocation.set(location);
  };

  const navigate = (href: string, mode: "push" | "replace"): void => {
    if (!isInternalHref(href)) {
      navigateExternal(href);
      return;
    }
    if (browserHistory.navigate(canonicalLocation(href), mode)) {
      currentMatch.set(resolveRoute(routes, browserHistory.currentRoute()).match);
    }
  };

  return {
    current: () => currentMatch(),
    currentLocationString: () => currentLocation(),
    push: (href) => navigate(href, "push"),
    replace: (href) => navigate(href, "replace"),
    goBack: () => browserHistory.goBack(),
    restoreScrollAfterCommit: () => browserHistory.restoreScrollAfterCommit(),
  };
}

/** 读取最近提供方提供的路由对象。 */
export function useRouter(): Router {
  return useContext(RouterContext);
}
