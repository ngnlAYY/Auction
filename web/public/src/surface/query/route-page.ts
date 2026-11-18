// 自动纠偏前的页面路由归属判断：旧页面在新地址下被唤醒时，只能写回仍属于自己的地址。

import type { RouteMatch } from "@auction/web-shared/router";
import { useRouter } from "@auction/web-shared/router";
import { untrack } from "solid-js";

/** 当前路由匹配是否属于指定页面；未命中任何路由时不属于任何页面。 */
export function routeBelongsToPage(route: RouteMatch, page: string): boolean {
  return route.kind === "matched" && route.name === page;
}

/**
 * 创建“当前路由仍属于本页”的判断。
 *
 * 返回的函数以 `untrack` 读取 Router 的最新匹配：调用点位于 effect 的 apply 阶段，
 * 不能订阅路由，也不能把路由变化当成该 effect 的依赖。自动纠偏在写地址栏前用它确认
 * 页面没有被前进/后退换出，避免旧页面把自己的查询参数写进新地址。
 */
export function usePageRouteGuard(page: string): () => boolean {
  const router = useRouter();
  return () => untrack(() => routeBelongsToPage(router.current(), page));
}
