// 服务能力和挂单 DTO 共用的模式、世界、费用与标价上限。

import { type InferOutput, maxValue, picklist, pipe } from "valibot";

import { unsignedInteger } from "../../../wire";

/** 挂单创建时选定的不可变模式。 */
export const ListingModeSchema = picklist(["direct", "consigned"]);
/** 页面可选的挂单模式。 */
export type ListingMode = InferOutput<typeof ListingModeSchema>;

/** 寄售装备所属的两个世界。 */
export const ListingWorldSchema = picklist(["main", "isekai"]);
/** 寄售装备所属世界。 */
export type ListingWorld = InferOutput<typeof ListingWorldSchema>;

/** 寄售标价上限，以 main Credits 计。 */
export const LISTING_PRICE_MAX = 1_000_000_000;

/** 不变量 6、10：冻结费率以整数万分比传递。 */
export const ListingFeeBpsSchema = pipe(unsignedInteger, maxValue(9_999));
