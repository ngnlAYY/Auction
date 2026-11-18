// 拍品直达链接：被超提醒等站外入口用 `/lots/{拍品 UUID}` 指向一件拍品。
// 独立的拍品页已移除，这里把链接解析成目录位置（世界、场次、分类与页码），跳到看板后打开
// 该拍品的详情抽屉。请求只是一次性的展示意图：不写 URL、不创建命令，也不判定出价资格——
// 目录里有这一行时由行自己打开抽屉（出价入口照常由行内操作栏投影），找不到时退回只读抽屉。

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, ok, type Result } from "@auction/web-shared/result";
import { type RouteMatch, useRouter } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createContext, createEffect, Match, Switch, untrack, useContext } from "solid-js";

import type { CatalogRow } from "../../../api/model";
import { catalogRowFromDetail } from "../../../api/model";
import { fetchCurrentAuction, fetchHistoricalAuction } from "../../../api/queries/current";
import { fetchLotDetail, type LotDetailFetchError } from "../../../api/queries/lots";
import { PublicRefreshGenerationContext } from "../../board/refresh";
import { useContentLoading } from "../../loading";
import type { PublicBoardQuery } from "../../query";
import { normalizeWorldQuery, publicBoardQueryDefault, publicHref } from "../../query";
import { catalogCategoryKey } from "../presentation";

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

/** 读出拍品所在位置后跳到看板；临时失败时允许在原解析范围内重试。 */
export function LotDeepLink(props: { readonly lotId: string }): JSX.Element {
  const request = useContext(LotViewRequestContext);
  const location = useContext(LotLinkLocationContext);
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const router = useRouter();
  // 同一 UUID 的普通深链也有独立的路由实例，不能沿用上次冻结位置。
  const lotId = props.lotId;
  const details = createTaskResource(
    () => {
      const frozen = location();
      return frozen?.lotId === lotId && frozen.route === router.current() ? frozen : null;
    },
    // 目录定位可能等多次请求。代号按详情开始读取时取样。
    (frozen, context) => resolve(lotId, frozen, refreshGeneration.peek(), context.signal),
  );
  const navigationFailed = createCell(false);
  useContentLoading(() => details.state() === "pending");
  createEffect(
    () => (details.state() === "ready" ? details() : undefined),
    (resolved) => {
      if (resolved?.ok !== true) return;
      const { target, query } = resolved.value;
      const href = publicHref("/", query);
      router.replace(href);
      if (untrack(router.currentLocationString) !== href) {
        navigationFailed.set(true);
        return;
      }
      // 只有看板落点写入成功后才交接；同一路径的其他历史条目不能打开它。
      request.set({ entryKey: untrack(router.currentEntryKey), target });
    },
  );
  const failure = () => {
    const detail = details();
    if (navigationFailed()) return "lot_detail_fetch_error";
    return details.state() === "ready" && detail?.ok === false ? detail.error.kind : null;
  };
  return (
    <Switch>
      <Match when={failure() === "not_found"}>
        <AsyncState
          kind="empty"
          title={t("catalog-detail")}
          detail={t("catalog-detail-not-found")}
        />
      </Match>
      <Match when={failure() === "lot_detail_fetch_error"}>
        <AsyncState
          kind="error"
          title={t("catalog-detail")}
          detail={t("data-error")}
          retryLabel={t("data-retry")}
          onRetry={() => {
            navigationFailed.set(false);
            details.restart();
          }}
        />
      </Match>
    </Switch>
  );
}

async function readLot(lotId: string, signal: AbortSignal) {
  const page = await fetchLotDetail(lotId, signal);
  if (!page.ok) return page;
  const { auction_id: auctionId, lot } = page.value;
  return lot === null
    ? err({ kind: "lot_detail_fetch_error" } satisfies LotDetailFetchError)
    : ok({ auctionId, row: catalogRowFromDetail(lot) });
}

/** 读出拍品及其场次，算出看板应当落到的目录位置。
 *
 * 直达链接多由被超提醒生成，指向某个世界的当前场次：拍品与两个世界的当前场次并行读取
 * （当前场次经缓存层与外壳的在途读取合并），命中即可定位，不再串行查场次目录。 */
async function resolve(
  lotId: string,
  frozen: LotLinkLocation | null,
  refreshGeneration: number,
  signal: AbortSignal,
): Promise<
  Result<{ readonly target: LotViewTarget; readonly query: PublicBoardQuery }, LotDetailFetchError>
> {
  if (frozen !== null) {
    const lot = await readLot(lotId, signal);
    if (!lot.ok) return lot;
    const { auctionId, row } = lot.value;
    if (frozen.auctionId !== auctionId) return err({ kind: "lot_detail_fetch_error" });
    return ok({
      target: { lotId, auctionId, world: frozen.world, row, refreshGeneration, rowReady: false },
      query: catalogPosition(row, frozen.world, auctionId, publicBoardQueryDefault()),
    });
  }
  const [page, main, isekai] = await Promise.all([
    readLot(lotId, signal),
    fetchCurrentAuction("main", signal),
    fetchCurrentAuction("isekai", signal),
  ]);
  if (!page.ok) return page;
  if (signal.aborted) return err({ kind: "lot_detail_fetch_error" });
  const { auctionId, row } = page.value;
  // 进行中的场次落到该世界的首页（不带场次参数），与用户平时看到的看板一致。
  const world = currentWorld(auctionId, [main, isekai]);
  if (world !== null) {
    return ok({
      target: { lotId, auctionId, world, row, refreshGeneration, rowReady: false },
      query: catalogPosition(row, world, null, publicBoardQueryDefault()),
    });
  }
  // 不是当前场次，或当前场次读取失败：按往期场次处理，指定场次的看板同样可以出价。
  // 拍品响应不带世界。读不出场次时仍回到看板并保留请求：看板展示的不会是这个场次，
  // 于是按只读抽屉打开，链接不至于落空。
  const auction = await fetchHistoricalAuction(auctionId, signal);
  if (!auction.ok) {
    return ok({
      target: { lotId, auctionId, world: null, row, refreshGeneration, rowReady: false },
      query: publicBoardQueryDefault(),
    });
  }
  return ok({
    target: {
      lotId,
      auctionId,
      world: auction.value.world,
      row,
      refreshGeneration,
      rowReady: false,
    },
    query: catalogPosition(row, auction.value.world, auctionId, publicBoardQueryDefault()),
  });
}

/** 拍品所在场次是哪个世界的当前场次。读取失败的世界不参与判定；都不命中（含读取失败）时
 * 返回 `null`，由调用方按往期场次查找。仅供测试使用。 */
export function currentWorld(
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
 * 每页条数沿用 `base` 里的本地偏好。仅供测试使用。 */
export function catalogPosition(
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
 * 第 ⌈n / 每页条数⌉ 页。撤拍留下的空号会让实际位置靠前，那时看板找不到这一行，退回只读抽屉。
 * 仅供测试使用。 */
export function catalogPage(code: string, pageSize: number): number | null {
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
