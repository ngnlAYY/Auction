// 重放结束后，已重挂的世界卡片必须等待第三次政策读取。

import { afterEach, beforeEach, expect, test } from "bun:test";
import { flush } from "solid-js";
import { captureEvent, findElement, required } from "../solid-dom-fixture";
import { button, clickButton, configPage, deferredReply, settle } from "./config-page-fixture.test";
import { loadConfigFixture } from "./config-render-fixture.test";
import { beginConfigTestLock, endConfigTestLock } from "./config-test-scope-fixture.test";

const fixture = await loadConfigFixture();

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
            ? { ok: false, error: "storage_unavailable" }
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
