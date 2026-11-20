// 审计健康面板：旧版 audit 指标与缺失服务的可用性说明。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import type { AdminDashboardSnapshot } from "../../../api/dashboard";
import type { DashboardDerived } from "./derived";
import { Badge, Card, countText } from "./widgets";

/** 审计健康面板的输入；面板保持挂载，随快照与共享派生值更新。 */
export interface AuditPanelProps {
  readonly snapshot: AdminDashboardSnapshot;
  readonly derived: DashboardDerived;
}

/** 审计健康面板；同时展示派生状态与缺失服务的指标可用性。 */
export function AuditPanel(props: AuditPanelProps): JSX.Element {
  const auditFailures = createValueMemo(() => props.derived.auditFailures);
  const auditMetricsUnavailable = createValueMemo(() => props.derived.auditMetricsUnavailable);
  const auditState = createValueMemo(() => props.derived.auditState);
  const auditStatus = createValueMemo(() => props.derived.auditStatus);
  return (
    <section class="admin-dashboard-panel" aria-labelledby="dashboard-audit-title">
      <h2 id="dashboard-audit-title">{t("dashboard-legacy-audit-health")}</h2>
      <div class="admin-dashboard-cards">
        <Card
          label={t("dashboard-legacy-audit-health")}
          value={<Badge state={auditState()} value={auditStatus()} />}
        />
        <Card
          label={t("dashboard-legacy-audit-failures-total")}
          value={countText(
            !auditMetricsUnavailable() || auditFailures() > 0 ? auditFailures() : null,
          )}
        />
      </div>
      <p class="admin-dashboard-status">
        {t("dashboard-legacy-audit-metrics-line", {
          public: availability(props.snapshot, "public"),
          admin: availability(props.snapshot, "admin"),
          worker: availability(props.snapshot, "worker"),
        })}
      </p>
      <details class="admin-dashboard-details">
        <summary>{t("dashboard-metrics-details")}</summary>
        <p class="admin-dashboard-status">{t("dashboard-legacy-audit-scope")}</p>
        <Show when={auditFailures() === 0 && !auditMetricsUnavailable()}>
          <p class="admin-dashboard-status">{t("dashboard-legacy-audit-no-failures")}</p>
        </Show>
      </details>
    </section>
  );
}

function availability(snapshot: AdminDashboardSnapshot, service: string): string {
  return snapshot.health.missing_services.some((missing) => missing === service)
    ? t("dashboard-legacy-audit-metrics-unavailable")
    : t("dashboard-legacy-audit-metrics-available");
}
