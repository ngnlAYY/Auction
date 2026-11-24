// Admin delivery DTO：投递安全投影、详情、处置准入、人工 Sent 证明与玩家支持汇总。

import * as v from "valibot";
import { integer, optionalNullable, strictStruct, uint32, withDefault } from "../../wire";
import type { AdminStrictPaginationResponse } from "./pagination";
import { AdminStrictPaginationResponseSchema } from "./pagination";

const ADMIN_DELIVERY_ORIGIN_KINDS = [
  "settlement",
  "settlement_recovery_action",
  "settlement_return_action",
  "user_key",
  "manual",
  "mail_intake",
  "operations_alert",
  "lot_watch",
  "seller_summary",
  "season_reminder",
  "consignment_notice",
  "consignment_disposition",
] as const;
const ADMIN_DELIVERY_CATEGORIES = [
  "outbid",
  "account",
  "intake",
  "settlement",
  "equipment",
  "operations",
  "manual",
] as const;
const ADMIN_DELIVERY_DISPOSITION_KINDS = [
  "auto_sent_proof",
  "operator_sent_confirmation",
  "admin_failed",
  "equipment_loss",
  "cod_returned",
  "recovery_equipment_recovered",
  "legacy_self",
  "preflight_retired",
  "operator_acknowledged",
] as const;
const ADMIN_DELIVERY_OBLIGATION_KINDS = [
  "winner_equipment_cod",
  "winner_payment_carrier_cod",
  "winner_equipment_handoff",
  "seller_payout",
  "buyer_refund",
  "unsold_return",
  "withdrawn_return",
] as const;
const ADMIN_DELIVERY_BUSINESS_STATES = ["ready", "blocked", "contingent"] as const;
const ADMIN_DELIVERY_RECOVERY_STAGES = [
  "awaiting_payment",
  "payment_overdue",
  "delivering_equipment",
  "reclaiming",
  "relist_pending",
  "refunding",
  "completed",
] as const;
const ADMIN_PLAYER_SUPPORT_CHANNELS = ["mm", "pm", "pending"] as const;
const ADMIN_PLAYER_SUPPORT_REFERENCE_KINDS = [
  "delivery",
  "obligation",
  "notification",
  "lot",
  "auction",
] as const;

/** Admin delivery 安全投影；不包含 payload、lease token 或原始 result metadata。
 *
 * `cod` 只表示经过 owner 校验后从版本化 send payload 提取的金额；其余三个结果字段同样由
 * owner 从版本化 metadata 派生，不把 metadata 原文传给浏览器。
 * 严格闭集与 Admin 浏览器端的 fail-closed 解码对齐；追加的模板、类别、世界与退役/抑制原因
 * 都是省略的可选字段，旧行仍编码为原形状。新增字段需要 Admin 与 API 同版部署，旧浏览器
 * 遇到新字段会 fail-closed 拒绝整页。 */
export const AdminDeliveryResponseSchema = strictStruct({
  id: v.string(),
  /** 来源闭集。结算退件 Take（`settlement_return_action`）只在列表、详情与玩家支持请求带
   * `include=settlement_returns` 时出现：缺省列表与玩家支持投递分区不含这类投递，缺省详情
   * 拒绝读取，升级前的页面不会读到它不认识的来源。 */
  origin_kind: v.picklist(ADMIN_DELIVERY_ORIGIN_KINDS),
  origin_id: v.string(),
  recipient: v.string(),
  cod: optionalNullable(integer),
  status: v.string(),
  /** 冻结模板键；Return/Take 等无模板投递省略。 */
  template: optionalNullable(v.string()),
  /** 发送类别闭集；旧载荷或非 Send 操作省略。 */
  category: optionalNullable(v.picklist(ADMIN_DELIVERY_CATEGORIES)),
  /** 投递载荷世界；旧载荷按 `main` 归属。 */
  world: optionalNullable(v.string()),
  /** 围栏前退役原因；只在确定未写入外部系统时出现。 */
  retirement_reason: optionalNullable(v.string()),
  /** 围栏前抑制原因；只在确定未写入外部系统时出现。 */
  suppression_reason: optionalNullable(v.string()),
  lease_expires_at: optionalNullable(v.string()),
  fencing_version: integer,
  attempt_count: uint32,
  claimed_at: optionalNullable(v.string()),
  sending_at: optionalNullable(v.string()),
  completed_at: optionalNullable(v.string()),
  external_reference: optionalNullable(v.string()),
  failure_code: optionalNullable(v.string()),
  reason_code: optionalNullable(v.string()),
  /** 已发布 wire 省略 false，解码时以同一默认值补齐；null 和错误类型仍须拒绝。 */
  take_required: withDefault(v.boolean(), false),
  returned_mail_id: optionalNullable(v.string()),
  created_at: v.string(),
  updated_at: v.string(),
});

export type AdminDeliveryResponse = v.InferOutput<typeof AdminDeliveryResponseSchema>;

export const AdminDeliveryListResponseSchema = strictStruct({
  deliveries: v.pipe(v.array(AdminDeliveryResponseSchema), v.readonly()),
  pagination: AdminStrictPaginationResponseSchema,
});

export type AdminDeliveryListResponse = v.InferOutput<typeof AdminDeliveryListResponseSchema>;

/** Admin delivery 列表携带后续处置使用的 fence；解析原文时必须传入，
 * 拒绝重复键与小数、指数、`-0` 等非整数词法。 */
export const ADMIN_DELIVERY_LIST_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;

/** Admin delivery 的旧版页码信封。 */
export type AdminDeliveryPaginationResponse = AdminStrictPaginationResponse;

/** Admin 对账判失败可选的固定原因码；历史投递与已提交命令上的既有失败码保持原样。 */
export const ADMIN_DELIVERY_RECONCILE_FAILURE_CODES: readonly [
  "verified_not_sent",
  "recipient_invalid",
  "no_longer_needed",
] = ["verified_not_sent", "recipient_invalid", "no_longer_needed"];

export interface AdminDeliveryCommandRequest {
  command_id: string;
  expected_fencing_version: number;
  action?: string;
  external_reference?: string;
  failure_code?: string;
}

export const AdminDeliveryCommandResponseSchema = strictStruct({
  delivery: AdminDeliveryResponseSchema,
});

export type AdminDeliveryCommandResponse = v.InferOutput<typeof AdminDeliveryCommandResponseSchema>;

/** 操作者确认无资金/资产影响的历史失败已经处置；`reason` 必填并进入幂等摘要。 */
export interface AdminDeliveryAcknowledgeRequest {
  command_id: string;
  expected_fencing_version: number;
  reason: string;
}

export const AdminDeliveryAcknowledgeResponseSchema = strictStruct({
  delivery_id: v.string(),
  fencing_version: integer,
  kind: v.string(),
});

export type AdminDeliveryAcknowledgeResponse = v.InferOutput<
  typeof AdminDeliveryAcknowledgeResponseSchema
>;

/** 投递处置阻断原因的稳定 wire 闭集。 */
export const AdminDeliveryActionBlockerSchema = v.picklist([
  "settlement_return_action_owned",
  "settlement_ineligible",
  "funded_delivery",
  "retired_delivery",
  "stale_fencing_version",
  "sent_sighted",
  "reconcile_succeeded_for_send",
  "delivery_not_eligible",
  "status_mismatch",
  "quarantined_mail_action_not_allowed",
  "missing_reconcile_reference",
  "recipient_renamed",
  "already_disposed",
  "not_acknowledgeable",
  "recovery_retry_ineligible",
  "settlement_fulfillment_state",
  "missing_sending_fence",
  "delivery_command_snapshot_unreadable",
]);

export type AdminDeliveryActionBlocker = v.InferOutput<typeof AdminDeliveryActionBlockerSchema>;

/** 准入为闭集。阻断时须携带稳定原因码，提交仍按状态、fence 和命令参数复核。
 * 线上是以 `state` 判别的内部标签枚举，两个变体的字段集合都是闭集。 */
export const AdminDeliveryActionAvailabilitySchema = v.union([
  strictStruct({ state: v.literal("eligible") }),
  strictStruct({ state: v.literal("blocked"), reason: AdminDeliveryActionBlockerSchema }),
]);

export type AdminDeliveryActionAvailability = v.InferOutput<
  typeof AdminDeliveryActionAvailabilitySchema
>;

/** 处置准入、Sent 预览与命令结果用于判定能否继续操作；解析这些回执原文时必须传入。
 * `uniqueKeys` 拒绝任意层的重复键，`numbers` 拒绝小数、指数与 `-0`，避免回执在
 * schema 解码前被后一个同名值覆盖。 */
export const ADMIN_DELIVERY_DECISION_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;

/** 读取后端给出的阻断原因；可执行时为空。 */
export function adminDeliveryActionAvailabilityBlocker(
  availability: AdminDeliveryActionAvailability,
): AdminDeliveryActionBlocker | null {
  return availability.state === "blocked" ? availability.reason : null;
}

/** 当前 fence 上的处置准入。人工 Sent 证明使用独立的证据预览。 */
export const AdminDeliveryActionsResponseSchema = strictStruct({
  release: AdminDeliveryActionAvailabilitySchema,
  retry_failed: AdminDeliveryActionAvailabilitySchema,
  reconcile_retry: AdminDeliveryActionAvailabilitySchema,
  reconcile_succeeded: AdminDeliveryActionAvailabilitySchema,
  reconcile_failed: AdminDeliveryActionAvailabilitySchema,
  acknowledge: AdminDeliveryActionAvailabilitySchema,
});

export type AdminDeliveryActionsResponse = v.InferOutput<typeof AdminDeliveryActionsResponseSchema>;

/** 原外部动作会话的 owner 身份版本；不含 Cookie、页面令牌或凭据摘要。 */
export const AdminDeliveryCredentialResponseSchema = strictStruct({
  credential_id: v.string(),
  credential_version: integer,
  runtime_generation: integer,
});

export type AdminDeliveryCredentialResponse = v.InferOutput<
  typeof AdminDeliveryCredentialResponseSchema
>;

/** 一次冻结外部动作尝试；`operation=send` 携带冻结内容摘要，`take` 携带目标 MID。 */
export const AdminDeliveryAttemptResponseSchema = strictStruct({
  operation: v.string(),
  fencing_version: integer,
  world_id: v.string(),
  sending_at: v.string(),
  recipient: optionalNullable(v.string()),
  subject: optionalNullable(v.string()),
  /** 协议规范化正文的 SHA-256（小写 hex），不返回正文。 */
  body_digest: optionalNullable(v.string()),
  credits: optionalNullable(integer),
  cod: optionalNullable(integer),
  /** 已发布 wire 省略空清单；解码时同样按空清单补齐。 */
  equipment_ids: withDefault(v.pipe(v.array(v.string()), v.readonly()), []),
  external_mail_id: optionalNullable(v.string()),
  credential: optionalNullable(AdminDeliveryCredentialResponseSchema),
  bound_sent_mail_id: optionalNullable(v.string()),
});

export type AdminDeliveryAttemptResponse = v.InferOutput<typeof AdminDeliveryAttemptResponseSchema>;

/** 维护流程写入的可引用 Sent 证据；`reference` 是 Admin 命令选择证据时的稳定键。
 * 两个装备清单键的存在性由 API 层按 `include=sent_proof` 校验：普通详情必须都省略，
 * sent_proof 详情必须都出现；schema 只保留缺省、显式 `null` 与数组三种状态。 */
export const AdminDeliveryEvidenceResponseSchema = strictStruct({
  reference: v.string(),
  kind: v.string(),
  external_mail_id: optionalNullable(v.string()),
  observed_at: optionalNullable(v.string()),
  sent_at: optionalNullable(v.string()),
  recipient: optionalNullable(v.string()),
  subject: optionalNullable(v.string()),
  body_digest: optionalNullable(v.string()),
  collected: optionalNullable(v.boolean()),
  cod_pending: optionalNullable(integer),
  remaining_credits: optionalNullable(integer),
  remaining_equipment_count: optionalNullable(integer),
  /** 仅 `include=sent_proof` 输出。后端外层 None 表示省略、内层 None 表示历史记录缺失；
   * 解码保留键是否出现（缺省与显式 null 不合并），两个访问器再统一读作 null。 */
  remaining_equipment_ids: v.exactOptional(v.nullable(v.pipe(v.array(v.string()), v.readonly()))),
  /** 仅 `include=sent_proof` 输出。存在时为同一 MID 在领取前实见的清单；
   * 缺省与显式 null 同样不合并，访问器统一读作 null。 */
  pre_collection_equipment_ids: v.exactOptional(
    v.nullable(v.pipe(v.array(v.string()), v.readonly())),
  ),
  receipt: optionalNullable(v.string()),
});

export type AdminDeliveryEvidenceResponse = v.InferOutput<
  typeof AdminDeliveryEvidenceResponseSchema
>;

/** 返回 Worker 实见的当前装备清单。 */
export function adminDeliveryEvidenceRemainingEquipmentIds(
  evidence: AdminDeliveryEvidenceResponse,
): readonly string[] | null {
  return evidence.remaining_equipment_ids ?? null;
}

/** 返回同一 MID 领取前的实见清单。 */
export function adminDeliveryEvidencePreCollectionEquipmentIds(
  evidence: AdminDeliveryEvidenceResponse,
): readonly string[] | null {
  return evidence.pre_collection_equipment_ids ?? null;
}

/** 领取状态；`collected` 缺失表示尚未观测，不等于未领取。 */
export const AdminDeliveryCustodyResponseSchema = strictStruct({
  collected: optionalNullable(v.boolean()),
  cod_pending: optionalNullable(integer),
  remaining_credits: optionalNullable(integer),
  receipt: optionalNullable(v.string()),
  observed_at: optionalNullable(v.string()),
  /** 已发布 wire 省略 false，解码时以同一默认值补齐。 */
  operator_confirmed_collected: withDefault(v.boolean(), false),
});

export type AdminDeliveryCustodyResponse = v.InferOutput<typeof AdminDeliveryCustodyResponseSchema>;

/** 追加式处置事实；来源是 Admin 命令账本与人工 Sent 证明。 */
export const AdminDeliveryDispositionResponseSchema = strictStruct({
  command_id: v.string(),
  /** 处置种类。缺省详情响应只输出已发布的闭集；`recovery_equipment_recovered` 只在请求带
   * `include=settlement_returns` 时输出，升级前的页面不会读到它不认识的值。 */
  kind: v.picklist(ADMIN_DELIVERY_DISPOSITION_KINDS),
  occurred_at: v.string(),
  status: optionalNullable(v.string()),
  fencing_version: optionalNullable(integer),
  failure_code: optionalNullable(v.string()),
  external_reference: optionalNullable(v.string()),
  actor: optionalNullable(v.string()),
  reason: optionalNullable(v.string()),
});

export type AdminDeliveryDispositionResponse = v.InferOutput<
  typeof AdminDeliveryDispositionResponseSchema
>;

/** 投递来源的结算义务摘要。 */
export const AdminDeliveryObligationResponseSchema = strictStruct({
  obligation_id: v.string(),
  auction_id: v.string(),
  lot_id: v.string(),
  world_id: v.string(),
  obligation_kind: v.picklist(ADMIN_DELIVERY_OBLIGATION_KINDS),
  business_state: v.picklist(ADMIN_DELIVERY_BUSINESS_STATES),
  amount: integer,
  fulfillment_state: optionalNullable(v.string()),
  fulfillment_error_code: optionalNullable(v.string()),
});

export type AdminDeliveryObligationResponse = v.InferOutput<
  typeof AdminDeliveryObligationResponseSchema
>;

/** 投递来源义务对应的恢复记录摘要。 */
export const AdminDeliveryRecoveryResponseSchema = strictStruct({
  recovery_id: v.string(),
  stage: v.picklist(ADMIN_DELIVERY_RECOVERY_STAGES),
  resolution: optionalNullable(v.string()),
  payment_deadline_at: v.string(),
  payment_obligation_id: v.string(),
  blocking_code: optionalNullable(v.string()),
  manual_reconcile_delivery_id: optionalNullable(v.string()),
});

export type AdminDeliveryRecoveryResponse = v.InferOutput<
  typeof AdminDeliveryRecoveryResponseSchema
>;

/** 投递详情：尝试历史、冻结期望摘要、Sent 观察/目击、领取状态、处置与来源义务。
 * 所有字段都是安全投影：不含投递 payload、正文、Cookie、登录 key 或页面令牌。 */
export const AdminDeliveryDetailResponseSchema = strictStruct({
  delivery: AdminDeliveryResponseSchema,
  /** 冻结 Send/Take 尝试，最新在前。 */
  attempts: v.pipe(v.array(AdminDeliveryAttemptResponseSchema), v.readonly()),
  /** 维护流程写入的可引用 Sent 证据，按观测时间倒序。 */
  evidence: v.pipe(v.array(AdminDeliveryEvidenceResponseSchema), v.readonly()),
  custody: AdminDeliveryCustodyResponseSchema,
  /** 追加式处置事实，最新在前。 */
  dispositions: v.pipe(v.array(AdminDeliveryDispositionResponseSchema), v.readonly()),
  obligation: optionalNullable(AdminDeliveryObligationResponseSchema),
  recovery: optionalNullable(AdminDeliveryRecoveryResponseSchema),
});

export type AdminDeliveryDetailResponse = v.InferOutput<typeof AdminDeliveryDetailResponseSchema>;

/** `include=disposition_actions` 的详情投影，准入与详情中的 fencing version 属于同一快照。 */
export const AdminDeliveryDetailWithActionsResponseSchema = strictStruct({
  detail: AdminDeliveryDetailResponseSchema,
  actions: AdminDeliveryActionsResponseSchema,
});

export type AdminDeliveryDetailWithActionsResponse = v.InferOutput<
  typeof AdminDeliveryDetailWithActionsResponseSchema
>;

/** 32 字节摘要的品牌；只有本模块经校验的构造函数可以产出，调用方不能直接填入 32 个
 * `number`。线上仍是 32 个数字的数组。 */
declare const DELIVERY_DIGEST_32_BRAND: unique symbol;
type DeliveryDigest32 = readonly [
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
  number,
] & { readonly [DELIVERY_DIGEST_32_BRAND]: true };

function isDeliveryDigest32(bytes: readonly number[]): bytes is DeliveryDigest32 {
  return (
    bytes.length === 32 && bytes.every((byte) => Number.isInteger(byte) && byte >= 0 && byte <= 255)
  );
}

/** 由字节数组构造 32 字节摘要；元素必须恰好 32 个且逐项为 `0..=255` 的整数。 */
export function adminDeliveryDigest32(bytes: readonly number[]): DeliveryDigest32 | null {
  const copy = [...bytes];
  return isDeliveryDigest32(copy) ? copy : null;
}

/** 把服务端的小写 hex 摘要解码为 32 字节元组；长度、字符或字节范围不合规时返回 `null`。
 * 每个元素都必须是 `0..=255` 的整数，线上仍按数字数组发送。 */
export function adminDeliveryDigest32FromHex(hex: string): DeliveryDigest32 | null {
  if (!/^[0-9a-f]{64}$/u.test(hex)) {
    return null;
  }
  const bytes: number[] = [];
  for (let index = 0; index < hex.length; index += 2) {
    bytes.push(Number.parseInt(hex.slice(index, index + 2), 16));
  }
  return adminDeliveryDigest32(bytes);
}

/** 人工 Sent 证明选用的已采集证据；摘要沿用 CLI 的 32 字节数组编码，线上是 32 个数字的数组。 */
export interface AdminDeliverySentEvidenceRequest {
  schema_version: number;
  world_id: string;
  external_mail_id: string;
  sender_uid: string;
  sender_name: string;
  recipient: string;
  subject: string;
  /** RFC3339 整分钟时间；秒与小数必须为零。 */
  sent_minute: string;
  /** RFC3339 观测时间，不得早于 `sent_minute`。 */
  observed_at: string;
  /** 原始正文摘要；32 字节，线上为数字数组。 */
  original_body_digest: DeliveryDigest32;
  /** 观测正文摘要；32 字节，线上为数字数组。 */
  observed_body_digest: DeliveryDigest32;
  remaining_credits: number;
  cod_pending: number;
  /** 后端只有 `serde(default)`、没有省略修饰；缺省时按空清单发送。 */
  equipment_ids: readonly string[];
  collected: boolean;
  receipt: string;
}

/** 人工 Sent 证明的无副作用预览；不创建命令、资金或审计事实。 */
export interface AdminDeliverySentProofPreviewRequest {
  /** 必须命中详情中已写入该投递的维护证据引用。 */
  evidence_reference: string;
  evidence: AdminDeliverySentEvidenceRequest;
}

/** 人工 Sent 证明确认；`command_id` 与 CLI 共用同一幂等摘要。 */
export interface AdminDeliverySentProofConfirmRequest {
  command_id: string;
  expected_fencing_version: number;
  reason: string;
  evidence_reference: string;
  evidence: AdminDeliverySentEvidenceRequest;
  /** Rust 省略 `false`；类型只允许显式 `true`，请求构造不得发送 `false`。 */
  accept_collected_assets?: true;
  /** Rust 省略 `false`；类型只允许显式 `true`，请求构造不得发送 `false`。 */
  accept_attempt_history?: true;
}

export const AdminDeliverySentProofPreviewResponseSchema = strictStruct({
  eligible: v.boolean(),
  blockers: v.pipe(v.array(v.string()), v.readonly()),
  delivery_id: v.string(),
  expected_fencing_version: integer,
  external_reference: v.string(),
  requires_collected_asset_acceptance: v.boolean(),
  requires_attempt_history_acceptance: v.boolean(),
  /** 仅 `include=responsibility_reasons` 输出。 */
  prior_unbound_attempts: optionalNullable(v.boolean()),
  /** 仅 `include=responsibility_reasons` 输出。 */
  sent_before_current_attempt: optionalNullable(v.boolean()),
  /** 目标是被 Admin 对账为失败的投递；确认即以 Sent 证据追加更正事实。 */
  corrects_admin_failure: v.boolean(),
});

export type AdminDeliverySentProofPreviewResponse = v.InferOutput<
  typeof AdminDeliverySentProofPreviewResponseSchema
>;

export const AdminDeliverySentProofResponseSchema = strictStruct({
  command_id: v.string(),
  delivery_id: v.string(),
  external_reference: v.string(),
  replayed: v.boolean(),
});

export type AdminDeliverySentProofResponse = v.InferOutput<
  typeof AdminDeliverySentProofResponseSchema
>;

export const AdminPlayerSupportUserResponseSchema = strictStruct({
  id: v.string(),
  username: v.string(),
  status: v.string(),
  game_player_id: optionalNullable(v.string()),
});

export type AdminPlayerSupportUserResponse = v.InferOutput<
  typeof AdminPlayerSupportUserResponseSchema
>;

export const AdminPlayerSupportBidResponseSchema = strictStruct({
  bid_id: v.string(),
  lot_id: v.string(),
  auction_id: v.string(),
  lot_number: integer,
  world_id: v.string(),
  amount: integer,
  mechanism: v.string(),
  lot_status: v.string(),
  created_at: v.string(),
});

export type AdminPlayerSupportBidResponse = v.InferOutput<
  typeof AdminPlayerSupportBidResponseSchema
>;

export const AdminPlayerSupportDealResponseSchema = strictStruct({
  lot_id: v.string(),
  auction_id: v.string(),
  lot_number: integer,
  world_id: v.string(),
  equipment_name: v.string(),
  final_price: integer,
  effective_end_at: optionalNullable(v.string()),
});

export type AdminPlayerSupportDealResponse = v.InferOutput<
  typeof AdminPlayerSupportDealResponseSchema
>;

export const AdminPlayerSupportObligationResponseSchema = strictStruct({
  obligation_id: v.string(),
  auction_id: v.string(),
  lot_id: v.string(),
  world_id: v.string(),
  obligation_kind: v.picklist(ADMIN_DELIVERY_OBLIGATION_KINDS),
  business_state: v.picklist(ADMIN_DELIVERY_BUSINESS_STATES),
  amount: integer,
  fulfillment_state: optionalNullable(v.string()),
  fulfillment_error_code: optionalNullable(v.string()),
  delivery_id: optionalNullable(v.string()),
});

export type AdminPlayerSupportObligationResponse = v.InferOutput<
  typeof AdminPlayerSupportObligationResponseSchema
>;

export const AdminPlayerSupportNotificationResponseSchema = strictStruct({
  notification_id: v.string(),
  /** `mm`、`pm` 或 `pending`。 */
  channel: v.picklist(ADMIN_PLAYER_SUPPORT_CHANNELS),
  kind: optionalNullable(v.string()),
  lot_id: optionalNullable(v.string()),
  auction_id: optionalNullable(v.string()),
  state: v.string(),
  state_reason: optionalNullable(v.string()),
  delivery_id: optionalNullable(v.string()),
  created_at: v.string(),
  updated_at: v.string(),
});

export type AdminPlayerSupportNotificationResponse = v.InferOutput<
  typeof AdminPlayerSupportNotificationResponseSchema
>;

/** 命中的参考编号实体；`kind` 为 `delivery`、`obligation`、`notification`、`lot` 或 `auction`。 */
export const AdminPlayerSupportReferenceResponseSchema = strictStruct({
  kind: v.picklist(ADMIN_PLAYER_SUPPORT_REFERENCE_KINDS),
  id: v.string(),
});

export type AdminPlayerSupportReferenceResponse = v.InferOutput<
  typeof AdminPlayerSupportReferenceResponseSchema
>;

/** 玩家支持只读汇总；经 `ApiResponse` 包裹，所有分区最多 `MAX_PLAYER_SUPPORT_ROWS` 行。 */
export const AdminPlayerSupportBundleResponseSchema = strictStruct({
  user: optionalNullable(AdminPlayerSupportUserResponseSchema),
  bids: v.pipe(v.array(AdminPlayerSupportBidResponseSchema), v.readonly()),
  deals: v.pipe(v.array(AdminPlayerSupportDealResponseSchema), v.readonly()),
  obligations: v.pipe(v.array(AdminPlayerSupportObligationResponseSchema), v.readonly()),
  deliveries: v.pipe(v.array(AdminDeliveryResponseSchema), v.readonly()),
  notifications: v.pipe(v.array(AdminPlayerSupportNotificationResponseSchema), v.readonly()),
  references: v.pipe(v.array(AdminPlayerSupportReferenceResponseSchema), v.readonly()),
  /** 任一分区达到行数上限；为真时页面不得宣称已展示完整历史。 */
  truncated: v.boolean(),
});

export type AdminPlayerSupportBundleResponse = v.InferOutput<
  typeof AdminPlayerSupportBundleResponseSchema
>;
