// Admin 同源错误 envelope 的解码与稳定失败码投影。
// 服务端正文的自然语言不进入界面；页面只消费稳定 `code`，再经 Fluent 映射。HTTP 状态
// 单独保留，供 404、409 等需改变交互流程的页面判断。

import { decode } from "@auction/contracts/wire";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import { type JsonFieldPolicy, jsonExactObject, parseJson } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { reportBrowserError } from "@auction/web-shared/ui/bootstrap";
import type * as v from "valibot";

import { type AdminSessionHost, adminSessionHostLive, requestAdminSessionCheck } from "./session";
import { serdeJsonNumbersFitF64 } from "./validation";

// 仅浏览器构造，供仍使用 String 的调用方区分「业务请求未发送」和发送后的传输故障。
const NOT_SENT_PREFIX = "not_sent:";
// String 接口仍传递已验证回执的端点与状态；回调与面板只显示其中的协议码。
const WRITE_ENDPOINT_PREFIX = "write_endpoint:";

/** 写请求的端点标签；取值就是 Rust `AdminWriteEndpoint::label` 的线上标签。 */
export type AdminWriteEndpoint =
  | "user-mutation"
  | "rename-revert"
  | "rules-publish"
  | "about-publish"
  | "mail-template";

function adminWriteEndpointLabel(endpoint: AdminWriteEndpoint): string {
  return endpoint;
}

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

function adminWriteEndpointIsVersionedMutation(endpoint: AdminWriteEndpoint): boolean {
  // 这些端点的 state_conflict 只来自冻结版本的递增围栏；运行配置可已有持久化意图。
  return (
    endpoint === "user-mutation" || endpoint === "rules-publish" || endpoint === "about-publish"
  );
}

/** 一次拒绝的裁定范围：只否定本次尝试，或已经过同 ID 重放检查。 */
type Rejection = "this_attempt" | "replay_checked";

/** 经过账本核对且状态可在原围栏下恢复的拒绝码。 */
const REPLAY_CHECKED_CODES: ReadonlySet<string> = new Set([
  "alert_stale",
  "stale_auction_state",
  "stale_lot_state",
  "stale_user_state",
  "rename_not_revertable",
  "mail_template_conflict",
  "runtime_switch_conflict",
  "already_disposed",
  "stale_settlement_return_state",
  "settlement_return_untrusted",
  "settlement_return_unbound",
  "settlement_return_assets_mismatch",
  "settlement_return_proof_required",
  "conflict",
]);

/** Admin 浏览器请求的稳定失败投影。 */
export interface AdminApiFailure {
  readonly status: number | null;
  readonly code: string;
}

/** 只有契约规定的错误信封能证明命令被拒绝，可空字段也须显式出现。 */
interface AdminApiErrorResponse {
  readonly code: string;
  readonly message: string;
  readonly data: Readonly<Record<string, unknown>> | null;
  readonly request_id: string | null;
}

function adminApiErrorResponseIntoCode(response: AdminApiErrorResponse): string {
  // 只有稳定码进入浏览器状态或界面。
  return response.code;
}

/** 业务请求发出前的客户端失败：没有 HTTP 状态。 */
export function adminApiFailureClient(code: string): AdminApiFailure {
  return { status: null, code };
}

/** 解码失败回执。无法读出契约错误码时，只有 HTTP 状态可用于展示。 */
export function adminApiFailureFromHttpBody(status: number, body: string): AdminApiFailure {
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
  return adminApiFailureFromHttp(status, adminApiErrorResponseIntoCode(envelope));
}

/** 是否冲突（HTTP 409）。 */
export function adminApiFailureIsConflict(failure: AdminApiFailure): boolean {
  return failure.status === 409;
}

/** 未知结果只能重试同一命令 ID 与原负载。错误状态本身不能证明本次未写入。 */
export function adminApiFailureOutcomeUnknown(failure: AdminApiFailure): boolean {
  return rejection(failure.code, failure.status) === null;
}

/** 在写请求收到 HTTP 回执后记录端点，供只传递 String 的下游保留分类所需的证据。 */
export function adminApiFailureAtWriteEndpoint(
  failure: AdminApiFailure,
  endpoint: AdminWriteEndpoint,
): AdminApiFailure {
  if (failure.status === null) {
    return failure;
  }
  const code = normalizeHttpCode(endpoint, failure.status, failure.code);
  return {
    status: failure.status,
    code: `${WRITE_ENDPOINT_PREFIX}${adminWriteEndpointLabel(endpoint)}:${failure.status}:${code}`,
  };
}

/** 业务请求发出前失败。保留原稳定码供界面解释失败原因。 */
export function adminApiFailureNotSentCode(code: string): string {
  // 准备请求的同名错误不能裁定写命令此前的尝试。
  const display = adminApiFailureDisplayCode(code);
  if (
    adminApiFailureDefiniteRejection(display) &&
    !adminApiFailureRejectionAfterReplayCheck(display)
  ) {
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

/** 只认发出前的失败，以及本次尝试未产生也不会产生所请求效果的拒绝。 */
export function adminApiFailureDefiniteRejection(code: string): boolean {
  return rejection(code, null) !== null;
}

/** 回执须先经过同 ID 重放检查，且仍在途的原请求也不可能通过之后的状态校验。 */
export function adminApiFailureRejectionAfterReplayCheck(code: string): boolean {
  return rejection(code, null) === "replay_checked";
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
  // 已查账本仍不足以排除一个尚未取锁的原请求。状态可在原围栏下恢复的拒绝只裁定本次尝试。
  const replayChecked =
    REPLAY_CHECKED_CODES.has(bareCode) ||
    (bareCode === "state_conflict" &&
      endpoint !== null &&
      adminWriteEndpointIsVersionedMutation(endpoint));
  return replayChecked ? "replay_checked" : "this_attempt";
}

function rejectionStatus(endpoint: AdminWriteEndpoint | null, code: string): number | null {
  // 同码的所有写入口都必须排除命令效果。只读探测、被丢弃的登录结果与拒绝事实不接受命令；
  // 已提交、仍可使配置生效的持久化意图必须保留为未知。
  if (
    code === "state_conflict" &&
    endpoint !== null &&
    adminWriteEndpointIsVersionedMutation(endpoint)
  ) {
    return 409;
  }
  switch (code) {
    case "invalid_request_id":
    case "invalid_query":
    case "invalid_json":
    case "invalid_replay":
    case "invalid_command_id":
    case "invalid_auction_id":
    case "invalid_auction_command":
    case "invalid_auction_schedule":
    case "invalid_auction_intake":
    case "unsupported_equipment_source":
    case "invalid_lot_id":
    case "invalid_lot_action":
    case "invalid_lot_command":
    case "invalid_recovery_command":
    case "invalid_world":
    case "invalid_world_command":
    case "invalid_delivery_id":
    case "invalid_user_id":
    case "invalid_user_command":
    case "invalid_rename_id":
    case "invalid_rename_command":
    case "invalid_delivery_command":
    case "invalid_alert_command":
    case "invalid_credential_action":
    case "invalid_credential_kind":
    case "invalid_credential_label":
    case "invalid_credential_player_name":
    case "invalid_credential_request":
    case "invalid_credential_secret":
    case "invalid_credential_status":
    case "invalid_credential_world":
    case "invalid_mail_request":
    case "invalid_runtime_config":
    case "invalid_locale":
    case "invalid_rule_document":
    case "invalid_settlement_return_command":
    case "invalid_settlement_return_id":
      return 400;
    case "unauthenticated":
      return 401;
    case "origin_forbidden":
    case "csrf_invalid":
    case "manual_early_settlement_disabled":
      return 403;
    case "not_found":
    case "mail_template_not_found":
    case "world_not_found":
    case "delivery_not_found":
    case "settlement_return_not_found":
      return 404;
    case "method_not_allowed":
      return 405;
    case "mail_template_conflict":
    case "runtime_switch_conflict":
    case "command_conflict":
    case "credential_conflict":
    case "stale_credential_state":
    case "stale_user_state":
    case "rename_not_revertable":
    case "rename_name_taken":
    case "user_conflict":
    case "stale_state_version":
    case "stale_world_state":
    case "world_has_active_auction":
    case "stale_auction_state":
    case "auction_intake_unavailable":
    case "seller_blacklisted":
    case "equipment_conflict":
    case "stale_lot_state":
    case "stale_recovery_state":
    case "stale_delivery_state":
    case "stale_settlement_return_state":
    case "invalid_auction_state":
    case "invalid_lot_state":
    case "alert_stale":
    case "already_disposed":
    case "delivery_sent_sighted":
    // 换绑可恢复，且快照校验早于兼容旧实例的第二次账本检查，两者只裁定本次。
    case "delivery_recipient_renamed":
    case "delivery_command_snapshot_unreadable":
    case "settlement_obligation_ineligible":
    case "sent_proof_rejected":
    case "settlement_return_untrusted":
    case "settlement_return_unbound":
    case "settlement_return_assets_mismatch":
    case "settlement_return_action_conflict":
    case "settlement_return_proof_required":
    case "settlement_return_resend_limit":
    case "settlement_return_obligation_ineligible":
    case "settlement_return_send_unresolved":
    case "settlement_return_observation_required":
    case "recovery_send_unattributed":
    case "recovery_not_confirmable":
    case "recovery_evidence_unresolved":
    case "external_access_paused":
    case "poll_rejected":
    case "conflict":
      return 409;
    case "body_too_large":
      return 413;
    case "unsupported_media_type":
      return 415;
    case "invalid_mail_template":
    case "invalid_runtime_switch":
    case "not_acknowledgeable":
    case "credential_login_rejected":
    case "credential_login_challenge":
      return 422;
    case "rate_limited":
    case "credential_login_busy":
      return 429;
    // 登录失败时未得到可导入的凭据，存储写入尚未开始。
    case "credential_login_unavailable":
      return 503;
    default:
      return null;
  }
}

function adminApiFailureFromHttp(status: number, code: string): AdminApiFailure {
  const normalized = normalizeHttpCode(null, status, code);
  return { status, code: normalized };
}

function invalidHttpBody(status: number): AdminApiFailure {
  reportBrowserError("dto_decode");
  return adminApiFailureFromHttp(status, `http_${status}`);
}

function normalizeHttpCode(
  endpoint: AdminWriteEndpoint | null,
  status: number,
  code: string,
): string {
  // 只允许完整的 413 契约信封证明正文在入口处被拒绝；非契约 413 的来源不可证。
  // 服务端只提供协议码，不能自行提供浏览器的未发送标记或端点标记。
  const expectedStatus = rejectionStatus(endpoint, code);
  if (
    code.startsWith(NOT_SENT_PREFIX) ||
    code.startsWith(WRITE_ENDPOINT_PREFIX) ||
    code === "invalid_request" ||
    (expectedStatus !== null && expectedStatus !== status)
  ) {
    return `http_${status}`;
  }
  return code;
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
  // 任务已取消或宿主已注销时 Rust 的 future 已被丢弃：不读正文，也不触发会话复核。
  if (signal.aborted || !adminSessionHostLive(host)) {
    return adminApiFailureClient("network_error");
  }
  const failure = readResponseFailure(response);
  // 读取正文期间不会让出事件循环；这里仍按同一取消信号收口副作用来源。
  if (
    !signal.aborted &&
    adminSessionHostLive(host) &&
    failure.status === 401 &&
    failure.code === "unauthenticated"
  ) {
    requestAdminSessionCheck(host);
  }
  return failure;
}

/**
 * 只读取错误 envelope 的稳定码与 HTTP 状态；认证副作用由唯一调用方 `responseFailure` 触发。
 * 回执必须按原始字节严格解码 UTF-8：坏字节不能证明命令被拒绝，也不能确认登出。
 */
export function readResponseFailure(response: TransportResponse): AdminApiFailure {
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
  return adminApiFailureFromHttpBody(response.status, body);
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
