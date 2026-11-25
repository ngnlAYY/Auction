// 确认并解除运营止售；回执须与 hold_id 和预期版本一致，否则按结果未知由会话宿主原样重放。

import type { AdminListingV2 } from "@auction/contracts/dto/admin/consignment/listing-v2";
import { createCell } from "@auction/web-shared/reactive/cell";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import { prepareAdminReleaseHold } from "../../api/consignment/quarantine";
import { adminSessionWritesAllowed } from "../../api/session";
import { useAdminSessionHost } from "../auth/shell";
import type { PendingCommands } from "../pending";
import type { DelistFeedback } from "./actions";

/** 读取只在 admin_actions 含 release_hold 且带有效止售时给出入口；写入绑定读到的 hold_id 与版本。 */
export function ReleaseHoldAction(props: {
  readonly record: AdminListingV2;
  readonly commands: PendingCommands;
  readonly currentRead: () => boolean;
  readonly onFeedback: (feedback: DelistFeedback | null) => void;
}): JSX.Element {
  const host = useAdminSessionHost();
  const entity = () => `consignment:${props.record.listing.listing_id}`;
  // 确认框绑定打开时的版本与 hold_id；读取世代变化后旧确认不得走新命令。
  const confirming = createCell<{ readonly version: number; readonly holdId: string } | null>(null);
  const preparing = createCell(false);
  const hold = () =>
    props.record.admin_actions.includes("release_hold") ? props.record.hold : null;
  const canSubmit = (): boolean =>
    props.currentRead() &&
    hold() !== null &&
    adminSessionWritesAllowed(host) &&
    !props.commands.contains(entity()) &&
    !props.commands.anyBusy() &&
    !preparing();

  const submit = async (): Promise<void> => {
    const target = confirming.peek();
    const current = hold();
    if (
      !canSubmit() ||
      target === null ||
      current === null ||
      target.version !== props.record.listing.version ||
      target.holdId !== current.hold_id
    ) {
      return;
    }
    preparing.set(true);
    const commands = props.commands;
    const feedback = props.onFeedback;
    const key = entity();
    feedback(null);
    try {
      const operation = await prepareAdminReleaseHold(
        host,
        props.record.listing.listing_id,
        target.version,
        target.holdId,
      );
      if (!props.currentRead()) return;
      const submitted = commands.submit(
        key,
        t("consignment-release-hold"),
        operation,
        (outcome) => feedback({ kind: "success", disposition: outcome.disposition }),
        (code) => {
          if (!commands.contains(key)) feedback({ kind: "error", code });
        },
      );
      if (submitted) confirming.set(null);
    } finally {
      preparing.set(false);
    }
  };

  return (
    <Show when={hold()}>
      {(current) => (
        <div class="admin-consignment-actions">
          <button
            type="button"
            class="admin-btn-primary"
            disabled={!canSubmit()}
            onClick={() => {
              if (!canSubmit()) return;
              props.onFeedback(null);
              confirming.set({ version: props.record.listing.version, holdId: current().hold_id });
            }}
          >
            {t("consignment-release-hold")}
          </button>
          <Show when={confirming() !== null}>
            <Dialog
              title={t("consignment-release-hold-confirm")}
              closeLabel={t("settlement-return-cancel")}
              busy={props.commands.anyBusy() || preparing()}
              onClose={() => confirming.set(null)}
            >
              <form
                class="admin-consignment-actions__form"
                onSubmit={(event) => {
                  event.preventDefault();
                  void submit();
                }}
              >
                <div class="admin-shell__dialog-actions">
                  <button
                    class="admin-btn-secondary"
                    type="button"
                    onClick={() => confirming.set(null)}
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
      )}
    </Show>
  );
}
