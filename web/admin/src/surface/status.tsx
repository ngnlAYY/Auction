// Admin 服务状态说明页。
// 旧版此路由是静态产品说明，不读取 live/ready；运行诊断继续由 Dashboard 和运维端点拥有。

import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

/** 渲染旧版静态状态说明。 */
export function page(): JSX.Element {
  return (
    <section aria-labelledby="admin-status-title">
      <h1 id="admin-status-title">{t("page-status-title")}</h1>
      <p>{t("page-status-description")}</p>
    </section>
  );
}
