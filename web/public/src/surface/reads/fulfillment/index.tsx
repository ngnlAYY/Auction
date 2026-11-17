// 公开履约进度页：按已结算场次逐件显示结算后的流程状态（流拍寄回、COD 付款与退回、异世界
// 交付、卖家货款与成交撤销）。
//
// 状态由服务端闭集给出，本页只把状态映射为文案与语气样式，不重建判定；付款期限与 COD 退回
// 次数只在服务端给出时显示。世界/场次选择、目录门禁与请求身份绑定沿用竞价日志页。重读失败时
// 缓存层回退的旧快照会连同其确认时刻一起提示，不把旧状态当作当前结果静默显示。

import type { AuctionSummaryResponse } from "@auction/contracts/dto/public/catalog/auction";
import { sleep } from "@auction/web-shared/platform/timer";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { ok, type Result } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, Match, Show, Switch, useContext } from "solid-js";

import type { FulfillmentRead } from "../../../api/fulfillment";
import {
  fetchFulfillmentAuctions,
  fetchFulfillmentPage,
  MAX_FULFILLMENT_PAGE_SIZE,
} from "../../../api/fulfillment";
import { fetchWorlds } from "../../../api/queries/current";
import { PublicRefreshGenerationContext, refreshPublicReads } from "../../board/refresh";
import { supportedWorlds, worldLabel } from "../../board/world";
import {
  LotDetailLookupContext,
  LotDetailLookupHost,
  useLotDetailLookup,
} from "../../catalog/detail/lookup";
import { formatAuctionTime } from "../../catalog/presentation";
import { useContentLoading } from "../../loading";
import { normalizePublicPageSize, publicPageMatches } from "../../pagination";
import {
  currentPublicPathname,
  updatePublicQueryState,
  usePublicQueryState,
  writePublicQuery,
} from "../../query";
import { rustTrim } from "../../query/text";
import { applyLotFilter, resetLotDraft, validLotFilter } from "../bid-log";
import type {
  FulfillmentAuctionsSnapshot,
  FulfillmentBodyState,
  FulfillmentProgressSnapshot,
  FulfillmentRequest,
} from "./model";
import { LOT_FILTER_ERROR_ID, LOT_FILTER_WRITE_DEBOUNCE_MILLIS } from "./model";
import { FulfillmentTable } from "./table";

/** 路由入口。 */
export function Fulfillment(): JSX.Element {
  // 世界/场次/页码/lot 筛选与地址栏双向同步，键与竞价日志共用。
  const queryState = usePublicQueryState();
  const queryStateSignal = queryState.state;
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const worldFilter = createValueMemo(() => {
    const value = queryStateSignal().world ?? "main";
    return value === "main" || value === "isekai" ? value : "main";
  });
  // 空串表示目录读取前尚未选定场次；目录到达后再替换地址栏并重置到第一页。
  const selectedAuctionId = createValueMemo(() => queryStateSignal().auctionId ?? "");
  const pageNumber = createValueMemo(() => queryStateSignal().page);
  // 接口每页最多 100 件；共享偏好或地址栏里更大的数量按上限请求，分页器显示实际数量。
  const pageSize = createValueMemo(() =>
    Math.min(queryStateSignal().pageSize, MAX_FULFILLMENT_PAGE_SIZE),
  );
  const lotFilter = createValueMemo(() => queryStateSignal().bidLogLot ?? "");
  // 输入框绑定草稿；合法草稿在输入停顿后才成为已应用筛选（地址栏的 `lot`）。
  const lotDraft = createCell(lotFilter());
  const lotWriteGeneration = createCell(0);
  const tasks = createTaskScope();

  createEffect(
    () => lotFilter(),
    (applied) => {
      // 地址栏（前进、后退或清除筛选）变更时同步输入框；用户正在输入的空格保留，
      // 否则每次回写都会把尚未提交的边缘空白抹掉。
      if (rustTrim(lotDraft.peek()) !== applied) {
        resetLotDraft(lotDraft, lotWriteGeneration, applied);
      }
    },
  );

  const worlds = createTaskResource(
    () => 0,
    async (_dependencies, context) => fetchWorlds(context.signal),
  );
  const worldOptions = createValueMemo(() => {
    const result = worlds();
    return result?.ok === true ? supportedWorlds(result.value) : [worldFilter()];
  });

  const auctions = createTaskResource(
    // 外壳轮询与人工刷新先重读已结算目录；进度请求随后只消费本轮确认的场次。
    () => [refreshGeneration(), worldFilter()] as const,
    async ([generation, world], context): Promise<FulfillmentAuctionsSnapshot | null> => {
      const result = await fetchFulfillmentAuctions(world, context.signal);
      if (!context.alive()) {
        return null;
      }
      return { world, generation, result };
    },
  );

  const currentAuctions = createValueMemo((): Result<AuctionSummaryResponse[], string> | null => {
    const snapshot = auctions();
    if (snapshot === null || snapshot === undefined) {
      return null;
    }
    return snapshot.world === worldFilter() ? snapshot.result : null;
  });

  // 保留仍在本世界目录里的选择；所选场次不在目录时纠偏到最近一场。
  createEffect(
    () => {
      const snapshot = auctions();
      if (snapshot === null || snapshot === undefined || !snapshot.result.ok) {
        return null;
      }
      // 后台刷新期间保留旧目录；旧目录不能改写新一轮已经确定的场次或页码。
      if (snapshot.world !== worldFilter() || snapshot.generation !== refreshGeneration()) {
        return null;
      }
      const selected = selectedAuctionId();
      const nextAuctionId =
        snapshot.result.value.find((auction) => auction.id === selected)?.id ??
        snapshot.result.value[0]?.id ??
        "";
      return selected === nextAuctionId ? null : nextAuctionId;
    },
    (nextAuctionId) => {
      if (nextAuctionId === null) {
        return;
      }
      updatePublicQueryState(
        queryStateSignal,
        (query) => {
          query.auctionId = nextAuctionId === "" ? null : nextAuctionId;
          query.page = 1;
        },
        "replace",
      );
    },
  );

  const progress = createTaskResource(
    () => {
      const auctionId = selectedAuctionId();
      const page = pageNumber();
      const size = pageSize();
      const world = worldFilter();
      const rawLot = lotFilter();
      const lot = rustTrim(rawLot);
      // 地址栏带入的非法筛选不发请求：服务端只会回 400，格式提示由输入框承担。
      const lotValid = validLotFilter(lot);
      const requestedLot = lot === "" ? null : lot;
      const directory = auctions();
      // 同查询后台刷新保留完成快照；目录/分页切换后旧快照不得装配到新页面。
      // 不直接订阅全局刷新：目录未就绪、失败或已不含该场时不发 GET。
      const readable =
        directory !== null &&
        directory !== undefined &&
        directory.world === world &&
        directory.generation === refreshGeneration.peek() &&
        directory.result.ok &&
        directory.result.value.some((auction) => auction.id === auctionId) &&
        lotValid;
      return {
        auctionId,
        world,
        lot: requestedLot,
        lotFilter: rawLot,
        page,
        pageSize: size,
        readable,
      } as const;
    },
    async (requested, context): Promise<FulfillmentProgressSnapshot | null> => {
      const request: FulfillmentRequest = [
        requested.auctionId,
        requested.world,
        requested.lot,
        requested.page,
        requested.pageSize,
      ];
      if (!requested.readable) {
        return { request, result: ok(null) };
      }
      const result = await fetchFulfillmentPage(
        requested.auctionId,
        requested.world,
        requested.lot,
        requested.page,
        requested.pageSize,
        context.signal,
      );
      if (!context.alive()) {
        return null;
      }
      return { request, result };
    },
  );

  const auctionOptions = createValueMemo((): readonly AuctionSummaryResponse[] => {
    const result = currentAuctions();
    return result?.ok === true ? result.value : [];
  });
  const selectedAuction = createValueMemo((): AuctionSummaryResponse | null => {
    const selected = selectedAuctionId();
    return auctionOptions().find((auction) => auction.id === selected) ?? null;
  });
  const currentResult = createValueMemo((): Result<FulfillmentRead | null, string> | null => {
    const snapshot = progress();
    if (snapshot === null || snapshot === undefined) {
      return null;
    }
    const [auctionId, world, lot, page, size] = snapshot.request;
    if (
      selectedAuction() === null ||
      auctionId !== selectedAuctionId() ||
      world !== worldFilter() ||
      (lot ?? "") !== rustTrim(lotFilter()) ||
      page !== pageNumber() ||
      size !== pageSize()
    ) {
      return null;
    }
    return snapshot.result;
  });

  // 目录或当前查询还没有结果时由顶栏提示加载，正文留空。
  const body = createValueMemo((): FulfillmentBodyState => {
    const auctionResult = currentAuctions();
    const progressResult = currentResult();
    if (
      auctionResult?.ok === false ||
      (auctionResult?.ok === true && progressResult?.ok === false)
    ) {
      return { kind: "error" };
    }
    if (auctionResult?.ok === true && auctionResult.value.length === 0) {
      return { kind: "empty-auctions" };
    }
    // 已应用的筛选非法（来自地址栏）时没有发出请求：正文留空且不提示加载。
    if (auctionResult?.ok === true && !validLotFilter(rustTrim(lotFilter()))) {
      return { kind: "invalid-filter" };
    }
    if (
      auctionResult?.ok === true &&
      progressResult?.ok === true &&
      progressResult.value !== null &&
      publicPageMatches(
        progressResult.value.page.page,
        progressResult.value.page.page_size,
        pageNumber(),
        pageSize(),
      )
    ) {
      return { kind: "table", read: progressResult.value };
    }
    return { kind: "loading" };
  });
  const tableRead = createValueMemo((): FulfillmentRead | null => {
    const current = body();
    return current.kind === "table" ? current.read : null;
  });
  const loading = createValueMemo(() => body().kind === "loading");
  useContentLoading(loading);

  const lookup = useLotDetailLookup();
  const invalidDraft = createValueMemo(() => !validLotFilter(rustTrim(lotDraft())));

  const submitLotFilter = (value: string): void => {
    // 草稿立即跟随输入；非法草稿不发请求，合法草稿停顿后才应用并写回地址栏。
    const lot = rustTrim(value);
    resetLotDraft(lotDraft, lotWriteGeneration, value);
    if (!validLotFilter(lot)) {
      return;
    }
    const generation = lotWriteGeneration.peek();
    tasks.spawn(async (context) => {
      await sleep(LOT_FILTER_WRITE_DEBOUNCE_MILLIS, context.signal);
      if (!context.alive()) {
        return;
      }
      // 仅提交最新草稿；清空按钮、场次切换与已经离开页面的输入都作废本次任务。
      if (lotWriteGeneration.peek() === generation && currentPublicPathname() === "/fulfillment") {
        applyLotFilter(queryStateSignal, lot);
      }
    });
  };

  return (
    <LotDetailLookupContext value={lookup}>
      <section class="public-fulfillment" aria-label={t("page-fulfillment-title")}>
        <div class="public-fulfillment__controls">
          <label for="public-fulfillment-world">
            <span>{t("world-label")}</span>
            <select
              id="public-fulfillment-world"
              name="world"
              autocomplete="off"
              value={worldFilter()}
              disabled={worldOptions().length === 0}
              onChange={(event) => {
                const world = event.currentTarget.value;
                // 切换世界清除场次与 lot 筛选：草稿一并清空，在途的防抖写入作废。
                resetLotDraft(lotDraft, lotWriteGeneration, "");
                updatePublicQueryState(
                  queryStateSignal,
                  (query) => {
                    // 主世界使用缺省 URL，非主世界才写入 `world`。
                    query.world = world !== "main" ? world : null;
                    query.auctionId = null;
                    query.bidLogLot = null;
                    query.page = 1;
                  },
                  "push",
                );
              }}
            >
              <For each={worldOptions()} keyed={false}>
                {(world) => <option value={world()}>{worldLabel(world())}</option>}
              </For>
            </select>
          </label>
          <label for="public-fulfillment-round">
            <span>{t("fulfillment-round")}</span>
            <select
              id="public-fulfillment-round"
              name="auction"
              autocomplete="off"
              disabled={auctionOptions().length === 0}
              value={selectedAuctionId()}
              onChange={(event) => {
                const auctionId = event.currentTarget.value;
                resetLotDraft(lotDraft, lotWriteGeneration, "");
                updatePublicQueryState(
                  queryStateSignal,
                  (query) => {
                    query.auctionId = auctionId;
                    query.bidLogLot = null;
                    query.page = 1;
                  },
                  "push",
                );
              }}
            >
              <For each={auctionOptions()} keyed={false}>
                {(auction) => (
                  <option value={auction().id}>
                    {t("fulfillment-round-option", { number: auction().sequence })}
                  </option>
                )}
              </For>
            </select>
          </label>
        </div>
        <div class="public-fulfillment__heading">
          <Show when={selectedAuction()}>
            {(auction) => (
              <div class="public-fulfillment__round">
                <strong class="public-fulfillment__round-number">
                  {t("fulfillment-round-number", { number: auction().sequence })}
                </strong>
                <span class="public-fulfillment__round-detail">
                  {t("fulfillment-world", { world: worldLabel(auction().world_id) })}
                </span>
                <Show when={auction().settled_at !== null && auction().settled_at !== undefined}>
                  <span class="public-fulfillment__round-detail">
                    {t("fulfillment-settled-at", {
                      date: formatAuctionTime(auction().settled_at ?? ""),
                    })}
                  </span>
                </Show>
              </div>
            )}
          </Show>
          <div class="public-fulfillment__toolbar">
            <label for="public-fulfillment-filter">{t("fulfillment-filter-label")}</label>
            <input
              id="public-fulfillment-filter"
              name="lot"
              autocomplete="off"
              type="search"
              enterkeyhint="search"
              autocapitalize="none"
              spellcheck="false"
              value={lotDraft()}
              placeholder={t("fulfillment-filter-placeholder")}
              aria-invalid={invalidDraft() ? "true" : "false"}
              aria-describedby={invalidDraft() ? LOT_FILTER_ERROR_ID : undefined}
              onInput={(event) => {
                submitLotFilter(event.currentTarget.value);
              }}
            />
            <Show when={lotDraft() !== ""}>
              {/* 清除立即生效，不等防抖。 */}
              <button
                type="button"
                onClick={() => {
                  resetLotDraft(lotDraft, lotWriteGeneration, "");
                  applyLotFilter(queryStateSignal, "");
                }}
              >
                {t("fulfillment-filter-clear")}
              </button>
            </Show>
            <Show when={invalidDraft()}>
              {/* 逐字输入 UUID 时每次按键都会判定；用礼貌播报，不打断输入。 */}
              <p
                id={LOT_FILTER_ERROR_ID}
                class="public-fulfillment__toolbar-error"
                role="status"
                aria-live="polite"
              >
                {t("fulfillment-filter-invalid")}
              </p>
            </Show>
          </div>
        </div>
        <Switch fallback={null}>
          <Match when={body().kind === "error"}>
            <div class="public-auction-empty" role="alert">
              <p class="public-auction-empty__title">{t("data-error")}</p>
              {/* 与看板错误重试同一入口：先使读缓存过期再推进代号，TTL 内的条目也会回到服务端。 */}
              <button
                class="public-auction-empty__retry"
                type="button"
                onClick={() => refreshPublicReads(refreshGeneration)}
              >
                {t("data-retry")}
              </button>
            </div>
          </Match>
          <Match when={body().kind === "empty-auctions"}>
            <div class="public-auction-empty" role="status">
              <p class="public-auction-empty__title">{t("fulfillment-empty-auctions")}</p>
            </div>
          </Match>
          <Match when={tableRead()}>
            {(read) => (
              <>
                {/* 重读遇到网络失败或服务暂时繁忙时，缓存层回退旧快照：标明它的确认时刻。
                            播报区域随表格常驻、只更新内容，读屏才会可靠朗读首次出现的提示。 */}
                <div
                  class="public-fulfillment__stale-region"
                  role="status"
                  aria-live="polite"
                  aria-atomic="true"
                >
                  <Show when={read().staleSince !== null}>
                    <p class="public-fulfillment__stale">
                      {t("fulfillment-stale", {
                        time: formatAuctionTime(read().staleSince ?? ""),
                      })}
                    </p>
                  </Show>
                </div>
                <FulfillmentTable
                  page={read().page}
                  currentPage={pageNumber()}
                  lotFilter={lotFilter()}
                  onPageChange={(nextPage) => {
                    // 从当前状态提交：仍在防抖中的筛选草稿不随翻页提交。
                    const next = { ...queryStateSignal.peek() };
                    next.page = nextPage;
                    writePublicQuery(next, "push");
                    queryStateSignal.set(next);
                  }}
                  onPageClamp={(nextPage) => {
                    // 纠偏无历史记录写回，不建立可返回的无效页条目。
                    const next = { ...queryStateSignal.peek() };
                    next.page = nextPage;
                    writePublicQuery(next, "replace");
                    queryStateSignal.set(next);
                  }}
                  onPageSizeChange={(size) => {
                    const next = { ...queryStateSignal.peek() };
                    next.pageSize = normalizePublicPageSize(size);
                    next.page = 1;
                    writePublicQuery(next, "push");
                    queryStateSignal.set(next);
                  }}
                />
              </>
            )}
          </Match>
        </Switch>
        <Show when={lookup.failed()}>
          <p class="action-feedback action-feedback--error" role="alert">
            {t("data-error")}
          </p>
        </Show>
        {/* 抽屉宿主在进度读取分支之外：翻页、筛选或后台重读替换表格时，已打开的详情不被卸载。 */}
        <LotDetailLookupHost lookup={lookup} />
      </section>
    </LotDetailLookupContext>
  );
}
