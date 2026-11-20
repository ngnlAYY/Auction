// 运行配置文档顶部的只读事实卡与 Worker 重启状态文案。

import type { AdminWorkerRestartStateDto } from "@auction/contracts/dto/admin/rules";
import { unreachable } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

/** 安全配置事实卡：可选状态与完整详情同时展示。 */
export function AdminRuntimeConfigFact(props: {
  readonly detail: string;
  readonly state: boolean | null;
  readonly label: string;
}): JSX.Element {
  return (
    <article>
      <div class="admin-config-fact-heading">
        <strong>{props.label}</strong>
        <Show when={props.state !== null}>
          <span
            class={
              props.state === true ? "admin-config-fact-enabled" : "admin-config-fact-disabled"
            }
          >
            {props.state === true ? t("admin-runtime-enabled") : t("admin-runtime-disabled")}
          </span>
        </Show>
      </div>
      <p title={props.detail}>{props.detail}</p>
    </article>
  );
}

export function workerRestartMessage(state: AdminWorkerRestartStateDto): string {
  switch (state) {
    case "restart_required":
      return t("admin-runtime-worker-restart-required");
    case "consistent":
      return t("admin-runtime-worker-restart-consistent");
    case "unknown":
      return t("admin-runtime-worker-restart-unknown");
    default:
      return unreachable(state);
  }
}
