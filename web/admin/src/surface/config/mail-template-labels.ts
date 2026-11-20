// 邮件模板卡片的占位符、问题提示与模板名称文案。

import { unreachable } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";

import {
  type MailTemplateBodyProblem,
  NOTIFICATION_IDENTITY,
} from "../../api/notification-templates";

/** 占位符按正文里的写法展示（带花括号）；没有时显示占位符号。 */
export function placeholderList(names: readonly string[]): string {
  if (names.length === 0) {
    return "—";
  }
  return names.map((name) => `{${name}}`).join(" ");
}

/** 浏览器即时校验问题对应的说明文案。 */
export function problemMessage(problem: MailTemplateBodyProblem): string {
  switch (problem) {
    case "format":
      return t("admin-mail-template-problem-format");
    case "unclosed":
      return t("admin-mail-template-problem-unclosed");
    case "unknown_placeholder":
      return t("admin-mail-template-problem-unknown");
    case "missing_placeholder":
      return t("admin-mail-template-problem-missing");
    case "missing_rejection_reason":
      return t("admin-mail-template-reason-required", {
        code: "{reason_code}",
        text: "{reason_text}",
      });
    case "missing_notification_identity":
      return t("admin-mail-template-notification-identity", {
        sentence: NOTIFICATION_IDENTITY,
      });
    case "watch_body_too_long":
      return t("admin-mail-template-watch-limit");
    case "request_too_large":
      return t("admin-error-body-too-large");
    default:
      return unreachable(problem);
  }
}

/** 模板键到界面名称；服务端新增而本页尚不认识的键直接显示键名。 */
export function templateLabel(key: string): string {
  switch (key) {
    case "auction_assets_rejected":
      return t("admin-runtime-template-auction-assets");
    case "auction_no_attachments":
      return t("admin-runtime-template-no-attachments");
    case "auction_equipment_rejected":
      return t("admin-runtime-template-equipment-rejected");
    case "user_assets_rejected":
      return t("admin-runtime-template-user-assets");
    case "register_key":
      return t("admin-runtime-template-register-key");
    case "register_result":
      return t("admin-runtime-template-register-result");
    case "rebind_result":
      return t("admin-runtime-template-rebind-result");
    case "rekey_key":
      return t("admin-runtime-template-rekey-key");
    case "rekey_result":
      return t("admin-runtime-template-rekey-result");
    case "settlement_winner_equipment_cod":
      return t("admin-runtime-template-winner-cod");
    case "settlement_payment_carrier":
      return t("admin-runtime-template-payment-carrier");
    case "settlement_payment_reminder":
      return t("admin-runtime-template-payment-reminder");
    case "settlement_equipment_handoff":
      return t("admin-runtime-template-equipment-handoff");
    case "settlement_seller_payout":
      return t("admin-runtime-template-seller-payout");
    case "settlement_buyer_refund":
      return t("admin-runtime-template-buyer-refund");
    case "lot_watch":
      return t("admin-runtime-template-lot-watch");
    case "lot_watch_pm":
      return t("admin-runtime-template-lot-watch-pm");
    case "operations_alert":
      return t("admin-runtime-template-operations-alert");
    default:
      return key;
  }
}
