// Admin 数据库权威业务世界客户端。

import {
  ADMIN_WORLD_SET_ENABLED_RESPONSE_FIELD_POLICY,
  AdminWorldListResponseSchema,
  type AdminWorldResponse,
  type AdminWorldSetEnabledRequest,
  AdminWorldSetEnabledResponseSchema,
} from "@auction/contracts/dto/admin/worlds";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { parseRustUuid } from "./credentials/decode";
import {
  type AdminWriteFailure,
  type AdminWriteNotSent,
  decodeAdminReceipt,
  invalidResponseError,
  responseError,
  writeNotSent,
  writeOutcomeUnknown,
} from "./failure";
import type { AdminSessionHost } from "./session";
import {
  type PreparedJsonCommand,
  prepareJsonCommandOnce,
  sendGet,
  sendPreparedJsonWrite,
} from "./transport";
import { MAX_JSON_SAFE_INTEGER } from "./validation";

/** 数据库权威的业务世界状态。 */
export interface AdminBusinessWorld {
  readonly id: string;
  readonly enabled: boolean;
  readonly availability: string;
  readonly policyRevision: number;
}

/** 列表里的 `policy_revision` 会作为后续启停命令的 CAS 输入，整数词法必须原样可回传。 */
const ADMIN_WORLD_LIST_RESPONSE_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  numbers: "integers",
};

/**
 * 读取数据库权威的业务世界状态；它与重启生效的 Worker 入站配置保持独立。
 *
 * 结构闭集由 contracts 的 `AdminWorldResponse`/`AdminWorldListResponse`
 * （`deny_unknown_fields`）负责；这里只保留世界枚举、数量与排序等跨字段谓词。
 */
export async function fetchAdminBusinessWorlds(
  host: AdminSessionHost,
  signal: AbortSignal,
): Promise<Result<readonly AdminBusinessWorld[], string>> {
  const response = await sendGet(host, "/api/v1/admin/worlds", signal);
  if (!response.ok) {
    return err(response.error);
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await responseError(received, signal, host));
  }
  const payload = decodeAdminReceipt(
    received,
    apiResponseSchema(AdminWorldListResponseSchema),
    ADMIN_WORLD_LIST_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const decoded = decodeWorlds(payload.value.data.worlds);
  if (!decoded.ok) {
    return err(invalidResponseError(decoded.error));
  }
  return ok(decoded.value);
}

/** 已冻结世界启停参数与首次执行时生成的正文；正文供首次发送和原样重试共用。 */
export interface PreparedAdminBusinessWorldSetEnabled {
  readonly worldId: string;
  readonly enabled: boolean;
  readonly expectedPolicyRevision: number;
  readonly body: PreparedJsonCommand;
}

/**
 * 冻结带 revision fence 的世界启停命令；执行体第一次运行时才结合 command ID 生成正文。
 * 同一命令的重试只能发送同一份文本。
 */
export function prepareAdminBusinessWorldSetEnabledOnce(
  worldId: string,
  enabled: boolean,
  expectedPolicyRevision: number,
): (commandId: string) => Result<PreparedAdminBusinessWorldSetEnabled, AdminWriteNotSent> {
  const prepare = prepareJsonCommandOnce((commandId) => {
    const command: AdminWorldSetEnabledRequest = {
      command_id: commandId,
      enabled,
      expected_policy_revision: expectedPolicyRevision,
    };
    return command;
  });
  return (commandId) => {
    // Rust 的 request 字段是 `Uuid`；解析后再写入正文，保证非规范拼写不会进入请求。
    const canonicalCommandId = parseRustUuid(commandId);
    if (
      canonicalCommandId === null ||
      !validWorldId(worldId) ||
      !Number.isSafeInteger(expectedPolicyRevision) ||
      expectedPolicyRevision <= 0 ||
      expectedPolicyRevision > MAX_JSON_SAFE_INTEGER
    ) {
      return err(writeNotSent("invalid_request"));
    }
    const prepared = prepare(canonicalCommandId);
    if (!prepared.ok) {
      return err(prepared.error);
    }
    return ok(
      Object.freeze({
        worldId,
        enabled,
        expectedPolicyRevision,
        body: prepared.value,
      }),
    );
  };
}

/** 提交已冻结的世界启停命令；只接受与命令一致的世界状态与递增围栏。 */
export async function setAdminBusinessWorldEnabled(
  host: AdminSessionHost,
  prepared: PreparedAdminBusinessWorldSetEnabled,
  signal: AbortSignal,
): Promise<Result<AdminBusinessWorld, AdminWriteFailure>> {
  const expected = prepared.expectedPolicyRevision;
  const response = await sendPreparedJsonWrite(
    host,
    { url: `/api/v1/admin/worlds/${prepared.worldId}`, method: "PATCH", endpoint: "general" },
    prepared.body,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const payload = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminWorldSetEnabledResponseSchema),
    ADMIN_WORLD_SET_ENABLED_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(writeOutcomeUnknown(payload.error));
  }
  const decoded = decodeWorld(payload.value.data.world);
  if (!decoded.ok) {
    return err(writeOutcomeUnknown(invalidResponseError(decoded.error)));
  }
  const world = decoded.value;
  // Rust 的 `i64::saturating_add(1)` 只可能多推进一版；期望值已是 JSON 安全上限时
  // 下一版无法由解码器接受，因此只允许原值。
  const nextRevision =
    expected < MAX_JSON_SAFE_INTEGER && Number.isSafeInteger(expected + 1) ? expected + 1 : null;
  if (
    world.id !== prepared.worldId ||
    world.enabled !== prepared.enabled ||
    (world.policyRevision !== expected && world.policyRevision !== nextRevision)
  ) {
    return err(writeOutcomeUnknown("invalid_response"));
  }
  return ok(world);
}

function decodeWorlds(
  values: readonly AdminWorldResponse[],
): Result<readonly AdminBusinessWorld[], string> {
  if (values.length !== 2) {
    return err("business_world_count");
  }
  const ids = new Set<string>();
  const worlds: AdminBusinessWorld[] = [];
  for (const value of values) {
    const decoded = decodeWorld(value);
    if (!decoded.ok) {
      return err(decoded.error);
    }
    const world = decoded.value;
    if (ids.has(world.id)) {
      return err("business_world_duplicate");
    }
    ids.add(world.id);
    worlds.push(world);
  }
  worlds.sort((left, right) => worldOrder(left.id) - worldOrder(right.id));
  return ok(worlds);
}

function decodeWorld(value: AdminWorldResponse): Result<AdminBusinessWorld, string> {
  if (
    !validWorldId(value.id) ||
    (value.availability !== "available" &&
      value.availability !== "degraded" &&
      value.availability !== "unavailable") ||
    !Number.isSafeInteger(value.policy_revision) ||
    value.policy_revision < 1 ||
    value.policy_revision > MAX_JSON_SAFE_INTEGER
  ) {
    return err("business_world");
  }
  return ok({
    id: value.id,
    enabled: value.enabled,
    availability: value.availability,
    policyRevision: value.policy_revision,
  });
}

function validWorldId(value: string): boolean {
  return value === "main" || value === "isekai";
}

function worldOrder(id: string): number {
  return id === "main" ? 0 : 1;
}

function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}
