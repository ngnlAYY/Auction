// 页面代码加载失败后的重载判定：同一标签页的同一路径在保护时间内只重载一次。

import { describe, expect, mock, test } from "bun:test";

import { pageLoadReloadDecision, reloadAfterPageLoadError } from "./page-load-recovery";

const ROUTE_PATH = "/bidlog";

describe("pageLoadReloadDecision", () => {
  test("没有标记时重载，并写下带时刻与路径的标记", () => {
    expect(pageLoadReloadDecision(null, ROUTE_PATH, 1_000)).toEqual({
      reload: true,
      mark: JSON.stringify([{ path: ROUTE_PATH, at: 1_000 }]),
    });
  });

  test("同一路径在保护时间内不再重载", () => {
    const first = pageLoadReloadDecision(null, ROUTE_PATH, 1_000);
    expect(pageLoadReloadDecision(first.mark, ROUTE_PATH, 1_000 + 29_999).reload).toBe(false);
  });

  test("保护时间过后可以再次重载", () => {
    const first = pageLoadReloadDecision(null, ROUTE_PATH, 1_000);
    expect(pageLoadReloadDecision(first.mark, ROUTE_PATH, 1_000 + 30_000).reload).toBe(true);
  });

  test("换了路径不受上一次标记限制", () => {
    const first = pageLoadReloadDecision(null, ROUTE_PATH, 1_000);
    expect(pageLoadReloadDecision(first.mark, "/history", 2_000).reload).toBe(true);
  });

  test("不同路径分别过期，写入新标记时丢弃已过期路径", () => {
    const first = pageLoadReloadDecision(null, "/rules", 1_000);
    const second = pageLoadReloadDecision(first.mark, "/about", 2_000);
    const third = pageLoadReloadDecision(second.mark, "/history", 31_000);
    expect(third.mark).toBe(
      JSON.stringify([
        { path: "/about", at: 2_000 },
        { path: "/history", at: 31_000 },
      ]),
    );
    expect(pageLoadReloadDecision(third.mark, "/rules", 31_000).reload).toBe(true);
    expect(pageLoadReloadDecision(third.mark, "/about", 31_000).reload).toBe(false);
  });

  test("写入第九条有效标记时淘汰最早写入的路径", () => {
    let mark: string | null = null;
    for (let index = 0; index < 9; index += 1) {
      const decision = pageLoadReloadDecision(mark, `/history/${index}`, 1_000 + index);
      expect(decision.reload).toBe(true);
      mark = decision.mark;
    }
    expect(pageLoadReloadDecision(mark, "/history/0", 2_000).reload).toBe(true);
    expect(pageLoadReloadDecision(mark, "/history/1", 2_000).reload).toBe(false);
    expect(pageLoadReloadDecision(mark, "/history/8", 2_000).reload).toBe(false);
  });

  test("路径里的竖线不影响解析", () => {
    const routePath = "/history/a|b";
    const first = pageLoadReloadDecision(null, routePath, 5_000);
    expect(pageLoadReloadDecision(first.mark, routePath, 6_000).reload).toBe(false);
  });

  test("时钟回拨按刚重载过处理", () => {
    const mark = JSON.stringify([{ path: ROUTE_PATH, at: 9_000 }]);
    expect(pageLoadReloadDecision(mark, ROUTE_PATH, 1_000).reload).toBe(false);
  });

  test("损坏或旧格式的标记按没有记录处理", () => {
    expect(pageLoadReloadDecision("garbage", ROUTE_PATH, 1_000).reload).toBe(true);
    expect(pageLoadReloadDecision(`9000|${ROUTE_PATH}`, ROUTE_PATH, 1_000).reload).toBe(true);
    const malformed = JSON.stringify([
      { path: ROUTE_PATH, at: 1_000 },
      { path: "/about?search=private", at: 1_000 },
    ]);
    expect(pageLoadReloadDecision(malformed, ROUTE_PATH, 2_000).reload).toBe(true);
  });
});

const MARK_KEY = "hv.page-load-reload";

function memoryStorage() {
  const items = new Map<string, string>();
  return {
    getItem: (key: string) => items.get(key) ?? null,
    setItem: (key: string, value: string) => items.set(key, value),
  };
}

function recoveryTab(path: string, localStorage: ReturnType<typeof memoryStorage>) {
  return {
    localStorage,
    sessionStorage: memoryStorage(),
    location: Object.assign(new URL(path, "https://example.test"), { reload: mock(() => {}) }),
  };
}

function recoverIn(tab: ReturnType<typeof recoveryTab>): boolean {
  const original = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", { configurable: true, value: tab });
  try {
    return reloadAfterPageLoadError();
  } finally {
    if (original === undefined) {
      Reflect.deleteProperty(globalThis, "window");
    } else {
      Object.defineProperty(globalThis, "window", original);
    }
  }
}

test.each(["/rules", "/about"])("第二个标签页打开 %s 时仍有独立的重载保护", (secondPath) => {
  const localStorage = memoryStorage();
  const firstTarget = "/rules?search=private#chapter";
  const first = recoveryTab(firstTarget, localStorage);
  const second = recoveryTab(secondPath === "/rules" ? firstTarget : secondPath, localStorage);

  expect(recoverIn(first)).toBe(true);
  expect(recoverIn(second)).toBe(true);
  expect(recoverIn(first)).toBe(false);
  expect(recoverIn(second)).toBe(false);
  expect(first.location.reload).toHaveBeenCalledTimes(1);
  expect(second.location.reload).toHaveBeenCalledTimes(1);
  expect(first.sessionStorage.getItem(MARK_KEY)).toMatch(/^\[\{"path":"\/rules","at":\d+\}\]$/);
  expect(second.sessionStorage.getItem(MARK_KEY)).toStartWith(`[{"path":"${secondPath}","at":`);
  expect(localStorage.getItem(MARK_KEY)).toBeNull();
});

test("同一标签页交替访问两个路径时分别保留重载保护", () => {
  const tab = recoveryTab("/rules", memoryStorage());
  expect(recoverIn(tab)).toBe(true);
  tab.location.pathname = "/about";
  expect(recoverIn(tab)).toBe(true);
  tab.location.pathname = "/rules";
  expect(recoverIn(tab)).toBe(false);
  tab.location.pathname = "/about";
  expect(recoverIn(tab)).toBe(false);
  expect(tab.location.reload).toHaveBeenCalledTimes(2);
});

test("更换查询条件或片段仍按同一页面防止重载循环", () => {
  const tab = recoveryTab("/bidlog?world=main#top", memoryStorage());
  expect(recoverIn(tab)).toBe(true);
  tab.location.search = "?world=isekai";
  tab.location.hash = "#lots";
  expect(recoverIn(tab)).toBe(false);
  expect(tab.sessionStorage.getItem(MARK_KEY)).toMatch(/^\[\{"path":"\/bidlog","at":\d+\}\]$/);
});

test("会话存储不能写入时停止重载", () => {
  const tab = recoveryTab("/rules", memoryStorage());
  tab.sessionStorage.setItem = () => {
    throw new Error("storage blocked");
  };
  expect(recoverIn(tab)).toBe(false);
  expect(tab.location.reload).not.toHaveBeenCalled();
});

test("防循环标记写入成功后自动重载抛错，返回失败", () => {
  const tab = recoveryTab(ROUTE_PATH, memoryStorage());
  let markAtReload: string | null = null;
  tab.location.reload.mockImplementation(() => {
    markAtReload = tab.sessionStorage.getItem(MARK_KEY);
    throw new Error("拒绝自动重载");
  });

  const reloading = recoverIn(tab);

  expect(tab.location.reload).toHaveBeenCalledTimes(1);
  expect(markAtReload).toMatch(/^\[\{"path":"\/bidlog","at":\d+\}\]$/);
  expect(reloading).toBe(false);
});
