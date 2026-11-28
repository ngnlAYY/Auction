// Admin 拒绝的 HTTP 状态约束与账本重放范围。

const ADMIN_WRITE_ENDPOINTS = Object.freeze([
  "general",
  "user-mutation",
  "rename-revert",
  "rules-publish",
  "about-publish",
  "mail-template",
] as const);

/** 实际业务写请求所属的端点类别。 */
export type AdminWriteEndpoint = (typeof ADMIN_WRITE_ENDPOINTS)[number] | "consignment-delist";

function adminWriteEndpointIsVersionedMutation(endpoint: AdminWriteEndpoint): boolean {
  // 这些端点的 state_conflict 只来自冻结版本的递增围栏；运行配置可已有持久化意图。
  return (
    endpoint === "user-mutation" || endpoint === "rules-publish" || endpoint === "about-publish"
  );
}

/** 已确定拒绝的范围：本次尝试，或已核对同一命令 ID 的较早尝试。 */
export type Rejection = "this_attempt" | "replay_checked";

// 同码的所有写入口都必须排除命令效果。已提交且仍可生效的持久化意图保持未知。
const REJECTION_GROUPS: ReadonlyArray<readonly [number, Rejection, readonly string[]]> = [
  [
    400,
    "this_attempt",
    [
      "invalid_request_id",
      "invalid_query",
      "invalid_json",
      "invalid_replay",
      "invalid_command_id",
      "invalid_auction_id",
      "invalid_auction_command",
      "invalid_auction_schedule",
      "invalid_auction_intake",
      "unsupported_equipment_source",
      "invalid_lot_id",
      "invalid_lot_action",
      "invalid_lot_command",
      "invalid_recovery_command",
      "invalid_world",
      "invalid_world_command",
      "invalid_delivery_id",
      "invalid_user_id",
      "invalid_user_command",
      "invalid_rename_id",
      "invalid_rename_command",
      "invalid_delivery_command",
      "invalid_alert_command",
      "invalid_credential_action",
      "invalid_credential_kind",
      "invalid_credential_label",
      "invalid_credential_player_name",
      "invalid_credential_request",
      "invalid_credential_secret",
      "invalid_credential_status",
      "invalid_credential_world",
      "invalid_mail_request",
      "invalid_runtime_config",
      "invalid_locale",
      "invalid_rule_document",
      "invalid_settlement_return_command",
      "invalid_settlement_return_id",
    ],
  ],
  [401, "this_attempt", ["unauthenticated"]],
  [403, "this_attempt", ["origin_forbidden", "csrf_invalid", "manual_early_settlement_disabled"]],
  [
    404,
    "this_attempt",
    [
      "not_found",
      "mail_template_not_found",
      "world_not_found",
      "delivery_not_found",
      "settlement_return_not_found",
    ],
  ],
  [405, "this_attempt", ["method_not_allowed"]],
  [
    409,
    "this_attempt",
    [
      "command_conflict",
      "credential_conflict",
      "stale_credential_state",
      "rename_name_taken",
      "user_conflict",
      "stale_state_version",
      "stale_world_state",
      "world_has_active_auction",
      "auction_intake_unavailable",
      "seller_blacklisted",
      "equipment_conflict",
      "stale_recovery_state",
      "stale_delivery_state",
      "invalid_auction_state",
      "invalid_lot_state",
      "delivery_sent_sighted",
      // 换绑可恢复，且快照校验早于兼容旧实例的第二次账本检查，两者只裁定本次。
      "delivery_recipient_renamed",
      "delivery_command_snapshot_unreadable",
      "settlement_obligation_ineligible",
      "sent_proof_rejected",
      "settlement_return_action_conflict",
      "settlement_return_resend_limit",
      "settlement_return_obligation_ineligible",
      "settlement_return_send_unresolved",
      "settlement_return_observation_required",
      "recovery_send_unattributed",
      "recovery_not_confirmable",
      "recovery_evidence_unresolved",
      "external_access_paused",
      "poll_rejected",
    ],
  ],
  [413, "this_attempt", ["body_too_large"]],
  [415, "this_attempt", ["unsupported_media_type"]],
  [
    422,
    "this_attempt",
    [
      "invalid_mail_template",
      "invalid_runtime_switch",
      "not_acknowledgeable",
      "credential_login_rejected",
      "credential_login_challenge",
    ],
  ],
  [429, "this_attempt", ["rate_limited", "credential_login_busy"]],
  // 能力检查在写入前拒绝本次尝试，同 ID 较早的未知结果留待确认。
  [501, "this_attempt", ["feature_not_supported"]],
  // 登录失败时未得到可导入的凭据，存储写入尚未开始。
  [503, "this_attempt", ["credential_login_unavailable"]],
  [
    409,
    "replay_checked",
    [
      // 只有原围栏已无法通过，账本检查才足以排除尚未取锁的较早请求。
      "alert_stale",
      "stale_auction_state",
      "stale_lot_state",
      "stale_user_state",
      "rename_not_revertable",
      "mail_template_conflict",
      "runtime_switch_conflict",
      "already_disposed",
      "stale_settlement_return_state",
      "settlement_return_untrusted",
      "settlement_return_unbound",
      "settlement_return_assets_mismatch",
      "settlement_return_proof_required",
      "conflict",
    ],
  ],
];

const REJECTION_RULES = new Map(
  REJECTION_GROUPS.flatMap(([status, scope, codes]) =>
    codes.map((code) => [code, { status, scope }] as const),
  ),
);

// 寄售仅由 Solid 提供，裁定单独维护，不进入跨栈样本。
const CONSIGNMENT_DELIST_REJECTIONS = new Map<string, { status: number; scope: Rejection }>([
  ["invalid_reason", { status: 400, scope: "this_attempt" }],
  // 关闭门在任何存储写入前拒绝本次尝试。
  ["consignment_disabled", { status: 404, scope: "this_attempt" }],
  ["listing_not_found", { status: 404, scope: "replay_checked" }],
  ["stale_version", { status: 409, scope: "replay_checked" }],
  ["listing_not_active", { status: 409, scope: "replay_checked" }],
  ["command_id_conflict", { status: 409, scope: "replay_checked" }],
  // 订单处置（7E-2）：确定拒绝只占该次命令 ID，不留任何资金或状态痕迹，可凭回执核对。
  ["opposite_payment_open", { status: 409, scope: "replay_checked" }],
  ["evidence_unproven", { status: 409, scope: "replay_checked" }],
  ["evidence_conflict", { status: 409, scope: "replay_checked" }],
  ["hold_mismatch", { status: 409, scope: "replay_checked" }],
  ["source_mismatch", { status: 409, scope: "replay_checked" }],
  ["disposition_conflict", { status: 409, scope: "replay_checked" }],
  ["action_not_allowed", { status: 403, scope: "replay_checked" }],
]);

function consignmentDelistRejection(
  code: string,
): { status: number; scope: Rejection } | undefined {
  switch (code) {
    case "unauthenticated":
    case "origin_forbidden":
    case "csrf_invalid":
    case "rate_limited":
      return REJECTION_RULES.get(code);
    default:
      return CONSIGNMENT_DELIST_REJECTIONS.get(code);
  }
}

/** 供共用样本的完整性测试枚举，不参与裁定。 */
export function writeRejectionCatalog(): {
  readonly endpoints: readonly AdminWriteEndpoint[];
  readonly rules: ReadonlyArray<{
    readonly code: string;
    readonly status: number;
    readonly scope: Rejection;
  }>;
} {
  return {
    endpoints: ADMIN_WRITE_ENDPOINTS,
    rules: Array.from(REJECTION_RULES, ([code, { status, scope }]) => ({ code, status, scope })),
  };
}

function isVersionConflict(endpoint: AdminWriteEndpoint | null, code: string): boolean {
  return (
    code === "state_conflict" &&
    endpoint !== null &&
    adminWriteEndpointIsVersionedMutation(endpoint)
  );
}

/** 状态与协议码吻合后，判断拒绝覆盖的尝试范围。 */
export function rejectionScope(endpoint: AdminWriteEndpoint | null, code: string): Rejection {
  if (endpoint === "consignment-delist") {
    return consignmentDelistRejection(code)?.scope ?? "this_attempt";
  }
  return isVersionConflict(endpoint, code)
    ? "replay_checked"
    : (REJECTION_RULES.get(code)?.scope ?? "this_attempt");
}

/** 返回该端点和协议码能够证明拒绝时的 HTTP 状态。 */
export function rejectionStatus(endpoint: AdminWriteEndpoint | null, code: string): number | null {
  if (endpoint === "consignment-delist") {
    return consignmentDelistRejection(code)?.status ?? null;
  }
  return isVersionConflict(endpoint, code) ? 409 : (REJECTION_RULES.get(code)?.status ?? null);
}
