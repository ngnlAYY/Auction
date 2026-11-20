// 运维配置和凭据表单的字段错误播报。

import { useFocusElementByIdAfterRender } from "@auction/web-shared/platform/a11y";
import { sleep } from "@auction/web-shared/platform/timer";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskScope, type TaskHandle } from "@auction/web-shared/reactive/tasks";
import type { JSX } from "@solidjs/web";
import type { Accessor } from "solid-js";

/** 当前表单的播报状态，重复提交同一错误时也会先清空再播报。 */
export interface FieldAnnouncer {
  /** 常驻礼貌播报区的文本；由 `FieldLiveRegion` 读取。 */
  readonly message: Accessor<string>;
  /** 表单提交或替换消息时撤销先前的播报。 */
  clear(): void;
  /** 字段纠正不撤销其他字段的播报；未标记的单字段表单播报沿用原撤销行为。 */
  clearField(fieldId: string): void;
  /** 让同一条文案的连续两次播报各自产生内容更新。 */
  announce(message: string): void;
  /** 离焦时先让读屏软件读出接收焦点的控件。 */
  announceOnBlur(fieldId: string, message: string): void;
  /** 焦点播报的等待期内语言可能切换；保留字段错误，在真正播报前翻译。 */
  announceLocalizedOnBlur(fieldId: string, message: () => string): void;
  /** 提交失败时由移焦播报字段错误：字段可聚焦且尚未聚焦时移焦；已聚焦或控件禁用时通过 live region 播报。 */
  invalidSubmit(fieldId: string, message: string, focusable: boolean): void;
}

/** 为表单保留一个常驻的礼貌播报区及其更新任务。 */
export function useFieldAnnouncer(): FieldAnnouncer {
  const message = createCell("");
  const field = createCell<string | null>(null);
  const tasks = createTaskScope();
  const focusField = useFocusElementByIdAfterRender();
  let task: TaskHandle | null = null;

  const cancelTask = (): void => {
    const current = task;
    task = null;
    if (current !== null) {
      current.cancel();
    }
  };

  const clear = (): void => {
    cancelTask();
    if (message.peek() !== "") {
      message.set("");
    }
    if (field.peek() !== null) {
      field.set(null);
    }
  };

  const schedule = (fieldId: string | null, text: () => string, delayMillis: number): void => {
    clear();
    field.set(fieldId);
    task = tasks.spawn(async (context) => {
      if (!(await sleep(delayMillis, context.signal))) {
        return;
      }
      if (!context.alive()) {
        return;
      }
      message.set(text());
    });
  };

  const clearField = (fieldId: string): void => {
    const pending = field.peek();
    if (pending === null || pending === fieldId) {
      clear();
    }
  };

  const announce = (text: string): void => {
    schedule(null, () => text, 50);
  };

  const announceLocalizedOnBlur = (fieldId: string, text: () => string): void => {
    schedule(fieldId, text, 500);
  };

  const announceOnBlur = (fieldId: string, text: string): void => {
    announceLocalizedOnBlur(fieldId, () => text);
  };

  /** 提交失败时由移焦播报字段错误。已聚焦或控件禁用时，通过 live region 播报。 */
  const invalidSubmit = (fieldId: string, text: string, focusable: boolean): void => {
    if (focusable && !fieldHasFocus(fieldId)) {
      clear();
      focusField(fieldId);
    } else {
      schedule(fieldId, () => text, 50);
    }
  };

  return {
    message,
    clear,
    clearField,
    announce,
    announceOnBlur,
    announceLocalizedOnBlur,
    invalidSubmit,
  };
}

/** 字段当前是否已获得焦点；文档没有活动元素时按未聚焦处理。 */
function fieldHasFocus(fieldId: string): boolean {
  return document.activeElement?.id === fieldId;
}

/** 供辅助技术读取的播报节点，从文档布局中移出。 */
export function FieldLiveRegion(props: { readonly announcer: FieldAnnouncer }): JSX.Element {
  return (
    <div
      aria-live="polite"
      aria-atomic="true"
      style="position:absolute;width:1px;height:1px;margin:-1px;overflow:hidden;padding:0;border:0;clip-path:inset(50%);white-space:nowrap"
    >
      {props.announcer.message()}
    </div>
  );
}
