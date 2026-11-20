// Admin 会话 DTO。

import * as v from "valibot";
import { optionalNullable, strictStruct } from "../../wire";

/** Admin 会话响应；`csrf_token` 是秘密值，不得进入日志。 */
export const AdminSessionResponseSchema = strictStruct({
  authenticated: v.boolean(),
  actor: v.string(),
  csrf_token: v.string(),
  expires_at: v.string(),
  absolute_expires_at: v.string(),
  /** 读取与登录仅在 `include=server_time` 时输出。活动续期始终输出。 */
  server_time: optionalNullable(v.string()),
});

export type AdminSessionResponse = v.InferOutput<typeof AdminSessionResponseSchema>;
