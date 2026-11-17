// Public 登录表单。
// 页面不保存或回显服务端 session token；登录成功后只展示安全身份投影。登录 key 只用于一次
// 请求，提交后立即从表单状态清空，不进入日志、本地存储或 URL。

import { useFocusElementByIdAfterRender } from "@auction/web-shared/platform/a11y";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { ok } from "@auction/web-shared/result";
import { useRouter } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, Show, useContext } from "solid-js";

import { fetchRegistrationInfo } from "../../api/registration";
import {
  loginPublic,
  type PublicSession,
  type PublicSessionApiError,
  publicSessionApiErrorLocal,
} from "../../api/session";
import { PublicSessionResourceContext } from "../index";
import { loginBlacklistedReason, loginErrorMessage } from "./error-message";
import { rustTrim, safeReturnPath } from "./login-return";

type LoginState =
  | { readonly kind: "success"; readonly session: PublicSession }
  | { readonly kind: "failure"; readonly error: PublicSessionApiError };

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
  const sessionResource = useContext(PublicSessionResourceContext);
  const registrationAccounts = createTaskResource(
    () => null,
    async (_dependencies, context) => {
      const info = await fetchRegistrationInfo(context.signal);
      if (!context.alive()) {
        return { ok: false as const, error: "network_error" };
      }
      return info.ok ? ok(info.value.bot_players.join(", ")) : info;
    },
  );
  const router = useRouter();
  const tasks = createTaskScope();
  const returnPath = createValueMemo(() => safeReturnPath(returnTo()));
  const botAccount = () => {
    const accounts = registrationAccounts();
    if (accounts === undefined || !accounts.ok || accounts.value === "") {
      return "";
    }
    return t("login-bot-account", { accounts: accounts.value });
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
            const loginResult = await loginPublic(submittedKey, context.signal);
            if (!context.alive()) {
              return;
            }
            submitting.set(false);
            if (loginResult.ok) {
              const session = loginResult.value;
              result.set({ kind: "success", session });
              sessionResource.set(ok(session));
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
      <p id="public-login-hint" class="public-account-login-hint">
        {t("login-hint", { bot_account: botAccount() })}
      </p>
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
