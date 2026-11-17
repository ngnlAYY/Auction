// 本地化时间展示组件；机器可读的 `datetime` 保留原始 RFC3339 值，
// 可见文本直接订阅当前语言变化。

import type { JSX } from "@solidjs/web";
import { formatLocalizedTimeForLanguage, useLocaleSession } from "../i18n";

/** RFC3339 时刻与可选的调用方状态类。 */
export interface TimeProps {
  readonly value: string;
  readonly class?: string;
}

/** 以 `<time datetime>` 展示 RFC3339 时间；无法解析时显示「—」。
 * `class` 供调用方叠加状态样式（如倒计时紧迫态）。 */
export function Time(props: TimeProps): JSX.Element {
  const session = useLocaleSession();
  const formatted = () => formatLocalizedTimeForLanguage(props.value, session.current());
  return (
    <time datetime={props.value} class={props.class}>
      {formatted()}
    </time>
  );
}
