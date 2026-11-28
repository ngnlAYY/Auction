// 结算退件案件的 Admin DTO 与闭集取值。
//
// 案件以退件入站邮件为身份：`return_id` 就是该入站邮件 ID。列表与详情只给安全摘要，不含正文、
// 发件人原文、Cookie 或装备 key；资产、金额与收件人只来自服务端冻结事实，命令请求只携带
// `command_id`、案件版本与动作名。闭集取值与服务端领域枚举逐项一致（`auction-http` 的测试
// 核对），浏览器按闭集失败关闭解码。

import * as v from "valibot";
import { integer, jsonValue, requiredNullable, strictStruct, uint8, uint32 } from "../../wire";
import { uuid } from "../../wire/uuid";
import { AdminStrictPaginationResponseSchema } from "./pagination";

/** 列表单页上限，与应用层端口一致。 */
export const ADMIN_SETTLEMENT_RETURN_MAX_PAGE_SIZE = 200;

/** 列表行偏移上限，与应用层端口一致；超过时整页请求拒绝。 */
export const ADMIN_SETTLEMENT_RETURN_MAX_OFFSET = 10_000;

/** 案件状态闭集（`settlement_return_cases.status`）。 */
export const ADMIN_SETTLEMENT_RETURN_STATUSES = [
  "quarantined",
  "recheck_pending",
  "awaiting_decision",
  "take_pending",
  "take_unknown",
  "resend_pending",
  "held",
  "resent",
  "inbox_closed",
  "reviewed",
] as const;

/** 已结案状态：代管、已补寄、留箱与已核对；其余状态计入待办。 */
export const ADMIN_SETTLEMENT_RETURN_CLOSED_STATUSES = [
  "held",
  "resent",
  "inbox_closed",
  "reviewed",
] as const;

/** 证据分类闭集（`settlement_return_evidence.match_state`）。 */
export const ADMIN_SETTLEMENT_RETURN_MATCH_STATES = [
  "untrusted",
  "unbound",
  "fulfillment_terminal",
  "manual_reconcile",
] as const;

/** 安全原因码闭集（`settlement_return_cases.reason_code`）。 */
export const ADMIN_SETTLEMENT_RETURN_REASONS = [
  "source_unproven",
  "evidence_invalid",
  "obligation_unknown",
  "subject_kind_mismatch",
  "delivery_unsent",
  "world_mismatch",
  "dispatch_unknown",
  "dispatch_ambiguous",
  "dispatch_already_returned",
  "dispatch_superseded",
  "body_unproven",
  "returned_assets_match",
  "returned_assets_mismatch",
  "received_before_send",
] as const;

/** 动作闭集；`allowed_actions`、阻断原因与命令结果共用。 */
export const ADMIN_SETTLEMENT_RETURN_ACTIONS = [
  "reclaim_and_resend",
  "reclaim_and_hold",
  "resend_from_custody",
  "close_inbox",
  "acknowledge",
  "recheck",
  "retry_take",
] as const;

/** `POST …/handle` 接受的动作：除重新核验外的全部动作；重新核验只走 `POST …/recheck`。 */
export const ADMIN_SETTLEMENT_RETURN_HANDLE_ACTIONS = [
  "reclaim_and_resend",
  "reclaim_and_hold",
  "resend_from_custody",
  "close_inbox",
  "acknowledge",
  "retry_take",
] as const;

export const AdminSettlementReturnActionSchema = v.picklist(ADMIN_SETTLEMENT_RETURN_ACTIONS);
export type AdminSettlementReturnAction = v.InferOutput<typeof AdminSettlementReturnActionSchema>;
export const AdminSettlementReturnHandleActionSchema = v.picklist(
  ADMIN_SETTLEMENT_RETURN_HANDLE_ACTIONS,
);
export type AdminSettlementReturnHandleAction = v.InferOutput<
  typeof AdminSettlementReturnHandleActionSchema
>;

/** 动作被拒绝的类型化原因闭集。 */
export const ADMIN_SETTLEMENT_RETURN_BLOCKERS = [
  "untrusted",
  "unbound",
  "assets_mismatch",
  "state_conflict",
  "resend_limit",
  "obligation_ineligible",
  "proof_required",
  "send_unresolved",
  "observation_required",
] as const;

/** 邮箱维护在原 Take 会话里能持久化为 `take_observed` 的原因。 */
const TAKE_OBSERVATIONS = [
  "assets_claimable",
  "assets_unclaimable",
  "assets_changed",
  "mail_missing",
  "receipt_unproven",
] as const;

/** Take 结果未知的原因闭集。除原会话观察外，其余原因由持久事实推出。 */
export const ADMIN_SETTLEMENT_RETURN_TAKE_UNKNOWN_REASONS = [
  ...TAKE_OBSERVATIONS,
  "session_changed",
  "take_unverifiable",
  "disposition_conflict",
  "awaiting_observation",
] as const;

/** 领回动作转结果未知时持久化在 `take_unknown.details` 中的失败码。 */
const TAKE_FAILURE_CODES = [
  "settlement_return_take_conflict",
  "settlement_return_take_unknown",
  "settlement_return_take_not_started",
  "settlement_return_take_failed",
  "mail_disposition_conflict",
] as const;

const SWEEP_RETRIES = v.pipe(uint8, v.maxValue(8));

/** 资产当前位置闭集；与资产对账使用同一组名称，互斥且不重复计数。 */
export const ADMIN_SETTLEMENT_RETURN_ASSET_LOCATIONS = [
  "unverified",
  "returned_in_inbox",
  "take_in_progress",
  "location_unknown",
  "taken_awaiting_resend",
  "taken_in_custody",
  "resent_in_transit",
] as const;

/** 领回后的处理计划闭集。 */
export const ADMIN_SETTLEMENT_RETURN_PLANS = ["resend", "hold"] as const;

/** 案件事件闭集（`settlement_return_events.event_kind`）。 */
export const ADMIN_SETTLEMENT_RETURN_EVENT_KINDS = [
  "opened",
  "acknowledged",
  "recheck_requested",
  "recheck_rebound",
  "recheck_unchanged",
  "take_requested",
  "take_activated",
  "take_proven",
  "take_unknown",
  "take_observed",
  "take_retry_requested",
  "held",
  "resend_authorized",
  "resent",
  "inbox_closed",
] as const;

/** 持久 Take 的执行状态闭集（`settlement_return_actions.execution_state`）。 */
export const ADMIN_SETTLEMENT_RETURN_TAKE_STATES = [
  "pending",
  "enqueued",
  "satisfied",
  "manual_reconcile",
] as const;

/** 结算义务种类闭集（列表筛选与摘要）。 */
export const ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS = [
  "winner_equipment_cod",
  "winner_payment_carrier_cod",
  "winner_equipment_handoff",
  "seller_payout",
  "buyer_refund",
  "unsold_return",
  "withdrawn_return",
] as const;

/** 案件的安全摘要；列表行与详情共用。 */
export const AdminSettlementReturnSummaryResponseSchema = strictStruct({
  return_id: v.string(),
  case_version: integer,
  status: v.picklist(ADMIN_SETTLEMENT_RETURN_STATUSES),
  match_state: v.picklist(ADMIN_SETTLEMENT_RETURN_MATCH_STATES),
  reason_code: v.picklist(ADMIN_SETTLEMENT_RETURN_REASONS),
  world_id: v.string(),
  external_mail_id: v.string(),
  received_at: v.string(),
  obligation_id: requiredNullable(v.string()),
  obligation_kind: requiredNullable(v.picklist(ADMIN_SETTLEMENT_RETURN_OBLIGATION_KINDS)),
  auction_id: requiredNullable(v.string()),
  lot_id: requiredNullable(v.string()),
  delivery_id: requiredNullable(v.string()),
  dispatch_no: requiredNullable(uint32),
  recipient: requiredNullable(v.string()),
  credits: integer,
  cod: integer,
  attachment_count: uint32,
  plan: requiredNullable(v.picklist(ADMIN_SETTLEMENT_RETURN_PLANS)),
  resend_limit: uint8,
  resends_used: requiredNullable(uint32),
  asset_location: v.picklist(ADMIN_SETTLEMENT_RETURN_ASSET_LOCATIONS),
  created_at: v.string(),
  updated_at: v.string(),
  closed_at: requiredNullable(v.string()),
});

export type AdminSettlementReturnSummaryResponse = v.InferOutput<
  typeof AdminSettlementReturnSummaryResponseSchema
>;

/** `GET /api/v1/admin/settlement-returns` 的分页结果。 */
export const AdminSettlementReturnListResponseSchema = strictStruct({
  returns: v.array(AdminSettlementReturnSummaryResponseSchema),
  pagination: AdminStrictPaginationResponseSchema,
});

export type AdminSettlementReturnListResponse = v.InferOutput<
  typeof AdminSettlementReturnListResponseSchema
>;

/** 案件处理事件（只追加）。 */
export const AdminSettlementReturnEventResponseSchema = v.pipe(
  strictStruct({
    case_version: v.pipe(integer, v.minValue(1)),
    event_kind: v.picklist(ADMIN_SETTLEMENT_RETURN_EVENT_KINDS),
    occurred_at: v.string(),
    actor_id: requiredNullable(v.string()),
    command_id: requiredNullable(uuid),
    details: jsonValue,
  }),
  v.check(
    (event) => validSettlementReturnEventDetails(event.event_kind, event.details),
    "invalid settlement return event details",
  ),
);

export type AdminSettlementReturnEventResponse = v.InferOutput<
  typeof AdminSettlementReturnEventResponseSchema
>;

/** 原义务的一次派发。 */
export const AdminSettlementReturnDispatchResponseSchema = strictStruct({
  dispatch_no: uint32,
  delivery_id: v.string(),
  /** 投递当前状态；补寄已授权但尚未准备时为空。 */
  delivery_status: requiredNullable(v.string()),
  sending_at: requiredNullable(v.string()),
  completed_at: requiredNullable(v.string()),
  /** 授权这次补寄的退件案件；首寄为空。 */
  authorized_by: requiredNullable(v.string()),
});

export type AdminSettlementReturnDispatchResponse = v.InferOutput<
  typeof AdminSettlementReturnDispatchResponseSchema
>;

/** 本案的持久 Take。 */
export const AdminSettlementReturnTakeResponseSchema = strictStruct({
  execution_state: v.picklist(ADMIN_SETTLEMENT_RETURN_TAKE_STATES),
  delivery_id: requiredNullable(v.string()),
  delivery_status: requiredNullable(v.string()),
  failure_code: requiredNullable(v.string()),
  /** 精确领回证明成立的时刻。 */
  proven_at: requiredNullable(v.string()),
});

export type AdminSettlementReturnTakeResponse = v.InferOutput<
  typeof AdminSettlementReturnTakeResponseSchema
>;

/** 代管责任：原权利人与代管资产；权益保留，不归 Bot 自有。 */
export const AdminSettlementReturnCustodyResponseSchema = strictStruct({
  /** 原派发冻结收件人（原权利人）。 */
  beneficiary: v.string(),
  credits: integer,
  equipment_ids: v.array(v.string()),
});

export type AdminSettlementReturnCustodyResponse = v.InferOutput<
  typeof AdminSettlementReturnCustodyResponseSchema
>;

/** 一个不被允许的动作及其类型化原因。 */
export const AdminSettlementReturnBlockerResponseSchema = strictStruct({
  action: v.picklist(ADMIN_SETTLEMENT_RETURN_ACTIONS),
  blocker: v.picklist(ADMIN_SETTLEMENT_RETURN_BLOCKERS),
});

export type AdminSettlementReturnBlockerResponse = v.InferOutput<
  typeof AdminSettlementReturnBlockerResponseSchema
>;

/** `GET /api/v1/admin/settlement-returns/{return_id}` 的详情；`allowed_actions` 与 `blockers`
 * 合起来恰好覆盖动作闭集，服务端在命令事务内仍会锁内复核。 */
export const AdminSettlementReturnDetailResponseSchema = strictStruct({
  summary: AdminSettlementReturnSummaryResponseSchema,
  events: v.array(AdminSettlementReturnEventResponseSchema),
  dispatches: v.array(AdminSettlementReturnDispatchResponseSchema),
  take: requiredNullable(AdminSettlementReturnTakeResponseSchema),
  /** 本案授权的补寄派发编号。 */
  resend_dispatch_no: requiredNullable(uint32),
  custody: requiredNullable(AdminSettlementReturnCustodyResponseSchema),
  allowed_actions: v.array(v.picklist(ADMIN_SETTLEMENT_RETURN_ACTIONS)),
  blockers: v.array(AdminSettlementReturnBlockerResponseSchema),
  /** 案件待重新核验（`recheck_pending`），但没有在线 Worker 能只读读取该世界的 MoogleMail：
   * 核验能力不可用，案件保持待核验，能力恢复后自动继续。其他状态恒为 `false`。 */
  recheck_unavailable: v.boolean(),
  /** 案件待领回（`take_pending`）、Take 尚未执行，但没有 Worker 能以持久凭据身份执行该世界的
   * Take（只配置 Cookie 或未启用邮件）：领回能力不可用，案件停在待领回，能力恢复后自动继续。
   * 其他状态恒为 `false`。 */
  take_unavailable: v.boolean(),
  /** 案件 Take 结果未知（`take_unknown`）时的具体原因，取
   * `ADMIN_SETTLEMENT_RETURN_TAKE_UNKNOWN_REASONS`；其他状态为 `null`。只读说明，再次领回的
   * 资格以 `allowed_actions` 为准。 */
  take_unknown_reason: requiredNullable(v.picklist(ADMIN_SETTLEMENT_RETURN_TAKE_UNKNOWN_REASONS)),
});

export type AdminSettlementReturnDetailResponse = v.InferOutput<
  typeof AdminSettlementReturnDetailResponseSchema
>;

/** `POST …/{return_id}/handle` 请求；recheck 只接受专用端点。 */
export const AdminSettlementReturnHandleRequestSchema = strictStruct({
  command_id: uuid,
  expected_version: v.pipe(integer, v.minValue(1)),
  action: v.picklist(ADMIN_SETTLEMENT_RETURN_HANDLE_ACTIONS),
});
export type AdminSettlementReturnHandleRequest = v.InferOutput<
  typeof AdminSettlementReturnHandleRequestSchema
>;

/** `POST …/{return_id}/recheck` 请求。 */
export const AdminSettlementReturnRecheckRequestSchema = strictStruct({
  command_id: uuid,
  expected_version: v.pipe(integer, v.minValue(1)),
});
export type AdminSettlementReturnRecheckRequest = v.InferOutput<
  typeof AdminSettlementReturnRecheckRequestSchema
>;

/** 命令结果；同一 `command_id` 原样重试返回首次提交时的结果。 */
export const AdminSettlementReturnCommandResponseSchema = strictStruct({
  command_id: uuid,
  return_id: uuid,
  action: v.picklist(ADMIN_SETTLEMENT_RETURN_ACTIONS),
  case_version: integer,
  status: v.picklist(ADMIN_SETTLEMENT_RETURN_STATUSES),
});

/** 解析 `AdminSettlementReturnCommandResponseSchema` 的退件命令回执必须传入的字段策略：任意
 * 一层出现重复键，或数字使用小数、指数与 `-0` 词法时整份响应失败，与 Rust 对
 * `case_version: i64` 的直接反序列化一致。用于判定命令成功或拒绝。 */
export const ADMIN_SETTLEMENT_RETURN_COMMAND_RESPONSE_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;

export type AdminSettlementReturnCommandResponse = v.InferOutput<
  typeof AdminSettlementReturnCommandResponseSchema
>;

/** 校验持久化案件事件的闭集细节，与 Admin HTTP 的事件投影一致。 */
export function validSettlementReturnEventDetails(kind: string, details: unknown): boolean {
  if (typeof details !== "object" || details === null || Array.isArray(details)) return false;
  const value = details as Record<string, unknown>;
  const keys = Object.keys(value).sort().join(",");
  const matches = (expected: string): boolean => keys === expected;
  const positiveInteger = (input: unknown): boolean =>
    typeof input === "number" && Number.isSafeInteger(input) && input > 0;
  const uuidValue = (input: unknown): boolean => v.safeParse(uuid, input).success;
  const stateValue = (input: unknown): boolean =>
    v.safeParse(v.picklist(ADMIN_SETTLEMENT_RETURN_MATCH_STATES), input).success;
  const reasonValue = (input: unknown): boolean =>
    v.safeParse(v.picklist(ADMIN_SETTLEMENT_RETURN_REASONS), input).success;
  switch (kind) {
    case "opened":
      return (
        (matches("dispatch_no,match_state,reason_code") &&
          (value.dispatch_no === null || positiveInteger(value.dispatch_no)) &&
          stateValue(value.match_state) &&
          reasonValue(value.reason_code)) ||
        (matches("backfilled") && value.backfilled === true)
      );
    case "acknowledged":
    case "recheck_requested":
    case "held":
      return matches("");
    case "recheck_rebound":
      return (
        (matches("delivery_id,dispatch_no,match_state,reason_code,verified_version") &&
          uuidValue(value.delivery_id) &&
          positiveInteger(value.dispatch_no) &&
          stateValue(value.match_state) &&
          reasonValue(value.reason_code) &&
          positiveInteger(value.verified_version)) ||
        (matches("backfill_orphan_ordering_repaired,migration") &&
          value.migration === 123 &&
          value.backfill_orphan_ordering_repaired === true)
      );
    case "recheck_unchanged":
      return (
        (matches("match_state,reason_code") &&
          stateValue(value.match_state) &&
          reasonValue(value.reason_code)) ||
        (matches("reason_code") && value.reason_code === "mail_not_found")
      );
    case "take_requested":
      return (
        (matches("plan") && (value.plan === "resend" || value.plan === "hold")) ||
        (matches("handover") && value.handover === true)
      );
    case "take_retry_requested":
      return (
        matches("delivery_id,fencing_version,observed_case_version") &&
        uuidValue(value.delivery_id) &&
        positiveInteger(value.fencing_version) &&
        positiveInteger(value.observed_case_version)
      );
    case "take_activated":
      return (
        (matches("delivery_id") && uuidValue(value.delivery_id)) ||
        (uuidValue(value.delivery_id) &&
          value.requeued === true &&
          typeof value.reason === "string" &&
          // 与 Admin HTTP safe_code 同口径：1..128 字节的 [a-z0-9_]；任意文案会被服务端拒绝。
          /^[a-z0-9_]{1,128}$/u.test(value.reason) &&
          ((matches("delivery_id,reason,requeued,restarts") && positiveInteger(value.restarts)) ||
            (matches("delivery_id,reason,requeued,sweep_retries") &&
              v.safeParse(SWEEP_RETRIES, value.sweep_retries).success)))
      );
    case "take_proven":
      return matches("dispatch_no,next,proof")
        ? positiveInteger(value.dispatch_no) &&
            value.next === "resend" &&
            // 与 Admin HTTP take_proof 的闭集一致；失败关闭的证明不接受任意字符串。
            (value.proof === "action_removed_or_success_page" ||
              value.proof === "action_recovered_inbox_v1")
        : matches("next,proof,resend_blocked") &&
            value.next === "hold" &&
            (value.proof === "action_removed_or_success_page" ||
              value.proof === "action_recovered_inbox_v1") &&
            (value.resend_blocked === null ||
              v.safeParse(v.picklist(ADMIN_SETTLEMENT_RETURN_BLOCKERS), value.resend_blocked)
                .success);
    case "take_unknown":
      return (
        matches("delivery_id,failure_code") &&
        (value.delivery_id === null || uuidValue(value.delivery_id)) &&
        v.safeParse(v.picklist(TAKE_FAILURE_CODES), value.failure_code).success
      );
    case "take_observed":
      return (
        matches("delivery_id,fencing_version,observation,sweep_retries") &&
        uuidValue(value.delivery_id) &&
        positiveInteger(value.fencing_version) &&
        v.safeParse(SWEEP_RETRIES, value.sweep_retries).success &&
        v.safeParse(v.picklist(TAKE_OBSERVATIONS), value.observation).success
      );
    case "resend_authorized":
      return matches("dispatch_no") && positiveInteger(value.dispatch_no);
    case "resent":
      return (
        matches("delivery_id,dispatch_no") &&
        uuidValue(value.delivery_id) &&
        positiveInteger(value.dispatch_no)
      );
    case "inbox_closed":
      return matches("asset_location") && value.asset_location === "returned_in_inbox";
    default:
      return false;
  }
}
