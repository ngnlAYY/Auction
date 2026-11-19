// 会话及私有响应的身份证据须与各自的请求结果分开。

import { expect, test } from "bun:test";
import { err, ok } from "@auction/web-shared/result";
import { flush } from "solid-js";
import { loginPublic } from "../../api/session";
import { publicWatchCacheRevision } from "../../api/session/revision";
import { fetchSessionPreference, setSessionPreference } from "../../api/session-preference";
import { fetchLotWatchPreference, setLotWatchPreference } from "../../api/watch-preference";
import { fetchLotWatches, setLotWatch } from "../../api/watches";
import { load, save } from "../watches/cache";
import { ALICE, BOB, fixture, loginReceipt, reject, success } from "./session-fixture";

const AUCTION_ID = "00000000-0000-4000-8000-000000000021";
const LOT_ID = "00000000-0000-4000-8000-000000000022";
const COMMAND_ID = "00000000-0000-4000-8000-000000000023";
const WATCH = { lot_id: LOT_ID, watched: true, channel: "mm", version: 1 };
const watches = (userId: string) => ({ user_id: userId, auction_id: AUCTION_ID, watches: [WATCH] });
const watchPreference = (userId: string) => ({
  user_id: userId,
  channel: "mm",
  version: 1,
  pm_available: false,
});
const sessionPreference = (userId: string) => ({
  user_id: userId,
  duration_seconds: null,
  version: 1,
  expires_at: null,
});

async function settle(): Promise<void> {
  await Bun.sleep(0);
  flush();
}

test.each([
  [
    "整场标记",
    (userId: string, signal: AbortSignal) => fetchLotWatches(userId, AUCTION_ID, signal),
    watches,
  ],
  ["被超提醒偏好", fetchLotWatchPreference, watchPreference],
  ["登录时长偏好", fetchSessionPreference, sessionPreference],
] as const)("%s 读到换号证据后，数据不交给旧账号", async (_name, read, payload) => {
  const mounted = fixture(ALICE);
  try {
    await settle();
    save(watches(ALICE.user_id), publicWatchCacheRevision());
    mounted.server.identity = BOB;
    mounted.server.read = () => reject("storage_unavailable", 503);
    mounted.server.mutate = () => success(payload(BOB.user_id));
    expect(await read(ALICE.user_id, mounted.signal)).toEqual(
      err({ code: "invalid_response", status: 200 }),
    );
    expect(mounted.commandOwner.peek()).toBe(BOB.user_id);
    expect(load(ALICE.user_id, AUCTION_ID)).toBeNull();
    await settle();
    expect(mounted.session()).toEqual(err("storage_unavailable"));
    expect(mounted.owners).toEqual([null, ALICE.user_id, BOB.user_id]);
  } finally {
    mounted.dispose();
  }
});

test.each([
  [
    "装备标记",
    (userId: string, signal: AbortSignal) =>
      setLotWatch(
        userId,
        AUCTION_ID,
        LOT_ID,
        {
          command_id: COMMAND_ID,
          watched: true,
          channel: "mm",
          expected_version: 0,
        },
        signal,
      ),
    (userId: string) => ({ user_id: userId, auction_id: AUCTION_ID, watch: WATCH }),
  ],
  [
    "被超提醒偏好",
    (userId: string, signal: AbortSignal) =>
      setLotWatchPreference(
        userId,
        {
          command_id: COMMAND_ID,
          channel: "mm",
          expected_version: 0,
        },
        signal,
      ),
    watchPreference,
  ],
  [
    "登录时长偏好",
    (userId: string, signal: AbortSignal) =>
      setSessionPreference(
        userId,
        {
          command_id: COMMAND_ID,
          duration_seconds: null,
          expected_version: 0,
        },
        signal,
      ),
    sessionPreference,
  ],
] as const)("%s 的有效命令回执也确认归属", async (_name, write, payload) => {
  const mounted = fixture(ALICE);
  try {
    await settle();
    mounted.server.identity = BOB;
    mounted.server.read = () => reject("storage_unavailable", 503);
    mounted.server.mutate = () => success(payload(BOB.user_id));
    const receipt = await write(BOB.user_id, mounted.signal);
    expect(receipt.ok && receipt.value).toEqual(payload(BOB.user_id));
    expect(mounted.commandOwner.peek()).toBe(BOB.user_id);
    await settle();
    expect(mounted.session()).toEqual(err("storage_unavailable"));
    expect(mounted.owners).toEqual([null, ALICE.user_id, BOB.user_id]);
  } finally {
    mounted.dispose();
  }
});

test("登录已确认新账号后，旧标记响应晚到也不能恢复旧归属", async () => {
  const mounted = fixture(ALICE);
  const old = Promise.withResolvers<Response>();
  try {
    await settle();
    mounted.server.mutate = (_path, method) =>
      method === "POST" ? loginReceipt(BOB) : old.promise;
    const reading = fetchLotWatches(ALICE.user_id, AUCTION_ID, mounted.signal);
    await settle();
    expect(mounted.requests.at(-1)?.path).toBe(`/api/v1/me/lot-watches?auction_id=${AUCTION_ID}`);
    mounted.server.identity = BOB;
    mounted.server.read = () => reject("storage_unavailable", 503);
    expect(await loginPublic("one-use", mounted.signal)).toEqual(ok(BOB));
    await settle();
    expect(mounted.commandOwner()).toBe(BOB.user_id);

    old.resolve(success(watches(ALICE.user_id)));
    expect((await reading).ok).toBe(false);
    await settle();
    expect(mounted.session()).toEqual(err("storage_unavailable"));
    expect(mounted.owners).toEqual([null, ALICE.user_id, BOB.user_id]);
  } finally {
    old.resolve(success(watches(ALICE.user_id)));
    mounted.dispose();
  }
});

test("锁内读回同账号后，较早发出的锁外读取不能覆盖确认结果", async () => {
  const mounted = fixture(ALICE);
  const old = Promise.withResolvers<Response>();
  try {
    await settle();
    mounted.server.identity = BOB;
    mounted.server.mutate = () => old.promise;
    const reading = fetchSessionPreference(ALICE.user_id, mounted.signal);
    await settle();

    mounted.server.identity = ALICE;
    mounted.checkSession("refresh");
    await settle();
    expect(mounted.session()).toEqual(ok(ALICE));
    old.resolve(success(sessionPreference(BOB.user_id)));
    expect(await reading).toEqual(err({ code: "invalid_response", status: 200 }));
    await settle();
    expect(mounted.session()).toEqual(ok(ALICE));
    expect(mounted.owners).toEqual([null, ALICE.user_id]);
  } finally {
    old.resolve(success(sessionPreference(BOB.user_id)));
    mounted.dispose();
  }
});

test.each([
  ["401", () => reject("unauthenticated", 401), "unauthenticated"],
  [
    "异常成功状态",
    () => Response.json({ code: "ok", message: "", data: null }, { status: 201 }),
    "invalid_response",
  ],
  [
    "异常成功信封",
    () => Response.json({ code: "unauthenticated", message: "", data: null }),
    "invalid_response",
  ],
] as const)("%s 的会话读回不构成匿名证据", async (_name, read, code) => {
  const mounted = fixture(ALICE);
  try {
    await settle();
    save(watches(ALICE.user_id), publicWatchCacheRevision());
    mounted.server.read = read;
    mounted.checkSession("refresh");
    await settle();
    expect(mounted.session()).toEqual(err(code));
    expect(mounted.commandOwner()).toBe(ALICE.user_id);
    expect(load(ALICE.user_id, AUCTION_ID)).toEqual(watches(ALICE.user_id));
  } finally {
    mounted.dispose();
  }
});
