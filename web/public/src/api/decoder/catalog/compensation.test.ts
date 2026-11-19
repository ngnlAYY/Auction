// 补偿时长的线上边界用后端共享样本回归。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { PublicCompensationMovementPageResponseSchema } from "@auction/contracts/dto/public/catalog/compensation";
import type { TransportResponse } from "@auction/web-shared/net/transport";
import { parseJson } from "@auction/web-shared/net/wire";
import { err } from "@auction/web-shared/result";
import { formatHeldDuration } from "@auction/web-shared/ui/format";
import { decodeSuccess, object } from "..";
import { decodeCompensationLots } from ".";

function fixtureText(name: string): string {
  return readFileSync(
    new URL(
      `../../../../../../crates/auction-contracts/tests/fixtures/public/${name}.json`,
      import.meta.url,
    ),
    "utf8",
  );
}

function responseWithText(text: string): TransportResponse {
  return {
    status: 200,
    statusText: "OK",
    headers: new Headers(),
    body: null,
    text: () => text,
    json(policy) {
      const decoded = parseJson(text, policy);
      return decoded.ok ? decoded : err({ kind: "decode" });
    },
  };
}

test("逐件补偿明细接受长时长，金额继续按安全整数解码", () => {
  const body = fixtureText("compensation-lots-long-hold");
  for (const [token, nanos, display] of [
    ["0", 0, "0s"],
    ["9072000000000000", 9_072_000_000_000_000, "105d 00h 00m"],
    ["9007199254740992", 2 ** 53, "104d 05h 59m"],
    ["9223372036854775807", 2 ** 63, "106751d 23h 47m"],
    ["9071999999999998", 9_071_999_999_999_998, "104d 23h 59m"],
    // JSON.parse 将整分钟前 1 纳秒的原值舍入到整分钟，Solid 据此显示下一分钟。
    ["9071999999999999", 9_072_000_000_000_000, "105d 00h 00m"],
  ] as const) {
    const page = decodeCompensationLots(responseWithText(body.replace("9072000000000000", token)));
    if (!page.ok) throw new Error(page.error);
    const lot = page.value.lots[0];
    if (!lot) throw new Error("样本缺少逐件结算明细");
    expect(lot.leader_held_nanos).toBe(nanos);
    expect(formatHeldDuration(lot.leader_held_nanos)).toBe(display);
  }
  for (const token of ["-1", "1.5", '"1"', "9223372036854777856"]) {
    expect(
      decodeCompensationLots(responseWithText(body.replace("9072000000000000", token))),
    ).toEqual(err("invalid_response"));
  }
  const unsafeAmount = body
    .replace("9072000000000000", "0")
    .replace(/"buyer_amount": \d+/u, '"buyer_amount": 9007199254740992');
  expect(decodeCompensationLots(responseWithText(unsafeAmount))).toEqual(err("invalid_response"));
});

test("补偿流水的契约 DTO 保留长时长与无拍品行", () => {
  const body = fixtureText("compensation-movements-long-hold");
  const decode = (text: string) =>
    decodeSuccess(
      responseWithText(text),
      PublicCompensationMovementPageResponseSchema,
      (value) => object(value) !== null,
    );
  const page = decode(body);
  if (!page.ok) throw new Error(page.error);
  expect(page.value.movements.map((row) => row.leader_held_nanos)).toEqual([
    null,
    9_072_000_000_000_000,
    9_072_000_000_000_000,
    null,
  ]);
  expect(decode(body.replace("9072000000000000", "-1"))).toEqual(err("invalid_response"));
});
