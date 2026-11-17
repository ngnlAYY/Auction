// 加载、错误与空状态共用的异步状态占位组件。
// 差异只在无障碍角色与是否提供重试入口；文案一律由调用方经 props 传入。

import type { JSX } from "@solidjs/web";
import { Show } from "solid-js";

import { unreachable } from "../../result";

/** 异步状态面板的三种语义。 */
export type AsyncStateKind = "loading" | "error" | "empty";

/** 异步状态面板的调用方文案与可选重试入口。 */
export interface AsyncStateProps {
  readonly kind: AsyncStateKind;
  readonly title: string;
  readonly detail: string;
  readonly retryLabel?: string;
  readonly onRetry?: () => void;
}

interface RetryAction {
  readonly label: string;
  readonly onClick: () => void;
}

// 同一页面可能出现多个状态面板，aria-labelledby 需要全页唯一 id。
let nextAsyncStateId = 1;

/** 展示异步数据的加载、错误或空结果状态。
 * 只呈现调用方已判定的安全文案，不把错误正文直接展示给浏览器；`retryLabel` 与 `onRetry`
 * 须同时提供才渲染重试按钮，避免只有样式没有行为的假按钮。 */
export function AsyncState(props: AsyncStateProps): JSX.Element {
  const titleId = `async-state-title-${nextAsyncStateId}`;
  nextAsyncStateId += 1;
  const retry = (): RetryAction | null => {
    const label = props.retryLabel;
    const onRetry = props.onRetry;
    return label === undefined || onRetry === undefined ? null : { label, onClick: onRetry };
  };
  // role="alert" 已隐含 assertive 的 aria-live，无需重复声明，避免部分阅读器双报。
  return (
    <section
      class={["ui-async-state", { [asyncStateKindClass(props.kind)]: true }]}
      role={asyncStateKindRole(props.kind)}
      aria-live={props.kind === "error" ? undefined : "polite"}
      aria-labelledby={titleId}
    >
      <h2 id={titleId} class="ui-async-state__title">
        {props.title}
      </h2>
      <p class="ui-async-state__detail">{props.detail}</p>
      <Show when={retry()}>
        {(action) => (
          <button type="button" class="ui-async-state__retry" onClick={() => action().onClick()}>
            {action().label}
          </button>
        )}
      </Show>
    </section>
  );
}

function asyncStateKindClass(kind: AsyncStateKind): string {
  switch (kind) {
    case "loading":
      return "ui-async-state--loading";
    case "error":
      return "ui-async-state--error";
    case "empty":
      return "ui-async-state--empty";
    default:
      return unreachable(kind);
  }
}

function asyncStateKindRole(kind: AsyncStateKind): "status" | "alert" {
  return kind === "error" ? "alert" : "status";
}
