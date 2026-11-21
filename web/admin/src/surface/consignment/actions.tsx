// 确认并下架挂单；未决命令由认证外壳持有，可用原正文恢复。

import type { AdminListing } from "@auction/contracts/dto/admin/consignment";
import { createCell } from "@auction/web-shared/reactive/cell";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import { delistReasonError, prepareAdminDelist } from "../../api/consignment/delist";
import { adminSessionWritesAllowed } from "../../api/session";
import { useAdminSessionHost } from "../auth/shell";
import type { PendingCommands } from "../pending";
import type { AdminActionFeedback } from "../presentation";
import { listingDispositionKey } from "./labels";
import { consignmentErrorMessage } from "./presentation";

/** 读取只提供下架资格，写入仍须确认版本和私有说明。 */
export function DelistAction(props: {
  readonly record: AdminListing;
  readonly commands: PendingCommands;
  readonly currentRead: () => boolean;
  readonly onFeedback: (feedback: AdminActionFeedback | null) => void;
}): JSX.Element {
  const host = useAdminSessionHost();
  const entity = () => `consignment:${props.record.listing.listing_id}`;
  const expectedVersion = createCell<number | null>(null);
  const reason = createCell("");
  const error = createCell<"required" | "invalid" | null>(null);
  const cancel = (): void => {
    expectedVersion.set(null);
    reason.set("");
    error.set(null);
  };
  const canSubmit = (): boolean =>
    props.currentRead() &&
    props.record.can_delist &&
    adminSessionWritesAllowed(host) &&
    !props.commands.contains(entity()) &&
    !props.commands.anyBusy();

  const submit = (event: SubmitEvent): void => {
    event.preventDefault();
    const version = expectedVersion.peek();
    const listingId = props.record.listing.listing_id;
    if (!canSubmit() || version === null || version !== props.record.listing.version) return;
    const explanation = reason.peek();
    const problem = delistReasonError(explanation);
    error.set(problem);
    if (problem !== null) return;

    // 不变量 1、6、7：确认绑定原版本，提交后私有说明只保留在会话宿主固定的命令里。
    const operation = prepareAdminDelist(host, listingId, version, explanation);
    const target = entity();
    const commands = props.commands;
    const feedback = props.onFeedback;
    const submitted = commands.submit(
      target,
      t("consignment-delist"),
      operation,
      (receipt) => {
        if (listingDispositionKey(receipt.disposition) !== null) feedback({ kind: "success" });
      },
      (code) => {
        if (!commands.contains(target)) feedback({ kind: "error", code });
      },
    );
    if (submitted) cancel();
  };

  return (
    <Show when={props.record.can_delist}>
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
            busy={props.commands.anyBusy()}
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
                <button class="admin-btn-secondary" type="button" onClick={cancel}>
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
