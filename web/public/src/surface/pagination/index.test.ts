// Public 分页数量的接受范围：预设与默认值停在契约上限。

import { expect, test } from "bun:test";
import { MAX_PUBLIC_PAGE_SIZE } from "@auction/contracts/dto/public/pagination";
import { createCell } from "@auction/web-shared/reactive/cell";

const {
  acceptedPublicPageSize,
  commitPublicPageSizeChoice,
  DEFAULT_PAGE_SIZE,
  normalizePublicPageSize,
  PUBLIC_PAGE_SIZES,
  updatePublicPageSize,
} = await import(".");
const { publicBoardQueryDefault } = await import("../query");

interface FakeHistory {
  readonly state: unknown;
  pushState(state: unknown, title: string, url?: string | null): void;
  replaceState(state: unknown, title: string, url?: string | null): void;
}

interface FakeWindow {
  readonly location: { pathname: string; search: string; hash: string };
  readonly history: FakeHistory;
  readonly localStorage: {
    getItem(key: string): string | null;
    setItem(key: string, value: string): void;
  };
  failHistoryWrites(fail: boolean): void;
  storageValue(key: string): string | null;
}

function installFakeWindow(
  initialUrl: string,
  initialStorage: Map<string, string>,
): {
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
    localStorage: {
      getItem(key) {
        return initialStorage.get(key) ?? null;
      },
      setItem(key, value) {
        initialStorage.set(key, value);
      },
    },
    failHistoryWrites(fail) {
      failWrites = fail;
    },
    storageValue(key) {
      return initialStorage.get(key) ?? null;
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

test("page_size_choices_stop_at_the_contract_maximum：每页数量选择停在契约上限", () => {
  expect(MAX_PUBLIC_PAGE_SIZE).toBe(200);
  for (const size of [1, 37, 200, ...PUBLIC_PAGE_SIZES]) {
    expect(acceptedPublicPageSize(size)).toBe(size);
    expect(normalizePublicPageSize(size)).toBe(size);
  }
  // 自定义输入 201 等越界数量不被接受：分页器不写 URL、不保存偏好，也就不会发出请求。
  for (const size of [201, 1_000, 2 ** 63 - 1, 0, -1, -(2 ** 63)]) {
    expect(acceptedPublicPageSize(size)).toBeNull();
    // 其他入口拿到越界值时回退默认值，不截断成上限。
    expect(normalizePublicPageSize(size)).toBe(DEFAULT_PAGE_SIZE);
  }
});

test("failed_page_size_write_keeps_preference：页长写入失败时不保存本地偏好", () => {
  const storage = new Map([
    ["hv.public.page-size", "20"],
    ["hv.public.movement-page-size", "20"],
  ]);
  const mounted = installFakeWindow("/", storage);
  try {
    const queryState = createCell(publicBoardQueryDefault());
    expect(queryState.peek().pageSize).toBe(20);

    mounted.fake.failHistoryWrites(true);
    expect(
      commitPublicPageSizeChoice(50, false, (size) => updatePublicPageSize(queryState, size)),
    ).toBe(false);
    expect(queryState.peek().pageSize).toBe(20);
    expect(mounted.fake.storageValue("hv.public.page-size")).toBe("20");

    mounted.fake.failHistoryWrites(false);
    expect(
      commitPublicPageSizeChoice(50, false, (size) => updatePublicPageSize(queryState, size)),
    ).toBe(true);
    expect(queryState.peek().pageSize).toBe(50);
    expect(mounted.fake.storageValue("hv.public.page-size")).toBe("50");
    expect(new URLSearchParams(mounted.fake.location.search).get("page_size")).toBe("50");
  } finally {
    mounted.restore();
  }
});
