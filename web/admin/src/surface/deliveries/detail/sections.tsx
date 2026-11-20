// 投递详情的只读分区：摘要、冻结尝试、Sent 证据、托管、处置与来源义务。

import type {
  AdminDeliveryAttemptResponse as DeliveryAttempt,
  AdminDeliveryCustodyResponse as DeliveryCustody,
  AdminDeliveryDetailResponse as DeliveryDetail,
  AdminDeliveryDispositionResponse as DeliveryDisposition,
  AdminDeliveryEvidenceResponse as DeliveryEvidence,
  AdminDeliveryObligationResponse as DeliveryObligation,
  AdminDeliveryRecoveryResponse as DeliveryRecovery,
} from "@auction/contracts/dto/admin/delivery";
import { Link } from "@auction/web-shared/router";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import { adminWorldLabel, formatAdminTime } from "../../presentation";
import { settlementReturnCase } from "../../returns/labels";
import { deliveryDisplayType, deliveryStatusLabel } from "../labels";
import { evidenceKindLabel } from "../sent-proof/render";
import {
  attemptAmounts,
  attemptMailId,
  dispositionActor,
  dispositionKindLabel,
  dispositionStatus,
  evidenceCustody,
  optionalInteger,
  optionalText,
} from "./sections-format";

/** 摘要分区属性。 */
export interface DeliverySummaryProps {
  readonly detail: DeliveryDetail;
}

/** 投递摘要；所有可选字段缺失时显示统一的空值占位。 */
export function DeliverySummary(props: DeliverySummaryProps): JSX.Element {
  const delivery = () => props.detail.delivery;
  return (
    <section class="admin-panel" aria-labelledby="delivery-detail-summary-title">
      <h2 id="delivery-detail-summary-title">{t("delivery-detail-summary")}</h2>
      <dl class="admin-delivery-detail__grid">
        <dt>{t("delivery-id")}</dt>
        <dd>
          <TruncatedText value={delivery().id} portal />
        </dd>
        <dt>{t("delivery-type")}</dt>
        <dd>{deliveryDisplayType(delivery().origin_kind)}</dd>
        <dt>{t("delivery-origin-id")}</dt>
        <dd>
          <TruncatedText value={delivery().origin_id} portal />
        </dd>
        {/* 结算退件 Take 由退件案件推进；投递台不提供人工动作，处理入口在案件页。 */}
        <Show when={settlementReturnCase(delivery().origin_kind, delivery().origin_id)}>
          {(caseId) => (
            <>
              <dt>{t("delivery-detail-settlement-return")}</dt>
              <dd>
                <Link href={`/settlement-returns/${caseId()}`}>
                  {t("delivery-detail-settlement-return-open")}
                </Link>
              </dd>
            </>
          )}
        </Show>
        <dt>{t("delivery-to")}</dt>
        <dd>{delivery().recipient}</dd>
        <dt>{t("delivery-cod")}</dt>
        <dd>{optionalInteger(delivery().cod)}</dd>
        <dt>{t("delivery-status")}</dt>
        <dd>
          <span class="admin-delivery-badge">{deliveryStatusLabel(delivery().status)}</span>
        </dd>
        <dt>{t("delivery-detail-fencing")}</dt>
        <dd>{delivery().fencing_version}</dd>
        <dt>{t("delivery-attempts")}</dt>
        <dd>{delivery().attempt_count}</dd>
        <dt>{t("delivery-detail-created")}</dt>
        <dd>{formatAdminTime(delivery().created_at)}</dd>
        <dt>{t("delivery-detail-updated")}</dt>
        <dd>{formatAdminTime(delivery().updated_at)}</dd>
        <dt>{t("delivery-detail-claimed")}</dt>
        <dd>{formatAdminTime(optionalText(delivery().claimed_at))}</dd>
        <dt>{t("delivery-detail-sending")}</dt>
        <dd>{formatAdminTime(optionalText(delivery().sending_at))}</dd>
        <dt>{t("delivery-detail-completed")}</dt>
        <dd>{formatAdminTime(optionalText(delivery().completed_at))}</dd>
        <dt>{t("delivery-external-reference")}</dt>
        <dd>
          <TruncatedText value={optionalText(delivery().external_reference)} portal />
        </dd>
        <dt>{t("delivery-failure-code")}</dt>
        <dd>{optionalText(delivery().failure_code)}</dd>
        <dt>{t("delivery-detail-reason-code")}</dt>
        <dd>{optionalText(delivery().reason_code)}</dd>
        <dt>{t("delivery-detail-returned-mid")}</dt>
        <dd>{optionalText(delivery().returned_mail_id)}</dd>
        <dt>{t("delivery-detail-take-required")}</dt>
        <dd>{delivery().take_required ? t("delivery-detail-yes") : t("delivery-detail-no")}</dd>
      </dl>
    </section>
  );
}

/** 冻结尝试分区属性。 */
export interface AttemptSectionProps {
  readonly attempts: readonly DeliveryAttempt[];
}

/** 冻结的 Send/Take 尝试历史；没有尝试时只显示说明。 */
export function AttemptSection(props: AttemptSectionProps): JSX.Element {
  return (
    <section class="admin-panel" aria-labelledby="delivery-detail-attempts-title">
      <h2 id="delivery-detail-attempts-title">{t("delivery-detail-attempts")}</h2>
      <Show
        when={props.attempts.length === 0}
        fallback={
          <div class="admin-shell__table-wrap">
            <table
              class="admin-shell__table admin-delivery-detail__table"
              aria-label={t("delivery-detail-attempts")}
            >
              <TableColumns
                layout="admin-delivery-attempts"
                widths={[9, 12, 9, 8, 12, 16, 12, 10, 12]}
              />
              <thead>
                <tr>
                  <th scope="col">{t("delivery-detail-attempt-operation")}</th>
                  <th scope="col">{t("delivery-detail-attempt-time")}</th>
                  <th scope="col">{t("delivery-detail-attempt-fence")}</th>
                  <th scope="col">{t("delivery-detail-attempt-world")}</th>
                  <th scope="col">{t("delivery-detail-attempt-recipient")}</th>
                  <th scope="col">{t("delivery-detail-attempt-subject")}</th>
                  <th scope="col">{t("delivery-detail-attempt-digest")}</th>
                  <th scope="col">{t("delivery-detail-attempt-amounts")}</th>
                  <th scope="col">{t("delivery-detail-attempt-mid")}</th>
                </tr>
              </thead>
              <tbody>
                <For
                  each={props.attempts}
                  keyed={(attempt) => `${attempt.fencing_version}-${attempt.operation}`}
                >
                  {(attempt) => (
                    <tr>
                      <td>
                        {attempt().operation === "send"
                          ? t("delivery-detail-attempt-send")
                          : t("delivery-detail-attempt-take")}
                      </td>
                      <td>{formatAdminTime(attempt().sending_at)}</td>
                      <td>{attempt().fencing_version}</td>
                      <td>{adminWorldLabel(attempt().world_id)}</td>
                      <td>
                        <TruncatedText value={optionalText(attempt().recipient)} portal />
                      </td>
                      <td>
                        <TruncatedText value={optionalText(attempt().subject)} portal />
                      </td>
                      <td>
                        <TruncatedText value={optionalText(attempt().body_digest)} portal />
                      </td>
                      <td>{attemptAmounts(attempt())}</td>
                      <td>
                        <TruncatedText value={attemptMailId(attempt())} portal />
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        }
      >
        <p class="admin-panel__note">{t("delivery-detail-attempts-empty")}</p>
      </Show>
    </section>
  );
}

/** Sent 证据分区属性。 */
export interface EvidenceSectionProps {
  readonly evidence: readonly DeliveryEvidence[];
}

/** 维护流程写入的可引用 Sent 证据；观测时间与发送时间同时展示。 */
export function EvidenceSection(props: EvidenceSectionProps): JSX.Element {
  return (
    <section class="admin-panel" aria-labelledby="delivery-detail-evidence-title">
      <h2 id="delivery-detail-evidence-title">{t("delivery-detail-evidence")}</h2>
      <Show
        when={props.evidence.length === 0}
        fallback={
          <div class="admin-shell__table-wrap">
            <table
              class="admin-shell__table admin-delivery-detail__table"
              aria-label={t("delivery-detail-evidence")}
            >
              <TableColumns
                layout="admin-delivery-evidence"
                widths={[10, 14, 12, 20, 14, 14, 16]}
              />
              <thead>
                <tr>
                  <th scope="col">{t("delivery-detail-evidence-kind")}</th>
                  <th scope="col">{t("delivery-detail-evidence-reference")}</th>
                  <th scope="col">{t("delivery-detail-evidence-observed")}</th>
                  <th scope="col">{t("delivery-detail-evidence-subject")}</th>
                  <th scope="col">{t("delivery-detail-evidence-digest")}</th>
                  <th scope="col">{t("delivery-detail-evidence-custody")}</th>
                  <th scope="col">{t("delivery-detail-evidence-receipt")}</th>
                </tr>
              </thead>
              <tbody>
                <For each={props.evidence} keyed={(item) => item.reference}>
                  {(item) => (
                    <tr>
                      <td>{evidenceKindLabel(item().kind)}</td>
                      <td>
                        <TruncatedText value={item().reference} portal />
                      </td>
                      <td>
                        {formatAdminTime(item().observed_at ?? "")}
                        <Show when={item().sent_at !== null}>
                          <div class="admin-delivery-detail__sub">
                            {formatAdminTime(item().sent_at ?? "")}
                          </div>
                        </Show>
                      </td>
                      <td>
                        <TruncatedText value={optionalText(item().subject)} portal />
                      </td>
                      <td>
                        <TruncatedText value={optionalText(item().body_digest)} portal />
                      </td>
                      <td>{evidenceCustody(item())}</td>
                      <td>
                        <TruncatedText value={optionalText(item().receipt)} portal />
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        }
      >
        <p class="admin-panel__note">{t("delivery-detail-evidence-empty")}</p>
      </Show>
    </section>
  );
}

/** 托管分区属性。 */
export interface CustodySectionProps {
  readonly custody: DeliveryCustody;
}

/** 当前领取托管状态；观测时间缺失时明确显示未观测。 */
export function CustodySection(props: CustodySectionProps): JSX.Element {
  const custody = () => props.custody;
  const observed = (): string =>
    custody().observed_at === null
      ? t("delivery-detail-custody-unobserved")
      : formatAdminTime(custody().observed_at ?? "");
  const collected = (): string => {
    if (custody().collected === true) {
      return t("delivery-detail-yes");
    }
    if (custody().collected === false) {
      return t("delivery-detail-no");
    }
    return t("delivery-detail-custody-unobserved");
  };
  return (
    <section class="admin-panel" aria-labelledby="delivery-detail-custody-title">
      <h2 id="delivery-detail-custody-title">{t("delivery-detail-custody")}</h2>
      <dl class="admin-delivery-detail__grid">
        <dt>{t("delivery-detail-custody-collected")}</dt>
        <dd>{collected()}</dd>
        <dt>{t("delivery-detail-custody-cod-pending")}</dt>
        <dd>{optionalInteger(custody().cod_pending)}</dd>
        <dt>{t("delivery-detail-custody-remaining-credits")}</dt>
        <dd>{optionalInteger(custody().remaining_credits)}</dd>
        <dt>{t("delivery-detail-custody-observed")}</dt>
        <dd>{observed()}</dd>
        <dt>{t("delivery-detail-custody-confirmed")}</dt>
        <dd>
          {custody().operator_confirmed_collected
            ? t("delivery-detail-yes")
            : t("delivery-detail-no")}
        </dd>
        <dt>{t("delivery-detail-custody-receipt")}</dt>
        <dd>
          <Show when={custody().receipt !== null} fallback={t("value-empty")}>
            <pre class="admin-delivery-detail__receipt">{custody().receipt}</pre>
          </Show>
        </dd>
      </dl>
    </section>
  );
}

/** 处置分区属性。 */
export interface DispositionSectionProps {
  readonly dispositions: readonly DeliveryDisposition[];
}

/** 追加式处置事实；未知种类原样显示，不推测状态。 */
export function DispositionSection(props: DispositionSectionProps): JSX.Element {
  return (
    <section class="admin-panel" aria-labelledby="delivery-detail-dispositions-title">
      <h2 id="delivery-detail-dispositions-title">{t("delivery-detail-dispositions")}</h2>
      <Show
        when={props.dispositions.length === 0}
        fallback={
          <div class="admin-shell__table-wrap">
            <table
              class="admin-shell__table admin-delivery-detail__table"
              aria-label={t("delivery-detail-dispositions")}
            >
              <TableColumns
                layout="admin-delivery-dispositions"
                widths={[16, 16, 22, 14, 16, 16]}
              />
              <thead>
                <tr>
                  <th scope="col">{t("delivery-detail-disposition-kind")}</th>
                  <th scope="col">{t("delivery-detail-disposition-time")}</th>
                  <th scope="col">{t("delivery-detail-disposition-command")}</th>
                  <th scope="col">{t("delivery-detail-disposition-status")}</th>
                  <th scope="col">{t("delivery-detail-disposition-failure")}</th>
                  <th scope="col">{t("delivery-detail-disposition-actor")}</th>
                </tr>
              </thead>
              <tbody>
                <For each={props.dispositions} keyed={(item) => item.command_id}>
                  {(item) => (
                    <tr>
                      <td>{dispositionKindLabel(item().kind)}</td>
                      <td>{formatAdminTime(item().occurred_at)}</td>
                      <td>
                        <TruncatedText value={item().command_id} portal />
                      </td>
                      <td>{dispositionStatus(item())}</td>
                      <td>
                        <TruncatedText
                          value={
                            item().failure_code ?? item().external_reference ?? t("value-empty")
                          }
                          portal
                        />
                      </td>
                      <td>
                        <TruncatedText value={dispositionActor(item())} portal />
                      </td>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        }
      >
        <p class="admin-panel__note">{t("delivery-detail-dispositions-empty")}</p>
      </Show>
    </section>
  );
}

/** 来源义务与恢复分区属性。 */
export interface ObligationSectionProps {
  readonly obligation: DeliveryObligation | null;
  readonly recovery: DeliveryRecovery | null;
}

/** 投递来源的结算义务与恢复记录；两者各自缺失时显示各自的说明。 */
export function ObligationSection(props: ObligationSectionProps): JSX.Element {
  return (
    <>
      <section class="admin-panel" aria-labelledby="delivery-detail-obligation-title">
        <h2 id="delivery-detail-obligation-title">{t("delivery-detail-obligation")}</h2>
        <Show
          when={props.obligation}
          fallback={<p class="admin-panel__note">{t("delivery-detail-obligation-none")}</p>}
        >
          {(obligation) => (
            <dl class="admin-delivery-detail__grid">
              <dt>{t("delivery-detail-obligation-id")}</dt>
              <dd>
                <TruncatedText value={obligation().obligation_id} portal />
              </dd>
              <dt>{t("delivery-detail-obligation-auction")}</dt>
              <dd>
                <TruncatedText value={obligation().auction_id} portal />
              </dd>
              <dt>{t("delivery-detail-obligation-lot")}</dt>
              <dd>
                <TruncatedText value={obligation().lot_id} portal />
              </dd>
              <dt>{t("delivery-detail-obligation-world")}</dt>
              <dd>{adminWorldLabel(obligation().world_id)}</dd>
              <dt>{t("delivery-detail-obligation-kind")}</dt>
              <dd>{obligation().obligation_kind}</dd>
              <dt>{t("delivery-detail-obligation-state")}</dt>
              <dd>{obligation().business_state}</dd>
              <dt>{t("delivery-detail-obligation-amount")}</dt>
              <dd>{obligation().amount}</dd>
              <dt>{t("delivery-detail-obligation-fulfillment")}</dt>
              <dd>{optionalText(obligation().fulfillment_state)}</dd>
              <dt>{t("delivery-detail-obligation-fulfillment-error")}</dt>
              <dd>{optionalText(obligation().fulfillment_error_code)}</dd>
            </dl>
          )}
        </Show>
      </section>
      <section class="admin-panel" aria-labelledby="delivery-detail-recovery-title">
        <h2 id="delivery-detail-recovery-title">{t("delivery-detail-recovery")}</h2>
        <Show
          when={props.recovery}
          fallback={<p class="admin-panel__note">{t("delivery-detail-recovery-none")}</p>}
        >
          {(recovery) => (
            <dl class="admin-delivery-detail__grid">
              <dt>{t("delivery-detail-recovery-id")}</dt>
              <dd>
                <TruncatedText value={recovery().recovery_id} portal />
              </dd>
              <dt>{t("delivery-detail-recovery-stage")}</dt>
              <dd>{recovery().stage}</dd>
              <dt>{t("delivery-detail-recovery-resolution")}</dt>
              <dd>{optionalText(recovery().resolution)}</dd>
              <dt>{t("delivery-detail-recovery-deadline")}</dt>
              <dd>{formatAdminTime(recovery().payment_deadline_at)}</dd>
              <dt>{t("delivery-detail-recovery-payment-obligation")}</dt>
              <dd>
                <TruncatedText value={recovery().payment_obligation_id} portal />
              </dd>
              <dt>{t("delivery-detail-recovery-blocking")}</dt>
              <dd>{optionalText(recovery().blocking_code)}</dd>
              <dt>{t("delivery-detail-recovery-manual-delivery")}</dt>
              <dd>{optionalText(recovery().manual_reconcile_delivery_id)}</dd>
            </dl>
          )}
        </Show>
      </section>
    </>
  );
}
