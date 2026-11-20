// 凭据页面的安全标签映射辅助。

import { unreachable } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";

import { adminApiErrorMessage, adminWorldLabel } from "../presentation";

/** 凭据写入反馈使用服务端的重启标志和稳定错误码。 */
export type CredentialFeedback =
  | { readonly kind: "success"; readonly restartRequired: boolean }
  | { readonly kind: "error"; readonly code: string };

/** 反馈是否表示一次成功写入；重启要求不影响成功与否。 */
export function credentialFeedbackIsSuccess(feedback: CredentialFeedback): boolean {
  return feedback.kind === "success";
}

/** 把服务端重启标志与稳定错误码映射为当前语言文案。 */
export function credentialFeedbackMessage(feedback: CredentialFeedback): string {
  switch (feedback.kind) {
    case "success":
      return feedback.restartRequired
        ? t("admin-credentials-action-restart")
        : t("admin-credentials-action-success");
    case "error":
      switch (feedback.code) {
        // 同类 active、标签、指纹和命令负载冲突共用一码，不能归到单个输入框。
        case "credential_conflict":
          return t("admin-credentials-action-conflict");
        case "stale_credential_state":
          return t("admin-credentials-action-stale");
        case "invalid_credential_label":
          return t("admin-credentials-field-label-invalid");
        default:
          return adminApiErrorMessage(feedback.code);
      }
    default:
      return unreachable(feedback);
  }
}

/** 凭据类型的展示名；未知值按原文显示。 */
export function credentialKindLabel(kind: string): string {
  switch (kind) {
    case "mail_cookie":
      return t("admin-credentials-kind-mail");
    case "forum_cookie":
      return t("admin-credentials-kind-forum");
    default:
      return kind;
  }
}

/** 凭据作用域除业务世界外还有跨世界共用的 `shared`；业务世界名统一委托
 * {@link adminWorldLabel}。 */
export function credentialWorldLabel(world: string): string {
  switch (world) {
    case "shared":
      return t("admin-credentials-world-shared");
    default:
      return adminWorldLabel(world);
  }
}

/** 凭据状态的展示名；未知值按原文显示。 */
export function credentialStatusLabel(status: string): string {
  switch (status) {
    case "active":
      return t("admin-credentials-status-active");
    case "disabled":
      return t("admin-credentials-status-disabled");
    case "invalid":
      return t("admin-credentials-status-invalid");
    default:
      return status;
  }
}

/** 凭据状态单元格的类名；未知状态只保留基础类。 */
export function credentialStatusClass(status: string): string {
  switch (status) {
    case "active":
      return "admin-credentials__status admin-credentials__status--active";
    case "disabled":
      return "admin-credentials__status admin-credentials__status--disabled";
    case "invalid":
      return "admin-credentials__status admin-credentials__status--invalid";
    default:
      return "admin-credentials__status";
  }
}
