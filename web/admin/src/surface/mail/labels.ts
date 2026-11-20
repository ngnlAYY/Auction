// Admin 邮件入站列表的状态和闭集标签映射。

import { t } from "@auction/web-shared/ui/i18n";

/** 列表状态对应的徽标类名；未知状态按待处理样式显示。 */
export function mailStatusClass(status: string): string {
  switch (status) {
    case "accepted":
      return "admin-mail__status admin-mail__status--accepted";
    case "failed":
      return "admin-mail__status admin-mail__status--failed";
    default:
      return "admin-mail__status admin-mail__status--pending";
  }
}

/** 邮件类别的中文标签；未知值原样显示。 */
export function mailKindLabel(kind: string): string {
  switch (kind) {
    case "equipment":
      return t("admin-mail-kind-equipment");
    case "bid":
      return t("admin-mail-kind-bid");
    case "delivery":
      return t("admin-mail-kind-delivery");
    case "registration":
      return t("admin-mail-kind-registration");
    case "rebind":
      return t("admin-mail-kind-rebind");
    case "rekey":
      return t("admin-mail-kind-rekey");
    case "unknown":
      return t("admin-mail-kind-unknown");
    default:
      return kind;
  }
}

/** 邮件处理状态的中文标签；未知值原样显示。 */
export function mailStatusLabel(status: string): string {
  switch (status) {
    case "accepted":
      return t("admin-mail-status-accepted");
    case "rejected":
      return t("admin-mail-status-rejected");
    case "failed":
      return t("admin-mail-status-failed");
    default:
      return status;
  }
}
