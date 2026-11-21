// 导航、公开页与登记页共用的寄售能力读取。

import type { ConsignmentStatus } from "@auction/contracts/dto/public/consignment/status";
import { createTaskResource, type TaskResource } from "@auction/web-shared/reactive/resource";
import { createRoot, onCleanup, runWithOwner } from "solid-js";

import { type ConsignmentReadError, fetchConsignmentStatus } from "../../api/consignment-status";

/** 最近一次能力结果。首读完成前由访问器返回 undefined。 */
export type ConsignmentCapability =
  | { readonly kind: "failed"; readonly error: ConsignmentReadError }
  | { readonly kind: "disabled" | "ready" | "read-only"; readonly status: ConsignmentStatus };

/** 订阅只读能力，同一页面外壳内的所有调用者共用一次读取和同一个重试入口。 */
export interface ConsignmentCapabilityReader {
  (): ConsignmentCapability | undefined;
  refresh(): void;
}

interface CapabilityOwner {
  readonly resource: TaskResource<ConsignmentCapability>;
  readonly dispose: () => void;
  readers: number;
}

let shared: CapabilityOwner | null = null;

/** 在 Solid 组件内调用；最后一个订阅离开时取消能力请求并释放快照。 */
export function useConsignmentCapability(): ConsignmentCapabilityReader {
  // 资源由订阅计数持有，不能随创建它的第一个组件卸载。
  shared ??= runWithOwner(null, () =>
    createRoot((dispose) => ({
      resource: createTaskResource(
        () => null,
        async (_, context): Promise<ConsignmentCapability> => {
          const result = await fetchConsignmentStatus(context.signal);
          if (!result.ok) return { kind: "failed", error: result.error };
          const status = result.value;
          if (!status.enabled) return { kind: "disabled", status };
          return { kind: status.registration_ready ? "ready" : "read-only", status };
        },
      ),
      dispose,
      readers: 0,
    })),
  );
  const owner = shared;

  owner.readers += 1;
  onCleanup(() => {
    owner.readers -= 1;
    if (owner.readers === 0) {
      shared = null;
      owner.dispose();
    }
  });
  return Object.assign(() => owner.resource(), { refresh: () => owner.resource.restart() });
}

/** 导航和匿名读取共用开启判断，登记暂不可用时目录依然可读。 */
export function consignmentEnabled(capability: ConsignmentCapability | undefined): boolean {
  return capability?.kind === "ready" || capability?.kind === "read-only";
}
