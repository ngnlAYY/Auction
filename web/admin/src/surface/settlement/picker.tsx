// Admin 结算报表的世界、期号选择器与目录分页。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show } from "solid-js";

import type {
  AdminSettlementCatalogEntry,
  AdminSettlementCatalogPage,
} from "../../api/settlement-catalog";
import { AdminPager, type AdminPagination } from "../pagination";
import { type AdminActionFeedback, adminWorldLabel, formatAdminTime } from "../presentation";
import { resetDetailState } from "./page-model";
import { pushSettlementQuery, settlementQueryDefault } from "./query";
import type { RecoveryConfirmationDraft } from "./recovery";

interface AdminSettlementPickerProps {
  readonly catalog: AdminSettlementCatalogPage | null;
  /** 事件发生时页面仍接纳当前地址；前进/后退后的恢复 effect 可能尚未应用。 */
  readonly isCurrent: () => boolean;
  readonly worldFilter: Cell<string>;
  readonly requestedSequence: Cell<number | null>;
  readonly selectedAuctionId: Cell<string>;
  readonly catalogPage: Cell<number>;
  readonly lotPage: Cell<number>;
  readonly recoveryPage: Cell<number>;
  readonly confirmation: Cell<RecoveryConfirmationDraft | null>;
  readonly reason: Cell<string>;
  readonly actionFeedback: Cell<AdminActionFeedback | null>;
  readonly selectionGeneration: Cell<number>;
}

/** 世界与期号选择器；切换任一项都会复位详情上下文并写入新的地址。 */
export function AdminSettlementPicker(props: AdminSettlementPickerProps): JSX.Element {
  const entries = (): readonly AdminSettlementCatalogEntry[] => props.catalog?.auctions ?? [];
  const periodDisabled = (): boolean => entries().length === 0;

  return (
    <div class="admin-settlement-selector__fields">
      <label for="settlement-world">
        {t("admin-settlement-select-world")}
        <select
          id="settlement-world"
          value={props.worldFilter()}
          onChange={(event) => {
            if (!props.isCurrent()) {
              return;
            }
            const nextWorld = event.currentTarget.value;
            if (nextWorld !== "all" && nextWorld !== "main" && nextWorld !== "isekai") {
              return;
            }
            props.worldFilter.set(nextWorld);
            props.requestedSequence.set(null);
            props.selectedAuctionId.set("");
            props.catalogPage.set(1);
            resetDetailState(
              props.lotPage,
              props.recoveryPage,
              props.confirmation,
              props.reason,
              props.actionFeedback,
              props.selectionGeneration,
            );
            pushSettlementQuery({ ...settlementQueryDefault(), worldId: nextWorld });
          }}
        >
          <option value="all">{t("admin-settlement-world-all")}</option>
          <option value="main">{adminWorldLabel("main")}</option>
          <option value="isekai">{adminWorldLabel("isekai")}</option>
        </select>
      </label>
      <label for="settlement-period">
        {t("admin-settlement-select-period")}
        <select
          id="settlement-period"
          disabled={periodDisabled()}
          value={props.selectedAuctionId()}
          onChange={(event) => {
            if (!props.isCurrent()) {
              return;
            }
            const auctionId = event.currentTarget.value;
            const selected = entries().find((entry) => entry.auctionId === auctionId);
            if (selected === undefined) {
              return;
            }
            props.selectedAuctionId.set(selected.auctionId);
            props.requestedSequence.set(selected.sequence);
            resetDetailState(
              props.lotPage,
              props.recoveryPage,
              props.confirmation,
              props.reason,
              props.actionFeedback,
              props.selectionGeneration,
            );
            pushSettlementQuery({
              worldId: props.worldFilter(),
              auctionId: selected.auctionId,
              sequence: selected.sequence,
              page: props.catalogPage(),
            });
          }}
        >
          <Show
            when={!periodDisabled()}
            fallback={<option value="">{t("admin-settlement-no-periods")}</option>}
          >
            <For each={entries()} keyed={false}>
              {(entry) => (
                // 补查可能在 select 的值已应用之后才把目标场次加入选项。
                <option
                  value={entry().auctionId}
                  selected={entry().auctionId === props.selectedAuctionId()}
                >
                  {periodOptionLabel(entry())}
                </option>
              )}
            </For>
          </Show>
        </select>
      </label>
    </div>
  );
}

interface AdminSettlementCatalogPagerProps {
  readonly page: number;
  readonly pageSize: number;
  readonly total: number;
  readonly pagination: AdminPagination;
  readonly isCurrent: () => boolean;
  readonly worldFilter: Cell<string>;
  readonly requestedSequence: Cell<number | null>;
  readonly selectedAuctionId: Cell<string>;
  readonly catalogPage: Cell<number>;
  readonly lotPage: Cell<number>;
  readonly recoveryPage: Cell<number>;
  readonly confirmation: Cell<RecoveryConfirmationDraft | null>;
  readonly reason: Cell<string>;
  readonly actionFeedback: Cell<AdminActionFeedback | null>;
  readonly selectionGeneration: Cell<number>;
}

/** 目录分页器；数量变化只重置目录页，翻页则清空场次选择。 */
export function AdminSettlementCatalogPager(props: AdminSettlementCatalogPagerProps): JSX.Element {
  return (
    <AdminPager
      pagination={props.pagination}
      page={Math.max(props.page, 1)}
      pageSize={Math.max(props.pageSize, 1)}
      total={props.total}
      label={t("admin-settlement-auction-pagination")}
      canChangePageSize={props.isCurrent}
      onSizeReset={() => {
        // 数量只重置目录页；返回 true 让 AdminPager 跳过常规翻页回调，
        // 保持选定场次、两个报告分页及未结束的命令上下文。
        props.catalogPage.set(1);
        pushSettlementQuery({
          worldId: props.worldFilter(),
          auctionId: props.selectedAuctionId(),
          sequence: props.requestedSequence(),
          page: 1,
        });
        return true;
      }}
      onPageChange={(nextPage) => {
        if (!props.isCurrent()) {
          return;
        }
        props.catalogPage.set(nextPage);
        props.requestedSequence.set(null);
        props.selectedAuctionId.set("");
        resetDetailState(
          props.lotPage,
          props.recoveryPage,
          props.confirmation,
          props.reason,
          props.actionFeedback,
          props.selectionGeneration,
        );
        pushSettlementQuery({
          ...settlementQueryDefault(),
          worldId: props.worldFilter(),
          page: nextPage,
        });
      }}
    />
  );
}

function periodOptionLabel(entry: AdminSettlementCatalogEntry): string {
  return t("admin-settlement-period-option", {
    world: adminWorldLabel(entry.worldId),
    sequence: entry.sequence,
    settled_at: formatAdminTime(entry.settledAt),
  });
}
