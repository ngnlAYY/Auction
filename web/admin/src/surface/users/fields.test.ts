// 用户名、搜索、论坛账号、手动 Key 与拉黑理由的服务端边界。

import { expect, test } from "bun:test";
import {
  blacklistReasonIssue,
  forumAccountIssue,
  manualKeyIssue,
  searchNameIssue,
  usernameIssue,
} from "./fields";

test("name_and_search_match_server_byte_and_single_line_rules", () => {
  const limit = `${"界".repeat(85)}x`;
  expect(usernameIssue(limit)).toBeNull();
  expect(searchNameIssue(limit)).toBeNull();
  expect(searchNameIssue("")).toBeNull();
  expect(usernameIssue("")).toBe("required");
  for (const value of [`${limit}x`, " 玩家", "玩\u{85}家"]) {
    expect(usernameIssue(value)).toBe("name");
    expect(searchNameIssue(value)).toBe("name");
  }
});

test("account_id_distinguishes_optional_empty_from_invalid_ascii_decimal", () => {
  expect(forumAccountIssue("", false)).toBeNull();
  expect(forumAccountIssue("", true)).toBe("required");
  expect(forumAccountIssue("1".repeat(32), true)).toBeNull();
  for (const value of ["1".repeat(33), "01", "１", "1 "]) {
    expect(forumAccountIssue(value, false)).toBe("forum_account");
  }
});

test("manual_key_counts_bytes_and_keeps_intentional_spaces", () => {
  for (const value of ["界".repeat(11), " ".repeat(32), "x".repeat(256)]) {
    expect(manualKeyIssue(value)).toBeNull();
  }
  for (const value of ["界".repeat(10), "x".repeat(257), `${"x".repeat(32)}\t`]) {
    expect(manualKeyIssue(value)).toBe("manual_key");
  }
});

test("blacklist_reason_accepts_1024_bytes_with_no_controls_or_edge_spaces", () => {
  const limit = `${"界".repeat(341)}x`;
  expect(blacklistReasonIssue(limit)).toBeNull();
  expect(blacklistReasonIssue("x".repeat(513))).toBeNull();
  for (const value of [`${limit}x`, "one\ntwo", "one "]) {
    expect(blacklistReasonIssue(value)).toBe("blacklist_reason");
  }
});
