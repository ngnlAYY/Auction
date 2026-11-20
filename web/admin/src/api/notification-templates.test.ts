// 出站通知模板的正文检查、读响应自洽性与契约样本回归。

import { afterEach, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import {
  ADMIN_MAIL_TEMPLATE_LIST_FIELD_POLICY,
  type AdminMailTemplateDto,
  AdminMailTemplateListResponseSchema,
  AdminMailTemplateUpdateResponseSchema,
} from "@auction/contracts/dto/admin/mail-templates";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { utf8ByteLength } from "@auction/web-shared/text";

import {
  createAdminRequestHarness,
  parseJsonObject,
  type RecordedRequest,
  removeAt,
  TEST_SESSION_BODY,
} from "./api-request-fixture";
import {
  fetchMailTemplates,
  mailTemplateBodyProblem,
  type PreparedMailTemplateAttempt,
  prepareMailTemplateAttemptOnce,
  submitMailTemplate,
} from "./notification-templates";

const LIST_URL = "/api/v1/admin/mail-templates";
const TEMPLATE_URL = "/api/v1/admin/mail-templates/settlement_seller_payout";
const SESSION_URL = "/api/v1/admin/session?include=server_time";
const COMMAND_ID = "00000000-0000-4000-8000-000000000002";
const DEFAULT_BODY = "Item: {item_name}\nAmount: {amount} Credits\nReference: {reference}";
const LIST_RESPONSE_SCHEMA = apiResponseSchema(AdminMailTemplateListResponseSchema);
const UPDATE_RESPONSE_SCHEMA = apiResponseSchema(AdminMailTemplateUpdateResponseSchema);

const FIXTURES = new URL(
  "../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);

function fixtureText(fixtureName: string): string {
  return readFileSync(new URL(`${fixtureName}.json`, FIXTURES), "utf8");
}

const requests = createAdminRequestHarness();
afterEach(() => {
  requests.cleanup();
});

function requestLines(recorded: readonly RecordedRequest[]): (readonly [string, string])[] {
  return recorded.map((request) => [request.method, request.url] as const);
}

function payout(): AdminMailTemplateDto {
  return {
    key: "settlement_seller_payout",
    body: DEFAULT_BODY,
    default_body: DEFAULT_BODY,
    customized: false,
    usable: true,
    revision: 0,
    updated_by: null,
    updated_at: null,
    required_placeholders: ["amount", "reference"],
    allowed_placeholders: ["amount", "reference", "item_name"],
  };
}

function templatesBody(template: AdminMailTemplateDto): string {
  return JSON.stringify({ code: "ok", message: "", data: { templates: [template] } });
}

/** 先证明 DTO schema 接受该条目，再让读取入口拒绝；负例只取决于条目自洽校验。 */
function expectListSchemaAccepts(template: AdminMailTemplateDto): void {
  const parsed = parseJson(templatesBody(template), ADMIN_MAIL_TEMPLATE_LIST_FIELD_POLICY);
  expect(parsed.ok).toBe(true);
  if (!parsed.ok) {
    throw new Error("模板列表样本应是合法 JSON");
  }
  expect(decode(LIST_RESPONSE_SCHEMA, parsed.value).ok).toBe(true);
}

async function readTemplatesBody(body: string): Promise<{
  readonly result: Awaited<ReturnType<typeof fetchMailTemplates>>;
  readonly requests: readonly RecordedRequest[];
}> {
  const registration = requests.registerHost();
  const recorded = requests.installFetch([{ body }]);
  const result = await fetchMailTemplates(registration.host, new AbortController().signal);
  return { result, requests: [...recorded] };
}

async function readTemplates(
  template: AdminMailTemplateDto,
): Promise<Awaited<ReturnType<typeof fetchMailTemplates>>> {
  return (await readTemplatesBody(templatesBody(template))).result;
}

async function submitWithResponse(
  prepared: PreparedMailTemplateAttempt,
  responseBody: string,
): Promise<{
  readonly result: Awaited<ReturnType<typeof submitMailTemplate>>;
  readonly requests: readonly RecordedRequest[];
}> {
  requests.installBrowserRequest();
  const registration = requests.registerHost();
  const recorded = requests.installFetch([
    { body: JSON.stringify(TEST_SESSION_BODY) },
    { body: responseBody },
  ]);
  const result = await submitMailTemplate(
    registration.host,
    prepared,
    new AbortController().signal,
  );
  return { result, requests: [...recorded] };
}

test("草稿按模板占位符检查", () => {
  const template = payout();
  expect(mailTemplateBodyProblem(template, DEFAULT_BODY)).toBeNull();
  // 可选占位符可以不用。
  expect(mailTemplateBodyProblem(template, "{amount} / {reference}")).toBeNull();
  expect(mailTemplateBodyProblem(template, "Payout: {amount} Credits")).toBe("missing_placeholder");
  expect(mailTemplateBodyProblem(template, "{amount} {reference} {login_key}")).toBe(
    "unknown_placeholder",
  );
  expect(mailTemplateBodyProblem(template, "{amount} {reference")).toBe("unclosed");
  expect(mailTemplateBodyProblem(template, "{amount} {reference} }")).toBe("unclosed");
  for (const body of [
    "",
    " {amount} {reference}",
    "{amount} {reference}\n",
    "{amount}\u{7}{reference}",
  ]) {
    expect(mailTemplateBodyProblem(template, body), JSON.stringify(body)).toBe("format");
  }
});

test("模板专属内容必须出现", () => {
  const rejected = {
    ...payout(),
    key: "auction_equipment_rejected",
    required_placeholders: [],
    allowed_placeholders: ["reason_code", "reason_text"],
  };
  expect(mailTemplateBodyProblem(rejected, "Rejected")).not.toBeNull();
  for (const body of ["{reason_code}", "{reason_text}"]) {
    expect(mailTemplateBodyProblem(rejected, body)).toBeNull();
  }

  const pm = {
    ...payout(),
    key: "lot_watch_pm",
    required_placeholders: ["item_name", "lot_code", "amount"],
    allowed_placeholders: ["item_name", "lot_code", "amount", "notification_id"],
  };
  expect(
    mailTemplateBodyProblem(pm, "{item_name} {lot_code} {amount} {notification_id}"),
  ).not.toBeNull();
  expect(
    mailTemplateBodyProblem(
      pm,
      "{item_name} {lot_code} {amount}\nNotification: {notification_id}.",
    ),
  ).toBeNull();
});

test("通知模板上限按 UTF-8 字节计算", () => {
  const template = { ...payout(), key: "lot_watch" };
  const prefix = "{item_name} {amount} {reference} ";
  const remaining = (16 << 10) - utf8ByteLength(prefix);
  const limit = `${prefix}${"界".repeat(Math.floor(remaining / 3))}${"x".repeat(remaining % 3)}`;
  expect(utf8ByteLength(limit)).toBe(16 << 10);
  expect(mailTemplateBodyProblem(template, limit)).toBeNull();
  expect(mailTemplateBodyProblem(template, `${limit}x`)).not.toBeNull();
});

test("模板检查把 JSON 转义计入请求上限但不缩小正文上限", () => {
  const template = {
    ...payout(),
    key: "auction_no_attachments",
    required_placeholders: [],
    allowed_placeholders: [],
  };
  expect(mailTemplateBodyProblem(template, `"`.repeat(50_000))).not.toBeNull();
  expect(mailTemplateBodyProblem(template, "x".repeat(64 * 1024))).toBeNull();
});

test("响应里的模板条目必须自洽", async () => {
  expect((await readTemplates(payout())).ok).toBe(true);
  // 没有修订却声称自定义；默认状态的正文与默认正文不一致；必需占位符不在允许集合里。
  const inconsistent = [
    { ...payout(), customized: true },
    { ...payout(), body: "something else" },
    {
      ...payout(),
      required_placeholders: [...payout().required_placeholders, "login_key"],
    },
    { ...payout(), key: "../config" },
  ];
  for (const template of inconsistent) {
    expectListSchemaAccepts(template);
    expect((await readTemplates(template)).ok).toBe(false);
  }
});

test("通知模板的读取与命令契约样本", async () => {
  const listBody = fixtureText("mail-templates");
  const read = await readTemplatesBody(listBody);
  expect(read.result.ok).toBe(true);
  if (!read.result.ok) {
    throw new Error("模板列表样本应通过读取校验");
  }
  expect(requestLines(read.requests)).toEqual([["GET", LIST_URL]]);
  const payoutEntry = read.result.value.find((entry) => entry.key === "settlement_seller_payout");
  expect(payoutEntry?.customized).toBe(true);
  expect(payoutEntry?.revision).toBe(1);

  // 先在 DTO 层确认必填字段约束，再检查公开入口如何处理同一负例。
  const missingTemplates = parseJsonObject(listBody, "mail-templates.json");
  expect(decode(LIST_RESPONSE_SCHEMA, missingTemplates).ok).toBe(true);
  expect(Array.isArray(removeAt(missingTemplates, "/data/templates"))).toBe(true);
  expect(decode(LIST_RESPONSE_SCHEMA, missingTemplates).ok).toBe(false);
  const missingRead = await readTemplatesBody(JSON.stringify(missingTemplates));
  expect(missingRead.result).toEqual({ ok: false, error: "invalid_response" });
  expect(requestLines(missingRead.requests)).toEqual([["GET", LIST_URL]]);

  const prepared = prepareMailTemplateAttemptOnce({
    commandId: COMMAND_ID,
    key: "settlement_seller_payout",
    expectedRevision: 1,
    body: null,
  })(COMMAND_ID);
  if (!prepared.ok) {
    throw new Error("模板命令准备失败");
  }
  const commandBody = fixtureText("mail-template-command");
  const accepted = await submitWithResponse(prepared.value, commandBody);
  expect(accepted.result.ok).toBe(true);
  if (!accepted.result.ok) {
    throw new Error("模板命令样本应被接受");
  }
  expect(accepted.result.value.key).toBe("settlement_seller_payout");
  expect(accepted.result.value.revision).toBe(2);
  expect(accepted.result.value.customized).toBe(false);
  expect(accepted.result.value.body).toBe(accepted.result.value.default_body);
  expect(requestLines(accepted.requests)).toEqual([
    ["GET", SESSION_URL],
    ["PUT", TEMPLATE_URL],
  ]);

  const missingRevision = parseJsonObject(commandBody, "mail-template-command.json");
  expect(decode(UPDATE_RESPONSE_SCHEMA, missingRevision).ok).toBe(true);
  expect(removeAt(missingRevision, "/data/template/revision")).toBe(2);
  expect(decode(UPDATE_RESPONSE_SCHEMA, missingRevision).ok).toBe(false);
  const missingReceipt = await submitWithResponse(prepared.value, JSON.stringify(missingRevision));
  expect(missingReceipt.result).toEqual({
    ok: false,
    error: { status: null, code: "invalid_response" },
  });
  expect(requestLines(missingReceipt.requests)).toEqual([
    ["GET", SESSION_URL],
    ["PUT", TEMPLATE_URL],
  ]);
});
