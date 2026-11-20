// Admin 用户字段校验与字段级错误展示。

import { unreachable } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import { validSafeText } from "../../api/validation";

/** 用户名的字节上限；服务端按 UTF-8 字节计。 */
export const USER_NAME_BYTES = 256;

/** 用户表单可归属到具体字段的本地校验问题。 */
export type UserFieldIssue =
  | "required"
  | "name"
  | "forum_account"
  | "manual_key"
  | "blacklist_reason"
  | "username_conflict";

/** 字段问题对应的界面文案键。 */
export function userFieldIssueMessage(issue: UserFieldIssue): string {
  switch (issue) {
    case "required":
      return t("admin-field-required");
    case "name":
      return t("admin-users-field-username-invalid");
    case "forum_account":
      return t("admin-users-field-forum-account-id-invalid");
    case "manual_key":
      return t("admin-users-field-manual-key-short");
    case "blacklist_reason":
      return t("admin-users-field-blacklist-reason-invalid");
    case "username_conflict":
      return t("admin-users-field-username-conflict");
    default:
      return unreachable(issue);
  }
}

/** 必填用户名：空值优先报 required，再检查安全文本与字节上限。 */
export function usernameIssue(value: string): UserFieldIssue | null {
  return requiredTextIssue(value, USER_NAME_BYTES, "name");
}

/** 搜索框可留空；只有非空且不合规时才报字段错误。 */
export function searchNameIssue(value: string): UserFieldIssue | null {
  return value !== "" && !validSafeText(value, USER_NAME_BYTES) ? "name" : null;
}

/** 论坛账号只接受 ASCII 数字；空值是否合法由调用方决定。 */
export function forumAccountIssue(value: string, required: boolean): UserFieldIssue | null {
  if (value === "") {
    return required ? "required" : null;
  }
  // Rust 按 UTF-8 字节计数，不能用 JS 的字符数代替。
  if (utf8ByteLength(value) > 32 || value.charCodeAt(0) === 0x30) {
    return "forum_account";
  }
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 0x30 || code > 0x39) {
      return "forum_account";
    }
  }
  return null;
}

/** 手动登录 Key 的字节区间与控制字符限制。 */
export function manualKeyIssue(value: string): UserFieldIssue | null {
  if (value === "") {
    return "required";
  }
  const bytes = utf8ByteLength(value);
  return bytes < 32 || bytes > 256 || /\p{Cc}/u.test(value) ? "manual_key" : null;
}

/** 拉黑理由与用户名使用同一套必填安全文本规则，上限为 1024 字节。 */
export function blacklistReasonIssue(value: string): UserFieldIssue | null {
  return requiredTextIssue(value, 1024, "blacklist_reason");
}

function requiredTextIssue(
  value: string,
  maximumBytes: number,
  issue: UserFieldIssue,
): UserFieldIssue | null {
  if (value === "") {
    return "required";
  }
  return validSafeText(value, maximumBytes) ? null : issue;
}

/** 字段旁错误提示；提交失败由焦点播报，修正输入不额外实时朗读。 */
export function UserFieldError(props: {
  readonly id: string;
  readonly issue: UserFieldIssue | null;
}): JSX.Element {
  return (
    <Show when={props.issue}>
      {(issue) => (
        <p class="admin-field-error" id={props.id}>
          {userFieldIssueMessage(issue())}
        </p>
      )}
    </Show>
  );
}
