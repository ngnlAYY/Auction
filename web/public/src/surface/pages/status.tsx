// Public 静态服务说明页。
// 旧版 `/status` 只说明 Public surface 的用途；健康检查和实时事件仍由机器 API 与页头连接
// 状态承担，不在这里重复暴露诊断表或建立第二条订阅。

import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

/** Public 服务说明页。 */
export function page(): JSX.Element {
  return (
    <section class="public-status-page" aria-label={t("page-status-title")}>
      <p>{t("page-status-note")}</p>
    </section>
  );
}
