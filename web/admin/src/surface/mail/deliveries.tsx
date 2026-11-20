// 收件关联投递的安全展示与手动重排；原邮件处理结果和外部完成证明仍由后端拥有。

import {
  type AdminDeliveryActionBlocker,
  type AdminDeliveryActionsResponse,
  adminDeliveryActionAvailabilityBlocker,
} from "@auction/contracts/dto/admin/delivery";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { err, ok, type Result } from "@auction/web-shared/result";
import { TableColumns } from "@auction/web-shared/ui/components/table-columns";
import { TruncatedText } from "@auction/web-shared/ui/components/truncated-text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, onCleanup, Show } from "solid-js";
import { fetchDeliveryDetail } from "../../api/delivery";
import type { Delivery } from "../../api/delivery/decode";
import {
  type DeliveryAction,
  executeDelivery,
  prepareDeliveryCommandOnce,
} from "../../api/delivery/queue";
import type { AdminMailIntakeDelivery } from "../../api/mail";
import { useAdminSessionHost } from "../auth/shell";
import { deliveryCommandEntity } from "../deliveries";
import {
  DeliveryConfirmationFacts,
  DeliveryEvidenceStatus,
  ReconcileEvidenceCandidates,
} from "../deliveries/dialog";
import { actionBlockerLabel, deliveryStatusLabel } from "../deliveries/labels";
import { closeDeliveryModal, DeliveryModal } from "../deliveries/modal";
import { AdminPager, useAdminPagination } from "../pagination";
import { commandDeliveryReported, requirePendingCommands } from "../pending";
import type { AdminActionFeedback } from "../presentation";

const DIALOG_ID = "mail-delivery-action-dialog";
const I64_MAX = 9_223_372_036_854_775_807n;
const U64_MAX = 18_446_744_073_709_551_615n;

/** 由邮件页持有的反馈写入口，每次写入或清除只针对指定投递。 */
export interface MailDeliveryNoticeWriter {
  set(
    notice: readonly [intakeId: string, feedback: AdminActionFeedback] | null,
    deliveryId: string,
  ): void;
}

/** 收件关联投递面板属性；反馈与刷新状态由邮件页持有。 */
export interface MailDeliveriesPanelProps {
  readonly intakeId: string;
  readonly deliveries: readonly AdminMailIntakeDelivery[];
  readonly notice: MailDeliveryNoticeWriter;
  readonly pageAlive: () => boolean;
  readonly refresh: Cell<bigint>;
}

/** 当前收件的投递分页、重排入口与手动确认。 */
export function MailDeliveriesPanel(props: MailDeliveriesPanelProps): JSX.Element {
  const commands = requirePendingCommands();
  const host = useAdminSessionHost();
  const page = createCell(1);
  const pagination = useAdminPagination(
    "mail-deliveries",
    ["mail_delivery_page_size"],
    null,
    I64_MAX,
  );
  const busy = (): boolean => commands.anyBusy();
  const pending = createCell<AdminMailIntakeDelivery | null>(null);
  const lifetime = createLifetimeToken();
  const total = (): number => props.deliveries.length;
  const currentPage = (): number => {
    const count = total();
    const pageSize = pagination.size();
    const maximum = count === 0 ? 1 : 1 + Math.trunc((count - 1) / pageSize);
    return Math.min(Math.max(page(), 1), maximum);
  };
  const visible = createValueMemo(() => {
    const pageSize = pagination.size();
    const offset = (currentPage() - 1) * pageSize;
    return props.deliveries.slice(offset, offset + pageSize);
  });
  const confirmAction = (request: readonly [Delivery, DeliveryAction, string]): void => {
    const [delivery, action, operation] = request;
    if (busy()) {
      return;
    }
    const selected = pending.peek();
    if (
      commands.contains(deliveryCommandEntity(delivery.id)) ||
      selected === null ||
      selected.id !== delivery.id ||
      selected.fencing_version !== delivery.fencing_version
    ) {
      return;
    }
    // 提交时冻结命令参数；执行体与重试都不再读取弹窗里的投递对象。
    const intakeId = props.intakeId;
    const deliveryId = delivery.id;
    const expectedFencingVersion = delivery.fencing_version;
    const operationLabel = retryLabel(operation);
    const notice = props.notice;
    const pageAlive = props.pageAlive;
    const refresh = props.refresh;
    const preparation = prepareDeliveryCommandOnce({
      deliveryId,
      action,
      expectedFencingVersion,
      externalReference: null,
      failureCode: null,
    });
    notice.set(null, deliveryId);
    const submitted = commands.submitWithReceipt(
      deliveryCommandEntity(deliveryId),
      operationLabel,
      async (commandId, signal) => {
        const result = await executeDelivery(host, preparation, commandId, signal);
        return result;
      },
      (_updated, sourceAlive) => {
        // 邮件页仍存活时，成功回执覆盖同一投递的旧错误，即使原面板已卸载。
        const pageActive = sourceAlive && pageAlive();
        if (pageActive) {
          notice.set([intakeId, { kind: "success" }], deliveryId);
        }
        const delivered = pageActive && lifetime.active();
        return commandDeliveryReported(delivered, () => ({
          kind: "text",
          text: t("admin-mail-delivery-requeued"),
        }));
      },
      (code) => {
        if (
          code === "stale_delivery_state" ||
          code === "stale_state_version" ||
          code === "delivery_recipient_renamed"
        ) {
          const current = refresh();
          refresh.set(current === U64_MAX ? U64_MAX : current + 1n);
        }
        if (pageAlive()) {
          notice.set([intakeId, { kind: "error", code }], deliveryId);
        }
      },
    );
    if (submitted) {
      closeDeliveryModal(DIALOG_ID);
      pending.set(null);
    }
  };
  return (
    <section class="admin-mail-deliveries" aria-labelledby="admin-mail-deliveries-title">
      <h3 id="admin-mail-deliveries-title">{t("admin-mail-deliveries-title")}</h3>
      <p class="admin-panel__note">{t("admin-mail-deliveries-help")}</p>
      <div class="admin-shell__table-wrap">
        <table
          class="admin-shell__table admin-mail-deliveries__table"
          aria-label={t("admin-mail-deliveries-title")}
        >
          {/* 操作类型、收件人、任务 ID、状态、尝试、失败原因、操作合计 100%。 */}
          <TableColumns layout="admin-mail-deliveries" widths={[12, 14, 20, 14, 6, 19, 15]} />
          <thead>
            <tr>
              <th scope="col">{t("admin-mail-delivery-operation")}</th>
              <th scope="col">{t("delivery-to")}</th>
              <th scope="col">{t("admin-mail-delivery-id")}</th>
              <th scope="col">{t("delivery-status")}</th>
              <th scope="col">{t("delivery-attempts")}</th>
              <th scope="col">{t("delivery-failure-code")}</th>
              <th scope="col">{t("catalog-actions")}</th>
            </tr>
          </thead>
          <tbody>
            <For each={visible()} keyed={(delivery) => delivery.id}>
              {(delivery) => (
                <MailDeliveryRow
                  delivery={delivery()}
                  busy={busy() || commands.contains(deliveryCommandEntity(delivery().id))}
                  onRetry={(target) => {
                    if (busy() || commands.contains(deliveryCommandEntity(target.id))) {
                      return;
                    }
                    props.notice.set(null, target.id);
                    pending.set(target);
                  }}
                />
              )}
            </For>
          </tbody>
        </table>
      </div>
      <Show when={total() === 0}>
        <p class="admin-panel__note">{t("admin-mail-deliveries-empty")}</p>
      </Show>
      <AdminPager
        pagination={pagination}
        local
        page={page()}
        pageSize={pagination.size()}
        total={total()}
        label={t("admin-mail-deliveries-title")}
        onPageChange={(next) => page.set(next)}
      />
      <Show when={pending()}>
        {(delivery) => (
          <MailRetryDialog
            delivery={delivery()}
            busy={busy()}
            onSubmit={confirmAction}
            onCancel={() => pending.set(null)}
          />
        )}
      </Show>
    </section>
  );
}

interface MailRetryDialogProps {
  readonly delivery: AdminMailIntakeDelivery;
  readonly busy: boolean;
  readonly onSubmit: (request: readonly [Delivery, DeliveryAction, string]) => void;
  readonly onCancel: () => void;
}

/** 当前 fence 的手动重排确认框；读取失败与版本变化都不允许提交。 */
function MailRetryDialog(props: MailRetryDialogProps): JSX.Element {
  const host = useAdminSessionHost();
  const deliveryId = createValueMemo(() => props.delivery.id);
  const resource = createTaskResource(deliveryId, async (id, context) => {
    const result = await fetchDeliveryDetail(host, id, context.signal);
    if (!context.alive()) {
      return null;
    }
    return result;
  });
  const loaded = createValueMemo(() => {
    if (resource.state() !== "ready") {
      return null;
    }
    return resource() ?? null;
  });
  const loadError = createValueMemo(() => {
    const current = loaded();
    return current !== null && !current.ok ? current.error : null;
  });
  const projected = createValueMemo(() => {
    const current = loaded();
    return current?.ok ? current.value : null;
  });
  const sameFence = (): boolean => {
    const current = projected();
    return (
      current !== null && current.detail.delivery.fencing_version === props.delivery.fencing_version
    );
  };
  const evidenceStatus = (): string | null => {
    const current = projected();
    if (current === null) {
      return loaded() === null ? t("delivery-reconcile-sightings-loading") : null;
    }
    if (!sameFence()) {
      return null;
    }
    const decision = retryAction(current.actions);
    return decision.ok &&
      decision.value === "reconcile_retry" &&
      current.detail.evidence.length === 0
      ? t("delivery-reconcile-sightings-empty")
      : null;
  };
  const retry = createValueMemo((): Result<DeliveryAction, AdminDeliveryActionBlocker> | null => {
    const current = projected();
    return current === null ? null : retryAction(current.actions);
  });
  const retrySuccess = createValueMemo(() => {
    const decision = retry();
    return decision?.ok ? decision.value : null;
  });
  const retryBlocker = createValueMemo(() => {
    const decision = retry();
    return decision !== null && !decision.ok ? decision.error : null;
  });
  return (
    <DeliveryModal
      dialogId={DIALOG_ID}
      title={retryLabel(props.delivery.operation)}
      busy={props.busy}
      onCancel={() => props.onCancel()}
    >
      <DeliveryEvidenceStatus message={evidenceStatus()} />
      <div>
        <Show when={loadError() !== null}>
          <p class="action-feedback action-feedback--error" role="alert">
            {t("delivery-reconcile-sightings-error")}
          </p>
          <button type="button" disabled={props.busy} onClick={() => resource.restart()}>
            {t("admin-retry")}
          </button>
        </Show>
        <Show when={projected()}>
          {(current) => (
            <Show
              when={sameFence()}
              fallback={
                <p class="action-feedback action-feedback--error" role="alert">
                  {t("delivery-action-blocked-stale")}
                </p>
              }
            >
              <Show
                when={retrySuccess()}
                fallback={
                  <Show when={retryBlocker()}>
                    {(blocker) => <p class="admin-panel__note">{actionBlockerLabel(blocker())}</p>}
                  </Show>
                }
              >
                {(action) => (
                  <>
                    <DeliveryConfirmationFacts delivery={current().detail.delivery} />
                    <Show when={action() === "reconcile_retry"}>
                      <ReconcileEvidenceCandidates evidence={current().detail.evidence} />
                      <p class="admin-mail-deliveries__warning">
                        {t("admin-mail-delivery-confirm-unknown")}
                      </p>
                    </Show>
                    <button
                      type="button"
                      class="admin-btn-primary"
                      disabled={props.busy}
                      onClick={() =>
                        props.onSubmit([
                          current().detail.delivery,
                          action(),
                          props.delivery.operation,
                        ])
                      }
                    >
                      {t("delivery-confirm")}
                    </button>
                  </>
                )}
              </Show>
            </Show>
          )}
        </Show>
      </div>
    </DeliveryModal>
  );
}

interface MailDeliveryRowProps {
  readonly delivery: AdminMailIntakeDelivery;
  readonly busy: boolean;
  readonly onRetry: (delivery: AdminMailIntakeDelivery) => void;
}

/** 投递表格的一行；按钮只传回当前行的安全投影。 */
function MailDeliveryRow(props: MailDeliveryRowProps): JSX.Element {
  const target = () => props.delivery;
  return (
    <tr>
      <td>
        <TruncatedText value={operationLabel(props.delivery.operation)} />
      </td>
      <td>
        <TruncatedText value={props.delivery.recipient} />
      </td>
      <td>
        <TruncatedText value={props.delivery.id} />
      </td>
      <td>
        <span class="admin-delivery-badge">{deliveryStatusLabel(props.delivery.status)}</span>
      </td>
      <td>{props.delivery.attempt_count}</td>
      <td>
        <TruncatedText value={props.delivery.failure_code ?? t("value-empty")} />
      </td>
      <td>
        <button type="button" disabled={props.busy} onClick={() => props.onRetry(target())}>
          {t("delivery-review-actions")}
        </button>
      </td>
    </tr>
  );
}

/** 按服务端当前 fence 的准入决定手动重排动作。 */
function retryAction(
  actions: AdminDeliveryActionsResponse,
): Result<DeliveryAction, AdminDeliveryActionBlocker> {
  const retry = adminDeliveryActionAvailabilityBlocker(actions.retry_failed);
  const reconcile = adminDeliveryActionAvailabilityBlocker(actions.reconcile_retry);
  if (retry === null) {
    return reconcile === null ? err("status_mismatch") : ok("retry");
  }
  if (reconcile === null) {
    return ok("reconcile_retry");
  }
  return err(retry === "status_mismatch" ? reconcile : retry);
}

/** 投递操作类型的中文标签；未知值显示空值占位。 */
function operationLabel(operation: string): string {
  switch (operation) {
    case "send":
      return t("admin-mail-delivery-send");
    case "return":
      return t("admin-mail-delivery-return");
    case "take":
      return t("admin-mail-delivery-take");
    default:
      return t("value-empty");
  }
}

/** 手动重排动作的中文标签；未知值显示空值占位。 */
function retryLabel(operation: string): string {
  switch (operation) {
    case "send":
      return t("admin-mail-delivery-resend");
    case "return":
      return t("admin-mail-delivery-return-again");
    case "take":
      return t("admin-mail-delivery-take-again");
    default:
      return t("value-empty");
  }
}

interface LifetimeToken {
  active(): boolean;
}

/** 组件卸载后仍可能收到命令回执；写局部反馈前检查自身 owner 是否仍存活。 */
function createLifetimeToken(): LifetimeToken {
  let active = true;
  onCleanup(() => {
    active = false;
  });
  return { active: () => active };
}
