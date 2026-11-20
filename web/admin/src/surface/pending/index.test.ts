// 未决命令宿主：确定拒绝、同 ID 重放、来源卸载后的回执与门禁占用。

import { expect, test } from "bun:test";
import { err, ok } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import { createComponent, createRoot, flush } from "solid-js";
import { adminApiFailureFromHttpBody, adminApiFailureNotSentCode } from "../../api/failure";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { cleanupScope } from "../../test-scope-fixture";
import { installAdminTranslation } from "../admin-locale-fixture.test";
import { adminApiErrorMessage } from "../presentation";
import { createElement, findAll, insert, installTestDocument } from "../solid-dom-fixture";
import { definiteRejection } from ".";
import { createPendingHarness, settlePending } from "./pending-command-fixture";

const { PendingCommandPanel } = await loadRenderModule<typeof import("./panel")>(
  new URL("panel.tsx", import.meta.url).pathname,
  "PendingCommandPanel",
);

function failure(status: number, code: string, message: string): string {
  return adminApiFailureFromHttpBody(
    status,
    JSON.stringify({ code, message, data: null, request_id: null }),
  ).code;
}

test("被拒绝的条目显示各自的稳定错误文案且不提供原样重试", async () => {
  const scope = cleanupScope();
  try {
    scope.defer(installAdminTranslation());
    scope.defer(flush);
    const harness = createPendingHarness();
    scope.defer(harness.dispose);
    let successCalls = 0;
    for (const [status, code] of [
      [409, "alert_stale"],
      [413, "body_too_large"],
    ] as const) {
      expect(
        harness.origin.commands.submit(
          code,
          "测试写入",
          async () => err(failure(status, code, "写入已拒绝")),
          () => {
            successCalls += 1;
            throw new Error("拒绝不能交付成功回执");
          },
          () => {},
        ),
      ).toBe(true);
    }
    await settlePending();
    expect(harness.host.entries.peek()).toEqual([]);
    expect(successCalls).toBe(0);
    const notices = harness.origin.rejections.peek();
    expect(notices.map((notice) => notice.identity.entity)).toEqual([
      "alert_stale",
      "body_too_large",
    ]);
    expect(notices.map((notice) => (notice.result.ok ? null : notice.result.error))).toEqual([
      "alert_stale",
      "body_too_large",
    ]);
    expect(t("admin-pending-dismiss")).toBe("关闭已拒绝操作");
    expect(adminApiErrorMessage("alert_stale")).toBe(
      "告警状态已变化。请核对重新读取后的列表，再决定是否处置。",
    );
    expect(adminApiErrorMessage("body_too_large")).toBe("请求体过大。请缩短内容后重试。");

    const document = installTestDocument();
    scope.defer(() => document.restore());
    const tree = createElement("div");
    createRoot((disposeRoot) => {
      scope.defer(disposeRoot);
      insert(tree, () =>
        createComponent(PendingCommandPanel, {
          owner: harness.origin.commands,
        }),
      );
    });
    document.add(tree);
    flush();
    expect(tree.textContent).toContain("关闭已拒绝操作");
    expect(tree.textContent).toContain("告警状态已变化。请核对重新读取后的列表，再决定是否处置。");
    expect(tree.textContent).toContain("请求体过大。请缩短内容后重试。");
    expect(tree.textContent).not.toContain("原样重试");
    expect(
      findAll(tree, (element) => element.matches("button")).map((element) =>
        element.textContent.trim(),
      ),
    ).toEqual(["关闭已拒绝操作", "关闭已拒绝操作"]);
  } finally {
    scope.dispose();
  }
});

test("被丢弃的辅助动作与账本冲突都是确定拒绝", () => {
  for (const code of [
    "command_conflict",
    "not_found",
    "stale_credential_state",
    "credential_conflict",
    "credential_login_rejected",
    "credential_login_challenge",
    "credential_login_busy",
    "invalid_auction_intake",
    "unsupported_equipment_source",
    "auction_intake_unavailable",
    "seller_blacklisted",
    "equipment_conflict",
  ]) {
    expect(definiteRejection(code), code).toBe(true);
  }
});

test("可能紧随已提交命令的错误保持未决", () => {
  for (const code of [
    "state_conflict",
    "storage_unavailable",
    "reconciliation_required",
    "outcome_unknown",
    "internal_error",
    "invalid_credentials",
  ]) {
    expect(definiteRejection(code), code).toBe(false);
  }
});

test("干净拒绝包含已回滚事务与瞬时守卫", () => {
  for (const code of [
    "invalid_json",
    "invalid_replay",
    "rate_limited",
    "invalid_runtime_config",
    "invalid_runtime_switch",
    "stale_recovery_state",
    "recovery_send_unattributed",
    "recovery_not_confirmable",
    "recovery_evidence_unresolved",
    "stale_auction_state",
    "stale_lot_state",
    "stale_delivery_state",
    "sent_proof_rejected",
    "world_has_active_auction",
    "world_not_found",
    "delivery_not_found",
    "not_acknowledgeable",
    "rename_name_taken",
    "user_conflict",
    "alert_stale",
    "invalid_alert_command",
    "invalid_delivery_command",
    "delivery_sent_sighted",
    "settlement_obligation_ineligible",
    "invalid_auction_state",
    "invalid_lot_state",
    "manual_early_settlement_disabled",
    "invalid_credential_request",
    "invalid_credential_secret",
    "external_access_paused",
    "invalid_mail_request",
    "poll_rejected",
    "conflict",
  ]) {
    expect(definiteRejection(code), code).toBe(true);
  }
});

test("写前校验是确定拒绝", () => {
  for (const code of [
    "invalid_request_id",
    "invalid_query",
    "unsupported_media_type",
    "body_too_large",
    "method_not_allowed",
    "origin_forbidden",
    "csrf_invalid",
    "invalid_command_id",
    "invalid_auction_id",
    "invalid_auction_command",
    "invalid_auction_schedule",
    "invalid_lot_id",
    "invalid_lot_action",
    "invalid_lot_command",
    "invalid_recovery_command",
    "invalid_world",
    "invalid_world_command",
    "invalid_delivery_id",
    "invalid_user_id",
    "invalid_rename_id",
    "invalid_rename_command",
    "invalid_credential_action",
    "invalid_credential_kind",
    "invalid_credential_label",
    "invalid_credential_player_name",
    "invalid_credential_status",
    "invalid_credential_world",
    "invalid_mail_template",
    "mail_template_not_found",
    "mail_template_conflict",
    "runtime_switch_conflict",
    "already_disposed",
  ]) {
    expect(definiteRejection(code), code).toBe(true);
  }
});

test("不可读的错误回执不能证明拒绝", () => {
  for (const code of ["invalid_response", "http_400", "http_413", "http_503", "new_error"]) {
    expect(definiteRejection(code), code).toBe(false);
  }
});

test("未经重放检查的拒绝不能清除较早的未知尝试", async () => {
  const harness = createPendingHarness();
  try {
    const ids: string[] = [];
    let attempt = 0;
    expect(
      harness.origin.commands.submit(
        "first",
        "test",
        async (commandId) => {
          ids.push(commandId);
          attempt += 1;
          switch (attempt) {
            case 1:
              return err("request_timeout");
            case 2:
              return err(adminApiFailureNotSentCode("alert_stale"));
            case 3:
              return err("rate_limited");
            case 4:
              return err("unauthenticated");
            case 5:
              return err("rename_name_taken");
            default:
              return ok(undefined);
          }
        },
        () => {},
        () => {},
      ),
    ).toBe(true);
    await settlePending();
    const commandId = ids[0];
    if (commandId === undefined) throw new Error("未生成命令 ID");
    expect(harness.host.entries.peek()[0]?.unknown).toBe(true);

    for (let retry = 0; retry < 4; retry += 1) {
      harness.host.retry(commandId);
      await settlePending();
      expect(harness.host.entries.peek()[0]?.unknown).toBe(true);
      harness.origin.commands.dismiss(commandId);
      expect(
        harness.origin.commands.submit(
          "first",
          "new",
          async () => ok(undefined),
          () => {},
          () => {},
        ),
      ).toBe(false);
      expect(harness.origin.commands.contains("first")).toBe(true);
    }

    harness.host.retry(commandId);
    await settlePending();
    expect(harness.origin.commands.contains("first")).toBe(false);
    expect(ids).toEqual([commandId, commandId, commandId, commandId, commandId, commandId]);
  } finally {
    harness.dispose();
  }
});

test("重放检查过的告警拒绝清除较早的未知尝试", async () => {
  const harness = createPendingHarness();
  try {
    const ids: string[] = [];
    expect(
      harness.origin.commands.submit(
        "alert",
        "silence",
        async (commandId) => {
          ids.push(commandId);
          return err(ids.length === 1 ? "request_timeout" : "alert_stale");
        },
        () => {},
        () => {},
      ),
    ).toBe(true);
    await settlePending();
    const commandId = ids[0];
    if (commandId === undefined) throw new Error("未生成告警命令 ID");
    expect(harness.host.entries.peek()[0]?.unknown).toBe(true);

    harness.host.retry(commandId);
    await settlePending();
    expect(ids).toEqual([commandId, commandId]);
    expect(harness.host.entries.peek()).toEqual([]);
    expect(harness.origin.rejections.peek()[0]?.result).toEqual(err("alert_stale"));
    expect(harness.origin.commands.contains("alert")).toBe(false);

    harness.origin.commands.dismiss(commandId);
    expect(harness.origin.rejections.peek()).toEqual([]);
    expect(
      harness.origin.commands.submit(
        "alert",
        "new",
        async () => ok(undefined),
        () => {},
        () => {},
      ),
    ).toBe(true);
    await settlePending();
    expect(harness.host.entries.peek()).toEqual([]);
  } finally {
    harness.dispose();
  }
});

test("新命令替换同一实体的旧拒绝通知", async () => {
  const harness = createPendingHarness();
  try {
    const failures: string[] = [];
    expect(
      harness.origin.commands.submit(
        "first",
        "old",
        async () => err("rate_limited"),
        () => {},
        (code) => failures.push(code),
      ),
    ).toBe(true);
    expect(harness.origin.commands.contains("first")).toBe(true);
    expect(harness.origin.commands.isBusy("first")).toBe(true);
    expect(
      harness.origin.commands.submit(
        "first",
        "new",
        async () => ok(undefined),
        () => {},
        () => {},
      ),
    ).toBe(false);
    expect(
      harness.origin.commands.submit(
        "other",
        "other",
        async () => err("invalid_json"),
        () => {},
        () => {},
      ),
    ).toBe(true);
    await settlePending();
    const rejected = harness.origin.rejections
      .peek()
      .find((notice) => notice.identity.entity === "first");
    const rejectedId = rejected?.identity.commandId;
    if (rejectedId === undefined) throw new Error("未保留旧拒绝");
    expect(failures).toEqual(["rate_limited"]);
    expect(harness.origin.commands.contains("first")).toBe(false);
    expect(harness.host.entries.peek()).toEqual([]);
    expect(harness.origin.rejections.peek()).toHaveLength(2);

    harness.host.retry(rejectedId);
    expect(
      harness.origin.commands.submit(
        "first",
        "new",
        async () => err("invalid_json"),
        () => {},
        () => {},
      ),
    ).toBe(true);
    await settlePending();
    expect(harness.host.entries.peek()).toEqual([]);
    const notices = harness.origin.rejections.peek();
    expect(notices).toHaveLength(2);
    expect(notices[0]?.identity.entity).toBe("other");
    expect(notices[1]?.identity.entity).toBe("first");
    expect(notices[1]?.identity.commandId).not.toBe(rejectedId);
    expect(notices[1]?.result).toEqual(err("invalid_json"));
    expect(failures).toEqual(["rate_limited"]);
  } finally {
    harness.dispose();
  }
});

test("首次确定失败可以关闭", async () => {
  for (const code of [adminApiFailureNotSentCode("network_error"), "stale_lot_state"]) {
    const harness = createPendingHarness();
    try {
      expect(
        harness.origin.commands.submit(
          "first",
          "test",
          async () => err(code),
          () => {},
          () => {},
        ),
      ).toBe(true);
      await settlePending();
      expect(harness.host.entries.peek()).toEqual([]);
      const commandId = harness.origin.rejections.peek()[0]?.identity.commandId;
      if (commandId === undefined) throw new Error("未保留首次拒绝");
      harness.origin.commands.dismiss(commandId);
      expect(harness.origin.rejections.peek()).toEqual([]);
    } finally {
      harness.dispose();
    }
  }
});

test("未发送错误以展示码交给发起页", async () => {
  const harness = createPendingHarness();
  try {
    const observed: { value: string | null } = { value: null };
    expect(
      harness.origin.commands.submit(
        "first",
        "test",
        async () => err(adminApiFailureNotSentCode("network_error")),
        () => {},
        (code) => {
          observed.value = code;
        },
      ),
    ).toBe(true);
    await settlePending();
    expect(observed.value).toBe("network_error");
    expect(harness.origin.rejections.peek()[0]?.result).toEqual(err("network_error"));
    expect(harness.origin.commands.contains("first")).toBe(false);
  } finally {
    harness.dispose();
  }
});
