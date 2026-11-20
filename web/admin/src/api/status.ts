// Admin 就绪探针。
// 探针只用于提示当前服务能力；解码失败与网络错误都折回稳定 code，不把服务端正文带到页面。

import { decode, strictStruct } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import * as v from "valibot";

import { invalidResponseError } from "./failure";
import type { AdminSessionHost } from "./session";
import { sendGet } from "./transport";

/** 页面展示的就绪状态；`degraded` 同时覆盖未就绪与探针失败。 */
export type AdminReadinessState = "checking" | "ready" | "degraded";

/** 单次探针的三态判定，避免把 503 与坏响应混为未就绪。 */
type AdminHealthProbeState = "healthy" | "not_ready" | "unavailable";

// readiness 是固定探针契约，两级对象都使用闭集字段。
const ReadinessResponseSchema = strictStruct({
  code: v.string(),
  message: v.string(),
  data: strictStruct({ ready: v.boolean() }),
});

/** 读取 `/health/ready`；只有 HTTP 成功且 `data.ready=true` 才算健康。 */
export async function fetchAdminReadiness(
  host: AdminSessionHost,
  signal: AbortSignal,
): Promise<Result<boolean, string>> {
  const probe = await fetchReady(host, signal);
  if (!probe.ok) {
    return err(probe.error);
  }
  return ok(probe.value === "healthy");
}

/** 一次性探针归类；HTTP 成功与 `ready` 的组合决定健康、未就绪或不可用。 */
async function fetchReady(
  host: AdminSessionHost,
  signal: AbortSignal,
): Promise<Result<AdminHealthProbeState, string>> {
  const response = await sendGet(host, "/health/ready", signal);
  if (!response.ok) {
    return err(response.error);
  }
  const received = response.value;
  const parsed = parseJson(received.text(), { uniqueKeys: "all" });
  if (!parsed.ok) {
    return err(invalidResponseError(parsed.error));
  }
  const payload = decode(ReadinessResponseSchema, parsed.value);
  if (!payload.ok) {
    return err(invalidResponseError(payload));
  }
  const ready = payload.value.data.ready;
  const httpOk = received.status >= 200 && received.status <= 299;
  const state: AdminHealthProbeState =
    httpOk && ready ? "healthy" : !httpOk && !ready ? "not_ready" : "unavailable";
  return ok(state);
}
