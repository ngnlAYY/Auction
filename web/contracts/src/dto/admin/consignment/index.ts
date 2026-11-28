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

const MAX_AUCTION_CONFLICTS = 50;
const EQUIPMENT_KEY = /^[0-9a-f]{10}$/i;

/** 运营下架说明的凭据筛查；v1 与 v2 Admin 记录共用。 */
export const DELIST_REASON = v.pipe(
  consignmentTextSchema(256),
  v.check((reason) => !/\/equip[/?]|key=/i.test(reason)),
  v.check((reason) => !reason.split(/[^0-9a-z]/i).some((token) => EQUIPMENT_KEY.test(token))),
  v.check((reason) => !reason.split(/[^0-9a-z_-]/i).some((token) => token.length === 43)),
);

/** 运营可见的挂单投影，不可借运营会话执行卖家操作。 */
export const AdminListingSchema = v.pipe(
  strictStruct({
    listing: OwnedListingSchema,
    // 免登录非寄售挂单没有站内账户。
    seller_user_id: requiredNullable(ConsignmentUuidSchema),
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

/** 详情接受至多 50 条最近碰撞。服务端保证 `detected_at DESC, lot_id DESC`，解码器不重排也不校验顺序。 */
export const AdminListingDetailSchema = v.pipe(
  strictStruct({
    record: AdminListingSchema,
    conflicts: v.pipe(v.array(ListingAuctionConflictSchema), v.maxLength(MAX_AUCTION_CONFLICTS)),
    conflicts_truncated: v.boolean(),
  }),
  v.check(
    ({ conflicts, conflicts_truncated }) =>
      !conflicts_truncated || conflicts.length === MAX_AUCTION_CONFLICTS,
  ),
);
/** Admin 挂单详情及安全碰撞投影。 */
export type AdminListingDetail = v.InferOutput<typeof AdminListingDetailSchema>;

/**
 * 不变量 1、6、7：下架绑定稳定命令、预期版本与私有说明。
 * 说明只进 Admin 私有视图。与应用层一致，在全文按 ASCII 大小写无关匹配
 * `/equip/`、`/equip?`、`key=`，并拒绝独立的 10 位十六进制片段和
 * 恰为 43 个 ASCII 字母、数字、连字符或下划线的片段。
 */
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
