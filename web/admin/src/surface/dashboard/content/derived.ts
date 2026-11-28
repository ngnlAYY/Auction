// 由聚合快照派生的指标状态；判断顺序与旧版 Dashboard 一致。
// 先看数据是否可用，再判失败/降级；各面板只消费结果，保证同一快照在任何面板里得到一致的状态文案。

import { t } from "@auction/web-shared/ui/i18n";

import type { AdminDashboardSnapshot } from "../../../api/dashboard";

/** Dashboard 面板统一使用的状态色调。 */
export type DashboardTone = "neutral" | "warn" | "ok" | "error";

/** 同一快照推导出的派生状态，避免每个面板各自重复推导同一指标。 */
export interface DashboardDerived {
  readonly pollState: DashboardTone;
  readonly pollStatus: string;
  readonly hasRealtimeConsumers: boolean;
  readonly pending: number | null;
  readonly failures: number | null;
  readonly outboxState: DashboardTone;
  readonly outboxStatus: string;
  readonly auditFailures: number;
  readonly auditMetricsUnavailable: boolean;
  readonly auditState: DashboardTone;
  readonly auditStatus: string;
  readonly fulfillmentBacklog: boolean;
  readonly fulfillmentState: DashboardTone;
  readonly recoveryState: DashboardTone;
  readonly recoveryStatus: string;
  readonly reconciliationDrift: boolean;
  readonly reconciliationState: DashboardTone;
}

/** 从同一快照推导面板共享状态；普通对象让装配层的按值比较能识别等价快照。 */
export function deriveDashboard(snapshot: AdminDashboardSnapshot): DashboardDerived {
  const mail = snapshot.mail.data;
  let pollState: DashboardTone;
  let pollStatus: string;
  if (snapshot.mail.status === "unknown" || mail === null) {
    pollState = "neutral";
    pollStatus = t("dashboard-legacy-poll-unknown");
  } else if (mail.pollers.length === 0) {
    pollState = "neutral";
    pollStatus = t("dashboard-legacy-poll-no-pollers");
  } else if (mail.pollers.some((poller) => poller.consecutive_failures > 0)) {
    pollState = "warn";
    pollStatus = t("dashboard-legacy-poll-degraded");
  } else if (mail.pollers.some((poller) => poller.paused)) {
    pollState = "neutral";
    pollStatus = t("dashboard-legacy-poll-paused");
  } else if (snapshot.mail.status === "ok") {
    pollState = "ok";
    pollStatus = t("dashboard-legacy-poll-running");
  } else {
    pollState = "warn";
    pollStatus = t("dashboard-legacy-poll-degraded");
  }

  const realtime = snapshot.realtime.data;
  const hasRealtimeConsumers = realtime !== null && realtime.lags.length > 0;
  const pending = realtime === null ? null : realtime.total_lag;
  const failures = realtime === null ? null : realtime.failure.count;
  const hasFailures = failures !== null && failures > 0;
  const hasPending = pending !== null && pending > 0;
  let outboxState: DashboardTone;
  let outboxStatus: string;
  if (realtime === null) {
    outboxState = "neutral";
    outboxStatus = t("dashboard-legacy-outbox-status-unavailable");
  } else if (hasFailures) {
    outboxState = "error";
    outboxStatus = t("dashboard-legacy-outbox-status-dead-letters");
  } else if (!hasRealtimeConsumers) {
    outboxState = "neutral";
    outboxStatus = t("dashboard-legacy-outbox-status-no-consumers");
  } else if (hasPending) {
    outboxState = "warn";
    outboxStatus = t("dashboard-legacy-outbox-status-lagging");
  } else {
    outboxState = "ok";
    outboxStatus = t("dashboard-legacy-outbox-status-ok");
  }

  const reconciliation = snapshot.reconciliation.data;
  const reconciliationDrift =
    reconciliation?.worlds.some(
      (world) => world.custody_missing_count > 0 || world.solvency_shortfall_credits > 0,
    ) ?? false;
  let reconciliationState: DashboardTone;
  if (reconciliation === null) {
    reconciliationState = "neutral";
  } else if (reconciliationDrift) {
    reconciliationState = "error";
  } else {
    reconciliationState = "ok";
  }

  const auditFailures = snapshot.health.audit_failure_count;
  const auditMetricsUnavailable = snapshot.health.missing_services.length > 0;
  let auditState: DashboardTone;
  if (auditFailures > 0) {
    auditState = "error";
  } else if (auditMetricsUnavailable) {
    auditState = "neutral";
  } else {
    auditState = "ok";
  }
  let auditStatus: string;
  if (auditFailures > 0) {
    auditStatus = t("dashboard-legacy-audit-status-failures");
  } else if (auditMetricsUnavailable) {
    auditStatus = t("dashboard-legacy-audit-status-unknown");
  } else {
    auditStatus = t("dashboard-legacy-audit-status-ok");
  }

  // 状态由服务端按同一告警策略阈值裁决；页面不再用“计数 > 0”建立第二套口径。
  const fulfillmentBacklog = snapshot.fulfillment.status === "degraded";
  let fulfillmentState: DashboardTone;
  if (snapshot.fulfillment.data === null) {
    fulfillmentState = "neutral";
  } else if (fulfillmentBacklog) {
    fulfillmentState = "error";
  } else if (snapshot.fulfillment.status === "ok") {
    fulfillmentState = "ok";
  } else {
    fulfillmentState = "neutral";
  }

  const recovery = snapshot.recovery.data;
  const recoveryActionable = snapshot.recovery.status === "degraded";
  const recoveryEvidenceUnavailable =
    recovery?.worlds.some(
      (world) =>
        world.balance_status === "unknown" ||
        world.balance_status === "unavailable" ||
        world.evidence_status === "unknown" ||
        world.evidence_status === "unavailable",
    ) ?? false;
  const recoveryEvidenceWarning =
    recovery?.worlds.some(
      (world) =>
        world.balance_status === "low" ||
        world.balance_status === "stale" ||
        world.balance_status === "busy" ||
        world.evidence_status === "stale",
    ) ?? false;
  let recoveryState: DashboardTone;
  if (recovery === null) {
    recoveryState = "neutral";
  } else if (recoveryEvidenceUnavailable) {
    recoveryState = "neutral";
  } else if (recoveryActionable) {
    recoveryState = "error";
  } else if (recoveryEvidenceWarning) {
    recoveryState = "warn";
  } else if (snapshot.recovery.status === "ok") {
    recoveryState = "ok";
  } else {
    recoveryState = "warn";
  }
  let recoveryStatus: string;
  if (recovery === null || recoveryEvidenceUnavailable) {
    recoveryStatus = t("dashboard-legacy-recovery-status-unavailable");
  } else if (recoveryState === "ok") {
    recoveryStatus = t("dashboard-legacy-recovery-status-ok");
  } else {
    recoveryStatus = t("dashboard-legacy-recovery-status-degraded");
  }

  return {
    pollState,
    pollStatus,
    hasRealtimeConsumers,
    pending,
    failures,
    outboxState,
    outboxStatus,
    auditFailures,
    auditMetricsUnavailable,
    auditState,
    auditStatus,
    fulfillmentBacklog,
    fulfillmentState,
    recoveryState,
    recoveryStatus,
    reconciliationDrift,
    reconciliationState,
  };
}
