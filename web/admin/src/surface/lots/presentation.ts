// Admin lot 页面状态、编码和安全展示辅助。

import { EQUIPMENT_CATEGORY_CODES } from "@auction/contracts/dto/public/catalog/lot";
import { unreachable } from "@auction/web-shared/result";
import { compareText } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";

import { equipmentAttributesSource } from "../../api/lots";
import type { LotUpdateNotice } from "./state";

/** 镜像 auction-domain 的 MAX_BID_AMOUNT，已存起拍价的读取边界独立保留。 */
export const MAX_START_PRICE: number = 1_000_000_000;

/** 新起拍价必须允许玩家报出至少一口合法价格。 */
export function parseStartPrice(value: string): number | null {
  // Rust `str::parse::<i64>` 只接受可选符号加十进制数字；不能使用会接受小数、指数或空串的
  // `parseInt` / `Number`，否则浏览器会提交服务端不会产生的价格。
  const text = rustTrim(value);
  if (!/^[+-]?[0-9]+$/u.test(text)) {
    return null;
  }
  let parsed: bigint;
  try {
    parsed = BigInt(text);
  } catch {
    return null;
  }
  if (parsed < 1n || parsed > BigInt(MAX_START_PRICE)) {
    return null;
  }
  return Number(parsed);
}

/** 拍品修改成功通知的展示文案。 */
export function lotUpdateNoticeLabel(notice: LotUpdateNotice): string {
  switch (notice) {
    case "repriced":
      return t("admin-lots-reprice-done");
    case "withdrawn":
      return t("admin-lots-withdraw-done");
    default:
      return unreachable(notice);
  }
}

/** 按旧版拍卖页面的约定生成稳定的拍品编码：契约表前缀加至少两位补零序号，
 * 未知分类显示 `Lot` 前缀，与 Public 页面、邮件和论坛代码一致。 */
export function adminLotCodeFor(category: string, lotNumber: number): string {
  const entry = EQUIPMENT_CATEGORY_CODES.find((candidate) => candidate[1] === category);
  const prefix = entry === undefined ? "Lot" : entry[2];
  return `${prefix}${String(lotNumber).padStart(2, "0")}`;
}

/** 将提交编号压缩为旧版列表中的可读后缀，同时保留完整值供悬浮提示查看。 */
export function submissionIdDisplay(value: string): string {
  // 旧版正则只在完整值末尾匹配 ASCII 数字；没有匹配时必须回退完整值。
  // 以 Unicode 字符而非 UTF-16 码元计数，尾部非数字回退才与 Rust 的字符边界一致。
  const characters = Array.from(value);
  let suffixStart = characters.length;
  while (suffixStart > 0) {
    const character = characters[suffixStart - 1];
    if (character === undefined || !/^[0-9]$/u.test(character)) {
      break;
    }
    suffixStart -= 1;
  }
  const fallback = characters.slice(-8).join("");
  if (suffixStart === characters.length) {
    return `#${fallback}`;
  }
  const stripped = characters.slice(suffixStart).join("").replace(/^0+/u, "");
  return `#${stripped === "" ? fallback : stripped}`;
}

/** 代理授权模式的展示文案；未知模式保留服务端原值。 */
export function proxyModeLabel(mode: string): string {
  switch (mode) {
    case "fast":
      return t("admin-lots-proxy-mode-fast");
    case "slow":
      return t("admin-lots-proxy-mode-slow");
    default:
      return mode;
  }
}

/** 代理授权状态的展示文案；未知状态保留服务端原值。 */
export function proxyStatusLabel(status: string): string {
  switch (status) {
    case "active":
      return t("admin-lots-proxy-status-active");
    case "final_fast":
      return t("admin-lots-proxy-status-final-fast");
    case "exhausted":
      return t("admin-lots-proxy-status-exhausted");
    case "frozen":
      return t("admin-lots-proxy-status-frozen");
    case "closed":
      return t("admin-lots-proxy-status-closed");
    default:
      return status;
  }
}

/** 装备名称与属性详情；属性优先复用响应原文，保留 `serde_json` 的数值词法与键序。 */
export function equipmentDetails(name: string, value: unknown): string {
  const source = equipmentAttributesSource(value);
  const details = source === null ? formatJsonValue(value, 0) : prettyPrintedJson(source);
  return `${name}\n${details}`;
}

/** 无原文时按 Rust `serde_json::to_string_pretty` 的缩进与键序格式化数值。 */
function formatJsonValue(value: unknown, depth: number): string {
  const indent = "  ".repeat(depth);
  const childIndent = "  ".repeat(depth + 1);
  if (value === null) {
    return "null";
  }
  if (typeof value === "string") {
    return JSON.stringify(value);
  }
  if (typeof value === "number") {
    return Number.isFinite(value) ? JSON.stringify(value) : "null";
  }
  if (typeof value === "boolean") {
    return value ? "true" : "false";
  }
  if (Array.isArray(value)) {
    if (value.length === 0) {
      return "[]";
    }
    const items = value.map((item) => `${childIndent}${formatJsonValue(item, depth + 1)}`);
    return `[\n${items.join(",\n")}\n${indent}]`;
  }
  if (typeof value === "object") {
    // 运行期已排除数组与 null；调用方只传入 JSON 解码得到的普通对象。
    const object = value as Readonly<Record<string, unknown>>;
    const keys = Object.keys(object).sort(compareText);
    if (keys.length === 0) {
      return "{}";
    }
    const entries = keys.map(
      (key) => `${childIndent}${JSON.stringify(key)}: ${formatJsonValue(object[key], depth + 1)}`,
    );
    return `{\n${entries.join(",\n")}\n${indent}}`;
  }
  return JSON.stringify(value) ?? String(value);
}

/** 传输原文是 compact JSON；数值、字符串与成员顺序原样保留，只替换结构空白。 */
function prettyPrintedJson(text: string): string {
  let output = "";
  let depth = 0;
  let index = 0;
  while (index < text.length) {
    const character = text[index];
    if (character === '"') {
      const end = jsonStringEnd(text, index);
      output += text.slice(index, end);
      index = end;
      continue;
    }
    switch (character) {
      case "{":
      case "[": {
        const closing = character === "{" ? "}" : "]";
        const next = nextNonWhitespace(text, index + 1);
        if (text[next] === closing) {
          output += `${character}${closing}`;
          index = next + 1;
          break;
        }
        depth += 1;
        output += `${character}\n${"  ".repeat(depth)}`;
        index += 1;
        break;
      }
      case "}":
      case "]":
        depth -= 1;
        output += `\n${"  ".repeat(depth)}${character}`;
        index += 1;
        break;
      case ",":
        output += `,\n${"  ".repeat(depth)}`;
        index += 1;
        break;
      case ":":
        output += ": ";
        index += 1;
        break;
      default:
        if (character === " " || character === "\t" || character === "\r" || character === "\n") {
          index += 1;
        } else {
          output += character;
          index += 1;
        }
    }
  }
  return output;
}

function nextNonWhitespace(text: string, start: number): number {
  let index = start;
  while (index < text.length) {
    const character = text[index];
    if (character !== " " && character !== "\t" && character !== "\r" && character !== "\n") {
      break;
    }
    index += 1;
  }
  return index;
}

function jsonStringEnd(text: string, start: number): number {
  let index = start + 1;
  while (index < text.length) {
    const character = text[index];
    if (character === "\\") {
      index += 2;
      continue;
    }
    if (character === '"') {
      return index + 1;
    }
    index += 1;
  }
  return text.length;
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
