// Admin 代理历史 DTO。

import * as v from "valibot";
import { integer, optionalNullable, strictStruct } from "../../wire";

/** Admin 代理历史的安全事件投影。
 * `deny_unknown_fields` 与 Admin 浏览器端的失败关闭解码对齐；线上契约冻结后只收紧
 * 反序列化，序列化输出不变。 */
export const AdminProxyEventResponseSchema = strictStruct({
  authorization_id: v.string(),
  authorization_sequence: integer,
  command_id: v.string(),
  created_at: v.string(),
  lot_id: v.string(),
  maximum_amount: integer,
  mode: v.string(),
  state_version: integer,
  status: v.string(),
  user_id: v.string(),
  usage_number: integer,
  username: v.string(),
});

export type AdminProxyEventResponse = v.InferOutput<typeof AdminProxyEventResponseSchema>;

/** Admin 代理历史的页码或游标分页信息。 */
export const AdminProxyEventPaginationResponseSchema = strictStruct({
  page: optionalNullable(integer),
  page_size: integer,
  total: optionalNullable(integer),
  cursor: optionalNullable(v.string()),
  next_cursor: optionalNullable(v.string()),
  has_more: optionalNullable(v.boolean()),
});

export type AdminProxyEventPaginationResponse = v.InferOutput<
  typeof AdminProxyEventPaginationResponseSchema
>;

/** Admin 代理历史响应。 */
export const AdminProxyEventListResponseSchema = strictStruct({
  proxy_events: v.array(AdminProxyEventResponseSchema),
  pagination: AdminProxyEventPaginationResponseSchema,
});

export type AdminProxyEventListResponse = v.InferOutput<typeof AdminProxyEventListResponseSchema>;
