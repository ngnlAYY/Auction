// 登录失败的本地化文案与黑名单附加原因。

import { type BaseErrorKind, baseApiErrorMessage } from "@auction/web-shared/net/errors";
import { t } from "@auction/web-shared/ui/i18n";

import type { PublicSessionApiError } from "../../api/session";

/** 先匹配登录业务码，未命中走共享基础层。 */
export function loginErrorMessage(error: PublicSessionApiError): string {
  switch (error.code) {
    case "empty_key":
    case "invalid_key":
      return t("common-error");
    case "invalid_credentials":
      return t("login-invalid-credentials");
    case "account_blacklisted":
      if (error.status === 403) {
        return t("login-blacklisted");
      }
      break;
    case "session_limit":
      return t("login-session-limit");
    case "session_lock_unavailable":
      return t("session-lock-unavailable");
    case "session_lock_timeout":
      return t("session-lock-timeout");
    default:
      break;
  }
  const code = error.code;
  if (code === null) {
    return t("common-error");
  }
  return (
    baseApiErrorMessage(code, (kind: BaseErrorKind) => {
      if (kind === "network") {
        return t("login-network-error");
      }
      if (kind === "timeout") {
        return t("login-request-timeout");
      }
      if (kind === "invalid_response") {
        return t("login-invalid-response");
      }
      return t("common-error");
    }) ?? t("common-error")
  );
}

export function loginBlacklistedReason(error: PublicSessionApiError): string | null {
  return error.status === 403 && error.code === "account_blacklisted" ? error.publicReason : null;
}
