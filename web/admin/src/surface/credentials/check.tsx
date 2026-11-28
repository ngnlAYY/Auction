// 手动 Cookie 校验按钮与瞬时结果；不在浏览器读取或保存 Cookie。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { Time } from "@auction/web-shared/ui/components/time";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { AdminCredential, AdminCredentialCheck } from "../../api/credentials";
import { checkAdminCredential } from "../../api/credentials";
import { useAdminSessionHost } from "../auth/shell";
import { runAdminFeedbackAction } from "../command";
import { type AdminActionFeedback, adminActionFeedbackMessage } from "../presentation";
import { credentialWorldLabel } from "./labels";

/** 当前凭据版本的手动校验入口与安全结果。 */
export interface CredentialCheckActionProps {
  readonly credential: AdminCredential;
  readonly busy: Cell<boolean>;
  readonly blocked: boolean;
}

/** 只显示属于当前凭据与版本的检查结果。 */
export function CredentialCheckAction(props: CredentialCheckActionProps): JSX.Element {
  const host = useAdminSessionHost();
  const tasks = createTaskScope();
  const checked = createCell<AdminCredentialCheck | null>(null);
  const checking = createCell(false);
  const feedback = createCell<AdminActionFeedback | null>(null);
  const resultId = () => `credential-check-${props.credential.id}`;
  const visible = () => {
    const result = checked();
    return result !== null &&
      result.credential_id === props.credential.id &&
      result.state_version === props.credential.state_version
      ? result
      : null;
  };
  const failed = () => {
    const current = feedback();
    return current !== null && current.kind !== "success" ? current : null;
  };
  const describedBy = () => (visible() !== null || failed() !== null ? resultId() : undefined);
  return (
    <>
      <button
        type="button"
        disabled={props.busy() || props.blocked}
        aria-busy={checking() ? "true" : "false"}
        aria-describedby={describedBy()}
        onClick={() => {
          if (props.busy() || props.blocked) {
            return;
          }
          checked.set(null);
          checking.set(true);
          const credential = props.credential;
          runAdminFeedbackAction(
            tasks,
            props.busy,
            feedback,
            async (context) => {
              try {
                return await checkAdminCredential(host, credential, context.signal);
              } finally {
                // 任何结束路径都必须撤下“检查中”，否则按钮会一直显示忙碌文案。
                if (context.alive()) {
                  checking.set(false);
                }
              }
            },
            (result) => checked.set(result),
          );
        }}
      >
        {checking() ? t("admin-credentials-checking") : t("admin-credentials-action-check")}
      </button>
      <Show
        when={visible()}
        fallback={
          <Show when={failed()}>
            {(current) => (
              <span
                id={resultId()}
                class="admin-credentials__feedback admin-credentials__feedback--error"
                role="alert"
              >
                {adminActionFeedbackMessage(current(), t("admin-credentials-check-failed"))}
              </span>
            )}
          </Show>
        }
      >
        {(result) => (
          <span id={resultId()} class="admin-credentials__feedback" role="status">
            <For each={result().checks} keyed={false}>
              {(check) => (
                <span class={checkStatusClass(check().status)}>
                  {t("admin-credentials-check-result", {
                    world: credentialWorldLabel(check().world),
                    status: checkStatusLabel(check().status),
                  })}{" "}
                </span>
              )}
            </For>
            {t("admin-credentials-check-time")}
            <Time value={result().checked_at} />
          </span>
        )}
      </Show>
    </>
  );
}

/** 检查状态对应的行内颜色修饰类；未知状态不加修饰。 */
function checkStatusClass(status: string): string {
  switch (status) {
    case "valid":
      return "admin-credentials__feedback--success";
    case "invalid":
      return "admin-credentials__feedback--error";
    default:
      return "";
  }
}

/** 检查状态译文；未知状态按“无法判定”展示。 */
function checkStatusLabel(status: string): string {
  switch (status) {
    case "valid":
      return t("admin-credentials-check-valid");
    case "invalid":
      return t("admin-credentials-check-invalid");
    default:
      return t("admin-credentials-check-inconclusive");
  }
}
