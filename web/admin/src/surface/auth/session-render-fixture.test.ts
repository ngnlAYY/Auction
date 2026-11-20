// 真实 Admin 布局的内存挂载与用户创建回执夹具。

import { expect } from "bun:test";
import { createRouter, type Router } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { createRoot, flush } from "solid-js";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { cleanupScope, preserveProperty } from "../../test-scope-fixture";
import { ADMIN_ROUTES } from "../router/route";
import { createComponent, mountNode, required, type TestElement } from "../solid-dom-fixture";
import { button, field, submitForm } from "../t10-view-fixture.test";
import {
  type AuthTestDocument,
  installAuthBroadcast,
  installAuthDocument,
  settleAuth,
} from "./auth-environment-fixture";
import { installAuthLocale } from "./auth-locale-fixture.test";
import {
  installFetch,
  response,
  sessionBody,
  type Transport,
  transport,
} from "./effects-session-fixture";
import type { CommandRequests } from "./session/command-host-fixture";

/** 原宿主使用的 Cookie。 */
export const COOKIE_A = "auction_admin_csrf=fixture-cookie";
/** 替代会话使用的 Cookie。 */
export const COOKIE_B = "auction_admin_csrf=cookie-b";
/** 打开回执中一次性 Key 的按钮文案。 */
export const VIEW_KEY = "查看生成的 Key";
/** 用户创建回执给出的一次性 Key。 */
export const GENERATED = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/** 真实认证布局的文档、路由与请求替身。 */
export interface Layout {
  readonly stubs: Transport;
  readonly document: AuthTestDocument;
  readonly router: Router;
  readonly requests: CommandRequests;
  readonly broadcast: ReturnType<typeof installAuthBroadcast>;
  readonly pathname: () => string;
}

/** 挂载生产认证布局，初始 GET 由用例交付。 */
export async function withLayout(
  path: string,
  check: (fixture: Layout) => Promise<void>,
  channelAvailable = true,
) {
  const { COMMAND_ROUTE, CommandHostHarness, commandRequests, installCommandPage } =
    await loadRenderModule<typeof import("./session/command-host-fixture")>(
      new URL("./session/command-host-fixture.tsx", import.meta.url).pathname,
      "CommandHostHarness",
      "commandRequests",
      "installCommandPage",
    );
  const scope = cleanupScope();
  try {
    const locale = installAuthLocale();
    scope.defer(locale.restore);
    const window = installFakeWindow(path);
    scope.defer(window.restore);
    const { document, restore } = installAuthDocument(COOKIE_A);
    scope.defer(restore);
    const stubs = transport();
    scope.defer(installFetch(stubs));
    const broadcast = installAuthBroadcast();
    scope.defer(broadcast.restore);
    if (!channelAvailable) {
      scope.defer(preserveProperty(globalThis, "BroadcastChannel"));
      Object.defineProperty(globalThis, "BroadcastChannel", {
        configurable: true,
        value: undefined,
      });
    }
    scope.defer(installCommandPage());
    const requests = commandRequests();
    const router = createRoot((dispose) => {
      scope.defer(() => {
        dispose();
        flush();
      });
      const router = createRouter({ routes: [...ADMIN_ROUTES, COMMAND_ROUTE] });
      mountNode(
        document,
        createComponent(CommandHostHarness, {
          router,
          locale: locale.session,
          requests,
        }),
      );
      return router;
    });
    await settleAuth();
    expect(stubs.get.calls).toBe(1);
    await check({
      stubs,
      document,
      router,
      broadcast,
      requests,
      pathname: () => window.fake.location.pathname,
    });
  } finally {
    scope.dispose();
  }
}

/** 读取内存文档可见的文本。 */
export function text(document: AuthTestDocument): string {
  return document.roots.map((node) => node.textContent).join("");
}

/** 点击已启用的实际渲染按钮。 */
export function click(document: AuthTestDocument, label: string): TestElement {
  const target = button(document, label);
  expect(target.hasAttribute("disabled")).toBe(false);
  target.click();
  return target;
}

/** 核对明文已从文本、元素和属性移除。 */
export function assertKeyHidden(document: AuthTestDocument): void {
  expect(
    document.querySelectorAll("code").filter((node) => node.textContent === GENERATED),
  ).toHaveLength(0);
  expect(text(document)).not.toContain(GENERATED);
  expect(
    document
      .querySelectorAll("*")
      .filter((node) =>
        Object.values(node.attributes).some(
          (value) => typeof value === "string" && value.includes(GENERATED),
        ),
      ),
  ).toHaveLength(0);
}

/** 从真实用户表单提交，交付写前复核并保留在途 POST。 */
export async function beginCreate({ stubs, document }: Layout, username = "ExamplePlayer") {
  click(document, "创建用户");
  await settleAuth();
  field(document, "create-username").input(username);
  submitForm(
    required(
      document.querySelectorAll("form").find((form) => form.matches(".admin-users-create")),
      "创建表单",
    ),
  );
  await settleAuth();
  stubs.get.finish(sessionBody());
  await settleAuth();
  expect(stubs.createUser.pendingCount()).toBe(1);
}

/** 构造创建用户的完整成功回执。 */
export function generatedUser(
  username = "ExamplePlayer",
  key = GENERATED,
  userId = "00000000-0000-4000-8000-000000000026",
): ReturnType<typeof response> {
  return response(
    201,
    JSON.stringify({
      code: "created",
      message: "用户已创建",
      data: {
        generated_key: key,
        user: {
          id: userId,
          username,
          forum_account_id: null,
          status: "active",
          key_status: "active",
          state_version: 1,
          created_at: "2025-11-15T09:46:40.110000Z",
          updated_at: "2025-11-15T09:46:40.110000Z",
        },
      },
    }),
  );
}
