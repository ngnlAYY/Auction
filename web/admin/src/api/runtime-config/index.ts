// Admin runtime 配置 HTTP 客户端。
// 浏览器只编辑非敏感 JSON 文档，并把服务端返回的 fingerprint 原样作为更新 fence；TOML
// 文件、密钥和命令账本永远不进入浏览器。

import type { EquipmentTaxonomyDto } from "@auction/contracts/dto/admin/equipment";
import {
  type AdminJsonValue,
  type AdminRuntimeConfigResponse,
  AdminRuntimeConfigResponseSchema,
  type AdminRuntimeConfigResponseWithWorkerRestartState,
  AdminRuntimeConfigResponseWithWorkerRestartStateSchema,
  type AdminRuntimeConfigUpdateRequest,
  type AdminRuntimeConfigValidateRequest,
  type AdminWorkerRestartStateDto,
} from "@auction/contracts/dto/admin/rules";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { err, ok, type Result } from "@auction/web-shared/result";

import {
  type AdminApiFailure,
  type AdminWriteFailure,
  type AdminWriteNotSent,
  adminApiFailureClient,
  adminApiFailureDisplayCode,
  decodeAdminReceipt,
  invalidRequestError,
  invalidResponseError,
  responseFailure,
  writeNotSent,
  writeOutcomeUnknown,
} from "../failure";
import { type AdminSessionHost, adminSessionHostLive } from "../session";
import {
  type PreparedJsonCommand,
  prepareJsonCommandOnce,
  sendGet,
  sendJsonPreview,
  sendPreparedJsonWrite,
} from "../transport";
import { validRuntimeDocument } from "./document";
import { invalidRuntimeResponse, validFingerprint } from "./fields";
import { validEquipmentTaxonomy } from "./taxonomy";

/** Runtime 配置请求失败时保留 HTTP 状态，页面可区分 fingerprint 冲突和普通错误。 */
export type AdminRuntimeConfigApiError = AdminApiFailure;

/** 读取、校验与保存共用的安全投影；Rust 结构里 `worker_restart_state` 可选，读取时补齐。 */
export interface AdminRuntimeConfig {
  readonly document: unknown;
  readonly fingerprint: string;
  readonly restart_required: boolean;
  readonly equipment_taxonomy: EquipmentTaxonomyDto | null;
  readonly worker_restart_state: AdminWorkerRestartStateDto | null;
}

/** 读取、校验与保存都显式请求的扩展投影：装备分类词典与告警区块的长期未领取阈值。缺省响应
 * 不含该阈值，只供升级前的页面；本页按含该键的闭集严格校验。 */
const RUNTIME_CONFIG_QUERY =
  "include_equipment_taxonomy=true&include=fulfillment_collection_overdue_age_seconds";

/** 配置文档会原样编辑并作为写命令发回，键与整数词法都按严格边界解析。 */
const RUNTIME_CONFIG_RESPONSE_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  numbers: "integers",
};

/** 写命令的原文解析策略：整数词法必须在转成 JS 数值前检查，小数与指数写法直接拒绝。 */
const RUNTIME_CONFIG_DOCUMENT_FIELD_POLICY: JsonFieldPolicy = {
  numbers: "integers",
};

/** 未决命令执行体复用的正文工厂；同一 `command_id` 的重试只会得到同一份文本。 */
export type AdminRuntimeConfigCommandPreparation = (
  commandId: string,
) => Result<PreparedJsonCommand, AdminWriteNotSent>;

/** 解析操作者编辑的配置文档原文；整数词法与安全范围都在转成 JS 数值前检查。 */
export function parseAdminRuntimeConfigDocumentText(
  text: string,
): Result<AdminJsonValue, AdminApiFailure> {
  const parsed = parseJson(text, RUNTIME_CONFIG_DOCUMENT_FIELD_POLICY);
  if (!parsed.ok) {
    return err(adminApiFailureClient(invalidRequestError(parsed.error)));
  }
  // 词法检查拒绝小数、指数与 `-0`；无损检查再拒绝超出安全范围的整数和稀疏数组，
  // 避免 `JSON.stringify` 把操作者原文悄悄改写成另一个值。
  if (!losslessJsonValue(parsed.value)) {
    return err(adminApiFailureClient(invalidRequestError(parsed.value)));
  }
  // `losslessJsonValue` 已递归确认为 JSON 标量、普通对象或无空槽数组，因此可以视为
  // `AdminJsonValue`，不再重复构造一遍文档。
  return ok(parsed.value as AdminJsonValue);
}

/** 收口预校验与保存共用的文档输入：字符串按编辑器原文严格解析，其余值做无损检查。 */
function runtimeConfigDocument(value: AdminJsonValue): Result<AdminJsonValue, AdminApiFailure> {
  if (typeof value === "string") {
    // 运行配置文档本身总是 JSON 对象，因此字符串输入只可能来自编辑器原文；在转成
    // JS 数值前按整数词法解析，避免 `1000.00000000000001` 被静默写成 `1000`。
    return parseAdminRuntimeConfigDocumentText(value);
  }
  return losslessJsonValue(value)
    ? ok(value)
    : err(adminApiFailureClient(invalidRequestError(value)));
}

/** 读取运行配置；扩展投影只被明确以 `invalid_query` 拒绝时回退一次基础投影。 */
export async function fetchAdminRuntimeConfig(
  host: AdminSessionHost,
  signal: AbortSignal,
): Promise<Result<AdminRuntimeConfig, AdminRuntimeConfigApiError>> {
  return readRuntimeConfig(host, signal, (url) => fetchRuntimeConfigBody(host, url, signal));
}

async function readRuntimeConfig(
  host: AdminSessionHost,
  signal: AbortSignal,
  get: (url: string) => Promise<Result<string, AdminRuntimeConfigApiError>>,
): Promise<Result<AdminRuntimeConfig, AdminRuntimeConfigApiError>> {
  const base = `/api/v1/admin/config?${RUNTIME_CONFIG_QUERY}`;
  const extended = `${base}&include_worker_restart_state=true`;
  let body: string;
  let includeWorker: boolean;
  const extendedResult = await get(extended);
  // 传输层可能在正文缓冲完成后、本层恢复执行前被取消；此时丢弃结果，回到取消结果。
  if (runtimeConfigCancelled(host, signal)) {
    return err(
      runtimeConfigCancellationFailure(extendedResult.ok ? undefined : extendedResult.error),
    );
  }
  if (!extendedResult.ok) {
    if (extendedResult.error.status === 400 && extendedResult.error.code === "invalid_query") {
      // 只对明确拒绝新投影的只读请求回退一次，保留编辑器所需的既有投影。
      const baseResult = await get(base);
      if (runtimeConfigCancelled(host, signal)) {
        return err(runtimeConfigCancellationFailure(baseResult.ok ? undefined : baseResult.error));
      }
      if (!baseResult.ok) {
        return err(baseResult.error);
      }
      body = baseResult.value;
      includeWorker = false;
    } else {
      return err(extendedResult.error);
    }
  } else {
    body = extendedResult.value;
    includeWorker = true;
  }
  const value = decodeRuntimeConfigReadResponse(body, includeWorker);
  if (!value.ok) {
    return value;
  }
  return decodeRuntimeConfig(value.value, false);
}

async function fetchRuntimeConfigBody(
  host: AdminSessionHost,
  url: string,
  signal: AbortSignal,
): Promise<Result<string, AdminRuntimeConfigApiError>> {
  const response = await sendGet(host, url, signal);
  // 不在取消后读取错误正文或触发会话复核，与 Rust future 被丢弃后的行为一致。
  if (runtimeConfigCancelled(host, signal)) {
    return err(runtimeConfigCancellationFailure(response.ok ? undefined : response.error));
  }
  if (!response.ok) {
    return err(adminApiFailureClient(response.error));
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await responseFailure(received, signal, host));
  }
  return ok(received.text());
}

function decodeRuntimeConfigReadResponse(
  body: string,
  includeWorker: boolean,
): Result<AdminRuntimeConfig, AdminRuntimeConfigApiError> {
  const parsed = parseJson(body, RUNTIME_CONFIG_RESPONSE_FIELD_POLICY);
  if (!parsed.ok) {
    return err(adminApiFailureClient(invalidResponseError(parsed.error)));
  }
  const hasWorker = workerRestartProjection(parsed.value);
  if (hasWorker === null) {
    return err(adminApiFailureClient(invalidResponseError("worker_restart_state")));
  }
  if (hasWorker !== includeWorker) {
    return err(invalidRuntimeResponse());
  }
  if (includeWorker) {
    const payload = decode(
      apiResponseSchema(AdminRuntimeConfigResponseWithWorkerRestartStateSchema),
      parsed.value,
    );
    if (!payload.ok) {
      return err(adminApiFailureClient(invalidResponseError(payload)));
    }
    return ok(configFromResponse(payload.value.data, payload.value.data.worker_restart_state));
  }
  const payload = decode(apiResponseSchema(AdminRuntimeConfigResponseSchema), parsed.value);
  if (!payload.ok) {
    return err(adminApiFailureClient(invalidResponseError(payload)));
  }
  return ok(configFromResponse(payload.value.data, "unknown"));
}

// 只检查投影键是否如约出现，不复制配置文档。共享 DTO 的 Option 与枚举还可接收 null 和带标签的对象。
function workerRestartProjection(value: unknown): boolean | null {
  if (!isJsonObject(value)) {
    return null;
  }
  if (typeof value.code !== "string" || typeof value.message !== "string") {
    return null;
  }
  const data = value.data;
  if (!isJsonObject(data)) {
    return null;
  }
  if (!Object.hasOwn(data, "worker_restart_state")) {
    return false;
  }
  return typeof data.worker_restart_state === "string" ? true : null;
}

/** 预校验候选文档；不改变服务端 fingerprint，也不登记未决命令。字符串输入按编辑器原文
 * 严格解析，其余 JSON 值必须能无损写回。 */
export async function validateAdminRuntimeConfig(
  host: AdminSessionHost,
  document: AdminJsonValue,
  signal: AbortSignal,
): Promise<Result<AdminRuntimeConfig, AdminRuntimeConfigApiError>> {
  const candidate = runtimeConfigDocument(document);
  if (!candidate.ok) {
    return candidate;
  }
  const command: AdminRuntimeConfigValidateRequest = { document: candidate.value };
  const response = await sendJsonPreview(
    host,
    `/api/v1/admin/config:validate?${RUNTIME_CONFIG_QUERY}`,
    command,
    signal,
  );
  if (runtimeConfigCancelled(host, signal)) {
    return err(runtimeConfigCancellationFailure(response.ok ? undefined : response.error));
  }
  if (!response.ok) {
    return err(response.error);
  }
  const payload = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminRuntimeConfigResponseSchema),
    RUNTIME_CONFIG_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(adminApiFailureClient(payload.error));
  }
  return decodeRuntimeConfig(configFromResponse(payload.value.data, null), false);
}

/** 冻结文档与 fingerprint；执行体第一次运行时才结合 command ID 生成正文。字符串输入按
 * 编辑器原文严格解析，其余 JSON 值必须能无损写回。 */
export function prepareAdminRuntimeConfigUpdateOnce(
  expectedFingerprint: string,
  document: AdminJsonValue,
): AdminRuntimeConfigCommandPreparation {
  let frozen: AdminJsonValue;
  try {
    // 操作者编辑后的文档会原样写回服务端。`JSON.stringify` 会丢弃 `undefined`、把 `-0`
    // 写成 `0`、把已超出安全范围的整数写成另一个值，这些输入不能静默改写后发出。
    const candidate = runtimeConfigDocument(document);
    if (!candidate.ok) {
      const error = writeNotSent(candidate.error.code);
      return () => err(error);
    }
    frozen = structuredClone(candidate.value);
  } catch (error) {
    // 输入不是可克隆的 JSON 值；错误对象与消息都不进入页面或控制台。
    return () => err(writeNotSent(invalidRequestError(error)));
  }
  return prepareJsonCommandOnce((commandId) => {
    const command: AdminRuntimeConfigUpdateRequest = {
      command_id: commandId,
      expected_fingerprint: expectedFingerprint,
      document: frozen,
    };
    return command;
  });
}

/** 经 `JSON.stringify` 往返后值不变的 JSON：安全整数、非负零、无空槽数组与普通对象。 */
function losslessJsonValue(value: unknown): boolean {
  try {
    return losslessJsonValueInner(value);
  } catch {
    return false;
  }
}

function losslessJsonValueInner(value: unknown): boolean {
  if (value === null || typeof value === "string" || typeof value === "boolean") {
    return true;
  }
  if (typeof value === "number") {
    // 运行配置文档的所有数值字段都是整数；JS 无法区分 `2` 与 `2.0`，但小数、指数
    // 与安全范围外的值会在原文解析或这里被拒绝，不进入 `JSON.stringify`。
    return Number.isSafeInteger(value) && !Object.is(value, -0);
  }
  if (Array.isArray(value)) {
    const keys = Object.keys(value);
    // `Array.prototype.every` 会跳过空槽，而 `JSON.stringify` 把空槽写成 `null`；额外
    // 自有属性同样会被丢弃，因此数组必须恰好由全部下标组成。
    if (keys.length !== value.length) {
      return false;
    }
    if (Object.getOwnPropertyNames(value).length !== value.length + 1) {
      return false;
    }
    if (Object.getOwnPropertySymbols(value).length > 0) {
      return false;
    }
    for (let index = 0; index < value.length; index += 1) {
      if (!Object.hasOwn(value, index) || !losslessJsonValueInner(value[index])) {
        return false;
      }
    }
    return true;
  }
  if (typeof value === "object") {
    const prototype = Object.getPrototypeOf(value);
    if (prototype !== Object.prototype && prototype !== null) {
      return false;
    }
    const keys = Object.keys(value);
    // 符号键、不可枚举属性与 `toJSON` 都不会按普通数据往返，拒绝这些非 JSON 输入。
    if (
      Object.getOwnPropertySymbols(value).length > 0 ||
      Object.getOwnPropertyNames(value).length !== keys.length
    ) {
      return false;
    }
    return keys.every((key) =>
      losslessJsonValueInner((value as Readonly<Record<string, unknown>>)[key]),
    );
  }
  return false;
}

/** 用准备好的固定正文发送保存命令，并校验保存回执。 */
export async function updateAdminRuntimeConfig(
  host: AdminSessionHost,
  commandId: string,
  prepare: AdminRuntimeConfigCommandPreparation,
  signal: AbortSignal,
): Promise<Result<AdminRuntimeConfig, AdminWriteFailure>> {
  const prepared = prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const response = await sendPreparedJsonWrite(
    host,
    { url: `/api/v1/admin/config?${RUNTIME_CONFIG_QUERY}`, method: "PUT", endpoint: "general" },
    prepared.value,
    signal,
  );
  if (runtimeConfigCancelled(host, signal)) {
    if (
      !response.ok &&
      response.error.kind !== "verified-http" &&
      response.error.code === "network_error"
    ) {
      return response;
    }
    return err(writeOutcomeUnknown("network_error"));
  }
  if (!response.ok) {
    return err(response.error);
  }
  const payload = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminRuntimeConfigResponseSchema),
    RUNTIME_CONFIG_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(writeOutcomeUnknown(payload.error));
  }
  const receipt = decodeRuntimeConfig(configFromResponse(payload.value.data, null), true);
  if (!receipt.ok) {
    return err(writeOutcomeUnknown(receipt.error.code));
  }
  const sent = parseJson(prepared.value.body, RUNTIME_CONFIG_DOCUMENT_FIELD_POLICY);
  if (!sent.ok || !isJsonObject(sent.value)) {
    return err(writeOutcomeUnknown(invalidRuntimeResponse().code));
  }
  // 只有与冻结候选对应的回执才能推进编辑基线。投影只作用于正文副本，重试复用原请求。
  const expected = sent.value.document;
  projectUpdateDocument(expected);
  return structuralEquals(receipt.value.document, expected)
    ? receipt
    : err(writeOutcomeUnknown(invalidRuntimeResponse().code));
}

/** 将完整的页面候选转成 HTTP 保存回执的文档投影，供结构相等比较。 */
function projectUpdateDocument(document: unknown): void {
  if (!isJsonObject(document)) {
    return;
  }
  // HTTP 已补齐邮件默认值、可交易开关和告警阈值，候选中含这些字段时会按原值回显。
  omitEmptyArrays(document, ["bot_players"]);
  if (!Object.hasOwn(document, "equipment_colors")) {
    document.equipment_colors = { rules: [] };
  }
  const colors = document.equipment_colors;
  if (isJsonObject(colors) && Array.isArray(colors.rules)) {
    for (const rule of colors.rules) {
      if (isJsonObject(rule)) {
        omitEmptyArrays(rule, ["categories", "equipment_types", "qualities"]);
      }
    }
  }
  const equipmentFilter = document.equipment_filter;
  if (
    isJsonObject(equipmentFilter) &&
    Array.isArray(equipmentFilter.qualities) &&
    equipmentFilter.qualities.length === 0
  ) {
    // HTTP 以实际生效的品质门槛投影空配置（materialize_business_defaults）。
    equipmentFilter.qualities = ["Legendary", "Peerless"];
  }
  const operations = document.operations;
  const schedule = isJsonObject(operations) ? operations.schedule : null;
  const earlyBird = isJsonObject(schedule) ? schedule.early_bird : null;
  if (isJsonObject(earlyBird)) {
    if (earlyBird.pre_season_session === true) {
      delete earlyBird.pre_season_session;
    }
    if (earlyBird.pre_season_days_before === 3) {
      delete earlyBird.pre_season_days_before;
    }
  }
}

function omitEmptyArrays(object: Record<string, unknown>, keys: readonly string[]): void {
  for (const key of keys) {
    const value = object[key];
    if (Array.isArray(value) && value.length === 0) {
      delete object[key];
    }
  }
}

function decodeRuntimeConfig(
  value: AdminRuntimeConfig,
  expectedRestartRequired: boolean,
): Result<AdminRuntimeConfig, AdminApiFailure> {
  if (
    value.restart_required !== expectedRestartRequired ||
    !validFingerprint(value.fingerprint) ||
    !validRuntimeDocument(value.document) ||
    value.equipment_taxonomy === null ||
    !validEquipmentTaxonomy(value.equipment_taxonomy)
  ) {
    return err(invalidRuntimeResponse());
  }
  return ok(value);
}

/** 把契约解码结果统一成 Rust DTO 的完整投影；缺省时可选的 `equipment_taxonomy` 读作 `null`。 */
function configFromResponse(
  data: AdminRuntimeConfigResponse | AdminRuntimeConfigResponseWithWorkerRestartState,
  workerRestartState: AdminWorkerRestartStateDto | null,
): AdminRuntimeConfig {
  return {
    document: data.document,
    fingerprint: data.fingerprint,
    restart_required: data.restart_required,
    equipment_taxonomy: data.equipment_taxonomy ?? null,
    worker_restart_state: workerRestartState,
  };
}

/** JSON 对象：非 null、非数组的 `typeof "object"`；解析产物只来自 JSON。 */
function isJsonObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** gloo 的 `Response::ok`：状态码 200 到 299。 */
function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}

/** AG1 在 `await` 恢复后复查取消与宿主存活；取消结果沿用传输层的 `network_error`。 */
function runtimeConfigCancelled(host: AdminSessionHost, signal: AbortSignal): boolean {
  return signal.aborted || !adminSessionHostLive(host);
}

/** 取消后的结果：保留传输层已标注的取消码（含 `not_sent` 阶段），其余统一为网络错误。 */
function runtimeConfigCancellationFailure(error?: AdminApiFailure | string): AdminApiFailure {
  if (error === undefined) {
    return adminApiFailureClient("network_error");
  }
  const code = typeof error === "string" ? error : error.code;
  if (adminApiFailureDisplayCode(code) !== "network_error") {
    return adminApiFailureClient("network_error");
  }
  return typeof error === "string" ? adminApiFailureClient(error) : error;
}
