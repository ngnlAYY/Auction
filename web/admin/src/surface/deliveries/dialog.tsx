// 投递动作确认与人工对账的当前 fence 证据。

import type {
  AdminDeliveryResponse as Delivery,
  AdminDeliveryEvidenceResponse as DeliveryEvidence,
} from "@auction/contracts/dto/admin/delivery";
import {
  ADMIN_DELIVERY_RECONCILE_FAILURE_CODES,
  type AdminDeliveryActionsResponse,
  type AdminDeliveryDetailWithActionsResponse,
  adminDeliveryActionAvailabilityBlocker,
} from "@auction/contracts/dto/admin/delivery";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskResource } from "@auction/web-shared/reactive/resource";
import { unreachable } from "@auction/web-shared/result";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, Match, onSettled, Show, Switch } from "solid-js";

import { fetchDeliveryDetail } from "../../api/delivery";
import { trimRustWhitespace } from "../../api/session-time";
import { useAdminSessionHost } from "../auth/shell";
import {
  availability,
  collectedLabel,
  failureCodeLabel,
  observedLabel,
  type ReconcileOption,
  reconcileOptionBlocker,
  sameFence,
  selectedReconcileAction,
} from "./dialog-policy";
import { actionBlockerLabel, deliveryDisplayId, deliveryStatusLabel } from "./labels";
import { closeDeliveryModal, DeliveryModal } from "./modal";
import {
  type DeliveryActionRequest,
  type DeliveryDialogKind,
  type DeliveryReconcileDraft,
  deliveryDialogKindAction,
} from "./model";
import { evidenceKindLabel } from "./sent-proof/render";

/** 投递动作弹窗的 DOM id；页面在提交被受理后据此关闭原生弹窗。 */
export const DIALOG_ID = "delivery-action-dialog";

/** 投递动作对话框属性。 */
export interface DeliveryDialogProps {
  readonly delivery: Delivery;
  readonly kind: DeliveryDialogKind;
  readonly busy: boolean;
  readonly draft: DeliveryReconcileDraft;
  readonly onDraftChange: (draft: DeliveryReconcileDraft) => void;
  readonly onConfirm: (request: DeliveryActionRequest) => void;
  readonly onCancel: () => void;
}

type DeliveryDialogView =
  | { readonly kind: "empty" }
  | { readonly kind: "error" }
  | { readonly kind: "stale" }
  | { readonly kind: "reconcile"; readonly projected: AdminDeliveryDetailWithActionsResponse }
  | { readonly kind: "action"; readonly projected: AdminDeliveryDetailWithActionsResponse };

/** 对当前 fence 的证据与准入分别展示读取结果；未取得当前判定时不提交操作。 */
export function DeliveryDialog(props: DeliveryDialogProps): JSX.Element {
  const host = useAdminSessionHost();
  const title = createValueMemo((): string => {
    // 先取快照再 switch，漏掉联合成员时 unreachable 会编译失败。
    const kind = props.kind;
    switch (kind) {
      case "release":
        return t("delivery-release");
      case "retry":
      case "reconcile_retry":
        return t("delivery-retry");
      case "reconcile":
        return t("delivery-reconcile-title", { id: deliveryDisplayId(props.delivery) });
      default:
        return unreachable(kind);
    }
  });
  const deliveryId = createValueMemo(() => props.delivery.id);
  const resource = createTaskResource(deliveryId, async (id, context) => {
    const result = await fetchDeliveryDetail(host, id, context.signal);
    if (!context.alive()) {
      return null;
    }
    return result;
  });
  const loaded = createValueMemo(() =>
    resource.state() === "ready" ? (resource() ?? null) : null,
  );
  const projected = createValueMemo(() => {
    const current = loaded();
    return current?.ok ? current.value : null;
  });
  const view = createValueMemo((): DeliveryDialogView => {
    if (loaded() === null) {
      return { kind: "empty" };
    }
    const current = projected();
    if (current === null) {
      return { kind: "error" };
    }
    if (!sameFence(props.delivery, current.detail.delivery)) {
      return { kind: "stale" };
    }
    return props.kind === "reconcile"
      ? { kind: "reconcile", projected: current }
      : { kind: "action", projected: current };
  });
  const reconcileCurrent = createValueMemo(() => {
    const current = view();
    return current.kind === "reconcile" ? current.projected : null;
  });
  const actionCurrent = createValueMemo(() => {
    const current = view();
    return current.kind === "action" ? current.projected : null;
  });
  const evidenceStatus = createValueMemo((): string | null => {
    if (loaded() === null) {
      return t("delivery-reconcile-sightings-loading");
    }
    const current = projected();
    return current !== null &&
      props.kind === "reconcile" &&
      sameFence(props.delivery, current.detail.delivery) &&
      current.detail.evidence.length === 0
      ? t("delivery-reconcile-sightings-empty")
      : null;
  });
  const action = createValueMemo(() => deliveryDialogKindAction(props.kind));
  const actionBlocker = createValueMemo(() => {
    const current = projected();
    return current === null
      ? null
      : adminDeliveryActionAvailabilityBlocker(availability(current.actions, action()));
  });
  const cancelButton = useCancelButtonFocus(() => props.kind === "reconcile_retry");
  const cancel = (): void => {
    closeDeliveryModal(DIALOG_ID);
    props.onCancel();
  };
  return (
    <DeliveryModal
      dialogId={DIALOG_ID}
      title={title()}
      busy={props.busy}
      onCancel={() => props.onCancel()}
    >
      <DeliveryEvidenceStatus message={evidenceStatus()} />
      <div>
        <Switch fallback={null}>
          <Match when={view().kind === "empty"}>{null}</Match>
          <Match when={view().kind === "error"}>
            <p class="action-feedback action-feedback--error" role="alert">
              {t("delivery-reconcile-sightings-error")}
            </p>
            <button type="button" disabled={props.busy} onClick={() => resource.restart()}>
              {t("admin-retry")}
            </button>
          </Match>
          <Match when={view().kind === "stale"}>
            <p class="action-feedback action-feedback--error" role="alert">
              {t("delivery-action-blocked-stale")}
            </p>
          </Match>
          <Match when={reconcileCurrent()}>
            {(current) => (
              <>
                <ReconcileEvidenceCandidates evidence={current().detail.evidence} />
                <ReconcileForm
                  delivery={current().detail.delivery}
                  actions={current().actions}
                  busy={props.busy}
                  draft={props.draft}
                  onDraftChange={props.onDraftChange}
                  onConfirm={props.onConfirm}
                />
              </>
            )}
          </Match>
          <Match when={actionCurrent()}>
            {(current) => (
              <>
                <DeliveryConfirmationFacts delivery={current().detail.delivery} />
                <Show
                  when={actionBlocker()}
                  fallback={
                    <button
                      type="button"
                      class="admin-btn-primary"
                      disabled={props.busy}
                      onClick={() =>
                        props.onConfirm({
                          deliveryId: current().detail.delivery.id,
                          action: action(),
                          expectedFencingVersion: current().detail.delivery.fencing_version,
                          externalReference: null,
                          failureCode: null,
                        })
                      }
                    >
                      {t("delivery-confirm")}
                    </button>
                  }
                >
                  {(blocker) => <p class="admin-panel__note">{actionBlockerLabel(blocker())}</p>}
                </Show>
              </>
            )}
          </Match>
        </Switch>
      </div>
      <div class="admin-shell__dialog-actions">
        <button type="button" disabled={props.busy} ref={cancelButton.ref} onClick={cancel}>
          {t("delivery-cancel")}
        </button>
      </div>
    </DeliveryModal>
  );
}

interface CancelButtonFocus {
  readonly ref: (element: HTMLButtonElement) => void;
}

function useCancelButtonFocus(secondStep: () => boolean): CancelButtonFocus {
  const button = createCell<HTMLButtonElement | null>(null);
  const secondStepValue = createValueMemo(secondStep);
  // 第二步复用已经打开的对话框；第一步的控件卸载后，焦点回到安全动作。
  createEffect(
    () => [secondStepValue(), button()] as const,
    ([value, element]) => {
      if (!value || element === null) {
        return;
      }
      // ref 回调发生在元素进入文档之前；等 DOM 提交后只为仍在文档中的按钮恢复焦点。
      onSettled(() => {
        if (element.isConnected) {
          element.focus();
        }
      });
    },
  );
  return {
    ref: (element) => {
      button.set(element);
    },
  };
}

/** 证据播报区属性。 */
export interface DeliveryEvidenceStatusProps {
  readonly message: string | null;
}

/** 先挂载播报区，再更新证据状态，首次打开也能播报正在读取。 */
export function DeliveryEvidenceStatus(props: DeliveryEvidenceStatusProps): JSX.Element {
  const announced = createCell<string | null>(null);
  const message = createValueMemo(() => props.message);
  createEffect(message, (value) => announced.set(value));
  return (
    <div role="status" aria-atomic="true">
      <Show when={announced() !== null}>
        <p class="admin-panel__note">{announced()}</p>
      </Show>
    </div>
  );
}

interface ReconcileFormProps {
  readonly delivery: Delivery;
  readonly actions: AdminDeliveryActionsResponse;
  readonly busy: boolean;
  readonly draft: DeliveryReconcileDraft;
  readonly onDraftChange: (draft: DeliveryReconcileDraft) => void;
  readonly onConfirm: (request: DeliveryActionRequest) => void;
}

function ReconcileForm(props: ReconcileFormProps): JSX.Element {
  const options = createValueMemo((): readonly ReconcileOption[] => [
    {
      value: "retry",
      label: t("delivery-reconcile-action-retry"),
      decision: props.actions.reconcile_retry,
    },
    {
      value: "succeeded",
      label: t("delivery-reconcile-action-succeeded"),
      decision: props.actions.reconcile_succeeded,
    },
    {
      value: "failed",
      label: t("delivery-reconcile-action-failed"),
      decision: props.actions.reconcile_failed,
    },
  ]);
  const allBlocked = createValueMemo(() =>
    options().every((option) => adminDeliveryActionAvailabilityBlocker(option.decision) !== null),
  );
  const selection = createValueMemo(() => props.draft.action);
  const selected = createValueMemo(() => selectedReconcileAction(selection(), props.actions));
  const submit = (event: SubmitEvent): void => {
    event.preventDefault();
    const action = selected();
    if (action === null || props.busy) {
      return;
    }
    const external = trimRustWhitespace(props.draft.externalReference);
    const failure = props.draft.failureCode;
    if (
      (action === "reconcile_succeeded" && external === "") ||
      (action === "reconcile_failed" &&
        !ADMIN_DELIVERY_RECONCILE_FAILURE_CODES.some((code) => code === failure))
    ) {
      return;
    }
    props.onConfirm({
      deliveryId: props.delivery.id,
      action,
      expectedFencingVersion: props.delivery.fencing_version,
      externalReference: action === "reconcile_succeeded" ? external : null,
      failureCode: action === "reconcile_failed" ? failure : null,
    });
  };
  return (
    <Show
      when={allBlocked()}
      fallback={
        <form class="admin-delivery-reconcile" onSubmit={submit}>
          <label for="delivery-reconcile-action">{t("delivery-reconcile-action")}</label>
          <select
            id="delivery-reconcile-action"
            required
            value={selection()}
            onChange={(event) =>
              props.onDraftChange({
                ...props.draft,
                action: event.currentTarget.value,
              })
            }
          >
            <option value="" disabled selected={selected() === null}>
              {t("delivery-reconcile-action-placeholder")}
            </option>
            <For each={options()} keyed={false}>
              {(option) => (
                <Show when={adminDeliveryActionAvailabilityBlocker(option().decision) === null}>
                  <option value={option().value} selected={selection() === option().value}>
                    {option().label}
                  </option>
                </Show>
              )}
            </For>
          </select>
          <For each={options()} keyed={false}>
            {(option) => (
              <Show when={reconcileOptionBlocker(option())}>
                {(blocker) => (
                  <p class="admin-panel__note">
                    {`${option().label} · ${actionBlockerLabel(blocker())}`}
                  </p>
                )}
              </Show>
            )}
          </For>
          <Show when={selected() === "reconcile_succeeded"}>
            <label for="delivery-external-reference">{t("delivery-external-reference")}</label>
            <input
              id="delivery-external-reference"
              type="text"
              required
              value={props.draft.externalReference}
              onInput={(event) =>
                props.onDraftChange({
                  ...props.draft,
                  externalReference: event.currentTarget.value,
                })
              }
            />
          </Show>
          <Show when={selected() === "reconcile_failed"}>
            <label for="delivery-failure-code">{t("delivery-failure-code")}</label>
            <select
              id="delivery-failure-code"
              required
              value={props.draft.failureCode}
              onChange={(event) =>
                props.onDraftChange({
                  ...props.draft,
                  failureCode: event.currentTarget.value,
                })
              }
            >
              <option value="" disabled selected={props.draft.failureCode === ""}>
                {t("delivery-failure-code-placeholder")}
              </option>
              <For each={ADMIN_DELIVERY_RECONCILE_FAILURE_CODES} keyed={(code) => code}>
                {(code) => (
                  <option value={code()} selected={props.draft.failureCode === code()}>
                    {failureCodeLabel(code())}
                  </option>
                )}
              </For>
            </select>
          </Show>
          <button type="submit" class="admin-btn-primary" disabled={props.busy}>
            {selected() === "reconcile_retry" ? t("delivery-retry") : t("delivery-reconcile")}
          </button>
        </form>
      }
    >
      <For each={options()} keyed={false}>
        {(option) => (
          <Show when={reconcileOptionBlocker(option())}>
            {(blocker) => (
              <p class="admin-panel__note">
                {`${option().label} · ${actionBlockerLabel(blocker())}`}
              </p>
            )}
          </Show>
        )}
      </For>
    </Show>
  );
}

/** 重新排队确认前的投递身份与状态。 */
export interface DeliveryConfirmationFactsProps {
  readonly delivery: Delivery;
}

/** 进入重新排队确认前展示本次操作绑定的真实投递身份和状态。 */
export function DeliveryConfirmationFacts(props: DeliveryConfirmationFactsProps): JSX.Element {
  return (
    <>
      <dl class="admin-delivery-detail__grid">
        <dt>{t("admin-mail-delivery-id")}</dt>
        <dd>
          <code>{props.delivery.id}</code>
        </dd>
        <dt>{t("delivery-to")}</dt>
        <dd>{props.delivery.recipient}</dd>
        <dt>{t("delivery-status")}</dt>
        <dd>{deliveryStatusLabel(props.delivery.status)}</dd>
        <dt>{t("delivery-template")}</dt>
        <dd>
          <Show when={props.delivery.template !== null} fallback={t("value-empty")}>
            {props.delivery.template}
          </Show>
        </dd>
        <Show when={props.delivery.failure_code !== null}>
          <dt>{t("delivery-failure-code")}</dt>
          <dd>{props.delivery.failure_code}</dd>
        </Show>
        <Show when={props.delivery.external_reference !== null}>
          <dt>{t("delivery-external-reference")}</dt>
          <dd>{props.delivery.external_reference}</dd>
        </Show>
      </dl>
      <p class="admin-panel__note">{t("delivery-requeue-consequence")}</p>
    </>
  );
}

/** 对账证据候选属性。 */
export interface ReconcileEvidenceCandidatesProps {
  readonly evidence: readonly DeliveryEvidence[];
}

/** 当前 fence 上可作为人工对账依据的 Sent 观察。 */
export function ReconcileEvidenceCandidates(props: ReconcileEvidenceCandidatesProps): JSX.Element {
  return (
    <section class="admin-delivery-reconcile__evidence">
      <h3>{t("delivery-reconcile-sightings")}</h3>
      <p class="admin-panel__note">{t("delivery-reconcile-sightings-help")}</p>
      <Show when={props.evidence.length !== 0}>
        <ul class="admin-delivery-reconcile__sightings">
          <For each={props.evidence} keyed={(item) => item.reference}>
            {(item) => (
              <li>
                <span class="admin-delivery-reconcile__kind">{evidenceKindLabel(item().kind)}</span>
                <code>{item().external_mail_id ?? t("value-empty")}</code>
                <span>{observedLabel(item().observed_at)}</span>
                <span>{collectedLabel(item().collected)}</span>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </section>
  );
}
