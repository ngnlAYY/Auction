// credentials 相关 DTO。

import * as v from "valibot";
import { integer, optionalNullable, requiredNullable, strictStruct } from "../../wire";
import type { AdminStrictPaginationResponse } from "./pagination";
import { AdminStrictPaginationResponseSchema } from "./pagination";

/** Admin 凭据导入请求；Cookie 与登录输入互斥，敏感字段只在本次请求内存在。 */
export interface AdminCredentialCreateRequest {
  command_id: string;
  kind: string;
  world: string;
  label: string;
  /** 可选预期完整玩家名称，仅 mail_cookie 接受；不是身份证明。 */
  player_name?: string;
  /** 后端在空串时省略该键；请用 `buildAdminCredentialCreateRequest` 构造请求，不能发送
   * `"secret": ""`。 */
  secret?: string;
  login?: AdminCredentialLoginRequest;
}

/** 论坛账号登录输入；响应只包含导入后的凭据安全投影，不返回 Cookie。 */
export interface AdminCredentialLoginRequest {
  username: string;
  password: string;
}

/** 构造凭据导入请求；`secret` 为空串时省略该键（Rust 的 `skip_serializing_if =
 * "String::is_empty"`），其余可选字段只在有值时写入，与 Rust 的 `Option` 省略规则一致。 */
export function buildAdminCredentialCreateRequest(
  input: AdminCredentialCreateRequest,
): AdminCredentialCreateRequest {
  const request: AdminCredentialCreateRequest = {
    command_id: input.command_id,
    kind: input.kind,
    world: input.world,
    label: input.label,
  };
  if (input.player_name !== undefined) {
    request.player_name = input.player_name;
  }
  if (input.secret !== undefined && input.secret !== "") {
    if (input.login !== undefined) {
      // 契约写明 Cookie 与论坛登录互斥；不能等到服务端 invalid_credential_request 才发现。
      throw new TypeError("凭据导入请求不能同时提供 Cookie 与论坛登录");
    }
    request.secret = input.secret;
  }
  if (input.login !== undefined) {
    request.login = input.login;
  }
  return request;
}

/** Admin 凭据改名或状态修改请求。 */
export interface AdminCredentialMutationRequest {
  command_id: string;
  credential_id: string;
  expected_state_version: number;
  action: string;
  label: string | null;
  status: string | null;
}

/** 已保存凭据的只读校验请求；不接受 Cookie、URL 或状态修改。 */
export interface AdminCredentialCheckRequest {
  expected_state_version: number;
}

/** 一个范围的认证观测：status 仅为 valid/invalid/inconclusive。 */
export const AdminCredentialCheckScopeResponseSchema = strictStruct({
  world: v.string(),
  status: v.string(),
});

/** 一个范围的认证观测：status 仅为 valid/invalid/inconclusive。 */
export type AdminCredentialCheckScopeResponse = v.InferOutput<
  typeof AdminCredentialCheckScopeResponseSchema
>;

/** 瞬时检查结果；不持久化、不改变凭据 active/disabled/invalid 状态。 */
export const AdminCredentialCheckResponseSchema = strictStruct({
  credential_id: v.string(),
  state_version: integer,
  checked_at: v.string(),
  checks: v.pipe(v.array(AdminCredentialCheckScopeResponseSchema), v.readonly()),
});

/** 瞬时检查结果；不持久化、不改变凭据 active/disabled/invalid 状态。 */
export type AdminCredentialCheckResponse = v.InferOutput<typeof AdminCredentialCheckResponseSchema>;

/** Admin 凭据删除请求。 */
export interface AdminCredentialDeleteRequest {
  command_id: string;
  credential_id: string;
  expected_state_version: number;
}

/** Admin 凭据安全响应；不含秘密及指纹，`rotated_at` 必须显式出现但可为 `null`。 */
export const AdminCredentialResponseSchema = strictStruct({
  id: v.string(),
  kind: v.string(),
  world: v.string(),
  label: v.string(),
  player_name: optionalNullable(v.string()),
  status: v.string(),
  state_version: integer,
  created_at: v.string(),
  rotated_at: requiredNullable(v.string()),
  updated_at: v.string(),
});

/** Admin 凭据安全响应；不含秘密及指纹，`rotated_at` 必须显式出现但可为 `null`。 */
export type AdminCredentialResponse = v.InferOutput<typeof AdminCredentialResponseSchema>;

/** Admin 凭据列表分页。 */
export const AdminCredentialPaginationResponseSchema = AdminStrictPaginationResponseSchema;

/** Admin 凭据列表分页。 */
export type AdminCredentialPaginationResponse = AdminStrictPaginationResponse;

/** Admin 凭据列表响应。 */
export const AdminCredentialListResponseSchema = strictStruct({
  credentials: v.pipe(v.array(AdminCredentialResponseSchema), v.readonly()),
  pagination: AdminCredentialPaginationResponseSchema,
  restart_required: v.boolean(),
});

/** Admin 凭据列表响应。 */
export type AdminCredentialListResponse = v.InferOutput<typeof AdminCredentialListResponseSchema>;

/** Admin 凭据导入/修改响应。 */
export const AdminCredentialMutationResponseSchema = strictStruct({
  credential: AdminCredentialResponseSchema,
  restart_required: v.boolean(),
});

/** 解析 `AdminCredentialMutationResponseSchema` 的凭据导入/修改命令回执必须传入的字段策略：
 * 任意一层出现重复键，或数字使用小数、指数与 `-0` 词法时整份响应失败。 */
export const ADMIN_CREDENTIAL_MUTATION_RESPONSE_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;

/** Admin 凭据导入/修改响应。 */
export type AdminCredentialMutationResponse = v.InferOutput<
  typeof AdminCredentialMutationResponseSchema
>;

/** Admin 凭据删除响应。 */
export const AdminCredentialDeleteResponseSchema = strictStruct({
  credential_id: v.string(),
  state_version: integer,
  restart_required: v.boolean(),
});

/** 解析 `AdminCredentialDeleteResponseSchema` 的删除命令回执必须传入的字段策略：任意一层
 * 出现重复键，或数字使用小数、指数与 `-0` 词法时整份响应失败，与 Rust 直接反序列化
 * `state_version: i64` 一致。 */
export const ADMIN_CREDENTIAL_DELETE_RESPONSE_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;

/** Admin 凭据删除响应。 */
export type AdminCredentialDeleteResponse = v.InferOutput<
  typeof AdminCredentialDeleteResponseSchema
>;
