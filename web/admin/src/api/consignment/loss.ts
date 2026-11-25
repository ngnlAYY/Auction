// Admin 损失案件：按 position 读案件，录入灭失证据、确认灭失并追加人工结果；
// 未知写只能以原命令 ID 与原正文重放。

import {
  AdminLossCaseSchema,
  ConfirmPositionLossRequestSchema,
  type LossCaseOutcomeKind,
  LossCaseOutcomeReceiptSchema,
  type LossProofReceipt,
  LossProofReceiptSchema,
  type PositionLossReceipt,
  PositionLossReceiptSchema,
  RecordLossCaseOutcomeRequestSchema,
  RecordLossProofRequestSchema,
} from "@auction/contracts/dto/admin/consignment/loss";
import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import { strictStruct } from "@auction/contracts/wire";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { type InferOutput, is, literal, string } from "valibot";

import {
  type AdminWriteFailure,
  decodeAdminReceipt,
  invalidResponseError,
  responseError,
  writeNotSent,
  writeOutcomeUnknown,
} from "../failure";
import type { AdminSessionHost } from "../session";
import { prepareJsonCommandOnce, sendGet, sendPreparedJsonWrite } from "../transport";

const POSITIONS = "/api/v2/admin/consignment/positions";

const CASE_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "generation"],
    ["data", "case_version"],
    ["data", "outcomes", null, "case_version"],
  ],
};
const RECEIPT_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [["data", "case_version"]],
};

/** 读取一个 position 的损失案件；没有案件时服务端返回 404，保持稳定协议码。 */
export async function fetchAdminLossCase(
  host: AdminSessionHost,
  positionId: string,
  signal: AbortSignal,
): Promise<Result<InferOutput<typeof AdminLossCaseSchema>, string>> {
  if (!is(ConsignmentUuidSchema, positionId)) return err("invalid_request");
  const sent = await sendGet(host, `${POSITIONS}/${positionId}/loss-case`, signal);
  if (!sent.ok) return err(sent.error);
  const response = sent.value;
  if (response.status !== 200) {
    if (response.status < 400) return err(invalidResponseError("admin loss case status"));
    return err(await responseError(response, signal, host));
  }
  const decoded = decodeAdminReceipt(
    response,
    strictStruct({ code: literal("ok"), message: string(), data: AdminLossCaseSchema }),
    CASE_POLICY,
  );
  if (!decoded.ok) return err(decoded.error);
  return decoded.value.data.position_id === positionId
    ? ok(decoded.value.data)
    : err(invalidResponseError("admin loss case target"));
}

/** 人工结果表单提交的字段；命令 ID 由会话宿主在提交时分配。 */
export interface LossOutcomeInput {
  readonly caseId: string;
  readonly expectedCaseVersion: number;
  readonly outcome: LossCaseOutcomeKind;
  readonly basisId: string;
  readonly privateNote: string;
}

type CommandOperation<T> = (
  commandId: string,
  signal: AbortSignal,
) => Promise<Result<T, AdminWriteFailure>>;

/** 追加人工结果；同一命令 ID 的重试复用同一正文，回执与请求不符按结果未知处理。 */
export function prepareAdminLossOutcome(
  host: AdminSessionHost,
  positionId: string,
  input: LossOutcomeInput,
): CommandOperation<InferOutput<typeof LossCaseOutcomeReceiptSchema>> {
  const body = (commandId: string) => ({
    command_id: commandId,
    case_id: input.caseId,
    expected_case_version: input.expectedCaseVersion,
    outcome: input.outcome,
    basis_id: input.basisId,
    private_note: input.privateNote,
  });
  const prepare = prepareJsonCommandOnce(body);
  return async (commandId, signal) => {
    if (!is(ConsignmentUuidSchema, positionId)) return err(writeNotSent("invalid_request"));
    if (!is(RecordLossCaseOutcomeRequestSchema, body(commandId))) {
      return err(writeNotSent("invalid_request"));
    }
    const prepared = prepare(commandId);
    if (!prepared.ok) return err(prepared.error);
    const sent = await sendPreparedJsonWrite(
      host,
      {
        url: `${POSITIONS}/${positionId}/loss-case/outcomes`,
        method: "POST",
        endpoint: "consignment-delist",
      },
      prepared.value,
      signal,
    );
    if (!sent.ok) return err(sent.error);
    const response = sent.value;
    const decoded = decodeAdminReceipt(
      response,
      strictStruct({
        code: literal("ok"),
        message: string(),
        data: LossCaseOutcomeReceiptSchema,
      }),
      RECEIPT_POLICY,
    );
    if (!decoded.ok) return err(writeOutcomeUnknown(decoded.error, response.status));
    const receipt = decoded.value.data;
    return response.status === 200 &&
      receipt.command_id === commandId &&
      receipt.case_id === input.caseId &&
      receipt.case_version === input.expectedCaseVersion + 1
      ? ok(receipt)
      : err(writeOutcomeUnknown(invalidResponseError("admin loss outcome"), response.status));
  };
}

const PROOF_POLICY: JsonFieldPolicy = { uniqueKeys: "all" };
const CONFIRM_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "listing_version"],
    ["data", "position_version"],
  ],
};

/** 录入灭失证据表单提交的字段；命令 ID 由会话宿主在提交时分配。 */
export interface LossProofInput {
  readonly sourceKind: string;
  readonly observationRef: string;
  readonly lastKnownRef: string;
  readonly privateNote: string;
}

/** 录入灭失证据；同一命令 ID 的重试复用同一正文，回执与请求不符按结果未知处理。 */
export function prepareAdminLossProof(
  host: AdminSessionHost,
  positionId: string,
  input: LossProofInput,
): CommandOperation<LossProofReceipt> {
  const body = (commandId: string) => ({
    command_id: commandId,
    source_kind: input.sourceKind,
    observation_ref: input.observationRef,
    last_known_ref: input.lastKnownRef,
    private_note: input.privateNote,
  });
  const prepare = prepareJsonCommandOnce(body);
  return async (commandId, signal) => {
    if (!is(ConsignmentUuidSchema, positionId)) return err(writeNotSent("invalid_request"));
    if (!is(RecordLossProofRequestSchema, body(commandId))) {
      return err(writeNotSent("invalid_request"));
    }
    const prepared = prepare(commandId);
    if (!prepared.ok) return err(prepared.error);
    const sent = await sendPreparedJsonWrite(
      host,
      {
        url: `${POSITIONS}/${positionId}/loss-proofs`,
        method: "POST",
        endpoint: "consignment-delist",
      },
      prepared.value,
      signal,
    );
    if (!sent.ok) return err(sent.error);
    const response = sent.value;
    const decoded = decodeAdminReceipt(
      response,
      strictStruct({ code: literal("ok"), message: string(), data: LossProofReceiptSchema }),
      PROOF_POLICY,
    );
    if (!decoded.ok) return err(writeOutcomeUnknown(decoded.error, response.status));
    const receipt = decoded.value.data;
    return response.status === 200 &&
      receipt.command_id === commandId &&
      receipt.position_id === positionId
      ? ok(receipt)
      : err(writeOutcomeUnknown(invalidResponseError("admin loss proof"), response.status));
  };
}

/** 确认灭失表单提交的字段：证据取自录入回执，版本取自当前读到的 position。 */
export interface ConfirmLossInput {
  readonly expectedVersion: number;
  readonly proofId: string;
  readonly reason: string;
}

/** 确认代管件灭失；同一命令 ID 的重试复用同一正文，回执与请求不符按结果未知处理。 */
export function prepareAdminConfirmLoss(
  host: AdminSessionHost,
  positionId: string,
  input: ConfirmLossInput,
): CommandOperation<PositionLossReceipt> {
  const body = (commandId: string) => ({
    command_id: commandId,
    expected_version: input.expectedVersion,
    proof_id: input.proofId,
    reason: input.reason,
  });
  const prepare = prepareJsonCommandOnce(body);
  return async (commandId, signal) => {
    if (!is(ConsignmentUuidSchema, positionId)) return err(writeNotSent("invalid_request"));
    if (!is(ConfirmPositionLossRequestSchema, body(commandId))) {
      return err(writeNotSent("invalid_request"));
    }
    const prepared = prepare(commandId);
    if (!prepared.ok) return err(prepared.error);
    const sent = await sendPreparedJsonWrite(
      host,
      {
        url: `${POSITIONS}/${positionId}/confirm-loss`,
        method: "POST",
        endpoint: "consignment-delist",
      },
      prepared.value,
      signal,
    );
    if (!sent.ok) return err(sent.error);
    const response = sent.value;
    const decoded = decodeAdminReceipt(
      response,
      strictStruct({ code: literal("ok"), message: string(), data: PositionLossReceiptSchema }),
      CONFIRM_POLICY,
    );
    if (!decoded.ok) return err(writeOutcomeUnknown(decoded.error, response.status));
    const receipt = decoded.value.data;
    return response.status === 200 &&
      receipt.command_id === commandId &&
      receipt.position_id === positionId &&
      receipt.position_version > input.expectedVersion
      ? ok(receipt)
      : err(writeOutcomeUnknown(invalidResponseError("admin confirm loss"), response.status));
  };
}
