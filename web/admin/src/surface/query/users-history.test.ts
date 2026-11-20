// 用户 query：浏览器历史恢复筛选，缩小结果时原页纠偏并保留前进搜索。

import { expect, spyOn, test } from "bun:test";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { createComponent, createRoot, flush } from "solid-js";
import { installBrowserRequest } from "../../api/api-request-fixture";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { cleanupScope } from "../../test-scope-fixture";
import { installAdminTranslation } from "../admin-locale-fixture.test";
import { createSessionHost } from "../pending/session-fixture";
import {
  click,
  createElement,
  insert,
  installTestDocument,
  type TestElement,
} from "../solid-dom-fixture";
import { mountQueryState } from "./query-fixture";
import {
  type UsersQuery,
  usersQueryApply,
  usersQueryCorrectPage,
  useUsersQueryState,
} from "./users";

const { page: pageComponent } = await loadRenderModule<typeof import("../users")>(
  new URL("../users/index.tsx", import.meta.url).pathname,
  "page",
);
const { UsersPageFixture } = await loadRenderModule<typeof import("../pending/users-page-fixture")>(
  new URL("../pending/users-page-fixture.tsx", import.meta.url).pathname,
  "UsersPageFixture",
);
const DEFAULT_QUERY: UsersQuery = { name: "", status: "all", page: 1n, sort: "", direction: "asc" };
const FIRST_SEARCH: UsersQuery = { ...DEFAULT_QUERY, name: "A" };
const FIRST_SEARCH_PAGE_TWO: UsersQuery = { ...FIRST_SEARCH, page: 2n };
const FORWARD_SEARCH: UsersQuery = { ...DEFAULT_QUERY, name: "B" };

test("历史恢复每一个已应用的名称和状态", () => {
  const first: UsersQuery = { ...DEFAULT_QUERY, name: "玩家-A", status: "active" };
  const second: UsersQuery = {
    ...DEFAULT_QUERY,
    name: "玩家-B",
    status: "blacklisted",
    page: 2n,
  };
  const { fake, state, dispose } = mountQueryState("/admin/users", useUsersQueryState);
  try {
    expect(state.state.peek()).toEqual(DEFAULT_QUERY);
    state.set(first, "push");
    expect([...fake.location.searchParams]).toEqual([
      ["name", "玩家-A"],
      ["status", "active"],
    ]);
    state.set(second, "push");
    expect([...fake.location.searchParams]).toEqual([
      ["name", "玩家-B"],
      ["status", "blacklisted"],
      ["page", "2"],
    ]);

    fake.history.back();
    expect(state.state.peek()).toEqual(first);
    fake.history.back();
    expect(state.state.peek()).toEqual(DEFAULT_QUERY);
    fake.history.forward();
    expect(state.state.peek()).toEqual(first);
    fake.history.forward();
    expect(state.state.peek()).toEqual(second);
  } finally {
    dispose();
  }

  const invalid = mountQueryState(
    `/admin/users?name=${encodeURIComponent("界".repeat(86))}&status=invalid`,
    useUsersQueryState,
  );
  try {
    expect(invalid.state.state.peek()).toEqual(DEFAULT_QUERY);
  } finally {
    invalid.dispose();
  }
});

test("缩小结果时纠页保留前进搜索", () => {
  const { fake, state, dispose } = mountQueryState("/admin/users?name=A", useUsersQueryState);
  try {
    expect(state.state.peek()).toEqual(FIRST_SEARCH);
    usersQueryApply(FIRST_SEARCH_PAGE_TWO, state);
    expect(state.state.peek()).toEqual(FIRST_SEARCH_PAGE_TWO);
    expect(fake.location.search).toBe("?name=A&page=2");
    expect(
      usersQueryCorrectPage(FIRST_SEARCH_PAGE_TWO, state, { page: 2, page_size: 10, total: 11 }),
    ).toBe(true);
    expect(state.state.peek()).toEqual(FIRST_SEARCH_PAGE_TWO);
    expect(fake.writes.map(({ mode, url }) => [mode, url.search])).toEqual([
      ["push", "?name=A&page=2"],
    ]);

    usersQueryApply(FORWARD_SEARCH, state);
    expect(state.state.peek()).toEqual(FORWARD_SEARCH);
    expect(fake.location.search).toBe("?name=B");

    fake.history.back();
    const restored = state.state.peek();
    expect(restored).toEqual(FIRST_SEARCH_PAGE_TWO);
    expect(fake.location.search).toBe("?name=A&page=2");
    expect(usersQueryCorrectPage(restored, state, { page: 1, page_size: 10, total: 8 })).toBe(true);
    expect(state.state.peek()).toEqual(FIRST_SEARCH_PAGE_TWO);
    expect(fake.location.search).toBe("?name=A&page=2");

    expect(usersQueryCorrectPage(restored, state, { page: 2, page_size: 10, total: 8 })).toBe(true);
    expect(state.state.peek()).toEqual(FIRST_SEARCH);
    expect(fake.location.search).toBe("?name=A");
    expect(fake.writes.map(({ mode, url }) => [mode, url.search])).toEqual([
      ["push", "?name=A&page=2"],
      ["push", "?name=B"],
      ["replace", "?name=A"],
    ]);

    fake.history.forward();
    expect(state.state.peek()).toEqual(FORWARD_SEARCH);
    expect(fake.location.search).toBe("?name=B");
    fake.history.back();
    expect(state.state.peek()).toEqual(FIRST_SEARCH);
    expect(fake.location.search).toBe("?name=A");
    fake.history.back();
    expect(state.state.peek()).toEqual(FIRST_SEARCH);
    expect(fake.location.search).toBe("?name=A");
  } finally {
    dispose();
  }
});

async function settlePage(page: TestElement): Promise<void> {
  for (let round = 0; round < 48; round += 1) {
    await Promise.resolve();
    flush();
    page.querySelectorAll("*");
  }
}

test("用户页重试失败的纠页并保留前进搜索", async () => {
  const scope = cleanupScope();
  try {
    scope.defer(installAdminTranslation());
    scope.defer(flush);
    const { fake, restore } = installFakeWindow("/admin/users?page_size=10");
    scope.defer(restore);
    const session = createSessionHost();
    scope.defer(session.dispose);
    const document = installTestDocument();
    scope.defer(() => document.restore());
    scope.defer(installBrowserRequest());
    const usersQuery = await import("./users");
    const observeQuery = spyOn(usersQuery, "useUsersQueryState");
    scope.defer(() => observeQuery.mockRestore());
    const replaceCalls = spyOn(fake.history, "replaceState");
    scope.defer(() => replaceCalls.mockRestore());

    const page = createElement("div");
    const requests: [string, number][] = [];
    let aTotal = 11;
    const respond = async (input: RequestInfo | URL): Promise<Response> => {
      const url = new URL(
        typeof input === "string" ? input : input instanceof URL ? input.href : input.url,
        "https://auction.test",
      );
      if (url.pathname !== "/api/v1/admin/users") {
        return new Response("not found", { status: 404 });
      }
      const name = url.searchParams.get("name") ?? "";
      const requestedPage = Number(url.searchParams.get("page") ?? "1");
      const total = name === "A" ? aTotal : 0;
      requests.push([name, requestedPage]);
      return Response.json({
        code: "ok",
        message: "",
        data: { users: [], pagination: { page: requestedPage, page_size: 10, total } },
      });
    };
    const fetchSpy = spyOn(globalThis, "fetch");
    scope.defer(() => fetchSpy.mockRestore());
    fetchSpy.mockImplementation(Object.assign(respond, { preconnect: fetch.preconnect }));

    createRoot((disposeRoot) => {
      scope.defer(disposeRoot);
      insert(
        page,
        createComponent(UsersPageFixture, {
          host: session.host,
          page: pageComponent,
        }),
      );
    });
    document.add(page);
    await settlePage(page);
    expect(requests).toEqual([["", 1]]);
    expect(observeQuery).toHaveBeenCalledTimes(1);
    const result = observeQuery.mock.results[0];
    if (result?.type !== "return") throw new Error("用户页未挂载查询状态");
    const state = result.value.state;
    expect(state.peek()).toEqual(DEFAULT_QUERY);

    const applyName = async (value: string): Promise<void> => {
      const input = required(document.getElementById("user-name-filter"), "用户名筛选输入框");
      const form = required(page.querySelector(".admin-users-filter"), "用户搜索表单");
      input.value = value;
      fire(input, "onInput");
      await settlePage(page);
      fire(form, "onSubmit");
      await settlePage(page);
    };
    const expectEntry = (query: UsersQuery, search: string): void => {
      expect(state.peek()).toEqual(query);
      expect(fake.location.search).toBe(search);
    };

    await applyName("A");
    expectEntry(FIRST_SEARCH, "?page_size=10&name=A");
    const next = page
      .querySelectorAll("button")
      .find((button) => button.getAttribute("aria-label") === "下一页");
    click(required(next, "下一页"));
    await settlePage(page);
    expectEntry(FIRST_SEARCH_PAGE_TWO, "?page_size=10&name=A&page=2");
    expect(replaceCalls).toHaveBeenCalledTimes(0);
    expect(page.querySelector('[aria-current="page"]')?.textContent.trim()).toBe("2");

    await applyName("B");
    expectEntry(FORWARD_SEARCH, "?page_size=10&name=B");

    aTotal = 8;
    fake.rejectWrites(true);
    fake.history.back();
    await settlePage(page);
    expect(requests).toEqual([
      ["", 1],
      ["A", 1],
      ["A", 2],
      ["B", 1],
      ["A", 2],
    ]);
    expectEntry(FIRST_SEARCH_PAGE_TWO, "?page_size=10&name=A&page=2");
    expect(replaceCalls).toHaveBeenCalledTimes(1);
    expect(fake.writes.filter(({ mode }) => mode === "replace")).toEqual([]);
    const alert = required(page.querySelector('[role="alert"]'), "纠页错误");
    expect(alert.querySelector(".ui-async-state__title")?.textContent.trim()).toBe(
      "无法更新地址栏中的页码。",
    );
    expect(alert.textContent).toContain("无法更新地址栏中的页码。");
    const retry = required(page.querySelector(".ui-async-state__retry"), "纠页重试");
    expect(retry.textContent.trim()).toBe("重试");

    fake.rejectWrites(false);
    click(retry);
    await settlePage(page);
    expect(replaceCalls).toHaveBeenCalledTimes(2);
    expect(
      fake.writes.filter(({ mode }) => mode === "replace").map(({ url }) => url.search),
    ).toEqual(["?page_size=10&name=A"]);
    expect(requests).toEqual([
      ["", 1],
      ["A", 1],
      ["A", 2],
      ["B", 1],
      ["A", 2],
      ["A", 1],
    ]);
    expectEntry(FIRST_SEARCH, "?page_size=10&name=A");
    expect(page.querySelector('[role="alert"]')).toBeNull();
    expect(page.querySelector(".ui-async-state__retry")).toBeNull();
    expect(page.querySelector('[aria-current="page"]')?.textContent.trim()).toBe("1");

    fake.history.forward();
    await settlePage(page);
    expectEntry(FORWARD_SEARCH, "?page_size=10&name=B");
    expect(requests.at(-1)).toEqual(["B", 1]);
    expect(observeQuery).toHaveBeenCalledTimes(1);
  } finally {
    scope.dispose();
  }
});

function required<T>(value: T | null | undefined, label: string): T {
  if (value == null) throw new Error(`缺少${label}`);
  return value;
}

function fire(control: TestElement, event: "onInput" | "onSubmit"): void {
  const handler = control.attributes[event];
  if (typeof handler !== "function") throw new Error(`控件缺少 ${event}`);
  handler({ currentTarget: control, preventDefault() {} });
}
