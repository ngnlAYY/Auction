// 退件操作区使用的确定拒绝范围，以及必须维持未决状态的错误。

import { expect, test } from "bun:test";

import { definiteRejection } from "../pending";

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
