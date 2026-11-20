// 首次部署输入校验：管理员文本、确认目标撤销与数据库地址合法性回归。

import { describe, expect, test } from "bun:test";

import {
  createSetupDraft,
  type SetupDraft,
  type SetupField,
  setupDraftSet,
  setupDraftValue,
} from "./draft";
import { databaseTarget, issue } from "./validation";

function confirmed(draft: SetupDraft): SetupDraft {
  return { ...draft, confirmedDatabaseTarget: databaseTarget(draft) };
}

describe("首次部署输入校验（对应 Rust surface/setup/validation.rs）", () => {
  test("setup_admin_actor_rejects_noncanonical_text_without_changing_the_input", () => {
    let draft = createSetupDraft();
    const field: SetupField = "admin_actor";
    for (const valid of ["管理 员", "é".repeat(64)]) {
      draft = setupDraftSet(draft, field, valid);
      expect(issue(draft, field)).toBeNull();
      expect(setupDraftValue(draft, field)).toBe(valid);
    }
    for (const invalid of [
      " admin",
      "admin ",
      "\u2003admin",
      "a\tmin",
      "a\u0085min",
      "admin:owner",
      "é".repeat(65),
      "",
    ]) {
      draft = setupDraftSet(draft, field, invalid);
      expect(issue(draft, field)).toBe("setup-admin-actor-invalid");
      expect(setupDraftValue(draft, field)).toBe(invalid);
    }
  });

  test("setup_confirmation_is_revoked_on_each_target_edit", () => {
    for (const [field, edited] of [
      ["database_host", "::1"],
      ["database_port", "5433"],
      ["database_port", "05432"],
      ["database_name", "other"],
    ] as const) {
      let draft = createSetupDraft();
      const original = setupDraftValue(draft, field);
      draft = confirmed(draft);
      expect(draft.confirmedDatabaseTarget).not.toBeNull();
      draft = setupDraftSet(draft, "database_password", "changed-password");
      draft = setupDraftSet(draft, field, original);
      expect(draft.confirmedDatabaseTarget).not.toBeNull();
      draft = setupDraftSet(draft, field, edited);
      expect(draft.confirmedDatabaseTarget).toBeNull();
      draft = setupDraftSet(draft, field, original);
      expect(draft.confirmedDatabaseTarget).toBeNull();
      expect(databaseTarget(draft)).not.toBeNull();
    }
  });

  test("setup_confirmation_requires_a_valid_host_port_and_name", () => {
    for (const [field, invalid] of [
      ["database_host", "https://database.example.test"],
      ["database_port", "0"],
      ["database_name", ".."],
    ] as const) {
      let draft = createSetupDraft();
      expect(databaseTarget(draft)).not.toBeNull();
      draft = setupDraftSet(draft, field, invalid);
      expect(databaseTarget(draft)).toBeNull();
    }
  });
});
