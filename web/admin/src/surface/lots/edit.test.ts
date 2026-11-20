// 对照 Rust 改价、撤拍、冲突重读与未知结果原样重试。

import { expect, test } from "bun:test";
import { err, ok } from "@auction/web-shared/result";
import { adminApiFailureFromHttpBody } from "../../api/failure";
import {
  assertEditorSubmitRemoved,
  commandId,
  DIALOG_TITLE_CLOSE,
  editField,
  present,
  settleEditor,
  submitForm,
  takeWrite,
} from "../auctions/editor-fixture.test";
import {
  assertEditedPrice,
  assertLotConflict,
  assertLotRequest,
  CLOSE_AND_REFRESH,
  CLOSE_CONFLICT,
  closeAfterLotRead,
  EDITED_PRICE,
  editingPrice,
  lot,
  openPrice,
  PRICE,
  REPRICE,
  withLotEdit,
} from "./lot-edit-fixture.test";

test("reprice_draft_survives_failed_refresh_and_successful_retry", () =>
  withLotEdit(async (context) => {
    const { page, reads } = context;
    await editingPrice(context);
    await page.click("刷新记录");
    expect(page.text()).toContain("正在刷新记录");
    expect(context.displayed()).toBe(7);
    await reads.respond(err("network_error"));
    expect(page.text()).toContain("刷新记录失败");
    expect(context.displayed()).toBe(7);
    assertEditedPrice(context, false);
    await page.click("重试");
    await reads.respond(ok(lot(8, 200, "pending")));
    expect(context.displayed()).toBe(8);
    assertLotConflict(context);
    expect(present(context.draft(), "旧草稿").base.stateVersion).toBe(7);
  }));

test("stale_price_closes_and_the_fresh_row_opens_a_new_draft", async () => {
  for (const label of [DIALOG_TITLE_CLOSE, CLOSE_CONFLICT]) {
    await withLotEdit(async (context) => {
      const { page } = context;
      const form = await editingPrice(context);
      page.conflict("stale_lot_state");
      await submitForm(form);
      const first = takeWrite(page);
      assertLotRequest(first, 7, "set_start_price", 777);
      assertLotConflict(context);
      await submitForm(form);
      expect(page.takeWrites()).toEqual([]);

      await closeAfterLotRead(context, label, lot(8, 200, "pending"));
      const next = await openPrice(page, "200");
      await editField(page.field(PRICE), "333");
      page.reply(
        200,
        JSON.stringify({
          code: "ok",
          message: "改价成功",
          data: { lot: lot(9, 333, "pending") },
          request_id: "review-fixture",
        }),
      );
      await submitForm(next);
      const second = takeWrite(page);
      assertLotRequest(second, 8, "set_start_price", 333);
      expect(commandId(first)).not.toBe(commandId(second));
      expect(context.draft()).toBeNull();
    });
  }
});

test("closing_a_price_draft_without_a_conflict_does_not_refresh_the_list", () =>
  withLotEdit(async (context) => {
    const { page, reads } = context;
    await editingPrice(context);
    await page.click(DIALOG_TITLE_CLOSE);
    expect(context.draft()).toBeNull();
    expect(present(page.button(REPRICE), "改价入口").hasAttribute("disabled")).toBe(false);
    expect(reads.started()).toBe(1);
    await openPrice(page, "100");
  }));

test("a_withdrawn_lot_has_no_edit_entry_on_the_refreshed_row", () =>
  withLotEdit(async (context) => {
    const { page, reads } = context;
    await page.click("撤拍");
    page.conflict("stale_lot_state");
    await submitForm(page.form());
    expect(page.textNodes().some((text) => text.includes("本次修改没有生效"))).toBe(true);
    assertEditorSubmitRemoved(page, "确认撤回");
    expect(context.occupied()).toBe(false);
    assertLotRequest(takeWrite(page), 7, "withdraw", null);
    expect(reads.started()).toBe(2);
    await page.click(DIALOG_TITLE_CLOSE);
    expect(context.draft()).toBeNull();
    expect(reads.started()).toBe(2);
    expect(present(page.button(REPRICE), "禁用的改价入口").hasAttribute("disabled")).toBe(true);
    await reads.respond(ok(lot(8, 100, "withdrawn")));
    expect(page.button(REPRICE)).toBeUndefined();
    expect(page.button("撤拍")).toBeUndefined();
  }));

test("closing_after_a_failed_conflict_read_requests_a_fresh_version", () =>
  withLotEdit(async (context) => {
    const { page, reads } = context;
    const form = await editingPrice(context);
    page.conflict("stale_lot_state");
    await submitForm(form);
    assertLotRequest(takeWrite(page), 7, "set_start_price", 777);
    const failure = adminApiFailureFromHttpBody(
      503,
      '{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}',
    );
    await reads.respond(err(failure.code));
    expect(reads.started()).toBe(2);
    await page.click(CLOSE_AND_REFRESH);
    expect(context.draft()).toBeNull();
    expect(reads.started()).toBe(3);
    expect(present(page.button(REPRICE), "禁用的改价入口").hasAttribute("disabled")).toBe(true);
    await reads.respond(ok(lot(8, 200, "pending")));
    await openPrice(page, "200");
    expect(present(context.draft(), "新草稿").base.stateVersion).toBe(8);
  }));

test("incomplete_conflict_receipts_leave_the_original_price_intent_locked", async () => {
  for (const body of [
    '{"code":"stale_lot_state","message":"版本已变化","data":null}',
    '["stale_lot_state","版本已变化",null,null]',
  ]) {
    await withLotEdit(async (context) => {
      const { page } = context;
      const form = await editingPrice(context);
      page.reply(409, body);
      await submitForm(form);
      const first = takeWrite(page);
      assertLotRequest(first, 7, "set_start_price", 777);
      expect(context.occupied()).toBe(true);
      expect(
        present(page.retry(commandId(first)), "原命令的重试按钮").hasAttribute("disabled"),
      ).toBe(false);
      expect(page.button(CLOSE_AND_REFRESH)).toBeUndefined();
      const conflictText = "这条记录已被其他操作更新，本次修改没有生效。";
      expect(page.hasText(conflictText)).toBe(false);
      const alerts = page.find((node) => node.getAttribute("role") === "alert");
      expect(alerts.some((alert) => alert.textContent.includes(conflictText))).toBe(false);
      expect(present(context.draft(), "锁定草稿").price).toBe(EDITED_PRICE);
      await submitForm(form);
      expect(page.takeWrites()).toEqual([]);
    });
  }
});

test("timed_out_price_replays_identical_bytes_before_a_verified_conflict_unlocks_it", () =>
  withLotEdit(async (context) => {
    const { page } = context;
    const form = await editingPrice(context);
    page.replyTimeout();
    await submitForm(form);
    const first = takeWrite(page);
    assertLotRequest(first, 7, "set_start_price", 777);
    expect(context.occupied()).toBe(true);
    const id = commandId(first);
    const retry = present(page.retry(id), "原命令条目的共享重试按钮");
    expect(retry.hasAttribute("disabled")).toBe(false);
    expect(page.retry("00000000-0000-0000-0000-000000000000")).toBeUndefined();
    expect(page.button(CLOSE_AND_REFRESH)).toBeUndefined();
    expect(page.button("刷新记录")).toBeUndefined();
    assertEditorSubmitRemoved(page, "保存");
    await submitForm(form);
    expect(page.takeWrites()).toEqual([]);

    page.conflict("stale_lot_state");
    expect(typeof retry.attributes.onClick).toBe("function");
    retry.click();
    await settleEditor();
    expect(takeWrite(page)).toEqual(first);
    assertLotConflict(context);
    expect(page.retry(id)).toBeUndefined();
  }));
