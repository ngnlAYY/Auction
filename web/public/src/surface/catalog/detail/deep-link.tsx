// 拍品直达链接：被超提醒等站外入口用 `/lots/{拍品 UUID}` 指向一件拍品。
// 独立的拍品页已移除，这里把链接解析成目录位置（世界、场次、分类与页码），跳到看板后打开
// 该拍品的详情抽屉。请求只是一次性的展示意图：不写 URL、不创建命令，也不判定出价资格——
// 目录里有这一行时由行自己打开抽屉（出价入口照常由行内操作栏投影），找不到时退回只读抽屉。

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import type { Result } from "@auction/web-shared/result";
import { useRouter } from "@auction/web-shared/router";
import type { JSX } from "@solidjs/web";
import { createContext, createEffect, useContext } from "solid-js";

import type { CatalogRow } from "../../../api/model";
import { catalogRowFromDetail } from "../../../api/model";
import { fetchCurrentAuction, fetchHistoricalAuction } from "../../../api/queries/current";
import { fetchLotDetail } from "../../../api/queries/lots";
import { useContentLoading } from "../../loading";
import type { PublicBoardQuery } from "../../query";
import { normalizeWorldQuery, publicBoardQueryDefault, publicHref } from "../../query";
import { catalogCategoryKey } from "../presentation";

/** 一次直达请求要查看的拍品。`auctionId` 来自拍品响应本身，是看板判断“当前展示的是不是
 * 这件拍品所在场次”的依据。 */
export interface LotViewTarget {
  readonly lotId: string;
  readonly auctionId: string;
}

/** 外壳持有的直达请求：解析页写入，看板处置一次后清空。它跨路由存活，但不进入 URL，
 * 刷新或后退都不会重新弹出抽屉。 */
export type LotViewRequest = Cell<LotViewTarget | null>;

/** 直达请求所在上下文；没有提供方时 useContext 会直接报错。 */
export const LotViewRequestContext = createContext<LotViewRequest>();

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
 * `urlWorld` 与 `displayedWorld` 不一致说明外壳的世界选择还没跟上地址栏，此时展示的是
 * 上一个世界的场次，不能据此下结论。`pageLots` 是已与当前筛选、页码对齐的目录页；
 * 读出之前一律等待。
 */
export function disposeLotView(
  target: LotViewTarget,
  urlWorld: string,
  displayedWorld: string,
  displayedAuctionId: string,
  pageLots: Iterable<string> | null,
): LotViewDisposition {
  if (urlWorld !== displayedWorld) {
    return "wait";
  }
  if (target.auctionId !== displayedAuctionId) {
    return "read_only";
  }
  if (pageLots === null) {
    return "wait";
  }
  for (const lotId of pageLots) {
    if (lotId === target.lotId) {
      return "row";
    }
  }
  return "read_only";
}

/** `/lots/{拍品 UUID}` 的解析页：读出拍品所在位置后无历史记录地跳到看板。
 * 读取失败（拍品不存在、网络错误）同样回到看板，只是不带直达请求。 */
export function LotDeepLink(props: { readonly lotId: string }): JSX.Element {
  const request = useContext(LotViewRequestContext);
  const router = useRouter();
  // 解析期间正文为空，加载提示交给顶栏。
  useContentLoading(() => true);
  const tasks = createTaskScope();
  const lotId = createValueMemo(() => props.lotId);
  // 以拍品为依赖：同一组件实例换了参数时重新解析，而不是沿用上一件的结果。
  createEffect(
    () => lotId(),
    (lotId) => {
      tasks.spawn(async (context) => {
        const resolved = await resolve(lotId, context.signal);
        if (!context.alive()) {
          return;
        }
        const href = resolved === null ? "/" : publicHref("/", resolved.query);
        if (resolved !== null) {
          request.set(resolved.target);
        }
        router.replace(href);
      });
    },
  );
  return null;
}

/** 读出拍品及其场次，算出看板应当落到的目录位置。
 *
 * 直达链接多由被超提醒生成，指向某个世界的当前场次：拍品与两个世界的当前场次并行读取
 * （当前场次经缓存层与外壳的在途读取合并），命中即可定位，不再串行查场次目录。 */
async function resolve(
  lotId: string,
  signal: AbortSignal,
): Promise<{ readonly target: LotViewTarget; readonly query: PublicBoardQuery } | null> {
  const [page, main, isekai] = await Promise.all([
    fetchLotDetail(lotId, signal),
    fetchCurrentAuction("main", signal),
    fetchCurrentAuction("isekai", signal),
  ]);
  if (signal.aborted || !page.ok) {
    return null;
  }
  const auctionId = page.value.auction_id;
  const item = page.value.lot;
  if (item === null) {
    return null;
  }
  const row = catalogRowFromDetail(item);
  const target: LotViewTarget = { lotId, auctionId };
  // 进行中的场次落到该世界的首页（不带场次参数），与用户平时看到的看板一致。
  const world = currentWorld(auctionId, [main, isekai]);
  if (world !== null) {
    return { target, query: catalogPosition(row, world, null, publicBoardQueryDefault()) };
  }
  // 不是当前场次，或当前场次读取失败：按往期场次处理，指定场次的看板同样可以出价。
  // 拍品响应不带世界。读不出场次时仍回到看板并保留请求：看板展示的不会是这个场次，
  // 于是按只读抽屉打开，链接不至于落空。
  const auction = await fetchHistoricalAuction(auctionId, signal);
  if (signal.aborted) {
    return null;
  }
  if (!auction.ok) {
    return { target, query: publicBoardQueryDefault() };
  }
  return {
    target,
    query: catalogPosition(row, auction.value.world, auctionId, publicBoardQueryDefault()),
  };
}

/** 拍品所在场次是哪个世界的当前场次。读取失败的世界不参与判定；都不命中（含读取失败）时
 * 返回 `null`，由调用方按往期场次查找。 */
function currentWorld(
  auctionId: string,
  currents: Iterable<Result<CurrentAuctionItemResponse | null, string>>,
): string | null {
  for (const current of currents) {
    if (current.ok && current.value !== null && current.value.id === auctionId) {
      return current.value.world;
    }
  }
  return null;
}

/** 拍品在目录里的位置：所在世界、（往期）场次、分类筛选与页码。其余筛选保持默认，
 * 每页条数沿用 `base` 里的本地偏好。 */
function catalogPosition(
  row: CatalogRow,
  world: string,
  historicalAuctionId: string | null,
  base: PublicBoardQuery,
): PublicBoardQuery {
  const category = catalogCategoryKey(row.category);
  // “其他”不是可筛选的分类：不带分类时目录按全场序号排列，分类内序号推不出页码。
  const categoryFilter = category === "other" ? null : category;
  const page = category === "other" ? 1 : (catalogPage(row.code, base.pageSize) ?? 1);
  const normalizedWorld = normalizeWorldQuery(world);
  return {
    ...base,
    // 主世界沿用无参数 URL。
    world: normalizedWorld === null || normalizedWorld === "main" ? null : normalizedWorld,
    auctionId: historicalAuctionId,
    category: categoryFilter,
    page,
  };
}

/** 按分类筛选后拍品所在的页码。编号末尾的数字是分类内序号，目录按同一顺序排列，第 n 件在
 * 第 ⌈n / 每页条数⌉ 页。撤拍留下的空号会让实际位置靠前，那时看板找不到这一行，退回只读抽屉。 */
function catalogPage(code: string, pageSize: number): number | null {
  let index = 0;
  while (index < code.length && !isAsciiDigit(code.charCodeAt(index))) {
    index += 1;
  }
  const digits = code.slice(index);
  if (digits.length === 0) {
    return null;
  }
  for (let digitIndex = 0; digitIndex < digits.length; digitIndex += 1) {
    if (!isAsciiDigit(digits.charCodeAt(digitIndex))) {
      return null;
    }
  }
  // Rust 用 i64 解析完整编号并以 i64 计算页码；用 BigInt 保留这段整数语义，避免超过
  // Number.MAX_SAFE_INTEGER 的合法编号被误判成非法。pageSize 在调用方来自本地分页偏好，
  // 是安全的整数；最终页码仍要落到 PublicBoardQuery 的 number 字段。
  let ordinal = 0n;
  for (let digitIndex = 0; digitIndex < digits.length; digitIndex += 1) {
    ordinal = ordinal * 10n + BigInt(digits.charCodeAt(digitIndex) - 0x30);
  }
  if (
    ordinal < 1n ||
    ordinal > 9223372036854775807n ||
    !Number.isSafeInteger(pageSize) ||
    pageSize <= 0
  ) {
    return null;
  }
  const page = (ordinal - 1n) / BigInt(pageSize) + 1n;
  // Rust 的 i64 页码最终要写入 PublicBoardQuery 的 number 字段；浏览器只能精确表示安全整数。
  // 超出时按源码解析失败的同一条“无法定位”分支处理，不生成被舍入的 URL。
  if (page > BigInt(Number.MAX_SAFE_INTEGER)) {
    return null;
  }
  return Number(page);
}

function isAsciiDigit(code: number): boolean {
  return code >= 0x30 && code <= 0x39;
}
