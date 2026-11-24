// v2 玩家与 Admin 写命令的 UUIDv4 线协议原语。

import * as v from "valibot";

/** 规范小写、带连字符且版本位为 4 的非零 UUID。 */
export const CommandUuidV4Schema = v.pipe(
  v.string(),
  v.regex(
    /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[0-9a-f]{4}-[0-9a-f]{12}$/u,
    "必须是规范小写的 UUIDv4",
  ),
);

/** 玩家或 Admin 提交的稳定命令 ID。 */
export type CommandUuidV4 = v.InferOutput<typeof CommandUuidV4Schema>;
