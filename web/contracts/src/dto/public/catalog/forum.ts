// Public 论坛镜像 DTO。

import * as v from "valibot";
import { int32, integer, optionalNullable, strictStruct } from "../../../wire";

/** Public 论坛同步启用状态；未知状态通过错误响应表达，不伪造默认布尔值。 */
export const PublicForumSyncStatusResponseSchema = strictStruct({
  enabled: v.boolean(),
});

export type PublicForumSyncStatusResponse = v.InferOutput<
  typeof PublicForumSyncStatusResponseSchema
>;

/** 论坛镜像正文可能包含完整装备 URL，调试输出与发送前正文保持同一脱敏边界。 */
export const PublicForumResponseSchema = strictStruct({
  auction_id: v.string(),
  status: v.string(),
  source_revision: integer,
  thread_url: optionalNullable(v.string()),
  title: optionalNullable(v.string()),
  body: optionalNullable(v.string()),
  content_revision: integer,
  processed_through_post_number: int32,
  content_synced_at: optionalNullable(v.string()),
  content_last_success_at: optionalNullable(v.string()),
  content_last_error_at: optionalNullable(v.string()),
  content_last_error_code: optionalNullable(v.string()),
  updated_at: v.string(),
});

export type PublicForumResponse = v.InferOutput<typeof PublicForumResponseSchema>;
