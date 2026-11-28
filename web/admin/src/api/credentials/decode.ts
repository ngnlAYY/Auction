// 凭据安全投影的回执解码：只在本文件实现闭集、时间、文本与版本等跨字段谓词。

import type {
  AdminCredentialCheckResponse,
  AdminCredentialDeleteResponse,
  AdminCredentialListResponse,
  AdminCredentialMutationResponse,
  AdminCredentialResponse,
} from "@auction/contracts/dto/admin/credentials";
import { err, ok, type Result } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";

import { validPageEcho } from "../pagination";
import { MAX_JSON_SAFE_INTEGER, validAdminTime, validSafeText } from "../validation";

/** 凭据安全投影的 wire 类型别名，与 Rust 的 `AdminCredential` 对应。 */
export type AdminCredential = AdminCredentialResponse;

const NIL_UUID = "00000000-0000-0000-0000-000000000000";
const HEX_UUID = /^[0-9a-fA-F]{32}$/;
const HYPHENATED_UUID =
  /^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}$/;

/**
 * Rust `Uuid::parse_str` 接受的形式（32 位十六进制、36 位连字符、花括号、`urn:uuid:`），
 * 大小写不敏感，不限制版本与 variant；返回规范的小写连字符形式。
 */
export function parseRustUuid(value: string): string | null {
  if (typeof value !== "string") {
    return null;
  }
  let raw: string;
  if (HEX_UUID.test(value)) {
    raw = value;
  } else if (HYPHENATED_UUID.test(value)) {
    raw = value.replaceAll("-", "");
  } else if (
    value.length === 38 &&
    value.startsWith("{") &&
    value.endsWith("}") &&
    HYPHENATED_UUID.test(value.slice(1, -1))
  ) {
    raw = value.slice(1, -1).replaceAll("-", "");
  } else if (
    value.length === 45 &&
    value.slice(0, 9).toLowerCase() === "urn:uuid:" &&
    HYPHENATED_UUID.test(value.slice(9))
  ) {
    raw = value.slice(9).replaceAll("-", "");
  } else {
    return null;
  }
  const lower = raw.toLowerCase();
  return `${lower.slice(0, 8)}-${lower.slice(8, 12)}-${lower.slice(12, 16)}-${lower.slice(16, 20)}-${lower.slice(20)}`;
}

/** Rust `Uuid::is_nil`：与规范 nil UUID 比较。 */
export function rustUuidIsNil(value: string): boolean {
  return value === NIL_UUID;
}

/** 检查回执的世界、状态与时间闭集；响应版本必须回显被检查凭据。 */
export function decodeCredentialCheck(
  result: AdminCredentialCheckResponse,
  credential: AdminCredential,
): Result<AdminCredentialCheckResponse, string> {
  let worlds: readonly string[];
  if (credential.kind === "forum_cookie") {
    worlds = ["shared"];
  } else if (credential.kind === "mail_cookie" && credential.world === "main") {
    worlds = ["main"];
  } else if (credential.kind === "mail_cookie" && credential.world === "isekai") {
    worlds = ["isekai"];
  } else if (credential.kind === "mail_cookie" && credential.world === "shared") {
    worlds = ["main", "isekai"];
  } else {
    return err("invalid_request");
  }
  if (
    result.credential_id !== credential.id ||
    result.state_version !== credential.state_version ||
    !validAdminTime(result.checked_at) ||
    result.checks.length !== worlds.length ||
    result.checks.some((check, index) => {
      const world = worlds[index];
      return (
        world === undefined ||
        check.world !== world ||
        (check.status !== "valid" && check.status !== "invalid" && check.status !== "inconclusive")
      );
    })
  ) {
    return err("invalid_response");
  }
  return ok(result);
}

/** 列表回执的 restart、分页回显、逐项投影与筛选回显校验。 */
export function decodeCredentialList(
  data: AdminCredentialListResponse,
  kindFilter: string,
  worldFilter: string,
  statusFilter: string,
  requestedPage: number,
  requestedSize: number,
): Result<AdminCredentialListResponse, string> {
  if (!data.restart_required) {
    return err("credential_restart");
  }
  const pagination = data.pagination;
  if (
    !validPageEcho(
      requestedPage,
      requestedSize,
      pagination.page,
      pagination.page_size,
      pagination.total,
      data.credentials.length,
    )
  ) {
    return err("credential_pagination");
  }
  const credentials: AdminCredential[] = [];
  for (const credential of data.credentials) {
    const decoded = decodeCredential(credential);
    if (!decoded.ok) {
      return err(decoded.error);
    }
    credentials.push(decoded.value);
  }
  if (
    credentials.some(
      (credential) =>
        (kindFilter !== "" && credential.kind !== kindFilter) ||
        (worldFilter !== "" && credential.world !== worldFilter) ||
        (statusFilter !== "" && credential.status !== statusFilter),
    )
  ) {
    return err("credential_filter");
  }
  return ok({ credentials, pagination, restart_required: true });
}

/** 导入/修改回执：凭据投影通过同一跨字段谓词，restart 标志原样保留。 */
export function decodeCredentialData(
  data: AdminCredentialMutationResponse,
): Result<AdminCredentialMutationResponse, string> {
  const credential = decodeCredential(data.credential);
  if (!credential.ok) {
    return err(credential.error);
  }
  return ok({ credential: credential.value, restart_required: data.restart_required });
}

/** 删除回执：ID 必须匹配请求，版本必须恰好递增且仍在安全整数范围。 */
export function decodeCredentialDelete(
  data: AdminCredentialDeleteResponse,
  requestedCredentialId: string,
  expectedStateVersion: number,
): Result<AdminCredentialDeleteResponse, string> {
  const expectedNext =
    expectedStateVersion < MAX_JSON_SAFE_INTEGER ? expectedStateVersion + 1 : null;
  if (
    !validUuid(data.credential_id) ||
    data.credential_id !== requestedCredentialId ||
    data.state_version !== expectedNext ||
    data.state_version < 1 ||
    data.state_version > MAX_JSON_SAFE_INTEGER
  ) {
    return err("credential_delete");
  }
  return ok(data);
}

/** 跨字段谓词保留为独立校验：类型/世界/状态枚举与值域仍在浏览器边界 fail-closed。 */
export function decodeCredential(value: AdminCredential): Result<AdminCredential, string> {
  if (
    !validUuid(value.id) ||
    (value.kind !== "forum_cookie" && value.kind !== "mail_cookie") ||
    (value.world !== "main" && value.world !== "isekai" && value.world !== "shared") ||
    !validSafeText(value.label, 256) ||
    (value.player_name !== null &&
      (value.kind !== "mail_cookie" || !validSafeText(value.player_name, 256))) ||
    (value.status !== "active" && value.status !== "disabled" && value.status !== "invalid") ||
    !Number.isSafeInteger(value.state_version) ||
    value.state_version < 1 ||
    value.state_version > MAX_JSON_SAFE_INTEGER ||
    !validAdminTime(value.created_at) ||
    (value.rotated_at !== null && !validAdminTime(value.rotated_at)) ||
    !validAdminTime(value.updated_at)
  ) {
    return err("credential");
  }
  return ok(value);
}
