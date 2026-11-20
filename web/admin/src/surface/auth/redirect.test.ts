// 冷启动地址规范化与登录回跳的回归：带 return 的地址不能影响之后的会话失效回跳。

import { expect, test } from "bun:test";
import { createRouter, type Router, RouterContext } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { type Component, createComponent, createRoot, flush } from "solid-js";

import { ADMIN_ROUTES } from "../router/route";
import {
  AdminLoginRedirect,
  AdminReturnRedirect,
  normalizeColdStartAdminLocation,
} from "./redirect";

function fakeRouter(replace: (href: string) => void): Router {
  return {
    current: () => ({ kind: "matched", name: "Audit", params: {}, query: {} }),
    currentLocationString: () => "/",
    currentEntryKey: () => "test-entry",
    push() {},
    replace,
    goBack() {},
    restoreScrollAfterCommit() {},
  };
}

function mountWithRouter(router: Router, component: Component): void {
  const previous = RouterContext.defaultValue;
  RouterContext.defaultValue = router;
  try {
    createRoot((dispose) => {
      createComponent(component, {});
      flush();
      dispose();
    });
  } finally {
    RouterContext.defaultValue = previous;
  }
}

function openAdminRouter(initialUrl: string, normalize = false) {
  const opened = installFakeWindow(initialUrl);
  if (normalize) {
    normalizeColdStartAdminLocation();
  }
  const normalizationWrites = opened.fake.writes.length;
  let dispose = (): void => {};
  const router = createRoot((disposeRoot) => {
    dispose = disposeRoot;
    return createRouter({ routes: ADMIN_ROUTES });
  });
  return { ...opened, router, normalizationWrites, disposeRouter: dispose };
}

test("冷启动规范化丢掉裸问号、裸片段和被路径规范化应丢弃的后缀", () => {
  const cases = [
    ["/audit?#", "/audit"],
    ["/audit?", "/audit"],
    ["/audit#", "/audit"],
    ["/%61udit?page=2#audit", "/audit"],
    ["/audit/?page=2#audit", "/audit?page=2#audit"],
    ["/setup/?x=1#step", "/setup"],
    ["/setup?x=1#step", "/setup"],
    ["/%73etup?x=1#step", "/setup"],
    ["/setup", "/setup"],
    ["/audit", "/audit"],
  ] as const;
  for (const [initial, expected] of cases) {
    const opened = installFakeWindow(initial);
    try {
      normalizeColdStartAdminLocation();
      expect(opened.fake.location.href).toBe(new URL(expected, "https://auction.test").href);
      expect(opened.fake.writes).toHaveLength(initial === expected ? 0 : 1);
    } finally {
      opened.restore();
    }
  }
});

test("Setup 冷启动在一次决策里丢弃后缀，重复初始化不再写历史", () => {
  const opened = installFakeWindow("/setup/?x=1#step");
  try {
    normalizeColdStartAdminLocation();
    expect(opened.fake.writes).toHaveLength(1);
    expect(opened.fake.location.href).toBe(new URL("/setup", "https://auction.test").href);
    normalizeColdStartAdminLocation();
    expect(opened.fake.writes).toHaveLength(1);
  } finally {
    opened.restore();
  }
});

test("Setup 冷启动接入路由后仍停在 /setup", () => {
  const opened = openAdminRouter("/setup/?x=1#step", true);
  try {
    expect(opened.normalizationWrites).toBe(1);
    expect(opened.router.current()).toMatchObject({ kind: "matched", name: "Setup" });
    expect(opened.router.currentLocationString()).toBe("/setup");
  } finally {
    opened.disposeRouter();
    opened.restore();
  }
});

test("return 参数跟随 Rust 的解析、路径边界与登录页拒绝规则", () => {
  const cases = [
    ["/login??return=%2Faudit", "/"],
    ["/login?return=%2Faudit%2F..%2Flogin%2F", "/"],
    ["/login?return=%2Faudit%3F", "/audit?"],
    ["/login?return=%2Fdeliveries%2Fx%5Ey", "/deliveries/x^y"],
  ] as const;
  for (const [initial, expected] of cases) {
    const opened = installFakeWindow(initial);
    const calls: string[] = [];
    try {
      mountWithRouter(
        fakeRouter((href) => calls.push(href)),
        AdminReturnRedirect,
      );
      expect(calls).toEqual([expected]);
    } finally {
      opened.restore();
    }
  }
});

test("登录链接编码完整站内地址，登录页自身回到根路径", () => {
  const opened = installFakeWindow("/audit/?page=2#audit");
  const calls: string[] = [];
  try {
    mountWithRouter(
      fakeRouter((href) => calls.push(href)),
      AdminLoginRedirect,
    );
    expect(calls).toEqual(["/login?return=%2Faudit%2F%3Fpage%3D2%23audit"]);
    opened.fake.location.href = new URL("/login", opened.fake.location).href;
    calls.length = 0;
    mountWithRouter(
      fakeRouter((href) => calls.push(href)),
      AdminLoginRedirect,
    );
    expect(calls).toEqual(["/login?return=%2F"]);
  } finally {
    opened.restore();
  }
});

test("共享 replace 保留历史条目键并同步路由匹配", () => {
  const opened = openAdminRouter("/login?return=%2Faudit%3Fpage%3D2");
  try {
    const key = opened.router.currentEntryKey();
    mountWithRouter(opened.router, AdminReturnRedirect);
    expect(opened.router.current()).toMatchObject({ kind: "matched", name: "Audit" });
    expect(opened.router.currentLocationString()).toBe("/audit?page=2");
    expect(opened.router.currentEntryKey()).toBe(key);
  } finally {
    opened.disposeRouter();
    opened.restore();
  }
});

test("共享 replace 保留条目键，但会去掉尾斜杠与裸问号", () => {
  const cases = [
    ["/login?return=%2Faudit%2F", "/audit"],
    ["/login?return=%2Faudit%3F", "/audit"],
  ] as const;
  for (const [initial, expected] of cases) {
    const opened = openAdminRouter(initial);
    try {
      const key = opened.router.currentEntryKey();
      mountWithRouter(opened.router, AdminReturnRedirect);
      expect(opened.router.currentLocationString()).toBe(expected);
      expect(opened.router.currentEntryKey()).toBe(key);
    } finally {
      opened.disposeRouter();
      opened.restore();
    }
  }
});

test("冷启动 return 不覆盖之后的会话失效回跳", () => {
  const opened = openAdminRouter("/alerts?return=%2Faudit", true);
  try {
    expect(opened.normalizationWrites).toBe(0);
    opened.router.push("/audit");
    expect(opened.router.currentLocationString()).toBe("/audit");
    const key = opened.router.currentEntryKey();
    opened.router.replace("/login?return=%2Faudit");
    expect(opened.router.currentEntryKey()).toBe(key);
    mountWithRouter(opened.router, AdminReturnRedirect);
    expect(opened.router.currentLocationString()).toBe("/audit");
    expect(opened.router.current()).toMatchObject({ kind: "matched", name: "Audit" });
    expect(opened.router.currentEntryKey()).toBe(key);
  } finally {
    opened.disposeRouter();
    opened.restore();
  }
});

test("admin_login_return_accepts_local_query_and_fragment：登录回跳接受本地查询与片段", () => {
  const cases = [
    ["?return=%2Faudit%3Fpage%3D2%26kind%3Dlogin%23audit", "/audit?page=2&kind=login#audit"],
    ["?return=%2Faudit%3Fquery%3D%252Fitem%23list", "/audit?query=%2Fitem#list"],
  ] as const;
  for (const [query, expected] of cases) {
    const opened = installFakeWindow(`https://admin.example.test/login${query}`);
    const calls: string[] = [];
    try {
      mountWithRouter(
        fakeRouter((href) => calls.push(href)),
        AdminReturnRedirect,
      );
      expect(calls).toEqual([expected]);
    } finally {
      opened.restore();
    }
  }
});

test("admin_login_return_rejects_login_variants_and_external_targets：登录回跳拒绝登录页变体与外部目标", () => {
  const queries = [
    "?return=%2Flogin%2F",
    "?return=%2F%256Cogin",
    "?return=%2Flogin%3Fpage%3D2",
    "?return=%2F%2Fexample.test",
    "?return=%2F%5Cexample.test",
    "?return=%2Faudit%2F..%2Flogin%2F",
    "?return=%2Faudit%2F%252e%252e%2Flogin%2F",
    "?return=%2Faudit%5C..%5Clogin%5C",
    "?return=%2Faudit%252F..%252Flogin",
    "?return=%2Faudit%2F%2Flogin",
    "?return=https%3A%2F%2Fexample.test",
    "?return=%2F%0Aexample.test",
  ] as const;
  for (const query of queries) {
    const opened = installFakeWindow(`https://admin.example.test/login${query}`);
    const calls: string[] = [];
    try {
      mountWithRouter(
        fakeRouter((href) => calls.push(href)),
        AdminReturnRedirect,
      );
      expect(calls).toEqual(["/"]);
    } finally {
      opened.restore();
    }
  }
});
