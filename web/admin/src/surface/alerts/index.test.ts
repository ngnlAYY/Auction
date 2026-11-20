// 告警页用户事件：冻结命令、切页重试与备注播报。

import { expect, test } from "bun:test";
import { parseJsonObject } from "../../api/api-request-fixture";
import { t11Buttons } from "../dashboard/t11-page-fixture.test";
import {
  fireT11Event,
  installT11Clock,
  oneT11Element,
  settleT11View,
} from "../dashboard/t11-solid-fixture.test";
import { findAll, type TestElement } from "../solid-dom-fixture";
import {
  type AlertsPageFixture,
  expectReasonFeedback,
  reasonLiveRegions,
  textStartingWith,
  withAlertsPage,
} from "./alerts-page-fixture.test";

function pendingPanel(tree: TestElement): TestElement {
  return oneT11Element(
    findAll(
      tree,
      (element) => element.tag === "section" && element.getAttribute("aria-label") === "未决操作",
    ),
    "告警页未决面板",
  );
}

function pageRetry(tree: TestElement): TestElement {
  const actions = findAll(pendingPanel(tree), (element) => element.tag === "button");
  const retry = oneT11Element(actions, "页内原样重试");
  expect(retry.textContent.trim()).toBe("原样重试");
  expect(retry.attributes.disabled).toBe(false);
  return retry;
}

function expectAlertPage(tree: TestElement, present: boolean): void {
  expect(findAll(tree, (element) => element.id === "alert-console-title")).toHaveLength(
    present ? 1 : 0,
  );
  expect(
    findAll(
      tree,
      (element) =>
        element.tag === "p" &&
        element.getAttribute("class") === "admin-command-notices__status" &&
        element.getAttribute("role") === "status",
    ),
  ).toHaveLength(1);
}

function expectWrites(
  page: AlertsPageFixture,
  action: "acknowledge" | "silence",
  count: number,
): void {
  const url = `/api/v1/admin/alerts/abababababababababababababababababababababababababababababababab/${action}`;
  expect(page.writes.map((request) => [request.method, request.url])).toEqual(
    Array.from({ length: count }, () => ["POST", url]),
  );
  const commandRequests = page.requests.filter(
    (request) =>
      request.url === "/api/v1/admin/session?include=server_time" ||
      (request.method === "POST" && request.url.startsWith("/api/v1/admin/alerts/")),
  );
  expect(commandRequests.map((request) => [request.method, request.url])).toEqual(
    Array.from({ length: count }, () => [
      ["GET", "/api/v1/admin/session?include=server_time"],
      ["POST", url],
    ]).flat(),
  );
}

async function retryAfterStale(action: "acknowledge" | "silence"): Promise<void> {
  await withAlertsPage(async (page) => {
    const actionIndex = action === "acknowledge" ? 0 : 1;
    fireT11Event(page.reason(), "onInput", "  第一轮备注  ");
    if (action === "silence") {
      await page.setSilenceHours("8");
    }
    fireT11Event(page.rowActions(false)[actionIndex], "onClick");
    await settleT11View();
    expect(textStartingWith(page.tree, "曾有请求结果未知")).toBe(1);
    expect(t11Buttons(page.tree, "原样重试")).toHaveLength(2);
    page.rowActions(true);
    pageRetry(page.tree);
    expect(page.reads).toEqual(["active"]);

    page.recur();
    await page.filter("all");
    expect(
      findAll(
        page.tree,
        (element) =>
          element.getAttribute("class") === "admin-alert-console__count" &&
          element.textContent === "18",
      ),
    ).toHaveLength(1);
    const retry = pageRetry(page.tree);
    page.rowActions(true);
    expect(page.reads).toEqual(["active", "all"]);
    expectWrites(page, action, 1);
    fireT11Event(page.reason(), "onInput", "第二轮备注");

    fireT11Event(retry, "onClick");
    await settleT11View();
    expect(textStartingWith(page.tree, "告警状态已变化。请核对重新读取后的列表")).toBe(2);
    expect(t11Buttons(page.tree, "关闭已拒绝操作")).toHaveLength(1);
    expect(page.reads).toEqual(["active", "all", "all"]);
    expectWrites(page, action, 2);
    expect(page.writes[1]).toEqual(page.writes[0]);

    if (action === "silence") {
      await page.setSilenceHours("1");
    }
    fireT11Event(page.rowActions(false)[actionIndex], "onClick");
    await settleT11View();
    expect(textStartingWith(page.tree, "操作已记录")).toBe(1);
    expect(
      findAll(
        page.tree,
        (element) => element.tag === "section" && element.getAttribute("aria-label") === "未决操作",
      ).flatMap((panel) => findAll(panel, (node) => node.tag === "button")),
    ).toEqual([]);
    expect(page.reads).toEqual(["active", "all", "all", "all"]);
    expectWrites(page, action, 3);

    const [first, , third] = page.writes;
    if (first === undefined || third === undefined) throw new Error("告警写入记录不完整");
    const original = parseJsonObject(first.body, "首次写入");
    const next = parseJsonObject(third.body, "复发后写入");
    expect(original).toMatchObject({ incident_generation: 2, reason: "第一轮备注" });
    expect(next).toMatchObject({ incident_generation: 3, reason: "第二轮备注" });
    if (action === "silence") {
      expect(original.silence_seconds).toBe(28_800);
      expect(next.silence_seconds).toBe(3_600);
    }
    const uuid = /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/u;
    expect(original.command_id).toMatch(uuid);
    expect(next.command_id).toMatch(uuid);
    expect(original.command_id).not.toBe(next.command_id);
  });
}

test("acknowledge_retries_original_incident_and_allows_new_command_after_stale", async () => {
  await retryAfterStale("acknowledge");
});

test("silence_retries_original_incident_and_allows_new_command_after_stale", async () => {
  await retryAfterStale("silence");
});

test("silence_cannot_replace_unknown_after_leaving_and_returning", async () => {
  await withAlertsPage(async (page) => {
    fireT11Event(page.reason(), "onInput", "原始备注");
    fireT11Event(page.rowActions(false)[1], "onClick");
    await settleT11View();
    await page.navigate();
    expectAlertPage(page.tree, false);
    await page.navigate();
    expectAlertPage(page.tree, true);
    const [acknowledge, silence] = page.rowActions(true);
    fireT11Event(acknowledge, "onClick");
    fireT11Event(silence, "onClick");
    await settleT11View();
    expectWrites(page, "silence", 1);

    await page.navigate();
    expectAlertPage(page.tree, false);
    expect(page.reads).toEqual(["active", "active"]);
    const notices = oneT11Element(
      findAll(
        page.tree,
        (element) =>
          element.tag === "section" &&
          element.getAttribute("aria-labelledby") === "admin-command-notices-title",
      ),
      "外壳未决操作区",
    );
    // Rust 的控件遍历只递归片段，命令标识里的三个 TruncatedText 提示不计入。
    const tooltipTriggers = findAll(
      notices,
      (element) =>
        element.tag === "span" &&
        element.id.startsWith("shared-truncated-tooltip-") &&
        element.id.endsWith("-trigger"),
    );
    expect(tooltipTriggers).toHaveLength(3);
    const controls = findAll(
      notices,
      (element) => element.attributes.onClick != null && !tooltipTriggers.includes(element),
    );
    expect(
      controls.map((control) => ({
        tag: control.tag,
        text: control.textContent.trim(),
        disabled: control.attributes.disabled,
        hasClickHandler: typeof control.attributes.onClick === "function",
      })),
    ).toEqual([
      { tag: "button", text: "原样重试", disabled: false, hasClickHandler: true },
      { tag: "button", text: "放弃", disabled: false, hasClickHandler: true },
    ]);
    const [retry] = controls;
    if (retry === undefined) throw new Error("外壳缺少原样重试按钮");
    fireT11Event(retry, "onClick");
    await settleT11View();
    expect(t11Buttons(page.tree, "关闭回执")).toHaveLength(1);
    expectWrites(page, "silence", 2);
    expect(page.writes[1]).toEqual(page.writes[0]);
    expect(page.reads).toEqual(["active", "active"]);
  });
});

test("reason_announcement_survives_empty_filter_until_the_draft_is_corrected", async () => {
  const clock = await installT11Clock();
  try {
    await withAlertsPage(async (page) => {
      const tooLong = "告".repeat(683);
      fireT11Event(page.reason(), "onInput", tooLong);
      await settleT11View();
      expectReasonFeedback(page.tree, false, false);
      fireT11Event(page.reason(), "onBlur");
      await settleT11View();
      expectReasonFeedback(page.tree, true, false);
      expect(clock.pending(500)).toBe(1);
      await clock.advance(500);
      expectReasonFeedback(page.tree, true, true);

      await page.filter("resolved");
      expect(textStartingWith(page.tree, "没有符合条件的告警")).toBe(1);
      expect(findAll(page.tree, (element) => element.id === "alert-console-reason")).toEqual([]);
      expect(reasonLiveRegions(page.tree)).toEqual([]);
      await page.filter("active");
      const reason = page.reason();
      expect(reason.value).toBe(tooLong);
      expectReasonFeedback(page.tree, true, false);
      reason.focus();
      expect(page.document.activeElement).toBe(reason);
      fireT11Event(reason, "onBlur");
      await settleT11View();
      expectReasonFeedback(page.tree, true, false);
      expect(clock.pending(500)).toBe(0);

      fireT11Event(page.rowActions(false)[0], "onClick");
      await settleT11View();
      expect(page.writes).toEqual([]);
      expectReasonFeedback(page.tree, true, false);
      await clock.advance(500);
      expectReasonFeedback(page.tree, true, true);

      fireT11Event(reason, "onInput", "已经修正");
      await settleT11View();
      expectReasonFeedback(page.tree, false, false);
      fireT11Event(reason, "onInput", tooLong);
      fireT11Event(reason, "onBlur");
      await settleT11View();
      await clock.advance(500);
      expectReasonFeedback(page.tree, true, true);
      fireT11Event(reason, "onBlur");
      await settleT11View();
      expectReasonFeedback(page.tree, true, true);
      expect(clock.pending(500)).toBe(0);
    });
  } finally {
    clock.restore();
  }
});
