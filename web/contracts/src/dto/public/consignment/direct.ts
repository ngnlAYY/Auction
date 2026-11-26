// 免登录非寄售登记的请求、挂单进度与凭命令 ID 查询的首次决定。

import * as v from "valibot";

import { requiredNullable, strictStruct } from "../../../wire";
import { StoredCommandResultSchema } from "./commands";
import {
  ConsignmentTimestampSchema,
  ConsignmentUuidSchema,
  ListingPriceSchema,
  ListingVersionSchema,
} from "./scalar";
import { ListingReasonSchema, ListingStateSchema, listingStateMatchesMode } from "./states";

/** 不变量 1、6、7：稳定命令 ID 与整数标价；模式固定非寄售，装备链接仅用于这次请求。 */
export interface RegisterDirectListing {
  command_id: string;
  seller_game_name: string;
  equipment_url: string;
  price: number;
}

/** 免登录登记请求的闭集线协议；名称与链接语义由服务端校验。 */
export const RegisterDirectListingSchema = strictStruct({
  command_id: ConsignmentUuidSchema,
  seller_game_name: v.string(),
  equipment_url: v.string(),
  price: ListingPriceSchema,
});

/** 不变量 6：免登录挂单只有非寄售模式，寄售专属状态不可能出现。 */
export const DirectListingProgressSchema = v.pipe(
  strictStruct({
    listing_id: ConsignmentUuidSchema,
    version: ListingVersionSchema,
    effective_state: ListingStateSchema,
    effective_reason: requiredNullable(ListingReasonSchema),
    expires_at: ConsignmentTimestampSchema,
  }),
  v.check(({ effective_state }) => listingStateMatchesMode(effective_state, "direct")),
);
/** 免登录挂单的当前进度，不含卖家名、费率与可执行动作。 */
export type DirectListingProgress = v.InferOutput<typeof DirectListingProgressSchema>;

/** 不变量 1、6：只有已受理的登记带挂单，且挂单必须是回执定位的那一个。 */
export const DirectCommandResultSchema = v.pipe(
  strictStruct({
    result: StoredCommandResultSchema,
    listing: requiredNullable(DirectListingProgressSchema),
  }),
  v.check(({ result: { receipt }, listing }) =>
    listing === null
      ? receipt.disposition === "rejected"
      : receipt.disposition === "registered" && receipt.listing_id === listing.listing_id,
  ),
);
/** 凭原命令 ID 查到的首次决定与挂单进度。 */
export type DirectCommandResult = v.InferOutput<typeof DirectCommandResultSchema>;
