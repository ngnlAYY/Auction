// Admin 用户创建对话框、一次性登录 Key 展示与创建命令交付。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { onCleanup, Show } from "solid-js";

import { adminApiFailureDisplayCode } from "../../api/failure";
import { createAdminUser, prepareAdminUserCreateOnce } from "../../api/users";
import { useAdminSessionHost } from "../auth/shell";
import {
  type CommandDelivery,
  commandDeliveryReported,
  type PendingCommandGate,
  requirePendingCommands,
  setIfAlive,
  trySetIfAlive,
} from "../pending";
import { type AdminActionFeedback, adminActionFeedbackMessage } from "../presentation";
import { useFocusField } from "../validation";
import {
  forumAccountIssue,
  manualKeyIssue,
  UserFieldError,
  type UserFieldIssue,
  usernameIssue,
} from "./fields";

interface UserFieldErrors {
  readonly username: UserFieldIssue | null;
  readonly forumAccount: UserFieldIssue | null;
  readonly manualKey: UserFieldIssue | null;
}

const EMPTY_USER_FIELD_ERRORS: UserFieldErrors = {
  username: null,
  forumAccount: null,
  manualKey: null,
};

/** 表单顺序上的第一个错误字段；用于提交失败后把焦点移过去。 */
function firstInvalidId(errors: UserFieldErrors): string | null {
  if (errors.username !== null) {
    return "create-username";
  }
  if (errors.forumAccount !== null) {
    return "create-forum-account-id";
  }
  if (errors.manualKey !== null) {
    return "create-key";
  }
  return null;
}

/** 脱敏占位；与 Rust `Debug` 的 `[REDACTED]` 保持一致。 */
const ADMIN_USER_GENERATED_KEY_REDACTED = "[REDACTED]";

/** 一次性展示的登录 Key；默认序列化与字符串化都只给出脱敏占位。
 * 明文只能由一次性弹窗经 `reveal()` 显式读取，禁止记录、打印或持久化整个对象。 */
export interface AdminUserGeneratedKey {
  /** 显式读取明文；只允许一次性展示弹窗调用。 */
  reveal(): string;
  /** 会话回执复制必须保留封装；不能用 `structuredClone` 退化成普通对象。 */
  copy(): AdminUserGeneratedKey;
  /** 嵌套进其他对象做 JSON 序列化时同样脱敏。 */
  toJSON(): "[REDACTED]";
  /** 拼进字符串或模板时同样脱敏。 */
  toString(): string;
}

/** 把明文收进闭包，实例本身没有可枚举或可检查的明文字段。 */
function adminUserGeneratedKey(plaintext: string): AdminUserGeneratedKey {
  const generated: AdminUserGeneratedKey = {
    reveal: () => plaintext,
    copy: () => adminUserGeneratedKey(plaintext),
    toJSON: () => ADMIN_USER_GENERATED_KEY_REDACTED,
    toString: () => ADMIN_USER_GENERATED_KEY_REDACTED,
  };
  // 方法不枚举，控制台展开对象时也看不到明文或内部方法。
  for (const name of ["reveal", "copy", "toJSON", "toString"] as const) {
    Object.defineProperty(generated, name, { enumerable: false });
  }
  return Object.freeze(generated);
}

/** 生成的 Key 保存到会话回执中，直到操作员确认交付。 */
export function adminUserGeneratedKeyDelivery(
  key: string | null,
  reported: boolean,
): CommandDelivery {
  if (key !== null) {
    return { kind: "receipt", receipt: { kind: "generated-key", key: adminUserGeneratedKey(key) } };
  }
  return commandDeliveryReported(reported, () => null);
}

function finishCreate(
  username: string,
  generatedKey: string | null,
  sourceAlive: boolean,
  open: Cell<boolean>,
  notice: Cell<string | null>,
): CommandDelivery {
  // open/notice 由用户页持有；来源页卸载后 sourceAlive 为 false，回执留给会话接管。
  if (sourceAlive) {
    setIfAlive(open, false, () => true);
  }
  return adminUserGeneratedKeyDelivery(
    generatedKey,
    sourceAlive && trySetIfAlive(notice, username, () => true),
  );
}

/** 创建用户对话框；手动 Key 只保留在本次命令工厂的内存中。 */
export interface AdminUserCreateFormProps {
  readonly gate: PendingCommandGate;
  readonly open: Cell<boolean>;
  readonly notice: Cell<string | null>;
}

export function AdminUserCreateForm(props: AdminUserCreateFormProps): JSX.Element {
  const username = createCell("");
  const forumAccountId = createCell("");
  const keyMode = createCell("generated");
  const manualKey = createCell("");
  const pending = requirePendingCommands();
  const host = useAdminSessionHost();
  const focusField = useFocusField();
  const feedback = createCell<AdminActionFeedback | null>(null);
  const fieldErrors = createCell<UserFieldErrors>(EMPTY_USER_FIELD_ERRORS);
  let alive = true;
  onCleanup(() => {
    alive = false;
  });
  const feedbackAlive = () => alive;
  const feedbackError = () => {
    const current = feedback();
    return current !== null && current.kind === "error" ? current : null;
  };

  // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管。
  return (
    <Dialog
      title={t("admin-users-create-title")}
      closeLabel={t("admin-users-create-close")}
      busy={pending.isBusy("create-user")}
      onClose={() => props.open.set(false)}
    >
      <form
        class="admin-users-create"
        method="post"
        action="/api/v1/admin/users"
        aria-label={t("admin-users-create-title")}
        novalidate
        onSubmit={(event) => {
          event.preventDefault();
          if (pending.contains("create-user") || props.gate.blocked()) {
            return;
          }
          const submittedUsername = username();
          const submittedForumAccountId = forumAccountId();
          const submittedKeyMode = keyMode();
          const submittedManualKey = manualKey();
          const errors: UserFieldErrors = {
            username: usernameIssue(submittedUsername),
            forumAccount: forumAccountIssue(submittedForumAccountId, false),
            manualKey: submittedKeyMode === "manual" ? manualKeyIssue(submittedManualKey) : null,
          };
          fieldErrors.set(errors);
          const invalidId = firstInvalidId(errors);
          if (invalidId !== null) {
            focusField(invalidId);
            return;
          }
          feedback.set(null);
          // 手动 Key 仅留在当前会话的工厂内存；重试复制原值，不从已清空的输入框重建。
          const manual = submittedKeyMode === "manual" ? submittedManualKey : null;
          const prepare = prepareAdminUserCreateOnce(
            submittedUsername,
            submittedForumAccountId === "" ? null : submittedForumAccountId,
            manual !== null ? "manual" : "generated",
            manual,
          );
          const submitted = pending.submitWithReceipt(
            "create-user",
            `${t("admin-users-create-open")} · ${submittedUsername}`,
            (commandId, signal) => createAdminUser(host, commandId, prepare, signal),
            (result, sourceAlive) =>
              finishCreate(
                result.user.username,
                result.generated_key,
                sourceAlive,
                props.open,
                props.notice,
              ),
            (code) => {
              if (code === "user_conflict" && alive) {
                fieldErrors.update((current) => ({
                  ...current,
                  username: "username_conflict",
                }));
              }
              setIfAlive(
                feedback,
                { kind: "error", code: adminApiFailureDisplayCode(code) },
                feedbackAlive,
              );
            },
          );
          if (submitted) {
            manualKey.set("");
          }
        }}
      >
        <label for="create-username">
          {t("admin-users-username")}
          <span aria-hidden="true"> *</span>
        </label>
        <input
          id="create-username"
          name="username"
          autocomplete="off"
          autocapitalize="none"
          spellcheck="false"
          disabled={pending.contains("create-user")}
          type="text"
          required
          aria-invalid={fieldErrors().username !== null ? "true" : "false"}
          aria-describedby={fieldErrors().username !== null ? "create-username-error" : undefined}
          value={username()}
          onInput={(event) => {
            if (pending.contains("create-user")) {
              return;
            }
            const value = event.currentTarget.value;
            if (fieldErrors.peek().username !== null) {
              fieldErrors.update((current) => ({ ...current, username: usernameIssue(value) }));
            }
            username.set(value);
          }}
        />
        <UserFieldError id="create-username-error" issue={fieldErrors().username} />
        <label for="create-forum-account-id">{t("admin-users-forum-account-id")}</label>
        <input
          id="create-forum-account-id"
          name="forum_account_id"
          autocomplete="off"
          disabled={pending.contains("create-user")}
          type="text"
          inputmode="numeric"
          aria-describedby={
            fieldErrors().forumAccount !== null
              ? "forum-account-id-help create-forum-account-id-error"
              : "forum-account-id-help"
          }
          aria-invalid={fieldErrors().forumAccount !== null ? "true" : "false"}
          value={forumAccountId()}
          onInput={(event) => {
            if (pending.contains("create-user")) {
              return;
            }
            const value = event.currentTarget.value;
            if (fieldErrors.peek().forumAccount !== null) {
              fieldErrors.update((current) => ({
                ...current,
                forumAccount: forumAccountIssue(value, false),
              }));
            }
            forumAccountId.set(value);
          }}
        />
        <p id="forum-account-id-help">{t("admin-users-forum-account-id-help")}</p>
        <UserFieldError id="create-forum-account-id-error" issue={fieldErrors().forumAccount} />
        <label for="create-key-mode">{t("admin-users-key-mode")}</label>
        <select
          id="create-key-mode"
          name="key_mode"
          autocomplete="off"
          disabled={pending.contains("create-user")}
          value={keyMode()}
          onChange={(event) => {
            if (pending.contains("create-user")) {
              return;
            }
            keyMode.set(event.currentTarget.value);
            manualKey.set("");
            fieldErrors.update((current) => ({ ...current, manualKey: null }));
          }}
        >
          <option value="generated">{t("admin-users-key-generated")}</option>
          <option value="manual">{t("admin-users-key-manual")}</option>
        </select>
        <Show when={keyMode() === "manual"}>
          <label for="create-key">
            {t("admin-users-manual-key")}
            <span aria-hidden="true"> *</span>
          </label>
          <input
            id="create-key"
            name="login_key"
            disabled={pending.contains("create-user")}
            type="password"
            autocomplete="new-password"
            required
            aria-invalid={fieldErrors().manualKey !== null ? "true" : "false"}
            aria-describedby={
              fieldErrors().manualKey === "manual_key"
                ? "create-key-error"
                : fieldErrors().manualKey !== null
                  ? "create-key-help create-key-error"
                  : "create-key-help"
            }
            value={manualKey()}
            onInput={(event) => {
              if (pending.contains("create-user")) {
                return;
              }
              const value = event.currentTarget.value;
              if (fieldErrors.peek().manualKey !== null) {
                fieldErrors.update((current) => ({
                  ...current,
                  manualKey: manualKeyIssue(value),
                }));
              }
              manualKey.set(value);
            }}
          />
          <Show when={fieldErrors().manualKey !== "manual_key"}>
            <p id="create-key-help">{t("admin-users-field-manual-key-short")}</p>
          </Show>
          <UserFieldError id="create-key-error" issue={fieldErrors().manualKey} />
        </Show>
        <button
          type="submit"
          class="admin-btn-primary"
          disabled={pending.contains("create-user") || props.gate.blocked()}
        >
          {t("admin-users-create-submit")}
        </button>
      </form>
      <Show when={feedbackError()}>
        {(current) => <p role="alert">{adminActionFeedbackMessage(current(), "")}</p>}
      </Show>
      <div class="admin-shell__dialog-actions">
        <button
          type="button"
          disabled={pending.isBusy("create-user")}
          onClick={() => props.open.set(false)}
        >
          {t("admin-users-create-close")}
        </button>
      </div>
    </Dialog>
  );
}

/** 一次性密钥展示弹窗；外壳由共享 Dialog 接管。 */
export function AdminUserGeneratedKeyDialog(props: {
  readonly generated: AdminUserGeneratedKey;
  readonly onClose: () => void;
}): JSX.Element {
  return (
    <Dialog
      title={t("admin-users-generated-key-title")}
      closeLabel={t("admin-users-generated-key-dismiss")}
      onClose={() => props.onClose()}
    >
      <div class="admin-users-secret">
        <p>{t("admin-users-generated-key-warning")}</p>
        <code>{props.generated.reveal()}</code>
        <div class="admin-shell__dialog-actions">
          <button type="button" onClick={() => props.onClose()}>
            {t("admin-users-generated-key-dismiss")}
          </button>
        </div>
      </div>
    </Dialog>
  );
}
