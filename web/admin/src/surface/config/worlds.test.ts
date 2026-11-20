// 重放结束后，已重挂的世界卡片必须等待第三次政策读取。

import { afterEach, beforeEach, expect, test } from "bun:test";
import { flush } from "solid-js";
import { writeOutcomeUnknown } from "../../api/failure";
import { captureEvent, findElement, required } from "../solid-dom-fixture";
import {
  button,
  clickButton,
  configPage,
  deferredReply,
  maybeButton,
  settle,
} from "./config-page-fixture.test";
import { apiFailure } from "./config-projection-fixture.test";
import { loadConfigFixture } from "./config-render-fixture.test";
import { beginConfigTestLock, endConfigTestLock } from "./config-test-scope-fixture.test";

const fixture = await loadConfigFixture();
const CONFLICT = "数据已变化，或本次命令与已有请求冲突。请重新加载后再试。";
const DISABLED = "主世界 已停止新的拍卖业务。";

function world(enabled: boolean, revision: number) {
  return { id: "main", enabled, availability: "available", policy_revision: revision };
}

function worlds(enabled: boolean, revision: number): Response {
  // Rust 的替身返回模型，真实读端点另要求含完整的两个世界。
  return Response.json({
    code: "ok",
    message: "",
    data: { worlds: [world(enabled, revision), { ...world(true, revision), id: "isekai" }] },
  });
}

beforeEach(beginConfigTestLock);
afterEach(endConfigTestLock);

test("reopened_world_card_awaits_a_current_policy_after_replay", async () => {
  const refreshed = deferredReply();
  let reads = 0;
  const page = configPage((request) => {
    expect(request.method).toBe("GET");
    expect(request.path).toBe("/api/v1/admin/worlds");
    reads += 1;
    expect(reads).toBeLessThanOrEqual(3);
    return reads <= 2 ? worlds(true, 7) : refreshed.wait();
  });
  const writes: string[] = [];
  const panel = () =>
    page.mount(() => fixture.createComponent(fixture.worlds.AdminBusinessWorlds, {}));
  try {
    page.mount(() =>
      fixture.createComponent(fixture.pendingPanel.PendingCommandNotices, {
        host: page.pending.host,
      }),
    );
    const leave = panel();
    await settle();
    expect(button(page.root, "关闭业务世界").hasAttribute("disabled")).toBe(false);

    // Rust 原卡片的请求替身只交给会话宿主，切换时不把来源页保留到新卡片。
    expect(
      page.pending.origin.commands.submit(
        "world:main",
        "关闭业务世界",
        async (commandId) => {
          writes.push(commandId);
          return writes.length === 1
            ? { ok: false, error: writeOutcomeUnknown("storage_unavailable") }
            : { ok: true, value: undefined };
        },
        () => {},
        () => {},
      ),
    ).toBe(true);
    await settle();
    expect(writes).toHaveLength(1);
    leave();
    page.pending.origin.detach();
    expect(
      findElement(
        page.root,
        (node) => node.tag === "button" && node.textContent === "关闭业务世界",
      ),
    ).toBeNull();
    panel();
    await settle();
    expect(reads).toBe(2);
    clickButton(page.root, "原样重试");
    await settle();
    expect(reads).toBe(3);
    const original = required(writes[0], "原命令");
    expect(writes).toEqual([original, original]);

    const duringRead = button(page.root, "关闭业务世界");
    expect(duringRead.hasAttribute("disabled")).toBe(true);
    captureEvent(duringRead, "onClick")();
    flush();
    expect(findElement(page.root, (node) => node.getAttribute("role") === "dialog")).toBeNull();

    refreshed.respond(worlds(false, 8));
    await settle();
    clickButton(page.root, "开放业务世界");
    await settle();
    const dialog = required(
      findElement(page.root, (node) => node.getAttribute("role") === "dialog"),
      "世界政策确认框",
    );
    expect(dialog.querySelector("h2")?.textContent).toBe("确认开放业务世界");
    expect(page.requests.map(({ method }) => method)).toEqual(["GET", "GET", "GET"]);
  } finally {
    page.dispose();
  }
});

test("world_stale_rejection_reloads_without_discarding_earlier_unknown", async () => {
  let reads = 0;
  const writes: string[] = [];
  const page = configPage((request) => {
    if (request.method === "GET") {
      expect(request.path).toBe("/api/v1/admin/worlds");
      reads += 1;
      return reads === 1 ? worlds(true, 7) : worlds(false, 8);
    }
    expect([request.method, request.path]).toEqual(["PATCH", "/api/v1/admin/worlds/main"]);
    writes.push(request.body);
    switch (writes.length) {
      case 1:
        return apiFailure(503, "storage_unavailable", "暂不可用");
      case 2:
        return apiFailure(429, "stale_world_state", "版本已变化");
      case 3:
        return apiFailure(409, "stale_world_state", "版本已变化");
      case 4:
        return Response.json({ code: "ok", message: "", data: { world: world(false, 8) } });
      default:
        throw new Error("额外的世界写入");
    }
  });
  try {
    page.mount(() => fixture.createComponent(fixture.worlds.AdminBusinessWorlds, {}));
    await settle();
    clickButton(page.root, "关闭业务世界");
    await settle();
    clickButton(page.root, "确认");
    await settle();
    clickButton(page.root, "原样重试");
    await settle();
    expect(reads).toBe(1);
    expect(maybeButton(page.root, "原样重试")).not.toBeNull();
    clickButton(page.root, "原样重试");
    await settle();
    expect(reads).toBeGreaterThan(1);
    expect(maybeButton(page.root, "原样重试")).not.toBeNull();
    expect(button(page.root, "开放业务世界").hasAttribute("disabled")).toBe(true);
    expect(page.root.textContent).toContain(CONFLICT);
    clickButton(page.root, "原样重试");
    await settle();
    expect(writes).toHaveLength(4);
    expect(new Set(writes).size).toBe(1);
    expect(JSON.parse(required(writes[0], "原启停命令"))).toEqual({
      command_id: expect.stringMatching(/^[\da-f-]{36}$/u),
      enabled: false,
      expected_policy_revision: 7,
    });
    expect(maybeButton(page.root, "原样重试")).toBeNull();
    expect(button(page.root, "开放业务世界").hasAttribute("disabled")).toBe(false);
    expect(page.root.textContent).toContain(DISABLED);
    expect(page.root.textContent).not.toContain(CONFLICT);
  } finally {
    page.dispose();
  }
});

test("world_receipt_mismatches_keep_the_command_for_verbatim_retry", async () => {
  let reads = 0;
  const writes: string[] = [];
  const target = world(false, 8);
  const receipts = [
    { ...target, id: "isekai" },
    { ...target, enabled: true },
    world(false, 9),
    target,
  ];
  const page = configPage((request) => {
    if (request.method === "GET") {
      expect(request.path).toBe("/api/v1/admin/worlds");
      reads += 1;
      return reads === 1 ? worlds(true, 7) : worlds(false, 8);
    }
    expect([request.method, request.path]).toEqual(["PATCH", "/api/v1/admin/worlds/main"]);
    const receipt = required(receipts[writes.length], "世界启停回执");
    writes.push(request.body);
    return Response.json({ code: "ok", message: "", data: { world: receipt } });
  });
  try {
    page.mount(() => fixture.createComponent(fixture.worlds.AdminBusinessWorlds, {}));
    await settle();
    clickButton(page.root, "关闭业务世界");
    await settle();
    clickButton(page.root, "确认");
    await settle();
    for (const mismatch of ["世界", "目标状态", "修订号"]) {
      expect(reads, mismatch).toBe(1);
      expect(page.root.textContent, mismatch).not.toContain(DISABLED);
      expect(maybeButton(page.root, "原样重试"), mismatch).not.toBeNull();
      clickButton(page.root, "原样重试");
      await settle();
    }
    const first = required(writes[0], "原启停命令");
    expect(writes).toEqual([first, first, first, first]);
    expect(JSON.parse(first)).toEqual({
      command_id: expect.stringMatching(/^[\da-f-]{36}$/u),
      enabled: false,
      expected_policy_revision: 7,
    });
    expect(maybeButton(page.root, "原样重试")).toBeNull();
    expect(page.root.textContent).toContain(DISABLED);
  } finally {
    page.dispose();
  }
});
