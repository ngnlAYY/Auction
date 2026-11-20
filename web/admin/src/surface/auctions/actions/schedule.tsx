// Admin 正式开拍前预览、开拍与计划结束时间修改对话框。

import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { ok, type Result, unreachable } from "@auction/web-shared/result";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { For, onCleanup, Show } from "solid-js";

import {
  type AdminAuction,
  prepareAdminAuctionScheduleOnce,
  updateAdminAuctionSchedule,
} from "../../../api/auctions";
import type { AdminSessionHost } from "../../../api/session";
import { parseStrictRfc3339 } from "../../../api/session-time";
import { datetimeLocalValue } from "../../../api/validation";
import { useAdminSessionHost } from "../../auth/shell";
import { type PendingCommands, requirePendingCommands, setIfAlive } from "../../pending";
import { PendingCommandPanel } from "../../pending/panel";
import { FieldError } from "../../validation";
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
  focusDraftFieldAfterRender,
  observeDraftWrite,
} from "../editor";
import {
  currentTimeMillis,
  parseLocalScheduleTime,
  type ScheduleField,
  type ScheduleIssue,
  type ScheduleMode,
  type ScheduleTimes,
  scheduleIssueMessage,
  validateSchedule,
} from "../presentation";
import type { AdminAuctionNotice } from "./model";

const SCHEDULE_FIELDS: readonly ScheduleField[] = ["preview", "open", "end"];

/** 排期对话框的草稿、权威读取与刷新世代。 */
export interface AdminAuctionScheduleDialogProps {
  readonly draft: Cell<ScheduleDraft | null>;
  readonly latestVersion: number | null;
  readonly read: DraftRead;
  readonly notice: Cell<AdminAuctionNotice | null>;
  readonly reloadGeneration: Cell<bigint>;
}

/** 预览/草稿场次的排期对话框：校验三项时间、冻结提交正文并接入未决命令宿主。 */
export function AdminAuctionScheduleDialog(props: AdminAuctionScheduleDialogProps): JSX.Element {
  const owner = requirePendingCommands();
  const host = useAdminSessionHost();
  const feedback = createCell<DraftWriteFeedback | null>(null);
  const issue = createCell<readonly [ScheduleField, ScheduleIssue] | null>(null);
  let alive = true;
  onCleanup(() => {
    alive = false;
  });
  const feedbackAlive = () => alive;

  return (
    <Show when={props.draft()}>
      {(openDraft) => {
        const auctionId = createValueMemo(() => openDraft().auctionId);
        const sequence = createValueMemo(() => openDraft().sequence);
        const mode = createValueMemo(() => openDraft().mode);
        const entity = createValueMemo(() => `auction:${auctionId()}`);
        const blocked = createValueMemo(() => owner.contains(entity()));
        const busy = createValueMemo(() => owner.isBusy(entity()));
        const failure = createValueMemo(() => feedback());
        const conflict = createValueMemo(() => {
          if (blocked()) {
            return false;
          }
          return (
            failure()?.staleWrite === true ||
            draftReadVersionChanged(props.read, openDraft().stateVersion, props.latestVersion)
          );
        });
        const canSubmit = createValueMemo(
          () =>
            draftReadIsReady(props.read) &&
            !blocked() &&
            !conflict() &&
            props.latestVersion === openDraft().stateVersion,
        );
        const retryAfterClose = createValueMemo(() =>
          draftReadRetryAfterClose(props.read, conflict()),
        );

        const onClose = () => {
          closeEditDraft(props.draft, props.reloadGeneration, retryAfterClose());
        };
        const onSubmit = (event: SubmitEvent) => {
          event.preventDefault();
          if (!canSubmit()) {
            return;
          }
          const submitted = props.draft();
          if (submitted === null) {
            return;
          }
          const times = scheduleDraftTimes(submitted, currentTimeMillis());
          if (!times.ok) {
            const [field] = times.error;
            issue.set(times.error);
            focusDraftFieldAfterRender(scheduleFieldId(field, submitted.auctionId));
            return;
          }
          issue.set(null);
          if (
            submitSchedule(
              owner,
              host,
              submitted,
              times.value,
              props.notice,
              props.draft,
              feedback,
              feedbackAlive,
            )
          ) {
            feedback.set(null);
          }
        };

        return (
          <Dialog
            title={t("admin-auctions-schedule-title", { number: sequence() })}
            closeLabel={t("admin-auctions-action-close")}
            busy={busy()}
            onClose={onClose}
          >
            <form class="admin-auction-open" novalidate onSubmit={onSubmit}>
              <p class="admin-panel__note">{t("admin-auctions-schedule-note")}</p>
              <For each={SCHEDULE_FIELDS} keyed={false}>
                {(field) => (
                  <ScheduleInputField
                    field={field()}
                    auctionId={auctionId()}
                    disabled={blocked()}
                    readOnly={conflict()}
                    value={scheduleDraftInput(openDraft(), field()).value}
                    previewLocked={mode() === "preview"}
                    draft={props.draft}
                    issue={issue}
                  />
                )}
              </For>
              <Show when={!blocked()}>
                <DraftFeedback
                  conflict={conflict()}
                  read={props.read}
                  failure={failure()?.displayCode ?? null}
                />
              </Show>
              <div class="admin-shell__dialog-actions">
                <Show when={!conflict()}>
                  <button type="button" disabled={busy()} onClick={onClose}>
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
                  <button class="admin-btn-primary" type="submit" disabled={!canSubmit()}>
                    {busy() ? t("admin-action-working") : t("admin-auctions-schedule-submit")}
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

function ScheduleInputField(props: {
  readonly field: ScheduleField;
  readonly auctionId: string;
  readonly disabled: boolean;
  readonly readOnly: boolean;
  readonly value: string;
  readonly previewLocked: boolean;
  readonly draft: Cell<ScheduleDraft | null>;
  readonly issue: Cell<readonly [ScheduleField, ScheduleIssue] | null>;
}): JSX.Element {
  const fieldLocked = createValueMemo(() => props.field === "preview" && props.previewLocked);
  const autofocus = createValueMemo(
    () => props.field === (props.previewLocked ? "open" : "preview"),
  );
  const inputId = createValueMemo(() => scheduleFieldId(props.field, props.auctionId));
  const errorId = createValueMemo(() => `${inputId()}-error`);
  const error = createValueMemo(() => {
    const current = props.issue();
    return current !== null && current[0] === props.field ? current[1] : null;
  });
  const describedBy = createValueMemo(() => (error() === null ? undefined : errorId()));
  const label = createValueMemo(() => scheduleFieldLabel(props.field));
  const errorMessage = createValueMemo(() => {
    const current = error();
    return current === null ? null : scheduleIssueMessage(current);
  });

  return (
    <>
      <label for={inputId()}>{label()}</label>
      <input
        id={inputId()}
        name={scheduleFieldName(props.field)}
        type={
          // 只读文本保留原本地时间字符串，允许整段选择和复制。
          props.readOnly ? "text" : "datetime-local"
        }
        required
        autofocus={autofocus() && !props.readOnly}
        disabled={props.disabled || (fieldLocked() && !props.readOnly)}
        readonly={props.readOnly}
        value={props.value}
        aria-invalid={error() === null ? "false" : "true"}
        aria-describedby={describedBy()}
        onInput={(event) => {
          if (props.disabled || fieldLocked() || props.readOnly) {
            return;
          }
          const value = event.currentTarget.value;
          props.draft.update((current) =>
            current === null ? current : scheduleDraftSetInput(current, props.field, value),
          );
          const current = props.issue();
          if (current !== null && current[0] === props.field) {
            props.issue.set(null);
          }
        }}
      />
      <FieldError id={errorId()} message={errorMessage()} />
    </>
  );
}

function scheduleFieldName(field: ScheduleField): string {
  switch (field) {
    case "preview":
      return "preview_scheduled_at";
    case "open":
      return "scheduled_open_at";
    case "end":
      return "scheduled_end_at";
    default:
      return unreachable(field);
  }
}

function scheduleFieldLabel(field: ScheduleField): string {
  switch (field) {
    case "preview":
      return t("admin-auctions-schedule-preview");
    case "open":
      return t("admin-auctions-schedule-open");
    case "end":
      return t("admin-auctions-schedule-end");
    default:
      return unreachable(field);
  }
}

function scheduleFieldId(field: ScheduleField, auctionId: string): string {
  return `schedule-${scheduleFieldName(field)}-${auctionId}`;
}

/** 实体、版本和三个原始时刻来自同一快照。编辑与刷新都不能更新这个版本。 */
export interface ScheduleDraft {
  readonly auctionId: string;
  readonly sequence: number;
  readonly stateVersion: number;
  readonly mode: ScheduleMode;
  readonly preview: ScheduleInput;
  readonly open: ScheduleInput;
  readonly end: ScheduleInput;
}

/** 从草稿或预览场次创建排期草稿；其他状态不能修改排期。 */
export function scheduleDraftFromAuction(auction: AdminAuction): ScheduleDraft | null {
  let mode: ScheduleMode;
  switch (auction.status) {
    case "draft":
      mode = "draft";
      break;
    case "preview":
      mode = "preview";
      break;
    default:
      return null;
  }
  return {
    auctionId: auction.id,
    sequence: auction.sequence,
    stateVersion: auction.state_version,
    mode,
    preview: scheduleInputNew(auction.preview_scheduled_at),
    open: scheduleInputNew(auction.scheduled_open_at),
    end: scheduleInputNew(auction.scheduled_ends_at),
  };
}

/** 草稿所属拍卖的 ID，供页级重试匹配实体。 */
export function scheduleDraftAuctionId(draft: ScheduleDraft): string {
  return draft.auctionId;
}

interface ScheduleInput {
  readonly original: string | null;
  readonly value: string;
  readonly edited: boolean;
}

function scheduleInputNew(original: string | null): ScheduleInput {
  return {
    original: original !== null && parseStrictRfc3339(original) !== null ? original : null,
    value: original === null ? "" : (datetimeLocalValue(original) ?? ""),
    edited: false,
  };
}

function scheduleInputResolve(input: ScheduleInput): Result<string, ScheduleIssue> {
  // 未改动的预填值已有确定的 UTC（包括回拨小时及秒以下精度）；新输入必须唯一可逆。
  if (!input.edited && input.original !== null) {
    return ok(input.original);
  }
  return parseLocalScheduleTime(input.value);
}

function scheduleDraftInput(draft: ScheduleDraft, field: ScheduleField): ScheduleInput {
  switch (field) {
    case "preview":
      return draft.preview;
    case "open":
      return draft.open;
    case "end":
      return draft.end;
    default:
      return unreachable(field);
  }
}

function scheduleDraftSetInput(
  draft: ScheduleDraft,
  field: ScheduleField,
  value: string,
): ScheduleDraft {
  const input = scheduleDraftInput(draft, field);
  const next: ScheduleInput = {
    original: input.original,
    value,
    edited: input.edited || input.value !== value,
  };
  return scheduleDraftReplaceInput(draft, field, next);
}

function scheduleDraftReplaceInput(
  draft: ScheduleDraft,
  field: ScheduleField,
  input: ScheduleInput,
): ScheduleDraft {
  switch (field) {
    case "preview":
      return { ...draft, preview: input };
    case "open":
      return { ...draft, open: input };
    case "end":
      return { ...draft, end: input };
    default:
      return unreachable(field);
  }
}

function scheduleDraftTimes(
  draft: ScheduleDraft,
  now: number,
): Result<ScheduleTimes, readonly [ScheduleField, ScheduleIssue]> {
  return validateSchedule(
    scheduleInputResolve(draft.preview),
    scheduleInputResolve(draft.open),
    scheduleInputResolve(draft.end),
    draft.mode,
    now,
  );
}

/** 把解析后的时刻写成 `Timestamp` 的 wire 文本：UTC、`Z` 结尾，小数秒取 0/3/6/9 位。 */
function scheduleWireTime(value: string): string {
  const nanoseconds = parseStrictRfc3339(value);
  if (nanoseconds === null) {
    // 提交前的校验已保证三项都是严格 RFC 3339；这里保留原文本只防御契约被破坏。
    return value;
  }
  let seconds = nanoseconds / 1_000_000_000n;
  let nanosecond = Number(nanoseconds % 1_000_000_000n);
  if (nanosecond < 0) {
    seconds -= 1n;
    nanosecond += 1_000_000_000;
  }
  const date = new Date(Number(seconds) * 1_000);
  return (
    `${String(date.getUTCFullYear()).padStart(4, "0")}-${padTwo(date.getUTCMonth() + 1)}` +
    `-${padTwo(date.getUTCDate())}T${padTwo(date.getUTCHours())}` +
    `:${padTwo(date.getUTCMinutes())}:${padTwo(date.getUTCSeconds())}` +
    `${wireFraction(nanosecond)}Z`
  );
}

/** 小数秒按 Rust `AutoSi` 的规则取位：整秒、整毫秒、整微秒与纳秒各不相同。 */
function wireFraction(nanosecond: number): string {
  if (nanosecond === 0) {
    return "";
  }
  if (nanosecond % 1_000_000 === 0) {
    return `.${String(nanosecond / 1_000_000).padStart(3, "0")}`;
  }
  if (nanosecond % 1_000 === 0) {
    return `.${String(nanosecond / 1_000).padStart(6, "0")}`;
  }
  return `.${String(nanosecond).padStart(9, "0")}`;
}

function padTwo(value: number): string {
  return String(value).padStart(2, "0");
}

/** 页级重试保留首次提交的版本、UTC 值和 command_id；只有已确认成功才清空原草稿。 */
function submitSchedule(
  owner: PendingCommands,
  host: AdminSessionHost,
  draft: ScheduleDraft,
  times: ScheduleTimes,
  notice: Cell<AdminAuctionNotice | null>,
  open: Cell<ScheduleDraft | null>,
  feedback: Cell<DraftWriteFeedback | null>,
  feedbackAlive: () => boolean,
): boolean {
  const submittedId = draft.auctionId;
  const submittedVersion = draft.stateVersion;
  // 冻结请求正文前按 `Timestamp` 的 wire 规则规范化；重试继续复用同一份正文。
  const preview = scheduleWireTime(times.preview);
  const openAt = scheduleWireTime(times.open);
  const end = scheduleWireTime(times.end);
  const prepare = prepareAdminAuctionScheduleOnce(preview, openAt, end, submittedVersion);
  return owner.submit(
    `auction:${submittedId}`,
    t("admin-auctions-action-schedule"),
    (commandId, signal) =>
      observeDraftWrite(
        updateAdminAuctionSchedule(
          host,
          commandId,
          submittedId,
          preview,
          openAt,
          end,
          submittedVersion,
          prepare,
          signal,
        ),
        "stale_auction_state",
        feedback,
        () => feedbackAlive() && !signal.aborted,
      ),
    () => {
      notice.set({ kind: "schedule" });
      setIfAlive(feedback, null, feedbackAlive);
      // 离开本框后从未决区重试时，回执不能关闭操作员新打开的另一份草稿。
      closeMatchingDraft(
        open,
        (candidate: ScheduleDraft) =>
          candidate.auctionId === submittedId && candidate.stateVersion === submittedVersion,
      );
    },
    () => {},
  );
}
