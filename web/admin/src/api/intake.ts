// Admin 装备录入 API 客户端。
// 浏览器只把用户当前填写的来源提交给后端；成功响应不返回装备 key 或来源正文。

import {
  ADMIN_INTAKE_RESPONSE_FIELD_POLICY,
  type AdminCanonicalEquipmentRequest,
  type AdminCanonicalEquipmentStatRequest,
  type AdminIntakeRequest,
  type AdminIntakeResponse,
  AdminIntakeResponseSchema,
} from "@auction/contracts/dto/admin/auction";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { type JsonObject, jsonKeysOnly, parseJson } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { compareText } from "@auction/web-shared/text";

import {
  type AdminWriteFailure,
  type AdminWriteNotSent,
  decodeAdminReceipt,
  invalidRequestError,
  invalidResponseError,
  writeNotSent,
  writeOutcomeUnknown,
} from "./failure";
import type { AdminSessionHost } from "./session";
import {
  type PreparedJsonCommand,
  prepareJsonCommandOnce,
  sendPreparedJsonWrite,
} from "./transport";
import { MAX_JSON_SAFE_INTEGER, validTrimmedText } from "./validation";

/** 录入成功后的拍品安全投影。 */
export type AdminIntakeResult = AdminIntakeResponse;

/** 未决命令执行体复用的正文工厂；同一 `command_id` 的重试只会得到同一份文本。 */
export type AdminIntakePreparation = (
  commandId: string,
) => Result<PreparedJsonCommand, AdminWriteNotSent>;

const I32_MAX = 2_147_483_647;
const I32_MIN = -2_147_483_648;

const EQUIPMENT_FIELDS = [
  "external_equipment_id",
  "name",
  "category",
  "equipment_type",
  "quality",
  "prefix",
  "suffix",
  "level",
  "tradeable",
  "stats",
  "key",
] as const;

const EQUIPMENT_STRING_FIELDS = [
  "external_equipment_id",
  "name",
  "category",
  "equipment_type",
  "quality",
  "prefix",
  "suffix",
  "key",
] as const;

const EQUIPMENT_STAT_FIELDS = ["value", "base"] as const;

// 结构闭集由 contracts 的 `AdminIntakeResponse`（`deny_unknown_fields`，`equipment_level`
// 保持旧 decoder 的必填 nullable 语义）负责；标识字段直接以 `Uuid` 解码（接受 RFC 4122
// 全版本），其余值域谓词保留在下方校验函数。

/** 冻结装备来源与出价人输入；canonical JSON 只解析一次，正文在首次执行时固定。 */
export function prepareAdminIntakeOnce(
  sourceKind: string,
  equipmentUrl: string,
  equipmentJson: string,
  startPrice: number,
  playerId: string,
): AdminIntakePreparation {
  let equipment: AdminCanonicalEquipmentRequest | null = null;
  if (sourceKind === "json") {
    const parsed = parseCanonicalEquipment(equipmentJson);
    if (!parsed.ok) {
      const code = parsed.error;
      return () => err(writeNotSent(code));
    }
    equipment = parsed.value;
  }
  const frozenEquipment = equipment;
  const frozenUrl = sourceKind === "url" && equipmentUrl !== "" ? equipmentUrl : null;
  return prepareJsonCommandOnce((commandId) => {
    const command: AdminIntakeRequest = {
      command_id: commandId,
      source_kind: sourceKind,
      equipment_url: frozenUrl,
      equipment: equipmentForWire(frozenEquipment),
      start_price: startPrice,
      player_id: playerId,
    };
    return command;
  });
}

/** 提交 URL 或 canonical JSON 装备来源，并返回新建 lot 的安全投影。
 * `command_id` 由表单按同一输入保持不变，网络失败后的重试不会创建第二条 intake。 */
export async function addAdminIntake(
  host: AdminSessionHost,
  commandId: string,
  worldId: string,
  startPrice: number,
  playerId: string,
  prepare: AdminIntakePreparation,
  signal: AbortSignal,
): Promise<Result<AdminIntakeResult, AdminWriteFailure>> {
  const prepared = prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const url = `/api/v1/admin/worlds/${worldId}/auction-intake/items`;
  const response = await sendPreparedJsonWrite(
    host,
    { url, method: "POST", endpoint: "general" },
    prepared.value,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const payload = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminIntakeResponseSchema),
    ADMIN_INTAKE_RESPONSE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(writeOutcomeUnknown(payload.error));
  }
  const result = decodeIntakeResult(payload.value.data);
  if (!result.ok) {
    return err(writeOutcomeUnknown(invalidResponseError(result.error)));
  }
  if (
    result.value.start_price !== startPrice ||
    result.value.seller_player_id !== rustTrim(playerId)
  ) {
    return err(writeOutcomeUnknown("invalid_response"));
  }
  return ok(result.value);
}

/** 表单按末值校验完整字段集，接口按具型 DTO 校验根字段唯一性并保留缺省语义。 */
export function parseCanonicalEquipment(
  equipmentJson: string,
  requireAllFields = false,
): Result<AdminCanonicalEquipmentRequest, string> {
  // 表单也传入字段策略，使被覆盖的原文字符串和数字一并受检。
  const parsed = parseJson(
    equipmentJson,
    requireAllFields ? {} : { uniqueRootKeys: EQUIPMENT_FIELDS },
  );
  if (!parsed.ok) {
    return err(invalidRequestError(parsed.error));
  }
  if (!validEquipmentJsonFields(equipmentJson)) {
    return err("invalid_request");
  }
  const equipment = asJsonObject(parsed.value);
  if (
    equipment === null ||
    !jsonKeysOnly(equipment, requireAllFields ? EQUIPMENT_FIELDS : [], EQUIPMENT_FIELDS)
  ) {
    return err("invalid_request");
  }
  for (const field of EQUIPMENT_STRING_FIELDS) {
    if (!isOptionalString(equipment[field])) {
      return err("invalid_request");
    }
  }
  const level = equipment.level;
  if (
    level !== undefined &&
    level !== null &&
    (typeof level !== "number" || !Number.isInteger(level) || level < I32_MIN || level > I32_MAX)
  ) {
    return err("invalid_request");
  }
  const tradeable = equipment.tradeable;
  if (tradeable !== undefined && tradeable !== null && typeof tradeable !== "boolean") {
    return err("invalid_request");
  }
  const parsedStats = parseEquipmentStats(equipment.stats);
  if (!parsedStats.ok) {
    return err(parsedStats.error);
  }
  return ok({
    external_equipment_id: optionalString(equipment.external_equipment_id),
    name: optionalString(equipment.name),
    category: optionalString(equipment.category),
    equipment_type: optionalString(equipment.equipment_type),
    quality: optionalString(equipment.quality),
    prefix: optionalString(equipment.prefix),
    suffix: optionalString(equipment.suffix),
    level: typeof level === "number" ? level : null,
    tradeable: typeof tradeable === "boolean" ? tradeable : null,
    stats: parsedStats.value,
    key: optionalString(equipment.key),
  });
}

function parseEquipmentStats(
  value: unknown,
): Result<Readonly<Record<string, AdminCanonicalEquipmentStatRequest>> | null, string> {
  if (value === undefined || value === null) {
    return ok(null);
  }
  const stats = asJsonObject(value);
  if (stats === null) {
    return err("invalid_request");
  }
  const output: Record<string, AdminCanonicalEquipmentStatRequest> = Object.create(null);
  for (const key of Object.keys(stats)) {
    const stat = asJsonObject(stats[key]);
    if (stat === null || !jsonKeysOnly(stat, [], EQUIPMENT_STAT_FIELDS)) {
      return err("invalid_request");
    }
    const statValue = stat.value;
    const base = stat.base;
    if (
      !isOptionalString(statValue) ||
      !(
        base === undefined ||
        base === null ||
        (typeof base === "number" && base >= 0 && base <= 1_000_000)
      )
    ) {
      return err("invalid_request");
    }
    const canonical: AdminCanonicalEquipmentStatRequest = {
      value: typeof statValue === "string" ? statValue : null,
    };
    if (typeof base === "number") {
      canonical.base = base;
    }
    output[key] = canonical;
  }
  return ok(orderedStats(output));
}

function equipmentForWire(
  equipment: AdminCanonicalEquipmentRequest | null,
): AdminCanonicalEquipmentRequest | null {
  if (equipment === null || equipment.stats === null) {
    return equipment;
  }
  const entries = Object.entries(equipment.stats);
  if (!entries.some(([, stat]) => Object.is(stat.base, -0))) {
    return equipment;
  }
  // JSON.stringify 会抹掉负零的符号。只嵌入固定的数字字面量，正文仍由命令工厂缓存。
  if (!("rawJSON" in JSON) || typeof JSON.rawJSON !== "function") {
    // 直接交给 JSON.stringify，将负零写成 0。
    return equipment;
  }
  const negativeZero: unknown = JSON.rawJSON("-0.0");
  const stats: Record<string, AdminCanonicalEquipmentStatRequest> = Object.create(null);
  for (const [name, stat] of entries) {
    stats[name] = Object.is(stat.base, -0)
      ? Object.assign({}, stat, { toJSON: () => ({ value: stat.value, base: negativeZero }) })
      : stat;
  }
  return { ...equipment, stats: orderedStats(stats) };
}

/** 检查末次 `level` 的整数词法，以及被覆盖属性内的重复具名字段。 */
function validEquipmentJsonFields(text: string): boolean {
  const stack: (Set<string> | null)[] = [];
  let atRootLevel = false;
  let finalLevelNumber: string | null = null;
  const tokens = text.matchAll(
    /("(?:[^"\\]|\\.)*")([ \t\r\n]*:)?|(-?[0-9]+(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?)|[{}[\]]/gu,
  );
  for (const [token, quoted, colon, number] of tokens) {
    if (quoted !== undefined) {
      // 第二层的 stats 映射允许同名属性覆盖。
      if (colon === undefined || stack.length === 2) {
        continue;
      }
      const decoded = parseJson(quoted);
      if (!decoded.ok || typeof decoded.value !== "string") {
        return false;
      }
      if (stack.length === 1) {
        atRootLevel = decoded.value === "level";
        if (atRootLevel) {
          finalLevelNumber = null;
        }
        continue;
      }
      const seen = stack[stack.length - 1];
      if (seen?.has(decoded.value)) {
        return false;
      }
      seen?.add(decoded.value);
    } else if (number !== undefined) {
      if (stack.length === 1 && atRootLevel) {
        finalLevelNumber = number;
      }
    } else if (token === "{") {
      stack.push(new Set());
    } else if (token === "[") {
      stack.push(null);
    } else if (token === "}" || token === "]") {
      stack.pop();
    }
  }
  return finalLevelNumber === null || parseJson(finalLevelNumber, { numbers: "integers" }).ok;
}

/** `JSON.stringify` 会把 `"2"`、`"10"` 这类整数键按数值顺序重排；用 ownKeys 保持
 * `BTreeMap` 的码点顺序，使固定正文与 Rust 序列化一致。 */
function orderedStats(
  stats: Record<string, AdminCanonicalEquipmentStatRequest>,
): Record<string, AdminCanonicalEquipmentStatRequest> {
  const keys = Object.keys(stats).sort(compareText);
  return new Proxy(stats, {
    ownKeys: () => keys,
    getOwnPropertyDescriptor: (target, key) => {
      if (typeof key !== "string") {
        return undefined;
      }
      const descriptor = Reflect.getOwnPropertyDescriptor(target, key);
      return descriptor === undefined ? undefined : { ...descriptor, enumerable: true };
    },
  });
}

function asJsonObject(value: unknown): JsonObject | null {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? (value as JsonObject)
    : null;
}

function isOptionalString(value: unknown): boolean {
  return value === undefined || value === null || typeof value === "string";
}

function optionalString(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

function decodeIntakeResult(value: AdminIntakeResult): Result<AdminIntakeResult, string> {
  if (
    !(value.auction_sequence >= 1 && value.auction_sequence <= MAX_JSON_SAFE_INTEGER) ||
    !(value.auction_state_version >= 1 && value.auction_state_version <= MAX_JSON_SAFE_INTEGER) ||
    !validTrimmedText(value.equipment_category) ||
    (value.equipment_level !== null && value.equipment_level < 0) ||
    !validTrimmedText(value.equipment_name) ||
    !validTrimmedText(value.external_equipment_id) ||
    value.lot_number <= 0 ||
    !(value.lot_state_version >= 1 && value.lot_state_version <= MAX_JSON_SAFE_INTEGER) ||
    !(value.preview_revision >= 0 && value.preview_revision <= MAX_JSON_SAFE_INTEGER) ||
    !validTrimmedText(value.seller_player_id) ||
    !(value.start_price >= 1 && value.start_price <= MAX_JSON_SAFE_INTEGER)
  ) {
    return err("intake_result");
  }
  return ok(value);
}

/** Rust `str::trim` 的空白集合；JS 的 `trim` 与之不同，不能替换。 */
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
    code === 0x09 ||
    code === 0x0a ||
    code === 0x0b ||
    code === 0x0c ||
    code === 0x0d ||
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
