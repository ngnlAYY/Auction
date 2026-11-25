// 挂单详情页的灭失办理入口：非终态才可办理、链接指向原 position、两个表单各只写一次，
// 未知写只能原样重送，版本落后不自动重试。

import { expect, test } from "bun:test";

import {
  installFixtureFetch,
  parseJsonObject,
  recordRequest,
  TEST_SESSION_BODY,
} from "../../api/api-request-fixture";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { requireElement, withAdminTree } from "../settlement/settlement-tree-fixture.test";
import { settle } from "../solid-dom-fixture";
import {
  adminDetail,
  DETAIL,
  DETAIL_PAGE,
  failure,
  heldListing,
  LISTING_ID,
  success,
} from "./fixtures";

type Tree = Parameters<Parameters<typeof withAdminTree>[1]>[0];
type View = Pick<Tree, "find">;

const POSITION = "10000000-0000-4000-8000-000000000010";
const PROOF = "10000000-0000-4000-8000-000000000011";
const SESSION = "/api/v1/admin/session";
const PROOFS = `/api/v2/admin/consignment/positions/${POSITION}/loss-proofs`;
const CONFIRM = `/api/v2/admin/consignment/positions/${POSITION}/confirm-loss`;
const LOSS_PAGE = `/consignment/positions/${POSITION}/loss-case`;
const NOTE = "运营夹具：灭失证据说明";
const REASON = "运营夹具：确认灭失说明";

function listing(state: "in_custody" | "lost" = "in_custody") {
  return {
    ...heldListing(),
    position: { position_id: POSITION, state, version: 2, generation: 1 },
  };
}

function proofReceipt(commandId: string) {
  return {
    schema_version: 2,
    operation: "record_position_loss_proof",
    disposition: "loss_proof_recorded",
    command_id: commandId,
    position_id: POSITION,
    proof_id: PROOF,
    proof_digest: "b".repeat(64),
  };
}

function confirmReceipt(commandId: string) {
  return {
    schema_version: 2,
    operation: "confirm_position_loss",
    disposition: "position_loss_recorded",
    command_id: commandId,
    listing_id: LISTING_ID,
    listing_version: 4,
    state: "closed",
    position_id: POSITION,
    position_version: 3,
    loss_fact_id: "10000000-0000-4000-8000-000000000012",
    loss_case_id: "10000000-0000-4000-8000-000000000013",
    seller_receipt_id: "10000000-0000-4000-8000-000000000014",
  };
}

function commandId(body: string): string {
  const id = parseJsonObject(body, "灭失命令").command_id;
  if (typeof id !== "string") throw new Error("内存命令没有 ID");
  return id;
}

function reply(value: { readonly status?: number; readonly body: string }): Response {
  return new Response(value.body, { status: value.status ?? 200 });
}

function button(tree: View, text: string) {
  return tree.find((node) => node.tag === "button" && node.textContent === text);
}

function present(tree: View, text: string): boolean {
  return tree.find((node) => node.textContent.includes(text)) !== null;
}

function byId(tree: View, id: string) {
  return requireElement(
    tree.find((node) => node.getAttribute("id") === id),
    id,
  );
}

function form(tree: View) {
  return requireElement(
    tree.find((node) => node.tag === "form"),
    "办理表单",
  );
}

function submitProof(tree: View): void {
  byId(tree, "loss-proof-source").input("fake-source");
  byId(tree, "loss-proof-observation").input("fake-observation-1");
  byId(tree, "loss-proof-last-known").input("fake bot inventory slot");
  byId(tree, "loss-proof-note").input(NOTE);
  form(tree).submit();
}

function submitConfirm(tree: View): void {
  byId(tree, "loss-confirm-reason").input(REASON);
  form(tree).submit();
}

interface Server {
  readonly proofs: string[];
  readonly confirms: string[];
}

/** 内存服务端：详情恒为同一 position，确认按 confirmReplies 依次应答（缺省成功）。 */
async function withEntry(
  state: "in_custody" | "lost",
  confirmReplies: ReadonlyArray<{ readonly status?: number; readonly body: string } | null>,
  run: (tree: Tree, server: Server) => Promise<void>,
): Promise<void> {
  await withAdminTree(DETAIL_PAGE, async (tree) => {
    const { detailPage } = await loadRenderModule<typeof import("./detail")>(
      `${import.meta.dir}/detail.tsx`,
      "detailPage",
    );
    tree.requests.installBrowserRequest();
    const server: Server = { proofs: [], confirms: [] };
    const restore = installFixtureFetch(async (input, init) => {
      const request = recordRequest(input, init);
      switch (new URL(request.url, "https://auction.test").pathname) {
        case DETAIL:
          return reply(success(adminDetail(listing(state))));
        case SESSION:
          return Response.json(TEST_SESSION_BODY);
        case PROOFS:
          server.proofs.push(request.body);
          return reply(success(proofReceipt(commandId(request.body))));
        case CONFIRM: {
          server.confirms.push(request.body);
          const planned = confirmReplies[server.confirms.length - 1] ?? null;
          return reply(planned ?? success(confirmReceipt(commandId(request.body))));
        }
        default:
          throw new Error("未预期的内存请求");
      }
    });
    try {
      tree.mount(() => detailPage({ params: { listing_id: LISTING_ID }, query: {} }));
      await settle();
      await run(tree, server);
    } finally {
      restore();
    }
  });
}

test("仍占用实物的 position 才显示办理入口；链接指向该 position 的损失页", async () => {
  await withEntry("in_custody", [], async (tree) => {
    const link = tree.find((node) => node.tag === "a" && node.getAttribute("href") === LOSS_PAGE);
    expect(link).not.toBeNull();
    expect(button(tree, "录入灭失证据")).not.toBeNull();
  });
  await withEntry("lost", [], async (tree) => {
    const link = tree.find((node) => node.tag === "a" && node.getAttribute("href") === LOSS_PAGE);
    expect(link).not.toBeNull();
    expect(button(tree, "录入灭失证据")).toBeNull();
    expect(tree.find((node) => node.tag === "form")).toBeNull();
  });
});

test("录入证据与确认灭失各只写一次；确认带回执证据与当前版本，成功后进入损失页", async () => {
  await withEntry("in_custody", [], async (tree, server) => {
    submitProof(tree);
    await settle();
    expect(server.proofs).toHaveLength(1);
    expect(parseJsonObject(server.proofs[0] ?? "", "证据命令")).toEqual({
      command_id: expect.stringMatching(/^[0-9a-f-]{36}$/u),
      source_kind: "fake-source",
      observation_ref: "fake-observation-1",
      last_known_ref: "fake bot inventory slot",
      private_note: NOTE,
    });
    // 私有说明不回显。
    expect(present(tree, NOTE)).toBe(false);
    expect(server.confirms).toHaveLength(0);

    submitConfirm(tree);
    await settle();
    expect(server.proofs).toHaveLength(1);
    expect(server.confirms).toHaveLength(1);
    expect(parseJsonObject(server.confirms[0] ?? "", "确认命令")).toEqual({
      command_id: expect.stringMatching(/^[0-9a-f-]{36}$/u),
      expected_version: 2,
      proof_id: PROOF,
      reason: REASON,
    });
    expect(commandId(server.confirms[0] ?? "")).not.toBe(commandId(server.proofs[0] ?? ""));
    expect(tree.router.currentLocationString()).toBe(LOSS_PAGE);
  });
});

test("确认结果未知：只能以原命令 ID 与原正文重送", async () => {
  await withEntry("in_custody", [failure(503, "outcome_unknown")], async (tree, server) => {
    submitProof(tree);
    await settle();
    submitConfirm(tree);
    await settle();
    expect(server.confirms).toHaveLength(1);
    expect(tree.router.currentLocationString()).toBe(DETAIL_PAGE);
    requireElement(button(tree, "原样重试"), "原样重试").click();
    await settle();
    expect(server.confirms).toHaveLength(2);
    expect(server.confirms[1]).toBe(server.confirms[0]);
    expect(tree.router.currentLocationString()).toBe(LOSS_PAGE);
  });
});

test("确认遇 stale_version：提示重读，不自动重试也不给原样重试", async () => {
  await withEntry("in_custody", [failure(409, "stale_version")], async (tree, server) => {
    submitProof(tree);
    await settle();
    submitConfirm(tree);
    await settle();
    await settle();
    expect(server.confirms).toHaveLength(1);
    expect(button(tree, "原样重试")).toBeNull();
    expect(present(tree, "挂单已变化，请刷新后确认")).toBe(true);
    expect(tree.router.currentLocationString()).toBe(DETAIL_PAGE);
  });
});
