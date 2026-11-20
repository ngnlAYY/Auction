// Admin 列表共用的数字分页信封。
//
// 各列表的分页字段形状完全相同，只在浏览器解码严格度上分为两种：带 `deny_unknown_fields`
// 的严格信封与不带该约束的宽松信封。各列表沿用原有名字，保持既有解码严格度。

import type * as v from "valibot";
import { integer, strictStruct, struct } from "../../wire";

/** 宽松解码的数字分页：浏览器忽略未知字段。只供原本宽松的列表沿用；新列表默认用
 * `AdminStrictPaginationResponse`。 */
export const AdminPaginationResponseSchema = struct({
  page: integer,
  page_size: integer,
  total: integer,
});

export type AdminPaginationResponse = v.InferOutput<typeof AdminPaginationResponseSchema>;

/** 严格解码的数字分页：浏览器拒绝未知字段。 */
export const AdminStrictPaginationResponseSchema = strictStruct({
  page: integer,
  page_size: integer,
  total: integer,
});

export type AdminStrictPaginationResponse = v.InferOutput<
  typeof AdminStrictPaginationResponseSchema
>;
