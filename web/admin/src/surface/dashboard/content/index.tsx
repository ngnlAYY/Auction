// 旧版 Admin Dashboard 的指标顺序与展示闭集。
// 快捷入口、摘要卡与各指标分组按旧版 DOM 顺序装配；状态判断集中在 `derived`，
// 每个分组的渲染落在各自子模块，避免单文件承载全部面板。

import { createValueMemo } from "@auction/web-shared/reactive/memo";
import type { JSX } from "@solidjs/web";

import type { AdminDashboardSnapshot } from "../../../api/dashboard";
import { AlertingPanel } from "./alerting";
import { AuditPanel } from "./audit";
import { CredentialsPanel } from "./credentials";
import { DeliveriesPanel } from "./deliveries";
import { deriveDashboard } from "./derived";
import { ExternalAccountsPanel } from "./external-accounts";
import { FulfillmentPanel } from "./fulfillment";
import { OutboxPanel } from "./outbox";
import { ReconciliationPanel } from "./reconciliation";
import { RecoveryPanel } from "./recovery";
import { SummarySection } from "./summary";
import { TracingPanel } from "./tracing";

/** Dashboard 内容区；`children` 由页面放在摘要之后、面板之前。 */
export interface DashboardContentProps {
  readonly snapshot: AdminDashboardSnapshot;
  readonly children?: JSX.Element;
}

/** 管理总览内容；同一快照的状态只推导一次。 */
export function DashboardContent(props: DashboardContentProps): JSX.Element {
  // 状态只推导一次，各面板读取同一份结果以保证同快照下状态一致。
  const derived = createValueMemo(() => deriveDashboard(props.snapshot));
  return (
    <>
      <SummarySection snapshot={props.snapshot} derived={derived()} />
      {props.children}
      {/* 每个分组是完整阅读单元；宽屏分栏，窄屏按原有 DOM 顺序展开。 */}
      <div class="admin-dashboard-panels">
        <AuditPanel snapshot={props.snapshot} derived={derived()} />
        <TracingPanel snapshot={props.snapshot} />
        <OutboxPanel snapshot={props.snapshot} derived={derived()} />
        <CredentialsPanel snapshot={props.snapshot} />
        <ExternalAccountsPanel snapshot={props.snapshot} />
        <DeliveriesPanel snapshot={props.snapshot} />
        <FulfillmentPanel snapshot={props.snapshot} derived={derived()} />
        <RecoveryPanel snapshot={props.snapshot} derived={derived()} />
        <ReconciliationPanel snapshot={props.snapshot} derived={derived()} />
        <AlertingPanel snapshot={props.snapshot} />
      </div>
    </>
  );
}
