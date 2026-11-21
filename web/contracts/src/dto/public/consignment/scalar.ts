// 寄售 DTO 的安全整数、外部装备编号、规范 UUID、UTC 时刻与文本边界。

import * as v from "valibot";

import { integer } from "../../../wire";
import { timestamp } from "../../../wire/timestamp";
import { LISTING_PRICE_MAX } from "./listing-options";

export { LISTING_PRICE_MAX, ListingFeeBpsSchema } from "./listing-options";

/** 不变量 6：只接受协议范围内的整数标价。 */
export const ListingPriceSchema = v.pipe(integer, v.minValue(1), v.maxValue(LISTING_PRICE_MAX));

/** 已提交的挂单版本从 1 开始，必须能由浏览器精确表示。 */
export const ListingVersionSchema = v.pipe(integer, v.minValue(1));

/** 统一使用协议规定的小写、带连字符的非零 UUID。 */
export const ConsignmentUuidSchema = v.pipe(
  v.string(),
  v.regex(/^[0-9a-f]{8}-(?:[0-9a-f]{4}-){3}[0-9a-f]{12}$/u),
  v.check((value) => value !== "00000000-0000-0000-0000-000000000000"),
);

/** 寄售时刻只接受定宽 RFC 3339 UTC，实际日历和时钟范围复用共用校验。 */
export const ConsignmentTimestampSchema = v.pipe(
  v.string(),
  v.regex(
    /^[0-9]{4}-[0-9]{2}-[0-9]{2}[Tt][0-9]{2}:[0-9]{2}:[0-9]{2}(?:\.[0-9]+)?(?:[Zz]|\+00:00)$/u,
  ),
  timestamp,
);

/** EID 不经过 Number 转换，避免丢失长编号精度。 */
export const ExternalEquipmentIdSchema = v.pipe(v.string(), v.regex(/^[1-9][0-9]{0,31}$/u));

/** 与 Rust `checked_text` 同样按 UTF-8 字节、Unicode 空白与控制字符判定。 */
export function consignmentTextSchema(maxBytes: number) {
  return v.pipe(
    v.string(),
    v.nonEmpty(),
    v.maxBytes(maxBytes),
    v.check(
      (value) => !/(?:^\p{White_Space}|\p{White_Space}$|[\p{Control}\p{Surrogate}])/u.test(value),
    ),
  );
}
