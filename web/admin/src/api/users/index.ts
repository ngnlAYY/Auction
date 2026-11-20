// Admin 用户管理 API 客户端。
// 浏览器只保存安全用户投影和当前表单内容；登录 key 的生成结果只在创建/重签响应里短暂
// 展示，不参与前端状态推导。

import type {
  AdminUserBlacklistRequest,
  AdminUserCreateRequest,
  AdminUserMutationRequest,
  AdminUserUnblacklistRequest,
} from "@auction/contracts/dto/admin/users";
import { err, ok, type Result, unreachable } from "@auction/web-shared/result";

import { parseRustUuid } from "../credentials/decode";
import {
  type AdminApiFailure,
  adminApiFailureAtWriteEndpoint,
  adminApiFailureClient,
  invalidResponseError,
  responseError,
} from "../failure";
import { validate as validatePagination, validPageEcho } from "../pagination";
import type { AdminSessionHost } from "../session";
import {
  type PreparedJsonCommand,
  prepareJsonCommandOnce,
  sendGet,
  sendPreparedJsonWrite,
} from "../transport";
import { decodeUserList, decodeUserMutation } from "./decode";
import type {
  AdminUser,
  AdminUserMutationResult,
  AdminUserPage,
  AdminUserStatusFilter,
} from "./types";

export type {
  AdminUser,
  AdminUserMutationResult,
  AdminUserPage,
  AdminUserStatusFilter,
} from "./types";
export { decodeUserMutation };

/** 状态筛选的线上取值；`all` 返回 null。 */
function adminUserStatusFilterAsQuery(
  status: AdminUserStatusFilter,
): "active" | "blacklisted" | null {
  switch (status) {
    case "all":
      return null;
    case "active":
      return "active";
    case "blacklisted":
      return "blacklisted";
    default:
      return unreachable(status);
  }
}

/** 获取 Admin 用户列表。 */
export async function fetchAdminUsers(
  host: AdminSessionHost,
  name: string,
  status: AdminUserStatusFilter,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<AdminUserPage, string>> {
  const bounds = validatePagination(page, pageSize, 200, 0x7fffffff);
  if (!bounds.ok) {
    return err(bounds.error);
  }
  const query = new URLSearchParams();
  query.append("page", page.toString());
  query.append("page_size", pageSize.toString());
  if (name !== "") {
    query.append("name", name);
  }
  const statusValue = adminUserStatusFilterAsQuery(status);
  if (statusValue !== null) {
    query.append("status", statusValue);
  }
  const response = await sendGet(host, `/api/v1/admin/users?${query.toString()}`, signal);
  if (!response.ok) {
    return err(response.error);
  }
  if (!responseOk(response.value.status)) {
    return err(await responseError(response.value, signal, host));
  }
  const pageResult = decodeUserList(response.value);
  if (!pageResult.ok) {
    return err(pageResult.error);
  }
  const pagination = pageResult.value.pagination;
  if (
    !validPageEcho(
      page,
      pageSize,
      pagination.page,
      pagination.page_size,
      pagination.total,
      pageResult.value.users.length,
    )
  ) {
    return err(invalidResponseError("user_pagination"));
  }
  return ok(pageResult.value);
}

/** 冻结创建用户的字段并构造一次命令正文。 */
export function prepareAdminUserCreate(
  commandId: string,
  username: string,
  forumAccountId: string | null,
  keyMode: string,
  key: string | null,
): AdminUserCreateRequest {
  return {
    command_id: commandId,
    username,
    forum_account_id: forumAccountId,
    key_mode: keyMode,
    key,
  };
}

/** 创建用户的已固定正文；`wantKey` 只在 `key_mode = generated` 时要求回执带 Key。 */
export interface PreparedAdminUserCreate {
  readonly prepared: PreparedJsonCommand;
  readonly wantKey: boolean;
}

/** 冻结创建用户的输入；同一 `command_id` 的重试不会重新拼装命令。 */
export function prepareAdminUserCreateOnce(
  username: string,
  forumAccountId: string | null,
  keyMode: string,
  key: string | null,
): (commandId: string) => Result<PreparedAdminUserCreate, AdminApiFailure> {
  const prepareCommand = prepareJsonCommandOnce((commandId) =>
    prepareAdminUserCreate(commandId, username, forumAccountId, keyMode, key),
  );
  const wantKey = keyMode === "generated";
  return (commandId) => {
    const prepared = prepareCommand(commandId);
    return prepared.ok ? ok({ prepared: prepared.value, wantKey }) : err(prepared.error);
  };
}

/** 创建用户；`key_mode = generated` 时返回一次性 key。 */
export async function createAdminUser(
  host: AdminSessionHost,
  commandId: string,
  prepare: (commandId: string) => Result<PreparedAdminUserCreate, AdminApiFailure>,
  signal: AbortSignal,
): Promise<Result<AdminUserMutationResult, string>> {
  const prepared = prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error.code);
  }
  const response = await sendPreparedJsonWrite(
    host,
    "/api/v1/admin/users",
    "POST",
    prepared.value.prepared,
    signal,
  );
  if (!response.ok) {
    return err(response.error.code);
  }
  return decodeUserMutation(host, response.value, prepared.value.wantKey, signal);
}

/** 冻结修改用户的字段并构造一次命令正文。 */
export function prepareAdminUserMutation(
  commandId: string,
  userId: string,
  expectedStateVersion: number,
  action: string,
  username: string | null,
  forumAccountId: string | null,
): AdminUserMutationRequest {
  return {
    command_id: commandId,
    user_id: userId,
    expected_state_version: expectedStateVersion,
    action,
    username,
    forum_account_id: forumAccountId,
  };
}

/** 修改用户的已固定正文；`wantKey` 只在重签 Key 时要求回执带 Key。 */
export interface PreparedAdminUserMutation {
  readonly prepared: PreparedJsonCommand;
  readonly wantKey: boolean;
}

/** 冻结用户、版本、动作与可选字段；无效用户 ID 在首次准备时失败。 */
export function prepareAdminUserMutationOnce(
  user: AdminUser,
  action: string,
  username: string | null,
  forumAccountId: string | null,
): (commandId: string) => Result<PreparedAdminUserMutation, AdminApiFailure> {
  const userId = parseRustUuid(user.id);
  const expectedStateVersion = user.state_version;
  const wantKey = action === "reissue_key";
  if (userId === null) {
    return () => err(adminApiFailureClient("invalid_request"));
  }
  const prepareCommand = prepareJsonCommandOnce((commandId) =>
    prepareAdminUserMutation(
      commandId,
      userId,
      expectedStateVersion,
      action,
      username,
      forumAccountId,
    ),
  );
  return (commandId) => {
    const prepared = prepareCommand(commandId);
    return prepared.ok ? ok({ prepared: prepared.value, wantKey }) : err(prepared.error);
  };
}

/** 修改用户名、论坛账号 ID 或登录 key 状态。 */
export async function mutateAdminUser(
  host: AdminSessionHost,
  commandId: string,
  prepare: (commandId: string) => Result<PreparedAdminUserMutation, AdminApiFailure>,
  signal: AbortSignal,
): Promise<Result<AdminUserMutationResult, string>> {
  const prepared = prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error.code);
  }
  const response = await sendPreparedJsonWrite(
    host,
    "/api/v1/admin/users",
    "PATCH",
    prepared.value.prepared,
    signal,
  );
  if (!response.ok) {
    return err(adminApiFailureAtWriteEndpoint(response.error, "user-mutation").code);
  }
  return decodeUserMutation(host, response.value, prepared.value.wantKey, signal);
}

/** 冻结拉黑命令的正文。 */
export function prepareAdminUserBlacklist(
  commandId: string,
  expectedStateVersion: number,
  publicReason: string,
): AdminUserBlacklistRequest {
  return {
    command_id: commandId,
    expected_state_version: expectedStateVersion,
    public_reason: publicReason,
  };
}

/** 拉黑用户所需的目标 URL 与已固定正文。 */
export interface PreparedAdminUserBlacklist {
  readonly url: string;
  readonly prepared: PreparedJsonCommand;
}

/** 冻结用户与公开理由；无效用户 ID 在首次准备时失败。 */
export function prepareAdminUserBlacklistOnce(
  user: AdminUser,
  publicReason: string,
): (commandId: string) => Result<PreparedAdminUserBlacklist, AdminApiFailure> {
  const userId = parseRustUuid(user.id);
  const expectedStateVersion = user.state_version;
  if (userId === null) {
    return () => err(adminApiFailureClient("invalid_request"));
  }
  const url = `/api/v1/admin/users/${userId}/blacklist`;
  const prepareCommand = prepareJsonCommandOnce((commandId) =>
    prepareAdminUserBlacklist(commandId, expectedStateVersion, publicReason),
  );
  return (commandId) => {
    const prepared = prepareCommand(commandId);
    return prepared.ok ? ok({ url, prepared: prepared.value }) : err(prepared.error);
  };
}

/** 将用户加入黑名单。 */
export async function blacklistAdminUser(
  host: AdminSessionHost,
  commandId: string,
  prepare: (commandId: string) => Result<PreparedAdminUserBlacklist, AdminApiFailure>,
  signal: AbortSignal,
): Promise<Result<AdminUser, string>> {
  const prepared = prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error.code);
  }
  const response = await sendPreparedJsonWrite(
    host,
    prepared.value.url,
    "PUT",
    prepared.value.prepared,
    signal,
  );
  if (!response.ok) {
    return err(adminApiFailureAtWriteEndpoint(response.error, "user-mutation").code);
  }
  const result = await decodeUserMutation(host, response.value, false, signal);
  return result.ok ? ok(result.value.user) : err(result.error);
}

/** 冻结解除拉黑命令的正文。 */
export function prepareAdminUserUnblacklist(
  commandId: string,
  expectedStateVersion: number,
): AdminUserUnblacklistRequest {
  return {
    command_id: commandId,
    expected_state_version: expectedStateVersion,
  };
}

/** 解除拉黑所需的目标 URL 与已固定正文。 */
export interface PreparedAdminUserUnblacklist {
  readonly url: string;
  readonly prepared: PreparedJsonCommand;
}

/** 冻结用户与版本；无效用户 ID 在首次准备时失败。 */
export function prepareAdminUserUnblacklistOnce(
  user: AdminUser,
): (commandId: string) => Result<PreparedAdminUserUnblacklist, AdminApiFailure> {
  const userId = parseRustUuid(user.id);
  const expectedStateVersion = user.state_version;
  if (userId === null) {
    return () => err(adminApiFailureClient("invalid_request"));
  }
  const url = `/api/v1/admin/users/${userId}/blacklist`;
  const prepareCommand = prepareJsonCommandOnce((commandId) =>
    prepareAdminUserUnblacklist(commandId, expectedStateVersion),
  );
  return (commandId) => {
    const prepared = prepareCommand(commandId);
    return prepared.ok ? ok({ url, prepared: prepared.value }) : err(prepared.error);
  };
}

/** 将用户移出黑名单。 */
export async function unblacklistAdminUser(
  host: AdminSessionHost,
  commandId: string,
  prepare: (commandId: string) => Result<PreparedAdminUserUnblacklist, AdminApiFailure>,
  signal: AbortSignal,
): Promise<Result<AdminUser, string>> {
  const prepared = prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error.code);
  }
  const response = await sendPreparedJsonWrite(
    host,
    prepared.value.url,
    "DELETE",
    prepared.value.prepared,
    signal,
  );
  if (!response.ok) {
    return err(adminApiFailureAtWriteEndpoint(response.error, "user-mutation").code);
  }
  const result = await decodeUserMutation(host, response.value, false, signal);
  return result.ok ? ok(result.value.user) : err(result.error);
}

/** 只接受 2xx 响应，与 gloo_net 的 `response.ok()` 一致。 */
function responseOk(status: number): boolean {
  return status >= 200 && status <= 299;
}
