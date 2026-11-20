// 运行配置页面：草稿、校验、保存与事实展示。

import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

import { RuntimeConfigDocument } from "./document";
import { MailTemplateCard } from "./mail-templates";
import { RuntimeSwitchCard } from "./runtime-switches";
import { AdminBusinessWorlds } from "./worlds";

export { useConfigDraft } from "./draft";
export { type ConfigCommand, type ConfigNotice, noticeText } from "./notice";

/** 加载并展示 Admin runtime 非敏感配置。 */
export function page(): JSX.Element {
  return (
    <section class="admin-config-page" aria-labelledby="config-title">
      <h1 id="config-title">{t("page-config-title")}</h1>
      <p class="admin-config-intro">{t("admin-runtime-note")}</p>
      <AdminBusinessWorlds />
      <RuntimeSwitchCard />
      <MailTemplateCard />
      <RuntimeConfigDocument />
    </section>
  );
}
