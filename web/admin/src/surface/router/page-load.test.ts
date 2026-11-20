// Admin 分块失败的真实路由、会话与浏览器重载边界。

import { expect, test } from "bun:test";
import { parseJson } from "@auction/web-shared/net/wire";
import {
  type AuthTestDocument,
  settleAuth,
  waitForPageTitle,
} from "../auth/auth-environment-fixture";
import { failure, response, sessionBody } from "../auth/effects-session-fixture";
import { accepted, decodeConfigData } from "../config/config-projection-fixture.test";
import { runtimeConfigFormFromResponse, runtimeConfigFormSetInput } from "../config/model";
import { captureEvent, required } from "../solid-dom-fixture";
import {
  COOKIE,
  DASHBOARD_API,
  type Fixture,
  MARK,
  NOW,
  withLayout,
  withSetup,
} from "./route-load-environment-fixture";
import { FIXTURE_KEY, SOURCE_ROUTE, WRITE_PATH } from "./route-load-fixture";

const STATUS = "/status";
const REFRESH = "刷新页面";
const RELOAD_CONFIRM =
  "刷新会清除当前标签页内未完成或结果未知的命令记录、尚未确认的回执与一次性 Key，以及待处理的重启提醒。仍要刷新吗？";
const LOAD_ERROR = "操作失败。请重新加载当前数据后再试。";
const RESTART = "请重启受影响的进程，以应用尚未生效的配置。";
const WORKER_CONSISTENT = "运行中的 Worker 已采用当前配置文件里需要重启才生效的设置。";

function source({ observation }: Fixture) {
  return required(observation.mounts.at(-1), "测试来源页");
}

async function failPage(fixture: Fixture, address = STATUS, name = "Status"): Promise<void> {
  const page = fixture.deferPage(name);
  await fixture.go(address);
  expect(page.attempts()).toBe(1);
  await page.fail();
  await settleAuth();
}

function button(document: AuthTestDocument, label: string) {
  return document
    .querySelectorAll("button")
    .find((element) => element.textContent.trim() === label);
}

async function click(document: AuthTestDocument, label: string): Promise<void> {
  required(button(document, label), `按钮「${label}」`).click();
  await settleAuth();
}

function dashboardLink(document: AuthTestDocument) {
  return document.querySelectorAll("a").find((link) => link.getAttribute("href") === "/");
}

function expectPageFailure({ document }: Fixture): void {
  const root = required(document.getElementById("main"), "应用入口");
  expect(root.getAttribute("data-app-ready")).toBe("true");
  expect(root.hasAttribute("data-app-failed")).toBe(false);
  const main = required(document.getElementById("admin-content"), "认证页面出口");
  const banner = main
    .querySelectorAll('[role="alert"]')
    .find((node) => node.textContent === LOAD_ERROR);
  expect(banner).toBeDefined();
  expect(button(document, REFRESH)?.hasAttribute("disabled")).toBe(false);
  expect(dashboardLink(document)).toBeDefined();
}

function expectBlocked(fixture: Fixture): void {
  expect(fixture.browser.attempts).toEqual([]);
  expect(fixture.browser.writes).toEqual([]);
  expectPageFailure(fixture);
}

async function cancelManualRefresh(fixture: Fixture): Promise<void> {
  await click(fixture.document, REFRESH);
  expect(fixture.browser.confirmations).toEqual([RELOAD_CONFIRM]);
  expect(fixture.browser.attempts).toEqual([]);
  expect(fixture.browser.reloads).toEqual([]);
  expect(fixture.browser.writes).toEqual([]);
  expect(fixture.browser.storage.has(MARK)).toBe(false);
  expectPageFailure(fixture);
}

test("干净会话首次分块失败先写标记，再按完整地址自动重载一次", async () => {
  await withLayout(async (fixture) => {
    await failPage(fixture, "/status#admin-status-title");
    const mark = `[{"path":"/status","at":${NOW}}]`;
    expect(fixture.browser.writes).toEqual([{ key: MARK, value: mark }]);
    expect(fixture.browser.reloads).toEqual([{ address: "/status#admin-status-title", mark }]);
  });
});

test("同路径改查询或片段再次失败也不自动重载，出口提示后仍可导航", async () => {
  await withLayout(async (fixture) => {
    await failPage(fixture);
    expect(fixture.browser.reloads).toHaveLength(1);
    const first = fixture.browser.storage.get(MARK);
    await fixture.go(SOURCE_ROUTE.pattern);
    await failPage(fixture, "/status?tab=other#admin-status-title");
    expect(fixture.browser.fake.location.search).toBe("?tab=other");
    expect(fixture.browser.fake.location.hash).toBe("#admin-status-title");
    expect(fixture.browser.attempts).toHaveLength(1);
    expect(fixture.browser.writes).toHaveLength(1);
    expect(fixture.browser.storage.get(MARK)).toBe(first);
    expectPageFailure(fixture);
    const refreshOldPage = captureEvent(required(button(fixture.document, REFRESH)), "onClick");
    const nav = required(dashboardLink(fixture.document), "导航总览");
    const navigate = nav.attributes.onClick;
    if (typeof navigate !== "function") throw new Error("导航缺少点击处理器");
    navigate({ button: 0, preventDefault() {} });
    await settleAuth();
    expect(fixture.router.currentLocationString()).toBe("/");
    expect(button(fixture.document, REFRESH)).toBeUndefined();
    refreshOldPage();
    expect(fixture.browser.attempts).toHaveLength(1);
    expect(fixture.api.dashboardReads).toEqual([DASHBOARD_API]);
    expect(fixture.document.getElementById("dashboard-title")).not.toBeNull();
  });
});

test("防循环标记不可写时停在当前页面，手动刷新不写标记", async () => {
  await withLayout(async (fixture) => {
    fixture.browser.rejectStorage();
    await failPage(fixture);
    expect(fixture.browser.attempts).toEqual([]);
    expect(fixture.browser.writes).toHaveLength(1);
    expect(fixture.browser.storage.has(MARK)).toBe(false);
    expectPageFailure(fixture);
    await click(fixture.document, REFRESH);
    expect(fixture.browser.reloads).toEqual([{ address: STATUS, mark: null }]);
    expect(fixture.browser.writes).toHaveLength(1);
  });
});

test.each(["成功", "失败"] as const)("旧导航的迟到分块%s不会重载或盖掉新页面", async (result) => {
  await withLayout(async (fixture) => {
    const page = fixture.deferPage();
    await fixture.go(STATUS);
    expect(page.attempts()).toBe(1);
    await fixture.go(SOURCE_ROUTE.pattern);
    const currentPage = required(fixture.document.getElementById("route-load-source"));
    if (result === "成功") {
      expect(await page.release()).toBe("loaded");
    } else {
      expect(await page.fail()).toBe("failed");
    }
    await settleAuth();
    expect(fixture.document.getElementById("route-load-source")).toBe(currentPage);
    expect(fixture.document.getElementById("admin-status-title")).toBeNull();
    expect(button(fixture.document, REFRESH)).toBeUndefined();
    expect(fixture.loadWarningCount()).toBe(0);
    expect(fixture.browser.writes).toEqual([]);
    expect(fixture.browser.attempts).toEqual([]);
  });
});

test("退出在途单独阻止自动重载，退出确认后旧分块不能重载登录页", async () => {
  await withLayout(async (fixture) => {
    await click(fixture.document, "退出登录");
    expect(fixture.stubs.logout.pendingCount()).toBe(1);
    await failPage(fixture);
    expectBlocked(fixture);
    await cancelManualRefresh(fixture);
    expect(fixture.stubs.logout.pendingCount()).toBe(1);

    const next = fixture.deferPage("Audit");
    await fixture.go("/audit");
    expect(next.attempts()).toBe(1);
    fixture.document.cookie = "";
    fixture.stubs.logout.finish(response(200, '{"code":"ok","message":"","data":null}'));
    await settleAuth();
    await next.fail();
    await settleAuth();
    expect(fixture.document.getElementById("admin-actor")).not.toBeNull();
    expect(button(fixture.document, REFRESH)).toBeUndefined();
    expect(fixture.loadWarningCount()).toBe(1);
    expect(fixture.browser.writes).toEqual([]);
    expect(fixture.browser.attempts).toEqual([]);
  });
});

test("退出未确认单独阻止自动重载，选择留在后台并复核后允许新失败自动恢复", async () => {
  await withLayout(async (fixture) => {
    await click(fixture.document, "退出登录");
    fixture.stubs.logout.finish({ kind: "network" });
    await settleAuth();
    expect(button(fixture.document, "留在后台")).toBeDefined();
    expect(fixture.stubs.get.pendingCount()).toBe(1);
    await failPage(fixture);
    expectBlocked(fixture);
    await cancelManualRefresh(fixture);
    expect(button(fixture.document, "留在后台")).toBeDefined();
    fixture.stubs.get.finish(sessionBody());
    await settleAuth();
    await click(fixture.document, "留在后台");
    expect(fixture.stubs.get.pendingCount()).toBe(1);
    fixture.stubs.get.finish(sessionBody());
    await settleAuth();
    expect(button(fixture.document, "留在后台")).toBeUndefined();
    await failPage(fixture, "/audit", "Audit");
    expect(fixture.browser.reloads.map(({ address }) => address)).toEqual(["/audit"]);
  });
});

async function beginWrite(fixture: Fixture, kind: "ordinary" | "key" = "ordinary") {
  source(fixture).send(kind);
  await settleAuth();
  await finishCsrf(fixture);
  expect(fixture.api.business.pendingCount()).toBe(1);
}

async function finishCsrf({ stubs }: Fixture) {
  expect(stubs.get.pendingCount()).toBe(1);
  stubs.get.finish(sessionBody());
  await settleAuth();
}

function writeSuccess() {
  return response(200, '{"code":"ok","message":"","data":null}');
}

test("在途命令单独阻止自动重载，跨未完成分块继续接收成功回执并保持宿主", async () => {
  await withLayout(async (fixture) => {
    const initial = source(fixture);
    await beginWrite(fixture);
    const status = fixture.deferPage();
    await fixture.go(STATUS);
    expect(status.attempts()).toBe(1);
    expect(fixture.observation.unmounts).toBe(1);
    expect(fixture.document.getElementById("route-load-source")).toBeNull();
    expect(fixture.document.getElementById("admin-status-title")).toBeNull();
    await status.fail();
    await settleAuth();
    expectBlocked(fixture);
    await cancelManualRefresh(fixture);
    expect(fixture.api.business.pendingCount()).toBe(1);
    expect(fixture.api.business.cancelled).toBe(0);
    fixture.api.business.finish(writeSuccess());
    await settleAuth();
    expect(fixture.document.getElementById("admin-content")?.textContent).toContain("测试成功回执");
    await fixture.go(SOURCE_ROUTE.pattern);
    expect(source(fixture).host).toBe(initial.host);
    expect(source(fixture).host.sessionHost).toBe(initial.host.sessionHost);
  });
});

test("结果未知命令单独阻止自动重载，出口可原样重试方法、路径、正文和 ID", async () => {
  await withLayout(async (fixture) => {
    await beginWrite(fixture);
    fixture.api.business.finish({ kind: "network" });
    await settleAuth();
    await failPage(fixture);
    expectBlocked(fixture);
    await cancelManualRefresh(fixture);
    const first = required(fixture.api.writes[0], "首次写请求");
    expect(first).toMatchObject({ method: "POST", url: WRITE_PATH });
    expect(accepted(parseJson(first.body))).toMatchObject({
      command_id: expect.stringMatching(/^[0-9a-f-]{36}$/u),
      value: "冻结的初始值",
    });
    await click(fixture.document, "原样重试");
    await finishCsrf(fixture);
    expect(fixture.api.writes).toEqual([first, first]);
    fixture.api.business.finish(writeSuccess());
    await settleAuth();
    expect(button(fixture.document, "原样重试")).toBeUndefined();
    await click(fixture.document, "关闭回执");
    await fixture.go(SOURCE_ROUTE.pattern);
    await failPage(fixture);
    expect(fixture.browser.reloads.map(({ address }) => address)).toEqual([STATUS]);
  });
});

test.each([
  ["普通成功", writeSuccess(), "测试成功回执"],
  ["明确拒绝", failure(409, "stale_user_state"), "已明确拒绝"],
] as const)("%s回执单独阻止自动重载，关闭回执后可自动恢复", async (_kind, reply, summary) => {
  await withLayout(async (fixture) => {
    await beginWrite(fixture);
    const intermediate = fixture.deferPage("Audit");
    await fixture.go("/audit");
    expect(intermediate.attempts()).toBe(1);
    fixture.api.business.finish(reply);
    expect(await intermediate.release()).toBe("loaded");
    await settleAuth();
    expect(fixture.api.business.pendingCount()).toBe(0);
    expect(button(fixture.document, "原样重试")).toBeUndefined();
    expect(fixture.document.getElementById("admin-content")?.textContent).toContain(summary);
    await failPage(fixture);
    expectBlocked(fixture);
    await cancelManualRefresh(fixture);
    expect(fixture.document.getElementById("admin-content")?.textContent).toContain(summary);
    await click(fixture.document, "关闭回执");
    await failPage(fixture, "/audit", "Audit");
    expect(fixture.browser.reloads.map(({ address }) => address)).toEqual(["/audit"]);
  });
});

test("一次性 Key 单独阻止自动重载，离开来源页仍可领取并确认", async () => {
  await withLayout(async (fixture) => {
    await beginWrite(fixture, "key");
    const intermediate = fixture.deferPage("Audit");
    await fixture.go("/audit");
    expect(intermediate.attempts()).toBe(1);
    fixture.api.business.finish(writeSuccess());
    expect(await intermediate.release()).toBe("loaded");
    await settleAuth();
    await failPage(fixture);
    expectBlocked(fixture);
    await cancelManualRefresh(fixture);
    const main = required(fixture.document.getElementById("admin-content"));
    expect(main.textContent).not.toContain(FIXTURE_KEY);
    await click(fixture.document, "查看生成的 Key");
    expect(main.querySelector('[role="dialog"]')?.textContent).toContain(FIXTURE_KEY);
    await click(fixture.document, "关闭");
    expect(main.textContent).not.toContain(FIXTURE_KEY);
    expect(button(fixture.document, "查看生成的 Key")).toBeUndefined();
    await failPage(fixture, "/audit", "Audit");
    expect(fixture.browser.reloads.map(({ address }) => address)).toEqual(["/audit"]);
  });
});

const CONFIG_FIELD = "server.public_base_url";

function changeDraft(fixture: Fixture, kind: "config" | "rules" | "about") {
  const { config, documents } = source(fixture);
  if (kind !== "config") {
    documents[kind].draft.set({ locale: "zh-CN", title: "尚未保存", markdown: "测试正文" });
    return () => documents[kind].draft.set(null);
  }
  const baseline = config.form.peek();
  config.baseline.set(baseline);
  config.form.set(
    runtimeConfigFormSetInput(baseline, CONFIG_FIELD, "https://edited.fixture.invalid"),
  );
  return () => config.form.set(baseline);
}

test.each(["config", "rules", "about"] as const)(
  "%s 草稿单独阻止自动重载；取消手动重载保留草稿和标记，放弃后可自动恢复",
  async (kind) => {
    await withLayout(async (fixture) => {
      const discard = changeDraft(fixture, kind);
      await settleAuth();
      const previous = `[{"path":"/setup","at":${NOW}}]`;
      fixture.browser.storage.set(MARK, previous);
      await failPage(fixture);
      expectBlocked(fixture);
      await click(fixture.document, REFRESH);
      expect(fixture.browser.attempts).toEqual([{ address: STATUS, mark: previous }]);
      expect(fixture.browser.reloads).toEqual([]);
      expect(fixture.browser.writes).toEqual([]);
      expect(fixture.browser.storage.get(MARK)).toBe(previous);
      // 连续取消仍触发原生离开确认，证明第一次取消后草稿仍在。
      await click(fixture.document, REFRESH);
      expect(fixture.browser.attempts).toHaveLength(2);
      expect(fixture.browser.reloads).toEqual([]);
      expect(fixture.browser.confirmations).toEqual([]);
      fixture.browser.acceptUnload();
      await click(fixture.document, REFRESH);
      expect(fixture.browser.reloads).toEqual([{ address: STATUS, mark: previous }]);
      discard();
      await fixture.go(SOURCE_ROUTE.pattern);
      await failPage(fixture);
      expect(fixture.browser.reloads.map(({ address }) => address)).toEqual([STATUS, STATUS]);
      expect(fixture.browser.writes).toHaveLength(1);
    });
  },
);

test("仅剩重启提醒也阻止重载，热配置保存及 Worker 一致后仍显示提醒", async () => {
  await withLayout(async (fixture) => {
    const { config } = source(fixture);
    const saved = {
      ...decodeConfigData({ ...fixture.api.snapshot.data, restart_required: true }),
      worker_restart_state: null,
    };
    const form = accepted(runtimeConfigFormFromResponse(saved));
    config.applySavedConfig(saved, form, true);
    await settleAuth();
    await failPage(fixture);
    expectBlocked(fixture);
    await cancelManualRefresh(fixture);
    fixture.browser.acceptManual();
    await click(fixture.document, REFRESH);
    expect(fixture.browser.confirmations).toEqual([RELOAD_CONFIRM, RELOAD_CONFIRM]);
    expect(fixture.browser.reloads).toEqual([{ address: STATUS, mark: null }]);
    expect(fixture.browser.writes).toEqual([]);

    config.applySavedConfig(saved, form, false);
    await fixture.go("/config");
    await waitForPageTitle(fixture.document, "config-title");
    await settleAuth();
    expect(fixture.api.configReads.length).toBeGreaterThan(0);
    const main = required(fixture.document.getElementById("admin-content"));
    expect(main.textContent).toContain(RESTART);
    expect(main.textContent).toContain(WORKER_CONSISTENT);
    await failPage(fixture, "/audit", "Audit");
    expect(fixture.browser.attempts).toHaveLength(1);
    expect(fixture.browser.writes).toEqual([]);
    expectPageFailure(fixture);
  });
});

test.each([STATUS, "/"] as const)(
  "冷启动 %s 先显示登录表单，确认会话后才加载目标页面",
  async (path) => {
    await withLayout(
      async (fixture) => {
        const status = required(fixture.bootPage, "冷启动目标分块");
        fixture.stubs.get.finish(failure(401, "unauthenticated"));
        await settleAuth();
        expect(fixture.document.getElementById("admin-actor")).not.toBeNull();
        expect(status.attempts()).toBe(0);
        expect(fixture.api.dashboardReads).toEqual([]);
        required(fixture.document.getElementById("admin-actor")).input("operator");
        required(fixture.document.getElementById("admin-secret")).input("fixture-password");
        required(fixture.document.querySelector("form")).submit();
        await settleAuth();
        expect(fixture.stubs.post.pendingCount()).toBe(1);
        fixture.document.cookie = `auction_admin_csrf=${COOKIE}`;
        fixture.stubs.post.finish(sessionBody());
        await settleAuth();
        expect(status.attempts()).toBe(0);
        fixture.stubs.get.finish(sessionBody());
        await settleAuth();
        if (path === "/") {
          expect(status.attempts()).toBe(0);
          expect(fixture.api.dashboardReads).toEqual([DASHBOARD_API]);
          expect(fixture.document.getElementById("dashboard-title")).not.toBeNull();
          return;
        }
        expect(status.attempts()).toBe(1);
        expect(fixture.document.getElementById("admin-status-title")).toBeNull();
        expect(await status.release()).toBe("loaded");
        const title = await waitForPageTitle(fixture.document, "admin-status-title");
        expect(title.textContent).toBe("服务状态");
      },
      { start: path, authenticate: false, deferOnBoot: "Status" },
    );
  },
);

test("首次设置在认证之前可操作，不发起后台会话查询", async () => {
  await withSetup((document, stubs, api) => {
    expect(document.getElementById("admin-setup-title")?.textContent).toBe("首次部署设置");
    const form = required(document.querySelector("form"), "设置表单");
    const token = required(form.querySelector('[id="setup-token"]'), "部署码输入框");
    const verify = required(button(document, "验证并进入下一步"), "验证部署码按钮");
    expect(verify.hasAttribute("disabled")).toBe(true);
    token.input("fixture-setup-token");
    expect(token.value).toBe("fixture-setup-token");
    expect(verify.hasAttribute("disabled")).toBe(false);
    expect(stubs.get.calls).toBe(0);
    expect(stubs.post.calls).toBe(0);
    expect(api.dashboardReads).toEqual([]);
  });
});

test.each([
  [
    "投递详情",
    "delivery-detail-title",
    (id: string) => `/deliveries/${id}`,
    "",
    "?include=sent_proof",
  ],
  [
    "退件详情",
    "settlement-return-detail-title",
    (id: string) => `/settlement-returns/${id}`,
    "",
    "",
  ],
  [
    "场次拍品",
    "auction-title",
    (id: string) => `/auctions/${id}/lots`,
    "?page_size=20",
    "?page=1&page_size=20",
  ],
] as const)(
  "同一%s路由更换 ID 后卸载旧页面，并读取新对象",
  async (_name, titleId, route, query, apiQuery) => {
    await withLayout(async (fixture) => {
      const first = "0f8fad5b-d9cb-469f-a165-70867728950e";
      const second = "2c9e63a1-7f4b-4a0e-b3d5-9f8c1e2d4b60";
      const apiPath = (id: string) => `/api/v1/admin${route(id)}${apiQuery}`;
      await fixture.go(route(first) + query);
      await waitForPageTitle(fixture.document, titleId);
      await settleAuth();
      const previous = required(fixture.document.getElementById(titleId));
      const mutations = fixture.document.watchMounts(previous);
      expect(fixture.api.detailReads).toEqual([apiPath(first)]);
      await fixture.go(route(second) + query);
      expect(await waitForPageTitle(fixture.document, titleId)).not.toBe(previous);
      expect(mutations).toContain("remove");
      expect(fixture.api.detailReads).toEqual([apiPath(first), apiPath(second)]);
    });
  },
);
