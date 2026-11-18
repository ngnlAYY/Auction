// 竞价合并刷新判定：未消费修订号只排定一次，窗口内合并，抖动范围有界。

import { expect, jest, onTestFinished, spyOn, test } from "bun:test";
import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { EQUIPMENT_CATEGORY_CODES } from "@auction/contracts/dto/public/catalog/lot";
import { SecondTickerContext, useSecondTicker } from "@auction/web-shared/platform/clock";
import { createPageVisibility } from "@auction/web-shared/platform/visibility";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource, type ResourceState } from "@auction/web-shared/reactive/resource";
import { ok } from "@auction/web-shared/result";
import {
  type Context,
  children,
  createComponent,
  createEffect,
  createRoot,
  type Element,
  flush,
} from "solid-js";

import { invalidateAll, resetStoreForTest } from "../../api/swr-cache";
import {
  PublicCommandOwnerContext,
  PublicContentLoadingContext,
  type PublicSessionResource,
  PublicSessionResourceContext,
  type PublicSessionResult,
} from "..";
import { publicBoardQueryDefault } from "../query";
import {
  CurrentAuctionContext,
  PageVisibilityContext,
  PublicReadinessContext,
  SelectedWorldContext,
} from "../shell/contexts";
import { currentUser } from "../watches/commands";
import { mountAuctionBoard } from ".";
import { PublicDisplayedAuctionContext, PublicPageRealtimeTargetContext } from "./realtime";
import {
  BID_REFRESH_COALESCE_MILLIS,
  BID_REFRESH_JITTER_MILLIS,
  type BidRefreshDecision,
  coalesceWindowMillis,
  PublicBidRefreshContext,
  PublicRefreshGenerationContext,
  planBidRefresh,
  refreshPublicReads,
} from "./refresh";

interface LotRequest {
  readonly page: number;
  readonly category: string | null;
  readonly signal: AbortSignal;
  complete(): void;
}

const AUCTION: CurrentAuctionItemResponse = {
  id: "00000000-0000-4000-8000-000000000001",
  world: "main",
  number: 1,
  status: "live",
  opens_at: "2026-11-18T00:00:00Z",
  scheduled_ends_at: null,
  presentation_mode: "live",
  bidding_open: true,
  preview_revision: 1,
  proxy_default_mode: "fast",
};
const FIRST_USER = "00000000-0000-4000-8000-000000000002";
const SECOND_USER = "00000000-0000-4000-8000-000000000003";
const EMPTY_CATEGORY_COUNTS = {
  all: 0,
  ...Object.fromEntries(EQUIPMENT_CATEGORY_CODES.map(([key]) => [key, 0])),
};

function authenticated(userId: string): PublicSessionResult {
  return ok({
    authenticated: true,
    user_id: userId,
    username: "viewer",
    expires_at: null,
    absolute_expires_at: null,
  });
}

function provide<T>(context: Context<T>, value: T) {
  return (render: () => Element) => () =>
    createComponent(context, {
      value,
      get children() {
        return render();
      },
    });
}

function interceptLots(): LotRequest[] {
  resetStoreForTest();
  const requests: LotRequest[] = [];
  const fetch = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(
      (input: RequestInfo | URL, init?: RequestInit) => {
        const url = new URL(input instanceof Request ? input.url : input, "http://localhost");
        const signal = init?.signal;
        if (url.pathname !== `/api/v1/auctions/${AUCTION.id}/lots` || signal == null) {
          throw new Error(`意外的看板请求 ${url.pathname}`);
        }
        const page = Number(url.searchParams.get("page"));
        const category = url.searchParams.get("category");
        const { promise, resolve, reject } = Promise.withResolvers<Response>();
        const abort = () => reject(signal.reason);
        signal.addEventListener("abort", abort, { once: true });
        requests.push({
          page,
          category,
          signal,
          complete() {
            signal.removeEventListener("abort", abort);
            resolve(
              Response.json({
                code: "ok",
                message: "ok",
                data: {
                  auction_id: AUCTION.id,
                  auction_status: AUCTION.status,
                  projection_kind: "live",
                  presentation_mode: AUCTION.presentation_mode,
                  bidding_open: AUCTION.bidding_open,
                  opens_at: AUCTION.opens_at,
                  preview_revision: AUCTION.preview_revision,
                  lots: [],
                  page,
                  page_size: Number(url.searchParams.get("page_size")),
                  total: 0,
                  category_counts: EMPTY_CATEGORY_COUNTS,
                },
              }),
            );
          },
        });
        return promise;
      },
      { preconnect: () => {} },
    ),
  );
  onTestFinished(() => {
    fetch.mockRestore();
    resetStoreForTest();
  });
  return requests;
}

function mountCatalog() {
  const requests = interceptLots();
  return createRoot((dispose) => {
    const owners: (string | null)[] = [];
    const displayedReadiness = createCell(false);
    const lotsReady = createCell(false);
    const snapshot = createCell<PublicSessionResult | undefined>(undefined);
    const session: PublicSessionResource = Object.assign(snapshot, {
      state: (): ResourceState => (snapshot() === undefined ? "pending" : "ready"),
      restart: () => {},
      cancel: () => {},
      clear: () => snapshot.set(undefined),
    });
    const commandOwner = createValueMemo(() => currentUser(session()));
    const revision = createCell(0);
    const refresh = createCell(0);
    const query = createCell({ ...publicBoardQueryDefault(), pageSize: 100 });
    const notify = () => {
      invalidateAll();
      revision.update((value) => value + 1);
    };
    const providers = [
      provide(PublicSessionResourceContext, session),
      provide(PublicCommandOwnerContext, commandOwner),
      provide(PublicContentLoadingContext, createCell<ReadonlySet<number>>(new Set())),
      provide(SelectedWorldContext, createCell(AUCTION.world)),
      provide(
        CurrentAuctionContext,
        createTaskResource(
          () => null,
          async () => [AUCTION.world, ok(AUCTION)] as const,
        ),
      ),
      provide(
        PublicReadinessContext,
        createTaskResource(
          () => null,
          async () => ok(true),
        ),
      ),
      provide(PublicPageRealtimeTargetContext, createCell(null)),
      provide(PublicDisplayedAuctionContext, createCell(null)),
      provide(SecondTickerContext, useSecondTicker()),
      provide(PageVisibilityContext, createPageVisibility()),
      provide(PublicRefreshGenerationContext, refresh),
      provide(PublicBidRefreshContext, { revision, notify }),
    ];
    const render = () =>
      mountAuctionBoard(query, (props) => {
        owners.push(props.userId);
        createEffect(() => props.bootPrerequisitesReady, displayedReadiness.set);
        createEffect(() => props.catalog.readySnapshot() !== null, lotsReady.set);
        return null;
      });
    createEffect(children(providers.reduceRight((child, wrap) => wrap(child), render)), () => {});
    return {
      dispose,
      requests,
      owners,
      displayedReadiness,
      lotsReady,
      session,
      notify,
      query,
      refresh,
    };
  });
}

async function settleCatalog(): Promise<void> {
  // 替身的响应体在内存中；让传输、解码与 Solid 的任务续体收口，不推进竞价计时器。
  for (let step = 0; step < 32; step += 1) {
    await Promise.resolve();
  }
  flush();
}

test("匿名晚到、登录确认和退出复用在途 lots，分页、筛选、刷新仍发起新读", async () => {
  const board = mountCatalog();
  try {
    flush();
    await settleCatalog();
    expect(board.requests).toHaveLength(1);
    expect(board.displayedReadiness.peek()).toBe(false);
    board.session.set(ok(null));
    flush();
    expect(board.displayedReadiness.peek()).toBe(true);
    expect(board.requests).toHaveLength(1);
    expect(board.requests[0]?.signal.aborted).toBe(false);

    board.session.set(authenticated(FIRST_USER));
    flush();
    expect(board.owners).toEqual([null, FIRST_USER]);
    expect(board.requests).toHaveLength(1);
    expect(board.requests[0]?.signal.aborted).toBe(false);

    board.session.set(ok(null));
    flush();
    expect(board.owners).toEqual([null, FIRST_USER, null]);
    expect(board.requests).toHaveLength(1);
    expect(board.requests[0]?.signal.aborted).toBe(false);

    board.query.update((query) => ({ ...query, page: 2 }));
    await settleCatalog();
    expect(board.requests.map((request) => request.page)).toEqual([1, 2]);
    expect(board.requests[0]?.signal.aborted).toBe(true);

    board.query.update((query) => ({ ...query, category: "shield" }));
    await settleCatalog();
    expect(board.requests.at(-1)?.category).toBe("shield");
    expect(board.requests).toHaveLength(3);

    refreshPublicReads(board.refresh);
    await settleCatalog();
    expect(board.requests.at(-1)?.category).toBe("shield");
    expect(board.requests).toHaveLength(4);
  } finally {
    board.dispose();
  }
});

test("换号保留已排定的竞价补读，换号后的单条通知也只补读一次", async () => {
  jest.useFakeTimers();
  onTestFinished(() => {
    jest.useRealTimers();
  });
  const board = mountCatalog();
  try {
    flush();
    await settleCatalog();
    board.requests[0]?.complete();
    await settleCatalog();
    expect(board.lotsReady.peek()).toBe(true);
    board.session.set(authenticated(FIRST_USER));
    flush();
    board.notify();
    flush();
    expect(jest.getTimerCount()).toBe(1);
    board.session.set(authenticated(SECOND_USER));
    flush();
    expect(board.owners).toEqual([null, FIRST_USER, SECOND_USER]);
    expect(board.requests).toHaveLength(1);
    expect(jest.getTimerCount()).toBe(1);

    const window = BID_REFRESH_COALESCE_MILLIS + BID_REFRESH_JITTER_MILLIS;
    jest.advanceTimersByTime(window);
    await settleCatalog();
    expect(board.requests).toHaveLength(2);

    board.requests[1]?.complete();
    await settleCatalog();
    board.notify();
    flush();
    jest.advanceTimersByTime(window);
    await settleCatalog();
    expect(board.requests).toHaveLength(3);
    board.requests[2]?.complete();
    await settleCatalog();
    jest.advanceTimersByTime(window);
    await settleCatalog();
    expect(board.requests).toHaveLength(3);
  } finally {
    board.dispose();
  }
});

/** 用“窗口从首条通知开始、不因后续通知延长”的同一规则模拟一个资源的重读次数。
 * 只建模合并策略本身，并按基础窗口计时；抖动只推迟截止时刻，边界另测。 */
function simulateRereads(notificationMillis: readonly number[]): number {
  let consumed = 0;
  let scheduled = false;
  let deadline = 0;
  let rereads = 0;
  let revision = 0;

  for (const now of notificationMillis) {
    // 计时器在截止时刻完成：先消费当时已到达的最新修订号并计数一次重读。
    if (scheduled && now >= deadline) {
      scheduled = false;
      rereads += 1;
      consumed = revision;
    }
    revision += 1;
    const decision: BidRefreshDecision = planBidRefresh(revision, consumed, scheduled, true);
    if (decision !== "idle" && decision !== "wait_for_ready" && decision !== "coalescing") {
      scheduled = true;
      deadline = now + BID_REFRESH_COALESCE_MILLIS;
    }
  }
  if (scheduled) {
    rereads += 1;
  }
  return rereads;
}

test("first_notification_schedules_exactly_once：首次通知只为当前修订号排定一次重读", () => {
  expect(planBidRefresh(7, 6, false, true)).toEqual({ kind: "schedule", revision: 7 });
});

test("consumed_revision_does_not_read_again：已消费的修订号不会重复重读", () => {
  expect(planBidRefresh(7, 7, false, true)).toBe("idle");
  // 即使上一窗口仍在等待，也不重复排定同一修订号。
  expect(planBidRefresh(7, 7, true, true)).toBe("idle");
});

test("notification_during_window_coalesces_without_extending：窗口内通知合并且不延长截止", () => {
  expect(planBidRefresh(8, 7, true, true)).toBe("coalescing");
});

test("not_ready_resource_waits_and_keeps_revision：资源未就绪时等待且保留修订号", () => {
  expect(planBidRefresh(8, 7, false, false)).toBe("wait_for_ready");
  // 就绪后同一修订号仍会排定，不因等待期间的状态变化丢失读取。
  expect(planBidRefresh(8, 7, false, true)).toEqual({ kind: "schedule", revision: 8 });
});

test("burst_inside_single_window_reads_once：单个窗口内的突发通知只重读一次", () => {
  expect(simulateRereads([0, 10, 20, 30, 40])).toBe(1);
});

test("window_is_not_extended_by_trailing_notifications：尾随通知不会延长窗口", () => {
  // 首条 0ms 的窗口在 100ms 截止；99ms 的通知合入，150ms 的通知另起窗口。
  expect(simulateRereads([0, 99, 150])).toBe(2);
});

test("sustained_bidding_reads_once_per_window：持续竞价按窗口重读而不是逐条重读", () => {
  // 每 40ms 一条、共 7 条：窗口在 0、120、240ms 分别开启，读 3 次而不是 7 次。
  expect(simulateRereads([0, 40, 80, 120, 160, 200, 240])).toBe(3);
});

test("sparse_bidding_reads_once_per_bid：稀疏竞价在每条通知后各重读一次", () => {
  // 出价间隔大于窗口时每条通知各读一次，不做延迟合并。
  expect(simulateRereads([0, 200, 400])).toBe(3);
});

test("jittered_window_spans_base_to_exclusive_upper_bound：抖动窗口覆盖基础下界到开区间上界", () => {
  const upper = BID_REFRESH_COALESCE_MILLIS + BID_REFRESH_JITTER_MILLIS;
  expect([BID_REFRESH_COALESCE_MILLIS, upper]).toEqual([100, 250]);
  // 样本 0 取基础窗口，样本趋近 1 仍小于上界。
  expect(coalesceWindowMillis(0)).toBe(100);
  expect(coalesceWindowMillis(0.5)).toBe(175);
  expect(coalesceWindowMillis(1 - Number.EPSILON)).toBe(249);
  // 随机源越界或给出非有限值时也不越出窗口。
  expect(coalesceWindowMillis(1)).toBe(249);
  expect(coalesceWindowMillis(-0.25)).toBe(100);
  expect(coalesceWindowMillis(Number.NaN)).toBe(100);
  expect(coalesceWindowMillis(Number.POSITIVE_INFINITY)).toBe(100);
  for (let step = 0; step < 1_000; step += 1) {
    const window = coalesceWindowMillis(step / 1_000);
    expect(window).toBeGreaterThanOrEqual(100);
    expect(window).toBeLessThan(upper);
  }
});
