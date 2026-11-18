// 竞价日志 lot 筛选的取值校验与写回：地址栏拒绝写入时不能提交已应用状态，恢复后同一值必须能重试。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";

import { readPublicQuery } from "../../query";

const { applyLotFilter, validLotFilter } = await import("./lots");

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

interface FakeHistory {
  readonly state: unknown;
  pushState(state: unknown, title: string, url?: string | null): void;
  replaceState(state: unknown, title: string, url?: string | null): void;
}

interface FakeWindow {
  readonly location: { pathname: string; search: string; hash: string };
  readonly history: FakeHistory;
  failHistoryWrites(fail: boolean): void;
}

function installFakeWindow(initialUrl: string): {
  readonly fake: FakeWindow;
  restore(): void;
} {
  const location = { pathname: "/", search: "", hash: "" };
  let state: unknown = null;
  let failWrites = false;
  const applyUrl = (url: string | null | undefined): void => {
    if (url === undefined || url === null) {
      return;
    }
    const parsed = new URL(url, "https://auction.test");
    location.pathname = parsed.pathname;
    location.search = parsed.search;
    location.hash = parsed.hash;
  };
  applyUrl(initialUrl);
  const fake: FakeWindow = {
    location,
    history: {
      get state() {
        return state;
      },
      pushState(nextState, _title, url) {
        if (failWrites) {
          throw new Error("history.pushState 被测试拒绝");
        }
        state = nextState;
        applyUrl(url);
      },
      replaceState(nextState, _title, url) {
        if (failWrites) {
          throw new Error("history.replaceState 被测试拒绝");
        }
        state = nextState;
        applyUrl(url);
      },
    },
    failHistoryWrites(fail) {
      failWrites = fail;
    },
  };
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    writable: true,
    value: fake,
  });
  return {
    fake,
    restore() {
      if (previousWindow === undefined) {
        Reflect.deleteProperty(globalThis, "window");
      } else {
        Object.defineProperty(globalThis, "window", previousWindow);
      }
    },
  };
}

test("failed_lot_filter_writes_keep_applied_state_and_retry_the_same_value：lot 写入失败不提交且同值可重试", () => {
  const mounted = installFakeWindow("/bidlog?page=3");
  try {
    const queryState = createCell(readPublicQuery());
    expect(queryState.peek().page).toBe(3);

    mounted.fake.failHistoryWrites(true);
    expect(applyLotFilter(queryState, "One12")).toBe(false);
    expect(queryState.peek().page).toBe(3);
    expect(queryState.peek().bidLogLot).toBeNull();
    expect(new URLSearchParams(mounted.fake.location.search).get("lot")).toBeNull();

    mounted.fake.failHistoryWrites(false);
    expect(applyLotFilter(queryState, "One12")).toBe(true);
    expect(queryState.peek().bidLogLot).toBe("One12");
    expect(queryState.peek().page).toBe(1);
    expect(new URLSearchParams(mounted.fake.location.search).get("lot")).toBe("One12");
  } finally {
    mounted.restore();
  }
});
