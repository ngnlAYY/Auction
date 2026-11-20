// T11 页面夹具：真实路由、会话宿主与未决命令外壳，HTTP 由内存回执应答。

import { expect, test } from "bun:test";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { createRouter, RouterContext } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import type { JSX } from "@solidjs/web";
import { createComponent, Show } from "solid-js";
import {
  createAdminRequestHarness,
  installFixtureFetch,
  type RecordedRequest,
  type Reply,
  recordRequest,
  TEST_SESSION_BODY,
} from "../../api/api-request-fixture";
import { fetchAdminReadiness } from "../../api/status";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { PendingCommandHostContext, usePendingCommandHost } from "../pending";
import { findAll, type TestElement } from "../solid-dom-fixture";
import { withT11View } from "./t11-solid-fixture.test";

/** 向真实 API 返回成功信封。 */
export function t11Reply(data: unknown): Reply {
  return { body: JSON.stringify({ code: "ok", message: "ok", data }) };
}

/** 装入局部内存传输；路由、方法、原始正文均可观测。 */
export function installT11Fetch(reply: (request: RecordedRequest) => Reply | Promise<Reply>) {
  const requests: RecordedRequest[] = [];
  const restore = installFixtureFetch(async (input, init) => {
    const request = recordRequest(input, init);
    requests.push(request);
    const response =
      request.method === "GET" && request.url === "/api/v1/admin/session?include=server_time"
        ? { body: JSON.stringify(TEST_SESSION_BODY) }
        : await reply(request);
    return new Response(response.body, { status: response.status ?? 200 });
  });
  return { requests, restore };
}

/** 寻找具有精确可见名称的已装配按钮。 */
export function t11Buttons(tree: TestElement, text: string): TestElement[] {
  return findAll(
    tree,
    (element) => element.tag === "button" && element.textContent.trim() === text,
  );
}

/** 挂载一个页面和稳定的外壳；用例通过真实组件事件或路由状态驱动更新。 */
export async function withT11AdminPage(
  initialUrl: string,
  Page: () => JSX.Element,
  run: (view: {
    readonly tree: TestElement;
    readonly document: Parameters<Parameters<typeof withT11View>[1]>[1];
    navigate(): void;
    readiness(): Awaited<ReturnType<typeof fetchAdminReadiness>> | undefined;
  }) => Promise<void>,
): Promise<void> {
  const { AdminSessionHostContext: Session } = await loadRenderModule<
    typeof import("../auth/shell")
  >(new URL("../auth/shell.tsx", import.meta.url).pathname, "AdminSessionHostContext");
  const { AdminReadinessContext: Readiness } = await loadRenderModule<
    typeof import("../auth/index")
  >(new URL("../auth/index.tsx", import.meta.url).pathname, "AdminReadinessContext");
  const { PendingCommandNotices: Notices } = await loadRenderModule<
    typeof import("../pending/panel")
  >(new URL("../pending/panel.tsx", import.meta.url).pathname, "PendingCommandNotices");
  const { pathname } = new URL(initialUrl, "https://auction.test");
  const harness = createAdminRequestHarness();
  try {
    harness.installBrowserRequest();
    const { host } = harness.registerHost();
    const window = installFakeWindow(initialUrl);
    try {
      let controls: Pick<Parameters<typeof run>[0], "navigate" | "readiness"> | null = null;
      await withT11View(
        () => {
          const router = createRouter({
            routes: [
              { name: "Page", pattern: pathname },
              { name: "Outside", pattern: "/outside" },
            ],
          });
          const readiness = createTaskResource(
            () => null,
            (_key, { signal }) => fetchAdminReadiness(host, signal),
          );
          controls = {
            navigate: () => {
              const route = router.current();
              if (route.kind === "matched" && route.name === "Page") {
                router.push("/outside");
              } else {
                router.goBack();
              }
            },
            readiness,
          };
          const pending = usePendingCommandHost(host);
          return createComponent(RouterContext, {
            value: router,
            get children() {
              return createComponent(Session, {
                value: host,
                get children() {
                  return createComponent(Readiness, {
                    value: readiness,
                    get children() {
                      return createComponent(PendingCommandHostContext, {
                        value: pending,
                        get children() {
                          return [
                            createComponent(Notices, { host: pending }),
                            createComponent(Show<boolean>, {
                              get when() {
                                const route = router.current();
                                return route.kind === "matched" && route.name === "Page";
                              },
                              get children() {
                                return createComponent(Page, {});
                              },
                            }),
                          ];
                        },
                      });
                    },
                  });
                },
              });
            },
          });
        },
        async (tree, document) => {
          if (controls === null) {
            throw new Error("测试页面没有挂载");
          }
          await run({ tree, document, ...controls });
        },
      );
    } finally {
      window.restore();
    }
  } finally {
    harness.cleanup();
  }
}

test("仪表盘传输夹具记录原始请求并转交业务回执", async () => {
  const endpoint = "/fixture?key=t11";
  const fixture = installT11Fetch(() => ({ ...t11Reply({ seen: true }), status: 202 }));
  try {
    const response = await fetch(endpoint, { method: "POST", body: '{"value":1}' });
    expect(response.status).toBe(202);
    expect(await response.text()).toBe('{"code":"ok","message":"ok","data":{"seen":true}}');
    expect(fixture.requests).toEqual([{ url: endpoint, method: "POST", body: '{"value":1}' }]);
  } finally {
    fixture.restore();
  }
});
