// 寄售只读请求的无缓存传输与安全结果分类，以及全站服务能力请求。

import {
  type ConsignmentStatus,
  ConsignmentStatusSchema,
} from "@auction/contracts/dto/public/consignment/status";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode, requiredNullable, struct } from "@auction/contracts/wire";
import { sendRequestObserved, type TransportError } from "@auction/web-shared/net/transport";
import { type JsonFieldPolicy, parseJson } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { reportBrowserError } from "@auction/web-shared/ui/bootstrap";
import { type GenericSchema, null_, string } from "valibot";

const UTF8 = new TextDecoder("utf-8", { fatal: true });
const ERROR_ENVELOPE = struct({
  code: string(),
  message: string(),
  data: null_(),
  request_id: requiredNullable(string()),
});

function parseConsignmentBody(body: Uint8Array | null, policy: JsonFieldPolicy) {
  if (body === null) return err("invalid_json");
  let source: string;
  try {
    source = UTF8.decode(body);
  } catch {
    return err("invalid_json");
  }
  return parseJson(source, policy);
}

/** 公开读取只保留已确认的分类，不向页面传递响应正文或未知错误码。 */
export type ConsignmentReadError =
  | {
      readonly kind:
        | "disabled"
        | "not-found"
        | "invalid-query"
        | "unavailable"
        | "invalid-response";
    }
  | { readonly kind: "transport"; readonly error: TransportError };

/** 解码契约 DTO；整数路径含信封的 data 层，匿名读省略凭据并禁用缓存。 */
export async function readPublicConsignment<T>(
  url: string,
  schema: GenericSchema<unknown, T>,
  signal: AbortSignal,
  integerPaths: NonNullable<JsonFieldPolicy["integerPaths"]> = [],
): Promise<Result<T, ConsignmentReadError>> {
  let unavailable = false;
  const sent = await sendRequestObserved(
    () =>
      ok({
        url,
        init: {
          method: "GET",
          headers: { Accept: "application/json" },
          cache: "no-store",
          credentials: "omit",
          redirect: "error",
        },
      }),
    (response) => {
      unavailable = response.status === 503;
    },
    signal,
  );
  if (signal.aborted) return err({ kind: "transport", error: { kind: "network", status: null } });
  if (unavailable) return err({ kind: "unavailable" });
  if (!sent.ok) return err({ kind: "transport", error: sent.error });

  const response = sent.value;

  const payload = parseConsignmentBody(response.body, { uniqueKeys: "all", integerPaths });
  if (response.status === 200) {
    if (payload.ok) {
      const decoded = decode(apiResponseSchema(schema), payload.value);
      if (decoded.ok && decoded.value.code === "ok") return ok(decoded.value.data);
    }
    // 诊断只带既有的错误分类，不带请求查询或响应数据。
    reportBrowserError("dto_decode");
    return err({ kind: "invalid-response" });
  }

  if (!payload.ok) return err({ kind: "invalid-response" });
  const decoded = decode(ERROR_ENVELOPE, payload.value);
  if (decoded.ok) {
    const code = decoded.value.code;
    if (response.status === 404 && code === "consignment_disabled")
      return err({ kind: "disabled" });
    if (response.status === 404 && code === "listing_not_found") return err({ kind: "not-found" });
    if (response.status === 400 && code === "invalid_query") return err({ kind: "invalid-query" });
  }
  return err({ kind: "invalid-response" });
}

/** 匿名查询寄售能力；关闭仍由服务端返回 200 与 enabled=false。 */
export function fetchConsignmentStatus(
  signal: AbortSignal,
): Promise<Result<ConsignmentStatus, ConsignmentReadError>> {
  return readPublicConsignment("/api/v1/consignment/status", ConsignmentStatusSchema, signal, [
    ["data", "fee_bps"],
    ["data", "min_price"],
    ["data", "max_price"],
  ]);
}
