// 首次部署配置表单的确认目标失效与结果未知反馈回归。

import { afterEach, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { parseJson } from "@auction/web-shared/net/wire";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createRoot, For, flush, Show, untrack } from "solid-js";
import { installBrowserRequest } from "../../api/api-request-fixture";

import { jsonObject } from "../../api/json-fixture";
import { spyOnBrowserFetch } from "../../api/t1-request-fixture.test";
import { installAdminTranslation } from "../admin-locale-fixture.test";
import { createSetupDraft, type SetupDraft, setupDraftSet } from "./draft";
import { ConfigurationForm } from "./form";

const fixtures = new URL(
  "../../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);

const restorers: Array<() => void> = [];

/** 一次 setup 请求的可见部分；正文只在构造阶段是字符串。 */
interface SeenRequest {
  readonly url: string;
  readonly method: string;
  readonly headers: Headers;
  readonly body: string;
}

function installFetch(status: number, body: string): SeenRequest[] {
  const seen: SeenRequest[] = [];
  const fetch = spyOnBrowserFetch();
  restorers.push(() => fetch.restore());
  fetch.respondWith(async (input, init) => {
    seen.push({
      url: typeof input === "string" ? input : input instanceof URL ? input.href : input.url,
      method: init?.method ?? "GET",
      headers: new Headers(init?.headers),
      body: typeof init?.body === "string" ? init.body : "",
    });
    return new Response(body, { status });
  });
  return seen;
}

/** 为延迟聚焦提供最小的 DOM：`getElementById` 返回可聚焦的 HTMLElement 替身。 */
function installFocusProbe(): { readonly focused: string[] } {
  const focused: string[] = [];
  class FocusElement {
    constructor(readonly id: string) {}
    focus(): void {
      focused.push(this.id);
    }
  }
  const previousDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  const previousHtmlElement = Object.getOwnPropertyDescriptor(globalThis, "HTMLElement");
  Object.defineProperty(globalThis, "HTMLElement", { configurable: true, value: FocusElement });
  restorers.push(() => {
    if (previousHtmlElement === undefined) Reflect.deleteProperty(globalThis, "HTMLElement");
    else Object.defineProperty(globalThis, "HTMLElement", previousHtmlElement);
  });
  Object.defineProperty(globalThis, "document", {
    configurable: true,
    value: { getElementById: (id: string) => new FocusElement(id) },
  });
  restorers.push(() => {
    if (previousDocument === undefined) Reflect.deleteProperty(globalThis, "document");
    else Object.defineProperty(globalThis, "document", previousDocument);
  });
  return { focused };
}

/** 表单只依赖“有翻译函数”；可恢复 spy 避免无语言包时消耗一次性错误上报。 */
function installTranslation(): void {
  restorers.push(installAdminTranslation((id) => id));
}

afterEach(() => {
  const errors: unknown[] = [];
  while (restorers.length > 0) {
    try {
      restorers.pop()?.();
    } catch (error) {
      errors.push(error);
    }
  }
  if (errors.length !== 0) throw new AggregateError(errors, "首次部署表单替身恢复失败");
});

function validDraft(): SetupDraft {
  let draft = createSetupDraft();
  for (const [field, value] of [
    ["database_username", "auction"],
    ["database_password", "secret"],
    ["admin_actor", "admin"],
    ["public_origin", "https://public.example.test"],
    ["admin_origin", "https://admin.example.test"],
  ] as const) {
    draft = setupDraftSet(draft, field, value);
  }
  return draft;
}

class CapturedElement {
  constructor(
    readonly tag: unknown,
    readonly attributes: Record<string, unknown>,
    readonly children: readonly unknown[],
  ) {}
}

function captureJsx(
  tag: unknown,
  attributes: Record<string, unknown> | null,
  ...nodes: unknown[]
): unknown {
  const props = attributes ?? {};
  if (tag === Show) {
    const when = props.when;
    const child = nodes.length === 1 ? nodes[0] : nodes;
    if (!when) {
      const fallback = props.fallback;
      return typeof fallback === "function" ? fallback() : fallback;
    }
    return typeof child === "function" ? child(() => when) : child;
  }
  if (tag === For) {
    const each = props.each;
    const child = nodes.length === 1 ? nodes[0] : nodes;
    if (!Array.isArray(each) || typeof child !== "function") {
      return null;
    }
    return each.map((item, index) =>
      child(
        () => item,
        () => index,
      ),
    );
  }
  if (typeof tag === "function") {
    return null;
  }
  return new CapturedElement(tag, props, nodes);
}

function renderedElements(node: unknown): CapturedElement[] {
  if (node instanceof CapturedElement) {
    return [node, ...node.children.flatMap(renderedElements)];
  }
  return Array.isArray(node) ? node.flatMap(renderedElements) : [];
}

function renderForm(
  draft: Cell<SetupDraft>,
  token: Cell<string>,
  notice: Cell<readonly [boolean, string] | null>,
  outcomeUnknown: Cell<boolean>,
): CapturedElement[] {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "jsx");
  if (previous === undefined) {
    throw new Error("测试 JSX 转译未装配");
  }
  Object.defineProperty(globalThis, "jsx", { ...previous, value: captureJsx });
  try {
    return untrack(() =>
      renderedElements(
        ConfigurationForm({
          token,
          draft,
          notice,
          outcomeUnknown,
          initializeAndStart: true,
          onAuthFailure() {},
          onCompleted() {},
        }),
      ),
    );
  } finally {
    Object.defineProperty(globalThis, "jsx", previous);
  }
}

function controls(elements: readonly CapturedElement[]) {
  const form = elements.find(({ tag }) => tag === "form");
  const confirm = elements.find(
    ({ tag, attributes }) => tag === "input" && attributes.id === "setup-confirm-initialize",
  );
  const onChange = confirm?.attributes.onChange;
  const onSubmit = form?.attributes.onSubmit;
  if (typeof onChange !== "function" || typeof onSubmit !== "function") {
    throw new Error("未装配首次部署确认表单");
  }
  return {
    onChange: (event: { currentTarget: { checked: boolean } }) => {
      onChange(event);
    },
    onSubmit: (event: { preventDefault(): void }) => {
      onSubmit(event);
    },
  };
}

async function waitFor(condition: () => boolean, label: string): Promise<void> {
  const deadline = Date.now() + 1_000;
  while (!condition()) {
    if (Date.now() > deadline) {
      throw new Error(`等待超时：${label}`);
    }
    await Bun.sleep(0);
    flush();
  }
}

test("setup_target_mismatch_clears_confirmation_and_allows_reconfirming：目标不一致时清除确认并允许重新确认", async () => {
  const responseBody = readFileSync(
    new URL("setup-confirm-target-mismatch.json", fixtures),
    "utf8",
  );
  installTranslation();
  restorers.push(installBrowserRequest());
  const seen = installFetch(400, responseBody);
  const focus = installFocusProbe();
  const draft = createCell(validDraft());
  const token = createCell("deploy-token");
  const notice = createCell<readonly [boolean, string] | null>(null);
  const outcomeUnknown = createCell(false);
  let dispose = (): void => {};
  let onChange: ReturnType<typeof controls>["onChange"] | undefined;
  try {
    createRoot((disposeRoot) => {
      dispose = disposeRoot;
      const controlled = controls(renderForm(draft, token, notice, outcomeUnknown));
      onChange = controlled.onChange;
      onChange({ currentTarget: { checked: true } });
      expect(draft.peek().confirmedDatabaseTarget).toEqual({
        host: "127.0.0.1",
        port: 5432,
        name: "auction",
      });

      controlled.onSubmit({ preventDefault() {} });
    });
    await waitFor(
      () =>
        draft.peek().confirmedDatabaseTarget === null &&
        notice.peek()?.[1] === "setup-error-confirm-initialize",
      "目标不一致回执",
    );
    if (onChange === undefined) {
      throw new Error("未取得确认事件处理器");
    }
    expect(draft.peek().confirmedDatabaseTarget).toBeNull();
    expect(notice.peek()).toEqual([false, "setup-error-confirm-initialize"]);
    await waitFor(() => focus.focused.includes("setup-confirm-initialize"), "聚焦重新确认控件");
    expect(seen).toHaveLength(1);
    const request = seen[0];
    if (request === undefined) throw new Error("未记录首次部署请求");
    expect([request.method, request.url]).toEqual(["POST", "/api/v1/setup/complete"]);
    expect(request.headers.get("x-setup-confirm-target")).toBe(
      "host=127.0.0.1&port=5432&name=auction",
    );
    const parsedBody = parseJson(request.body);
    expect(parsedBody.ok).toBe(true);
    if (!parsedBody.ok) throw new Error("首次部署请求正文不是合法 JSON");
    expect(jsonObject(parsedBody.value, "首次部署请求").confirm_initialize).toBe(true);

    // 输入已经变化但页面仍显示旧目标：刷新前 memo 尚未读到新草稿。
    draft.set(setupDraftSet(draft.peek(), "database_name", "current_target"));
    onChange({ currentTarget: { checked: true } });
    expect(draft.peek().confirmedDatabaseTarget).toBeNull();

    flush();
    onChange({ currentTarget: { checked: true } });
    expect(draft.peek().confirmedDatabaseTarget).toEqual({
      host: "127.0.0.1",
      port: 5432,
      name: "current_target",
    });
  } finally {
    dispose();
    flush();
  }
});

test("setup_unknown_outcome_requires_manual_verification：结果未知时要求人工核验", async () => {
  installTranslation();
  restorers.push(installBrowserRequest());
  const seen = installFetch(500, '{"code":"setup_outcome_unknown"}');
  const draft = createCell(validDraft());
  const token = createCell("deploy-token");
  const notice = createCell<readonly [boolean, string] | null>(null);
  const outcomeUnknown = createCell(false);
  let dispose = (): void => {};
  try {
    createRoot((disposeRoot) => {
      dispose = disposeRoot;
      const { onChange, onSubmit } = controls(renderForm(draft, token, notice, outcomeUnknown));
      onChange({ currentTarget: { checked: true } });
      expect(draft.peek().confirmedDatabaseTarget).not.toBeNull();
      onSubmit({ preventDefault() {} });
    });
    await waitFor(() => outcomeUnknown.peek(), "结果未知回执");
    expect(notice.peek()).toEqual([false, "setup-error-outcome-unknown"]);
    expect(draft.peek().confirmedDatabaseTarget).toBeNull();
    expect(draft.peek().values.admin_secret).toBe("");
    expect(draft.peek().values.database_password).toBe("");
    expect(outcomeUnknown.peek()).toBe(true);
    expect(token.peek()).toBe("");
    expect(seen).toHaveLength(1);
    const request = seen[0];
    if (request === undefined) throw new Error("未记录首次部署请求");
    expect([request.method, request.url]).toEqual(["POST", "/api/v1/setup/complete"]);
    expect(request.headers.get("x-setup-confirm-target")).toBe(
      "host=127.0.0.1&port=5432&name=auction",
    );
  } finally {
    dispose();
    flush();
  }
});
