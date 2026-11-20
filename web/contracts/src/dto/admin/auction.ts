// auction 相关 DTO。

import * as v from "valibot";
import { int32, integer, requiredNullable, strictStruct, struct } from "../../wire";
import { uuid } from "../../wire/uuid";
import { AdminAuctionResponseSchema } from "./settlement";

/** Admin 响应中的 `uuid::Uuid` 解码：接受 `uuid` 原语的四种输入写法，输出统一为小写
 * 连字符格式，与 Rust `Uuid` 的 `Display` 一致。 */
const CANONICAL_UUID = v.pipe(
  uuid,
  v.transform((value) => {
    const hex = value
      .replace(/^urn:uuid:/i, "")
      .replace(/[{}]/g, "")
      .replace(/-/g, "")
      .toLowerCase();
    return [
      hex.slice(0, 8),
      hex.slice(8, 12),
      hex.slice(12, 16),
      hex.slice(16, 20),
      hex.slice(20),
    ].join("-");
  }),
);

/** 旧版生命周期响应不会附带列表专属的 `scheduled_ends_at` 或 `lot_count`，因此这里不复用
 * `AdminAuctionResponse`，避免把列表扩展字段泄漏进同一 v1 命令 wire。nullable 生命周期字段
 * 在旧 decoder 中必须显式出现（可为 `null`），未知字段一律拒绝。 */
export const AdminAuctionCommandResponseSchema = strictStruct({
  id: v.string(),
  world_id: v.string(),
  sequence: integer,
  status: v.string(),
  preview_scheduled_at: v.string(),
  scheduled_open_at: v.string(),
  preview_opened_at: requiredNullable(v.string()),
  preview_manual_open: v.boolean(),
  preview_revision: integer,
  opened_at: requiredNullable(v.string()),
  final_phase_at: requiredNullable(v.string()),
  settled_at: requiredNullable(v.string()),
  cancelled_at: requiredNullable(v.string()),
  cancelled_reason: requiredNullable(v.string()),
  cancelled_by: requiredNullable(v.string()),
  state_version: integer,
  created_at: v.string(),
  updated_at: v.string(),
});

/** 解析 `AdminAuctionCommandResponseSchema` 的命令回执必须传入的字段策略：任意一层出现重复
 * 键，或数字使用小数、指数与 `-0` 词法时整份响应失败，与 Rust 对 `i64` 字段的直接反序列化
 * 一致。用于判定命令成功或拒绝。 */
export const ADMIN_AUCTION_COMMAND_RESPONSE_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;

/** Admin 拍卖生命周期投影；nullable 生命周期字段必须显式出现（可为 `null`）。 */
export type AdminAuctionCommandResponse = v.InferOutput<typeof AdminAuctionCommandResponseSchema>;

/** `deny_unknown_fields` 与 Admin 浏览器端的 fail-closed 解码对齐；wire 冻结后只收紧
 * 反序列化，序列化输出不变。 */
export const AdminAuctionLifecycleResponseSchema = strictStruct({
  auction: AdminAuctionCommandResponseSchema,
});

/** 解析 `AdminAuctionLifecycleResponseSchema` 的生命周期命令回执必须传入的字段策略。 */
export const ADMIN_AUCTION_LIFECYCLE_RESPONSE_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;

/** Admin 生命周期命令响应信封。 */
export type AdminAuctionLifecycleResponse = v.InferOutput<
  typeof AdminAuctionLifecycleResponseSchema
>;

/** 建场命令；`command_id`、`auction_id`、`world_id`、`sequence`、`expected_state_version`
 * 缺失或为 `null` 时按零值处理。三项计划时间缺省为 `null`。 */
export interface AdminCreateAuctionRequest {
  command_id: string;
  auction_id: string;
  world_id: string;
  sequence: number;
  preview_scheduled_at: string | null;
  scheduled_open_at: string | null;
  scheduled_end_at: string | null;
  expected_state_version: number;
}

/** Admin 建场成功响应。该结构沿用 Rust 的开放反序列化（允许未知键）；建场接口判定成功时
 * 应按旧浏览器的闭集解码使用 `AdminAuctionLifecycleResponseSchema`。 */
export const AdminAuctionCreateResponseSchema = struct({
  auction: AdminAuctionCommandResponseSchema,
});

/** 解析 `AdminAuctionCreateResponseSchema` 的建场命令回执必须传入的字段策略。 */
export const ADMIN_AUCTION_CREATE_RESPONSE_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;

/** Admin 建场成功响应；携带旧版生命周期投影。 */
export type AdminAuctionCreateResponse = v.InferOutput<typeof AdminAuctionCreateResponseSchema>;

/** 计划修改命令；`command_id`、`expected_state_version` 缺失或为 `null` 时按零值处理，
 * 三项计划时间缺省为 `null`。 */
export interface AdminUpdateAuctionScheduleRequest {
  command_id: string;
  preview_scheduled_at: string | null;
  scheduled_open_at: string | null;
  scheduled_end_at: string | null;
  expected_state_version: number;
}

/** Admin 拍卖计划修改成功响应；返回列表同形投影以显式携带三项计划时间。 */
export const AdminUpdateAuctionScheduleResponseSchema = struct({
  auction: AdminAuctionResponseSchema,
});

/** 解析 `AdminUpdateAuctionScheduleResponseSchema` 的计划修改命令回执必须传入的字段策略。 */
export const ADMIN_UPDATE_AUCTION_SCHEDULE_RESPONSE_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;

/** Admin 拍卖计划修改成功响应；返回列表同形投影以显式携带三项计划时间。 */
export type AdminUpdateAuctionScheduleResponse = v.InferOutput<
  typeof AdminUpdateAuctionScheduleResponseSchema
>;

/** `source_kind` 缺省时沿用旧契约的 URL 来源；URL 与 canonical JSON 必须二选一。装备 key
 * 只在 HTTPS 请求和应用适配器的短生命周期内使用，绝不进入成功响应。
 * 完整来源 URL 同样包含装备 key；调试输出不能只遮蔽 canonical JSON 中的独立 key 字段。 */
export interface AdminIntakeRequest {
  command_id: string;
  source_kind: string | null;
  equipment_url: string | null;
  equipment: AdminCanonicalEquipmentRequest | null;
  start_price: number;
  player_id: string;
}

/** Admin canonical 装备输入 DTO。
 * 旧版使用 pointer-heavy wire 区分“字段缺失/`null`”和合法零值；因此除 `level` 外的必填字段
 * 在 DTO 中保留 `Option`，由 HTTP 防腐层统一映射为 `invalid_auction_command`。严格字段闭集仍由
 * `deny_unknown_fields` 保证。 */
export interface AdminCanonicalEquipmentRequest {
  external_equipment_id: string | null;
  name: string | null;
  category: string | null;
  /** 历史 canonical 基础名，护甲仍为种类与部位的组合（如 `Phase Robe`）。 */
  equipment_type: string | null;
  quality: string | null;
  prefix: string | null;
  suffix: string | null;
  level: number | null;
  tradeable: boolean | null;
  stats: Readonly<Record<string, AdminCanonicalEquipmentStatRequest>> | null;
  key: string | null;
}

/** canonical 装备属性 DTO。 */
export interface AdminCanonicalEquipmentStatRequest {
  value: string | null;
  /** 后端在值为 `null` 时省略该键。 */
  base?: number;
}

/** Admin 装备录入成功响应；字段与旧版 `addIntakeItem` 契约保持一致。nullable 的
 * `equipment_level` 在旧 decoder 中必须显式出现（可为 `null`），未知字段一律拒绝。
 * UUID 字段按 Rust `Uuid` 的 `Display` 输出统一为小写连字符格式。 */
export const AdminIntakeResponseSchema = strictStruct({
  auction_id: CANONICAL_UUID,
  auction_sequence: integer,
  auction_state_version: integer,
  preview_revision: integer,
  submission_id: CANONICAL_UUID,
  lot_id: CANONICAL_UUID,
  lot_number: int32,
  lot_state_version: integer,
  equipment_id: CANONICAL_UUID,
  external_equipment_id: v.string(),
  equipment_name: v.string(),
  equipment_category: v.string(),
  equipment_level: requiredNullable(int32),
  start_price: integer,
  seller_player_id: v.string(),
});

/** 解析 `AdminIntakeResponseSchema` 的装备录入命令回执必须传入的字段策略。 */
export const ADMIN_INTAKE_RESPONSE_FIELD_POLICY = {
  uniqueKeys: "all",
  numbers: "integers",
} as const;

/** Admin 装备录入成功响应；UUID 字段解码后统一为小写连字符格式。 */
export type AdminIntakeResponse = v.InferOutput<typeof AdminIntakeResponseSchema>;
