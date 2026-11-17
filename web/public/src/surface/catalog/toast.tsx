// Public 命令成功后的页面级提示条：弹窗关闭后在视口右下角给出结果，几秒后自行消失。
// 提示条只是已提交结果的回显，不保存命令状态；目录行与详情抽屉仍从权威投影读取价格。

import { sleep } from "@auction/web-shared/platform/timer";
import type { Cell } from "@auction/web-shared/reactive/cell";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createTaskScope } from "@auction/web-shared/reactive/tasks";
import type { JSX } from "@solidjs/web";
import { createContext, createEffect, Show } from "solid-js";

/** 自动消失前的停留时间；内容只有一两句，且同一结果随后出现在目录行上。 */
const TOAST_MILLIS = 5_000;

/** 一条页面级结果提示；`id` 用于区分连续两条内容相同的提示。 */
export interface CatalogToast {
  /** 单调序号：区分连续两条内容相同的提示，旧计时器不会提前清掉新提示。 */
  readonly id: number;
  readonly lotCode: string;
  readonly message: string;
  /** 需要与主句并列给出的补充事实（例如未领先时的当前价）。 */
  readonly detail: string | null;
}

/** 提示条写入口。信号由身份宿主持有，身份切换时上一身份的结果不会留在页面上。 */
export interface CatalogToastContextValue {
  show(lotCode: string, message: string, detail: string | null): void;
}

/** 提示条写入口的上下文；由身份宿主提供，页面级提示通过它回显命令结果。 */
export const CatalogToastContext = createContext<CatalogToastContextValue>();

/** 身份宿主持有的提示条状态与写入口。 */
export interface CatalogToastProviderState {
  readonly toast: Cell<CatalogToast | null>;
  readonly context: CatalogToastContextValue;
}

/** 在身份宿主建立提示条上下文，返回供 `CatalogToastHost` 渲染的信号。 */
export function useCatalogToastProvider(): CatalogToastProviderState {
  const current = createCell<CatalogToast | null>(null);
  const sequence = createCell(0);
  const context: CatalogToastContextValue = {
    show(lotCode, message, detail) {
      // 新提示直接替换旧提示：同一时刻只回显最近一次结果。
      const id = sequence.peek() + 1;
      sequence.set(id);
      current.set({ id, lotCode, message, detail });
    },
  };
  return { toast: current, context };
}

/** 常驻提示条宿主；计时器归身份宿主所有，不随发出提示的命令弹窗卸载。 */
export function CatalogToastHost(props: {
  readonly toast: Cell<CatalogToast | null>;
}): JSX.Element {
  // 计时任务归常驻的提示条宿主所有：发出提示的命令弹窗随即卸载，不能由它持有计时器。
  const tasks = createTaskScope();
  createEffect(
    () => props.toast()?.id,
    (id) => {
      if (id === undefined) {
        return;
      }
      tasks.spawn(async (context) => {
        if (!(await sleep(TOAST_MILLIS, context.signal))) {
          return;
        }
        if (!context.alive()) {
          return;
        }
        if (props.toast.peek()?.id === id) {
          props.toast.set(null);
        }
      });
    },
  );
  const current = () => props.toast();

  return (
    // 播报区域常驻 DOM，内容出现时读屏才会朗读；它不抢焦点，也不拦截指针。
    <div class="public-toast-region" role="status" aria-live="polite" aria-atomic="true">
      <Show when={current()}>
        {(toast) => (
          <div class="public-toast">
            <b class="public-toast__code">{toast().lotCode}</b>
            <span class="public-toast__message">{toast().message}</span>
            <Show when={toast().detail !== null}>
              <span class="public-toast__detail">{toast().detail ?? ""}</span>
            </Show>
          </div>
        )}
      </Show>
    </div>
  );
}
