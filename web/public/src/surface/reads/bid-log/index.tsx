// 公开竞价日志页。Solid 版与 Rust 版有意不同：世界由页面顶部的世界按钮组写入查询状态
// （Rust 版是页面内的下拉框）；本页作为跨世界页，缺省范围为全部世界。

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
import { PublicRefreshGenerationContext, useBidRefresh } from "../../board/refresh";
import { WorldTabs, worldLabel } from "../../board/world";
import {
  LotDetailLookupContext,
  LotDetailLookupHost,
  useLotDetailLookup,
} from "../../catalog/detail/lookup";
import { useContentLoading } from "../../loading";
import { publicPageMatches, updatePublicPageSize } from "../../pagination";
import {
  currentPublicPathname,
  normalizedWorldQuery,
  updatePublicQueryState,
  usePublicQueryState,
  type WorldScope,
  worldScopeFromQuery,
} from "../../query";
import { usePageRouteGuard } from "../../query/route-page";
import { rustTrim } from "../../query/text";
import { applyLotFilter, resetLotDraft, validLotFilter } from "./lots";
import { createScopeSelection, type ScopeSelection, sameScopeSelection } from "./scope-selection";
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
  selection: ScopeSelection,
  worldId: string,
  lotId: string | null,
  page: number,
  pageSize: number,
];

interface BidLogSnapshot {
  readonly requested: BidLogRequested;
  readonly result: Result<PublicBidLogListResponse | null, string>;
}

interface AuctionSnapshot {
  readonly selection: ScopeSelection;
  readonly generation: number;
  readonly result: Result<AuctionSummaryResponse[], string>;
}

interface AuctionOptionGroup {
  readonly world: string;
  readonly auctions: AuctionSummaryResponse[];
}

type BidLogBody =
  | { readonly kind: "error" }
  | { readonly kind: "empty" }
  | { readonly kind: "invalid" }
  | { readonly kind: "loading" }
  | { readonly kind: "page"; readonly page: PublicBidLogListResponse };

/** 场次下拉框中的一项；原生选中态跟随查询状态，目录刷新后不会按下标错配到另一场。 */
function BidLogRoundOption(props: {
  readonly auction: AuctionSummaryResponse;
  readonly selected: boolean;
}): JSX.Element {
  return (
    <option value={props.auction.id} selected={props.selected}>
      {/* 选项只显示期数；场次状态由下拉框旁边的状态标签给出。 */}
      {t("bidlog-round-short", { number: props.auction.sequence })}
    </option>
  );
}

/** 公开竞价日志页。 */
export function BidLog(): JSX.Element {
  const tasks = createTaskScope();
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const routeStillOnPage = usePageRouteGuard("BidLog");
  // 世界/拍卖/页码/lot 筛选与地址栏双向同步。
  const queryState = usePublicQueryState();
  const queryStateSignal = queryState.state;
  // 跨世界页：缺省或非法 `world` 表示全部世界，显式值限定单个世界。
  const worldScope = createValueMemo(
    (): WorldScope => worldScopeFromQuery("multi", normalizedWorldQuery(queryStateSignal().world)),
  );
  // 世界范围与本次选择的序号一起变化；目录资源只依赖这一个值，避免“新范围 + 旧代际”
  // 先启动一轮读取再被下一轮取消。目录纠偏、记录读取门禁与实时重读都只接受这次选择确认的目录。
  const scopeSelection = createScopeSelection(worldScope);
  const selectedAuctionId = createValueMemo(() => queryStateSignal().auctionId ?? "");
  const pageNumber = createValueMemo(() => queryStateSignal().page);
  const pageSize = createValueMemo(() => queryStateSignal().pageSize);
  const lotFilter = createValueMemo(() => queryStateSignal().bidLogLot ?? "");
  // 输入框绑定草稿；合法草稿在输入停顿后才成为已应用筛选（地址栏的 `lot`），记录请求只读后者。
  const lotDraft = createCell(untrack(lotFilter));
  const lotWriteGeneration = createCell(0);
  // 选择身份保护“只消费当前这次范围选择确认过的目录”：切回曾经访问的范围也不复用上一轮
  // 快照；同一范围内的后台刷新仍保留旧目录展示。世界变化时无条件重置草稿并作废旧任务，
  // 即使草稿去空白后等于已应用值，也要清除纯空白输入。
  let appliedSelection = untrack(scopeSelection);
  createEffect(
    () => scopeSelection(),
    (selection) => {
      if (sameScopeSelection(selection, appliedSelection)) {
        return;
      }
      appliedSelection = selection;
      resetLotDraft(lotDraft, lotWriteGeneration, untrack(lotFilter));
    },
  );
  // 同一范围内，前进/后退等外部导航改变已应用筛选时才同步草稿；本页写入的值与草稿去空白后
  // 相同，不覆盖仍在编辑的原文。
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
      // 写回成功后才提交本地状态；失败时保留别名，下一次查询变化再重试。
      updatePublicQueryState(
        queryStateSignal,
        (latest) => {
          latest.bidLogPageAlias = false;
        },
        "replace",
      );
    },
  );
  // 生命周期、可见页定时和人工刷新先重读目录；记录请求随后只消费本轮确认的公开场次。
  const auctions = createTaskResource(
    () => ({
      generation: refreshGeneration(),
      selection: scopeSelection(),
    }),
    async (dependencies, context): Promise<AuctionSnapshot | null> => {
      const result = await fetchBidLogAuctions(
        dependencies.selection.scope === "all" ? null : dependencies.selection.scope,
        context.signal,
      );
      return context.alive()
        ? {
            selection: dependencies.selection,
            generation: dependencies.generation,
            result,
          }
        : null;
    },
  );

  // 保留仍可公开读取的历史选择；仅在所选场次不属于本范围目录时纠偏。
  createEffect(
    () => {
      const selection = scopeSelection();
      return {
        snapshot: auctions(),
        selection,
        generation: refreshGeneration(),
        selected: selectedAuctionId(),
      };
    },
    (current) => {
      const snapshot = current.snapshot;
      if (snapshot === null || snapshot === undefined || !snapshot.result.ok) {
        return;
      }
      // 后台刷新保留旧目录用于展示，但它不能纠偏别的范围选择或新生命周期代际的选择。
      if (
        !sameScopeSelection(snapshot.selection, current.selection) ||
        snapshot.generation !== current.generation
      ) {
        return;
      }
      // 前进/后退换页时旧页面可能在新地址下被唤醒；纠偏只能写回仍属于本页的地址。
      if (!routeStillOnPage()) {
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

  const currentAuctions = createValueMemo(() => {
    const snapshot = auctions();
    if (snapshot === null || snapshot === undefined) {
      return null;
    }
    return sameScopeSelection(snapshot.selection, scopeSelection()) ? snapshot.result : null;
  });
  const auctionOptions = createValueMemo((): readonly AuctionSummaryResponse[] => {
    const current = currentAuctions();
    return current?.ok === true ? current.value : [];
  });
  const selectedAuction = createValueMemo(
    () => auctionOptions().find((auction) => auction.id === selectedAuctionId()) ?? null,
  );
  // 全部世界时按世界分组；目录已按主世界、异世界排序，因此相同世界始终相邻。
  const auctionGroups = createValueMemo((): readonly AuctionOptionGroup[] => {
    const groups: AuctionOptionGroup[] = [];
    for (const auction of auctionOptions()) {
      const previous = groups[groups.length - 1];
      if (previous !== undefined && previous.world === auction.world_id) {
        previous.auctions.push(auction);
      } else {
        groups.push({ world: auction.world_id, auctions: [auction] });
      }
    }
    return groups;
  });

  const bidLog = createTaskResource(
    () => {
      const auctionId = selectedAuctionId();
      const page = pageNumber();
      const size = pageSize();
      const selection = scopeSelection();
      const lot = rustTrim(lotFilter());
      // 地址栏带入的非法筛选同样不发请求：服务端只会回 400，格式提示由输入框承担。
      const lotValid = validLotFilter(lot);
      const lotId = lot !== "" ? lot : null;
      // 记录接口始终使用所选场次自己的世界；范围只决定目录，不得把 `all` 传给记录读取。
      const selected = selectedAuction();
      const worldId = selected?.world_id ?? "";
      const requested: BidLogRequested = [auctionId, selection, worldId, lotId, page, size];
      // 不直接订阅全局刷新：先确认本轮目录，避免按过期的场次可见性读取记录。
      // 目录未就绪、失败或已移除该场时不发 GET；查询切换也不能绕过这道门禁。
      const snapshot = auctions();
      const readable =
        snapshot !== null &&
        snapshot !== undefined &&
        sameScopeSelection(snapshot.selection, selection) &&
        snapshot.generation === refreshGeneration.peek() &&
        snapshot.result.ok &&
        selected !== null &&
        worldId !== "" &&
        snapshot.result.value.some((auction) => auction.id === auctionId) &&
        lotValid;
      return { auctionId, page, size, worldId, lotId, requested, readable };
    },
    async (dependencies, context): Promise<BidLogSnapshot | null> => {
      let result: Result<PublicBidLogListResponse | null, string>;
      if (dependencies.readable) {
        const fetched = await fetchBidLogPage(
          dependencies.auctionId,
          dependencies.page,
          dependencies.worldId,
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
  const directoryCurrent = createValueMemo(() => {
    const snapshot = auctions();
    return (
      snapshot !== null &&
      snapshot !== undefined &&
      sameScopeSelection(snapshot.selection, scopeSelection()) &&
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
    const selected = selectedAuction();
    if (snapshot === null || snapshot === undefined || selected === null) {
      return null;
    }
    const [auctionId, selection, worldId, lotId, page, size] = snapshot.requested;
    if (
      auctionId === selected.id &&
      sameScopeSelection(selection, scopeSelection()) &&
      worldId === selected.world_id &&
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
        {/* 世界切换：页面顶部的按钮组（WorldTabs，Solid 版专属）；Rust 版用页面内的下拉框。 */}
        <div class="public-world-toolbar">
          <WorldTabs />
        </div>
        <div class="public-auction-bid-log-heading">
          <div class="public-auction-bid-log-round-header">
            <select
              id="public-auction-bid-log-round"
              class="public-auction-bid-log-round-select"
              name="auction"
              autocomplete="off"
              aria-label={t("bidlog-round")}
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
              <Show
                when={worldScope() === "all"}
                fallback={
                  <For each={auctionOptions()} keyed={(auction) => auction.id}>
                    {(auction) => (
                      <BidLogRoundOption
                        auction={auction()}
                        selected={selectedAuctionId() === auction().id}
                      />
                    )}
                  </For>
                }
              >
                <For each={auctionGroups()} keyed={(group) => group.world}>
                  {(group) => (
                    <optgroup label={worldLabel(group().world)}>
                      <For each={group().auctions} keyed={(auction) => auction.id}>
                        {(auction) => (
                          <BidLogRoundOption
                            auction={auction()}
                            selected={selectedAuctionId() === auction().id}
                          />
                        )}
                      </For>
                    </optgroup>
                  )}
                </For>
              </Show>
            </select>
            <Show when={selectedAuction()}>
              {(auction) => (
                <>
                  <span class="public-auction-bid-log-round-world">
                    {t("bidlog-world", { world: worldLabel(auction().world_id) })}
                  </span>
                  <span class={bidLogStatusClass(auction().status)}>
                    {bidLogStatusLabel(auction().status)}
                  </span>
                </>
              )}
            </Show>
          </div>
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
                world={selectedAuction()?.world_id ?? null}
                lotFilter={currentLotFilter()}
                onPageChange={(nextPage) => {
                  // 从当前状态提交：已应用的 lot 筛选与地址栏同时写入，仍在防抖中的草稿
                  // 不随翻页提交。写入失败时保留原页，查询入口不提交本地状态。
                  updatePublicQueryState(
                    queryStateSignal,
                    (query) => {
                      query.page = nextPage;
                    },
                    "push",
                  );
                }}
                onPageClamp={(nextPage) => {
                  // 纠偏无历史记录写回，不建立可返回的无效页条目。
                  updatePublicQueryState(
                    queryStateSignal,
                    (query) => {
                      query.page = nextPage;
                    },
                    "replace",
                  );
                }}
                onPageSizeChange={(size) => updatePublicPageSize(queryStateSignal, size)}
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
