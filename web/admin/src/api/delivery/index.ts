// Admin 投递详情与人工 Sent 证明的同源 HTTP 客户端。
//
// 只消费 `auction-contracts` 的安全投影；详情与预览都是只读，确认复用服务端稳定命令 ID
// 与幂等摘要。浏览器不推导状态机，也不放宽服务端闭集：未知操作类型、越界数字、非规范
// 摘要或错误投递身份一律 fail-closed。

import {
  ADMIN_DELIVERY_DECISION_FIELD_POLICY,
  ADMIN_DELIVERY_LIST_FIELD_POLICY,
  type AdminDeliveryAcknowledgeRequest,
  type AdminDeliveryAcknowledgeResponse,
  AdminDeliveryAcknowledgeResponseSchema,
  type AdminDeliveryDetailWithActionsResponse,
  AdminDeliveryDetailWithActionsResponseSchema,
  type AdminDeliverySentEvidenceRequest,
  type AdminDeliverySentProofConfirmRequest,
  type AdminDeliverySentProofPreviewRequest,
  type AdminDeliverySentProofPreviewResponse,
  AdminDeliverySentProofPreviewResponseSchema,
  type AdminDeliverySentProofResponse,
  AdminDeliverySentProofResponseSchema,
  type AdminPlayerSupportBundleResponse,
  AdminPlayerSupportBundleResponseSchema,
  adminDeliveryDigest32,
} from "@auction/contracts/dto/admin/delivery";
import { err, ok, type Result } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";

import {
  type AdminApiFailure,
  type AdminWriteFailure,
  type AdminWriteNotSent,
  adminApiFailureClient,
  decodeAdminReceipt,
  invalidResponseError,
  responseError,
  writeNotSent,
  writeOutcomeUnknown,
} from "../failure";
import type { AdminSessionHost } from "../session";
import { parseStrictRfc3339, trimRustWhitespace } from "../session-time";
import {
  type PreparedJsonCommand,
  prepareJsonCommandOnce,
  sendGet,
  sendJsonPreview,
  sendPreparedJsonWrite,
} from "../transport";
import { MAX_JSON_SAFE_INTEGER, validCount, validSafeText, validSuccessCode } from "../validation";
import { strictDeliveryEnvelopeSchema } from "./decode";
import { decodePlayerSupport, playerSupportUrl } from "./player-support";
import { decodeConfirmation, decodeDetailWithActions, decodePreview } from "./responses";

/** 与服务端 `MAX_PLAYER_SUPPORT_ROWS` 对齐；超过即视为损坏响应。 */
export const MAX_PLAYER_SUPPORT_ROWS = 50;

const PREPARED_ACKNOWLEDGEMENTS = new WeakSet<PreparedAcknowledgeDeliveryCommand>();
const PREPARED_SENT_PROOFS = new WeakSet<PreparedSentProofCommand>();

/** 玩家支持用户检索词的 UTF-8 字节上限，与服务端 `MAX_PLAYER_SUPPORT_USER_LEN` 对齐。 */
export const MAX_PLAYER_SUPPORT_USER_BYTES = 256;

/** 解除告警和人工 Sent 证明共用的理由上限，按 UTF-8 字节计算。 */
export const MAX_DELIVERY_REASON_BYTES = 2_048;

/** 校验将写入处置事实的单行理由，表单在调用前去除首尾空白。 */
export function validDeliveryReason(reason: string): boolean {
  return validSafeText(reason, MAX_DELIVERY_REASON_BYTES);
}

/** 列表与玩家支持请求的扩展投影（`include` 取值）。本页面的闭集认识结算退件 Take 来源
 * （`settlement_return_action`）与装备收回处置（`recovery_equipment_recovered`），必须显式请求；
 * 服务端缺省投影只输出升级前页面认识的值。 */
export const DELIVERY_PROJECTION_INCLUDE = "settlement_returns";

/** Rust 的 `Uuid::nil`；未决命令宿主只会生成规范 v4 UUID，nil 是唯一需显式拒绝的值。 */
const NIL_UUID = "00000000-0000-0000-0000-000000000000";

/** 人工 Sent 证明确认的冻结输入。源码结构体里的 `command_id` 由未决命令宿主在执行体给出，
 * 因此单独作为准备函数的参数传入；请求正文仍按 `AdminDeliverySentProofConfirmRequest` 带该字段。 */
export interface SentProofCommand {
  readonly deliveryId: string;
  readonly expectedFencingVersion: number;
  readonly reason: string;
  readonly evidenceReference: string;
  readonly evidence: AdminDeliverySentEvidenceRequest;
  readonly acceptCollectedAssets: boolean;
  readonly acceptAttemptHistory: boolean;
}

/** 已冻结正文与校验参数的人工 Sent 证明命令；发送与回执校验只读这一份值。 */
export interface PreparedSentProofCommand {
  readonly commandId: string;
  readonly command: SentProofCommand;
  readonly body: PreparedJsonCommand;
}

/** 解除告警的冻结参数；发送与回执校验只读这一份值。 */
export interface AcknowledgeDeliveryCommand {
  readonly deliveryId: string;
  readonly expectedFencingVersion: number;
  readonly reason: string;
}

/** 已冻结正文与校验参数的解除告警命令。 */
export interface PreparedAcknowledgeDeliveryCommand {
  readonly commandId: string;
  readonly command: AcknowledgeDeliveryCommand;
  readonly body: PreparedJsonCommand;
}

/** 人工 Sent 证明确认的准备函数；同一个 `command_id` 的重试返回同一份命令与正文。 */
export type SentProofCommandPreparation = (
  commandId: string,
) => Result<PreparedSentProofCommand, AdminWriteNotSent>;

/** 解除告警的准备函数；同一个 `command_id` 的重试返回同一份命令与正文。 */
export type AcknowledgeDeliveryPreparation = (
  commandId: string,
) => Result<PreparedAcknowledgeDeliveryCommand, AdminWriteNotSent>;

/** 在提交时冻结确认输入；执行体第一次调用返回的函数时才构造正文，后续重试复用同一文本。 */
export function prepareSentProofCommand(command: SentProofCommand): SentProofCommandPreparation {
  // 证据对象也是提交输入的一部分：复制可变清单，避免队列等待期间被草稿改写。
  const snapshot = safeSnapshotSentEvidence(command.evidence);
  if (!snapshot.ok) {
    const failure = writeNotSent(snapshot.error.code);
    return () => err(failure);
  }
  const frozen: SentProofCommand = Object.freeze({
    deliveryId: command.deliveryId,
    expectedFencingVersion: command.expectedFencingVersion,
    reason: command.reason,
    evidenceReference: command.evidenceReference,
    evidence: snapshot.value,
    acceptCollectedAssets: command.acceptCollectedAssets,
    acceptAttemptHistory: command.acceptAttemptHistory,
  });
  const prepare = prepareJsonCommandOnce((commandId) => {
    const request: AdminDeliverySentProofConfirmRequest = {
      command_id: commandId,
      expected_fencing_version: frozen.expectedFencingVersion,
      reason: frozen.reason,
      evidence_reference: frozen.evidenceReference,
      evidence: frozen.evidence,
      ...(frozen.acceptCollectedAssets ? { accept_collected_assets: true as const } : {}),
      ...(frozen.acceptAttemptHistory ? { accept_attempt_history: true as const } : {}),
    };
    return request;
  });
  return (commandId) => {
    if (!validSentProofCommand(commandId, frozen)) {
      return err(writeNotSent("invalid_delivery_command"));
    }
    const prepared = prepare(commandId);
    if (!prepared.ok) {
      return err(prepared.error);
    }
    const preparedCommand = Object.freeze({ commandId, command: frozen, body: prepared.value });
    PREPARED_SENT_PROOFS.add(preparedCommand);
    return ok(preparedCommand);
  };
}

/** 提交时冻结解除告警参数；正文与回执校验共用同一个不可变命令。 */
export function prepareAcknowledgeDelivery(
  deliveryId: string,
  expectedFencingVersion: number,
  reason: string,
): AcknowledgeDeliveryPreparation {
  const frozen: AcknowledgeDeliveryCommand = Object.freeze({
    deliveryId,
    expectedFencingVersion,
    reason,
  });
  const prepare = prepareJsonCommandOnce((commandId) => {
    const request: AdminDeliveryAcknowledgeRequest = {
      command_id: commandId,
      expected_fencing_version: frozen.expectedFencingVersion,
      reason: frozen.reason,
    };
    return request;
  });
  return (commandId) => {
    if (!validAcknowledgeCommand(commandId, frozen)) {
      return err(writeNotSent("invalid_delivery_command"));
    }
    const prepared = prepare(commandId);
    if (!prepared.ok) {
      return err(prepared.error);
    }
    const preparedCommand = Object.freeze({ commandId, command: frozen, body: prepared.value });
    PREPARED_ACKNOWLEDGEMENTS.add(preparedCommand);
    return ok(preparedCommand);
  };
}

/** 详情在同一快照读取处置准入与 Sent 实见附件，供证明对话框直接使用。 */
function deliveryDetailUrl(deliveryId: string): string {
  return `/api/v1/admin/deliveries/${deliveryId}?include=sent_proof`;
}

/** 读取单条投递的完整证据详情；响应必须精确对应请求的投递 ID。 */
export async function fetchDeliveryDetail(
  host: AdminSessionHost,
  deliveryId: string,
  signal: AbortSignal,
): Promise<Result<AdminDeliveryDetailWithActionsResponse, string>> {
  if (!validUuid(deliveryId)) {
    return err("invalid_delivery_id");
  }
  const response = await sendGet(host, deliveryDetailUrl(deliveryId), signal);
  if (!response.ok) {
    return err(response.error);
  }
  if (!responseOk(response.value.status)) {
    return err(await responseError(response.value, signal, host));
  }
  const payload = decodeAdminReceipt(
    response.value,
    strictDeliveryEnvelopeSchema(AdminDeliveryDetailWithActionsResponseSchema),
    ADMIN_DELIVERY_DECISION_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const detail = decodeDetailWithActions(payload.value, deliveryId, true);
  if (!detail.ok) {
    return err(invalidResponseError("delivery_detail"));
  }
  return ok(detail.value);
}

/** 无副作用预览；返回闭集 blockers，确认前必须由服务端再次判定。 */
export async function previewDeliverySentProof(
  host: AdminSessionHost,
  deliveryId: string,
  evidenceReference: string,
  evidence: AdminDeliverySentEvidenceRequest,
  signal: AbortSignal,
): Promise<Result<AdminDeliverySentProofPreviewResponse, AdminApiFailure>> {
  if (!validUuid(deliveryId) || !validSafeText(evidenceReference, 128)) {
    return err(adminApiFailureClient("invalid_delivery_command"));
  }
  const snapshot = safeSnapshotSentEvidence(evidence);
  if (!snapshot.ok) {
    return err(snapshot.error);
  }
  const url = `/api/v1/admin/deliveries/${deliveryId}/sent-proof/preview?include=responsibility_reasons`;
  const request: AdminDeliverySentProofPreviewRequest = {
    evidence_reference: evidenceReference,
    evidence: snapshot.value,
  };
  const response = await sendJsonPreview(host, url, request, signal);
  if (!response.ok) {
    return err(response.error);
  }
  const payload = decodeAdminReceipt(
    response.value,
    strictDeliveryEnvelopeSchema(AdminDeliverySentProofPreviewResponseSchema),
    ADMIN_DELIVERY_DECISION_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(adminApiFailureClient(payload.error));
  }
  const preview = decodePreview(payload.value, deliveryId);
  if (!preview.ok) {
    return err(adminApiFailureClient(invalidResponseError("delivery_sent_preview")));
  }
  return ok(preview.value);
}

/** 确认无资金/资产影响的历史失败已处置；不改变投递状态，只追加解除告警的事实。 */
export async function acknowledgeDelivery(
  host: AdminSessionHost,
  prepared: PreparedAcknowledgeDeliveryCommand,
  signal: AbortSignal,
): Promise<Result<AdminDeliveryAcknowledgeResponse, AdminWriteFailure>> {
  if (!PREPARED_ACKNOWLEDGEMENTS.has(prepared)) {
    return err(writeNotSent("invalid_delivery_command"));
  }
  const { deliveryId, expectedFencingVersion } = prepared.command;
  const url = `/api/v1/admin/deliveries/${deliveryId}/acknowledge`;
  const response = await sendPreparedJsonWrite(
    host,
    { url, method: "POST", endpoint: "general" },
    prepared.body,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const payload = decodeAdminReceipt(
    response.value,
    strictDeliveryEnvelopeSchema(AdminDeliveryAcknowledgeResponseSchema),
    ADMIN_DELIVERY_DECISION_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(writeOutcomeUnknown(payload.error));
  }
  return decodeAcknowledgement(payload.value, deliveryId, expectedFencingVersion);
}

function decodeAcknowledgement(
  payload: {
    readonly code: string;
    readonly message: string;
    readonly data: AdminDeliveryAcknowledgeResponse;
  },
  deliveryId: string,
  expectedFencingVersion: number,
): Result<AdminDeliveryAcknowledgeResponse, AdminWriteFailure> {
  if (
    !validSuccessCode(payload.code) ||
    payload.message.length === 0 ||
    payload.data.delivery_id !== deliveryId ||
    payload.data.fencing_version !== expectedFencingVersion ||
    payload.data.kind !== "operator_acknowledged"
  ) {
    return err(writeOutcomeUnknown(invalidResponseError("delivery_acknowledge")));
  }
  return ok(payload.data);
}

/** 提交人工 Sent 证明确认；同 `command_id` 同参由服务端重放原结果。 */
export async function confirmDeliverySentProof(
  host: AdminSessionHost,
  prepared: PreparedSentProofCommand,
  signal: AbortSignal,
): Promise<Result<AdminDeliverySentProofResponse, AdminWriteFailure>> {
  if (!PREPARED_SENT_PROOFS.has(prepared)) {
    return err(writeNotSent("invalid_delivery_command"));
  }
  const { deliveryId } = prepared.command;
  const { commandId } = prepared;
  const url = `/api/v1/admin/deliveries/${deliveryId}/sent-proof/confirm`;
  const response = await sendPreparedJsonWrite(
    host,
    { url, method: "POST", endpoint: "general" },
    prepared.body,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const payload = decodeAdminReceipt(
    response.value,
    strictDeliveryEnvelopeSchema(AdminDeliverySentProofResponseSchema),
    ADMIN_DELIVERY_DECISION_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(writeOutcomeUnknown(payload.error));
  }
  const confirmation = decodeConfirmation(payload.value, commandId, deliveryId);
  if (!confirmation.ok) {
    return err(writeOutcomeUnknown(invalidResponseError("delivery_sent_confirmation")));
  }
  return ok(confirmation.value);
}

/** 判断去除首尾空白后的用户检索词是否超过字节上限。 */
export function playerSupportUserTooLong(user: string): boolean {
  return utf8ByteLength(user) > MAX_PLAYER_SUPPORT_USER_BYTES;
}

/** 玩家支持只读汇总；`user` 与 `reference` 必须恰好提供一个。 */
export async function fetchPlayerSupport(
  host: AdminSessionHost,
  user: string,
  reference: string,
  signal: AbortSignal,
): Promise<Result<AdminPlayerSupportBundleResponse, string>> {
  const trimmedUser = trimRustWhitespace(user);
  const trimmedReference = trimRustWhitespace(reference);
  let key: "user" | "reference";
  let value: string;
  if (trimmedReference === "" && trimmedUser !== "" && !playerSupportUserTooLong(trimmedUser)) {
    key = "user";
    value = trimmedUser;
  } else if (trimmedUser === "" && validUuid(trimmedReference)) {
    key = "reference";
    value = trimmedReference;
  } else {
    return err("invalid_player_support_query");
  }
  if (key === "user" && /\p{Cc}/u.test(value)) {
    return err("invalid_player_support_query");
  }
  const response = await sendGet(host, playerSupportUrl(key, value), signal);
  if (!response.ok) {
    return err(response.error);
  }
  if (!responseOk(response.value.status)) {
    return err(await responseError(response.value, signal, host));
  }
  const payload = decodeAdminReceipt(
    response.value,
    strictDeliveryEnvelopeSchema(AdminPlayerSupportBundleResponseSchema),
    ADMIN_DELIVERY_LIST_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const bundle = decodePlayerSupport(payload.value);
  if (!bundle.ok) {
    return err(invalidResponseError("player_support"));
  }
  return ok(bundle.value);
}

/** gloo 的 `Response::ok`：状态码 200 到 299。 */
function responseOk(status: number): boolean {
  return status >= 200 && status <= 299;
}

/** 解除告警的字段校验与 Rust 的入口检查一致；命令 ID 由宿主生成，只需拒绝全零值。 */
function validAcknowledgeCommand(commandId: string, command: AcknowledgeDeliveryCommand): boolean {
  return (
    commandId !== NIL_UUID &&
    validUuid(command.deliveryId) &&
    Number.isSafeInteger(command.expectedFencingVersion) &&
    command.expectedFencingVersion >= 1 &&
    command.expectedFencingVersion <= MAX_JSON_SAFE_INTEGER &&
    validDeliveryReason(command.reason)
  );
}

/** 人工 Sent 证明的字段校验与 Rust 的入口检查一致；命令 ID 由宿主生成，只需拒绝全零值。 */
function validSentProofCommand(commandId: string, command: SentProofCommand): boolean {
  return (
    commandId !== NIL_UUID &&
    validUuid(command.deliveryId) &&
    Number.isSafeInteger(command.expectedFencingVersion) &&
    command.expectedFencingVersion >= 1 &&
    command.expectedFencingVersion <= MAX_JSON_SAFE_INTEGER &&
    validSafeText(command.evidenceReference, 128) &&
    validDeliveryReason(command.reason)
  );
}

/** 逐字段复制请求证据：与 Rust 结构体序列化一致，不让草稿上的额外键进入正文。
 * 摘要必须能构造为 32 字节元组，数值必须落在对应线类型可表示的范围内。 */
function snapshotSentEvidence(
  evidence: AdminDeliverySentEvidenceRequest,
): Result<AdminDeliverySentEvidenceRequest, AdminApiFailure> {
  const originalBodyDigest = adminDeliveryDigest32(evidence.original_body_digest);
  const observedBodyDigest = adminDeliveryDigest32(evidence.observed_body_digest);
  const sentMinute =
    typeof evidence.sent_minute === "string" ? parseStrictRfc3339(evidence.sent_minute) : null;
  const observedAt =
    typeof evidence.observed_at === "string" ? parseStrictRfc3339(evidence.observed_at) : null;
  if (
    evidence.schema_version !== 1 ||
    evidence.world_id !== "main" ||
    typeof evidence.external_mail_id !== "string" ||
    !/^[1-9][0-9]{0,63}$/u.test(evidence.external_mail_id) ||
    typeof evidence.sender_uid !== "string" ||
    !/^[1-9][0-9]{0,63}$/u.test(evidence.sender_uid) ||
    typeof evidence.sender_name !== "string" ||
    !validSafeText(evidence.sender_name, 256) ||
    typeof evidence.recipient !== "string" ||
    !validSafeText(evidence.recipient, 256) ||
    typeof evidence.subject !== "string" ||
    !validSafeText(evidence.subject, 512) ||
    sentMinute === null ||
    sentMinute % 60_000_000_000n !== 0n ||
    observedAt === null ||
    observedAt < sentMinute ||
    typeof evidence.collected !== "boolean" ||
    typeof evidence.receipt !== "string" ||
    utf8ByteLength(evidence.receipt) > 2048 ||
    !Array.isArray(evidence.equipment_ids) ||
    evidence.equipment_ids.length > 1 ||
    evidence.equipment_ids.some((id) => typeof id !== "string" || !/^[1-9][0-9]{0,63}$/u.test(id))
  ) {
    return err(adminApiFailureClient("invalid_request"));
  }
  if (
    originalBodyDigest === null ||
    observedBodyDigest === null ||
    !Number.isSafeInteger(evidence.schema_version) ||
    evidence.schema_version < 0 ||
    evidence.schema_version > 0xffff ||
    !validCount(evidence.remaining_credits) ||
    !validCount(evidence.cod_pending)
  ) {
    return err(adminApiFailureClient("invalid_request"));
  }
  return ok(
    Object.freeze({
      schema_version: evidence.schema_version,
      world_id: evidence.world_id,
      external_mail_id: evidence.external_mail_id,
      sender_uid: evidence.sender_uid,
      sender_name: evidence.sender_name,
      recipient: evidence.recipient,
      subject: evidence.subject,
      sent_minute: evidence.sent_minute,
      observed_at: evidence.observed_at,
      original_body_digest: Object.freeze(originalBodyDigest),
      observed_body_digest: Object.freeze(observedBodyDigest),
      remaining_credits: evidence.remaining_credits,
      cod_pending: evidence.cod_pending,
      equipment_ids: Object.freeze([...evidence.equipment_ids]),
      collected: evidence.collected,
      receipt: evidence.receipt,
    }),
  );
}

/** 证据快照的异常边界：畸形运行时输入统一降级为稳定的 `invalid_request`。 */
function safeSnapshotSentEvidence(
  evidence: AdminDeliverySentEvidenceRequest,
): Result<AdminDeliverySentEvidenceRequest, AdminApiFailure> {
  try {
    return snapshotSentEvidence(evidence);
  } catch {
    return err(adminApiFailureClient("invalid_request"));
  }
}
