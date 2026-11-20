// Dashboard 快捷入口与顶部摘要卡。
// 快捷入口顺序、语义卡片 id 与轮询状态徽标沿用旧版 Dashboard。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { Link } from "@auction/web-shared/router";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

import type { AdminDashboardSnapshot } from "../../../api/dashboard";
import type { DashboardDerived } from "./derived";
import { countText } from "./widgets";

/** 顶部六格指标的输入；指标在稳定挂载的摘要里随快照更新。 */
export interface SummaryMetricsProps {
  readonly snapshot: AdminDashboardSnapshot;
  readonly derived: DashboardDerived;
}

/** 顶部六格指标；值、副标题与 `data-state` 与旧版逐个一致。 */
export function SummaryMetrics(props: SummaryMetricsProps): JSX.Element {
  const workerUp = createValueMemo(() => props.snapshot.worker.condition === "available");
  const activeAlerts = createValueMemo(() => {
    const data = props.snapshot.alerting.data;
    return data === null ? null : data.active_signal_count;
  });
  return (
    <div class="console-kpi console-kpi--6">
      <div class="console-kpi__cell">
        <div class="console-kpi__label">{t("dashboard-legacy-worker-channel")}</div>
        <div class="console-kpi__value">{props.snapshot.worker.condition}</div>
        <div class="console-kpi__sub">
          {workerUp() ? t("dashboard-legacy-channel-up") : t("dashboard-legacy-channel-down")}
        </div>
      </div>
      <div class="console-kpi__cell">
        <div class="console-kpi__label">{t("dashboard-legacy-serving")}</div>
        <div class="console-kpi__value">
          {props.snapshot.worker.ready ? t("dashboard-legacy-yes") : t("dashboard-legacy-no")}
        </div>
      </div>
      <div class="console-kpi__cell" data-state={props.derived.pollState}>
        <div class="console-kpi__label">{t("dashboard-legacy-poll-status")}</div>
        <div class="console-kpi__value">{props.derived.pollStatus}</div>
      </div>
      <div class="console-kpi__cell">
        <div class="console-kpi__label">{t("dashboard-legacy-audit-health")}</div>
        <div class="console-kpi__value">{props.derived.auditStatus}</div>
      </div>
      <div class="console-kpi__cell">
        <div class="console-kpi__label">{t("dashboard-legacy-outbox-pending")}</div>
        <div class="console-kpi__value">{countText(props.derived.pending)}</div>
      </div>
      <div class="console-kpi__cell">
        <div class="console-kpi__label">{t("dashboard-legacy-alerting-active")}</div>
        <div class="console-kpi__value">{countText(activeAlerts())}</div>
      </div>
    </div>
  );
}

/** 顶部摘要与快捷入口的输入；快捷入口在稳定挂载的摘要里保留原节点。 */
export interface SummarySectionProps {
  readonly snapshot: AdminDashboardSnapshot;
  readonly derived: DashboardDerived;
}

/** 顶部摘要与快捷入口；入口标题、图标装饰与正文层级按旧版输出。 */
export function SummarySection(props: SummarySectionProps): JSX.Element {
  return (
    <>
      <SummaryMetrics snapshot={props.snapshot} derived={props.derived} />
      <section class="admin-dashboard-overview" aria-labelledby="dashboard-shortcuts-title">
        <div class="admin-dashboard-overview__head">
          <h2 id="dashboard-shortcuts-title">{t("dashboard-shortcuts-title")}</h2>
        </div>
        <nav class="admin-quick-links" aria-label={t("dashboard-shortcuts-title")}>
          <Link class="admin-quick-link" href="/auctions" title={t("dashboard-open-auctions-note")}>
            <span class="admin-quick-link__icon" aria-hidden="true" />
            <span class="admin-quick-link__body">
              <strong>{t("dashboard-open-auctions")}</strong>
              <small>{t("dashboard-open-auctions-note")}</small>
            </span>
          </Link>
          <Link class="admin-quick-link" href="/mails" title={t("dashboard-open-mail-note")}>
            <span class="admin-quick-link__icon admin-quick-link__icon--mail" aria-hidden="true" />
            <span class="admin-quick-link__body">
              <strong>{t("dashboard-open-mail")}</strong>
              <small>{t("dashboard-open-mail-note")}</small>
            </span>
          </Link>
          <Link
            class="admin-quick-link"
            href="/deliveries"
            title={t("dashboard-open-deliveries-note")}
          >
            <span
              class="admin-quick-link__icon admin-quick-link__icon--delivery"
              aria-hidden="true"
            />
            <span class="admin-quick-link__body">
              <strong>{t("dashboard-open-deliveries")}</strong>
              <small>{t("dashboard-open-deliveries-note")}</small>
            </span>
          </Link>
          <Link
            class="admin-quick-link"
            href="/settlement"
            title={t("dashboard-open-settlement-note")}
          >
            <span
              class="admin-quick-link__icon admin-quick-link__icon--settlement"
              aria-hidden="true"
            />
            <span class="admin-quick-link__body">
              <strong>{t("nav-settlement")}</strong>
              <small>{t("dashboard-open-settlement-note")}</small>
            </span>
          </Link>
          <Link class="admin-quick-link" href="/outbox" title={t("dashboard-open-status-note")}>
            <span
              class="admin-quick-link__icon admin-quick-link__icon--status"
              aria-hidden="true"
            />
            <span class="admin-quick-link__body">
              <strong>{t("dashboard-open-status")}</strong>
              <small>{t("dashboard-open-status-note")}</small>
            </span>
          </Link>
          <Link class="admin-quick-link" href="/config">
            <span
              class="admin-quick-link__icon admin-quick-link__icon--config"
              aria-hidden="true"
            />
            <span class="admin-quick-link__body">
              <strong>{t("nav-config")}</strong>
            </span>
          </Link>
        </nav>
      </section>
    </>
  );
}
