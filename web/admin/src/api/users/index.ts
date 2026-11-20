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
  type AdminWriteFailure,
  type AdminWriteNotSent,
  invalidResponseError,
  responseError,
  writeNotSent,
  writeOutcomeUnknown,
} from "../failure";
import { validate as validatePagination, validPageEcho } from "../pagination";
import type { AdminSessionHost } from "../session";
import {
  type PreparedJsonCommand,
  prepareJsonCommandOnce,
  sendGet,
  sendPreparedJsonWrite,
} from "../transport";
import { decodeUserList, decodeUserMutation, matchesUserMutationTarget } from "./decode";
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

/** 创建用户的已固定正文与可回显的字段。 */
export interface PreparedAdminUserCreate {
  readonly prepared: PreparedJsonCommand;
  readonly wantKey: boolean;
  readonly username: string;
  readonly forumAccountId: string | null;
}

/** 冻结创建用户的输入；同一 `command_id` 的重试不会重新拼装命令。 */
export function prepareAdminUserCreateOnce(
  username: string,
  forumAccountId: string | null,
  keyMode: string,
  key: string | null,
): (commandId: string) => Result<PreparedAdminUserCreate, AdminWriteNotSent> {
  const prepareCommand = prepareJsonCommandOnce((commandId) =>
    prepareAdminUserCreate(commandId, username, forumAccountId, keyMode, key),
  );
  const wantKey = keyMode === "generated";
  return (commandId) => {
    const prepared = prepareCommand(commandId);
    return prepared.ok
      ? ok({ prepared: prepared.value, wantKey, username, forumAccountId })
      : err(prepared.error);
  };
}

/** 创建用户；`key_mode = generated` 时返回一次性 key。 */
export async function createAdminUser(
  host: AdminSessionHost,
  commandId: string,
  prepare: (commandId: string) => Result<PreparedAdminUserCreate, AdminWriteNotSent>,
  signal: AbortSignal,
): Promise<Result<AdminUserMutationResult, AdminWriteFailure>> {
  const prepared = prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const response = await sendPreparedJsonWrite(
    host,
    { url: "/api/v1/admin/users", method: "POST", endpoint: "general" },
    prepared.value.prepared,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const result = await decodeUserMutation(host, response.value, prepared.value.wantKey, signal);
  if (!result.ok) {
    return result;
  }
  // 创建没有预知的用户 ID；核对冻结表单和初始投影后才可交付 Key。
  const user = result.value.user;
  if (
    user.username !== prepared.value.username ||
    user.forum_account_id !== prepared.value.forumAccountId ||
    user.status !== "active" ||
    user.key_status !== "active" ||
    user.state_version !== 1
  ) {
    return err(writeOutcomeUnknown(invalidResponseError("user_create_receipt")));
  }
  return result;
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

/** 修改用户的已固定正文与回执核对字段。 */
export interface PreparedAdminUserMutation {
  readonly prepared: PreparedJsonCommand;
  readonly userId: string;
  readonly expectedStateVersion: number;
  readonly action: string;
  readonly username: string | null;
  readonly forumAccountId: string | null;
}

/** 冻结用户、版本、动作与可选字段；无效用户 ID 在首次准备时失败。 */
export function prepareAdminUserMutationOnce(
  user: AdminUser,
  action: string,
  username: string | null,
  forumAccountId: string | null,
): (commandId: string) => Result<PreparedAdminUserMutation, AdminWriteNotSent> {
  const userId = parseRustUuid(user.id);
  const expectedStateVersion = user.state_version;
  if (userId === null) {
    return () => err(writeNotSent("invalid_request"));
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
    return prepared.ok
      ? ok({
          prepared: prepared.value,
          userId,
          expectedStateVersion,
          action,
          username,
          forumAccountId,
        })
      : err(prepared.error);
  };
}

/** 修改用户名、论坛账号 ID 或登录 key 状态。 */
export async function mutateAdminUser(
  host: AdminSessionHost,
  commandId: string,
  prepare: (commandId: string) => Result<PreparedAdminUserMutation, AdminWriteNotSent>,
  signal: AbortSignal,
): Promise<Result<AdminUserMutationResult, AdminWriteFailure>> {
  const prepared = prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const response = await sendPreparedJsonWrite(
    host,
    { url: "/api/v1/admin/users", method: "PATCH", endpoint: "user-mutation" },
    prepared.value.prepared,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const { userId, expectedStateVersion, action } = prepared.value;
  const result = await decodeUserMutation(host, response.value, action === "reissue_key", signal);
  if (!result.ok) {
    return result;
  }
  const user = result.value.user;
  if (
    !matchesUserMutationTarget(user, userId, expectedStateVersion) ||
    !matchesUserMutationAction(user, prepared.value)
  ) {
    return err(writeOutcomeUnknown(invalidResponseError("user_mutation_receipt")));
  }
  return result;
}

function matchesUserMutationAction(user: AdminUser, command: PreparedAdminUserMutation): boolean {
  switch (command.action) {
    case "rename":
      return user.username === command.username;
    case "set_forum_account_id":
      return command.forumAccountId !== null && user.forum_account_id === command.forumAccountId;
    case "revoke_key":
      return user.key_status === "revoked";
    case "reissue_key":
      return user.key_status === "active";
    default:
      return false;
  }
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

/** 拉黑用户所需的目标 URL、已固定正文与回执核对字段。 */
export interface PreparedAdminUserBlacklist {
  readonly url: string;
  readonly prepared: PreparedJsonCommand;
  readonly userId: string;
  readonly expectedStateVersion: number;
}

/** 冻结用户与公开理由；无效用户 ID 在首次准备时失败。 */
export function prepareAdminUserBlacklistOnce(
  user: AdminUser,
  publicReason: string,
): (commandId: string) => Result<PreparedAdminUserBlacklist, AdminWriteNotSent> {
  const userId = parseRustUuid(user.id);
  const expectedStateVersion = user.state_version;
  if (userId === null) {
    return () => err(writeNotSent("invalid_request"));
  }
  const url = `/api/v1/admin/users/${userId}/blacklist`;
  const prepareCommand = prepareJsonCommandOnce((commandId) =>
    prepareAdminUserBlacklist(commandId, expectedStateVersion, publicReason),
  );
  return (commandId) => {
    const prepared = prepareCommand(commandId);
    return prepared.ok
      ? ok({ url, prepared: prepared.value, userId, expectedStateVersion })
      : err(prepared.error);
  };
}

/** 将用户加入黑名单。 */
export async function blacklistAdminUser(
  host: AdminSessionHost,
  commandId: string,
  prepare: (commandId: string) => Result<PreparedAdminUserBlacklist, AdminWriteNotSent>,
  signal: AbortSignal,
): Promise<Result<AdminUser, AdminWriteFailure>> {
  const prepared = prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const response = await sendPreparedJsonWrite(
    host,
    { url: prepared.value.url, method: "PUT", endpoint: "user-mutation" },
    prepared.value.prepared,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const result = await decodeUserMutation(host, response.value, false, signal);
  if (!result.ok) {
    return result;
  }
  const user = result.value.user;
  if (
    user.status !== "blacklisted" ||
    !matchesUserMutationTarget(user, prepared.value.userId, prepared.value.expectedStateVersion)
  ) {
    return err(writeOutcomeUnknown(invalidResponseError("user_blacklist_receipt")));
  }
  return ok(user);
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

/** 解除拉黑所需的目标 URL、已固定正文与回执核对字段。 */
export interface PreparedAdminUserUnblacklist {
  readonly url: string;
  readonly prepared: PreparedJsonCommand;
  readonly userId: string;
  readonly expectedStateVersion: number;
}

/** 冻结用户与版本；无效用户 ID 在首次准备时失败。 */
export function prepareAdminUserUnblacklistOnce(
  user: AdminUser,
): (commandId: string) => Result<PreparedAdminUserUnblacklist, AdminWriteNotSent> {
  const userId = parseRustUuid(user.id);
  const expectedStateVersion = user.state_version;
  if (userId === null) {
    return () => err(writeNotSent("invalid_request"));
  }
  const url = `/api/v1/admin/users/${userId}/blacklist`;
  const prepareCommand = prepareJsonCommandOnce((commandId) =>
    prepareAdminUserUnblacklist(commandId, expectedStateVersion),
  );
  return (commandId) => {
    const prepared = prepareCommand(commandId);
    return prepared.ok
      ? ok({ url, prepared: prepared.value, userId, expectedStateVersion })
      : err(prepared.error);
  };
}

/** 将用户移出黑名单。 */
export async function unblacklistAdminUser(
  host: AdminSessionHost,
  commandId: string,
  prepare: (commandId: string) => Result<PreparedAdminUserUnblacklist, AdminWriteNotSent>,
  signal: AbortSignal,
): Promise<Result<AdminUser, AdminWriteFailure>> {
  const prepared = prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const response = await sendPreparedJsonWrite(
    host,
    { url: prepared.value.url, method: "DELETE", endpoint: "user-mutation" },
    prepared.value.prepared,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const result = await decodeUserMutation(host, response.value, false, signal);
  if (!result.ok) {
    return result;
  }
  const user = result.value.user;
  if (
    user.status !== "active" ||
    !matchesUserMutationTarget(user, prepared.value.userId, prepared.value.expectedStateVersion)
  ) {
    return err(writeOutcomeUnknown(invalidResponseError("user_unblacklist_receipt")));
  }
  return ok(user);
}

/** 只接受 2xx 响应，与 gloo_net 的 `response.ok()` 一致。 */
function responseOk(status: number): boolean {
  return status >= 200 && status <= 299;
}
