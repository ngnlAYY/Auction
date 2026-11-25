// 订单证据的恢复段：Sale 封闭事实、Admin 处置记录与复核工作。
// 只展示 ID、动作与时刻；这里没有金额，也没有任何装备 key、预约码或原信正文（不变量 7）。

import type { OrderEvidenceItemV2 } from "@auction/contracts/dto/admin/consignment/order-v2";
import { Time } from "@auction/web-shared/ui/components/time";
import { t } from "@auction/web-shared/ui/i18n";
import type { JSX } from "@solidjs/web";

import { evidenceKeys } from "./order-labels";

type RecoveryItem = Extract<OrderEvidenceItemV2, { kind: "closure" | "resolution" | "recheck" }>;

/** 本段证据的类别判定，供订单详情把条目分流到本组件。 */
export function isRecoveryEvidence(item: OrderEvidenceItemV2): item is RecoveryItem {
  return item.kind === "closure" || item.kind === "resolution" || item.kind === "recheck";
}

function Field(props: { readonly label: string; readonly children: JSX.Element }): JSX.Element {
  return (
    <span>
      {props.label}: {props.children}
    </span>
  );
}

/** 恢复段条目的字段；ID 只作为核对线索显示。 */
export function RecoveryEvidenceFields(props: { readonly item: RecoveryItem }): JSX.Element {
  const item = props.item;
  switch (item.kind) {
    case "closure":
      return (
        <>
          <Field label={t("consignment-evidence-field-closure")}>
            {t(evidenceKeys.closure(item.closure_kind))}
          </Field>
          <Time value={item.closed_at} />
        </>
      );
    case "resolution":
      return (
        <>
          <Field label={t("consignment-evidence-field-action")}>
            {t(evidenceKeys.action(item.action))}
          </Field>
          <Field label={t("consignment-evidence-field-evidence-id")}>
            <code>{item.evidence_id}</code>
          </Field>
          <Time value={item.resolved_at} />
        </>
      );
    case "recheck":
      return (
        <>
          <Field label={t("consignment-evidence-field-work-id")}>
            <code>{item.work_id}</code>
          </Field>
          <Time value={item.requested_at} />
        </>
      );
  }
}
