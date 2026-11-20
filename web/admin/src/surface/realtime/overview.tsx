// Admin realtime 运维页的总览指标卡：计数取值，以及按事件、消费者与失败分区语义的状态分级。

import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

/** 指标卡的数据可用性状态。 */
export type MetricState = "unavailable" | "neutral" | "ok" | "warn" | "error";

/** 渲染一张实时运维概览指标卡。 */
export function metricCard(label: string, value: string, state: MetricState): JSX.Element {
  return (
    <div class="admin-realtime-metric" data-state={state}>
      <strong class="admin-realtime-metric__value">{value}</strong>
      <span class="admin-realtime-metric__label">{label}</span>
    </div>
  );
}

/** 计数缺失时显示固定占位文案。 */
export function metricValue(value: number | null): string {
  return value === null ? t("value-empty") : String(value);
}

/** 事件数量的状态：缺失不可用，零为中性，有数据为正常。 */
export function activityCountState(value: number | null): MetricState {
  if (value === null) {
    return "unavailable";
  }
  return value === 0 ? "neutral" : "ok";
}

/** 消费者数量的状态：缺失不可用，零为警告，有数据为正常。 */
export function consumerCountState(value: number | null): MetricState {
  if (value === null) {
    return "unavailable";
  }
  return value === 0 ? "warn" : "ok";
}

/** 失败数量的状态：缺失不可用，零为正常，有失败为错误。 */
export function failureCountState(value: number | null): MetricState {
  if (value === null) {
    return "unavailable";
  }
  return value === 0 ? "ok" : "error";
}
