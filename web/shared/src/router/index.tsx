// 共享路由模块入口：路由表、匹配上下文、Link 与 RouteOutlet 的公共导出。

export type { Router, RouterOptions } from "./context";
export { createRouter, RouterContext, useRouter } from "./context";
export type { LinkProps } from "./link";
export { Link } from "./link";
export type {
  RouteComponentPage,
  RouteLazyPage,
  RouteOutletProps,
  RoutePage,
  RoutePageComponent,
  RoutePageLoader,
  RoutePageProps,
} from "./outlet";
export { lazyPage, pageComponent, RouteOutlet } from "./outlet";
export type {
  MatchedRoute,
  NotFoundRoute,
  RouteDefinition,
  RouteMatch,
  RouteParams,
  RouteQuery,
} from "./pattern";
