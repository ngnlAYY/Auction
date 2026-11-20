// 结算筛选的同址 push 一次写入完整目标地址，并保留等待中的片段落点。

import { expect, test } from "bun:test";
import {
  installFakeWindow,
  installScrollDocument,
  openRouter,
} from "@auction/web-shared/router/test-window";

import { pushSettlementQuery, readSettlementQuery, settlementQueryDefault } from "./query";

function fragmentEntryKey(state: unknown): string {
  if (!Array.isArray(state)) {
    throw new Error("历史条目必须是数组");
  }
  const [kind, fragment, key]: unknown[] = state;
  expect([kind, fragment]).toEqual(["fragment", "report"]);
  if (typeof key !== "string") {
    throw new Error("片段历史条目缺少标识");
  }
  return key;
}

test("带片段的同地址 push 创建条目并继续等待片段挂载", () => {
  const previousWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  let disposeRouter: (() => void) | null = null;
  let restoreDocument: (() => void) | null = null;
  try {
    const { router, fake, disposeRouter: dispose } = openRouter("/#report");
    disposeRouter = dispose;
    const dom = installScrollDocument();
    restoreDocument = dom.restore;
    const pushes = fake.writes.filter(({ mode }) => mode === "push");
    const firstKey = fragmentEntryKey(fake.history.state);

    pushSettlementQuery(settlementQueryDefault());

    expect(fake.location.href).toBe("https://auction.test/#report");
    expect(
      fake.writes
        .filter(({ mode }) => mode === "push")
        .slice(pushes.length)
        .map(({ url }) => url.href),
    ).toEqual(["https://auction.test/#report"]);
    expect(fragmentEntryKey(fake.history.state)).not.toBe(firstKey);

    router.restoreScrollAfterCommit();
    fake.commitFrame();
    expect(dom.observers()).toBe(1);
    expect(dom.seen).toEqual([]);
    dom.mount("report");
    fake.commitFrame();
    expect(dom.seen).toEqual(["report"]);
  } finally {
    try {
      disposeRouter?.();
    } finally {
      try {
        restoreDocument?.();
      } finally {
        if (previousWindow === undefined) {
          Reflect.deleteProperty(globalThis, "window");
        } else {
          Object.defineProperty(globalThis, "window", previousWindow);
        }
      }
    }
  }
});

const UUID = "12345678-1234-4abc-8000-123456789012";

function read(search: string) {
  const { restore } = installFakeWindow(`/settlement?${search}`);
  try {
    return readSettlementQuery();
  } finally {
    restore();
  }
}

test("sequence_only_links_require_an_absent_auction_parameter", () => {
  expect(read("sequence=7")).toMatchObject({ ok: true, value: { auctionId: "", sequence: 7 } });
  expect(read(`auction=${UUID}&sequence=7&world=main&page=2`)).toEqual({
    ok: true,
    value: { auctionId: UUID, sequence: 7, worldId: "main", page: 2 },
  });
  for (const invalid of [
    "auction=bad&sequence=7",
    "auction=&sequence=7",
    "auction=00000000-0000-0000-0000-000000000000&sequence=7",
    `auction=${UUID.toUpperCase()}&sequence=7`,
    "auction=12345678-1234-7abc-8000-123456789012&sequence=7",
    "auction=12345678-1234-4abc-0000-123456789012&sequence=7",
    `auction=${UUID}`,
  ]) {
    expect(read(invalid), invalid).toEqual({ ok: false, error: "invalid_settlement_link" });
  }
});

test("invalid_sequence_and_repeated_parameters_never_select_a_default", () => {
  expect(read("")).toEqual({
    ok: true,
    value: { auctionId: "", sequence: null, worldId: "all", page: 1 },
  });
  for (const invalid of [
    "sequence=",
    "sequence=0",
    "sequence=-1",
    "sequence=7.5",
    "sequence=%2B7",
    "sequence=9007199254740992",
    "sequence=seven",
    "sequence=7&sequence=7",
    "sequence=7&%73equence=8",
    "world=main&world=isekai&sequence=7",
    "world=bad&sequence=7",
    "page=1&page=2",
    `auction=${UUID}&auction=${UUID}&sequence=7`,
  ]) {
    expect(read(invalid), invalid).toEqual({ ok: false, error: "invalid_settlement_link" });
  }
});

test("duplicate_page_size_cannot_select_a_settlement", () => {
  expect(read("sequence=7&page_size=10")).toMatchObject({ ok: true, value: { sequence: 7 } });
  for (const search of ["sequence=7&page_size=10&page_size=20", "page_size=10&page%5Fsize=10"]) {
    expect(read(search), search).toEqual({ ok: false, error: "invalid_settlement_link" });
  }
});
