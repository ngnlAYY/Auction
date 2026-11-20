// Admin 用户与黑名单回执的严格解码；contracts 的 flatten 组合无法表达旧闭集，故在此保留镜像。

import type {
  AdminUserBlacklistHistoryResponse,
  AdminUserPaginationResponse,
  AdminUserResponse,
} from "@auction/contracts/dto/admin/users";
import {
  ADMIN_USER_MUTATION_WITH_KEY_RESPONSE_FIELD_POLICY,
  ADMIN_USER_MUTATION_WITHOUT_KEY_RESPONSE_FIELD_POLICY,
  AdminUserMutationResponseWithKeySchema,
  AdminUserMutationResponseWithoutKeySchema,
  AdminUserResponseEntries,
} from "@auction/contracts/dto/admin/users";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { integer, strictStruct } from "@auction/contracts/wire";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";
import * as v from "valibot";

import { decodeAdminReceipt, invalidResponseError } from "../failure";
import { validResponse } from "../pagination";
import { type AdminSessionHost, adminSessionHostLive } from "../session";
import {
  MAX_JSON_SAFE_INTEGER,
  parseAdminTime,
  validAdminTime,
  validSafeText,
} from "../validation";
import type { AdminUser, AdminUserMutationResult, AdminUserPage } from "./types";

// 保留镜像的理由：contracts 的 `AdminUserResponse` 被 `AdminUserListItemResponse` 以
// `#[serde(flatten)]` 内嵌，serde 不支持 flatten 与 `deny_unknown_fields` 组合，因此用户
// 列表与写命令 wire 无法由 contracts DTO 承担闭集；下方 Strict 镜像继续保持 fail-closed。
// 同步义务：`AdminUserResponse`/`AdminUserListItemResponse`/`AdminUserBlacklistHistoryResponse`/
// `AdminUserPaginationResponse`/`AdminUserMutationResponse` 字段集变化时必须同步本文件；
// decode 函数中构造 DTO 的穷举字段列表会在编译期捕捉漂移。
// 严格 wire 镜像同样遮蔽一次性 Key；TS 没有 Debug 格式，禁止把 generated_key 写入日志或控制台。
const StrictBlacklistHistorySchema = strictStruct({
  public_reason: v.string(),
  blacklisted_at: v.string(),
});

const StrictPaginationSchema = strictStruct({
  page: integer,
  page_size: integer,
  total: integer,
});

const StrictListedUserSchema = strictStruct({
  ...AdminUserResponseEntries,
  blacklist_count: integer,
  blacklist_history: v.array(StrictBlacklistHistorySchema),
});

const StrictUserListSchema = strictStruct({
  users: v.array(StrictListedUserSchema),
  pagination: StrictPaginationSchema,
});

/** 列表回执的 `state_version` 决定后续操作，必须拒绝重复键与所有非整数词法。 */
const ADMIN_USER_LIST_RESPONSE_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  numbers: "integers",
};

/** 解码完整用户列表回执；列表、摘要与分页错误都收敛为 `invalid_response`。 */
export function decodeUserList(response: TransportResponse): Result<AdminUserPage, string> {
  const receipt = decodeAdminReceipt(
    response,
    apiResponseSchema(StrictUserListSchema),
    ADMIN_USER_LIST_RESPONSE_FIELD_POLICY,
  );
  if (!receipt.ok) {
    return err(receipt.error);
  }
  const users: AdminUser[] = [];
  for (const candidate of receipt.value.data.users) {
    const decoded = decodeListedUser(candidate);
    if (!decoded.ok) {
      return err(invalidResponseError(decoded));
    }
    users.push(decoded.value);
  }
  const pagination = decodePagination(receipt.value.data.pagination);
  if (!pagination.ok) {
    return err(invalidResponseError(pagination));
  }
  return ok({ users, pagination: pagination.value });
}

/** 基础用户回执转换为列表行投影：写命令响应没有黑名单摘要，摘要置空。 */
function adminUserFromResponse(user: AdminUserResponse): AdminUser {
  return {
    id: user.id,
    username: user.username,
    forum_account_id: user.forum_account_id,
    status: user.status,
    key_status: user.key_status,
    state_version: user.state_version,
    created_at: user.created_at,
    updated_at: user.updated_at,
    blacklist_count: 0,
    blacklist_history: [],
  };
}

/** 解码用户列表；基础用户、黑名单摘要与分页都在浏览器边界 fail-closed。 */
function decodeListedUser(
  value: v.InferOutput<typeof StrictListedUserSchema>,
): Result<AdminUser, "user" | "blacklist_history" | "user_history_order"> {
  if (
    !Number.isSafeInteger(value.blacklist_count) ||
    value.blacklist_count < 0 ||
    value.blacklist_count > MAX_JSON_SAFE_INTEGER
  ) {
    return err("user");
  }
  const historyLength = Math.min(value.blacklist_count, 10);
  if (value.blacklist_history.length !== historyLength) {
    return err("user");
  }
  const history: AdminUserBlacklistHistoryResponse[] = [];
  for (const candidate of value.blacklist_history) {
    const decoded = decodeBlacklistHistory(candidate);
    if (!decoded.ok) {
      return err(decoded.error);
    }
    history.push(decoded.value);
  }
  for (let index = 1; index < history.length; index += 1) {
    const previous = history[index - 1];
    const next = history[index];
    if (previous === undefined || next === undefined) {
      return err("blacklist_history");
    }
    const previousTime = parseAdminTime(previous.blacklisted_at);
    const nextTime = parseAdminTime(next.blacklisted_at);
    if (previousTime === null || nextTime === null) {
      return err("blacklist_history");
    }
    if (nextTime > previousTime) {
      return err("user_history_order");
    }
  }
  if (!validUser(value)) {
    return err("user");
  }
  return ok({
    id: value.id,
    username: value.username,
    forum_account_id: value.forum_account_id,
    status: value.status,
    key_status: value.key_status,
    state_version: value.state_version,
    created_at: value.created_at,
    updated_at: value.updated_at,
    blacklist_count: value.blacklist_count,
    blacklist_history: history,
  });
}

/** 基础用户字段的闭集与文本边界；列表与写回执共用。 */
function validUser(value: AdminUserResponse): boolean {
  return (
    validUuid(value.id) &&
    validSafeText(value.username, 256) &&
    (value.forum_account_id === null || validForumAccountId(value.forum_account_id)) &&
    (value.status === "active" || value.status === "blacklisted") &&
    (value.key_status === "active" || value.key_status === "revoked") &&
    value.state_version >= 1 &&
    value.state_version <= MAX_JSON_SAFE_INTEGER &&
    validAdminTime(value.created_at) &&
    validAdminTime(value.updated_at)
  );
}

/** 论坛账号 ID 为非零开头的 1–32 位 ASCII 数字。 */
function validForumAccountId(value: string): boolean {
  if (value.length === 0 || value.length > 32 || value.charCodeAt(0) === 0x30) {
    return false;
  }
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    if (code < 0x30 || code > 0x39) {
      return false;
    }
  }
  return true;
}

/** 黑名单摘要的公开理由与时间边界。 */
function decodeBlacklistHistory(
  value: v.InferOutput<typeof StrictBlacklistHistorySchema>,
): Result<AdminUserBlacklistHistoryResponse, "blacklist_history"> {
  if (!validSafeText(value.public_reason, 1024) || !validAdminTime(value.blacklisted_at)) {
    return err("blacklist_history");
  }
  return ok({ public_reason: value.public_reason, blacklisted_at: value.blacklisted_at });
}

/** 分页三元组必须满足共享的响应边界。 */
function decodePagination(
  value: v.InferOutput<typeof StrictPaginationSchema>,
): Result<AdminUserPaginationResponse, "pagination"> {
  if (!validResponse(value.page, value.page_size, value.total)) {
    return err("pagination");
  }
  return ok({ page: value.page, page_size: value.page_size, total: value.total });
}

/** 基础用户回执的闭集校验；失败只在内部标记，最终统一上报 DTO 解码类别。 */
function decodeUser(value: AdminUserResponse): Result<AdminUserResponse, "user"> {
  if (!validUser(value)) {
    return err("user");
  }
  return ok({
    id: value.id,
    username: value.username,
    forum_account_id: value.forum_account_id,
    status: value.status,
    key_status: value.key_status,
    state_version: value.state_version,
    created_at: value.created_at,
    updated_at: value.updated_at,
  });
}

/** 一次性 Key 必须为 32–256 字节且不含控制字符。 */
function validGeneratedKey(value: string): boolean {
  const length = utf8ByteLength(value);
  return length >= 32 && length <= 256 && !/\p{Cc}/u.test(value);
}

/** 解码用户写命令回执；`wantKey` 对应创建生成 Key 与重签 Key。 */
export async function decodeUserMutation(
  host: AdminSessionHost,
  response: TransportResponse,
  wantKey: boolean,
  signal: AbortSignal,
): Promise<Result<AdminUserMutationResult, string>> {
  // 回执正文已缓冲，但任务取消或宿主注销仍对应 Rust 被丢弃的 future。
  if (signal.aborted || !adminSessionHostLive(host)) {
    return err("network_error");
  }
  if (wantKey) {
    const receipt = decodeAdminReceipt(
      response,
      apiResponseSchema(AdminUserMutationResponseWithKeySchema),
      ADMIN_USER_MUTATION_WITH_KEY_RESPONSE_FIELD_POLICY,
    );
    if (!receipt.ok) {
      return err(receipt.error);
    }
    const generatedKey = receipt.value.data.generated_key;
    if (!validGeneratedKey(generatedKey)) {
      return err("invalid_response");
    }
    const user = decodeUser(receipt.value.data.user);
    if (!user.ok) {
      return err(invalidResponseError(user));
    }
    // 结果是含一次性 Key 的普通对象；TS 没有 Rust 的 Debug 脱敏，禁止整体记录。
    return ok({ user: adminUserFromResponse(user.value), generated_key: generatedKey });
  }
  const receipt = decodeAdminReceipt(
    response,
    apiResponseSchema(AdminUserMutationResponseWithoutKeySchema),
    ADMIN_USER_MUTATION_WITHOUT_KEY_RESPONSE_FIELD_POLICY,
  );
  if (!receipt.ok) {
    return err(receipt.error);
  }
  const user = decodeUser(receipt.value.data.user);
  if (!user.ok) {
    return err(invalidResponseError(user));
  }
  return ok({ user: adminUserFromResponse(user.value), generated_key: null });
}
