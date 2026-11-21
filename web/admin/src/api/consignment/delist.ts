// 运营下架使用 Admin 写门禁，固定命令正文并核对成功回执。

import {
  type AdminDelistListing,
  AdminDelistListingSchema,
} from "@auction/contracts/dto/admin/consignment";
import {
  type CommandReceipt,
  CommandReceiptSchema,
} from "@auction/contracts/dto/public/consignment";
import { ConsignmentUuidSchema } from "@auction/contracts/dto/public/consignment/scalar";
import { strictStruct } from "@auction/contracts/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { is, literal, string } from "valibot";

import {
  type AdminWriteFailure,
  decodeAdminReceipt,
  invalidResponseError,
  writeNotSent,
  writeOutcomeUnknown,
} from "../failure";
import type { AdminSessionHost } from "../session";
import { trimRustWhitespace } from "../session-time";
import {
  prepareJsonCommandOnce,
  readVerifiedWriteReceipt,
  sendPreparedJsonWrite,
} from "../transport";
import { LISTINGS_URL } from "./read";

const RECEIPT_SCHEMA = strictStruct({
  code: literal("ok"),
  message: string(),
  data: CommandReceiptSchema,
});

const RECEIPT_POLICY = { uniqueKeys: "all", numbers: "integers" } as const;

/** 采用下架请求契约判断输入，空白表单单独显示必填提示。 */
export function delistReasonError(reason: string): "required" | "invalid" | null {
  if (trimRustWhitespace(reason).length === 0) return "required";
  // 请求 schema 同时校验 UUID 与版本；这两个固定样例只隔离待验证的原因。
  return is(AdminDelistListingSchema, {
    command_id: "00000000-0000-4000-8000-000000000001",
    expected_version: 1,
    reason,
  })
    ? null
    : "invalid";
}

/**
 * 在确认时固定目标、版本及私有原因。不变量 1、6、7：同一命令只重试同一正文，
 * 无法核实目标或版本的回执保留未决，说明不进入诊断信息。
 */
export function prepareAdminDelist(
  host: AdminSessionHost,
  listingId: string,
  expectedVersion: number,
  reason: string,
): (commandId: string, signal: AbortSignal) => Promise<Result<CommandReceipt, AdminWriteFailure>> {
  const reasonValid = delistReasonError(reason) === null;
  const body = (commandId: string): AdminDelistListing => ({
    command_id: commandId,
    expected_version: expectedVersion,
    reason,
  });
  const prepare = prepareJsonCommandOnce(body);
  return async (commandId, signal) => {
    if (!is(ConsignmentUuidSchema, listingId)) return err(writeNotSent("invalid_listing_id"));
    if (!reasonValid) return err(writeNotSent("invalid_reason"));
    const request = body(commandId);
    if (!is(AdminDelistListingSchema, request)) return err(writeNotSent("invalid_listing_request"));
    const prepared = prepare(commandId);
    if (!prepared.ok) return prepared;
    const sent = await sendPreparedJsonWrite(
      host,
      {
        url: `${LISTINGS_URL}/${listingId}/delist`,
        method: "POST",
        endpoint: "consignment-delist",
      },
      prepared.value,
      signal,
    );
    if (!sent.ok) {
      const failure = sent.error;
      if (failure.kind === "verified-http") {
        const evidence = readVerifiedWriteReceipt(failure.receipt);
        if (evidence?.status === 503 && evidence.protocolCode === "outcome_unknown") {
          return err(writeOutcomeUnknown("outcome_unknown", 503));
        }
      }
      return sent;
    }
    const response = sent.value;
    const decoded = decodeAdminReceipt(response, RECEIPT_SCHEMA, RECEIPT_POLICY);
    if (!decoded.ok) return err(writeOutcomeUnknown(decoded.error, response.status));
    const receipt = decoded.value.data;
    if (
      response.status !== 200 ||
      receipt.command_id !== commandId ||
      receipt.listing_id !== listingId ||
      receipt.operation !== "admin_delist" ||
      receipt.disposition !== "closed" ||
      receipt.listing_version !== expectedVersion + 1
    ) {
      return err(
        writeOutcomeUnknown(invalidResponseError("consignment delist receipt"), response.status),
      );
    }
    return ok(receipt);
  };
}
