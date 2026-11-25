// 本人 v2 寄售命令回执查询（D9 §6.1、§6.3）：原成功回执或已入账的确定拒绝。
//
// 成功回执与原写响应的 data 逐字段相同，复用各命令的 v2 回执；确定拒绝只给安全码、命令 ID 与首次
// HTTP 状态。各变体的 operation / disposition 字面量互不相同，联合解码不会歧义。

import * as v from "valibot";

import { integer, strictStruct } from "../../../wire";
import { DepositReservationReceiptSchema } from "./deposit";
import { ListingGenerationSchema, WithdrawReceiptV2Schema } from "./listing-v2";
import { PurchaseReceiptV2Schema } from "./purchase-v2";
import { ConsignmentUuidSchema, ListingPriceSchema, ListingVersionSchema } from "./scalar";

/** 重列的 `200 relisted` 回执：挂单回到 available，带沿用的价格与新处分代序号。 */
export const RelistReceiptV2Schema = v.pipe(
  strictStruct({
    schema_version: v.literal(2),
    operation: v.literal("relist"),
    disposition: v.literal("relisted"),
    command_id: ConsignmentUuidSchema,
    listing_id: ConsignmentUuidSchema,
    listing_version: ListingVersionSchema,
    state: v.literal("available"),
    price: ListingPriceSchema,
    generation: ListingGenerationSchema,
  }),
  // 重列是上代之后的新代，最小为 2。
  v.check(({ generation }) => generation >= 2),
);
/** 重列回执。 */
export type RelistReceiptV2 = v.InferOutput<typeof RelistReceiptV2Schema>;

/** 本人 v2 写命令入账的确定拒绝码闭集（D9 §6.2 的购买、重列、撤回与预约行）。 */
export const OwnedCommandRejectionCodeV2Schema = v.picklist([
  "stale_version",
  "price_changed",
  "listing_not_active",
  "equipment_committed",
  "custody_unproven",
  "operations_hold",
  "season_closed",
  "self_purchase",
  "bot_party",
  "participant_ineligible",
  "funding_unavailable",
  "mail_unavailable",
  "equipment_busy",
  "listing_limit_reached",
  "action_not_allowed",
]);

/** 已入账的确定拒绝：只含安全码、命令 ID 与首次 HTTP 状态。 */
export const RejectedCommandV2Schema = v.pipe(
  strictStruct({
    schema_version: v.literal(2),
    operation: v.picklist(["purchase", "relist", "withdraw", "issue_reservation"]),
    disposition: v.literal("rejected"),
    command_id: ConsignmentUuidSchema,
    code: OwnedCommandRejectionCodeV2Schema,
    http_status: integer,
  }),
  // 确定拒绝只可能是 4xx，或确定写前登记过的可信 503。
  v.check(({ http_status }) => (http_status >= 400 && http_status < 500) || http_status === 503),
);
/** 已入账的确定拒绝。 */
export type RejectedCommandV2 = v.InferOutput<typeof RejectedCommandV2Schema>;

/** `GET /api/v2/me/consignment/commands/{command_id}` 的 data。 */
export const OwnedCommandReceiptV2Schema = v.union([
  PurchaseReceiptV2Schema,
  RelistReceiptV2Schema,
  WithdrawReceiptV2Schema,
  DepositReservationReceiptSchema,
  RejectedCommandV2Schema,
]);
/** 本人 v2 命令回执。 */
export type OwnedCommandReceiptV2 = v.InferOutput<typeof OwnedCommandReceiptV2Schema>;
