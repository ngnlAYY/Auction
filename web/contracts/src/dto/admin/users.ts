// Admin 用户、黑名单与改名历史 DTO。

import * as v from "valibot";
import { integer, optionalNullable, strictStruct, struct } from "../../wire";
import type { AdminPaginationResponse } from "./pagination";
import { AdminPaginationResponseSchema } from "./pagination";

/** Admin 用户创建请求；`key` 仅在手工 key 模式提供，服务端不会把它写入日志或命令账本。
 * `command_id`、`username`、`key_mode` 缺失或 null 由后端读作零值，再由命令专属校验报错。 */
export interface AdminUserCreateRequest {
  command_id: string;
  username: string;
  forum_account_id: string | null;
  key_mode: string;
  key: string | null;
}

/** Admin 用户修改请求；`command_id`、`user_id`、`expected_state_version`、`action`
 * 缺失或 null 由后端读作零值。 */
export interface AdminUserMutationRequest {
  command_id: string;
  user_id: string;
  expected_state_version: number;
  action: string;
  username: string | null;
  forum_account_id: string | null;
}

/** Admin 用户封禁请求；`command_id`、`expected_state_version`、`public_reason`
 * 缺失或 null 由后端读作零值。 */
export interface AdminUserBlacklistRequest {
  command_id: string;
  expected_state_version: number;
  public_reason: string;
}

/** Admin 用户解除封禁请求；`command_id` 与 `expected_state_version` 缺失或 null 由后端读作零值。 */
export interface AdminUserUnblacklistRequest {
  command_id: string;
  expected_state_version: number;
}

/** Admin 改名回滚请求；`command_id` 与 `expected_state_version` 缺失或 null 由后端读作零值。 */
export interface AdminRenameRevertRequest {
  command_id: string;
  expected_state_version: number;
}

/** Admin 用户安全响应。 */
export const AdminUserResponseEntries = {
  id: v.string(),
  username: v.string(),
  forum_account_id: optionalNullable(v.string()),
  status: v.string(),
  key_status: v.string(),
  state_version: integer,
  created_at: v.string(),
  updated_at: v.string(),
};

export const AdminUserResponseSchema = struct(AdminUserResponseEntries);

export type AdminUserResponse = v.InferOutput<typeof AdminUserResponseSchema>;

/** Admin 用户列表数字分页。 */
export type AdminUserPaginationResponse = AdminPaginationResponse;

/** Admin 用户列表中的黑名单历史安全投影。 */
export const AdminUserBlacklistHistoryResponseSchema = struct({
  public_reason: v.string(),
  blacklisted_at: v.string(),
});

export type AdminUserBlacklistHistoryResponse = v.InferOutput<
  typeof AdminUserBlacklistHistoryResponseSchema
>;

/** Admin 用户列表条目；写命令响应仍只使用 `AdminUserResponse`。 */
export const AdminUserListItemResponseSchema = struct({
  ...AdminUserResponseEntries,
  blacklist_count: integer,
  blacklist_history: v.pipe(v.array(AdminUserBlacklistHistoryResponseSchema), v.readonly()),
});

export type AdminUserListItemResponse = v.InferOutput<typeof AdminUserListItemResponseSchema>;

export const AdminUserListResponseSchema = struct({
  users: v.pipe(v.array(AdminUserListItemResponseSchema), v.readonly()),
  pagination: AdminPaginationResponseSchema,
});

export type AdminUserListResponse = v.InferOutput<typeof AdminUserListResponseSchema>;

/** Admin 用户修改响应；生成 key 只在本次创建/重签响应中出现，调试输出必须脱敏。 */
export const AdminUserMutationResponseSchema = struct({
  user: AdminUserResponseSchema,
  generated_key: optionalNullable(v.string()),
});

export type AdminUserMutationResponse = v.InferOutput<typeof AdminUserMutationResponseSchema>;

/** 用户写回执的旧 decoder 闭集镜像；`user` 的未知字段同样拒绝。 */
const AdminStrictUserResponseSchema = strictStruct(AdminUserResponseEntries);

/** 创建（`key_mode = "generated"`）与重签 key 的成功回执：`generated_key` 必须出现且为
 * 字符串，显式 `null` 或缺失都会失败；值的字符集与长度仍由用户 API 车道校验。接口层解析
 * 这类回执时必须传入下面的字段策略。 */
export const AdminUserMutationResponseWithKeySchema = strictStruct({
  user: AdminStrictUserResponseSchema,
  generated_key: v.string(),
});

/** 解析 `AdminUserMutationResponseWithKeySchema` 的创建/重签命令回执必须传入的字段策略：
 * 任意一层出现重复键，或数字使用小数、指数与 `-0` 词法时整份响应失败。 */
export const ADMIN_USER_MUTATION_WITH_KEY_RESPONSE_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;

export type AdminUserMutationResponseWithKey = v.InferOutput<
  typeof AdminUserMutationResponseWithKeySchema
>;

/** 其他用户写命令（修改、封禁、解封、改名回滚）的成功回执：`generated_key` 必须缺席，
 * 显式 `null` 也会作为未知字段失败。接口层解析这类回执时必须传入下面的字段策略。 */
export const AdminUserMutationResponseWithoutKeySchema = strictStruct({
  user: AdminStrictUserResponseSchema,
});

/** 解析 `AdminUserMutationResponseWithoutKeySchema` 的其他用户写命令回执必须传入的字段
 * 策略：任意一层出现重复键，或数字使用小数、指数与 `-0` 词法时整份响应失败。 */
export const ADMIN_USER_MUTATION_WITHOUT_KEY_RESPONSE_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;

export type AdminUserMutationResponseWithoutKey = v.InferOutput<
  typeof AdminUserMutationResponseWithoutKeySchema
>;

/** Admin 改名历史安全投影。 */
export const AdminRenameResponseSchema = struct({
  id: v.string(),
  user_id: v.string(),
  user_state_version: integer,
  previous_username: v.string(),
  next_username: v.string(),
  actor: v.string(),
  created_at: v.string(),
  reverted_at: optionalNullable(v.string()),
});

export type AdminRenameResponse = v.InferOutput<typeof AdminRenameResponseSchema>;

/** Admin 改名历史数字分页。 */
export type AdminRenamePaginationResponse = AdminPaginationResponse;

/** Admin 改名历史列表响应。 */
export const AdminRenameListResponseSchema = struct({
  renames: v.pipe(v.array(AdminRenameResponseSchema), v.readonly()),
  pagination: AdminPaginationResponseSchema,
});

export type AdminRenameListResponse = v.InferOutput<typeof AdminRenameListResponseSchema>;
