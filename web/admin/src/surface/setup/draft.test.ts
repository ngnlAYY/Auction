// 首次部署草稿生成、替换与清除管理员随机密钥的回归。

import { describe, expect, test } from "bun:test";
import { utf8ByteLength } from "@auction/web-shared/text";

import {
  createSetupDraft,
  setupDraftClearPasswords,
  setupDraftGenerateAdminSecret,
  setupDraftResetAdvanced,
  setupDraftValue,
} from "./draft";

describe("首次部署草稿（对应 Rust surface/setup/draft.rs）", () => {
  test("setup_generates_and_replaces_random_admin_secret", () => {
    let draft = createSetupDraft();
    const original = setupDraftValue(draft, "admin_secret");
    expect(utf8ByteLength(original)).toBe(64);
    expect(original).toMatch(/^[0-9a-fA-F]+$/u);
    expect(setupDraftValue(draft, "admin_confirmation")).toBe(original);

    draft = setupDraftResetAdvanced(draft);
    expect(setupDraftValue(draft, "admin_secret")).toBe(original);

    draft = setupDraftGenerateAdminSecret(draft);
    expect(setupDraftValue(draft, "admin_secret")).not.toBe(original);
    expect(setupDraftValue(draft, "admin_confirmation")).toBe(
      setupDraftValue(draft, "admin_secret"),
    );

    draft = setupDraftClearPasswords(draft);
    expect(setupDraftValue(draft, "admin_secret")).toBe("");
    expect(setupDraftValue(draft, "admin_confirmation")).toBe("");
  });
});
