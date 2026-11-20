// 投递水位面板：按状态展示待处理、失败与人工核对数量。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import type { AdminDashboardSnapshot } from "../../../api/dashboard";
import { Badge, Card } from "./widgets";

/** 投递水位面板的输入；面板保持挂载，随快照更新。 */
export interface DeliveriesPanelProps {
  readonly snapshot: AdminDashboardSnapshot;
}

/** 投递水位面板；未知分区只显示无数据提示。 */
export function DeliveriesPanel(props: DeliveriesPanelProps): JSX.Element {
  const delivery = createValueMemo(() => props.snapshot.delivery.data);
  return (
    <section class="admin-dashboard-panel" aria-labelledby="dashboard-deliveries-title">
      <h2 id="dashboard-deliveries-title">{t("dashboard-legacy-deliveries-by-status")}</h2>
      <Show
        when={delivery()}
        fallback={<p class="admin-dashboard-status">{t("dashboard-legacy-no-data")}</p>}
      >
        {(current) => (
          <div class="admin-dashboard-cards">
            <Card
              label={t("dashboard-legacy-delivery-pending")}
              value={current().water.pending.toString()}
            />
            <Card
              label={t("dashboard-legacy-delivery-failed")}
              value={
                <Badge
                  state={current().water.failed > 0 ? "error" : "ok"}
                  value={current().water.failed.toString()}
                />
              }
            />
            <Card
              label={t("dashboard-legacy-delivery-manual-reconcile")}
              value={
                <Badge
                  state={current().water.manual_reconcile > 0 ? "error" : "ok"}
                  value={current().water.manual_reconcile.toString()}
                />
              }
            />
            <Card
              label={t("dashboard-legacy-delivery-oldest-pending")}
              value={t("dashboard-legacy-duration-seconds", {
                seconds: current().water.oldest_pending_age_seconds,
              })}
            />
            <Card
              label={t("dashboard-legacy-delivery-oldest-manual-reconcile")}
              value={t("dashboard-legacy-duration-seconds", {
                seconds: current().water.oldest_manual_reconcile_age_seconds,
              })}
            />
          </div>
        )}
      </Show>
    </section>
  );
}
