// 公开 v2 目录分页；详情沿用 listing-v2 的 `ListingDetailV2Schema`。

import { listingPageSchema } from "./catalog";
import { ListingSummaryV2Schema } from "./listing-v2";

/** 公开 v2 目录的分页响应数据。 */
export const ListingSummaryV2PageSchema = listingPageSchema(ListingSummaryV2Schema);
