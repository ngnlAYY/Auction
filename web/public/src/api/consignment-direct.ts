// 免登录非寄售登记与凭命令 ID 查询首次决定；不带会话身份头，Origin 与来源限流由服务端把关。

import {
  type CommandReceipt,
  CommandReceiptSchema,
  type DirectCommandResult,
  DirectCommandResultSchema,
  type RegisterDirectListing,
  RegisterDirectListingSchema,
} from "@auction/contracts/dto/public/consignment";
import { decode } from "@auction/contracts/wire";
import { transportErrorCode } from "@auction/web-shared/net/transport";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";
import type { GenericSchema } from "valibot";

import { sendRequest } from "./clock";
import { COMMAND_NOT_SENT } from "./commands";
import {
  type ConsignmentPrivateError,
  decodePrivateResponse,
  validConsignmentUuid,
} from "./consignment-private";
import { consignmentReceiptMatches, consignmentWriteRejected } from "./consignment-write";

const DIRECT_LISTINGS = "/api/v1/consignment/direct-listings";
const DIRECT_COMMANDS = "/api/v1/consignment/direct-commands";
const RECEIPT_INTEGERS = [
  ["data", "listing_version"],
  ["data", "price"],
] as const;
const RESULT_INTEGERS = [
  ["data", "result", "http_status"],
  ["data", "result", "receipt", "listing_version"],
  ["data", "result", "receipt", "price"],
  ["data", "listing", "version"],
] as const;

type Request = { readonly method: "GET" } | { readonly method: "POST"; readonly body: object };

async function requestDirect<T>(
  url: string,
  request: Request,
  status: 200 | 202,
  schema: GenericSchema<unknown, T>,
  signal: AbortSignal,
  integerPaths: NonNullable<JsonFieldPolicy["integerPaths"]>,
): Promise<Result<T, ConsignmentPrivateError>> {
  let dispatched = false;
  const sent = await sendRequest((requestSignal) => {
    if (requestSignal.aborted) return err({ kind: "network", status: null });
    dispatched = true;
    const headers = { Accept: "application/json" };
    return ok({
      url,
      init: {
        method: request.method,
        credentials: "same-origin",
        cache: "no-store",
        redirect: "error",
        ...(request.method === "GET"
          ? { headers }
          : {
              headers: { ...headers, "content-type": "application/json" },
              body: JSON.stringify(request.body),
            }),
      },
    });
  }, signal);
  if (!dispatched) return err(COMMAND_NOT_SENT);
  if (signal.aborted) return err({ status: 0, code: "network_error" });
  return sent.ok
    ? decodePrivateResponse(sent.value, status, request.method, schema, integerPaths)
    : err({
        status: sent.error.kind === "network" ? (sent.error.status ?? 0) : 0,
        code: transportErrorCode(sent.error),
      });
}

/** 不变量 1、6：首次提交只有本地未发送、Origin 拒绝或登记路由认可的可信拒绝才算结案。 */
export function directWriteRejected(error: ConsignmentPrivateError): boolean {
  return (
    (error.status === 403 && error.code === "origin_forbidden") ||
    consignmentWriteRejected("register", error)
  );
}

/** 不变量 1、6、7：免登录登记携带固定命令 ID，链接只进入这一次的请求正文。 */
export async function registerDirectListing(
  command: RegisterDirectListing,
  signal: AbortSignal,
): Promise<Result<CommandReceipt, ConsignmentPrivateError>> {
  const decoded = decode(RegisterDirectListingSchema, command);
  if (!decoded.ok) {
    return err(
      validConsignmentUuid(command.command_id)
        ? { status: 400, code: "invalid_listing_request" }
        : { status: 400, code: "invalid_command_id" },
    );
  }
  if (utf8ByteLength(decoded.value.equipment_url) > 2048) {
    return err({ status: 400, code: "invalid_equipment_url" });
  }
  const result = await requestDirect(
    DIRECT_LISTINGS,
    { method: "POST", body: decoded.value },
    202,
    CommandReceiptSchema,
    signal,
    RECEIPT_INTEGERS,
  );
  return result.ok &&
    !consignmentReceiptMatches(result.value, command.command_id, {
      operation: "register",
      price: command.price,
    })
    ? err({ status: 202, code: "invalid_response" })
    : result;
}

/** 按原命令 ID 读首次决定；回执必须属于所查命令的登记。 */
export async function fetchDirectCommand(
  commandId: string,
  signal: AbortSignal,
): Promise<Result<DirectCommandResult, ConsignmentPrivateError>> {
  if (!validConsignmentUuid(commandId)) return err({ status: 400, code: "invalid_command_id" });
  const result = await requestDirect(
    `${DIRECT_COMMANDS}/${commandId}`,
    { method: "GET" },
    200,
    DirectCommandResultSchema,
    signal,
    RESULT_INTEGERS,
  );
  return result.ok &&
    (result.value.result.receipt.command_id !== commandId ||
      result.value.result.receipt.operation !== "register")
    ? err({ status: 200, code: "invalid_response" })
    : result;
}
