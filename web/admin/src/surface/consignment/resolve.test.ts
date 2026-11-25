// Admin 订单处置面板：未知写只能用原命令 ID 与原正文重放；确定拒绝不提供重放；动作入口是 wire 闭集的子集。

import { expect, test } from "bun:test";

import { ResolveOrderRequestSchema } from "@auction/contracts/dto/admin/consignment/order-resolve";
import { createCell } from "@auction/web-shared/reactive/cell";
import { is } from "valibot";

import {
  installFixtureFetch,
  parseJsonObject,
  recordRequest,
  TEST_SESSION_BODY,
} from "../../api/api-request-fixture";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { requireElement, withAdminTree } from "../settlement/settlement-tree-fixture.test";
import { settle } from "../solid-dom-fixture";
import { failure, success } from "./fixtures";

type Tree = Parameters<Parameters<typeof withAdminTree>[1]>[0];

const CONTRACT = "30000000-0000-4000-8000-000000000001";
const EVIDENCE = "30000000-0000-4000-8000-000000000002";
const SESSION = "/api/v1/admin/session";
const RESOLVE = `/api/v2/admin/consignment/orders/${CONTRACT}/resolve`;

function receipt(commandId: string) {
  return {
    schema_version: 2,
    operation: "resolve",
    disposition: "resolution_recorded",
    command_id: commandId,
    contract_id: CONTRACT,
    contract_version: 4,
    action: "refund_lost",
    resolution_id: "30000000-0000-4000-8000-000000000003",
  };
}

function path(request: ReturnType<typeof recordRequest>): string {
  return new URL(request.url, "https://auction.test").pathname;
}

function reply(value: { readonly status?: number; readonly body: string }): Response {
  return new Response(value.body, { status: value.status ?? 200 });
}

function commandId(body: string): string {
  const id = parseJsonObject(body, "订单处置命令").command_id;
  if (typeof id !== "string") throw new Error("内存命令没有 ID");
  return id;
}

function button(tree: Pick<Tree, "find">, text: string) {
  return tree.find((node) => node.tag === "button" && node.textContent === text);
}

function present(tree: Pick<Tree, "find">, text: string): boolean {
  return tree.find((node) => node.textContent.includes(text)) !== null;
}

async function withPanel(run: (tree: Tree, mount: () => void) => Promise<void>): Promise<void> {
  await withAdminTree(`/consignment/orders/${CONTRACT}`, async (tree) => {
    const { OrderResolvePanel } = await loadRenderModule<typeof import("./resolve")>(
      `${import.meta.dir}/resolve.tsx`,
      "OrderResolvePanel",
    );
    await run(tree, () =>
      tree.mount(() =>
        OrderResolvePanel({
          contractId: CONTRACT,
          version: 3,
          sale: true,
          refresh: createCell(0n),
        }),
      ),
    );
  });
}

function submitForm(tree: Tree, action: string): void {
  const select = requireElement(
    tree.find((node) => node.getAttribute("id") === "resolve-action"),
    "处置动作",
  );
  select.value = action;
  select.change();
  requireElement(
    tree.find((node) => node.getAttribute("id") === "resolve-evidence"),
    "依据 ID",
  ).input(EVIDENCE);
  requireElement(
    tree.find((node) => node.tag === "form"),
    "处置表单",
  ).submit();
}

test("未知写：保留原命令 ID 与原正文，只能原样重试", async () => {
  await withPanel(async (tree, mount) => {
    tree.requests.installBrowserRequest();
    const posts: string[] = [];
    const restore = installFixtureFetch(async (input, init) => {
      const request = recordRequest(input, init);
      switch (path(request)) {
        case SESSION:
          return Response.json(TEST_SESSION_BODY);
        case RESOLVE:
          posts.push(request.body);
          if (posts.length === 1) return reply(failure(503, "outcome_unknown"));
          return reply(success(receipt(commandId(request.body))));
        default:
          throw new Error("未预期的内存请求");
      }
    });
    try {
      mount();
      await settle();
      submitForm(tree, "refund_lost");
      await settle();
      expect(posts).toHaveLength(1);
      expect(parseJsonObject(posts[0] ?? "", "订单处置命令")).toEqual({
        command_id: expect.stringMatching(/^[0-9a-f-]{36}$/u),
        expected_version: 3,
        action: "refund_lost",
        evidence_id: EVIDENCE,
      });
      // 未决期间不能提交新命令（包括排入复核），只能原样重试。
      const submit = tree.find(
        (node) => node.tag === "button" && node.getAttribute("type") === "submit",
      );
      expect(submit?.disabled).toBe(true);
      requireElement(button(tree, "原样重试"), "原样重试").click();
      await settle();
      expect(posts).toHaveLength(2);
      expect(posts[1]).toEqual(posts[0]);
      expect(button(tree, "原样重试")).toBeNull();
      expect(present(tree, "处置已记录，订单版本 4。")).toBe(true);
    } finally {
      restore();
    }
  });
});

test("相反付款未了结：确定拒绝，显示专用提示且不提供重放", async () => {
  await withPanel(async (tree, mount) => {
    tree.requests.installBrowserRequest();
    let posts = 0;
    const restore = installFixtureFetch(async (input, init) => {
      const request = recordRequest(input, init);
      switch (path(request)) {
        case SESSION:
          return Response.json(TEST_SESSION_BODY);
        case RESOLVE:
          posts += 1;
          return reply(failure(409, "opposite_payment_open"));
        default:
          throw new Error("未预期的内存请求");
      }
    });
    try {
      mount();
      await settle();
      submitForm(tree, "refund_returned");
      await settle();
      expect(posts).toBe(1);
      expect(present(tree, "相反方向的付款仍未了结，不能转为退款。")).toBe(true);
      expect(button(tree, "原样重试")).toBeNull();
    } finally {
      restore();
    }
  });
});

test("表单动作是 wire 闭集的子集，且每个动作都能组成合法请求", async () => {
  await withPanel(async (tree, mount) => {
    tree.requests.installBrowserRequest();
    const restore = installFixtureFetch(async (input, init) => {
      const request = recordRequest(input, init);
      if (path(request) === SESSION) return Response.json(TEST_SESSION_BODY);
      throw new Error("未预期的内存请求");
    });
    try {
      mount();
      await settle();
      const actions = tree
        .elements((node) => node.tag === "option")
        .map((node) => node.getAttribute("value"));
      expect(actions).toEqual([
        "cancel_unpaid",
        "refund_returned",
        "refund_lost",
        "refund_season",
        "retain_entitlement",
        "redispatch_original",
      ]);
      for (const action of actions) {
        expect(
          is(ResolveOrderRequestSchema, {
            command_id: "30000000-0000-4000-8000-0000000000aa",
            expected_version: 1,
            action,
            evidence_id: EVIDENCE,
          }),
        ).toBe(true);
      }
    } finally {
      restore();
    }
  });
});
