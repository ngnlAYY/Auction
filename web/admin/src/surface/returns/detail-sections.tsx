// 结算退件详情的只读展示区：摘要、领回（Take）、代管、派发链与事件。
//
// 这些组件只渲染服务端安全投影，不发起请求、不持有读取世代；命令入口在详情页与动作区。

import type {
  AdminSettlementReturnCustodyResponse,
  AdminSettlementReturnDispatchResponse,
  AdminSettlementReturnEventResponse,
  AdminSettlementReturnSummaryResponse,
  AdminSettlementReturnTakeResponse,
} from "@auction/contracts/dto/admin/returns";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { Link } from "@auction/web-shared/router";
import { Time } from "@auction/web-shared/ui/components/time";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import { deliveryStatusLabel } from "../deliveries/labels";
import { adminWorldLabel } from "../presentation";
import {
  eventDetailsText,
  eventKey,
  kindKey,
  label,
  locationKey,
  matchKey,
  planKey,
  reasonKey,
  showsAssetLocation,
  statusKey,
  takeStateKey,
  takeUnknownGuidanceKey,
  takeUnknownReasonKey,
} from "./labels";
import { assetsText } from "./list";

/** 案件摘要：状态、证据口径、资产位置与派发/代管字段的只读展示。 */
export function SummarySection(props: {
  readonly summary: AdminSettlementReturnSummaryResponse;
}): JSX.Element {
  const empty = (): string => t("value-empty");
  const kind = createValueMemo(() => {
    const value = props.summary.obligation_kind;
    return value === null ? empty() : label(kindKey, value);
  });
  const plan = createValueMemo(() => {
    const value = props.summary.plan;
    return value === null ? empty() : label(planKey, value);
  });
  const resends = createValueMemo(() => {
    const used = props.summary.resends_used;
    return used === null
      ? empty()
      : t("settlement-return-resends-value", {
          used: String(used),
          limit: String(props.summary.resend_limit),
        });
  });
  const dispatch = createValueMemo(() => {
    const value = props.summary.dispatch_no;
    return value === null ? empty() : String(value);
  });

  return (
    <section class="admin-panel" aria-labelledby="settlement-return-summary-title">
      <h2 id="settlement-return-summary-title">{t("settlement-return-summary-title")}</h2>
      <Show
        when={showsAssetLocation(props.summary.status)}
        fallback={
          <p class="admin-panel__note">{t("settlement-return-location-resent-in-transit")}</p>
        }
      >
        <p class="admin-panel__note">{t("settlement-return-location-note")}</p>
      </Show>
      <dl class="admin-delivery-detail__grid">
        <Show when={showsAssetLocation(props.summary.status)}>
          <dt>{t("settlement-return-field-location")}</dt>
          <dd>
            <strong>{label(locationKey, props.summary.asset_location)}</strong>
          </dd>
        </Show>
        <dt>{t("settlement-return-column-status")}</dt>
        <dd>{label(statusKey, props.summary.status)}</dd>
        <dt>{t("settlement-return-field-id")}</dt>
        <dd>
          <TruncatedText value={props.summary.return_id} portal />
        </dd>
        <dt>{t("settlement-return-field-version")}</dt>
        <dd>{props.summary.case_version}</dd>
        <dt>{t("settlement-return-column-match")}</dt>
        <dd>{label(matchKey, props.summary.match_state)}</dd>
        <dt>{t("settlement-return-field-reason")}</dt>
        <dd>{label(reasonKey, props.summary.reason_code)}</dd>
        <dt>{t("settlement-return-column-world")}</dt>
        <dd>{adminWorldLabel(props.summary.world_id)}</dd>
        <dt>{t("settlement-return-field-mid")}</dt>
        <dd>{props.summary.external_mail_id}</dd>
        <dt>{t("settlement-return-column-received")}</dt>
        <dd>
          <Time value={props.summary.received_at} />
        </dd>
        <dt>{t("settlement-return-column-kind")}</dt>
        <dd>{kind()}</dd>
        <dt>{t("settlement-return-field-obligation")}</dt>
        <dd>
          <TruncatedText value={props.summary.obligation_id ?? empty()} portal />
        </dd>
        <dt>{t("settlement-return-field-returned-delivery")}</dt>
        <dd>
          <Show when={props.summary.delivery_id} fallback={empty()}>
            {(deliveryId) => (
              <Link href={`/deliveries/${deliveryId()}`}>
                <TruncatedText value={deliveryId()} portal />
              </Link>
            )}
          </Show>
        </dd>
        <dt>{t("settlement-return-field-dispatch")}</dt>
        <dd>{dispatch()}</dd>
        <dt>{t("settlement-return-column-recipient")}</dt>
        <dd>
          <TruncatedText value={props.summary.recipient ?? empty()} portal />
        </dd>
        <dt>{t("settlement-return-column-assets")}</dt>
        <dd>{assetsText(props.summary)}</dd>
        <dt>{t("settlement-return-field-plan")}</dt>
        <dd>{plan()}</dd>
        <dt>{t("settlement-return-field-resends")}</dt>
        <dd>{resends()}</dd>
        <dt>{t("settlement-return-field-created")}</dt>
        <dd>
          <Time value={props.summary.created_at} />
        </dd>
        <dt>{t("settlement-return-field-updated")}</dt>
        <dd>
          <Time value={props.summary.updated_at} />
        </dd>
        <dt>{t("settlement-return-field-closed")}</dt>
        <dd>
          <Show when={props.summary.closed_at} fallback={empty()}>
            {(closedAt) => <Time value={closedAt()} />}
          </Show>
        </dd>
      </dl>
    </section>
  );
}

/** 领回（Take）状态、结果未知原因与运维指引的只读展示。 */
export function TakeSection(props: {
  readonly take: AdminSettlementReturnTakeResponse | null;
  readonly status: string;
  readonly unknownReason: string | null;
}): JSX.Element {
  const empty = (): string => t("value-empty");
  const deliveryStatus = createValueMemo(() => {
    const value = props.take?.delivery_status;
    return value === null || value === undefined ? empty() : deliveryStatusLabel(value);
  });
  return (
    <section class="admin-panel" aria-labelledby="settlement-return-take-title">
      <h2 id="settlement-return-take-title">{t("settlement-return-take-title")}</h2>
      <Show when={props.status === "take_unknown"}>
        <p class="action-feedback action-feedback--error" role="note">
          {t("settlement-return-take-unknown-warning")}
        </p>
      </Show>
      {/* 结果未知的具体原因（最近一次原会话观察或由持久事实推出）与对应处置指引。 */}
      <Show when={props.unknownReason}>
        {(reason) => (
          <p class="admin-panel__note" role="note">
            {t("settlement-return-take-unknown-reason", {
              reason: label(takeUnknownReasonKey, reason()),
            })}{" "}
            {label(takeUnknownGuidanceKey, reason())}
          </p>
        )}
      </Show>
      <Show
        when={props.take}
        fallback={<p class="admin-panel__note">{t("settlement-return-take-none")}</p>}
      >
        {(take) => (
          <dl class="admin-delivery-detail__grid">
            <dt>{t("settlement-return-take-state")}</dt>
            <dd>{label(takeStateKey, take().execution_state)}</dd>
            <dt>{t("settlement-return-take-delivery")}</dt>
            <dd>
              <Show when={take().delivery_id} fallback={empty()}>
                {(deliveryId) => (
                  <Link href={`/deliveries/${deliveryId()}`}>
                    <TruncatedText value={deliveryId()} portal />
                  </Link>
                )}
              </Show>
            </dd>
            <dt>{t("settlement-return-take-delivery-status")}</dt>
            <dd>{deliveryStatus()}</dd>
            <dt>{t("settlement-return-take-failure")}</dt>
            <dd>{take().failure_code ?? empty()}</dd>
            <dt>{t("settlement-return-take-proven")}</dt>
            <dd>
              <Show when={take().proven_at} fallback={empty()}>
                {(provenAt) => <Time value={provenAt()} />}
              </Show>
            </dd>
          </dl>
        )}
      </Show>
    </section>
  );
}

/** 已领回资产的代管信息，包括受益人、Credits 与装备 ID。 */
export function CustodySection(props: {
  readonly custody: AdminSettlementReturnCustodyResponse;
}): JSX.Element {
  const equipment = createValueMemo(() =>
    props.custody.equipment_ids.length === 0
      ? t("value-empty")
      : props.custody.equipment_ids.join(", "),
  );
  return (
    <section class="admin-panel" aria-labelledby="settlement-return-custody-title">
      <h2 id="settlement-return-custody-title">{t("settlement-return-custody-title")}</h2>
      <p class="admin-panel__note">{t("settlement-return-custody-note")}</p>
      <dl class="admin-delivery-detail__grid">
        <dt>{t("settlement-return-custody-beneficiary")}</dt>
        <dd>
          <TruncatedText value={props.custody.beneficiary} portal />
        </dd>
        <dt>{t("settlement-return-custody-credits")}</dt>
        <dd>{props.custody.credits}</dd>
        <dt>{t("settlement-return-custody-equipment")}</dt>
        <dd>
          <TruncatedText value={equipment()} size="wide" portal />
        </dd>
      </dl>
    </section>
  );
}

/** 原义务派发链与补寄标记；标注本次退回及当前案件授权的派发。 */
export function DispatchSection(props: {
  readonly dispatches: readonly AdminSettlementReturnDispatchResponse[];
  readonly returnedDelivery: string | null;
  readonly resendDispatchNo: number | null;
}): JSX.Element {
  return (
    <section class="admin-panel" aria-labelledby="settlement-return-dispatches-title">
      <h2 id="settlement-return-dispatches-title">{t("settlement-return-dispatches-title")}</h2>
      <p class="admin-panel__note">{t("settlement-return-dispatches-note")}</p>
      <Show
        when={props.dispatches.length === 0}
        fallback={
          <div class="admin-shell__table-wrap">
            <table class="admin-shell__table" aria-label={t("settlement-return-dispatches-title")}>
              <thead>
                <tr>
                  <th scope="col">{t("settlement-return-dispatch-no")}</th>
                  <th scope="col">{t("settlement-return-dispatch-delivery")}</th>
                  <th scope="col">{t("settlement-return-dispatch-status")}</th>
                  <th scope="col">{t("settlement-return-dispatch-sending")}</th>
                  <th scope="col">{t("settlement-return-dispatch-completed")}</th>
                  <th scope="col">{t("settlement-return-dispatch-authorized")}</th>
                </tr>
              </thead>
              <tbody>
                <For each={props.dispatches} keyed={(dispatch) => dispatch.dispatch_no}>
                  {(dispatch) => (
                    <DispatchRow
                      dispatch={dispatch()}
                      returned={props.returnedDelivery === dispatch().delivery_id}
                      resend={props.resendDispatchNo === dispatch().dispatch_no}
                    />
                  )}
                </For>
              </tbody>
            </table>
          </div>
        }
      >
        <p class="admin-panel__note">{t("settlement-return-dispatches-empty")}</p>
      </Show>
    </section>
  );
}

function DispatchRow(props: {
  readonly dispatch: AdminSettlementReturnDispatchResponse;
  readonly returned: boolean;
  readonly resend: boolean;
}): JSX.Element {
  const empty = (): string => t("value-empty");
  const status = createValueMemo(() => {
    const value = props.dispatch.delivery_status;
    return value === null ? empty() : deliveryStatusLabel(value);
  });
  const number = createValueMemo(() =>
    props.dispatch.dispatch_no === 1
      ? t("settlement-return-dispatch-first", { number: String(props.dispatch.dispatch_no) })
      : t("settlement-return-dispatch-resend", { number: String(props.dispatch.dispatch_no) }),
  );
  return (
    <tr>
      <td>
        {number()}
        <Show when={props.returned}>
          {" "}
          <span class="admin-dashboard-badge admin-dashboard-badge--warn">
            {t("settlement-return-dispatch-returned")}
          </span>
        </Show>
        <Show when={props.resend}>
          {" "}
          <span class="admin-dashboard-badge admin-dashboard-badge--neutral">
            {t("settlement-return-dispatch-this-case")}
          </span>
        </Show>
      </td>
      <td>
        <Link href={`/deliveries/${props.dispatch.delivery_id}`}>
          <TruncatedText value={props.dispatch.delivery_id} portal />
        </Link>
      </td>
      <td>{status()}</td>
      <td>
        <Show when={props.dispatch.sending_at} fallback={empty()}>
          {(sendingAt) => <Time value={sendingAt()} />}
        </Show>
      </td>
      <td>
        <Show when={props.dispatch.completed_at} fallback={empty()}>
          {(completedAt) => <Time value={completedAt()} />}
        </Show>
      </td>
      <td>
        <Show when={props.dispatch.authorized_by} fallback={empty()}>
          {(caseId) => (
            <Link href={`/settlement-returns/${caseId()}`}>
              <TruncatedText value={caseId()} portal />
            </Link>
          )}
        </Show>
      </td>
    </tr>
  );
}

/** 按案件版本排列的处理事件及其安全细节。 */
export function EventSection(props: {
  readonly events: readonly AdminSettlementReturnEventResponse[];
}): JSX.Element {
  return (
    <section class="admin-panel" aria-labelledby="settlement-return-events-title">
      <h2 id="settlement-return-events-title">{t("settlement-return-events-title")}</h2>
      <div class="admin-shell__table-wrap">
        <table class="admin-shell__table" aria-label={t("settlement-return-events-title")}>
          <thead>
            <tr>
              <th scope="col">{t("settlement-return-field-version")}</th>
              <th scope="col">{t("settlement-return-event-kind")}</th>
              <th scope="col">{t("settlement-return-event-time")}</th>
              <th scope="col">{t("settlement-return-event-actor")}</th>
              <th scope="col">{t("settlement-return-event-details")}</th>
            </tr>
          </thead>
          <tbody>
            <For each={props.events} keyed={(event) => event.case_version}>
              {(event) => (
                <tr>
                  <td>{event().case_version}</td>
                  <td>{label(eventKey, event().event_kind)}</td>
                  <td>
                    <Time value={event().occurred_at} />
                  </td>
                  <td>{event().actor_id ?? t("settlement-return-event-worker")}</td>
                  <td>
                    <TruncatedText value={eventDetailsText(event().details)} size="wide" portal />
                  </td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </div>
    </section>
  );
}
