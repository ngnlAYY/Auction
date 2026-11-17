// 可访问对话框外壳；遮罩点击不关闭，Escape 在空闲时关闭。
// 焦点循环、初始聚焦、关闭后恢复与背景滚动锁由 useDialogFocusTrap 统一处理（它识别
// `[role="dialog"][aria-modal="true"]`）；本组件只负责 markup 与关闭语义。
// `busy` 时 Escape 与关闭按钮禁用；内容区取消按钮由调用方同步保护。

import type { JSX } from "@solidjs/web";

/** 模态弹窗的标题、关闭语义与内容槽。 */
export interface DialogProps {
  readonly title: string;
  readonly closeLabel: string;
  readonly busy?: boolean;
  readonly onClose: () => void;
  readonly children: JSX.Element;
}

// 同一页面可能同时存在多个弹窗（嵌套或队列），aria-labelledby 需要全页唯一的 id。
let nextDialogId = 1;

/** 模态弹窗外壳：遮罩、居中/贴底面板、标题栏与受 busy 守卫的 Escape/关闭按钮。
 * `children` 是内容槽；焦点陷阱始终把 DOM 中最内层弹窗视为当前焦点范围。 */
export function Dialog(props: DialogProps): JSX.Element {
  const titleId = `ui-dialog-title-${nextDialogId}`;
  nextDialogId += 1;
  const busy = () => props.busy === true;
  // Escape 关闭挂在遮罩根上；stopPropagation 避免嵌套弹窗时一次 Escape 连关两层。
  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: 遮罩只承担 Escape 与冒泡拦截，交互面板在内层。
    <div
      class="ui-dialog"
      role="presentation"
      onClick={(event) => event.stopPropagation()}
      onKeyDown={(event) => {
        if (event.key !== "Escape") {
          return;
        }
        event.preventDefault();
        event.stopPropagation();
        if (!busy()) {
          props.onClose();
        }
      }}
    >
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: 面板只阻止点击冒泡，键盘关闭由遮罩根统一处理。 */}
      <section
        class="ui-dialog__panel"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabindex="-1"
        onClick={(event) => event.stopPropagation()}
      >
        <div class="ui-dialog__header">
          <h2 class="ui-dialog__title" id={titleId}>
            {props.title}
          </h2>
          <button
            type="button"
            class="ui-dialog__close"
            aria-label={props.closeLabel}
            disabled={busy()}
            onClick={() => props.onClose()}
          >
            {props.closeLabel}
          </button>
        </div>
        {props.children}
      </section>
    </div>
  );
}
