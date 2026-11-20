// Dashboard 面板共用的稳定挂载组件，刷新时原位更新值与状态。

import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

/** 指标卡片：上方为值，下方为标签。 */
export function Card(props: { readonly label: string; readonly value: JSX.Element }): JSX.Element {
  return (
    <div class="admin-dashboard-card">
      <div class="admin-dashboard-value">{props.value}</div>
      <div class="admin-dashboard-label">{props.label}</div>
    </div>
  );
}

/** 带语义 id 与状态的指标卡片。 */
export function SemanticCard(props: {
  readonly id: string;
  readonly label: string;
  readonly state: string;
  readonly value: JSX.Element;
}): JSX.Element {
  return (
    <div class="admin-dashboard-card" id={props.id} data-state={props.state}>
      <div class="admin-dashboard-value">{props.value}</div>
      <div class="admin-dashboard-label">{props.label}</div>
    </div>
  );
}

/** 状态徽标；class 只由状态色调决定，不携带业务文案。 */
export function Badge(props: { readonly state: string; readonly value: string }): JSX.Element {
  return (
    <span class={["admin-dashboard-badge", `admin-dashboard-badge--${props.state}`]}>
      {props.value}
    </span>
  );
}

/** 可空计数：缺失时显示旧版统一的“无数据”文案。 */
export function countText(value: number | null): string {
  return value === null ? t("dashboard-legacy-no-data") : value.toString();
}
