// 配置卡片共用的真实 Provider、内存 HTTP 端点和测试路由。

import { expect, test } from "bun:test";
import { LocaleSessionContext } from "@auction/web-shared/ui/i18n/session";
import { createRoot, type Element, useContext } from "solid-js";
import {
  installBrowserRequest,
  installWriteFetch,
  type WriteReply,
} from "../../api/api-request-fixture";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { cleanupScope } from "../../test-scope-fixture";
import { PendingCommandHostContext } from "../pending";
import { createPendingHarness } from "../pending/pending-command-fixture";
import {
  captureEvent,
  createElement,
  findElement,
  insert,
  insertNode,
  installTestDocument,
  removeNode,
  required,
  type TestElement,
} from "../solid-dom-fixture";
import { configLocaleSession, provide } from "./config-test-scope-fixture.test";

const { AdminSessionHostContext } = await loadRenderModule<typeof import("../auth/shell")>(
  new URL("../auth/shell.tsx", import.meta.url).pathname,
  "AdminSessionHostContext",
);

class TestNetworkFailure extends TypeError {}

/** 模拟浏览器 fetch 在收到响应头之前失败。 */
export function networkFailure(): never {
  throw new TestNetworkFailure("network_error");
}

/** 模拟配置页切换时，页面根卸载，但会话宿主可继续执行在途任务。 */
export function configPage(reply: WriteReply) {
  const scope = cleanupScope();
  try {
    const document = installTestDocument();
    scope.defer(document.restore);
    scope.defer(installBrowserRequest());
    const unexpected: unknown[] = [];
    const fetch = installWriteFetch(async (request, init, index) => {
      try {
        return await reply(request, init, index);
      } catch (error) {
        if (
          error instanceof TestNetworkFailure ||
          (error instanceof DOMException && error.name === "AbortError" && init?.signal?.aborted)
        ) {
          throw error;
        }
        unexpected.push(error);
        throw error;
      }
    });
    scope.defer(fetch.restore);
    const pending = createPendingHarness();
    scope.defer(pending.dispose);
    const root = createElement("div");
    document.add(root);
    return {
      document,
      pending,
      root,
      requests: fetch.writes,
      mount(render: () => Element) {
        const mounted = cleanupScope();
        scope.defer(mounted.dispose);
        try {
          const slot = createElement("div");
          insertNode(root, slot);
          mounted.defer(() => removeNode(slot));
          return createRoot((stop) => {
            mounted.defer(stop);
            insert(slot, () =>
              provide(LocaleSessionContext, configLocaleSession, () =>
                provide(AdminSessionHostContext, pending.host.sessionHost, () =>
                  provide(PendingCommandHostContext, pending.host, render),
                ),
              ),
            );
            return mounted.dispose;
          });
        } catch (error) {
          mounted.dispose();
          throw error;
        }
      },
      dispose() {
        scope.dispose();
        expect(unexpected).toEqual([]);
      },
    };
  } catch (error) {
    scope.dispose();
    throw error;
  }
}

/** 仅推进微任务及 Solid 更新，不推进业务定时器。 */
export { settle } from "../solid-dom-fixture";

/** 对应 Rust 的 RecordRead，收到该请求后才允许释放预先安排的回执。 */
export function deferredReply() {
  const { promise, resolve } = Promise.withResolvers<Response>();
  let phase: "new" | "waiting" | "answered" = "new";
  return {
    wait(): Promise<Response> {
      expect(phase).toBe("new");
      phase = "waiting";
      return promise;
    },
    respond(response: Response): void {
      expect(phase).toBe("waiting");
      phase = "answered";
      resolve(response);
    },
  };
}

/** 在实际的状态或错误提示节点核对完整文案。 */
export function noticeLine(
  tree: TestElement,
  role: "alert" | "status",
  selector: string,
  message: string,
): TestElement | null {
  return findElement(
    tree,
    (node) =>
      node.matches(selector) &&
      node.textContent === message &&
      (node.getAttribute("role") === role || node.parentElement?.getAttribute("role") === role),
  );
}

/** 从实际挂载树按按钮文案定位。 */
export function maybeButton(tree: TestElement, text: string): TestElement | null {
  return findElement(tree, (node) => node.tag === "button" && node.textContent.trim() === text);
}

/** 必须存在的实际按钮。 */
export function button(tree: TestElement, text: string): TestElement {
  return required(maybeButton(tree, text), `按钮「${text}」`);
}

/** 正常用户操作须有事件且按钮可用；旧回调围栏测试直接调用节点事件。 */
export function clickButton(tree: TestElement, text: string): void {
  const control = button(tree, text);
  expect(control.hasAttribute("disabled")).toBe(false);
  captureEvent(control, "onClick")();
}

/** 在挂载后读取由真实 Provider 提供的值。 */
export function getProvidedHost() {
  return {
    admin: required(useContext(AdminSessionHostContext), "认证宿主"),
    pending: required(useContext(PendingCommandHostContext), "未决宿主"),
  };
}

test("配置页的延迟回执夹具只允许对已发起的请求交付一次", async () => {
  const fixture = deferredReply();
  const response = new Response("fixture reply");
  expect(() => fixture.respond(response)).toThrow();
  const pending = fixture.wait();
  expect(() => fixture.wait()).toThrow();
  fixture.respond(response);
  expect(await pending).toBe(response);
  expect(() => fixture.respond(response)).toThrow();
});
