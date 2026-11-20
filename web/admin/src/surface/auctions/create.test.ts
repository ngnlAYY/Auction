// 从真实创建表单覆盖 Rust 私有期号解析器的全部边界。

import { expect, test } from "bun:test";
import { parseJson } from "@auction/web-shared/net/wire";
import { createCell } from "@auction/web-shared/reactive/cell";
import { success } from "../../api/auction-contract-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { createComponent } from "../solid-dom-fixture";
import type { AdminAuctionNotice } from "./actions/model";
import { withUtc } from "./auction-time-fixture.test";
import {
  commandId,
  editField,
  settleEditor,
  submitForm,
  takeWrite,
  withEditor,
} from "./editor-fixture.test";

test("creates_only_positive_json_safe_sequence_numbers", () =>
  withUtc(async () => {
    const { AdminAuctionCreateForm } = await loadRenderModule<typeof import("./create")>(
      `${import.meta.dir}/create.tsx`,
      "AdminAuctionCreateForm",
    );
    await withEditor(
      () =>
        createComponent(AdminAuctionCreateForm, {
          notice: createCell<AdminAuctionNotice | null>(null),
        }),
      async (page) => {
        await page.click("创建拍卖");
        const world = page.field("create-world");
        expect(typeof world.attributes.onChange).toBe("function");
        world.change("main");
        await settleEditor();
        await editField(page.field("create-preview"), "2099-04-01T11:00");
        await editField(page.field("create-open"), "2099-04-01T12:00");
        await editField(page.field("create-end"), "2099-04-01T13:00");
        const field = page.field("create-sequence");
        const form = page.form();

        for (const input of ["0", "-1", "9007199254740992", "1.5", "99999999999999999999"]) {
          await editField(field, input);
          await submitForm(form);
          expect(field.getAttribute("aria-invalid")).toBe("true");
          expect(page.field("create-sequence-error").textContent).toBe(
            "请输入 1 至 9007199254740991 之间的整数期号。",
          );
          for (const id of ["create-world", "create-preview", "create-open", "create-end"]) {
            expect(page.field(id).getAttribute("aria-invalid")).toBe("false");
          }
          expect(page.takeWrites()).toEqual([]);
        }

        await editField(field, "9007199254740991");
        page.conflict("world_has_active_auction");
        await submitForm(form);
        expect(field.getAttribute("aria-invalid")).toBe("false");
        const write = takeWrite(page);
        expect(write.method).toBe("POST");
        expect(write.path).toBe("/api/v1/admin/auctions");
        expect(success(parseJson(write.body))).toMatchObject({
          command_id: commandId(write),
          world_id: "main",
          sequence: 9_007_199_254_740_991,
          preview_scheduled_at: "2099-04-01T11:00:00Z",
          scheduled_open_at: "2099-04-01T12:00:00Z",
          scheduled_end_at: "2099-04-01T13:00:00Z",
          expected_state_version: 0,
        });
      },
    );
  }));

test("create_only_marks_the_world_field_for_world_not_found", () =>
  withUtc(async () => {
    const { AdminAuctionCreateForm } = await loadRenderModule<typeof import("./create")>(
      `${import.meta.dir}/create.tsx`,
      "AdminAuctionCreateForm",
    );
    for (const [status, code, expected] of [
      [404, "world_not_found", true],
      [409, "world_has_active_auction", false],
    ] as const) {
      await withEditor(
        () => createComponent(AdminAuctionCreateForm, { notice: createCell(null) }),
        async (page) => {
          await page.click("创建拍卖");
          const world = page.field("create-world");
          world.change("main");
          await settleEditor();
          await editField(page.field("create-preview"), "2099-04-01T11:00");
          await editField(page.field("create-open"), "2099-04-01T12:00");
          await editField(page.field("create-end"), "2099-04-01T13:00");
          page.reply(
            status,
            JSON.stringify({ code, message: "拒绝", data: null, request_id: null }),
          );
          await submitForm(page.form());
          const write = takeWrite(page);
          expect(write.method).toBe("POST");
          expect(write.path).toBe("/api/v1/admin/auctions");
          expect(world.getAttribute("aria-invalid")).toBe(String(expected));
          expect(world.getAttribute("aria-describedby")).toBe(
            expected ? "create-world-error" : null,
          );
          if (expected) {
            expect(page.field("create-world-error").textContent).toBe(
              "该业务世界已不存在，请重新加载当前状态。",
            );
          }
        },
      );
    }
  }));
