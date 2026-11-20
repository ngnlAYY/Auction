// Admin recovery 表格、命令确认对话框和幂等提交交互。

import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { err, ok } from "@auction/web-shared/result";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { Time } from "@auction/web-shared/ui/components/time";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { type Accessor, For, Show } from "solid-js";

import { adminApiFailureIsConflict } from "../../api/failure";
import {
  type AdminRecovery,
  type AdminRecoveryReport,
  confirmAdminRecovery,
  prepareAdminRecoveryConfirmationOnce,
} from "../../api/recovery";
import { useAdminSessionHost } from "../auth/shell";
import { adminLotCodeFor } from "../lots/presentation";
import type { AdminPagination } from "../pagination";
import {
  commandDeliveryReported,
  requirePendingCommands,
  setIfAlive,
  trySetIfAlive,
} from "../pending";
import { type AdminActionFeedback, adminActionFeedbackIsSuccess } from "../presentation";
import { FieldError, useFocusField } from "../validation";
import {
  pageSlice,
  recoveryActionSummary,
  recoveryCommandLabel,
  recoveryStageLabel,
  sendAttributionBlockerLabel,
} from "./presentation";
import { readSettlementQuery } from "./query";
import {
  type RecoveryReasonIssue,
  recoveryReasonIssueMessage,
  validateRecoveryReason,
} from "./recovery-reason";
import { AdminSettlementPager } from "./report";

/** 恢复确认冻结的场次、恢复事实与操作；提交后不再读取表格或弹窗草稿。 */
export interface RecoveryConfirmationDraft {
  auctionId: string;
  recoveryId: string;
  stateVersion: number;
  action: string;
  equipmentName: string;
  lotCode: string;
}

/** 恢复报告与命令回执共同依赖的读取世代。 */
export interface RecoveryRead {
  reload: Cell<number>;
  recoveryReload: Cell<number>;
  generation: readonly [number, number];
  /** 页面选择上下文的存活判定；命令行应在页面销毁后停止写参与选择的信号与反馈。 */
  alive: () => boolean;
}

/** 报告与回执都只认同一代读取；任一世代推进后旧表格不能再授权命令。 */
export function recoveryReadIsCurrent(read: RecoveryRead): boolean {
  return read.reload() === read.generation[0] && read.recoveryReload() === read.generation[1];
}

/** 恢复报告表格；报告为空时显示空状态，报告缺失时只保留标题。 */
export function AdminRecoveryTable(props: {
  readonly report: AdminRecoveryReport | null;
  readonly currentPage: number;
  readonly pagination: AdminPagination;
  readonly page: Cell<number>;
  readonly actionBusy: Accessor<boolean>;
  readonly recoveryRead: RecoveryRead;
  readonly confirmation: Cell<RecoveryConfirmationDraft | null>;
  readonly reason: Cell<string>;
  readonly actionFeedback: Cell<AdminActionFeedback | null>;
}): JSX.Element {
  const rows = (): AdminRecovery[] => {
    const report = props.report;
    return report === null
      ? []
      : pageSlice(report.recoveries, props.currentPage, props.pagination.size());
  };
  return (
    <section
      class="admin-auction-report admin-auction-recovery"
      aria-labelledby="settlement-recovery-title"
    >
      <h2 id="settlement-recovery-title">{t("admin-recovery-title")}</h2>
      <Show when={props.report}>
        {(report) => (
          <>
            <Show
              when={report().recoveries.length === 0}
              fallback={
                <div class="admin-settlement__table-wrap">
                  <table
                    class="admin-shell__table admin-auction-table"
                    aria-label={t("admin-recovery-title")}
                  >
                    <TableColumns layout="admin-recovery" widths={[6, 27, 10, 18, 14, 12, 13]} />
                    <thead>
                      <tr>
                        <th scope="col">{t("admin-recovery-lot")}</th>
                        <th scope="col">{t("admin-recovery-equipment")}</th>
                        <th scope="col">{t("admin-recovery-stage")}</th>
                        <th scope="col">{t("admin-recovery-deadline")}</th>
                        <th scope="col">{t("admin-recovery-evidence")}</th>
                        <th scope="col">{t("admin-recovery-actions")}</th>
                        <th scope="col">{t("admin-recovery-commands")}</th>
                      </tr>
                    </thead>
                    <tbody>
                      <For each={rows()} keyed={false}>
                        {(recovery) => (
                          <AdminRecoveryRow
                            recovery={recovery()}
                            actionBusy={props.actionBusy}
                            recoveryRead={props.recoveryRead}
                            confirmation={props.confirmation}
                            reason={props.reason}
                            actionFeedback={props.actionFeedback}
                          />
                        )}
                      </For>
                    </tbody>
                  </table>
                </div>
              }
            >
              <AsyncState kind="empty" title={t("admin-recovery-empty")} detail="" />
            </Show>
            <AdminSettlementPager
              currentPage={props.page()}
              totalRows={report().recoveries.length}
              pagination={props.pagination}
              page={props.page}
              label={t("admin-recovery-pagination")}
            />
          </>
        )}
      </Show>
    </section>
  );
}

function AdminRecoveryRow(props: {
  readonly recovery: AdminRecovery;
  readonly actionBusy: Accessor<boolean>;
  readonly recoveryRead: RecoveryRead;
  readonly confirmation: Cell<RecoveryConfirmationDraft | null>;
  readonly reason: Cell<string>;
  readonly actionFeedback: Cell<AdminActionFeedback | null>;
}): JSX.Element {
  const pending = requirePendingCommands();
  const recoveryId = (): string => props.recovery.id;
  const displayEquipmentIdentity = (): string =>
    `${props.recovery.equipment_name}(${props.recovery.world_id})`;
  const lotCode = (): string =>
    adminLotCodeFor(props.recovery.equipment_category, props.recovery.lot_number);
  const evidence = (): string => {
    // 发送归属不成立时显示具体原因（Worker 阻断与 Admin 确认共用同一判定），否则沿用阻断码或
    // 收回证明。
    const blocker = props.recovery.send_attribution_blocker;
    if (blocker !== null) {
      return sendAttributionBlockerLabel(blocker);
    }
    return props.recovery.blocking_code ?? props.recovery.custody_proof_kind ?? t("value-empty");
  };
  const actions = (): string => recoveryActionSummary(props.recovery.actions);
  const entity = (): string => settlementRecoveryEntity(recoveryId());
  return (
    <tr>
      <td data-label={t("admin-recovery-lot")}>{lotCode()}</td>
      <td data-label={t("admin-recovery-equipment")}>
        <TruncatedText value={displayEquipmentIdentity()} size="wide" portal={true} />
      </td>
      <td data-label={t("admin-recovery-stage")}>{recoveryStageLabel(props.recovery.stage)}</td>
      <td data-label={t("admin-recovery-deadline")}>
        <Time value={props.recovery.payment_deadline_at} />
      </td>
      <td data-label={t("admin-recovery-evidence")}>
        <TruncatedText value={evidence()} portal={true} />
      </td>
      <td data-label={t("admin-recovery-actions")}>
        <TruncatedText value={actions()} size="wide" portal={true} />
      </td>
      <td data-label={t("admin-recovery-commands")}>
        <div class="admin-auction-actions">
          <For each={props.recovery.allowed_commands} keyed={false}>
            {(action) => (
              <button
                class="admin-btn-primary admin-auction-recovery-action"
                type="button"
                disabled={
                  !recoveryReadIsCurrent(props.recoveryRead) ||
                  props.actionBusy() ||
                  pending.contains(entity())
                }
                onClick={() => {
                  if (
                    !recoveryReadIsCurrent(props.recoveryRead) ||
                    !readSettlementQuery().ok ||
                    pending.anyBusy() ||
                    pending.contains(entity())
                  ) {
                    return;
                  }
                  props.confirmation.set({
                    auctionId: props.recovery.auction_id,
                    recoveryId: props.recovery.id,
                    stateVersion: props.recovery.state_version,
                    action: action(),
                    equipmentName: props.recovery.equipment_name,
                    lotCode: lotCode(),
                  });
                  props.reason.set("");
                  const feedback = props.actionFeedback();
                  if (feedback !== null && adminActionFeedbackIsSuccess(feedback)) {
                    props.actionFeedback.set(null);
                  }
                }}
              >
                {recoveryCommandLabel(action())}
              </button>
            )}
          </For>
        </div>
      </td>
    </tr>
  );
}

/** 恢复确认框：只提交打开时冻结的场次与版本，未知结果交由认证外壳保留同一命令。 */
export function AdminRecoveryDialog(props: {
  readonly draft: RecoveryConfirmationDraft;
  readonly confirmation: Cell<RecoveryConfirmationDraft | null>;
  readonly reason: Cell<string>;
  readonly busy: Accessor<boolean>;
  readonly feedback: Cell<AdminActionFeedback | null>;
  readonly selectedAuctionId: Cell<string>;
  readonly selectionGeneration: Cell<number>;
  readonly recoveryRead: RecoveryRead;
  readonly recoveryReloadGeneration: Cell<number>;
}): JSX.Element {
  const pending = requirePendingCommands();
  const host = useAdminSessionHost();
  const focusField = useFocusField();
  const reasonIssue = createCell<RecoveryReasonIssue | null>(null);
  const confirmation = props.confirmation;
  const reason = props.reason;
  const feedback = props.feedback;
  const selectedAuctionId = props.selectedAuctionId;
  const selectionGeneration = props.selectionGeneration;
  const recoveryReloadGeneration = props.recoveryReloadGeneration;
  const openedGeneration = selectionGeneration();
  const requiresReason = (): boolean => props.draft.action !== "reclaimed";
  const showsRefund = (): boolean => activatesBuyerRefund(props.draft.action);
  const entity = (): string => settlementRecoveryEntity(props.draft.recoveryId);
  const blocked = (): boolean => pending.contains(entity());
  const issueMessage = (): string | null => {
    const issue = reasonIssue();
    return issue === null ? null : recoveryReasonIssueMessage(issue);
  };
  // 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管；共享组件暂不支持 aria-describedby，
  // 描述段落保留在内容区首部。
  return (
    <Dialog
      title={recoveryCommandLabel(props.draft.action)}
      closeLabel={t("admin-recovery-cancel")}
      busy={props.busy()}
      onClose={() => {
        confirmation.set(null);
        reason.set("");
        reasonIssue.set(null);
      }}
    >
      <p>
        {t("admin-recovery-confirm-description", {
          lot: props.draft.lotCode,
          equipment: props.draft.equipmentName,
        })}
      </p>
      <Show when={showsRefund()}>
        <p>{t("admin-recovery-confirm-refund-description")}</p>
      </Show>
      <form
        class="admin-auction-recovery-confirm"
        novalidate={true}
        onSubmit={(event) => {
          event.preventDefault();
          const draft: RecoveryConfirmationDraft = {
            auctionId: props.draft.auctionId,
            recoveryId: props.draft.recoveryId,
            stateVersion: props.draft.stateVersion,
            action: props.draft.action,
            equipmentName: props.draft.equipmentName,
            lotCode: props.draft.lotCode,
          };
          const frozenEntity = settlementRecoveryEntity(draft.recoveryId);
          // 首次提交只作用于打开确认时选中的场次，重试继续使用冻结的意图。
          if (
            !recoveryReadIsCurrent(props.recoveryRead) ||
            !readSettlementQuery().ok ||
            pending.anyBusy() ||
            pending.contains(frozenEntity) ||
            selectedAuctionId() !== draft.auctionId ||
            selectionGeneration() !== openedGeneration
          ) {
            return;
          }
          const checked = validateRecoveryReason(draft.action, reason());
          if (!checked.ok) {
            reasonIssue.set(checked.error);
            focusField("admin-recovery-reason");
            return;
          }
          const reasonValue = checked.value;
          // 存活判定只在提交时取一次；命令重试继续使用同一页面选择上下文。
          const selectionAlive = props.recoveryRead.alive;
          const submittedGeneration = openedGeneration;
          const submission = prepareAdminRecoveryConfirmationOnce({
            recoveryId: draft.recoveryId,
            expectedStateVersion: draft.stateVersion,
            action: draft.action,
            reason: reasonValue,
          });
          const successAuctionId = draft.auctionId;
          const failureAuctionId = draft.auctionId;
          const submitted = pending.submitWithReceipt(
            frozenEntity,
            recoveryCommandLabel(draft.action),
            async (commandId, signal) => {
              const result = await confirmAdminRecovery(host, commandId, submission, signal);
              if (signal.aborted) {
                return result.ok ? ok(result.value) : err(result.error.code);
              }
              if (
                !result.ok &&
                adminApiFailureIsConflict(result.error) &&
                sameRecoverySelection(
                  selectionAlive,
                  selectedAuctionId,
                  selectionGeneration,
                  draft.auctionId,
                  submittedGeneration,
                )
              ) {
                recoveryReloadGeneration.set(saturatingIncrement(recoveryReloadGeneration.peek()));
              }
              return result.ok ? ok(result.value) : err(result.error.code);
            },
            (_value, sourceAlive) => {
              const delivered =
                sourceAlive &&
                sameRecoverySelection(
                  selectionAlive,
                  selectedAuctionId,
                  selectionGeneration,
                  successAuctionId,
                  submittedGeneration,
                ) &&
                trySetIfAlive(feedback, { kind: "success" }, selectionAlive);
              return commandDeliveryReported(delivered, () => null);
            },
            (code) => {
              if (
                sameRecoverySelection(
                  selectionAlive,
                  selectedAuctionId,
                  selectionGeneration,
                  failureAuctionId,
                  submittedGeneration,
                )
              ) {
                setIfAlive(feedback, { kind: "error", code }, selectionAlive);
              }
            },
          );
          if (submitted) {
            // 完整意图归认证外壳；关闭展示不能修改未知命令的原因、版本或 UUID。
            confirmation.set(null);
            reason.set("");
            reasonIssue.set(null);
            feedback.set(null);
          }
        }}
      >
        <Show when={requiresReason()}>
          <label for="admin-recovery-reason">
            {t("admin-recovery-reason")}
            <textarea
              id="admin-recovery-reason"
              name="reason"
              rows="4"
              required={true}
              value={reason()}
              aria-invalid={reasonIssue() !== null ? "true" : "false"}
              aria-describedby={reasonIssue() !== null ? "admin-recovery-reason-error" : undefined}
              onInput={(event) => {
                reason.set(event.currentTarget.value);
                reasonIssue.set(null);
              }}
            />
          </label>
          <FieldError id="admin-recovery-reason-error" message={issueMessage()} />
        </Show>
        <div class="admin-shell__dialog-actions">
          <button
            class="admin-btn-secondary admin-auction-recovery-action"
            type="button"
            disabled={props.busy()}
            autofocus={true}
            onClick={() => {
              confirmation.set(null);
              reason.set("");
              reasonIssue.set(null);
            }}
          >
            {t("admin-recovery-cancel")}
          </button>
          <button
            class="admin-btn-primary admin-auction-recovery-action"
            type="submit"
            disabled={props.busy() || blocked() || !recoveryReadIsCurrent(props.recoveryRead)}
          >
            {t("admin-recovery-confirm")}
          </button>
        </div>
      </form>
    </Dialog>
  );
}

function settlementRecoveryEntity(recoveryId: string): string {
  return `settlement-recovery:${recoveryId}`;
}

/** 无效链接会卸载选择视图。已提交命令的回执不得再读取销毁的视图状态。 */
function sameRecoverySelection(
  sourceAlive: () => boolean,
  auction: Cell<string>,
  generation: Cell<number>,
  expectedAuction: string,
  expectedGeneration: number,
): boolean {
  return (
    sourceAlive() && auction.peek() === expectedAuction && generation.peek() === expectedGeneration
  );
}

function activatesBuyerRefund(action: string): boolean {
  return action === "equipment_recovered";
}

function saturatingIncrement(value: number): number {
  return value >= Number.MAX_SAFE_INTEGER ? Number.MAX_SAFE_INTEGER : value + 1;
}
