// Admin 站点文档（规则、关于）API 客户端。
// 浏览器只持有文档安全快照；发布命令的版本 fence 与 command_id 由服务端最终校验。
// 两种文档的请求与响应形状相同，只有接口路径不同。

import {
  type AdminRulesCandidateRequest,
  type AdminRulesCandidateResponse,
  AdminRulesCandidateResponseSchema,
  type AdminRulesPublishRequest,
  type AdminRulesResponse,
  AdminRulesResponseSchema,
} from "@auction/contracts/dto/admin/rules";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import type { JsonFieldPolicy } from "@auction/web-shared/net/wire";
import { err, ok, type Result, unreachable } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";

import {
  type AdminApiFailure,
  type AdminWriteEndpoint,
  adminApiFailureAtWriteEndpoint,
  adminApiFailureClient,
  decodeAdminReceipt,
  responseFailure,
} from "./failure";
import type { AdminSessionHost } from "./session";
import {
  type PreparedJsonCommand,
  prepareJsonCommandOnce,
  sendGet,
  sendJsonWrite,
  sendPreparedJsonWrite,
} from "./transport";
import { MAX_JSON_SAFE_INTEGER, validAdminTime, validTrimmedText } from "./validation";

// 结构闭集由 contracts 的 `AdminRulesResponse` 与 `AdminRulesCandidateResponse`
//（`deny_unknown_fields`）负责；本文件只保留 locale/title/发布时间等值域谓词与请求回显校验。

/** Admin 规则请求失败时保留 HTTP 状态，页面可以把版本冲突和普通网络错误区分开。 */
export type AdminRulesApiError = AdminApiFailure;

/** 管理端可编辑的站点文档种类：决定接口路径，并隔离各自的草稿与未决命令。 */
export type AdminDocumentKind = "rules" | "about";

/** 站点文档的 wire 投影别名，与 Rust 的 `AdminRules` 对应。 */
export type AdminRules = AdminRulesResponse;

/** 站点文档候选校验的 wire 投影别名，与 Rust 的 `AdminRulesCandidate` 对应。 */
export type AdminRulesCandidate = AdminRulesCandidateResponse;

/**
 * 文档读取回执的 revision 会作为下一次发布的 `expected_revision` 发回服务端，
 * 因此按整数词法解析；其余字段都是字符串。
 */
const ADMIN_RULES_DOCUMENT_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
  integerPaths: [["data", "revision"]],
};

/** 候选回显的标题与正文会被冻结进发布命令，按拒绝重复键的严格词法读取。 */
const ADMIN_RULES_CANDIDATE_FIELD_POLICY: JsonFieldPolicy = {
  uniqueKeys: "all",
};

/** 准备一次发布；返回的函数在第一次运行时生成并缓存固定正文。 */
export type AdminRulesPublishPreparation = (
  commandId: string,
) => Result<PreparedAdminRulesPublish, AdminApiFailure>;

/** 已冻结的发布输入与首次执行时生成的正文；正文供首次发送和原样重试共用，回执也用同一份输入核对。 */
export interface PreparedAdminRulesPublish {
  readonly expectedRevision: number;
  readonly locale: string;
  readonly title: string;
  readonly markdown: string;
  readonly body: PreparedJsonCommand;
}

function adminDocumentKindApiPath(kind: AdminDocumentKind): string {
  switch (kind) {
    case "rules":
      return "/api/v1/admin/rules";
    case "about":
      return "/api/v1/admin/about";
    default:
      return unreachable(kind);
  }
}

/** 未决命令账本里的实体 key；一种文档同时只允许一个未决发布。 */
export function adminDocumentKindPendingKey(kind: AdminDocumentKind): string {
  switch (kind) {
    case "rules":
      return "rules";
    case "about":
      return "about";
    default:
      return unreachable(kind);
  }
}

/** 读取一种站点文档；语言切换会让请求带上旧版约定的 `lang` 查询参数。 */
export async function fetchAdminRules(
  host: AdminSessionHost,
  kind: AdminDocumentKind,
  locale: string,
  signal: AbortSignal,
): Promise<Result<AdminRules, AdminRulesApiError>> {
  const query = new URLSearchParams();
  query.append("lang", rustTrim(locale));
  const url = `${adminDocumentKindApiPath(kind)}?${query.toString()}`;
  const response = await sendGet(host, url, signal);
  if (!response.ok) {
    return err(adminApiFailureClient(response.error));
  }
  if (!responseOk(response.value.status)) {
    return err(await responseFailure(response.value, signal, host));
  }
  const decoded = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminRulesResponseSchema),
    ADMIN_RULES_DOCUMENT_FIELD_POLICY,
  );
  if (!decoded.ok) {
    return err(adminApiFailureClient("invalid_response"));
  }
  return decodeReadDocument(decoded.value.data, locale);
}

function decodeReadDocument(
  value: AdminRules,
  locale: string,
): Result<AdminRules, AdminRulesApiError> {
  const rules = decodeRules(value);
  if (!rules.ok || rules.value.locale !== rustTrim(locale)) {
    return err(adminApiFailureClient("invalid_response"));
  }
  return ok(rules.value);
}

/** 校验候选文档；成功只代表可以发布，不会改变权威文档。 */
export async function validateAdminRules(
  host: AdminSessionHost,
  kind: AdminDocumentKind,
  locale: string,
  title: string,
  markdown: string,
  signal: AbortSignal,
): Promise<Result<AdminRulesCandidate, AdminRulesApiError>> {
  const request: AdminRulesCandidateRequest = { locale, title, markdown };
  const response = await sendJsonWrite(
    host,
    adminDocumentKindApiPath(kind),
    "POST",
    request,
    signal,
  );
  if (!response.ok) {
    return err(response.error);
  }
  const decoded = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminRulesCandidateResponseSchema),
    ADMIN_RULES_CANDIDATE_FIELD_POLICY,
  );
  if (!decoded.ok) {
    return err(adminApiFailureClient("invalid_response"));
  }
  const candidate = decodeCandidate(decoded.value.data);
  if (
    !candidate.ok ||
    candidate.value.locale !== request.locale ||
    candidate.value.title !== rustTrim(request.title) ||
    candidate.value.markdown !== request.markdown
  ) {
    return err(adminApiFailureClient("invalid_response"));
  }
  return ok(candidate.value);
}

/**
 * 冻结发布输入；正文只在该命令第一次执行时生成，同一 `command_id` 的重试复用同一份字节。
 */
export function prepareAdminRulesPublish(
  expectedRevision: number,
  locale: string,
  title: string,
  markdown: string,
): AdminRulesPublishPreparation {
  // 冻结一份输入：正文与回执校验字段都由它生成，调用方不再各自传一套。
  const frozen = Object.freeze({ expectedRevision, locale, title, markdown });
  const prepare = prepareJsonCommandOnce(
    (commandId): AdminRulesPublishRequest => ({
      command_id: commandId,
      expected_revision: frozen.expectedRevision,
      locale: frozen.locale,
      title: frozen.title,
      markdown: frozen.markdown,
    }),
  );
  return (commandId) => {
    const prepared = prepare(commandId);
    if (!prepared.ok) {
      return err(prepared.error);
    }
    return ok(Object.freeze({ ...frozen, body: prepared.value }));
  };
}

/** 发布站点文档；失败回执按文档种类归入对应的写端点。 */
export async function publishAdminRules(
  host: AdminSessionHost,
  kind: AdminDocumentKind,
  commandId: string,
  prepare: AdminRulesPublishPreparation,
  signal: AbortSignal,
): Promise<Result<AdminRules, AdminRulesApiError>> {
  const prepared = prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const endpoint: AdminWriteEndpoint = kind === "rules" ? "rules-publish" : "about-publish";
  const response = await sendPreparedJsonWrite(
    host,
    adminDocumentKindApiPath(kind),
    "PUT",
    prepared.value.body,
    signal,
  );
  if (!response.ok) {
    return err(adminApiFailureAtWriteEndpoint(response.error, endpoint));
  }
  const decoded = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminRulesResponseSchema),
    ADMIN_RULES_DOCUMENT_FIELD_POLICY,
  );
  if (!decoded.ok) {
    return err(adminApiFailureClient("invalid_response"));
  }
  return decodePublishReceipt(decoded.value.data, prepared.value);
}

function decodePublishReceipt(
  value: AdminRules,
  request: PreparedAdminRulesPublish,
): Result<AdminRules, AdminRulesApiError> {
  const rules = decodeRules(value);
  if (
    !rules.ok ||
    rules.value.locale !== request.locale ||
    rules.value.title !== request.title ||
    rules.value.markdown !== request.markdown ||
    rules.value.revision !==
      (request.expectedRevision < MAX_JSON_SAFE_INTEGER ? request.expectedRevision + 1 : null)
  ) {
    return err(adminApiFailureClient("invalid_response"));
  }
  return ok(rules.value);
}

function decodeCandidate(
  value: AdminRulesCandidate,
): Result<AdminRulesCandidate, "rules_candidate"> {
  if (!validLocale(value.locale) || !validTrimmedText(value.title)) {
    return err("rules_candidate");
  }
  return ok(value);
}

function decodeRules(value: AdminRules): Result<AdminRules, "rules"> {
  if (
    !validLocale(value.locale) ||
    !validTrimmedText(value.title) ||
    !validTrimmedText(value.published_by) ||
    !Number.isSafeInteger(value.revision) ||
    value.revision < 1 ||
    value.revision > MAX_JSON_SAFE_INTEGER ||
    !validAdminTime(value.published_at)
  ) {
    return err("rules");
  }
  return ok(value);
}

function validLocale(value: string): boolean {
  if (
    value.length === 0 ||
    utf8ByteLength(value) > 35 ||
    value.startsWith("-") ||
    value.endsWith("-")
  ) {
    return false;
  }
  for (let index = 0; index < value.length; index += 1) {
    const code = value.charCodeAt(index);
    const asciiDigit = code >= 0x30 && code <= 0x39;
    const asciiUpper = code >= 0x41 && code <= 0x5a;
    const asciiLower = code >= 0x61 && code <= 0x7a;
    if (!asciiDigit && !asciiUpper && !asciiLower && code !== 0x2d) {
      return false;
    }
  }
  return true;
}

function responseOk(status: number): boolean {
  return status >= 200 && status <= 299;
}

/** 与 Rust `str::trim` 的字符集合一致的裁剪。 */
function rustTrim(value: string): string {
  let start = 0;
  let end = value.length;
  while (start < end && isRustWhitespace(value.charCodeAt(start))) {
    start += 1;
  }
  while (end > start && isRustWhitespace(value.charCodeAt(end - 1))) {
    end -= 1;
  }
  return value.slice(start, end);
}

function isRustWhitespace(code: number): boolean {
  return (
    (code >= 0x09 && code <= 0x0d) ||
    code === 0x20 ||
    code === 0x85 ||
    code === 0xa0 ||
    code === 0x1680 ||
    (code >= 0x2000 && code <= 0x200a) ||
    code === 0x2028 ||
    code === 0x2029 ||
    code === 0x202f ||
    code === 0x205f ||
    code === 0x3000
  );
}
