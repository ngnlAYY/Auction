// 导航测试的 Provider 装配；组件通过真实 RouterContext 读取路由，避免上下文类型断言。

import type { Router } from "@auction/web-shared/router";
import { RouterContext } from "@auction/web-shared/router";
import type { RouteMatch } from "@auction/web-shared/router/pattern";
import type { JSX } from "@solidjs/web";

import { NavigationLinks } from "./index";

/** 把侧栏放进真实路由上下文；调用方负责挂载与清理测试文档。 */
export function NavigationHarness(props: {
  readonly router: Router;
  readonly currentRoute: RouteMatch;
}): JSX.Element {
  return (
    <RouterContext value={props.router}>
      <NavigationLinks currentRoute={props.currentRoute} />
    </RouterContext>
  );
}
