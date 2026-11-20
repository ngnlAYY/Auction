// 配置页的写入候选、反馈类型与安全文案。

import type { AdminJsonValue } from "@auction/contracts/dto/admin/rules";
import { unreachable } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";

import { adminApiErrorMessage } from "../presentation";

/** 已冻结的写入候选；指纹与文档只来自确认那一刻的编辑结果。 */
export interface ConfigCommand {
  readonly fingerprint: string;
  readonly document: AdminJsonValue;
}

/** 配置页对当前草稿或读取结果的反馈。 */
export type ConfigNotice =
  | { readonly kind: "validated" }
  | { readonly kind: "saved" }
  | { readonly kind: "invalid-form" }
  | { readonly kind: "action-error"; readonly code: string }
  | { readonly kind: "conflict" };

/** 配置反馈的安全文案；错误码只经统一适配器转成可展示文本。 */
export function noticeText(notice: ConfigNotice): string {
  switch (notice.kind) {
    case "validated":
      return t("admin-runtime-validated");
    case "saved":
      return t("admin-runtime-action-success");
    case "invalid-form":
      return t("admin-runtime-invalid-form");
    case "action-error":
      return adminApiErrorMessage(notice.code);
    case "conflict":
      return t("admin-runtime-conflict");
    default:
      return unreachable(notice);
  }
}
