// v2 寄售能力：两世界并行状态、季界与未来可放开的购买能力。

import * as v from "valibot";

import { requiredNullable, strictStruct } from "../../../wire";
import {
  LISTING_PRICE_MAX,
  ListingFeeBpsSchema,
  ListingModeSchema,
  ListingWorldSchema,
} from "./listing-options";
import { ConsignmentTimestampSchema } from "./scalar";

/** 冻结季历计算出的季态闭集。 */
export const SeasonStateV2Schema = v.picklist([
  "not_seasonal",
  "open",
  "preclose",
  "closed",
  "unknown",
]);
/** 世界季态。 */
export type SeasonStateV2 = v.InferOutput<typeof SeasonStateV2Schema>;

const WORLD_CAPABILITY = strictStruct({
  mail_ready: v.boolean(),
  purchase_ready: v.boolean(),
  season_state: SeasonStateV2Schema,
  cutoff_at: requiredNullable(ConsignmentTimestampSchema),
  season_end_at: requiredNullable(ConsignmentTimestampSchema),
});

function isConsistentWorld(world: v.InferOutput<typeof WORLD_CAPABILITY>): boolean {
  if (world.season_state === "unknown") {
    return (
      !world.mail_ready &&
      !world.purchase_ready &&
      world.cutoff_at === null &&
      world.season_end_at === null
    );
  }
  if (world.season_state === "not_seasonal") {
    return world.cutoff_at === null && world.season_end_at === null;
  }
  return world.cutoff_at !== null && world.season_end_at !== null;
}

/** 单个世界的真实收件与购买能力。 */
export const WorldCapabilityV2Schema = v.pipe(WORLD_CAPABILITY, v.check(isConsistentWorld));
/** 单个世界的能力投影。 */
export type WorldCapabilityV2 = v.InferOutput<typeof WorldCapabilityV2Schema>;

/** 键恰为 `main` 与 `isekai` 的两世界能力对象。 */
export const ConsignmentWorldsV2Schema = strictStruct({
  main: WorldCapabilityV2Schema,
  isekai: WorldCapabilityV2Schema,
});
/** 两世界能力。 */
export type ConsignmentWorldsV2 = v.InferOutput<typeof ConsignmentWorldsV2Schema>;

const STATUS_FIELDS = strictStruct({
  enabled: v.boolean(),
  registration_ready: v.boolean(),
  registration_modes: v.array(ListingModeSchema),
  registration_worlds: v.array(ListingWorldSchema),
  fee_bps: ListingFeeBpsSchema,
  min_price: v.literal(1),
  max_price: v.literal(LISTING_PRICE_MAX),
  accepts_mail: v.boolean(),
  purchasable: v.boolean(),
  worlds: ConsignmentWorldsV2Schema,
});

/** 匿名可读的 v2 寄售能力。第 4 期生产者把购买两字段输出 false，但类型保留布尔。 */
export const ConsignmentStatusV2Schema = v.pipe(
  STATUS_FIELDS,
  v.check((status) => {
    if (status.registration_ready && !status.enabled) return false;
    if (
      !status.registration_ready &&
      (status.registration_modes.length > 0 || status.registration_worlds.length > 0)
    ) {
      return false;
    }
    if (
      status.registration_ready &&
      (status.registration_modes.length === 0 || status.registration_worlds.length === 0)
    ) {
      return false;
    }
    if (status.worlds.main.season_state !== "not_seasonal") return false;
    if (status.worlds.isekai.season_state === "not_seasonal") return false;
    return (
      status.accepts_mail === (status.worlds.main.mail_ready || status.worlds.isekai.mail_ready) &&
      status.purchasable ===
        (status.worlds.main.purchase_ready || status.worlds.isekai.purchase_ready)
    );
  }),
);
/** v2 服务能力快照。 */
export type ConsignmentStatusV2 = v.InferOutput<typeof ConsignmentStatusV2Schema>;
