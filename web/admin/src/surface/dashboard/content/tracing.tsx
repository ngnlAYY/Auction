// 进程追踪健康面板：逐进程展示 tracing 状态与导出计数。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { AdminDashboardSnapshot } from "../../../api/dashboard";
import { Badge } from "./widgets";

/** 逐进程 tracing 健康面板的输入；面板保持挂载，随快照更新。 */
export interface TracingPanelProps {
  readonly snapshot: AdminDashboardSnapshot;
}

/** 逐进程的 tracing 健康面板；没有未过期心跳时显示不可用提示。 */
export function TracingPanel(props: TracingPanelProps): JSX.Element {
  const process = createValueMemo(() => props.snapshot.process.data);
  const rows = createValueMemo(() => process()?.processes ?? []);
  return (
    <section class="admin-dashboard-panel" aria-labelledby="dashboard-tracing-title">
      <h2 id="dashboard-tracing-title">{t("dashboard-legacy-tracing-health")}</h2>
      <Show
        when={rows().length > 0}
        fallback={
          <p class="admin-dashboard-status">{t("dashboard-legacy-tracing-status-unavailable")}</p>
        }
      >
        <div class="admin-dashboard-cards">
          {/* 列表是全部未过期心跳，重启交接时同一角色会短暂并存新旧实例；
              key 取心跳主键，角色不能保证唯一。 */}
          <For each={rows()} keyed={(item) => `${item.service_name}/${item.instance_id}`}>
            {(item) => (
              <div class="admin-dashboard-card">
                <div class="admin-dashboard-value">
                  <span class="admin-dashboard-metric-stack">
                    <Badge
                      state={tracingTone(item().tracing.status)}
                      value={tracingStatus(item().tracing.status)}
                    />
                    <span class="admin-dashboard-label">
                      {t("dashboard-legacy-tracing-counts", {
                        dropped: item().tracing.dropped_span_count,
                        failures: item().tracing.export_failure_count,
                      })}
                    </span>
                  </span>
                </div>
                <div class="admin-dashboard-label">
                  {t("dashboard-legacy-tracing-process", { role: item().process_role })}
                </div>
              </div>
            )}
          </For>
        </div>
      </Show>
    </section>
  );
}

function tracingTone(status: string): string {
  switch (status) {
    case "disabled":
      return "neutral";
    case "ok":
      return "ok";
    default:
      return "error";
  }
}

function tracingStatus(status: string): string {
  switch (status) {
    case "disabled":
      return t("dashboard-legacy-tracing-status-disabled");
    case "ok":
      return t("dashboard-legacy-tracing-status-ok");
    case "degraded":
      return t("dashboard-legacy-tracing-status-degraded");
    default:
      return t("dashboard-legacy-tracing-status-unavailable");
  }
}
