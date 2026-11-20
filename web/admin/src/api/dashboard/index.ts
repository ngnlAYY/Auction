// Admin dashboard 聚合投影客户端。
// 页面只读取各 owner 提供的安全摘要；数据库行、delivery payload、Cookie 和外部错误正文
// 不会进入浏览器。分区 DTO 与后端 dashboard 响应保持字段闭集，可空字段也必须显式出现
// （值可为 `null`）；类型表达不了的跨字段语义由 `valid` 子模块在解码后校验。

import { AdminDashboardResponseSchema } from "@auction/contracts/dto/admin/audit";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, type Result } from "@auction/web-shared/result";

import { decodeAdminReceipt, responseError } from "../failure";
import type { AdminSessionHost } from "../session";
import { sendGet } from "../transport";
import { decodeDashboardResponse } from "./decode";
import type { AdminDashboardSnapshot } from "./types";

export type {
  AdminAlertingData,
  AdminAlertingSection,
  AdminCollectionOverdueItem,
  AdminCredentialAvailability,
  AdminCredentialData,
  AdminCredentialSection,
  AdminDashboardHealth,
  AdminDashboardSection,
  AdminDashboardSnapshot,
  AdminDashboardWorker,
  AdminDeliveryData,
  AdminDeliverySection,
  AdminDeliveryWater,
  AdminExternalAccountsData,
  AdminExternalAccountsSection,
  AdminExternalCredential,
  AdminExternalCredentialList,
  AdminExternalForumData,
  AdminExternalMailData,
  AdminExternalMaintenanceData,
  AdminExternalPoller,
  AdminExternalPollerList,
  AdminFulfillmentData,
  AdminFulfillmentSection,
  AdminMailData,
  AdminMailSection,
  AdminProcessData,
  AdminProcessSectionData,
  AdminRealtimeData,
  AdminRealtimeFailure,
  AdminRealtimeLag,
  AdminReconciliationData,
  AdminReconciliationSection,
  AdminReconciliationWorld,
  AdminRecoveryData,
  AdminRecoverySection,
  AdminRecoveryWorld,
  AdminTracingData,
} from "./types";

/** 看板读取地址：始终显式请求等待领取扩展投影（等待领取分组、长期未领取清单与在途字段）；
 * 缺省响应保持已发布闭集，只供升级前的页面解码。 */
const DASHBOARD_URL = "/api/v1/admin/dashboard?include=collection_wait";

// Dashboard 的轮询 `state_version` 会被邮件暂停/恢复命令原样回传，轮询项里的 `paused`
// 决定暂停或恢复方向；重复键会让这些字段取到末值，必须与 Rust 的顶部信封一样拒绝。
// 其余整数只用于展示，但仍按整份文档检查重复键，避免同一字段出现两种读法。
const ADMIN_DASHBOARD_RESPONSE_FIELD_POLICY: JsonFieldPolicy = {
  integerPaths: [["data", "mail", "data", "pollers", null, "state_version"]],
  uniqueKeys: "all",
};

/** 读取 Admin dashboard 的完整安全快照。 */
export async function fetchDashboard(
  host: AdminSessionHost,
  signal: AbortSignal,
): Promise<Result<AdminDashboardSnapshot, string>> {
  const response = await sendGet(host, DASHBOARD_URL, signal);
  if (!response.ok) {
    return err(response.error);
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await responseError(received, signal, host));
  }
  const payload = decodeAdminReceipt(
    received,
    apiResponseSchema(AdminDashboardResponseSchema),
    ADMIN_DASHBOARD_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  return decodeDashboardResponse(payload.value.data);
}

function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}
