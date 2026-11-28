// 论坛装备标色规则的编辑状态与不可变更新。

import type { RuntimeConfigForm } from "./index";
import {
  type ColorSelection,
  colorSelectionResolve,
  colorSelectionUsesLegacySyntax,
} from "./nested";

/** 论坛装备标色规则的前端编辑状态；多选列表按行保存，兼容旧单值规则回填。 */
export interface RuntimeEquipmentColorForm {
  readonly sourceIndex: number | null;
  readonly rowId: number;
  readonly category: ColorSelection;
  readonly equipmentType: ColorSelection;
  /** 旧材质单值保持精确匹配，最终选项变更时才改用多选。 */
  readonly legacyEquipmentType: string | null;
  readonly quality: ColorSelection;
  readonly levelMin: string;
  readonly levelMax: string;
  readonly prefixes: string;
  readonly suffixes: string;
  readonly prefixColor: string;
  readonly suffixColor: string;
  readonly combinedColor: string;
}

/** 展示与单项编辑只借用集合，避免每次勾选复制全部规则及其文本。 */
export function runtimeConfigFormEquipmentColors(
  form: RuntimeConfigForm,
): readonly RuntimeEquipmentColorForm[] {
  return form.equipmentColors;
}

export function runtimeConfigFormAddEquipmentColor(form: RuntimeConfigForm): RuntimeConfigForm {
  const rule: RuntimeEquipmentColorForm = {
    sourceIndex: null,
    rowId: form.nextColorRowId,
    category: { text: "", conflictingSingle: null, resolved: null },
    equipmentType: { text: "", conflictingSingle: null, resolved: null },
    legacyEquipmentType: null,
    quality: { text: "", conflictingSingle: null, resolved: null },
    levelMin: "0",
    levelMax: "0",
    prefixes: "",
    suffixes: "",
    prefixColor: "#ff0000",
    suffixColor: "#00aa00",
    combinedColor: "#aa00aa",
  };
  return {
    ...form,
    equipmentColors: [...form.equipmentColors, rule],
    nextColorRowId: form.nextColorRowId + 1,
  };
}

export function runtimeConfigFormRemoveEquipmentColor(
  form: RuntimeConfigForm,
  index: number,
): RuntimeConfigForm {
  if (index < 0 || index >= form.equipmentColors.length) {
    return form;
  }
  return {
    ...form,
    equipmentColors: form.equipmentColors.filter((_, position) => position !== index),
  };
}

export function runtimeConfigFormMoveEquipmentColor(
  form: RuntimeConfigForm,
  index: number,
  delta: number,
): RuntimeConfigForm {
  const target = index + delta;
  if (!Number.isSafeInteger(target) || target < 0) {
    return form;
  }
  if (index < 0 || index >= form.equipmentColors.length || target >= form.equipmentColors.length) {
    return form;
  }
  const equipmentColors = [...form.equipmentColors];
  const [moved] = equipmentColors.splice(index, 1);
  if (moved === undefined) {
    return form;
  }
  equipmentColors.splice(target, 0, moved);
  return { ...form, equipmentColors };
}

export function runtimeConfigFormSetEquipmentColorField(
  form: RuntimeConfigForm,
  index: number,
  field: string,
  value: string,
): RuntimeConfigForm {
  const rule = form.equipmentColors[index];
  if (rule === undefined) {
    return form;
  }
  let nextValue = value;
  if (
    field === "equipment_type" &&
    colorSelectionUsesLegacySyntax(rule.equipmentType) &&
    rule.legacyEquipmentType !== null &&
    eqIgnoreAsciiCase(rule.legacyEquipmentType, nextValue)
  ) {
    // 选回同一材质时恢复旧单值的拼写与精确匹配，避免临时换选扩大规则。
    nextValue = rule.legacyEquipmentType;
  }
  let updated: RuntimeEquipmentColorForm | null = null;
  switch (field) {
    case "category":
      if (rule.category.conflictingSingle === null) {
        updated = { ...rule, category: { ...rule.category, text: nextValue } };
      }
      break;
    case "equipment_type":
      if (rule.equipmentType.conflictingSingle === null) {
        updated = { ...rule, equipmentType: { ...rule.equipmentType, text: nextValue } };
      }
      break;
    case "quality":
      if (rule.quality.conflictingSingle === null) {
        updated = { ...rule, quality: { ...rule.quality, text: nextValue } };
      }
      break;
    case "level_min":
      updated = { ...rule, levelMin: nextValue };
      break;
    case "level_max":
      updated = { ...rule, levelMax: nextValue };
      break;
    case "prefixes":
      updated = { ...rule, prefixes: nextValue };
      break;
    case "suffixes":
      updated = { ...rule, suffixes: nextValue };
      break;
    case "prefix_color":
      updated = { ...rule, prefixColor: nextValue };
      break;
    case "suffix_color":
      updated = { ...rule, suffixColor: nextValue };
      break;
    case "combined_color":
      updated = { ...rule, combinedColor: nextValue };
      break;
    default:
      return form;
  }
  if (updated === null) {
    return form;
  }
  return {
    ...form,
    equipmentColors: form.equipmentColors.map((current, position) =>
      position === index ? updated : current,
    ),
  };
}

export function runtimeConfigFormResolveEquipmentColorSelection(
  form: RuntimeConfigForm,
  index: number,
  field: string,
  keepSingle: boolean,
): RuntimeConfigForm {
  const rule = form.equipmentColors[index];
  if (rule === undefined) {
    return form;
  }
  let updated: RuntimeEquipmentColorForm;
  switch (field) {
    case "category":
      updated = { ...rule, category: colorSelectionResolve(rule.category, keepSingle) };
      break;
    case "equipment_type":
      updated = { ...rule, equipmentType: colorSelectionResolve(rule.equipmentType, keepSingle) };
      break;
    case "quality":
      updated = { ...rule, quality: colorSelectionResolve(rule.quality, keepSingle) };
      break;
    default:
      return form;
  }
  return {
    ...form,
    equipmentColors: form.equipmentColors.map((current, position) =>
      position === index ? updated : current,
    ),
  };
}

/** 旧材质单值只在当前仍按旧语法时才报告。 */
export function runtimeEquipmentColorFormLegacyEquipmentType(
  rule: RuntimeEquipmentColorForm,
): string | null {
  return rule.legacyEquipmentType !== null &&
    colorSelectionUsesLegacySyntax(rule.equipmentType) &&
    rule.legacyEquipmentType === rule.equipmentType.text
    ? rule.legacyEquipmentType
    : null;
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
