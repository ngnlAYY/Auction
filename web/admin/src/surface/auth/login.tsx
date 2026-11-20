// Admin 登录表单与就绪状态徽标。
// secret 只交给外壳发起一次 Basic Auth 请求；表单只做校验、展示与失焦恢复。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { unreachable } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import { createEffect, onSettled, Show } from "solid-js";
import type { AdminSessionHost } from "../../api/session";
import { trimRustWhitespace } from "../../api/session-time";
import type { AdminReadinessState } from "../../api/status";
import { adminApiErrorMessage } from "../presentation";
import { useAdminSessionHost } from "./shell";

/** 只交给外壳使用一次的登录凭据。 */
export interface LoginSubmission {
  /** 表单挂载时取得的宿主；外壳只用它派发这次登录。 */
  readonly host: AdminSessionHost;
  readonly actor: string;
  readonly secret: string;
}

/** Admin 登录表单；请求、在途票与失败码都由外壳持有。 */
export function AdminLogin(props: {
  readonly submitting: boolean;
  readonly loginError: string | null;
  readonly onEdit: () => void;
  readonly onSubmit: (submission: LoginSubmission) => void;
}) {
  // 认证上下文覆盖未认证子树；表单不再通过属性借用外壳的句柄。
  const host = useAdminSessionHost();
  const actor = createCell("");
  const secret = createCell("");
  const validationError = createCell<string | null>(null);
  const errorCode = createValueMemo(() => validationError() ?? props.loginError);
  let actorInput: HTMLInputElement | null = null;
  let secretInput: HTMLInputElement | null = null;

  // 每次本地校验失败都要恢复焦点，即使错误码与上次相同。
  createEffect(
    () => [props.submitting, props.loginError, validationError()] as const,
    ([submitting, loginError, localError]) => {
      if (submitting || (localError === null && loginError === null)) {
        return;
      }
      // DOM 更新完成后聚焦；与提交校验共用 Rust `str::trim` 的空白判定。
      onSettled(() => {
        const input = trimRustWhitespace(actor.peek()) === "" ? actorInput : secretInput;
        input?.focus();
      });
    },
  );

  function submit(event: SubmitEvent): void {
    event.preventDefault();
    if (props.submitting) {
      return;
    }
    if (trimRustWhitespace(actor()) === "" || secret() === "") {
      validationError.set("invalid_request");
      return;
    }
    validationError.set(null);
    const submittedActor = actor();
    const submittedSecret = secret();
    secret.set("");
    props.onSubmit({ host, actor: submittedActor, secret: submittedSecret });
  }

  return (
    <main class="admin-login-page" aria-labelledby="admin-login-title">
      <section class="admin-login-card" aria-labelledby="admin-login-title">
        <div class="admin-login-brand">
          <span class="admin-login-mark" aria-hidden="true" />
          <div class="admin-login-brand__text">
            <p class="admin-login-brand__eyebrow">HV-Auction</p>
            <h1 id="admin-login-title">{t("page-login-title")}</h1>
          </div>
        </div>
        <form
          class="admin-auth-form"
          aria-busy={props.submitting ? "true" : "false"}
          onSubmit={submit}
        >
          <label for="admin-actor">{t("admin-login-actor")}</label>
          <input
            id="admin-actor"
            name="username"
            type="text"
            autocomplete="username"
            autocapitalize="none"
            autocorrect="off"
            spellcheck="false"
            enterkeyhint="next"
            required
            disabled={props.submitting}
            aria-invalid={errorCode() !== null ? "true" : "false"}
            aria-describedby={errorCode() !== null ? "admin-login-error" : undefined}
            ref={(element) => {
              actorInput = element;
            }}
            value={actor()}
            onInput={(event) => {
              actor.set(event.currentTarget.value);
              validationError.set(null);
              props.onEdit();
            }}
          />
          <label for="admin-secret">{t("admin-login-secret")}</label>
          <input
            id="admin-secret"
            name="password"
            type="password"
            autocomplete="current-password"
            enterkeyhint="go"
            required
            disabled={props.submitting}
            aria-invalid={errorCode() !== null ? "true" : "false"}
            aria-describedby={errorCode() !== null ? "admin-login-error" : undefined}
            ref={(element) => {
              secretInput = element;
            }}
            value={secret()}
            onInput={(event) => {
              secret.set(event.currentTarget.value);
              validationError.set(null);
              props.onEdit();
            }}
          />
          <button
            class="admin-btn-primary"
            type="submit"
            disabled={props.submitting}
            aria-busy={props.submitting ? "true" : "false"}
          >
            <Show when={props.submitting} fallback={t("admin-login-submit")}>
              {t("admin-login-working")}
            </Show>
          </button>
        </form>
        <Show when={errorCode() !== null}>
          <p id="admin-login-error" class="action-feedback action-feedback--error" role="alert">
            {loginErrorMessage(errorCode() ?? "")}
          </p>
        </Show>
      </section>
    </main>
  );
}

function loginErrorMessage(code: string): string {
  // 登录 POST 没有业务命令 ID，超时反馈不能承诺保留密码或幂等重试。
  if (code === "request_timeout") {
    return t("admin-error-reconciliation-required");
  }
  return adminApiErrorMessage(code);
}

/** 全局 readiness 状态徽标，显示在已登录外壳的顶栏；圆点与降级文字按状态着色。 */
export function AdminReadinessStatus(props: { state: AdminReadinessState }) {
  const presentation = () => readinessPresentation(props.state);
  return (
    <p
      class={[
        "admin-readiness-status",
        {
          "admin-readiness-status--checking": presentation().kind === "checking",
          "admin-readiness-status--ready": presentation().kind === "ready",
          "admin-readiness-status--degraded": presentation().kind === "degraded",
        },
      ]}
      data-state={presentation().kind}
      aria-live="polite"
      role="status"
    >
      <span class="admin-readiness-status__indicator" aria-hidden="true" />
      {presentation().label}
    </p>
  );
}

function readinessPresentation(state: AdminReadinessState): {
  readonly kind: AdminReadinessState;
  readonly label: string;
} {
  switch (state) {
    case "checking":
      return { kind: "checking", label: t("admin-readiness-checking") };
    case "ready":
      return { kind: "ready", label: t("admin-readiness-ready") };
    case "degraded":
      return { kind: "degraded", label: t("admin-readiness-degraded") };
    default:
      return unreachable(state);
  }
}
