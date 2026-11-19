// 预览 lot、详情装备摘要与完整装备结构的字段闭集和跨字段校验。

import { type JsonObject, jsonKeysOnly } from "@auction/web-shared/net/wire";
import { validUuid } from "@auction/web-shared/ui/validation";

import {
  gameUrl,
  nonempty,
  object,
  optionalBool,
  optionalFinite,
  optionalInt,
  optionalNonempty,
  optionalNonnegative,
  optionalPrimaryAttributeInitials,
  optionalString,
  positive,
  rawNonemptyStr,
  string,
  trimmedNonemptyStr,
} from "..";

/** 校验预览 lot 的字段闭集、身份、装备摘要及详情中的完整装备。 */
export function validPreviewLot(
  value: unknown,
  auctionId: string,
  lotId: string | null,
  detail: boolean,
): boolean {
  const decoded = object(value);
  if (decoded === null) return false;
  const optional = detail
    ? [
        "equipment_category",
        "equipment_level",
        "primary_attribute_initials",
        "avg_roll",
        "equipment",
      ]
    : ["equipment_category", "equipment_level", "primary_attribute_initials", "avg_roll"];
  return (
    jsonKeysOnly(
      decoded,
      [
        "id",
        "auction_id",
        "code",
        "status",
        "seller_name",
        "start_price",
        "equipment_name",
        "equipment_group",
        "game_equipment_url",
      ],
      optional,
    ) &&
    validUuid(string(decoded.id) ?? "") &&
    (lotId === null || string(decoded.id) === lotId) &&
    string(decoded.auction_id) === auctionId &&
    nonempty(decoded.code) &&
    string(decoded.status) === "pending" &&
    nonempty(decoded.seller_name) &&
    positive(decoded.start_price) &&
    nonempty(decoded.equipment_name) &&
    validEquipmentGroup(decoded.equipment_group) &&
    optionalNonempty(decoded, "equipment_category") &&
    optionalNonnegative(decoded, "equipment_level") &&
    optionalPrimaryAttributeInitials(decoded, "primary_attribute_initials") &&
    optionalFinite(decoded, "avg_roll") &&
    gameUrl(decoded.game_equipment_url) &&
    (!detail || validPreviewDetailEquipment(decoded))
  );
}

function validPreviewDetailEquipment(summary: JsonObject): boolean {
  const equipment = object(summary.equipment ?? null);
  if (equipment === null || !validPreviewEquipment(equipment)) return false;
  return (
    string(equipment.name) === string(summary.equipment_name) &&
    string(equipment.equipment_group) === string(summary.equipment_group) &&
    optionalString(equipment, "category") === optionalString(summary, "equipment_category") &&
    optionalInt(equipment, "level") === optionalInt(summary, "equipment_level") &&
    optionalString(equipment, "primary_attribute_initials") ===
      optionalString(summary, "primary_attribute_initials") &&
    string(equipment.game_equipment_url) === string(summary.game_equipment_url)
  );
}

function validPreviewEquipment(value: JsonObject): boolean {
  return (
    jsonKeysOnly(
      value,
      ["name", "equipment_group", "game_equipment_url"],
      ["category", "level", "primary_attribute_initials", "info_summary", "stats", "avg_roll"],
    ) &&
    nonempty(value.name) &&
    validEquipmentGroup(value.equipment_group) &&
    optionalNonempty(value, "category") &&
    optionalNonnegative(value, "level") &&
    optionalPrimaryAttributeInitials(value, "primary_attribute_initials") &&
    optionalNonempty(value, "info_summary") &&
    optionalStats(value, "stats") &&
    optionalFinite(value, "avg_roll") &&
    gameUrl(value.game_equipment_url)
  );
}

/** 校验完整装备结构；`full` 为真时要求属性对象存在。 */
export function validEquipment(value: unknown, full: boolean): boolean {
  const decoded = object(value);
  if (decoded === null) return false;
  const required = ["name", "category", "equipment_group", "game_equipment_url"];
  const optional = [
    "attributes",
    "level",
    "primary_attribute_initials",
    "info_summary",
    "stats",
    "avg_roll",
  ];
  return (
    jsonKeysOnly(decoded, required, optional) &&
    nonempty(decoded.name) &&
    validEquipmentGroup(decoded.equipment_group) &&
    nonempty(decoded.category) &&
    optionalNonnegative(decoded, "level") &&
    optionalPrimaryAttributeInitials(decoded, "primary_attribute_initials") &&
    optionalNonempty(decoded, "info_summary") &&
    optionalStats(decoded, "stats") &&
    optionalFinite(decoded, "avg_roll") &&
    gameUrl(decoded.game_equipment_url) &&
    (!full || optionalAttributes(decoded, "attributes"))
  );
}

function validEquipmentGroup(value: unknown): boolean {
  return string(value) === "rare" || string(value) === "fusion";
}

function validAttributes(value: unknown): boolean {
  const decoded = object(value);
  if (decoded === null) return false;
  return (
    jsonKeysOnly(
      decoded,
      ["quality"],
      ["type", "prefix", "suffix", "level", "tradeable", "stats"],
    ) &&
    nonempty(decoded.quality) &&
    // 已存装备允许缺省的词缀为空串，无等级以 null 保存。
    ["type", "prefix", "suffix"].every(
      (key) => !Object.hasOwn(decoded, key) || typeof decoded[key] === "string",
    ) &&
    (decoded.level === null || optionalNonnegative(decoded, "level")) &&
    optionalBool(decoded, "tradeable") &&
    optionalAttributeStats(decoded, "stats")
  );
}

function validAttributeStats(value: unknown): boolean {
  const decoded = object(value);
  if (decoded === null) return false;
  for (const [name, stat] of Object.entries(decoded)) {
    if (!trimmedNonemptyStr(name)) return false;
    const statObject = object(stat);
    if (
      statObject === null ||
      !jsonKeysOnly(statObject, ["value"], ["base"]) ||
      !nonempty(statObject.value) ||
      (statObject.base !== null && !optionalFinite(statObject, "base"))
    ) {
      return false;
    }
  }
  return true;
}

function validStats(value: unknown): boolean {
  const decoded = object(value);
  if (decoded === null) return false;
  for (const [name, stat] of Object.entries(decoded)) {
    if (!rawNonemptyStr(name)) return false;
    const statObject = object(stat);
    if (
      statObject === null ||
      !jsonKeysOnly(statObject, ["value"], ["base", "roll"]) ||
      !nonempty(statObject.value) ||
      !optionalFinite(statObject, "base") ||
      !optionalFinite(statObject, "roll")
    ) {
      return false;
    }
  }
  return true;
}

function optionalAttributes(value: JsonObject, key: string): boolean {
  return !Object.hasOwn(value, key) || validAttributes(value[key]);
}

function optionalStats(value: JsonObject, key: string): boolean {
  return !Object.hasOwn(value, key) || validStats(value[key]);
}

function optionalAttributeStats(value: JsonObject, key: string): boolean {
  return !Object.hasOwn(value, key) || validAttributeStats(value[key]);
}
