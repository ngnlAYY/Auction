// Admin lot 列表 API 客户端。
// 只解码后端安全投影；浏览器不接触数据库内部模型或装备密钥。

import type { AdminLotUpdateRequest } from "@auction/contracts/dto/admin/commands";
import {
  ADMIN_LOT_LIST_FIELD_POLICY,
  ADMIN_LOT_UPDATE_FIELD_POLICY,
  AdminLotListResponseSchema,
  type AdminLotPaginationResponse,
  type AdminLotResponse,
  AdminLotUpdateResponseSchema,
} from "@auction/contracts/dto/admin/lot";
import type { AdminAuctionResponse } from "@auction/contracts/dto/admin/settlement";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { parseJson } from "@auction/web-shared/net/wire";
import { err, ok, type Result } from "@auction/web-shared/result";
import { validUuid } from "@auction/web-shared/ui/validation";

import { decodeAuction, sendAuctionEdit } from "./auctions";
import {
  type AdminWriteFailure,
  type AdminWriteNotSent,
  decodeAdminReceipt,
  invalidResponseError,
  responseError,
  writeNotSent,
  writeOutcomeUnknown,
} from "./failure";
import { validate, validPageEcho } from "./pagination";
import type { AdminSessionHost } from "./session";
import { type PreparedJsonCommand, prepareJsonCommandOnce, sendGet } from "./transport";
import {
  MAX_JSON_SAFE_INTEGER,
  validAdminTime,
  validOptionalText,
  validOptionalTime,
  validTrimmedText,
} from "./validation";

/** Admin lot 安全投影；与 Rust `AdminLotResponse` 的线上字段一一对应。 */
export type AdminLot = AdminLotResponse;
/** lot 列表的数字分页回执；页码、页大小与总数都必须显式回显。 */
export type AdminLotPagination = AdminLotPaginationResponse;

/** lot 页的浏览器本地模型；响应 body 保持旧 v1 闭集，额外能力只从非破坏性响应头读取。 */
export interface AdminLotPage {
  readonly auction: AdminAuctionResponse;
  readonly lots: readonly AdminLot[];
  readonly pagination: AdminLotPagination;
  readonly capabilities: AdminLotCapabilities;
}

/** 响应头携带的 lot 页能力；旧 v1 响应正文不包含这些扩展字段。 */
export interface AdminLotCapabilities {
  readonly manualIntake: boolean;
}

interface RawJsonMember {
  readonly key: string;
  readonly value: RawJsonNode;
}

type RawJsonNode =
  | {
      readonly kind: "object";
      readonly start: number;
      readonly end: number;
      readonly members: readonly RawJsonMember[];
    }
  | {
      readonly kind: "array";
      readonly start: number;
      readonly end: number;
      readonly items: readonly RawJsonNode[];
    }
  | { readonly kind: "scalar"; readonly start: number; readonly end: number };

// 响应原文里的装备属性仍带着 serde_json 的数值词法与成员顺序；只有展示需要它，
// 因此按解析出的对象身份登记，不改变 DTO 结构或传输层。
const EQUIPMENT_ATTRIBUTES_SOURCE = new WeakMap<object, string>();

/** 取回装备属性在响应原文中的子树；没有保留原文时返回 `null`。 */
export function equipmentAttributesSource(value: unknown): string | null {
  if (typeof value !== "object" || value === null) {
    return null;
  }
  return EQUIPMENT_ATTRIBUTES_SOURCE.get(value) ?? null;
}

function rememberEquipmentAttributesSource(value: unknown, source: string): void {
  if (typeof value === "object" && value !== null) {
    EQUIPMENT_ATTRIBUTES_SOURCE.set(value, source);
  }
}

/** 响应原文中每个 lot 的 `equipment_attributes` 子树；结构不符时返回 `null`。 */
function equipmentAttributeSources(text: string): readonly (string | null)[] | null {
  const root = parseRawJsonNode(text, 0);
  if (root === null || skipJsonWhitespace(text, root.next) !== text.length) {
    return null;
  }
  const data = rawJsonMember(root.node, "data");
  const lots = data === null ? null : rawJsonMember(data, "lots");
  if (lots === null || lots.kind !== "array") {
    return null;
  }
  const sources: (string | null)[] = [];
  for (const item of lots.items) {
    const attributes = rawJsonMember(item, "equipment_attributes");
    sources.push(attributes === null ? null : text.slice(attributes.start, attributes.end));
  }
  return sources;
}

function rawJsonMember(node: RawJsonNode, key: string): RawJsonNode | null {
  if (node.kind !== "object") {
    return null;
  }
  let found: RawJsonNode | null = null;
  for (const member of node.members) {
    if (member.key === key) {
      found = member.value;
    }
  }
  return found;
}

interface ParsedRawJsonNode {
  readonly node: RawJsonNode;
  readonly next: number;
}

// 响应已经通过严格 JSON 解析；这里只按源码位置切出装备属性子树，不改写任何词法。
const JSON_NUMBER = /-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?/uy;

function parseRawJsonNode(text: string, start: number): ParsedRawJsonNode | null {
  const index = skipJsonWhitespace(text, start);
  const character = text[index];
  if (character === '"') {
    const end = jsonStringEnd(text, index);
    return end < 0 ? null : { node: { kind: "scalar", start: index, end }, next: end };
  }
  if (character === "{") {
    return parseRawJsonObject(text, index);
  }
  if (character === "[") {
    return parseRawJsonArray(text, index);
  }
  for (const [literal, length] of [
    ["true", 4],
    ["null", 4],
    ["false", 5],
  ] as const) {
    if (text.startsWith(literal, index)) {
      const end = index + length;
      return { node: { kind: "scalar", start: index, end }, next: end };
    }
  }
  JSON_NUMBER.lastIndex = index;
  const number = JSON_NUMBER.exec(text);
  if (number === null) {
    return null;
  }
  const end = index + number[0].length;
  return { node: { kind: "scalar", start: index, end }, next: end };
}

function parseRawJsonObject(text: string, start: number): ParsedRawJsonNode | null {
  const members: RawJsonMember[] = [];
  let cursor = skipJsonWhitespace(text, start + 1);
  if (text[cursor] === "}") {
    const end = cursor + 1;
    return { node: { kind: "object", start, end, members }, next: end };
  }
  while (cursor < text.length) {
    const keyEnd = jsonStringEnd(text, cursor);
    if (keyEnd < 0) {
      return null;
    }
    const keyText = parseJson(text.slice(cursor, keyEnd));
    if (!keyText.ok || typeof keyText.value !== "string") {
      return null;
    }
    cursor = skipJsonWhitespace(text, keyEnd);
    if (text[cursor] !== ":") {
      return null;
    }
    const value = parseRawJsonNode(text, cursor + 1);
    if (value === null) {
      return null;
    }
    members.push({ key: keyText.value, value: value.node });
    cursor = skipJsonWhitespace(text, value.next);
    if (text[cursor] === ",") {
      cursor = skipJsonWhitespace(text, cursor + 1);
      continue;
    }
    if (text[cursor] === "}") {
      const end = cursor + 1;
      return { node: { kind: "object", start, end, members }, next: end };
    }
    return null;
  }
  return null;
}

function parseRawJsonArray(text: string, start: number): ParsedRawJsonNode | null {
  const items: RawJsonNode[] = [];
  let cursor = skipJsonWhitespace(text, start + 1);
  if (text[cursor] === "]") {
    const end = cursor + 1;
    return { node: { kind: "array", start, end, items }, next: end };
  }
  while (cursor < text.length) {
    const value = parseRawJsonNode(text, cursor);
    if (value === null) {
      return null;
    }
    items.push(value.node);
    cursor = skipJsonWhitespace(text, value.next);
    if (text[cursor] === ",") {
      cursor = skipJsonWhitespace(text, cursor + 1);
      continue;
    }
    if (text[cursor] === "]") {
      const end = cursor + 1;
      return { node: { kind: "array", start, end, items }, next: end };
    }
    return null;
  }
  return null;
}

function skipJsonWhitespace(text: string, start: number): number {
  let index = start;
  while (index < text.length) {
    const character = text[index];
    if (character !== " " && character !== "\t" && character !== "\r" && character !== "\n") {
      break;
    }
    index += 1;
  }
  return index;
}

function jsonStringEnd(text: string, start: number): number {
  if (text[start] !== '"') {
    return -1;
  }
  let index = start + 1;
  while (index < text.length) {
    const character = text[index];
    if (character === "\\") {
      index += 2;
      continue;
    }
    if (character === '"') {
      return index + 1;
    }
    index += 1;
  }
  return -1;
}

/** 修改动作的固定字符串；命令重试必须沿用第一次提交时的值。 */
export type AdminLotUpdateAction = "withdraw" | "set_start_price";

/** `AdminLotUpdateAction::as_str` 的线上取值；写命令直接复用该字符串。 */
function adminLotUpdateActionAsStr(action: AdminLotUpdateAction): string {
  return action;
}

/** 只接受 2xx 响应，与 gloo_net 的 `response.ok()` 一致。 */
function responseOk(status: number): boolean {
  return status >= 200 && status <= 299;
}

/** 拉取 auction 下的 lot 安全投影；拍卖与 lot 必须来自同一数据库快照。 */
export async function fetchAdminLots(
  host: AdminSessionHost,
  auctionId: string,
  page: number,
  pageSize: number,
  signal: AbortSignal,
): Promise<Result<AdminLotPage, string>> {
  const bounds = validate(page, pageSize, 100, 100_000);
  if (!bounds.ok) {
    return err(bounds.error);
  }
  const url = `/api/v1/admin/auctions/${auctionId}/lots?page=${page}&page_size=${pageSize}`;
  const response = await sendGet(host, url, signal);
  if (!response.ok) {
    return err(response.error);
  }
  if (!responseOk(response.value.status)) {
    return err(await responseError(response.value, signal, host));
  }
  const manualIntake = response.value.headers.get("x-auction-capability-manual-intake") === "true";
  const receipt = decodeAdminReceipt(
    response.value,
    apiResponseSchema(AdminLotListResponseSchema),
    ADMIN_LOT_LIST_FIELD_POLICY,
  );
  if (!receipt.ok) {
    return err(receipt.error);
  }
  const auction = decodeAuction(receipt.value.data.auction);
  if (!auction.ok) {
    return err(invalidResponseError(auction));
  }
  const attributeSources = equipmentAttributeSources(response.value.text());
  const lots: AdminLot[] = [];
  for (const [index, candidate] of receipt.value.data.lots.entries()) {
    const filtered = decodeLot(candidate);
    if (!filtered.ok) {
      return err(invalidResponseError(filtered));
    }
    lots.push(filtered.value);
    const source = attributeSources?.[index] ?? null;
    if (source !== null) {
      rememberEquipmentAttributesSource(filtered.value.equipment_attributes, source);
    }
  }
  if (auction.value.id !== auctionId || lots.some((lot) => lot.auction_id !== auction.value.id)) {
    return err("invalid_response");
  }
  const pagination = receipt.value.data.pagination;
  if (
    !validPageEcho(
      page,
      pageSize,
      pagination.page,
      pagination.page_size,
      pagination.total,
      lots.length,
    )
  ) {
    return err(invalidResponseError("lot_pagination"));
  }
  return ok({
    auction: auction.value,
    lots,
    pagination,
    capabilities: { manualIntake },
  });
}

/** 提交 Admin draft lot 修改；调用方在同一输入重试时复用 `command_id`。 */
export async function updateAdminLot(
  host: AdminSessionHost,
  lotId: string,
  commandId: string,
  prepare: (commandId: string) => Result<PreparedJsonCommand, AdminWriteNotSent>,
  signal: AbortSignal,
): Promise<Result<AdminLot, AdminWriteFailure>> {
  // 路径段必须是规范 UUID；否则同一命令会改写不同资源或发出畸形 URL。
  if (!validUuid(lotId)) {
    return err(writeNotSent("invalid_request"));
  }
  const prepared = prepare(commandId);
  if (!prepared.ok) {
    return err(prepared.error);
  }
  const receipt = await sendAuctionEdit(
    host,
    "PATCH",
    `/api/v1/admin/lots/${lotId}`,
    prepared.value,
    apiResponseSchema(AdminLotUpdateResponseSchema),
    ADMIN_LOT_UPDATE_FIELD_POLICY,
    signal,
  );
  if (!receipt.ok) {
    return err(receipt.error);
  }
  const lot = decodeLot(receipt.value.data.lot);
  if (!lot.ok) {
    return err(writeOutcomeUnknown(invalidResponseError(lot)));
  }
  if (lot.value.id !== lotId || lot.value.state_version <= 0) {
    return err(writeOutcomeUnknown("invalid_response"));
  }
  return ok(lot.value);
}

/** 跨字段谓词保留为独立校验：slow/提交字段组合与值域仍在浏览器边界 fail-closed。 */
function decodeLot(value: AdminLot): Result<AdminLot, "lot"> {
  if (
    !validUuid(value.id) ||
    !validUuid(value.auction_id) ||
    !validUuid(value.equipment_id) ||
    !validOptionalUuid(value.submission_id) ||
    value.lot_number <= 0 ||
    (value.status !== "pending" &&
      value.status !== "live" &&
      value.status !== "sold" &&
      value.status !== "unsold" &&
      value.status !== "withdrawn") ||
    value.start_price < 1 ||
    value.start_price > MAX_JSON_SAFE_INTEGER ||
    value.current_price < value.start_price ||
    value.current_price > MAX_JSON_SAFE_INTEGER ||
    !validOptionalUuid(value.leader_user_id) ||
    !validOptionalTime(value.effective_end_at) ||
    !validOptionalTime(value.slow_due_at) ||
    !validOptionalUuid(value.slow_authorization_id) ||
    value.slow_generation < 0 ||
    value.slow_generation > MAX_JSON_SAFE_INTEGER ||
    value.state_version < 1 ||
    value.state_version > MAX_JSON_SAFE_INTEGER ||
    !validAdminTime(value.created_at) ||
    !validAdminTime(value.updated_at) ||
    !validTrimmedText(value.external_equipment_id) ||
    !validTrimmedText(value.equipment_name) ||
    !validTrimmedText(value.equipment_category) ||
    !isJsonObject(value.equipment_attributes) ||
    !validAdminTime(value.equipment_collected_at) ||
    !validOptionalText(value.seller) ||
    !validOptionalText(value.mailbox_key) ||
    !validOptionalText(value.external_mail_id) ||
    !validOptionalTime(value.submission_received_at) ||
    (value.submission_source_kind !== null &&
      value.submission_source_kind !== "admin" &&
      value.submission_source_kind !== "mail") ||
    !validOptionalText(value.submission_source_reference) ||
    ((value.status === "live" || value.status === "sold") && value.effective_end_at === null) ||
    (value.slow_due_at === null) !== (value.slow_authorization_id === null) ||
    (value.slow_due_at !== null && (value.status !== "live" || value.slow_generation <= 0)) ||
    !validSubmissionFields(value)
  ) {
    return err("lot");
  }
  return ok(value);
}

function validSubmissionFields(value: AdminLot): boolean {
  if (value.submission_id === null) {
    return [
      value.seller,
      value.submission_source_kind,
      value.submission_source_reference,
      value.mailbox_key,
      value.external_mail_id,
      value.submission_received_at,
    ].every((item) => item === null);
  }
  if (
    value.seller === null ||
    value.submission_source_kind === null ||
    value.submission_received_at === null
  ) {
    return false;
  }
  if (value.submission_source_kind === "mail") {
    return (
      value.submission_source_reference === null &&
      value.mailbox_key !== null &&
      value.external_mail_id !== null
    );
  }
  return (
    value.submission_source_reference !== null &&
    value.mailbox_key === null &&
    value.external_mail_id === null
  );
}

function validOptionalUuid(value: string | null): boolean {
  return value === null || validUuid(value);
}

function isJsonObject(value: unknown): boolean {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/** 构造并固定一次写命令；同一 `command_id` 的所有重试发送同一份正文。 */
export function prepareAdminLotUpdate(
  commandId: string,
  expectedStateVersion: number,
  action: AdminLotUpdateAction,
  startPrice: number | null,
): AdminLotUpdateRequest {
  const command: AdminLotUpdateRequest = {
    command_id: commandId,
    expected_state_version: expectedStateVersion,
    action: adminLotUpdateActionAsStr(action),
  };
  // Rust 在 `start_price = None` 时省略该键；不能发送显式 undefined。
  if (startPrice !== null) {
    command.start_price = startPrice;
  }
  return command;
}

/** 冻结字段后得到的正文工厂；同一 `command_id` 的重试不会重新拼装命令。 */
export function prepareAdminLotUpdateOnce(
  expectedStateVersion: number,
  action: AdminLotUpdateAction,
  startPrice: number | null,
): (commandId: string) => Result<PreparedJsonCommand, AdminWriteNotSent> {
  return prepareJsonCommandOnce((commandId) =>
    prepareAdminLotUpdate(commandId, expectedStateVersion, action, startPrice),
  );
}
