// 资产对账面板：托管缺失、偿付缺口与在途资产的逐世界状态。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { AdminDashboardSnapshot } from "../../../api/dashboard";
import { adminWorldLabel } from "../../presentation";
import type { DashboardDerived, DashboardTone } from "./derived";
import { Badge, Card } from "./widgets";

/** 资产对账面板的输入；面板保持挂载，随快照与共享派生值更新。 */
export interface ReconciliationPanelProps {
  readonly snapshot: AdminDashboardSnapshot;
  readonly derived: DashboardDerived;
}

/** 资产对账面板；在途资产只展示，不计入缺失与缺口。 */
export function ReconciliationPanel(props: ReconciliationPanelProps): JSX.Element {
  const reconciliation = createValueMemo(() => props.snapshot.reconciliation.data);
  const reconciliationState = createValueMemo(() => props.derived.reconciliationState);
  const reconciliationDrift = createValueMemo(() => props.derived.reconciliationDrift);
  return (
    <section class="admin-dashboard-panel" aria-labelledby="dashboard-reconciliation-title">
      <h2 id="dashboard-reconciliation-title">{t("dashboard-reconciliation-title")}</h2>
      <Show
        when={reconciliation()}
        fallback={
          <p class="admin-dashboard-status">{t("dashboard-reconciliation-status-unavailable")}</p>
        }
      >
        {(current) => (
          <>
            <div class="admin-dashboard-cards">
              <Card
                label={t("dashboard-reconciliation-state")}
                value={
                  <Badge
                    state={reconciliationState()}
                    value={
                      reconciliationDrift()
                        ? t("dashboard-reconciliation-differences")
                        : t("dashboard-reconciliation-ok")
                    }
                  />
                }
              />
              <Card
                label={t("dashboard-reconciliation-custody-age")}
                value={t("dashboard-legacy-duration-seconds", {
                  seconds: current().custody_report_age_seconds,
                })}
              />
              <Card
                label={t("dashboard-reconciliation-solvency-age")}
                value={t("dashboard-legacy-duration-seconds", {
                  seconds: current().solvency_report_age_seconds,
                })}
              />
            </div>
            <h3>{t("dashboard-reconciliation-worlds")}</h3>
            <div class="admin-dashboard-cards">
              <For each={current().worlds} keyed={(world) => world.world}>
                {(world) => (
                  <div class="admin-dashboard-card">
                    <div class="admin-dashboard-value">
                      <span>
                        {t("dashboard-reconciliation-custody")}{" "}
                        <Badge
                          state={discrepancyTone(world().custody_missing_count)}
                          value={world().custody_missing_count.toString()}
                        />
                      </span>
                      <span class="admin-dashboard-label">
                        {t("dashboard-reconciliation-solvency")}{" "}
                        <Badge
                          state={discrepancyTone(world().solvency_shortfall_credits)}
                          value={t("dashboard-reconciliation-credits", {
                            credits: world().solvency_shortfall_credits,
                          })}
                        />
                      </span>
                      {/* 在途只展示：已寄出、等待收件人领取，不计入缺失与缺口。 */}
                      <span class="admin-dashboard-label">
                        {t("dashboard-reconciliation-custody-in-transit")}{" "}
                        <Badge
                          state="neutral"
                          value={world().custody_in_transit_count.toString()}
                        />
                      </span>
                      <span class="admin-dashboard-label">
                        {t("dashboard-reconciliation-solvency-in-transit")}{" "}
                        <Badge
                          state="neutral"
                          value={t("dashboard-reconciliation-in-transit-credits", {
                            count: world().solvency_in_transit_count,
                            credits: world().solvency_in_transit_credits,
                          })}
                        />
                      </span>
                    </div>
                    <div class="admin-dashboard-label">{adminWorldLabel(world().world)}</div>
                  </div>
                )}
              </For>
            </div>
            <p class="admin-panel__note">{t("dashboard-reconciliation-in-transit-note")}</p>
          </>
        )}
      </Show>
    </section>
  );
}

/** 托管缺失数与偿付缺口：大于零即为差异，零为正常。 */
function discrepancyTone(count: number): DashboardTone {
  return count > 0 ? "error" : "ok";
}
