// Admin 业务世界目录与启停 DTO。

import * as v from "valibot";
import { integer, strictStruct } from "../../wire";

/** Admin 可见的业务世界权威状态。
 * `deny_unknown_fields` 与 Admin 浏览器端的 fail-closed 解码对齐；只收紧反序列化。 */
export const AdminWorldResponseSchema = strictStruct({
  id: v.string(),
  enabled: v.boolean(),
  availability: v.string(),
  policy_revision: integer,
});

export type AdminWorldResponse = v.InferOutput<typeof AdminWorldResponseSchema>;

export const AdminWorldListResponseSchema = strictStruct({
  worlds: v.pipe(v.array(AdminWorldResponseSchema), v.readonly()),
});

export type AdminWorldListResponse = v.InferOutput<typeof AdminWorldListResponseSchema>;

export interface AdminWorldSetEnabledRequest {
  /** 后端 `uuid::Uuid`，线上按 UUID 字符串发送。 */
  command_id: string;
  enabled: boolean;
  expected_policy_revision: number;
}

export const AdminWorldSetEnabledResponseSchema = strictStruct({
  world: AdminWorldResponseSchema,
});

/** 解析 `AdminWorldSetEnabledResponseSchema` 的世界启停命令回执必须传入的字段策略：任意
 * 一层出现重复键，或数字使用小数、指数与 `-0` 词法时整份响应失败，与 Rust 对
 * `policy_revision: i64` 的直接反序列化一致。 */
export const ADMIN_WORLD_SET_ENABLED_RESPONSE_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;

export type AdminWorldSetEnabledResponse = v.InferOutput<typeof AdminWorldSetEnabledResponseSchema>;
