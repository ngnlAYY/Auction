// 创建对话框卸载后的重试交付，以及无 Key 回执的门禁。

import { expect, spyOn, test } from "bun:test";
import { err, ok } from "@auction/web-shared/result";
import { flush } from "solid-js";
import { createAdminRequestHarness, parseJsonObject } from "../../api/api-request-fixture";
import { adminApiFailureFromHttpBody } from "../../api/failure";
import type { PreparedJsonCommand } from "../../api/transport";
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

const GENERATED = "一次性测试值";

async function withUserCreate(
  generateKey: boolean,
  run: (
    document: TestDocument,
    attempts: readonly { id: string; body: PreparedJsonCommand }[],
  ) => Promise<void>,
) {
  const users = await import("../../api/users");
  const Fixture = await loadUserCreate();
  const harness = createAdminRequestHarness();
  const attempts: { id: string; body: PreparedJsonCommand }[] = [];
  const manualKey = " ".repeat(32);
  let restoreCreate: (() => void) | undefined;
  try {
    const create = spyOn(users, "createAdminUser");
    restoreCreate = () => create.mockRestore();
    const unexpectedRequests = harness.installFetch([]);
    const { host } = harness.registerHost();
    create.mockImplementation(async (requestHost, id, prepare) => {
      expect(requestHost).toBe(host);
      const prepared = prepare(id);
      if (!prepared.ok) throw new Error("创建命令未成功冻结");
      expect(prepared.value.wantKey).toBe(generateKey);
      attempts.push({ id, body: prepared.value.prepared });
      if (attempts.length === 1 && generateKey) {
        return err(
          adminApiFailureFromHttpBody(
            503,
            '{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}',
          ).code,
        );
      }
      return ok({
        user: {
          id: "00000000-0000-4000-8000-000000000026",
          username: "玩家",
          forum_account_id: null,
          status: "active",
          key_status: "active",
          state_version: 1,
          created_at: "2026-11-19T00:00:00Z",
          updated_at: "2026-11-19T00:00:00Z",
          blacklist_count: 0,
          blacklist_history: [],
        },
        generated_key: generateKey ? GENERATED : null,
      });
    });
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
        expect(parseJsonObject(first.body.body, "创建用户请求")).toEqual({
          command_id: first.id,
          username: "玩家",
          forum_account_id: null,
          key_mode: generateKey ? "generated" : "manual",
          key: generateKey ? null : manualKey,
        });
        await run(document, attempts);
        expect(unexpectedRequests).toHaveLength(0);
      },
    );
  } finally {
    try {
      restoreCreate?.();
    } finally {
      harness.cleanup();
    }
  }
}

test("successful_retry_delivers_to_the_page_after_the_dialog_unmounts", async () => {
  await withUserCreate(true, async (document, attempts) => {
    await clickButton(document, "关闭创建框");
    expect(document.querySelectorAll("form")).toHaveLength(0);
    expect(hasText(document, GENERATED)).toBe(false);

    await clickButton(document, "原样重试");
    expect(attempts).toHaveLength(2);
    const [first, retry] = attempts;
    if (first === undefined || retry === undefined) throw new Error("缺少首次命令或原样重试");
    expect(retry.id).toBe(first.id);
    expect(retry.body).toBe(first.body);
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
  await withUserCreate(false, async (document) => {
    expect(hasText(document, "玩家")).toBe(true);
    expect(findButton(document, "关闭")).toBeNull();
    expect(button(document, "开始下一次").hasAttribute("disabled")).toBe(false);
  });
});
