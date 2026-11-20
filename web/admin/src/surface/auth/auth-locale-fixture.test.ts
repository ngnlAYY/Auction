// 认证测试使用真实默认译文，并在用例退出时恢复翻译入口。

import { expect, test } from "bun:test";
import { adminLocaleSession, installAdminTranslation } from "../admin-locale-fixture.test";

/** 装配认证外壳使用的默认语言会话及译文替身。 */
export function installAuthLocale() {
  return { session: adminLocaleSession, restore: installAdminTranslation() };
}

test("认证语言夹具提供默认语言并恢复进入时的翻译函数", async () => {
  const translations = await import("@auction/web-shared/ui/i18n/session");
  const original = translations.t;
  const locale = installAuthLocale();
  try {
    expect(locale.session.current()).toBe("zh-CN");
    expect(translations.t("value-empty")).toBe("—");
  } finally {
    locale.restore();
  }
  expect(translations.t).toBe(original);
});
