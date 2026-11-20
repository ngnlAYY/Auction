// 运行配置 JSON 的结构校验、字段读取与响应错误构造。

import type { JsonObject } from "@auction/web-shared/net/wire";

import { type AdminApiFailure, adminApiFailureClient } from "../failure";
import { MAX_JSON_SAFE_INTEGER, validSafeText } from "../validation";

/** JSON 对象；解析产物只来自 `parseJson`，不会是数组或带自定义原型的对象。 */
function asJsonObject(value: unknown): JsonObject | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return null;
  }
  return value as JsonObject;
}

/** `bot_players` 等兼容字段可以缺省，但出现时仍必须是闭集内已知键。 */
export function optionalKeysObject(value: unknown, keys: readonly string[]): JsonObject | null {
  const object = asJsonObject(value);
  if (object === null) {
    return null;
  }
  return Object.keys(object).every((key) => keys.includes(key)) ? object : null;
}

/** 读取布尔字段；缺键或类型不符返回 `null`。 */
export function boolField(object: JsonObject, key: string): boolean | null {
  const value = object[key];
  return typeof value === "boolean" ? value : null;
}

/** 读取字符串字段；缺键或类型不符返回 `null`。 */
export function stringField(object: JsonObject, key: string): string | null {
  const value = object[key];
  return typeof value === "string" ? value : null;
}

/** 读取非负安全整数字段；缺键、越界或类型不符返回 `null`。 */
export function u64Field(object: JsonObject, key: string): number | null {
  return safeU64(object[key]);
}

/** JSON u64 投影：只接受 0 到安全整数上限的整数。 */
export function safeU64(value: unknown): number | null {
  return typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value >= 0 &&
    value <= MAX_JSON_SAFE_INTEGER
    ? value
    : null;
}

/** JSON i64 投影：只接受安全范围内的整数。 */
export function safeI64(value: unknown): number | null {
  return typeof value === "number" &&
    Number.isSafeInteger(value) &&
    value >= -MAX_JSON_SAFE_INTEGER &&
    value <= MAX_JSON_SAFE_INTEGER
    ? value
    : null;
}

/** 值必须是由字符串组成的数组。 */
export function validStringArray(value: unknown): boolean {
  return Array.isArray(value) && value.every((item) => typeof item === "string");
}

/** 响应只核对字段闭集、类型和安全整数，取值规则由服务端校验候选时裁定。 */
export function validScalars(
  value: unknown,
  booleans: readonly string[],
  strings: readonly string[],
  numbers: readonly string[],
  nested: readonly string[],
): boolean {
  return validScalarsWithSigned(value, booleans, strings, numbers, [], nested);
}

/** 有符号字段可携带安全范围内的负数，业务范围留给表单和服务端。 */
export function validScalarsWithSigned(
  value: unknown,
  booleans: readonly string[],
  strings: readonly string[],
  unsigned: readonly string[],
  signed: readonly string[],
  nested: readonly string[],
): boolean {
  const object = asJsonObject(value);
  if (object === null) {
    return false;
  }
  return (
    Object.keys(object).length ===
      booleans.length + strings.length + unsigned.length + signed.length + nested.length &&
    nested.every((name) => Object.hasOwn(object, name)) &&
    booleans.every((name) => boolField(object, name) !== null) &&
    strings.every((name) => stringField(object, name) !== null) &&
    unsigned.every((name) => u64Field(object, name) !== null) &&
    signed.every((name) => safeI64(object[name]) !== null)
  );
}

/** 文本非空（或允许空）、UTF-8 长度与首尾空白/控制字符符合安全文本边界。 */
export function validText(value: string, maximumBytes: number, allowEmpty: boolean): boolean {
  if (value.length === 0) {
    return allowEmpty;
  }
  return validSafeText(value, maximumBytes);
}

/** fingerprint 必须是 `sha256:` 加 64 位小写十六进制。 */
export function validFingerprint(value: string): boolean {
  return (
    value.length === 71 && value.startsWith("sha256:") && /^[0-9a-f]{64}$/u.test(value.slice(7))
  );
}

/** 响应结构不符合闭集或运行时规则；与 Rust 一样不额外上报 DTO 解码信标。 */
export function invalidRuntimeResponse(): AdminApiFailure {
  return adminApiFailureClient("invalid_response");
}

/** 读取字段值；缺键或非对象读到 `null`，与 Rust 的 `Option` 缺值一致。 */
export function fieldValue(value: unknown, key: string): unknown {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return null;
  }
  return Object.hasOwn(value, key) ? (value as JsonObject)[key] : null;
}

/** 与 Rust 的 `value.get(key).is_some()` 等价：非对象视为缺键。 */
export function hasField(value: unknown, key: string): boolean {
  return typeof value === "object" && value !== null && !Array.isArray(value)
    ? Object.hasOwn(value, key)
    : false;
}
