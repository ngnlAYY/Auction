// 匿名寄售能力的解码入口，供首屏导航读取。

import { array, boolean, check, type InferOutput, literal, pipe } from "valibot";

import { strictStruct } from "../../../wire";
import {
  LISTING_PRICE_MAX,
  ListingFeeBpsSchema,
  ListingModeSchema,
  ListingWorldSchema,
} from "./listing-options";

/** 匿名可读的寄售能力，前三期收件和购买固定为 false。 */
export const ConsignmentStatusSchema = pipe(
  strictStruct({
    enabled: boolean(),
    registration_ready: boolean(),
    registration_modes: array(ListingModeSchema),
    registration_worlds: array(ListingWorldSchema),
    accepts_mail: literal(false),
    purchasable: literal(false),
    fee_bps: ListingFeeBpsSchema,
    min_price: literal(1),
    max_price: literal(LISTING_PRICE_MAX),
  }),
  check((status) =>
    status.registration_ready
      ? status.enabled &&
        status.registration_modes.length > 0 &&
        status.registration_worlds.length > 0
      : status.registration_modes.length === 0 && status.registration_worlds.length === 0,
  ),
);
/** Public 页面可用的服务能力快照。 */
export type ConsignmentStatus = InferOutput<typeof ConsignmentStatusSchema>;
