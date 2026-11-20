// 规则草稿与反馈。

import { unreachable } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";

import type { AdminRules, AdminRulesCandidate } from "../../api/rules";
import { adminApiErrorMessage } from "../presentation";
import type { RuleNotice } from "./model";

export const DEFAULT_RULES_LOCALE = "en";

export function noticeText(notice: RuleNotice): string {
  switch (notice.kind) {
    case "validated":
      return t("admin-rules-validated");
    case "published":
      return t("admin-rules-success");
    case "action-error":
      return adminApiErrorMessage(notice.code);
    case "conflict":
      return t("admin-rules-conflict");
    default:
      return unreachable(notice);
  }
}

export function toDraft(document: AdminRules, requestedLocale: string): AdminRulesCandidate {
  return {
    locale: requestedLocale,
    title: document.title,
    markdown: document.markdown,
  };
}

export function missingRulesDocument(locale: string): AdminRules {
  return {
    locale,
    title: "",
    markdown: "",
    published_by: t("value-empty"),
    revision: 0,
    published_at: "",
  };
}
