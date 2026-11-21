// Public 登录表单。
// 页面不保存或回显服务端 session token；登录成功后只展示安全身份投影。登录 key 只用于一次
// 请求，提交后立即从表单状态清空，不进入日志、本地存储或 URL。

import type { PublicRegistrationInfoResponse } from "@auction/contracts/dto/public/account";
import { useFocusElementByIdAfterRender } from "@auction/web-shared/platform/a11y";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource, type ResourceState } from "@auction/web-shared/reactive/resource";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { err, type Result } from "@auction/web-shared/result";
import { useRouter } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, Show } from "solid-js";

import { fetchRegistrationInfo } from "../../api/registration";
import {
  loginPublic,
  type PublicSession,
  type PublicSessionApiError,
  publicSessionApiErrorLocal,
} from "../../api/session";
import { loginBlacklistedReason, loginErrorMessage } from "./error-message";
import { rustTrim, safeReturnPath } from "./login-return";

type LoginState =
  | { readonly kind: "success"; readonly session: PublicSession }
  | { readonly kind: "failure"; readonly error: PublicSessionApiError };

type RegistrationView =
  | { readonly kind: "loading" }
  | { readonly kind: "error" }
  | { readonly kind: "ready"; readonly accounts: string };

/** 名单重读挂起时优先显示加载态，即使资源仍保留上一次的结果。 */
function registrationView(
  state: ResourceState,
  result: Result<PublicRegistrationInfoResponse, string> | undefined,
): RegistrationView {
  if (state === "pending") {
    return { kind: "loading" };
  }
  if (result?.ok !== true) {
    return { kind: "error" };
  }
  return { kind: "ready", accounts: result.value.bot_players.join(", ") };
}

/** 注册名单只通过自己的匿名读取与重试入口更新。 */
export function useRegistrationAccounts() {
  const accounts = createTaskResource(
    () => null,
    (_dependencies, context) => fetchRegistrationInfo(context.signal),
  );
  return {
    view: createValueMemo(() => registrationView(accounts.state(), accounts())),
    retry: accounts.restart,
  };
}

/** 登录页的路由 props；返回地址保持响应式，查询变化后必须重算。 */
export interface LoginProps {
  readonly returnTo: string | null;
}

/** 登录表单；返回地址在组件体取得，提交时只保留已经净化的副本。
 *
 * 路由包装层传入带 getter 的 props，`createValueMemo` 才能在 `return_to` 变化后重算；
 * 字符串重载保留给直接构造页面的旧薄包装，不提供查询变化的响应性。
 */
export function login(props: LoginProps): JSX.Element;
export function login(returnTo?: string | null): JSX.Element;
export function login(input: LoginProps | string | null = null): JSX.Element {
  const returnTo = (): string | null =>
    typeof input === "object" && input !== null ? input.returnTo : input;
  const key = createCell("");
  const result = createCell<LoginState | null>(null);
  const submitting = createCell(false);
  const focusFailure = useFocusElementByIdAfterRender();
  createEffect(
    () => result(),
    (snapshot) => {
      if (snapshot?.kind === "failure") {
        focusFailure("public-login-key");
      }
    },
    { defer: true },
  );
  const registration = useRegistrationAccounts();
  const router = useRouter();
  const tasks = createTaskScope();
  const returnPath = createValueMemo(() => safeReturnPath(returnTo()));
  const registrationFailed = () => registration.view().kind === "error";
  const registrationHint = () => {
    const view = registration.view();
    if (view.kind === "loading") {
      return t("data-loading");
    }
    if (view.kind === "error") {
      return t("data-error");
    }
    const botAccount =
      view.accounts === "" ? "" : t("login-bot-account", { accounts: view.accounts });
    return t("login-hint", { bot_account: botAccount });
  };
  const failed = () => result()?.kind === "failure";
  const failure = () => {
    const snapshot = result();
    return snapshot?.kind === "failure" ? snapshot.error : null;
  };
  const success = () => {
    const snapshot = result();
    return snapshot?.kind === "success" ? snapshot.session : null;
  };

  return (
    <section
      class="public-account-login-page"
      aria-labelledby="login-title"
      aria-busy={submitting() ? "true" : "false"}
    >
      <h1 id="login-title">{t("page-login-title")}</h1>
      <form
        class="public-account-login-form"
        onSubmit={(event) => {
          event.preventDefault();
          if (submitting()) {
            return;
          }
          const submittedKey = rustTrim(key());
          if (submittedKey === "") {
            result.set({ kind: "failure", error: publicSessionApiErrorLocal("empty_key") });
            return;
          }
          // 原始 key 只用于这一次请求；成功或失败都不继续留在表单状态中。
          key.set("");
          submitting.set(true);
          const returnPathValue = returnPath();
          tasks.spawn(async (context) => {
            const loginResult = await loginPublic(submittedKey, context.signal).catch(() =>
              err(publicSessionApiErrorLocal("network_error")),
            );
            if (!context.alive()) {
              return;
            }
            submitting.set(false);
            if (loginResult.ok) {
              const session = loginResult.value;
              result.set({ kind: "success", session });
              router.replace(returnPathValue);
              return;
            }
            result.set({ kind: "failure", error: loginResult.error });
          });
        }}
      >
        <label for="public-login-key">{t("login-key-label")}</label>
        <input
          id="public-login-key"
          name="login_key"
          type="password"
          autocomplete="current-password"
          autocapitalize="none"
          spellcheck="false"
          enterkeyhint="go"
          maxlength="1024"
          aria-invalid={failed() ? "true" : "false"}
          aria-describedby={failed() ? "public-login-hint public-login-error" : "public-login-hint"}
          placeholder={
            submitting() || failed()
              ? "••••••••••••••••••••••••••••••••"
              : t("login-key-placeholder")
          }
          data-masked={submitting() || failed() ? "true" : "false"}
          disabled={submitting()}
          value={key()}
          onInput={(event) => {
            key.set(event.currentTarget.value);
            result.set(null);
          }}
        />
        <Show when={failure()}>
          {(error) => (
            <p id="public-login-error" class="public-account-login-error" role="alert">
              <span>{loginErrorMessage(error())}</span>
              <Show when={loginBlacklistedReason(error()) !== null}>
                <span class="public-account-login-ban-reason">
                  {t("login-blacklisted-reason", { reason: loginBlacklistedReason(error()) ?? "" })}
                </span>
              </Show>
            </p>
          )}
        </Show>
        <button
          type="submit"
          disabled={submitting()}
          aria-busy={submitting() ? "true" : "false"}
          data-error={failed() ? "true" : "false"}
        >
          {submitting() ? t("login-working") : t("login-submit")}
        </button>
      </form>
      <p
        id="public-login-hint"
        class="public-account-login-hint"
        role={registrationFailed() ? "alert" : "status"}
      >
        {registrationHint()}
      </p>
      <Show when={registrationFailed()}>
        {/* 名单重试只重启匿名读取，不能重提登录或清空已输入的 key。 */}
        <button type="button" onClick={() => registration.retry()}>
          {t("data-retry")}
        </button>
      </Show>
      <Show when={success()}>
        {(session) => (
          <p class="action-feedback action-feedback--success" role="status">
            {t("login-success", { username: session().username })}
          </p>
        )}
      </Show>
    </section>
  );
}
