// 运行配置表单到服务端完整闭集文档的序列化；空白或非法的中间态一律拒绝提交。

import type { AdminJsonValue } from "@auction/contracts/dto/admin/rules";
import { structuralEquals } from "@auction/web-shared/reactive/equality";
import { err, ok, type Result } from "@auction/web-shared/result";

import { MAX_JSON_SAFE_INTEGER } from "../../../api/validation";
import {
  ensureBlock as earlyBirdEnsureBlock,
  field as earlyBirdField,
  inputPaths as earlyBirdInputPaths,
  submittedValue as earlyBirdSubmittedValue,
} from "../early-bird";
import {
  LIST_FIELDS as OPERATIONAL_LIST_FIELDS,
  SIGNED_NUMBER_FIELDS as OPERATIONAL_SIGNED_NUMBER_FIELDS,
  TEXT_FIELDS as OPERATIONAL_TEXT_FIELDS,
  UNSIGNED_NUMBER_FIELDS as OPERATIONAL_UNSIGNED_NUMBER_FIELDS,
} from "../operational-fields";
import type { RuntimeEquipmentColorForm } from "./equipment-colors";
import {
  LIST_FIELDS,
  MAX_DASHBOARD_MAILBOXES,
  type RuntimeConfigForm,
  runtimeConfigFormEditedInput,
  runtimeConfigFormEditsField,
  SIGNED_NUMBER_FIELDS,
  TEXT_FIELDS,
  UNSIGNED_NUMBER_FIELDS,
} from "./index";
import type { RuntimeMailboxForm } from "./mailboxes";
import {
  type ColorSelection,
  colorSelectionWrite,
  lines,
  listText,
  nestedValue,
  setNestedValue,
} from "./nested";

type JsonObject = Readonly<Record<string, AdminJsonValue>>;

/** 生成服务端要求的完整闭集文档；任一数字处于空白或非法中间态时拒绝提交。 */
export function runtimeConfigFormToDocument(form: RuntimeConfigForm): Result<AdminJsonValue, null> {
  // 校验与写入都以读取时的投影为底。未编辑的值不经输入框往返，避免保存时改写兼容配置。
  const cloned = cloneDocument(form.document);
  if (cloned === null) {
    return err(null);
  }
  let document: AdminJsonValue = cloned;
  for (const path of [...TEXT_FIELDS, ...OPERATIONAL_TEXT_FIELDS]) {
    const input = runtimeConfigFormEditedInput(form, path);
    if (input !== null) {
      const updated = setNestedValue(document, path, input);
      if (!updated.ok) {
        return updated;
      }
      document = updated.value;
    }
  }
  // 模式变化派生的清理仅作用于候选，撤销模式编辑时保留原值和用户显式的输入。
  const mode = runtimeConfigFormEditedInput(form, "listener.mode");
  const inactiveListenerFields: readonly string[] =
    mode === "tcp"
      ? ["listener.group", "listener.runtime_directory", "listener.socket_path"]
      : mode === "unix"
        ? ["listener.address"]
        : [];
  for (const path of inactiveListenerFields) {
    const updated = setNestedValue(document, path, "");
    if (!updated.ok) {
      return updated;
    }
    document = updated.value;
  }
  for (const path of [...LIST_FIELDS, ...OPERATIONAL_LIST_FIELDS]) {
    const input = runtimeConfigFormEditedInput(form, path);
    if (input !== null) {
      const updated = setNestedValue(document, path, lines(input));
      if (!updated.ok) {
        return updated;
      }
      document = updated.value;
    }
  }
  for (const path of [...UNSIGNED_NUMBER_FIELDS, ...OPERATIONAL_UNSIGNED_NUMBER_FIELDS]) {
    const input = runtimeConfigFormEditedInput(form, path);
    if (input !== null) {
      const value = parseRustU64(input);
      if (value === null) {
        return err(null);
      }
      const updated = setNestedValue(document, path, value);
      if (!updated.ok) {
        return updated;
      }
      document = updated.value;
    }
  }
  for (const path of [...SIGNED_NUMBER_FIELDS, ...OPERATIONAL_SIGNED_NUMBER_FIELDS]) {
    const input = runtimeConfigFormEditedInput(form, path);
    if (input !== null) {
      const value = parseRustI64(input);
      if (value === null) {
        return err(null);
      }
      const updated = setNestedValue(document, path, value);
      if (!updated.ok) {
        return updated;
      }
      document = updated.value;
    }
  }
  for (const path of earlyBirdInputPaths()) {
    const input = runtimeConfigFormEditedInput(form, path);
    if (input !== null) {
      const name = earlyBirdField(path);
      if (name === null) {
        return err(null);
      }
      const block = earlyBirdEnsureBlock(document);
      if (!block.ok) {
        return block;
      }
      const value = earlyBirdSubmittedValue(path, input);
      if (!value.ok) {
        return value;
      }
      const updated = setNestedValue(
        document,
        `operations.schedule.early_bird.${name}`,
        value.value,
      );
      if (!updated.ok) {
        return updated;
      }
      document = updated.value;
    }
  }
  for (const [path, value] of Object.entries(form.booleanEdits)) {
    if (!runtimeConfigFormEditsField(form, path)) {
      continue;
    }
    const name = earlyBirdField(path);
    if (name !== null) {
      const block = earlyBirdEnsureBlock(document);
      if (!block.ok) {
        return block;
      }
      const updated = setNestedValue(document, `operations.schedule.early_bird.${name}`, value);
      if (!updated.ok) {
        return updated;
      }
      document = updated.value;
    } else {
      const updated = setNestedValue(document, path, value);
      if (!updated.ok) {
        return updated;
      }
      document = updated.value;
    }
  }
  const mailboxes = mailboxesDocument(form.mailboxes);
  if (!structuralEquals(nestedValue(document, "dashboard_mailboxes"), mailboxes)) {
    const updated = setNestedValue(document, "dashboard_mailboxes", mailboxes);
    if (!updated.ok) {
      return updated;
    }
    document = updated.value;
  }
  if (form.hadEquipmentColors || form.equipmentColors.length > 0) {
    const sourceRulesValue = nestedValue(form.document, "equipment_colors.rules");
    const sourceRules = Array.isArray(sourceRulesValue) ? sourceRulesValue : null;
    const rules: AdminJsonValue[] = [];
    for (const rule of form.equipmentColors) {
      let source: JsonObject | null = null;
      if (rule.sourceIndex !== null) {
        const candidate = sourceRules?.[rule.sourceIndex];
        if (!isJsonObject(candidate)) {
          return err(null);
        }
        source = candidate;
      }
      const converted = equipmentColorDocument(rule, source);
      if (!converted.ok) {
        return converted;
      }
      rules.push(converted.value);
    }
    if (!structuralEquals(sourceRules, rules)) {
      const updated = setNestedValue(document, "equipment_colors", { rules });
      if (!updated.ok) {
        return updated;
      }
      document = updated.value;
    }
  }
  return ok(document);
}

/** 仪表盘邮箱当前的完整投影。 */
function mailboxesDocument(mailboxes: readonly RuntimeMailboxForm[]): AdminJsonValue[] {
  return mailboxes.map((mailbox) => ({
    world_id: mailbox.worldId,
    mailbox_key: mailbox.mailboxKey,
  }));
}

/** 编辑后的邮箱超过上限且与原文不同；只删除多余行时仍允许保存其余行。 */
export function runtimeConfigFormEditedMailboxesExceedLimit(form: RuntimeConfigForm): boolean {
  return (
    form.mailboxes.length > MAX_DASHBOARD_MAILBOXES &&
    !structuralEquals(
      nestedValue(form.document, "dashboard_mailboxes"),
      mailboxesDocument(form.mailboxes),
    )
  );
}

/** 原始颜色规则中某个等级边界的安全整数投影。 */
export function runtimeConfigFormSourceColorLevel(
  form: RuntimeConfigForm,
  index: number,
  bound: string,
): number | null {
  const rule = form.equipmentColors[index];
  if (rule === undefined || rule.sourceIndex === null) {
    return null;
  }
  const sourceRules = nestedValue(form.document, "equipment_colors.rules");
  if (!Array.isArray(sourceRules)) {
    return null;
  }
  const sourceRule = sourceRules[rule.sourceIndex];
  if (sourceRule === undefined) {
    return null;
  }
  const value = nestedValue(sourceRule, `level.${bound}`);
  return typeof value === "number" && Number.isSafeInteger(value) && !Object.is(value, -0)
    ? value
    : null;
}

function equipmentColorDocument(
  rule: RuntimeEquipmentColorForm,
  source: JsonObject | null,
): Result<AdminJsonValue, null> {
  let object: JsonObject = source === null ? {} : { ...source };
  const selections: readonly (readonly [string, string, ColorSelection])[] = [
    ["categories", "category", rule.category],
    ["equipment_types", "equipment_type", rule.equipmentType],
    ["qualities", "quality", rule.quality],
  ];
  for (const [plural, singular, selection] of selections) {
    const written = colorSelectionWrite(selection, object, source, plural, singular);
    if (!written.ok) {
      return written;
    }
    object = written.value;
  }
  const levelMin = parseRustI64(rule.levelMin);
  const levelMax = parseRustI64(rule.levelMax);
  if (levelMin === null || levelMax === null) {
    return err(null);
  }
  object = { ...object, level: { min: levelMin, max: levelMax } };
  for (const [name, value] of [
    ["prefixes", rule.prefixes],
    ["suffixes", rule.suffixes],
  ] as const) {
    let original: string | null = null;
    if (source !== null) {
      const text = listText(Object.hasOwn(source, name) ? (source[name] ?? null) : null);
      if (!text.ok) {
        return text;
      }
      original = text.value;
    }
    if (original !== value) {
      object = { ...object, [name]: lines(value) };
    }
  }
  for (const [name, value] of [
    ["prefix_color", rule.prefixColor],
    ["suffix_color", rule.suffixColor],
    ["combined_color", rule.combinedColor],
  ] as const) {
    object = { ...object, [name]: value };
  }
  return ok(object);
}

/** Rust `u64::from_str` 的十进制语法；超过 JS 安全整数时按既有精度边界拒绝。 */
function parseRustU64(value: string): number | null {
  if (!/^\+?[0-9]+$/u.test(value)) {
    return null;
  }
  const number = Number(value);
  return Number.isSafeInteger(number) && number <= MAX_JSON_SAFE_INTEGER && number >= 0
    ? number
    : null;
}

/** Rust `i64::from_str` 的十进制语法；超过 JS 安全整数时按既有精度边界拒绝。 */
function parseRustI64(value: string): number | null {
  if (!/^[+-]?[0-9]+$/u.test(value)) {
    return null;
  }
  const number = Number(value);
  return Number.isSafeInteger(number) && Math.abs(number) <= MAX_JSON_SAFE_INTEGER
    ? Object.is(number, -0)
      ? 0
      : number
    : null;
}

/** 文档来自解码器，只包含 JSON 值；克隆失败时按非法中间态拒绝，不进入异常路径。 */
function cloneDocument(document: AdminJsonValue): AdminJsonValue | null {
  try {
    const cloned: unknown = structuredClone(document);
    return isJsonValue(cloned) ? cloned : null;
  } catch {
    return null;
  }
}

function isJsonValue(value: unknown): value is AdminJsonValue {
  if (value === null || typeof value === "boolean" || typeof value === "string") {
    return true;
  }
  if (typeof value === "number") {
    return Number.isFinite(value);
  }
  if (Array.isArray(value)) {
    return value.every(isJsonValue);
  }
  return isJsonObject(value) && Object.values(value).every(isJsonValue);
}

function isJsonObject(value: unknown): value is JsonObject {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  const prototype = Object.getPrototypeOf(value);
  return prototype === Object.prototype || prototype === null;
}
