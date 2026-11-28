// Admin 凭据管理 API 客户端。
// 导入的密码和 Cookie 随会话命令驻留内存，未知结果只允许原样手动重试。
// 列表和命令响应只包含凭据安全投影。

import {
  ADMIN_CREDENTIAL_DELETE_RESPONSE_FIELD_POLICY,
  ADMIN_CREDENTIAL_MUTATION_RESPONSE_FIELD_POLICY,
  type AdminCredentialCheckRequest,
  type AdminCredentialCheckResponse,
  AdminCredentialCheckResponseSchema,
  type AdminCredentialDeleteResponse,
  AdminCredentialDeleteResponseSchema,
  AdminCredentialListResponseSchema,
  type AdminCredentialMutationResponse,
  AdminCredentialMutationResponseSchema,
} from "@auction/contracts/dto/admin/credentials";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import {
  DEFAULT_REQUEST_TIMEOUT_MILLIS,
  type TransportResponse,
} from "@auction/web-shared/net/transport";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";
import {
  type AdminWriteFailure,
  type AdminWriteNotSent,
  decodeAdminReceipt,
  invalidResponseError,
  responseError,
  writeOutcomeUnknown,
} from "../failure";
import { validate as validatePagination } from "../pagination";
import type { AdminSessionHost } from "../session";
import {
  prepareJsonCommand,
  sendGet,
  sendPreparedJsonPreview,
  sendPreparedJsonWrite,
} from "../transport";
import { MAX_JSON_SAFE_INTEGER } from "../validation";
import {
  type AdminCredential,
  decodeCredential,
  decodeCredentialCheck,
  decodeCredentialData,
  decodeCredentialDelete,
  decodeCredentialList,
} from "./decode";
import {
  copyCredential,
  type PreparedAdminCredentialCreate,
  type PreparedAdminCredentialDelete,
  type PreparedAdminCredentialMutation,
} from "./prepare";

export type {
  PreparedAdminCredentialCreate,
  PreparedAdminCredentialDelete,
  PreparedAdminCredentialMutation,
} from "./prepare";
export {
  prepareAdminCredentialCreate,
  prepareAdminCredentialCreateOnce,
  prepareAdminCredentialDelete,
  prepareAdminCredentialMutation,
} from "./prepare";

// 导入须容纳服务端 10 秒论坛获取及原子写入；不延长其他 API 请求，也不自动重试。
const CREDENTIAL_IMPORT_TIMEOUT_MILLIS = 20_000;

// shared 邮件检查串行检查两个世界，须覆盖 WORKER_MAIL_CREDENTIAL_CHECK_TIMEOUT_SECONDS（12 秒）
// 和 Admin HTTP 默认的 15 秒预算，另留 3 秒给响应传输。
const SHARED_MAIL_CHECK_TIMEOUT_MILLIS = 18_000;

// 结构闭集由 contracts 的 `AdminCredentialResponse`（`rotated_at` 保持必填 nullable 语义）、
// `AdminCredentialListResponse`/`AdminCredentialMutationResponse`/`AdminCredentialDeleteResponse`
//（均为 `deny_unknown_fields`）负责；本文件只保留筛选回显与版本递增等跨字段谓词。

/** 凭据列表与检查结果中的版本、计数会决定后续操作，因此按整数词法严格解析。 */
const ADMIN_CREDENTIAL_LIST_RESPONSE_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  numbers: "integers",
};

/** 检查结果回执同样只用于提示，但版本与状态仍按服务端整数闭集解析。 */
const ADMIN_CREDENTIAL_CHECK_RESPONSE_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  numbers: "integers",
};

/** 凭据安全投影；与 Rust 的 `AdminCredential` 对应。 */
export type { AdminCredential } from "./decode";
/** 只读检查回执。 */
export type AdminCredentialCheck = AdminCredentialCheckResponse;
/** 删除命令回执。 */
export type AdminCredentialDeleteResult = AdminCredentialDeleteResponse;

/** 凭据列表页的浏览器展示投影；分页字段在这里展平，wire 形状仍由 contracts 拥有。 */
export interface AdminCredentialPage {
  readonly credentials: readonly AdminCredential[];
  readonly page: number;
  readonly page_size: number;
  readonly total: number;
  readonly restart_required: boolean;
}

/** 只读检查指定版本；响应的世界、状态与时间必须通过闭集校验。 */
export async function checkAdminCredential(
  host: AdminSessionHost,
  credential: AdminCredential,
  signal: AbortSignal,
): Promise<Result<AdminCredentialCheck, string>> {
  let snapshot: AdminCredential;
  try {
    snapshot = copyCredential(credential);
  } catch {
    return err("invalid_request");
  }
  const decoded = decodeCredential(snapshot);
  if (!decoded.ok) {
    return err("invalid_request");
  }
  const credentialId = decoded.value.id;
  if (!validUuid(credentialId)) {
    return err("invalid_request");
  }
  const request: AdminCredentialCheckRequest = {
    expected_state_version: decoded.value.state_version,
  };
  const prepared = prepareJsonCommand(request);
  if (!prepared.ok) {
    return err("invalid_request");
  }
  const response = await sendPreparedJsonPreview(
    host,
    `/api/v1/admin/credentials/${credentialId}/check`,
    prepared.value,
    signal,
    credentialCheckTimeout(decoded.value),
  );
  if (!response.ok) {
    return err(response.error.code);
  }
  const payload = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminCredentialCheckResponseSchema),
    ADMIN_CREDENTIAL_CHECK_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  return decodeCredentialCheck(payload.value.data, decoded.value);
}

function credentialCheckTimeout(credential: AdminCredential): number {
  if (credential.kind === "mail_cookie" && credential.world === "shared") {
    return SHARED_MAIL_CHECK_TIMEOUT_MILLIS;
  }
  return DEFAULT_REQUEST_TIMEOUT_MILLIS;
}

/** 获取带筛选和页码的 Admin 凭据安全列表。 */
export async function fetchAdminCredentials(
  host: AdminSessionHost,
  kind: string,
  world: string,
  status: string,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<AdminCredentialPage, string>> {
  const bounds = validatePagination(page, pageSize, 200, 0x7fffffff);
  if (!bounds.ok) {
    return err(bounds.error);
  }
  const requestedPage = Math.max(page, 1);
  const kindValue = rustTrim(kind);
  const worldValue = rustTrim(world);
  const statusValue = rustTrim(status);
  const query = new URLSearchParams();
  query.append("page", requestedPage.toString());
  query.append("page_size", pageSize.toString());
  if (kindValue !== "") {
    query.append("kind", kindValue);
  }
  if (worldValue !== "") {
    query.append("world", worldValue);
  }
  if (statusValue !== "") {
    query.append("status", statusValue);
  }
  const response = await sendGet(host, `/api/v1/admin/credentials?${query.toString()}`, signal);
  if (!response.ok) {
    return err(response.error);
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await responseError(received, signal, host));
  }
  const payload = decodeAdminReceipt(
    received,
    apiResponseSchema(AdminCredentialListResponseSchema),
    ADMIN_CREDENTIAL_LIST_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const decoded = decodeCredentialList(
    payload.value.data,
    kindValue,
    worldValue,
    statusValue,
    page,
    pageSize,
  );
  if (!decoded.ok) {
    return err(invalidResponseError(decoded.error));
  }
  const data = decoded.value;
  return ok({
    credentials: data.credentials,
    page: data.pagination.page,
    page_size: data.pagination.page_size,
    total: data.pagination.total,
    restart_required: data.restart_required,
  });
}

/** 发送已固定的导入正文，并核对响应的凭据投影。 */
export async function createAdminCredential(
  host: AdminSessionHost,
  prepared: Result<PreparedAdminCredentialCreate, AdminWriteNotSent>,
  signal: AbortSignal,
): Promise<Result<AdminCredentialMutationResponse, AdminWriteFailure>> {
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const command = prepared.value;
  const response = await sendPreparedJsonWrite(
    host,
    { url: "/api/v1/admin/credentials", method: "POST", endpoint: "general" },
    command.prepared,
    signal,
    CREDENTIAL_IMPORT_TIMEOUT_MILLIS,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const payload = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminCredentialMutationResponseSchema),
    ADMIN_CREDENTIAL_MUTATION_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(writeOutcomeUnknown(payload.error));
  }
  const decoded = decodeCredentialData(payload.value.data);
  if (!decoded.ok) {
    return err(writeOutcomeUnknown(invalidResponseError(decoded.error)));
  }
  const data = decoded.value;
  if (
    data.credential.kind !== command.kind ||
    data.credential.world !== command.world ||
    data.credential.label !== command.label ||
    data.credential.player_name !== command.player_name
  ) {
    return err(writeOutcomeUnknown("invalid_response"));
  }
  return ok(data);
}

/** 修改凭据标签或状态；`prepared` 已同时固定正文、路径参数与回执核对字段。 */
export async function mutateAdminCredential(
  host: AdminSessionHost,
  prepared: Result<PreparedAdminCredentialMutation, AdminWriteNotSent>,
  signal: AbortSignal,
): Promise<Result<AdminCredentialMutationResponse, AdminWriteFailure>> {
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const command = prepared.value;
  const response = await sendPreparedJsonWrite(
    host,
    { url: "/api/v1/admin/credentials", method: "PATCH", endpoint: "general" },
    command.prepared,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const payload = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminCredentialMutationResponseSchema),
    ADMIN_CREDENTIAL_MUTATION_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(writeOutcomeUnknown(payload.error));
  }
  const decoded = decodeCredentialData(payload.value.data);
  if (!decoded.ok) {
    return err(writeOutcomeUnknown(invalidResponseError(decoded.error)));
  }
  const data = decoded.value;
  const updated = data.credential;
  const expectedNext =
    command.previous.state_version < MAX_JSON_SAFE_INTEGER
      ? command.previous.state_version + 1
      : null;
  if (
    updated.id !== command.credential_id ||
    updated.state_version !== expectedNext ||
    updated.kind !== command.previous.kind ||
    updated.world !== command.previous.world ||
    updated.player_name !== command.previous.player_name ||
    (command.action === "rename"
      ? command.label !== updated.label ||
        command.status !== null ||
        updated.status !== command.previous.status
      : command.action === "set_status"
        ? command.status !== updated.status ||
          command.label !== null ||
          updated.label !== command.previous.label
        : true)
  ) {
    return err(writeOutcomeUnknown("invalid_response"));
  }
  return ok(data);
}

/** 删除凭据；`prepared` 已同时固定正文、路径参数与回执核对字段。 */
export async function deleteAdminCredential(
  host: AdminSessionHost,
  prepared: Result<PreparedAdminCredentialDelete, AdminWriteNotSent>,
  signal: AbortSignal,
): Promise<Result<AdminCredentialDeleteResult, AdminWriteFailure>> {
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const command = prepared.value;
  const response = await sendPreparedJsonWrite(
    host,
    { url: "/api/v1/admin/credentials", method: "DELETE", endpoint: "general" },
    command.prepared,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const payload = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminCredentialDeleteResponseSchema),
    ADMIN_CREDENTIAL_DELETE_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(writeOutcomeUnknown(payload.error));
  }
  const decoded = decodeCredentialDelete(
    payload.value.data,
    command.credential_id,
    command.previous.state_version,
  );
  if (!decoded.ok) {
    return err(writeOutcomeUnknown(invalidResponseError(decoded.error)));
  }
  return ok(decoded.value);
}
/** gloo 的 `Response::ok`：状态码 200 到 299。 */
function responseOk(response: TransportResponse): boolean {
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
