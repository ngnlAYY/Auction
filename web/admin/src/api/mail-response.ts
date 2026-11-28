// Admin 邮件响应的跨字段闭集校验。
// 轮询路径沿用允许扩展字段的旧边界；入站摘要、详情和分页的字段闭集由 contracts 负责。

import type {
  AdminMailDisplayResponse,
  AdminMailEquipmentDiagnosticResponse,
  AdminMailIntakeDetailResponse,
  AdminMailIntakeListResponse,
  AdminMailIntakeMetadataResponse,
  AdminMailIntakeSummaryResponse,
  AdminMailPollingResponse,
  AdminMailPollTriggerResponse,
} from "@auction/contracts/dto/admin/mail";
import { AdminMailPollingDiagnosticsResponseSchema } from "@auction/contracts/dto/admin/mail/progress";
import type { ApiResponse } from "@auction/contracts/dto/response";
import { int32, integer, optionalNullable, struct } from "@auction/contracts/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { compareText, utf8ByteLength } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";
import * as v from "valibot";

import { validStatus } from "./delivery/decode";
import type { AdminMailIntakeQuery } from "./mail";
import { validDiagnostics } from "./mail-diagnostics";
import { validPageEcho } from "./pagination";
import {
  MAX_JSON_SAFE_INTEGER,
  parseAdminTime,
  validAdminTime,
  validSafeText,
  validTrimmedText,
} from "./validation";

/** 轮询状态的宽松外壳：只约束已知字段类型，诊断扩展按 contracts 解码。 */
export const StrictMailPollingSchema = struct({
  consecutive_failures: int32,
  last_poll_at: v.string(),
  mailbox_key: v.string(),
  next_poll_at: v.string(),
  paused: v.boolean(),
  state_version: integer,
  updated_at: v.string(),
  diagnostics: optionalNullable(AdminMailPollingDiagnosticsResponseSchema),
});

/** 轮询响应的 `data` 外壳。 */
export const StrictMailStateSchema = struct({
  polling: StrictMailPollingSchema,
});

/** `StrictMailStateSchema` 的推导类型。 */
export type StrictMailState = v.InferOutput<typeof StrictMailStateSchema>;

/** 校验轮询状态的时间与版本边界；诊断扩展必须与邮箱一致。 */
export function decodeMailState(data: StrictMailState): Result<AdminMailPollingResponse, string> {
  const polling = data.polling;
  const lastPollAt = parseAdminTime(polling.last_poll_at);
  if (lastPollAt === null) {
    return err("mail_poll_time");
  }
  const nextPollAt = parseAdminTime(polling.next_poll_at);
  if (nextPollAt === null) {
    return err("mail_next_time");
  }
  if (
    !validSafeText(polling.mailbox_key, 256) ||
    nextPollAt < lastPollAt ||
    polling.consecutive_failures < 0 ||
    polling.state_version < 1 ||
    polling.state_version > MAX_JSON_SAFE_INTEGER ||
    !validAdminTime(polling.updated_at) ||
    (polling.diagnostics !== null && !validDiagnostics(polling.diagnostics, polling.mailbox_key))
  ) {
    return err("mail_polling");
  }
  return ok({
    mailbox_key: polling.mailbox_key,
    last_poll_at: polling.last_poll_at,
    next_poll_at: polling.next_poll_at,
    consecutive_failures: polling.consecutive_failures,
    paused: polling.paused,
    state_version: polling.state_version,
    updated_at: polling.updated_at,
    diagnostics: polling.diagnostics,
  });
}

/** 摘要字段的共享值域谓词；详情路径复用同一校验，保证两个端点的旧字段语义一致。 */
export function validMailIntake(
  value: AdminMailIntakeSummaryResponse,
  kindValues: readonly string[],
): boolean {
  const receivedAt = parseAdminTime(value.received_at);
  if (receivedAt === null) {
    return false;
  }
  const processedAt = parseAdminTime(value.processed_at);
  if (processedAt === null) {
    return false;
  }
  return (
    validUuid(value.id) &&
    validSafeText(value.mailbox_key, 256) &&
    validSafeText(value.external_mail_id, 512) &&
    validSafeText(value.world_id, 256) &&
    (value.sender === null || validSafeText(value.sender, 256)) &&
    (value.subject === null || validSafeText(value.subject, 512)) &&
    validSafeText(value.decision_code, 256) &&
    kindValues.includes(value.kind) &&
    validTrimmedText(value.status) &&
    processedAt >= receivedAt
  );
}

/** 校验分页回显、每行摘要与筛选回显；服务端不能返回筛选外的行。 */
export function decodeMailIntakePage(
  data: AdminMailIntakeListResponse,
  query: AdminMailIntakeQuery,
  requestedPage: number,
  requestedPageSize: number,
  kindValues: readonly string[],
): Result<AdminMailIntakeListResponse, string> {
  const pagination = data.pagination;
  if (
    !validPageEcho(
      requestedPage,
      requestedPageSize,
      pagination.page,
      pagination.page_size,
      pagination.total,
      data.intakes.length,
    )
  ) {
    return err("mail_pagination");
  }
  if (!data.intakes.every((intake) => validMailIntake(intake, kindValues))) {
    return err("mail_intake");
  }
  if (
    data.intakes.some(
      (intake) =>
        (query.worldId !== "" && intake.world_id !== query.worldId) ||
        (query.kind !== "" && intake.kind !== query.kind) ||
        (query.status !== "" && intake.status !== query.status),
    )
  ) {
    return err("mail_filter");
  }
  return ok(data);
}

/** 立即轮询回执只接受 `accepted`、非空消息、本命令 ID 与正的安全整数 poller 数。 */
export function acceptedPollers(
  payload: ApiResponse<AdminMailPollTriggerResponse>,
  commandId: string,
): number | null {
  const receipt = payload.data;
  return payload.code === "accepted" &&
    payload.message !== "" &&
    validUuid(receipt.command_id) &&
    receipt.command_id === commandId &&
    Number.isSafeInteger(receipt.pollers) &&
    receipt.pollers >= 1
    ? receipt.pollers
    : null;
}

/** 校验详情摘要、元数据、投递、展示与装备诊断。 */
export function decodeMailIntakeDetail(
  value: AdminMailIntakeDetailResponse,
  kindValues: readonly string[],
): Result<AdminMailIntakeDetailResponse, string> {
  const summary: AdminMailIntakeSummaryResponse = {
    id: value.id,
    mailbox_key: value.mailbox_key,
    external_mail_id: value.external_mail_id,
    has_message_display: value.message_display !== null,
    world_id: value.world_id,
    kind: value.kind,
    status: value.status,
    decision_code: value.decision_code,
    received_at: value.received_at,
    processed_at: value.processed_at,
    sender: value.sender,
    subject: value.subject,
  };
  if (!validMailIntake(summary, kindValues)) {
    return err("mail_intake");
  }
  const metadata = decodeMailMetadata(value.metadata);
  if (!metadata.ok) {
    return metadata;
  }
  const deliveries = decodeMailDeliveries(value);
  if (!deliveries.ok) {
    return deliveries;
  }
  const display = decodeMailDisplay(value.message_display);
  if (!display.ok) {
    return display;
  }
  const equipment = decodeMailEquipmentDiagnostic(value.equipment_diagnostic);
  if (!equipment.ok) {
    return equipment;
  }
  return ok(value);
}

/** 关联任务只接受本封邮件列出的唯一 delivery；重试仍由服务端状态版本最终裁决。 */
function decodeMailDeliveries(value: AdminMailIntakeDetailResponse): Result<null, string> {
  if (value.deliveries.length > value.metadata.delivery_ids.length) {
    return err("mail_deliveries");
  }
  // 借用声明的 ID 并逐项消费，同时检查关联范围和重复项，避免每个任务重扫整份列表。
  const remaining = new Set(value.metadata.delivery_ids);
  for (const delivery of value.deliveries) {
    if (
      !validUuid(delivery.id) ||
      !remaining.delete(delivery.id) ||
      !(
        delivery.operation === "send" ||
        delivery.operation === "return" ||
        delivery.operation === "take"
      ) ||
      !validSafeText(delivery.recipient, 512) ||
      delivery.attempt_count > 2_147_483_647 ||
      (delivery.reason_code !== null && !validSafeText(delivery.reason_code, 512)) ||
      !validStatus(delivery.status) ||
      delivery.fencing_version < 0 ||
      delivery.fencing_version > MAX_JSON_SAFE_INTEGER ||
      ((delivery.status === "failed" || delivery.status === "manual_reconcile") &&
        delivery.fencing_version === 0) ||
      (delivery.failure_code !== null && !validSafeText(delivery.failure_code, 512)) ||
      (delivery.external_reference !== null && !validSafeText(delivery.external_reference, 512))
    ) {
      return err("mail_deliveries");
    }
  }
  return ok(null);
}

function decodeMailEquipmentDiagnostic(
  value: AdminMailEquipmentDiagnosticResponse | null,
): Result<null, string> {
  if (value === null) {
    return ok(null);
  }
  const detailValid = validEquipmentDetail(value.category, value.detail_code, value.stage);
  const evidenceValid = validEquipmentEvidence(
    value.evidence_status,
    value.evidence_id,
    value.retention_error,
  );
  if (
    value.schema_version !== 1 ||
    !validSafeText(value.equipment_id, 64) ||
    !asciiDigits(value.equipment_id) ||
    value.attachment_position < 1 ||
    value.attachment_position > 1000 ||
    !(
      value.stage === "request" ||
      value.stage === "response" ||
      value.stage === "parse" ||
      value.stage === "validate"
    ) ||
    !validEquipmentCategory(value.category) ||
    !detailValid ||
    (value.http_status !== null && (value.http_status < 100 || value.http_status > 599)) ||
    (value.elapsed_millis !== null && value.elapsed_millis > 300_000) ||
    !evidenceValid
  ) {
    return err("mail_equipment_diagnostic");
  }
  return ok(null);
}

function validEquipmentDetail(category: string, detailCode: string | null, stage: string): boolean {
  if (category === "protocol_drift" && detailCode !== null && stage === "parse") {
    return (
      detailCode === "equipment_name_invalid" ||
      detailCode === "equipment_quality_unknown" ||
      detailCode === "equipment_kind_unknown" ||
      detailCode === "equipment_category_unknown" ||
      detailCode === "equipment_category_mismatch" ||
      detailCode === "equipment_level_invalid" ||
      detailCode === "equipment_page_structure_invalid" ||
      detailCode === "equipment_attributes_invalid" ||
      detailCode === "equipment_canonical_mismatch"
    );
  }
  if (category === "snapshot_invalid") {
    return (
      detailCode !== null &&
      (detailCode === "adapter_invalid" ||
        detailCode === "fingerprint_invalid" ||
        detailCode === "equipment_id_invalid" ||
        detailCode === "name_invalid" ||
        detailCode === "category_invalid" ||
        detailCode === "level_invalid" ||
        detailCode === "collected_at_invalid" ||
        detailCode === "collected_at_future" ||
        detailCode === "attributes_invalid")
    );
  }
  if (category === "filter_rejected") {
    return (
      detailCode !== null &&
      (detailCode === "quality_not_allowed" ||
        detailCode === "prefix_not_allowed" ||
        detailCode === "suffix_not_allowed" ||
        detailCode === "category_not_allowed" ||
        detailCode === "equipment_type_not_allowed" ||
        detailCode === "level_not_allowed" ||
        detailCode === "attributes_invalid" ||
        detailCode === "tradeable_required")
    );
  }
  return detailCode === null;
}

function validEquipmentCategory(value: string): boolean {
  return (
    value === "configuration" ||
    value === "credential_unavailable" ||
    value === "session_expired" ||
    value === "timeout" ||
    value === "network" ||
    value === "redirect" ||
    value === "response_too_large" ||
    value === "content_encoding" ||
    value === "rate_limited" ||
    value === "server_error" ||
    value === "http_status" ||
    value === "content_type" ||
    value === "protocol_drift" ||
    value === "identity_mismatch" ||
    value === "snapshot_invalid" ||
    value === "filter_rejected"
  );
}

function validEquipmentEvidence(
  status: string,
  evidenceId: string | null,
  retentionError: string | null,
): boolean {
  switch (status) {
    case "retained":
    case "body_unavailable":
      return evidenceId !== null && validUuid(evidenceId) && retentionError === null;
    case "save_failed":
    case "completion_unconfirmed":
      return (
        evidenceId !== null &&
        validUuid(evidenceId) &&
        retentionError !== null &&
        validMachineCode(retentionError)
      );
    case "prepare_failed":
      return evidenceId === null && retentionError !== null && validMachineCode(retentionError);
    case "disabled":
      return evidenceId === null && retentionError === null;
    default:
      return false;
  }
}

function asciiDigits(value: string): boolean {
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    if (code < 0x30 || code > 0x39) {
      return false;
    }
  }
  return true;
}

function validMachineCode(value: string): boolean {
  switch (value) {
    case "invalid_context":
    case "invalid_outcome":
    case "invalid_message":
    case "payload_too_large":
    case "body_unavailable":
    case "encryption":
    case "serialization":
    case "io":
    case "capacity":
    case "not_found":
    case "authentication":
    case "unsupported_platform":
    case "retention_timeout":
    case "retention_cancelled":
    case "recorder_unavailable":
      return true;
    default:
      return false;
  }
}

function decodeMailDisplay(value: AdminMailDisplayResponse | null): Result<null, string> {
  if (value === null) {
    return ok(null);
  }
  if (
    value.schema_version !== 1 ||
    value.redaction_version !== 1 ||
    utf8ByteLength(value.body) > 64 * 1024 ||
    hasDisallowedControl(value.body) ||
    value.attachment_ids.length > 1000
  ) {
    return err("mail_display");
  }
  let previous: string | null = null;
  for (const id of value.attachment_ids) {
    if (!validSafeText(id, 128) || (previous !== null && compareText(previous, id) >= 0)) {
      return err("mail_display");
    }
    previous = id;
  }
  if (value.credits < 0 || value.cod < 0) {
    return err("mail_display");
  }
  return ok(null);
}

function hasDisallowedControl(value: string): boolean {
  for (const character of value) {
    const code = character.codePointAt(0) ?? 0;
    const control = code <= 0x1f || (code >= 0x7f && code <= 0x9f);
    if (control && code !== 0x0a && code !== 0x0d && code !== 0x09) {
      return true;
    }
  }
  return false;
}

function decodeMailMetadata(value: AdminMailIntakeMetadataResponse): Result<null, string> {
  if (
    value.added_count < 0 ||
    value.existing_count < 0 ||
    value.preview_revision < 0 ||
    value.preview_revision > MAX_JSON_SAFE_INTEGER ||
    value.attachment_count < 0 ||
    !validOptionalMetadataText(value.auction_id) ||
    !validOptionalMetadataText(value.submission_id) ||
    !validOptionalMetadataText(value.user_id) ||
    !validOptionalMetadataText(value.owner_result_code) ||
    !validOptionalMetadataText(value.reason_code) ||
    !validMetadataList(value.warning_codes) ||
    !validDeliveryIds(value.delivery_ids)
  ) {
    return err("mail_metadata");
  }
  return ok(null);
}

function validOptionalMetadataText(value: string): boolean {
  return value === "" || validSafeText(value, 512);
}

function validMetadataList(values: readonly string[]): boolean {
  return values.length <= 1000 && values.every((value) => validSafeText(value, 512));
}

/** 关联投递 ID：至多 1000 条、每条为 UUID 且不得重复（与 Rust `valid_delivery_ids` 一致）。 */
function validDeliveryIds(values: readonly string[]): boolean {
  if (values.length > 1000) {
    return false;
  }
  const seen = new Set<string>();
  return values.every((value) => {
    if (!validUuid(value) || seen.has(value)) {
      return false;
    }
    seen.add(value);
    return true;
  });
}
