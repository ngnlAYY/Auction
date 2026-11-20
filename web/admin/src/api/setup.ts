// 一次性本机初始化接口；不会复用常规 Admin 会话或 CSRF API。

import type { SetupRequest, SetupResponse } from "@auction/contracts/setup";
import { SetupErrorResponseSchema, SetupResponseSchema } from "@auction/contracts/setup";
import { decode } from "@auction/contracts/wire";
import type { TransportRequest, TransportResponse } from "@auction/web-shared/net/transport";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";

import { sendRequest } from "./transport";

// Setup 回执只含字符串与布尔值；用它判定成功、拒绝或结果未知，必须按严格词法解析。
// 重复键与浮点/指数数字一律拒绝，已知字段与扩展字段的字符串都由解析器校验 Unicode。
// 位置数组、未知字段中的超大数等生产环境不会返回的输入按裁定 G3 不处理，不为它们加兼容。
const SETUP_JSON_POLICY: JsonFieldPolicy = { uniqueKeys: "all", numbers: "integers" };

/** 合并部署确认目标；与 `DatabaseTarget` 同形，但接口层不依赖页面草稿模块。 */
interface ConfirmedTarget {
  readonly host: string;
  readonly port: number;
  readonly name: string;
}

/** 配置表单开放前先独立验证部署码；不发送配置或使用普通 Admin 会话。 */
export async function verifyFirstRun(
  token: string,
  signal: AbortSignal,
): Promise<Result<boolean, string>> {
  const response = await sendRequest(
    (requestSignal) =>
      setupRequest(requestSignal, "/api/v1/setup/verify", {
        method: "POST",
        headers: { "x-setup-token": token },
      }),
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const body = setupResponse(response.value, "verified");
  return body.ok ? ok(body.value.initialize_and_start) : err(body.error);
}

/** 令牌仅在本次请求 header 发送，成功响应只消费稳定阶段值。 */
export async function submitFirstRun(
  token: string,
  form: SetupRequest,
  complete: boolean,
  confirmedTarget: ConfirmedTarget | null,
  signal: AbortSignal,
): Promise<Result<void, string>> {
  const path = complete ? "/api/v1/setup/complete" : "/api/v1/setup/validate";
  const confirmation =
    confirmedTarget === null
      ? null
      : encodeTarget(confirmedTarget.host, confirmedTarget.port, confirmedTarget.name);
  const response = await sendRequest((requestSignal) => {
    const headers: Record<string, string> = { "x-setup-token": token };
    if (confirmation !== null) {
      headers["x-setup-confirm-target"] = confirmation;
    }
    let body: string | undefined;
    try {
      body = JSON.stringify(form);
    } catch {
      return err("invalid_input");
    }
    if (body === undefined) {
      return err("invalid_input");
    }
    return setupRequest(requestSignal, path, {
      method: "POST",
      headers: { ...headers, "content-type": "application/json" },
      body,
    });
  }, signal);
  if (!response.ok) {
    return err(mapSubmissionFailure(complete, response.error));
  }
  const result = setupResponse(response.value, complete ? "completed" : "validated");
  if (!result.ok) {
    return err(mapSubmissionFailure(complete, result.error));
  }
  return ok(undefined);
}

function encodeTarget(host: string, port: number, name: string): string {
  const target = new URLSearchParams();
  target.append("host", host);
  target.append("port", String(port));
  target.append("name", name);
  return target.toString();
}

/** 超时或响应丢失不代表服务端未发布目录；保存动作不能提示操作者直接重试。 */
function mapSubmissionFailure(complete: boolean, code: string): string {
  if (
    complete &&
    (code === "network_error" ||
      code === "request_timeout" ||
      code === "invalid_response" ||
      code === "setup_unknown")
  ) {
    return "setup_outcome_unknown";
  }
  return code;
}

function setupResponse(
  response: TransportResponse,
  expected: string,
): Result<SetupResponse, string> {
  const parsed = response.json(SETUP_JSON_POLICY);
  if (!responseOk(response)) {
    if (!parsed.ok) {
      return err("setup_unknown");
    }
    const body = decodeSetupErrorResponse(parsed.value);
    return err(body === null ? "setup_unknown" : body.code);
  }
  if (!parsed.ok) {
    return err("setup_unknown");
  }
  const body = decodeSetupResponse(parsed.value);
  if (body === null) {
    return err("setup_unknown");
  }
  return body.status === expected ? ok(body) : err("setup_unknown");
}

/** 与 gloo 请求构建同一边界：本机构造失败按 `invalid_input` 处理并保留未发送标记。 */
function setupRequest(
  signal: AbortSignal,
  url: string,
  init: RequestInit,
): Result<TransportRequest, string> {
  try {
    const request = new Request(url, { ...init, signal });
    return ok({ url, init: { ...init, headers: request.headers } });
  } catch {
    return err("invalid_input");
  }
}

function decodeSetupResponse(value: unknown): SetupResponse | null {
  // `struct` 只接受对象并忽略未知键；位置数组按裁定 G3 不处理。
  const body = decode(SetupResponseSchema, value);
  return body.ok ? body.value : null;
}

function decodeSetupErrorResponse(value: unknown) {
  // 拒绝回执同样是具名对象；缺字段与数组由共享 schema 拒绝，未知键按 Serde 跳过。
  const body = decode(SetupErrorResponseSchema, value);
  return body.ok ? body.value : null;
}

/** gloo 的 `Response::ok`：状态码 200 到 299。 */
function responseOk(response: TransportResponse): boolean {
  return response.status >= 200 && response.status <= 299;
}
