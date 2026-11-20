// 内存装配真实的 Admin 编辑组件、未决命令宿主与读取控制面。

import { expect, spyOn, test } from "bun:test";
import { DEFAULT_REQUEST_TIMEOUT_MILLIS } from "@auction/web-shared/net/transport";
import { parseJson } from "@auction/web-shared/net/wire";
import type { Cell } from "@auction/web-shared/reactive/cell";
import type { Result } from "@auction/web-shared/result";
import { createRouter, RouterContext } from "@auction/web-shared/router";
import { installFakeWindow } from "@auction/web-shared/router/test-window";
import { LocaleSessionContext } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { type Accessor, createRoot, Errored, flush } from "solid-js";
import {
  createAdminRequestHarness,
  installFixtureFetch,
  jsonObject,
  recordRequest,
  TEST_SESSION_BODY,
} from "../../api/api-request-fixture";
import { success } from "../../api/auction-contract-fixture.test";
import type { AdminSessionHost } from "../../api/session";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { preserveProperty } from "../../test-scope-fixture";
import { adminLocaleSession, installAdminTranslation } from "../admin-locale-fixture.test";
import {
  PendingCommandHostContext,
  type PendingCommands,
  PendingCommandsContext,
  usePendingCommandHost,
} from "../pending";
import { createCommandReload, usePendingCommands } from "../pending/sources";
import {
  createComponent,
  installTestDocument,
  mountNode,
  TestElement,
  visit,
} from "../solid-dom-fixture";
import type { DraftRead } from "./editor";

/** 编辑测试新开的场次或拍品对话框都用此标题栏名称关闭。 */
export const DIALOG_TITLE_CLOSE = "取消";

const SESSION_PATH = "/api/v1/admin/session?include=server_time";

/** 收集真实网络边界收到的方法、相对地址和冻结正文。 */
export interface CapturedWrite {
  readonly method: string;
  readonly path: string;
  readonly body: string;
}

/** Rust CapturedWrite 同时验证命令 ID 是第 4 版 UUID。 */
export function commandId(write: CapturedWrite): string {
  const id = jsonObject(success(parseJson(write.body)), "命令正文").command_id;
  if (typeof id !== "string") {
    throw new Error("写命令缺少 command_id 字符串");
  }
  expect(id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u);
  return id;
}

/** 取一个确定存在的测试值，避免缺控件或缺读取时短路断言。 */
export function present<T>(value: T | null | undefined, label: string): T {
  if (value === null || value === undefined) {
    throw new Error(`缺少${label}`);
  }
  return value;
}

/** 让 Bun 的 fetch、Solid 的排队副作用和任务微任务全部向前推进。 */
export async function settleEditor(): Promise<void> {
  for (let round = 0; round < 8; round += 1) {
    await new Promise<void>((resolve) => setImmediate(resolve));
    flush();
  }
}

/** 每次真实读取都先挂起，再由用例按时序交付指定结果。 */
export function recordRead<T>() {
  const reads: ((value: Result<T, string>) => void)[] = [];
  let count = 0;
  return {
    fetch: () => {
      count += 1;
      return new Promise<Result<T, string>>((resolve) => reads.push(resolve));
    },
    started: () => count,
    async respond(result: Result<T, string>) {
      present(reads.shift(), "已开始的读取")(result);
      await settleEditor();
    },
  };
}

/** 页级草稿读取与响应式版本探针。 */
export interface ReadView<T> {
  readonly read: Accessor<DraftRead>;
  readonly snapshot: Accessor<T | null>;
}

/** 在已挂载组件内运行真实草稿读取逻辑。 */
export interface EditorScope {
  readonly owner: PendingCommands;
  readonly host: AdminSessionHost;
  readonly reload: Cell<bigint>;
  useRead<T>(
    scope: string,
    editing: Accessor<boolean>,
    fetch: () => Promise<Result<T, string>>,
  ): ReadView<T>;
}

/** 对照 Rust 编辑助手，在一次挂载内寻找控件、派发事件与核对请求。 */
export interface EditorTest {
  find(predicate: (element: TestElement) => boolean): TestElement[];
  textNodes(root?: TestElement): string[];
  button(label: string): TestElement | undefined;
  click(label: string): Promise<void>;
  field(id: string): TestElement;
  form(): TestElement;
  text(): string;
  hasText(text: string): boolean;
  retry(command: string): TestElement | undefined;
  reply(status: number, body: string): void;
  conflict(code: string): void;
  replyTimeout(): void;
  takeWrites(): CapturedWrite[];
}

function inside(root: unknown, predicate: (element: TestElement) => boolean): TestElement[] {
  const result: TestElement[] = [];
  visit(root, (node) => {
    if (predicate(node)) result.push(node);
  });
  return result;
}

function textNodes(root: unknown): string[] {
  if (typeof root === "function") return textNodes(root());
  if (Array.isArray(root)) return root.flatMap(textNodes);
  if (root instanceof TestElement) {
    return root.tag === "#text" ? [root.text] : root.nodes.flatMap(textNodes);
  }
  return typeof root === "string" || typeof root === "number" ? [String(root)] : [];
}

function namedButton(root: unknown, label: string): TestElement | undefined {
  return inside(
    root,
    (node) =>
      node.tag === "button" &&
      (node.textContent === label || node.getAttribute("aria-label") === label),
  )[0];
}

function installDialogAncestors(cleanups: (() => void)[]): void {
  const modal = '[role="dialog"][aria-modal="true"]';
  const prototype = TestElement.prototype;
  cleanups.push(preserveProperty(prototype, "closest"));
  Object.defineProperty(prototype, "closest", {
    configurable: true,
    value(this: TestElement, selector: string) {
      for (let node: TestElement | null = this; node !== null; node = node.parentElement) {
        const matches =
          selector === modal
            ? node.getAttribute("role") === "dialog" && node.getAttribute("aria-modal") === "true"
            : node.matches(selector);
        if (matches) return node;
      }
      return null;
    },
  });
  cleanups.push(preserveProperty(prototype, "contains"));
  Object.defineProperty(prototype, "contains", {
    configurable: true,
    value(this: TestElement, child: TestElement) {
      for (let node: TestElement | null = child; node !== null; node = node.parentElement) {
        if (node === this) return true;
      }
      return false;
    },
  });
  cleanups.push(preserveProperty(globalThis, "Element"));
  Object.defineProperty(globalThis, "Element", { configurable: true, value: TestElement });
}

function cleanupAll(cleanups: (() => void)[]): void {
  const errors: unknown[] = [];
  for (const cleanup of cleanups.reverse()) {
    try {
      cleanup();
    } catch (error) {
      errors.push(error);
    }
  }
  if (errors.length > 0) throw new AggregateError(errors, "恢复编辑测试替身失败");
}

/** 使用真实中文 FTL 生成用例独立译文，安装和释放替身均限定在本用例。 */
export async function withEditor(
  render: (scope: EditorScope) => JSX.Element,
  run: (page: EditorTest) => void | Promise<void>,
  read?: (path: string, signal: AbortSignal | null) => Promise<Response>,
): Promise<void> {
  const timers = await import("@auction/web-shared/platform/timer");
  const base = import.meta.dir;
  const [shell, paging, editor] = await Promise.all([
    loadRenderModule<typeof import("../auth/shell")>(
      `${base}/../auth/shell.tsx`,
      "AdminSessionHostContext",
    ),
    loadRenderModule<typeof import("../pagination")>(
      `${base}/../pagination/index.tsx`,
      "useAdminPagination",
    ),
    loadRenderModule<typeof import("./editor")>(`${base}/editor.tsx`, "useDraftPage"),
  ]);
  const harness = createAdminRequestHarness();
  const replies: (Response | "timeout")[] = [];
  const writes: CapturedWrite[] = [];
  const unexpected: string[] = [];
  const cleanups = [harness.cleanup];
  let expireTransport: (() => void) | undefined;
  let dispose = (): void => {};
  let renderError: unknown;
  const checkRender = () => {
    if (renderError !== undefined) throw renderError;
  };
  try {
    const originalSleep = timers.sleep;
    const sleep = spyOn(timers, "sleep");
    cleanups.push(() => sleep.mockRestore());
    // 聚焦等待一轮微任务，配合 settleEditor 推进渲染，无需等待真实的零延时计时器。
    sleep.mockImplementation((milliseconds, signal) =>
      milliseconds === 0
        ? Promise.resolve().then(() => signal?.aborted !== true)
        : originalSleep(milliseconds, signal),
    );
    cleanups.push(installAdminTranslation());
    const document = installTestDocument();
    cleanups.push(document.restore);
    installDialogAncestors(cleanups);
    const window = installFakeWindow("/?page_size=50");
    cleanups.push(window.restore);
    harness.installBrowserRequest();
    const { host } = harness.registerHost();
    cleanups.push(
      installFixtureFetch(async (input, init) => {
        const { method, url, body } = recordRequest(input, init);
        const target = new URL(url, "https://auction.test");
        const path = target.pathname + target.search;
        if (target.origin !== "https://auction.test") {
          unexpected.push(`${method} ${target.href}`);
          throw new Error("编辑测试只能使用同源内存请求");
        }
        if (path === SESSION_PATH && method === "GET") {
          return Response.json(TEST_SESSION_BODY);
        }
        if (
          target.pathname === "/api/v1/browser-errors" &&
          target.searchParams.get("class") === "dto_decode" &&
          method === "POST"
        ) {
          return new Response(null, { status: 204 });
        }
        if (read !== undefined && method === "GET") {
          return read(path, init?.signal ?? null);
        }
        writes.push({ method, path, body });
        const reply = replies.shift();
        if (reply === undefined) {
          unexpected.push(`${method} ${path}`);
          throw new Error(`请求没有脚本回执：${method} ${path}`);
        }
        if (reply === "timeout") {
          const signal = present(init?.signal, "传输取消信号");
          const pending = new Promise<Response>((_resolve, reject) => {
            signal.addEventListener("abort", () => reject(new Error("传输已取消")), { once: true });
          });
          present(expireTransport, "测试超时计时器")();
          return pending;
        }
        return reply;
      }),
    );
    const mount = () => {
      const reload = createCommandReload(0n);
      const owner = usePendingCommands(reload, () => true);
      const scope: EditorScope = {
        owner,
        host,
        reload,
        useRead: (name, editing, fetch) => {
          const pagination = paging.useAdminPagination(name, ["page_size"], 100, 100_000);
          const [read, page] = editor.useDraftPage(pagination, reload, editing, () => null, fetch);
          return {
            read,
            snapshot: () => {
              const current = page();
              return current?.ok === true ? current.value : null;
            },
          };
        },
      };
      return createComponent(PendingCommandsContext, {
        value: owner,
        get children() {
          return render(scope);
        },
      });
    };
    createRoot((disposeRoot) => {
      dispose = disposeRoot;
      const pendingHost = usePendingCommandHost(host);
      const router = createRouter({ routes: [{ name: "Fixture", pattern: "/" }] });
      mountNode(
        document,
        createComponent(Errored, {
          fallback: (error) => {
            renderError = error;
            return null;
          },
          get children() {
            return createComponent(RouterContext, {
              value: router,
              get children() {
                return createComponent(shell.AdminSessionHostContext, {
                  value: host,
                  get children() {
                    return createComponent(PendingCommandHostContext, {
                      value: pendingHost,
                      get children() {
                        return createComponent(LocaleSessionContext, {
                          value: adminLocaleSession,
                          get children() {
                            return mount();
                          },
                        });
                      },
                    });
                  },
                });
              },
            });
          },
        }),
      );
    });
    await settleEditor();
    checkRender();
    await run({
      find: (predicate) => inside(document.roots, predicate),
      textNodes: (root) => textNodes(root ?? document.roots),
      button: (label) => namedButton(document.roots, label),
      async click(label) {
        const button = present(namedButton(document.roots, label), `按钮「${label}」`);
        expect(button.hasAttribute("disabled")).toBe(false);
        expect(typeof button.attributes.onClick).toBe("function");
        button.click();
        await settleEditor();
      },
      field: (id) => present(document.getElementById(id), `控件 ${id}`),
      form: () =>
        present(
          inside(
            document.roots,
            (node) => node.tag === "form" && typeof node.attributes.onSubmit === "function",
          )[0],
          "编辑表单",
        ),
      text: () => document.roots.map((element) => element.textContent).join(""),
      hasText: (text) => textNodes(document.roots).includes(text),
      retry: (id) => {
        const entry = inside(
          document.roots,
          (node) => node.getAttribute("data-command-id") === id,
        )[0];
        return entry === undefined ? undefined : namedButton(entry, "原样重试");
      },
      reply: (status, body) => replies.push(new Response(body, { status })),
      conflict: (code) =>
        replies.push(
          Response.json(
            { code, message: "版本已变化", data: null, request_id: "review-fixture" },
            { status: 409 },
          ),
        ),
      replyTimeout() {
        const original = timers.createTimeoutFuture;
        const timeout = spyOn(timers, "createTimeoutFuture").mockImplementation((milliseconds) => {
          if (milliseconds !== DEFAULT_REQUEST_TIMEOUT_MILLIS) return original(milliseconds);
          const pending = Promise.withResolvers<void>();
          expireTransport = pending.resolve;
          return { promise: pending.promise, cancel() {} };
        });
        cleanups.push(() => timeout.mockRestore());
        replies.push("timeout");
      },
      takeWrites: () => writes.splice(0),
    });
    checkRender();
    expect(unexpected).toEqual([]);
    expect(replies).toEqual([]);
  } finally {
    try {
      dispose();
      flush();
    } finally {
      cleanupAll(cleanups);
    }
  }
}

/** 排期和改价测试共享的控件状态断言。 */
export function assertField(page: EditorTest, id: string, value: string, readOnly: boolean): void {
  const field = page.field(id);
  expect(field.value).toBe(value);
  expect(field.hasAttribute("readonly")).toBe(readOnly);
  expect(field.hasAttribute("disabled")).toBe(false);
  if (readOnly && field.hasAttribute("type")) {
    expect(field.getAttribute("type")).toBe("text");
  }
}

/** 编辑表单仍挂载时，核对原提交文案和可提交控件已经移除。 */
export function assertEditorSubmitRemoved(page: EditorTest, label: string): void {
  expect(page.hasText(label)).toBe(false);
  const submitControls = page
    .form()
    .querySelectorAll("button, input")
    .filter((node) => {
      const type = node.getAttribute("type")?.toLowerCase();
      return node.tag === "button"
        ? type !== "button" && type !== "reset"
        : type === "submit" || type === "image";
    });
  expect(submitControls).toHaveLength(0);
}

/** 录入真实 input 事件，然后等待该次更新反映在同一挂载中。 */
export async function editField(field: TestElement, value: string): Promise<void> {
  expect(typeof field.attributes.onInput).toBe("function");
  field.input(value);
  await settleEditor();
}

/** 按 Rust 夹具的语义直接给 form 派发 submit，也可验证从 DOM 移除提交按钮后的防护。 */
export async function submitForm(form: TestElement): Promise<void> {
  expect(typeof form.attributes.onSubmit).toBe("function");
  form.submit();
  await settleEditor();
}

/** 获取本步骤的唯一业务请求。 */
export function takeWrite(page: EditorTest): CapturedWrite {
  const writes = page.takeWrites();
  expect(writes).toHaveLength(1);
  return present(writes[0], "唯一的业务请求");
}

test("编辑器读取夹具按顺序交付回执并拒绝没有在途请求的回执", async () => {
  const reads = recordRead<string>();
  const first = reads.fetch();
  const second = reads.fetch();
  const success = { ok: true, value: "first" } as const;
  const failed = { ok: false, error: "storage_unavailable" } as const;
  expect(reads.started()).toBe(2);
  await reads.respond(success);
  expect(await first).toEqual(success);
  await reads.respond(failed);
  expect(await second).toEqual(failed);
  await expect(reads.respond(success)).rejects.toThrow("缺少已开始的读取");
});
