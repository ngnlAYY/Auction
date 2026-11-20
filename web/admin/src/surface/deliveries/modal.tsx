// 投递确认使用浏览器原生模态能力，旧浏览器沿用现有弹窗外壳。

import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { Dialog } from "@auction/web-shared/ui/components/dialog";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";
import { onSettled, Show } from "solid-js";

/** 投递确认弹窗的属性；原生弹窗与共享 `Dialog` 共用这些内容。 */
export interface DeliveryModalProps {
  /** 弹窗本身的 DOM id；标题 id 由它派生。 */
  readonly dialogId: string;
  /** 弹窗标题。 */
  readonly title: string;
  /** 命令执行中时禁止取消并禁用关闭按钮。 */
  readonly busy: boolean;
  /** 取消或关闭弹窗时通知页面。 */
  readonly onCancel: () => void;
  /** 弹窗主体内容。 */
  readonly children: JSX.Element;
}

/** 由浏览器处理顶层、惰性背景和焦点；取消始终是初始焦点。 */
export function DeliveryModal(props: DeliveryModalProps): JSX.Element {
  const native = createCell(true);
  let dialogElement: HTMLDialogElement | null = null;
  const titleId = createValueMemo(() => `${props.dialogId}-title`);
  onSettled(() => {
    if (dialogElement === null || !invokeDialog(dialogElement, "showModal")) {
      native.set(false);
    }
  });
  return (
    <Show
      when={native()}
      fallback={
        <Dialog
          title={props.title}
          closeLabel={t("delivery-cancel")}
          busy={props.busy}
          onClose={() => props.onCancel()}
        >
          {props.children}
        </Dialog>
      }
    >
      <style>
        {"dialog.admin-delivery-modal:modal { overscroll-behavior: contain; }\n" +
          "dialog.admin-delivery-modal::backdrop { overflow: hidden; overscroll-behavior: contain; background: var(--color-overlay); }"}
      </style>
      <dialog
        id={props.dialogId}
        class="ui-dialog__panel admin-delivery-modal"
        aria-labelledby={titleId()}
        ref={(element) => {
          dialogElement = element;
        }}
        onCancel={(event) => {
          if (props.busy) {
            event.preventDefault();
          } else {
            props.onCancel();
          }
        }}
      >
        <div class="ui-dialog__header">
          <h2 id={titleId()} class="ui-dialog__title">
            {props.title}
          </h2>
          <button
            type="button"
            class="ui-dialog__close"
            disabled={props.busy}
            autofocus={true}
            onClick={() => {
              closeDeliveryModal(props.dialogId);
              props.onCancel();
            }}
          >
            {t("delivery-cancel")}
          </button>
        </div>
        {props.children}
      </dialog>
    </Show>
  );
}

/** 明确关闭让浏览器把焦点交还打开弹窗的控件。 */
export function closeDeliveryModal(dialogId: string): void {
  const element = document.getElementById(dialogId);
  if (element !== null) {
    invokeDialog(element, "close");
  }
}

function invokeDialog(element: Element, method: string): boolean {
  try {
    const candidate = Reflect.get(element, method);
    if (typeof candidate !== "function") {
      return false;
    }
    Reflect.apply(candidate, element, []);
    return true;
  } catch {
    return false;
  }
}
