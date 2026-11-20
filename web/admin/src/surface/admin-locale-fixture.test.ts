// Admin 测试的独立 Fluent bundle 与可恢复翻译入口。

import { expect, type Mock, spyOn, test } from "bun:test";
import { readFileSync } from "node:fs";
import { buildBundle, DEFAULT_ADMIN_LOCALE, type LocaleCode } from "@auction/web-shared/ui/i18n";
import {
  type LocaleSession,
  type t,
  withoutFluentIsolation,
} from "@auction/web-shared/ui/i18n/session";
import type { FluentBundle } from "@fluent/bundle";

const I18N_ROOT = new URL("../../../../crates/auction-web-admin/i18n/", import.meta.url);
const translations = await import("@auction/web-shared/ui/i18n/session");
const translationLayers: { readonly translate: typeof t }[] = [];
let translationSpy: Mock<typeof t> | null = null;

/** 译文固定为默认语言时使用的只读会话。 */
export const adminLocaleSession: LocaleSession = Object.freeze({
  current: () => DEFAULT_ADMIN_LOCALE,
  selected: () => DEFAULT_ADMIN_LOCALE,
  isReady: () => true,
  error: () => null,
  bootstrap() {},
  switch() {},
});

/** 读取一门 Admin 译文的原文。 */
export function adminFtl(locale: string): string {
  return readFileSync(new URL(`${locale}.ftl`, I18N_ROOT), "utf8");
}

/** 构造指定语言的 Fluent bundle；默认语言缺失时直接让测试失败。 */
export function adminBundle(locale: LocaleCode): FluentBundle {
  const built = buildBundle(
    DEFAULT_ADMIN_LOCALE,
    adminFtl(DEFAULT_ADMIN_LOCALE),
    locale,
    locale === DEFAULT_ADMIN_LOCALE ? null : adminFtl(locale),
  );
  if (!built.ok) {
    throw new Error(`无法装配 ${locale}：${built.error}`);
  }
  return built.value;
}

/** 从独立 bundle 格式化仓库译文，回调可随本用例的语言状态更新。 */
export function createAdminTranslator(current = (): LocaleCode => DEFAULT_ADMIN_LOCALE): typeof t {
  const bundles = new Map<LocaleCode, FluentBundle>();
  return (id, args) => {
    const locale = current();
    let bundle = bundles.get(locale);
    if (bundle === undefined) {
      bundle = adminBundle(locale);
      bundles.set(locale, bundle);
    }
    const pattern = bundle.getMessage(id)?.value;
    if (pattern == null) throw new Error(`缺少译文 ${locale} ${id}`);
    const variables = Object.fromEntries(
      Object.entries(args ?? {}).map(([key, value]) => [key, String(value)]),
    );
    const errors: Error[] = [];
    const text = bundle.formatPattern(pattern, variables, errors);
    if (errors[0] !== undefined) throw errors[0];
    return withoutFluentIsolation(text);
  };
}

/** 让最后安装且未释放的译文生效，最后一层释放时恢复生产入口。 */
export function installAdminTranslation(translate = createAdminTranslator()): () => void {
  const replacement = translationSpy ?? spyOn(translations, "t");
  replacement.mockImplementation(translate);
  translationSpy = replacement;
  const layer = { translate };
  translationLayers.push(layer);
  return () => {
    const index = translationLayers.indexOf(layer);
    if (index < 0) return;
    translationLayers.splice(index, 1);
    const current = translationLayers.at(-1);
    if (current !== undefined) {
      replacement.mockImplementation(current.translate);
    } else {
      replacement.mockRestore();
      translationSpy = null;
    }
  };
}

test("内层译文释放后恢复外层，最后恢复安装前的函数", () => {
  const original = translations.t;
  const restoreOuter = installAdminTranslation(() => "outer");
  try {
    const restoreInner = installAdminTranslation(() => "inner");
    try {
      expect(translations.t("value-empty")).toBe("inner");
    } finally {
      restoreInner();
    }
    expect(translations.t("value-empty")).toBe("outer");
  } finally {
    restoreOuter();
  }
  expect(translations.t).toBe(original);
});

test("先释放外层译文时仍使用内层，重复释放不影响当前替身", () => {
  const original = translations.t;
  const restoreOuter = installAdminTranslation(() => "outer");
  const restoreInner = installAdminTranslation(() => "inner");
  try {
    restoreOuter();
    restoreOuter();
    expect(translations.t("value-empty")).toBe("inner");
  } finally {
    restoreInner();
    restoreOuter();
  }
  expect(translations.t).toBe(original);
});
