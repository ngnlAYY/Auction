// 实时、审计与站点文档页面共用的内存浏览器，保留实际事件、请求和历史入口。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import type { LocaleCode } from "@auction/web-shared/ui/i18n";
import { type LocaleSession, LocaleSessionContext } from "@auction/web-shared/ui/i18n/session";
import type { JSX } from "@solidjs/web";
import { createRoot, getObserver } from "solid-js";
import {
  createAdminRequestHarness,
  type FetchHandler,
  installFixtureFetch,
  type Reply,
  recordRequest,
} from "../api/api-request-fixture";
import { loadRenderModule } from "../solid-render-fixture.test";
import { cleanupScope } from "../test-scope-fixture";
import { createAdminTranslator, installAdminTranslation } from "./admin-locale-fixture.test";
import { PendingCommandHostContext, usePendingCommandHost } from "./pending";
import {
  createComponent,
  createElement,
  insertNode,
  installTestDocument,
  mountNode,
  settle,
  TestDocument,
  TestElement,
  valueUpdates,
} from "./solid-dom-fixture";

const ORIGIN = "https://auction.test";

/** 页面通过真实传输层发出的请求。 */
export interface PageRequest {
  readonly method: string;
  readonly url: URL;
  readonly body: string;
  readonly signal: AbortSignal | null;
}

/** 同一次挂载的查询、请求记录与测试时语言切换入口。 */
export interface MountedPage {
  readonly document: TestDocument;
  readonly requests: readonly PageRequest[];
  takeRequests(): PageRequest[];
  address(): [string, string][];
  restore(query: string | URLSearchParams): Promise<void>;
  back(): Promise<void>;
  forward(): Promise<void>;
  translateIn(locale: "en" | "zh-CN"): Promise<void>;
}

/** 在同一根下挂载页面和会话宿主，作用域退出时恢复全局环境。 */
export async function withPage(
  address: string,
  render: () => JSX.Element,
  respond: (request: PageRequest) => Reply | Promise<Reply>,
  check: (page: MountedPage) => Promise<void> | void,
): Promise<void> {
  const { AdminSessionHostContext } = await loadRenderModule<
    Pick<typeof import("./auth/shell"), "AdminSessionHostContext">
  >(new URL("./auth/shell.tsx", import.meta.url).pathname, "AdminSessionHostContext");
  const language = createCell<LocaleCode>("zh-CN");
  const scope = cleanupScope();
  try {
    scope.defer(
      installAdminTranslation(
        createAdminTranslator(() => (getObserver() === null ? language.peek() : language())),
      ),
    );
    const { fake, restore } = installFakeWindow(address);
    scope.defer(restore);
    const document = installTestDocument();
    scope.defer(document.restore);
    const requests: PageRequest[] = [];
    const unexpected: unknown[] = [];
    const api = createAdminRequestHarness();
    scope.defer(api.cleanup);
    api.installBrowserRequest();
    const { host } = api.registerHost();
    scope.defer(installFixtureFetch(recordFetch(requests, unexpected, respond)));
    createRoot((dispose) => {
      scope.defer(dispose);
      const pending = usePendingCommandHost(host);
      const locale: LocaleSession = {
        current: language,
        selected: language,
        isReady: () => true,
        error: () => null,
        bootstrap() {},
        switch() {},
      };
      mountNode(document, () =>
        createComponent(LocaleSessionContext, {
          value: locale,
          get children() {
            return createComponent(AdminSessionHostContext, {
              value: host,
              get children() {
                return createComponent(PendingCommandHostContext, {
                  value: pending,
                  get children() {
                    return render();
                  },
                });
              },
            });
          },
        }),
      );
    });
    await settle();
    let cursor = 0;
    await check({
      document,
      requests,
      takeRequests() {
        const next = requests.slice(cursor);
        cursor = requests.length;
        return next;
      },
      address: () => [...fake.location.searchParams],
      async restore(query) {
        fake.history.pushState(null, "", `${fake.location.pathname}?${query}`);
        fake.dispatchEvent(new Event("popstate"));
        await settle();
      },
      async back() {
        fake.history.back();
        await settle();
      },
      async forward() {
        fake.history.forward();
        await settle();
      },
      async translateIn(locale) {
        language.set(locale);
        await settle();
      },
    });
    expect(unexpected).toEqual([]);
  } finally {
    scope.dispose();
  }
}

function recordFetch(
  requests: PageRequest[],
  unexpected: unknown[],
  respond: (request: PageRequest) => Reply | Promise<Reply>,
): FetchHandler {
  return async (input, init) => {
    const recorded = recordRequest(input, init);
    const request = {
      ...recorded,
      url: new URL(recorded.url, ORIGIN),
      signal: init?.signal ?? null,
    };
    requests.push(request);
    try {
      const reply = await respond(request);
      return new Response(reply.body, { status: reply.status ?? 200 });
    } catch (error) {
      // 传输层会把替身抛出的断言归类为网络错误，测试须另外暴露原始失败。
      if (
        !(
          error instanceof DOMException &&
          error.name === "AbortError" &&
          request.signal?.aborted === true
        )
      ) {
        unexpected.push(error);
      }
      throw error;
    }
  };
}

/** 按稳定 ID 获取真实页面生成的控件；缺少控件即为测试失败。 */
export function byId(document: TestDocument, id: string): TestElement {
  const element = document.getElementById(id);
  if (element === null) {
    throw new Error(`缺少 #${id}`);
  }
  return element;
}

/** 按页面可见名称点击按钮，验证按钮存在、启用并绑定事件。 */
export async function click(root: TestDocument | TestElement, label: string): Promise<void> {
  const button = root.querySelectorAll("button").find((node) => node.textContent === label);
  if (button === undefined) {
    throw new Error(`缺少按钮「${label}」`);
  }
  expect(button.hasAttribute("disabled")).toBe(false);
  expect(typeof button.attributes.onClick).toBe("function");
  button.click();
  await settle();
}

/** 从真实表单派发提交事件。 */
export async function submit(form: TestElement): Promise<void> {
  expect(form.tag).toBe("form");
  expect(typeof form.attributes.onSubmit).toBe("function");
  form.submit();
  await settle();
}

/** 从真实输入控件派发 input 事件。 */
export async function input(control: TestElement, value: string): Promise<void> {
  expect(typeof control.attributes.onInput).toBe("function");
  control.input(value);
  await settle();
}

/** 在本次输入后观察一次应用写入，原生控件自身设置 value 不算。 */
export async function inputAndExpectValueUpdate(
  control: TestElement,
  value: string,
): Promise<void> {
  const before = valueUpdates(control).length;
  await input(control, value);
  expect(valueUpdates(control).slice(before)).toContain(value);
}

/** 展开 Solid 的惰性子节点，保留每一段实际渲染的文本及其顺序。 */
export function textParts(root: unknown): string[] {
  if (typeof root === "function") {
    return textParts(root());
  }
  if (Array.isArray(root)) {
    return root.flatMap(textParts);
  }
  if (typeof root === "string" || typeof root === "number") {
    return [String(root)];
  }
  if (root instanceof TestElement) {
    return root.tag === "#text" ? [root.text] : root.nodes.flatMap(textParts);
  }
  return [];
}

/** 读取指定控件实际选中的全部选项，保留空值及原顺序。 */
export function selectedOptions(document: TestDocument, id: string): string[] {
  const control = byId(document, id);
  expect(control.tag).toBe("select");
  return control
    .querySelectorAll("option")
    .filter((option) => option.selected)
    .map((option) => option.value);
}

/** 构造与 Rust TestPage 相同的内存成功回执。 */
export function okReply(data: object, message = "已读取"): Reply {
  return { body: JSON.stringify({ code: "ok", message, data }) };
}

/** 对照 Rust 重试测试的首次 503 回执。 */
export const STORAGE_UNAVAILABLE: Reply = {
  status: 503,
  body: '{"code":"storage_unavailable","message":"存储暂不可用","data":null,"request_id":null}',
};

/** 与 Rust GET 替身相同，只记录请求，直到查询被替换或页面卸载。 */
export function pendingRead({ method, signal }: PageRequest): Promise<Reply> {
  expect(method).toBe("GET");
  if (signal === null) {
    throw new Error("读取请求缺少取消信号");
  }
  return new Promise((_resolve, reject) => {
    if (signal.aborted) {
      reject(new DOMException("已取消", "AbortError"));
      return;
    }
    signal.addEventListener("abort", () => reject(new DOMException("已取消", "AbortError")), {
      once: true,
    });
  });
}

test("页面夹具读取下拉框的实际选中项，包括默认首项和显式空选", () => {
  const document = new TestDocument();
  const select = createElement("select", { id: "status" });
  insertNode(select, createElement("option", { value: "active" }));
  insertNode(select, createElement("option", { value: "completed" }));
  document.add(select);
  expect(selectedOptions(document, "status")).toEqual(["active"]);
  select.value = "completed";
  expect(selectedOptions(document, "status")).toEqual(["completed"]);
  select.value = "missing";
  expect(selectedOptions(document, "status")).toEqual([]);
});
