// 运营挂单读取共用的成功信封、原始整数词法与同源传输。

import { strictStruct } from "@auction/contracts/wire";
import { type JsonFieldPolicy, jsonExactObject } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { type GenericSchema, type InferOutput, literal, string } from "valibot";

import { decodeAdminReceipt, invalidResponseError, responseError } from "../failure";
import type { AdminSessionHost } from "../session";
import { sendGet } from "../transport";

/** Admin 会话下的挂单集合端点。 */
export const LISTINGS_URL = "/api/v1/admin/consignment/listings";

const INTEGER_FIELDS = [
  ["version"],
  ["price"],
  ["pricing_version"],
  ["fee_bps"],
  ["estimated_fee"],
  ["estimated_seller_amount"],
  ["view", "progress", "attempts"],
  ["view", "progress", "max_attempts"],
  ["view", "equipment", "level", "value"],
] as const;

/** 只限制具型整数，快照中的属性仍可用小数。 */
export function listingIntegerPaths(
  prefix: readonly (string | null)[],
): NonNullable<JsonFieldPolicy["integerPaths"]> {
  return INTEGER_FIELDS.map((field) => [...prefix, ...field]);
}

/** 只接受 200 的严格成功信封，认证与错误回执沿用 Admin 外壳。 */
export async function readListingResponse<Schema extends GenericSchema>(
  host: AdminSessionHost,
  url: string,
  schema: Schema,
  policy: JsonFieldPolicy,
  signal: AbortSignal,
): Promise<Result<InferOutput<Schema>, string>> {
  const sent = await sendGet(host, url, signal);
  if (!sent.ok) return sent;
  const response = sent.value;
  if (response.status !== 200) {
    if (response.status >= 200 && response.status < 300) {
      return err(invalidResponseError("consignment HTTP status"));
    }
    const code = await responseError(response, signal, host);
    const status = listingErrorStatus(code);
    if (status !== null) {
      // Admin 的共享守卫允许带证据的 error data；寄售业务错误只允许 null。
      const parsed = response.json({ uniqueKeys: "all" });
      const envelope = parsed.ok
        ? jsonExactObject(parsed.value, ["code", "message", "data", "request_id"])
        : null;
      if (response.status !== status || envelope?.data !== null) {
        return err(invalidResponseError("consignment HTTP error"));
      }
    }
    return err(code);
  }
  const decoded = decodeAdminReceipt(
    response,
    strictStruct({ code: literal("ok"), message: string(), data: schema }),
    policy,
  );
  return decoded.ok ? ok(decoded.value.data) : decoded;
}

function listingErrorStatus(code: string): number | null {
  switch (code) {
    case "listing_not_found":
      return 404;
    case "invalid_query":
      return 400;
    case "consignment_unavailable":
      return 503;
    default:
      return null;
  }
}
