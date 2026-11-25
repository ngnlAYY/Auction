// 验证 Admin 损失案件线协议：请求与回执没有金额、付款或发行字段，版本与记录自洽。

import { expect, test } from "bun:test";

import { decode } from "../../../wire";
import {
  AdminLossCaseSchema,
  ConfirmPositionLossRequestSchema,
  LossCaseOutcomeReceiptSchema,
  PositionLossReceiptSchema,
  RecordLossCaseOutcomeRequestSchema,
  RecordLossProofRequestSchema,
} from "./loss";

const COMMAND = "00000000-0000-4000-8000-000000000001";
const CASE = "00000000-0000-4000-8000-000000000002";
const BASIS = "00000000-0000-4000-8000-000000000003";

const request = {
  command_id: COMMAND,
  case_id: CASE,
  expected_case_version: 1,
  outcome: "reviewing",
  basis_id: BASIS,
  private_note: "运营夹具：复核中",
};

const receipt = {
  schema_version: 2,
  operation: "record_loss_case_outcome",
  disposition: "case_outcome_recorded",
  command_id: COMMAND,
  case_id: CASE,
  case_version: 2,
  outcome_id: "00000000-0000-4000-8000-000000000004",
  seller_receipt_id: "00000000-0000-4000-8000-000000000005",
};

const opened = {
  outcome_id: "00000000-0000-4000-8000-000000000004",
  case_version: 1,
  state: "opened",
  basis_id: null,
  private_note: null,
  occurred_at: "2026-11-21T00:00:00Z",
  seller_receipt_id: "00000000-0000-4000-8000-000000000005",
};

const lossCase = {
  position_id: COMMAND,
  case_id: CASE,
  loss_fact_id: "00000000-0000-4000-8000-000000000006",
  proof_id: "00000000-0000-4000-8000-000000000007",
  proof_source_kind: "fake-source",
  proof_digest: "a".repeat(64),
  listing_id: "00000000-0000-4000-8000-000000000008",
  seller_user_id: "00000000-0000-4000-8000-000000000009",
  generation: 1,
  case_version: 1,
  state: "opened",
  outcomes: [opened],
};

test("人工结果请求与回执拒绝金额、付款与发行字段", () => {
  expect(decode(RecordLossCaseOutcomeRequestSchema, request).ok).toBe(true);
  expect(decode(LossCaseOutcomeReceiptSchema, receipt).ok).toBe(true);
  for (const extra of ["amount", "refund", "payout", "credits"]) {
    expect(decode(RecordLossCaseOutcomeRequestSchema, { ...request, [extra]: 1 }).ok).toBe(false);
    expect(decode(LossCaseOutcomeReceiptSchema, { ...receipt, [extra]: 1 }).ok).toBe(false);
    expect(decode(AdminLossCaseSchema, { ...lossCase, [extra]: 1 }).ok).toBe(false);
  }
  expect(
    decode(RecordLossCaseOutcomeRequestSchema, {
      ...request,
      command_id: "00000000-0000-1000-8000-000000000001",
    }).ok,
  ).toBe(false);
});

test("案件版本必须连续且与最新记录一致，人工结果回执版本至少为 2", () => {
  expect(decode(AdminLossCaseSchema, lossCase).ok).toBe(true);
  expect(decode(AdminLossCaseSchema, { ...lossCase, case_version: 2 }).ok).toBe(false);
  expect(decode(AdminLossCaseSchema, { ...lossCase, state: "closed" }).ok).toBe(false);
  expect(decode(LossCaseOutcomeReceiptSchema, { ...receipt, case_version: 1 }).ok).toBe(false);
});

test("灭失证据请求拒绝装备链接与 key 形状；灭失确认请求与回执拒绝金额字段", () => {
  const proof = {
    command_id: COMMAND,
    source_kind: "fake-source",
    observation_ref: "fake-observation-1",
    last_known_ref: "fake bot inventory slot",
    private_note: "运营夹具：灭失证据",
  };
  expect(decode(RecordLossProofRequestSchema, proof).ok).toBe(true);
  for (const leaked of ["https://example.invalid/equip/1/abcdef0123", "key=fake"]) {
    expect(decode(RecordLossProofRequestSchema, { ...proof, observation_ref: leaked }).ok).toBe(
      false,
    );
    expect(decode(RecordLossProofRequestSchema, { ...proof, last_known_ref: leaked }).ok).toBe(
      false,
    );
  }
  const confirm = {
    command_id: COMMAND,
    expected_version: 2,
    proof_id: "00000000-0000-4000-8000-000000000007",
    reason: "运营夹具：确认灭失",
  };
  const recorded = {
    schema_version: 2,
    operation: "confirm_position_loss",
    disposition: "position_loss_recorded",
    command_id: COMMAND,
    listing_id: "00000000-0000-4000-8000-000000000008",
    listing_version: 4,
    state: "closed",
    position_id: "00000000-0000-4000-8000-000000000010",
    position_version: 3,
    loss_fact_id: "00000000-0000-4000-8000-000000000006",
    loss_case_id: CASE,
    seller_receipt_id: "00000000-0000-4000-8000-000000000005",
  };
  expect(decode(ConfirmPositionLossRequestSchema, confirm).ok).toBe(true);
  expect(decode(PositionLossReceiptSchema, recorded).ok).toBe(true);
  for (const extra of ["amount", "refund", "compensation"]) {
    expect(decode(ConfirmPositionLossRequestSchema, { ...confirm, [extra]: 1 }).ok).toBe(false);
    expect(decode(PositionLossReceiptSchema, { ...recorded, [extra]: 1 }).ok).toBe(false);
  }
});
