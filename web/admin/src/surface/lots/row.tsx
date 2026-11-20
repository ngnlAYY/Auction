// Admin lot 列表行和编辑入口。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import type { AdminLot } from "../../api/lots";
import { requirePendingCommands } from "../pending";
import { formatAdminTime, lotStatusLabel } from "../presentation";
import { type LotEdit, lotSnapshotFromLot, openLotEdit } from "./edit";
import { equipmentDetails, submissionIdDisplay } from "./presentation";
import type { LotUpdateNotice } from "./state";

/** 拍品列表行的安全投影、页面共享的选择状态与编辑草稿。 */
export interface AdminLotRowProps {
  readonly lot: AdminLot;
  readonly selectedLot: Cell<readonly [string, string] | null>;
  readonly notice: Cell<LotUpdateNotice | null>;
  readonly allowLotUpdate: boolean;
  readonly editsReady: boolean;
  readonly draft: Cell<LotEdit | null>;
}

/** 一行拍品：编码、装备详情、卖家、提交、状态、价格与编辑/代理入口。 */
export function AdminLotRow(props: AdminLotRowProps): JSX.Element {
  const owner = requirePendingCommands();
  const lotId = createValueMemo(() => props.lot.id);
  const canUpdate = createValueMemo(() => props.allowLotUpdate && props.lot.status === "pending");
  const entity = createValueMemo(() => `lot:${props.lot.id}`);
  const frozen = createValueMemo(() => owner.contains(entity()));
  const editDisabled = createValueMemo(() => frozen() || !props.editsReady);
  // 打开对话框时冻结 lot 的版本、编码与起拍价；行消失或读取失败都不改写它。
  const snapshot = createValueMemo(() => lotSnapshotFromLot(props.lot));
  const lotCode = createValueMemo(() => snapshot().code);
  const submission = createValueMemo(() => {
    const value = props.lot.submission_id;
    return value === null ? null : { value, display: submissionIdDisplay(value) };
  });
  const slowSchedule = createValueMemo(() => {
    const dueAt = props.lot.slow_due_at;
    return dueAt === null
      ? null
      : {
          dueAt,
          title: props.lot.slow_authorization_id ?? "",
          generation: props.lot.slow_generation,
        };
  });

  return (
    <tr class="admin-lots__row">
      <td data-label={t("admin-lots-code")}>
        <span class="admin-lots__code">{lotCode()}</span>
      </td>
      <td data-label={t("admin-lots-equipment")}>
        <TruncatedText
          value={equipmentDetails(props.lot.equipment_name, props.lot.equipment_attributes)}
          display={props.lot.equipment_name}
          className="admin-lots__equipment-name"
        />
      </td>
      <td data-label={t("admin-lots-seller")}>
        <TruncatedText
          value={props.lot.seller ?? t("value-empty")}
          className="admin-lots__seller-name"
        />
      </td>
      <td data-label={t("admin-lots-submission")}>
        <Show
          when={submission()}
          fallback={<span class="admin-lots__empty-value">{t("value-empty")}</span>}
        >
          {(value) => (
            <TruncatedText value={value().value} display={value().display} size="compact" />
          )}
        </Show>
      </td>
      <td class="admin-lots__status-cell" data-label={t("admin-lots-status")}>
        <span class={["admin-auction-badge", `admin-lots__status--${props.lot.status}`]}>
          {lotStatusLabel(props.lot.status)}
        </span>
        <Show when={slowSchedule()}>
          {(schedule) => (
            <span class="admin-lots__slow-schedule" title={schedule().title}>
              {t("admin-lots-slow-schedule", {
                generation: schedule().generation,
                time: formatAdminTime(schedule().dueAt),
              })}
            </span>
          )}
        </Show>
      </td>
      <td
        class="admin-auction-num admin-lots__start-price"
        data-label={t("admin-lots-start-price")}
      >
        <span class="admin-lots__price-value">{formatCreditAmount(props.lot.start_price)}</span>
      </td>
      <td class="admin-auction-num admin-lots__current-price" data-label={t("admin-lots-price")}>
        <strong class="admin-lots__price-value">
          {formatCreditAmount(props.lot.current_price)}
        </strong>
      </td>
      <td class="admin-lots__actions-cell" data-label={t("admin-lots-actions")}>
        <div class="admin-auction-actions">
          <Show when={canUpdate()}>
            <button
              class="admin-btn-primary"
              type="button"
              disabled={editDisabled()}
              onClick={() => {
                if (!editDisabled() && openLotEdit(props.draft, "reprice", snapshot())) {
                  props.notice.set(null);
                }
              }}
            >
              {t("admin-lots-reprice")}
            </button>
            <button
              class="admin-btn-danger"
              type="button"
              disabled={editDisabled()}
              onClick={() => {
                if (!editDisabled() && openLotEdit(props.draft, "withdraw", snapshot())) {
                  props.notice.set(null);
                }
              }}
            >
              {t("admin-lots-withdraw")}
            </button>
          </Show>
          <button
            class="admin-btn-secondary"
            type="button"
            disabled={frozen()}
            onClick={() => {
              props.selectedLot.set([lotId(), lotCode()]);
            }}
          >
            {t("admin-lots-proxy-open")}
          </button>
        </div>
      </td>
    </tr>
  );
}
