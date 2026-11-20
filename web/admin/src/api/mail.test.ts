// 将 Rust 邮件 API 的回执、入站解码与生产样本用例迁到同一层入口。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import {
  AdminMailIntakeDetailResponseSchema,
  AdminMailIntakeListResponseSchema,
  AdminMailIntakeSummaryResponseSchema,
} from "@auction/contracts/dto/admin/mail";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { parseJson } from "@auction/web-shared/net/wire";
import { utf8ByteLength } from "@auction/web-shared/text";
import { removeAt } from "./api-request-fixture";
import { ADMIN_MAIL_INTAKE_KIND_VALUES, adminMailIntakeQueryDefault } from "./mail";
import {
  acceptedPollers,
  decodeMailIntakeDetail,
  decodeMailIntakePage,
  decodeMailState,
  StrictMailStateSchema,
  validMailIntake,
} from "./mail-response";

const FIXTURES = new URL(
  "../../../../crates/auction-contracts/tests/fixtures/admin/",
  import.meta.url,
);
const KINDS = ADMIN_MAIL_INTAKE_KIND_VALUES;

function fixture(name: string): unknown {
  return requiredValue(parseJson(readFileSync(new URL(`${name}.json`, FIXTURES), "utf8")));
}

function requiredValue<T>(result: { ok: true; value: T } | { ok: false }): T {
  if (!result.ok) throw new Error("邮件样本应解码成功");
  return result.value;
}

const PAGE_ENVELOPE = apiResponseSchema(AdminMailIntakeListResponseSchema);
const DETAIL_ENVELOPE = apiResponseSchema(AdminMailIntakeDetailResponseSchema);

test("poll_trigger_uses_the_count_from_the_accepted_receipt", () => {
  const id = "0f8fad5b-d9cb-469f-a165-70867728950e";
  const payload = {
    code: "accepted",
    message: "邮件轮询已触发",
    data: { command_id: id, pollers: 3 },
  };
  expect(acceptedPollers(payload, id)).toBe(3);
  expect(acceptedPollers(payload, "00000000-0000-0000-0000-000000000000")).toBeNull();
  expect(acceptedPollers({ ...payload, code: "ok" }, id)).toBeNull();
  expect(acceptedPollers({ ...payload, data: { ...payload.data, pollers: 0 } }, id)).toBeNull();
});

test("mail_intake_accepts_rekey_and_rejects_unrecognized_kind", () => {
  const intake = requiredValue(
    decode(AdminMailIntakeSummaryResponseSchema, {
      id: "0f8fad5b-d9cb-469f-a165-70867728950e",
      mailbox_key: "main-mailbox",
      external_mail_id: "1234",
      world_id: "main",
      kind: "rekey",
      status: "accepted",
      decision_code: "rekeyed",
      received_at: "2026-11-06T10:00:00Z",
      processed_at: "2026-11-06T10:00:01Z",
      has_message_display: false,
    }),
  );
  expect(validMailIntake(intake, KINDS)).toBe(true);
  expect(validMailIntake({ ...intake, kind: "other_kind" }, KINDS)).toBe(false);
});

test("rekey_reads_decode_backend_contract_fixtures", () => {
  const cases: readonly [string, string, number][] = [
    ["mail-intake", "", 2],
    ["mail-intake-rekey", "rekey", 1],
  ];
  for (const [name, kind, count] of cases) {
    const payload = requiredValue(decode(PAGE_ENVELOPE, fixture(name)));
    const query = { ...adminMailIntakeQueryDefault(), kind };
    const page = requiredValue(decodeMailIntakePage(payload.data, query, 1, 50, KINDS));
    expect(page.intakes).toHaveLength(count);
    expect(page.intakes[0]).toMatchObject({ kind: "rekey" });
  }
  const payload = requiredValue(decode(DETAIL_ENVELOPE, fixture("mail-intake-detail")));
  const detail = requiredValue(decodeMailIntakeDetail(payload.data, KINDS));
  expect(detail.kind).toBe("rekey");
  expect(detail.subject).toBeNull();
});

test("legacy_mail_intake_page_decodes_projected_text_at_the_byte_limit", () => {
  const payload = requiredValue(decode(PAGE_ENVELOPE, fixture("mail-intake-legacy")));
  const query = adminMailIntakeQueryDefault();
  const page = requiredValue(decodeMailIntakePage(payload.data, query, 1, 20, KINDS));
  expect(page.intakes).toHaveLength(9);
  const row = page.intakes.find((item) => item.external_mail_id === "9026");
  if (row?.subject == null) throw new Error("缺少边界主题");
  const subject = row.subject;
  expect(utf8ByteLength(subject)).toBe(512);
  expect(subject.endsWith(" login key[REDACTED]")).toBe(true);
  const invalid = {
    ...page,
    intakes: page.intakes.map((item) => (item === row ? { ...row, subject: `${subject}x` } : item)),
  };
  expect(decodeMailIntakePage(invalid, query, 1, 20, KINDS)).toEqual({
    ok: false,
    error: "mail_intake",
  });
});

test("legacy_mail_intake_detail_decodes_projected_text_and_rejects_control_text", () => {
  const fixtures = fixture("mail-intake-legacy-detail");
  if (!Array.isArray(fixtures) || fixtures.length !== 4) throw new Error("应有四份历史详情样本");
  const [redacted, partial, atLimit, trimmed] = fixtures.map((item: unknown) =>
    requiredValue(decodeMailIntakeDetail(requiredValue(decode(DETAIL_ENVELOPE, item)).data, KINDS)),
  );
  if (
    redacted === undefined ||
    partial === undefined ||
    atLimit?.subject == null ||
    trimmed === undefined
  ) {
    throw new Error("缺少历史详情或边界主题");
  }
  expect([
    redacted.mailbox_key,
    redacted.external_mail_id,
    redacted.world_id,
    redacted.decision_code,
    redacted.sender,
    redacted.subject,
  ]).toEqual(Array.from({ length: 6 }, () => "[REDACTED]"));
  expect(partial.subject).toBe("补充说明：登录 key[REDACTED]");
  expect(utf8ByteLength(atLimit.subject)).toBe(512);
  expect(atLimit.subject.endsWith(" login key[REDACTED]")).toBe(true);
  expect(trimmed.sender).toBe("Player");
  expect(trimmed.subject).toBe("拍卖问题");
  expect(decodeMailIntakeDetail({ ...redacted, subject: "多行\n主题" }, KINDS)).toEqual({
    ok: false,
    error: "mail_intake",
  });
  expect(decodeMailIntakeDetail({ ...redacted, external_mail_id: "9001\t9002" }, KINDS)).toEqual({
    ok: false,
    error: "mail_intake",
  });
});

test("mail_poll_contract_fixtures", () => {
  const schema = apiResponseSchema(StrictMailStateSchema);
  const cases: readonly [string, boolean][] = [
    ["mail", false],
    ["mail-diagnostics", true],
  ];
  for (const [name, hasDiagnostics] of cases) {
    const wire = fixture(name);
    const payload = requiredValue(decode(schema, wire));
    const invalid = structuredClone(wire);
    expect(removeAt(invalid, "/data/polling/paused")).toBe(false);
    expect(decode(schema, invalid).ok).toBe(false);
    const polling = requiredValue(decodeMailState(payload.data));
    expect(polling.mailbox_key).toBe("main");
    expect(polling.paused).toBe(false);
    expect(polling.state_version).toBe(3);
    if (hasDiagnostics) {
      expect(polling.diagnostics).toMatchObject({
        completed_external_mail_id: "9002",
        progress: null,
      });
    } else {
      expect(polling.diagnostics).toBeNull();
    }
  }
});
