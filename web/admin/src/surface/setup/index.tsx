// 首次部署页面编排：部署码验证通过后才挂载配置表单，不进入普通 Admin 会话外壳。

import { ThemePicker } from "@auction/web-shared/platform/theme";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { useAppBootReady } from "@auction/web-shared/ui/bootstrap";
import { LanguagePicker, t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Match, Show, Switch } from "solid-js";

import { verifyFirstRun } from "../../api/setup";
import { ADMIN_LOCALE_CATALOG } from "..";
import { createSetupDraft, setupDraftClearPasswords, setupDraftValue } from "./draft";
import { ConfigurationForm, type SetupNotice } from "./form";

/** 部署码和草稿只属于当前页面；刷新需重新验证，后续请求仍由服务端独立鉴权。 */
export function page(): JSX.Element {
  useAppBootReady(() => true);
  const token = createCell("");
  const verified = createCell(false);
  const draft = createCell(createSetupDraft());
  const busy = createCell(false);
  const notice = createCell<SetupNotice | null>(null);
  const completed = createCell(false);
  const initializeAndStart = createCell(false);
  // 页面内不可逆的停靠状态；输入变化或组件重渲染不能解除结果不确定的门禁。
  const outcomeUnknown = createCell(false);
  const tasks = createTaskScope();

  return (
    <main class="admin-setup" aria-labelledby="admin-setup-title">
      <div class={verified() ? "admin-setup__card" : "admin-setup__card admin-setup__card--access"}>
        <LanguagePicker label={t("language-label")} catalog={ADMIN_LOCALE_CATALOG} />
        <ThemePicker
          label={t("theme-label")}
          darkLabel={t("theme-dark")}
          lightLabel={t("theme-light")}
        />
        <h1 id="admin-setup-title">{t("setup-title")}</h1>
        <Switch>
          <Match when={!verified()}>
            <p class="admin-panel__note">{t("setup-intro")}</p>
            <form
              class="admin-setup__form"
              onSubmit={(event) => {
                event.preventDefault();
                if (
                  busy.peek() ||
                  verified.peek() ||
                  completed.peek() ||
                  outcomeUnknown.peek() ||
                  rustTrim(token.peek()).length === 0
                ) {
                  return;
                }
                busy.set(true);
                notice.set(null);
                const submittedToken = token.peek();
                tasks.spawn(async (context) => {
                  const result = await verifyFirstRun(submittedToken, context.signal);
                  if (!context.alive()) {
                    return;
                  }
                  if (result.ok) {
                    initializeAndStart.set(result.value);
                    verified.set(true);
                  } else {
                    const code = result.error;
                    if (code === "setup_outcome_unknown") {
                      outcomeUnknown.set(true);
                      token.set("");
                      draft.update(setupDraftClearPasswords);
                    }
                    notice.set([false, errorKey(code)]);
                  }
                  busy.set(false);
                });
              }}
            >
              <fieldset disabled={busy() || outcomeUnknown()}>
                <legend>{t("setup-access")}</legend>
                <label for="setup-token">{t("setup-token")}</label>
                <input
                  id="setup-token"
                  name="setup_token"
                  autocomplete="off"
                  autocapitalize="none"
                  enterkeyhint="go"
                  spellcheck="false"
                  type="password"
                  required={true}
                  value={token()}
                  onInput={(event) => {
                    if (busy.peek() || outcomeUnknown.peek()) {
                      return;
                    }
                    token.set(event.currentTarget.value);
                    notice.set(null);
                  }}
                />
              </fieldset>
              <div class="admin-setup__actions">
                <button
                  class="admin-btn-primary"
                  type="submit"
                  disabled={busy() || outcomeUnknown() || rustTrim(token()).length === 0}
                >
                  {busy() ? t("setup-verifying") : t("setup-verify")}
                </button>
              </div>
            </form>
          </Match>
          <Match when={!completed()}>
            <Show when={!outcomeUnknown()}>
              <p class="admin-panel__note">{t("setup-verified")}</p>
            </Show>
            <ConfigurationForm
              token={token}
              draft={draft}
              notice={notice}
              outcomeUnknown={outcomeUnknown}
              initializeAndStart={initializeAndStart()}
              onAuthFailure={() => {
                draft.update(setupDraftClearPasswords);
                verified.set(false);
                token.set("");
              }}
              onCompleted={() => {
                token.set("");
                completed.set(true);
              }}
            />
          </Match>
        </Switch>
        <Show when={completed() && initializeAndStart()}>
          <a href={setupDraftValue(draft(), "admin_origin")}>{t("setup-open-admin")}</a>
        </Show>
        <Show when={notice()}>
          {(current) => (
            <p
              class={current()[0] ? "action-feedback" : "action-feedback action-feedback--error"}
              role={current()[0] ? "status" : "alert"}
            >
              {t(current()[1])}
            </p>
          )}
        </Show>
      </div>
    </main>
  );
}

/** 把服务端错误码映射为首次设置页面的译文键；未知错误回落到通用提示。 */
export function errorKey(code: string): string {
  switch (code) {
    case "invalid_input":
      return "setup-error-input";
    case "setup_origin":
      return "setup-error-origin";
    case "setup_confirmation_required":
      return "setup-error-confirm-initialize";
    case "database_unavailable":
      return "setup-error-database";
    case "configuration_invalid":
      return "setup-error-config";
    case "setup_locked":
    case "setup_completed":
      return "setup-error-locked";
    case "setup_busy":
      return "setup-error-busy";
    case "setup_auth":
      return "setup-error-token";
    case "setup_io":
      return "setup-error-io";
    case "setup_outcome_unknown":
      return "setup-error-outcome-unknown";
    case "network":
    case "network_error":
    case "request_timeout":
      return "setup-error-network";
    default:
      return "setup-error-unknown";
  }
}

/** Rust `str::trim` 的空白集合；JS 的 `trim` 与之不同，不能替换。 */
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
    code === 0x09 ||
    code === 0x0a ||
    code === 0x0b ||
    code === 0x0c ||
    code === 0x0d ||
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
