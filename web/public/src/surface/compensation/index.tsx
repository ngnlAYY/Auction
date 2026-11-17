// Public 补偿池页面装配：余额、场次批次与逐件结算明细。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, ok, type Result } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { formatCreditAmountFull, formatPpm } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Show, Switch, useContext } from "solid-js";

import {
  type CompensationLotPage,
  type CompensationPoolPage,
  fetchCompensationLots,
  fetchCompensationPool,
} from "../../api/compensation";
import { PublicRefreshGenerationContext } from "../board/refresh";
import {
  LotDetailLookupContext,
  LotDetailLookupHost,
  useLotDetailLookup,
} from "../catalog/detail/lookup";
import { useContentLoading } from "../loading";
import { normalizePublicPageSize, publicPageMatches } from "../pagination";
import { updatePublicQueryState, usePublicQueryState } from "../query/state";
import { CompensationPoolTable } from "./table";

interface CompensationPoolLoad {
  readonly requested: readonly [number, number];
  readonly result: Result<CompensationPoolPage, string>;
}

interface CompensationLotsLoad {
  readonly auctionId: string;
  readonly page: number;
  readonly pageSize: number;
  readonly result: Result<CompensationLotPage | null, string>;
}

/** 批次与展开明细使用各自的页码/数量；批次与明细分页均通过 URL 恢复。 */
export function poolPage(): JSX.Element {
  const refreshGeneration = useContext(PublicRefreshGenerationContext);
  const queryState = usePublicQueryState();
  const queryStateSignal = queryState.state;
  const pageNumber = createValueMemo(() => Math.max(queryStateSignal().page, 1));
  const pageSize = createValueMemo(() => queryStateSignal().pageSize);
  const selectedAuctionId = createValueMemo(() => queryStateSignal().auctionId ?? "");
  const movementPage = createValueMemo(() => queryStateSignal().movementPage);
  const movementPageSize = createValueMemo(() => queryStateSignal().movementPageSize);
  // 逐件明细的查看按钮在页内打开只读的拍品详情抽屉；读取任务归页面宿主，不随翻页卸载。
  const lookup = useLotDetailLookup();
  const pool = createTaskResource(
    () => [refreshGeneration(), pageNumber(), pageSize()] as const,
    async ([, requestedPage, requestedSize], context): Promise<CompensationPoolLoad> => {
      const result = await fetchCompensationPool(requestedPage, requestedSize, context.signal);
      if (!context.alive()) {
        return {
          requested: [requestedPage, requestedSize] as const,
          result: err("network_error"),
        };
      }
      return {
        requested: [requestedPage, requestedSize] as const,
        result,
      };
    },
  );
  const lots = createTaskResource(
    () => [refreshGeneration(), selectedAuctionId(), movementPage(), movementPageSize()] as const,
    async ([, auctionId, page, pageSize], context): Promise<CompensationLotsLoad> => {
      let result: Result<CompensationLotPage | null, string>;
      if (auctionId === "") {
        result = ok(null);
      } else {
        const fetched = await fetchCompensationLots(auctionId, page, pageSize, context.signal);
        if (!context.alive()) {
          return { auctionId, page, pageSize, result: err("network_error") };
        }
        result = fetched.ok
          ? fetched.value.round.auction_id === auctionId
            ? ok(fetched.value)
            : err("invalid_response")
          : fetched;
      }
      return { auctionId, page, pageSize, result };
    },
  );

  const current = createValueMemo((): Result<CompensationPoolPage, string> | null => {
    const loaded = pool();
    if (
      loaded === undefined ||
      loaded.requested[0] !== pageNumber() ||
      loaded.requested[1] !== pageSize()
    ) {
      return null;
    }
    return loaded.result;
  });
  const currentFailed = () => current()?.ok === false;
  const currentMismatch = () => {
    const result = current();
    return (
      result?.ok === true &&
      !publicPageMatches(result.value.page, result.value.page_size, pageNumber(), pageSize())
    );
  };
  const currentPage = createValueMemo((): CompensationPoolPage | null => {
    const result = current();
    return result?.ok === true &&
      publicPageMatches(result.value.page, result.value.page_size, pageNumber(), pageSize())
      ? result.value
      : null;
  });
  // 没有当前页码的结果（含等待页码纠偏后的重读）时由顶栏提示加载，正文留空。
  useContentLoading(() => {
    const result = current();
    if (result === null) {
      return true;
    }
    if (!result.ok) {
      return false;
    }
    return !publicPageMatches(result.value.page, result.value.page_size, pageNumber(), pageSize());
  });
  // 只保留同一轮次及同一分页请求的快照，避免旧数据触发新页码纠偏。
  const lotState = (): Result<CompensationLotPage | null, string> | null => {
    const loaded = lots();
    if (loaded === undefined) {
      return null;
    }
    return loaded.auctionId === selectedAuctionId() &&
      loaded.page === movementPage() &&
      loaded.pageSize === movementPageSize()
      ? loaded.result
      : null;
  };
  const retry = () => {
    refreshGeneration.set(Math.min(refreshGeneration.peek() + 1, Number.MAX_SAFE_INTEGER));
  };

  return (
    <LotDetailLookupContext value={lookup}>
      <Switch fallback={null}>
        <Match when={current() === null}>{null}</Match>
        <Match when={currentFailed()}>
          <AsyncState
            kind="error"
            title={t("page-compensation-pool-title")}
            detail={t("data-error")}
            retryLabel={t("data-retry")}
            onRetry={retry}
          />
        </Match>
        <Match when={currentMismatch()}>{null}</Match>
        <Match when={currentPage()}>
          {(page) => (
            <CompensationPoolTable
              page={page()}
              selectedAuctionId={selectedAuctionId()}
              lots={lotState()}
              onPageChange={(nextPage) => {
                updatePublicQueryState(
                  queryStateSignal,
                  (query) => {
                    query.page = Math.max(nextPage, 1);
                    query.auctionId = null;
                    query.movementPage = 1;
                  },
                  "push",
                );
              }}
              onPageClamp={(nextPage) => {
                updatePublicQueryState(
                  queryStateSignal,
                  (query) => {
                    query.page = Math.max(nextPage, 1);
                    query.auctionId = null;
                    query.movementPage = 1;
                  },
                  "replace",
                );
              }}
              onPageSizeChange={(size) => {
                updatePublicQueryState(
                  queryStateSignal,
                  (query) => {
                    query.pageSize = normalizePublicPageSize(size);
                    query.page = 1;
                    query.auctionId = null;
                    query.movementPage = 1;
                  },
                  "push",
                );
              }}
              onSelectRound={(auctionId) => {
                updatePublicQueryState(
                  queryStateSignal,
                  (query) => {
                    query.auctionId = query.auctionId !== auctionId ? auctionId : null;
                    query.movementPage = 1;
                  },
                  "push",
                );
              }}
              onLotPageChange={(nextPage) => {
                updatePublicQueryState(
                  queryStateSignal,
                  (query) => {
                    query.movementPage = Math.max(nextPage, 1);
                  },
                  "push",
                );
              }}
              onLotPageClamp={(nextPage) => {
                updatePublicQueryState(
                  queryStateSignal,
                  (query) => {
                    query.movementPage = Math.max(nextPage, 1);
                  },
                  "replace",
                );
              }}
              onLotPageSizeChange={(size) => {
                updatePublicQueryState(
                  queryStateSignal,
                  (query) => {
                    query.movementPageSize = normalizePublicPageSize(size);
                    query.movementPage = 1;
                  },
                  "push",
                );
              }}
              onRetryLots={retry}
            />
          )}
        </Match>
      </Switch>
      <Show when={lookup.failed()}>
        <p class="action-feedback action-feedback--error" role="alert">
          {t("data-error")}
        </p>
      </Show>
      {/* 抽屉宿主在批次读取分支之外：翻页或后台重读替换表格时，已打开的详情不被卸载。 */}
      <LotDetailLookupHost lookup={lookup} />
    </LotDetailLookupContext>
  );
}

/** 补偿池页面统一展示精确金额，不把整千值缩写为 K/M，避免同一列混用单位影响复算。 */
export function credits(amount: number): string {
  return `${formatCreditAmountFull(amount)}c`;
}

/** 以补偿池视角带符号展示金额：流入为 `+`，流出为 `−`。 */
export function signedAmount(amount: number): string {
  if (amount > 0) {
    return `+${credits(amount)}`;
  }
  if (amount < 0) {
    return `−${credits(Math.abs(amount))}`;
  }
  return credits(0);
}

/** 实际兑付率：补偿支出占名义折扣需求的比例，精确到 0.01%；没有需求时不适用。 */
export function coverageLabel(compensationOut: number, discountDemand: number): string {
  if (discountDemand <= 0) {
    return "—";
  }
  const ppm =
    ((compensationOut > 0 ? BigInt(compensationOut) : 0n) * 1_000_000n) / BigInt(discountDemand);
  const floored = ((ppm > 1_000_000n ? 1_000_000n : ppm) / 100n) * 100n;
  const value = floored <= 2_147_483_647n ? Number(floored) : 1_000_000;
  return formatPpm(value, false);
}
