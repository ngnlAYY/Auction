// 出价记录与履约的 lot 筛选校验、地址栏写回与历史记录恢复。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createRouter, RouterContext } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { createRoot, flush } from "solid-js";
import {
  auctionParameter,
  InitialPublicLocationContext,
  initialPublicLocation,
  queryAuctionId,
  readPublicQuery,
  usePublicQueryState,
} from "../../query";
import { PUBLIC_ROUTES } from "../../router/route";

const { applyLotFilter, useLotFilterDraft, validLotFilter } = await import("./lots");

const AUCTION_A = "00000000-0000-4000-8000-000000000001";
const AUCTION_B = "00000000-0000-4000-8000-000000000002";
const DEBOUNCE_MILLIS = 10;

test("lot_filter_accepts_what_the_bid_log_endpoint_accepts：lot 筛选接受接口接受的取值", () => {
  expect(validLotFilter("")).toBe(true);
  expect(validLotFilter("One12")).toBe(true);
  expect(validLotFilter("a".repeat(32))).toBe(true);
  expect(validLotFilter("67e55044-10b1-426f-9247-bb680e5fe0c8")).toBe(true);
});

test("lot_filter_rejects_values_the_endpoint_answers_with_400：lot 筛选拒绝接口会以 400 拒绝的取值", () => {
  expect(validLotFilter("a".repeat(33))).toBe(false);
  expect(validLotFilter("One 12")).toBe(false);
  expect(validLotFilter("One-12")).toBe(false);
  expect(validLotFilter("拍品12")).toBe(false);
  // 地址栏原样保留的超长或含控制字符的值同样落到字段错误，不发请求。
  expect(validLotFilter("a".repeat(129))).toBe(false);
  expect(validLotFilter("One\u000712")).toBe(false);
  // 服务端只接受规范小写连字符形式的非 nil UUID。
  expect(validLotFilter("67E55044-10B1-426F-9247-BB680E5FE0C8")).toBe(false);
  expect(validLotFilter("{67e55044-10b1-426f-9247-bb680e5fe0c8}")).toBe(false);
  expect(validLotFilter("00000000-0000-0000-0000-000000000000")).toBe(false);
});

test("failed_lot_filter_writes_keep_applied_state_and_retry_the_same_value：lot 写入失败不提交且同值可重试", () => {
  const mounted = installFakeWindow("/bidlog?page=3");
  try {
    const queryState = createCell(readPublicQuery());
    expect(queryState.peek().page).toBe(3);

    mounted.fake.rejectWrites(true);
    expect(applyLotFilter(queryState, "One12")).toBe(false);
    expect(queryState.peek().page).toBe(3);
    expect(queryState.peek().bidLogLot).toBeNull();
    expect(new URLSearchParams(mounted.fake.location.search).get("lot")).toBeNull();

    mounted.fake.rejectWrites(false);
    expect(applyLotFilter(queryState, "One12")).toBe(true);
    expect(queryState.peek().bidLogLot).toBe("One12");
    expect(queryState.peek().page).toBe(1);
    expect(new URLSearchParams(mounted.fake.location.search).get("lot")).toBe("One12");
  } finally {
    mounted.restore();
  }
});

function mountLotDraft(initialUrl: string) {
  const { fake, restore } = installFakeWindow(initialUrl);
  const initial = initialPublicLocation();
  const previousRouter = RouterContext.defaultValue;
  const previousInitial = InitialPublicLocationContext.defaultValue;
  const root = createRoot((dispose) => {
    const router = createRouter({ routes: PUBLIC_ROUTES });
    // 单元测试沿用查询同步用例的做法，直接给无渲染树的 hook 传入上下文。
    RouterContext.defaultValue = router;
    InitialPublicLocationContext.defaultValue = initial;
    const query = usePublicQueryState();
    const lot = useLotFilterDraft(query.state, fake.location.pathname, DEBOUNCE_MILLIS);
    flush();
    initial.expire();
    return { dispose, router, query, lot };
  });
  return {
    lot: root.lot,
    query: root.query,
    params: () => new URLSearchParams(fake.location.search),
    replaceAddress(url: string) {
      fake.history.replaceState(null, "", url);
    },
    back() {
      root.router.goBack();
      flush();
    },
    forward() {
      fake.history.forward();
      flush();
    },
    dispose() {
      root.dispose();
      flush();
      RouterContext.defaultValue = previousRouter;
      InitialPublicLocationContext.defaultValue = previousInitial;
      restore();
    },
  };
}

test.each(["/bidlog", "/fulfillment"])(
  "%s：后退到同世界另一场时由 popstate 恢复查询并作废草稿",
  async (pathname) => {
    const mounted = mountLotDraft(`${pathname}?world=main&auction=${AUCTION_A}&page=3`);
    try {
      mounted.query.push({
        ...mounted.query.state.peek(),
        auction: auctionParameter(AUCTION_B),
        page: 2,
      });
      flush();
      expect(queryAuctionId(mounted.query.state.peek())).toBe(AUCTION_B);
      mounted.lot.input("One12");
      expect(mounted.lot.draft.peek()).toBe("One12");

      mounted.back();
      expect(queryAuctionId(mounted.query.state.peek())).toBe(AUCTION_A);
      expect(mounted.lot.draft.peek()).toBe("");
      await Bun.sleep(2 * DEBOUNCE_MILLIS);
      expect(mounted.params().get("lot")).toBeNull();
      expect(mounted.params().get("page")).toBe("3");

      mounted.forward();
      expect(queryAuctionId(mounted.query.state.peek())).toBe(AUCTION_B);
      expect(mounted.params().get("page")).toBe("2");
    } finally {
      mounted.dispose();
    }
  },
);

test("换世界恢复已应用筛选，返回原范围也不会恢复旧计时器", async () => {
  const original = `/bidlog?world=main&auction=${AUCTION_B}`;
  const mounted = mountLotDraft(original);
  try {
    mounted.lot.input("One12");
    mounted.query.push({
      ...mounted.query.state.peek(),
      world: "isekai",
      auction: auctionParameter(AUCTION_A),
      bidLogLot: "Two7",
    });
    flush();
    expect(mounted.lot.draft.peek()).toBe("Two7");
    mounted.back();
    expect(mounted.lot.draft.peek()).toBe("");
    await Bun.sleep(2 * DEBOUNCE_MILLIS);
    expect(mounted.params().get("lot")).toBeNull();
  } finally {
    mounted.dispose();
  }
});

test.each([`world=main&auction=${AUCTION_A}`, `world=all&auction=${AUCTION_B}`])(
  "查询状态尚未同步时，作废属于另一范围的写入（%s）",
  async (scope) => {
    const mounted = mountLotDraft(`/bidlog?world=main&auction=${AUCTION_B}`);
    try {
      mounted.lot.input("One12");
      mounted.replaceAddress(`/bidlog?${scope}&lot=Two7&page=3`);
      await Bun.sleep(2 * DEBOUNCE_MILLIS);
      expect(mounted.params().get("lot")).toBe("Two7");
      expect(mounted.params().get("page")).toBe("3");
    } finally {
      mounted.dispose();
    }
  },
);

test("地址栏已切换同场 lot、查询还未同步时放弃旧草稿", async () => {
  const scope = `/bidlog?world=main&auction=${AUCTION_A}`;
  const mounted = mountLotDraft(`${scope}&lot=One1`);
  try {
    mounted.lot.input("One12");
    mounted.replaceAddress(`${scope}&lot=Two7&page=3`);
    expect(mounted.query.state.peek().bidLogLot).toBe("One1");
    await Bun.sleep(2 * DEBOUNCE_MILLIS);
    expect(mounted.params().get("lot")).toBe("Two7");
    expect(mounted.params().get("page")).toBe("3");
  } finally {
    mounted.dispose();
  }
});

test("场次未变时只写入最后一个合法草稿，保留输入中的空白", async () => {
  const raw = `%77orld=%6dain&a%75ction=${AUCTION_A}&keep=%2b%20x`;
  const mounted = mountLotDraft(`/bidlog?${raw}&bidlog_page=3`);
  try {
    mounted.lot.input("One12");
    mounted.lot.input(" Two7 ");
    expect(mounted.params().get("lot")).toBeNull();
    await Bun.sleep(2 * DEBOUNCE_MILLIS);
    flush();
    expect(mounted.params().get("lot")).toBe("Two7");
    expect(mounted.params().get("page")).toBeNull();
    expect(window.location.search).toBe(`?${raw}&lot=Two7`);
    expect(mounted.lot.draft.peek()).toBe(" Two7 ");
  } finally {
    mounted.dispose();
  }
});

test.each([
  ["lot=One12&lot=Two7", "One12", "duplicate"],
  ["lot=+", " ", "invalid"],
  ["lot=One12%09", "One12\t", "invalid"],
] as const)("lot 深链保留非法原文 %s，修正并成功写入后才解除门禁", async (search, raw, error) => {
  const mounted = mountLotDraft(`/bidlog?world=main&auction=${AUCTION_A}&${search}`);
  try {
    expect(mounted.lot.draft.peek()).toBe(raw);
    expect(mounted.lot.draftError()).toBe(error);
    expect(mounted.lot.appliedValid()).toBe(false);
    expect(mounted.lot.interacted.peek()).toBe(false);

    mounted.lot.input("One12");
    await Bun.sleep(2 * DEBOUNCE_MILLIS);
    flush();
    expect(mounted.params().getAll("lot")).toEqual(["One12"]);
    expect(mounted.lot.appliedValid()).toBe(true);
  } finally {
    mounted.dispose();
  }
});
