// Admin 拍卖创建表单。

import { createUuidV4 } from "@auction/web-shared/platform/uuid";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { unreachable } from "@auction/web-shared/result";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { onCleanup, Show } from "solid-js";

import { createAdminAuction, prepareAdminAuctionCreateOnce } from "../../api/auctions";
import { MAX_JSON_SAFE_INTEGER } from "../../api/validation";
import { useAdminSessionHost } from "../auth/shell";
import { requirePendingCommands, setIfAlive } from "../pending";
import {
  type AdminActionFeedback,
  adminActionFeedbackMessage,
  adminWorldLabel,
} from "../presentation";
import { FieldError, useFocusField } from "../validation";
import type { AdminAuctionNotice } from "./actions/model";
import {
  currentTimeMillis,
  parseLocalScheduleTime,
  type ScheduleField,
  type ScheduleIssue,
  type ScheduleTimes,
  scheduleIssueMessage,
  validateSchedule,
} from "./presentation";

/** 首次提交时冻结标识与规范时间；未知结果重试不重新取时间或生成身份。 */
interface CreateRequest {
  readonly auctionId: string;
  readonly worldId: string;
  readonly sequence: number;
  readonly times: ScheduleTimes;
}

/** 创建表单各字段的校验结果；null 表示该字段当前无错误。 */
interface CreateFieldErrors {
  readonly world: CreateFieldIssue | null;
  readonly sequence: CreateFieldIssue | null;
  readonly schedule: readonly [ScheduleField, ScheduleIssue] | null;
}

const EMPTY_FIELD_ERRORS: CreateFieldErrors = {
  world: null,
  sequence: null,
  schedule: null,
};

function fieldErrorsAny(errors: CreateFieldErrors): boolean {
  return errors.world !== null || errors.sequence !== null || errors.schedule !== null;
}

/** 表单序上第一个错误字段的控件 id，用于提交失败后聚焦。 */
function firstInvalidId(errors: CreateFieldErrors): string | null {
  if (errors.world !== null) {
    return "create-world";
  }
  if (errors.sequence !== null) {
    return "create-sequence";
  }
  const schedule = errors.schedule;
  if (schedule === null) {
    return null;
  }
  return CREATE_SCHEDULE_FIELD_IDS[schedule[0]] ?? null;
}

const CREATE_SCHEDULE_FIELD_IDS: Record<ScheduleField, string> = {
  preview: "create-preview",
  open: "create-open",
  end: "create-end",
};

function scheduleIssueOf(errors: CreateFieldErrors, field: ScheduleField): ScheduleIssue | null {
  const schedule = errors.schedule;
  return schedule !== null && schedule[0] === field ? schedule[1] : null;
}

/** 字段错误的分类；渲染时翻译，保证切换语言后已展示的错误同步更新。 */
type CreateFieldIssue =
  | "required"
  | "invalid_sequence"
  // 服务端返回世界不存在。
  | "world_not_found";

function createFieldIssueMessage(issue: CreateFieldIssue): string {
  switch (issue) {
    case "required":
      return t("admin-field-required");
    case "invalid_sequence":
      return t("admin-auctions-create-field-sequence-range", {
        max: MAX_JSON_SAFE_INTEGER,
      });
    case "world_not_found":
      return t("admin-error-world-not-found");
    default:
      return unreachable(issue);
  }
}

function parseSequence(value: string): number | null {
  // 与 Admin 创建命令使用的公开 JSON 安全整数上限一致。
  const text = rustTrim(value);
  if (!/^[+-]?[0-9]+$/u.test(text)) {
    return null;
  }
  const parsed = Number(text);
  return Number.isSafeInteger(parsed) && parsed >= 1 && parsed <= MAX_JSON_SAFE_INTEGER
    ? parsed
    : null;
}

/** Rust `str::trim` 的空白集合；JS 的 `trim` 与之不同，不能替换。 */
function rustTrim(value: string): string {
  let start = 0;
  let end = value.length;
  while (start < end && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

function isRustWhitespace(code: number): boolean {
  return (
    code === 0x09 ||
    code === 0x0a ||
    code === 0x0b ||
    code === 0x0c ||
    code === 0x0d ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}

/** 场次创建入口：负责创建按钮与弹窗开关，表单内的成功提示仍由页面持有。 */
export function AdminAuctionCreateForm(props: {
  readonly notice: Cell<AdminAuctionNotice | null>;
}): JSX.Element {
  const open = createCell(false);
  const pending = requirePendingCommands();

  return (
    <>
      <button
        class="admin-btn-primary admin-auction-create-opener"
        type="button"
        disabled={pending.contains("create-auction")}
        onClick={() => {
          if (!open() && !pending.contains("create-auction")) {
            // 打开新流程时清掉上一次的页面级成功提示。
            props.notice.set(null);
            open.set(true);
          }
        }}
      >
        {t("admin-auctions-create-opener")}
      </button>
      <Show when={open()}>
        {/* 外壳（遮罩/标题栏/关闭守卫）由共享 Dialog 接管；aria-busy 挂在表单上。 */}
        <Dialog
          title={t("admin-auctions-create-title")}
          closeLabel={t("admin-auctions-create-close")}
          busy={pending.isBusy("create-auction")}
          onClose={() => open.set(false)}
        >
          <AdminAuctionCreateFields open={open} notice={props.notice} />
        </Dialog>
      </Show>
    </>
  );
}

function AdminAuctionCreateFields(props: {
  readonly open: Cell<boolean>;
  readonly notice: Cell<AdminAuctionNotice | null>;
}): JSX.Element {
  const worldId = createCell("");
  const sequence = createCell("1");
  const previewScheduledAt = createCell("");
  const scheduledOpenAt = createCell("");
  const scheduledEndAt = createCell("");
  const pending = requirePendingCommands();
  const host = useAdminSessionHost();
  const focusField = useFocusField();
  const busy = () => pending.isBusy("create-auction");
  const frozen = () => pending.contains("create-auction");
  const feedback = createCell<AdminActionFeedback | null>(null);
  const fieldErrors = createCell<CreateFieldErrors>(EMPTY_FIELD_ERRORS);
  let alive = true;
  onCleanup(() => {
    alive = false;
  });

  const worldIssue = createValueMemo(() => fieldErrors().world);
  const sequenceIssue = createValueMemo(() => fieldErrors().sequence);
  const previewIssue = createValueMemo(() => scheduleIssueOf(fieldErrors(), "preview"));
  const openIssue = createValueMemo(() => scheduleIssueOf(fieldErrors(), "open"));
  const endIssue = createValueMemo(() => scheduleIssueOf(fieldErrors(), "end"));
  const worldMessage = createValueMemo(() => {
    const issue = worldIssue();
    return issue === null ? null : createFieldIssueMessage(issue);
  });
  const sequenceMessage = createValueMemo(() => {
    const issue = sequenceIssue();
    return issue === null ? null : createFieldIssueMessage(issue);
  });
  const previewMessage = createValueMemo(() => {
    const issue = previewIssue();
    return issue === null ? null : scheduleIssueMessage(issue);
  });
  const openMessage = createValueMemo(() => {
    const issue = openIssue();
    return issue === null ? null : scheduleIssueMessage(issue);
  });
  const endMessage = createValueMemo(() => {
    const issue = endIssue();
    return issue === null ? null : scheduleIssueMessage(issue);
  });

  const clearScheduleIssue = (field: ScheduleField) => {
    const current = fieldErrors.peek();
    if (scheduleIssueOf(current, field) !== null) {
      fieldErrors.set({ ...current, schedule: null });
    }
  };

  const onSubmit = (event: SubmitEvent) => {
    event.preventDefault();
    if (frozen()) {
      return;
    }
    const worldIdValue = worldId();
    let errors: CreateFieldErrors = EMPTY_FIELD_ERRORS;
    if (rustTrim(worldIdValue).length === 0) {
      errors = { ...errors, world: "required" };
    }
    const sequenceValue = parseSequence(sequence());
    if (sequenceValue === null) {
      errors = { ...errors, sequence: "invalid_sequence" };
    }
    const times = validateSchedule(
      parseLocalScheduleTime(previewScheduledAt()),
      parseLocalScheduleTime(scheduledOpenAt()),
      parseLocalScheduleTime(scheduledEndAt()),
      "create",
      currentTimeMillis(),
    );
    if (!times.ok) {
      errors = { ...errors, schedule: times.error };
    }
    if (fieldErrorsAny(errors)) {
      fieldErrors.set(errors);
      const fieldId = firstInvalidId(errors);
      if (fieldId !== null) {
        focusField(fieldId);
      }
      return;
    }
    if (sequenceValue === null || !times.ok) {
      return;
    }
    fieldErrors.set(EMPTY_FIELD_ERRORS);
    feedback.set(null);
    const request: CreateRequest = {
      auctionId: createUuidV4(),
      worldId: worldIdValue,
      sequence: sequenceValue,
      times: {
        preview: times.value.preview,
        open: times.value.open,
        end: times.value.end,
      },
    };
    const prepare = prepareAdminAuctionCreateOnce(
      request.auctionId,
      request.worldId,
      request.sequence,
      request.times.preview,
      request.times.open,
      request.times.end,
    );
    const title =
      `${t("admin-auctions-create-submit")} · ${adminWorldLabel(request.worldId)}` +
      ` · ${request.sequence}`;
    pending.submit(
      "create-auction",
      title,
      (commandId, signal) =>
        createAdminAuction(host, commandId, request.auctionId, prepare, signal),
      () => {
        // 页面持有成功提示；关闭弹窗不取消请求，也不丢弃回执。
        props.notice.set({ kind: "created" });
        props.open.set(false);
      },
      (code) => {
        if (!alive) {
          return;
        }
        if (code === "world_not_found") {
          fieldErrors.update((current) => ({
            ...current,
            world: "world_not_found",
          }));
        }
        setIfAlive(feedback, { kind: "error", code }, () => alive);
      },
    );
  };

  return (
    <>
      <form
        class="admin-auction-create"
        novalidate
        aria-busy={busy() ? "true" : "false"}
        onSubmit={onSubmit}
      >
        <p class="admin-panel__note">{t("admin-auctions-create-timezone")}</p>
        <label for="create-world">{t("admin-auctions-create-world")}</label>
        <select
          id="create-world"
          name="world"
          autocomplete="off"
          disabled={frozen()}
          autofocus={true}
          required
          aria-invalid={worldIssue() !== null ? "true" : "false"}
          aria-describedby={worldIssue() !== null ? "create-world-error" : undefined}
          value={worldId()}
          onChange={(event) => {
            if (frozen()) {
              return;
            }
            worldId.set(event.currentTarget.value);
            fieldErrors.update((current) => ({ ...current, world: null }));
            feedback.set(null);
          }}
        >
          <option value="" disabled>
            {t("value-empty")}
          </option>
          <option value="main">{adminWorldLabel("main")}</option>
          <option value="isekai">{adminWorldLabel("isekai")}</option>
        </select>
        <FieldError id="create-world-error" message={worldMessage()} />
        <label for="create-sequence">{t("admin-auctions-create-sequence")}</label>
        <input
          id="create-sequence"
          name="sequence"
          autocomplete="off"
          inputmode="numeric"
          type="number"
          min="1"
          max={String(MAX_JSON_SAFE_INTEGER)}
          required
          disabled={frozen()}
          aria-invalid={sequenceIssue() !== null ? "true" : "false"}
          aria-describedby={sequenceIssue() !== null ? "create-sequence-error" : undefined}
          value={sequence()}
          onInput={(event) => {
            if (frozen()) {
              return;
            }
            sequence.set(event.currentTarget.value);
            fieldErrors.update((current) => ({ ...current, sequence: null }));
            feedback.set(null);
          }}
        />
        <FieldError id="create-sequence-error" message={sequenceMessage()} />
        <label for="create-preview">{t("admin-auctions-create-preview")}</label>
        <input
          id="create-preview"
          name="preview_scheduled_at"
          autocomplete="off"
          type="datetime-local"
          required
          disabled={frozen()}
          aria-invalid={previewIssue() !== null ? "true" : "false"}
          aria-describedby={previewIssue() !== null ? "create-preview-error" : undefined}
          value={previewScheduledAt()}
          onInput={(event) => {
            if (frozen()) {
              return;
            }
            previewScheduledAt.set(event.currentTarget.value);
            clearScheduleIssue("preview");
            feedback.set(null);
          }}
        />
        <FieldError id="create-preview-error" message={previewMessage()} />
        <label for="create-open">{t("admin-auctions-create-open")}</label>
        <input
          id="create-open"
          name="scheduled_open_at"
          autocomplete="off"
          type="datetime-local"
          required
          disabled={frozen()}
          aria-invalid={openIssue() !== null ? "true" : "false"}
          aria-describedby={openIssue() !== null ? "create-open-error" : undefined}
          value={scheduledOpenAt()}
          onInput={(event) => {
            if (frozen()) {
              return;
            }
            scheduledOpenAt.set(event.currentTarget.value);
            clearScheduleIssue("open");
            feedback.set(null);
          }}
        />
        <FieldError id="create-open-error" message={openMessage()} />
        <label for="create-end">{t("admin-auctions-create-end")}</label>
        <input
          id="create-end"
          name="scheduled_end_at"
          autocomplete="off"
          type="datetime-local"
          required
          disabled={frozen()}
          aria-invalid={endIssue() !== null ? "true" : "false"}
          aria-describedby={endIssue() !== null ? "create-end-error" : undefined}
          value={scheduledEndAt()}
          onInput={(event) => {
            if (frozen()) {
              return;
            }
            scheduledEndAt.set(event.currentTarget.value);
            clearScheduleIssue("end");
            feedback.set(null);
          }}
        />
        <FieldError id="create-end-error" message={endMessage()} />
        <button type="submit" disabled={frozen()}>
          {busy() ? t("admin-action-working") : t("admin-auctions-create-submit")}
        </button>
        <Show when={feedback()}>
          {(current) => (
            <p
              class={
                current().kind === "success"
                  ? "admin-panel__note action-feedback action-feedback--success"
                  : "admin-panel__note action-feedback action-feedback--error"
              }
              role={current().kind === "success" ? "status" : "alert"}
            >
              {current().kind === "success"
                ? t("admin-auctions-create-success")
                : adminActionFeedbackMessage(current(), t("admin-auctions-create-success"))}
            </p>
          )}
        </Show>
      </form>
      <div class="admin-shell__dialog-actions">
        <button type="button" disabled={busy()} onClick={() => props.open.set(false)}>
          {t("admin-auctions-create-close")}
        </button>
      </div>
    </>
  );
}
