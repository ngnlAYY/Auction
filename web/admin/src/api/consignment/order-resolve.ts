// Admin 订单 recheck 与 resolve：未知写只能以原命令 ID 与原正文重放。
// 退款金额只由 6B 单写者按冻结成交额决定，请求里没有任何金额、收件人或装备字段。

import {
  type OrderResolveAction,
  RecheckOrderReceiptSchema,
  RecheckOrderRequestSchema,
  RedispatchQueuedReceiptSchema,
  ResolutionRecordedReceiptSchema,
  ResolveOrderRequestSchema,
} from "@auction/contracts/dto/admin/consignment/order-resolve";
import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import { strictStruct } from "@auction/contracts/wire";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { type InferOutput, is, literal, string } from "valibot";

import {
  type AdminWriteFailure,
  decodeAdminReceipt,
  invalidResponseError,
  writeNotSent,
  writeOutcomeUnknown,
} from "../failure";
import type { AdminSessionHost } from "../session";
import { prepareJsonCommandOnce, sendPreparedJsonWrite } from "../transport";

const ORDERS = "/api/v2/admin/consignment/orders";

const RECEIPT_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [
    ["data", "schema_version"],
    ["data", "contract_version"],
  ],
};

type CommandOperation<T> = (
  commandId: string,
  signal: AbortSignal,
) => Promise<Result<T, AdminWriteFailure>>;

/** resolve 的成功回执：处置已记录（200），或补寄已排队（202）。 */
export type ResolveReceipt =
  | { readonly status: 200; readonly receipt: InferOutput<typeof ResolutionRecordedReceiptSchema> }
  | { readonly status: 202; readonly receipt: InferOutput<typeof RedispatchQueuedReceiptSchema> };

/** 排入订单复核；同一命令 ID 的重试复用同一正文，回执与请求不符按结果未知处理。 */
export function prepareAdminOrderRecheck(
  host: AdminSessionHost,
  contractId: string,
  expectedVersion: number,
): CommandOperation<InferOutput<typeof RecheckOrderReceiptSchema>> {
  const body = (commandId: string) => ({
    command_id: commandId,
    expected_version: expectedVersion,
  });
  const prepare = prepareJsonCommandOnce(body);
  return async (commandId, signal) => {
    if (!is(ConsignmentUuidSchema, contractId) || !is(RecheckOrderRequestSchema, body(commandId))) {
      return err(writeNotSent("invalid_request"));
    }
    const prepared = prepare(commandId);
    if (!prepared.ok) return err(prepared.error);
    const sent = await sendPreparedJsonWrite(
      host,
      { url: `${ORDERS}/${contractId}/recheck`, method: "POST", endpoint: "consignment-delist" },
      prepared.value,
      signal,
    );
    if (!sent.ok) return err(sent.error);
    const response = sent.value;
    const decoded = decodeAdminReceipt(
      response,
      strictStruct({
        code: literal("accepted"),
        message: string(),
        data: RecheckOrderReceiptSchema,
      }),
      RECEIPT_POLICY,
    );
    if (!decoded.ok) return err(writeOutcomeUnknown(decoded.error, response.status));
    const receipt = decoded.value.data;
    return response.status === 202 &&
      receipt.contract_version === expectedVersion &&
      receipt.command_id === commandId &&
      receipt.contract_id === contractId
      ? ok(receipt)
      : err(writeOutcomeUnknown(invalidResponseError("admin order recheck"), response.status));
  };
}

/** 订单处置表单提交的字段；命令 ID 由会话宿主在提交时分配。 */
export interface ResolveOrderInput {
  readonly expectedVersion: number;
  readonly action: OrderResolveAction;
  readonly evidenceId: string;
}

/** 订单处置；同一命令 ID 的重试复用同一正文，回执与请求不符按结果未知处理。 */
export function prepareAdminOrderResolve(
  host: AdminSessionHost,
  contractId: string,
  input: ResolveOrderInput,
): CommandOperation<ResolveReceipt> {
  const body = (commandId: string) => ({
    command_id: commandId,
    expected_version: input.expectedVersion,
    action: input.action,
    evidence_id: input.evidenceId,
  });
  const prepare = prepareJsonCommandOnce(body);
  return async (commandId, signal) => {
    if (!is(ConsignmentUuidSchema, contractId) || !is(ResolveOrderRequestSchema, body(commandId))) {
      return err(writeNotSent("invalid_request"));
    }
    const prepared = prepare(commandId);
    if (!prepared.ok) return err(prepared.error);
    const sent = await sendPreparedJsonWrite(
      host,
      { url: `${ORDERS}/${contractId}/resolve`, method: "POST", endpoint: "consignment-delist" },
      prepared.value,
      signal,
    );
    if (!sent.ok) return err(sent.error);
    const response = sent.value;
    const unknown = (cause: string) =>
      err(writeOutcomeUnknown(invalidResponseError(cause), response.status));
    // 补寄是 202 accepted，其余处置是 200 ok；状态、处置与请求动作必须逐项一致。
    if (response.status === 202) {
      const decoded = decodeAdminReceipt(
        response,
        strictStruct({
          code: literal("accepted"),
          message: string(),
          data: RedispatchQueuedReceiptSchema,
        }),
        RECEIPT_POLICY,
      );
      if (!decoded.ok) return err(writeOutcomeUnknown(decoded.error, response.status));
      const receipt = decoded.value.data;
      return input.action === "redispatch_original" &&
        receipt.contract_version === input.expectedVersion &&
        receipt.command_id === commandId &&
        receipt.contract_id === contractId
        ? ok({ status: 202, receipt })
        : unknown("admin order redispatch");
    }
    const decoded = decodeAdminReceipt(
      response,
      strictStruct({
        code: literal("ok"),
        message: string(),
        data: ResolutionRecordedReceiptSchema,
      }),
      RECEIPT_POLICY,
    );
    if (!decoded.ok) return err(writeOutcomeUnknown(decoded.error, response.status));
    const receipt = decoded.value.data;
    const nextVersion =
      input.expectedVersion < Number.MAX_SAFE_INTEGER ? input.expectedVersion + 1 : null;
    return response.status === 200 &&
      receipt.contract_version === nextVersion &&
      receipt.command_id === commandId &&
      receipt.contract_id === contractId &&
      receipt.action === input.action
      ? ok({ status: 200, receipt })
      : unknown("admin order resolve");
  };
}
