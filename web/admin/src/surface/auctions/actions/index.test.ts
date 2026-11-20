// 对照 Rust 场次排期与生命周期对话框的失败、重读和新命令时序。

import { expect, test } from "bun:test";
import { withUtc } from "../auction-time-fixture.test";
import {
  type CapturedWrite,
  commandId,
  DIALOG_TITLE_CLOSE,
  editField,
  submitForm,
  takeWrite,
} from "../editor-fixture.test";
import {
  AUCTION_ID,
  acceptAuctionCommand,
  assertAuctionConflict,
  assertScheduleBody,
  auctionWire,
  closeAfterAuctionRead,
  openAuction,
  refreshAuctionDraft,
  withAuctionActions,
} from "./auction-actions-fixture.test";

const CLOSE_CONFLICT = "关闭";
const SCHEDULE = "修改拍卖时间";
const SAVE_SCHEDULE = "保存拍卖时间";
const END = "schedule-scheduled_end_at";
const EDITED_END = "2099-04-01T14:17";
const CANCEL = "取消拍卖";
const CONFIRM_CANCEL = "确认取消拍卖";
const REASON = "action-reason";
const EDITED_REASON = "保留待核对的取消原因";

test("schedule_draft_survives_failed_refresh_and_successful_retry", () =>
  withUtc(() =>
    withAuctionActions((context) =>
      refreshAuctionDraft(context, SCHEDULE, END, "2099-04-01T13:00", EDITED_END, SAVE_SCHEDULE),
    ),
  ));

test("lifecycle_reason_survives_failed_refresh_and_successful_retry", () =>
  withAuctionActions((context) =>
    refreshAuctionDraft(context, CANCEL, REASON, "", EDITED_REASON, CONFIRM_CANCEL),
  ));

test("stale_schedule_closes_and_reopens_with_the_latest_schedule", () =>
  withUtc(async () => {
    for (const closeLabel of [DIALOG_TITLE_CLOSE, CLOSE_CONFLICT]) {
      await withAuctionActions(async (context) => {
        const { page } = context;
        const { id, form } = await openAuction(context, SCHEDULE, END, "2099-04-01T13:00");
        await editField(page.field(id), EDITED_END);
        page.conflict("stale_auction_state");
        await submitForm(form);
        const first = takeWrite(page);
        assertScheduleBody(first, 7, "11:00:00", "12:00:00", "14:17:00");
        assertAuctionConflict(context, id, EDITED_END, SAVE_SCHEDULE);
        await submitForm(form);
        expect(page.takeWrites()).toEqual([]);

        const latest = {
          ...auctionWire(),
          state_version: 8,
          preview_scheduled_at: "2099-04-01T11:30:08.123456Z",
          scheduled_open_at: "2099-04-01T12:30:01Z",
          scheduled_ends_at: "2099-04-01T13:30:00Z",
        };
        await closeAfterAuctionRead(context, closeLabel, SCHEDULE, latest);
        const next = await openAuction(context, SCHEDULE, END, "2099-04-01T13:30");
        await editField(page.field(next.id), "2099-04-01T15:00");
        const second = await acceptAuctionCommand(context, next.form, {
          ...latest,
          state_version: 9,
          scheduled_ends_at: "2099-04-01T15:00:00Z",
        });
        assertScheduleBody(second, 8, "11:30:08.123456", "12:30:01", "15:00:00");
        expect(commandId(first)).not.toBe(commandId(second));
      });
    }
  }));

function assertCancelBody(request: CapturedWrite, version: number, reason: string): void {
  const id = commandId(request);
  expect(request).toEqual({
    method: "POST",
    path: `/api/v1/admin/auctions/${AUCTION_ID}/cancel`,
    body: `{"command_id":"${id}","expected_state_version":${version},"reason":"${reason}"}`,
  });
}

test("stale_lifecycle_confirmation_closes_and_reopens_with_a_blank_reason", async () => {
  for (const closeLabel of [DIALOG_TITLE_CLOSE, CLOSE_CONFLICT]) {
    await withAuctionActions(async (context) => {
      const { page } = context;
      const { id, form } = await openAuction(context, CANCEL, REASON, "");
      await editField(page.field(id), EDITED_REASON);
      page.conflict("stale_auction_state");
      await submitForm(form);
      const first = takeWrite(page);
      assertCancelBody(first, 7, EDITED_REASON);
      assertAuctionConflict(context, id, EDITED_REASON, CONFIRM_CANCEL);

      const latest = { ...auctionWire(), state_version: 8 };
      await closeAfterAuctionRead(context, closeLabel, CANCEL, latest);
      const next = await openAuction(context, CANCEL, REASON, "");
      const reason = "重新确认后的原因";
      await editField(page.field(next.id), reason);
      const { scheduled_ends_at: _end, ...lifecycle } = latest;
      const second = await acceptAuctionCommand(context, next.form, {
        ...lifecycle,
        status: "cancelled",
        state_version: 9,
        cancelled_at: "2026-11-01T00:00:00Z",
        cancelled_by: "operator",
        cancelled_reason: reason,
      });
      assertCancelBody(second, 8, reason);
      expect(commandId(first)).not.toBe(commandId(second));
    });
  }
});
