// 凭据写入读出真实默认译文，并区分冲突与旧版本。

import { expect, spyOn, test } from "bun:test";
import { createAdminTranslator } from "../admin-locale-fixture.test";
import { credentialFeedbackMessage } from "./labels";

test("writes_report_restart_from_the_server_flag", async () => {
  const i18n = await import("@auction/web-shared/ui/i18n");
  const translate = createAdminTranslator();
  let restore: (() => void) | undefined;
  try {
    const spy = spyOn(i18n, "t");
    restore = () => spy.mockRestore();
    spy.mockImplementation(translate);
    expect(credentialFeedbackMessage({ kind: "success", restartRequired: true })).toBe(
      "操作已完成。请重启 Worker 以应用凭据变更。",
    );
    expect(credentialFeedbackMessage({ kind: "success", restartRequired: false })).toBe(
      "操作已完成，无需重启 Worker。",
    );
    expect(spy.mock.calls.map(([key]) => key)).toEqual([
      "admin-credentials-action-restart",
      "admin-credentials-action-success",
    ]);
  } finally {
    restore?.();
  }
});

test("credential_conflict_and_stale_version_receive_distinct_feedback", async () => {
  const i18n = await import("@auction/web-shared/ui/i18n");
  const translate = createAdminTranslator();
  let restore: (() => void) | undefined;
  try {
    const spy = spyOn(i18n, "t");
    restore = () => spy.mockRestore();
    spy.mockImplementation(translate);
    expect(credentialFeedbackMessage({ kind: "error", code: "credential_conflict" })).toBe(
      "凭据冲突。每种 Cookie 在所有世界中合计只能有一条 active 凭据。标签重复、Cookie 指纹重复或命令负载冲突也会触发此错误。请核对当前凭据。",
    );
    expect(credentialFeedbackMessage({ kind: "error", code: "stale_credential_state" })).toBe(
      "凭据状态版本已变化，请重新读取列表后再决定如何操作。",
    );
    expect(spy.mock.calls.map(([key]) => key)).toEqual([
      "admin-credentials-action-conflict",
      "admin-credentials-action-stale",
    ]);
  } finally {
    restore?.();
  }
});
