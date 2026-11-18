// 从生产外壳挂载的锚点取地址，并用真实路由与页面观察导航结果。

import { expect, spyOn, test } from "bun:test";
import { flush } from "solid-js";
import { resetStoreForTest } from "../../api/swr-cache";
import { type DomElement, mountNavigation } from "../shell/navigation-fixture.test";
import { ALICE, reject, success } from "../shell/session-fixture";

const HISTORICAL_ID = "67e55044-10b1-426f-9247-bb680e5fe0c8";
const SETTLED = {
  id: "00000000-0000-4000-8000-000000000041",
  world_id: "main",
  sequence: 41,
  status: "settled",
  scheduled_open_at: "2026-01-01T00:00:00Z",
  settled_at: "2026-01-02T00:00:00Z",
  lot_count: 0,
  sold_count: 0,
  unsold_count: 0,
  sold_amount: 0,
  state_version: 1,
};
const PRIMARY = "public-primary-navigation";
const SHARED_PATHS = [
  "/",
  "/bidlog",
  "/fulfillment",
  "/auctions",
  "/history",
  "/rules",
  "/about",
  "/blacklist",
  "/compensation-pool",
  "/forum",
] as const;

async function mountPage(initialUrl = "/?world=main", otherWorldOpen = false) {
  resetStoreForTest();
  let identity: typeof ALICE | null = ALICE;
  const requests: URL[] = [];
  const fetcher = spyOn(globalThis, "fetch").mockImplementation(
    Object.assign(
      async (input: URL | RequestInfo) => {
        const url = new URL(String(input), "https://auction.test");
        requests.push(url);
        switch (url.pathname) {
          case "/api/v1/session":
            return success(identity);
          case "/api/v1/worlds":
            return success({
              worlds: { main: { enabled: true }, isekai: { enabled: otherWorldOpen } },
            });
          case "/api/v1/auctions/current":
            return success({ auction: null });
          case "/api/v1/auctions": {
            const auctions = url.searchParams.get("status") === "settled" ? [SETTLED] : [];
            return success({ auctions, page: 1, page_size: 200, total: auctions.length });
          }
          case "/api/v1/price-history":
            return success({ rows: [], page: 1, page_size: 20, total: 0 });
          case "/health/ready":
            return success({ ready: true });
          case "/api/v1/me/bids":
            return success({
              bids: [],
              proxies: [],
              bid_total: 0,
              proxy_total: 0,
              page: 1,
              bid_page: 1,
              proxy_page: 1,
              page_size: 10,
            });
          case "/api/v1/me/lot-watch-preference":
            return success({
              user_id: ALICE.user_id,
              channel: "none",
              version: 1,
              pm_available: false,
            });
          case "/api/v1/me/session-preference":
            return success({
              user_id: ALICE.user_id,
              duration_seconds: null,
              version: 1,
              expires_at: null,
            });
          default:
            return reject("storage_unavailable", 503);
        }
      },
      { preconnect() {} },
    ),
  );
  let mounted: Awaited<ReturnType<typeof mountNavigation>> | null = null;
  try {
    mounted = await mountNavigation(initialUrl);
  } catch (error) {
    fetcher.mockRestore();
    throw error;
  }
  const page = mounted;
  return {
    ...page,
    requests,
    endSession() {
      identity = null;
      page.focus();
    },
    dispose() {
      page.dispose();
      fetcher.mockRestore();
      resetStoreForTest();
    },
  };
}

async function waitFor(condition: () => boolean): Promise<void> {
  for (let turn = 0; turn < 100; turn += 1) {
    flush();
    if (condition()) return;
    await Bun.sleep(1);
  }
  throw new Error("等待外壳渲染或读取超时");
}

type Page = Awaited<ReturnType<typeof mountPage>>;

function requireElement(page: Page, predicate: (node: DomElement) => boolean, parent?: DomElement) {
  const node = page.find(predicate, parent);
  if (node === null) throw new Error("生产外壳未渲染所需元素");
  return node;
}

function href(page: Page, path: string, parent?: DomElement): string {
  const node = requireElement(
    page,
    (element) => {
      const target = element.getAttribute("href");
      return (
        element.tag === "a" && target !== null && new URL(target, page.location).pathname === path
      );
    },
    parent,
  );
  const target = node.getAttribute("href");
  if (target === null) throw new Error("导航锚点缺少 href");
  return target;
}

function press(node: DomElement): void {
  const click = node.attributes.onClick;
  if (typeof click !== "function") throw new Error("面板按钮没有事件处理器");
  click();
  flush();
}

function openPanel(page: Page, trigger: string): void {
  press(requireElement(page, (element) => element.getAttribute("id") === trigger));
}

test("手机设置到出价记录保留世界，桌面与手机通用入口都使用外壳范围", async () => {
  const page = await mountPage();
  try {
    openPanel(page, "public-navigation-toggle");
    const mobile = () => requireElement(page, (element) => element.getAttribute("id") === PRIMARY);
    const desktop = requireElement(page, (element) => element.tag === "aside");
    for (const path of SHARED_PATHS) {
      const expected = path === "/history" ? path : `${path}?world=main`;
      expect(href(page, path, mobile())).toBe(expected);
      expect(href(page, path, desktop)).toBe(expected);
    }
    const settings = href(page, "/settings", mobile());
    expect(settings).toBe("/settings?world=main");
    page.router.push(settings);
    flush();
    expect(page.router.currentLocationString()).toBe("/settings?world=main");
    const bidLog = href(page, "/bidlog", mobile());
    expect(bidLog).toBe("/bidlog?world=main");
    page.router.push(bidLog);
    await waitFor(() => page.requests.some((url) => url.pathname === "/api/v1/auctions"));
    expect(page.location.pathname).toBe("/bidlog");
    expect(page.location.searchParams.get("world")).toBe("main");
    expect(
      page.requests.find((url) => url.pathname === "/api/v1/auctions")?.searchParams.get("world"),
    ).toBe("main");
  } finally {
    page.dispose();
  }
});

test.each(["手机导航", "顶栏会话", "会话结束提示"])(
  "实际 %s 的登录入口继承世界",
  async (source) => {
    const page = await mountPage();
    try {
      await waitFor(
        () =>
          page.find(
            (element) => element.getAttribute("id") === "public-account-session-trigger",
          ) !== null,
      );
      page.endSession();
      const loginClass =
        source === "会话结束提示"
          ? "public-shell__session-ended-login"
          : "public-account-session-login";
      const region = () =>
        source === "手机导航"
          ? page.find((element) => element.getAttribute("id") === PRIMARY)
          : page.find(
              (element) =>
                element.tag === "a" &&
                element.attributes.class?.toString().includes(loginClass) === true,
            );
      await waitFor(
        () =>
          region() !== null &&
          page.find(
            (element) => element.getAttribute("href")?.startsWith("/login") === true,
            region() ?? undefined,
          ) !== null,
      );
      const login = href(page, "/login", region() ?? undefined);
      expect(login).toBe("/login?world=main");
      page.router.push(login);
      flush();
      expect(page.router.currentLocationString()).toBe("/login?world=main");
    } finally {
      page.dispose();
    }
  },
);

test("从历史看板切回唯一开放的主世界，实际出价、我的和履约保持范围", async () => {
  const page = await mountPage(`/?world=isekai&auction=${HISTORICAL_ID}`);
  try {
    await waitFor(
      () =>
        page.find((element) => element.getAttribute("id") === "public-account-session-trigger") !==
        null,
    );
    const worldButton = (disabled: boolean) =>
      page.find(
        (node) =>
          node.tag === "button" &&
          node.getAttribute("class") === "public-auction-filter__tab" &&
          node.attributes.disabled === disabled,
      );
    await waitFor(() => worldButton(true) !== null);
    const main = worldButton(false);
    if (main === null) throw new Error("往期看板未渲染主世界按钮");
    press(main);
    expect(page.location.searchParams.get("world")).toBe("main");
    expect(page.location.searchParams.has("auction")).toBe(false);
    const mobile = requireElement(page, (element) => element.getAttribute("id") === PRIMARY);
    const bidLog = href(page, "/bidlog", mobile);
    expect(bidLog).toBe("/bidlog?world=main");
    page.router.push(bidLog);
    await waitFor(() =>
      page.requests.some((url) => url.pathname === `/api/v1/auctions/${SETTLED.id}/bid-log`),
    );
    expect(
      page.requests
        .find((url) => url.pathname === `/api/v1/auctions/${SETTLED.id}/bid-log`)
        ?.searchParams.get("world"),
    ).toBe("main");

    openPanel(page, "public-account-session-trigger");
    const menu = requireElement(
      page,
      (element) => element.getAttribute("id") === "public-account-session-panel",
    );
    expect(href(page, "/me", menu)).toBe("/me?world=main");
    const account = href(page, "/account", mobile);
    expect(account).toBe("/account?world=main");
    // 让接下来观察到的 current 请求确实来自账户页，而非外壳启动时的同 URL 缓存。
    resetStoreForTest();
    const from = page.requests.length;
    page.router.push(account);
    const accountPaths = [
      "/api/v1/me/bids",
      "/api/v1/me/lot-watch-preference",
      "/api/v1/me/session-preference",
    ];
    try {
      await waitFor(() =>
        [...accountPaths, "/api/v1/auctions/current"].every((path) =>
          page.requests.slice(from).some((request) => request.pathname === path),
        ),
      );
    } catch (error) {
      throw new Error(
        `账户页 ${page.location.href} 的实际请求 ${page.requests
          .slice(from)
          .map((request) => request.pathname)
          .join(", ")}`,
        { cause: error },
      );
    }
    expect(page.router.currentLocationString()).toBe("/me?world=main");
    expect(
      page.requests
        .slice(from)
        .filter((request) => request.pathname === "/api/v1/auctions/current")
        .map((request) => request.searchParams.get("world")),
    ).toEqual(["main"]);
    expect(
      page.requests
        .slice(from)
        .filter((request) => accountPaths.includes(request.pathname))
        .every((request) => !request.searchParams.has("world")),
    ).toBe(true);
    const fulfillment = href(page, "/fulfillment", mobile);
    expect(fulfillment).toBe("/fulfillment?world=main");
    page.router.push(fulfillment);
    const endpoint = `/api/v1/auctions/${SETTLED.id}/fulfillment`;
    await waitFor(() => page.requests.some((url) => url.pathname === endpoint));
    expect(page.requests.find((url) => url.pathname === endpoint)?.searchParams.get("world")).toBe(
      "main",
    );
  } finally {
    page.dispose();
  }
});

test("历史成交价的世界筛选由应用提交，外壳进出链接都省略 world", async () => {
  const page = await mountPage("/bidlog?world=main", true);
  const priceReads = () => page.requests.filter((url) => url.pathname === "/api/v1/price-history");
  try {
    const mobile = requireElement(page, (node) => node.getAttribute("id") === PRIMARY);
    const history = href(page, "/history", mobile);
    expect(history).toBe("/history");
    page.router.push(history);
    const settings = () =>
      page.find((node) => node.getAttribute("class") === "public-history-settings-toggle");
    await waitFor(() => settings() !== null);
    expect(priceReads()).toEqual([]);
    press(
      requireElement(
        page,
        (node) => node.getAttribute("class") === "public-history-settings-toggle",
      ),
    );
    press(
      requireElement(
        page,
        (node) =>
          node.getAttribute("data-history-field") === "world" &&
          node.getAttribute("aria-pressed") === "false",
      ),
    );
    expect(page.router.currentLocationString()).toBe("/history");
    expect(priceReads()).toEqual([]);
    requireElement(
      page,
      (node) => node.tag === "form" && node.getAttribute("class") === "public-history-filters",
    ).submit();
    await waitFor(() => priceReads().length > 0);
    const selectedWorld = priceReads()[0]?.searchParams.get("world");
    expect(selectedWorld).toBe("main");
    expect(page.router.currentLocationString()).toBe("/history?world=main");
    for (const target of ["/", "/bidlog", "/account", "/fulfillment", "/settings"]) {
      expect(href(page, target, mobile)).toBe(target);
    }
  } finally {
    page.dispose();
  }
});

test("唯一开放主世界的自动回退省略世界，外壳链接沿用缺省", async () => {
  const page = await mountPage("/forum?world=isekai");
  try {
    await waitFor(() => !page.location.searchParams.has("world"));
    expect(page.location.pathname).toBe("/forum");
    const mobile = requireElement(page, (node) => node.getAttribute("id") === PRIMARY);
    expect(href(page, "/bidlog", mobile)).toBe("/bidlog");
  } finally {
    page.dispose();
  }
});
