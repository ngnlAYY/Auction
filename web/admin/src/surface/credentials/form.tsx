// 凭据页面的安全录入表单。

import type { AdminCredentialMutationResponse } from "@auction/contracts/dto/admin/credentials";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import { createAdminCredential, prepareAdminCredentialCreateOnce } from "../../api/credentials";
import { FieldLiveRegion, useFieldAnnouncer } from "../config/field-announcement";
import { type CommandDelivery, commandDeliveryReported, requirePendingCommands } from "../pending";
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
  credentialKindLabel,
} from "./labels";
import { createCredentialSecretDraft } from "./secret-draft";

// 新凭据尚无服务端 ID，导入入口在本会话内共用一个串行实体。
const IMPORT_ENTITY = "credential:import";

/** 只选择当前导入来源；获取成功仍仅写入所选 kind/world 的一条凭据。 */
type CredentialSource = "cookie" | "forum_login";

/** 创建表单在写入成功后推进父页的读取世代。 */
export interface CredentialCreateFormProps {
  readonly reloadGeneration: Cell<bigint>;
}

/** 录入凭据。已提交的导入由会话持有，离页后仍可用同一命令原样重试。 */
export function CredentialCreateForm(props: CredentialCreateFormProps): JSX.Element {
  const pending = requirePendingCommands();
  const host = pending.host.sessionHost;
  const kind = createCell("mail_cookie");
  const world = createCell("main");
  const label = createCell("");
  const playerName = createCell("");
  const source = createCell<CredentialSource>("cookie");
  const { secret, username, password, clear, isAlive } = createCredentialSecretDraft();
  const busy = () => pending.isBusy(IMPORT_ENTITY);
  const blocked = () => pending.contains(IMPORT_ENTITY);
  const unknown = () => blocked() && !busy();
  const feedback = createCell<CredentialFeedback | null>(null);
  const announcer = useFieldAnnouncer();
  const fields = useCredentialFields(announcer);

  const onSuccess = (
    result: AdminCredentialMutationResponse,
    sourceAlive: boolean,
  ): CommandDelivery => {
    const delivered = sourceAlive && isAlive();
    if (delivered) {
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
      clear();
      source.set("cookie");
      fields.reset(CREDENTIAL_FIELD_ALL);
      advanceCommandReload(props.reloadGeneration);
    }
    return commandDeliveryReported(delivered, () =>
      result.restart_required
        ? { kind: "text", text: t("admin-credentials-action-restart") }
        : null,
    );
  };

  const onFailure = (code: string, submittedSource: CredentialSource): void => {
    if (!isAlive()) {
      return;
    }
    // 未决命令仍占用实体时，禁用的字段通过常驻播报区反馈。
    const serverErrors = credentialFieldErrorsFromServer(code, submittedSource === "cookie");
    const invalid = credentialFieldErrorsFirstInvalid(serverErrors);
    if (invalid !== null) {
      fields.show(serverErrors);
      announcer.invalidSubmit(invalid[0], credentialFieldIssueMessage(invalid[1]), !blocked());
    } else {
      const failure: CredentialFeedback = { kind: "error", code };
      announcer.announce(credentialFeedbackMessage(failure));
      feedback.set(failure);
    }
  };

  const onSubmit = (event: SubmitEvent): void => {
    event.preventDefault();
    if (blocked()) {
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
      announcer.invalidSubmit(firstInvalid[0], credentialFieldIssueMessage(firstInvalid[1]), true);
      return;
    }
    // 只把冻结输入交给命令工厂。宿主用同一 ID 运行和重试，不再读取表单。
    const prepare = prepareAdminCredentialCreateOnce({
      kind: submittedKind,
      world: submittedWorld,
      label: submittedLabel,
      ...(submittedPlayerName === null ? {} : { player_name: submittedPlayerName }),
      ...(rawSecret === "" ? {} : { secret: rawSecret }),
      ...(login === null ? {} : { login }),
    });
    pending.submitWithReceipt(
      IMPORT_ENTITY,
      t("admin-credentials-import-action", {
        kind: credentialKindLabel(submittedKind),
        label: submittedLabel,
      }),
      (commandId, signal) => createAdminCredential(host, prepare(commandId), signal),
      onSuccess,
      (code) => onFailure(code, submittedSource),
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
            disabled={blocked()}
            value={kind()}
            onChange={(event) => {
              if (blocked()) {
                return;
              }
              kind.set(event.currentTarget.value);
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
            disabled={blocked()}
            required
            aria-invalid={fields.error("label") !== null ? "true" : "false"}
            aria-describedby={fields.error("label") !== null ? "import-label-error" : undefined}
            value={label()}
            onInput={(event) => {
              if (blocked()) {
                return;
              }
              const value = event.currentTarget.value;
              fields.edited("label", value);
              label.set(value);
            }}
            onBlur={() => {
              if (!blocked()) {
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
            disabled={blocked()}
            value={world()}
            onChange={(event) => {
              if (blocked()) {
                return;
              }
              world.set(event.currentTarget.value);
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
              disabled={blocked()}
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
                if (blocked()) {
                  return;
                }
                const value = event.currentTarget.value;
                fields.edited("player_name", value);
                playerName.set(value);
              }}
              onBlur={() => {
                if (!blocked()) {
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
            disabled={blocked()}
            aria-describedby="import-source-help"
            value={source()}
            onChange={(event) => {
              if (blocked()) {
                return;
              }
              source.set(event.currentTarget.value === "forum_login" ? "forum_login" : "cookie");
              // 已知未写入时切换来源，清除离开分支的敏感值，避免隐藏表单继续持有。
              clear();
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
                  disabled={blocked()}
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
                    if (blocked()) {
                      return;
                    }
                    const value = event.currentTarget.value;
                    fields.edited("username", value);
                    username.set(value);
                  }}
                  onBlur={() => {
                    if (!blocked()) {
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
                  disabled={blocked()}
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
                    if (blocked()) {
                      return;
                    }
                    const value = event.currentTarget.value;
                    fields.edited("password", value);
                    password.set(value);
                  }}
                  onBlur={() => {
                    if (!blocked()) {
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
              disabled={blocked()}
              required
              aria-invalid={fields.error("secret") !== null ? "true" : "false"}
              aria-describedby={
                fields.error("secret") !== null
                  ? "import-secret-help import-secret-error"
                  : "import-secret-help"
              }
              value={secret()}
              onInput={(event) => {
                if (blocked()) {
                  return;
                }
                const value = event.currentTarget.value;
                fields.edited("secret", value);
                secret.set(value);
              }}
              onBlur={() => {
                if (!blocked()) {
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
        <button type="submit" class="admin-btn-primary" disabled={blocked()}>
          {busy()
            ? t("admin-action-working")
            : source() === "forum_login"
              ? t("admin-credentials-login-submit")
              : t("admin-credentials-create-submit")}
        </button>
      </form>
      <FieldLiveRegion announcer={announcer} />
    </>
  );
}
