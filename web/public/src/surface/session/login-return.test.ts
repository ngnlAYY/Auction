// 登录返回地址的嵌套查询及同源约束。

import { expect, test } from "bun:test";
import { writeQueryPairs } from "@auction/web-shared/net/query";
import { RouterContext } from "@auction/web-shared/router";
import { compileRoutes, resolveRoute } from "@auction/web-shared/router/pattern";
import { openRouter } from "@auction/web-shared/router/test-window";
import { createMemo, createRoot, flush } from "solid-js";

import { queryPairsString } from "../query/serialization";
import {
  InitialLoginReturnLocationContext,
  initialLoginReturnLocation,
  loginHrefForCurrentLocation,
  safeReturnPath,
} from "./login-return";

const ROUTES = compileRoutes([{ name: "Login", pattern: "/login", query: ["return_to"] }]);

test("一次解码 return_to，完整恢复筛选、页码和已经编码的值", () => {
  const source = "/?world=isekai&page=3&type=one%2Csta&name=%E5%BC%93+A%26B%3Dx";
  const href = `/login?${queryPairsString([["return_to", source]])}`;
  const resolved = resolveRoute(ROUTES, href);
  expect(resolved.match).toMatchObject({ kind: "matched", query: { return_to: source } });
  expect(resolved.routeString).toBe(href);
  expect(resolveRoute(ROUTES, resolved.routeString).match).toEqual(resolved.match);
  expect(safeReturnPath(source)).toBe(source);
});

test.each([
  "https://example.test/",
  "//example.test/",
  "javascript:alert(1)",
  "/\\example.test/",
  "/me#login-duration",
])("拒绝不符合现有回跳规则的地址 %s", (target) => {
  const result = resolveRoute(ROUTES, `/login?return_to=${encodeURIComponent(target)}`);
  const returnTo = result.match.kind === "matched" ? (result.match.query.return_to ?? null) : null;
  expect(returnTo).toBe(target);
  expect(safeReturnPath(returnTo)).toBe("/");
});

test("首次来源失效后，登录链接随站内导航使用真实地址栏", () => {
  const source = "/?world=isekai&page=3";
  const location = new URL(source, "https://auction.test");
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  const previousRouter = RouterContext.defaultValue;
  const previousInitial = InitialLoginReturnLocationContext.defaultValue;
  Object.defineProperty(globalThis, "window", { configurable: true, value: { location } });
  try {
    const initial = initialLoginReturnLocation();
    InitialLoginReturnLocationContext.defaultValue = initial;
    RouterContext.defaultValue = {
      current: () => ({ kind: "matched", name: "Home", params: {}, query: {} }),
      currentLocationString: () => `${location.pathname}${location.search}`,
      currentEntryKey: () => "test-entry",
      push() {},
      replace() {},
      goBack() {},
      restoreScrollAfterCommit() {},
    };
    const loginHref = () =>
      createRoot((dispose) => {
        try {
          return loginHrefForCurrentLocation();
        } finally {
          dispose();
        }
      });
    // 模拟首次规范化期间的暂时空查询，回退仍须完整编码。
    location.search = "";
    const startup = resolveRoute(ROUTES, loginHref()).match;
    expect(startup).toMatchObject({ kind: "matched", query: { return_to: source } });

    initial.expire();
    location.pathname = "/rules";
    expect(loginHref()).toBe("/login?return_to=%2Frules");
    location.pathname = "/";
    expect(loginHref()).toBe("/login");
    location.search = "?page=2&name=A%26B";
    const current = resolveRoute(ROUTES, loginHref()).match;
    expect(current).toMatchObject({
      kind: "matched",
      query: { return_to: "/?page=2&name=A%26B" },
    });
  } finally {
    InitialLoginReturnLocationContext.defaultValue = previousInitial;
    RouterContext.defaultValue = previousRouter;
    if (previousWindow === undefined) {
      Reflect.deleteProperty(globalThis, "window");
    } else {
      Object.defineProperty(globalThis, "window", previousWindow);
    }
  }
});

test("同页查询写入成功后，已挂载的登录链接读到新页码", () => {
  const opened = openRouter("/?page=2");
  const previousRouter = RouterContext.defaultValue;
  RouterContext.defaultValue = opened.router;
  const mounted = createRoot((dispose) => ({
    href: createMemo(loginHrefForCurrentLocation),
    dispose,
  }));
  try {
    expect(mounted.href()).toBe("/login?return_to=%2F%3Fpage%3D2");
    writeQueryPairs(["page"], [["page", "3"]], "replace");
    expect(opened.router.currentLocationString()).toBe("/?page=3");
    flush();
    expect(mounted.href()).toBe("/login?return_to=%2F%3Fpage%3D3");
  } finally {
    mounted.dispose();
    RouterContext.defaultValue = previousRouter;
    opened.disposeRouter();
    opened.restore();
  }
});
