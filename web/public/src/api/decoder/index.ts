// Public 成功响应的 route-specific decoder 共享信封与基础谓词。
// 信封之后，每条 route 的 data 还要通过闭集键与跨字段谓词校验：先验证原始 JSON，再用
// contracts schema 解码，避免宽松反序列化把缺字段、未知字段或自相矛盾的投影送进组件。
// 按路由分组的 decoder 位于同目录其他模块；本文件只保留共享 envelope 解码与基础谓词原语。

import { decode } from "@auction/contracts/wire";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import type { JsonObject } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { reportBrowserError } from "@auction/web-shared/ui/bootstrap";
import type * as v from "valibot";

/** 解析成功 envelope，并在 schema 解码前执行 route-specific data 校验。 */
export function decodeSuccess<TSchema extends v.GenericSchema>(
  response: TransportResponse,
  schema: TSchema,
  validate: (value: unknown) => boolean,
): Result<v.InferOutput<TSchema>, string> {
  const payload = response.json();
  if (!payload.ok) {
    reportBrowserError("dto_decode");
    return err("invalid_response");
  }
  const decoded = decodeSuccessValue(payload.value, schema, validate);
  if (!decoded.ok) {
    reportBrowserError("dto_decode");
  }
  return decoded;
}

/** 解析旧共享 client 接受的错误 envelope；顶层扩展允许存在，但三项基础字段必须显式出现。 */
export function decodeError(
  response: TransportResponse,
): Result<readonly [string, unknown], string> {
  const payload = response.json();
  const decoded = payload.ok ? decodeErrorValue(payload.value) : err("invalid_response");
  if (!decoded.ok) {
    reportBrowserError("dto_decode");
  }
  return decoded;
}

/** 错误 envelope 无效时统一返回旧 client 的 `invalid_response`。 */
export function decodeErrorCode(response: TransportResponse): string {
  const decoded = decodeError(response);
  return decoded.ok ? decoded.value[0] : decoded.error;
}

function decodeSuccessValue<TSchema extends v.GenericSchema>(
  payload: unknown,
  schema: TSchema,
  validate: (value: unknown) => boolean,
): Result<v.InferOutput<TSchema>, string> {
  const envelope = object(payload);
  if (envelope === null) {
    return err("invalid_response");
  }
  if (
    string(envelope.code) === null ||
    string(envelope.message) === null ||
    !Object.hasOwn(envelope, "data")
  ) {
    return err("invalid_response");
  }
  const data = envelope.data;
  if (!validate(data)) {
    return err("invalid_response");
  }
  // JS 无法区分 1 与 1.0，也不接受超出安全范围的整数；旧整数规范化在这里是空操作。
  // `decode` 用 ok 标记成败，`null` 本身可以是合法输出（例如匿名会话读取）。
  const decoded = decode(schema, data);
  return decoded.ok ? ok(decoded.value) : err("invalid_response");
}

function decodeErrorValue(payload: unknown): Result<readonly [string, unknown], string> {
  const envelope = object(payload);
  if (envelope === null) {
    return err("invalid_response");
  }
  const code = string(envelope.code);
  if (code === null || string(envelope.message) === null) {
    return err("invalid_response");
  }
  if (!Object.hasOwn(envelope, "data")) {
    return err("invalid_response");
  }
  const decoded: readonly [string, unknown] = [code, envelope.data];
  return ok(decoded);
}

/** 可选键缺省，出现时必须是去首尾 Rust 空白的非空字符串。 */
export function optionalNonempty(value: JsonObject, key: string): boolean {
  return !Object.hasOwn(value, key) || nonempty(value[key]);
}

/** 可选键缺省，出现时必须是非负安全整数。 */
export function optionalNonnegative(value: JsonObject, key: string): boolean {
  return !Object.hasOwn(value, key) || nonnegative(value[key]);
}

/** 可选键缺省，出现时必须是可解析的时刻。 */
export function optionalTime(value: JsonObject, key: string): boolean {
  return !Object.hasOwn(value, key) || time(value[key]);
}

/** 可选键缺省，出现时必须是有限数字。 */
export function optionalFinite(value: JsonObject, key: string): boolean {
  return !Object.hasOwn(value, key) || finite(value[key]);
}

/** 可选键缺省，出现时必须是 SDAEIW 有序子序列。 */
export function optionalPrimaryAttributeInitials(value: JsonObject, key: string): boolean {
  if (!Object.hasOwn(value, key)) {
    return true;
  }
  const text = string(value[key]);
  return text !== null && validPrimaryAttributeInitials(text);
}

/** 可选键缺省，出现时必须是布尔值。 */
export function optionalBool(value: JsonObject, key: string): boolean {
  return !Object.hasOwn(value, key) || boolValue(value[key]) !== null;
}

/** 可选字符串；缺省、null 或非字符串都读作没有值。 */
export function optionalString(value: JsonObject, key: string): string | null {
  return string(value[key]);
}

/** 可选安全整数；缺省、null 或非整数都读作没有值。 */
export function optionalInt(value: JsonObject, key: string): number | null {
  return int(value[key]);
}

/** 普通、非数组的 JSON 对象；其他值返回 null。 */
export function object(value: unknown): JsonObject | null {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return null;
  }
  // 上面的检查已证明是对象；本模块只按字符串键读取 JSON 值。
  return value as JsonObject;
}

/** 字符串值；其他值返回 null。 */
export function string(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}

/** 安全整数；有限但带小数的兼容写法在 JS 中已经是同一个数。 */
export function int(value: unknown): number | null {
  return typeof value === "number" && Number.isSafeInteger(value) ? value : null;
}

/** 正安全整数。 */
export function positive(value: unknown): boolean {
  const decoded = int(value);
  return decoded !== null && decoded > 0;
}

/** 分页数量接受正安全整数；Rust 另接受完整正 i64，超过 2^53−1 的值在 JS 中无法精确表示，
 * 按计划约定以 `invalid_response` 失败关闭。 */
export function positivePageSize(value: unknown): boolean {
  return positive(value);
}

/** 非负安全整数。 */
export function nonnegative(value: unknown): boolean {
  const decoded = int(value);
  return decoded !== null && decoded >= 0;
}

/** revision 是非负版本序号。 */
export function revision(value: unknown): boolean {
  return nonnegative(value);
}

/** 布尔值；其他值返回 null。 */
export function boolValue(value: unknown): boolean | null {
  return typeof value === "boolean" ? value : null;
}

/** 有限数字。 */
export function finite(value: unknown): boolean {
  return typeof value === "number" && Number.isFinite(value);
}

/** 去除首尾 Rust 空白后仍非空，且不含首尾空白。 */
export function nonempty(value: unknown): boolean {
  const text = string(value);
  return text !== null && trimmedNonemptyStr(text);
}

/** 账户显示名只要求非空，允许首尾空白。 */
export function accountNonempty(value: unknown): boolean {
  const text = string(value);
  return text !== null && rawNonemptyStr(text);
}

/** 按调用方要求选择严格 trim 或账户名的宽松非空判断。 */
export function nonemptyMode(value: unknown, trimmed: boolean): boolean {
  return trimmed ? nonempty(value) : accountNonempty(value);
}

/** 非空且没有首尾 Rust 空白。 */
export function trimmedNonemptyStr(value: string): boolean {
  return value.length > 0 && isRustTrimmed(value);
}

/** 只要求非空。 */
export function rawNonemptyStr(value: string): boolean {
  return value.length > 0;
}

/** 可解析的时刻；wasm32 分支使用浏览器的 `Date.parse`。 */
export function time(value: unknown): boolean {
  const text = string(value);
  return text !== null && text.length > 0 && !Number.isNaN(Date.parse(text));
}

/** 时刻对应的毫秒数；无法解析时返回 null。 */
export function timeValue(value: unknown): number | null {
  const text = string(value);
  if (text === null) {
    return null;
  }
  const timestamp = Date.parse(text);
  return Number.isNaN(timestamp) ? null : timestamp;
}

/** 世界 ID：1–64 字节，已 trim，只含小写字母、数字、下划线与连字符。 */
export function world(value: unknown): boolean {
  const text = string(value);
  return (
    text !== null &&
    text.length > 0 &&
    utf8ByteLength(text) <= 64 &&
    isRustTrimmed(text) &&
    [...text].every((character) => isWorldCharacter(character))
  );
}

/** 代理模式闭集。 */
export function proxyMode(value: unknown): boolean {
  const mode = string(value);
  return mode === "fast" || mode === "slow";
}

/** 代理状态闭集。 */
export function proxyStatus(value: unknown): boolean {
  const status = string(value);
  return (
    status === "active" || status === "exhausted" || status === "frozen" || status === "closed"
  );
}

/** 装备主属性首字母必须是 SDAEIW 的有序子序列。 */
export function validPrimaryAttributeInitials(value: string): boolean {
  const ordered = "SDAEIW";
  let cursor = 0;
  if (value.length === 0) {
    return false;
  }
  for (let index = 0; index < value.length; index += 1) {
    const position = ordered.indexOf(value.charAt(index), cursor);
    if (position < 0) {
      return false;
    }
    cursor = position + 1;
  }
  return true;
}

/** 论坛帖子链接必须绑定规范 topic 与帖号，且锚点与帖号一致。 */
export function forumPostUrl(value: string): boolean {
  const prefix = "https://forums.e-hentai.org/index.php?showtopic=";
  if (!value.startsWith(prefix)) {
    return false;
  }
  const rest = value.slice(prefix.length);
  const entryIndex = rest.indexOf("#entry");
  if (entryIndex < 0) {
    return false;
  }
  const topicAndPost = rest.slice(0, entryIndex);
  const entryId = rest.slice(entryIndex + "#entry".length);
  const postIndex = topicAndPost.indexOf("&p=");
  if (postIndex < 0) {
    return false;
  }
  const threadId = topicAndPost.slice(0, postIndex);
  const postId = topicAndPost.slice(postIndex + "&p=".length);
  return canonicalForumId(threadId) && canonicalForumId(postId) && postId === entryId;
}

/** 论坛 ID：1–32 字节 ASCII 数字，不以 0 开头。 */
export function canonicalForumId(value: string): boolean {
  return (
    value.length > 0 &&
    utf8ByteLength(value) <= 32 &&
    !value.startsWith("0") &&
    [...value].every((character) => isAsciiDigit(character))
  );
}

/** 服务端必须交付真实 ID/key URL；仅为历史投影兼容保留空字符串解码。 */
export function gameUrl(value: unknown): boolean {
  const text = string(value);
  if (text === null) {
    return false;
  }
  if (text === "") {
    return true;
  }
  try {
    const url = new URL(text);
    const path = text.startsWith("https://hentaiverse.org/equip/")
      ? text.slice("https://hentaiverse.org/equip/".length)
      : text.startsWith("https://hentaiverse.org/isekai/equip/")
        ? text.slice("https://hentaiverse.org/isekai/equip/".length)
        : null;
    if (path === null) {
      return false;
    }
    const parts = path.split("/");
    const equipmentId = parts[0] ?? "";
    const key = parts[1] ?? "";
    return (
      url.protocol === "https:" &&
      url.hostname === "hentaiverse.org" &&
      url.username === "" &&
      url.password === "" &&
      // URL API 不区分“没有 query/fragment”和“空 query/fragment”，原始文本另行核对。
      !text.includes("?") &&
      !text.includes("#") &&
      url.search === "" &&
      url.hash === "" &&
      equipmentId.length > 0 &&
      utf8ByteLength(equipmentId) <= 32 &&
      [...equipmentId].every((character) => isAsciiDigit(character)) &&
      utf8ByteLength(key) === 10 &&
      [...key].every((character) => isAsciiHexDigit(character)) &&
      parts.length === 2
    );
  } catch {
    return false;
  }
}

const RUST_WHITESPACE =
  /^[\t\n\v\f\r \u0085\u00a0\u1680\u2000-\u200a\u2028\u2029\u202f\u205f\u3000]$/;

function isRustWhitespace(character: string): boolean {
  return RUST_WHITESPACE.test(character);
}

function isRustTrimmed(value: string): boolean {
  return !isRustWhitespace(value.charAt(0)) && !isRustWhitespace(value.charAt(value.length - 1));
}

function isAsciiDigit(character: string): boolean {
  return isCodePointInRange(character, 0x30, 0x39);
}

function isAsciiHexDigit(character: string): boolean {
  return (
    isAsciiDigit(character) ||
    isCodePointInRange(character, 0x61, 0x66) ||
    isCodePointInRange(character, 0x41, 0x46)
  );
}

function isWorldCharacter(character: string): boolean {
  return (
    isCodePointInRange(character, 0x61, 0x7a) ||
    isAsciiDigit(character) ||
    character === "_" ||
    character === "-"
  );
}

function isCodePointInRange(character: string, minimum: number, maximum: number): boolean {
  const code = character.codePointAt(0);
  return code !== undefined && code >= minimum && code <= maximum;
}
