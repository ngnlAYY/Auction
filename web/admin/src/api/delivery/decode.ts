// Admin 投递安全投影的 fail-closed 解码与状态机字段校验。
// 模板/类别/世界/原因只接受服务端闭集，未知形状、越界数字与不自洽的时间链一律让整页
// 失败关闭；浏览器不推导状态迁移，也不放宽服务端已判定的终态。

import type {
  AdminDeliveryCommandResponse,
  AdminDeliveryListResponse,
  AdminDeliveryResponse,
} from "@auction/contracts/dto/admin/delivery";
import { strictStruct } from "@auction/contracts/wire";
import { err, ok, type Result, unreachable } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";
import * as v from "valibot";

import { validPageEcho } from "../pagination";
import { parseStrictRfc3339 } from "../session-time";
import {
  MAX_JSON_SAFE_INTEGER,
  validCount,
  validOptionalText,
  validSuccessCode,
  validTrimmedText,
} from "../validation";
import {
  isZeroDeliveryTime,
  preflightRetirementReasonValid,
  validReturnedMailId,
  validStateFields,
  validSuppressionProjection,
} from "./decode-state";
import type { DeliveryAction } from "./queue";

/** 对外保留源码中的短名；DTO 仍以 contracts 的类型为准。 */
export type Delivery = AdminDeliveryResponse;
export type DeliveryPage = AdminDeliveryListResponse;

/**
 * Delivery 路由专用的成功响应外壳；不能用宽松的共享 DTO 接受未知字段。
 * 共享 `ApiResponse` 同时服务 Public surface，刻意保持开放；delivery 路由要求信封闭集，
 * 因此本地保留该泛型外壳。同步义务：`ApiResponse` 的 code/message/data 形状变化时必须同步此处。
 */
export interface StrictDeliveryEnvelope<T> {
  readonly code: string;
  readonly message: string;
  readonly data: T;
}

/** 与 {@link StrictDeliveryEnvelope} 对应的严格 schema；调用方按响应类别选择字段策略。 */
export function strictDeliveryEnvelopeSchema<const TSchema extends v.GenericSchema>(
  dataSchema: TSchema,
) {
  return strictStruct({
    code: v.string(),
    message: v.string(),
    data: dataSchema,
  });
}

const MAX_DELIVERY_PAGE_SIZE = 100;
const I32_MAX = 2_147_483_647;

export function decodeDeliveryList(
  payload: StrictDeliveryEnvelope<AdminDeliveryListResponse>,
  requestedPage: number,
  requestedSize: number,
): Result<DeliveryPage, null> {
  if (!validSuccessCode(payload.code) || payload.message.length === 0) {
    return err(null);
  }

  const pagination = payload.data.pagination;
  if (
    pagination.page_size > MAX_DELIVERY_PAGE_SIZE ||
    !validPageEcho(
      requestedPage,
      requestedSize,
      pagination.page,
      pagination.page_size,
      pagination.total,
      payload.data.deliveries.length,
    )
  ) {
    return err(null);
  }

  const deliveries: Delivery[] = [];
  for (const value of payload.data.deliveries) {
    const delivery = decodeDelivery(value);
    if (!delivery.ok) {
      return err(null);
    }
    deliveries.push(delivery.value);
  }
  return ok({ deliveries, pagination });
}

export function decodeDeliveryCommand(
  payload: StrictDeliveryEnvelope<AdminDeliveryCommandResponse>,
  requestedDeliveryId: string,
  action: DeliveryAction,
  expectedFencingVersion: number,
): Result<Delivery, null> {
  if (!validSuccessCode(payload.code) || payload.message.length === 0) {
    return err(null);
  }

  const delivery = decodeDelivery(payload.data.delivery);
  if (!delivery.ok) {
    return err(null);
  }
  const expectedStatus = commandStatusForAction(action);
  if (
    delivery.value.id !== requestedDeliveryId ||
    delivery.value.fencing_version <= expectedFencingVersion ||
    delivery.value.status !== expectedStatus
  ) {
    return err(null);
  }
  return ok(delivery.value);
}

export function decodeDelivery(value: AdminDeliveryResponse): Result<Delivery, null> {
  const retirementReasonValid = preflightRetirementReasonValid(value);
  // 展示标记必须绑定到已知退役原因及其来源、失败码，避免通过普通失败分支获得退役语义。
  if (
    (value.retirement_reason !== null &&
      (retirementReasonValid !== true || value.reason_code !== value.retirement_reason)) ||
    !validSuppressionProjection(value)
  ) {
    return err(null);
  }
  if (
    !validOptionalText(value.template) ||
    (value.template !== null && utf8ByteLength(value.template) > 64) ||
    (value.category !== null && !validCategory(value.category)) ||
    (value.world !== null && !validWorld(value.world))
  ) {
    return err(null);
  }
  if (!validUuid(value.id)) {
    return err(null);
  }
  const createdAt = parseDeliveryTime(value.created_at);
  const updatedAt = parseDeliveryTime(value.updated_at);
  if (createdAt === null || updatedAt === null) {
    return err(null);
  }
  const leaseExpiresAt = parseOptionalDeliveryTime(value.lease_expires_at);
  if (!leaseExpiresAt.ok) {
    return err(null);
  }
  const claimedAt = parseOptionalDeliveryTime(value.claimed_at);
  if (!claimedAt.ok) {
    return err(null);
  }
  const sendingAt = parseOptionalDeliveryTime(value.sending_at);
  if (!sendingAt.ok) {
    return err(null);
  }
  const completedAt = parseOptionalDeliveryTime(value.completed_at);
  if (!completedAt.ok) {
    return err(null);
  }

  if (
    !validOriginKind(value.origin_kind) ||
    !validTrimmedText(value.origin_id) ||
    !validTrimmedText(value.recipient) ||
    (value.cod !== null && !validCount(value.cod)) ||
    !validStatus(value.status) ||
    value.fencing_version < 0 ||
    value.fencing_version > MAX_JSON_SAFE_INTEGER ||
    value.attempt_count > I32_MAX ||
    isZeroDeliveryTime(createdAt) ||
    isZeroDeliveryTime(updatedAt) ||
    updatedAt < createdAt ||
    (claimedAt.value !== null && isZeroDeliveryTime(claimedAt.value)) ||
    (leaseExpiresAt.value !== null && isZeroDeliveryTime(leaseExpiresAt.value)) ||
    !validOptionalText(value.external_reference) ||
    !validOptionalText(value.failure_code) ||
    !validOptionalText(value.reason_code) ||
    !validReturnedMailId(value.returned_mail_id) ||
    !validStateFields(
      value,
      createdAt,
      updatedAt,
      claimedAt.value,
      sendingAt.value,
      completedAt.value,
      retirementReasonValid,
    )
  ) {
    return err(null);
  }

  // 与 Rust `Delivery` 一样按字段复制，避免调用方持有原始响应对象的可变引用。
  return ok({
    id: value.id,
    origin_kind: value.origin_kind,
    origin_id: value.origin_id,
    recipient: value.recipient,
    cod: value.cod,
    status: value.status,
    template: value.template,
    category: value.category,
    world: value.world,
    retirement_reason: value.retirement_reason,
    suppression_reason: value.suppression_reason,
    lease_expires_at: value.lease_expires_at,
    fencing_version: value.fencing_version,
    attempt_count: value.attempt_count,
    claimed_at: value.claimed_at,
    sending_at: value.sending_at,
    completed_at: value.completed_at,
    external_reference: value.external_reference,
    failure_code: value.failure_code,
    reason_code: value.reason_code,
    take_required: value.take_required,
    returned_mail_id: value.returned_mail_id,
    created_at: value.created_at,
    updated_at: value.updated_at,
  });
}

export function parseDeliveryTime(value: string): bigint | null {
  return parseStrictRfc3339(value);
}

export function validStatus(value: string): boolean {
  switch (value) {
    case "prepared":
    case "pending":
    case "claimed":
    case "sending":
    case "succeeded":
    case "failed":
    case "manual_reconcile":
      return true;
    default:
      return false;
  }
}

function parseOptionalDeliveryTime(value: string | null): Result<bigint | null, null> {
  if (value === null) {
    return ok(null);
  }
  const parsed = parseDeliveryTime(value);
  return parsed === null ? err(null) : ok(parsed);
}

/** 当前五个动作的映射参数；`DeliveryAction` 新增成员时调用处会编译失败。 */
type KnownDeliveryAction =
  | "release"
  | "retry"
  | "reconcile_retry"
  | "reconcile_succeeded"
  | "reconcile_failed";

function commandStatusForAction(action: DeliveryAction): string {
  return commandStatusForKnownAction(action);
}

function commandStatusForKnownAction(action: KnownDeliveryAction): string {
  switch (action) {
    case "release":
    case "retry":
    case "reconcile_retry":
      return "pending";
    case "reconcile_succeeded":
      return "succeeded";
    case "reconcile_failed":
      return "failed";
    default:
      return unreachable(action);
  }
}

function validOriginKind(value: string): boolean {
  switch (value) {
    case "settlement":
    case "settlement_recovery_action":
    case "settlement_return_action":
    case "user_key":
    case "manual":
    case "mail_intake":
    case "operations_alert":
    case "lot_watch":
    case "seller_summary":
    case "season_reminder":
    case "consignment_notice":
    case "consignment_disposition":
      return true;
    default:
      return false;
  }
}

function validCategory(value: string): boolean {
  switch (value) {
    case "outbid":
    case "account":
    case "intake":
    case "settlement":
    case "equipment":
    case "operations":
    case "manual":
      return true;
    default:
      return false;
  }
}

function validWorld(value: string): boolean {
  return value === "main" || value === "isekai";
}
