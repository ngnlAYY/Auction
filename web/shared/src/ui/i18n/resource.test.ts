// i18n bundle 构造与 FTL 平铺子集校验的回归用例，逐条对照 Rust `ui::i18n` 的测试。

import { expect, test } from "bun:test";
import type { FluentBundle } from "@fluent/bundle";
import { buildBundle } from "./resource";

const DEFAULT_FTL = "greeting = Hello\nfallback-only = Default fallback\n";

function translate(bundle: FluentBundle, id: string): string {
  const message = bundle.getMessage(id);
  if (message === undefined || message.value === null) {
    throw new Error(`message ${id} missing`);
  }
  const errors: Error[] = [];
  const text = bundle.formatPattern(message.value, null, errors);
  expect(errors).toEqual([]);
  return text;
}

test("target_overrides_and_falls_back_to_default：目标语言覆盖同名键并回退默认资源", () => {
  const bundle = buildBundle("en", DEFAULT_FTL, "zh-CN", "greeting = 你好\n");
  if (!bundle.ok) {
    throw new Error(`expected valid bundle: ${bundle.error}`);
  }
  expect(translate(bundle.value, "greeting")).toBe("你好");
  expect(translate(bundle.value, "fallback-only")).toBe("Default fallback");
});

test("default_language_ignores_extra_resource：目标语言等于默认语言时忽略额外资源", () => {
  const bundle = buildBundle("en", DEFAULT_FTL, "en", "greeting = 你好\n");
  if (!bundle.ok) {
    throw new Error(`expected valid bundle: ${bundle.error}`);
  }
  expect(translate(bundle.value, "greeting")).toBe("Hello");
});

test("invalid_ftl_is_rejected：非法 FTL 整份拒绝", () => {
  expect(buildBundle("en", DEFAULT_FTL, "ja", "= broken\n").ok).toBe(false);
  expect(buildBundle("en", "= broken\n", "en", null).ok).toBe(false);
});
