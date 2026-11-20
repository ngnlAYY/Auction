// 用户列表与写命令的实际 schema 和回执解码，直接读取 Rust 契约样本。

import { expect, test } from "bun:test";
import {
  ADMIN_USER_MUTATION_WITH_KEY_RESPONSE_FIELD_POLICY,
  ADMIN_USER_MUTATION_WITHOUT_KEY_RESPONSE_FIELD_POLICY,
  AdminUserMutationResponseWithKeySchema,
  AdminUserMutationResponseWithoutKeySchema,
} from "@auction/contracts/dto/admin/users";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { err } from "@auction/web-shared/result";
import { createAdminRequestHarness, parseJsonObject, setAt } from "../api-request-fixture";
import { decodeAdminReceipt } from "../failure";
import {
  adminFixtureText,
  bufferedAdminResponse,
  mustSucceed,
  withoutFixtureField,
} from "../t10-contract-fixture.test";
import { decodeUserList, decodeUserMutation } from "./decode";

test("users_decode_backend_contract_fixtures", () => {
  for (const [name, count] of [
    ["users", 0],
    ["users-blacklisted", 1],
  ] as const) {
    const page = mustSucceed(decodeUserList(bufferedAdminResponse(adminFixtureText(name))));
    expect(page.users[0]?.blacklist_history).toHaveLength(count);
  }
});

test("rename_revert_contract_fixture", async () => {
  const text = adminFixtureText("rename-revert-command");
  const response = bufferedAdminResponse(text);
  const decodeReceipt = (input: string) =>
    decodeAdminReceipt(
      bufferedAdminResponse(input),
      apiResponseSchema(AdminUserMutationResponseWithoutKeySchema),
      ADMIN_USER_MUTATION_WITHOUT_KEY_RESPONSE_FIELD_POLICY,
    );
  expect(decodeReceipt(text).ok).toBe(true);
  expect(decodeReceipt(withoutFixtureField(text, "/data/user/username"))).toEqual(
    err("invalid_response"),
  );

  const harness = createAdminRequestHarness();
  try {
    const { host } = harness.registerHost();
    const { user } = mustSucceed(
      await decodeUserMutation(host, response, false, new AbortController().signal),
    );
    expect(user.username).toBe("Previous");
    expect(user.state_version).toBe(4);
  } finally {
    harness.cleanup();
  }
});

test("generated_user_key_receipt_contract_fixture", async () => {
  const text = adminFixtureText("user-create-command");
  const response = bufferedAdminResponse(text);
  const decodeReceipt = (input: string) =>
    decodeAdminReceipt(
      bufferedAdminResponse(input),
      apiResponseSchema(AdminUserMutationResponseWithKeySchema),
      ADMIN_USER_MUTATION_WITH_KEY_RESPONSE_FIELD_POLICY,
    );
  const receipt = mustSucceed(decodeReceipt(text));
  expect(decodeReceipt(withoutFixtureField(text, "/data/generated_key"))).toEqual(
    err("invalid_response"),
  );

  const harness = createAdminRequestHarness();
  try {
    const { host } = harness.registerHost();
    const signal = new AbortController().signal;
    const { user, generated_key } = mustSucceed(
      await decodeUserMutation(host, response, true, signal),
    );
    expect(receipt.code).toBe("created");
    expect(user.username).toBe("ExamplePlayer");
    expect(user.state_version).toBe(1);
    expect(generated_key).toBe("A".repeat(43));

    // 保留回执结构，只改成不足 32 字节的 Key，以区分结构解码和语义校验。
    const invalid = parseJsonObject(text, "生成 Key 契约样本");
    setAt(invalid, "/data/generated_key", "A".repeat(31));
    const shortKey = JSON.stringify(invalid);
    expect(decodeReceipt(shortKey).ok).toBe(true);
    expect(await decodeUserMutation(host, bufferedAdminResponse(shortKey), true, signal)).toEqual(
      err("invalid_response"),
    );
  } finally {
    harness.cleanup();
  }
});
