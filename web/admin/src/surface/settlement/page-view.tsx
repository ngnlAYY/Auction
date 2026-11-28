// Admin 结算页的选择器、报告与恢复确认视图。

import { focusElementById } from "@auction/web-shared/platform/a11y";
import { topModal } from "@auction/web-shared/platform/a11y/escape";
import { sleep } from "@auction/web-shared/platform/timer";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { type Accessor, createEffect, For, Show } from "solid-js";

import type { AdminRecoveryReport } from "../../api/recovery";
import type { AdminSettlementReport } from "../../api/settlement";
import type { AdminSettlementCatalogPage } from "../../api/settlement-catalog";
import type { AdminPagination } from "../pagination";
import { type PendingCommands, PendingCommandsContext } from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import {
  type AdminActionFeedback,
  adminActionFeedbackIsSuccess,
  adminActionFeedbackMessage,
  adminApiErrorMessage,
} from "../presentation";
import { AdminSettlementCatalogPager, AdminSettlementPicker } from "./picker";
import { type SettlementRead, type SettlementReadError, SettlementReadRetry } from "./read-retry";
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
  readonly contradictoryLink: Accessor<boolean>;
  readonly locationIsCurrent: () => boolean;
  readonly catalogPageForPager: Accessor<AdminSettlementCatalogPage | null>;
  readonly catalogPagerIsCurrent: () => boolean;
  readonly reportTitle: Accessor<string>;
  readonly detailLoading: (read: "settlement" | "recovery") => boolean;
  readonly reportAuctionId: Accessor<string | null>;
  readonly report: Accessor<AdminSettlementReport | null>;
  readonly readErrors: Record<SettlementRead, Accessor<SettlementReadError | null>>;
  readonly readPending: (read: SettlementRead) => boolean;
  readonly retryRead: (read: SettlementRead, scope: string) => void;
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
  const focusTasks = createTaskScope();
  createEffect(
    () => state.recoveryDraft() !== null,
    (open, wasOpen) => {
      if (open || wasOpen !== true) {
        return;
      }
      focusTasks.spawn(async (context) => {
        // 先等移除弹窗与焦点陷阱还焦；保留浏览器或用户已放置的焦点。
        if (
          !(await sleep(0, context.signal)) ||
          !context.alive() ||
          typeof document === "undefined"
        ) {
          return;
        }
        const focus = document.activeElement;
        if (
          (focus === null || focus === document.body || focus === document.documentElement) &&
          topModal(document) === null
        ) {
          focusElementById("settlement-report-panel-title");
        }
      });
    },
  );
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
            <h2 id="settlement-selector-title" tabindex="-1">
              {t("admin-settlement-selector-title")}
            </h2>
            <p class="admin-auction-intro">{t("admin-settlement-selector-note")}</p>
            <AdminSettlementPicker
              catalog={state.pickerCatalog()}
              isCurrent={state.locationIsCurrent}
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
            <ReadError state={state} read="catalog" />
            <ReadError state={state} read="catalogLookup" />
            <Show when={state.contradictoryLink()}>
              <AsyncState kind="error" title={adminApiErrorMessage("invalid_response")} detail="" />
            </Show>
            <Show when={state.catalogPageForPager()}>
              {(current) => (
                <AdminSettlementCatalogPager
                  page={current().page}
                  pageSize={current().pageSize}
                  total={current().total}
                  pagination={state.catalogPagination}
                  isCurrent={state.catalogPagerIsCurrent}
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
            <h2 id="settlement-report-panel-title" tabindex="-1">
              {state.reportTitle()}
            </h2>
            <Show when={state.selectedAuctionId() === ""}>
              <p class="admin-auction-intro">{t("admin-settlement-pick-prompt")}</p>
            </Show>
            <Show when={state.reportAuctionId()}>
              {(auctionId: Accessor<string>) => (
                <>
                  <Show when={state.detailLoading("settlement")}>
                    <AsyncState kind="loading" title={t("admin-settlement-loading")} detail="" />
                  </Show>
                  <ReadError state={state} read="settlement" />
                  <For each={[auctionId()]} keyed={(currentId) => currentId}>
                    {() => (
                      <AdminSettlementReportView
                        report={state.report()}
                        recovery={state.recoveryReport()}
                        lotPage={state.lotPage}
                        recoveryPage={state.recoveryPage}
                        actionBusy={state.actionBusy}
                        recoveryRead={state.recoveryRead()}
                        confirmation={state.confirmation}
                        reason={state.reason}
                        actionFeedback={state.actionFeedback}
                        recoveryStatus={
                          <>
                            <Show when={state.detailLoading("recovery")}>
                              <AsyncState
                                kind="loading"
                                title={t("admin-settlement-loading")}
                                detail=""
                              />
                            </Show>
                            <ReadError state={state} read="recovery" />
                          </>
                        }
                      />
                    )}
                  </For>
                </>
              )}
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

function ReadError(props: {
  readonly state: AdminSettlementViewState;
  readonly read: SettlementRead;
}): JSX.Element {
  const error = () => props.state.readErrors[props.read]();
  return (
    <Show when={error()?.scope} keyed>
      {(scope) => (
        <SettlementReadRetry
          read={props.read}
          scope={scope}
          message={error()?.message ?? ""}
          pending={props.state.readPending(props.read)}
          onRetry={() => props.state.retryRead(props.read, scope)}
        />
      )}
    </Show>
  );
}
