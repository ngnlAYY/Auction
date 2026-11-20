// 生产认证布局中的未决命令宿主：显式复核、会话换代与浏览器历史后退。

import { expect, spyOn, test } from "bun:test";
import { createRouter, type Router } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { createRoot, flush } from "solid-js";
import {
  adminSessionHostLive,
  adminSessionWritesAllowed,
  confirmedAdminSession,
} from "../../../api/session";
import { loadRenderModule } from "../../../solid-render-fixture.test";
import { ADMIN_ROUTES } from "../../router/route";
import { createComponent, mountNode, required, type TestElement } from "../../solid-dom-fixture";
import {
  type AuthTestDocument,
  installAuthDocument,
  installManualTimers,
  settleAuth,
} from "../auth-environment-fixture";
import { installAuthLocale } from "../auth-locale-fixture.test";
import {
  failure,
  installFetch,
  response,
  sessionBody,
  type Transport,
  transport,
} from "../effects-session-fixture";
import type { CommandRequests } from "./command-host-fixture";

const COMMAND_PATH = "/test-authenticated-commands";
const INITIAL_COOKIE = "current-cookie";
const NEXT_COOKIE = "next-cookie";
const SESSION_ENDS = ["logout", "logout-with-current-cookie", "idle", "broadcast"] as const;

type SessionEnd = (typeof SESSION_ENDS)[number];

function button(document: AuthTestDocument, title: string): TestElement | null {
  return (
    document.querySelectorAll("button").find((element) => element.textContent.trim() === title) ??
    null
  );
}

function click(document: AuthTestDocument, title: string): void {
  const target = required(button(document, title), `按钮 ${title}`);
  expect(target.hasAttribute("disabled")).toBe(false);
  target.click();
}

interface CommandHostTest {
  readonly document: AuthTestDocument;
  readonly router: Router;
  readonly requests: CommandRequests;
  readonly stubs: Transport;
  expire(): void;
  historyWrites(): number;
}

async function withCommandHost(check: (fixture: CommandHostTest) => Promise<void>): Promise<void> {
  const harness = await loadRenderModule<typeof import("./command-host-fixture")>(
    new URL("./command-host-fixture.tsx", import.meta.url).pathname,
    "CommandHostHarness",
    "installCommandPage",
    "commandRequests",
  );
  const stubs = transport();
  const locale = await installAuthLocale();
  let timers: ReturnType<typeof installManualTimers> | null = null;
  let clock: { mockRestore(): void } | null = null;
  let restoreFetch = (): void => {};
  try {
    timers = installManualTimers();
    const timeout = timers;
    const manualClock = spyOn(performance, "now").mockReturnValue(1_000);
    clock = manualClock;
    restoreFetch = installFetch(stubs);
    await withCommandLayout(
      harness,
      locale.session,
      async (document, router, requests, historyWrites) => {
        await settleAuth();
        expect(stubs.get.calls).toBe(1);
        stubs.get.finish(sessionBody(INITIAL_COOKIE, "operator"));
        await settleAuth();
        const host = required(requests.hosts[0], "初始会话宿主").session;
        expect(adminSessionWritesAllowed(host)).toBe(true);
        expect(confirmedAdminSession(host)?.csrfToken).toBe(INITIAL_COOKIE);
        await check({
          document,
          router,
          requests,
          stubs,
          historyWrites,
          expire: () => {
            manualClock.mockReturnValue(901_000);
            timeout.fire(899_000);
          },
        });
        expect(requests.successes).toBe(0);
      },
    );
  } finally {
    try {
      restoreFetch();
    } finally {
      try {
        clock?.mockRestore();
      } finally {
        try {
          timers?.restore();
        } finally {
          locale.restore();
        }
      }
    }
  }
}

async function withCommandLayout(
  harness: typeof import("./command-host-fixture"),
  locale: Awaited<ReturnType<typeof installAuthLocale>>["session"],
  check: (
    document: AuthTestDocument,
    router: Router,
    requests: CommandRequests,
    historyWrites: () => number,
  ) => Promise<void>,
): Promise<void> {
  const window = installFakeWindow(COMMAND_PATH);
  let restorePage = (): void => {};
  let restoreDocument = (): void => {};
  let disposeRoot = (): void => {};
  try {
    restorePage = harness.installCommandPage();
    const installed = installAuthDocument(`auction_admin_csrf=${INITIAL_COOKIE}`);
    restoreDocument = installed.restore;
    const { document } = installed;
    const requests = harness.commandRequests();
    const router = createRoot((dispose) => {
      disposeRoot = dispose;
      const router = createRouter({ routes: [...ADMIN_ROUTES, harness.COMMAND_ROUTE] });
      mountNode(
        document,
        createComponent(harness.CommandHostHarness, { requests, router, locale }),
      );
      return router;
    });
    flush();
    await check(document, router, requests, () => window.fake.writes.length);
  } finally {
    try {
      disposeRoot();
      flush();
    } finally {
      try {
        restoreDocument();
      } finally {
        try {
          restorePage();
        } finally {
          window.restore();
        }
      }
    }
  }
}

async function sendUnavailableCommand({ document, requests }: CommandHostTest): Promise<void> {
  click(document, "发送会话命令");
  await settleAuth();
  expect(requests.sent).toHaveLength(1);
  expect(requests.replies).toEqual([{ status: 503, code: "storage_unavailable" }]);
  expect(requests.failures).toEqual(["storage_unavailable"]);
  expect(button(document, "原样重试")).not.toBeNull();
}

test("pending_commands_survive_session_review_but_cannot_retry_until_confirmed：复核期间保留命令但确认前不能重试", async () => {
  await withCommandHost(async (fixture) => {
    const { document, requests, stubs } = fixture;
    const host = required(requests.hosts[0], "原会话宿主").session;
    await sendUnavailableCommand(fixture);

    click(document, "复核测试会话");
    await settleAuth();
    expect(stubs.get.calls).toBe(2);
    expect(stubs.logout.calls).toBe(0);
    expect(adminSessionWritesAllowed(host)).toBe(false);
    expect(requests.sourceUnmounted).toBe(false);
    const blocked = required(button(document, "原样重试"), "复核中的原样重试");
    expect(blocked.hasAttribute("disabled")).toBe(true);
    blocked.dispatchClick();
    await settleAuth();
    expect(requests.sent).toHaveLength(1);

    stubs.get.finish(sessionBody(INITIAL_COOKIE, "operator"));
    await settleAuth();
    expect(adminSessionWritesAllowed(host)).toBe(true);
    const retry = required(button(document, "原样重试"), "恢复后的原样重试");
    expect(retry.hasAttribute("disabled")).toBe(false);
    retry.click();
    await settleAuth();
    expect(requests.sent).toHaveLength(2);
    expect(requests.sent[1]).toBe(requests.sent[0]);
    expect(requests.failures).toEqual(["storage_unavailable", "storage_unavailable"]);
    expect(stubs.get.calls).toBe(2);
    expect(button(document, "原样重试")).not.toBeNull();
  });
});

async function endSession(fixture: CommandHostTest, end: SessionEnd): Promise<void> {
  const { document, requests, stubs } = fixture;
  if (end === "logout" || end === "logout-with-current-cookie") {
    click(document, "退出登录");
    await settleAuth();
    expect(stubs.logout.calls).toBe(1);
    if (end === "logout") {
      document.cookie = "";
    } else {
      expect(document.cookie).toBe(`auction_admin_csrf=${INITIAL_COOKIE}`);
    }
    stubs.logout.finish(response(200, '{"code":"ok","message":"","data":null}'));
  } else {
    if (end === "idle") {
      fixture.expire();
      await settleAuth();
    } else {
      const sender = new BroadcastChannel("hv-auction-admin-session");
      try {
        sender.postMessage("session-ended");
        await settleAuth();
        expect(requests.cancelled).toBe(1);
        expect(button(document, "原样重试")).toBeNull();
      } finally {
        sender.close();
      }
    }
    expect(stubs.get.calls).toBe(2);
    expect(stubs.get.pendingCount()).toBe(1);
    stubs.get.finish(failure(401, "unauthenticated"));
  }
  await settleAuth();
}

async function loginNextSession({
  document,
  router,
  stubs,
  requests,
}: CommandHostTest): Promise<void> {
  const form = required(document.querySelectorAll("form").at(0), "新会话登录表单");
  required(document.getElementById("admin-actor"), "管理员输入框").input("operator");
  required(document.getElementById("admin-secret"), "密码框").input("fixture-password");
  await settleAuth();
  const reads = stubs.get.calls;
  form.submit();
  await settleAuth();
  expect(stubs.post.calls).toBe(1);
  document.cookie = `auction_admin_csrf=${NEXT_COOKIE}`;
  stubs.post.finish(sessionBody(NEXT_COOKIE, "operator"));
  await settleAuth();
  expect(stubs.get.calls).toBe(reads + 1);
  expect(button(document, "发送会话命令")).toBeNull();
  stubs.get.finish(sessionBody(NEXT_COOKIE, "operator"));
  await settleAuth();
  expect(router.currentLocationString()).toBe(COMMAND_PATH);
  expect(stubs.post.calls).toBe(1);
  const nextHost = required(requests.hosts[1], "新会话宿主").session;
  expect(nextHost).not.toBe(requests.hosts[0]?.session);
  expect(adminSessionWritesAllowed(nextHost)).toBe(true);
  expect(confirmedAdminSession(nextHost)?.csrfToken).toBe(NEXT_COOKIE);
}

test.each(SESSION_ENDS)(
  "ending_the_session_drops_unknown_records_and_cancels_only_its_waiting_requests：结束会话丢弃未知记录并取消在途请求（%s）",
  async (end) => {
    await withCommandHost(async (fixture) => {
      const { document, router, requests } = fixture;
      const host = required(requests.hosts[0], "原会话宿主").session;
      const entry = router.currentEntryKey();
      await sendUnavailableCommand(fixture);
      click(document, "保持命令在途");
      await settleAuth();
      expect(button(document, "原样重试")).not.toBeNull();
      expect(requests.sent).toHaveLength(2);
      expect(requests.cancelled).toBe(0);
      expect(requests.sourceUnmounted).toBe(false);

      await endSession(fixture, end);
      expect(requests.cancelled).toBe(1);
      expect(requests.sourceUnmounted).toBe(true);
      expect(adminSessionHostLive(host)).toBe(false);
      expect(button(document, "原样重试")).toBeNull();
      expect(router.current()).toMatchObject({ kind: "matched", name: "Login" });
      expect(requests.sent).toHaveLength(2);

      document.cookie = "";
      await loginNextSession(fixture);
      expect(router.currentEntryKey()).toBe(entry);
      expect(button(document, "原样重试")).toBeNull();
      expect(requests.sent).toHaveLength(2);
      click(document, "发送会话命令");
      await settleAuth();
      expect(requests.sent).toHaveLength(3);
      expect(requests.sent[2]).not.toBe(requests.sent[0]);
      expect(requests.cancelled).toBe(1);
    });
  },
);

test("authenticated_outlet_navigation_keeps_the_same_command_host：站内切页保留同一命令宿主", async () => {
  await withCommandHost(async (fixture) => {
    const { document, router, requests, historyWrites } = fixture;
    expect(requests.hosts).toHaveLength(1);
    const initialHosts = required(requests.hosts[0], "初次挂载的宿主");
    const entry = router.currentEntryKey();
    await sendUnavailableCommand(fixture);
    click(document, "保持命令在途");
    await settleAuth();
    expect(button(document, "原样重试")).not.toBeNull();
    expect(requests.sourceUnmounted).toBe(false);
    expect(requests.sent).toHaveLength(2);
    expect(requests.cancelled).toBe(0);

    click(document, "离开命令页");
    await settleAuth();
    expect(requests.sourceUnmounted).toBe(true);
    expect(router.currentLocationString()).toBe("/status");
    expect(document.getElementById("admin-status-title")?.textContent).toBe("服务状态");
    expect(button(document, "发送会话命令")).toBeNull();
    expect(button(document, "原样重试")).not.toBeNull();
    expect(adminSessionHostLive(initialHosts.session)).toBe(true);
    expect(requests.cancelled).toBe(0);

    const writes = historyWrites();
    click(document, "返回命令页");
    await settleAuth();
    expect(router.currentLocationString()).toBe(COMMAND_PATH);
    expect(router.currentEntryKey()).toBe(entry);
    expect(historyWrites()).toBe(writes);
    expect(button(document, "发送会话命令")).not.toBeNull();
    expect(button(document, "原样重试")).not.toBeNull();
    expect(requests.hosts).toHaveLength(2);
    const restoredHosts = required(requests.hosts[1], "后退后挂载的宿主");
    expect(restoredHosts.session).toBe(initialHosts.session);
    expect(restoredHosts.pending).toBe(initialHosts.pending);
    expect(requests.cancelled).toBe(0);
  });
});
