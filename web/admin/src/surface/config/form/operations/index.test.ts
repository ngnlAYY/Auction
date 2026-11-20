// 独立装配五套真实译文，核对受管分组的热读说明。

import { describe, expect, test } from "bun:test";
import type { LocaleCode } from "@auction/web-shared/ui/i18n";
import type { FluentBundle, FluentVariable } from "@fluent/bundle";

import { adminBundle } from "../../../admin-locale-fixture.test";

function format(bundle: FluentBundle, key: string, args?: Record<string, FluentVariable>): string {
  const message = bundle.getMessage(key);
  expect(message?.value).toBeDefined();
  const errors: Error[] = [];
  const pattern = message?.value;
  if (pattern === null || pattern === undefined) {
    throw new Error(`缺少译文 ${key}`);
  }
  const formatted = bundle.formatPattern(pattern, args ?? null, errors);
  expect(errors).toEqual([]);
  return formatted;
}

describe("operational fields", () => {
  test("managed_group_note_uses_the_conditional_hot_reload_copy_in_every_locale", () => {
    const locales: readonly LocaleCode[] = ["zh-CN", "en", "zh-TW", "ja", "ko"];
    for (const locale of locales) {
      const bundle = adminBundle(locale);
      const conditional = format(bundle, "admin-runtime-hot-fields-note");
      const managed = format(bundle, "admin-runtime-op-managed-note", {
        hot_reload_note: conditional,
      });
      expect(managed).toContain(conditional);
    }
  });
});
