// 表单模型与 JSON 文档之间的嵌套读写辅助。

import type { EquipmentTaxonomyDto } from "@auction/contracts/dto/admin/equipment";
import type { AdminJsonValue } from "@auction/contracts/dto/admin/rules";
import { err, ok, type Result } from "@auction/web-shared/result";

import { trimRustWhitespace } from "../../../api/session-time";

type JsonObject = Readonly<Record<string, AdminJsonValue>>;

/** 按点分路径读取嵌套值，并区分“找到叶子”和“路径不存在”。
 *
 * 叶子可以是显式 JSON `null`；中间层必须是普通对象，数组不能作为中间段。
 */
export function nestedValueFound(root: AdminJsonValue, path: string): Result<AdminJsonValue, null> {
  let current: AdminJsonValue = root;
  for (const key of path.split(".")) {
    if (!isJsonObject(current) || !Object.hasOwn(current, key)) {
      return err(null);
    }
    const next = current[key];
    if (next === undefined) {
      return err(null);
    }
    current = next;
  }
  return ok(current);
}

/** 只关心叶子值、且缺失与显式 JSON `null` 等价的调用方使用；需要区分时用 `nestedValueFound`。 */
export function nestedValue(root: AdminJsonValue, path: string): AdminJsonValue | null {
  const found = nestedValueFound(root, path);
  return found.ok ? found.value : null;
}

/** 按点分路径替换叶子值；中间路径缺失或不是普通对象时拒绝。 */
export function setNestedValue(
  root: AdminJsonValue,
  path: string,
  value: AdminJsonValue,
): Result<AdminJsonValue, null> {
  return setAt(root, path.split("."), 0, value);
}

function setAt(
  current: AdminJsonValue,
  segments: readonly string[],
  index: number,
  value: AdminJsonValue,
): Result<AdminJsonValue, null> {
  if (!isJsonObject(current)) {
    return err(null);
  }
  const key = segments[index];
  if (key === undefined) {
    return err(null);
  }
  if (index === segments.length - 1) {
    return ok({ ...current, [key]: value });
  }
  if (!Object.hasOwn(current, key)) {
    return err(null);
  }
  const next = current[key];
  if (next === undefined) {
    return err(null);
  }
  const child = setAt(next, segments, index + 1, value);
  if (!child.ok) {
    return child;
  }
  return ok({ ...current, [key]: child.value });
}

/** 字符串数组按行拼接；缺失、非数组或含非字符串项时拒绝。 */
export function listText(value: AdminJsonValue | null): Result<string, null> {
  if (!Array.isArray(value)) {
    return err(null);
  }
  const texts: string[] = [];
  for (const item of value) {
    if (typeof item !== "string") {
      return err(null);
    }
    texts.push(item);
  }
  return ok(texts.join("\n"));
}

/** 多选维度的编辑状态；旧文档的单值与多选投影冲突时保留原单值。 */
export interface ColorSelection {
  readonly text: string;
  readonly conflictingSingle: string | null;
  readonly resolved: "single" | "multiple" | null;
}

/** 读取单值/多选两个字段；多选非空时以多选为准，并记录待解决的单值冲突。 */
export function colorSelectionFromSource(
  object: JsonObject,
  plural: string,
  singular: string,
): Result<ColorSelection, null> {
  let scalar = "";
  if (Object.hasOwn(object, singular)) {
    const value = object[singular];
    if (typeof value !== "string") {
      return err(null);
    }
    scalar = value;
  }
  if (Object.hasOwn(object, plural)) {
    const value = object[plural];
    if (!Array.isArray(value)) {
      return err(null);
    }
    if (value.length > 0) {
      const text = listText(value);
      if (!text.ok) {
        return text;
      }
      return ok({
        text: text.value,
        conflictingSingle: scalar === "" ? null : scalar,
        resolved: null,
      });
    }
  }
  return ok({ text: scalar, conflictingSingle: null, resolved: null });
}

/** 没有待解决的单值冲突，且未明确选择多选时，仍按旧单值语法解释。 */
export function colorSelectionUsesLegacySyntax(selection: ColorSelection): boolean {
  return selection.conflictingSingle === null && selection.resolved !== "multiple";
}

/** 解决单值/多选冲突；只有存在冲突时才有变化。 */
export function colorSelectionResolve(
  selection: ColorSelection,
  keepSingle: boolean,
): ColorSelection {
  if (selection.conflictingSingle === null) {
    return selection;
  }
  const single = selection.conflictingSingle;
  return {
    text: keepSingle ? single : selection.text,
    conflictingSingle: null,
    resolved: keepSingle ? "single" : "multiple",
  };
}

/** 把选择写回规则对象；原投影中的冲突需由管理员解决，保留单值时仍按精确匹配解释旧材质。 */
export function colorSelectionWrite(
  selection: ColorSelection,
  object: JsonObject,
  source: JsonObject | null,
  plural: string,
  singular: string,
): Result<Record<string, AdminJsonValue>, null> {
  if (selection.resolved === "single" && objectString(object, singular) === selection.text) {
    return ok(withoutKey(object, plural));
  }
  const original = source === null ? null : colorSelectionFromSource(source, plural, singular);
  if (original !== null && !original.ok) {
    return original;
  }
  const sameValues = original?.ok === true && original.value.text === selection.text;
  if (sameValues && selection.resolved === null) {
    return ok({ ...object });
  }
  let next: Record<string, AdminJsonValue> = { ...object, [singular]: "" };
  if (!sameValues) {
    const selected = lines(selection.text);
    next = selected.length === 0 ? withoutKey(next, plural) : { ...next, [plural]: selected };
  }
  return ok(next);
}

/** 旧材质单值只在词典确认是材质时保留，供显示精确匹配的提示。 */
export function legacyMaterialSelection(
  object: JsonObject,
  taxonomy: EquipmentTaxonomyDto,
): string | null {
  const single = objectString(object, "equipment_type");
  if (single === null) {
    return null;
  }
  return taxonomy.materials.some((material) => eqIgnoreAsciiCase(material.name, single))
    ? single
    : null;
}

/** 把按行文本转成提交值：逐行 `str::lines`、去首尾空白并丢弃空行。 */
export function lines(value: string): AdminJsonValue[] {
  return rustLines(value)
    .map(trimRustWhitespace)
    .filter((line) => line !== "");
}

/** Rust `str::lines`：按 `\n` 分行，`\r\n` 去掉行尾 `\r`，末尾换行不产生空行。 */
function rustLines(value: string): string[] {
  const result: string[] = [];
  let start = 0;
  for (let index = 0; index < value.length; index += 1) {
    if (value.charAt(index) !== "\n") {
      continue;
    }
    let line = value.slice(start, index);
    if (line.endsWith("\r")) {
      line = line.slice(0, -1);
    }
    result.push(line);
    start = index + 1;
  }
  if (start < value.length) {
    result.push(value.slice(start));
  }
  return result;
}

function objectString(object: JsonObject, key: string): string | null {
  if (!Object.hasOwn(object, key)) {
    return null;
  }
  const value = object[key];
  return typeof value === "string" ? value : null;
}

function withoutKey(object: JsonObject, key: string): Record<string, AdminJsonValue> {
  const next: Record<string, AdminJsonValue> = { ...object };
  delete next[key];
  return next;
}

function isJsonObject(value: unknown): value is JsonObject {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** `eq_ignore_ascii_case` 只折叠 ASCII 大小写；非 ASCII 字符必须完全相同。 */
function eqIgnoreAsciiCase(left: string, right: string): boolean {
  if (left.length !== right.length) {
    return false;
  }
  for (let index = 0; index < left.length; index += 1) {
    if (asciiLower(left.charCodeAt(index)) !== asciiLower(right.charCodeAt(index))) {
      return false;
    }
  }
  return true;
}

function asciiLower(code: number): number {
  return code >= 0x41 && code <= 0x5a ? code + 0x20 : code;
}
