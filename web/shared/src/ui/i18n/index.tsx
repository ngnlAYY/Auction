// Public 与 Admin 共用的语言目录、Fluent 会话、时间格式与语言切换器。
// `hv.lang` 与 `<html lang>` 是已发布的浏览器契约；存储值非法时回退各 surface 的默认语言。

import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import { type LocaleCatalog, SUPPORTED_LOCALES } from "./catalog";
import { t, useLocaleSession } from "./session";

export {
  DEFAULT_ADMIN_LOCALE,
  DEFAULT_PUBLIC_LOCALE,
  initialLanguage,
  LANGUAGE_STORAGE_KEY,
  type LocaleCatalog,
  type LocaleCode,
  type LocaleSource,
  parseSupported,
  SUPPORTED_LOCALES,
  type SupportedLocale,
  storedLanguage,
} from "./catalog";
export { buildBundle } from "./resource";
export {
  createLocaleSession,
  currentLanguage,
  type LocaleError,
  type LocaleSession,
  LocaleSessionContext,
  syncDocumentLanguage,
  t,
  translationFailed,
  useLocaleEffects,
  useLocaleSession,
  withoutFluentIsolation,
} from "./session";
export { formatLocalizedTimeForLanguage } from "./time";

/** 原生 `<select>` 自带键盘、屏幕阅读器语义与移动端选择面板。 */
export function LanguagePicker(props: { label: string; catalog: LocaleCatalog }): JSX.Element {
  const session = useLocaleSession();
  return (
    <>
      <label class="language-picker">
        <span class="language-picker__label">{props.label}</span>
        <select
          id="language"
          name="language"
          class="language-picker__select"
          value={session.current()}
          onChange={(event) => session.switch(props.catalog, event.currentTarget.value)}
        >
          <For each={SUPPORTED_LOCALES} keyed={false}>
            {(locale) => (
              <option value={locale().code} selected={session.current() === locale().code}>
                {locale().label}
              </option>
            )}
          </For>
        </select>
      </label>
      <Show when={session.error() !== null}>
        <p class="language-picker__error" role="alert">
          {t("language-switch-failed")}
        </p>
      </Show>
    </>
  );
}
