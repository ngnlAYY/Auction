// 对照 Rust 论坛同步状态、线程身份、链接和发布状态的回执检查。

import { expect, test } from "bun:test";
import {
  PublicForumResponseSchema,
  PublicForumSyncStatusResponseSchema,
} from "@auction/contracts/dto/public/catalog/forum";
import { apiResponseSchema } from "@auction/contracts/dto/response";
import { decode } from "@auction/contracts/wire";
import { err } from "@auction/web-shared/result";
import { jsonObjectAt, parseJsonObject } from "./api-request-fixture";
import {
  AUCTION_ID,
  checkedFixture,
  fixtureText,
  requestFixture,
  success,
  withoutField,
} from "./auction-contract-fixture.test";
import { fetchAdminForumStatus } from "./forum";

const SYNC = ["GET", "/api/v1/admin/forum/sync-status"] as const;

function readForum(sync: string, thread?: string, auctionId = AUCTION_ID, beacons = 0) {
  return requestFixture(
    thread === undefined ? [{ body: sync }] : [{ body: sync }, { body: thread }],
    thread === undefined ? [SYNC] : [SYNC, ["GET", `/api/v1/admin/auctions/${auctionId}/forum`]],
    (host, signal) => fetchAdminForumStatus(host, auctionId, signal),
    beacons,
  );
}

test("forum_contract_fixtures", async () => {
  const publishedBody = fixtureText("forum");
  for (const [name, enabled] of [
    ["forum-sync", true],
    ["forum-sync-disabled", false],
  ] as const) {
    const body = fixtureText(name);
    checkedFixture(body, PublicForumSyncStatusResponseSchema, "/data/enabled");
    expect(success(await readForum(body, enabled ? publishedBody : undefined)).enabled).toBe(
      enabled,
    );
    expect(await readForum(withoutField(body, "/data/enabled"), undefined, AUCTION_ID, 1)).toEqual({
      ok: false,
      error: "invalid_response",
    });
  }
  const enabledBody = fixtureText("forum-sync");
  for (const [name, status, published] of [
    ["forum", "published", true],
    ["forum-pending", "pending", false],
  ] as const) {
    const body = fixtureText(name);
    checkedFixture(body, PublicForumResponseSchema, "/data/source_revision");
    const { thread } = success(await readForum(enabledBody, body));
    if (thread === null) throw new Error("论坛状态缺少帖子投影");
    expect(thread).toMatchObject({ auction_id: AUCTION_ID, status });
    expect(thread.content_synced_at != null).toBe(published);
    expect(thread.thread_url != null).toBe(published);
    expect(
      await readForum(enabledBody, withoutField(body, "/data/source_revision"), AUCTION_ID, 1),
    ).toEqual({ ok: false, error: "invalid_response" });
  }
});

test("forum_response_binds_identity_url_and_publication", async () => {
  const sync = fixtureText("forum-sync");
  const body = fixtureText("forum");
  const invalidResponse = err("invalid_response");
  const schema = apiResponseSchema(PublicForumResponseSchema);
  expect(success(await readForum(sync, body)).thread).toMatchObject({ auction_id: AUCTION_ID });
  expect(await readForum(sync, body, "00000000-0000-4000-8000-000000000003", 1)).toEqual(
    invalidResponse,
  );
  for (const [key, value] of [
    ["thread_url", "http://forums.e-hentai.org/index.php?showtopic=123"],
    ["content_synced_at", null],
  ] as const) {
    const invalid = parseJsonObject(body, "论坛样本");
    jsonObjectAt(invalid, "/data")[key] = value;
    success(decode(schema, invalid));
    expect(await readForum(sync, JSON.stringify(invalid), AUCTION_ID, 1)).toEqual(invalidResponse);
  }
});
