// 恢复表格的移动卡片标签、旧报告事件守卫与退款后果提示。

import { expect, test } from "bun:test";
import { AdminRecoveryResponseSchema } from "@auction/contracts/dto/admin/recovery";
import { decode } from "@auction/contracts/wire";
import { createCell } from "@auction/web-shared/reactive/cell";
import { flush } from "solid-js";
import { requireOk } from "../../api/settlement-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { PendingCommandsContext } from "../pending";
import { usePendingCommands } from "../pending/sources";
import type { AdminActionFeedback } from "../presentation";
import { createComponent, createElement, insert, settle } from "../solid-dom-fixture";
import type { RecoveryConfirmationDraft, RecoveryRead } from "./recovery";
import { cardLabels, requireElement, withAdminTree } from "./settlement-tree-fixture.test";

const AUCTION_ID = "00000000-0000-0000-0000-000000000002";
const RECOVERY_ID = "00000000-0000-0000-0000-000000000001";
const REFUND_TEXT = "确认“装备已恢复”还会激活买家退款，并关闭原卖家货款义务。";

function recoveryRow() {
  return requireOk(
    decode(AdminRecoveryResponseSchema, {
      id: RECOVERY_ID,
      world_id: "main",
      auction_id: AUCTION_ID,
      lot_number: 1,
      lot_id: "00000000-0000-0000-0000-000000000003",
      equipment_name: "Phase Robe",
      equipment_category: "Cloth",
      stage: "reclaiming",
      resolution: null,
      state_version: 2,
      payment_deadline_at: "2027-01-01T00:00:00Z",
      reminder_count: 0,
      reminder_limit: 1,
      custody_proof_kind: null,
      custody_proof_observed_at: null,
      blocking_code: null,
      manual_reconcile_delivery_id: null,
      relisted_auction_id: null,
      relisted_lot_id: null,
      actions: [],
      allowed_commands: ["reclaimed"],
      updated_at: "2027-01-01T00:00:00Z",
    }),
  );
}

function loadRecovery() {
  return loadRenderModule<typeof import("./recovery")>(
    `${import.meta.dir}/recovery.tsx`,
    "AdminRecoveryTable",
    "AdminRecoveryDialog",
  );
}

async function withRecoveryTable(run: Parameters<typeof withAdminTree>[1]) {
  const { AdminRecoveryTable } = await loadRecovery();
  const { useAdminPagination } = await loadRenderModule<
    Pick<typeof import("../pagination"), "useAdminPagination">
  >(new URL("../pagination/index.tsx", import.meta.url).pathname, "useAdminPagination");
  await withAdminTree("/settlement?recovery_page_size=10", async (tree) => {
    const report = { auction_id: AUCTION_ID, recoveries: [recoveryRow()] };
    tree.mount(() => {
      const reload = createCell(0);
      const loaded = createCell(0);
      const page = createCell(1);
      const confirmation = createCell<RecoveryConfirmationDraft | null>(null);
      const pagination = useAdminPagination(
        "settlement-recovery",
        ["recovery_page_size"],
        null,
        100_000,
      );
      const pending = usePendingCommands(createCell(0n), () => true);
      const invalidate = createElement("button", { onClick: () => reload.set(1) });
      insert(invalidate, "使恢复报告失效");
      const finish = createElement("button", { onClick: () => loaded.set(1) });
      insert(finish, "完成恢复报告读取");
      const draft = createElement("p");
      insert(draft, () => (confirmation() === null ? "" : "已有恢复确认草稿"));
      return createComponent(PendingCommandsContext, {
        value: pending,
        get children() {
          return [
            invalidate,
            finish,
            createComponent(AdminRecoveryTable, {
              report,
              currentPage: 1,
              pagination,
              page,
              actionBusy: () => false,
              get recoveryRead(): RecoveryRead {
                return {
                  reload,
                  recoveryReload: reload,
                  generation: [loaded(), loaded()],
                  alive: () => true,
                };
              },
              confirmation,
              reason: createCell(""),
              actionFeedback: createCell<AdminActionFeedback | null>(null),
            }),
            draft,
          ];
        },
      });
    });
    await settle();
    await run(tree);
  });
}

test("every_recovery_cell_renders_its_card_label", async () => {
  await withRecoveryTable((tree) => {
    const body = requireElement(
      tree.find((node) => node.tag === "tbody"),
      "恢复表体",
    );
    expect(body.querySelectorAll("tr")).toHaveLength(1);
    expect(cardLabels(body)).toEqual(
      new Set(["拍品", "装备", "恢复阶段", "付款截止", "证据/阻塞原因", "恢复操作", "操作"]),
    );
  });
});

test("recovery_entry_rejects_events_from_an_invalidated_report", async () => {
  await withRecoveryTable(async (tree) => {
    const button = (label: string) => {
      const target = requireElement(
        tree.find((node) => node.tag === "button" && node.textContent === label),
        label,
      );
      expect(typeof target.attributes.onClick, label).toBe("function");
      return target;
    };
    expect(button("确认已回收").hasAttribute("disabled")).toBe(false);

    const clickEnabled = (label: string) => {
      const target = button(label);
      expect(target.hasAttribute("disabled"), label).toBe(false);
      target.click();
    };
    clickEnabled("使恢复报告失效");
    await settle();
    const stale = button("确认已回收");
    expect(stale.hasAttribute("disabled")).toBe(true);
    // 直接派发旧按钮事件，核对事件处理器本身拒绝失效的报告。
    stale.dispatchClick();
    await settle();
    expect(
      tree.find((node) => node.tag === "p" && node.textContent === "已有恢复确认草稿"),
    ).toBeNull();

    clickEnabled("完成恢复报告读取");
    await settle();
    clickEnabled("确认已回收");
    await settle();
    expect(
      tree.find((node) => node.tag === "p" && node.textContent === "已有恢复确认草稿"),
    ).not.toBeNull();
  });
});

test("refund_consequences_apply_only_to_recovered_equipment", async () => {
  const { AdminRecoveryDialog } = await loadRecovery();
  await withAdminTree("/settlement", async (tree) => {
    const action = createCell("equipment_recovered");
    tree.mount(() => {
      const pending = usePendingCommands(createCell(0n), () => true);
      const reload = createCell(0);
      return createComponent(PendingCommandsContext, {
        value: pending,
        get children() {
          return createComponent(AdminRecoveryDialog, {
            get draft() {
              return {
                action: action(),
                auctionId: AUCTION_ID,
                recoveryId: RECOVERY_ID,
                stateVersion: 2,
                equipmentName: "Phase Robe",
                lotCode: "C1",
              };
            },
            confirmation: createCell<RecoveryConfirmationDraft | null>(null),
            reason: createCell(""),
            busy: () => false,
            feedback: createCell<AdminActionFeedback | null>(null),
            selectedAuctionId: createCell(AUCTION_ID),
            selectionGeneration: createCell(0),
            recoveryRead: { reload, recoveryReload: reload, generation: [0, 0], alive: () => true },
            recoveryReloadGeneration: reload,
          });
        },
      });
    });
    await settle();
    const findDialog = () => tree.find((node) => node.getAttribute("role") === "dialog");
    const dialog = requireElement(findDialog(), "确认对话框");
    const consequence = () =>
      tree.find((node) => node.tag === "p" && node.textContent === REFUND_TEXT);
    const changeAction = async (next: string) => {
      action.set(next);
      flush();
      await settle();
      expect(findDialog(), next).toBe(dialog);
    };
    expect(consequence()).not.toBeNull();
    await changeAction("reclaimed");
    expect(consequence()).toBeNull();
    await changeAction("equipment_recovered");
    expect(consequence()).not.toBeNull();
    await changeAction("cancelled");
    expect(consequence()).toBeNull();
  });
});
