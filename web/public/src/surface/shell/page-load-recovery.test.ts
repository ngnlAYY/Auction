// Public 分块失败时，竞价记录保护与两个手动刷新入口共用生产外壳。

import { expect, jest, mock, spyOn, test } from "bun:test";
import { setImmediate } from "node:timers";
import { t } from "@auction/web-shared/ui/i18n";
import { flush } from "solid-js";
import {
  bidReceipt,
  type CompetitionPage,
  confirm,
  dispatch,
  hasClass,
  LOT_ID,
  mountCompetition,
  openBid,
  renderedText,
  SECOND_LOT_ID,
  type SentRequest,
  settle,
  waitFor,
} from "../commands/competition-fixture.test";
import { installDialogFocus } from "./dialog-focus-fixture.test";
import { DomElement, deferLotPage, deferPublicPage } from "./navigation-fixture.test";
import { reject } from "./session-fixture";

const TARGET = `/lots/${LOT_ID}?world=main#details`;
const MARK_KEY = "hv.page-load-reload";
const ORIGINAL_MARK = '[{"path":"/about","at":1700000000000}]';
const RELOAD_TIME = 1_800_000_000_000;

function installReloadBrowser(mark: string | null, writable = true) {
  const storage = new Map<string, string>();
  if (mark !== null) storage.set(MARK_KEY, mark);
  const setItem = mock((key: string, value: string) => {
    if (!writable) throw new Error("测试拒绝写入会话存储");
    storage.set(key, value);
  });
  const reload = mock(() => {});
  const ask = mock((_message: string) => false);
  Object.assign(window, {
    confirm: ask,
    sessionStorage: { getItem: (key: string) => storage.get(key) ?? null, setItem },
  });
  Object.assign(window.location, { reload });
  return { storage, setItem, reload, ask };
}

function failure(page: CompetitionPage) {
  return page.find(
    (element) =>
      element.getAttribute("role") === "alert" && renderedText(element) === t("page-load-failed"),
  );
}

function refreshButton(page: CompetitionPage, entry: "页头" | "页内") {
  return page.need(
    (element) =>
      element.tag === "button" &&
      (entry === "页头"
        ? element.getAttribute("aria-label") === t("realtime-refresh")
        : renderedText(element) === t("stale-build-refresh")),
  );
}

function unknownNotice(page: CompetitionPage) {
  return page.find((element) => hasClass(element, "public-command-notices"));
}

function receipt(page: CompetitionPage) {
  return page.find((element) => hasClass(element, "public-command-notices__receipt"));
}

function noticeButton(
  page: CompetitionPage,
  key: string,
  parent: DomElement | null = unknownNotice(page),
) {
  if (parent === null) throw new Error("竞价恢复提示未挂载");
  return page.need(
    (element) => element.tag === "button" && renderedText(element) === t(key),
    parent,
  );
}

async function firstBid(page: CompetitionPage): Promise<SentRequest> {
  await openBid(page);
  const form = page.need(
    (element) => element.tag === "form" && hasClass(element, "public-auction-bid-form"),
  );
  const sent = await confirm(page, form);
  expect(sent.path).toBe(`/api/v1/lots/${LOT_ID}/bids`);
  expect(sent.body).toMatch(/"command_id":"[0-9a-f-]+"/u);
  return sent;
}

async function retryBid(page: CompetitionPage, first: SentRequest): Promise<SentRequest> {
  const before = page.commands.length;
  dispatch(noticeButton(page, "data-retry"), "Click");
  await waitFor(() => page.commands.length > before);
  const sent = page.commands[before];
  if (sent === undefined) throw new Error("未发出原样重试");
  // 正文包含原 command_id，也涵盖金额与版本等其他冻结字段。
  expect([sent.path, sent.body, sent.expectedUser]).toEqual([
    first.path,
    first.body,
    first.expectedUser,
  ]);
  return sent;
}

const scenarios = ["首次提交在途", "原样重试在途", "结果未知", "成功回执", "拒绝回执"] as const;

test.each(scenarios)("分块失败保护%s，页头与页内刷新均先确认", async (scenario) => {
  const page = await mountCompetition();
  const chunk = await deferLotPage();
  try {
    const browser = installReloadBrowser(ORIGINAL_MARK);
    const first = await firstBid(page);
    page.go("/status");
    await waitFor(() => page.find((element) => hasClass(element, "public-status-page")) !== null);
    let pending = first;
    if (scenario === "原样重试在途" || scenario === "结果未知") {
      first.respond(reject("storage_unavailable", 503));
      await waitFor(() => unknownNotice(page) !== null);
      if (scenario === "原样重试在途") pending = await retryBid(page, first);
    } else if (scenario === "成功回执" || scenario === "拒绝回执") {
      first.respond(scenario === "成功回执" ? bidReceipt() : reject("bid_too_low", 409));
      await waitFor(() => receipt(page) !== null);
    }
    const visibleReceipt = receipt(page);

    page.go(TARGET);
    await chunk.started;
    chunk.fail();
    await waitFor(() => failure(page) !== null);
    expect(browser.reload).not.toHaveBeenCalled();
    expect(browser.setItem).not.toHaveBeenCalled();
    expect(receipt(page)).toBe(visibleReceipt);
    for (const entry of ["页头", "页内"] as const) {
      dispatch(refreshButton(page, entry), "Click");
      expect(browser.ask).toHaveBeenLastCalledWith(t("command-reload-confirm"));
      expect(browser.reload).not.toHaveBeenCalled();
      expect(browser.storage.get(MARK_KEY)).toBe(ORIGINAL_MARK);
      expect(receipt(page)).toBe(visibleReceipt);
      expect(first.signal.aborted).toBe(false);
    }
    expect(browser.ask).toHaveBeenCalledTimes(2);

    const addresses: string[] = [];
    browser.reload.mockImplementation(() => {
      addresses.push(page.location.href);
    });
    browser.ask.mockReturnValue(true);
    for (const entry of ["页头", "页内"] as const) dispatch(refreshButton(page, entry), "Click");
    expect(browser.ask).toHaveBeenCalledTimes(4);
    expect(addresses).toEqual([`https://auction.test${TARGET}`, `https://auction.test${TARGET}`]);
    expect(browser.setItem).not.toHaveBeenCalled();
    // 重载替身保留当前文档，因而可以在相同的失败页验证原请求仍可重试。
    if (visibleReceipt === null) {
      if (scenario !== "结果未知") pending.respond(reject("storage_unavailable", 503));
      await waitFor(
        () =>
          unknownNotice(page) !== null &&
          page.find(
            (element) =>
              element.tag === "button" &&
              renderedText(element) === t("data-retry") &&
              element.attributes.disabled === false,
          ) !== null,
      );
      const retry = await retryBid(page, first);
      retry.respond(bidReceipt());
      await waitFor(() => receipt(page) !== null);
    }
  } finally {
    chunk.restore();
    page.dispose();
  }
});

test.each(["关闭回执", "放弃未知命令"] as const)(
  "%s清空宿主后下一次分块失败自动重载",
  async (action) => {
    const focus = installDialogFocus();
    try {
      const page = await mountCompetition();
      focus.attach(page.find);
      const chunk = await deferLotPage();
      const nextChunk = await deferPublicPage("AuctionIndex");
      try {
        const browser = installReloadBrowser(null);
        const sent = await firstBid(page);
        page.go(TARGET);
        await chunk.started;
        sent.respond(action === "关闭回执" ? bidReceipt() : reject("storage_unavailable", 503));
        await waitFor(() =>
          action === "关闭回执" ? receipt(page) !== null : unknownNotice(page) !== null,
        );
        chunk.fail();
        await waitFor(() => failure(page) !== null);
        expect(browser.reload).not.toHaveBeenCalled();

        if (action === "关闭回执") {
          dispatch(
            page.need((element) => hasClass(element, "public-command-notices__receipt-close")),
            "Click",
          );
        } else {
          dispatch(noticeButton(page, "command-abandon"), "Click");
          const decision = page.need((element) =>
            hasClass(element, "public-command-notices__confirmation"),
          );
          dispatch(noticeButton(page, "command-abandon", decision), "Click");
        }
        expect(receipt(page)).toBeNull();
        expect(unknownNotice(page)).toBeNull();
        dispatch(refreshButton(page, "页内"), "Click");
        dispatch(refreshButton(page, "页头"), "Click");
        expect(browser.ask).not.toHaveBeenCalled();
        expect(browser.reload).toHaveBeenCalledTimes(2);
        expect(browser.setItem).not.toHaveBeenCalled();

        page.go("/?world=main#lots");
        await nextChunk.started;
        nextChunk.fail();
        await waitFor(() => browser.reload.mock.calls.length === 3);
        expect(browser.setItem).toHaveBeenCalledTimes(1);
        expect(browser.storage.get(MARK_KEY)).toStartWith('[{"path":"/","at":');
      } finally {
        nextChunk.restore();
        chunk.restore();
        page.dispose();
      }
    } finally {
      focus.restore();
    }
  },
);

test.each(["回执刚到达", "记录刚清空"] as const)(
  "页头等待期间到达的分块错误仍按即时竞价状态确认（%s）",
  async (record) => {
    const page = await mountCompetition();
    const chunk = await deferLotPage();
    try {
      const browser = installReloadBrowser(null);
      const sent = await firstBid(page);
      page.go(TARGET);
      await chunk.started;
      jest.useFakeTimers();
      dispatch(refreshButton(page, "页头"), "Click");
      expect(browser.ask).not.toHaveBeenCalled();
      sent.respond(bidReceipt());
      chunk.fail();
      await new Promise<void>((resolve) => setImmediate(resolve));
      flush();
      expect(failure(page)).not.toBeNull();
      expect(receipt(page)).not.toBeNull();
      if (record === "记录刚清空") {
        dispatch(
          page.need((element) => hasClass(element, "public-command-notices__receipt-close")),
          "Click",
        );
        expect(receipt(page)).toBeNull();
      }
      jest.advanceTimersByTime(80);
      await new Promise<void>((resolve) => setImmediate(resolve));
      flush();
      expect(browser.setItem).not.toHaveBeenCalled();
      if (record === "记录刚清空") {
        expect(browser.ask).not.toHaveBeenCalled();
        expect(browser.reload).toHaveBeenCalledTimes(1);
      } else {
        expect(browser.ask).toHaveBeenCalledTimes(1);
        expect(browser.reload).not.toHaveBeenCalled();
        expect(receipt(page)).not.toBeNull();
      }
    } finally {
      jest.useRealTimers();
      chunk.restore();
      page.dispose();
    }
  },
);

test("确认和重载抛错时保留回执，离页后旧按钮失效", async () => {
  const page = await mountCompetition();
  const chunk = await deferLotPage();
  try {
    const browser = installReloadBrowser(null);
    const sent = await firstBid(page);
    page.go(TARGET);
    await chunk.started;
    sent.respond(bidReceipt());
    chunk.fail();
    await waitFor(() => failure(page) !== null && receipt(page) !== null);
    const button = refreshButton(page, "页内");
    const visibleReceipt = receipt(page);
    browser.ask.mockImplementationOnce(() => {
      throw new Error("拒绝打开确认");
    });
    dispatch(button, "Click");
    expect(browser.reload).not.toHaveBeenCalled();
    browser.ask.mockReturnValue(true);
    browser.reload.mockImplementationOnce(() => {
      throw new Error("拒绝重载");
    });
    dispatch(button, "Click");
    expect(browser.reload).toHaveBeenCalledTimes(1);
    expect(receipt(page)).toBe(visibleReceipt);
    expect(failure(page)).not.toBeNull();
    expect(browser.setItem).not.toHaveBeenCalled();

    page.go("/status");
    await waitFor(() => page.find((element) => hasClass(element, "public-status-page")) !== null);
    expect(failure(page)).toBeNull();
    dispatch(button, "Click");
    expect(browser.ask).toHaveBeenCalledTimes(2);
    expect(browser.reload).toHaveBeenCalledTimes(1);
  } finally {
    chunk.restore();
    page.dispose();
  }
});

test("旧导航的分块失败不会重载当前地址", async () => {
  const page = await mountCompetition("main", false, { href: "/status" });
  const chunk = await deferLotPage();
  try {
    const browser = installReloadBrowser(null);
    page.go(TARGET);
    await chunk.started;
    page.go("/status");
    await waitFor(() => page.find((element) => hasClass(element, "public-status-page")) !== null);
    chunk.fail();
    await settle();
    expect(browser.reload).not.toHaveBeenCalled();
    expect(browser.setItem).not.toHaveBeenCalled();
    expect(failure(page)).toBeNull();
  } finally {
    chunk.restore();
    page.dispose();
  }
});

test("首屏分块失败且会话存储拒写时揭开骨架，仍能使用导航", async () => {
  const focus = installDialogFocus();
  const attributes = { id: "main", "aria-busy": "true" };
  const startup = Object.assign(new DomElement("div", attributes), {
    hasAttribute: (name: string) => Object.hasOwn(attributes, name),
    setAttribute: (name: string, value: string) => Reflect.set(attributes, name, value),
  });
  const chunk = await deferPublicPage("AuctionIndex");
  try {
    const page = await mountCompetition();
    focus.attach((find, parent) => (find(startup) ? startup : page.find(find, parent)));
    try {
      const browser = installReloadBrowser(null, false);
      await chunk.started;
      expect(startup.getAttribute("aria-busy")).toBe("true");
      chunk.fail();
      await waitFor(() => failure(page) !== null);
      expect(startup.getAttribute("data-app-ready")).toBe("true");
      expect(startup.getAttribute("aria-busy")).toBe("false");
      expect(browser.reload).not.toHaveBeenCalled();
      expect(browser.setItem).toHaveBeenCalledTimes(1);
      const bidLogLink = page.need(
        (element) =>
          element.tag === "a" && element.getAttribute("href")?.startsWith("/bidlog") === true,
      );
      dispatch(bidLogLink, "Click");
      await waitFor(
        () => page.find((element) => hasClass(element, "public-auction-bid-log-page")) !== null,
      );
      expect(failure(page)).toBeNull();
    } finally {
      page.dispose();
    }
  } finally {
    chunk.restore();
    focus.restore();
  }
});

test("首屏分块失败且自动重载抛错时揭开骨架，仍能使用导航", async () => {
  const focus = installDialogFocus();
  const attributes = { id: "main", "aria-busy": "true" };
  const startup = Object.assign(new DomElement("div", attributes), {
    hasAttribute: (name: string) => Object.hasOwn(attributes, name),
    setAttribute: (name: string, value: string) => Reflect.set(attributes, name, value),
  });
  const chunk = await deferPublicPage("AuctionIndex");
  try {
    const page = await mountCompetition();
    focus.attach((find, parent) => (find(startup) ? startup : page.find(find, parent)));
    try {
      const browser = installReloadBrowser(null);
      browser.reload.mockImplementation(() => {
        throw new Error("拒绝自动重载");
      });
      await chunk.started;
      expect(startup.getAttribute("aria-busy")).toBe("true");
      chunk.fail();
      await waitFor(() => browser.reload.mock.calls.length === 1);
      await settle();
      expect(browser.setItem).toHaveBeenCalledTimes(1);
      expect(browser.storage.get(MARK_KEY)).toStartWith('[{"path":"/","at":');
      expect(browser.ask).not.toHaveBeenCalled();
      expect(failure(page)).not.toBeNull();
      expect(startup.getAttribute("data-app-ready")).toBe("true");
      expect(startup.getAttribute("aria-busy")).toBe("false");

      const bidLogLink = page.need(
        (element) =>
          element.tag === "a" && element.getAttribute("href")?.startsWith("/bidlog") === true,
      );
      dispatch(bidLogLink, "Click");
      await waitFor(
        () => page.find((element) => hasClass(element, "public-auction-bid-log-page")) !== null,
      );
      expect(failure(page)).toBeNull();
    } finally {
      page.dispose();
    }
  } finally {
    chunk.restore();
    focus.restore();
  }
});

test("空宿主首次重载完整地址，30 秒内同路径的新查询与片段显示错误", async () => {
  const page = await mountCompetition("main", false, { href: "/status" });
  const chunk = await deferLotPage();
  const clock = spyOn(Date, "now").mockReturnValue(RELOAD_TIME);
  try {
    const browser = installReloadBrowser(null);
    const addresses: string[] = [];
    browser.reload.mockImplementation(() => {
      addresses.push(page.location.href);
    });
    page.go(TARGET);
    await chunk.started;
    chunk.fail();
    await waitFor(() => addresses.length === 1);
    expect(addresses).toEqual([`https://auction.test${TARGET}`]);
    expect(browser.storage.get(MARK_KEY)).toStartWith(`[{"path":"/lots/${LOT_ID}","at":`);
    chunk.restore();
    page.go("/status");

    const repeated = await deferLotPage();
    try {
      page.go(`/lots/${LOT_ID}?world=isekai#history`);
      await repeated.started;
      repeated.fail();
      await waitFor(() => failure(page) !== null);
      expect(addresses).toHaveLength(1);
      expect(browser.setItem).toHaveBeenCalledTimes(1);
      repeated.restore();
      page.go("/status");
      const different = await deferLotPage();
      try {
        page.go(`/lots/${SECOND_LOT_ID}`);
        await different.started;
        different.fail();
        await waitFor(() => addresses.length === 2);
        expect(addresses[1]).toBe(`https://auction.test/lots/${SECOND_LOT_ID}`);
      } finally {
        different.restore();
      }
    } finally {
      repeated.restore();
    }
  } finally {
    clock.mockRestore();
    chunk.restore();
    page.dispose();
  }
});

test("新的失败路径只保留最近八条自动重载标记", async () => {
  const page = await mountCompetition("main", false, { href: "/status" });
  const chunk = await deferLotPage();
  const previous = Array.from({ length: 8 }, (_, index) => ({
    path: `/lots/previous-${index}`,
    at: RELOAD_TIME,
  }));
  const clock = spyOn(Date, "now").mockReturnValue(RELOAD_TIME);
  try {
    const browser = installReloadBrowser(JSON.stringify(previous));
    page.go(TARGET);
    await chunk.started;
    chunk.fail();
    await waitFor(() => browser.reload.mock.calls.length === 1);
    const serialized = browser.storage.get(MARK_KEY);
    expect(serialized).toStartWith(JSON.stringify(previous.slice(1)).slice(0, -1));
    expect(serialized).not.toContain("/lots/previous-0");
    expect(serialized).toContain(`"path":"/lots/${LOT_ID}"`);
    expect(browser.setItem).toHaveBeenCalledTimes(1);
  } finally {
    clock.mockRestore();
    chunk.restore();
    page.dispose();
  }
});
