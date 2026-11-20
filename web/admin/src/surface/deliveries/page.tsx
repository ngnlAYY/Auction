// Delivery 页面状态、查询资源和动作编排。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { err, unreachable } from "@auction/web-shared/result";
import { useRouter } from "@auction/web-shared/router";
import { AsyncState } from "@auction/web-shared/ui/components/async-state";
import { t } from "@auction/web-shared/ui/i18n";
import { validUuid } from "@auction/web-shared/ui/validation";
import type { JSX } from "@solidjs/web";
import { Match, onCleanup, Show, Switch } from "solid-js";

import type { Delivery, DeliveryPage } from "../../api/delivery/decode";
import {
  type DeliveryAction,
  executeDelivery,
  fetchDeliveries,
  prepareDeliveryCommandOnce,
} from "../../api/delivery/queue";
import { trimRustWhitespace } from "../../api/session-time";
import { useAdminSessionHost } from "../auth/shell";
import { AdminPager, useAdminPage, useAdminPagination } from "../pagination";
import { PendingCommandsContext, setIfAlive } from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { advanceCommandReload, createCommandReload, usePendingCommands } from "../pending/sources";
import { type AdminActionFeedback, adminApiErrorMessage } from "../presentation";
import { deliveriesQueryFilters, useDeliveriesQueryState } from "../query/deliveries";
import { deliveryCommandEntity } from ".";
import { DeliveryDialog, DIALOG_ID } from "./dialog";
import { DeliveryFilters, type DeliveryFiltersDraft } from "./filters";
import { actionFeedbackMessage } from "./labels";
import { closeDeliveryModal } from "./modal";
import {
  type DeliveryActionRequest,
  type DeliveryDialogKind,
  type DeliveryDialogState,
  type DeliveryReconcileDraft,
  deliveryReconcileDraftForDelivery,
} from "./model";
import { DeliveryTable } from "./table";

type DeliveriesView =
  | { readonly kind: "loading" }
  | { readonly kind: "error"; readonly code: string }
  | { readonly kind: "ready"; readonly page: DeliveryPage };

/** 加载 delivery 安全投影并编排带 fencing 的操作员动作。 */
export function page(): JSX.Element {
  const host = useAdminSessionHost();
  const refresh = createCommandReload(0n);
  const notice = createCell<AdminActionFeedback | null>(null);
  const successAction = createCell<DeliveryAction | null>(null);
  const commands = usePendingCommands(refresh, (entity) => entity.startsWith("delivery:"));
  // 状态筛选与分页跟地址栏双向同步；对话框、对账草稿和命令上下文是瞬态，不入 URL。
  const query = useDeliveriesQueryState();
  const queryState = query.state;
  const requestedQuery = createValueMemo(() => {
    const current = queryState();
    return [current.status, deliveriesQueryFilters(current), current.page] as const;
  });
  const pendingDialog = createCell<DeliveryDialogState | null>(null);
  const reconcileDraft = createCell<DeliveryReconcileDraft | null>(null);
  // 玩家报出的投递 Reference 可直接定位详情；只接受规范 UUID，不猜测其他编号。
  const lookup = createCell("");
  const lookupInvalid = createCell(false);
  const router = useRouter();
  let alive = true;
  onCleanup(() => {
    alive = false;
  });
  const openLookup = (event: SubmitEvent): void => {
    event.preventDefault();
    const target = trimRustWhitespace(lookup());
    if (!validUuid(target)) {
      lookupInvalid.set(true);
      return;
    }
    lookupInvalid.set(false);
    router.push(`/deliveries/${target}`);
  };
  const pagination = useAdminPagination(
    "deliveries",
    ["page_size"],
    100,
    9_223_372_036_854_775_807n,
  );
  const deliveries = useAdminPage(
    pagination,
    () => {
      refresh();
      return requestedQuery();
    },
    (request, size, signal) => {
      const pageNumber = request[2];
      if (pageNumber > BigInt(Number.MAX_SAFE_INTEGER)) {
        return Promise.resolve(err("admin_pagination_bounds"));
      }
      return fetchDeliveries(host, request[0], request[1], Number(pageNumber), size, signal);
    },
  );

  const openDialog = ([delivery, kind]: readonly [Delivery, DeliveryDialogKind]): void => {
    if (!commands.anyBusy() && !commands.contains(deliveryCommandEntity(delivery.id))) {
      notice.set(null);
      successAction.set(null);
      const draft = reconcileDraft();
      if (kind === "reconcile" && (draft === null || draft.deliveryId !== delivery.id)) {
        reconcileDraft.set(deliveryReconcileDraftForDelivery(delivery));
      }
      pendingDialog.set({ delivery, kind });
    }
  };
  const updateReconcileDraft = (draft: DeliveryReconcileDraft): void => {
    if (commands.contains(deliveryCommandEntity(draft.deliveryId))) {
      return;
    }
    const current = reconcileDraft();
    if (current !== null && current.deliveryId === draft.deliveryId) {
      reconcileDraft.set(draft);
    }
  };
  const confirmAction = (request: DeliveryActionRequest): void => {
    if (commands.anyBusy() || commands.contains(deliveryCommandEntity(request.deliveryId))) {
      return;
    }
    const dialog = pendingDialog();
    if (dialog === null) {
      return;
    }
    if (
      dialog.delivery.id !== request.deliveryId ||
      dialog.delivery.fencing_version !== request.expectedFencingVersion
    ) {
      return;
    }
    if (request.action === "reconcile_retry" && dialog.kind === "reconcile") {
      // 旧版 retry 人工对账先关闭字段框，再进入共用的第二确认框；此时不能发 mutation。
      pendingDialog.set({ delivery: dialog.delivery, kind: "reconcile_retry" });
      return;
    }
    const { deliveryId, action, expectedFencingVersion, externalReference, failureCode } = request;
    // 在提交时复制并冻结命令参数；执行体与重试都不再读取对话框里的投递对象。
    const preparation = prepareDeliveryCommandOnce({
      deliveryId,
      action,
      expectedFencingVersion,
      externalReference,
      failureCode,
    });
    notice.set(null);
    successAction.set(null);
    const submitted = commands.submit(
      deliveryCommandEntity(deliveryId),
      deliveryActionLabel(action),
      async (commandId, signal) => {
        const result = await executeDelivery(host, preparation, commandId, signal);
        return result;
      },
      () => {
        setIfAlive(notice, { kind: "success" }, () => alive);
        setIfAlive(successAction, action, () => alive);
      },
      (code) => {
        // 新的版本拒绝不能抹掉更早一次未知；原 fence 与 UUID 仍由宿主保留。
        if (
          code === "stale_delivery_state" ||
          code === "stale_state_version" ||
          code === "delivery_recipient_renamed"
        ) {
          advanceCommandReload(refresh);
        }
        setIfAlive(notice, { kind: "error", code }, () => alive);
      },
    );
    if (submitted) {
      closeDeliveryModal(DIALOG_ID);
      pendingDialog.set(null);
      reconcileDraft.set(null);
    }
  };
  const view = createValueMemo((): DeliveriesView => {
    const current = deliveries();
    if (current === null) {
      return { kind: "loading" };
    }
    return current.ok
      ? { kind: "ready", page: current.value }
      : { kind: "error", code: current.error };
  });
  const readyPage = createValueMemo(() => {
    const current = view();
    return current.kind === "ready" ? current.page : null;
  });
  const loadError = createValueMemo(() => {
    const current = view();
    return current.kind === "error" ? current : null;
  });
  const pending = createValueMemo(() => pendingDialog());
  // 行内对账草稿由对话框所在行订阅；页面只在页级确认框打开时读取，表格只转交信号，
  // 因此输入外部引用时页面、表格与其他行都不重渲染。
  const rowDialog = createValueMemo(() => {
    const dialog = pending();
    return dialog !== null && (dialog.kind === "reconcile" || dialog.kind === "reconcile_retry")
      ? dialog
      : null;
  });
  const pageDialog = createValueMemo(() => {
    const dialog = pending();
    return dialog !== null && dialog.kind !== "reconcile" && dialog.kind !== "reconcile_retry"
      ? dialog
      : null;
  });
  const successMessage = createValueMemo(() => deliverySuccessMessage(successAction()));

  return (
    <PendingCommandsContext value={commands}>
      <section class="admin-delivery-page" aria-labelledby="delivery-title">
        <h1 id="delivery-title">{t("page-deliveries-title")}</h1>
        <PendingCommandPanel owner={commands} />
        <form class="admin-delivery-lookup" onSubmit={openLookup}>
          <label for="delivery-lookup-id">
            <span>{t("delivery-detail-lookup-label")}</span>
            <input
              id="delivery-lookup-id"
              type="text"
              value={lookup()}
              onInput={(event) => {
                lookup.set(event.currentTarget.value);
                lookupInvalid.set(false);
              }}
            />
          </label>
          <button type="submit">{t("delivery-detail-lookup-open")}</button>
          <Show when={lookupInvalid()}>
            <p class="action-feedback action-feedback--error" role="alert">
              {t("delivery-detail-lookup-invalid")}
            </p>
          </Show>
        </form>
        <DeliveryFilters
          status={queryState().status}
          recipient={queryState().recipient}
          reference={queryState().reference}
          template={queryState().template}
          category={queryState().category}
          world={queryState().world}
          createdFrom={queryState().createdFrom}
          createdTo={queryState().createdTo}
          onStatusChange={(status) => {
            // 选择状态后立即回到第一页并保留排序，由页面 push 到地址栏。
            const next = { ...queryState(), status, page: 1n };
            query.push(next);
          }}
          onFiltersChange={(filters: DeliveryFiltersDraft) => {
            // 提交筛选后回到第一页；空值由 query 序列化自动省略。
            const next = {
              ...queryState(),
              recipient: filters.recipient,
              reference: filters.reference,
              template: filters.template,
              category: filters.category,
              world: filters.world,
              createdFrom: filters.createdFrom,
              createdTo: filters.createdTo,
              page: 1n,
            };
            query.push(next);
          }}
        />
        <Switch fallback={<AsyncState kind="loading" title={t("data-loading")} detail="" />}>
          <Match when={readyPage()}>
            {(current) => (
              <>
                <DeliveryTable
                  page={current()}
                  sort={queryState().sort}
                  direction={queryState().direction}
                  onSortChange={([sort, direction]) => {
                    const next = { ...queryState(), sort, direction };
                    query.push(next);
                  }}
                  onOpen={openDialog}
                  pendingDialog={rowDialog()}
                  draft={reconcileDraft}
                  onDraftChange={updateReconcileDraft}
                  onConfirm={confirmAction}
                  onCancel={() => {
                    pendingDialog.set(null);
                    reconcileDraft.set(null);
                  }}
                />
              </>
            )}
          </Match>
          <Match when={loadError()}>
            {(error) => (
              <AsyncState
                kind="error"
                title={adminApiErrorMessage(error().code)}
                detail=""
                retryLabel={t("admin-retry")}
                onRetry={() => advanceCommandReload(refresh)}
              />
            )}
          </Match>
        </Switch>
        <Show when={notice()}>
          {(feedback) => (
            <p
              class={
                feedback().kind === "success"
                  ? "action-feedback action-feedback--success"
                  : "action-feedback action-feedback--error"
              }
              role={feedback().kind === "success" ? "status" : "alert"}
            >
              {actionFeedbackMessage(feedback(), successMessage())}
            </p>
          )}
        </Show>
        <Show when={readyPage()}>
          {(current) => (
            <AdminPager
              pagination={pagination}
              page={current().pagination.page}
              pageSize={current().pagination.page_size}
              total={current().pagination.total}
              label={t("delivery-pagination")}
              onPageChange={(nextPage) => {
                const next = { ...queryState(), page: BigInt(nextPage) };
                query.push(next);
              }}
            />
          )}
        </Show>
        <Show when={pageDialog()}>
          {(dialog) => (
            <DeliveryDialog
              delivery={dialog().delivery}
              kind={dialog().kind}
              busy={commands.anyBusy()}
              draft={reconcileDraft() ?? deliveryReconcileDraftForDelivery(dialog().delivery)}
              onDraftChange={() => {}}
              onConfirm={confirmAction}
              onCancel={() => {
                pendingDialog.set(null);
                reconcileDraft.set(null);
              }}
            />
          )}
        </Show>
      </section>
    </PendingCommandsContext>
  );
}

function deliveryActionLabel(action: DeliveryAction): string {
  switch (action) {
    case "release":
      return t("delivery-release");
    case "retry":
      return t("delivery-retry");
    case "reconcile_retry":
      return t("delivery-reconcile-action-retry");
    case "reconcile_succeeded":
      return t("delivery-reconcile-action-succeeded");
    case "reconcile_failed":
      return t("delivery-reconcile-action-failed");
    default:
      return unreachable(action);
  }
}

function deliverySuccessMessage(action: DeliveryAction | null): string {
  switch (action) {
    case "release":
      return t("delivery-action-released");
    case "retry":
      return t("delivery-action-retried");
    case "reconcile_retry":
    case "reconcile_succeeded":
    case "reconcile_failed":
      return t("delivery-action-reconciled");
    case null:
      return t("delivery-action-success");
    default:
      return unreachable(action);
  }
}
