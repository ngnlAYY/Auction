// 在同一挂载中装配 Rust 拍品编辑测试所用的真实行、草稿与列表读。

import { expect, test } from "bun:test";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { ok } from "@auction/web-shared/result";
import { type Accessor, onCleanup, Show } from "solid-js";
import type { AdminLot } from "../../api/lots";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { AUCTION_ID } from "../auctions/actions/auction-actions-fixture.test";
import {
  assertEditorSubmitRemoved,
  assertField,
  type CapturedWrite,
  commandId,
  type EditorTest,
  editField,
  present,
  recordRead,
  withEditor,
} from "../auctions/editor-fixture.test";
import { createComponent } from "../solid-dom-fixture";
import type { LotEdit } from "./edit";
import type { LotUpdateNotice } from "./state";

/** Rust 拍品编辑助手的固定拍品及价格字段。 */
export const LOT_ID = "20f99cb5-7b75-4853-bc49-9c71df78381b";
/** 编辑对话框的真实价格控件标识。 */
export const PRICE = `price-${LOT_ID}`;
/** Rust 在各冲突场景里输入的起拍价。 */
export const EDITED_PRICE = "777";
/** 拍品行的改价入口。 */
export const REPRICE = "改起拍价";
/** 列表读失败的冲突关闭按钮。 */
export const CLOSE_AND_REFRESH = "关闭并刷新列表";
/** 已完成冲突读的关闭按钮。 */
export const CLOSE_CONFLICT = "关闭";

/** Rust 拍品快照，作为列表替身和成功写回执的同一份投影。 */
export function lot(stateVersion: number, price: number, status: string): AdminLot {
  return {
    id: LOT_ID,
    auction_id: AUCTION_ID,
    equipment_id: "20f99cb5-7b75-4853-bc49-9c71df78381d",
    submission_id: null,
    lot_number: 1,
    status,
    start_price: price,
    current_price: price,
    leader_user_id: null,
    effective_end_at: null,
    slow_due_at: null,
    slow_generation: 0,
    slow_authorization_id: null,
    state_version: stateVersion,
    created_at: "2026-11-01T00:00:00Z",
    updated_at: "2026-11-01T01:00:00Z",
    external_equipment_id: "123",
    equipment_name: "Phase Robe",
    equipment_category: "Cloth",
    equipment_attributes: {},
    equipment_collected_at: "2026-11-01T00:00:00Z",
    seller: null,
    submission_source_kind: null,
    submission_source_reference: null,
    mailbox_key: null,
    external_mail_id: null,
    submission_received_at: null,
  };
}

type RecordRead = ReturnType<typeof recordRead<AdminLot>>;

/** 通过内存读取和控件事件驱动真实编辑生产入口。 */
export interface LotCase {
  readonly page: EditorTest;
  readonly reads: RecordRead;
  draft(): LotEdit | null;
  displayed(): number | null;
  occupied(): boolean;
}

/** 挂载真实拍品行与编辑对话框，首轮读取在返回给用例前按 Rust 样本交付。 */
export async function withLotEdit(run: (context: LotCase) => Promise<void>): Promise<void> {
  const [edit, row] = await Promise.all([
    loadRenderModule<typeof import("./edit")>(
      `${import.meta.dir}/edit.tsx`,
      "lotEditLotId",
      "AdminLotEditDialog",
    ),
    loadRenderModule<typeof import("./row")>(`${import.meta.dir}/row.tsx`, "AdminLotRow"),
  ]);
  const reads = recordRead<AdminLot>();
  let mounted: Omit<LotCase, "page" | "reads"> | undefined;
  await withEditor(
    ({ host, reload, owner, useRead }) => {
      const draft = createCell<LotEdit | null>(null);
      const notice = createCell<LotUpdateNotice | null>(null);
      const selectedLot = createCell<readonly [string, string] | null>(null);
      const { read, snapshot } = useRead("lots", () => draft() !== null, reads.fetch);
      const version = createValueMemo(() => {
        const selected = draft();
        const current = snapshot();
        return selected !== null && current !== null && edit.lotEditLotId(selected) === current.id
          ? current.state_version
          : null;
      });
      let alive = true;
      onCleanup(() => {
        alive = false;
      });
      mounted = {
        draft: () => draft.peek(),
        displayed: () => snapshot()?.state_version ?? null,
        occupied: () => owner.contains(`lot:${LOT_ID}`),
      };
      return [
        createComponent(
          () =>
            Show({
              get when() {
                return snapshot();
              },
              children: (current: Accessor<AdminLot>) =>
                createComponent(row.AdminLotRow, {
                  get lot() {
                    return current();
                  },
                  get editsReady() {
                    return read().kind === "ready";
                  },
                  allowLotUpdate: true,
                  selectedLot,
                  notice,
                  draft,
                }),
            }),
          {},
        ),
        createComponent(Show, {
          get when() {
            return draft() !== null;
          },
          children: () =>
            createComponent(edit.AdminLotEditDialog, {
              draft,
              notice,
              host,
              reloadGeneration: reload,
              pageAlive: () => alive,
              get latestVersion() {
                return version();
              },
              get read() {
                return read();
              },
            }),
        }),
      ];
    },
    async (page) => {
      await reads.respond(ok(lot(7, 100, "pending")));
      await run({ ...present(mounted, "拍品页面"), page, reads });
    },
  );
}

/** 从真实行点击改价入口并验证预填值。 */
export async function openPrice(page: EditorTest, initial: string) {
  await page.click(REPRICE);
  assertField(page, PRICE, initial, false);
  return page.form();
}

/** 打开当前快照的 100 起拍价并派发 777 输入。 */
export async function editingPrice(context: LotCase) {
  const { page } = context;
  const form = await openPrice(page, "100");
  await editField(page.field(PRICE), EDITED_PRICE);
  assertEditedPrice(context, false);
  return form;
}

/** 同时从真实草稿及控件读取起拍价，区分编辑状态和输入替身缓存。 */
export function assertEditedPrice(context: LotCase, readOnly: boolean): void {
  expect(present(context.draft(), "实际改价草稿").price).toBe(EDITED_PRICE);
  assertField(context.page, PRICE, EDITED_PRICE, readOnly);
}

/** 版本冲突会释放未决宿主、保留只读草稿并移除提交。 */
export function assertLotConflict(context: LotCase): void {
  const { page } = context;
  expect(context.occupied()).toBe(false);
  expect(page.textNodes().some((text) => text.includes("本次修改没有生效"))).toBe(true);
  assertEditorSubmitRemoved(page, "保存");
  assertEditedPrice(context, true);
}

/** 冲突后的读取已经完成时，两种关闭按钮都只清掉草稿。 */
export async function closeAfterLotRead(context: LotCase, closeLabel: string, current: AdminLot) {
  expect(context.reads.started()).toBe(2);
  await context.reads.respond(ok(current));
  expect(context.displayed()).toBe(current.state_version);
  expect(context.reads.started()).toBe(2);
  await context.page.click(closeLabel);
  expect(context.draft()).toBeNull();
  expect(context.reads.started()).toBe(2);
  expect(context.displayed()).toBe(current.state_version);
}

/** Rust 拍品写请求的精确线格式；撤拍请求省略 start_price。 */
export function assertLotRequest(
  request: CapturedWrite,
  version: number,
  action: string,
  price: number | null,
) {
  const id = commandId(request);
  const priceField = price === null ? "" : `,"start_price":${price}`;
  expect(request).toEqual({
    method: "PATCH",
    path: `/api/v1/admin/lots/${LOT_ID}`,
    body: `{"command_id":"${id}","expected_state_version":${version},"action":"${action}"${priceField}}`,
  });
}

test("拍品写入夹具区分撤拍时省略价格和传入 null", () => {
  const command = {
    command_id: "00000000-0000-4000-8000-000000000002",
    expected_state_version: 7,
    action: "withdraw",
  };
  const request = {
    method: "PATCH",
    path: `/api/v1/admin/lots/${LOT_ID}`,
    body: JSON.stringify(command),
  };
  expect(() => assertLotRequest(request, 7, "withdraw", null)).not.toThrow();
  const invalid = { ...request, body: JSON.stringify({ ...command, start_price: null }) };
  expect(() => assertLotRequest(invalid, 7, "withdraw", null)).toThrow();
});
