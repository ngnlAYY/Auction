// 外部访问暂停卡片的确认围栏、冻结重试、焦点及播报。

import { afterEach, beforeEach, expect, jest, spyOn, test } from "bun:test";
import type { RuntimeSwitchListResponse } from "@auction/contracts/dto/admin/runtime-switches";
import { parseJson } from "@auction/web-shared/net/wire";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { utf8ByteLength } from "@auction/web-shared/text";
import { flush } from "solid-js";
import { cleanupScope } from "../../test-scope-fixture";
import { captureEvent, findElement, required, type TestElement } from "../solid-dom-fixture";
import {
  button,
  clickButton,
  configPage,
  deferredReply,
  maybeButton,
  settle,
} from "./config-page-fixture.test";
import { accepted, apiFailure, jsonObject } from "./config-projection-fixture.test";
import { loadConfigFixture } from "./config-render-fixture.test";
import { beginConfigTestLock, endConfigTestLock } from "./config-test-scope-fixture.test";

const fixture = await loadConfigFixture();
const READ_PATH = "/api/v1/admin/runtime-switches";
const WRITE_PATH = "/api/v1/admin/runtime-switches/external-access-paused";
const UNRESOLVED = "曾有请求结果未知，请保留原命令，勿为同一实体新建操作。";
const CONFLICT = "数据已变化，或本次命令与已有请求冲突。请重新加载后再试。";
const INVALID_REASON = "请输入 1 至 1024 个 UTF-8 字节的单行原因，且不含控制字符。";
const TOO_LONG = "界".repeat(342);
const DB_TIME = "2026-11-19T00:00:00Z";

function intent(paused: boolean, revision: number, expiresAt: string | null) {
  return {
    name: "external_access_paused",
    revision,
    operation: paused ? "set" : "clear",
    value: paused ? true : null,
    expires_at: expiresAt,
    actor_id: "operator",
    entrypoint: "cli",
    reason: "drill",
  };
}

function snapshot(paused: boolean, revision: number): Response {
  const data: RuntimeSwitchListResponse = {
    database_time: DB_TIME,
    database_intent: [intent(paused, revision, null)],
    admin_last_success_age_seconds: 1,
    admin_applied: [],
  };
  return Response.json({ code: "ok", message: "", data });
}

function replyForAttempt(attempt: number, body: string, acceptReplay: boolean): Response {
  switch (attempt) {
    case 1:
      return apiFailure(503, "storage_unavailable", "暂不可用");
    case 2:
    case 4:
      return apiFailure(429, "rate_limited", "请求过多");
    case 3: {
      if (!acceptReplay) {
        return apiFailure(409, "runtime_switch_conflict", "版本已变化");
      }
      const request = jsonObject(accepted(parseJson(body)));
      const expiry = request.expires_at;
      if (typeof expiry !== "string") {
        throw new Error("暂停重放没有冻结期限");
      }
      return Response.json({
        code: "ok",
        message: "",
        data: { accepted: intent(true, 8, expiry) },
      });
    }
    default:
      throw new Error(`额外的暂停写入 ${attempt}`);
  }
}

function pauseCard(initialReads = 1, acceptReplay = false) {
  const refresh = deferredReply();
  let reads = 0;
  const sent: string[] = [];
  const page = configPage((request) => {
    const url = new URL(request.url);
    expect(url.search).toBe("");
    if (request.method === "GET") {
      expect(request.path).toBe(READ_PATH);
      reads += 1;
      if (reads <= initialReads) {
        return snapshot(false, 7);
      }
      if (reads === initialReads + 1) {
        return refresh.wait();
      }
      // 最新命令的 429 可能触发额外重读，脚本仍返回已加载的数据库意图。
      expect(sent).toHaveLength(4);
      expect(reads).toBe(initialReads + 2);
      return snapshot(true, 8);
    }
    expect(request.method).toBe("POST");
    expect(request.path).toBe(WRITE_PATH);
    sent.push(request.body);
    return replyForAttempt(sent.length, request.body, acceptReplay);
  });
  return {
    ...page,
    sent,
    readCount: () => reads,
    finishRefresh: (paused = true, revision = 8) => refresh.respond(snapshot(paused, revision)),
    mountCard: () =>
      page.mount(() => fixture.createComponent(fixture.runtimeSwitches.RuntimeSwitchCard, {})),
    mountNotices: () =>
      page.mount(() =>
        fixture.createComponent(fixture.pendingPanel.PendingCommandNotices, {
          host: page.pending.host,
        }),
      ),
    reason: () => required(page.document.getElementById("admin-pause-reason"), "暂停原因"),
    form: () => required(page.root.querySelector("form"), "暂停表单"),
  };
}

async function click(tree: TestElement, label: string): Promise<void> {
  clickButton(tree, label);
  await settle();
}

function openConfirmation(card: ReturnType<typeof pauseCard>, reason: string): void {
  card.reason().input(reason);
  flush();
  expect(card.form().submit()).toBe(true);
  flush();
}

function assertNewResume(sent: string[]): void {
  expect(sent).toHaveLength(4);
  const first = jsonObject(accepted(parseJson(required(sent[0], "首次暂停"))));
  const last = jsonObject(accepted(parseJson(required(sent[3], "新恢复命令"))));
  const uuid = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/iu;
  expect(first.command_id).toMatch(uuid);
  expect(last.command_id).toMatch(uuid);
  expect(last.command_id).not.toBe(first.command_id);
  expect(last.expected_revision).toBe(8);
  expect(last.paused).toBe(false);
  expect(last.expires_at).toBeNull();
}

beforeEach(beginConfigTestLock);
afterEach(endConfigTestLock);

test("an_open_pause_confirmation_rejects_clicks_until_its_read_is_current", async () => {
  const scope = cleanupScope();
  try {
    const commandSources = await import("../pending/sources");
    const cells = await import("@auction/web-shared/reactive/cell");
    const card = pauseCard();
    scope.defer(card.dispose);
    const createReload = commandSources.createCommandReload;
    const generations: Cell<bigint>[] = [];
    const reload = spyOn(commandSources, "createCommandReload");
    scope.defer(() => reload.mockRestore());
    reload.mockImplementation((initial) => {
      const cell = createReload(initial);
      generations.push(cell);
      return cell;
    });

    // 记录实际卡片的状态读取句柄，逐次核对界面未完整展示的冻结草稿。
    const readers: (() => unknown)[] = [];
    const createCell = cells.createCell;
    const observeCells = spyOn(cells, "createCell");
    try {
      observeCells.mockImplementation((initial) => {
        const cell = createCell(initial);
        readers.push(cell.peek);
        return cell;
      });
      card.mountCard();
      await settle();
    } finally {
      observeCells.mockRestore();
    }

    expect(generations).toHaveLength(1);
    const clock = spyOn(Date, "now");
    scope.defer(() => clock.mockRestore());
    clock.mockReturnValue(Date.parse(DB_TIME));
    openConfirmation(card, "演练");
    const readConfirmation = required(
      readers.find((read) => {
        const value = read();
        return value !== null && typeof value === "object" && "expectedRevision" in value;
      }),
      "暂停确认草稿",
    );
    const frozen = {
      expectedRevision: 7,
      paused: true,
      expiresAt: 1_795_053_600_000_000_000n,
      reason: "演练",
    };
    expect(readConfirmation()).toEqual(frozen);
    const oldConfirm = button(card.root, "确认");
    expect(oldConfirm.hasAttribute("disabled")).toBe(false);
    const dispatchOldConfirm = captureEvent(oldConfirm, "onClick");
    clock.mockReturnValue(Date.parse("2026-11-19T00:01:00Z"));
    required(generations[0], "卡片命令世代").set(1n);
    await settle();
    expect(card.readCount()).toBe(2);
    const disabledConfirm = button(card.root, "确认");
    expect(disabledConfirm.hasAttribute("disabled")).toBe(true);
    for (const dispatch of [dispatchOldConfirm, captureEvent(disabledConfirm, "onClick")]) {
      dispatch();
      await settle();
      expect(card.readCount()).toBe(2);
      expect(card.sent).toHaveLength(0);
      expect(readConfirmation()).toEqual(frozen);
    }

    card.finishRefresh(false, 7);
    await settle();
    expect(button(card.root, "确认")).toBe(disabledConfirm);
    await click(card.root, "确认");
    expect(card.sent).toHaveLength(1);
    const request = jsonObject(accepted(parseJson(required(card.sent[0], "首次请求"))));
    expect(request).toEqual({
      command_id: expect.stringMatching(
        /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/iu,
      ),
      expected_revision: 7,
      paused: true,
      expires_at: "2026-11-19T02:00:00Z",
      reason: "演练",
    });
  } finally {
    scope.dispose();
  }
});

test("ordinary_refresh_keeps_the_form_draft_and_validation_until_reply", async () => {
  const card = pauseCard();
  try {
    card.mountCard();
    await settle();
    const field = card.reason();
    field.input(TOO_LONG);
    flush();
    field.blur();
    flush();
    expect(utf8ByteLength(TOO_LONG)).toBe(1026);
    expect(field.getAttribute("aria-invalid")).toBe("true");
    expect(card.root.textContent).toContain(INVALID_REASON);
    await click(card.root, "刷新");
    expect(card.readCount()).toBe(2);
    expect(card.reason()).toBe(field);
    expect(field.value).toBe(TOO_LONG);
    expect(field.hasAttribute("disabled")).toBe(false);
    expect(field.hasAttribute("readonly")).toBe(false);
    expect(field.getAttribute("aria-invalid")).toBe("true");
    expect(card.root.textContent).toContain(INVALID_REASON);
    expect(button(card.root, "暂停外部访问…")).not.toBeNull();

    openConfirmation(card, "演练");
    expect(button(card.root, "确认").hasAttribute("disabled")).toBe(false);
    await click(card.root, "返回");
    expect(maybeButton(card.root, "确认")).toBeNull();
    card.finishRefresh();
    await settle();
    expect(button(card.root, "恢复外部访问…")).not.toBeNull();
    expect(card.sent).toEqual([]);
  } finally {
    card.dispose();
  }
});

test("pause_retries_verbatim_until_a_ledger_checked_conflict_then_rereads", async () => {
  const card = pauseCard();
  try {
    card.mountCard();
    await settle();
    openConfirmation(card, "演练");
    await click(card.root, "确认");
    expect(card.root.textContent).toContain(UNRESOLVED);
    await click(card.root, "原样重试");
    expect(button(card.root, "原样重试")).not.toBeNull();
    expect(card.readCount()).toBe(1);

    await click(card.root, "原样重试");
    expect(card.sent).toHaveLength(3);
    expect(card.sent[1]).toBe(card.sent[0]);
    expect(card.sent[2]).toBe(card.sent[0]);
    expect(maybeButton(card.root, "原样重试")).toBeNull();
    expect(card.root.textContent).toContain(CONFLICT);
    expect(card.readCount()).toBe(2);
    expect(button(card.root, "暂停外部访问…").hasAttribute("disabled")).toBe(true);
    expect(card.form().submit()).toBe(true);
    await settle();
    expect(maybeButton(card.root, "确认")).toBeNull();
    expect(card.sent).toHaveLength(3);

    card.finishRefresh();
    await settle();
    expect(button(card.root, "恢复外部访问…").hasAttribute("disabled")).toBe(false);
    expect(card.form().submit()).toBe(true);
    await settle();
    await click(card.root, "确认");
    assertNewResume(card.sent);
  } finally {
    card.dispose();
  }
});

test("pause_cannot_replace_unknown_after_leaving_and_returning", async () => {
  const card = pauseCard();
  try {
    card.mountNotices();
    const leave = card.mountCard();
    await settle();
    openConfirmation(card, "演练");
    await click(card.root, "确认");
    expect(card.sent).toHaveLength(1);
    leave();
    expect(card.root.querySelector("form")).toBeNull();
    card.mountCard();
    await settle();
    expect(card.readCount()).toBe(2);
    card.finishRefresh();
    await settle();
    expect(card.readCount()).toBe(2);
    openConfirmation(card, "重新输入的原因");
    expect(maybeButton(card.root, "确认")).toBeNull();
    expect(button(card.root, "原样重试")).not.toBeNull();
    expect(card.sent).toHaveLength(1);

    await click(card.root, "原样重试");
    expect(card.sent).toHaveLength(2);
    expect(card.sent[1]).toBe(card.sent[0]);
  } finally {
    card.dispose();
  }
});

test("remounted_pause_waits_for_the_read_after_the_original_command_settles", async () => {
  for (const acceptReplay of [true, false]) {
    const card = pauseCard(2, acceptReplay);
    try {
      card.mountNotices();
      const leave = card.mountCard();
      await settle();
      openConfirmation(card, "第一次暂停");
      await click(card.root, "确认");
      leave();
      expect(card.root.querySelector("form")).toBeNull();
      card.mountCard();
      await settle();
      expect(card.readCount()).toBe(2);
      expect(button(card.root, "暂停外部访问…")).not.toBeNull();
      const form = card.form();
      await click(card.root, "原样重试");
      await click(card.root, "原样重试");
      expect(maybeButton(card.root, "原样重试")).toBeNull();
      expect(card.readCount()).toBe(3);
      expect(button(card.root, "暂停外部访问…").hasAttribute("disabled")).toBe(true);
      openConfirmation(card, "恢复访问");
      expect(maybeButton(card.root, "确认")).toBeNull();
      expect(card.sent).toHaveLength(3);
      expect(card.sent[1]).toBe(card.sent[0]);
      expect(card.sent[2]).toBe(card.sent[0]);

      card.finishRefresh();
      await settle();
      expect(card.form()).toBe(form);
      expect(button(card.root, "恢复外部访问…").hasAttribute("disabled")).toBe(false);
      expect(form.submit()).toBe(true);
      await settle();
      await click(card.root, "确认");
      assertNewResume(card.sent);
      expect(jsonObject(accepted(parseJson(required(card.sent[3])))).reason).toBe("恢复访问");
    } finally {
      card.dispose();
    }
  }
});

async function withInvalidReason(
  focus: "reason" | "submit",
  check: (parts: {
    readonly card: ReturnType<typeof pauseCard>;
    readonly region: TestElement;
    readonly reason: TestElement;
    deliver(): Promise<void>;
  }) => Promise<void> | void,
): Promise<void> {
  const card = pauseCard();
  try {
    card.mountCard();
    await settle();
    const reason = card.reason();
    reason.input(TOO_LONG);
    flush();
    const region = required(
      findElement(card.root, (element) => element.getAttribute("aria-live") === "polite"),
      "礼貌播报区",
    );
    card.document.activeElement = focus === "reason" ? reason : button(card.root, "暂停外部访问…");
    expect(utf8ByteLength(reason.value)).toBe(1026);
    expect(card.document.focusCalls).toEqual([]);
    jest.useFakeTimers();
    try {
      await check({
        card,
        reason,
        region,
        async deliver() {
          jest.advanceTimersByTime(50);
          await settle();
        },
      });
      expect(card.sent).toEqual([]);
    } finally {
      jest.useRealTimers();
    }
  } finally {
    card.dispose();
  }
}

test("enter_with_focus_on_invalid_reason_announces_without_moving_focus", async () => {
  await withInvalidReason("reason", async ({ card, reason, region, deliver }) => {
    expect(card.form().submit()).toBe(true);
    flush();
    expect(reason.getAttribute("aria-invalid")).toBe("true");
    expect(region.textContent).toBe("");
    expect(card.document.activeElement).toBe(reason);
    expect(card.document.focusCalls).toEqual([]);
    await deliver();
    expect(region.textContent).toBe(INVALID_REASON);
    expect(card.document.focusCalls).toEqual([]);
  });
});

test("submit_from_elsewhere_moves_focus_to_invalid_reason", async () => {
  await withInvalidReason("submit", async ({ card, reason, region, deliver }) => {
    expect(card.form().submit()).toBe(true);
    jest.advanceTimersByTime(0);
    await settle();
    expect(card.document.activeElement).toBe(reason);
    expect(card.document.focusCalls).toEqual([reason]);
    expect(reason.getAttribute("aria-invalid")).toBe("true");
    expect(region.textContent).toBe("");
    await deliver();
    expect(region.textContent).toBe("");
  });
});

test("repeat_submit_clears_the_live_region_before_announcing_the_same_error", async () => {
  await withInvalidReason("reason", async ({ card, reason, region, deliver }) => {
    expect(card.form().submit()).toBe(true);
    await deliver();
    expect(region.textContent).toBe(INVALID_REASON);
    expect(card.form().submit()).toBe(true);
    flush();
    expect(region.textContent).toBe("");
    expect(card.document.activeElement).toBe(reason);
    await deliver();
    expect(
      findElement(card.root, (element) => element.getAttribute("aria-live") === "polite"),
    ).toBe(region);
    expect(region.textContent).toBe(INVALID_REASON);
    expect(card.document.focusCalls).toEqual([]);
  });
});
