// 拍品直达交接：外壳、详情页与看板通过同一组上下文传递一次性请求。

import type { Cell } from "@auction/web-shared/reactive/cell";
import type { RouteMatch } from "@auction/web-shared/router";
import { createContext } from "solid-js";

import type { CatalogRow } from "../../../api/model";

/** 一次直达请求携带已读详情，只有对齐后的目录行可以接管它。 */
export interface LotViewTarget {
  readonly lotId: string;
  readonly auctionId: string;
  readonly world: string | null;
  readonly row: CatalogRow;
  readonly refreshGeneration: number;
  readonly rowReady: boolean;
}

/** 外壳内存中的直达请求，只能由解析后落到的看板历史条目消费一次。 */
export interface LotViewRequest {
  readonly entryKey: string;
  readonly target: LotViewTarget;
}

/** 直达请求所在上下文；没有提供方时 useContext 会直接报错。 */
export const LotViewRequestContext = createContext<Cell<LotViewRequest | null>>();

/** “查看拍品”此次解析所用的冻结位置，只在外壳内存里交接。 */
export interface LotLinkLocation {
  readonly lotId: string;
  readonly auctionId: string;
  readonly world: string;
  readonly route: RouteMatch;
}

/** 冻结位置绑定到发起解析的导航，允许同次解析失败后重试。 */
export const LotLinkLocationContext = createContext<Cell<LotLinkLocation | null>>();

/** 看板对直达请求的处置。 */
export type LotViewDisposition =
  /** 看板还没落到目标位置（世界仍在同步，或目录页尚未读出）：保留请求。 */
  | "wait"
  /** 目录当前页有这一行：交给行自己打开抽屉，出价与标记入口照常可用。 */
  | "row"
  /** 看板展示的不是这件拍品所在的场次，或这一页没有它：退回只读抽屉。 */
  | "read_only";

/** 判定当前看板如何处置直达请求。
 *
 * 仍在切换世界或等待对齐目录时留给看板；读取终止后没有对应行时复用已读详情。
 */
export function disposeLotView(
  target: LotViewTarget,
  urlWorld: string,
  displayed: { readonly world: string; readonly id: string } | null,
  reading: boolean,
  pageLots: Iterable<string>,
): LotViewDisposition {
  if (target.world === null) return "read_only";
  if (target.world !== urlWorld || (displayed !== null && displayed.world !== urlWorld)) {
    return reading ? "wait" : "read_only";
  }
  if (displayed !== null && displayed.id !== target.auctionId) {
    return "read_only";
  }
  for (const lotId of pageLots) {
    if (lotId === target.lotId) {
      return "row";
    }
  }
  return reading ? "wait" : "read_only";
}
