// Admin 订单处置面板：排入复核，或按闭集动作处置（退款类与权利保留经 6B 写端，未付取消经 7A-2 的封闭写端，补寄只排队新轮）。
// 本面板没有金额、收件人或装备输入；退款金额只由后台按冻结成交额一次转换。未知写只能原样重试。

import type { OrderResolveAction } from "@auction/contracts/dto/admin/consignment/order-resolve";
import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, Show, untrack } from "solid-js";
import { is } from "valibot";

import {
  prepareAdminOrderRecheck,
  prepareAdminOrderResolve,
  type ResolveReceipt,
} from "../../api/consignment/order-resolve";
import { adminSessionWritesAllowed } from "../../api/session";
import { useAdminSessionHost } from "../auth/shell";
import { PendingCommandPanel } from "../pending/panel";
import { advanceCommandReload, usePendingCommands } from "../pending/sources";
import { RESOLVE_ACTION_KEYS } from "./order-labels";
import { consignmentErrorMessage } from "./presentation";

/** 表单提供的动作：wire 闭集的全部六项，顺序是 D9 的声明顺序。 */
export const RESOLVE_FORM_ACTIONS = [
  "cancel_unpaid",
  "refund_returned",
  "refund_lost",
  "refund_season",
  "retain_entitlement",
  "redispatch_original",
] as const satisfies readonly OrderResolveAction[];

type Feedback =
  | { readonly kind: "error"; readonly code: string }
  | { readonly kind: "resolved"; readonly receipt: ResolveReceipt }
  | { readonly kind: "rechecked"; readonly workId: string };

const ERROR_KEYS: Readonly<Record<string, string>> = {
  stale_version: "consignment-resolve-stale",
  opposite_payment_open: "consignment-resolve-opposite-payment",
  evidence_unproven: "consignment-resolve-evidence-unproven",
  evidence_conflict: "consignment-resolve-evidence-conflict",
  action_not_allowed: "consignment-resolve-action-not-allowed",
  disposition_conflict: "consignment-resolve-disposition-conflict",
};

function resolveErrorMessage(code: string): string {
  const key = ERROR_KEYS[code];
  return key === undefined ? consignmentErrorMessage(code) : t(key);
}

function isFormAction(value: string): value is (typeof RESOLVE_FORM_ACTIONS)[number] {
  return (RESOLVE_FORM_ACTIONS as readonly string[]).includes(value);
}

/** 订单处置面板；`refresh` 是父级订单读取的世代，写成功或版本落后后推进它让父级重读。 */
export function OrderResolvePanel(props: {
  readonly contractId: string;
  readonly version: number;
  readonly sale: boolean;
  readonly refresh: Cell<bigint>;
}): JSX.Element {
  const host = useAdminSessionHost();
  const contractId = untrack(() => props.contractId);
  const entity = `order:${contractId}`;
  const commands = usePendingCommands(props.refresh, (candidate) => candidate === entity);
  const feedback = createCell<Feedback | null>(null);
  const preparing = createCell(false);
  const action = createCell<(typeof RESOLVE_FORM_ACTIONS)[number]>("refund_returned");
  const evidenceId = createCell("");
  const invalidEvidence = createCell(false);
  const canSubmit = (): boolean =>
    adminSessionWritesAllowed(host) &&
    !commands.contains(entity) &&
    !commands.anyBusy() &&
    !preparing();
  const settled = (): void => {
    // 版本落后由会话宿主了结命令后推进读取世代；这里只在非未决时显示错误。
    advanceCommandReload(props.refresh);
  };

  const recheck = (): void => {
    if (!canSubmit()) return;
    feedback.set(null);
    commands.submit(
      entity,
      t("consignment-resolve-recheck"),
      prepareAdminOrderRecheck(host, contractId, props.version),
      (receipt) => {
        feedback.set({ kind: "rechecked", workId: receipt.work_id });
        settled();
      },
      (code) => {
        if (!commands.contains(entity)) feedback.set({ kind: "error", code });
      },
    );
  };

  const submit = async (event: SubmitEvent): Promise<void> => {
    event.preventDefault();
    if (!canSubmit()) return;
    // 不变量 1、6：先校验证据 ID；提交后正文只保留在会话宿主固定的命令里。
    const evidence = evidenceId.peek().trim();
    if (!is(ConsignmentUuidSchema, evidence)) {
      invalidEvidence.set(true);
      return;
    }
    invalidEvidence.set(false);
    preparing.set(true);
    feedback.set(null);
    try {
      const submitted = commands.submit(
        entity,
        t(RESOLVE_ACTION_KEYS[action.peek()]),
        prepareAdminOrderResolve(host, contractId, {
          expectedVersion: props.version,
          action: action.peek(),
          evidenceId: evidence,
        }),
        (receipt) => {
          feedback.set({ kind: "resolved", receipt });
          settled();
        },
        (code) => {
          if (!commands.contains(entity)) feedback.set({ kind: "error", code });
        },
      );
      if (submitted) evidenceId.set("");
    } finally {
      preparing.set(false);
    }
  };

  return (
    <section class="admin-consignment-actions" aria-labelledby="admin-resolve-title">
      <h2 id="admin-resolve-title">{t("consignment-resolve-title")}</h2>
      <p>{t("consignment-resolve-no-money")}</p>
      <Show when={feedback()}>{(notice) => <FeedbackPanel feedback={notice()} />}</Show>
      <PendingCommandPanel owner={commands} />
      <button type="button" disabled={!canSubmit()} onClick={recheck}>
        {t("consignment-resolve-recheck")}
      </button>
      <Show when={props.sale}>
        <form class="admin-consignment-actions__form" novalidate onSubmit={submit}>
          <label for="resolve-action">{t("consignment-resolve-action-field")}</label>
          <select
            id="resolve-action"
            value={action()}
            onChange={(event) => {
              const value = event.currentTarget.value;
              if (isFormAction(value)) action.set(value);
            }}
          >
            <For each={RESOLVE_FORM_ACTIONS}>
              {(item) => <option value={item}>{t(RESOLVE_ACTION_KEYS[item])}</option>}
            </For>
          </select>
          <label for="resolve-evidence">{t("consignment-resolve-evidence-field")}</label>
          <input
            id="resolve-evidence"
            type="text"
            autocomplete="off"
            spellcheck={false}
            value={evidenceId()}
            aria-invalid={invalidEvidence() ? "true" : "false"}
            onInput={(event) => {
              evidenceId.set(event.currentTarget.value);
              invalidEvidence.set(false);
            }}
          />
          <Show when={invalidEvidence()}>
            <p class="action-feedback action-feedback--error" role="alert">
              {t("consignment-resolve-evidence-invalid")}
            </p>
          </Show>
          <button class="admin-btn-primary" type="submit" disabled={!canSubmit()}>
            {t("consignment-resolve-submit")}
          </button>
        </form>
      </Show>
    </section>
  );
}

/** 反馈在 JSX 表达式（跟踪作用域）里读取，随 feedback 更新；不在 Match 条件里直接读响应式值。 */
function FeedbackPanel(props: { readonly feedback: Feedback }): JSX.Element {
  return <>{renderFeedback(props.feedback)}</>;
}

function renderFeedback(feedback: Feedback): JSX.Element {
  switch (feedback.kind) {
    case "error":
      return (
        <p class="action-feedback action-feedback--error" role="alert">
          {resolveErrorMessage(feedback.code)}
        </p>
      );
    case "resolved": {
      const { receipt } = feedback;
      return (
        <p class="action-feedback" role="status">
          {receipt.status === 200
            ? t("consignment-resolve-recorded", {
                version: String(receipt.receipt.contract_version),
              })
            : t("consignment-resolve-queued", { id: receipt.receipt.dispatch_id })}
        </p>
      );
    }
    case "rechecked":
      return (
        <p class="action-feedback" role="status">
          {t("consignment-resolve-rechecked", { id: feedback.workId })}
        </p>
      );
  }
}
