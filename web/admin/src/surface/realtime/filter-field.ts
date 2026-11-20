// 实时与审计共用的地址筛选值、校验和请求门禁。

import type { QueryPair } from "@auction/web-shared/net/query";
import { err, type Result, unreachable } from "@auction/web-shared/result";

import { trimRustWhitespace } from "../../api/session-time";
import { validSafeText } from "../../api/validation";

/** 表单草稿或已恢复的筛选。无效地址保留每个原值，直到用户明确编辑并提交。 */
export type FilterValue =
  | { readonly kind: "text"; readonly value: string }
  | { readonly kind: "invalid"; readonly values: readonly string[] };

/** 默认的空文本筛选。 */
export function filterValueDefault(): FilterValue {
  return { kind: "text", value: "" };
}

/** 用户编辑后的文本筛选。 */
export function filterValueFromText(value: string): FilterValue {
  return { kind: "text", value };
}

/** 仅单值可以通过校验。重复值的顺序、空值和相同值都保留。 */
export function filterValueRestore(
  pairs: readonly (readonly [string, string])[],
  key: string,
  normalize: (text: string) => string | null,
): FilterValue {
  const values: string[] = [];
  for (const [name, value] of pairs) {
    if (name === key) {
      values.push(value);
    }
  }
  const first = values[0];
  if (first === undefined) {
    return filterValueDefault();
  }
  if (values.length > 1) {
    return { kind: "invalid", values };
  }
  const normalized = normalize(first);
  return normalized === null
    ? { kind: "invalid", values: [first] }
    : { kind: "text", value: normalized };
}

/** 无效地址不能从其中择一发起请求，即使该值单独看是合法的。 */
export function filterValueText(value: FilterValue): string | null {
  return value.kind === "text" ? value.value : null;
}

/** 保留单个原值供编辑，其余原值在字段旁显示。 */
export function filterValueInputValue(value: FilterValue): string {
  if (value.kind === "text") {
    return value.value;
  }
  return value.values[0] ?? "";
}

/** 获得要在错误区域显示的原值，提交校验失败的草稿也走相同的展示路径。 */
export function filterValueErrorValues(value: FilterValue, invalidDraft: boolean): string[] {
  if (value.kind === "invalid") {
    return [...value.values];
  }
  return invalidDraft ? [value.value] : [];
}

/** 其他筛选或分页写回时，保留尚未修正的无效参数。 */
export function filterValueAppendPairs(value: FilterValue, pairs: QueryPair[], key: string): void {
  if (value.kind === "text") {
    if (value.value === "") {
      return;
    }
    pairs.push([key, value.value]);
    return;
  }
  for (const original of value.values) {
    pairs.push([key, original]);
  }
}

/** 浏览器会净化部分控制字符，须在裁剪空白之前拒绝它们。 */
export function normalizeFilterInput(input: string): string | null {
  // Rust 的 `char::is_control` 是 Unicode 类别 Cc；在裁剪之前检查。
  if (/\p{Cc}/u.test(input)) {
    return null;
  }
  return trimRustWhitespace(input);
}

/** 可选文本允许为空，非空时按服务端的 UTF-8 字节上限校验。 */
export function normalizeFilterText(input: string, maximum: number): string | null {
  const value = normalizeFilterInput(input);
  if (value === null) {
    return null;
  }
  return value === "" || validSafeText(value, maximum) ? value : null;
}

/** 无效的已应用条件不能变成全量请求，刷新和轮询也走同一门禁。 */
export async function fetchFiltered<Q, T>(
  request: Q | null,
  fetch: (request: Q) => Promise<Result<T, string>>,
): Promise<Result<T, string>> {
  if (request === null) {
    return err("invalid_query");
  }
  return fetch(request);
}

/** 事件及失败查询接受的筛选字段。 */
export type FilterField =
  | "event_stream"
  | "event_kind"
  | "failure_stream"
  | "failure_consumer"
  | "failure_code"
  | "failure_state";

/** 空白输入表示未设筛选，非空值按应用端口的字节和字符规则校验。 */
export function filterFieldNormalize(field: FilterField, input: string): string | null {
  let maximum: number;
  switch (field) {
    case "event_stream":
    case "failure_stream":
      maximum = 256;
      break;
    case "event_kind":
    case "failure_consumer":
      maximum = 128;
      break;
    case "failure_code":
      maximum = 64;
      break;
    case "failure_state": {
      const value = normalizeFilterInput(input);
      if (value === null) {
        return null;
      }
      return value === "" || value === "dead_letter" || value === "completed" ? value : null;
    }
    default:
      return unreachable(field);
  }
  const value = normalizeFilterText(input, maximum);
  if (value === null) {
    return null;
  }
  // 失败代码不含空白；Rust 的空白集合与 JS 的正则不共享。
  return field === "failure_code" && containsRustWhitespace(value) ? null : value;
}

/** 筛选字段在表单中的稳定 ID。 */
export function filterFieldInputId(field: FilterField): string {
  switch (field) {
    case "event_stream":
      return "realtime-event-stream";
    case "event_kind":
      return "realtime-event-kind";
    case "failure_stream":
      return "realtime-failure-stream";
    case "failure_consumer":
      return "realtime-failure-consumer";
    case "failure_code":
      return "realtime-failure-code";
    case "failure_state":
      return "outbox-status";
    default:
      return unreachable(field);
  }
}

function containsRustWhitespace(value: string): boolean {
  for (const character of value) {
    if (isRustWhitespace(character.codePointAt(0) ?? 0)) {
      return true;
    }
  }
  return false;
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
