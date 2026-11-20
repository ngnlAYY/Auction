// 将 Rust 场次动作测试的草稿和列表读取装配到同一个 Solid 页面。

import { expect, test } from "bun:test";
import { AdminAuctionResponseSchema } from "@auction/contracts/dto/admin/settlement";
import { decode } from "@auction/contracts/wire";
import { createCell } from "@auction/web-shared/reactive/cell";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { err, ok } from "@auction/web-shared/result";
import { type Accessor, Show } from "solid-js";
import { success } from "../../../api/auction-contract-fixture.test";
import type { AdminAuction, AdminAuctionCapabilities } from "../../../api/auctions";
import { loadRenderModule } from "../../../solid-render-fixture.test";
import { createComponent } from "../../solid-dom-fixture";
import {
  assertEditorSubmitRemoved,
  assertField,
  type CapturedWrite,
  commandId,
  type EditorTest,
  editField,
  present,
  type ReadView,
  recordRead,
  submitForm,
  takeWrite,
  withEditor,
} from "../editor-fixture.test";
import type { AuctionDrafts } from "./index";
import type { AdminAuctionNotice } from "./model";

/** Rust 场次动作集成测试的场次 ID。 */
export const AUCTION_ID = "20f99cb5-7b75-4853-bc49-9c71df78381c";

const END_FIELD = `schedule-scheduled_end_at-${AUCTION_ID}`;
const REASON_FIELD = `action-reason-${AUCTION_ID}`;

/** Rust 原始场次 JSON；省略兼容字段 lot_count。 */
export function auctionWire() {
  return {
    id: AUCTION_ID,
    world_id: "main",
    sequence: 3,
    state_version: 7,
    status: "draft",
    preview_scheduled_at: "2099-04-01T11:00:00Z",
    scheduled_open_at: "2099-04-01T12:00:00Z",
    scheduled_ends_at: "2099-04-01T13:00:00Z",
    preview_opened_at: null,
    preview_manual_open: false,
    preview_revision: 0,
    opened_at: null,
    final_phase_at: null,
    settled_at: null,
    cancelled_at: null,
    cancelled_reason: null,
    cancelled_by: null,
    created_at: "2099-04-01T10:00:00Z",
    updated_at: "2099-04-01T10:00:00Z",
  };
}

const CAPABILITIES: AdminAuctionCapabilities = {
  modify_schedule: true,
  cancel_auction: true,
  manual_preview_open: false,
  manual_early_open: false,
  manual_early_settlement: false,
  manual_intake: false,
};

type RecordRead = ReturnType<typeof recordRead<AdminAuction>>;

/** 场次用例的初始快照、真实草稿和读取版本探针。 */
export interface AuctionCase {
  readonly page: EditorTest;
  readonly reads: RecordRead;
  readonly drafts: AuctionDrafts;
  displayed(): number | null;
  occupied(): boolean;
}

/** 将同一个真实行入口与草稿对话框连接到可顺序应答的分页读。 */
export async function withAuctionActions(
  run: (context: AuctionCase) => Promise<void>,
): Promise<void> {
  const [actions, row] = await Promise.all([
    loadRenderModule<typeof import("./index")>(
      `${import.meta.dir}/index.tsx`,
      "AuctionDraftsContext",
      "useAuctionDrafts",
      "AdminAuctionDraftDialogs",
    ),
    loadRenderModule<typeof import("./row")>(`${import.meta.dir}/row.tsx`, "AdminAuctionRow"),
  ]);
  const reads = recordRead<AdminAuction>();
  let mounted: Omit<AuctionCase, "page" | "reads"> | undefined;
  await withEditor(
    ({ reload, owner, useRead }) => {
      const drafts = actions.useAuctionDrafts();
      const notice = createCell<AdminAuctionNotice | null>(null);
      const { read, snapshot }: ReadView<AdminAuction> = useRead(
        "auctions",
        () => drafts.auctionId() !== null,
        reads.fetch,
      );
      const version = createValueMemo(() => {
        const current = snapshot();
        return current !== null && current.id === drafts.auctionId() ? current.state_version : null;
      });
      mounted = {
        drafts,
        displayed: () => snapshot()?.state_version ?? null,
        occupied: () => owner.contains(`auction:${AUCTION_ID}`),
      };
      return createComponent(actions.AuctionDraftsContext, {
        value: drafts,
        get children() {
          return [
            createComponent(
              () =>
                Show({
                  get when() {
                    return snapshot();
                  },
                  children: (current: Accessor<AdminAuction>) =>
                    createComponent(row.AdminAuctionRow, {
                      get auction() {
                        return current();
                      },
                      get editsReady() {
                        return read().kind === "ready";
                      },
                      capabilities: CAPABILITIES,
                      notice,
                    }),
                }),
              {},
            ),
            createComponent(actions.AdminAuctionDraftDialogs, {
              drafts,
              notice,
              reloadGeneration: reload,
              get latestVersion() {
                return version();
              },
              get read() {
                return read();
              },
            }),
          ];
        },
      });
    },
    async (page) => {
      await reads.respond(ok(success(decode(AdminAuctionResponseSchema, auctionWire()))));
      await run({ ...present(mounted, "场次页面"), page, reads });
    },
  );
}

/** 提交实际字段事件、检查旧快照和失败重试后显示的冲突。 */
export async function refreshAuctionDraft(
  context: AuctionCase,
  entry: string,
  field: string,
  initial: string,
  draft: string,
  submitLabel: string,
): Promise<void> {
  const { page, reads } = context;
  const { id } = await openAuction(context, entry, field, initial);
  await editField(page.field(id), draft);
  assertAuctionField(context, id, draft, false);
  await page.click("刷新记录");
  expect(page.text()).toContain("正在刷新记录");
  expect(context.displayed()).toBe(7);
  await reads.respond(err("network_error"));
  expect(page.text()).toContain("刷新记录失败");
  expect(context.displayed()).toBe(7);
  assertAuctionField(context, id, draft, false);
  await page.click("重试");
  await respondAuction(context, { ...auctionWire(), state_version: 8 });
  expect(context.displayed()).toBe(8);
  assertAuctionConflict(context, id, draft, submitLabel);
}

/** 打开真实行入口并核对可编辑的初值。 */
export async function openAuction(
  context: AuctionCase,
  entry: string,
  field: string,
  initial: string,
) {
  const { page } = context;
  await page.click(entry);
  const id = `${field}-${AUCTION_ID}`;
  assertAuctionField(context, id, initial, false);
  return { id, form: page.form() };
}

function assertAuctionField(context: AuctionCase, id: string, value: string, readOnly: boolean) {
  const { drafts, page } = context;
  switch (id) {
    case END_FIELD:
      expect(present(drafts.schedule.peek(), "实际排期草稿").end.value).toBe(value);
      break;
    case REASON_FIELD:
      expect(present(drafts.lifecycle.peek(), "实际生命周期草稿").reason).toBe(value);
      break;
    default:
      throw new Error(`未知的场次草稿控件 ${id}`);
  }
  assertField(page, id, value, readOnly);
}

/** 拒绝后只保留旧草稿与对照值，移除提交控件和未决命令。 */
export function assertAuctionConflict(
  context: AuctionCase,
  id: string,
  value: string,
  submitLabel: string,
): void {
  const { page, drafts } = context;
  expect(page.textNodes().some((text) => text.includes("本次修改没有生效"))).toBe(true);
  assertEditorSubmitRemoved(page, submitLabel);
  expect(context.occupied()).toBe(false);
  expect(drafts.auctionId()).toBe(AUCTION_ID);
  assertAuctionField(context, id, value, true);
}

/** 将 Rust 列表快照交付真实读取逻辑。 */
export function respondAuction(context: AuctionCase, value: unknown): Promise<void> {
  return context.reads.respond(ok(success(decode(AdminAuctionResponseSchema, value))));
}

/** 冲突读已完成时关闭草稿，无须再发一次读取。 */
export async function closeAfterAuctionRead(
  context: AuctionCase,
  closeLabel: string,
  entry: string,
  latest: ReturnType<typeof auctionWire>,
): Promise<void> {
  expect(context.reads.started()).toBe(2);
  await respondAuction(context, latest);
  expect(context.displayed()).toBe(latest.state_version);
  expect(context.reads.started()).toBe(2);
  await context.page.click(closeLabel);
  expect(context.drafts.auctionId()).toBeNull();
  expect(context.reads.started()).toBe(2);
  expect(context.displayed()).toBe(latest.state_version);
  expect(present(context.page.button(entry), "新版场次行入口").hasAttribute("disabled")).toBe(
    false,
  );
}

/** 通过真实表单提交，处理成功回执后收起草稿并取出唯一写请求。 */
export async function acceptAuctionCommand(
  context: AuctionCase,
  form: ReturnType<EditorTest["form"]>,
  auction: unknown,
) {
  context.page.reply(
    200,
    JSON.stringify({ code: "ok", message: "操作成功", request_id: null, data: { auction } }),
  );
  await submitForm(form);
  expect(context.drafts.auctionId()).toBeNull();
  return takeWrite(context.page);
}

/** 对照 Rust 逐字检查排期请求方法、路径、正文及字段顺序。 */
export function assertScheduleBody(
  request: CapturedWrite,
  version: number,
  preview: string,
  open: string,
  end: string,
): void {
  const id = commandId(request);
  expect(request).toEqual({
    method: "PATCH",
    path: `/api/v1/admin/auctions/${AUCTION_ID}/schedule`,
    body: `{"command_id":"${id}","preview_scheduled_at":"2099-04-01T${preview}Z","scheduled_open_at":"2099-04-01T${open}Z","scheduled_end_at":"2099-04-01T${end}Z","expected_state_version":${version}}`,
  });
}

test("场次快照供不同用例独立修改，原始快照保留兼容缺字段", () => {
  const wire = auctionWire();
  wire.state_version += 1;
  expect(auctionWire()).toMatchObject({ id: AUCTION_ID, state_version: 7 });
  expect(wire).not.toHaveProperty("lot_count");
});
