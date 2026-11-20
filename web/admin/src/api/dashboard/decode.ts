// Admin dashboard 分区外壳与整份快照的解码。
// 每个可空字段的显式出现由 `types` 中的严格 schema 负责；这里只保留分区外壳规则与
// `valid` 子模块的跨字段语义，任一不符都让整份快照失败关闭。

import {
  AdminDashboardResponseSchema,
  type AdminDashboardSectionResponse,
} from "@auction/contracts/dto/admin/audit";
import { decode } from "@auction/contracts/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import type * as v from "valibot";

import { invalidResponseError } from "../failure";
import {
  AdminAlertingDataSchema,
  AdminCredentialDataSchema,
  type AdminDashboardSection,
  type AdminDashboardSnapshot,
  AdminDeliveryDataSchema,
  AdminExternalAccountsDataSchema,
  AdminFulfillmentDataSchema,
  AdminMailDataSchema,
  AdminProcessSectionDataSchema,
  AdminRealtimeDataSchema,
  AdminReconciliationDataSchema,
  AdminRecoveryDataSchema,
} from "./types";
import { validDashboardSemantics } from "./valid";

/** 解码整份 dashboard 投影，并在分区外壳与跨字段语义不成立时失败关闭。 */
export function decodeDashboardResponse(value: unknown): Result<AdminDashboardSnapshot, string> {
  const decoded = decode(AdminDashboardResponseSchema, value);
  if (!decoded.ok) {
    return err(invalidResponseError(decoded));
  }
  const snapshot = decoded.value;
  const process = decodeSection(snapshot.process, AdminProcessSectionDataSchema, false);
  if (!process.ok) {
    return err(process.error);
  }
  // 心跳读取成功但没有存活进程时，后端返回降级分区，此时没有可报告的观测时刻。
  if (
    process.value.observed_at === null &&
    process.value.data !== null &&
    (process.value.status !== "degraded" || process.value.data.processes.length > 0)
  ) {
    return err(invalidResponseError("dashboard_process_observed_at"));
  }
  const mail = decodeSection(snapshot.mail, AdminMailDataSchema, true);
  if (!mail.ok) {
    return err(mail.error);
  }
  const realtime = decodeSection(snapshot.realtime, AdminRealtimeDataSchema, true);
  if (!realtime.ok) {
    return err(realtime.error);
  }
  const credentials = decodeSection(snapshot.credentials, AdminCredentialDataSchema, true);
  if (!credentials.ok) {
    return err(credentials.error);
  }
  const externalAccounts = decodeSection(
    snapshot.external_accounts,
    AdminExternalAccountsDataSchema,
    true,
  );
  if (!externalAccounts.ok) {
    return err(externalAccounts.error);
  }
  const delivery = decodeSection(snapshot.delivery, AdminDeliveryDataSchema, true);
  if (!delivery.ok) {
    return err(delivery.error);
  }
  const fulfillment = decodeSection(snapshot.fulfillment, AdminFulfillmentDataSchema, true);
  if (!fulfillment.ok) {
    return err(fulfillment.error);
  }
  const recovery = decodeSection(snapshot.recovery, AdminRecoveryDataSchema, true);
  if (!recovery.ok) {
    return err(recovery.error);
  }
  const alerting = decodeSection(snapshot.alerting, AdminAlertingDataSchema, false);
  if (!alerting.ok) {
    return err(alerting.error);
  }
  const reconciliation = decodeSection(
    snapshot.reconciliation,
    AdminReconciliationDataSchema,
    true,
  );
  if (!reconciliation.ok) {
    return err(reconciliation.error);
  }
  const complete: AdminDashboardSnapshot = {
    health: snapshot.health,
    worker: snapshot.worker,
    process: process.value,
    mail: mail.value,
    realtime: realtime.value,
    credentials: credentials.value,
    external_accounts: externalAccounts.value,
    delivery: delivery.value,
    fulfillment: fulfillment.value,
    recovery: recovery.value,
    alerting: alerting.value,
    reconciliation: reconciliation.value,
  };
  if (!validDashboardSemantics(complete)) {
    return err(invalidResponseError("dashboard_semantics"));
  }
  return ok(complete);
}

/** 解码一个分区外壳；`unknown` 只允许带非空错误码，`ok`/`degraded` 必须有数据。 */
function decodeSection<TSchema extends v.GenericSchema>(
  section: AdminDashboardSectionResponse,
  schema: TSchema,
  observedAtRequired: boolean,
): Result<AdminDashboardSection<v.InferOutput<TSchema>>, string> {
  let data: v.InferOutput<TSchema> | null = null;
  if (section.data !== null) {
    const decoded = decode(schema, section.data);
    if (!decoded.ok) {
      return err(invalidResponseError(decoded));
    }
    data = decoded.value;
  }
  let valid = false;
  switch (section.status) {
    case "unknown":
      valid =
        section.observed_at === null &&
        data === null &&
        section.error_code !== null &&
        section.error_code !== "";
      break;
    case "ok":
    case "degraded":
      valid =
        section.error_code === null &&
        data !== null &&
        (!observedAtRequired || section.observed_at !== null);
      break;
    default:
      valid = false;
      break;
  }
  if (!valid) {
    return err("invalid_response");
  }
  return ok({
    status: section.status,
    observed_at: section.observed_at,
    error_code: section.error_code,
    data,
  });
}
