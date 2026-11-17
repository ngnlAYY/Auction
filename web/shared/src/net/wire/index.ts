// 浏览器 wire 格式的共享原语：与 serde_json 对齐的严格解析、安全整数边界与键集合谓词。
// 两端 decoder 都以这些谓词拒绝缺字段、未知字段和已丢失精度的整数，业务规则仍留在各自模块。

import { err, ok, type Result } from "../../result";

/** JSON（IEEE-754 double）能无损表达的最大整数；金额、计数与版本都不得超过它。 */
export const JSON_MAX_SAFE_INTEGER = 9_007_199_254_740_991;

/** [`JSON_MAX_SAFE_INTEGER`] 的浮点写法；JavaScript 两种写法得到同一个数。 */
export const JSON_MAX_SAFE_INTEGER_F64 = 9_007_199_254_740_991.0;

/** 键值对形式的 JSON 对象。 */
export type JsonObject = Record<string, unknown>;

/** 判断一个 `unknown` 是否为普通、非数组的 JSON 对象。 */
function jsonObject(value: unknown): JsonObject | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return null;
  }
  let prototype: unknown;
  try {
    prototype = Object.getPrototypeOf(value);
  } catch {
    return null;
  }
  if (prototype !== Object.prototype && prototype !== null) {
    return null;
  }
  // JSON 对象与显式无原型对象之外的类型均不接受；后续只按字符串键读取。
  return value as JsonObject;
}

/** 字符串中是否存在无法配对的 UTF-16 代理项。 */
function hasLoneSurrogate(text: string): boolean {
  for (let index = 0; index < text.length; index += 1) {
    const code = text.charCodeAt(index);
    if (code >= 0xd800 && code <= 0xdbff) {
      index += 1;
      if (index >= text.length) {
        return true;
      }
      const low = text.charCodeAt(index);
      if (low < 0xdc00 || low > 0xdfff) {
        return true;
      }
    } else if (code >= 0xdc00 && code <= 0xdfff) {
      return true;
    }
  }
  return false;
}

/** 结果中的每个数字有限，且每个字符串与对象键都是合法 Unicode。 */
function validStrictJsonValue(root: unknown): boolean {
  const pending: unknown[] = [root];
  while (pending.length > 0) {
    const value = pending.pop();
    if (typeof value === "number") {
      if (!Number.isFinite(value)) {
        return false;
      }
      continue;
    }
    if (typeof value === "string") {
      if (hasLoneSurrogate(value)) {
        return false;
      }
      continue;
    }
    if (Array.isArray(value)) {
      for (const item of value) {
        pending.push(item);
      }
      continue;
    }
    if (typeof value === "object" && value !== null) {
      const object = value as JsonObject;
      for (const key of Object.keys(object)) {
        if (hasLoneSurrogate(key)) {
          return false;
        }
        pending.push(object[key]);
      }
    }
  }
  return true;
}

/** 以 `serde_json` 的拒绝边界解析 JSON：非有限数与孤立代理项都属于无效输入。 */
export function parseJson(text: string): Result<unknown, "invalid_json"> {
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return err("invalid_json");
  }
  return validStrictJsonValue(value) ? ok(value) : err("invalid_json");
}

/** 对象的键必须与 `keys` 完全一致：不接受缺失键，也不接受多余键。 */
function jsonExactKeys(object: JsonObject, keys: readonly string[]): boolean {
  const objectKeys = Object.keys(object);
  return objectKeys.length === keys.length && keys.every((key) => Object.hasOwn(object, key));
}

/** 值必须是键集合与 `keys` 完全一致（不缺键、无多余键）的对象；否则为 `null`。 */
export function jsonExactObject(value: unknown, keys: readonly string[]): JsonObject | null {
  const object = jsonObject(value);
  return object !== null && jsonExactKeys(object, keys) ? object : null;
}

/** 值是否为键集合与 `keys` 完全一致的对象；布尔形式便于直接作为 decoder 谓词。 */
export function jsonHasExactKeys(value: unknown, keys: readonly string[]): boolean {
  const object = jsonObject(value);
  return object !== null && jsonExactKeys(object, keys);
}

/** 键集合闭包：`required` 必须全部出现，只允许 `required ∪ optional`，`optional` 可以缺省。 */
export function jsonKeysOnly(
  object: JsonObject,
  required: readonly string[],
  optional: readonly string[],
): boolean {
  return (
    required.every((key) => Object.hasOwn(object, key)) &&
    Object.keys(object).every((key) => required.includes(key) || optional.includes(key))
  );
}
