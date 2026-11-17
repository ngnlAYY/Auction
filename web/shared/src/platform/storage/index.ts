// 浏览器 `localStorage` 的容错读写。
// 存储不可用、被隐私设置禁用、配额不足或值损坏都只返回缺失/失败，不抛到当前页面；
// 本模块不持有键名，偏好与缓存的键由各 surface 自行声明。

import { parseJson } from "../../net/wire";

/** 单条本地 JSON 值的解析上限：拒绝被其他脚本写入或损坏的超大值，而不是部分解析。 */
const MAX_PARSED_BYTES = 2_000_000;

/** 取得可用存储；访问被安全策略阻止时视为存储不可用。 */
function availableStorage(): Storage | null {
  if (typeof window === "undefined") {
    return null;
  }
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

/** 读取原始字符串值；缺失或读取失败返回 `null`。 */
export function getItem(key: string): string | null {
  const storage = availableStorage();
  if (storage === null) {
    return null;
  }
  try {
    return storage.getItem(key);
  } catch {
    return null;
  }
}

/** 写入字符串值；成功返回 `true`，配额不足等失败返回 `false`。 */
export function setItem(key: string, value: string): boolean {
  const storage = availableStorage();
  if (storage === null) {
    return false;
  }
  try {
    storage.setItem(key, value);
    return true;
  } catch {
    return false;
  }
}

/** 删除单个键；键不存在或存储不可用时静默返回。 */
export function removeItem(key: string): void {
  const storage = availableStorage();
  if (storage === null) {
    return;
  }
  try {
    storage.removeItem(key);
  } catch {
    // 删除失败不影响当前页面；不覆盖或清理其他键。
  }
}

/** 当前存储中的全部键；按浏览器给出的顺序返回，存储不可用时为空。 */
export function keys(): readonly string[] {
  const storage = availableStorage();
  if (storage === null) {
    return [];
  }
  const result: string[] = [];
  let length = 0;
  try {
    length = storage.length;
  } catch {
    return [];
  }
  for (let index = 0; index < length; index += 1) {
    try {
      const key = storage.key(index);
      if (key !== null) {
        result.push(key);
      }
    } catch {
      // 单个键读取失败按缺失处理，继续读取其余键。
    }
  }
  return result;
}

/** 把解析出的 JSON 值收窄为调用方 DTO；形状不符时返回 `null`。 */
export type ParsedValueDecoder<T> = (value: unknown) => T | null;

/** 读取并反序列化 JSON 值；缺失、超过解析上限或格式损坏时返回 `null`。 */
export function getParsed(key: string): unknown | null;
export function getParsed<T>(key: string, decode: ParsedValueDecoder<T>): T | null;
export function getParsed<T>(key: string, decode?: ParsedValueDecoder<T>): T | unknown | null {
  const raw = getItem(key);
  if (raw === null || new TextEncoder().encode(raw).byteLength > MAX_PARSED_BYTES) {
    return null;
  }
  try {
    const parsed = parseJson(raw);
    if (!parsed.ok) {
      return null;
    }
    // 泛型本身不能提供运行期形状信息；调用方传入 decoder 时才把 JSON 当作 DTO 返回。
    return decode === undefined ? parsed.value : decode(parsed.value);
  } catch {
    return null;
  }
}
