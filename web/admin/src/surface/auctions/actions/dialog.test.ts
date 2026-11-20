// 对照 Rust 提前进入最终阶段的确认文案。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { loadRenderModule } from "../../../solid-render-fixture.test";
import { createComponent } from "../../solid-dom-fixture";
import { present, withEditor } from "../editor-fixture.test";
import type { AdminAuctionNotice } from "./model";

test("final_confirmation_names_the_world_and_round_without_assuming_minutes", async () => {
  const { AdminAuctionActionDialog, lifecycleDraftNew } = await loadRenderModule<
    typeof import("./dialog")
  >(`${import.meta.dir}/dialog.tsx`, "AdminAuctionActionDialog", "lifecycleDraftNew");
  for (const [worldId, label] of [
    ["main", "主世界"],
    ["isekai", "Isekai"],
  ] as const) {
    await withEditor(
      ({ reload }) =>
        createComponent(AdminAuctionActionDialog, {
          latestVersion: 5,
          notice: createCell<AdminAuctionNotice | null>(null),
          pending: createCell<ReturnType<typeof lifecycleDraftNew> | null>(
            lifecycleDraftNew(
              {
                auctionId: "test-auction",
                worldId,
                sequence: 74,
                stateVersion: 5,
              },
              "final-phase",
            ),
          ),
          reloadGeneration: reload,
          read: { kind: "ready" },
        }),
      (page) => {
        const dialogs = page.find((node) => node.getAttribute("role") === "dialog");
        expect(dialogs).toHaveLength(1);
        const dialogTexts = page.textNodes(present(dialogs[0], "最终阶段确认框"));
        const texts = page.textNodes();
        expect(
          dialogTexts.filter((text) => text.includes(label) && text.includes("74")),
        ).toHaveLength(2);
        expect(texts.filter((text) => text.includes(label) && text.includes("74"))).toHaveLength(2);
        expect(dialogTexts.some((text) => text.includes("本场冻结的策略"))).toBe(true);
        expect(texts.every((text) => !text.includes("15 分钟"))).toBe(true);
      },
    );
  }
});
