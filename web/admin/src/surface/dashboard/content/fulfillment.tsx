// 履约健康面板：积压状态、分项计数、已寄出等待领取分组、长期未领取清单与需要人工介入的提示。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { Link } from "@auction/web-shared/router";
import { Time } from "@auction/web-shared/ui/components/time";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { AdminCollectionOverdueItem, AdminDashboardSnapshot } from "../../../api/dashboard";
import { adminLotCodeFor } from "../../lots/presentation";
import { adminWorldLabel } from "../../presentation";
import type { DashboardDerived } from "./derived";
import { Badge, Card, SemanticCard } from "./widgets";

/** 等待领取按整天展示寄出后的等待时长（向下取整）。 */
const SECONDS_PER_DAY = 86_400;

/** 履约健康面板的输入；面板保持挂载，随快照与共享派生值更新。 */
export interface FulfillmentPanelProps {
  readonly snapshot: AdminDashboardSnapshot;
  readonly derived: DashboardDerived;
}

/** 履约健康面板；长数值状态由统一的派生结果给出。 */
export function FulfillmentPanel(props: FulfillmentPanelProps): JSX.Element {
  const fulfillment = createValueMemo(() => props.snapshot.fulfillment.data);
  const fulfillmentState = createValueMemo(() => props.derived.fulfillmentState);
  const fulfillmentBacklog = createValueMemo(() => props.derived.fulfillmentBacklog);
  return (
    <section class="admin-dashboard-panel" aria-labelledby="dashboard-fulfillment-title">
      <h2 id="dashboard-fulfillment-title">{t("dashboard-legacy-fulfillment-health")}</h2>
      <Show
        when={fulfillment()}
        fallback={
          <p class="admin-dashboard-status">
            {t("dashboard-legacy-fulfillment-status-unavailable")}
          </p>
        }
      >
        {(current) => (
          <>
            <div class="admin-dashboard-cards">
              <SemanticCard
                id="dashboard-fulfillment-state"
                label={t("dashboard-legacy-fulfillment-health")}
                state={fulfillmentState()}
                value={
                  <Badge state={fulfillmentState()} value={fulfillmentStatus(fulfillmentState())} />
                }
              />
              <Card
                label={t("dashboard-legacy-fulfillment-pending")}
                value={
                  <Badge
                    state={current().pending > 0 ? "warn" : "ok"}
                    value={current().pending.toString()}
                  />
                }
              />
              <Card
                label={t("dashboard-legacy-fulfillment-blocked")}
                value={
                  <Badge
                    state={current().blocked > 0 ? "error" : "ok"}
                    value={current().blocked.toString()}
                  />
                }
              />
              <Card
                label={t("dashboard-legacy-fulfillment-failed")}
                value={
                  <Badge
                    state={current().failed > 0 ? "error" : "ok"}
                    value={current().failed.toString()}
                  />
                }
              />
              <Card
                label={t("dashboard-legacy-fulfillment-manual-reconcile")}
                value={
                  <Badge
                    state={current().manual_reconcile > 0 ? "error" : "ok"}
                    value={current().manual_reconcile.toString()}
                  />
                }
              />
              <Card
                label={t("dashboard-legacy-fulfillment-waiting-payment")}
                value={current().waiting_payment.toString()}
              />
              <Card
                label={t("dashboard-legacy-fulfillment-waiting-prerequisite")}
                value={current().waiting_prerequisite.toString()}
              />
              <Card
                label={t("dashboard-legacy-fulfillment-oldest-backlog")}
                value={t("dashboard-legacy-duration-seconds", {
                  seconds: current().oldest_backlog_age_seconds,
                })}
              />
            </div>
            {/* 已寄出、只等收件人领取的义务单列：不计入待处理，但“读不到”与“未领取”分开显示。
                主计数是寄出时长达到长期未领取阈值的件数，总等待数只作背景。 */}
            <h3>{t("dashboard-fulfillment-awaiting-title")}</h3>
            <div class="admin-dashboard-cards">
              <Card
                label={t("dashboard-fulfillment-awaiting-overdue")}
                value={
                  <Badge
                    state={current().collection_overdue_count > 0 ? "warn" : "ok"}
                    value={current().collection_overdue_count.toString()}
                  />
                }
              />
              <Card
                label={t("dashboard-fulfillment-awaiting-total")}
                value={current().awaiting_collection.toString()}
              />
              <Card
                label={t("dashboard-fulfillment-awaiting-oldest")}
                value={t("dashboard-fulfillment-awaiting-days", {
                  days: Math.trunc(
                    current().oldest_awaiting_collection_age_seconds / SECONDS_PER_DAY,
                  ),
                })}
              />
              <Card
                label={t("dashboard-fulfillment-awaiting-unreadable")}
                value={
                  <Badge
                    state={current().awaiting_collection_unreadable > 0 ? "warn" : "ok"}
                    value={current().awaiting_collection_unreadable.toString()}
                  />
                }
              />
            </div>
            <p class="admin-panel__note">{t("dashboard-fulfillment-awaiting-note")}</p>
            <Show when={current().collection_overdue_items.length > 0}>
              <OverdueList
                items={current().collection_overdue_items}
                overdueCount={current().collection_overdue_count}
              />
            </Show>
            <Show when={fulfillmentBacklog()}>
              <aside
                class="admin-dashboard-attention"
                aria-labelledby="dashboard-fulfillment-attention-title"
                data-state="error"
              >
                <div class="admin-dashboard-attention__copy" role="status" aria-atomic="true">
                  <h3 id="dashboard-fulfillment-attention-title">
                    {t("dashboard-fulfillment-attention-title")}
                  </h3>
                  <p>{t("dashboard-fulfillment-attention-note")}</p>
                </div>
                <nav
                  class="admin-dashboard-attention__actions"
                  aria-label={t("dashboard-fulfillment-attention-title")}
                >
                  <Link href="/deliveries">{t("dashboard-open-deliveries")}</Link>
                  <Link href="/settlement">{t("nav-settlement")}</Link>
                </nav>
              </aside>
            </Show>
          </>
        )}
      </Show>
    </section>
  );
}

interface OverdueListProps {
  readonly items: readonly AdminCollectionOverdueItem[];
  readonly overdueCount: number;
}

/** 长期未领取清单：只列寄出时长达到阈值的等待领取，寄出最早优先；条数有上限，总超期件数在
 * 说明里给出。只读展示，不提供重发或撤回入口。 */
function OverdueList(props: OverdueListProps): JSX.Element {
  return (
    <>
      <h3 id="dashboard-fulfillment-overdue-title">{t("dashboard-fulfillment-overdue-title")}</h3>
      <p class="admin-panel__note">
        {t("dashboard-fulfillment-overdue-summary", {
          count: props.overdueCount,
          shown: props.items.length,
        })}
      </p>
      <div class="admin-shell__table-wrap">
        <table class="admin-shell__table" aria-labelledby="dashboard-fulfillment-overdue-title">
          <thead>
            <tr>
              <th scope="col">{t("dashboard-fulfillment-overdue-lot")}</th>
              <th scope="col">{t("dashboard-fulfillment-overdue-equipment")}</th>
              <th scope="col">{t("dashboard-fulfillment-overdue-kind")}</th>
              <th scope="col">{t("dashboard-fulfillment-overdue-recipient")}</th>
              <th scope="col">{t("dashboard-fulfillment-overdue-sent-at")}</th>
              <th scope="col">{t("dashboard-fulfillment-overdue-waited")}</th>
              <th scope="col">{t("dashboard-fulfillment-overdue-read")}</th>
            </tr>
          </thead>
          <tbody>
            <For each={props.items} keyed={(item) => item.obligation_id}>
              {(item) => (
                <tr>
                  <td>
                    {t("admin-auctions-row-heading", {
                      world: adminWorldLabel(item().world),
                      number: item().auction_sequence,
                    })}{" "}
                    {overdueLotCode(item())}
                  </td>
                  <td>{item().equipment_name}</td>
                  <td>{overdueKindLabel(item().obligation_kind)}</td>
                  <td>{item().recipient}</td>
                  <td>
                    <Time value={item().sent_at} />
                  </td>
                  <td>
                    {t("dashboard-fulfillment-awaiting-days", {
                      days: Math.trunc(item().age_seconds / SECONDS_PER_DAY),
                    })}
                  </td>
                  <td>
                    {/* 没有读取失败记录只说明最近一次读取没有失败，不是“已读到、
                        确认未领取”的证据。 */}
                    <Show
                      when={item().last_read_failed}
                      fallback={t("dashboard-fulfillment-overdue-read-no-failure")}
                    >
                      <Badge state="warn" value={t("dashboard-fulfillment-overdue-read-failed")} />
                    </Show>
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
    </>
  );
}

/** 清单项的拍品代码：分类前缀加分类内编号（服务端 `lot_number` 即 `lots.category_number`），
 * 与 Admin lot 页、结算报表一致。 */
function overdueLotCode(item: AdminCollectionOverdueItem): string {
  return adminLotCodeFor(item.equipment_category, item.lot_number);
}

/** 等待领取义务类别的当前语言名称；解码已拒绝闭集外的值，这里只兜底原样显示。 */
function overdueKindLabel(kind: string): string {
  switch (kind) {
    case "seller_payout":
      return t("dashboard-fulfillment-overdue-kind-seller-payout");
    case "buyer_refund":
      return t("dashboard-fulfillment-overdue-kind-buyer-refund");
    case "unsold_return":
      return t("dashboard-fulfillment-overdue-kind-unsold-return");
    case "withdrawn_return":
      return t("dashboard-fulfillment-overdue-kind-withdrawn-return");
    case "winner_equipment_handoff":
      return t("dashboard-fulfillment-overdue-kind-handoff");
    default:
      return kind;
  }
}

function fulfillmentStatus(state: string): string {
  switch (state) {
    case "ok":
      return t("dashboard-legacy-fulfillment-status-ok");
    case "error":
      return t("dashboard-legacy-fulfillment-status-degraded");
    default:
      return t("dashboard-legacy-fulfillment-status-unavailable");
  }
}
