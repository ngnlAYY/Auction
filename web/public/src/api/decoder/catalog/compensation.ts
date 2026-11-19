// 补偿池批次、逐件结算明细及补偿轮次的字段闭集校验。

import { validUuid } from "@auction/web-shared/ui/validation";

import {
  accountNonempty,
  gameUrl,
  int,
  nonempty,
  nonnegative,
  object,
  positive,
  string,
  time,
} from "..";
import { validNumberedPage } from "./auction";

/** 校验补偿池余额、更新时间和逐轮结算明细。 */
export function validCompensationPool(value: unknown): boolean {
  const pool = object(value);
  if (pool === null) return false;
  const rounds = pool.rounds;
  // 两个世界共用一个补偿池，单行始终存在，因此余额与更新时间都必须给出。
  if (!validNumberedPage(pool) || !nonnegative(pool.balance)) return false;
  if (!time(pool.updated_at) || !Array.isArray(rounds)) return false;
  return rounds.every(validCompensationRound);
}

/** 校验单场逐件补偿明细页与所属轮次。 */
export function validCompensationLots(value: unknown): boolean {
  const page = object(value);
  if (page === null) return false;
  const lots = page.lots;
  if (!validNumberedPage(page) || !validCompensationRound(page.round) || !Array.isArray(lots))
    return false;
  return lots.every(validCompensationLot);
}

function validCompensationRound(value: unknown): boolean {
  const round = object(value);
  if (round === null) return false;
  const policyMode = string(round.policy_mode);
  return (
    validUuid(string(round.auction_id) ?? "") &&
    accountNonempty(round.world_id) &&
    positive(round.auction_sequence) &&
    (policyMode === "legacy_zero_v0" ||
      policyMode === "hold_curve_v1" ||
      policyMode === "hold_curve_v2") &&
    nonnegative(round.opening_balance) &&
    nonnegative(round.tax_income) &&
    nonnegative(round.discount_demand) &&
    nonnegative(round.compensation_out) &&
    nonnegative(round.closing_balance) &&
    nonnegative(round.movement_count) &&
    nonnegative(round.reversal_count) &&
    int(round.reversal_amount) !== null &&
    time(round.settled_at)
  );
}

function validCompensationLot(value: unknown): boolean {
  const lot = object(value);
  if (lot === null) return false;
  if (
    !validUuid(string(lot.lot_id) ?? "") ||
    !positive(lot.lot_number) ||
    !accountNonempty(lot.equipment_name) ||
    !accountNonempty(lot.equipment_category) ||
    // 明细返回完整 ID/key 链接，复用目录校验，但不接受目录兼容的空串。
    !nonempty(lot.game_equipment_url) ||
    !gameUrl(lot.game_equipment_url) ||
    !positive(lot.end_price)
  ) {
    return false;
  }
  const holdPpm = int(lot.hold_ppm);
  const adjustmentPpm = int(lot.adjustment_ppm);
  if (
    holdPpm === null ||
    holdPpm < 0 ||
    holdPpm > 1_000_000 ||
    adjustmentPpm === null ||
    adjustmentPpm < -1_000_000 ||
    adjustmentPpm > 1_000_000 ||
    !nonnegative(lot.tax_amount) ||
    !nonnegative(lot.discount_amount) ||
    !nonnegative(lot.compensation_amount) ||
    !nonnegative(lot.buyer_amount) ||
    !nonnegative(lot.seller_amount) ||
    !accountNonempty(lot.buyer_name) ||
    !accountNonempty(lot.seller_name) ||
    typeof lot.voided !== "boolean"
  ) {
    return false;
  }
  if (!Object.hasOwn(lot, "reversal")) return true;
  const reversal = object(lot.reversal);
  return (
    reversal !== null &&
    int(reversal.amount) !== null &&
    nonnegative(reversal.tax_shortfall) &&
    nonnegative(reversal.balance_after) &&
    time(reversal.reversed_at)
  );
}
