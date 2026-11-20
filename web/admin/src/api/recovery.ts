// Admin recovery 报告与确认命令 HTTP 客户端。
// 报告只暴露后端允许管理端看到的事实。确认命令携带报告中的版本 fence，并由服务端
// 重新锁定、校验证据和记录幂等结果；浏览器不能把按钮状态当作写入授权。

import {
  type AdminRecoveryActionResponse,
  type AdminRecoveryConfirmationRequest,
  AdminRecoveryConfirmationResponseSchema,
  type AdminRecoveryConfirmationResultResponse,
  type AdminRecoveryReportResponse,
  AdminRecoveryReportResponseSchema,
  type AdminRecoveryResponse,
} from "@auction/contracts/dto/admin/recovery";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";

import {
  type AdminWriteFailure,
  type AdminWriteNotSent,
  decodeAdminReceipt,
  invalidRequestError,
  invalidResponseError,
  responseError,
  writeNotSent,
  writeOutcomeUnknown,
} from "./failure";
import { type AdminSessionHost, adminSessionHostLive } from "./session";
import {
  type PreparedJsonCommand,
  prepareJsonCommandOnce,
  sendGet,
  sendPreparedJsonWrite,
} from "./transport";
import { validAdminTime, validTrimmedText } from "./validation";

/** 对外保留源码中的短名；DTO 仍以 contracts 的类型为准。 */
export type AdminRecoveryAction = AdminRecoveryActionResponse;
export type AdminRecoveryReport = AdminRecoveryReportResponse;
export type AdminRecovery = AdminRecoveryResponse;
export type AdminRecoveryConfirmationResult = AdminRecoveryConfirmationResultResponse;

/** report 里的版本与允许命令会决定下一次提交，因此整份读取按 serde 的整数 visitor 解析。 */
const ADMIN_RECOVERY_REPORT_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  numbers: "integers",
};

/** 命令回执的版本推进是后续操作与未知结果判定的依据。 */
const ADMIN_RECOVERY_CONFIRMATION_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  numbers: "integers",
};

/** 读取指定原拍卖的 recovery 报告；始终请求发送归属原因的扩展投影。 */
export async function fetchAdminRecovery(
  host: AdminSessionHost,
  auctionId: string,
  signal: AbortSignal,
): Promise<Result<AdminRecoveryReport, string>> {
  const url = recoveryReportUrl(auctionId);
  const response = await sendGet(host, url, signal);
  if (!response.ok) {
    return err(response.error);
  }
  // 传输层最后一次存活检查与本次恢复之间仍可能取消或注销；不再消费缓冲的成功响应。
  if (signal.aborted || !adminSessionHostLive(host)) {
    return err("network_error");
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await responseError(received, signal, host));
  }
  const payload = decodeAdminReceipt(
    received,
    apiResponseSchema(AdminRecoveryReportResponseSchema),
    ADMIN_RECOVERY_REPORT_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const report = payload.value.data;
  if (!validateRecoveryReport(report, auctionId)) {
    return err(invalidResponseError("recovery report wire 无效"));
  }
  return ok(report);
}

function recoveryReportUrl(auctionId: string): string {
  return `/api/v1/admin/auctions/${auctionId}/recovery-report?include=send_attribution`;
}

/** 冻结的一次恢复确认意图；URL、正文与回执期望都取自这一份值。 */
export interface AdminRecoveryConfirmationIntent {
  readonly recoveryId: string;
  readonly expectedStateVersion: number;
  readonly action: string;
  readonly reason: string;
}

// 只有本模块的工厂能构造提交；外部同形对象与展开副本都无法把另一份意图的正文混进本次路由与回执核对。
const adminRecoveryConfirmationSubmissionBrand: unique symbol = Symbol(
  "admin.recovery-confirmation-submission",
);

/** 一次未决命令的提交；公开字段供页面读取，发送入口只认工厂登记的原对象。 */
export interface AdminRecoveryConfirmationSubmission {
  readonly [adminRecoveryConfirmationSubmissionBrand]: true;
  readonly intent: AdminRecoveryConfirmationIntent;
  readonly prepare: (commandId: string) => Result<PreparedJsonCommand, AdminWriteNotSent>;
}

/** 登记表中的提交状态：路由、正文与回执期望都取自同一份冻结意图。 */
interface AdminRecoveryConfirmationSubmissionState {
  readonly intent: AdminRecoveryConfirmationIntent;
  readonly prepare: (commandId: string) => Result<PreparedJsonCommand, AdminWriteNotSent>;
}

// 品牌会随对象展开被复制；冻结意图与正文工厂只按原对象从私有表取出，副本不能提交。
const adminRecoveryConfirmationSubmissions = new WeakMap<
  AdminRecoveryConfirmationSubmission,
  AdminRecoveryConfirmationSubmissionState
>();

/** 冻结提交意图并创建正文工厂；执行体第一次运行时结合 command ID 生成正文。 */
export function prepareAdminRecoveryConfirmationOnce(
  intent: AdminRecoveryConfirmationIntent,
): AdminRecoveryConfirmationSubmission {
  // 复制并冻结，提交后不再读取确认框草稿。
  const frozen = Object.freeze({ ...intent });
  const prepare = prepareJsonCommandOnce((commandId) => {
    const command: AdminRecoveryConfirmationRequest = {
      command_id: commandId,
      expected_state_version: frozen.expectedStateVersion,
      action: frozen.action,
      reason: frozen.reason,
    };
    return command;
  });
  const submission = Object.freeze({
    [adminRecoveryConfirmationSubmissionBrand]: true as const,
    intent: frozen,
    prepare,
  });
  adminRecoveryConfirmationSubmissions.set(submission, { intent: frozen, prepare });
  return submission;
}

/** 提交带版本 fence 的 recovery 确认命令。 */
export async function confirmAdminRecovery(
  host: AdminSessionHost,
  commandId: string,
  submission: AdminRecoveryConfirmationSubmission,
  signal: AbortSignal,
): Promise<Result<AdminRecoveryConfirmationResult, AdminWriteFailure>> {
  const state = adminRecoveryConfirmationSubmissions.get(submission);
  if (state === undefined) {
    return err(writeNotSent(invalidRequestError(submission)));
  }
  const intent = state.intent;
  // Rust 用 i64 构造该字段；NaN 会被序列化成 null，小数也不是服务端能构造的版本。
  if (!Number.isSafeInteger(intent.expectedStateVersion)) {
    return err(writeNotSent(invalidRequestError(intent.expectedStateVersion)));
  }
  const prepared = state.prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const url = `/api/v1/admin/recoveries/${intent.recoveryId}/confirm`;
  const response = await sendPreparedJsonWrite(
    host,
    { url, method: "POST", endpoint: "general" },
    prepared.value,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  // 已发出的写请求在传输层最后一次存活检查之后取消：结果未知，不能按未发送处理。
  if (signal.aborted || !adminSessionHostLive(host)) {
    return err(writeOutcomeUnknown("network_error"));
  }
  const received = response.value;
  const payload = decodeAdminReceipt(
    received,
    apiResponseSchema(AdminRecoveryConfirmationResponseSchema),
    ADMIN_RECOVERY_CONFIRMATION_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(writeOutcomeUnknown(payload.error));
  }
  const recovery = payload.value.data.recovery;
  if (
    !validateRecoveryConfirmation(
      recovery,
      intent.recoveryId,
      intent.action,
      intent.expectedStateVersion,
    )
  ) {
    return err(writeOutcomeUnknown(invalidResponseError("recovery confirmation 无效")));
  }
  return ok(recovery);
}

/** 恢复旧浏览器的严格 report 语义；服务端 mapper 正常不代表 wire 漂移可以被静默接受。 */
function validateRecoveryReport(report: AdminRecoveryReport, requestedAuctionId: string): boolean {
  if (!validUuid(report.auction_id) || report.auction_id !== requestedAuctionId) {
    return false;
  }
  const recoveryIds = new Set<string>();
  for (const recovery of report.recoveries) {
    if (
      recovery.auction_id !== report.auction_id ||
      recoveryIds.has(recovery.id) ||
      !validateRecovery(recovery)
    ) {
      return false;
    }
    recoveryIds.add(recovery.id);
  }
  return true;
}

function validateRecovery(recovery: AdminRecovery): boolean {
  const custodyProofValid =
    (recovery.custody_proof_kind === null && recovery.custody_proof_observed_at === null) ||
    (recovery.custody_proof_kind !== null &&
      recovery.custody_proof_observed_at !== null &&
      validTrimmedText(recovery.custody_proof_kind) &&
      validAdminTime(recovery.custody_proof_observed_at));
  const relistedValid = validOptionalUuidPair(
    recovery.relisted_auction_id,
    recovery.relisted_lot_id,
  );
  const actionIds = new Set<string>();

  if (
    !validUuid(recovery.id) ||
    !validUuid(recovery.auction_id) ||
    !validUuid(recovery.lot_id) ||
    recovery.lot_number <= 0 ||
    (recovery.world_id !== "main" && recovery.world_id !== "isekai") ||
    !validTrimmedText(recovery.equipment_name) ||
    !validTrimmedText(recovery.equipment_category) ||
    !validRecoveryStage(recovery.stage) ||
    recovery.state_version <= 0 ||
    !validAdminTime(recovery.payment_deadline_at) ||
    recovery.reminder_count < 0 ||
    recovery.reminder_limit <= 0 ||
    recovery.reminder_count > recovery.reminder_limit ||
    (recovery.resolution !== null && !validRecoveryCommand(recovery.resolution)) ||
    !custodyProofValid ||
    (recovery.blocking_code !== null && !validRecoveryError(recovery.blocking_code)) ||
    (recovery.send_attribution_blocker !== null &&
      !(
        recovery.stage === "reclaiming" &&
        validSendAttributionBlocker(recovery.send_attribution_blocker)
      )) ||
    (recovery.manual_reconcile_delivery_id !== null &&
      !validUuid(recovery.manual_reconcile_delivery_id)) ||
    !relistedValid
  ) {
    return false;
  }
  for (const action of recovery.actions) {
    if (actionIds.has(action.id) || !validateRecoveryAction(action)) {
      return false;
    }
    actionIds.add(action.id);
  }
  return (
    recovery.allowed_commands.length <= 1 &&
    recovery.allowed_commands.every(validRecoveryCommand) &&
    validAdminTime(recovery.updated_at)
  );
}

function validateRecoveryAction(action: AdminRecoveryAction): boolean {
  const proofValid =
    (action.proof_kind === null && action.proof_observed_at === null) ||
    (action.proof_kind !== null &&
      action.proof_observed_at !== null &&
      validTrimmedText(action.proof_kind) &&
      validAdminTime(action.proof_observed_at));

  return (
    validUuid(action.id) &&
    validRecoveryActionKind(action.kind) &&
    validRecoveryActionState(action.state) &&
    (action.delivery_id === null || validUuid(action.delivery_id)) &&
    (action.failure_code === null || validRecoveryError(action.failure_code)) &&
    proofValid &&
    validAdminTime(action.updated_at)
  );
}

function validateRecoveryConfirmation(
  recovery: AdminRecoveryConfirmationResult,
  requestedRecoveryId: string,
  requestedAction: string,
  expectedStateVersion: number,
): boolean {
  const relisted = recovery.relisted_auction_id !== null;
  if (
    !validUuid(recovery.id) ||
    recovery.id !== requestedRecoveryId ||
    !validRecoveryCommand(recovery.action) ||
    recovery.action !== requestedAction ||
    recovery.resolution !== recovery.action ||
    !validRecoveryStage(recovery.stage) ||
    recovery.state_version <= expectedStateVersion ||
    !validOptionalUuidPair(recovery.relisted_auction_id, recovery.relisted_lot_id)
  ) {
    return false;
  }
  switch (recovery.stage) {
    case "relist_pending":
      return !relisted;
    case "refunding":
      return recovery.action === "equipment_recovered" && relisted;
    case "completed":
      return recovery.action !== "equipment_recovered" && relisted;
    default:
      return false;
  }
}

function validOptionalUuidPair(left: string | null, right: string | null): boolean {
  if (left === null && right === null) {
    return true;
  }
  return left !== null && right !== null && validUuid(left) && validUuid(right);
}

function validRecoveryStage(value: string): boolean {
  return inClosedSet(
    [
      "awaiting_payment",
      "payment_overdue",
      "delivering_equipment",
      "reclaiming",
      "relist_pending",
      "refunding",
      "completed",
    ],
    value,
  );
}

function validRecoveryCommand(value: string): boolean {
  return inClosedSet(
    ["reclaimed", "cancelled", "equipment_recovered", "returned_after_three_cod_returns"],
    value,
  );
}

function validRecoveryActionKind(value: string): boolean {
  return inClosedSet(
    [
      "payment_reminder",
      "reclaim_equipment_take",
      "cancel_carrier_take",
      "recover_equipment_take",
      // 旧后端已有该事实，旧浏览器遗漏；作为明确扩展保留。
      "buyer_blacklist",
    ],
    value,
  );
}

function validRecoveryActionState(value: string): boolean {
  return inClosedSet(
    ["pending", "prepared", "enqueued", "satisfied", "failed", "manual_reconcile"],
    value,
  );
}

/** 发送归属不成立的具体原因闭集；与服务端恢复报告同一集合。 */
function validSendAttributionBlocker(value: string): boolean {
  return inClosedSet(
    [
      "return_not_current",
      "send_in_flight",
      "sends_exceed_returns",
      "send_without_sent_mid",
      "send_records_incomplete",
      "send_after_return",
    ],
    value,
  );
}

function validRecoveryError(value: string): boolean {
  return inClosedSet(
    [
      "invalid_recovery_fact",
      "recovery_recipient_unavailable",
      "recovery_delivery_conflict",
      "recovery_delivery_unavailable",
      "recovery_delivery_failed",
      "stale_recovery_claim",
      "recovery_evidence_not_observed",
      "recovery_evidence_unavailable",
      "recovery_evidence_mismatch",
      "recovery_outcome_unknown",
      "recovery_relist_unavailable",
    ],
    value,
  );
}

function inClosedSet(values: readonly string[], value: string): boolean {
  return values.some((candidate) => candidate === value);
}

function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}
