// Admin 损失案件读模型、人工结果请求与回执的线协议。
// 案件与回执只有状态、版本、依据与私有说明：没有任何金额、付款或发行字段，
// 也没有装备 key、URL 或证据原文（不变量 6、7；决定 22）。

import * as v from "valibot";

import { integer, JSON_MAX_SAFE_INTEGER, requiredNullable, strictStruct } from "../../../wire";
import { CommandUuidV4Schema } from "../../public/consignment/command";
import {
  ConsignmentTimestampSchema,
  ConsignmentUuidSchema,
  consignmentTextSchema,
  ListingVersionSchema,
} from "../../public/consignment/scalar";
import { DELIST_REASON } from "./index";

const SHA256_HEX = v.pipe(v.string(), v.regex(/^[0-9a-f]{64}$/u));

/** 案件版本从 1 开始（版本 1 是开案记录），必须能由浏览器精确表示。 */
export const LossCaseVersionSchema = v.pipe(
  integer,
  v.minValue(1),
  v.maxValue(JSON_MAX_SAFE_INTEGER),
);

/** 案件状态；只读状态，不授予付款或发行能力。 */
export const LossCaseStateSchema = v.picklist(["opened", "reviewing", "closed"]);
/** 案件状态。 */
export type LossCaseState = v.InferOutput<typeof LossCaseStateSchema>;

/** Admin 可追加的人工结果种类。 */
export const LossCaseOutcomeKindSchema = v.picklist(["reviewing", "closed"]);
/** 人工结果种类。 */
export type LossCaseOutcomeKind = v.InferOutput<typeof LossCaseOutcomeKindSchema>;

/** 案件的一条追加记录；版本 1 是开案（无依据与说明），其后的人工结果必须带依据与说明。 */
export const AdminLossOutcomeSchema = v.pipe(
  strictStruct({
    outcome_id: ConsignmentUuidSchema,
    case_version: LossCaseVersionSchema,
    state: LossCaseStateSchema,
    basis_id: requiredNullable(ConsignmentUuidSchema),
    private_note: requiredNullable(DELIST_REASON),
    occurred_at: ConsignmentTimestampSchema,
    seller_receipt_id: ConsignmentUuidSchema,
  }),
  v.check(({ case_version, state }) => (case_version === 1) === (state === "opened")),
  v.check(
    ({ case_version, basis_id, private_note }) =>
      (case_version === 1) === (basis_id === null && private_note === null) &&
      (basis_id === null) === (private_note === null),
  ),
);
/** 案件记录。 */
export type AdminLossOutcome = v.InferOutput<typeof AdminLossOutcomeSchema>;

/** Admin 按 position 查到的损失案件；证据只给来源与摘要，不给原文，也没有任何金额字段。 */
export const AdminLossCaseSchema = v.pipe(
  strictStruct({
    position_id: ConsignmentUuidSchema,
    case_id: ConsignmentUuidSchema,
    loss_fact_id: ConsignmentUuidSchema,
    proof_id: ConsignmentUuidSchema,
    proof_source_kind: consignmentTextSchema(64),
    proof_digest: SHA256_HEX,
    listing_id: ConsignmentUuidSchema,
    seller_user_id: ConsignmentUuidSchema,
    generation: LossCaseVersionSchema,
    case_version: LossCaseVersionSchema,
    state: LossCaseStateSchema,
    outcomes: v.pipe(v.array(AdminLossOutcomeSchema), v.minLength(1)),
  }),
  // 版本连续，且案件版本与状态等于最新一条记录。
  v.check(({ outcomes }) => outcomes.every((outcome, index) => outcome.case_version === index + 1)),
  v.check(({ outcomes, case_version, state }) => {
    const latest = outcomes.at(-1);
    return latest !== undefined && latest.case_version === case_version && latest.state === state;
  }),
);
/** 损失案件。 */
export type AdminLossCase = v.InferOutput<typeof AdminLossCaseSchema>;

/** Admin 追加人工结果。不变量 1、6、7：稳定 UUIDv4、案件、预期案件版本、结果、依据与私有说明。 */
export interface RecordLossCaseOutcomeRequest {
  command_id: string;
  case_id: string;
  expected_case_version: number;
  outcome: LossCaseOutcomeKind;
  basis_id: string;
  private_note: string;
}

/** 人工结果请求的闭集线协议；没有金额、付款或发行字段，多余字段整体拒绝。 */
export const RecordLossCaseOutcomeRequestSchema = strictStruct({
  command_id: CommandUuidV4Schema,
  case_id: ConsignmentUuidSchema,
  expected_case_version: LossCaseVersionSchema,
  outcome: LossCaseOutcomeKindSchema,
  basis_id: ConsignmentUuidSchema,
  private_note: DELIST_REASON,
});

/** 人工结果的 200 回执：只有案件、版本与记录标识；人工结果的版本至少是 2。 */
export const LossCaseOutcomeReceiptSchema = v.pipe(
  strictStruct({
    schema_version: v.literal(2),
    operation: v.literal("record_loss_case_outcome"),
    disposition: v.literal("case_outcome_recorded"),
    command_id: ConsignmentUuidSchema,
    case_id: ConsignmentUuidSchema,
    case_version: LossCaseVersionSchema,
    outcome_id: ConsignmentUuidSchema,
    seller_receipt_id: ConsignmentUuidSchema,
  }),
  v.check(({ case_version }) => case_version >= 2),
);
/** 人工结果回执。 */
export type LossCaseOutcomeReceipt = v.InferOutput<typeof LossCaseOutcomeReceiptSchema>;

/** Admin 录入一条灭失证据（rulings Q4）。不变量 1、6、7：稳定 UUIDv4、来源、观察身份、最后确定位置与
 * 私有说明；三个摘要由服务端在锁内推导，不收自请求；观察身份与最后确定位置拒装备链接与 key 形状。 */
export interface RecordLossProofRequest {
  command_id: string;
  source_kind: string;
  observation_ref: string;
  last_known_ref: string;
  private_note: string;
}

const LOSS_PROOF_FIELDS = {
  source_kind: consignmentTextSchema(64),
  observation_ref: v.pipe(DELIST_REASON, v.maxBytes(128)),
  last_known_ref: DELIST_REASON,
  private_note: DELIST_REASON,
};

/** 灭失证据表单字段（不含命令 ID）；页面在分配命令前用它做写前校验。 */
export const LossProofFieldsSchema = strictStruct(LOSS_PROOF_FIELDS);

/** 灭失证据请求的闭集线协议；多余字段整体拒绝。 */
export const RecordLossProofRequestSchema = strictStruct({
  command_id: CommandUuidV4Schema,
  ...LOSS_PROOF_FIELDS,
});

/** 灭失证据的 200 回执：position、证据 ID 与摘要；不回显私有说明与证据原文。 */
export const LossProofReceiptSchema = strictStruct({
  schema_version: v.literal(2),
  operation: v.literal("record_position_loss_proof"),
  disposition: v.literal("loss_proof_recorded"),
  command_id: ConsignmentUuidSchema,
  position_id: ConsignmentUuidSchema,
  proof_id: ConsignmentUuidSchema,
  proof_digest: SHA256_HEX,
});
/** 灭失证据回执。 */
export type LossProofReceipt = v.InferOutput<typeof LossProofReceiptSchema>;

/** Admin 确认代管件灭失。不变量 1、6、7：稳定 UUIDv4、预期 position 版本、独立证据 ID 与私有说明；
 * 没有金额、退款或赔款字段。 */
export interface ConfirmPositionLossRequest {
  command_id: string;
  expected_version: number;
  proof_id: string;
  reason: string;
}

/** 灭失确认请求的闭集线协议；多余字段整体拒绝。 */
export const ConfirmPositionLossRequestSchema = strictStruct({
  command_id: CommandUuidV4Schema,
  expected_version: LossCaseVersionSchema,
  proof_id: ConsignmentUuidSchema,
  reason: DELIST_REASON,
});

/** 灭失确认的 200 回执：挂单关闭、position 版本、损失事实、案件与卖家回执；没有合同、退款或赔款字段。 */
export const PositionLossReceiptSchema = strictStruct({
  schema_version: v.literal(2),
  operation: v.literal("confirm_position_loss"),
  disposition: v.literal("position_loss_recorded"),
  command_id: ConsignmentUuidSchema,
  listing_id: ConsignmentUuidSchema,
  listing_version: ListingVersionSchema,
  state: v.literal("closed"),
  position_id: ConsignmentUuidSchema,
  position_version: LossCaseVersionSchema,
  loss_fact_id: ConsignmentUuidSchema,
  loss_case_id: ConsignmentUuidSchema,
  seller_receipt_id: ConsignmentUuidSchema,
});
/** 灭失确认回执。 */
export type PositionLossReceipt = v.InferOutput<typeof PositionLossReceiptSchema>;
