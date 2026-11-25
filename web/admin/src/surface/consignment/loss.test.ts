// Admin 损失案件页：版本落后重读且不重放，未知写只能用原命令 ID 与原正文重放。

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
import { failure, success } from "./fixtures";

type Tree = Parameters<Parameters<typeof withAdminTree>[1]>[0];

const POSITION = "20000000-0000-4000-8000-000000000001";
const CASE = "20000000-0000-4000-8000-000000000002";
const BASIS = "20000000-0000-4000-8000-000000000003";
const SESSION = "/api/v1/admin/session";
const CASE_PATH = `/api/v2/admin/consignment/positions/${POSITION}/loss-case`;
const OUTCOMES = `${CASE_PATH}/outcomes`;
const NOTE = "运营夹具：复核中";

function lossCase(version: number) {
  const opened = {
    outcome_id: "20000000-0000-4000-8000-000000000010",
    case_version: 1,
    state: "opened",
    basis_id: null,
    private_note: null,
    occurred_at: "2026-11-21T00:00:00Z",
    seller_receipt_id: "20000000-0000-4000-8000-000000000011",
  };
  const reviewing = {
    outcome_id: "20000000-0000-4000-8000-000000000012",
    case_version: 2,
    state: "reviewing",
    basis_id: BASIS,
    private_note: NOTE,
    occurred_at: "2026-11-21T00:10:00Z",
    seller_receipt_id: "20000000-0000-4000-8000-000000000013",
  };
  return {
    position_id: POSITION,
    case_id: CASE,
    loss_fact_id: "20000000-0000-4000-8000-000000000020",
    proof_id: "20000000-0000-4000-8000-000000000021",
    proof_source_kind: "fake-source",
    proof_digest: "a".repeat(64),
    listing_id: "20000000-0000-4000-8000-000000000022",
    seller_user_id: "20000000-0000-4000-8000-000000000023",
    generation: 1,
    case_version: version,
    state: version === 1 ? "opened" : "reviewing",
    outcomes: version === 1 ? [opened] : [opened, reviewing],
  };
}

function receipt(commandId: string) {
  return {
    schema_version: 2,
    operation: "record_loss_case_outcome",
    disposition: "case_outcome_recorded",
    command_id: commandId,
    case_id: CASE,
    case_version: 2,
    outcome_id: "20000000-0000-4000-8000-000000000012",
    seller_receipt_id: "20000000-0000-4000-8000-000000000013",
  };
}

function path(request: ReturnType<typeof recordRequest>): string {
  return new URL(request.url, "https://auction.test").pathname;
}

function reply(value: { readonly status?: number; readonly body: string }): Response {
  return new Response(value.body, { status: value.status ?? 200 });
}

function commandId(body: string): string {
  const id = parseJsonObject(body, "人工结果命令").command_id;
  if (typeof id !== "string") throw new Error("内存命令没有 ID");
  return id;
}

function button(tree: Pick<Tree, "find">, text: string) {
  return tree.find((node) => node.tag === "button" && node.textContent === text);
}

function present(tree: Pick<Tree, "find">, text: string): boolean {
  return tree.find((node) => node.textContent.includes(text)) !== null;
}

async function withCase(run: (tree: Tree, mount: () => void) => Promise<void>): Promise<void> {
  await withAdminTree(`/consignment/positions/${POSITION}/loss-case`, async (tree) => {
    const { lossCasePage } = await loadRenderModule<typeof import("./loss")>(
      `${import.meta.dir}/loss.tsx`,
      "lossCasePage",
    );
    await run(tree, () =>
      tree.mount(() => lossCasePage({ params: { position_id: POSITION }, query: {} })),
    );
  });
}

function submitForm(tree: Tree): void {
  requireElement(
    tree.find((node) => node.getAttribute("id") === "loss-basis"),
    "依据 ID",
  ).input(BASIS);
  requireElement(
    tree.find((node) => node.getAttribute("id") === "loss-note"),
    "私有说明",
  ).input(NOTE);
  requireElement(
    tree.find((node) => node.tag === "form"),
    "人工结果表单",
  ).submit();
}

test("版本落后：提示刷新并重读案件，不用旧版本重放", async () => {
  await withCase(async (tree, mount) => {
    tree.requests.installBrowserRequest();
    const posts: string[] = [];
    let reads = 0;
    const restore = installFixtureFetch(async (input, init) => {
      const request = recordRequest(input, init);
      switch (path(request)) {
        case CASE_PATH:
          reads += 1;
          return reply(success(lossCase(reads === 1 ? 1 : 2)));
        case SESSION:
          return Response.json(TEST_SESSION_BODY);
        case OUTCOMES:
          posts.push(request.body);
          return reply(failure(409, "stale_version"));
        default:
          throw new Error("未预期的内存请求");
      }
    });
    try {
      mount();
      await settle();
      submitForm(tree);
      await settle();
      expect(posts).toHaveLength(1);
      expect(parseJsonObject(posts[0] ?? "", "人工结果命令")).toEqual({
        command_id: expect.stringMatching(/^[0-9a-f-]{36}$/u),
        case_id: CASE,
        expected_case_version: 1,
        outcome: "reviewing",
        basis_id: BASIS,
        private_note: NOTE,
      });
      expect(present(tree, "案件版本已变化，请刷新后确认。")).toBe(true);
      expect(button(tree, "原样重试")).toBeNull();
      expect(reads).toBe(2);
    } finally {
      restore();
    }
  });
});

test("未知写：保留原命令 ID 与原正文，只能原样重试", async () => {
  await withCase(async (tree, mount) => {
    tree.requests.installBrowserRequest();
    const posts: string[] = [];
    const restore = installFixtureFetch(async (input, init) => {
      const request = recordRequest(input, init);
      switch (path(request)) {
        case CASE_PATH:
          return reply(success(lossCase(posts.length < 2 ? 1 : 2)));
        case SESSION:
          return Response.json(TEST_SESSION_BODY);
        case OUTCOMES:
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
      submitForm(tree);
      await settle();
      expect(posts).toHaveLength(1);
      // 未决期间不能提交新命令，只能原样重试。
      const submit = tree.find(
        (node) => node.tag === "button" && node.getAttribute("type") === "submit",
      );
      expect(submit?.disabled).toBe(true);
      requireElement(button(tree, "原样重试"), "原样重试").click();
      await settle();
      expect(posts).toHaveLength(2);
      expect(posts[1]).toEqual(posts[0]);
      expect(button(tree, "原样重试")).toBeNull();
      expect(present(tree, "人工结果已记录，案件版本 2。")).toBe(true);
    } finally {
      restore();
    }
  });
});
