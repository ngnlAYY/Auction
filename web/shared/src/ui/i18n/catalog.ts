// 语言目录、受支持语言顺序与 `hv.lang` 偏好读取。
// 已保存偏好依赖固定的存储键；存储值非法时由调用方回退各 surface 的默认语言。

import { getItem } from "../../platform/storage";

/** 已发布的语言偏好存储键；用户浏览器里已保存的偏好依赖该键，不得重命名。 */
export const LANGUAGE_STORAGE_KEY = "hv.lang";

/** 受支持的 BCP 47 语言标记。 */
export type LocaleCode = "en" | "zh-CN" | "zh-TW" | "ja" | "ko";

/** 一个受支持的界面语言及其自身名称。 */
export interface SupportedLocale {
  readonly code: LocaleCode;
  readonly label: string;
}

/** 受支持语言集合，顺序即语言切换器的显示顺序；简体与繁体互不回退。 */
export const SUPPORTED_LOCALES = [
  { code: "en", label: "English" },
  { code: "zh-CN", label: "简体中文" },
  { code: "zh-TW", label: "繁體中文" },
  { code: "ja", label: "日本語" },
  { code: "ko", label: "한국어" },
] as const satisfies readonly SupportedLocale[];

export const DEFAULT_PUBLIC_LOCALE: LocaleCode = "en";
export const DEFAULT_ADMIN_LOCALE: LocaleCode = "zh-CN";

/** 一个非默认语言及其同源资源 URL；URL 由各 surface 在构建期生成。 */
export interface LocaleSource {
  readonly id: LocaleCode;
  readonly url: string;
}

/** surface 的语言目录：默认语言文本编译期传入，其余语言按需取回。 */
export interface LocaleCatalog {
  readonly defaultLocale: LocaleCode;
  readonly defaultFtl: string;
  readonly sources: readonly LocaleSource[];
}

/** 只接受 `SUPPORTED_LOCALES` 中的取值；非法值由调用方回退默认语言。 */
export function parseSupported(code: string): LocaleCode | null {
  for (const locale of SUPPORTED_LOCALES) {
    if (locale.code === code) {
      return locale.code;
    }
  }
  return null;
}

/** 读取浏览器已保存偏好；存储不可用或值非法时返回 `null`。 */
export function storedLanguage(): LocaleCode | null {
  const stored = getItem(LANGUAGE_STORAGE_KEY);
  return stored === null ? null : parseSupported(stored);
}

/** 决定初始语言：优先使用已保存偏好，否则使用该 surface 的默认语言。 */
export function initialLanguage(defaultLocale: LocaleCode): LocaleCode {
  return storedLanguage() ?? defaultLocale;
}
