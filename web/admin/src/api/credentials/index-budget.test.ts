// 凭据只读检查经实际接口把预算交给写传输。

import { expect, spyOn, test } from "bun:test";
import {
  createAdminRequestHarness,
  parseJsonObject,
  TEST_SESSION_BODY,
} from "../api-request-fixture";
import { mustSucceed } from "../t10-contract-fixture.test";
import { type AdminCredential, checkAdminCredential } from ".";

const CREDENTIAL_ID = "11111111-1111-4111-8111-111111111111";
const OBSERVED_AT = "2026-11-18T00:00:00Z";
const CHECK_PATH = `/api/v1/admin/credentials/${CREDENTIAL_ID}/check`;

test("only_shared_mail_check_extends_the_browser_budget", async () => {
  const transport = await import("../transport");
  const credential: AdminCredential = {
    id: CREDENTIAL_ID,
    kind: "mail_cookie",
    world: "shared",
    label: "Worker",
    player_name: null,
    status: "active",
    state_version: 7,
    created_at: OBSERVED_AT,
    rotated_at: null,
    updated_at: OBSERVED_AT,
  };
  for (const [input, worlds, budget] of [
    [credential, ["main", "isekai"], 18_000],
    [{ ...credential, world: "main" }, ["main"], 10_000],
    [{ ...credential, kind: "forum_cookie" }, ["shared"], 10_000],
  ] as const) {
    const harness = createAdminRequestHarness();
    let restoreWrite: (() => void) | undefined;
    try {
      const write = spyOn(transport, "sendPreparedJsonWrite");
      restoreWrite = () => write.mockRestore();
      harness.installBrowserRequest();
      const { host } = harness.registerHost();
      const requests = harness.installFetch([
        { body: JSON.stringify(TEST_SESSION_BODY) },
        {
          body: JSON.stringify({
            code: "ok",
            message: "",
            data: {
              credential_id: CREDENTIAL_ID,
              state_version: 7,
              checked_at: OBSERVED_AT,
              checks: worlds.map((world) => ({ world, status: "inconclusive" })),
            },
          }),
        },
      ]);
      mustSucceed(await checkAdminCredential(host, input, new AbortController().signal));
      expect(write).toHaveBeenCalledTimes(1);
      expect(write.mock.calls[0]?.[5]).toBe(budget);
      expect(requests.map(({ method, url }) => [method, url])).toEqual([
        ["GET", "/api/v1/admin/session?include=server_time"],
        ["POST", CHECK_PATH],
      ]);
      const check = requests[1];
      if (check === undefined) throw new Error("缺少凭据检查请求");
      expect(parseJsonObject(check.body, "凭据检查正文")).toEqual({ expected_state_version: 7 });
    } finally {
      try {
        restoreWrite?.();
      } finally {
        harness.cleanup();
      }
    }
  }
});
