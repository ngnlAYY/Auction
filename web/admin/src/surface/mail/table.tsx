// Admin 邮件入站摘要表格和行展示。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { compareText } from "@auction/web-shared/text";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type { AdminMailIntake, AdminMailIntakePage } from "../../api/mail";
import { AdminPager, type AdminPagination } from "../pagination";
import { adminWorldLabel } from "../presentation";
import { SortableTh, type SortDirection, sortRows } from "../sort";
import { mailKindLabel, mailStatusClass, mailStatusLabel } from "./labels";
import { processingCodeMessage } from "./messages";

/** 当前页入站摘要、分页 owner、排序状态与选择回调。 */
export interface MailIntakeTableProps {
  readonly page: AdminMailIntakePage;
  readonly pagination: AdminPagination;
  /** 当前排序列（空串 = 服务端原始顺序）与方向；变更经 `onSortChange` 由页面写回 URL。 */
  readonly sort: string;
  readonly direction: SortDirection;
  /** 翻页回调：由页面把新页码 push 到地址栏并更新查询状态。 */
  readonly onPageChange: (page: number) => void;
  readonly onSortChange: (value: readonly [string, SortDirection]) => void;
  readonly onSelect: (intakeId: string) => void;
}

/** 入站邮件摘要表：当前页排序、详情入口、空态与分页。 */
export function MailIntakeTable(props: MailIntakeTableProps): JSX.Element {
  const isEmpty = createValueMemo(() => props.page.intakes.length === 0);
  // 服务端不接收排序参数；排序只作用于当前页副本，等值行保持原有相对顺序。
  const intakes = createValueMemo(() => {
    const rows = [...props.page.intakes];
    sortMailIntakes(rows, props.sort, props.direction);
    return rows;
  });
  return (
    <>
      <div class="admin-shell__table-wrap">
        <table
          class="admin-shell__table admin-mail-table"
          aria-label={t("admin-mail-intake-title")}
        >
          {/* 列序：邮件 ID、世界、发件人、标题、类型、状态、错误、原始信息、时间；比例总和为 100%。 */}
          <TableColumns layout="admin-mails" widths={[7, 6, 13, 19, 8, 9, 15, 12, 11]} />
          <thead>
            <tr>
              <th scope="col">{t("admin-mail-external-id")}</th>
              <SortableTh
                column="world"
                label={t("admin-mail-intake-world")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <th scope="col">{t("admin-mail-intake-from")}</th>
              <th scope="col">{t("admin-mail-intake-subject")}</th>
              <SortableTh
                column="kind"
                label={t("admin-mail-intake-kind")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <SortableTh
                column="status"
                label={t("admin-mail-intake-status")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
              <th scope="col">{t("admin-mail-intake-error")}</th>
              <th scope="col">{t("admin-mail-original")}</th>
              <SortableTh
                column="received"
                label={t("admin-mail-intake-received")}
                active={props.sort}
                direction={props.direction}
                onSort={props.onSortChange}
              />
            </tr>
          </thead>
          <tbody>
            <For each={intakes()} keyed={false}>
              {(intake) => <MailIntakeRow intake={intake()} onSelect={props.onSelect} />}
            </For>
          </tbody>
        </table>
      </div>
      <Show when={isEmpty()}>
        <AsyncState kind="empty" title={t("admin-mail-empty")} detail="" />
      </Show>
      <AdminPager
        pagination={props.pagination}
        page={props.page.pagination.page}
        pageSize={props.page.pagination.page_size}
        total={props.page.pagination.total}
        label={t("page-mails-title")}
        onPageChange={props.onPageChange}
      />
    </>
  );
}

interface MailIntakeRowProps {
  readonly intake: AdminMailIntake;
  readonly onSelect: (intakeId: string) => void;
}

/** 一行入站邮件安全摘要；原文缺失时按源码显示「未保留」占位。 */
function MailIntakeRow(props: MailIntakeRowProps): JSX.Element {
  const intakeId = createValueMemo(() => props.intake.id);
  const detailLabel = createValueMemo(
    () => `${t("admin-mail-open-detail")}: ${props.intake.external_mail_id}`,
  );
  const decision = createValueMemo(() =>
    props.intake.status === "accepted" ? "" : processingCodeMessage(props.intake.decision_code),
  );
  const sender = createValueMemo(
    () => props.intake.sender ?? t("admin-mail-original-not-retained"),
  );
  const subject = createValueMemo(
    () => props.intake.subject ?? t("admin-mail-original-not-retained"),
  );
  const original = createValueMemo(() =>
    props.intake.has_message_display
      ? t("admin-mail-original-available")
      : t("admin-mail-original-not-retained"),
  );
  return (
    <tr class="admin-mail__row">
      <td>
        <button
          class="admin-mail__detail-trigger"
          type="button"
          title={detailLabel()}
          aria-label={detailLabel()}
          onClick={() => props.onSelect(intakeId())}
        >
          <span class="shared-truncated-text shared-truncated-text--compact">
            {props.intake.external_mail_id}
          </span>
        </button>
      </td>
      <td>
        <TruncatedText value={adminWorldLabel(props.intake.world_id)} size="compact" />
      </td>
      <td>
        <TruncatedText value={sender()} size="compact" />
      </td>
      <td>
        <TruncatedText value={subject()} size="wide" />
      </td>
      <td>
        <TruncatedText value={mailKindLabel(props.intake.kind)} />
      </td>
      <td>
        <span class={mailStatusClass(props.intake.status)}>
          <TruncatedText value={mailStatusLabel(props.intake.status)} size="compact" />
        </span>
      </td>
      <td>
        <TruncatedText value={decision()} size="wide" />
      </td>
      <td>
        <TruncatedText value={original()} size="compact" />
      </td>
      <td>
        <Time value={props.intake.received_at} />
      </td>
    </tr>
  );
}

/** 对当前页入站邮件行排序；`column` 为空或不在允许集合时保持服务端原始顺序。 */
function sortMailIntakes(
  intakes: AdminMailIntake[],
  column: string,
  direction: SortDirection,
): void {
  sortRows(intakes, column, direction, (left, right, current) => {
    switch (current) {
      case "received":
        return compareText(left.received_at, right.received_at);
      case "kind":
        return compareText(left.kind, right.kind);
      case "status":
        return compareText(left.status, right.status);
      case "world":
        return compareText(left.world_id, right.world_id);
      default:
        return 0;
    }
  });
}
