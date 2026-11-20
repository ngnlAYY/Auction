// 用真实认证外壳验证失效回执、写前异代证据和未决命令的展示。

import { expect, test } from "bun:test";
import { adminWriteFailureDisplayCode } from "../../api/failure";
import { type AdminSessionHost, adminSessionWritesAllowed } from "../../api/session";
import {
  fetchAdminBusinessWorlds,
  prepareAdminBusinessWorldSetEnabledOnce,
  setAdminBusinessWorldEnabled,
} from "../../api/worlds";
import { required, type TestElement } from "../solid-dom-fixture";
import { button, onlyElement } from "../t10-view-fixture.test";
import { settleAuth } from "./auth-environment-fixture";
import { failure, sessionBody, type Transport, unauthorized } from "./effects-session-fixture";
import {
  assertKeyHidden,
  beginCreate,
  COOKIE_A,
  COOKIE_B,
  click,
  GENERATED,
  generatedUser,
  VIEW_KEY,
  withLayout,
} from "./session-render-fixture.test";

const COMMAND_PAGE = "/test-authenticated-commands";
const RETRY_ORIGINAL = "原样重试";

function isVisible(element: TestElement): boolean {
  for (let ancestor: TestElement | null = element; ancestor; ancestor = ancestor.parentElement) {
    if (
      ancestor.hasAttribute("hidden") ||
      ancestor.hasAttribute("inert") ||
      ancestor.getAttribute("aria-hidden") === "true"
    ) {
      return false;
    }
  }
  return true;
}

function sendWorldWrite(host: AdminSessionHost) {
  const prepared = prepareAdminBusinessWorldSetEnabledOnce(
    "main",
    false,
    2,
  )("00000000-0000-4000-8000-000000000011");
  if (!prepared.ok) throw new Error("应能冻结世界操作");
  return setAdminBusinessWorldEnabled(host, prepared.value, new AbortController().signal);
}

async function startBusinessRequest(stubs: Transport, host: AdminSessionHost, write: boolean) {
  if (write) {
    const pending = sendWorldWrite(host);
    await settleAuth();
    expect(stubs.get.pendingCount()).toBe(1);
    stubs.get.finish(sessionBody());
    await settleAuth();
    expect(stubs.businessWrite.pendingCount()).toBe(1);
    return async () => {
      stubs.businessWrite.finish(unauthorized());
      const result = await pending;
      if (result.ok) throw new Error("业务 PATCH 应收到拒绝");
      return adminWriteFailureDisplayCode(result.error);
    };
  }
  const pending = fetchAdminBusinessWorlds(host, new AbortController().signal);
  await settleAuth();
  expect(stubs.businessRead.pendingCount()).toBe(1);
  return async () => {
    stubs.businessRead.finish(unauthorized());
    const result = await pending;
    if (result.ok) throw new Error("业务 GET 应收到拒绝");
    return result.error;
  };
}

for (const method of ["GET", "PATCH"] as const) {
  test(`真实认证外壳收到业务 ${method} 的可信 401 会换票，迟到 200 不能开写，只通知一次`, async () => {
    await withLayout(COMMAND_PAGE, async ({ stubs, document, requests, broadcast, pathname }) => {
      stubs.get.finish(sessionBody());
      await settleAuth();
      const host = required(requests.hosts[0], "认证外壳下发的句柄").session;
      expect(adminSessionWritesAllowed(host)).toBe(true);
      const finishBusiness = await startBusinessRequest(stubs, host, method === "PATCH");

      if (method === "GET") {
        click(document, "退出登录");
        await settleAuth();
        stubs.logout.finish(failure(429, "rate_limited"));
        await settleAuth();
        expect(stubs.get.pendingCount()).toBe(1);
      }
      const beforeRead = stubs.get.calls;
      stubs.get.allowNextReplyAfterAbort();
      if (method === "GET") click(document, "复核测试会话");
      else document.setHidden(false);
      await settleAuth();
      expect(stubs.get.calls).toBe(beforeRead + 1);
      expect(stubs.get.pendingCount()).toBe(1);
      expect(adminSessionWritesAllowed(host)).toBe(method === "PATCH");
      const beforeGet = stubs.get.calls;
      const beforeCancelled = stubs.get.cancelled;

      expect(await finishBusiness()).toBe("unauthenticated");
      await settleAuth();
      const whileReviewing = {
        reads: stubs.get.calls - beforeGet,
        cancelled: stubs.get.cancelled - beforeCancelled,
        writesAllowed: adminSessionWritesAllowed(host),
      };
      stubs.get.finish(sessionBody());
      await settleAuth();
      expect({
        ...whileReviewing,
        writesAllowedAfterOldReply: adminSessionWritesAllowed(host),
      }).toEqual({
        reads: 1,
        cancelled: 1,
        writesAllowed: false,
        writesAllowedAfterOldReply: false,
      });
      expect(stubs.get.pendingCount()).toBe(1);
      expect(broadcast.sent).toEqual([]);

      stubs.get.finish(unauthorized());
      await settleAuth();
      expect(pathname()).toBe("/login");
      expect(adminSessionWritesAllowed(host)).toBe(false);
      expect(broadcast.sent).toEqual(["session-ended"]);
      expect(stubs.logout.calls).toBe(method === "GET" ? 1 : 0);
      expect(stubs.businessWrite.calls).toBe(method === "PATCH" ? 1 : 0);
    });
  });
}

test("例行 GET 的同代 503 关写后，迟到的写前 200(B) 仍移除 Key 并保留回执", async () => {
  await withLayout("/users", async (fixture) => {
    const { stubs, document, router, requests } = fixture;
    stubs.get.finish(sessionBody());
    await settleAuth();
    await beginCreate(fixture);
    router.push(COMMAND_PAGE);
    await settleAuth();
    stubs.createUser.finish(generatedUser());
    await settleAuth();
    click(document, VIEW_KEY);
    await settleAuth();

    const host = required(requests.hosts[0], "认证外壳下发的句柄").session;
    const write = sendWorldWrite(host);
    await settleAuth();
    expect(stubs.get.pendingCount()).toBe(1);
    document.setHidden(false);
    await settleAuth();
    expect(stubs.get.pendingCount()).toBe(2);
    stubs.get.finish(failure(503, "storage_unavailable"), 1);
    await settleAuth();
    expect(adminSessionWritesAllowed(host)).toBe(false);
    expect(document.querySelectorAll("code").map((node) => node.textContent)).toContain(GENERATED);

    stubs.get.finish(sessionBody("cookie-b"));
    const result = await write;
    if (result.ok) throw new Error("异代正文不应授权业务写");
    expect(adminWriteFailureDisplayCode(result.error)).toBe("unauthenticated");
    await settleAuth();
    assertKeyHidden(document);
    expect(stubs.businessWrite.calls).toBe(0);
    expect(button(document, VIEW_KEY).hasAttribute("disabled")).toBe(true);
    const receipt = onlyElement(document, ".admin-command-notices__receipt");

    expect(stubs.get.pendingCount()).toBe(1);
    stubs.get.finish(sessionBody());
    await settleAuth();
    expect(adminSessionWritesAllowed(host)).toBe(true);
    expect(onlyElement(document, ".admin-command-notices__receipt")).toBe(receipt);
    assertKeyHidden(document);
    click(document, VIEW_KEY);
    await settleAuth();
    expect(document.querySelectorAll("code").map((node) => node.textContent)).toContain(GENERATED);
    expect(stubs.businessWrite.calls).toBe(0);
  });
});

test("异代复核 503 仍显示原未决命令，确认 A 后只能人工原样重试", async () => {
  await withLayout("/users", async (fixture) => {
    const { stubs, document, router } = fixture;
    stubs.get.finish(sessionBody());
    await settleAuth();
    await beginCreate(fixture);
    router.push(COMMAND_PAGE);
    await settleAuth();
    stubs.createUser.finish({ kind: "network" });
    await settleAuth();
    const original = onlyElement(document, ".admin-pending__entry");
    const commandId = original.getAttribute("data-command-id");
    expect(commandId).toBeTruthy();
    expect(original.textContent).toContain("ExamplePlayer");
    expect(stubs.userCreateBodies).toHaveLength(1);
    const body = stubs.userCreateBodies[0];
    if (typeof body !== "string") throw new Error("用户创建请求应发送 JSON 正文");
    expect(body).toContain(`"command_id":"${commandId}"`);
    expect(body).toContain('"username":"ExamplePlayer"');
    expect(button(document, RETRY_ORIGINAL).hasAttribute("disabled")).toBe(false);

    document.cookie = COOKIE_B;
    document.setHidden(false);
    await settleAuth();
    stubs.get.finish(failure(503, "storage_unavailable"));
    await settleAuth();
    expect(onlyElement(document, ".admin-pending__entry")).toBe(original);
    const retry = button(document, RETRY_ORIGINAL);
    expect(isVisible(original)).toBe(true);
    expect(isVisible(retry)).toBe(true);
    expect(retry.hasAttribute("disabled")).toBe(true);
    expect(stubs.createUser.calls).toBe(1);

    document.cookie = COOKIE_A;
    document.setHidden(false);
    await settleAuth();
    stubs.get.finish(sessionBody());
    await settleAuth();
    expect(stubs.createUser.calls).toBe(1);
    expect(onlyElement(document, ".admin-pending__entry")).toBe(original);
    click(document, RETRY_ORIGINAL);
    await settleAuth();
    expect(stubs.get.pendingCount()).toBe(1);
    stubs.get.finish(sessionBody());
    await settleAuth();
    expect(stubs.createUser.pendingCount()).toBe(1);
    expect(stubs.userCreateBodies).toEqual([body, body]);
    stubs.createUser.finish(generatedUser());
    await settleAuth();
    expect(document.querySelectorAll(".admin-pending__entry")).toHaveLength(0);
  });
});
