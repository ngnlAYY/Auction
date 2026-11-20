// 在内存中控制秒级节拍，核对开拍按钮与真实未决集合的订阅时机。

import { expect, spyOn, test } from "bun:test";
import { AdminAuctionResponseSchema } from "@auction/contracts/dto/admin/settlement";
import { decode } from "@auction/contracts/wire";
import { SecondTickerContext, useSecondTicker } from "@auction/web-shared/platform/clock";
import { createCell } from "@auction/web-shared/reactive/cell";
import { err } from "@auction/web-shared/result";
import { writeOutcomeUnknown } from "../../../api/failure";
import { loadRenderModule } from "../../../solid-render-fixture.test";
import type { PendingCommands } from "../../pending";
import { createComponent } from "../../solid-dom-fixture";
import { present, settleEditor, withEditor } from "../editor-fixture.test";
import { AUCTION_ID, auctionWire } from "./auction-actions-fixture.test";
import type { AdminAuctionNotice } from "./model";

const OTHER_AUCTION_ID = "20f99cb5-7b75-4853-bc49-9c71df78382d";

test.each([
  ["未就绪时本行进入未决仍等秒级节拍", false, AUCTION_ID, true],
  ["就绪时本行进入未决立即显示禁用的开拍按钮", true, AUCTION_ID, true],
  ["就绪时另一行进入未决立即显示本行可用的开拍按钮", true, OTHER_AUCTION_ID, false],
] as const)("%s", async (_label, editsReady, pendingAuctionId, buttonDisabled) => {
  const [dialog, drafts] = await Promise.all([
    loadRenderModule<typeof import("./dialog")>(
      `${import.meta.dir}/dialog.tsx`,
      "AdminAuctionActionDialog",
      "lifecycleDraftNew",
    ),
    loadRenderModule<typeof import(".")>(
      `${import.meta.dir}/index.tsx`,
      "AuctionDraftsContext",
      "useAuctionDrafts",
    ),
  ]);
  const auction = decode(AdminAuctionResponseSchema, { ...auctionWire(), status: "preview" });
  if (!auction.ok) {
    throw new Error("预览场次夹具无法解码");
  }
  const opensAt = Date.parse(auction.value.scheduled_open_at);
  let now = opensAt - 500;
  let tick: (() => void) | undefined;
  let owner: PendingCommands | undefined;
  let restoreIntervals = (): void => {};
  const time = spyOn(Date, "now").mockImplementation(() => now);
  try {
    await withEditor(
      (scope) => {
        owner = scope.owner;
        const fixtureWindow = window;
        const savedSetInterval = Object.getOwnPropertyDescriptor(fixtureWindow, "setInterval");
        const savedClearInterval = Object.getOwnPropertyDescriptor(fixtureWindow, "clearInterval");
        restoreIntervals = () => {
          for (const [name, descriptor] of [
            ["setInterval", savedSetInterval],
            ["clearInterval", savedClearInterval],
          ] as const) {
            if (descriptor === undefined) Reflect.deleteProperty(fixtureWindow, name);
            else Object.defineProperty(fixtureWindow, name, descriptor);
          }
        };
        Object.defineProperties(fixtureWindow, {
          setInterval: {
            configurable: true,
            value: (callback: () => void, delay: number) => {
              expect(delay).toBe(1000);
              tick = callback;
              return 1;
            },
          },
          clearInterval: {
            configurable: true,
            value: () => {
              tick = undefined;
            },
          },
        });
        return createComponent(SecondTickerContext, {
          value: useSecondTicker(),
          get children() {
            return createComponent(drafts.AuctionDraftsContext, {
              value: drafts.useAuctionDrafts(),
              get children() {
                return createComponent(dialog.AdminAuctionActions, {
                  auction: auction.value,
                  editsReady,
                  notice: createCell<AdminAuctionNotice | null>(null),
                  capabilities: {
                    modify_schedule: false,
                    cancel_auction: false,
                    manual_preview_open: false,
                    manual_early_open: false,
                    manual_early_settlement: false,
                    manual_intake: false,
                  },
                });
              },
            });
          },
        });
      },
      async (page) => {
        const pending = present(owner, "未决命令所有者");
        const entity = `auction:${AUCTION_ID}`;
        const pendingEntity = `auction:${pendingAuctionId}`;
        expect(page.button("立即开拍")).toBeUndefined();
        expect(pending.contains(entity)).toBe(false);

        now = opensAt;
        expect(
          pending.submit(
            pendingEntity,
            "场次命令",
            async () => err(writeOutcomeUnknown("network_error")),
            () => {},
            () => {},
          ),
        ).toBe(true);
        await settleEditor();
        expect(pending.contains(pendingEntity)).toBe(true);
        expect(pending.isBusy(pendingEntity)).toBe(false);
        expect(pending.contains(entity)).toBe(pendingAuctionId === AUCTION_ID);

        if (!editsReady) {
          expect(page.button("立即开拍")).toBeUndefined();
          present(tick, "秒级节拍")();
          await settleEditor();
        }
        expect(present(page.button("立即开拍"), "开拍按钮").hasAttribute("disabled")).toBe(
          buttonDisabled,
        );
      },
    );
  } finally {
    restoreIntervals();
    time.mockRestore();
  }
});
