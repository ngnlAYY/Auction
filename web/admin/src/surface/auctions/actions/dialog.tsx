// Admin 拍卖生命周期动作按钮与确认对话框。

import { useSecondTick } from "@auction/web-shared/platform/clock";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { Link } from "@auction/web-shared/router";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, For, onCleanup, Show } from "solid-js";

import type { AdminAuction, AdminAuctionCapabilities } from "../../../api/auctions";
import {
  type AdminAuctionLifecycleAction,
  executeAdminAuctionLifecycle,
  prepareAdminAuctionLifecycleOnce,
} from "../../../api/lifecycle";
import type { AdminSessionHost } from "../../../api/session";
import { useAdminSessionHost } from "../../auth/shell";
import { type PendingCommands, requirePendingCommands, setIfAlive } from "../../pending";
import { PendingCommandPanel } from "../../pending/panel";
import { adminWorldLabel } from "../../presentation";
import { FieldError, useFocusField } from "../../validation";
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
} from "../editor";
import { currentTimeMillis } from "../presentation";
import { type AuctionDrafts, requireAuctionDrafts } from ".";
import {
  type AdminAuctionNotice,
  type LifecycleReasonIssue,
  lifecycleActionButtonClass,
  lifecycleActionConfirmNote,
  lifecycleActionConfirmTitle,
  lifecycleActionLabel,
  lifecycleActionSubmitLabel,
  lifecycleActions,
  lifecycleReasonIssueMessage,
  pendingOpenMillis,
  validateLifecycleReason,
} from "./model";

/** 打开确认时冻结的场次身份与版本；后续列表刷新不改写本次意图。 */
export interface LifecycleTarget {
  readonly auctionId: string;
  readonly worldId: string;
  readonly sequence: number;
  readonly stateVersion: number;
}

export function lifecycleTargetFromAuction(auction: AdminAuction): LifecycleTarget {
  return {
    auctionId: auction.id,
    worldId: auction.world_id,
    sequence: auction.sequence,
    stateVersion: auction.state_version,
  };
}

export interface LifecycleDraft {
  readonly target: LifecycleTarget;
  readonly action: AdminAuctionLifecycleAction;
  readonly reason: string;
}

export function lifecycleDraftNew(
  target: LifecycleTarget,
  action: AdminAuctionLifecycleAction,
): LifecycleDraft {
  return { target: { ...target }, action, reason: "" };
}

export function lifecycleDraftAuctionId(draft: LifecycleDraft): string {
  return draft.target.auctionId;
}

export interface AdminAuctionActionsProps {
  readonly auction: AdminAuction;
  readonly capabilities: AdminAuctionCapabilities;
  readonly notice: Cell<AdminAuctionNotice | null>;
  readonly editsReady: boolean;
}

export function AdminAuctionActions(props: AdminAuctionActionsProps): JSX.Element {
  // 只有预览阶段等待开拍的入口订阅秒级时钟。
  const auction = createValueMemo(() => props.auction);
  const capabilities = createValueMemo(() => props.capabilities);
  const owner = requirePendingCommands();
  const entity = createValueMemo(() => `auction:${auction().id}`);
  const blocked = createValueMemo(() => !props.editsReady || owner.contains(entity()));
  const now = createValueMemo(() => {
    auction();
    capabilities();
    // 编辑就绪后，直接订阅未决集合中不会改变 `blocked` 布尔值的变化；未就绪时等秒级时钟。
    if (props.editsReady) {
      owner.contains(entity());
    }
    return currentTimeMillis();
  });
  const drafts = requireAuctionDrafts();
  const target = createValueMemo(() => lifecycleTargetFromAuction(auction()));
  const actions = createValueMemo(() =>
    lifecycleActions(auction().status, capabilities(), auction().scheduled_open_at, now()),
  );
  const waitingUntil = createValueMemo(() =>
    pendingOpenMillis(
      auction().status,
      capabilities().manual_early_open,
      auction().scheduled_open_at,
      now(),
    ),
  );
  const canEditSchedule = createValueMemo(
    () =>
      capabilities().modify_schedule &&
      (auction().status === "draft" || auction().status === "preview"),
  );

  return (
    <div class="admin-auctions__actions">
      <Link class="admin-auctions__open-link" href={`/auctions/${auction().id}/lots`}>
        {t("admin-auctions-open")}
      </Link>
      <Show when={waitingUntil()}>
        {(scheduledOpenMillis) => (
          <AdminAuctionScheduledOpenButton
            scheduledOpenMillis={scheduledOpenMillis()}
            blocked={blocked()}
            notice={props.notice}
            target={target()}
            drafts={drafts}
          />
        )}
      </Show>
      <For each={actions()} keyed={false}>
        {(action) => (
          <AdminAuctionActionButton
            action={action()}
            blocked={blocked()}
            notice={props.notice}
            target={target()}
            drafts={drafts}
          />
        )}
      </For>
      <Show when={canEditSchedule()}>
        <button
          class="admin-btn-secondary"
          type="button"
          disabled={blocked()}
          onClick={() => {
            if (!blocked() && !owner.contains(entity()) && drafts.openSchedule(auction())) {
              props.notice.set(null);
            }
          }}
        >
          {t("admin-auctions-action-schedule")}
        </button>
      </Show>
    </div>
  );
}

/** 等待开拍时间的「开拍」入口，仍排在其他生命周期动作之前。 */
function AdminAuctionScheduledOpenButton(props: {
  readonly scheduledOpenMillis: number;
  readonly blocked: boolean;
  readonly notice: Cell<AdminAuctionNotice | null>;
  readonly target: LifecycleTarget;
  readonly drafts: AuctionDrafts;
}): JSX.Element {
  const secondTick = useSecondTick();
  const arrived = createValueMemo(() => {
    secondTick();
    return currentTimeMillis() >= props.scheduledOpenMillis;
  });
  return (
    <Show when={arrived()}>
      <AdminAuctionActionButton
        action="open"
        blocked={props.blocked}
        notice={props.notice}
        target={props.target}
        drafts={props.drafts}
      />
    </Show>
  );
}

function AdminAuctionActionButton(props: {
  readonly action: AdminAuctionLifecycleAction;
  readonly blocked: boolean;
  readonly notice: Cell<AdminAuctionNotice | null>;
  readonly target: LifecycleTarget;
  readonly drafts: AuctionDrafts;
}): JSX.Element {
  return (
    <button
      class={lifecycleActionButtonClass(props.action)}
      type="button"
      disabled={props.blocked}
      onClick={() => {
        if (!props.blocked && props.drafts.openLifecycle({ ...props.target }, props.action)) {
          props.notice.set(null);
        }
      }}
    >
      {lifecycleActionLabel(props.action)}
    </button>
  );
}

export interface AdminAuctionActionDialogProps {
  readonly latestVersion: number | null;
  readonly notice: Cell<AdminAuctionNotice | null>;
  readonly pending: Cell<LifecycleDraft | null>;
  readonly reloadGeneration: Cell<bigint>;
  readonly read: DraftRead;
}

export function AdminAuctionActionDialog(props: AdminAuctionActionDialogProps): JSX.Element {
  const owner = requirePendingCommands();
  const host = useAdminSessionHost();
  const focusField = useFocusField();
  const issue = createCell<LifecycleReasonIssue | null>(null);
  const feedback = createCell<DraftWriteFeedback | null>(null);
  // 错误与写入反馈属于具体草稿；切换到另一条草稿时不能沿用上一条的错误或冲突状态。
  const draftIdentity = createValueMemo(() => {
    const current = props.pending();
    return current === null
      ? null
      : `${current.action}|${current.target.auctionId}|${current.target.stateVersion}`;
  });
  createEffect(draftIdentity, () => {
    issue.set(null);
    feedback.set(null);
  });
  let alive = true;
  onCleanup(() => {
    alive = false;
  });
  const failureAlive = () => alive;

  return (
    <Show when={props.pending()}>
      {(draft) => {
        const action = createValueMemo(() => draft().action);
        const isCancel = createValueMemo(() => action() === "cancel");
        const requiresReason = createValueMemo(() => isCancel() || action() === "early-settlement");
        const target = createValueMemo(() => draft().target);
        const auctionId = createValueMemo(() => target().auctionId);
        const entity = createValueMemo(() => `auction:${auctionId()}`);
        const busy = createValueMemo(() => owner.isBusy(entity()));
        const blocked = createValueMemo(() => owner.contains(entity()));
        const failure = createValueMemo(() => feedback());
        const conflict = createValueMemo(() => {
          if (blocked()) {
            return false;
          }
          return (
            failure()?.staleWrite === true ||
            draftReadVersionChanged(props.read, target().stateVersion, props.latestVersion)
          );
        });
        const canSubmit = createValueMemo(
          () =>
            draftReadIsReady(props.read) &&
            !blocked() &&
            !conflict() &&
            props.latestVersion === target().stateVersion,
        );
        const retryAfterClose = createValueMemo(() =>
          draftReadRetryAfterClose(props.read, conflict()),
        );
        const reasonId = createValueMemo(() => `action-reason-${auctionId()}`);
        const hintId = createValueMemo(() => `${reasonId()}-hint`);
        const errorId = createValueMemo(() => `${reasonId()}-error`);
        const describedBy = createValueMemo(() =>
          issue() !== null ? `${hintId()} ${errorId()}` : hintId(),
        );
        const issueMessage = createValueMemo(() => {
          const current = issue();
          return current === null ? null : lifecycleReasonIssueMessage(current);
        });
        const world = createValueMemo(() => adminWorldLabel(target().worldId));
        const sequence = createValueMemo(() => target().sequence);

        const onClose = () => {
          closeEditDraft(props.pending, props.reloadGeneration, retryAfterClose());
        };
        const onSubmit = (event: SubmitEvent) => {
          event.preventDefault();
          if (!canSubmit()) {
            return;
          }
          const current = props.pending.peek();
          if (current === null) {
            return;
          }
          let reason: string | null = null;
          if (requiresReason()) {
            const validated = validateLifecycleReason(current.reason);
            if (!validated.ok) {
              issue.set(validated.error);
              focusField(reasonId());
              return;
            }
            reason = validated.value;
          }
          issue.set(null);
          if (
            runLifecycleAction(
              owner,
              host,
              current,
              reason,
              feedback,
              props.notice,
              props.pending,
              failureAlive,
            )
          ) {
            feedback.set(null);
          }
        };
        const onReasonInput = (
          event: InputEvent & { readonly currentTarget: HTMLTextAreaElement },
        ) => {
          if (blocked() || conflict()) {
            return;
          }
          props.pending.update((current) =>
            current === null ? null : { ...current, reason: event.currentTarget.value },
          );
          issue.set(null);
        };

        return (
          <Dialog
            title={lifecycleActionConfirmTitle(action(), sequence(), world())}
            closeLabel={t("admin-auctions-action-close")}
            busy={busy()}
            onClose={onClose}
          >
            <form class="admin-auction-open" novalidate onSubmit={onSubmit}>
              <p>
                {t("admin-auctions-confirm-target", {
                  world: world(),
                  number: sequence(),
                })}
              </p>
              <p>{lifecycleActionConfirmNote(action())}</p>
              <Show when={requiresReason()}>
                <label for={reasonId()}>
                  {isCancel()
                    ? t("admin-auctions-cancel-reason")
                    : t("admin-auctions-early-settlement-reason")}
                </label>
                <textarea
                  id={reasonId()}
                  name="reason"
                  required
                  autofocus={!conflict()}
                  disabled={blocked()}
                  readonly={conflict()}
                  value={draft().reason}
                  aria-invalid={issue() !== null ? "true" : "false"}
                  aria-describedby={describedBy()}
                  onInput={onReasonInput}
                />
                <p id={hintId()}>
                  {isCancel()
                    ? t("admin-auctions-cancel-reason-hint")
                    : t("admin-auctions-early-settlement-reason-hint")}
                </p>
                <FieldError id={errorId()} message={issueMessage()} />
              </Show>
              <Show when={!blocked()}>
                <DraftFeedback
                  conflict={conflict()}
                  read={props.read}
                  failure={failure()?.displayCode ?? null}
                />
              </Show>
              <div class="admin-shell__dialog-actions">
                <Show when={!conflict()}>
                  <button
                    type="button"
                    disabled={busy()}
                    autofocus={!requiresReason()}
                    onClick={onClose}
                  >
                    {t("admin-auctions-action-close")}
                  </button>
                </Show>
                <Show when={!blocked()}>
                  <DraftRefreshButton
                    read={props.read}
                    conflict={conflict()}
                    reload={props.reloadGeneration}
                    onClose={onClose}
                  />
                </Show>
                <Show when={!conflict() && (!blocked() || busy())}>
                  <button
                    class={lifecycleActionButtonClass(action())}
                    type="submit"
                    disabled={!canSubmit()}
                  >
                    {busy() ? t("admin-action-working") : lifecycleActionSubmitLabel(action())}
                  </button>
                </Show>
              </div>
            </form>
            <Show when={blocked() && !busy()}>
              <PendingCommandPanel owner={owner} />
            </Show>
          </Dialog>
        );
      }}
    </Show>
  );
}

/** 页级请求以操作员打开确认时看到的版本提交，未知结果只重试同一完整意图。 */
function runLifecycleAction(
  owner: PendingCommands,
  host: AdminSessionHost,
  draft: LifecycleDraft,
  reason: string | null,
  feedback: Cell<DraftWriteFeedback | null>,
  notice: Cell<AdminAuctionNotice | null>,
  pending: Cell<LifecycleDraft | null>,
  alive: () => boolean,
): boolean {
  const action = draft.action;
  const submittedTarget: LifecycleTarget = { ...draft.target };
  const auctionId = draft.target.auctionId;
  const stateVersion = draft.target.stateVersion;
  const prepare = prepareAdminAuctionLifecycleOnce(stateVersion, reason);
  return owner.submit(
    `auction:${auctionId}`,
    lifecycleActionLabel(action),
    (commandId, signal) =>
      observeDraftWrite(
        executeAdminAuctionLifecycle(host, commandId, auctionId, action, prepare, signal),
        "stale_auction_state",
        feedback,
        () => alive() && !signal.aborted,
      ),
    () => {
      setIfAlive(feedback, null, alive);
      // 未决区回执只关闭同一个版本的确认，不能影响另一个场次的草稿。
      closeMatchingDraft(
        pending,
        (candidate: LifecycleDraft) =>
          lifecycleTargetsEqual(candidate.target, submittedTarget) && candidate.action === action,
      );
      const noticeValue: AdminAuctionNotice = { kind: "lifecycle", action };
      notice.set(noticeValue);
    },
    () => {},
  );
}

function lifecycleTargetsEqual(left: LifecycleTarget, right: LifecycleTarget): boolean {
  return (
    left.auctionId === right.auctionId &&
    left.worldId === right.worldId &&
    left.sequence === right.sequence &&
    left.stateVersion === right.stateVersion
  );
}
