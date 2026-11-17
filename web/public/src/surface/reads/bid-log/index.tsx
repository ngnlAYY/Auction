// 公开竞价日志页。

import type { AuctionSummaryResponse } from "@auction/contracts/dto/public/catalog/auction";
import type { PublicBidLogListResponse } from "@auction/contracts/dto/public/catalog/history";
import { sleep } from "@auction/web-shared/platform/timer";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { ok, type Result } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, Match, Show, Switch, untrack, useContext } from "solid-js";

import { fetchBidLogAuctions, fetchBidLogPage } from "../../../api/queries/bid-log";
import { fetchWorlds } from "../../../api/queries/current";
import { PublicRefreshGenerationContext, useBidRefresh } from "../../board/refresh";
import { supportedWorlds, worldLabel } from "../../board/world";
import {
  LotDetailLookupContext,
  LotDetailLookupHost,
  useLotDetailLookup,
} from "../../catalog/detail/lookup";
import { useContentLoading } from "../../loading";
import { normalizePublicPageSize, publicPageMatches } from "../../pagination";
import {
  currentPublicPathname,
  type PublicBoardQuery,
  updatePublicQueryState,
  usePublicQueryState,
  writePublicQuery,
} from "../../query";
import { rustTrim } from "../../query/text";
import { applyLotFilter, resetLotDraft, validLotFilter } from "./lots";
import { BidLogRow, BidLogTable, bidLogStatusClass, bidLogStatusLabel } from "./table";

export {
  applyLotFilter,
  BidLogRow,
  BidLogTable,
  bidLogStatusClass,
  bidLogStatusLabel,
  resetLotDraft,
  validLotFilter,
};

/** lot 筛选生效的防抖窗口：输入停顿后才应用筛选并无历史记录地写回地址栏，连续输入只发一次
 * 记录请求。 */
const LOT_FILTER_WRITE_DEBOUNCE_MILLIS = 300;
/** lot 筛选格式提示的元素 ID，供输入框 `aria-describedby` 引用。 */
const LOT_FILTER_ERROR_ID = "public-auction-bid-log-filter-error";

/** 一次竞价日志读取所对应的查询；刷新完成时用它与当前查询核对，避免迟到结果覆盖新页面。 */
type BidLogRequested = readonly [
  auctionId: string,
  world: string,
  lotId: string | null,
  page: number,
  pageSize: number,
];

interface BidLogSnapshot {
  readonly requested: BidLogRequested;
  readonly result: Result<PublicBidLogListResponse | null, string>;
}

interface AuctionSnapshot {
  readonly world: string;
  readonly generation: number;
  readonly result: Result<AuctionSummaryResponse[], string>;
}

type BidLogBody =
  | { readonly kind: "error" }
  | { readonly kind: "empty" }
  | { readonly kind: "invalid" }
  | { readonly kind: "loading" }
  | { readonly kind: "page"; readonly page: PublicBidLogListResponse };

/** 公开竞价日志页。 */
export function BidLog(): JSX.Element {
  const tasks = createTaskScope();
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  // 世界/拍卖/页码/lot 筛选与地址栏双向同步。
  const queryState = usePublicQueryState();
  const queryStateSignal = queryState.state;
  const worldFilter = createValueMemo(() => {
    const world = queryStateSignal().world;
    return world === "main" || world === "isekai" ? world : "main";
  });
  const selectedAuctionId = createValueMemo(() => queryStateSignal().auctionId ?? "");
  const pageNumber = createValueMemo(() => queryStateSignal().page);
  const pageSize = createValueMemo(() => queryStateSignal().pageSize);
  const lotFilter = createValueMemo(() => queryStateSignal().bidLogLot ?? "");
  // 输入框绑定草稿；合法草稿在输入停顿后才成为已应用筛选（地址栏的 `lot`），记录请求只读后者。
  const lotDraft = createCell(untrack(lotFilter));
  const lotWriteGeneration = createCell(0);
  // 前进/后退等外部导航改变已应用筛选时，草稿随之同步并作废在途的防抖写入；本页写入的值与
  // 草稿去空白后相同，不覆盖仍在编辑的原文。
  createEffect(
    () => lotFilter(),
    (applied) => {
      if (rustTrim(lotDraft.peek()) !== applied) {
        resetLotDraft(lotDraft, lotWriteGeneration, applied);
      }
    },
  );
  // 迁移期间曾写出 `bidlog_page`；读取后立即无历史记录地收敛回旧版 `page`。
  createEffect(
    () => queryStateSignal(),
    (query) => {
      if (!query.bidLogPageAlias) {
        return;
      }
      const next: PublicBoardQuery = { ...queryStateSignal.peek(), bidLogPageAlias: false };
      writePublicQuery(next, "replace");
      queryStateSignal.set(next);
    },
  );
  const worlds = createTaskResource(
    () => null,
    async (_dependencies, context) => {
      const result = await fetchWorlds(context.signal);
      return context.alive() ? result : null;
    },
  );
  const worldOptions = createValueMemo(() => {
    const snapshot = worlds();
    return snapshot?.ok === true ? supportedWorlds(snapshot.value) : [worldFilter()];
  });
  // 生命周期、可见页定时和人工刷新先重读目录；记录请求随后只消费本轮确认的公开场次。
  const auctions = createTaskResource(
    () => ({ generation: refreshGeneration(), world: worldFilter() }),
    async (dependencies, context): Promise<AuctionSnapshot | null> => {
      const result = await fetchBidLogAuctions(dependencies.world, context.signal);
      return context.alive()
        ? { world: dependencies.world, generation: dependencies.generation, result }
        : null;
    },
  );

  // 保留仍可公开读取的历史选择；仅在所选场次不属于本世界目录时纠偏。
  createEffect(
    () => ({
      snapshot: auctions(),
      world: worldFilter(),
      generation: refreshGeneration(),
      selected: selectedAuctionId(),
    }),
    (current) => {
      const snapshot = current.snapshot;
      if (snapshot === null || snapshot === undefined || !snapshot.result.ok) {
        return;
      }
      // 后台刷新保留旧目录用于展示，但它不能纠偏新世界或新生命周期代际的选择。
      if (snapshot.world !== current.world || snapshot.generation !== current.generation) {
        return;
      }
      const auctionList = snapshot.result.value;
      const nextAuctionId =
        auctionList.find((auction) => auction.id === current.selected)?.id ??
        auctionList[0]?.id ??
        "";
      if (current.selected !== nextAuctionId) {
        updatePublicQueryState(
          queryStateSignal,
          (query) => {
            query.auctionId = nextAuctionId !== "" ? nextAuctionId : null;
            query.page = 1;
          },
          "replace",
        );
      }
    },
  );

  const bidLog = createTaskResource(
    () => {
      const auctionId = selectedAuctionId();
      const page = pageNumber();
      const size = pageSize();
      const world = worldFilter();
      const lot = rustTrim(lotFilter());
      // 地址栏带入的非法筛选同样不发请求：服务端只会回 400，格式提示由输入框承担。
      const lotValid = validLotFilter(lot);
      const lotId = lot !== "" ? lot : null;
      const requested: BidLogRequested = [auctionId, world, lotId, page, size];
      // 不直接订阅全局刷新：先确认本轮目录，避免按过期的场次可见性读取记录。
      // 目录未就绪、失败或已移除该场时不发 GET；查询切换也不能绕过这道门禁。
      const snapshot = auctions();
      const readable =
        snapshot !== null &&
        snapshot !== undefined &&
        snapshot.world === world &&
        snapshot.generation === refreshGeneration.peek() &&
        snapshot.result.ok &&
        snapshot.result.value.some((auction) => auction.id === auctionId) &&
        lotValid;
      return { auctionId, page, size, world, lotId, requested, readable };
    },
    async (dependencies, context): Promise<BidLogSnapshot | null> => {
      let result: Result<PublicBidLogListResponse | null, string>;
      if (dependencies.readable) {
        const fetched = await fetchBidLogPage(
          dependencies.auctionId,
          dependencies.page,
          dependencies.world,
          dependencies.lotId,
          dependencies.size,
          context.signal,
        );
        if (!context.alive()) {
          return null;
        }
        result = fetched.ok ? ok(fetched.value) : fetched;
      } else {
        result = ok(null);
      }
      return { requested: dependencies.requested, result };
    },
  );
  const currentAuctions = createValueMemo(() => {
    const snapshot = auctions();
    if (snapshot === null || snapshot === undefined) {
      return null;
    }
    return snapshot.world === worldFilter() ? snapshot.result : null;
  });
  const auctionOptions = createValueMemo((): readonly AuctionSummaryResponse[] => {
    const current = currentAuctions();
    return current?.ok === true ? current.value : [];
  });
  const selectedAuction = createValueMemo(
    () => auctionOptions().find((auction) => auction.id === selectedAuctionId()) ?? null,
  );
  const directoryCurrent = createValueMemo(() => {
    const snapshot = auctions();
    return (
      snapshot !== null &&
      snapshot !== undefined &&
      snapshot.world === worldFilter() &&
      snapshot.generation === refreshGeneration()
    );
  });
  useBidRefresh(bidLog, () => directoryCurrent() && selectedAuction() !== null);
  const lookup = useLotDetailLookup();
  const currentPage = createValueMemo(() => pageNumber());
  const currentLotFilter = createValueMemo(() => lotFilter());
  const draftInvalid = createValueMemo(() => !validLotFilter(rustTrim(lotDraft())));
  const onLotFilterInput = (value: string): void => {
    // 草稿立即跟随输入，工具条不随请求结果卸载，以保留焦点。非法草稿不发请求，列表保留
    // 已应用筛选的结果，由行内提示说明格式；合法草稿停顿后才应用并写回地址栏。
    const lot = rustTrim(value);
    resetLotDraft(lotDraft, lotWriteGeneration, value);
    if (!validLotFilter(lot)) {
      return;
    }
    const generation = lotWriteGeneration.peek();
    tasks.spawn(async (context) => {
      await sleep(LOT_FILTER_WRITE_DEBOUNCE_MILLIS, context.signal);
      // 迟到任务不能改写已经离开的页面或更新的输入。
      if (
        context.alive() &&
        lotWriteGeneration.peek() === generation &&
        currentPublicPathname() === "/bidlog"
      ) {
        applyLotFilter(queryStateSignal, lot);
      }
    });
  };
  // 同查询后台刷新保留完成快照；目录确认场次已不可见后，迟到记录也不能恢复旧行和分页。
  const currentResult = createValueMemo(() => {
    const snapshot = bidLog();
    if (snapshot === null || snapshot === undefined || selectedAuction() === null) {
      return null;
    }
    const [auctionId, world, lotId, page, size] = snapshot.requested;
    if (
      auctionId === selectedAuctionId() &&
      world === worldFilter() &&
      (lotId ?? "") === rustTrim(currentLotFilter()) &&
      page === currentPage() &&
      size === pageSize()
    ) {
      return snapshot.result;
    }
    return null;
  });
  // 目录或当前查询的记录还没有结果时由顶栏提示加载，正文留空。
  const bodyState = createValueMemo((): BidLogBody => {
    const auctionsNow = currentAuctions();
    const resultNow = currentResult();
    if (auctionsNow?.ok === false) {
      return { kind: "error" };
    }
    if (auctionsNow?.ok === true && resultNow?.ok === false) {
      return { kind: "error" };
    }
    if (auctionsNow?.ok === true && auctionsNow.value.length === 0) {
      return { kind: "empty" };
    }
    // 已应用的筛选非法（来自地址栏）时没有发出请求：正文留空且不提示加载，格式提示在输入框旁。
    if (auctionsNow?.ok === true && !validLotFilter(rustTrim(currentLotFilter()))) {
      return { kind: "invalid" };
    }
    if (
      auctionsNow?.ok === true &&
      resultNow?.ok === true &&
      resultNow.value !== null &&
      publicPageMatches(resultNow.value.page, resultNow.value.page_size, currentPage(), pageSize())
    ) {
      return { kind: "page", page: resultNow.value };
    }
    return { kind: "loading" };
  });
  const loadedPage = createValueMemo(() => {
    const state = bodyState();
    return state.kind === "page" ? state.page : null;
  });
  useContentLoading(() => bodyState().kind === "loading");

  return (
    <LotDetailLookupContext value={lookup}>
      <section class="public-auction-bid-log-page" aria-label={t("page-bidlog-title")}>
        <div class="public-auction-bid-log-controls">
          <label for="public-auction-bid-log-world">
            <span>{t("world-label")}</span>
            <select
              id="public-auction-bid-log-world"
              name="world"
              autocomplete="off"
              value={worldFilter()}
              disabled={worldOptions().length === 0}
              onChange={(event) => {
                const world = event.currentTarget.value;
                // 切换世界清除 lot 筛选：草稿一并清空，在途的防抖写入作废。
                resetLotDraft(lotDraft, lotWriteGeneration, "");
                updatePublicQueryState(
                  queryStateSignal,
                  (query) => {
                    // 旧版 Main 使用缺省 URL，非 Main 世界才写入 `world`。
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
          <label for="public-auction-bid-log-round">
            <span>{t("bidlog-round")}</span>
            <select
              id="public-auction-bid-log-round"
              name="auction"
              autocomplete="off"
              value={selectedAuctionId()}
              disabled={auctionOptions().length === 0}
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
                    {t("bidlog-round-option", {
                      number: auction().sequence,
                      status: bidLogStatusLabel(auction().status),
                    })}
                  </option>
                )}
              </For>
            </select>
          </label>
        </div>
        <div class="public-auction-bid-log-heading">
          <Show when={selectedAuction()}>
            {(auction) => (
              <div class="public-auction-bid-log-round-header">
                <strong class="public-auction-bid-log-round-number">
                  {t("bidlog-round-number", { number: auction().sequence })}
                </strong>
                <span class="public-auction-bid-log-round-world">
                  {t("bidlog-world", { world: worldLabel(auction().world_id) })}
                </span>
                <span class={bidLogStatusClass(auction().status)}>
                  {bidLogStatusLabel(auction().status)}
                </span>
              </div>
            )}
          </Show>
          <div class="public-auction-bid-log-toolbar">
            <label for="public-auction-bid-log-filter">{t("bidlog-filter-label")}</label>
            <input
              id="public-auction-bid-log-filter"
              name="lot"
              autocomplete="off"
              type="search"
              enterkeyhint="search"
              autocapitalize="none"
              spellcheck="false"
              value={lotDraft()}
              placeholder={t("bidlog-filter-placeholder")}
              aria-invalid={draftInvalid() ? "true" : "false"}
              aria-describedby={draftInvalid() ? LOT_FILTER_ERROR_ID : undefined}
              onInput={(event) => onLotFilterInput(event.currentTarget.value)}
            />
            <Show when={lotDraft() !== ""}>
              <button
                type="button"
                onClick={() => {
                  // 清除立即生效，不等防抖。
                  resetLotDraft(lotDraft, lotWriteGeneration, "");
                  applyLotFilter(queryStateSignal, "");
                }}
              >
                {t("bidlog-filter-clear")}
              </button>
            </Show>
            <Show when={draftInvalid()}>
              <p
                id={LOT_FILTER_ERROR_ID}
                class="public-auction-bid-log-toolbar__error"
                // 逐字输入 UUID 时每次按键都会判定；用礼貌播报，不打断输入。
                role="status"
                aria-live="polite"
              >
                {t("bidlog-filter-invalid")}
              </p>
            </Show>
          </div>
        </div>
        <Switch>
          <Match when={bodyState().kind === "error"}>
            <div class="public-auction-empty" role="alert">
              <p class="public-auction-empty__title">{t("data-error")}</p>
              <button
                class="public-auction-empty__retry"
                type="button"
                onClick={() => {
                  refreshGeneration.set(
                    Math.min(refreshGeneration.peek() + 1, Number.MAX_SAFE_INTEGER),
                  );
                }}
              >
                {t("data-retry")}
              </button>
            </div>
          </Match>
          <Match when={bodyState().kind === "empty"}>
            <div class="public-auction-empty" role="status">
              <p class="public-auction-empty__title">{t("data-empty")}</p>
            </div>
          </Match>
          <Match when={loadedPage()}>
            {(page) => (
              <BidLogTable
                page={page()}
                currentPage={currentPage()}
                world={worldFilter()}
                lotFilter={currentLotFilter()}
                onPageChange={(nextPage) => {
                  // 从当前状态提交：已应用的 lot 筛选与地址栏同时写入，仍在防抖中的草稿
                  // 不随翻页提交。
                  const next: PublicBoardQuery = { ...queryStateSignal.peek(), page: nextPage };
                  writePublicQuery(next, "push");
                  queryStateSignal.set(next);
                }}
                onPageClamp={(nextPage) => {
                  // 纠偏无历史记录写回，不建立可返回的无效页条目。
                  const next: PublicBoardQuery = { ...queryStateSignal.peek(), page: nextPage };
                  writePublicQuery(next, "replace");
                  queryStateSignal.set(next);
                }}
                onPageSizeChange={(size) => {
                  const next: PublicBoardQuery = {
                    ...queryStateSignal.peek(),
                    pageSize: normalizePublicPageSize(size),
                    page: 1,
                  };
                  writePublicQuery(next, "push");
                  queryStateSignal.set(next);
                }}
              />
            )}
          </Match>
        </Switch>
        <Show when={lookup.failed()}>
          <p class="action-feedback action-feedback--error" role="alert">
            {t("data-error")}
          </p>
        </Show>
        {/* 抽屉宿主在记录读取分支之外：翻页、筛选或后台重读替换表格时，已打开的详情不被卸载。 */}
        <LotDetailLookupHost lookup={lookup} />
      </section>
    </LotDetailLookupContext>
  );
}
