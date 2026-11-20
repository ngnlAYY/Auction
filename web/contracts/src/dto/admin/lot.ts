// Admin lot DTO：lot 安全投影、draft 修改结果、列表分页与列表响应。

import * as v from "valibot";
import { int32, integer, jsonValue, requiredNullable, strictStruct } from "../../wire";
import type { AdminStrictPaginationResponse } from "./pagination";
import { AdminStrictPaginationResponseSchema } from "./pagination";
import { AdminAuctionResponseSchema } from "./settlement";

/** Admin lot 的安全投影；不包含装备密钥、代理上限或原始外部正文。
 * 反序列化按旧浏览器 decoder 的闭集语义收紧：nullable 字段必须显式出现（可为 `null`），
 * 未知字段一律拒绝；wire 冻结后服务端字段集不变，序列化输出不受影响。 */
export const AdminLotResponseSchema = strictStruct({
  id: v.string(),
  auction_id: v.string(),
  equipment_id: v.string(),
  submission_id: requiredNullable(v.string()),
  /** Rust `i32`。默认 JSON 解析无法区分 `1` 与 `1.0`；Admin lot API 解码时必须传入本模块的
   * 字段策略，覆盖本类型的全部五个整数字段，否则 `1.0` 会被接受而 Rust 直接反序列化会拒绝。 */
  lot_number: int32,
  status: v.string(),
  start_price: integer,
  current_price: integer,
  leader_user_id: requiredNullable(v.string()),
  effective_end_at: requiredNullable(v.string()),
  slow_due_at: requiredNullable(v.string()),
  slow_generation: integer,
  slow_authorization_id: requiredNullable(v.string()),
  state_version: integer,
  created_at: v.string(),
  updated_at: v.string(),
  external_equipment_id: v.string(),
  equipment_name: v.string(),
  equipment_category: v.string(),
  equipment_attributes: jsonValue,
  equipment_collected_at: v.string(),
  seller: requiredNullable(v.string()),
  submission_source_kind: requiredNullable(v.string()),
  submission_source_reference: requiredNullable(v.string()),
  mailbox_key: requiredNullable(v.string()),
  external_mail_id: requiredNullable(v.string()),
  submission_received_at: requiredNullable(v.string()),
});

export type AdminLotResponse = v.InferOutput<typeof AdminLotResponseSchema>;

/** Admin draft lot 修改成功响应；返回同一事务快照中的 lot 安全投影。 */
export const AdminLotUpdateResponseSchema = strictStruct({
  lot: AdminLotResponseSchema,
});

export type AdminLotUpdateResponse = v.InferOutput<typeof AdminLotUpdateResponseSchema>;

/** Admin lot 修改回执用于判定命令是否被接受；解析统一成功信封时必须传入。
 * `equipment_attributes` 是自由 JSON，不能整份要求整数词法，因此只列出具型整数字段。 */
export const ADMIN_LOT_UPDATE_FIELD_POLICY = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "lot", "lot_number"],
    ["data", "lot", "start_price"],
    ["data", "lot", "current_price"],
    ["data", "lot", "slow_generation"],
    ["data", "lot", "state_version"],
  ],
} as const;

/** Admin lot 列表数字分页。 */
export type AdminLotPaginationResponse = AdminStrictPaginationResponse;

/** Admin lot 列表响应；拍卖和 lot 来自同一数据库快照。 */
export const AdminLotListResponseSchema = strictStruct({
  auction: AdminAuctionResponseSchema,
  lots: v.pipe(v.array(AdminLotResponseSchema), v.readonly()),
  pagination: AdminStrictPaginationResponseSchema,
});

export type AdminLotListResponse = v.InferOutput<typeof AdminLotListResponseSchema>;

/** Admin lot 列表读取的字段策略；拍卖、lot 与分页的具型整数字段都必须按整数解析。
 * 列表中的 `auction.status` 决定是否开放修改，`lot.state_version` 决定草稿能否提交并作为
 * 写入前置条件，因此解析完整响应原文时必须传入并拒绝任意层的重复键。
 * `equipment_attributes` 是自由 JSON，不能整份要求整数词法，不列入路径。 */
export const ADMIN_LOT_LIST_FIELD_POLICY = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "auction", "sequence"],
    ["data", "auction", "preview_revision"],
    ["data", "auction", "state_version"],
    ["data", "auction", "lot_count"],
    ["data", "lots", null, "lot_number"],
    ["data", "lots", null, "start_price"],
    ["data", "lots", null, "current_price"],
    ["data", "lots", null, "slow_generation"],
    ["data", "lots", null, "state_version"],
    ["data", "pagination", "page"],
    ["data", "pagination", "page_size"],
    ["data", "pagination", "total"],
  ],
} as const;
