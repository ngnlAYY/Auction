// 管理端错误文案按稳定码与阶段前缀选择译键的回归。

import { describe, expect, spyOn, test } from "bun:test";
import { createAdminTranslator } from "./admin-locale-fixture.test";
import { adminApiErrorMessage } from "./presentation";

describe("管理端错误文案（对应 Rust surface/presentation.rs）", () => {
  test("error_copy_uses_the_stable_code_for_each_prefix", async () => {
    const i18n = await import("@auction/web-shared/ui/i18n/session");
    const translate = spyOn(i18n, "t").mockImplementation(createAdminTranslator());
    try {
      expect(adminApiErrorMessage("not_sent:request_timeout")).toBe(
        "请求超时，服务端结果可能尚未返回。表单内容已保留，请在当前页面再次提交；相同命令不会重复录入。",
      );
      expect(adminApiErrorMessage("write_endpoint:rules-publish:409:state_conflict")).toBe(
        "数据已变化，或本次命令与已有请求冲突。请重新加载后再试。",
      );
      expect(adminApiErrorMessage("invalid_mail_template")).toBe(
        "模板正文未通过校验。请检查必需与允许的占位符。",
      );
      expect(translate.mock.calls.map(([id]) => id)).toEqual([
        "admin-error-request-timeout",
        "admin-error-conflict",
        "admin-error-invalid-mail-template",
      ]);
    } finally {
      translate.mockRestore();
    }
  });
});
