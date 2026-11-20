// Delivery 页面组件之间共享的本地状态模型。

import { unreachable } from "@auction/web-shared/result";

import type { Delivery } from "../../api/delivery/decode";
import type { DeliveryAction } from "../../api/delivery/queue";

/** 当前打开的投递对话框及其动作种类。 */
export interface DeliveryDialogState {
  readonly delivery: Delivery;
  readonly kind: DeliveryDialogKind;
}

/** 页面 owner 持有的人工对账草稿；同一 delivery 重开时保留，不同 delivery 切换时替换。 */
export interface DeliveryReconcileDraft {
  readonly deliveryId: string;
  readonly action: string;
  readonly externalReference: string;
  readonly failureCode: string;
}

/** 为指定投递创建空白的人工对账草稿。 */
export function deliveryReconcileDraftForDelivery(delivery: Delivery): DeliveryReconcileDraft {
  return {
    deliveryId: delivery.id,
    action: "",
    externalReference: "",
    failureCode: "",
  };
}

/** 投递动作对话框的四种形态；`reconcile` 先收集字段，`reconcile_retry` 是第二确认。 */
export type DeliveryDialogKind = "release" | "retry" | "reconcile" | "reconcile_retry";

/** 对话框形态对应的线上投递动作；两种人工对账都提交 reconcile retry。 */
export function deliveryDialogKindAction(kind: DeliveryDialogKind): DeliveryAction {
  switch (kind) {
    case "release":
      return "release";
    case "retry":
      return "retry";
    case "reconcile":
    case "reconcile_retry":
      return "reconcile_retry";
    default:
      return unreachable(kind);
  }
}

/** 页面确认后交给未决命令宿主的冻结动作请求。 */
export interface DeliveryActionRequest {
  readonly deliveryId: string;
  readonly action: DeliveryAction;
  readonly expectedFencingVersion: number;
  readonly externalReference: string | null;
  readonly failureCode: string | null;
}
