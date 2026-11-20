// Dashboard 快照解码后的语义校验。
// 键闭集与“必填可空”由分区结构的 `deny_unknown_fields` 与
// `auction_contracts::deserialize_required_option` 承担；这里只检查类型表达不了的
// 跨字段一致性与取值闭集，任一不符整份快照失败关闭。显式为 null 的分区外壳字段
// （error_code、observed_at、未知分区的 data）与带 default 的轮询 diagnostics 按“没有”处理；
// 后端从不输出这些 null。

import { trimRustWhitespace } from "../session-time";
import type {
  AdminAlertingSection,
  AdminCollectionOverdueItem,
  AdminDashboardSnapshot,
  AdminExternalAccountsData,
  AdminFulfillmentData,
  AdminMailData,
  AdminRealtimeData,
  AdminReconciliationData,
} from "./types";

/** 校验类型系统无法表达的跨字段语义；任一不符都应让整份快照失败关闭。 */
export function validDashboardSemantics(snapshot: AdminDashboardSnapshot): boolean {
  return (
    (snapshot.mail.data === null || validMail(snapshot.mail.data)) &&
    (snapshot.realtime.data === null || validRealtime(snapshot.realtime.data)) &&
    (snapshot.external_accounts.data === null ||
      validExternalAccounts(snapshot.external_accounts.data)) &&
    (snapshot.fulfillment.data === null || validFulfillment(snapshot.fulfillment.data)) &&
    (snapshot.reconciliation.data === null || validReconciliation(snapshot.reconciliation.data)) &&
    validAlerting(snapshot.alerting)
  );
}

/** 轮询摘要复用邮件页的 DTO，但 dashboard 从不选择诊断；出现诊断说明 wire 形状漂移。 */
function validMail(data: AdminMailData): boolean {
  return data.pollers.every((poller) => poller.diagnostics === null);
}

function validRealtime(data: AdminRealtimeData): boolean {
  return data.total_lag >= 0;
}

/**
 * 等待领取的读取失败数与超期件数都是其子集，长期未领取清单不多于超期件数；没有等待领取时
 * 不能带等待年龄。
 */
function validFulfillment(data: AdminFulfillmentData): boolean {
  return (
    data.awaiting_collection >= 0 &&
    data.awaiting_collection_unreadable >= 0 &&
    data.awaiting_collection_unreadable <= data.awaiting_collection &&
    data.collection_overdue_count >= 0 &&
    data.collection_overdue_count <= data.awaiting_collection &&
    data.collection_overdue_items.length <= data.collection_overdue_count &&
    data.collection_overdue_items.every(validCollectionOverdueItem) &&
    data.oldest_awaiting_collection_age_seconds >= 0 &&
    (data.awaiting_collection > 0 || data.oldest_awaiting_collection_age_seconds === 0)
  );
}

/** 清单只会出现等待领取的五类义务（COD 与载体付款按等待付款口径，不在其中）。 */
function validCollectionOverdueItem(item: AdminCollectionOverdueItem): boolean {
  const text = (value: string) => value !== "" && value === trimRustWhitespace(value);
  return (
    (item.obligation_kind === "seller_payout" ||
      item.obligation_kind === "buyer_refund" ||
      item.obligation_kind === "unsold_return" ||
      item.obligation_kind === "withdrawn_return" ||
      item.obligation_kind === "winner_equipment_handoff") &&
    (item.world === "main" || item.world === "isekai") &&
    item.auction_sequence > 0 &&
    item.lot_number > 0 &&
    item.age_seconds >= 0 &&
    text(item.obligation_id) &&
    text(item.equipment_category) &&
    text(item.equipment_name) &&
    text(item.recipient) &&
    text(item.sent_at)
  );
}

/** 对账报告按业务世界汇总，最多 main 与 isekai 两项；在途件数、笔数与金额不得为负。 */
function validReconciliation(data: AdminReconciliationData): boolean {
  return (
    data.worlds.length <= 2 &&
    data.worlds.every(
      (world) =>
        world.custody_in_transit_count >= 0 &&
        world.solvency_in_transit_count >= 0 &&
        world.solvency_in_transit_credits >= 0,
    )
  );
}

function validExternalAccounts(data: AdminExternalAccountsData): boolean {
  // 忙碌时长只在忙碌期间存在，且不得为负。
  const busyAgeConsistent =
    data.mail.busy && data.mail.busy_age_seconds !== null
      ? data.mail.busy_age_seconds >= 0
      : !data.mail.busy && data.mail.busy_age_seconds === null;
  return (
    busyAgeConsistent &&
    (data.maintenance.state === "idle" ||
      data.maintenance.state === "running" ||
      data.maintenance.state === "succeeded" ||
      data.maintenance.state === "failed" ||
      data.maintenance.state === "canceled") &&
    validProjectionStatus(data.mail.pollers.status) &&
    validProjectionStatus(data.credentials.status) &&
    data.forum.write_unknown_count >= 0
  );
}

/** 子投影只允许 `available|unavailable`；不可用时空列表不是“没有条目”的事实。 */
function validProjectionStatus(status: string): boolean {
  return status === "available" || status === "unavailable";
}

/**
 * 告警关闭时分区恒为 ok 且不带观测时间；开启时分区观测时间必须等于最近一次扫描时间，
 * 两者同时缺失表示尚未扫描过。unknown 分区没有数据，外壳已由 `decodeSection` 校验。
 */
function validAlerting(section: AdminAlertingSection): boolean {
  const data = section.data;
  if (data === null) {
    return true;
  }
  if (!data.enabled) {
    return section.status === "ok" && section.observed_at === null;
  }
  return section.observed_at === data.last_observed_at;
}
