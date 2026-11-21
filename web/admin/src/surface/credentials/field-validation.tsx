// 凭据表单字段在提交、编辑和离焦时共享的校验与反馈。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { unreachable } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import { validSafeText } from "../../api/validation";
import type { FieldAnnouncer } from "../config/field-announcement";

/** 凭据表单的文本字段；枚举顺序决定错误数组与表单序。 */
export type CredentialField = "label" | "player_name" | "secret" | "username" | "password";

/** 全部字段的表单顺序。 */
export const CREDENTIAL_FIELD_ALL: readonly CredentialField[] = [
  "label",
  "player_name",
  "secret",
  "username",
  "password",
];

/** 字段在错误数组与编辑标记中的固定槽位；与枚举声明顺序一致。 */
type CredentialFieldIndex = 0 | 1 | 2 | 3 | 4;

/** 字段在错误数组中的位置；与枚举声明顺序一致。 */
function credentialFieldIndex(field: CredentialField): CredentialFieldIndex {
  switch (field) {
    case "label":
      return 0;
    case "player_name":
      return 1;
    case "secret":
      return 2;
    case "username":
      return 3;
    case "password":
      return 4;
    default:
      return unreachable(field);
  }
}

/** 字段对应控件的 DOM id。 */
export function credentialFieldId(field: CredentialField): string {
  switch (field) {
    case "label":
      return "import-label";
    case "player_name":
      return "import-player-name";
    case "secret":
      return "import-secret";
    case "username":
      return "import-username";
    case "password":
      return "current-password";
    default:
      return unreachable(field);
  }
}

/** 字段值的本地校验分类；`null` 表示当前值可提交。 */
export function credentialFieldIssue(
  field: CredentialField,
  value: string,
): CredentialFieldIssue | null {
  // 标签与凭据内容只含空白时优先按“必填”处理，与源码 match 的守卫顺序一致。
  if ((field === "label" || field === "secret") && rustTrim(value).length === 0) {
    return "required";
  }
  switch (field) {
    case "label":
      return validSafeText(value, 256) ? null : "label_invalid";
    case "player_name":
      // 预期完整玩家名称可以为空；非空时仍须满足原文长度与字符边界。
      return value === "" || validSafeText(value, 256) ? null : "player_name_invalid";
    case "secret":
      return null;
    case "username":
    case "password":
      if (value === "") {
        return "required";
      }
      if (field === "username") {
        return validSafeText(value, 256) ? null : "username_invalid";
      }
      return utf8ByteLength(value) > 4096 || hasAsciiControl(value) ? "password_invalid" : null;
    default:
      return unreachable(field);
  }
}

/** 一个字段错误的槽位；`null` 表示无错误。 */
export type CredentialFieldErrors = readonly [
  CredentialFieldIssue | null,
  CredentialFieldIssue | null,
  CredentialFieldIssue | null,
  CredentialFieldIssue | null,
  CredentialFieldIssue | null,
];

/** 空错误表；每次写入都复制数组，供响应式 Cell 比较与替换。 */
export function credentialFieldErrorsDefault(): CredentialFieldErrors {
  return [null, null, null, null, null];
}

/** 读取某个字段的错误。 */
export function credentialFieldErrorsGet(
  errors: CredentialFieldErrors,
  field: CredentialField,
): CredentialFieldIssue | null {
  return errors[credentialFieldIndex(field)];
}

/** 替换某个字段的错误并返回新表；不修改传入的数组。 */
export function credentialFieldErrorsSet(
  errors: CredentialFieldErrors,
  field: CredentialField,
  issue: CredentialFieldIssue | null,
): CredentialFieldErrors {
  const index = credentialFieldIndex(field);
  return [
    index === 0 ? issue : errors[0],
    index === 1 ? issue : errors[1],
    index === 2 ? issue : errors[2],
    index === 3 ? issue : errors[3],
    index === 4 ? issue : errors[4],
  ];
}

/** 表单序上第一个错误字段的控件 id 及其文案类型。 */
export function credentialFieldErrorsFirstInvalid(
  errors: CredentialFieldErrors,
): readonly [string, CredentialFieldIssue] | null {
  for (const field of CREDENTIAL_FIELD_ALL) {
    const issue = credentialFieldErrorsGet(errors, field);
    if (issue !== null) {
      return [credentialFieldId(field), issue];
    }
  }
  return null;
}

/** 只有服务端明确归属字段的错误才移动焦点。 */
export function credentialFieldErrorsFromServer(
  code: string,
  cookieSource: boolean,
): CredentialFieldErrors {
  let errors = credentialFieldErrorsDefault();
  switch (code) {
    case "invalid_credential_label":
      errors = credentialFieldErrorsSet(errors, "label", "label_invalid");
      break;
    case "invalid_credential_player_name":
      errors = credentialFieldErrorsSet(errors, "player_name", "player_name_invalid");
      break;
    case "invalid_credential_secret":
      if (cookieSource) {
        errors = credentialFieldErrorsSet(errors, "secret", "secret_invalid");
      }
      break;
    default:
      break;
  }
  return errors;
}

/** 凭据表单的字段错误反馈句柄；状态随创建它的组件存活。 */
export interface CredentialFieldFeedback {
  error(field: CredentialField): CredentialFieldIssue | null;
  show(errors: CredentialFieldErrors): void;
  edited(field: CredentialField, value: string): void;
  blurred(field: CredentialField, value: string): void;
  reset(fields: readonly CredentialField[]): void;
}

/** 为一次凭据表单创建字段错误状态；离焦提示共用页面提供的播报器。 */
export function useCredentialFields(announcer: FieldAnnouncer): CredentialFieldFeedback {
  const touched = new Set<CredentialField>();
  const errors: Cell<CredentialFieldErrors> = createCell(credentialFieldErrorsDefault());
  return {
    error: (field) => credentialFieldErrorsGet(errors(), field),
    show: (next) => errors.set(next),
    edited: (field, value) => {
      touched.add(field);
      const previous = credentialFieldErrorsGet(errors.peek(), field);
      if (previous !== null) {
        const issue = credentialFieldIssue(field, value);
        if (issue !== previous) {
          errors.update((current) => credentialFieldErrorsSet(current, field, issue));
          announcer.clearField(credentialFieldId(field));
        }
      }
    },
    blurred: (field, value) => {
      if (!touched.has(field)) {
        return;
      }
      // 离焦时提示；仍有效的服务端错误保留至输入发生变化。
      const issue = credentialFieldIssue(field, value);
      if (issue === null) {
        return;
      }
      if (credentialFieldErrorsGet(errors.peek(), field) === issue) {
        return;
      }
      errors.update((current) => credentialFieldErrorsSet(current, field, issue));
      announcer.announceOnBlur(credentialFieldId(field), credentialFieldIssueMessage(issue));
    },
    reset: (fields) => {
      errors.update((current) => {
        let next = current;
        for (const field of fields) {
          touched.delete(field);
          next = credentialFieldErrorsSet(next, field, null);
        }
        return next;
      });
    },
  };
}

/** 字段错误的分类；渲染时翻译，保证切换语言后已展示的错误同步更新。 */
export type CredentialFieldIssue =
  | "required"
  /** 标签不符合长度或字符边界。 */
  | "label_invalid"
  /** 预期完整玩家名称不符合原文边界。 */
  | "player_name_invalid"
  /** 服务端返回凭据内容无效。 */
  | "secret_invalid"
  /** 论坛账号名称超出长度或字符边界。 */
  | "username_invalid"
  /** 论坛密码超出长度或字符边界。 */
  | "password_invalid";

/** 字段错误的当前语言文案。 */
export function credentialFieldIssueMessage(issue: CredentialFieldIssue): string {
  switch (issue) {
    case "required":
      return t("admin-field-required");
    case "label_invalid":
      return t("admin-credentials-field-label-invalid");
    case "player_name_invalid":
      return t("admin-credentials-field-player-name-invalid");
    case "secret_invalid":
      return t("admin-credentials-field-secret-invalid");
    case "username_invalid":
      return t("admin-credentials-field-username-invalid");
    case "password_invalid":
      return t("admin-credentials-field-password-invalid");
    default:
      return unreachable(issue);
  }
}

/** 字段错误的行内提示。 */
export interface CredentialFieldErrorProps {
  readonly id: string;
  readonly issue?: CredentialFieldIssue | null;
}

/** 有错误时渲染一行 `admin-field-error`；无错误时不输出节点。 */
export function CredentialFieldError(props: CredentialFieldErrorProps): JSX.Element {
  return (
    <Show when={props.issue ?? null}>
      {(issue) => (
        <p class="admin-field-error" id={props.id}>
          {credentialFieldIssueMessage(issue())}
        </p>
      )}
    </Show>
  );
}

/** Rust `str::trim` 的 Unicode White_Space 集合；不能用会接受 FEFF 的 JS `trim`。 */
function rustTrim(value: string): string {
  let start = 0;
  let end = value.length;
  while (start < end && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

function isRustWhitespace(code: number): boolean {
  return (
    (code >= 0x09 && code <= 0x0d) ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}

/** Rust `u8::is_ascii_control`：0x00–0x1F 与 0x7F；不包含 C1 控制符。 */
function hasAsciiControl(value: string): boolean {
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code <= 0x1f || code === 0x7f) {
      return true;
    }
  }
  return false;
}
