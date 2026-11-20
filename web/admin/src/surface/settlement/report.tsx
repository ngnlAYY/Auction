// Admin 结算 lot 表格、资金池摘要和分页展示。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { formatCreditAmount, formatHeldDuration, formatPpm } from "@auction/web-shared/ui/format";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { type Accessor, For, Show } from "solid-js";

import type { AdminRecoveryReport } from "../../api/recovery";
import type {
  AdminSettlementLot,
  AdminSettlementReport,
  AdminSettlementSummary,
} from "../../api/settlement";
import { adminLotCodeFor } from "../lots/presentation";
import { AdminPager, type AdminPagination, useAdminPagination } from "../pagination";
import type { AdminActionFeedback } from "../presentation";
import { pageCount, pageSlice, priceAdjustmentLabel } from "./presentation";
import { AdminRecoveryTable, type RecoveryConfirmationDraft, type RecoveryRead } from "./recovery";

const I64_MAX = 9_223_372_036_854_775_807n;

/** 结算报告的分页所有者；页码只在本地报告内变化，不写筛选历史。 */
export function AdminSettlementReportView(props: {
  readonly report: AdminSettlementReport;
  readonly recovery: AdminRecoveryReport | null;
  readonly lotPage: Cell<number>;
  readonly recoveryPage: Cell<number>;
  readonly actionBusy: Accessor<boolean>;
  readonly recoveryRead: RecoveryRead;
  readonly confirmation: Cell<RecoveryConfirmationDraft | null>;
  readonly reason: Cell<string>;
  readonly actionFeedback: Cell<AdminActionFeedback | null>;
}): JSX.Element {
  const lotPagination = useAdminPagination("settlement-lots", ["lot_page_size"], null, I64_MAX);
  const recoveryPagination = useAdminPagination(
    "settlement-recovery",
    ["recovery_page_size"],
    null,
    I64_MAX,
  );
  const lotTotalPages = (): number => pageCount(props.report.lots.length, lotPagination.size());
  const currentLotPage = (): number => clamp(props.lotPage(), 1, lotTotalPages());
  const lotRows = (): AdminSettlementLot[] =>
    pageSlice(props.report.lots, currentLotPage(), lotPagination.size());
  const recoveryTotalPages = (): number =>
    props.recovery === null
      ? 1
      : pageCount(props.recovery.recoveries.length, recoveryPagination.size());
  const currentRecoveryPage = (): number => clamp(props.recoveryPage(), 1, recoveryTotalPages());
  return (
    <>
      <div class="admin-settlement__table-wrap">
        <table
          class="admin-shell__table admin-auction-table"
          aria-label={t("page-settlement-title")}
        >
          <TableColumns layout="admin-settlement" widths={[7, 20, 11, 9, 10, 10, 13, 9, 11]} />
          <thead>
            <tr>
              <th class="admin-auction-num" scope="col">
                {t("admin-settlement-lot-id")}
              </th>
              <th scope="col">{t("admin-settlement-equipment")}</th>
              <th scope="col">
                <StackedPair
                  primary={t("admin-settlement-seller")}
                  secondary={t("admin-settlement-winner")}
                  secondaryLabel={null}
                />
              </th>
              <th class="admin-auction-num" scope="col">
                {t("admin-settlement-final-price")}
              </th>
              <th class="admin-auction-num" scope="col">
                <StackedPair
                  primary={t("admin-settlement-hold")}
                  secondary={t("admin-settlement-adjustment")}
                  secondaryLabel={null}
                />
              </th>
              <th class="admin-auction-num" scope="col">
                {t("admin-settlement-held-time")}
              </th>
              <th class="admin-auction-num" scope="col">
                <StackedPair
                  primary={t("admin-settlement-discount-penalty")}
                  secondary={t("admin-settlement-listed-price")}
                  secondaryLabel={null}
                />
              </th>
              <th class="admin-auction-num" scope="col">
                {t("admin-settlement-compensation")}
              </th>
              <th class="admin-auction-num" scope="col">
                <StackedPair
                  primary={t("admin-settlement-buyer")}
                  secondary={t("admin-settlement-seller-amount")}
                  secondaryLabel={null}
                />
              </th>
            </tr>
          </thead>
          <tbody>
            <For each={lotRows()} keyed={false}>
              {(lot) => <AdminSettlementLotRow lot={lot()} />}
            </For>
          </tbody>
        </table>
      </div>
      <Show when={props.report.lots.length === 0}>
        <AsyncState kind="empty" title={t("admin-settlement-no-lots")} detail="" />
      </Show>
      <AdminSettlementPager
        currentPage={props.lotPage()}
        totalRows={props.report.lots.length}
        pagination={lotPagination}
        page={props.lotPage}
        label={t("admin-settlement-lots-pagination")}
      />
      <AdminSettlementSummaryCard summary={props.report.settlement} />
      <AdminRecoveryTable
        report={props.recovery}
        currentPage={currentRecoveryPage()}
        pagination={recoveryPagination}
        page={props.recoveryPage}
        actionBusy={props.actionBusy}
        recoveryRead={props.recoveryRead}
        confirmation={props.confirmation}
        reason={props.reason}
        actionFeedback={props.actionFeedback}
      />
    </>
  );
}

function AdminSettlementLotRow(props: { readonly lot: AdminSettlementLot }): JSX.Element {
  const holdShare = (): string => formatPpm(props.lot.hold_ppm, false);
  const adjustmentRate = (): string => formatPpm(props.lot.adjustment_ppm, true);
  const heldTime = (): string => formatHeldDuration(props.lot.leader_held_nanos);
  const priceAdjustment = (): string => priceAdjustmentLabel(props.lot);
  const lotCode = (): string => adminLotCodeFor(props.lot.equipment_category, props.lot.lot_number);
  const seller = (): string => props.lot.seller ?? t("value-empty");
  const winner = (): string => props.lot.winner ?? t("value-empty");
  return (
    <tr>
      <td class="admin-auction-num" data-label={t("admin-settlement-lot-id")}>
        {lotCode()}
      </td>
      <td data-label={t("admin-settlement-equipment")}>
        {/* 装备名本身即游戏装备链接，最多两行、超出由 CSS 截断；DOM 与 title 保留全文，
            读屏名称以可见装备名为主体并说明跳转动作。API 层已拒绝无效装备 URL。 */}
        <a
          class="admin-settlement-equipment-link"
          href={props.lot.game_equipment_url}
          target="_blank"
          rel="noopener noreferrer"
          title={props.lot.equipment_name}
          aria-label={t("admin-settlement-open-equipment-label", {
            equipment: props.lot.equipment_name,
          })}
        >
          {props.lot.equipment_name}
        </a>
      </td>
      <td data-label={t("admin-settlement-seller")}>
        <StackedPair
          primary={seller()}
          secondary={winner()}
          secondaryLabel={t("admin-settlement-winner")}
          truncated={true}
        />
      </td>
      <td class="admin-auction-num" data-label={t("admin-settlement-final-price")}>
        {formatCreditAmount(props.lot.final_price)}
      </td>
      <td class="admin-auction-num" data-label={t("admin-settlement-hold")}>
        <StackedPair
          primary={holdShare()}
          secondary={adjustmentRate()}
          secondaryLabel={t("admin-settlement-adjustment")}
        />
      </td>
      <td class="admin-auction-num" data-label={t("admin-settlement-held-time")}>
        {heldTime()}
      </td>
      <td class="admin-auction-num" data-label={t("admin-settlement-discount-penalty")}>
        <StackedPair
          primary={priceAdjustment()}
          secondary={formatCreditAmount(props.lot.listed_price)}
          secondaryLabel={t("admin-settlement-listed-price")}
        />
      </td>
      <td class="admin-auction-num" data-label={t("admin-settlement-compensation")}>
        {formatCreditAmount(props.lot.compensation_amount)}
      </td>
      <td class="admin-auction-num" data-label={t("admin-settlement-buyer")}>
        <StackedPair
          primary={formatCreditAmount(props.lot.buyer_amount)}
          secondary={formatCreditAmount(props.lot.seller_amount)}
          secondaryLabel={t("admin-settlement-seller-amount")}
        />
      </td>
    </tr>
  );
}

/** 相邻两个字段共用一列：上行是原顺序中的前一字段，下行是后一字段，表头与单元格同构。
 * 十三个字段单行至少需要约 1400px，合并成对字段后桌面报告卡内可完整显示金额与比例。
 * 用稳定的组件节点承载两行；`truncated` 对卖家/得主使用可原位更新值的截断控件。 */
function StackedPair(props: {
  readonly primary: string;
  readonly secondary: string;
  readonly secondaryLabel: string | null;
  readonly truncated?: boolean;
}): JSX.Element {
  return (
    <>
      <span class="admin-settlement__line">
        {props.truncated === true ? <TruncatedText value={props.primary} /> : props.primary}
      </span>
      <span
        class="admin-settlement__line admin-settlement__line--secondary"
        data-label={props.secondaryLabel ?? undefined}
      >
        {props.truncated === true ? <TruncatedText value={props.secondary} /> : props.secondary}
      </span>
    </>
  );
}

function AdminSettlementSummaryCard(props: {
  readonly summary: AdminSettlementSummary | null;
}): JSX.Element {
  return (
    <section class="admin-auction-report" aria-labelledby="settlement-pool-title">
      <h2 id="settlement-pool-title">{t("admin-settlement-summary-title")}</h2>
      <Show
        when={props.summary}
        fallback={<p class="admin-auction-status">{t("admin-settlement-not-settled")}</p>}
      >
        {(summary) => (
          <>
            <div class="admin-auction-cards">
              <SettlementMetric
                label={t("admin-settlement-tax-income")}
                value={summary().tax_income}
              />
              <SettlementMetric
                label={t("admin-settlement-discount")}
                value={summary().discount_demand}
              />
              <SettlementMetric
                label={t("admin-settlement-compensation-out")}
                value={summary().compensation_out}
              />
              <SettlementMetric
                label={t("admin-settlement-closing")}
                value={summary().closing_balance}
              />
              <SettlementMetric
                label={t("admin-settlement-opening")}
                value={summary().opening_balance}
              />
            </div>
            <p class="admin-auction-status">
              {t("admin-settlement-summary-meta")} <Time value={summary().settled_at} />
            </p>
          </>
        )}
      </Show>
    </section>
  );
}

function SettlementMetric(props: { readonly label: string; readonly value: number }): JSX.Element {
  return (
    <div class="admin-auction-card">
      <div class="admin-auction-card-value">{formatCreditAmount(props.value)}</div>
      <div class="admin-auction-card-label">{props.label}</div>
    </div>
  );
}

/** 结算报告与恢复表格共用的本地翻页器：页码只写入本地状态，不创建筛选历史；
 * 每页数量仍由共享分页器保存偏好并同步 `page_size`。 */
export function AdminSettlementPager(props: {
  readonly currentPage: number;
  readonly totalRows: number;
  readonly pagination: AdminPagination;
  readonly page: Cell<number>;
  readonly label: string;
}): JSX.Element {
  return (
    <AdminPager
      page={props.currentPage}
      pageSize={props.pagination.size()}
      pagination={props.pagination}
      local={true}
      total={Math.min(props.totalRows, Number.MAX_SAFE_INTEGER)}
      label={props.label}
      onPageChange={(nextPage) => props.page.set(nextPage)}
    />
  );
}

function clamp(value: number, minimum: number, maximum: number): number {
  return Math.min(Math.max(value, minimum), maximum);
}
