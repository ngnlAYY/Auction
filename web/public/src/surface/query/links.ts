// Public 查询链接生成与历史导航。

import { useRouter } from "@auction/web-shared/router";
import { createEffect } from "solid-js";

import { type PublicBoardQuery, publicBoardQueryDefault } from "./board";
import { currentBrowserHash, currentBrowserSearch } from "./location";
import { publicQueryString } from "./serialization";

export { setPublicQuery } from "./state";

export function publicHref(path: string, query: PublicBoardQuery): string {
  return `${path}${publicQueryString(query)}`;
}

/** 生成首页查询；`world` 是导航实际继承的地址值，`null` 表示省略并采用主世界缺省。
 * 只保留目标世界与本地数量偏好，避免从其他页面带回不属于首页的筛选和页码。 */
export function publicAuctionHomeQuery(world: string | null): PublicBoardQuery {
  return {
    ...publicBoardQueryDefault(),
    world,
  };
}

/** 把旧版公开端别名无历史记录地收敛到规范路径，并保留查询与片段。 */
export function redirectPublicAlias(path: string): null {
  const router = useRouter();
  const target = `${path}${currentBrowserSearch()}${currentBrowserHash()}`;
  createEffect(
    () => null,
    () => {
      router.replace(target);
    },
  );
  return null;
}
