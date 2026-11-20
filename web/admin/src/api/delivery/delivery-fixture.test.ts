// Admin 投递接口迁移测试的共享夹具。
// 这里的构造器与判定逐字对应 Rust `delivery/tests.rs` 的辅助函数；只用于测试，
// 生产模块不依赖本文件。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import {
  AdminDeliveryCommandResponseSchema,
  type AdminDeliveryDetailResponse,
  AdminDeliveryDetailResponseSchema,
  type AdminDeliveryDetailWithActionsResponse,
  AdminDeliveryDetailWithActionsResponseSchema,
  AdminDeliveryListResponseSchema,
} from "@auction/contracts/dto/admin/delivery";
import { type Decoded, decode, optionalNullable, strictStruct } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { utf8ByteLength } from "@auction/web-shared/text";
import { validUuid } from "@auction/web-shared/ui/validation";
import { boolean, integer, maxValue, minValue, number, optional, pipe, string } from "valibot";

import { parseStrictRfc3339 } from "../session-time";
import { validOptionalText, validTrimmedText } from "../validation";
import {
  decodeDeliveryList,
  type StrictDeliveryEnvelope,
  strictDeliveryEnvelopeSchema,
} from "./decode";
import { decodeDetailWithActions } from "./responses";

/** 测试用的 JSON 对象；Rust 侧对应 `serde_json::Value` 的对象分支。 */
export type JsonRecord = Record<string, unknown>;

/** Rust 测试常量 `DELIVERY_ID`。 */
export const DELIVERY_ID = "0f8fad5b-d9cb-469f-a165-70867728950e";
/** Rust 测试常量 `OTHER_ID`。 */
export const OTHER_ID = "2c9e63a1-7f4b-4a0e-b3d5-9f8c1e2d4b60";

const ADMIN_FIXTURE_DIR = new URL(
  "../../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);
const DELIVERY_FIXTURE_DIR = new URL(
  "../../../../../crates/auction-web-admin/src/api/delivery/",
  import.meta.url,
);

/** Rust `time(offset)` 使用带 UTC 偏移的 RFC 3339 时间。 */
export function time(offset: number): string {
  return `2026-11-02T01:${String(offset).padStart(2, "0")}:00+00:00`;
}

/** Rust `retired_before_send` 的围栏前退役投影。 */
export function retiredBeforeSend(
  originKind: string,
  reasonCode: string,
  failureCode: string,
): JsonRecord {
  return {
    id: DELIVERY_ID,
    origin_kind: originKind,
    origin_id: `${originKind}:1:alice`,
    recipient: "alice",
    cod: 0,
    status: "failed",
    world: "main",
    retirement_reason: reasonCode,
    lease_expires_at: null,
    fencing_version: 1,
    attempt_count: 0,
    claimed_at: null,
    sending_at: null,
    completed_at: "2026-11-06T10:00:05.000000Z",
    external_reference: null,
    failure_code: failureCode,
    reason_code: reasonCode,
    created_at: "2026-11-06T10:00:00.000000Z",
    updated_at: "2026-11-06T10:00:05.000000Z",
  };
}

/** Rust `preparation_failure`：发送前准备失败并带抑制原因。 */
export function preparationFailure(failureCode: string): JsonRecord {
  const failed = delivery();
  failed.status = "failed";
  failed.sending_at = null;
  failed.failure_code = failureCode;
  failed.suppression_reason = failureCode;
  return failed;
}

/** Rust `delivery()` 的规范成功投影。 */
export function delivery(): JsonRecord {
  return {
    id: DELIVERY_ID,
    origin_kind: "settlement",
    origin_id: "42",
    recipient: "Sample",
    cod: 0,
    status: "manual_reconcile",
    template: "settlement_seller_payout",
    category: "settlement",
    world: "main",
    lease_expires_at: null,
    fencing_version: 3,
    attempt_count: 1,
    claimed_at: time(0),
    sending_at: time(0),
    completed_at: time(1),
    external_reference: null,
    failure_code: "write_unknown",
    created_at: time(0),
    updated_at: time(1),
  };
}

/** Rust `attempt()` 的冻结 Send 尝试。 */
export function attempt(): JsonRecord {
  return {
    operation: "send",
    fencing_version: 3,
    world_id: "main",
    sending_at: time(0),
    recipient: "Sample",
    subject: "Auction settlement",
    body_digest: "ab".repeat(32),
    credits: 300_000,
    cod: 0,
  };
}

/** Rust `detail()` 的完整详情。 */
export function detail() {
  const dispositions: JsonRecord[] = [];
  return {
    delivery: delivery(),
    attempts: [attempt()],
    evidence: [evidence()],
    custody: {
      collected: false,
      cod_pending: 0,
      remaining_credits: 300_000,
      observed_at: time(2),
    },
    dispositions,
  };
}

/** Rust `detail()` 中的 Sent 观察证据。 */
function evidence(): JsonRecord {
  return {
    reference: "mailbox_sweep:7000001",
    kind: "mailbox_sweep",
    external_mail_id: "7000001",
    observed_at: time(2),
    recipient: "Sample",
    subject: "Auction settlement",
    body_digest: "cd".repeat(32),
    collected: false,
    cod_pending: 0,
    remaining_credits: 300_000,
    remaining_equipment_count: 0,
  };
}

// Solid 的详情校验入口还需要准入参数，普通详情先单独验证其线上形状。
const ELIGIBLE_ACTIONS: AdminDeliveryDetailWithActionsResponse["actions"] = {
  release: { state: "eligible" },
  retry_failed: { state: "eligible" },
  reconcile_retry: { state: "eligible" },
  reconcile_succeeded: { state: "eligible" },
  reconcile_failed: { state: "eligible" },
  acknowledge: { state: "eligible" },
};

/** 普通详情线上信封的严格 schema。 */
export const PLAIN_DETAIL_ENVELOPE = strictDeliveryEnvelopeSchema(
  AdminDeliveryDetailResponseSchema,
);

/** 读取 `auction-contracts/tests/fixtures/admin` 下的 JSON 文本。 */
export function readAdminFixture(name: string): string {
  return readFileSync(new URL(`${name}.json`, ADMIN_FIXTURE_DIR), "utf8");
}

/** 读取 `auction-web-admin/src/api/delivery` 下的 JSON 文本。 */
export function readDeliveryFixture(name: string): string {
  return readFileSync(new URL(`${name}.json`, DELIVERY_FIXTURE_DIR), "utf8");
}

/** 严格解析测试样本 JSON；样本损坏时直接失败，避免测试静默跳过。 */
export function parseFixture(text: string): unknown {
  const parsed = parseJson(text);
  if (!parsed.ok) {
    throw new Error("测试样本不是合法 JSON");
  }
  return parsed.value;
}

/** 按 Rust 的 `unwrap` 要求测试前提成立，失败时立即指出该步骤。 */
export function requireDecoded<T>(result: Decoded<T>, label: string): T {
  if (!result.ok) {
    throw new Error(`${label} 应通过`);
  }
  return result.value;
}

/** 逐层确认原始 JSON 的对象形状，供单字段修改使用。 */
export function jsonObject(value: unknown, label: string): JsonRecord {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`${label} 必须是 JSON 对象`);
  }
  // 输入来自 JSON 或手写夹具，且已确认是非 null 的非数组对象。
  return value as JsonRecord;
}

/** 按 Rust 的下标访问要求夹具中存在该元素。 */
export function requireElement<T>(values: readonly T[], index: number, label: string): T {
  const value = values[index];
  if (value === undefined) {
    throw new Error(`${label} 缺少第 ${index + 1} 项`);
  }
  return value;
}

/** 克隆样本并删除指定的已有字段，保持其他字段原样。 */
export function withoutField(value: unknown, ...path: [string, ...string[]]): unknown {
  const copy = structuredClone(value);
  const key = path.pop();
  const parent = path.reduce<unknown>((current, field) => jsonObject(current, field)[field], copy);
  const object = jsonObject(parent, path.join("/"));
  if (key === undefined || !Object.hasOwn(object, key)) {
    throw new Error(`夹具缺少字段 ${[...path, key].join("/")}`);
  }
  delete object[key];
  return copy;
}

/** 带处置准入的严格信封 schema，对应 Rust 的 serde 边界。 */
export const DETAIL_WITH_ACTIONS_ENVELOPE = strictDeliveryEnvelopeSchema(
  AdminDeliveryDetailWithActionsResponseSchema,
);

/** 投递命令回执的严格信封 schema。 */
export const DELIVERY_COMMAND_ENVELOPE = strictDeliveryEnvelopeSchema(
  AdminDeliveryCommandResponseSchema,
);

/** 保留普通详情信封的 code/message，并调用生产详情校验。 */
export function decodePlainDetail(envelope: StrictDeliveryEnvelope<AdminDeliveryDetailResponse>) {
  return decodeDetailWithActions(
    {
      ...envelope,
      data: { detail: envelope.data, actions: ELIGIBLE_ACTIONS },
    },
    envelope.data.delivery.id,
    false,
  );
}

/** 构造 Rust 手写详情测试使用的信封，要求 DTO 解码成功后才测试详情校验。 */
export function decodeDetailFixture(detailValue: unknown) {
  const envelope = requireDecoded(
    decode(PLAIN_DETAIL_ENVELOPE, { code: "ok", message: "loaded", data: detailValue }),
    "普通详情信封",
  );
  return decodePlainDetail(envelope);
}

/** Rust `check_before_send_projection`：同一个投影必须同时通过列表与详情解码。 */
export function checkBeforeSendProjection(deliveryValue: unknown): {
  readonly list: boolean;
  readonly detail: boolean;
} {
  const listEnvelope = requireDecoded(
    decode(strictDeliveryEnvelopeSchema(AdminDeliveryListResponseSchema), {
      code: "ok",
      message: "loaded",
      data: {
        deliveries: [deliveryValue],
        pagination: { page: 1, page_size: 10, total: 1 },
      },
    }),
    "投递列表信封",
  );
  return {
    list: decodeDeliveryList(listEnvelope, 1, 10).ok,
    detail: decodeDetailFixture({ ...detail(), delivery: deliveryValue, attempts: [] }).ok,
  };
}

// Rust DTO 的 i64 反序列化接受 2^53，须让 v0.1.8 的谓词亲自拒绝该边界输入。
const FIXTURE_INTEGER = pipe(number(), integer());
const V018_ROW_SCHEMA = strictStruct({
  id: string(),
  origin_kind: string(),
  origin_id: string(),
  recipient: string(),
  status: string(),
  template: optionalNullable(string()),
  category: optionalNullable(string()),
  world: optionalNullable(string()),
  cod: optionalNullable(FIXTURE_INTEGER),
  retirement_reason: optionalNullable(string()),
  suppression_reason: optionalNullable(string()),
  reason_code: optionalNullable(string()),
  returned_mail_id: optionalNullable(string()),
  lease_expires_at: optionalNullable(string()),
  external_reference: optionalNullable(string()),
  fencing_version: FIXTURE_INTEGER,
  attempt_count: pipe(FIXTURE_INTEGER, minValue(0), maxValue(4_294_967_295)),
  claimed_at: optionalNullable(string()),
  sending_at: optionalNullable(string()),
  completed_at: optionalNullable(string()),
  failure_code: optionalNullable(string()),
  take_required: optional(boolean(), false),
  created_at: string(),
  updated_at: string(),
});

const V018_CATEGORIES = new Set([
  "outbid",
  "account",
  "intake",
  "settlement",
  "equipment",
  "operations",
  "manual",
]);
const V018_ZERO_TIME = -62_135_596_800n * 1_000_000_000n;
const V018_JSON_MAX_SAFE_INTEGER = 9_007_199_254_740_991;

function v018Time(value: string | null): bigint | null {
  return value === null ? null : parseStrictRfc3339(value);
}

/**
 * 摘录 Rust `v018_accepts_mail_intake_failure`，用于核对旧版列表兼容边界。
 * 该函数直接消费原始 JSON，不接受当前 schema 的额外收紧。
 */
export function v018AcceptsMailIntakeFailure(input: unknown): boolean {
  const row = requireDecoded(decode(V018_ROW_SCHEMA, input), "v0.1.8 投递 DTO");
  const created = v018Time(row.created_at);
  const updated = v018Time(row.updated_at);
  const claimed = v018Time(row.claimed_at);
  const completed = v018Time(row.completed_at);
  if (created === null || updated === null || claimed === null || completed === null) {
    return false;
  }
  if (
    !validUuid(row.id) ||
    row.origin_kind !== "mail_intake" ||
    row.status !== "failed" ||
    !validTrimmedText(row.origin_id) ||
    !validTrimmedText(row.recipient) ||
    !validOptionalText(row.template) ||
    (row.template !== null && utf8ByteLength(row.template) > 64) ||
    (row.category !== null && !V018_CATEGORIES.has(row.category)) ||
    (row.world !== null && row.world !== "main" && row.world !== "isekai") ||
    (row.cod !== null && !(row.cod >= 0 && row.cod <= V018_JSON_MAX_SAFE_INTEGER)) ||
    !validOptionalText(row.retirement_reason) ||
    !validOptionalText(row.suppression_reason) ||
    !validOptionalText(row.reason_code) ||
    (row.returned_mail_id !== null &&
      (row.returned_mail_id === "" ||
        utf8ByteLength(row.returned_mail_id) > 32 ||
        /[^0-9]/.test(row.returned_mail_id))) ||
    row.lease_expires_at !== null ||
    row.external_reference !== null ||
    !(row.fencing_version >= 1 && row.fencing_version <= V018_JSON_MAX_SAFE_INTEGER) ||
    row.attempt_count === 0 ||
    created === V018_ZERO_TIME ||
    updated === V018_ZERO_TIME ||
    updated < created ||
    claimed < created ||
    row.reason_code === "lot_watch_invalidated" ||
    row.reason_code === "recipient_renamed" ||
    row.reason_code === "mail_disposition_conflict" ||
    row.failure_code === "recovery_superseded"
  ) {
    return false;
  }
  if (row.sending_at === null && row.failure_code === "invalid_payload") {
    return (
      row.reason_code === null &&
      claimed !== V018_ZERO_TIME &&
      completed !== V018_ZERO_TIME &&
      completed >= claimed &&
      completed <= updated &&
      !row.take_required &&
      row.returned_mail_id === null
    );
  }
  if (typeof row.failure_code !== "string" || !validTrimmedText(row.failure_code)) {
    return false;
  }
  const sending = v018Time(row.sending_at);
  return sending !== null && sending >= claimed && completed >= sending;
}

test("投递负例夹具克隆嵌套样本，只删除实际存在的目标字段", () => {
  const sample = { data: { delivery: { status: "failed", fencing_version: 3 } } };
  expect(withoutField(sample, "data", "delivery", "fencing_version")).toEqual({
    data: { delivery: { status: "failed" } },
  });
  expect(sample.data.delivery.fencing_version).toBe(3);
  expect(() => withoutField(sample, "data", "delivery", "missing")).toThrow(
    "data/delivery/missing",
  );
});
