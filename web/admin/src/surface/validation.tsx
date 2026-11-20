// Admin 表单的字段级校验辅助。
// 字段错误跟随控件即时展示：控件带 `aria-invalid`，错误文本经 `aria-describedby` 关联到
// 控件旁的错误节点；提交失败时焦点移到第一个错误字段。这里只处理输入层校验，
// 服务端业务错误仍走页面级反馈，但可明确归属字段的 400 类错误尽量落到对应字段。

import { useFocusElementByIdAfterRender } from "@auction/web-shared/platform/a11y";
import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

/** 渲染字段旁的错误提示；无错误时不渲染任何节点，`aria-describedby` 也不应再指向该 id。
 * `id` 是错误节点 id；必须与控件 `aria-describedby` 使用的值一致。
 * `message` 是已翻译的错误文案；`null` 表示该字段当前无错误。 */
export function FieldError(props: {
  readonly id: string;
  readonly message: string | null;
}): JSX.Element {
  return (
    <Show when={props.message !== null}>
      {/* role="alert" 让错误出现即被辅助技术播报，不必等用户移动焦点。 */}
      <p class="admin-field-error" id={props.id} role="alert">
        {props.message}
      </p>
    </Show>
  );
}

/** 提交失败后把焦点移到指定字段（调用方传入表单序上第一个错误字段的控件 id）。
 * 错误节点在事件回调返回后才写进 DOM，因此等渲染完成再聚焦。
 * 返回的函数只能在组件体里创建，事件处理器直接调用它。 */
export function useFocusField(): (fieldId: string) => void {
  return useFocusElementByIdAfterRender();
}
