//! Public 语言目录：默认英文编译期内嵌，其余四种从带指纹的静态资源按需取回。

import { DEFAULT_PUBLIC_LOCALE, type LocaleCatalog } from "@auction/web-shared/ui/i18n";

import en from "../../../../../crates/auction-web-public/i18n/en.ftl?raw";
import jaUrl from "../../../../../crates/auction-web-public/i18n/ja.ftl?url";
import koUrl from "../../../../../crates/auction-web-public/i18n/ko.ftl?url";
import zhCNUrl from "../../../../../crates/auction-web-public/i18n/zh-CN.ftl?url";
import zhTWUrl from "../../../../../crates/auction-web-public/i18n/zh-TW.ftl?url";

export const PUBLIC_LOCALE_CATALOG: LocaleCatalog = {
  defaultLocale: DEFAULT_PUBLIC_LOCALE,
  defaultFtl: en,
  sources: [
    { id: "zh-CN", url: zhCNUrl },
    { id: "zh-TW", url: zhTWUrl },
    { id: "ja", url: jaUrl },
    { id: "ko", url: koUrl },
  ],
};
