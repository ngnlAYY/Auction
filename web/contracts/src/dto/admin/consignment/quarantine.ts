// Admin 隔离详情、观察投影、重查/解除请求与回执，以及运营止售回执。

import * as v from "valibot";

import { integer, JSON_MAX_SAFE_INTEGER, requiredNullable, strictStruct } from "../../../wire";
import { CommandUuidV4Schema } from "../../public/consignment/command";
import { ListingStampV2Schema } from "../../public/consignment/listing-v2";
import {
  ConsignmentTimestampSchema,
  ConsignmentUuidSchema,
  consignmentTextSchema,
  ExternalEquipmentIdSchema,
  ListingVersionSchema,
} from "../../public/consignment/scalar";
import { ListingWorldSchema } from "../../public/consignment/states";

const MAX_OBSERVATIONS = 200;
const MAX_ACCEPT_ITEMS = 20;

/** 隔离版本从 1 开始，必须能由浏览器精确表示。 */
export const QuarantineVersionSchema = v.pipe(
  integer,
  v.minValue(1),
  v.maxValue(JSON_MAX_SAFE_INTEGER),
);
/** 隔离版本。 */
export type QuarantineVersion = v.InferOutput<typeof QuarantineVersionSchema>;

/** 隔离投影的持久态。 */
export const QuarantineStateSchema = v.picklist(["active", "released"]);
/** 隔离状态。 */
export type QuarantineState = v.InferOutput<typeof QuarantineStateSchema>;

/** 隔离责任码；逐字对齐第 4 期领域枚举 `QuarantineReason` 的 12 个码。 */
export const QuarantineReasonSchema = v.picklist([
  "duplicate_raw_eid",
  "canonical_alias_ambiguous",
  "unknown_attachment",
  "list_detail_conflict",
  "asset_count_unknown",
  "same_name_changed_uid",
  "identity_unproven",
  "amount_unproven",
  "return_unproven",
  "take_unproven",
  "unsafe_return_shape",
  "suspected_system_rts",
]);
/** 隔离原因。 */
export type QuarantineReason = v.InferOutput<typeof QuarantineReasonSchema>;

/** Admin 在隔离详情上可执行的后续动作。 */
export const QuarantineActionSchema = v.picklist(["recheck", "release"]);
/** 隔离动作。 */
export type QuarantineAction = v.InferOutput<typeof QuarantineActionSchema>;

/** 脱敏清单中的附件种类。 */
export const QuarantineObservationItemKindSchema = v.picklist([
  "equipment",
  "credits",
  "cod",
  "unknown",
]);
/** 观察项种类。 */
export type QuarantineObservationItemKind = v.InferOutput<
  typeof QuarantineObservationItemKindSchema
>;

/** 隔离解除的六种最终决定。 */
export const QuarantineDecisionKindSchema = v.picklist([
  "accept",
  "return",
  "archive_empty",
  "acquire_return",
  "auction_return_case",
  "auction_recovery_return",
]);
/** 解除决定种类。 */
export type QuarantineDecisionKind = v.InferOutput<typeof QuarantineDecisionKindSchema>;

const EXTERNAL_MID = v.pipe(v.string(), v.regex(/^[1-9][0-9]{0,31}$/u));
const SHA256_HEX = v.pipe(v.string(), v.regex(/^[0-9a-f]{64}$/u));
const NON_NEGATIVE = v.pipe(integer, v.minValue(0), v.maxValue(JSON_MAX_SAFE_INTEGER));
const OBSERVATION_ORDINAL = v.pipe(integer, v.minValue(1), v.maxValue(MAX_OBSERVATIONS));

/** 脱敏清单的一项；不回显原 token、正文、装备 key 或受保护引用。 */
export const QuarantineObservationItemSchema = v.pipe(
  strictStruct({
    ordinal: OBSERVATION_ORDINAL,
    kind: QuarantineObservationItemKindSchema,
    external_equipment_id: requiredNullable(ExternalEquipmentIdSchema),
    amount: requiredNullable(NON_NEGATIVE),
  }),
  v.check(({ kind, external_equipment_id, amount }) => {
    if (kind === "equipment") {
      return external_equipment_id !== null && amount === null;
    }
    if (kind === "credits" || kind === "cod") {
      return external_equipment_id === null && amount !== null;
    }
    return external_equipment_id === null && amount === null;
  }),
);
/** 脱敏观察项。 */
export type QuarantineObservationItem = v.InferOutput<typeof QuarantineObservationItemSchema>;

/** 一条追加观察的安全投影；原文只在受控证据中保存。 */
export const QuarantineObservationSchema = v.pipe(
  strictStruct({
    observation_id: ConsignmentUuidSchema,
    observed_at: ConsignmentTimestampSchema,
    parser_version: consignmentTextSchema(64),
    digest: SHA256_HEX,
    attachment_count: v.pipe(integer, v.minValue(0), v.maxValue(MAX_OBSERVATIONS)),
    credits: requiredNullable(NON_NEGATIVE),
    cod: requiredNullable(NON_NEGATIVE),
    returnable: requiredNullable(v.boolean()),
    takeable: requiredNullable(v.boolean()),
    items: v.array(QuarantineObservationItemSchema),
  }),
  v.check(
    ({ attachment_count, items }) =>
      attachment_count === items.length &&
      new Set(items.map(({ ordinal }) => ordinal)).size === items.length &&
      // 唯一、下限 1 且不超过总数，三项合起来才是连续的 1..N。
      items.every(({ ordinal }) => ordinal <= attachment_count),
  ),
);
/** 观察安全投影。 */
export type QuarantineObservation = v.InferOutput<typeof QuarantineObservationSchema>;

function isAcceptListings(listings: readonly v.InferOutput<typeof ListingStampV2Schema>[]) {
  return (
    listings.length >= 1 &&
    listings.length <= MAX_ACCEPT_ITEMS &&
    listings.every(({ state }) => state === "deposit_accepted") &&
    new Set(listings.map(({ listing_id }) => listing_id)).size === listings.length
  );
}

/** 隔离解除后的最终决定与 owner 引用。 */
export const QuarantineReleaseOutcomeSchema = v.union([
  v.pipe(
    strictStruct({
      decision_kind: v.literal("accept"),
      listing_changes: v.array(ListingStampV2Schema),
    }),
    v.check(({ listing_changes }) => isAcceptListings(listing_changes)),
  ),
  strictStruct({ decision_kind: v.literal("return") }),
  strictStruct({ decision_kind: v.literal("archive_empty") }),
  strictStruct({
    decision_kind: v.literal("acquire_return"),
    return_intake_id: ConsignmentUuidSchema,
    original_round: v.pipe(integer, v.minValue(1), v.maxValue(JSON_MAX_SAFE_INTEGER)),
    take_work_id: ConsignmentUuidSchema,
    take_delivery_id: ConsignmentUuidSchema,
  }),
  strictStruct({
    decision_kind: v.literal("auction_return_case"),
    return_mail_id: ConsignmentUuidSchema,
    return_evidence_id: ConsignmentUuidSchema,
    obligation_id: requiredNullable(ConsignmentUuidSchema),
  }),
  strictStruct({
    decision_kind: v.literal("auction_recovery_return"),
    obligation_id: ConsignmentUuidSchema,
    return_mail_id: ConsignmentUuidSchema,
    recovery_id: ConsignmentUuidSchema,
    recovery_handoff_id: ConsignmentUuidSchema,
  }),
]);
/** 解除结果与 owner 引用。 */
export type QuarantineReleaseOutcome = v.InferOutput<typeof QuarantineReleaseOutcomeSchema>;

/** 已解除隔离的最终决定记录。 */
export const QuarantineReleaseRecordSchema = strictStruct({
  release_id: ConsignmentUuidSchema,
  decision_id: ConsignmentUuidSchema,
  released_at: ConsignmentTimestampSchema,
  outcome: QuarantineReleaseOutcomeSchema,
});
/** 解除记录。 */
export type QuarantineReleaseRecord = v.InferOutput<typeof QuarantineReleaseRecordSchema>;

/** Admin 隔离详情。只返回安全投影，不含原证密文、正文、预约码或装备 key。 */
export const QuarantineDetailSchema = v.pipe(
  strictStruct({
    quarantine_id: ConsignmentUuidSchema,
    version: QuarantineVersionSchema,
    state: QuarantineStateSchema,
    world: ListingWorldSchema,
    external_mid: EXTERNAL_MID,
    first_decision_id: ConsignmentUuidSchema,
    reason: QuarantineReasonSchema,
    // 累计观察总数；observations 只含最新至多 200 条。
    observation_total: NON_NEGATIVE,
    observations: v.array(QuarantineObservationSchema),
    allowed_actions: v.array(QuarantineActionSchema),
    release: requiredNullable(QuarantineReleaseRecordSchema),
  }),
  v.check((detail) => {
    if (
      detail.observations.length === 0 ||
      detail.observations.length > MAX_OBSERVATIONS ||
      detail.observation_total < detail.observations.length ||
      new Set(detail.observations.map(({ observation_id }) => observation_id)).size !==
        detail.observations.length ||
      new Set(detail.allowed_actions).size !== detail.allowed_actions.length
    ) {
      return false;
    }
    if (detail.state === "active") {
      return (
        detail.release === null &&
        detail.allowed_actions.every((action) => action === "recheck" || action === "release")
      );
    }
    return (
      detail.release !== null &&
      detail.allowed_actions.length === 0 &&
      detail.release.decision_id !== detail.first_decision_id
    );
  }),
);
/** 隔离详情。 */
export type QuarantineDetail = v.InferOutput<typeof QuarantineDetailSchema>;

const RELEASE_COMMON = {
  schema_version: v.literal(2),
  operation: v.literal("release_quarantine"),
  disposition: v.literal("quarantine_released"),
  command_id: ConsignmentUuidSchema,
  quarantine_id: ConsignmentUuidSchema,
  release_id: ConsignmentUuidSchema,
  decision_id: ConsignmentUuidSchema,
} as const;

/** 解除隔离的 200 回执；分支字段与最终决定平铺，未列字段一律拒绝。 */
export const QuarantineReleaseReceiptSchema = v.union([
  v.pipe(
    strictStruct({
      ...RELEASE_COMMON,
      decision_kind: v.literal("accept"),
      listing_changes: v.array(ListingStampV2Schema),
    }),
    v.check(({ listing_changes }) => isAcceptListings(listing_changes)),
  ),
  strictStruct({ ...RELEASE_COMMON, decision_kind: v.literal("return") }),
  strictStruct({ ...RELEASE_COMMON, decision_kind: v.literal("archive_empty") }),
  strictStruct({
    ...RELEASE_COMMON,
    decision_kind: v.literal("acquire_return"),
    return_intake_id: ConsignmentUuidSchema,
    original_round: v.pipe(integer, v.minValue(1), v.maxValue(JSON_MAX_SAFE_INTEGER)),
    take_work_id: ConsignmentUuidSchema,
    take_delivery_id: ConsignmentUuidSchema,
  }),
  strictStruct({
    ...RELEASE_COMMON,
    decision_kind: v.literal("auction_return_case"),
    return_mail_id: ConsignmentUuidSchema,
    return_evidence_id: ConsignmentUuidSchema,
    obligation_id: requiredNullable(ConsignmentUuidSchema),
  }),
  strictStruct({
    ...RELEASE_COMMON,
    decision_kind: v.literal("auction_recovery_return"),
    obligation_id: ConsignmentUuidSchema,
    return_mail_id: ConsignmentUuidSchema,
    recovery_id: ConsignmentUuidSchema,
    recovery_handoff_id: ConsignmentUuidSchema,
  }),
]);
/** 解除隔离回执。 */
export type QuarantineReleaseReceipt = v.InferOutput<typeof QuarantineReleaseReceiptSchema>;

/** Admin 重查隔离。不变量 1、6：稳定 UUIDv4 与预期版本。 */
export interface QuarantineRecheckRequest {
  command_id: string;
  expected_version: number;
}

/** 重查隔离请求的闭集线协议。 */
export const QuarantineRecheckRequestSchema = strictStruct({
  command_id: CommandUuidV4Schema,
  expected_version: QuarantineVersionSchema,
});

/** 重查已排入的 202 回执。 */
export const QuarantineRecheckReceiptSchema = strictStruct({
  schema_version: v.literal(2),
  operation: v.literal("recheck_quarantine"),
  disposition: v.literal("recheck_queued"),
  command_id: ConsignmentUuidSchema,
  quarantine_id: ConsignmentUuidSchema,
  work_id: ConsignmentUuidSchema,
});
/** 重查回执。 */
export type QuarantineRecheckReceipt = v.InferOutput<typeof QuarantineRecheckReceiptSchema>;

/** Admin 解除隔离。不得提供跳过证明的 force 字段。 */
export interface QuarantineReleaseRequest {
  command_id: string;
  expected_version: number;
  observation_id: string;
  proposed_kind: QuarantineDecisionKind;
}

/** 解除隔离请求的闭集线协议。 */
export const QuarantineReleaseRequestSchema = strictStruct({
  command_id: CommandUuidV4Schema,
  expected_version: QuarantineVersionSchema,
  observation_id: ConsignmentUuidSchema,
  proposed_kind: QuarantineDecisionKindSchema,
});

/** 运营止售设置或解除的 200 回执；L 与 hold ID 均为必填。 */
export const ConsignmentHoldReceiptSchema = v.pipe(
  strictStruct({
    schema_version: v.literal(2),
    operation: v.picklist(["admin_delist", "release_hold"]),
    disposition: v.picklist(["hold_set", "hold_released"]),
    command_id: ConsignmentUuidSchema,
    listing_id: ConsignmentUuidSchema,
    listing_version: ListingVersionSchema,
    state: v.literal("held_off_market"),
    hold_id: ConsignmentUuidSchema,
  }),
  v.check(
    ({ operation, disposition, state }) =>
      (operation === "admin_delist" && disposition === "hold_set" && state === "held_off_market") ||
      (operation === "release_hold" &&
        disposition === "hold_released" &&
        state === "held_off_market"),
  ),
);
/** 止售回执。 */
export type ConsignmentHoldReceipt = v.InferOutput<typeof ConsignmentHoldReceiptSchema>;

/** Admin 下架 v2 的两种合法回执：未代管件关闭，代管件设置独立止售。 */
export const ConsignmentDelistReceiptV2Schema = v.union([
  strictStruct({
    schema_version: v.literal(2),
    operation: v.literal("admin_delist"),
    disposition: v.literal("closed"),
    command_id: ConsignmentUuidSchema,
    listing_id: ConsignmentUuidSchema,
    listing_version: ListingVersionSchema,
    state: v.literal("closed"),
  }),
  strictStruct({
    schema_version: v.literal(2),
    operation: v.literal("admin_delist"),
    disposition: v.literal("hold_set"),
    command_id: ConsignmentUuidSchema,
    listing_id: ConsignmentUuidSchema,
    listing_version: ListingVersionSchema,
    state: v.literal("held_off_market"),
    hold_id: ConsignmentUuidSchema,
  }),
]);
/** Admin 下架回执。 */
export type ConsignmentDelistReceiptV2 = v.InferOutput<typeof ConsignmentDelistReceiptV2Schema>;

/** Admin 解除指定运营止售。不变量 1、6：稳定 UUIDv4、预期版本与原 hold。 */
export interface ReleaseConsignmentHoldRequest {
  command_id: string;
  expected_version: number;
  hold_id: string;
}

/** 解除止售请求的闭集线协议。 */
export const ReleaseConsignmentHoldRequestSchema = strictStruct({
  command_id: CommandUuidV4Schema,
  expected_version: ListingVersionSchema,
  hold_id: ConsignmentUuidSchema,
});
