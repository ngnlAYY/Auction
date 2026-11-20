// 对照 Rust 排期草稿的版本冻结、原始时间精度与清空行为。

import { expect, spyOn, test } from "bun:test";
import { AdminAuctionResponseSchema } from "@auction/contracts/dto/admin/settlement";
import { decode } from "@auction/contracts/wire";
import { type Cell, createCell } from "@auction/web-shared/reactive/cell";
import { err } from "@auction/web-shared/result";
import { success } from "../../../api/auction-contract-fixture.test";
import { loadRenderModule } from "../../../solid-render-fixture.test";
import { createComponent } from "../../solid-dom-fixture";
import { withUtc } from "../auction-time-fixture.test";
import {
  type EditorTest,
  editField,
  present,
  settleEditor,
  submitForm,
  withEditor,
} from "../editor-fixture.test";
import { auctionWire } from "./auction-actions-fixture.test";
import type { AdminAuctionNotice } from "./model";
import type { ScheduleDraft } from "./schedule";

const AUCTION_ID = "00000000-0000-0000-0000-000000000001";
const PREVIEW = "2099-04-01T11:00:03.123456Z";
const PREVIEW_ID = `schedule-preview_scheduled_at-${AUCTION_ID}`;
const END_ID = `schedule-scheduled_end_at-${AUCTION_ID}`;

function auctionInput() {
  return { ...auctionWire(), id: AUCTION_ID, preview_scheduled_at: PREVIEW };
}

async function withSchedule(
  run: (
    page: EditorTest,
    draft: Cell<ScheduleDraft | null>,
    schedule: typeof import("./schedule"),
    submitted: readonly (readonly [string, string, string, string, number])[],
  ) => Promise<void>,
): Promise<void> {
  const schedule = await loadRenderModule<typeof import("./schedule")>(
    `${import.meta.dir}/schedule.tsx`,
    "scheduleDraftFromAuction",
    "AdminAuctionScheduleDialog",
  );
  const initial = success(decode(AdminAuctionResponseSchema, auctionInput()));
  const submitted: (readonly [string, string, string, string, number])[] = [];
  const clock = spyOn(Date, "now");
  try {
    const auctionsApi = await import("../../../api/auctions");
    const update = spyOn(auctionsApi, "updateAdminAuctionSchedule");
    try {
      clock.mockReturnValue(Date.parse("2099-04-01T10:00:00Z"));
      // Rust 的纯草稿测试没有 HTTP UUID 验证。用同一个 ID 驱动表单，并在该边界观察实际解析结果。
      update.mockImplementation(async (_host, _command, id, preview, open, end, version) => {
        submitted.push([id, preview, open, end, version]);
        return err("invalid_auction_state");
      });
      await withUtc(async () => {
        let mounted: Cell<ScheduleDraft | null> | undefined;
        await withEditor(
          ({ reload }) => {
            const draft = createCell(schedule.scheduleDraftFromAuction(initial));
            mounted = draft;
            return createComponent(schedule.AdminAuctionScheduleDialog, {
              draft,
              // Rust 直接测试两个各自有效的草稿。用对应的版本解锁真实表单中的私有时间解析。
              get latestVersion() {
                return draft()?.stateVersion ?? null;
              },
              read: { kind: "ready" },
              notice: createCell<AdminAuctionNotice | null>(null),
              reloadGeneration: reload,
            });
          },
          (page) => run(page, present(mounted, "排期草稿"), schedule, submitted),
        );
      });
    } finally {
      update.mockRestore();
    }
  } finally {
    clock.mockRestore();
  }
}

test("a_refresh_does_not_rebase_values_or_version", () =>
  withSchedule(async (page, draft, schedule, submitted) => {
    await editField(page.field(END_ID), "2099-04-01T14:00");
    const latest = success(
      decode(AdminAuctionResponseSchema, {
        ...auctionInput(),
        state_version: 8,
        scheduled_open_at: "2099-04-01T12:30:00Z",
      }),
    );
    const old = present(draft.peek(), "旧排期草稿");
    expect(old.stateVersion).toBe(7);
    expect(old.end).toMatchObject({ value: "2099-04-01T14:00", edited: true });
    await submitForm(page.form());
    expect(submitted).toHaveLength(1);
    expect(submitted[0]).toEqual([
      AUCTION_ID,
      PREVIEW,
      "2099-04-01T12:00:00Z",
      "2099-04-01T14:00:00Z",
      7,
    ]);

    const fresh = present(schedule.scheduleDraftFromAuction(latest), "新排期草稿");
    expect(fresh.end.original).toBe("2099-04-01T13:00:00Z");
    expect(fresh.end.original).toBe(old.end.original);
    expect(fresh.end.edited).toBe(false);
    expect(fresh.stateVersion).toBe(8);
    draft.set(fresh);
    await settleEditor();
    await submitForm(page.form());
    expect(submitted).toHaveLength(2);
    expect(submitted[1]).toEqual([
      AUCTION_ID,
      PREVIEW,
      "2099-04-01T12:30:00Z",
      "2099-04-01T13:00:00Z",
      8,
    ]);
  }));

test("untouched_prefill_keeps_original_utc_precision", () =>
  withSchedule(async (page, draft, _schedule, submitted) => {
    expect(present(draft.peek(), "初始排期草稿").preview.original).toBe(PREVIEW);
    await submitForm(page.form());
    expect(submitted).toHaveLength(1);
    expect(submitted[0]).toEqual([
      AUCTION_ID,
      PREVIEW,
      "2099-04-01T12:00:00Z",
      "2099-04-01T13:00:00Z",
      7,
    ]);

    await editField(page.field(PREVIEW_ID), "");
    expect(present(draft.peek(), "编辑后的排期草稿").preview.edited).toBe(true);
    await submitForm(page.form());
    expect(page.field(PREVIEW_ID).getAttribute("aria-invalid")).toBe("true");
    expect(page.field(`${PREVIEW_ID}-error`).textContent).toBe("请填写此字段。");
    expect(submitted).toHaveLength(1);
  }));
