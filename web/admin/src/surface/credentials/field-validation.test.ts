// 凭据各文本控件的提交规则和 UTF-8 字节上限。

import { expect, test } from "bun:test";
import { credentialFieldIssue } from "./field-validation";

test("each_text_field_uses_its_own_submission_rules", () => {
  for (const [field, valid, invalid, issue] of [
    ["label", "账号一", " ", "required"],
    ["label", "账号一", " 账号一", "label_invalid"],
    ["player_name", "", " ", "player_name_invalid"],
    ["player_name", "Player Name", "Player Name\n", "player_name_invalid"],
    ["secret", "example=value", " \u{3000}", "required"],
    ["username", "Forum User", "Forum User ", "username_invalid"],
    ["password", " Password ", "", "required"],
    ["password", "Password", "bad\npassword", "password_invalid"],
  ] as const) {
    expect(credentialFieldIssue(field, valid)).toBeNull();
    expect(credentialFieldIssue(field, invalid)).toBe(issue);
  }
  expect(credentialFieldIssue("label", "名".repeat(86))).toBe("label_invalid");
  expect(credentialFieldIssue("username", "名".repeat(86))).toBe("username_invalid");
  expect(credentialFieldIssue("password", "密".repeat(1366))).toBe("password_invalid");
});
