// lot 目录与详情响应的列表、投影、最终限制与分类统计校验。

import { EQUIPMENT_CATEGORY_CODES } from "@auction/contracts/dto/public/catalog/lot";
import { type JsonObject, jsonKeysOnly } from "@auction/web-shared/net/wire";
import { validUuid } from "@auction/web-shared/ui/validation";

import { type CatalogLevelFilter, catalogLevelFilterMatchesLevel } from "../../model";
import {
  boolValue,
  int,
  nonnegative,
  object,
  positive,
  positivePageSize,
  revision,
  string,
  time,
} from "..";
import { validLiveLot } from "./live-lot";
import { validPreviewLot } from "./preview-equipment";

/** 校验 lot 目录页的投影绑定、编号页、分类计数与逐行筛选一致性。 */
export function validLotList(
  value: unknown,
  auctionId: string,
  category: string | null,
  equipmentGroup: string | null,
  levels: CatalogLevelFilter,
): boolean {
  const decoded = object(value);
  if (decoded === null) return false;
  if (
    !jsonKeysOnly(
      decoded,
      [
        "auction_id",
        "auction_status",
        "projection_kind",
        "presentation_mode",
        "bidding_open",
        "opens_at",
        "preview_revision",
        "lots",
        "total",
        "page",
        "page_size",
        "category_counts",
      ],
      ["final_restriction"],
    ) ||
    !validProjection(decoded, auctionId) ||
    !positive(decoded.page) ||
    !positivePageSize(decoded.page_size) ||
    !nonnegative(decoded.total) ||
    !validCategoryCounts(decoded.category_counts)
  ) {
    return false;
  }
  const categoryKey = category ?? "all";
  const counts = object(decoded.category_counts);
  if (counts === null) return false;
  if (
    !categoryCountKeys().includes(categoryKey) ||
    int(counts[categoryKey]) !== int(decoded.total)
  ) {
    return false;
  }
  const lots = decoded.lots;
  if (!Array.isArray(lots)) return false;
  return lots.every((lot) => {
    const preview = string(decoded.projection_kind) === "preview";
    const valid = preview
      ? validPreviewLot(lot, auctionId, null, false)
      : validLiveLot(lot, auctionId, null) &&
        (string(decoded.auction_status) !== "settled" || object(lot)?.status !== "live");
    // 请求分组、等级和每行服务端判定必须一致；只校验，不把漏筛响应变成残缺页。
    const lotObject = object(lot);
    const equipment = preview ? lotObject : lotObject === null ? null : object(lotObject.equipment);
    const level = equipment === null ? null : int(equipment[preview ? "equipment_level" : "level"]);
    return (
      valid &&
      catalogLevelFilterMatchesLevel(levels, level) &&
      (equipmentGroup === null ||
        (equipment !== null && string(equipment.equipment_group) === equipmentGroup))
    );
  });
}

/** 校验单个 lot 详情页的投影绑定与 lot 身份。 */
export function validLotDetail(value: unknown, auctionId: string | null, lotId: string): boolean {
  const decoded = object(value);
  if (decoded === null) return false;
  const responseAuctionId = string(decoded.auction_id);
  if (responseAuctionId === null) return false;
  if (
    (auctionId !== null && auctionId !== responseAuctionId) ||
    !jsonKeysOnly(
      decoded,
      [
        "auction_id",
        "auction_status",
        "projection_kind",
        "presentation_mode",
        "bidding_open",
        "opens_at",
        "preview_revision",
        "lot",
      ],
      ["final_restriction"],
    ) ||
    !validProjection(decoded, responseAuctionId)
  ) {
    return false;
  }
  if (string(decoded.projection_kind) === "preview") {
    return validPreviewLot(decoded.lot ?? null, responseAuctionId, lotId, true);
  }
  return (
    validLiveLot(decoded.lot ?? null, responseAuctionId, lotId) &&
    (string(decoded.auction_status) !== "settled" || object(decoded.lot ?? null)?.status !== "live")
  );
}

function validProjection(value: JsonObject, auctionId: string): boolean {
  const status = string(value.auction_status);
  const projectionKind = string(value.projection_kind);
  const presentationMode = string(value.presentation_mode);
  const biddingOpen = boolValue(value.bidding_open);
  return (
    string(value.auction_id) === auctionId &&
    validUuid(auctionId) &&
    (status === "final" ||
      status === "live" ||
      status === "preview" ||
      status === "settled" ||
      status === "settling") &&
    (projectionKind === "live" || projectionKind === "preview") &&
    (presentationMode === "live" ||
      presentationMode === "opening" ||
      presentationMode === "preview") &&
    biddingOpen !== null &&
    time(value.opens_at) &&
    revision(value.preview_revision) &&
    validFinalRestriction(value) &&
    (projectionKind === "preview"
      ? status === "preview" &&
        biddingOpen === false &&
        (presentationMode === "opening" || presentationMode === "preview") &&
        !Object.hasOwn(value, "final_restriction")
      : projectionKind === "live"
        ? (presentationMode === "live" && (status === "live" || status === "final")) ||
          ((status === "settling" || status === "settled") &&
            biddingOpen === false &&
            presentationMode === "live")
        : false)
  );
}

function validFinalRestriction(value: JsonObject): boolean {
  const restriction = object(value.final_restriction ?? null);
  switch (string(value.auction_status)) {
    case "final":
      return (
        restriction !== null &&
        jsonKeysOnly(
          restriction,
          ["new_slow_proxy_allowed", "existing_slow_proxy_converted"],
          [],
        ) &&
        boolValue(restriction.new_slow_proxy_allowed) === false &&
        boolValue(restriction.existing_slow_proxy_converted) === true
      );
    default:
      return !Object.hasOwn(value, "final_restriction");
  }
}

/** 分类统计的完整键集合：`all` 后按契约表顺序排列各大类的公开键。 */
function categoryCountKeys(): readonly string[] {
  return ["all", ...EQUIPMENT_CATEGORY_CODES.map(([key]) => key)];
}

function validCategoryCounts(value: unknown): boolean {
  const decoded = object(value);
  if (decoded === null) return false;
  const keys = categoryCountKeys();
  if (!jsonKeysOnly(decoded, keys, []) || !keys.every((key) => nonnegative(decoded[key])))
    return false;
  let categoriesTotal = 0;
  for (const key of keys.slice(1)) {
    const count = int(decoded[key]);
    if (count !== null) categoriesTotal += count;
  }
  return categoriesTotal <= (int(decoded.all) ?? -1);
}
