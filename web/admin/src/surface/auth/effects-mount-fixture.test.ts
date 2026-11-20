// 认证副作用测试的会话挂载与真实登录表单驱动。

import { expect, test } from "bun:test";
import { createRoot, flush } from "solid-js";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { createComponent, mountNode, required } from "../solid-dom-fixture";
import {
  type AuthTestDocument,
  installAuthDocument,
  installManualTimers,
  settleAuth,
} from "./auth-environment-fixture";
import { installAuthLocale } from "./auth-locale-fixture.test";
import { type AdminSessionHandle, useSession } from "./effects";
import {
  failure,
  installFetch,
  sessionBody,
  type Transport,
  transport,
} from "./effects-session-fixture";

/** 挂载会话状态机及可选视图，持有本次会话的释放入口。 */
export interface MountedSession {
  readonly session: AdminSessionHandle;
  readonly document: AuthTestDocument;
  dispose(): void;
}

/** 安装会话传输和文档，按需在同一根作用域里挂载视图。 */
export function mountSession(
  stubs: Transport,
  render?: (session: AdminSessionHandle, document: AuthTestDocument) => void,
): MountedSession {
  const { document, restore: restoreDocument } = installAuthDocument(
    "auction_admin_csrf=fixture-cookie",
  );
  let restoreFetch = (): void => {};
  let disposeRoot = (): void => {};
  const dispose = (): void => {
    try {
      disposeRoot();
      flush();
    } finally {
      try {
        restoreFetch();
      } finally {
        restoreDocument();
      }
    }
  };
  try {
    restoreFetch = installFetch(stubs);
    const session = createRoot((dispose) => {
      disposeRoot = dispose;
      const handle = useSession();
      render?.(handle, document);
      return handle;
    });
    flush();
    return { session, document, dispose };
  } catch (error) {
    dispose();
    throw error;
  }
}

/** 挂载状态机并完成一次成功的 Cookie 读取。 */
export async function authenticated(stubs: Transport): Promise<MountedSession> {
  const mounted = mountSession(stubs);
  try {
    await settleAuth();
    expect(stubs.get.calls).toBe(1);
    stubs.get.finish(sessionBody());
    await settleAuth();
    expect(mounted.session.view.peek().canWrite).toBe(true);
    return mounted;
  } catch (error) {
    mounted.dispose();
    throw error;
  }
}

/** 驱动真实登录表单，核对广播后的卸载、重挂与反馈清除。 */
export async function verifyBroadcastLogin(
  failureKind: "invalid_credentials" | "timeout" | null,
): Promise<void> {
  const { LoginHarness } = await loadRenderModule<typeof import("./effects-fixture")>(
    new URL("./effects-fixture.tsx", import.meta.url).pathname,
    "LoginHarness",
  );
  const stubs = transport();
  const locale = await installAuthLocale();
  let timers: ReturnType<typeof installManualTimers> | null = null;
  let mounted: MountedSession | null = null;
  try {
    timers = installManualTimers();
    mounted = mountSession(stubs, (session, document) => {
      mountNode(document, createComponent(LoginHarness, { session }));
    });
    const { document, session } = mounted;
    const text = () => document.roots.map((node) => node.textContent).join("");
    await settleAuth();
    expect(stubs.get.calls).toBe(1);
    stubs.get.finish(failure(401, "unauthenticated"));
    await settleAuth();
    expect(session.view.peek().loggedOut).toBe(true);

    const form = required(document.querySelectorAll("form").at(0), "登录表单");
    const inputs = document.querySelectorAll("input");
    expect(inputs.map((input) => input.id)).toEqual(["admin-actor", "admin-secret"]);
    const actor = required(inputs[0], "用户名输入框");
    const secret = required(inputs[1], "密码输入框");
    actor.input("fixture-operator");
    secret.input("fixture-password");
    await settleAuth();
    form.submit();
    await settleAuth();
    expect(stubs.post.calls).toBe(1);
    expect(stubs.loginAuthorizations).toEqual([
      "Basic Zml4dHVyZS1vcGVyYXRvcjpmaXh0dXJlLXBhc3N3b3Jk",
    ]);
    expect(session.view.peek().loginPending).toBe(true);
    expect(document.getElementById("admin-secret")?.value).toBe("");
    expect(form.querySelectorAll("button").map((button) => button.textContent)).toContain(
      "登录中…",
    );

    const broadcast = required(
      document.querySelectorAll("button").find((button) => button.textContent === "广播结束会话"),
      "广播按钮",
    );
    expect(broadcast.hasAttribute("disabled")).toBe(false);
    broadcast.click();
    broadcast.click();
    await settleAuth();
    expect(session.view.peek().loggedOut).toBe(false);
    expect(session.view.peek().canWrite).toBe(false);
    expect(document.getElementById("admin-actor")).toBeNull();
    expect(text()).not.toContain("登录中…");
    expect(stubs.get.calls).toBe(1);

    if (failureKind === "timeout") {
      // 由共享传输自身的预算触发未知结果。
      timers.fire(10_000);
    } else {
      stubs.post.finish(failureKind === null ? sessionBody() : failure(401, "invalid_credentials"));
    }
    await settleAuth();
    expect(stubs.get.calls).toBe(2);
    expect(session.view.peek().canWrite).toBe(false);
    expect(session.view.peek().loggedOut).toBe(false);

    stubs.get.finish(failureKind === null ? sessionBody() : failure(401, "unauthenticated"));
    await settleAuth();
    expect(stubs.get.calls).toBe(2);
    expect(stubs.post.calls).toBe(1);
    expect(session.view.peek().canWrite).toBe(failureKind === null);
    expect(session.view.peek().loggedOut).toBe(failureKind !== null);
    if (failureKind !== null) {
      const code = failureKind === "timeout" ? "request_timeout" : "invalid_credentials";
      const message =
        failureKind === "timeout" ? "操作结果尚不确定，人工核对前请勿重试。" : "用户名或密码无效。";
      expect(session.view.peek().loginError).toBe(code);
      expect(document.getElementById("admin-login-error")?.textContent).toBe(message);
      const remounted = required(document.getElementById("admin-actor"), "重挂表单的用户名输入框");
      expect(remounted).not.toBe(actor);
      remounted.input("fixture-new-input");
      await settleAuth();
      expect(session.view.peek().loginError).toBeNull();
      expect(text()).not.toContain(message);
    }
    expect(stubs.post.calls).toBe(1);
    expect(stubs.get.calls).toBe(2);
  } finally {
    try {
      mounted?.dispose();
    } finally {
      try {
        timers?.restore();
      } finally {
        locale.restore();
      }
    }
  }
}

test("会话挂载夹具卸载时取消待读请求并恢复全局属性", async () => {
  const originalFetch = globalThis.fetch;
  const originalDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  const stubs = transport();
  const mounted = mountSession(stubs);
  try {
    await settleAuth();
    expect(stubs.get.pendingCount()).toBe(1);
  } finally {
    mounted.dispose();
  }
  expect(stubs.get.cancelled).toBe(1);
  expect(globalThis.fetch).toBe(originalFetch);
  expect(Object.getOwnPropertyDescriptor(globalThis, "document")).toEqual(originalDocument);
});
