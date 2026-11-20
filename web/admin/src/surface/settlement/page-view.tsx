// Admin 结算页的选择器、报告与恢复确认视图。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { type Accessor, For, Show } from "solid-js";

import type { AdminRecoveryReport } from "../../api/recovery";
import type { AdminSettlementCatalogPage } from "../../api/settlement-catalog";
import type { AdminPagination } from "../pagination";
import { type PendingCommands, PendingCommandsContext } from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import {
  type AdminActionFeedback,
  adminActionFeedbackIsSuccess,
  adminActionFeedbackMessage,
} from "../presentation";
import type { SettlementDetail } from "./page-model";
import { AdminSettlementCatalogPager, AdminSettlementPicker } from "./picker";
import { AdminRecoveryDialog, type RecoveryConfirmationDraft, type RecoveryRead } from "./recovery";
import { AdminSettlementReportView } from "./report";

/** 结算页面交给视图的全部状态与派生读取。 */
export interface AdminSettlementViewState {
  readonly worldFilter: Cell<string>;
  readonly selectedAuctionId: Cell<string>;
  readonly requestedSequence: Cell<number | null>;
  readonly catalogPage: Cell<number>;
  readonly catalogPagination: AdminPagination;
  readonly lotPage: Cell<number>;
  readonly recoveryPage: Cell<number>;
  readonly confirmation: Cell<RecoveryConfirmationDraft | null>;
  readonly reason: Cell<string>;
  readonly actionFeedback: Cell<AdminActionFeedback | null>;
  readonly selectionGeneration: Cell<number>;
  readonly recoveryReloadGeneration: Cell<number>;
  readonly pickerCatalog: Accessor<AdminSettlementCatalogPage | null>;
  readonly catalogLoading: Accessor<boolean>;
  readonly catalogError: Accessor<string | null>;
  readonly catalogData: Accessor<AdminSettlementCatalogPage | null>;
  readonly reportTitle: Accessor<string>;
  readonly detailLoading: Accessor<boolean>;
  readonly detail: Accessor<SettlementDetail>;
  readonly recoveryReport: Accessor<AdminRecoveryReport | null>;
  readonly actionBusy: Accessor<boolean>;
  readonly recoveryRead: Accessor<RecoveryRead>;
  readonly recoveryDraft: Accessor<RecoveryConfirmationDraft | null>;
  readonly pending: PendingCommands;
}

/** 渲染选择器、结算报告、恢复表格与确认框；状态由页面模型提供。 */
export function AdminSettlementPageView(props: {
  readonly state: AdminSettlementViewState;
}): JSX.Element {
  const state = props.state;
  return (
    <PendingCommandsContext value={state.pending}>
      <section class="admin-auction-page admin-settlement-page" aria-labelledby="settlement-title">
        <h1 id="settlement-title">{t("page-settlement-title")}</h1>
        <PendingCommandPanel owner={state.pending} />
        <div class="admin-settlement-layout">
          <section
            class="admin-settlement-layout__selector"
            aria-labelledby="settlement-selector-title"
          >
            <h2 id="settlement-selector-title">{t("admin-settlement-selector-title")}</h2>
            <p class="admin-auction-intro">{t("admin-settlement-selector-note")}</p>
            <AdminSettlementPicker
              catalog={state.pickerCatalog()}
              worldFilter={state.worldFilter}
              requestedSequence={state.requestedSequence}
              selectedAuctionId={state.selectedAuctionId}
              catalogPage={state.catalogPage}
              lotPage={state.lotPage}
              recoveryPage={state.recoveryPage}
              confirmation={state.confirmation}
              reason={state.reason}
              actionFeedback={state.actionFeedback}
              selectionGeneration={state.selectionGeneration}
            />
            <Show when={state.catalogLoading()}>
              <AsyncState kind="loading" title={t("admin-settlement-catalog-loading")} detail="" />
            </Show>
            <Show when={state.catalogError()}>
              {(error) => <AsyncState kind="error" title={error()} detail="" />}
            </Show>
            <Show when={state.catalogData()}>
              {(current) => (
                <AdminSettlementCatalogPager
                  page={current().page}
                  pageSize={current().pageSize}
                  total={current().total}
                  pagination={state.catalogPagination}
                  worldFilter={state.worldFilter}
                  requestedSequence={state.requestedSequence}
                  selectedAuctionId={state.selectedAuctionId}
                  catalogPage={state.catalogPage}
                  lotPage={state.lotPage}
                  recoveryPage={state.recoveryPage}
                  confirmation={state.confirmation}
                  reason={state.reason}
                  actionFeedback={state.actionFeedback}
                  selectionGeneration={state.selectionGeneration}
                />
              )}
            </Show>
          </section>
          <section
            class="admin-settlement-layout__report"
            aria-labelledby="settlement-report-panel-title"
          >
            <h2 id="settlement-report-panel-title">{state.reportTitle()}</h2>
            <Show when={state.selectedAuctionId() === ""}>
              <p class="admin-auction-intro">{t("admin-settlement-pick-prompt")}</p>
            </Show>
            <Show when={state.selectedAuctionId() !== ""}>
              <Show when={state.detailLoading()}>
                <AsyncState kind="loading" title={t("admin-settlement-loading")} detail="" />
              </Show>
              <Show when={state.detail().error}>
                {(error) => <AsyncState kind="error" title={error()} detail="" />}
              </Show>
              <Show when={state.detail().report}>
                {(current) => (
                  // key 必须位于迭代片段，才能随场次销毁报告内的分页草稿。
                  <For each={[current()]} keyed={(report) => report.auction.id}>
                    {(report) => (
                      <AdminSettlementReportView
                        report={report()}
                        recovery={state.recoveryReport()}
                        lotPage={state.lotPage}
                        recoveryPage={state.recoveryPage}
                        actionBusy={state.actionBusy}
                        recoveryRead={state.recoveryRead()}
                        confirmation={state.confirmation}
                        reason={state.reason}
                        actionFeedback={state.actionFeedback}
                      />
                    )}
                  </For>
                )}
              </Show>
            </Show>
            <Show when={state.actionFeedback()}>
              {(feedback) => (
                <p
                  class={[
                    "action-feedback",
                    {
                      "action-feedback--success": adminActionFeedbackIsSuccess(feedback()),
                      "action-feedback--error": !adminActionFeedbackIsSuccess(feedback()),
                    },
                  ]}
                  role={adminActionFeedbackIsSuccess(feedback()) ? "status" : "alert"}
                >
                  {adminActionFeedbackMessage(feedback(), t("admin-recovery-action-success"))}
                </p>
              )}
            </Show>
          </section>
        </div>
        <Show when={state.recoveryDraft()}>
          {(draft) => (
            <AdminRecoveryDialog
              draft={draft()}
              confirmation={state.confirmation}
              reason={state.reason}
              busy={state.actionBusy}
              feedback={state.actionFeedback}
              selectedAuctionId={state.selectedAuctionId}
              selectionGeneration={state.selectionGeneration}
              recoveryRead={state.recoveryRead()}
              recoveryReloadGeneration={state.recoveryReloadGeneration}
            />
          )}
        </Show>
      </section>
    </PendingCommandsContext>
  );
}
