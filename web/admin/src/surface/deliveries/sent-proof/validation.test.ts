// 对应 Rust `sent_proof/tests.rs` 中的草稿组装与发送分钟校验。

import { describe, expect, test } from "bun:test";
import { utf8ByteLength } from "@auction/web-shared/text";

import { withTestTimezone } from "../delivery-dom-fixture.test";
import { selectableEvidence } from "./evidence";
import { builtEvidence, detail, evidence, filledDraft, firstOf } from "./sent-proof-fixture";
import { sentProofDraftForEvidence, sentProofIssueFieldId } from "./state";
import { buildEvidence, sentMinuteFromInput, sentMinuteFromTimestamp } from "./validation";

describe("Sent 草稿校验（对应 Rust sent_proof/tests.rs）", () => {
  test("normalizes_sent_minute_to_utc_zero_seconds", () =>
    withTestTimezone("UTC", () => {
      expect(sentMinuteFromInput("2026-11-02T01:14")).toEqual({
        ok: true,
        value: "2026-11-02T01:14:00Z",
      });
      expect(sentMinuteFromInput("")).toEqual({ ok: false, error: "invalid" });
      expect(sentMinuteFromTimestamp("2026-11-02T01:14:37Z")).toBeNull();
      expect(sentMinuteFromTimestamp("2026-11-02T01:14:00.000000Z")).toBe(
        "2026-11-02T01:14:00.000000Z",
      );
    }));

  test("untouched_prefill_submits_observed_utc_minute", () =>
    withTestTimezone("UTC", () => {
      const value = detail();
      firstOf(value.evidence).sent_at = "2027-11-07T09:30:00Z";
      const draft = {
        ...filledDraft(value),
        sentMinute: "2027-11-07T08:30",
      };
      const unedited = builtEvidence(value, draft);
      expect(unedited.sent_minute).toBe("2027-11-07T09:30:00Z");

      const edited = builtEvidence(value, { ...draft, sentMinuteEdited: true });
      expect(edited.sent_minute).toBe("2027-11-07T08:30:00Z");

      expect(buildEvidence(value, { ...draft, sentMinuteEdited: true, sentMinute: "" })).toEqual({
        ok: false,
        error: { kind: "sent-minute", error: "invalid" },
      });
    }));

  test("builds_evidence_from_frozen_attempt_and_observation", () => {
    const value = detail();
    const built = builtEvidence(value);
    expect(built.schema_version).toBe(1);
    expect(built.world_id).toBe("main");
    expect(built.external_mail_id).toBe("7000001");
    expect(built.sender_uid).toBe("12345");
    expect(built.recipient).toBe("Sample");
    expect(built.sent_minute).toBe("2026-11-02T01:01:00+00:00");
    expect([...built.original_body_digest]).toEqual(Array.from({ length: 32 }, () => 0xab));
    expect([...built.observed_body_digest]).toEqual(Array.from({ length: 32 }, () => 0xcd));
    expect(built.remaining_credits).toBe(300_000);
    expect(built.equipment_ids).toEqual([]);
  });

  test("rejects_draft_without_recorded_evidence_or_sender", () => {
    const value = detail();
    expect(
      buildEvidence(value, { ...filledDraft(value), evidenceReference: "mailbox_sweep:999" }),
    ).toEqual({ ok: false, error: { kind: "evidence" } });

    expect(buildEvidence(value, { ...filledDraft(value), senderUid: "0" })).toEqual({
      ok: false,
      error: { kind: "sender-uid" },
    });

    expect(buildEvidence(value, { ...filledDraft(value), senderName: "  " })).toEqual({
      ok: false,
      error: { kind: "sender-name" },
    });
  });

  test("rejects_unselectable_kind_and_missing_digest", () => {
    const unknownKind = detail();
    firstOf(unknownKind.evidence).kind = "compose_discard";
    expect(selectableEvidence(unknownKind)).toEqual([]);
    expect(buildEvidence(unknownKind, filledDraft(detail()))).toEqual({
      ok: false,
      error: { kind: "evidence" },
    });

    const missingDigest = detail();
    firstOf(missingDigest.evidence).body_digest = null;
    expect(buildEvidence(missingDigest, filledDraft(detail()))).toEqual({
      ok: false,
      error: { kind: "digest" },
    });
  });

  test("draft_uses_observed_equipment_ids_only_while_uncollected", () => {
    const value = detail();
    firstOf(value.attempts).credits = 0;
    firstOf(value.attempts).cod = 0;
    firstOf(value.attempts).equipment_ids = ["300753957"];
    firstOf(value.evidence).remaining_equipment_count = 1;
    firstOf(value.evidence).remaining_equipment_ids = ["300753958"];

    const uncollected = builtEvidence(value, filledDraft(value));
    expect(uncollected.equipment_ids).toEqual(["300753958"]);

    const collected = builtEvidence(value, { ...filledDraft(value), collected: true });
    expect(collected.collected).toBe(true);
    expect(collected.equipment_ids).toEqual([]);
  });

  test("uncollected_observation_with_no_ids_cannot_preview", () => {
    const value = detail();
    firstOf(value.attempts).equipment_ids = ["300753957"];
    firstOf(value.evidence).remaining_equipment_ids = null;
    const draft = filledDraft(value);
    expect(buildEvidence(value, draft)).toEqual({
      ok: false,
      error: { kind: "missing-observed-equipment-ids" },
    });
    expect(buildEvidence(value, { ...draft, collected: true })).toEqual({
      ok: false,
      error: { kind: "missing-observed-equipment-ids" },
    });

    firstOf(value.evidence).collected = true;
    const historical = builtEvidence(value, { ...draft, collected: true });
    expect(historical.equipment_ids).toEqual([]);
  });

  test("only_collected_proof_validates_and_submits_the_receipt", () => {
    const value = detail();
    const draft = { ...filledDraft(value), collected: true, receipt: "x".repeat(2049) };
    expect(buildEvidence(value, draft)).toEqual({ ok: false, error: { kind: "receipt" } });

    const hidden = builtEvidence(value, { ...draft, collected: false });
    expect(hidden.receipt).toBe("");

    const receipt = " Removed attachment: 300,000 Credits ";
    const uncollected = builtEvidence(value, { ...draft, collected: false, receipt });
    expect(uncollected.receipt).toBe("");

    const collected = builtEvidence(value, { ...draft, collected: true, receipt });
    expect(collected.receipt).toBe("Removed attachment: 300,000 Credits");
  });

  test("invalid_preview_fields_use_their_own_error_locations", () => {
    const value = detail();
    const invalidUid = buildEvidence(value, { ...filledDraft(value), senderUid: "0" });
    if (invalidUid.ok) throw new Error("无效 UID 被接受");
    expect(sentProofIssueFieldId(invalidUid.error)).toBe("delivery-sent-sender-uid");

    const invalidCod = buildEvidence(value, {
      ...filledDraft(value),
      senderUid: "12345",
      codPending: "-1",
    });
    if (invalidCod.ok) throw new Error("无效 COD 被接受");
    expect(sentProofIssueFieldId(invalidCod.error)).toBe("delivery-sent-cod-pending");

    expect(
      buildEvidence(value, { ...filledDraft(value), senderUid: "12345", codPending: "0" }).ok,
    ).toBe(true);
  });

  test("prefilled_sent_minute_is_a_local_datetime_value_without_offset", () => {
    const draft = sentProofDraftForEvidence(evidence());
    expect(utf8ByteLength(draft.sentMinute)).toBe(16);
    expect(draft.sentMinute).toMatch(/^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}$/u);
    expect(draft.sentMinuteEdited).toBe(false);
  });
});
