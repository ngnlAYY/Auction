// 未决命令跨导航：外壳回执、重试字节、放弃确认、门禁与一次性生成 Key。

import { expect, test } from "bun:test";
import { err, ok, type Result } from "@auction/web-shared/result";
import { createComponent, createRoot, flush } from "solid-js";
import { adminApiFailureFromHttpBody } from "../../api/failure";
import type { PreparedJsonCommand } from "../../api/transport";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { cleanupScope } from "../../test-scope-fixture";
import { installAdminTranslation } from "../admin-locale-fixture.test";
import {
  click,
  createElement,
  findAll,
  insert,
  installTestDocument,
  type TestElement,
} from "../solid-dom-fixture";
import { settlePending } from "./pending-command-fixture";
import { createSessionHost, type SessionHostFixture } from "./session-fixture";
import type { SessionPageRequests } from "./session-page-fixture";
import { type CommandSource, liveSource } from "./sources";

const { SessionPage } = await loadRenderModule<typeof import("./session-page-fixture")>(
  new URL("session-page-fixture.tsx", import.meta.url).pathname,
  "SessionPage",
);
const SOURCE_ENTITY = "alert:fixture";
const SUBMIT = "发送测试命令";
const NAVIGATE = "切换测试路由";
function failure(status: number, code: string): string {
  return adminApiFailureFromHttpBody(
    status,
    JSON.stringify({ code, message: "请求失败", data: null, request_id: null }),
  ).code;
}

function deferred<T>(): {
  readonly promise: Promise<T>;
  resolve(value: T): void;
} {
  let resolve = (_value: T): void => {};
  const promise = new Promise<T>((settle) => {
    resolve = settle;
  });
  return { promise, resolve };
}

interface MountOptions {
  readonly delayedWrite?: boolean;
  readonly delayedKey?: boolean;
  readonly replies: readonly Result<void, string>[];
  readonly gatedDestination?: boolean;
}

interface MountedSession {
  readonly host: SessionHostFixture;
  readonly tree: TestElement;
  readonly writes: string[];
  readonly submissions: readonly (readonly [string, boolean])[];
  readonly keySourceAlive: readonly boolean[];
  delivered(): number;
  source(): CommandSource;
  completeWrite(value: Result<void, string>): void;
  completeKey(value: Result<string, string>): void;
  settle(): Promise<void>;
  dispose(): void;
}

function mountSession(options: MountOptions): MountedSession {
  const scope = cleanupScope();
  try {
    scope.defer(installAdminTranslation());
    scope.defer(flush);
    const host = createSessionHost();
    scope.defer(host.dispose);
    const document = installTestDocument();
    scope.defer(() => document.restore());
    const tree = createElement("div");
    const writes: string[] = [];
    const submissions: [string, boolean][] = [];
    const keySourceAlive: boolean[] = [];
    const replies = [...options.replies];
    const delayedWrite = options.delayedWrite === true ? deferred<Result<void, string>>() : null;
    const delayedKey = options.delayedKey === true ? deferred<Result<string, string>>() : null;
    let delivered = 0;
    let keyInFlight = false;

    const requests: SessionPageRequests = {
      async submit(prepared: PreparedJsonCommand) {
        writes.push(prepared.body);
        if (delayedWrite !== null && writes.length === 1) {
          return delayedWrite.promise;
        }
        const reply = replies.shift();
        if (reply === undefined) throw new Error("发送应有对应的测试回执");
        return reply;
      },
      submitGenerated:
        delayedKey === null
          ? undefined
          : () => {
              keyInFlight = true;
              return delayedKey.promise;
            },
      submitted(entity, accepted) {
        submissions.push([entity, accepted]);
      },
      delivered() {
        delivered += 1;
      },
      keyDelivered(sourceAlive) {
        keySourceAlive.push(sourceAlive);
      },
    };
    createRoot((disposeRoot) => {
      scope.defer(disposeRoot);
      insert(
        tree,
        createComponent(SessionPage, {
          host: host.host,
          requests,
          gatedDestination: options.gatedDestination === true,
        }),
      );
    });
    document.add(tree);
    flush();
    findAll(tree, () => false);

    return {
      host,
      tree,
      writes,
      submissions,
      keySourceAlive,
      delivered: () => delivered,
      source() {
        const source = host.host.sources.peek().at(-1)?.deref();
        if (source === undefined) throw new Error("来源页未登记");
        return source;
      },
      completeWrite(value) {
        if (delayedWrite === null) throw new Error("本次会话没有在途写入");
        delayedWrite.resolve(value);
      },
      completeKey(value) {
        if (delayedKey === null || !keyInFlight) throw new Error("本次会话没有在途 Key 请求");
        keyInFlight = false;
        delayedKey.resolve(value);
      },
      async settle() {
        await settlePending();
        flush();
        findAll(tree, () => false);
        flush();
      },
      dispose: scope.dispose,
    };
  } catch (error) {
    scope.dispose();
    throw error;
  }
}

function buttonByText(tree: TestElement, text: string): TestElement {
  const button = findAll(tree, (element) => element.matches("button")).find(
    (element) => element.textContent.trim() === text,
  );
  if (button === undefined) throw new Error(`缺少按钮 ${text}`);
  return button;
}

function hasText(tree: TestElement, text: string): boolean {
  return tree.textContent.includes(text);
}

function expectNoKeyInAttributes(
  tree: TestElement,
  key: string,
  dialog: TestElement | null = null,
): void {
  const dialogNodes = new Set(dialog === null ? [] : findAll(dialog, () => true));
  const leakingAttributes: string[] = [];
  for (const element of findAll(tree, (node) => !dialogNodes.has(node))) {
    for (const [name, value] of Object.entries(element.attributes)) {
      if (typeof value === "string" && value.includes(key)) {
        leakingAttributes.push(`${element.tag}[${name}]`);
      }
    }
  }
  expect(leakingAttributes).toEqual([]);
}

function expectKeyHidden(tree: TestElement, key: string): void {
  expect(tree.textContent).not.toContain(key);
  expectNoKeyInAttributes(tree, key);
}

function expectOneTimeKeyDialog(tree: TestElement, key: string): void {
  const dialog = tree.querySelector('[role="dialog"]');
  if (dialog === null) throw new Error("缺少一次性 Key 对话框");
  expect(dialog.querySelectorAll("code").map((code) => code.textContent)).toEqual([key]);
  // 对话框内的 code 已占用唯一一次明文，整棵树不能再出现一份。
  expect(tree.textContent.split(key)).toHaveLength(2);
  expectNoKeyInAttributes(tree, key, dialog);
}

function press(tree: TestElement, text: string): TestElement {
  const button = buttonByText(tree, text);
  click(button);
  flush();
  findAll(tree, () => false);
  flush();
  return button;
}

test("在途命令跨导航后由外壳用相同字节重试", async () => {
  const page = mountSession({ delayedWrite: true, replies: [ok(undefined)] });
  try {
    press(page.tree, SUBMIT);
    await page.settle();
    expect(page.writes).toHaveLength(1);
    expect(page.submissions).toEqual([[SOURCE_ENTITY, true]]);
    const source = page.source();
    expect(buttonByTextOrNull(page.tree, "原样重试")).toBeNull();

    press(page.tree, NAVIGATE);
    expect(buttonByTextOrNull(page.tree, SUBMIT)).toBeNull();
    expect(
      page.host.host.entries.peek().find((entry) => entry.identity.entity === SOURCE_ENTITY)?.busy,
    ).toBe(true);
    expect(page.host.host.sources.peek()).toEqual([]);
    expect(liveSource(page.host.host, new WeakRef(source))).toBeNull();

    page.completeWrite(err(failure(503, "storage_unavailable")));
    await page.settle();
    expect(hasText(page.tree, "结果未知的操作")).toBe(true);
    expect(buttonByTextOrNull(page.tree, "原样重试")).not.toBeNull();

    press(page.tree, "原样重试");
    await page.settle();
    expect(buttonByTextOrNull(page.tree, "原样重试")).toBeNull();
    expect(hasText(page.tree, "已成功。")).toBe(true);
    expect(page.host.host.entries.peek()).toEqual([]);
    expect(page.host.host.notices.peek()).toHaveLength(1);
    expect(page.delivered()).toBe(0);
    expect(page.writes).toHaveLength(2);
    expect(page.writes[1]).toBe(page.writes[0]);

    press(page.tree, "关闭回执");
    expect(page.host.host.notices.peek()).toEqual([]);
  } finally {
    page.dispose();
  }
});

test("放弃未知命令不会再次发送", async () => {
  const page = mountSession({ replies: [err(failure(503, "storage_unavailable"))] });
  try {
    press(page.tree, SUBMIT);
    await page.settle();
    press(page.tree, NAVIGATE);

    press(page.tree, "放弃");
    expect(hasText(page.tree, "可能已经生效")).toBe(true);
    expect(page.writes).toHaveLength(1);
    press(page.tree, "保留未决操作");
    expect(buttonByTextOrNull(page.tree, "原样重试")).not.toBeNull();
    expect(buttonByTextOrNull(page.tree, "确认放弃")).toBeNull();

    press(page.tree, "放弃");
    press(page.tree, "确认放弃");
    expect(buttonByTextOrNull(page.tree, "原样重试")).toBeNull();
    expect(page.host.host.entries.peek()).toEqual([]);
    expect(page.writes).toHaveLength(1);

    press(page.tree, NAVIGATE);
    const submit = buttonByText(page.tree, SUBMIT);
    expect(submit.disabled).toBe(false);
  } finally {
    page.dispose();
  }
});

test("已了结命令刷新仍挂载的来源页且不投递外壳回执", async () => {
  const page = mountSession({ replies: [ok(undefined), err(failure(409, "alert_stale"))] });
  try {
    const source = page.source();
    press(page.tree, SUBMIT);
    await page.settle();
    expect(hasText(page.tree, "来源页收到回执")).toBe(true);
    expect(source.reload.peek()).toBe(1n);
    expect(page.host.host.entries.peek()).toEqual([]);
    expect(page.host.host.notices.peek()).toEqual([]);

    press(page.tree, SUBMIT);
    await page.settle();
    expect(source.reload.peek()).toBe(2n);
    expect(page.host.host.entries.peek()).toEqual([]);
    expect(page.host.host.notices.peek()).toEqual([]);
    expect(buttonByTextOrNull(page.tree, "关闭已拒绝操作")).not.toBeNull();
    expect(buttonByTextOrNull(page.tree, "原样重试")).toBeNull();
    expect(page.delivered()).toBe(1);
  } finally {
    page.dispose();
  }
});

test("已卸载命令既不占用也不释放另一页的一次性结果门禁", async () => {
  const page = mountSession({ delayedWrite: true, replies: [], gatedDestination: true });
  try {
    press(page.tree, SUBMIT);
    press(page.tree, NAVIGATE);
    press(page.tree, "交付一次性结果");
    expect(page.submissions).toEqual([
      [SOURCE_ENTITY, true],
      ["world:fixture", true],
    ]);
    expect(buttonByText(page.tree, "交付一次性结果").disabled).toBe(true);

    page.completeWrite(ok(undefined));
    await page.settle();
    expect(buttonByText(page.tree, "交付一次性结果").disabled).toBe(true);
    press(page.tree, "确认已查看");
    expect(buttonByText(page.tree, "交付一次性结果").disabled).toBe(false);
    expect(page.delivered()).toBe(0);
  } finally {
    page.dispose();
  }
});

test("来源卸载后外壳立即接管未知命令的重试与生成 Key", async () => {
  const key = "已生成的测试 Key";
  const page = mountSession({
    replies: [err(failure(503, "storage_unavailable")), ok(undefined)],
  });
  try {
    press(page.tree, SUBMIT);
    await page.settle();
    expectKeyHidden(page.tree, key);
    press(page.tree, "生成待确认 Key");
    await page.settle();
    expect(page.submissions).toEqual([
      [SOURCE_ENTITY, true],
      ["user:fixture", true],
    ]);
    expect(page.keySourceAlive).toEqual([true]);
    expectOneTimeKeyDialog(page.tree, key);
    expect(buttonByTextOrNull(page.tree, "查看生成的 Key")).toBeNull();
    expect(buttonByText(page.tree, "原样重试").disabled).toBe(true);

    press(page.tree, NAVIGATE);
    expect(hasText(page.tree, "只读页面")).toBe(true);
    expect(buttonByText(page.tree, "原样重试").disabled).toBe(false);
    expect(page.host.host.sources.peek()).toEqual([]);
    expectKeyHidden(page.tree, key);
    expect(buttonByTextOrNull(page.tree, "查看生成的 Key")).not.toBeNull();

    press(page.tree, "原样重试");
    await page.settle();
    expectKeyHidden(page.tree, key);
    press(page.tree, "查看生成的 Key");
    expectOneTimeKeyDialog(page.tree, key);
    press(page.tree, "关闭");
    expect(buttonByTextOrNull(page.tree, "查看生成的 Key")).toBeNull();
    expectKeyHidden(page.tree, key);
    expect(page.writes).toHaveLength(2);
    expect(page.writes[1]).toBe(page.writes[0]);
  } finally {
    page.dispose();
  }
});

test("已卸载来源的生成 Key 只在一次性对话框中展示", async () => {
  const key = "仅供测试的生成 Key";
  const page = mountSession({ delayedKey: true, replies: [] });
  try {
    expect(page.source().gate).toBeNull();
    press(page.tree, SUBMIT);
    expect(page.submissions).toEqual([["create-user", true]]);
    await page.settle();
    press(page.tree, NAVIGATE);
    expect(page.host.host.sources.peek()).toEqual([]);
    page.completeKey(ok(key));
    await page.settle();
    expect(page.keySourceAlive).toEqual([false]);

    expectKeyHidden(page.tree, key);
    expect(buttonByTextOrNull(page.tree, "查看生成的 Key")).not.toBeNull();
    press(page.tree, "查看生成的 Key");
    expectOneTimeKeyDialog(page.tree, key);
    press(page.tree, "关闭");
    expectKeyHidden(page.tree, key);
    expect(buttonByTextOrNull(page.tree, "查看生成的 Key")).toBeNull();
  } finally {
    page.dispose();
  }
});

function buttonByTextOrNull(tree: TestElement, text: string): TestElement | null {
  return (
    findAll(tree, (element) => element.matches("button")).find(
      (element) => element.textContent.trim() === text,
    ) ?? null
  );
}
