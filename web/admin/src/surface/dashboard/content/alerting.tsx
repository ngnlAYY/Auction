// 告警投递面板：信号/批次计数与时间戳明细。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import type { AdminDashboardSnapshot } from "../../../api/dashboard";
import { formatAdminTime } from "../../presentation";
import { Badge, Card } from "./widgets";

/** 告警投递面板的输入；面板保持挂载，随快照更新。 */
export interface AlertingPanelProps {
  readonly snapshot: AdminDashboardSnapshot;
}

/** 告警投递面板；未知分区只显示不可用提示。 */
export function AlertingPanel(props: AlertingPanelProps): JSX.Element {
  const alerting = createValueMemo(() => props.snapshot.alerting.data);
  const badgeState = createValueMemo(() => {
    const data = alerting();
    if (data === null || !data.enabled) {
      return "neutral";
    }
    return props.snapshot.alerting.status === "ok" ? "ok" : "error";
  });
  const badgeStatus = createValueMemo(() => {
    const data = alerting();
    if (data === null) {
      return t("dashboard-legacy-alerting-status-unavailable");
    }
    if (!data.enabled) {
      return t("dashboard-legacy-alerting-status-disabled");
    }
    return props.snapshot.alerting.status === "ok"
      ? t("dashboard-legacy-alerting-status-ok")
      : t("dashboard-legacy-alerting-status-degraded");
  });
  return (
    <section class="admin-dashboard-panel" aria-labelledby="dashboard-alerting-title">
      <h2 id="dashboard-alerting-title">{t("dashboard-legacy-alerting-health")}</h2>
      <Show
        when={alerting()}
        fallback={
          <p class="admin-dashboard-status">{t("dashboard-legacy-alerting-status-unavailable")}</p>
        }
      >
        {(data) => (
          <>
            <div class="admin-dashboard-cards">
              <Card
                label={t("dashboard-legacy-alerting-health")}
                value={<Badge state={badgeState()} value={badgeStatus()} />}
              />
              <Card
                label={t("dashboard-legacy-alerting-active")}
                value={data().active_signal_count.toString()}
              />
              <Card
                label={t("dashboard-legacy-alerting-due")}
                value={data().due_signal_count.toString()}
              />
              <Card
                label={t("dashboard-legacy-alerting-claimed")}
                value={data().claimed_batch_count.toString()}
              />
              <Card
                label={t("dashboard-legacy-alerting-succeeded")}
                value={data().succeeded_batch_count.toString()}
              />
              <Card
                label={t("dashboard-legacy-alerting-failed")}
                value={data().failed_batch_count.toString()}
              />
              <Card
                label={t("dashboard-legacy-alerting-manual-reconcile")}
                value={data().manual_reconcile_count.toString()}
              />
              <Card
                label={t("dashboard-legacy-alerting-webhook-pending")}
                value={data().webhook_pending_count.toString()}
              />
              <Card
                label={t("dashboard-legacy-alerting-webhook-sending")}
                value={data().webhook_sending_count.toString()}
              />
              <Card
                label={t("dashboard-legacy-alerting-webhook-failed")}
                value={data().webhook_failed_count.toString()}
              />
              <Card
                label={t("dashboard-legacy-alerting-webhook-manual")}
                value={data().webhook_manual_reconcile_count.toString()}
              />
            </div>
            <details class="admin-dashboard-details">
              <summary>{t("dashboard-metrics-details")}</summary>
              <div class="admin-dashboard-cards admin-dashboard-cards--timestamps">
                <Card
                  label={t("dashboard-legacy-alerting-last-observed")}
                  value={timestampText(data().last_observed_at)}
                />
                <Card
                  label={t("dashboard-legacy-alerting-last-success")}
                  value={timestampText(data().last_success_at)}
                />
                <Card
                  label={t("dashboard-legacy-alerting-last-failure")}
                  value={timestampText(data().last_failure_at)}
                />
                <Card
                  label={t("dashboard-legacy-alerting-last-error")}
                  value={
                    data().last_error_code === ""
                      ? t("dashboard-legacy-not-available")
                      : data().last_error_code
                  }
                />
              </div>
            </details>
          </>
        )}
      </Show>
    </section>
  );
}

/** 时间戳缺失时显示统一占位，存在时按管理端格式展示。 */
function timestampText(value: string | null): string {
  return value === null ? t("dashboard-legacy-not-available") : formatAdminTime(value);
}
