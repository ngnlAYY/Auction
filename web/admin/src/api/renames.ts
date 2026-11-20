// Admin 改名历史 API 客户端。
// 页面只接收改名事实的安全投影；首次回滚按稳定用户 ID 读取当前版本，再冻结完整命令。
// 浏览器不自行推导用户状态；结果不确定时必须重放同一个命令 ID 与版本，不能重新读取后改写负载。

import { AdminPaginationResponseSchema } from "@auction/contracts/dto/admin/pagination";
import type {
  AdminRenameListResponse,
  AdminRenamePaginationResponse,
  AdminRenameResponse,
  AdminRenameRevertRequest,
} from "@auction/contracts/dto/admin/users";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { integer, optionalNullable, requiredNullable, struct } from "@auction/contracts/wire";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";
import * as v from "valibot";

import { parseRustUuid, rustUuidIsNil } from "./credentials/decode";
import {
  adminApiFailureAtWriteEndpoint,
  adminApiFailureNotSentCode,
  decodeAdminReceipt,
  invalidResponseError,
  responseError,
} from "./failure";
import { validate as validatePagination, validPageEcho, validResponse } from "./pagination";
import type { AdminSessionHost } from "./session";
import {
  type PreparedJsonCommand,
  prepareJsonCommand,
  sendGet,
  sendPreparedJsonWrite,
} from "./transport";
import { type AdminUser, decodeUserMutation } from "./users";
import { MAX_JSON_SAFE_INTEGER, validAdminTime, validTrimmedText } from "./validation";

/** 改名命令准备阶段读取历史的固定数量；可见列表数量由页面显式传入。 */
export const TABLE_PAGE_SIZE = 10;

/** 改名历史的 wire 投影别名，与 Rust 的 `AdminRename` 对应。 */
export type AdminRename = AdminRenameResponse;

/**
 * 旧 rename decoder 允许对象增加字段；普通展示仍允许省略 `user_state_version`，准备回滚时
 * 则必须从按稳定用户 ID 重新读取的当前投影取得非零版本，不能把兼容零值当作 fence。
 * 保留镜像的理由：wire 允许扩展字段且允许省略 `user_state_version`，而 contracts 的
 * `AdminRenameResponse` 将该字段作为必填标量、不能加 `deny_unknown_fields`；镜像继续按
 * 旧语义逐项校验。同步义务：`AdminRenameResponse`/`AdminRenameListResponse`/
 * `AdminRenamePaginationResponse` 字段集变化时必须同步本文件；后续 DTO 构造的穷举字段
 * 列表会在类型检查时捕捉漂移。
 */
const StrictRenameSchema = struct({
  actor: v.string(),
  created_at: v.string(),
  id: v.string(),
  next_username: v.string(),
  previous_username: v.string(),
  reverted_at: requiredNullable(v.string()),
  user_id: v.string(),
  user_state_version: optionalNullable(integer),
});

const StrictRenamePaginationSchema = AdminPaginationResponseSchema;

const StrictRenameListSchema = struct({
  pagination: StrictRenamePaginationSchema,
  renames: v.array(StrictRenameSchema),
});

/**
 * 旧 decoder 只对已知整数列使用整数词法；未知扩展字段里的其他数值仍应按 serde 的
 * `IgnoredAny` 接受，所以这里用 `integerPaths` 而不是整份文档的 `numbers: "integers"`。
 */
const ADMIN_RENAME_LIST_RESPONSE_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "pagination", "page"],
    ["data", "pagination", "page_size"],
    ["data", "pagination", "total"],
    ["data", "renames", null, "user_state_version"],
  ],
};

/** 改名历史的浏览器分页投影；wire 中嵌套的 pagination 在这里展平。 */
export interface AdminRenamePage {
  readonly renames: readonly AdminRename[];
  readonly page: number;
  readonly page_size: number;
  readonly total: number;
}

/** 获取带用户筛选、名称筛选与页码的改名历史，并展平分页信封。 */
export async function fetchAdminRenames(
  host: AdminSessionHost,
  userId: string,
  name: string,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<AdminRenamePage, string>> {
  const bounds = validatePagination(page, pageSize, 200, 0x7fffffff);
  if (!bounds.ok) {
    return err(bounds.error);
  }
  const query = new URLSearchParams();
  query.append("page", page.toString());
  query.append("page_size", pageSize.toString());
  if (userId !== "all") {
    query.append("user_id", userId);
  }
  const nameValue = rustTrim(name);
  if (nameValue !== "") {
    query.append("name", nameValue);
  }
  const response = await sendGet(host, `/api/v1/admin/renames?${query.toString()}`, signal);
  if (!response.ok) {
    return err(response.error);
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await responseError(received, signal, host));
  }
  const payload = decodeAdminReceipt(
    received,
    apiResponseSchema(StrictRenameListSchema),
    ADMIN_RENAME_LIST_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const decoded = decodeRenameList(payload.value.data, userId, page, pageSize);
  if (!decoded.ok) {
    return err(invalidResponseError(decoded.error));
  }
  const data = decoded.value;
  return ok({
    renames: data.renames,
    page: data.pagination.page,
    page_size: data.pagination.page_size,
    total: data.pagination.total,
  });
}

/** 当前页面冻结的回滚命令；只有首次准备读取版本，后续重试不再改变路径或请求正文。 */
export interface AdminRenameRevertCommand {
  readonly rename_id: string;
  readonly user_id: string;
  readonly request: AdminRenameRevertRequest;
  readonly prepared: PreparedJsonCommand;
}

/** 复用改名历史的精确用户筛选读取当前版本，不依赖用户名、匹配数量或目标历史所在页码。 */
export async function prepareAdminRenameRevert(
  host: AdminSessionHost,
  commandId: string,
  rename: AdminRename,
  signal: AbortSignal,
): Promise<Result<AdminRenameRevertCommand, string>> {
  const canonicalCommandId = parseRustUuid(commandId);
  const renameId = parseRustUuid(rename.id);
  const userId = parseRustUuid(rename.user_id);
  if (
    canonicalCommandId === null ||
    renameId === null ||
    userId === null ||
    rustUuidIsNil(canonicalCommandId) ||
    rustUuidIsNil(renameId) ||
    rustUuidIsNil(userId)
  ) {
    return err("invalid_request");
  }
  const history = await fetchAdminRenames(host, userId, "", 1, TABLE_PAGE_SIZE, signal);
  const expectedStateVersion = prepareRevertVersion(history);
  if (!expectedStateVersion.ok) {
    return err(expectedStateVersion.error);
  }
  const request: AdminRenameRevertRequest = Object.freeze({
    command_id: canonicalCommandId,
    expected_state_version: expectedStateVersion.value,
  });
  const prepared = prepareJsonCommand(request);
  if (!prepared.ok) {
    return err(prepared.error.code);
  }
  return ok(
    Object.freeze({
      rename_id: renameId,
      user_id: userId,
      request,
      prepared: prepared.value,
    }),
  );
}

function prepareRevertVersion(history: Result<AdminRenamePage, string>): Result<number, string> {
  // 此阶段只有读取；写入尚未发出。准备成功后冻结版本，重试必须沿用同一命令 ID 与请求体。
  if (!history.ok) {
    return err(adminApiFailureNotSentCode(history.error));
  }
  // 每行的版本来自同一次事务中 JOIN 的当前用户；任一改名行都能提供该用户的版本。
  const first = history.value.renames[0];
  if (first === undefined) {
    return err(adminApiFailureNotSentCode("invalid_response"));
  }
  const version = first.user_state_version;
  if (version <= 0 || !history.value.renames.every((row) => row.user_state_version === version)) {
    return err(adminApiFailureNotSentCode("invalid_response"));
  }
  return ok(version);
}

/** 发送已冻结的回滚命令；响应丢失后的重试直接进入服务端命令账本重放。 */
export async function revertAdminRename(
  host: AdminSessionHost,
  command: AdminRenameRevertCommand,
  signal: AbortSignal,
): Promise<Result<AdminUser, string>> {
  const expectedStateVersion = command.request.expected_state_version;
  const response = await sendPreparedJsonWrite(
    host,
    `/api/v1/admin/renames/${command.rename_id}/revert`,
    "POST",
    command.prepared,
    signal,
  );
  if (!response.ok) {
    return err(adminApiFailureAtWriteEndpoint(response.error, "rename-revert").code);
  }
  const decoded = await decodeUserMutation(host, response.value, false, signal);
  if (!decoded.ok) {
    return err(decoded.error);
  }
  const user = decoded.value.user;
  if (
    user.id !== command.user_id ||
    user.state_version !==
      (expectedStateVersion < MAX_JSON_SAFE_INTEGER ? expectedStateVersion + 1 : null)
  ) {
    return err("invalid_response");
  }
  return ok(user);
}

function decodeRenameList(
  data: v.InferOutput<typeof StrictRenameListSchema>,
  userFilter: string,
  requestedPage: number,
  requestedSize: number,
): Result<AdminRenameListResponse, string> {
  const pagination = decodeRenamePagination(data.pagination);
  if (!pagination.ok) {
    return err(pagination.error);
  }
  if (
    !validPageEcho(
      requestedPage,
      requestedSize,
      pagination.value.page,
      pagination.value.page_size,
      pagination.value.total,
      data.renames.length,
    )
  ) {
    return err("rename_pagination");
  }
  const renames: AdminRename[] = [];
  for (const value of data.renames) {
    const decoded = decodeRename(value);
    if (!decoded.ok) {
      return err(decoded.error);
    }
    renames.push(decoded.value);
  }
  if (userFilter !== "all" && renames.some((rename) => rename.user_id !== userFilter)) {
    return err("rename_filter");
  }
  return ok({ renames, pagination: pagination.value });
}

function decodeRename(
  value: v.InferOutput<typeof StrictRenameSchema>,
): Result<AdminRename, string> {
  const rawVersion = value.user_state_version;
  if (
    !validUuid(value.id) ||
    !validUuid(value.user_id) ||
    !validTrimmedText(value.previous_username) ||
    !validTrimmedText(value.next_username) ||
    !validTrimmedText(value.actor) ||
    !validAdminTime(value.created_at) ||
    (value.reverted_at !== null && !validAdminTime(value.reverted_at)) ||
    (rawVersion !== null && (rawVersion < 1 || rawVersion > MAX_JSON_SAFE_INTEGER))
  ) {
    return err("rename");
  }
  return ok({
    id: value.id,
    user_id: value.user_id,
    user_state_version: rawVersion ?? 0,
    previous_username: value.previous_username,
    next_username: value.next_username,
    actor: value.actor,
    created_at: value.created_at,
    reverted_at: value.reverted_at,
  });
}

function decodeRenamePagination(
  value: v.InferOutput<typeof StrictRenamePaginationSchema>,
): Result<AdminRenamePaginationResponse, string> {
  if (!validResponse(value.page, value.page_size, value.total)) {
    return err("rename_pagination");
  }
  return ok(value);
}

/** gloo 的 `Response::ok`：状态码 200 到 299。 */
function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}

/** Rust `str::trim` 的 Unicode White_Space 集合；请求筛选不能改用会接受 FEFF 的 JS `trim`。 */
function rustTrim(value: string): string {
  let start = 0;
  let end = value.length;
  while (start < end && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

function isRustWhitespace(code: number): boolean {
  return (
    (code >= 0x09 && code <= 0x0d) ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}
