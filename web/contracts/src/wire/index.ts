// DTO 字段的线上表示原语：把后端序列化规则逐条对应为运行期 schema。
// 字段该用哪一个原语由后端 DTO 的属性决定，见 web/README.md 的“契约”一节；不得按字段名猜测。

import * as v from "valibot";

/** JSON number 能精确表达的最大整数；金额、版本与序号都不得超出。 */
export const JSON_MAX_SAFE_INTEGER = 9_007_199_254_740_991;

/** i64、isize：有限、无小数且在安全整数范围内。 */
export const integer = v.pipe(v.number(), v.safeInteger());

/** 无符号整数（u64、usize）：安全整数且不小于 0。 */
export const unsignedInteger = v.pipe(v.number(), v.safeInteger(), v.minValue(0));

/** i32：32 位有符号整数的取值范围。 */
export const int32 = v.pipe(
  v.number(),
  v.safeInteger(),
  v.minValue(-2_147_483_648),
  v.maxValue(2_147_483_647),
);

/** u32：32 位无符号整数的取值范围。 */
export const uint32 = v.pipe(v.number(), v.safeInteger(), v.minValue(0), v.maxValue(4_294_967_295));

/** i16：16 位有符号整数的取值范围。 */
export const int16 = v.pipe(v.number(), v.safeInteger(), v.minValue(-32_768), v.maxValue(32_767));

/** u16：16 位无符号整数的取值范围。 */
export const uint16 = v.pipe(v.number(), v.safeInteger(), v.minValue(0), v.maxValue(65_535));

/** u8：8 位无符号整数的取值范围。 */
export const uint8 = v.pipe(v.number(), v.safeInteger(), v.minValue(0), v.maxValue(255));

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

/** 输入是普通 JSON 对象：不是 null，也不是数组。 */
function isJsonObject(input: unknown): input is Record<string, unknown> {
  return typeof input === "object" && input !== null && !Array.isArray(input);
}

/**
 * 结构体（未标 deny_unknown_fields）：只接受 JSON 对象，未知键被忽略。
 * 不直接用 v.object：它会把数组当成对象，再用各字段的缺省值拼出一个“合法”结果。
 * 后端从不把结构体编码成数组，这里一律拒绝数组。
 */
export function struct<const TEntries extends v.ObjectEntries>(entries: TEntries) {
  return v.pipe(v.custom<Record<string, unknown>>(isJsonObject), v.object(entries));
}

/** 结构体（deny_unknown_fields）：只接受 JSON 对象，出现未知键即失败。 */
export function strictStruct<const TEntries extends v.ObjectEntries>(entries: TEntries) {
  return v.pipe(v.custom<Record<string, unknown>>(isJsonObject), v.strictObject(entries));
}

/**
 * 字符串键的映射（BTreeMap<String, T>、HashMap<String, T>）。
 * 只接受 JSON 对象；每个条目的值都按 schema 校验，任何键都不丢弃。结果是无原型对象，
 * `__proto__`、`constructor` 这类键名也只是普通条目；遍历用 Object.keys 或 Object.entries。
 */
export function stringMap<TSchema extends v.GenericSchema>(schema: TSchema) {
  return v.pipe(
    v.custom<Record<string, unknown>>(isJsonObject),
    v.check((input) => Object.keys(input).every((key) => v.is(schema, input[key]))),
    v.transform((input): Readonly<Record<string, v.InferOutput<TSchema>>> => {
      const output: Record<string, v.InferOutput<TSchema>> = Object.create(null);
      for (const key of Object.keys(input)) {
        output[key] = v.parse(schema, input[key]);
      }
      return output;
    }),
  );
}

/** 任意 JSON 值（serde_json::Value）；结构由消费它的模块另行校验。 */
export const jsonValue = v.unknown();

/** 解码结果：成功时带值（值本身可以是 null），失败时只有标记。 */
export type Decoded<T> = { readonly ok: true; readonly value: T } | { readonly ok: false };

/**
 * 用 schema 解码一份未知数据；失败由调用方映射为 invalid_response。
 * 结果用 ok 区分成败，不用 null 表示失败：`data: null` 对可空 schema 是合法的成功值。
 */
export function decode<TSchema extends v.GenericSchema>(
  schema: TSchema,
  data: unknown,
): Decoded<v.InferOutput<TSchema>> {
  const parsed = v.safeParse(schema, data);
  return parsed.success ? { ok: true, value: parsed.output } : { ok: false };
}
