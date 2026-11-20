// 录入用例共用的装备原文，允许直接写入重复键和数字词法。

/** 生成 Rust 表单接受的完整装备 JSON，调用方可替换指定字段的原文。 */
export function equipmentJson(
  stats = '"Defense":{"value":"2"}',
  prefix = '"prefix":""',
  level = "null",
): string {
  return `{"external_equipment_id":"123","name":"Legendary Phase Robe","category":"Cloth","equipment_type":"Phase Robe","quality":"Legendary",${prefix},"suffix":"","level":${level},"tradeable":true,"stats":{${stats}},"key":"0123456789"}`;
}

/** 生成带指定 `base` 原文的完整装备 JSON。 */
export function equipmentJsonWithBase(base: string): string {
  return equipmentJson(`"Defense":{"value":"2","base":${base}}`);
}

/** serde 会拒绝这些被覆盖的属性，按原文逐项解码时能看到首项。 */
export const OVERWRITTEN_INVALID_STATS = [
  ["字段类型错误", '"Defense":{"value":true},"Defense":{"value":"2"}'],
  ["属性值为空", '"Defense":null,"Defense":{"value":"2"}'],
  ["属性带未知字段", '"Defense":{"value":"1","extra":0},"Defense":{"value":"2"}'],
] as const;
