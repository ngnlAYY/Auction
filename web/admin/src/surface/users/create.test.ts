// 创建对话框卸载后的重试交付，以及无 Key 回执的门禁。

import { expect, test } from "bun:test";
import { flush } from "solid-js";
import {
  createAdminRequestHarness,
  installFixtureFetch,
  parseJsonObject,
  type RecordedRequest,
  type Reply,
  recordRequest,
  TEST_SESSION_BODY,
} from "../../api/api-request-fixture";
import type { TestDocument } from "../solid-dom-fixture";
import { settle } from "../solid-dom-fixture";
import {
  button,
  clickButton,
  field,
  findButton,
  hasText,
  loadUserCreate,
  onlyElement,
  submitForm,
  withT10View,
} from "../t10-view-fixture.test";
import { adminUserGeneratedKeyDelivery } from "./create";

const GENERATED = "A".repeat(43);
const STORAGE_UNAVAILABLE: Reply = {
  status: 503,
  body: '{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}',
};

function created(generated: boolean, username = "玩家"): Reply {
  const user = {
    id: "00000000-0000-4000-8000-000000000026",
    username,
    forum_account_id: null,
    status: "active",
    key_status: "active",
    state_version: 1,
    created_at: "2026-11-19T00:00:00Z",
    updated_at: "2026-11-19T00:00:00Z",
  };
  return {
    status: 201,
    body: JSON.stringify({
      code: "created",
      message: "",
      data: generated ? { user, generated_key: GENERATED } : { user },
    }),
  };
}

async function withUserCreate(
  generateKey: boolean,
  replies: readonly Reply[],
  run: (document: TestDocument, attempts: readonly RecordedRequest[]) => Promise<void> | void,
) {
  const Fixture = await loadUserCreate();
  const harness = createAdminRequestHarness();
  const attempts: RecordedRequest[] = [];
  const unexpected: string[] = [];
  const manualKey = " ".repeat(32);
  harness.installBrowserRequest();
  const restore = installFixtureFetch(async (input, init) => {
    const request = recordRequest(input, init);
    const path = new URL(request.url, "https://auction.test").pathname;
    if (request.method === "GET" && path === "/api/v1/admin/session")
      return Response.json(TEST_SESSION_BODY);
    if (request.method === "POST" && path === "/api/v1/browser-errors") return Response.json({});
    const reply = replies[attempts.length];
    if (request.method !== "POST" || path !== "/api/v1/admin/users" || reply === undefined) {
      unexpected.push(`${request.method} ${request.url}`);
      throw new Error("未预期的创建用户请求");
    }
    attempts.push(request);
    return new Response(reply.body, { status: reply.status ?? 200 });
  });
  try {
    const { host } = harness.registerHost();
    await withT10View(
      () => Fixture({ host }),
      async (document) => {
        field(document, "create-username").input("玩家");
        if (!generateKey) {
          field(document, "create-key-mode").change("manual");
          flush();
          field(document, "create-key").input(manualKey);
        }
        flush();
        const form = onlyElement(document, "form");
        submitForm(form);
        await settle();
        expect(attempts).toHaveLength(1);
        const first = attempts[0];
        if (first === undefined) throw new Error("未执行创建命令");
        expect(parseJsonObject(first.body, "创建用户请求")).toEqual({
          command_id: expect.stringMatching(/^[0-9a-f-]{36}$/u),
          username: "玩家",
          forum_account_id: null,
          key_mode: generateKey ? "generated" : "manual",
          key: generateKey ? null : manualKey,
        });
        await run(document, attempts);
        expect(unexpected).toEqual([]);
      },
    );
  } finally {
    restore();
    harness.cleanup();
  }
}

test.each([
  ["503 storage_unavailable", STORAGE_UNAVAILABLE],
  ["错用户名的成功回执", created(true, "其他用户")],
] as const)("创建收到 %s 后关窗仍可原样重试并交付 Key", async (_, firstReply) => {
  await withUserCreate(true, [firstReply, created(true)], async (document, attempts) => {
    expect(findButton(document, "原样重试")).not.toBeNull();
    await clickButton(document, "关闭创建框");
    expect(document.querySelectorAll("form")).toHaveLength(0);
    expect(hasText(document, GENERATED)).toBe(false);

    await clickButton(document, "原样重试");
    expect(attempts).toHaveLength(2);
    const [first, retry] = attempts;
    if (first === undefined || retry === undefined) throw new Error("缺少首次命令或原样重试");
    expect(retry).toEqual(first);
    expect(hasText(document, "玩家")).toBe(true);
    expect(hasText(document, GENERATED)).toBe(true);
    expect(button(document, "开始下一次").hasAttribute("disabled")).toBe(true);

    const delivery = adminUserGeneratedKeyDelivery(GENERATED, false);
    if (delivery.kind !== "receipt" || delivery.receipt?.kind !== "generated-key") {
      throw new Error("一次性 Key 缺少脱敏封装");
    }
    expect(Bun.inspect(delivery.receipt.key)).not.toContain(GENERATED);
    expect(JSON.stringify(delivery.receipt.key)).toBe('"[REDACTED]"');

    await clickButton(document, "关闭");
    expect(hasText(document, GENERATED)).toBe(false);
    expect(button(document, "开始下一次").hasAttribute("disabled")).toBe(false);
  });
});

test("a_keyless_result_does_not_open_the_key_dialog_or_hold_the_gate", async () => {
  await withUserCreate(false, [created(false)], (document) => {
    expect(hasText(document, "玩家")).toBe(true);
    expect(findButton(document, "关闭")).toBeNull();
    expect(button(document, "开始下一次").hasAttribute("disabled")).toBe(false);
  });
});

test("创建用户的 409 user_conflict 仍显示在用户名字段", async () => {
  await withUserCreate(
    true,
    [
      {
        status: 409,
        body: '{"code":"user_conflict","message":"用户已存在","data":null,"request_id":null}',
      },
    ],
    (document) => {
      expect(field(document, "create-username").getAttribute("aria-invalid")).toBe("true");
      expect(field(document, "create-username").getAttribute("aria-describedby")).toBe(
        "create-username-error",
      );
      expect(field(document, "create-username-error").textContent).toBe(
        "用户名或论坛账号与现有记录冲突。",
      );
      expect(hasText(document, GENERATED)).toBe(false);
      expect(findButton(document, "原样重试")).toBeNull();
      expect(button(document, "开始下一次").hasAttribute("disabled")).toBe(false);
    },
  );
});
