// 页面需要保留查询解析器省略的原文时，跟踪真实地址栏的搜索段。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { useRouter } from "@auction/web-shared/router";

import { currentBrowserSearch } from "../query/location";

export { updatePublicQueryPairs, writePublicQueryPatch } from "../query/location";

/** 读取地址栏原文，包含解析后查询状态无法区分的重复或非法条件。 */
export function usePublicQuerySearch(): () => string {
  const router = useRouter();
  return createValueMemo(() => {
    router.currentLocationString();
    return currentBrowserSearch();
  });
}
