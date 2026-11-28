// 确认并下架挂单；v2 对代管件改设运营止售，未决命令由认证外壳持有，可用原正文恢复。

import type { AdminListingV2 } from "@auction/contracts/dto/admin/consignment/listing-v2";
import { createCell } from "@auction/web-shared/reactive/cell";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import { delistReasonError } from "../../api/consignment/delist";
import { prepareAdminHold } from "../../api/consignment/quarantine";
import { adminSessionWritesAllowed } from "../../api/session";
import { useAdminSessionHost } from "../auth/shell";
import type { PendingCommands } from "../pending";
import { consignmentErrorMessage } from "./presentation";

/** 下架或解除止售成功的服务端处置；详情页据此选择成功文案。 */
export type DelistFeedback =
  | { readonly kind: "success"; readonly disposition: "closed" | "hold_set" | "hold_released" }
  | { readonly kind: "error"; readonly code: string };

/** 读取只提供下架资格，写入仍须确认版本和私有说明。 */
export function DelistAction(props: {
  readonly record: AdminListingV2;
  readonly commands: PendingCommands;
  readonly currentRead: () => boolean;
  readonly onFeedback: (feedback: DelistFeedback | null) => void;
}): JSX.Element {
  const host = useAdminSessionHost();
  const entity = () => `consignment:${props.record.listing.listing_id}`;
  const expectedVersion = createCell<number | null>(null);
  const preparing = createCell(false);
  const reason = createCell("");
  const error = createCell<"required" | "invalid" | null>(null);
  const cancel = (): void => {
    expectedVersion.set(null);
    reason.set("");
    error.set(null);
  };
  // 未决时只能原样重试。运营二次确认放弃后可提交新命令，由服务端按版本仲裁；
  // 同版本至多一次下架生效，另一条命令收到 stale_version 或 listing_not_active。
  // Admin 无寄售回执查询入口，整页重载会丢失内存未决记录；此时重读详情，再按当前版本提交。
  const canSubmit = (): boolean =>
    props.currentRead() &&
    props.record.admin_actions.includes("delist") &&
    adminSessionWritesAllowed(host) &&
    !props.commands.contains(entity()) &&
    !props.commands.anyBusy() &&
    !preparing();

  const submit = async (event: SubmitEvent): Promise<void> => {
    event.preventDefault();
    const version = expectedVersion.peek();
    const listingId = props.record.listing.listing_id;
    if (!canSubmit() || version === null || version !== props.record.listing.version) return;
    const explanation = reason.peek();
    const problem = delistReasonError(explanation);
    error.set(problem);
    if (problem !== null) return;

    preparing.set(true);
    const target = entity();
    const commands = props.commands;
    const feedback = props.onFeedback;
    props.onFeedback(null);
    try {
      const operation = await prepareAdminHold(host, listingId, version, explanation);
      // 准备期间操作员可能已经取消确认；取消后不得再提交这条已冻结的意图。
      if (!props.currentRead() || expectedVersion.peek() !== version) return;
      // 不变量 1、6、7：确认绑定原版本，提交后私有说明只保留在会话宿主固定的命令里。
      const submitted = commands.submit(
        target,
        t("consignment-delist"),
        operation,
        (outcome) => {
          feedback({ kind: "success", disposition: outcome.receipt.disposition });
        },
        (code) => {
          if (!commands.contains(target)) feedback({ kind: "error", code });
        },
      );
      if (submitted) cancel();
    } finally {
      preparing.set(false);
    }
  };

  return (
    <Show when={props.record.admin_actions.includes("delist")}>
      <div class="admin-consignment-actions">
        <button
          type="button"
          class="admin-btn-danger"
          disabled={!canSubmit()}
          onClick={() => {
            if (!canSubmit()) return;
            props.onFeedback(null);
            expectedVersion.set(props.record.listing.version);
          }}
        >
          {t("consignment-delist")}
        </button>
        <Show when={expectedVersion() !== null}>
          <Dialog
            title={t("consignment-delist-confirm")}
            closeLabel={t("settlement-return-cancel")}
            busy={props.commands.anyBusy() || preparing()}
            onClose={cancel}
          >
            <form class="admin-consignment-actions__form" novalidate onSubmit={submit}>
              <label for="consignment-delist-reason">{t("consignment-delist-reason")}</label>
              <textarea
                id="consignment-delist-reason"
                required
                autocomplete="off"
                spellcheck={false}
                rows={2}
                value={reason()}
                aria-invalid={error() === null ? "false" : "true"}
                aria-describedby={error() === null ? undefined : "consignment-reason-error"}
                onInput={(event) => {
                  reason.set(event.currentTarget.value);
                  error.set(null);
                }}
              />
              <Show when={error()}>
                {(problem) => (
                  <p
                    id="consignment-reason-error"
                    class="action-feedback action-feedback--error"
                    role="alert"
                  >
                    {problem() === "required"
                      ? t("consignment-reason-required")
                      : consignmentErrorMessage("invalid_reason")}
                  </p>
                )}
              </Show>
              <div class="admin-shell__dialog-actions">
                <button
                  class="admin-btn-secondary"
                  type="button"
                  disabled={props.commands.anyBusy() || preparing()}
                  onClick={cancel}
                >
                  {t("settlement-return-cancel")}
                </button>
                <button class="admin-btn-primary" type="submit" disabled={!canSubmit()}>
                  {t("settlement-return-confirm")}
                </button>
              </div>
            </form>
          </Dialog>
        </Show>
      </div>
    </Show>
  );
}
