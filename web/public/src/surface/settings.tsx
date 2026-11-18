//! Public 设置页：语言与主题两项只保存在本机的偏好。
//! Solid 版专属（Rust 版没有这个页面）：手机顶栏放不下这两个切换器，入口在手机导航里；
//! 桌面端顶栏仍保留同一组切换器，两处绑定同一份状态，所以这里的控件另取 id。

import { ThemePicker } from "@auction/web-shared/platform/theme";
import { LanguagePicker, t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

import { PUBLIC_LOCALE_CATALOG } from "./router/locale-catalog";

export function page(): JSX.Element {
  return (
    <section class="public-settings-page" aria-labelledby="public-settings-title">
      <h1 id="public-settings-title" class="public-settings-page__title">
        {t("page-settings-title")}
      </h1>
      <div class="public-settings-page__list">
        <LanguagePicker
          id="settings-language"
          label={t("language-label")}
          catalog={PUBLIC_LOCALE_CATALOG}
        />
        <ThemePicker
          id="settings-theme"
          label={t("theme-label")}
          darkLabel={t("theme-dark")}
          lightLabel={t("theme-light")}
        />
      </div>
      <p class="public-settings-page__hint">{t("settings-local-hint")}</p>
    </section>
  );
}
