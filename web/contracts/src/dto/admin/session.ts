// Admin 会话 DTO。

import * as v from "valibot";
import { optionalNullable, strictStruct } from "../../wire";
import { timestamp } from "../../wire/timestamp";

/** Admin 会话响应；`csrf_token` 是秘密值，不得进入日志。 */
export const AdminSessionResponseSchema = strictStruct({
  authenticated: v.boolean(),
  actor: v.string(),
  csrf_token: v.string(),
  expires_at: timestamp,
  absolute_expires_at: timestamp,
  /** 读取与登录仅在 `include=server_time` 时输出。活动续期始终输出。 */
  server_time: optionalNullable(timestamp),
});

export type AdminSessionResponse = v.InferOutput<typeof AdminSessionResponseSchema>;
