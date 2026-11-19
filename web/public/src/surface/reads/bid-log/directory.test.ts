// 出价、履约页面的目录门禁、按需翻页与地址写入失败恢复。

import { expect, jest, onTestFinished, spyOn, test } from "bun:test";
import { t } from "@auction/web-shared/ui/i18n";
import { flush } from "solid-js";

import { resetStoreForTest } from "../../../api/swr-cache";
import {
  dispatch,
  mountCompetition,
  renderedText,
  settle,
  waitFor,
} from "../../commands/competition-fixture.test";
import { mountReadPage } from "../../pagination/browser-fixture";
import { success } from "../../shell/session-fixture";
import { Fulfillment } from "../fulfillment";
import { BidLog } from ".";
import { mountRoundPage } from "./page-tree-fixture.test";

function settledAuction(sequence: number) {
  return {
    id: `00000000-0000-4000-8000-${String(sequence).padStart(12, "0")}`,
    world_id: "main",
    sequence,
    status: "settled",
    scheduled_open_at: "2026-01-01T00:00:00Z",
    settled_at: "2026-01-02T00:00:00Z",
    lot_count: 0,
    sold_count: 0,
    unsold_count: 0,
    sold_amount: 0,
    state_version: 1,
  };
}

const LATEST = settledAuction(201);
const OLDER = settledAuction(1);
const FIRST_PAGE = Array.from({ length: 200 }, (_, index) => settledAuction(201 - index));

function mockReadRequests(
  first: readonly ReturnType<typeof settledAuction>[],
  second: () => Promise<Response> = async () => directoryPage([OLDER], 2, first.length + 1),
  total = first.length + 1,
) {
  resetStoreForTest();
  const urls: URL[] = [];
  const fetcher = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(
      async (input: URL | RequestInfo) => {
        const url = new URL(String(input), "https://auction.test");
        urls.push(url);
        if (url.pathname !== "/api/v1/auctions") {
          return new Response("{}", { status: 503 });
        }
        if (url.searchParams.get("status") !== "settled") {
          return directoryPage([], 1, 0);
        }
        return url.searchParams.get("page") === "1" ? directoryPage(first, 1, total) : second();
      },
      { preconnect() {} },
    ),
  );
  return {
    urls,
    dispose() {
      fetcher.mockRestore();
      resetStoreForTest();
    },
  };
}

function directoryPage(
  auctions: readonly ReturnType<typeof settledAuction>[],
  page: number,
  total: number,
) {
  return Response.json({
    code: "ok",
    message: "ok",
    data: { auctions, page, page_size: 200, total },
  });
}

async function settleRead() {
  for (let turn = 0; turn < 3; turn += 1) {
    await Bun.sleep(0);
    flush();
  }
}

const pages = [
  ["/bidlog", "bid-log", BidLog],
  ["/fulfillment", "fulfillment", Fulfillment],
] as const;

test.each(
  pages.flatMap((entry) =>
    ["world=bad", "world=main&world=isekai"].map((world) => [...entry, world] as const),
  ),
)("%s 尾斜杠地址在规范化拒写与历史恢复时阻断读取（%s）", async (path, api, render, world) => {
  const reads = mockReadRequests([{ ...LATEST, world_id: "isekai" }], undefined, 1);
  const href = `${path}/?${world}&auction=${LATEST.id}&bidlog_page=2`;
  const mounted = mountReadPage(href, render, undefined, true);
  const expectBlocked = () => {
    expect(`${mounted.location.pathname}${mounted.location.search}`).toBe(href);
    expect(
      reads.urls.filter(
        (url) => url.pathname === "/api/v1/auctions" || url.pathname.endsWith(`/${api}`),
      ),
    ).toEqual([]);
    expect(mounted.loading.peek().size).toBe(0);
  };
  try {
    await settleRead();
    expectBlocked();
    mounted.rejectWrites(false);
    mounted.router.push(`${path}?world=also-bad`);
    expect(mounted.location.pathname).toBe(path);
    mounted.rejectWrites(true);
    mounted.back();
    await settleRead();
    expectBlocked();
  } finally {
    mounted.dispose();
    reads.dispose();
  }
});

test.each(
  pages.flatMap(
    ([path, api]) =>
      [
        [path, api, "world=bad", null, "auction-history-world-all"],
        [path, api, "world=main&w%6Frld=main", "main", "auction-world-main"],
        [path, api, "world=%20isekai", "isekai", "auction-world-isekai"],
      ] as const,
  ),
)("%s 在世界修正成功前不读取目录或正文（%s，%s）", async (path, api, world, selected, label) => {
  const search = `?${world}&auction=${LATEST.id}&page=9223372036854775807&keep=%2b%20x`;
  const reads = mockReadRequests([{ ...LATEST, world_id: selected ?? "main" }], undefined, 1);
  const page = await mountRoundPage(`${path}${search}`, path === "/bidlog");
  const directory = () => reads.urls.filter((url) => url.pathname === "/api/v1/auctions");
  const body = () => reads.urls.filter((url) => url.pathname.endsWith(`/${api}`));
  try {
    await settleRead();
    const error = () => page.find(({ attributes }) => attributes.id === "public-read-world-error");
    expect(renderedText(error())).toContain(
      t(world.includes("&") ? "filter-error-duplicate" : "world-invalid-link"),
    );
    expect(error()?.attributes.role).toBe("alert");
    expect(directory()).toEqual([]);
    expect(body()).toEqual([]);
    expect(page.loading.peek().size).toBe(0);
    expect(page.location.search).toBe(search);
    const choose = page.find(
      (node) =>
        node.tag === "button" &&
        node.attributes["aria-describedby"] === "public-read-world-error" &&
        renderedText(node) === t(label),
    );
    if (choose === null) throw new Error("未挂载世界修正按钮");
    page.rejectWrites(true);
    choose.click();
    await settleRead();
    expect(directory()).toEqual([]);
    expect(body()).toEqual([]);
    expect(page.location.search).toBe(search);
    expect(error()?.attributes.role).toBe("alert");

    page.rejectWrites(false);
    choose.click();
    await settleRead();
    expect(error()).toBeNull();
    expect(page.location.searchParams.getAll("world")).toEqual(selected === null ? [] : [selected]);
    expect(page.location.search).toContain("keep=%2b%20x");
    expect(directory().map((url) => url.searchParams.get("world"))).toEqual(
      path === "/bidlog" ? [selected, selected, selected] : [selected],
    );
    expect(
      body().map((url) => [url.searchParams.get("world"), url.searchParams.get("page")]),
    ).toEqual([[selected ?? "main", "1"]]);
  } finally {
    page.dispose();
    reads.dispose();
  }
});

test.each(
  pages.flatMap(
    ([path, api]) =>
      [
        [path, api, "clear"],
        [path, api, "select"],
      ] as const,
  ),
)("%s 的非法或重复场次需要显式恢复（%s，%s）", async (path, api, action) => {
  const page = await mountCompetition("main", true);
  const suffix = `/${api}`;
  const body = () => page.reads.filter((read) => read.split("?")[0]?.endsWith(suffix));
  const locationAtRead: string[][] = [];
  const raw = action === "clear" ? "auction=bad" : `auction=${LATEST.id}&a%75ction=${OLDER.id}`;
  const search = `?${raw}&lot=One12&keep=%2b%20x`;
  try {
    page.server.readRounds = (url) =>
      url.searchParams.get("status") === "settled"
        ? directoryPage([LATEST, OLDER], 1, 2)
        : directoryPage([], 1, 0);
    const pageFields = (url: URL) => {
      locationAtRead.push(page.location.searchParams.getAll("auction"));
      return {
        page: Number(url.searchParams.get("page")),
        page_size: Number(url.searchParams.get("page_size")),
        total: 0,
      };
    };
    page.server.readBidLog = (url) => success({ bids: [], ...pageFields(url) });
    page.server.readPage = (url) => {
      const id = url.pathname.split("/").at(-2);
      const round = [LATEST, OLDER].find((auction) => auction.id === id);
      return url.pathname.endsWith("/fulfillment") && round !== undefined
        ? success({
            auction: { id: round.id, world: round.world_id, number: round.sequence },
            lots: [],
            ...pageFields(url),
          })
        : null;
    };
    page.go(`${path}${search}`);
    await waitFor(
      () =>
        page.find(
          (element) => element.tag === "option" && element.getAttribute("value") === LATEST.id,
        ) !== null,
    );
    await settle();
    expect(page.location.search).toBe(search);
    expect(body()).toEqual([]);
    const alert = () =>
      page.find(
        (element) =>
          element.getAttribute("role") === "alert" &&
          renderedText(element) === t("auction-invalid-link"),
      );
    const alertId = alert()?.getAttribute("id");
    if (!alertId) throw new Error("未挂载非法场次提示");
    const recover = page.need(
      (element) => element.tag === "button" && renderedText(element) === t("auction-view-current"),
    );
    const select = Object.assign(
      page.need(
        (element) => element.tag === "select" && element.getAttribute("name") === "auction",
      ),
      { value: "" },
    );
    expect(select.getAttribute("aria-invalid")).toBe("true");
    expect(select.getAttribute("aria-describedby")).toBe(alertId);
    const apply = () => {
      if (action === "clear") {
        dispatch(recover, "Click");
      } else {
        select.value = OLDER.id;
        dispatch(select, "Change");
      }
    };
    page.rejectWrites("push");
    apply();
    await settle();
    expect(page.location.search).toBe(search);
    expect(alert()).not.toBeNull();
    expect(select.value).toBe("");
    expect(body()).toEqual([]);

    page.rejectWrites(false);
    apply();
    await waitFor(() => body().length !== 0);
    await settle();
    const selected = action === "clear" ? LATEST.id : OLDER.id;
    const urls = body().map((value) => new URL(value, page.location));
    expect(
      urls.map((url) => [url.pathname, url.searchParams.get("page"), url.searchParams.get("lot")]),
    ).toEqual([[`/api/v1/auctions/${selected}/${api}`, "1", null]]);
    expect(page.location.searchParams.getAll("auction")).toEqual([selected]);
    expect(locationAtRead).toEqual([[selected]]);
    expect(page.location.search).toContain("keep=%2b%20x");
    expect(alert()).toBeNull();
    expect(select.getAttribute("aria-invalid")).toBeNull();
  } finally {
    page.dispose();
  }
});

test.each(pages)("%s 的非法场次恢复后，目录晚于计时器返回也不会写回旧 lot", async (path, api) => {
  const page = await mountCompetition("main", true);
  const directory = Promise.withResolvers<Response>();
  const body = () => page.reads.filter((read) => read.split("?")[0]?.endsWith(`/${api}`));
  try {
    page.server.readRounds = (url) =>
      url.searchParams.get("status") === "settled" ? directory.promise : directoryPage([], 1, 0);
    page.server.readBidLog = () => success({ bids: [], page: 1, page_size: 20, total: 0 });
    page.server.readPage = (url) =>
      url.pathname.endsWith("/fulfillment")
        ? success({
            auction: { id: LATEST.id, world: "main", number: LATEST.sequence },
            lots: [],
            page: 1,
            page_size: 20,
            total: 0,
          })
        : null;
    page.go(`${path}?auction=invalid&keep=%2b%20x`);
    const recover = () =>
      page.find(
        (element) =>
          element.tag === "button" && renderedText(element) === t("auction-view-current"),
      );
    await waitFor(() => recover() !== null);
    const input = Object.assign(
      page.need((element) => element.tag === "input" && element.getAttribute("name") === "lot"),
      { value: "One12" },
    );
    jest.useFakeTimers();
    onTestFinished(() => jest.useRealTimers());
    dispatch(input, "Input");
    expect(input.getAttribute("value")).toBe("One12");
    const button = recover();
    if (button === null) throw new Error("未挂载恢复按钮");
    dispatch(button, "Click");
    expect(page.location.searchParams.has("auction")).toBe(false);
    jest.advanceTimersByTime(300);
    for (let turn = 0; turn < 10; turn += 1) {
      await Promise.resolve();
      flush();
    }
    expect(page.location.search).toBe("?keep=%2b%20x");
    expect(body()).toEqual([]);

    jest.useRealTimers();
    directory.resolve(directoryPage([LATEST], 1, 1));
    await waitFor(() => body().length > 0);
    expect(body().map((read) => new URL(read, page.location).searchParams.get("lot"))).toEqual([
      null,
    ]);
    expect(page.location.searchParams.get("auction")).toBe(LATEST.id);
    expect(page.location.searchParams.has("lot")).toBe(false);
  } finally {
    directory.resolve(directoryPage([], 1, 0));
    page.dispose();
  }
});

test.each(pages.flatMap((entry) => [3, 1002].map((page) => [...entry, page] as const)))(
  "%s 默认选场与页码重置只写一次，正文从第一页开始",
  async (path, api, render, page) => {
    const reads = mockReadRequests([LATEST], undefined, 1);
    const search = `?world=main&page=${page}&page_size=10`;
    const mounted = mountReadPage(`${path}${search}`, render);
    try {
      await settleRead();
      const body = reads.urls.filter(
        (url) => url.pathname === `/api/v1/auctions/${LATEST.id}/${api}`,
      );
      expect(body.map((url) => url.searchParams.get("page"))).toEqual(["1"]);
      expect(body[0]?.searchParams.get("page_size")).toBe("10");
      const selected = mounted.writes.filter(({ url }) => url.search !== search);
      expect(selected.map(({ mode, url }) => [mode, url.search])).toEqual([
        ["replace", `?world=main&page_size=10&auction=${LATEST.id}`],
      ]);
      expect(mounted.location.searchParams.get("page")).toBeNull();
    } finally {
      mounted.dispose();
      reads.dispose();
    }
  },
);

test.each(pages)(
  "%s 显示搜索框隐藏的换行，修正并写址后只读取一次正文",
  async (path, api, render) => {
    const reads = mockReadRequests([LATEST], undefined, 1);
    const mounted = mountReadPage(`${path}?auction=${LATEST.id}&lot=One%0A12`, render);
    try {
      await settleRead();
      const input = mounted.elements.find(
        ({ tag, props }) => tag === "input" && props.name === "lot",
      );
      const error = mounted.elements.find(
        ({ props }) => props.id === input?.props["aria-describedby"],
      );
      expect(error?.props.role).toBe("status");
      expect(
        mounted.elements.some(
          ({ tag, children }) => tag === "bdi" && children.includes('"One\\n12"'),
        ),
      ).toBe(true);
      const body = () => reads.urls.filter((url) => url.pathname.endsWith(`/${api}`));
      expect(body()).toHaveLength(0);
      const onInput = input?.props.onInput;
      if (typeof onInput !== "function") throw new Error("未挂载 lot 输入");
      jest.useFakeTimers();
      onTestFinished(() => jest.useRealTimers());
      const inputCorrection = () => onInput({ currentTarget: { value: "One12" } });
      const debounce = async (milliseconds: number) => {
        jest.advanceTimersByTime(milliseconds);
        for (let turn = 0; turn < 10; turn += 1) {
          await Promise.resolve();
          flush();
        }
      };
      mounted.rejectWrites(true);
      inputCorrection();
      await debounce(300);
      expect(mounted.location.searchParams.get("lot")).toBe("One\n12");
      expect(body()).toHaveLength(0);

      mounted.rejectWrites(false);
      inputCorrection();
      await debounce(299);
      expect(mounted.location.searchParams.get("lot")).toBe("One\n12");
      expect(body()).toHaveLength(0);
      await debounce(1);
      expect(mounted.location.searchParams.get("lot")).toBe("One12");
      expect(body().map((url) => [url.pathname, url.searchParams.get("lot")])).toEqual([
        [`/api/v1/auctions/${LATEST.id}/${api}`, "One12"],
      ]);
    } finally {
      mounted.dispose();
      reads.dispose();
      jest.useRealTimers();
    }
  },
);

test.each(pages)(
  "%s 默认场次确认后正文先读，场次下拉按需读取后续目录",
  async (path, api, render) => {
    const continuation = Promise.withResolvers<Response>();
    const reads = mockReadRequests(FIRST_PAGE, () => continuation.promise);
    const mounted = mountReadPage(`${path}?world=main`, render);
    const body = `/api/v1/auctions/${LATEST.id}/${api}`;
    try {
      await settleRead();
      expect(mounted.location.searchParams.get("auction")).toBe(LATEST.id);
      expect(reads.urls.map((url) => url.pathname)).toContain(body);
      expect(
        reads.urls.filter(
          (url) => url.pathname === "/api/v1/auctions" && url.searchParams.get("page") === "2",
        ),
      ).toHaveLength(0);

      const focus = mounted.elements.find(
        ({ tag, props }) => tag === "select" && props.name === "auction",
      )?.props.onFocus;
      if (typeof focus !== "function") throw new Error("未挂载场次下拉框");
      focus();
      await settleRead();
      expect(
        reads.urls.filter(
          (url) => url.pathname === "/api/v1/auctions" && url.searchParams.get("page") === "2",
        ),
      ).toHaveLength(1);
      continuation.resolve(directoryPage([OLDER], 2, 201));
      await settleRead();
      expect(reads.urls.filter((url) => url.pathname === body)).toHaveLength(1);
      focus();
      await settleRead();
      expect(
        reads.urls.filter(
          (url) => url.pathname === "/api/v1/auctions" && url.searchParams.get("page") === "2",
        ),
      ).toHaveLength(1);
    } finally {
      continuation.resolve(directoryPage([OLDER], 2, 201));
      mounted.dispose();
      reads.dispose();
    }
  },
);

test("全部世界中的异世界首页不能提前确认为默认场次", async () => {
  const continuation = Promise.withResolvers<Response>();
  const isekai = FIRST_PAGE.map((auction) => ({ ...auction, world_id: "isekai" }));
  const reads = mockReadRequests(isekai, () => continuation.promise);
  const mounted = mountReadPage("/fulfillment", Fulfillment);
  try {
    await settleRead();
    expect(reads.urls.some((url) => url.searchParams.get("page") === "2")).toBe(true);
    expect(reads.urls.some((url) => url.pathname.endsWith("/fulfillment"))).toBe(false);

    continuation.resolve(directoryPage([OLDER], 2, 201));
    await settleRead();
    expect(mounted.location.searchParams.get("auction")).toBe(OLDER.id);
    const body = reads.urls.find((url) => url.pathname.endsWith("/fulfillment"));
    expect(body?.pathname).toBe(`/api/v1/auctions/${OLDER.id}/fulfillment`);
    expect(body?.searchParams.get("world")).toBe("main");
  } finally {
    continuation.resolve(directoryPage([OLDER], 2, 201));
    mounted.dispose();
    reads.dispose();
  }
});

test.each(pages)("%s 切换到非法世界后，旧目录迟到也不续读或写回场次", async (path, api, render) => {
  const latePage = Promise.withResolvers<Response>();
  const reads = mockReadRequests(FIRST_PAGE, () => latePage.promise);
  const mounted = mountReadPage(`${path}?world=main&auction=${OLDER.id}`, render);
  try {
    await settleRead();
    const before = reads.urls.filter((url) => url.pathname === "/api/v1/auctions").length;
    expect(reads.urls.some((url) => url.searchParams.get("page") === "2")).toBe(true);
    const search = `?world=bad&auction=${OLDER.id}`;
    mounted.router.push(`${path}${search}`);
    await settleRead();
    latePage.resolve(directoryPage([OLDER], 2, 201));
    await settleRead();
    expect(mounted.location.search).toBe(search);
    expect(reads.urls.filter((url) => url.pathname === "/api/v1/auctions")).toHaveLength(before);
    expect(reads.urls.some((url) => url.pathname.endsWith(`/${api}`))).toBe(false);
  } finally {
    latePage.resolve(directoryPage([OLDER], 2, 201));
    mounted.dispose();
    reads.dispose();
  }
});

test.each(pages)(
  "%s 深链场次要等可见目录确认，并按实际页长替换越界页码",
  async (path, api, render) => {
    const continuation = Promise.withResolvers<Response>();
    const reads = mockReadRequests(FIRST_PAGE, () => continuation.promise);
    const mounted = mountReadPage(
      `${path}?world=main&auction=${OLDER.id}&page_size=200&page=102`,
      render,
    );
    try {
      await settleRead();
      expect(reads.urls.some((url) => url.pathname.endsWith(`/${api}`))).toBe(false);
      expect(reads.urls.some((url) => url.searchParams.get("page") === "2")).toBe(true);
      const expectedPage = path === "/fulfillment" ? "101" : "51";
      expect(mounted.location.searchParams.get("page")).toBe(expectedPage);
      continuation.resolve(directoryPage([OLDER], 2, 201));
      await settleRead();
      const body = reads.urls.find((url) => url.pathname.endsWith(`/${api}`));
      expect(body?.pathname).toBe(`/api/v1/auctions/${OLDER.id}/${api}`);
      expect(body?.searchParams.get("page")).toBe(expectedPage);
      expect(body?.searchParams.get("page_size")).toBe(path === "/fulfillment" ? "100" : "200");
      expect(body?.searchParams.get("world")).toBe("main");
    } finally {
      continuation.resolve(directoryPage([OLDER], 2, 201));
      mounted.dispose();
      reads.dispose();
    }
  },
);

test.each(pages)("%s 单场自动替换被拒后结束加载，手动选择可恢复", async (path, api, render) => {
  const reads = mockReadRequests([LATEST], undefined, 1);
  const mounted = mountReadPage(path, render);
  mounted.rejectWrites(true);
  try {
    await settleRead();
    expect(mounted.location.searchParams.has("auction")).toBe(false);
    expect(mounted.loading.peek().size).toBe(0);
    expect(reads.urls.some((url) => url.pathname.endsWith(`/${api}`))).toBe(false);

    const change = mounted.elements.find(
      ({ tag, props }) => tag === "select" && props.name === "auction",
    )?.props.onChange;
    if (typeof change !== "function") throw new Error("未挂载场次下拉框");
    const select = { value: LATEST.id };
    change({ currentTarget: select });
    expect(select.value).toBe("");

    mounted.rejectWrites(false);
    select.value = LATEST.id;
    change({ currentTarget: select });
    await settleRead();
    expect(mounted.location.searchParams.get("auction")).toBe(LATEST.id);
    expect(reads.urls.some((url) => url.pathname === `/api/v1/auctions/${LATEST.id}/${api}`)).toBe(
      true,
    );
  } finally {
    mounted.dispose();
    reads.dispose();
  }
});
