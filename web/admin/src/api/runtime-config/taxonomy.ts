// 服务端装备词典投影的闭集结构校验；浏览器不维护装备或词缀名称白名单。

import type {
  EquipmentBoundedKindOptionDto,
  EquipmentKindOptionDto,
  EquipmentTaxonomyDto,
} from "@auction/contracts/dto/admin/equipment";

import { validText } from "./fields";

/** 词典名称用作复选框键，分类、材质和部位引用决定提示候选；各列表也限制渲染开销。 */
export function validEquipmentTaxonomy(value: EquipmentTaxonomyDto): boolean {
  return (
    validText(value.revision, 128, false) &&
    validNames(value.categories, 64, 64, false) &&
    validOptionNames(value.qualities, 64, 64, false) &&
    validOptionNames(value.kinds, 512, 64, false) &&
    validOptionNames(value.materials, 64, 64, false) &&
    validOptionNames(value.resolved_kinds, 16, 64, true) &&
    value.materials.every(
      (option) =>
        value.categories.includes(option.category) &&
        isArmorCategory(option.category) &&
        validAffixes(option.prefixes) &&
        validAffixes(option.suffixes),
    ) &&
    value.kinds.every((option) => validKind(value, option)) &&
    value.resolved_kinds.every((option) => validResolvedKind(value, option))
  );
}

function validKind(taxonomy: EquipmentTaxonomyDto, option: EquipmentKindOptionDto): boolean {
  const material = option.material;
  const slot = option.slot;
  const armor = isArmorCategory(option.category);
  return (
    taxonomy.categories.includes(option.category) &&
    taxonomy.materials.every((candidate) => !asciiEqIgnoreCase(candidate.name, option.name)) &&
    // 只有三类护甲有独立种类与部位，武器、法杖及盾牌均不携带这两个字段。
    (material !== null) === armor &&
    (slot !== null) === armor &&
    (material === null || validText(material, 64, false)) &&
    (slot === null || isArmorSlot(slot)) &&
    (material === null ||
      // 旧完整种类的材质不一定提供族选项；若提供，所属大类必须一致。
      taxonomy.materials.every(
        (candidate) =>
          !asciiEqIgnoreCase(candidate.name, material) || candidate.category === option.category,
      )) &&
    validAffixes(option.prefixes) &&
    validAffixes(option.suffixes)
  );
}

function validResolvedKind(
  taxonomy: EquipmentTaxonomyDto,
  option: EquipmentBoundedKindOptionDto,
): boolean {
  return (
    taxonomy.categories.includes(option.category) &&
    isArmorCategory(option.category) &&
    taxonomy.kinds.every((kind) => !asciiEqIgnoreCase(kind.name, option.name)) &&
    taxonomy.materials.every((material) => !asciiEqIgnoreCase(material.name, option.name)) &&
    taxonomy.materials.some(
      (material) =>
        asciiEqIgnoreCase(material.name, option.material) && material.category === option.category,
    ) &&
    isArmorSlot(option.slot) &&
    validAffixes(option.prefixes) &&
    validAffixes(option.suffixes)
  );
}

function isArmorCategory(category: string): boolean {
  return category === "Cloth" || category === "Light" || category === "Heavy";
}

function isArmorSlot(slot: string): boolean {
  return (
    slot === "head" || slot === "body" || slot === "hands" || slot === "legs" || slot === "feet"
  );
}

function validAffixes(values: readonly string[]): boolean {
  return validNames(values, 256, 128, true);
}

/** 先按列表长度拒绝超限输入，再惰性读取名称；避免为注定失败的词典分配整份名称数组。 */
function validOptionNames(
  options: readonly { readonly name: string }[],
  maximum: number,
  maximumBytes: number,
  allowEmpty: boolean,
): boolean {
  if (options.length > maximum) {
    return false;
  }
  return validNames(optionNames(options), maximum, maximumBytes, allowEmpty);
}

function* optionNames(options: readonly { readonly name: string }[]): Iterable<string> {
  for (const option of options) {
    yield option.name;
  }
}

/** 按 Rust 迭代顺序校验名称列表：逐项长度、上限、ASCII 不区分大小写去重。 */
export function validNames(
  values: Iterable<string>,
  maximum: number,
  maximumBytes: number,
  allowEmpty: boolean,
): boolean {
  const seen: string[] = [];
  for (const value of values) {
    if (seen.length === maximum || !validText(value, maximumBytes, false)) {
      return false;
    }
    seen.push(value);
  }
  // 排序后只需比较相邻项即可判定大小写不敏感的重复，避免逐项两两比较的平方开销。
  seen.sort(compareAsciiFolded);
  for (let index = 1; index < seen.length; index += 1) {
    const previous = seen[index - 1];
    const current = seen[index];
    if (previous !== undefined && current !== undefined && asciiEqIgnoreCase(previous, current)) {
      return false;
    }
  }
  return allowEmpty || seen.length > 0;
}

/** Rust `eq_ignore_ascii_case`：只折叠 ASCII 字母，非 ASCII 字符按原样比较。 */
function asciiEqIgnoreCase(left: string, right: string): boolean {
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

/** 与 Rust 按 ASCII 折叠后的 UTF-8 字节序一致：UTF-8 字节序就是码点序。 */
function compareAsciiFolded(left: string, right: string): number {
  const leftPoints = Array.from(left, (character) => asciiLower(character.codePointAt(0) ?? 0));
  const rightPoints = Array.from(right, (character) => asciiLower(character.codePointAt(0) ?? 0));
  const length = Math.min(leftPoints.length, rightPoints.length);
  for (let index = 0; index < length; index += 1) {
    const leftPoint = leftPoints[index] ?? 0;
    const rightPoint = rightPoints[index] ?? 0;
    if (leftPoint !== rightPoint) {
      return leftPoint - rightPoint;
    }
  }
  return leftPoints.length - rightPoints.length;
}
