// 凭据回执的结构解码、重启标志、版本与检查范围回归。

import { expect, test } from "bun:test";
import {
  ADMIN_CREDENTIAL_DELETE_RESPONSE_FIELD_POLICY,
  ADMIN_CREDENTIAL_MUTATION_RESPONSE_FIELD_POLICY,
  AdminCredentialCheckResponseSchema,
  AdminCredentialDeleteResponseSchema,
  AdminCredentialMutationResponseSchema,
  AdminCredentialResponseSchema,
} from "@auction/contracts/dto/admin/credentials";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { err } from "@auction/web-shared/result";
import { createAdminRequestHarness } from "../api-request-fixture";
import { decodeAdminReceipt } from "../failure";
import {
  adminFixtureText,
  bufferedAdminResponse,
  mustSucceed,
  withoutFixtureField,
} from "../t10-contract-fixture.test";
import { fetchAdminCredentials } from ".";
import { decodeCredentialCheck, decodeCredentialData, decodeCredentialDelete } from "./decode";

const CREDENTIAL_ID = "11111111-1111-4111-8111-111111111111";
const OBSERVED_AT = "2026-11-18T00:00:00Z";
const CREDENTIAL = {
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

test("credential_mutations_preserve_the_server_restart_flag", () => {
  for (const restartRequired of [true, false]) {
    const mutation = mustSucceed(
      decode(AdminCredentialMutationResponseSchema, {
        credential: CREDENTIAL,
        restart_required: restartRequired,
      }),
    );
    const deletion = mustSucceed(
      decode(AdminCredentialDeleteResponseSchema, {
        credential_id: CREDENTIAL_ID,
        state_version: 8,
        restart_required: restartRequired,
      }),
    );

    expect(mustSucceed(decodeCredentialData(mutation)).restart_required).toBe(restartRequired);
    expect(mustSucceed(decodeCredentialDelete(deletion, CREDENTIAL_ID, 7)).restart_required).toBe(
      restartRequired,
    );
    expect(decodeCredentialDelete(deletion, CREDENTIAL_ID, 8)).toEqual(err("credential_delete"));
    const invalid = {
      ...mutation,
      credential: { ...mutation.credential, status: "unknown" },
    };
    expect(decodeCredentialData(invalid)).toEqual(err("credential"));
  }
});

test("shared_mail_check_preserves_per_world_inconclusive_results", () => {
  const credential = mustSucceed(decode(AdminCredentialResponseSchema, CREDENTIAL));
  const payload = {
    credential_id: CREDENTIAL_ID,
    state_version: 7,
    checked_at: OBSERVED_AT,
    checks: [
      { world: "main", status: "inconclusive" },
      { world: "isekai", status: "inconclusive" },
    ] as const,
  };
  const check = mustSucceed(decode(AdminCredentialCheckResponseSchema, payload));
  const result = mustSucceed(decodeCredentialCheck(check, credential));
  expect(result.checks).toHaveLength(2);
  expect(result.checks.map((check) => check.status)).toEqual(["inconclusive", "inconclusive"]);

  const checks = [payload.checks[0], { ...payload.checks[1], world: "main" }];
  const duplicate = mustSucceed(decode(AdminCredentialCheckResponseSchema, { ...payload, checks }));
  expect(decodeCredentialCheck(duplicate, credential)).toEqual(err("invalid_response"));
});

test("credentials_decode_backend_contract_fixtures", async () => {
  for (const [name, kind, world, status] of [
    ["credentials", "mail_cookie", "main", "active"],
    ["credentials-forum", "forum_cookie", "shared", "disabled"],
  ] as const) {
    const harness = createAdminRequestHarness();
    try {
      harness.installBrowserRequest();
      const requests = harness.installFetch([{ body: adminFixtureText(name) }]);
      const { host } = harness.registerHost();
      const page = mustSucceed(
        await fetchAdminCredentials(host, kind, world, status, 1, 50, new AbortController().signal),
      );
      expect(requests).toEqual([
        {
          method: "GET",
          url: `/api/v1/admin/credentials?page=1&page_size=50&kind=${kind}&world=${world}&status=${status}`,
          body: "",
        },
      ]);
      const first = page.credentials[0];
      if (first === undefined) throw new Error(`样本 ${name} 没有首条凭据`);
      expect(first.player_name !== null).toBe(kind === "mail_cookie");
      expect(page.restart_required).toBe(true);
    } finally {
      harness.cleanup();
    }
  }
});

test("credential_command_contract_fixtures", () => {
  const mutation = adminFixtureText("credential-command");
  const decodeMutation = (text: string) =>
    decodeAdminReceipt(
      bufferedAdminResponse(text),
      apiResponseSchema(AdminCredentialMutationResponseSchema),
      ADMIN_CREDENTIAL_MUTATION_RESPONSE_FIELD_POLICY,
    );
  const mutationReceipt = mustSucceed(decodeMutation(mutation));
  expect(decodeMutation(withoutFixtureField(mutation, "/data/credential/label"))).toEqual(
    err("invalid_response"),
  );
  const result = mustSucceed(decodeCredentialData(mutationReceipt.data));
  expect(result.credential.label).toBe("New label");
  expect(result.restart_required).toBe(true);

  const deletion = adminFixtureText("credential-delete-command");
  const decodeDelete = (text: string) =>
    decodeAdminReceipt(
      bufferedAdminResponse(text),
      apiResponseSchema(AdminCredentialDeleteResponseSchema),
      ADMIN_CREDENTIAL_DELETE_RESPONSE_FIELD_POLICY,
    );
  const deleteReceipt = mustSucceed(decodeDelete(deletion));
  expect(decodeDelete(withoutFixtureField(deletion, "/data/credential_id"))).toEqual(
    err("invalid_response"),
  );
  const deleted = mustSucceed(
    decodeCredentialDelete(deleteReceipt.data, "00000000-0000-4000-8000-000000000025", 5),
  );
  expect(deleted.state_version).toBe(6);
  expect(deleted.restart_required).toBe(true);
});
