// Delivery 列表、行动作和分页组件。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { Link } from "@auction/web-shared/router";
import { compareText } from "@auction/web-shared/text";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { formatCreditAmount } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { Delivery, DeliveryPage } from "../../api/delivery/decode";
import { requirePendingCommands } from "../pending";
import { MobileSortControl, SortableTh, type SortDirection, sortRows } from "../sort";
import { deliveryCommandEntity } from ".";
import { DeliveryDialog } from "./dialog";
import {
  deliveryCategoryLabel,
  deliveryDisplayId,
  deliveryDisplayType,
  deliveryOutcomeLabel,
  deliveryStatusLabel,
} from "./labels";
import {
  type DeliveryActionRequest,
  type DeliveryDialogKind,
  type DeliveryDialogState,
  type DeliveryReconcileDraft,
  deliveryReconcileDraftForDelivery,
} from "./model";

/** Delivery 表格的当前页、排序状态、对话框状态与行内草稿。 */
export interface DeliveryTableProps {
  readonly page: DeliveryPage;
  /** 当前排序列（空串 = 服务端原始顺序）与方向；变更经 `onSortChange` 由页面写回 URL。 */
  readonly sort: string;
  readonly direction: SortDirection;
  readonly onSortChange: (value: readonly [string, SortDirection]) => void;
  readonly onOpen: (value: readonly [Delivery, DeliveryDialogKind]) => void;
  readonly pendingDialog: DeliveryDialogState | null;
  /** 行内对账草稿；表格只转交信号，由对话框所在行读取。 */
  readonly draft: Cell<DeliveryReconcileDraft | null>;
  readonly onDraftChange: (draft: DeliveryReconcileDraft) => void;
  readonly onConfirm: (request: DeliveryActionRequest) => void;
  readonly onCancel: () => void;
}

interface DeliveryTableRow {
  readonly delivery: Delivery;
  readonly dialog: DeliveryDialogState | null;
}

/** 展示后端返回的 delivery 安全投影。 */
export function DeliveryTable(props: DeliveryTableProps): JSX.Element {
  const columns = createValueMemo<readonly (readonly [string, string])[]>(() => [
    ["id", t("delivery-id")],
    ["template", t("delivery-template")],
    ["category", t("delivery-category")],
    ["cod", t("delivery-cod")],
    ["status", t("delivery-status")],
    ["attempts", t("delivery-attempts")],
  ]);
  const isEmpty = createValueMemo(() => props.page.deliveries.length === 0);
  // 只有对话框所在行收到对话框状态；开合对话框时其余行 props 不变。行数据在 JSX 外配好，
  // 循环体根节点才能直接是带 key 的 DeliveryRow。
  const rows = createValueMemo<readonly DeliveryTableRow[]>(() => {
    const deliveries = [...props.page.deliveries];
    sortDeliveries(deliveries, props.sort, props.direction);
    const pendingDialog = props.pendingDialog;
    return deliveries.map((delivery) => ({
      delivery,
      dialog:
        pendingDialog !== null && pendingDialog.delivery.id === delivery.id ? pendingDialog : null,
    }));
  });
  return (
    <>
      <MobileSortControl
        columns={columns()}
        sort={props.sort}
        direction={props.direction}
        onSortChange={props.onSortChange}
      />
      <div class="admin-delivery-table-wrap">
        <table class="admin-delivery-table" aria-label={t("page-deliveries-title")}>
          <TableColumns layout="admin-deliveries" widths={[7, 9, 13, 11, 9, 7, 7, 6, 21, 10]} />
          <thead>
            <tr>
              <SortableTh
                column="id"
                label={t("delivery-id")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <th scope="col">{t("delivery-type")}</th>
              <th scope="col">{t("delivery-to")}</th>
              <SortableTh
                column="template"
                label={t("delivery-template")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <SortableTh
                column="category"
                label={t("delivery-category")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <SortableTh
                column="cod"
                label={t("delivery-cod")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <SortableTh
                column="status"
                label={t("delivery-status")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <SortableTh
                column="attempts"
                label={t("delivery-attempts")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <th scope="col">{t("delivery-outcome")}</th>
              <th scope="col">{t("catalog-actions")}</th>
            </tr>
          </thead>
          <tbody>
            <For each={rows()} keyed={(row) => row.delivery.id}>
              {(row) => (
                // 按业务 ID 固定 key，避免刷新/重排后行内本地状态错位绑定到另一行。
                <DeliveryRow
                  delivery={row().delivery}
                  onOpen={props.onOpen}
                  pendingDialog={row().dialog}
                  draft={props.draft}
                  onDraftChange={props.onDraftChange}
                  onConfirm={props.onConfirm}
                  onCancel={props.onCancel}
                />
              )}
            </For>
          </tbody>
        </table>
      </div>
      <Show when={isEmpty()}>
        <AsyncState kind="empty" title={t("delivery-empty")} detail="" />
      </Show>
    </>
  );
}

/** 列表打开处置预览，实际操作需读取当前 fence 的判定。 */
export function DeliveryRow(props: {
  readonly delivery: Delivery;
  readonly onOpen: (value: readonly [Delivery, DeliveryDialogKind]) => void;
  readonly pendingDialog: DeliveryDialogState | null;
  readonly draft: Cell<DeliveryReconcileDraft | null>;
  readonly onDraftChange: (draft: DeliveryReconcileDraft) => void;
  readonly onConfirm: (request: DeliveryActionRequest) => void;
  readonly onCancel: () => void;
}): JSX.Element {
  const commands = requirePendingCommands();
  const kind = createValueMemo<DeliveryDialogKind | null>(() => {
    switch (props.delivery.status) {
      case "claimed":
        return "release";
      case "failed":
        return "retry";
      case "manual_reconcile":
        return "reconcile";
      default:
        return null;
    }
  });
  const status = createValueMemo(() => deliveryStatusLabel(props.delivery.status));
  const cod = createValueMemo(() =>
    props.delivery.cod === null ? t("value-empty") : formatCreditAmount(props.delivery.cod),
  );
  const template = createValueMemo(() => props.delivery.template ?? t("value-empty"));
  const category = createValueMemo(() =>
    props.delivery.category === null
      ? t("value-empty")
      : deliveryCategoryLabel(props.delivery.category),
  );
  const outcome = createValueMemo(() => deliveryOutcomeLabel(props.delivery));
  const recipient = createValueMemo(() => props.delivery.recipient);
  const fullIdentifier = createValueMemo(() =>
    props.delivery.origin_id === "" ? props.delivery.id : props.delivery.origin_id,
  );
  const displayIdentifier = createValueMemo(() => deliveryDisplayId(props.delivery));
  const displayType = createValueMemo(() => deliveryDisplayType(props.delivery.origin_kind));
  const entity = createValueMemo(() => deliveryCommandEntity(props.delivery.id));
  const busy = createValueMemo(() => commands.anyBusy());
  const disabled = createValueMemo(() => commands.anyBusy() || commands.contains(entity()));

  return (
    <>
      <tr>
        <td data-label={t("delivery-id")}>
          <TruncatedText
            value={fullIdentifier()}
            display={displayIdentifier()}
            size="compact"
            portal={true}
            legacyDom={true}
          />
        </td>
        <td data-label={t("delivery-type")}>
          <TruncatedText value={displayType()} portal={true} legacyDom={true} />
        </td>
        <td data-label={t("delivery-to")}>
          <TruncatedText value={recipient()} portal={true} legacyDom={true} />
        </td>
        <td data-label={t("delivery-template")}>
          <TruncatedText value={template()} portal={true} legacyDom={true} />
        </td>
        <td data-label={t("delivery-category")}>
          <TruncatedText value={category()} portal={true} legacyDom={true} />
        </td>
        <td class="admin-delivery-num" data-label={t("delivery-cod")}>
          {cod()}
        </td>
        <td data-label={t("delivery-status")}>
          <span class="admin-delivery-badge">{status()}</span>
        </td>
        <td class="admin-delivery-num" data-label={t("delivery-attempts")}>
          {props.delivery.attempt_count}
        </td>
        <td data-label={t("delivery-outcome")}>
          <TruncatedText value={outcome()} portal={true} legacyDom={true} />
        </td>
        <td class="admin-delivery-actions-cell">
          <div class="admin-delivery-actions">
            <Link class="admin-delivery-detail-link" href={`/deliveries/${props.delivery.id}`}>
              {t("delivery-detail-open")}
            </Link>
            <Show when={kind()}>
              {(dialogKind) => (
                <button
                  type="button"
                  class="admin-btn-primary"
                  disabled={disabled()}
                  onClick={() => props.onOpen([props.delivery, dialogKind()])}
                >
                  {t("delivery-review-actions")}
                </button>
              )}
            </Show>
          </div>
        </td>
      </tr>
      <Show when={props.pendingDialog}>
        {(dialog) => (
          <tr class="admin-delivery-dialog-row">
            <td colspan="10">
              <DeliveryDialog
                delivery={dialog().delivery}
                kind={dialog().kind}
                busy={busy()}
                // 只在对话框打开时读取草稿，订阅随之收窄到本行。
                draft={props.draft() ?? deliveryReconcileDraftForDelivery(props.delivery)}
                onDraftChange={props.onDraftChange}
                onConfirm={props.onConfirm}
                onCancel={props.onCancel}
              />
            </td>
          </tr>
        )}
      </Show>
    </>
  );
}

/**
 * 对当前页 delivery 行排序；`column` 为空或不在允许集合时保持服务端原始顺序。
 * 到付金额缺省（None）在升序时排在最前。
 */
function sortDeliveries(deliveries: Delivery[], column: string, direction: SortDirection): void {
  sortRows(deliveries, column, direction, (left, right, sortColumn) => {
    switch (sortColumn) {
      case "id":
        return compareText(left.id, right.id);
      case "template":
        return compareOptionalText(left.template, right.template);
      case "category":
        return compareOptionalText(left.category, right.category);
      case "cod":
        return compareOptionalInteger(left.cod, right.cod);
      case "status":
        return compareText(left.status, right.status);
      case "attempts":
        return compareInteger(left.attempt_count, right.attempt_count);
      default:
        return 0;
    }
  });
}

/** `Option<T>` 的排序与 Rust 一致：None 小于任何 Some。 */
function compareOptionalText(left: string | null, right: string | null): number {
  if (left === null) {
    return right === null ? 0 : -1;
  }
  if (right === null) {
    return 1;
  }
  return compareText(left, right);
}

/** `Option<i64>` 的排序与 Rust 一致：None 小于任何 Some。 */
function compareOptionalInteger(left: number | null, right: number | null): number {
  if (left === null) {
    return right === null ? 0 : -1;
  }
  if (right === null) {
    return 1;
  }
  return compareInteger(left, right);
}

function compareInteger(left: number, right: number): number {
  if (left < right) {
    return -1;
  }
  if (left > right) {
    return 1;
  }
  return 0;
}
