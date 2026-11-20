// Admin 装备词典的只读投影；不进入 runtime 文档、命令摘要或装备持久化快照。

import * as v from "valibot";
import { optionalNullable, strictStruct, withDefault } from "../../wire";

/** 管理端推荐品质候选；候选范围独立于名称解析兼容和自动收件品质策略。 */
export const EquipmentQualityOptionDtoSchema = strictStruct({
  name: v.string(),
  legacy: v.boolean(),
});

export type EquipmentQualityOptionDto = v.InferOutput<typeof EquipmentQualityOptionDtoSchema>;

/** 已明确的完整基础名索引；护甲组合用于兼容旧精确规则，新种类候选使用独立 materials。 */
export const EquipmentKindOptionDtoSchema = strictStruct({
  /** 历史基础名，例如 `Phase Robe`；其种类与部位分别在 material、slot 中表达。 */
  name: v.string(),
  category: v.string(),
  /** 护甲种类；仅 Cloth、Light、Heavy 有值，与 slot 同时存在。 */
  material: optionalNullable(v.string()),
  /** 护甲语义部位；武器、法杖和盾牌为 None。 */
  slot: optionalNullable(v.string()),
  legacy: v.boolean(),
  prefixes: v.pipe(v.array(v.string()), v.readonly()),
  suffixes: v.pipe(v.array(v.string()), v.readonly()),
});

export type EquipmentKindOptionDto = v.InferOutput<typeof EquipmentKindOptionDtoSchema>;

/** 当前配置中已识别的有限兼容种类，不将语法可能组合枚举为确证选项。 */
export const EquipmentBoundedKindOptionDtoSchema = strictStruct({
  /** 领域解析后的完整规范名称。 */
  name: v.string(),
  /** 领域确定的大类。 */
  category: v.string(),
  /** 领域确定的材质，引用单独的材质族。 */
  material: v.string(),
  /** 语义部位标识，与确证种类沿用同一闭集。 */
  slot: v.string(),
  /** 该具体种类的推荐前缀。 */
  prefixes: v.pipe(v.array(v.string()), v.readonly()),
  /** 该具体种类的推荐后缀。 */
  suffixes: v.pipe(v.array(v.string()), v.readonly()),
});

export type EquipmentBoundedKindOptionDto = v.InferOutput<
  typeof EquipmentBoundedKindOptionDtoSchema
>;

/** 多选规则支持的护甲种类；沿用 materials 字段名，同种类覆盖全部部位。 */
export const EquipmentMaterialOptionDtoSchema = strictStruct({
  name: v.string(),
  category: v.string(),
  prefixes: v.pipe(v.array(v.string()), v.readonly()),
  suffixes: v.pipe(v.array(v.string()), v.readonly()),
});

export type EquipmentMaterialOptionDto = v.InferOutput<typeof EquipmentMaterialOptionDtoSchema>;

/** 服务端统一词典的推荐候选；`revision` 标识词典内容版本。 */
export const EquipmentTaxonomyDtoSchema = strictStruct({
  revision: v.string(),
  categories: v.pipe(v.array(v.string()), v.readonly()),
  qualities: v.pipe(v.array(EquipmentQualityOptionDtoSchema), v.readonly()),
  kinds: v.pipe(v.array(EquipmentKindOptionDtoSchema), v.readonly()),
  materials: v.pipe(v.array(EquipmentMaterialOptionDtoSchema), v.readonly()),
  /** 仅来自本次配置、经领域有限语法识别的具体种类；与确证目录分开，旧投影缺省为空。 */
  resolved_kinds: withDefault(
    v.pipe(v.array(EquipmentBoundedKindOptionDtoSchema), v.readonly()),
    [],
  ),
});

export type EquipmentTaxonomyDto = v.InferOutput<typeof EquipmentTaxonomyDtoSchema>;
