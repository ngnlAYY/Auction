// 只从服务端统一装备词典投影计算展示候选；历史已选值由复选框组保留。

import type { EquipmentTaxonomyDto } from "@auction/contracts/dto/admin/equipment";

import { trimRustWhitespace } from "../../../../api/session-time";

/** 列出当前分类可选的装备种类与材质族。 */
export function equipmentKindsFor(
  taxonomy: EquipmentTaxonomyDto,
  category: string,
): readonly string[] {
  const categories = selections(category);
  const result: string[] = [];
  for (const option of taxonomy.kinds) {
    // 护甲种类由单独的族投影提供；具体部位完整名只保留为已有规则的精确选中值。
    if (
      option.material === null &&
      option.slot === null &&
      selectedCategory(categories, option.category)
    ) {
      result.push(option.name);
    }
  }
  for (const option of taxonomy.materials) {
    if (selectedCategory(categories, option.category)) {
      result.push(option.name);
    }
  }
  return result;
}

/** 列出多选分类与种类命中规则的前缀候选。 */
export function prefixesFor(
  taxonomy: EquipmentTaxonomyDto,
  category: string,
  kind: string,
): readonly string[] {
  return affixesFor(taxonomy, category, kind, true);
}

/** 列出多选分类与种类命中规则的后缀候选。 */
export function suffixesFor(
  taxonomy: EquipmentTaxonomyDto,
  category: string,
  kind: string,
): readonly string[] {
  return affixesFor(taxonomy, category, kind, false);
}

/** 多选维度取并集；只按投影中的精确名称关联，未知名称不会猜测成某个材质族。 */
function affixesFor(
  taxonomy: EquipmentTaxonomyDto,
  category: string,
  kind: string,
  prefixes: boolean,
): readonly string[] {
  const categories = selections(category);
  const kinds = selections(kind);
  const result: string[] = [];
  const candidates = [...taxonomy.kinds, ...taxonomy.resolved_kinds, ...taxonomy.materials];
  for (const option of candidates) {
    if (!selectedCategory(categories, option.category)) {
      continue;
    }
    // 具体部位完整名只保留为已有规则的精确选中值。
    if (kinds.length > 0 && !kinds.some((selected) => eqIgnoreAsciiCase(selected, option.name))) {
      continue;
    }
    for (const affix of prefixes ? option.prefixes : option.suffixes) {
      if (!result.some((previous) => eqIgnoreAsciiCase(previous, affix))) {
        result.push(affix);
      }
    }
  }
  return result;
}

/** 按 Rust `str::lines` 的换行与行尾 `\r` 语义拆行。 */
function rustLines(value: string): string[] {
  const result: string[] = [];
  let start = 0;
  while (start < value.length) {
    const end = value.indexOf("\n", start);
    if (end < 0) {
      result.push(value.slice(start));
      break;
    }
    const line = value.slice(start, end);
    result.push(line.endsWith("\r") ? line.slice(0, -1) : line);
    start = end + 1;
  }
  return result;
}

/** 空白的多选维度表示任意；否则按显示词忽略 ASCII 大小写匹配。 */
function selectedCategory(categories: readonly string[], category: string): boolean {
  return (
    categories.length === 0 || categories.some((selected) => eqIgnoreAsciiCase(selected, category))
  );
}

/** 多选值按行拆分（`\n` 或 `\r\n`），去掉首尾空白并丢弃空行；逗号不是分隔符。 */
function selections(value: string): string[] {
  return rustLines(value)
    .map(trimRustWhitespace)
    .filter((line) => line !== "");
}

/** `eq_ignore_ascii_case` 只折叠 ASCII 大小写；非 ASCII 字符必须完全相同。 */
function eqIgnoreAsciiCase(left: string, right: string): boolean {
  if (left.length !== right.length) {
    return false;
  }
  for (let i = 0; i < left.length; i += 1) {
    if (asciiLower(left.charCodeAt(i)) !== asciiLower(right.charCodeAt(i))) {
      return false;
    }
  }
  return true;
}

function asciiLower(code: number): number {
  return code >= 0x41 && code <= 0x5a ? code + 0x20 : code;
}
