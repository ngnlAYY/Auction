// 退件操作区使用的确定拒绝范围，以及必须维持未决状态的错误。

import { expect, test } from "bun:test";
import { AdminSettlementReturnDetailResponseSchema } from "@auction/contracts/dto/admin/returns";
import { decode } from "@auction/contracts/wire";
import { createCell } from "@auction/web-shared/reactive/cell";

import {
  installFixtureFetch,
  recordRequest,
  TEST_SESSION_BODY,
} from "../../api/api-request-fixture";
import { adminApiFailureDefiniteRejection as definiteRejection } from "../../api/failure";
import { CASE, returnDetail } from "../../api/returns-fixture";
import { adminContract, requireOk } from "../../api/settlement-fixture.test";
import { loadRenderModule } from "../../solid-render-fixture.test";
import { PendingCommandsContext } from "../pending";
import { usePendingCommands } from "../pending/sources";
import type { AdminActionFeedback } from "../presentation";
import { requireElement, withAdminTree } from "../settlement/settlement-tree-fixture.test";
import { createComponent, settle } from "../solid-dom-fixture";

test("typed_rejections_are_definite_and_unknown_outcomes_are_not", () => {
  for (const code of [
    "invalid_settlement_return_command",
    "invalid_settlement_return_id",
    "settlement_return_not_found",
    "stale_settlement_return_state",
    "command_conflict",
    "settlement_return_untrusted",
    "settlement_return_unbound",
    "settlement_return_assets_mismatch",
    "settlement_return_action_conflict",
    "settlement_return_proof_required",
    "settlement_return_resend_limit",
    "settlement_return_obligation_ineligible",
    "settlement_return_send_unresolved",
    "settlement_return_observation_required",
  ]) {
    expect(definiteRejection(code), code).toBe(true);
  }
  for (const code of [
    "temporarily_unavailable",
    "network_error",
    "timeout",
    "invalid_response",
    "http_503",
    "outcome_unknown",
  ]) {
    expect(definiteRejection(code), code).toBe(false);
  }
});

for (const scenario of ["写入拒绝", "写前网络失败"] as const) {
  test(`退件动作：${scenario}的显示码决定专属重读`, async () => {
    const { ActionSection } = await loadRenderModule<
      Pick<typeof import("./actions"), "ActionSection">
    >(`${import.meta.dir}/actions.tsx`, "ActionSection");
    const detail = requireOk(
      decode(AdminSettlementReturnDetailResponseSchema, returnDetail().data),
    );
    await withAdminTree(`/settlement-returns/${CASE}`, async (tree) => {
      tree.requests.installBrowserRequest();
      const refresh = createCell(0n);
      const sourceGeneration = createCell(0n);
      const notice = createCell<AdminActionFeedback | null>(null);
      const requests: ReturnType<typeof recordRequest>[] = [];
      const restore = installFixtureFetch(async (input, init) => {
        const request = recordRequest(input, init);
        requests.push(request);
        const url = new URL(request.url, "https://auction.test");
        if (url.pathname === "/api/v1/admin/session") {
          if (scenario === "写前网络失败") throw new TypeError("内存会话传输中断");
          return Response.json(TEST_SESSION_BODY);
        }
        if (url.pathname === `/api/v1/admin/settlement-returns/${CASE}/handle`) {
          return new Response(adminContract("error-stale-settlement-return-state"), {
            status: 409,
          });
        }
        throw new Error(`未预期的请求 ${request.url}`);
      });
      try {
        tree.mount(() => {
          // 分别观察退件动作和宿主广播的读取世代。
          const owner = usePendingCommands(sourceGeneration, () => true);
          return createComponent(PendingCommandsContext, {
            value: owner,
            get children() {
              return createComponent(ActionSection, {
                detail,
                notice,
                refresh,
                loadedGeneration: 0n,
                restoreFocus() {},
              });
            },
          });
        });
        await settle();
        for (const label of ["领回并补寄", "确认提交"]) {
          requireElement(
            tree.find((node) => node.tag === "button" && node.textContent === label),
            label,
          ).click();
          await settle();
        }
        expect(requests.map(({ method }) => method)).toEqual(
          scenario === "写入拒绝" ? ["GET", "POST"] : ["GET"],
        );
        expect(refresh.peek()).toBe(scenario === "写入拒绝" ? 1n : 0n);
        expect(sourceGeneration.peek()).toBe(1n);
        expect(notice.peek()).toEqual({
          kind: "error",
          code: scenario === "写入拒绝" ? "stale_settlement_return_state" : "network_error",
        });
      } finally {
        restore();
      }
    });
  });
}
