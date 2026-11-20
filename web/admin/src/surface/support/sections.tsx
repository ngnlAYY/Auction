// 玩家支持汇总的分区表格：出价、成交、义务、投递与通知。
// 所有字段都来自 `auction-contracts` 的只读安全投影；页面不展示 payload、正文或凭据。
// 有既有译文映射的状态（世界、拍品状态、投递状态与类别）复用对应 helper；其余机读状态码
// 以 `code` 原样呈现，不在本页另建映射。

import type {
  AdminPlayerSupportBidResponse,
  AdminPlayerSupportBundleResponse,
  AdminPlayerSupportDealResponse,
  AdminPlayerSupportNotificationResponse,
  AdminPlayerSupportObligationResponse,
} from "@auction/contracts/dto/admin/delivery";
import { Link } from "@auction/web-shared/router";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { Delivery } from "../../api/delivery/decode";
import { deliveryCategoryLabel, deliveryStatusLabel } from "../deliveries/labels";
import { adminWorldLabel, formatAdminTime, lotStatusLabel } from "../presentation";

/** 汇总头部：用户身份与截断提示。 */
export function summary(bundle: AdminPlayerSupportBundleResponse): JSX.Element {
  return (
    <section class="admin-panel" aria-labelledby="support-summary-title">
      <h2 id="support-summary-title">{t("support-summary-title")}</h2>
      <Show when={bundle.user} fallback={<p class="admin-panel__note">{t("support-no-user")}</p>}>
        {(user) => (
          <dl class="admin-support-summary__facts">
            <dt>{t("support-user")}</dt>
            <dd>
              <TruncatedText value={user().username} portal={true} legacyDom={true} />
            </dd>
            <dt>{t("support-user-id")}</dt>
            <dd>
              <TruncatedText value={user().id} portal={true} legacyDom={true} />
            </dd>
            <dt>{t("support-user-status")}</dt>
            <dd>
              {user().status === "blacklisted"
                ? t("admin-users-status-blacklisted")
                : t("admin-users-status-active")}
            </dd>
            <Show when={user().game_player_id}>
              {(identity) => (
                <>
                  <dt>{t("support-user-game-id")}</dt>
                  <dd>
                    <TruncatedText value={identity()} portal={true} legacyDom={true} />
                  </dd>
                </>
              )}
            </Show>
          </dl>
        )}
      </Show>
      <Show when={bundle.truncated}>
        <p class="admin-panel__note" role="status">
          {t("support-truncated")}
        </p>
      </Show>
      <Show when={bundle.references.length > 0}>
        <ul class="admin-support-references">
          <For each={bundle.references} keyed={false}>
            {(reference) => <li>{`${reference().kind}: ${reference().id}`}</li>}
          </For>
        </ul>
      </Show>
    </section>
  );
}

/** 出价分区。 */
export function bids(bids: readonly AdminPlayerSupportBidResponse[]): JSX.Element {
  return (
    <section class="admin-panel admin-support-section" aria-labelledby="support-bids-title">
      <h2 id="support-bids-title">{t("support-bids-title")}</h2>
      <Show
        when={bids.length === 0}
        fallback={
          <div class="admin-shell__table-wrap">
            <table class="admin-shell__table">
              <TableColumns layout="admin-support-bids" widths={[18, 10, 10, 12, 12, 18, 20]} />
              <thead>
                <tr>
                  <th scope="col">{t("support-lot")}</th>
                  <th scope="col">{t("support-world")}</th>
                  <th scope="col">{t("support-lot-number")}</th>
                  <th scope="col">{t("support-amount")}</th>
                  <th scope="col">{t("support-mechanism")}</th>
                  <th scope="col">{t("delivery-status")}</th>
                  <th scope="col">{t("support-created-at")}</th>
                </tr>
              </thead>
              <tbody>
                <For each={bids} keyed={false}>
                  {(bid) => (
                    <tr>
                      <td>{lotLink(bid().auction_id, bid().lot_id)}</td>
                      <td>{adminWorldLabel(bid().world_id)}</td>
                      <td>{bid().lot_number}</td>
                      <td>{formatCreditAmount(bid().amount)}</td>
                      <td>
                        <code>{bid().mechanism}</code>
                      </td>
                      <td>{lotStatusLabel(bid().lot_status)}</td>
                      <td>{formatAdminTime(bid().created_at)}</td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        }
      >
        <p class="admin-panel__note">{t("support-empty")}</p>
      </Show>
    </section>
  );
}

/** 成交分区。 */
export function deals(deals: readonly AdminPlayerSupportDealResponse[]): JSX.Element {
  return (
    <section class="admin-panel admin-support-section" aria-labelledby="support-deals-title">
      <h2 id="support-deals-title">{t("support-deals-title")}</h2>
      <Show
        when={deals.length === 0}
        fallback={
          <div class="admin-shell__table-wrap">
            <table class="admin-shell__table">
              <TableColumns layout="admin-support-deals" widths={[18, 26, 10, 10, 14, 22]} />
              <thead>
                <tr>
                  <th scope="col">{t("support-lot")}</th>
                  <th scope="col">{t("support-equipment")}</th>
                  <th scope="col">{t("support-world")}</th>
                  <th scope="col">{t("support-lot-number")}</th>
                  <th scope="col">{t("support-final-price")}</th>
                  <th scope="col">{t("support-ended-at")}</th>
                </tr>
              </thead>
              <tbody>
                <For each={deals} keyed={false}>
                  {(deal) => (
                    <tr>
                      <td>{lotLink(deal().auction_id, deal().lot_id)}</td>
                      <td>
                        <TruncatedText
                          value={deal().equipment_name}
                          portal={true}
                          legacyDom={true}
                        />
                      </td>
                      <td>{adminWorldLabel(deal().world_id)}</td>
                      <td>{deal().lot_number}</td>
                      <td>{formatCreditAmount(deal().final_price)}</td>
                      <td>
                        <Show when={deal().effective_end_at} fallback={t("value-empty")}>
                          {(at) => <>{formatAdminTime(at())}</>}
                        </Show>
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        }
      >
        <p class="admin-panel__note">{t("support-empty")}</p>
      </Show>
    </section>
  );
}

/** 义务分区。 */
export function obligations(
  obligations: readonly AdminPlayerSupportObligationResponse[],
): JSX.Element {
  return (
    <section class="admin-panel admin-support-section" aria-labelledby="support-obligations-title">
      <h2 id="support-obligations-title">{t("support-obligations-title")}</h2>
      <Show
        when={obligations.length === 0}
        fallback={
          <div class="admin-shell__table-wrap">
            <table class="admin-shell__table">
              <TableColumns
                layout="admin-support-obligations"
                widths={[18, 16, 10, 12, 12, 14, 18]}
              />
              <thead>
                <tr>
                  <th scope="col">{t("support-obligation")}</th>
                  <th scope="col">{t("support-kind")}</th>
                  <th scope="col">{t("support-world")}</th>
                  <th scope="col">{t("support-amount")}</th>
                  <th scope="col">{t("support-business-state")}</th>
                  <th scope="col">{t("support-fulfillment")}</th>
                  <th scope="col">{t("support-delivery")}</th>
                </tr>
              </thead>
              <tbody>
                <For each={obligations} keyed={false}>
                  {(obligation) => (
                    <tr>
                      <td>
                        <TruncatedText
                          value={obligation().obligation_id}
                          portal={true}
                          legacyDom={true}
                        />
                      </td>
                      <td>
                        <code>{obligation().obligation_kind}</code>
                      </td>
                      <td>{adminWorldLabel(obligation().world_id)}</td>
                      <td>{formatCreditAmount(obligation().amount)}</td>
                      <td>
                        <code>{obligation().business_state}</code>
                      </td>
                      <td>
                        <Show when={obligation().fulfillment_state} fallback={t("value-empty")}>
                          {(state) => <code>{state()}</code>}
                        </Show>
                        <Show when={obligation().fulfillment_error_code !== null}>
                          <span class="admin-panel__note">
                            {" "}
                            ({obligation().fulfillment_error_code})
                          </span>
                        </Show>
                      </td>
                      <td>
                        <Show when={obligation().delivery_id} fallback={t("value-empty")}>
                          {(deliveryId) => (
                            <Link href={`/deliveries/${deliveryId()}`}>
                              {t("support-open-delivery")}
                            </Link>
                          )}
                        </Show>
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        }
      >
        <p class="admin-panel__note">{t("support-empty")}</p>
      </Show>
    </section>
  );
}

/** 投递分区。 */
export function deliveries(deliveries: readonly Delivery[]): JSX.Element {
  return (
    <section class="admin-panel admin-support-section" aria-labelledby="support-deliveries-title">
      <h2 id="support-deliveries-title">{t("support-deliveries-title")}</h2>
      <Show
        when={deliveries.length === 0}
        fallback={
          <div class="admin-shell__table-wrap">
            <table class="admin-shell__table">
              <TableColumns
                layout="admin-support-deliveries"
                widths={[10, 20, 14, 12, 12, 12, 20]}
              />
              <thead>
                <tr>
                  <th scope="col">{t("delivery-id")}</th>
                  <th scope="col">{t("delivery-to")}</th>
                  <th scope="col">{t("delivery-template")}</th>
                  <th scope="col">{t("delivery-category")}</th>
                  <th scope="col">{t("delivery-status")}</th>
                  <th scope="col">{t("delivery-outcome")}</th>
                  <th scope="col">{t("support-created-at")}</th>
                </tr>
              </thead>
              <tbody>
                <For each={deliveries} keyed={false}>
                  {(delivery) => (
                    <tr>
                      <td>
                        <Link href={`/deliveries/${delivery().id}`}>
                          {Array.from(delivery().id).slice(0, 8).join("")}
                        </Link>
                      </td>
                      <td>
                        <TruncatedText
                          value={delivery().recipient}
                          portal={true}
                          legacyDom={true}
                        />
                      </td>
                      <td>{delivery().template ?? t("value-empty")}</td>
                      <td>
                        <Show when={delivery().category} fallback={t("value-empty")}>
                          {(category) => <>{deliveryCategoryLabel(category())}</>}
                        </Show>
                      </td>
                      <td>{deliveryStatusLabel(delivery().status)}</td>
                      <td>
                        <Show
                          when={delivery().retirement_reason}
                          fallback={
                            <Show
                              when={delivery().suppression_reason}
                              fallback={delivery().failure_code ?? ""}
                            >
                              {(reason) => (
                                <>{t("delivery-outcome-suppressed", { reason: reason() })}</>
                              )}
                            </Show>
                          }
                        >
                          {(reason) => <>{t("delivery-outcome-retired", { reason: reason() })}</>}
                        </Show>
                      </td>
                      <td>{formatAdminTime(delivery().created_at)}</td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        }
      >
        <p class="admin-panel__note">{t("support-empty")}</p>
      </Show>
    </section>
  );
}

/** 通知分区。 */
export function notifications(
  notifications: readonly AdminPlayerSupportNotificationResponse[],
): JSX.Element {
  return (
    <section
      class="admin-panel admin-support-section"
      aria-labelledby="support-notifications-title"
    >
      <h2 id="support-notifications-title">{t("support-notifications-title")}</h2>
      <Show
        when={notifications.length === 0}
        fallback={
          <div class="admin-shell__table-wrap">
            <table class="admin-shell__table">
              <TableColumns
                layout="admin-support-notifications"
                widths={[18, 10, 14, 12, 12, 18, 16]}
              />
              <thead>
                <tr>
                  <th scope="col">{t("support-notification")}</th>
                  <th scope="col">{t("support-channel")}</th>
                  <th scope="col">{t("support-kind")}</th>
                  <th scope="col">{t("delivery-status")}</th>
                  <th scope="col">{t("delivery-outcome")}</th>
                  <th scope="col">{t("support-delivery")}</th>
                  <th scope="col">{t("support-created-at")}</th>
                </tr>
              </thead>
              <tbody>
                <For each={notifications} keyed={false}>
                  {(notification) => (
                    <tr>
                      <td>
                        <TruncatedText
                          value={notification().notification_id}
                          portal={true}
                          legacyDom={true}
                        />
                      </td>
                      <td>
                        <code>{notification().channel}</code>
                      </td>
                      <td>{notification().kind ?? t("value-empty")}</td>
                      <td>
                        <code>{notification().state}</code>
                      </td>
                      <td>{notification().state_reason ?? ""}</td>
                      <td>
                        <Show when={notification().delivery_id} fallback={t("value-empty")}>
                          {(deliveryId) => (
                            <Link href={`/deliveries/${deliveryId()}`}>
                              {t("support-open-delivery")}
                            </Link>
                          )}
                        </Show>
                      </td>
                      <td>{formatAdminTime(notification().created_at)}</td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        }
      >
        <p class="admin-panel__note">{t("support-empty")}</p>
      </Show>
    </section>
  );
}

/** 拍品 UUID 在固定列宽下会被省略；用截断文本补完整值浮层，链接本身保留唯一焦点。 */
function lotLink(auctionId: string, lotId: string): JSX.Element {
  return (
    <Link href={`/auctions/${auctionId}/lots`}>
      <TruncatedText value={lotId} focusable={false} portal={true} legacyDom={true} />
    </Link>
  );
}
