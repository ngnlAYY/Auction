// 真实拍品页订阅会话写入，并按有效列表响应确认录入通知的世界。

import { expect, test } from "bun:test";
import { parseJson } from "@auction/web-shared/net/wire";
import { createCell } from "@auction/web-shared/reactive/cell";
import { ok } from "@auction/web-shared/result";
import type { Accessor } from "solid-js";
import { For, untrack } from "solid-js";
import { jsonObject } from "../../api/api-request-fixture";
import { AUCTION_ID, fixtureText, success } from "../../api/auction-contract-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import {
  commandId,
  type EditorScope,
  type EditorTest,
  editField,
  present,
  settleEditor,
  submitForm,
  takeWrite,
  withEditor,
} from "../auctions/editor-fixture.test";
import { createComponent } from "../solid-dom-fixture";

const OTHER_AUCTION = "0190f2b4-1c2d-4e5f-8a6b-7c8d9e0f1a21";
const VISIBLE_LOT = "00000000-0000-4000-8000-000000000023";
const HIDDEN_LOT = "00000000-0000-4000-8000-000000000024";
const REPRICE = "改起拍价";
const EQUIPMENT_URL = "https://hentaiverse.org/equip/123/0123456789";

function lotsPath(id = AUCTION_ID, page = 1): string {
  return `/api/v1/admin/auctions/${id}/lots?page=${page}&page_size=50`;
}

function lotsResponse({
  id = AUCTION_ID,
  world = "main",
  page = 1,
  total = 1,
  empty = false,
  status = "draft",
  name = "Legendary Axe",
  lotStatus = "pending",
  manualIntake = false,
} = {}): Response {
  const envelope = jsonObject(success(parseJson(fixtureText("lots"))), "拍品列表");
  const data = jsonObject(envelope.data, "data");
  const auction = jsonObject(data.auction, "auction");
  if (!Array.isArray(data.lots)) throw new Error("拍品列表样本缺少行");
  const lot = jsonObject(data.lots[0], "拍品行");
  return Response.json(
    {
      ...envelope,
      data: {
        auction: { ...auction, id, world_id: world, status },
        lots: empty ? [] : [{ ...lot, auction_id: id, equipment_name: name, status: lotStatus }],
        pagination: { page, page_size: 50, total },
      },
    },
    { headers: { "x-auction-capability-manual-intake": String(manualIntake) } },
  );
}

interface LotsTest {
  readonly page: EditorTest;
  readonly owner: EditorScope["owner"];
  readonly paths: () => string[];
  readonly respond: (index: number, response: Response) => Promise<void>;
  readonly aborted: (index: number) => boolean;
  readonly switchAuction: (id: string) => Promise<void>;
}

async function withLots(run: (context: LotsTest) => Promise<void>): Promise<void> {
  const { page: lots } = await loadRenderModule<typeof import("./page")>(
    `${import.meta.dir}/page.tsx`,
    "page",
  );
  const selected = createCell(AUCTION_ID);
  const reads: {
    path: string;
    signal: AbortSignal | null;
    resolve: (value: Response) => void;
  }[] = [];
  let mounted: EditorScope | undefined;
  await withEditor(
    (scope) => {
      mounted = scope;
      return createComponent(For, {
        get each() {
          return [selected()];
        },
        keyed: (id: string) => id,
        children: (id: Accessor<string>) => lots(untrack(id)),
      });
    },
    (page) =>
      run({
        page,
        ...present(mounted, "来源页命令宿主"),
        paths: () => reads.map((read) => read.path),
        aborted: (index) => present(reads[index], "旧 GET").signal?.aborted === true,
        async respond(index, response) {
          present(reads[index], "已发出的列表 GET").resolve(response);
          await settleEditor();
        },
        async switchAuction(id) {
          selected.set(id);
          await settleEditor();
        },
      }),
    (path, signal) => {
      const response = Promise.withResolvers<Response>();
      reads.push({ path, signal, resolve: response.resolve });
      return response.promise;
    },
  );
}

async function settleCommand({ owner }: LotsTest, entity: string): Promise<void> {
  let delivered = 0;
  expect(
    owner.submit(
      entity,
      "已在其他视图提交的命令",
      async () => ok(null),
      () => {
        delivered += 1;
      },
      () => {
        throw new Error("预期写入成功");
      },
    ),
  ).toBe(true);
  await settleEditor();
  expect(delivered).toBe(1);
}

test("first_lots_read_coalesces_matching_intake_and_hides_the_old_edit_entry", () =>
  withLots(async (context) => {
    const { page, paths, respond } = context;
    expect(paths()).toEqual([lotsPath()]);
    await settleCommand(context, "intake:main");
    await settleCommand(context, "intake:main");
    await settleCommand(context, "intake:isekai");
    expect(paths()).toEqual([lotsPath()]);
    await respond(0, lotsResponse({ name: "录入前的拍品" }));
    expect(paths()).toEqual([lotsPath(), lotsPath()]);
    expect(page.text()).not.toContain("录入前的拍品");
    expect(page.button(REPRICE)).toBeUndefined();
    await respond(1, lotsResponse({ name: "录入后的拍品" }));
    expect(page.text()).toContain("录入后的拍品");
    expect(page.button(REPRICE)?.hasAttribute("disabled")).toBe(false);
    expect(paths()).toEqual([lotsPath(), lotsPath()]);
  }));

test("first_lots_read_does_not_reload_for_only_foreign_intake", () =>
  withLots(async (context) => {
    await settleCommand(context, "intake:isekai");
    await context.respond(0, lotsResponse());
    expect(context.page.text()).toContain("Legendary Axe");
    expect(context.paths()).toEqual([lotsPath()]);
  }));

test("failed_or_wrong_auction_reads_leave_intake_pending_until_a_valid_empty_list", () =>
  withLots(async (context) => {
    const { page, paths, respond } = context;
    await settleCommand(context, "intake:main");
    await respond(0, new Response("", { status: 503 }));
    expect(paths()).toEqual([lotsPath()]);
    await page.click("重试");
    await respond(1, lotsResponse({ id: OTHER_AUCTION, world: "isekai", empty: true, total: 0 }));
    expect(paths()).toEqual([lotsPath(), lotsPath()]);
    await page.click("重试");
    await respond(2, lotsResponse({ empty: true, total: 0 }));
    expect(paths()).toEqual(Array.from({ length: 4 }, () => lotsPath()));
    await respond(3, lotsResponse({ empty: true, total: 0 }));
    expect(paths()).toHaveLength(4);
  }));

test("matching_intake_reloads_the_current_page_after_pagination_and_read_failure", () =>
  withLots(async (context) => {
    const { page, paths, respond, aborted } = context;
    await respond(0, lotsResponse({ total: 51 }));
    await page.click("下一页");
    expect(paths()).toEqual([lotsPath(), lotsPath(AUCTION_ID, 2)]);
    await settleCommand(context, "intake:isekai");
    expect(paths()).toHaveLength(2);
    await settleCommand(context, "intake:main");
    expect(aborted(1)).toBe(true);
    await respond(2, new Response("", { status: 503 }));
    await settleCommand(context, "intake:main");
    expect(paths()).toEqual([
      lotsPath(),
      ...Array.from({ length: 3 }, () => lotsPath(AUCTION_ID, 2)),
    ]);
    await respond(3, lotsResponse({ page: 2, total: 51, name: "可用的第二页" }));
    await respond(1, lotsResponse({ page: 2, total: 51, name: "已过期的第二页" }));
    expect(page.text()).not.toContain("已过期的第二页");
    expect(page.text()).toContain("可用的第二页");
  }));

test("auction_and_visible_lot_receipts_reload_but_foreign_entities_do_not", () =>
  withLots(async (context) => {
    const { page, paths, respond, aborted } = context;
    await settleCommand(context, `auction:${OTHER_AUCTION}`);
    expect(paths()).toHaveLength(1);
    await settleCommand(context, "intake:main");
    await settleCommand(context, `auction:${AUCTION_ID}`);
    expect(paths()).toEqual([lotsPath(), lotsPath()]);
    expect(aborted(0)).toBe(true);
    await respond(0, lotsResponse({ world: "isekai", name: "已过期的首读" }));
    expect(page.text()).not.toContain("已过期的首读");
    await respond(1, lotsResponse());
    expect(paths()).toHaveLength(3);
    expect(page.text()).not.toContain("已过期的首读");
    await respond(2, lotsResponse());
    expect(page.button(REPRICE)?.hasAttribute("disabled")).toBe(false);
    await settleCommand(context, `lot:${HIDDEN_LOT}`);
    expect(paths()).toHaveLength(3);
    for (const [name, lotStatus] of [
      ["改价后的拍品", "pending"],
      ["撤拍后的拍品", "withdrawn"],
    ] as const) {
      await settleCommand(context, `lot:${VISIBLE_LOT}`);
      const read = paths().length - 1;
      await respond(read, lotsResponse({ name, lotStatus }));
      expect(page.text()).toContain(name);
    }
    expect(paths()).toHaveLength(5);
    expect(page.button(REPRICE)).toBeUndefined();
    await settleCommand(context, `auction:${AUCTION_ID}`);
    expect(paths()).toHaveLength(6);
    await respond(5, lotsResponse({ status: "preview", name: "预览中的拍品" }));
    expect(page.text()).toContain("预览中的拍品");
    expect(page.button(REPRICE)).toBeUndefined();
  }));

test("switching_auctions_drops_the_previous_pages_buffered_intake", () =>
  withLots(async (context) => {
    const { page, paths, respond, switchAuction, aborted } = context;
    await settleCommand(context, "intake:main");
    await switchAuction(OTHER_AUCTION);
    expect(aborted(0)).toBe(true);
    await respond(1, lotsResponse({ id: OTHER_AUCTION, name: "新场次的拍品" }));
    await respond(0, lotsResponse({ name: "已经卸载的场次" }));
    expect(page.text()).toContain("新场次的拍品");
    expect(page.text()).not.toContain("已经卸载的场次");
    expect(paths()).toEqual([lotsPath(), lotsPath(OTHER_AUCTION)]);
  }));

async function submitIntake(page: EditorTest): Promise<void> {
  await editField(page.field("auction-intake-url"), EQUIPMENT_URL);
  await editField(page.field("auction-intake-price"), "1000");
  await editField(page.field("auction-intake-player"), "Player");
  const [form] = page.find(
    (node) => node.tag === "form" && node.getAttribute("class") === "admin-auction-intake-form",
  );
  await submitForm(present(form, "手工录入表单"));
}

test("invalid_intake_receipt_waits_for_the_identical_retry_before_reloading", () =>
  withLots(async (context) => {
    const { page, owner, paths, respond } = context;
    await respond(0, lotsResponse({ manualIntake: true }));
    page.reply(
      201,
      fixtureText("lot-intake-command").replace('"start_price": 1000', '"start_price": 1001'),
    );
    await submitIntake(page);
    const first = takeWrite(page);
    expect(first).toEqual({
      method: "POST",
      path: "/api/v1/admin/worlds/main/auction-intake/items",
      body: JSON.stringify({
        command_id: commandId(first),
        source_kind: "url",
        equipment_url: EQUIPMENT_URL,
        equipment: null,
        start_price: 1000,
        player_id: "Player",
      }),
    });
    expect(owner.host.notices.peek()).toEqual([]);
    expect(paths()).toEqual([lotsPath()]);
    const occupiedAfterMismatch = owner.contains("intake:main");
    page.reply(201, fixtureText("lot-intake-command"));
    owner.host.retry(commandId(first));
    await settleEditor();
    expect(takeWrite(page)).toEqual(first);
    expect(paths()).toEqual([lotsPath(), lotsPath()]);
    expect(occupiedAfterMismatch).toBe(true);
    expect(owner.host.notices.peek()).toHaveLength(1);
    expect(owner.host.notices.peek()[0]).toMatchObject({
      identity: { commandId: commandId(first), entity: "intake:main" },
      result: { ok: true, value: { kind: "text", text: expect.stringContaining("已添加拍卖") } },
    });
    expect(owner.contains("intake:main")).toBe(false);
    await respond(1, lotsResponse());
  }));

test("unknown_intake_locks_only_the_original_world_form", () =>
  withLots(async (context) => {
    const { page, owner, paths, respond, switchAuction } = context;
    await respond(0, lotsResponse({ manualIntake: true }));
    expect(page.field("auction-intake-url").hasAttribute("disabled")).toBe(false);
    page.reply(503, "");
    await submitIntake(page);
    expect(takeWrite(page).path).toBe("/api/v1/admin/worlds/main/auction-intake/items");
    expect(page.field("auction-intake-url").hasAttribute("disabled")).toBe(true);
    expect(owner.contains("intake:main")).toBe(true);
    expect(owner.host.notices.peek()).toEqual([]);
    expect(paths()).toEqual([lotsPath()]);
    await switchAuction(OTHER_AUCTION);
    await respond(1, lotsResponse({ id: OTHER_AUCTION, world: "isekai", manualIntake: true }));
    expect(owner.contains("intake:main")).toBe(true);
    expect(page.field("auction-intake-url").hasAttribute("disabled")).toBe(false);
    expect(paths()).toEqual([lotsPath(), lotsPath(OTHER_AUCTION)]);
  }));
