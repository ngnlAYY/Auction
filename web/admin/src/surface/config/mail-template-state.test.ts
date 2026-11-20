// 邮件模板卡片、草稿修订围栏及未决命令冻结重试。

import { afterEach, beforeEach, expect, jest, spyOn, test } from "bun:test";
import type { AdminMailTemplateDto } from "@auction/contracts/dto/admin/mail-templates";
import { parseJson } from "@auction/web-shared/net/wire";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { ok } from "@auction/web-shared/result";
import { adminWriteFailureDisplayCode, observedResponseStatus } from "../../api/failure";
import { cleanupScope } from "../../test-scope-fixture";
import { required } from "../solid-dom-fixture";
import {
  clickButton,
  configPage,
  getProvidedHost,
  maybeButton,
  networkFailure,
  noticeLine,
  settle,
} from "./config-page-fixture.test";
import { accepted, apiFailure, jsonObject } from "./config-projection-fixture.test";
import { loadConfigFixture } from "./config-render-fixture.test";
import { beginConfigTestLock, endConfigTestLock } from "./config-test-scope-fixture.test";
import { type TemplateListRead, useTemplateEditState } from "./mail-template-state";

const KEY = "auction_no_attachments";
const ENTITY = "mail-template:auction_no_attachments";
const UNAVAILABLE = "所需服务暂时不可用。请稍后再试。";
const SAVED = "数据库已接受；各角色约 5 秒内生效。";
const SAVED_BANNER = `拍卖邮件：没有附件 · ${SAVED}`;
const COMMAND_ID_V4 = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu;
const fixture = await loadConfigFixture();

function published(revision: number): AdminMailTemplateDto {
  return {
    key: KEY,
    customized: true,
    usable: true,
    body: `服务器正文 ${revision}`,
    default_body: "默认正文",
    revision,
    updated_by: "admin",
    updated_at: null,
    allowed_placeholders: [],
    required_placeholders: [],
  };
}

function wireTemplate(revision: number, body = `服务器正文 ${revision}`) {
  // Rust 的替身直接返回模型。真实 HTTP 解码要求已修订项有时间，写回执正文也须与命令一致。
  return { ...published(revision), body, updated_at: "2026-11-19T00:00:00Z" };
}

function list(revision: number): Response {
  return Response.json({ code: "ok", message: "", data: { templates: [wireTemplate(revision)] } });
}

function saved(revision: number, body: string): Response {
  return Response.json({
    code: "ok",
    message: "",
    data: { template: wireTemplate(revision, body) },
  });
}

function editState(page: ReturnType<typeof configPage>) {
  const latest = createCell(published(5));
  const refresh = createCell(0n);
  const states = createCell<ReturnType<typeof useTemplateEditState> | null>(null);
  page.mount(() => {
    const { admin } = getProvidedHost();
    const resource = createTaskResource(
      () => [refresh(), latest()] as const,
      async ([generation, template]): Promise<TemplateListRead> => [generation, ok([template])],
    );
    const state = useTemplateEditState(admin, resource, refresh);
    // 此时仅把新状态交给挂载者，状态仍由当前编辑器作用域销毁。
    states.set(state);
    return null;
  });
  return { latest, state: required(states.peek(), "邮件模板编辑器") };
}

function draftAt(
  state: ReturnType<typeof useTemplateEditState>,
  revision: number,
  body: string,
): void {
  state.drafts.set(new Map([[KEY, { revision, body }]]));
}

function submitCard(page: ReturnType<typeof configPage>, value: string): void {
  const form = required(page.root.querySelector("form"), "邮件模板表单");
  const field = required(form.querySelector("textarea"), "模板正文");
  expect(field.value).toBe("服务器正文 5");
  field.input(value);
  expect(form.submit()).toBe(true);
}

beforeEach(beginConfigTestLock);
afterEach(endConfigTestLock);

test("刷新后的列表不能改写仍在编辑的草稿版本", async () => {
  const page = configPage(() => {
    throw new Error("草稿测试不应发送请求");
  });
  try {
    const { latest, state } = editState(page);
    await settle();
    draftAt(state, 5, "管理员的草稿");
    expect(state.beginSave(published(5))).toBeNull();
    expect(state.confirmation.peek()).toEqual({
      key: KEY,
      expectedRevision: 5,
      body: "管理员的草稿",
    });

    state.confirmation.set(null);
    for (const revision of [6, 0]) {
      latest.set(published(revision));
      await settle();
      expect(state.listReady()).toBe(true);
      expect(state.currentRevision(KEY)).toBe(revision);
      expect(state.canEdit(KEY, revision)).toBe(true);
      expect(state.beginSave(published(revision))).toBeNull();
      expect(state.confirmation.peek()).toBeNull();
      expect(state.drafts.peek().get(KEY)).toEqual({ revision: 5, body: "管理员的草稿" });
    }

    latest.set(published(6));
    await settle();
    draftAt(state, 6, "最新版本");
    expect(state.beginSave(published(6))).toBeNull();
    expect(state.confirmation.peek()).toEqual({ key: KEY, expectedRevision: 6, body: "最新版本" });
    expect(page.requests).toEqual([]);
  } finally {
    page.dispose();
  }
});

test("版本冲突在刷新前就阻止保存", async () => {
  const page = configPage(() => {
    throw new Error("草稿测试不应发送请求");
  });
  try {
    const { state } = editState(page);
    await settle();
    const template = published(5);
    expect(state.currentRevision(KEY)).toBe(5);
    expect(state.listReady()).toBe(true);
    draftAt(state, 5, "仍未保存的草稿");
    state.conflicts.set(new Set([KEY]));
    expect(state.beginSave(template)).toBeNull();
    expect(state.confirmation.peek()).toBeNull();

    state.conflicts.set(new Set());
    expect(state.beginSave(template)).toBeNull();
    expect(state.confirmation.peek()).toEqual({
      key: KEY,
      expectedRevision: 5,
      body: "仍未保存的草稿",
    });
    expect(page.requests).toEqual([]);
  } finally {
    page.dispose();
  }
});

test("首次保存模板仅在完整 413 后释放实体", async () => {
  for (const [reply, pending] of [
    [apiFailure(413, "body_too_large", "请求体过大"), false],
    [
      new Response('{"code":"body_too_large","message":"请求体过大","data":null}', { status: 413 }),
      true,
    ],
  ] as const) {
    const page = configPage((request) => {
      expect([request.method, request.path]).toEqual([
        "PUT",
        `/api/v1/admin/mail-templates/${KEY}`,
      ]);
      return reply;
    });
    try {
      const { state } = editState(page);
      await settle();
      draftAt(state, 5, "保留草稿");
      expect(state.beginSave(published(5))).toBeNull();
      state.submit(required(state.confirmation.peek(), "确认保存模板"));
      await settle();
      expect(page.requests).toHaveLength(1);
      expect(JSON.parse(required(page.requests[0], "模板写入").body)).toEqual({
        command_id: expect.stringMatching(COMMAND_ID_V4),
        expected_revision: 5,
        body: "保留草稿",
      });
      expect(state.pending.contains(ENTITY)).toBe(pending);
      expect(state.drafts.peek().get(KEY)?.body).toBe("保留草稿");
      expect(state.accepted.peek()).toBeNull();
    } finally {
      page.dispose();
    }
  }
});

test("模板重试发送冻结的修订直到账本核对冲突后才允许重新加载", async () => {
  const conflicts = [
    '{"code":"mail_template_conflict","message":"版本已变化","data":null}',
    '{"code":"mail_template_conflict","message":"版本已变化","data":null,"request_id":7}',
    '{"code":"mail_template_conflict","message":"版本已变化","data":null,"request_id":null,"extra":true}',
    '{"code":"mail_template_conflict","message":"版本已变化","data":null,"request_id":null}',
  ];
  let timedOut = false;
  const scope = cleanupScope();
  const page = configPage((request, init, index) => {
    expect(request.method).toBe("PUT");
    expect(request.path).toBe(`/api/v1/admin/mail-templates/${KEY}`);
    if (index === 0) {
      const signal = required(init?.signal, "首个请求的超时信号");
      return new Promise((_resolve, reject) => {
        signal.addEventListener(
          "abort",
          () => {
            timedOut = true;
            reject(new DOMException("请求超时", "AbortError"));
          },
          { once: true },
        );
      });
    }
    if (index === 1) {
      return apiFailure(401, "unauthenticated", "未认证");
    }
    if (index === 2) {
      return apiFailure(429, "rate_limited", "请求过多");
    }
    if (index === 3) {
      return networkFailure();
    }
    if (index === 4) {
      return apiFailure(413, "body_too_large", "请求体过大");
    }
    if (index === 5) {
      return new Response('{"code":"body_too_large","message":"请求体过大","data":null}', {
        status: 413,
      });
    }
    if (index === 10) {
      return saved(7, "以最新版本编辑");
    }
    return new Response(required(conflicts[index - 6], "冲突回执"), { status: 409 });
  });
  scope.defer(page.dispose);
  try {
    const mailApi = await import("../../api/notification-templates");
    const submit = mailApi.submitMailTemplate;
    const outcomes: [number | null, string][] = [];
    const observe = spyOn(mailApi, "submitMailTemplate");
    scope.defer(() => observe.mockRestore());
    observe.mockImplementation(async (...args) => {
      const result = await submit(...args);
      outcomes.push(
        result.ok
          ? [null, "accepted"]
          : [observedResponseStatus(result.error), adminWriteFailureDisplayCode(result.error)],
      );
      return result;
    });
    const { latest, state } = editState(page);
    await settle();
    const original = published(5);
    draftAt(state, 5, '"'.repeat(50_000));
    expect(state.beginSave(original)).toBe("request_too_large");
    expect(state.confirmation.peek()).toBeNull();
    expect(page.requests).toEqual([]);

    draftAt(state, 5, "保留草稿");
    expect(state.beginSave(original)).toBeNull();
    jest.useFakeTimers();
    try {
      state.submit(required(state.confirmation.peek(), "首次确认"));
      await settle();
      expect(page.requests).toHaveLength(1);
      expect(timedOut).toBe(false);
      jest.advanceTimersByTime(10_000);
      await settle();
      expect(timedOut).toBe(true);
      expect(state.pending.contains(ENTITY)).toBe(true);
    } finally {
      jest.useRealTimers();
    }
    const firstBytes = required(page.requests[0], "首次请求").body;
    const first = jsonObject(accepted(parseJson(firstBytes)));
    const { command_id: commandId } = first;
    if (typeof commandId !== "string") {
      throw new Error("冻结请求缺少命令 ID");
    }
    expect(commandId).toMatch(COMMAND_ID_V4);
    expect(first.expected_revision).toBe(5);
    expect(first.body).toBe("保留草稿");

    latest.set(published(6));
    await settle();
    expect(state.currentRevision(KEY)).toBe(6);
    for (let count = 2; count <= 10; count += 1) {
      page.pending.host.retry(commandId);
      await settle();
      expect(page.requests).toHaveLength(count);
      expect(required(page.requests[count - 1], "重试请求").body).toBe(firstBytes);
      if (count < 10) {
        expect(state.pending.contains(ENTITY)).toBe(true);
        expect(state.conflicts.peek().has(KEY)).toBe(false);
        state.discard(KEY, true);
        expect(state.drafts.peek().get(KEY)?.body).toBe("保留草稿");
      }
    }
    expect(state.pending.contains(ENTITY)).toBe(false);
    expect(state.conflicts.peek().has(KEY)).toBe(true);
    expect(state.drafts.peek().get(KEY)?.body).toBe("保留草稿");

    state.discard(KEY, true);
    await settle();
    expect(state.drafts.peek().has(KEY)).toBe(false);
    expect(state.conflicts.peek().has(KEY)).toBe(false);
    expect(state.currentRevision(KEY)).toBe(6);
    draftAt(state, 6, "以最新版本编辑");
    expect(state.beginSave(published(6))).toBeNull();
    state.submit(required(state.confirmation.peek(), "重新加载后的确认"));
    await settle();
    expect(page.requests).toHaveLength(11);
    const next = jsonObject(accepted(parseJson(required(page.requests[10], "第十一次请求").body)));
    expect(next.expected_revision).toBe(6);
    expect(next.body).toBe("以最新版本编辑");
    expect(next.command_id).not.toBe(first.command_id);
    expect(outcomes).toEqual([
      [null, "request_timeout"],
      [401, "unauthenticated"],
      [429, "rate_limited"],
      [null, "network_error"],
      [413, "body_too_large"],
      [413, "http_413"],
      [409, "http_409"],
      [409, "http_409"],
      [409, "http_409"],
      [409, "mail_template_conflict"],
      [null, "accepted"],
    ]);
    expect(state.accepted.peek()).toBe(KEY);
    expect(state.drafts.peek().has(KEY)).toBe(false);
    expect(state.pending.contains(ENTITY)).toBe(false);
  } finally {
    scope.dispose();
  }
});

test("保存回执在随后的列表读取失败后仍然保留", async () => {
  const page = configPage((request, _init, index) => {
    if (index === 1) {
      expect(request.method).toBe("PUT");
      expect(request.path).toBe(`/api/v1/admin/mail-templates/${KEY}`);
      expect(jsonObject(accepted(parseJson(request.body)))).toMatchObject({
        expected_revision: 5,
        body: "已更新正文",
      });
      return saved(6, "已更新正文");
    }
    expect(request.method).toBe("GET");
    expect(request.path).toBe("/api/v1/admin/mail-templates");
    return index === 2
      ? apiFailure(503, "storage_unavailable", "暂不可用")
      : list(index === 0 ? 5 : 6);
  });
  try {
    page.mount(() => fixture.createComponent(fixture.mailTemplates.MailTemplateCard, {}));
    await settle();
    expect(page.root.textContent).not.toContain(SAVED);
    submitCard(page, "已更新正文");
    await settle();
    clickButton(page.root, "确认");
    await settle();
    expect(noticeLine(page.root, "alert", ".ui-async-state__title", UNAVAILABLE)).not.toBeNull();
    expect(noticeLine(page.root, "status", ".admin-shell__banner", SAVED_BANNER)).not.toBeNull();
    clickButton(page.root, "刷新");
    await settle();
    expect(page.root.textContent).not.toContain(UNAVAILABLE);
    expect(noticeLine(page.root, "status", ".admin-shell__banner", SAVED_BANNER)).not.toBeNull();
    expect(page.requests.map(({ method }) => method)).toEqual(["GET", "PUT", "GET", "GET"]);
  } finally {
    page.dispose();
  }
});

test("离开再返回的模板不能替换结果未知的命令", async () => {
  const page = configPage((request) => {
    if (request.method === "GET") {
      expect(request.path).toBe("/api/v1/admin/mail-templates");
      return list(5);
    }
    expect(request.method).toBe("PUT");
    expect(request.path).toBe(`/api/v1/admin/mail-templates/${KEY}`);
    return apiFailure(503, "storage_unavailable", "暂不可用");
  });
  const mount = () =>
    page.mount(() => fixture.createComponent(fixture.mailTemplates.MailTemplateCard, {}));
  try {
    page.mount(() =>
      fixture.createComponent(fixture.pendingPanel.PendingCommandNotices, {
        host: page.pending.host,
      }),
    );
    const leave = mount();
    await settle();
    submitCard(page, "第一次正文");
    await settle();
    clickButton(page.root, "确认");
    await settle();
    expect(page.requests.map(({ method }) => method)).toEqual(["GET", "PUT"]);
    const firstBytes = required(page.requests[1], "首发请求").body;
    const first = jsonObject(accepted(parseJson(firstBytes)));
    expect(first).toMatchObject({ expected_revision: 5, body: "第一次正文" });
    expect(first.command_id).toMatch(COMMAND_ID_V4);

    leave();
    expect(page.root.querySelector("form")).toBeNull();
    mount();
    await settle();
    submitCard(page, "第二次正文");
    await settle();
    expect(maybeButton(page.root, "确认")).toBeNull();
    expect(page.requests.map(({ method }) => method)).toEqual(["GET", "PUT", "GET"]);
    clickButton(page.root, "原样重试");
    await settle();
    expect(page.requests.map(({ method }) => method)).toEqual(["GET", "PUT", "GET", "PUT"]);
    expect(required(page.requests[3], "重试请求").body).toBe(firstBytes);
  } finally {
    page.dispose();
  }
});
