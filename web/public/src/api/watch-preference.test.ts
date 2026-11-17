// 账号竞价警告偏好的闭集互换与权威响应键集合回归断言。

import { expect, test } from "bun:test";

import { jsonHasExactKeys } from "@auction/web-shared/net/wire";

const { RESPONSE_KEYS, channelFlags, channelValue } = await import("./watch-preference");

test("channel_flags_and_value_round_trip_the_closed_set：渠道闭集往返一致", () => {
  for (const value of ["none", "mm", "pm", "both"] as const) {
    const flags = channelFlags(value);
    expect(flags).not.toBeNull();
    if (flags === null) {
      throw new Error(`闭集值 ${value} 必须可解析`);
    }
    const [mm, pm] = flags;
    expect(channelValue(mm, pm)).toBe(value);
  }
  expect(channelFlags("")).toBeNull();
  expect(channelFlags("MM")).toBeNull();
  expect(channelFlags("mm,pm")).toBeNull();
});

test("preference_response_requires_pm_available：偏好响应必须带 pm_available", () => {
  const full = {
    user_id: "44f6d78b-ad02-447e-897f-f89731f98566",
    channel: "mm",
    version: 1,
    pm_available: false,
  };
  expect(jsonHasExactKeys(full, RESPONSE_KEYS)).toBe(true);
  const missing = { user_id: "u", channel: "mm", version: 1 };
  expect(jsonHasExactKeys(missing, RESPONSE_KEYS)).toBe(false);
});
