// 出站通知模板客户端：读取全部模板，逐个保存正文或恢复内置默认正文。

import {
  ADMIN_MAIL_TEMPLATE_LIST_FIELD_POLICY,
  ADMIN_MAIL_TEMPLATE_UPDATE_FIELD_POLICY,
  type AdminMailTemplateDto,
  AdminMailTemplateListResponseSchema,
  type AdminMailTemplateUpdateRequest,
  AdminMailTemplateUpdateResponseSchema,
} from "@auction/contracts/dto/admin/mail-templates";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { err, ok, type Result } from "@auction/web-shared/result";
import { utf8ByteLength } from "@auction/web-shared/text";

import { parseRustUuid, rustUuidIsNil } from "./credentials/decode";
import {
  type AdminApiFailure,
  adminApiFailureAtWriteEndpoint,
  adminApiFailureClient,
  decodeAdminReceipt,
  invalidRequestError,
  invalidResponseError,
  responseError,
} from "./failure";
import type { AdminSessionHost } from "./session";
import { trimRustWhitespace } from "./session-time";
import {
  type PreparedJsonCommand,
  prepareJsonCommand,
  sendGet,
  sendPreparedJsonWrite,
} from "./transport";

/** 单个模板正文的字节上限，与服务端一致。 */
const MAX_TEMPLATE_BODY_BYTES = 64 * 1024;
const MAX_WATCH_BODY_BYTES = 16 * 1024;
// 镜像 auction-http 的 server/surfaces.rs 中 PUT /api/v1/admin/mail-templates/{template_key}。
const MAX_TEMPLATE_REQUEST_BYTES = 96 * 1024;
const NIL_UUID = "00000000-0000-0000-0000-000000000000";

/** 读取全部模板，并核对非空、逐项自洽与模板键唯一。 */
export async function fetchMailTemplates(
  host: AdminSessionHost,
  signal: AbortSignal,
): Promise<Result<readonly AdminMailTemplateDto[], string>> {
  const response = await sendGet(host, "/api/v1/admin/mail-templates", signal);
  if (!response.ok) {
    return err(response.error);
  }
  const received = response.value;
  if (!responseOk(received)) {
    return err(await responseError(received, signal, host));
  }
  const payload = decodeAdminReceipt(
    received,
    apiResponseSchema(AdminMailTemplateListResponseSchema),
    ADMIN_MAIL_TEMPLATE_LIST_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(payload.error);
  }
  const templates = payload.value.data.templates;
  if (
    templates.length === 0 ||
    !templates.every(
      (template, index) =>
        validTemplateEntry(template) &&
        !templates.slice(0, index).some((earlier) => earlier.key === template.key),
    )
  ) {
    return err(invalidResponseError("mail templates"));
  }
  return ok(templates);
}

/** 响应里的一项是否自洽：键是 snake_case，版本与“是否自定义”对得上，必需占位符都在允许集合里。 */
function validTemplateEntry(template: AdminMailTemplateDto): boolean {
  return (
    validTemplateKey(template.key) &&
    Number.isSafeInteger(template.revision) &&
    template.revision >= 0 &&
    template.body !== "" &&
    template.default_body !== "" &&
    (template.revision > 0 || !template.customized) &&
    (template.customized || (template.usable && template.body === template.default_body)) &&
    template.revision > 0 === (template.updated_by !== null) &&
    template.revision > 0 === (template.updated_at !== null) &&
    template.required_placeholders.every((name) => template.allowed_placeholders.includes(name))
  );
}

function validTemplateKey(key: string): boolean {
  return utf8ByteLength(key) <= 64 && /^[a-z][a-z0-9_]*$/u.test(key);
}

/**
 * 一次保存或恢复默认的尝试。结果未知时必须整份原样重试：命令 ID、期望版本与正文都在首次
 * 提交时确定。
 */
export interface MailTemplateAttempt {
  readonly commandId: string;
  readonly key: string;
  readonly expectedRevision: number;
  /** 新正文；`null` 表示恢复内置默认正文。 */
  readonly body: string | null;
}

/** 已冻结的模板尝试与首次执行时生成的正文。 */
export interface PreparedMailTemplateAttempt {
  readonly attempt: MailTemplateAttempt;
  readonly body: PreparedJsonCommand;
}

function encodeTemplateRequest(
  commandId: string,
  expectedRevision: number,
  body: string | null,
): Result<PreparedJsonCommand, string> {
  const command: AdminMailTemplateUpdateRequest = {
    command_id: commandId,
    expected_revision: expectedRevision,
    body,
  };
  const prepared = prepareJsonCommand(command);
  if (!prepared.ok) {
    return err(prepared.error.code);
  }
  if (utf8ByteLength(prepared.value.body) > MAX_TEMPLATE_REQUEST_BYTES) {
    return err("body_too_large");
  }
  return ok(prepared.value);
}

/**
 * 冻结保存或恢复默认的输入；执行体第一次运行时才结合 command ID 生成并缓存正文。
 * 完整写请求的 96 KiB 上限在正文生成后立即检查，同一失败也随正文一起缓存。
 */
export function prepareMailTemplateAttemptOnce(
  attempt: MailTemplateAttempt,
): (commandId: string) => Result<PreparedMailTemplateAttempt, AdminApiFailure> {
  const frozen = Object.freeze({
    commandId: attempt.commandId,
    key: attempt.key,
    expectedRevision: attempt.expectedRevision,
    body: attempt.body,
  });
  // 预检与首次发送共用同一个编码器；结果（包括超限失败）随命令 ID 缓存，重试不再重新编码。
  let cachedId: string | null = null;
  let cached: Result<PreparedJsonCommand, AdminApiFailure> | null = null;
  const wrap = (
    prepared: Result<PreparedJsonCommand, AdminApiFailure>,
  ): Result<PreparedMailTemplateAttempt, AdminApiFailure> =>
    prepared.ok ? ok(Object.freeze({ attempt: frozen, body: prepared.value })) : prepared;
  return (commandId) => {
    const canonicalCommandId = parseRustUuid(frozen.commandId);
    if (
      canonicalCommandId === null ||
      rustUuidIsNil(canonicalCommandId) ||
      canonicalCommandId !== commandId ||
      !Number.isSafeInteger(frozen.expectedRevision) ||
      frozen.expectedRevision < 0 ||
      !validTemplateKey(frozen.key) ||
      frozen.body === ""
    ) {
      return err(adminApiFailureClient("invalid_request"));
    }
    if (cached !== null) {
      if (commandId !== cachedId) {
        return err(adminApiFailureClient(invalidRequestError(commandId)));
      }
      return wrap(cached);
    }
    const encoded = encodeTemplateRequest(commandId, frozen.expectedRevision, frozen.body);
    cachedId = commandId;
    cached = encoded.ok ? ok(encoded.value) : err(adminApiFailureClient(encoded.error));
    return wrap(cached);
  };
}

/** 提交保存或恢复默认；只接受与命令一致的已接受结果。 */
export async function submitMailTemplate(
  host: AdminSessionHost,
  prepared: PreparedMailTemplateAttempt,
  signal: AbortSignal,
): Promise<Result<AdminMailTemplateDto, AdminApiFailure>> {
  const attempt = prepared.attempt;
  const response = await sendPreparedJsonWrite(
    host,
    `/api/v1/admin/mail-templates/${attempt.key}`,
    "PUT",
    prepared.body,
    signal,
  );
  if (!response.ok) {
    return err(adminApiFailureAtWriteEndpoint(response.error, "mail-template"));
  }
  const payload = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminMailTemplateUpdateResponseSchema),
    ADMIN_MAIL_TEMPLATE_UPDATE_FIELD_POLICY,
  );
  if (!payload.ok) {
    return err(adminApiFailureClient(payload.error));
  }
  const accepted = payload.value.data.template;
  // 重放返回的是原结果，版本恰好是期望版本加一；正文与“是否自定义”必须与命令一致。
  const nextRevision =
    attempt.expectedRevision < Number.MAX_SAFE_INTEGER &&
    Number.isSafeInteger(attempt.expectedRevision + 1)
      ? attempt.expectedRevision + 1
      : null;
  if (
    !validTemplateEntry(accepted) ||
    accepted.key !== attempt.key ||
    accepted.revision !== nextRevision ||
    accepted.customized !== (attempt.body !== null) ||
    (attempt.body !== null && accepted.body !== attempt.body)
  ) {
    return err(adminApiFailureClient(invalidResponseError("mail template update")));
  }
  return ok(accepted);
}

/** 草稿正文不能保存的原因；只做浏览器里的即时提示，服务端仍按同一规则完整校验。 */
export type MailTemplateBodyProblem =
  /** 正文为空、首尾有空白、过长或含控制字符。 */
  | "format"
  /** 花括号没有成对出现。 */
  | "unclosed"
  /** 使用了该模板不允许的占位符。 */
  | "unknown_placeholder"
  /** 缺少必需占位符。 */
  | "missing_placeholder"
  /** 装备拒收原因缺失。 */
  | "missing_rejection_reason"
  /** 私信缺少与发送证明绑定的固定句。 */
  | "missing_notification_identity"
  /** 被超提醒超过冻结投递正文的字节上限。 */
  | "watch_body_too_long"
  /** 完整编码后的写请求超出路由的请求体上限。 */
  | "request_too_large";

/** 私信发送证明绑定的固定句，与服务端逐字一致。 */
export const NOTIFICATION_IDENTITY = "Notification: {notification_id}.";

/** 按正文格式、占位符及模板专属要求检查草稿；旧的不可用正文也可读取并修正。 */
export function mailTemplateBodyProblem(
  template: AdminMailTemplateDto,
  body: string,
): MailTemplateBodyProblem | null {
  if (
    body === "" ||
    body !== trimRustWhitespace(body) ||
    utf8ByteLength(body) > MAX_TEMPLATE_BODY_BYTES ||
    hasForbiddenControlCharacter(body)
  ) {
    return "format";
  }
  let cursor = 0;
  const seen: string[] = [];
  while (true) {
    const relativeStart = body.indexOf("{", cursor);
    if (relativeStart < 0) {
      break;
    }
    const start = relativeStart;
    if (body.slice(cursor, start).includes("}")) {
      return "unclosed";
    }
    const relativeEnd = body.indexOf("}", start + 1);
    if (relativeEnd < 0) {
      return "unclosed";
    }
    const end = relativeEnd;
    const placeholder = body.slice(start + 1, end);
    if (!template.allowed_placeholders.includes(placeholder)) {
      return "unknown_placeholder";
    }
    seen.push(placeholder);
    cursor = end + 1;
  }
  if (body.slice(cursor).includes("}")) {
    return "unclosed";
  }
  if (template.required_placeholders.some((name) => !seen.includes(name))) {
    return "missing_placeholder";
  }
  let problem: MailTemplateBodyProblem | null = null;
  if (
    template.key === "auction_equipment_rejected" &&
    !seen.includes("reason_code") &&
    !seen.includes("reason_text")
  ) {
    problem = "missing_rejection_reason";
  } else if (template.key === "lot_watch_pm" && !body.includes(NOTIFICATION_IDENTITY)) {
    problem = "missing_notification_identity";
  } else if (
    (template.key === "lot_watch" || template.key === "lot_watch_pm") &&
    utf8ByteLength(body) > MAX_WATCH_BODY_BYTES
  ) {
    problem = "watch_body_too_long";
  }
  if (problem !== null) {
    return problem;
  }
  // UUID 的规范文本固定为 36 字节。确认前用等长值预检，HTTP 发送复用同一个编码器。
  const encoded = encodeTemplateRequest(NIL_UUID, template.revision, body);
  if (encoded.ok) {
    return null;
  }
  return encoded.error === "body_too_large" ? "request_too_large" : "format";
}

function hasForbiddenControlCharacter(value: string): boolean {
  for (const character of value) {
    if (
      /\p{Cc}/u.test(character) &&
      character !== "\n" &&
      character !== "\r" &&
      character !== "\t"
    ) {
      return true;
    }
  }
  return false;
}

function responseOk(response: { readonly status: number }): boolean {
  return response.status >= 200 && response.status <= 299;
}
