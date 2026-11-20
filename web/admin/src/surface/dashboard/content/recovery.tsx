// 结算恢复面板：逾期/待上架/失败动作计数与逐世界余额证据。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { AdminDashboardSnapshot } from "../../../api/dashboard";
import { adminWorldLabel } from "../../presentation";
import type { DashboardDerived, DashboardTone } from "./derived";
import { Badge, Card } from "./widgets";

/** 恢复健康面板的输入；面板保持挂载，随快照与共享派生值更新。 */
export interface RecoveryPanelProps {
  readonly snapshot: AdminDashboardSnapshot;
  readonly derived: DashboardDerived;
}

/** 恢复健康面板；分区不可用时只显示稳定提示，不虚构计数。 */
export function RecoveryPanel(props: RecoveryPanelProps): JSX.Element {
  const recovery = createValueMemo(() => props.snapshot.recovery.data);
  const recoveryState = createValueMemo(() => props.derived.recoveryState);
  const recoveryStatus = createValueMemo(() => props.derived.recoveryStatus);
  return (
    <section class="admin-dashboard-panel" aria-labelledby="dashboard-recovery-title">
      <h2 id="dashboard-recovery-title">{t("dashboard-legacy-recovery-health")}</h2>
      <Show
        when={recovery()}
        fallback={
          <p class="admin-dashboard-status">{t("dashboard-legacy-recovery-status-unavailable")}</p>
        }
      >
        {(data) => (
          <>
            <div class="admin-dashboard-cards">
              <Card
                label={t("dashboard-legacy-recovery-health")}
                value={<Badge state={recoveryState()} value={recoveryStatus()} />}
              />
              <Card
                label={t("dashboard-legacy-recovery-payment-overdue")}
                value={data().payment_overdue_count.toString()}
              />
              <Card
                label={t("dashboard-legacy-recovery-relist-pending")}
                value={data().relist_pending_count.toString()}
              />
              <Card
                label={t("dashboard-legacy-recovery-failed-actions")}
                value={data().failed_action_count.toString()}
              />
              <Card
                label={t("dashboard-legacy-recovery-manual-reconcile")}
                value={data().manual_reconcile_count.toString()}
              />
              <Card
                label={t("dashboard-legacy-recovery-oldest-overdue")}
                value={t("dashboard-legacy-recovery-age", {
                  seconds: data().oldest_overdue_age_seconds,
                })}
              />
            </div>
            <h3>{t("dashboard-legacy-recovery-worlds")}</h3>
            <div class="admin-dashboard-cards">
              <For each={data().worlds} keyed={(world) => world.world}>
                {(world) => (
                  <div class="admin-dashboard-card">
                    <div class="admin-dashboard-value">
                      <span class="admin-dashboard-recovery-world">
                        {/* 只记账状态保留后端历史观察，但不能把旧金额呈为当前余额。 */}
                        <Show when={world().balance_status !== "not_applicable"}>
                          <span>
                            {world().balance_observed_at !== null &&
                            world().balance_status !== "unknown" &&
                            world().balance_status !== "unavailable"
                              ? t("dashboard-legacy-recovery-balance-credits", {
                                  credits: world().balance_credits,
                                })
                              : t("dashboard-legacy-no-data")}
                          </span>
                        </Show>
                        <Badge
                          state={recoveryBalanceTone(world().balance_status)}
                          value={recoveryBalanceStatus(world().balance_status)}
                        />
                        <span class="admin-dashboard-label">
                          {t("dashboard-legacy-recovery-evidence-label")}{" "}
                          <Badge
                            state={recoveryEvidenceTone(world().evidence_status)}
                            value={recoveryEvidenceStatus(world().evidence_status)}
                          />
                        </span>
                        <Show when={world().balance_status === "busy"}>
                          <span class="admin-dashboard-label">
                            {t("dashboard-legacy-recovery-balance-busy-hint")}
                          </span>
                        </Show>
                      </span>
                    </div>
                    <div class="admin-dashboard-label">{adminWorldLabel(world().world)}</div>
                  </div>
                )}
              </For>
            </div>
          </>
        )}
      </Show>
    </section>
  );
}

function recoveryBalanceTone(status: string): DashboardTone {
  switch (status) {
    case "ok":
      return "ok";
    case "low":
    case "stale":
    case "busy":
      return "warn";
    default:
      return "neutral";
  }
}

function recoveryEvidenceTone(status: string): DashboardTone {
  switch (status) {
    case "fresh":
      return "ok";
    case "stale":
      return "warn";
    default:
      return "neutral";
  }
}

function recoveryBalanceStatus(status: string): string {
  switch (status) {
    case "not_applicable":
      return t("dashboard-legacy-recovery-balance-not-applicable");
    case "ok":
      return t("dashboard-legacy-recovery-balance-ok");
    case "low":
      return t("dashboard-legacy-recovery-balance-low");
    case "stale":
      return t("dashboard-legacy-recovery-balance-stale");
    case "busy":
      return t("dashboard-legacy-recovery-balance-busy");
    case "unavailable":
      return t("dashboard-legacy-recovery-balance-unavailable");
    default:
      return t("dashboard-legacy-recovery-balance-unknown");
  }
}

function recoveryEvidenceStatus(status: string): string {
  switch (status) {
    case "not_applicable":
      return t("dashboard-legacy-recovery-evidence-not-applicable");
    case "fresh":
      return t("dashboard-legacy-recovery-evidence-fresh");
    case "stale":
      return t("dashboard-legacy-recovery-evidence-stale");
    case "unavailable":
      return t("dashboard-legacy-recovery-evidence-unavailable");
    default:
      return t("dashboard-legacy-recovery-evidence-unknown");
  }
}
