// Public 账户、代理授权、会话、注册信息、世界状态、规则与黑名单 DTO。

import * as v from "valibot";
import {
  integer,
  optionalNullable,
  requiredNullable,
  strictStruct,
  stringMap,
  struct,
} from "../../wire";

/** Public 普通竞价结果中的公开 lot 状态。 */
export const PlaceBidResultResponseSchema = struct({
  lot_id: v.string(),
  current_price: integer,
  effective_end_at: v.string(),
  state_version: integer,
  leading: v.boolean(),
});

export type PlaceBidResultResponse = v.InferOutput<typeof PlaceBidResultResponseSchema>;

/** Public 普通竞价成功后的稳定结果；同一 `command_id` 重放返回同一份 lot 投影。 */
export const PlaceBidResponseSchema = struct({
  bid: PlaceBidResultResponseSchema,
});

export type PlaceBidResponse = v.InferOutput<typeof PlaceBidResponseSchema>;

/** 账户投影中的最小装备信息。 */
export const PublicOwnedEquipmentResponseSchema = struct({
  name: v.string(),
  category: v.string(),
});

export type PublicOwnedEquipmentResponse = v.InferOutput<typeof PublicOwnedEquipmentResponseSchema>;

export const PublicOwnedBidResponseSchema = struct({
  id: v.string(),
  auction_id: v.string(),
  lot_id: v.string(),
  lot_number: integer,
  amount: integer,
  accepted_state_version: integer,
  fact_index: integer,
  mechanism: v.string(),
  created_at: v.string(),
  leading: v.boolean(),
  equipment: PublicOwnedEquipmentResponseSchema,
});

export type PublicOwnedBidResponse = v.InferOutput<typeof PublicOwnedBidResponseSchema>;

/** 账户代理授权 quota。 */
export const PublicOwnedProxyQuotaResponseSchema = struct({
  used: integer,
  limit: integer,
});

export type PublicOwnedProxyQuotaResponse = v.InferOutput<
  typeof PublicOwnedProxyQuotaResponseSchema
>;

/** 账户代理授权的历史转换事实。 */
export const PublicOwnedProxyTransitionResponseSchema = struct({
  transition_number: integer,
  kind: v.string(),
  old_maximum_amount: optionalNullable(integer),
  new_maximum_amount: integer,
  old_mode: optionalNullable(v.string()),
  new_mode: v.string(),
  old_status: optionalNullable(v.string()),
  new_status: v.string(),
  reason: optionalNullable(v.string()),
  resulting_state_version: integer,
  created_at: v.string(),
});

export type PublicOwnedProxyTransitionResponse = v.InferOutput<
  typeof PublicOwnedProxyTransitionResponseSchema
>;

/** 账户代理授权当前可执行能力。 */
export const PublicOwnedProxyCapabilitiesResponseSchema = struct({
  can_lower: v.boolean(),
  waiting: v.boolean(),
  at_maximum: v.boolean(),
});

export type PublicOwnedProxyCapabilitiesResponse = v.InferOutput<
  typeof PublicOwnedProxyCapabilitiesResponseSchema
>;

/** 当前用户自己的代理授权投影。 */
export const PublicOwnedProxyResponseSchema = struct({
  authorization_id: v.string(),
  auction_id: v.string(),
  lot_id: v.string(),
  lot_number: integer,
  authorization_sequence: integer,
  usage_number: integer,
  mode: v.string(),
  maximum_amount: integer,
  quota: PublicOwnedProxyQuotaResponseSchema,
  transitions: v.array(PublicOwnedProxyTransitionResponseSchema),
  status: v.string(),
  terminal_reason: optionalNullable(v.string()),
  state_version: integer,
  created_at: v.string(),
  updated_at: v.string(),
  equipment: PublicOwnedEquipmentResponseSchema,
  bid: optionalNullable(PublicOwnedBidResponseSchema),
  invalid_reason: optionalNullable(v.string()),
  capabilities: PublicOwnedProxyCapabilitiesResponseSchema,
});

export type PublicOwnedProxyResponse = v.InferOutput<typeof PublicOwnedProxyResponseSchema>;

/** 已发布代理命令响应的最小公开字段；不携带账户页专属的拍卖、装备、时间和 capability 字段，
 * 也不把命令账本的原始结果暴露给浏览器。 */
export const PublicProxyCommandBidResponseSchema = struct({
  lot_id: v.string(),
  current_price: integer,
  effective_end_at: v.string(),
  state_version: integer,
  leading: v.boolean(),
});

export type PublicProxyCommandBidResponse = v.InferOutput<
  typeof PublicProxyCommandBidResponseSchema
>;

/** Public 代理命令成功后的授权投影。 */
export const PublicProxyCommandAuthorizationResponseSchema = struct({
  authorization_id: v.string(),
  lot_id: v.string(),
  authorization_sequence: integer,
  usage_number: integer,
  mode: v.string(),
  maximum_amount: integer,
  quota: PublicOwnedProxyQuotaResponseSchema,
  transitions: v.array(PublicOwnedProxyTransitionResponseSchema),
  status: v.string(),
  terminal_reason: optionalNullable(v.string()),
  state_version: integer,
  bid: optionalNullable(PublicProxyCommandBidResponseSchema),
});

export type PublicProxyCommandAuthorizationResponse = v.InferOutput<
  typeof PublicProxyCommandAuthorizationResponseSchema
>;

export const PublicProxyCommandResponseSchema = struct({
  authorization: PublicProxyCommandAuthorizationResponseSchema,
});

export type PublicProxyCommandResponse = v.InferOutput<typeof PublicProxyCommandResponseSchema>;

/** 当前用户竞价与代理历史列表响应。 */
export const PublicMyBidsResponseSchema = struct({
  bids: v.array(PublicOwnedBidResponseSchema),
  proxies: v.array(PublicOwnedProxyResponseSchema),
  bid_total: integer,
  proxy_total: integer,
  page: integer,
  bid_page: integer,
  proxy_page: integer,
  page_size: integer,
});

export type PublicMyBidsResponse = v.InferOutput<typeof PublicMyBidsResponseSchema>;

export const PublicMyProxyResponseSchema = struct({
  authorization: optionalNullable(PublicOwnedProxyResponseSchema),
});

export type PublicMyProxyResponse = v.InferOutput<typeof PublicMyProxyResponseSchema>;

/** Public 注册说明中的配置账号；不包含凭据，也不表示远端邮箱已通过可用性验证。 */
export const PublicRegistrationInfoResponseSchema = strictStruct({
  bot_players: v.array(v.string()),
});

export type PublicRegistrationInfoResponse = v.InferOutput<
  typeof PublicRegistrationInfoResponseSchema
>;

/** Public 登录请求；原始 key 只在 HTTP 到应用端口的短路径中存在，调试输出必须脱敏。 */
export interface PublicLoginRequest {
  key: string;
}

/** Public 登录结果；两个截止字段同时为 null 表示可撤销的永久会话。 */
export const PublicLoginResponseSchema = struct({
  user_id: v.string(),
  username: v.string(),
  status: v.string(),
  expires_at: requiredNullable(v.string()),
  absolute_expires_at: requiredNullable(v.string()),
});

export type PublicLoginResponse = v.InferOutput<typeof PublicLoginResponseSchema>;

/** Public 权威会话；永久会话仍受账号、凭据和撤销状态约束。 */
export const PublicSessionCurrentResponseSchema = struct({
  authenticated: v.boolean(),
  user_id: v.string(),
  username: v.string(),
  expires_at: requiredNullable(v.string()),
  absolute_expires_at: requiredNullable(v.string()),
});

export type PublicSessionCurrentResponse = v.InferOutput<typeof PublicSessionCurrentResponseSchema>;

/** 账号后续登录时长及当前会话实际截止；偏好与现有其他设备会话相互独立。 */
export const PublicSessionPreferenceResponseSchema = strictStruct({
  user_id: v.string(),
  duration_seconds: requiredNullable(integer),
  version: integer,
  expires_at: requiredNullable(v.string()),
});

export type PublicSessionPreferenceResponse = v.InferOutput<
  typeof PublicSessionPreferenceResponseSchema
>;

/** 更新当前会话与后续登录时长；必须显式提交 null 才选择永久，缺字段不能扩大有效期。 */
export interface PublicSetSessionPreferenceRequest {
  command_id: string;
  duration_seconds: number | null;
  expected_version: number;
}

/** Public 黑名单响应中的可公开原因。 */
export const PublicAccountBlockedResponseSchema = struct({
  public_reason: v.string(),
});

export type PublicAccountBlockedResponse = v.InferOutput<typeof PublicAccountBlockedResponseSchema>;

/** Public 世界条目的稳定响应。 */
export const WorldStatusResponseSchema = struct({
  enabled: v.boolean(),
});

export type WorldStatusResponse = v.InferOutput<typeof WorldStatusResponseSchema>;

/** Public 世界目录响应，保持已发布的以世界 ID 为 key 的 JSON 结构。 */
export const WorldsResponseSchema = struct({
  worlds: stringMap(WorldStatusResponseSchema),
});

export type WorldsResponse = v.InferOutput<typeof WorldsResponseSchema>;

export const PublicRulesResponseSchema = struct({
  locale: v.string(),
  title: v.string(),
  markdown: v.string(),
  revision: integer,
  published_at: v.string(),
});

export type PublicRulesResponse = v.InferOutput<typeof PublicRulesResponseSchema>;

/** Public 黑名单条目的稳定响应。 */
export const PublicBlacklistEntryResponseSchema = struct({
  username: v.string(),
  reason: v.string(),
  blacklisted_at: v.string(),
});

export type PublicBlacklistEntryResponse = v.InferOutput<typeof PublicBlacklistEntryResponseSchema>;

/** Public 黑名单分页响应，保持已发布的 `users` 字段命名。 */
export const PublicBlacklistResponseSchema = struct({
  users: v.array(PublicBlacklistEntryResponseSchema),
  total: integer,
  page: integer,
  page_size: integer,
});

export type PublicBlacklistResponse = v.InferOutput<typeof PublicBlacklistResponseSchema>;
