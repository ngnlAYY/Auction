// 手工录入表单的本地校验与 Rust 语义辅助函数。
// 这些函数不接触组件状态；表单负责收集输入、冻结提交参数并驱动界面更新。

import type { AdminCanonicalEquipmentStatRequest } from "@auction/contracts/dto/admin/auction";
import { unreachable } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import { t } from "@auction/web-shared/ui/i18n";

import { parseCanonicalEquipment } from "../../api/intake";
import { MAX_START_PRICE } from "../lots/presentation";

/** 录入表单各字段的校验结果；null 表示该字段当前无错误。
 * 来源字段（URL 或 JSON）共用一个错误位，随来源类型切换到对应控件。 */
export interface IntakeFieldErrors {
  source: IntakeFieldIssue | null;
  price: IntakeFieldIssue | null;
  player: IntakeFieldIssue | null;
}

/** 建立无错误的录入字段状态。 */
export function emptyIntakeFieldErrors(): IntakeFieldErrors {
  return { source: null, price: null, player: null };
}

/** 判断录入字段是否存在需要提示的错误。 */
export function intakeFieldErrorsAny(errors: IntakeFieldErrors): boolean {
  return errors.source !== null || errors.price !== null || errors.player !== null;
}

/** 表单序上第一个错误字段的控件 id；来源字段按当前来源类型取 URL 或 JSON 控件。 */
export function intakeFieldErrorsFirstInvalidId(
  errors: IntakeFieldErrors,
  sourceKind: string,
): string | null {
  if (errors.source !== null) {
    return sourceKind === "url" ? "auction-intake-url" : "auction-intake-json";
  }
  if (errors.price !== null) {
    return "auction-intake-price";
  }
  if (errors.player !== null) {
    return "auction-intake-player";
  }
  return null;
}

/** 字段错误的分类；渲染时翻译，保证切换语言后已展示的错误同步更新。 */
export type IntakeFieldIssue =
  | "required"
  | "invalid_json"
  | "invalid_price"
  /** 服务端返回卖家已被拉黑。 */
  | "player_blacklisted"
  /** 服务端返回装备冲突。 */
  | "equipment_conflict";

function intakeFieldIssueMessage(issue: IntakeFieldIssue): string {
  switch (issue) {
    case "required":
      return t("admin-field-required");
    case "invalid_json":
      return t("admin-intake-invalid-json");
    case "invalid_price":
      return t("admin-lots-invalid-price", { max: MAX_START_PRICE });
    case "player_blacklisted":
      return t("admin-error-auction-intake-player-blacklisted");
    case "equipment_conflict":
      return t("admin-error-equipment-conflict");
    default:
      return unreachable(issue);
  }
}

/** 将存在的字段错误按当前语言翻译。 */
export function optionalIntakeFieldIssueMessage(issue: IntakeFieldIssue | null): string | null {
  return issue === null ? null : intakeFieldIssueMessage(issue);
}

const CONTROL_CHARACTER = /\p{Cc}/u;

/** 在请求发送前复现旧版对 canonical JSON 的结构校验，避免把明显错误的 JSON 交给网络层。 */
export function validCanonicalEquipmentJson(value: string): boolean {
  const parsed = parseCanonicalEquipment(value, true);
  if (!parsed.ok) {
    return false;
  }
  const equipment = parsed.value;
  const externalEquipmentId = equipment.external_equipment_id;
  if (
    typeof externalEquipmentId !== "string" ||
    externalEquipmentId === "" ||
    utf8ByteLength(externalEquipmentId) > 64 ||
    !/^[0-9]*$/u.test(externalEquipmentId)
  ) {
    return false;
  }
  for (const [key, maximum] of [
    ["name", 512],
    ["category", 128],
    ["equipment_type", 128],
    ["quality", 128],
  ] as const) {
    if (!canonicalText(equipment[key], maximum)) {
      return false;
    }
  }
  for (const key of ["prefix", "suffix"] as const) {
    if (!canonicalOptionalText(equipment[key], 128)) {
      return false;
    }
  }
  const level = equipment.level;
  const levelValid = level === null || (level >= 1 && level <= 100_000);
  if (!levelValid || typeof equipment.tradeable !== "boolean") {
    return false;
  }
  const stats = equipment.stats;
  if (stats === null || Object.keys(stats).length > 256) {
    return false;
  }
  if (!Object.entries(stats).every(([name, stat]) => validCanonicalStat(name, stat))) {
    return false;
  }
  const key = equipment.key;
  return typeof key === "string" && utf8ByteLength(key) === 10 && /^[0-9A-Fa-f]{10}$/u.test(key);
}

function canonicalText(value: unknown, maximum: number): boolean {
  return (
    typeof value === "string" &&
    value !== "" &&
    utf8ByteLength(value) <= maximum &&
    value === rustTrim(value) &&
    !CONTROL_CHARACTER.test(value)
  );
}

function canonicalOptionalText(value: unknown, maximum: number): boolean {
  return (
    typeof value === "string" &&
    utf8ByteLength(value) <= maximum &&
    value === rustTrim(value) &&
    !CONTROL_CHARACTER.test(value)
  );
}

function validCanonicalStat(name: string, stat: AdminCanonicalEquipmentStatRequest): boolean {
  return canonicalText(name, 128) && canonicalText(stat.value, 256);
}

/** Rust `str::trim` 的空白集合；JS 的 `trim` 与之不同，不能替换。 */
export function rustTrim(value: string): string {
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
