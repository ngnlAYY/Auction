// 第 4 期 v2 共同回执字段与可重放的写操作拒绝码。

import * as v from "valibot";

/** v2 写操作中可由 HTTP 回执确认的 operation。 */
export const ConsignmentOperationV2Schema = v.picklist([
  "issue_reservation",
  "withdraw",
  "admin_delist",
  "release_hold",
  "recheck_quarantine",
  "release_quarantine",
]);
/** v2 写操作名。 */
export type ConsignmentOperationV2 = v.InferOutput<typeof ConsignmentOperationV2Schema>;

/**
 * 第 4 期写操作初次提交时记录的公开处置。
 *
 * `observation_appended`、`reservation_retired`、`accepted` 等内部处置只进 4M 的持久账本，
 * 不出现在公开回执 DTO。
 */
export const ConsignmentDispositionV2Schema = v.picklist([
  "reservation_issued",
  "withdrawal_requested",
  "closed",
  "hold_set",
  "hold_released",
  "recheck_queued",
  "quarantine_released",
]);
/** v2 首次成功处置。 */
export type ConsignmentDispositionV2 = v.InferOutput<typeof ConsignmentDispositionV2Schema>;

/**
 * 第 4 期写操作可返回的确定拒绝码与结果未知码。
 *
 * `equipment_committed`、`custody_unproven`、`operations_hold` 是预留码：第 4 期不产生，
 * 后续期使用。`command_id_conflict` 不写任何事实，因此不入命令账本。
 */
export const WriteRejectionCodeV2Schema = v.picklist([
  "command_id_conflict",
  "stale_version",
  "listing_not_active",
  "equipment_busy",
  "equipment_committed",
  "custody_unproven",
  "operations_hold",
  "season_closed",
  "hold_mismatch",
  "source_mismatch",
  "fence_mismatch",
  "evidence_unproven",
  "evidence_conflict",
  "disposition_conflict",
  "participant_ineligible",
  "bot_party",
  "action_not_allowed",
  "listing_limit_reached",
  "consignment_disabled",
  "mail_unavailable",
  "outcome_unknown",
]);
/** 第 4 期写操作的拒绝码。 */
export type WriteRejectionCodeV2 = v.InferOutput<typeof WriteRejectionCodeV2Schema>;

/** 与该码配对的 HTTP 状态。 */
export const WRITE_REJECTION_HTTP_STATUS = {
  command_id_conflict: 409,
  stale_version: 409,
  listing_not_active: 409,
  equipment_busy: 409,
  equipment_committed: 409,
  custody_unproven: 409,
  operations_hold: 409,
  season_closed: 409,
  hold_mismatch: 409,
  source_mismatch: 409,
  fence_mismatch: 409,
  evidence_unproven: 409,
  evidence_conflict: 409,
  disposition_conflict: 409,
  participant_ineligible: 403,
  bot_party: 403,
  action_not_allowed: 403,
  listing_limit_reached: 429,
  consignment_disabled: 404,
  mail_unavailable: 503,
  outcome_unknown: 503,
} as const satisfies Record<WriteRejectionCodeV2, number>;

/** 只有提交结果未知时需要按原输入重放并先查命令账本。 */
export const WRITE_REJECTION_OUTCOME_UNKNOWN: ReadonlySet<WriteRejectionCodeV2> = new Set([
  "outcome_unknown",
]);

/** 只有确定写前的 `mail_unavailable` 可在客户端白名单中视为可信 503。 */
export const WRITE_REJECTION_TRUSTED_BEFORE_WRITE: ReadonlySet<WriteRejectionCodeV2> = new Set([
  "mail_unavailable",
]);

/**
 * 写请求在进入命令账本前就被拒绝的请求错误码。
 *
 * 这些码表示请求形状、命令 ID、目标或查询本身无效，均可确定没有写入任何事实，客户端
 * 不得以“结果未知”重放业务命令。`503 consignment_unavailable` 不在本闭集内：它可能是
 * 提交前的能力不可用，也可能是提交结果未知，客户端只能按“结果未知，原样重放”处理。
 * 数据库的 v2 命令账本不记录本闭集。
 */
export const RequestErrorCodeV2Schema = v.picklist([
  "invalid_request",
  "invalid_command_id",
  "invalid_listing_id",
  "invalid_listing_request",
  "invalid_equipment_url",
  "invalid_price",
  "invalid_reason",
  "invalid_query",
  "command_not_found",
]);
/** 确定未写入的请求错误码。 */
export type RequestErrorCodeV2 = v.InferOutput<typeof RequestErrorCodeV2Schema>;

/** 请求错误码与 HTTP 状态；这些码一律不得按未知结果重放业务命令。 */
export const REQUEST_ERROR_HTTP_STATUS = {
  invalid_request: 400,
  invalid_command_id: 400,
  invalid_listing_id: 400,
  invalid_listing_request: 400,
  invalid_equipment_url: 400,
  invalid_price: 400,
  invalid_reason: 400,
  invalid_query: 400,
  command_not_found: 404,
} as const satisfies Record<RequestErrorCodeV2, number>;
