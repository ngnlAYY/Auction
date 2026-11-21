// 告警备注的字节校验、提交焦点与错误播报。

import { sleep } from "@auction/web-shared/platform/timer";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { createTaskScope, type TaskHandle } from "@auction/web-shared/reactive/tasks";
import { utf8ByteLength } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { createEffect, onSettled, Show } from "solid-js";

import { trimRustWhitespace } from "../../api/session-time";

const REASON_INPUT_ID = "alert-console-reason";
/** 与服务端确认/静默命令的备注长度上限一致。 */
const MAX_ALERT_REASON_BYTES = 2_048;

/** 按去除首尾空白后实际提交的备注校验 UTF-8 字节数。 */
export function validAlertReasonLength(value: string): boolean {
  return utf8ByteLength(trimRustWhitespace(value)) <= MAX_ALERT_REASON_BYTES;
}

type ReasonError = "clear" | "unannounced" | "announced";

function reasonErrorVisible(error: ReasonError): boolean {
  return error !== "clear";
}

function reasonErrorCorrectedBy(error: ReasonError, value: string): boolean {
  return reasonErrorVisible(error) && validAlertReasonLength(value);
}

// 对应 Rust `AlertReasonDraft` 的私有 `error` 字段：外部只能经本模块读取与修改。
const ALERT_REASON_ERROR: unique symbol = Symbol("alertReasonError");

/** 校验与播报记录跟随页级草稿，切换筛选不会重新播报同一个错误。 */
export interface AlertReasonDraft {
  readonly value: Cell<string>;
  readonly [ALERT_REASON_ERROR]: Cell<ReasonError>;

  /** 失焦或提交时标记尚未播报的备注错误；已有可见错误时保持不变。 */
  markInvalid(): void;
}

/** 创建随页面存活的告警备注草稿。 */
export function useAlertReasonDraft(): AlertReasonDraft {
  const value = createCell("");
  const error = createCell<ReasonError>("clear");
  return {
    value,
    [ALERT_REASON_ERROR]: error,
    markInvalid() {
      if (error.peek() === "clear") {
        error.set("unannounced");
      }
    },
  };
}

/** 共享备注输入。提交的校验结果由告警台设置，字段负责在渲染错误后移动焦点或播报。 */
export function AlertReasonInput(props: {
  draft: AlertReasonDraft;
  focusRequested: Cell<boolean>;
}): JSX.Element {
  const input = createCell<HTMLInputElement | null>(null);
  // 草稿可能在组件复用时被替换，错误单元始终从当前 props 取得。
  const draftError = (): Cell<ReasonError> => props.draft[ALERT_REASON_ERROR];
  // live region 在挂载时为空；播报任务随字段卸载取消，只在送出消息时记录已播报。
  const live = createCell(false);
  const announcementTask = createCell<TaskHandle | null>(null);
  const tasks = createTaskScope();
  const invalidNow = createValueMemo(() => reasonErrorVisible(draftError()()));
  const message = createValueMemo(() =>
    t("alert-console-reason-too-long", { maximum: String(MAX_ALERT_REASON_BYTES) }),
  );

  const announceError = (): void => {
    clearAnnouncement(announcementTask, live);
    const task = tasks.spawn(async (context) => {
      try {
        // 让读屏先播报失焦后到达的控件，显式提交的兜底也先清空再写入。
        if (!(await sleep(500, context.signal)) || !context.alive()) {
          return;
        }
        const error = draftError();
        if (reasonErrorVisible(error.peek())) {
          live.set(true);
          error.set("announced");
        }
      } finally {
        if (announcementTask.peek() === task) {
          announcementTask.set(null);
        }
      }
    });
    announcementTask.set(task);
  };

  createEffect(
    () => props.focusRequested(),
    (requested) => {
      if (!requested) {
        return;
      }
      props.focusRequested.set(false);
      clearAnnouncement(announcementTask, live);
      // Rust 在错误视图更新后的 effect 中排队聚焦任务；这里等渲染稳定后再读取输入框引用。
      onSettled(() => {
        const mounted = input.peek();
        tasks.spawn(async (context) => {
          if (!context.alive()) {
            return;
          }
          if (!reasonErrorVisible(draftError().peek())) {
            return;
          }
          const moved = await moveReasonFocus(mounted, reasonHasFocus);
          if (!context.alive()) {
            return;
          }
          const error = draftError();
          if (!reasonErrorVisible(error.peek())) {
            return;
          }
          if (moved) {
            // 聚焦成功由字段描述播报，之后的失焦共用同一条已播报记录。
            error.set("announced");
          } else {
            announceError();
          }
        });
      });
    },
  );

  return (
    <>
      <p id="alert-console-reason-limit" class="admin-panel__note">
        {t("alert-console-reason-limit", { maximum: String(MAX_ALERT_REASON_BYTES) })}
      </p>
      <div class="admin-alert-console__reason">
        <label for={REASON_INPUT_ID}>{t("alert-console-reason")}</label>
        <input
          id={REASON_INPUT_ID}
          type="text"
          value={props.draft.value()}
          aria-invalid={invalidNow() ? "true" : "false"}
          aria-describedby={
            invalidNow()
              ? "alert-console-reason-limit alert-console-reason-error"
              : "alert-console-reason-limit"
          }
          ref={(element) => input.set(element)}
          onInput={(event) => {
            const value = event.currentTarget.value;
            const error = draftError();
            if (reasonErrorCorrectedBy(error.peek(), value)) {
              error.set("clear");
              clearAnnouncement(announcementTask, live);
            }
            props.draft.value.set(value);
          }}
          onBlur={() => {
            if (!validAlertReasonLength(props.draft.value.peek())) {
              props.draft.markInvalid();
              const error = draftError();
              // 等待期间也只排一次，连续失焦不会推迟第一次播报。
              if (error.peek() === "unannounced" && announcementTask.peek() === null) {
                announceError();
              }
            }
          }}
        />
      </div>
      <Show when={invalidNow()}>
        <p id="alert-console-reason-error" class="admin-field-error">
          {message()}
        </p>
      </Show>
      <p
        style="position:absolute; inline-size:1px; block-size:1px; overflow:hidden; clip-path:inset(50%); white-space:nowrap"
        aria-live="polite"
        aria-atomic="true"
      >
        <Show when={live()}>{message()}</Show>
      </p>
    </>
  );
}

function clearAnnouncement(announcementTask: Cell<TaskHandle | null>, live: Cell<boolean>): void {
  const previous = announcementTask.peek();
  announcementTask.set(null);
  if (previous !== null) {
    previous.cancel();
  }
  if (live.peek()) {
    live.set(false);
  }
}

/** 成功迁移焦点时由字段描述播报。焦点已在字段上、控件未挂载或迁移失败时走 live region。 */
async function moveReasonFocus(
  input: HTMLInputElement | null,
  hasFocus: () => boolean,
): Promise<boolean> {
  if (hasFocus()) {
    return false;
  }
  if (input === null) {
    return false;
  }
  try {
    input.focus();
  } catch {
    // 浏览器拒绝聚焦时与 Rust 的 `set_focus` 失败相同，改走 live region。
    return false;
  }
  return hasFocus();
}

function reasonHasFocus(): boolean {
  return document.activeElement?.id === REASON_INPUT_ID;
}
