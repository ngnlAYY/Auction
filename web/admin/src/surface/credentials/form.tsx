// 凭据页面的安全录入表单。

import type { AdminCredentialCreateRequest } from "@auction/contracts/dto/admin/credentials";
import { createUuidV4 } from "@auction/web-shared/platform/uuid";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import { createAdminCredential, prepareAdminCredentialCreate } from "../../api/credentials";
import { useAdminSessionHost } from "../auth/shell";
import { runAdminAction } from "../command";
import { FieldLiveRegion, useFieldAnnouncer } from "../config/field-announcement";
import { definiteRejection } from "../pending";
import { advanceCommandReload } from "../pending/sources";
import { adminWorldLabel } from "../presentation";
import {
  CREDENTIAL_FIELD_ALL,
  CredentialFieldError,
  credentialFieldErrorsDefault,
  credentialFieldErrorsFirstInvalid,
  credentialFieldErrorsFromServer,
  credentialFieldErrorsSet,
  credentialFieldId,
  credentialFieldIssue,
  credentialFieldIssueMessage,
  useCredentialFields,
} from "./field-validation";
import {
  type CredentialFeedback,
  credentialFeedbackIsSuccess,
  credentialFeedbackMessage,
} from "./labels";

/** 只选择当前导入来源；获取成功仍仅写入所选 kind/world 的一条凭据。 */
type CredentialSource = "cookie" | "forum_login";

/** 创建表单在写入成功后推进父页的读取世代。 */
export interface CredentialCreateFormProps {
  readonly reloadGeneration: Cell<bigint>;
}

/** 安全录入一条邮件 Cookie 或论坛登录凭据；秘密只留在本组件的短生命周期状态中。 */
export function CredentialCreateForm(props: CredentialCreateFormProps): JSX.Element {
  const host = useAdminSessionHost();
  const tasks = createTaskScope();
  const kind = createCell("mail_cookie");
  const world = createCell("main");
  const label = createCell("");
  const playerName = createCell("");
  const secret = createCell("");
  const source = createCell<CredentialSource>("cookie");
  const username = createCell("");
  const password = createCell("");
  const commandIdState = createCell<string | null>(null);
  const busy = createCell(false);
  // 未知结果累计在原表单；来源、密码/CK 和其他字段原样冻结，不复制到未决集合。
  // 只有用户提交才发起登录；后续前置拒绝不能解除先前未知，只有明确成功才能解锁。
  const unknown = createCell(false);
  const feedback = createCell<CredentialFeedback | null>(null);
  const announcer = useFieldAnnouncer();
  const fields = useCredentialFields(announcer);
  // 同一 command_id 的手动重试复用首次固定的正文；字段变化时命令 ID 与正文一起作废。
  let preparedCommand: {
    readonly commandId: string;
    readonly prepared: ReturnType<typeof prepareAdminCredentialCreate>;
  } | null = null;

  // 命令 ID 失效时一并释放含秘密的正文；未知结果仍复用原 ID 与原正文。
  const invalidateCommand = (): void => {
    commandIdState.set(null);
    preparedCommand = null;
  };

  const onSubmit = (event: SubmitEvent): void => {
    event.preventDefault();
    if (busy()) {
      return;
    }
    announcer.clear();
    feedback.set(null);
    const submittedKind = kind();
    const submittedSource = source();
    const submittedWorld = world();
    const submittedLabel = label();
    const expectedPlayerName = playerName();
    const submittedPlayerName =
      submittedKind === "mail_cookie" && expectedPlayerName !== "" ? expectedPlayerName : null;
    const rawSecret = submittedSource === "cookie" ? secret() : "";
    const login =
      submittedSource === "forum_login" ? { username: username(), password: password() } : null;
    // 已聚焦的错误字段经常驻播报区反馈，其他情况移焦后读出字段错误。
    let errors = credentialFieldErrorsDefault();
    errors = credentialFieldErrorsSet(
      errors,
      "label",
      credentialFieldIssue("label", submittedLabel),
    );
    if (submittedPlayerName !== null) {
      errors = credentialFieldErrorsSet(
        errors,
        "player_name",
        credentialFieldIssue("player_name", submittedPlayerName),
      );
    }
    if (login !== null) {
      errors = credentialFieldErrorsSet(
        errors,
        "username",
        credentialFieldIssue("username", login.username),
      );
      errors = credentialFieldErrorsSet(
        errors,
        "password",
        credentialFieldIssue("password", login.password),
      );
    } else {
      errors = credentialFieldErrorsSet(
        errors,
        "secret",
        credentialFieldIssue("secret", rawSecret),
      );
    }
    fields.show(errors);
    const firstInvalid = credentialFieldErrorsFirstInvalid(errors);
    if (firstInvalid !== null) {
      announcer.invalidSubmit(
        firstInvalid[0],
        credentialFieldIssueMessage(firstInvalid[1]),
        !unknown(),
      );
      return;
    }
    const commandId = commandIdState.peek() ?? createUuidV4();
    if (commandIdState.peek() === null) {
      commandIdState.set(commandId);
    }
    const command: AdminCredentialCreateRequest = {
      command_id: commandId,
      kind: submittedKind,
      world: submittedWorld,
      label: submittedLabel,
      ...(submittedPlayerName === null ? {} : { player_name: submittedPlayerName }),
      ...(rawSecret === "" ? {} : { secret: rawSecret }),
      ...(login === null ? {} : { login }),
    };
    if (preparedCommand === null || preparedCommand.commandId !== commandId) {
      preparedCommand = {
        commandId,
        prepared: prepareAdminCredentialCreate(command),
      };
    }
    const prepared = preparedCommand.prepared;
    runAdminAction(
      tasks,
      busy,
      (context) => createAdminCredential(host, prepared, context.signal),
      (result) => {
        const saved: CredentialFeedback = {
          kind: "success",
          restartRequired: result.restart_required,
        };
        announcer.announce(credentialFeedbackMessage(saved));
        feedback.set(saved);
        kind.set("mail_cookie");
        world.set("main");
        label.set("");
        playerName.set("");
        secret.set("");
        username.set("");
        password.set("");
        source.set("cookie");
        invalidateCommand();
        unknown.set(false);
        fields.reset(CREDENTIAL_FIELD_ALL);
        advanceCommandReload(props.reloadGeneration);
      },
      (code) => {
        // 前置拒绝只能说明本次未写入，不能消除先前未知；不凭任意 409 解锁。
        const remainsUnknown = unknown() || !definiteRejection(code);
        unknown.set(remainsUnknown);
        // 可归属字段的服务端错误落到字段旁，其余保持页面级反馈。
        const serverErrors = credentialFieldErrorsFromServer(code, submittedSource === "cookie");
        const invalid = credentialFieldErrorsFirstInvalid(serverErrors);
        if (invalid !== null) {
          fields.show(serverErrors);
          announcer.invalidSubmit(
            invalid[0],
            credentialFieldIssueMessage(invalid[1]),
            !remainsUnknown,
          );
        } else {
          const failure: CredentialFeedback = { kind: "error", code };
          announcer.announce(credentialFeedbackMessage(failure));
          feedback.set(failure);
        }
      },
    );
  };

  return (
    <>
      <form
        class="admin-credentials-import"
        aria-busy={busy() ? "true" : "false"}
        method="post"
        novalidate
        aria-describedby={unknown() ? "import-unknown-help" : undefined}
        onSubmit={onSubmit}
      >
        <h2>{t("admin-credentials-create-title")}</h2>
        <label for="import-kind">
          {t("admin-credentials-kind")}
          <select
            id="import-kind"
            name="kind"
            autocomplete="off"
            disabled={busy() || unknown()}
            value={kind()}
            onChange={(event) => {
              if (busy() || unknown()) {
                return;
              }
              kind.set(event.currentTarget.value);
              invalidateCommand();
            }}
          >
            <option value="mail_cookie">{t("admin-credentials-kind-mail")}</option>
            <option value="forum_cookie">{t("admin-credentials-kind-forum")}</option>
          </select>
        </label>
        <label for="import-label">
          {t("admin-credentials-label")}
          <input
            id="import-label"
            name="label"
            autocomplete="off"
            type="text"
            disabled={busy() || unknown()}
            required
            aria-invalid={fields.error("label") !== null ? "true" : "false"}
            aria-describedby={fields.error("label") !== null ? "import-label-error" : undefined}
            value={label()}
            onInput={(event) => {
              if (busy() || unknown()) {
                return;
              }
              const value = event.currentTarget.value;
              fields.edited("label", value);
              label.set(value);
              invalidateCommand();
            }}
            onBlur={() => {
              if (!busy() && !unknown()) {
                fields.blurred("label", label.peek());
              }
            }}
          />
        </label>
        <CredentialFieldError id="import-label-error" issue={fields.error("label")} />
        <label for="import-world">
          {t("admin-credentials-world")}
          <select
            id="import-world"
            name="world"
            autocomplete="off"
            disabled={busy() || unknown()}
            value={world()}
            onChange={(event) => {
              if (busy() || unknown()) {
                return;
              }
              world.set(event.currentTarget.value);
              invalidateCommand();
            }}
          >
            <option value="main">{adminWorldLabel("main")}</option>
            <option value="isekai">{adminWorldLabel("isekai")}</option>
            <option value="shared">{t("admin-credentials-world-shared")}</option>
          </select>
        </label>
        <Show when={kind() === "mail_cookie"}>
          <label for="import-player-name">
            {t("admin-credentials-player-name")}
            <input
              id="import-player-name"
              name="player_name"
              type="text"
              disabled={busy() || unknown()}
              autocomplete="off"
              autocapitalize="none"
              autocorrect="off"
              spellcheck="false"
              aria-invalid={fields.error("player_name") !== null ? "true" : "false"}
              aria-describedby={
                fields.error("player_name") !== null
                  ? "import-player-name-help import-player-name-error"
                  : "import-player-name-help"
              }
              value={playerName()}
              onInput={(event) => {
                if (busy() || unknown()) {
                  return;
                }
                const value = event.currentTarget.value;
                fields.edited("player_name", value);
                playerName.set(value);
                invalidateCommand();
              }}
              onBlur={() => {
                if (!busy() && !unknown()) {
                  fields.blurred("player_name", playerName.peek());
                }
              }}
            />
          </label>
          <p id="import-player-name-help">{t("admin-credentials-player-name-help")}</p>
          <CredentialFieldError id="import-player-name-error" issue={fields.error("player_name")} />
        </Show>
        <label for="import-source">
          {t("admin-credentials-source")}
          <select
            id="import-source"
            name="source"
            autocomplete="off"
            disabled={busy() || unknown()}
            aria-describedby="import-source-help"
            value={source()}
            onChange={(event) => {
              if (busy() || unknown()) {
                return;
              }
              source.set(event.currentTarget.value === "forum_login" ? "forum_login" : "cookie");
              // 已知未写入时切换来源，清除离开分支的敏感值，避免隐藏表单继续持有。
              secret.set("");
              username.set("");
              password.set("");
              invalidateCommand();
              fields.reset(["secret", "username", "password"]);
              feedback.set(null);
              announcer.clear();
            }}
          >
            <option value="cookie">{t("admin-credentials-source-cookie")}</option>
            <option value="forum_login">{t("admin-credentials-source-login")}</option>
          </select>
        </label>
        <p id="import-source-help">{t("admin-credentials-source-help")}</p>
        <Show
          when={source() === "cookie"}
          fallback={
            <>
              <label for="import-username">
                {t("admin-credentials-login-username")}
                <input
                  id="import-username"
                  name="username"
                  type="text"
                  autocomplete="username"
                  autocapitalize="none"
                  autocorrect="off"
                  spellcheck="false"
                  disabled={busy() || unknown()}
                  required
                  maxlength={256}
                  aria-invalid={fields.error("username") !== null ? "true" : "false"}
                  aria-describedby={
                    fields.error("username") !== null
                      ? "import-login-help import-username-error"
                      : "import-login-help"
                  }
                  value={username()}
                  onInput={(event) => {
                    if (busy() || unknown()) {
                      return;
                    }
                    const value = event.currentTarget.value;
                    fields.edited("username", value);
                    username.set(value);
                    invalidateCommand();
                  }}
                  onBlur={() => {
                    if (!busy() && !unknown()) {
                      fields.blurred("username", username.peek());
                    }
                  }}
                />
              </label>
              <CredentialFieldError id="import-username-error" issue={fields.error("username")} />
              <label for="current-password">
                {t("admin-credentials-login-password")}
                <input
                  id="current-password"
                  name="password"
                  type="password"
                  autocomplete="current-password"
                  autocapitalize="none"
                  autocorrect="off"
                  spellcheck="false"
                  disabled={busy() || unknown()}
                  required
                  maxlength={4096}
                  aria-invalid={fields.error("password") !== null ? "true" : "false"}
                  aria-describedby={
                    fields.error("password") !== null
                      ? "import-login-help import-password-error"
                      : "import-login-help"
                  }
                  value={password()}
                  onInput={(event) => {
                    if (busy() || unknown()) {
                      return;
                    }
                    const value = event.currentTarget.value;
                    fields.edited("password", value);
                    password.set(value);
                    invalidateCommand();
                  }}
                  onBlur={() => {
                    if (!busy() && !unknown()) {
                      fields.blurred("password", password.peek());
                    }
                  }}
                />
              </label>
              <CredentialFieldError id="import-password-error" issue={fields.error("password")} />
              <p id="import-login-help">{t("admin-credentials-login-help")}</p>
            </>
          }
        >
          <label for={credentialFieldId("secret")}>
            {t("admin-credentials-secret")}
            <input
              id={credentialFieldId("secret")}
              name="cookie"
              type="password"
              autocomplete="off"
              autocapitalize="none"
              autocorrect="off"
              spellcheck="false"
              disabled={busy() || unknown()}
              required
              aria-invalid={fields.error("secret") !== null ? "true" : "false"}
              aria-describedby={
                fields.error("secret") !== null
                  ? "import-secret-help import-secret-error"
                  : "import-secret-help"
              }
              value={secret()}
              onInput={(event) => {
                if (busy() || unknown()) {
                  return;
                }
                const value = event.currentTarget.value;
                fields.edited("secret", value);
                secret.set(value);
                invalidateCommand();
              }}
              onBlur={() => {
                if (!busy() && !unknown()) {
                  fields.blurred("secret", secret.peek());
                }
              }}
            />
          </label>
          <CredentialFieldError id="import-secret-error" issue={fields.error("secret")} />
          <p id="import-secret-help">{t("admin-credentials-secret-help")}</p>
        </Show>
        <Show when={unknown()}>
          <p id="import-unknown-help" role="status">
            {t("admin-credentials-import-unknown")}
          </p>
        </Show>
        <Show when={feedback()}>
          {(current) => (
            <p
              class={
                credentialFeedbackIsSuccess(current())
                  ? "action-feedback action-feedback--success"
                  : "action-feedback action-feedback--error"
              }
            >
              {credentialFeedbackMessage(current())}
            </p>
          )}
        </Show>
        <button type="submit" class="admin-btn-primary" disabled={busy()}>
          {busy()
            ? t("admin-action-working")
            : unknown()
              ? t("admin-pending-retry")
              : source() === "forum_login"
                ? t("admin-credentials-login-submit")
                : t("admin-credentials-create-submit")}
        </button>
      </form>
      <FieldLiveRegion announcer={announcer} />
    </>
  );
}
