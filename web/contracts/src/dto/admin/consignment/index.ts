// Admin 寄售列表、详情、拍卖碰撞与下架请求的线协议。

import * as v from "valibot";

import { requiredNullable, strictStruct } from "../../../wire";
import { listingPageSchema } from "../../public/consignment/catalog";
import { OwnedListingSchema } from "../../public/consignment/owned";
import {
  ConsignmentTimestampSchema,
  ConsignmentUuidSchema,
  consignmentTextSchema,
  ListingVersionSchema,
} from "../../public/consignment/scalar";
import { ActiveListingStateSchema } from "../../public/consignment/states";

// 合同未定义任意裸凭据的识别规则。沿用 Rust 的装备路径校验，拒绝可识别的链接。
const DELIST_REASON = v.pipe(
  consignmentTextSchema(256),
  v.check((reason) => !/\/equip\/[0-9]+\/[0-9a-f]+(?![0-9a-z])/i.test(reason)),
);

/** 运营可见的挂单投影，不可借运营会话执行卖家操作。 */
export const AdminListingSchema = v.pipe(
  strictStruct({
    listing: OwnedListingSchema,
    seller_user_id: ConsignmentUuidSchema,
    seller_name: v.string(),
    admin_delist_reason: requiredNullable(DELIST_REASON),
    can_delist: v.boolean(),
  }),
  v.check(
    ({ listing, can_delist }) =>
      listing.allowed_actions.length === 0 &&
      (!can_delist || v.is(ActiveListingStateSchema, listing.effective_state)),
  ),
);
/** 运营列表和详情共用的单条记录。 */
export type AdminListing = v.InferOutput<typeof AdminListingSchema>;

/** 运营查询分页响应。 */
export const AdminListingPageSchema = listingPageSchema(AdminListingSchema);
/** 带准确总数的运营挂单页。 */
export type AdminListingPage = v.InferOutput<typeof AdminListingPageSchema>;

/** 挂单与已提交拍卖 lot 的碰撞记录。 */
export const ListingAuctionConflictSchema = strictStruct({
  lot_id: ConsignmentUuidSchema,
  detected_at: ConsignmentTimestampSchema,
});
/** 可供运营追溯的拍卖碰撞。 */
export type ListingAuctionConflict = v.InferOutput<typeof ListingAuctionConflictSchema>;

/** 详情只接受至多 50 条最近碰撞及明确的截断标记。 */
export const AdminListingDetailSchema = strictStruct({
  record: AdminListingSchema,
  conflicts: v.pipe(v.array(ListingAuctionConflictSchema), v.maxLength(50)),
  conflicts_truncated: v.boolean(),
});
/** Admin 挂单详情及安全碰撞投影。 */
export type AdminListingDetail = v.InferOutput<typeof AdminListingDetailSchema>;

/** 不变量 1、6、7：下架绑定稳定命令、预期版本与私有说明。 */
export interface AdminDelistListing {
  command_id: string;
  expected_version: number;
  reason: string;
}

/** 下架请求的闭集线协议，说明按规范单行与字节上限校验。 */
export const AdminDelistListingSchema = strictStruct({
  command_id: ConsignmentUuidSchema,
  expected_version: ListingVersionSchema,
  reason: DELIST_REASON,
});
