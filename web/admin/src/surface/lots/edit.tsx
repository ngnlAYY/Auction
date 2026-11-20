// 页面持有的拍品改价与撤回草稿、版本冲突及提交。

import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { onCleanup, Show } from "solid-js";

import {
  type AdminLot,
  type AdminLotUpdateAction,
  prepareAdminLotUpdateOnce,
  updateAdminLot,
} from "../../api/lots";
import type { AdminSessionHost } from "../../api/session";
import {
  closeEditDraft,
  closeMatchingDraft,
  DraftFeedback,
  type DraftRead,
  DraftRefreshButton,
  type DraftWriteFeedback,
  draftReadIsReady,
  draftReadRetryAfterClose,
  draftReadVersionChanged,
  observeDraftWrite,
} from "../auctions/editor";
import { type PendingCommands, requirePendingCommands, setIfAlive } from "../pending";
import { PendingCommandPanel } from "../pending/panel";
import { FieldError, useFocusField } from "../validation";
import { adminLotCodeFor, MAX_START_PRICE, parseStartPrice } from "./presentation";
import type { LotDialog, LotUpdateNotice } from "./state";

/** 打开编辑对话框时冻结的 lot 版本、编码与起拍价；行消失或读取失败都不改写它。 */
export interface LotSnapshot {
  readonly lotId: string;
  readonly code: string;
  readonly stateVersion: number;
  readonly startPrice: number;
  readonly seller: string | null;
}

/** 从当前 lot 安全投影生成编辑快照；所有字段按值复制。 */
export function lotSnapshotFromLot(lot: AdminLot): LotSnapshot {
  return {
    lotId: lot.id,
    code: adminLotCodeFor(lot.equipment_category, lot.lot_number),
    stateVersion: lot.state_version,
    startPrice: lot.start_price,
    seller: lot.seller,
  };
}

/** 编辑值和版本来自同一打开操作；行消失或读取失败都不销毁它们。 */
export interface LotEdit {
  readonly kind: LotDialog;
  readonly base: LotSnapshot;
  readonly price: string;
}

/** 草稿对应的 lot ID；版本与编辑值来自同一次打开操作。 */
export function lotEditLotId(edit: LotEdit): string {
  return edit.base.lotId;
}

function lotEditNew(kind: LotDialog, base: LotSnapshot): LotEdit {
  // 调用方仍持有原快照引用；按值复制后才能与草稿里的价格文本保持同一打开操作。
  return { kind, base: { ...base }, price: String(base.startPrice) };
}

/** 没有未决草稿时打开编辑；已有草稿时保持原意图并返回 `false`。 */
export function openLotEdit(
  draft: Cell<LotEdit | null>,
  kind: LotDialog,
  base: LotSnapshot,
): boolean {
  if (draft.peek() !== null) {
    return false;
  }
  draft.set(lotEditNew(kind, base));
  return true;
}

/** 拍品编辑对话框的页面句柄、派生读取状态与回执位置。 */
export interface AdminLotEditDialogProps {
  readonly draft: Cell<LotEdit | null>;
  readonly latestVersion: number | null;
  readonly read: DraftRead;
  readonly reloadGeneration: Cell<bigint>;
  readonly notice: Cell<LotUpdateNotice | null>;
  readonly host: AdminSessionHost;
  readonly pageAlive: () => boolean;
}

/** 页面拥有的改价或撤回对话框；提交后由未决命令宿主保留同一命令与完整负载。 */
export function AdminLotEditDialog(props: AdminLotEditDialogProps): JSX.Element {
  const owner = requirePendingCommands();
  const writeFeedback = createCell<DraftWriteFeedback | null>(null);
  const invalidPrice = createCell(false);
  const focusField = useFocusField();
  let alive = true;
  onCleanup(() => {
    alive = false;
  });
  const failureAlive = () => alive;

  return (
    <Show when={props.draft()}>
      {(edit) => {
        const entity = createValueMemo(() => `lot:${lotEditLotId(edit())}`);
        const frozen = createValueMemo(() => owner.contains(entity()));
        const busy = createValueMemo(() => owner.isBusy(entity()));
        const conflict = createValueMemo(
          () =>
            !frozen() &&
            (writeFeedback()?.staleWrite === true ||
              draftReadVersionChanged(props.read, edit().base.stateVersion, props.latestVersion)),
        );
        const canSubmit = createValueMemo(
          () =>
            draftReadIsReady(props.read) &&
            !frozen() &&
            !conflict() &&
            props.latestVersion === edit().base.stateVersion,
        );
        const retryAfterClose = createValueMemo(() =>
          draftReadRetryAfterClose(props.read, conflict()),
        );
        const reprice = createValueMemo(() => edit().kind === "reprice");
        const code = createValueMemo(() => edit().base.code);
        const priceInputId = createValueMemo(() => `price-${lotEditLotId(edit())}`);
        const priceErrorId = createValueMemo(() => `${priceInputId()}-error`);
        const describedBy = createValueMemo(() => (invalidPrice() ? priceErrorId() : undefined));
        const seller = createValueMemo(() => edit().base.seller ?? t("value-empty"));

        const onClose = () => {
          closeEditDraft(props.draft, props.reloadGeneration, retryAfterClose());
        };
        const onSubmit = (event: SubmitEvent) => {
          event.preventDefault();
          if (!canSubmit()) {
            return;
          }
          const current = props.draft.peek();
          if (current === null) {
            return;
          }
          const price = reprice() ? parseStartPrice(current.price) : null;
          if (reprice() && price === null) {
            invalidPrice.set(true);
            focusField(priceInputId());
            return;
          }
          if (
            submitLotUpdate(
              owner,
              current,
              price,
              props.draft,
              writeFeedback,
              props.notice,
              props.host,
              failureAlive,
              props.pageAlive,
            )
          ) {
            writeFeedback.set(null);
          }
        };
        const onInput = (event: InputEvent & { readonly currentTarget: HTMLInputElement }) => {
          if (frozen() || conflict()) {
            return;
          }
          props.draft.update((current) =>
            current === null ? null : { ...current, price: event.currentTarget.value },
          );
          invalidPrice.set(false);
        };

        return (
          <Dialog
            title={
              reprice()
                ? t("admin-lots-reprice-title", { code: code() })
                : t("admin-lots-withdraw-title", { code: code() })
            }
            closeLabel={t("admin-lots-action-cancel")}
            busy={busy()}
            onClose={onClose}
          >
            <form class={{ "admin-auction-reprice": reprice() }} novalidate onSubmit={onSubmit}>
              <Show
                when={reprice()}
                fallback={<p>{t("admin-lots-withdraw-warning", { seller: seller() })}</p>}
              >
                <label for={priceInputId()}>{t("admin-lots-start-price-label")}</label>
                <input
                  id={priceInputId()}
                  name="start_price"
                  type={conflict() ? "text" : "number"}
                  min="1"
                  max={String(MAX_START_PRICE)}
                  step="1"
                  required
                  disabled={frozen()}
                  readonly={conflict()}
                  autofocus={!conflict()}
                  value={edit().price}
                  aria-invalid={invalidPrice() ? "true" : "false"}
                  aria-describedby={describedBy()}
                  onInput={onInput}
                />
                <FieldError
                  id={priceErrorId()}
                  message={
                    invalidPrice() ? t("admin-lots-invalid-price", { max: MAX_START_PRICE }) : null
                  }
                />
              </Show>
              <Show when={!frozen()}>
                <DraftFeedback
                  conflict={conflict()}
                  read={props.read}
                  failure={writeFeedback()?.displayCode ?? null}
                />
              </Show>
              <div class="admin-shell__dialog-actions">
                <Show when={!conflict()}>
                  <button type="button" disabled={busy()} onClick={() => onClose()}>
                    {t("admin-lots-action-cancel")}
                  </button>
                </Show>
                <Show when={!frozen()}>
                  <DraftRefreshButton
                    read={props.read}
                    conflict={conflict()}
                    reload={props.reloadGeneration}
                    onClose={onClose}
                  />
                </Show>
                <Show when={!conflict() && (!frozen() || busy())}>
                  <button
                    class={{
                      "admin-btn-primary": reprice(),
                      "admin-btn-danger": !reprice(),
                    }}
                    type="submit"
                    disabled={!canSubmit()}
                  >
                    {busy()
                      ? t("admin-action-working")
                      : reprice()
                        ? t("admin-lots-reprice-save")
                        : t("admin-lots-withdraw-confirm")}
                  </button>
                </Show>
              </div>
            </form>
            <Show when={frozen() && !busy()}>
              <PendingCommandPanel owner={owner} />
            </Show>
          </Dialog>
        );
      }}
    </Show>
  );
}

/** 版本来自打开时的快照；未知结果重试同一完整负载，旧回执只关闭对应的草稿。 */
function submitLotUpdate(
  owner: PendingCommands,
  edit: LotEdit,
  price: number | null,
  draft: Cell<LotEdit | null>,
  failure: Cell<DraftWriteFeedback | null>,
  notice: Cell<LotUpdateNotice | null>,
  host: AdminSessionHost,
  failureAlive: () => boolean,
  noticeAlive: () => boolean,
): boolean {
  let action: AdminLotUpdateAction;
  let success: LotUpdateNotice;
  let label: string;
  if (edit.kind === "reprice") {
    action = "set_start_price";
    success = "repriced";
    label = t("admin-lots-reprice");
  } else {
    action = "withdraw";
    success = "withdrawn";
    label = t("admin-lots-withdraw");
  }
  const lotId = edit.base.lotId;
  const stateVersion = edit.base.stateVersion;
  const base: LotSnapshot = { ...edit.base };
  const kind = edit.kind;
  const prepare = prepareAdminLotUpdateOnce(stateVersion, action, price);
  const submitted = owner.submit(
    `lot:${lotId}`,
    label,
    (commandId, signal) =>
      observeDraftWrite(
        updateAdminLot(host, lotId, commandId, prepare, signal),
        "stale_lot_state",
        failure,
        () => failureAlive() && !signal.aborted,
      ),
    () => {
      closeMatchingDraft(
        draft,
        (candidate: LotEdit) => lotSnapshotsEqual(candidate.base, base) && candidate.kind === kind,
      );
      setIfAlive(notice, success, noticeAlive);
    },
    () => {},
  );
  if (submitted) {
    setIfAlive(notice, null, noticeAlive);
  }
  return submitted;
}

function lotSnapshotsEqual(left: LotSnapshot, right: LotSnapshot): boolean {
  return (
    left.lotId === right.lotId &&
    left.code === right.code &&
    left.stateVersion === right.stateVersion &&
    left.startPrice === right.startPrice &&
    left.seller === right.seller
  );
}
