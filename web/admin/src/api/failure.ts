// Admin 同源错误 envelope 的解码与稳定失败码投影。
// 服务端正文的自然语言不进入界面；页面只消费稳定 `code`，再经 Fluent 映射。HTTP 状态
// 单独保留，供 404、409 等需改变交互流程的页面判断。

import { decode } from "@auction/contracts/wire";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import { type JsonFieldPolicy, jsonExactObject, parseJson } from "@auction/web-shared/net/wire";
import { err, ok, type Result, unreachable } from "@auction/web-shared/result";
import { reportBrowserError } from "@auction/web-shared/ui/bootstrap";
import type * as v from "valibot";

import {
  type AdminWriteEndpoint,
  type Rejection,
  rejectionScope,
  rejectionStatus,
} from "./rejection";
import {
  type AdminSessionHost,
  adminSessionHostLive,
  reportAdminSessionInvalidation,
} from "./session";
import { readVerifiedWriteReceipt, type VerifiedWriteReceipt } from "./transport";
import { serdeJsonNumbersFitF64 } from "./validation";

// 仅浏览器构造，供仍使用 String 的调用方区分「业务请求未发送」和发送后的传输故障。
const NOT_SENT_PREFIX = "not_sent:";
// 旧显示投影仍识别端点标记，服务端不能提供此类内部标记。
const WRITE_ENDPOINT_PREFIX = "write_endpoint:";

export type { AdminWriteEndpoint } from "./rejection";

function adminWriteEndpointParse(label: string): AdminWriteEndpoint | null {
  switch (label) {
    case "user-mutation":
    case "rename-revert":
    case "rules-publish":
    case "about-publish":
    case "mail-template":
      return label;
    default:
      return null;
  }
}

/** Admin 浏览器请求的稳定失败投影。 */
export interface AdminApiFailure {
  readonly status: number | null;
  readonly code: string;
}

/** 本次业务请求在交给传输前失败。 */
export interface AdminWriteNotSent {
  readonly kind: "not-sent";
  readonly code: string;
}

/** 已交给传输或无法证明未发送，且尚无足以裁定结果的回执。 */
export interface AdminWriteOutcomeUnknown {
  readonly kind: "outcome-unknown";
  readonly code: string;
  readonly responseStatus: number | null;
}

/** 宿主裁定命令时可用的失败证据。 */
export type AdminWriteFailure =
  | AdminWriteNotSent
  | AdminWriteOutcomeUnknown
  | { readonly kind: "verified-http"; readonly receipt: VerifiedWriteReceipt };

/** 准备阶段只携带可展示的稳定码。 */
export function writeNotSent(code: string): AdminWriteNotSent {
  return { kind: "not-sent", code };
}

/** 结果未知时保留观测到的完整响应状态，供页面决定是否重读。 */
export function writeOutcomeUnknown(
  code: string,
  responseStatus: number | null = null,
): AdminWriteOutcomeUnknown {
  return { kind: "outcome-unknown", code, responseStatus };
}

/** 只用已验证的业务写回执判断能否了结同一个命令 ID 较早的未知尝试。 */
export function classifyAdminWriteFailure(failure: AdminWriteFailure): Rejection | "unknown" {
  switch (failure.kind) {
    case "not-sent":
      return "this_attempt";
    case "outcome-unknown":
      return "unknown";
    case "verified-http": {
      const evidence = readVerifiedWriteReceipt(failure.receipt);
      if (evidence === null) {
        return "unknown";
      }
      const { endpoint, status, protocolCode } = evidence;
      return rejectionStatus(endpoint, protocolCode) === status
        ? rejectionScope(endpoint, protocolCode)
        : "unknown";
    }
    default:
      return unreachable(failure);
  }
}

/** 将写失败投影为界面原有的稳定码。 */
export function adminWriteFailureDisplayCode(failure: AdminWriteFailure): string {
  if (failure.kind === "verified-http") {
    const evidence = readVerifiedWriteReceipt(failure.receipt);
    return evidence === null
      ? "invalid_response"
      : normalizeHttpCode(evidence.endpoint, evidence.status, evidence.protocolCode);
  }
  return failure.code;
}

/** 仅返回本次实际业务写完整响应的状态。 */
export function observedResponseStatus(failure: AdminWriteFailure): number | null {
  switch (failure.kind) {
    case "not-sent":
      return null;
    case "outcome-unknown":
      return failure.responseStatus;
    case "verified-http":
      return readVerifiedWriteReceipt(failure.receipt)?.status ?? null;
    default:
      return unreachable(failure);
  }
}

/** 只有契约规定的错误信封能证明命令被拒绝，可空字段也须显式出现。 */
interface AdminApiErrorResponse {
  readonly code: string;
  readonly message: string;
  readonly data: Readonly<Record<string, unknown>> | null;
  readonly request_id: string | null;
}

/** HTTP 错误解码结果，只保留状态和通过信封校验的协议码。 */
export type DecodedAdminHttpFailure =
  | { readonly kind: "valid-envelope"; readonly status: number; readonly protocolCode: string }
  | { readonly kind: "invalid-envelope"; readonly status: number };

/** 业务请求发出前的客户端失败：没有 HTTP 状态。 */
export function adminApiFailureClient(code: string): AdminApiFailure {
  return { status: null, code };
}

/** 解码失败回执。无法读出契约错误码时，只有 HTTP 状态可用于展示。 */
export function adminApiFailureFromHttpBody(status: number, body: string): AdminApiFailure {
  return projectHttpFailure(decodeHttpFailure(status, body));
}

function decodeHttpFailure(status: number, body: string): DecodedAdminHttpFailure {
  // Serde 也能按字段顺序解码结构体，错误回执只接受具名的 JSON 对象。
  if (!trimAsciiStart(body).startsWith("{")) {
    return invalidHttpBody(status);
  }
  // serde_json 在 f64 溢出时拒绝整个回执；共享 parseJson 只检查 JS Number 是否有限，
  // 这里按原文重跑 serde 的默认数值路径，避免溢出数值仍能解除未知命令。
  if (!serdeJsonNumbersFitF64(body)) {
    return invalidHttpBody(status);
  }
  // 任意层的重复键、缺失字段、未知字段与无法放回 JSON 的值都由这一步拒绝。
  // 错误 `data` 里的具型版本字段都按 serde 的整数 visitor 读取：`2.0`、`2e0` 与 `-0`
  // 不能解除既有未决命令。该信封没有浮点字段，因此整份文档都可按整数词法收口。
  const parsed = parseJson(body, {
    uniqueKeys: "all",
    numbers: "integers",
  });
  if (!parsed.ok) {
    return invalidHttpBody(status);
  }
  const envelope = decodeAdminApiErrorResponse(parsed.value);
  if (envelope === null) {
    return invalidHttpBody(status);
  }
  return { kind: "valid-envelope", status, protocolCode: envelope.code };
}

/** 是否冲突（HTTP 409）。 */
export function adminApiFailureIsConflict(failure: AdminApiFailure): boolean {
  return failure.status === 409;
}

/** 未知结果只能重试同一命令 ID 与原负载。错误状态本身不能证明本次未写入。 */
export function adminApiFailureOutcomeUnknown(failure: AdminApiFailure): boolean {
  return rejection(failure.code, failure.status) === null;
}

/** 非命令接口仍使用原有的发送前失败投影。 */
export function adminApiFailureNotSentCode(code: string): string {
  // 准备请求的同名错误不能裁定写命令此前的尝试。
  const display = adminApiFailureDisplayCode(code);
  if (rejection(display, null) === "this_attempt") {
    return display;
  }
  return `${NOT_SENT_PREFIX}${display}`;
}

/** 取适用于既有界面文案的码，去掉浏览器内部的阶段标记。 */
export function adminApiFailureDisplayCode(code: string): string {
  const codeWithoutPrefix = code.startsWith(NOT_SENT_PREFIX)
    ? code.slice(NOT_SENT_PREFIX.length)
    : code;
  const endpointCode = writeEndpointCode(codeWithoutPrefix);
  return endpointCode === null ? codeWithoutPrefix : endpointCode.code;
}

/** 兼容认证接口的失败投影，判断本次认证请求是否明确失败。 */
export function adminApiFailureDefiniteRejection(code: string): boolean {
  return rejection(code, null) !== null;
}

/** 从端点标记中拆出端点、回执状态与协议码；任一段不合契约时为 `null`。 */
function writeEndpointCode(code: string): {
  readonly endpoint: AdminWriteEndpoint;
  readonly status: number;
  readonly code: string;
} | null {
  if (!code.startsWith(WRITE_ENDPOINT_PREFIX)) {
    return null;
  }
  const rest = code.slice(WRITE_ENDPOINT_PREFIX.length);
  const labelEnd = rest.indexOf(":");
  if (labelEnd < 0) {
    return null;
  }
  const receipt = rest.slice(labelEnd + 1);
  const statusEnd = receipt.indexOf(":");
  if (statusEnd < 0) {
    return null;
  }
  const endpoint = adminWriteEndpointParse(rest.slice(0, labelEnd));
  if (endpoint === null) {
    return null;
  }
  const status = parseU16(receipt.slice(0, statusEnd));
  if (status === null) {
    return null;
  }
  return { endpoint, status, code: receipt.slice(statusEnd + 1) };
}

/** 裁定一次回执或失败码能否证明本次尝试未产生效果。 */
function rejection(code: string, status: number | null): Rejection | null {
  if (status === null && (code.startsWith(NOT_SENT_PREFIX) || code === "invalid_request")) {
    return "this_attempt";
  }
  let endpoint: AdminWriteEndpoint | null = null;
  let effectiveStatus = status;
  let bareCode = code;
  const endpointCode = writeEndpointCode(code);
  if (endpointCode !== null) {
    if (status !== null && status !== endpointCode.status) {
      return null;
    }
    endpoint = endpointCode.endpoint;
    effectiveStatus = endpointCode.status;
    bareCode = endpointCode.code;
  }
  const expectedStatus = rejectionStatus(endpoint, bareCode);
  if (expectedStatus === null) {
    return null;
  }
  if (effectiveStatus !== null && effectiveStatus !== expectedStatus) {
    return null;
  }
  return rejectionScope(endpoint, bareCode);
}

function projectHttpFailure(failure: DecodedAdminHttpFailure): AdminApiFailure {
  const { status } = failure;
  const code =
    failure.kind === "valid-envelope"
      ? normalizeHttpCode(null, status, failure.protocolCode)
      : `http_${status}`;
  return { status, code };
}

function invalidHttpBody(status: number): DecodedAdminHttpFailure {
  reportBrowserError("dto_decode");
  return { kind: "invalid-envelope", status };
}

function normalizeHttpCode(
  endpoint: AdminWriteEndpoint | null,
  status: number,
  code: string,
): string {
  // 只允许完整的 413 契约信封证明正文在入口处被拒绝；非契约 413 的来源不可证。
  // 服务端只提供协议码，不能自行提供浏览器的未发送标记或端点标记。
  const expectedStatus = rejectionStatus(endpoint, code);
  if (!validHttpProtocolCode(code) || (expectedStatus !== null && expectedStatus !== status)) {
    return `http_${status}`;
  }
  return code;
}

/** 远端回执不可携带浏览器内部的阶段标记或构造失败码。 */
export function validHttpProtocolCode(code: string): boolean {
  return (
    !code.startsWith(NOT_SENT_PREFIX) &&
    !code.startsWith(WRITE_ENDPOINT_PREFIX) &&
    code !== "invalid_request"
  );
}

/** 校验错误信封的显式字段；缺字段、未知字段与错误类型都为 `null`。 */
function decodeAdminApiErrorResponse(value: unknown): AdminApiErrorResponse | null {
  const envelope = jsonExactObject(value, ["code", "message", "data", "request_id"]);
  if (envelope === null) {
    return null;
  }
  const { code, message, data, request_id: requestId } = envelope;
  if (typeof code !== "string" || typeof message !== "string") {
    return null;
  }
  // 只有对象或 null 是错误契约；异常 data 不能作为清除未知命令的证据。
  let evidence: Readonly<Record<string, unknown>> | null = null;
  if (data !== null) {
    if (!isJsonObject(data)) {
      return null;
    }
    evidence = data;
  }
  if (requestId !== null && typeof requestId !== "string") {
    return null;
  }
  return { code, message, data: evidence, request_id: requestId };
}

/** JSON 对象：非 null、非数组的 `typeof "object"`；解析产物只来自 JSON。 */
function isJsonObject(value: unknown): value is Readonly<Record<string, unknown>> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** 只接受 `0..=65535` 的十进制回执状态；与 Rust `str::parse::<u16>` 一样允许前导 `+`。 */
function parseU16(text: string): number | null {
  if (!/^\+?[0-9]+$/.test(text)) {
    return null;
  }
  const value = Number(text);
  return Number.isSafeInteger(value) && value <= 65_535 ? value : null;
}

/** 与 Rust `str::trim_ascii_start` 相同：只去掉行首的 ASCII 空白。 */
function trimAsciiStart(value: string): string {
  let start = 0;
  while (start < value.length) {
    const code = value.charCodeAt(start);
    if (code !== 0x09 && code !== 0x0a && code !== 0x0c && code !== 0x0d && code !== 0x20) {
      break;
    }
    start += 1;
  }
  return value.slice(start);
}

/** 将同源错误 envelope 缩减为稳定错误码；正文无效时只保留 HTTP 状态兜底。 */
export async function responseFailure(
  response: TransportResponse,
  signal: AbortSignal,
  host: AdminSessionHost,
): Promise<AdminApiFailure> {
  const decoded = decodeResponseFailure(response, signal, host);
  return decoded === null ? adminApiFailureClient("network_error") : projectHttpFailure(decoded);
}

/** 解码完整 HTTP 错误响应，并向原宿主交付可信的会话失效证据。 */
export function decodeResponseFailure(
  response: TransportResponse,
  signal: AbortSignal,
  host: AdminSessionHost,
): DecodedAdminHttpFailure | null {
  // 任务已取消或宿主已注销时 Rust 的 future 已被丢弃：不读正文，也不交付失效证据。
  if (signal.aborted || !adminSessionHostLive(host)) {
    return null;
  }
  const failure = decodeResponseHttpFailure(response);
  // 读取正文期间不会让出事件循环；这里仍按同一取消信号收口副作用来源。
  if (
    !signal.aborted &&
    adminSessionHostLive(host) &&
    failure.kind === "valid-envelope" &&
    failure.status === 401 &&
    failure.protocolCode === "unauthenticated"
  ) {
    reportAdminSessionInvalidation(response);
  }
  return failure;
}

/**
 * 只读取错误 envelope 的稳定码与 HTTP 状态；认证副作用由 `decodeResponseFailure` 触发。
 * 回执必须按原始字节严格解码 UTF-8：坏字节不能证明命令被拒绝，也不能确认登出。
 */
export function readResponseFailure(response: TransportResponse): AdminApiFailure {
  return projectHttpFailure(decodeResponseHttpFailure(response));
}

function decodeResponseHttpFailure(response: TransportResponse): DecodedAdminHttpFailure {
  let body: string;
  try {
    // 保留 BOM，让首字符检查拒绝它；与 Rust 的 `from_utf8` + `trim_ascii_start` 一致。
    body =
      response.body === null
        ? ""
        : new TextDecoder("utf-8", { fatal: true, ignoreBOM: true }).decode(response.body);
  } catch {
    // 与 Rust `str::from_utf8` 一样，非法字节使整条回执降级为 http_<status>。
    return invalidHttpBody(response.status);
  }
  return decodeHttpFailure(response.status, body);
}

/** `Result<_, string>` API 只返回稳定错误码，兼容现有页面状态而不泄漏底层错误正文。 */
export async function responseError(
  response: TransportResponse,
  signal: AbortSignal,
  host: AdminSessionHost,
): Promise<string> {
  return (await responseFailure(response, signal, host)).code;
}

/** 解析决定命令结果的回执；字段策略必须由调用方按 schema 给出。 */
export function decodeAdminReceipt<TSchema extends v.GenericSchema>(
  response: TransportResponse,
  schema: TSchema,
  policy: JsonFieldPolicy,
): Result<v.InferOutput<TSchema>, string> {
  const parsed = response.json(policy);
  if (!parsed.ok) {
    return err(invalidResponseError(parsed.error));
  }
  const decoded = decode(schema, parsed.value);
  return decoded.ok ? ok(decoded.value) : err(invalidResponseError(decoded));
}

/** 请求构造失败只暴露稳定业务 code；底层构建错误不进入页面。 */
export function invalidRequestError(_error: unknown): string {
  return "invalid_request";
}

/** 响应解码失败时上报 DTO 解码类别，页面只看到稳定 code。 */
export function invalidResponseError(_error: unknown): string {
  reportBrowserError("dto_decode");
  return "invalid_response";
}
