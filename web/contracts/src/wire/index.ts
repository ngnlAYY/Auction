// DTO 字段的线上表示原语：把后端序列化规则逐条对应为运行期 schema。
// 字段该用哪一个原语由后端 DTO 的属性决定，见 web/README.md 的“契约”一节；不得按字段名猜测。

import * as v from "valibot";

/** JSON number 能精确表达的最大整数；金额、版本与序号都不得超出。 */
export const JSON_MAX_SAFE_INTEGER = 9_007_199_254_740_991;

/** 有符号整数（i32/i64 等）：有限、无小数且在安全整数范围内。 */
export const integer = v.pipe(v.number(), v.safeInteger());

/** 无符号整数（u8/u32/u64 等）：安全整数且不小于 0。 */
export const unsignedInteger = v.pipe(v.number(), v.safeInteger(), v.minValue(0));

/** 浮点数（f64）：必须有限。 */
export const float = v.pipe(v.number(), v.finite());

/** 普通 Option<T>：键缺失与 null 都读作 null。 */
export function optionalNullable<TSchema extends v.GenericSchema>(schema: TSchema) {
  return v.optional(v.nullable(schema), null);
}

/** 必填 nullable（deserialize_required_option）：键必须出现，值可以是 null。 */
export function requiredNullable<TSchema extends v.GenericSchema>(schema: TSchema) {
  return v.nullable(schema);
}

/** 键缺失时的内部占位值；JSON 里不可能出现 symbol，因此不会与任何线上取值混淆。 */
const MISSING: unique symbol = Symbol("missing");

/** 可缺省但不可为 null（default + deserialize_optional_field）：缺失读作 null，出现时必须是 T。 */
export function optionalField<TSchema extends v.GenericSchema>(schema: TSchema) {
  return v.optional(
    v.pipe(
      v.union([v.literal(MISSING), schema]),
      v.transform((value): v.InferOutput<TSchema> | null =>
        value === MISSING ? null : (value as v.InferOutput<TSchema>),
      ),
    ),
    MISSING,
  );
}

/** 带 default 的字段：键缺失时取给定默认值，出现时必须是 T（null 被拒绝）。 */
export function withDefault<TSchema extends v.GenericSchema>(
  schema: TSchema,
  fallback: v.InferInput<TSchema>,
) {
  return v.optional(schema, fallback as never);
}

/** null 读作零值（deserialize_null_default）；键是否可缺失由调用方另行组合 withDefault。 */
export function nullAsDefault<TSchema extends v.GenericSchema>(
  schema: TSchema,
  fallback: v.InferOutput<TSchema>,
) {
  return v.pipe(
    v.nullable(schema),
    v.transform((value): v.InferOutput<TSchema> => (value === null ? fallback : value)),
  );
}

/** 任意 JSON 值（serde_json::Value）；结构由消费它的模块另行校验。 */
export const jsonValue = v.unknown();

/** 用 schema 解码一份未知数据；失败返回 null，由调用方映射为 invalid_response。 */
export function decode<TSchema extends v.GenericSchema>(
  schema: TSchema,
  data: unknown,
): v.InferOutput<TSchema> | null {
  const parsed = v.safeParse(schema, data);
  return parsed.success ? parsed.output : null;
}
