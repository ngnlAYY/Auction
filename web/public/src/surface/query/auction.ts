// 场次选择的唯一查询状态，保留非法或重复的原值。

import { validUuid } from "@auction/web-shared/ui/validation";

/** 缺省、合法 UUID 和无法唯一解析的场次。 */
export type AuctionParameter =
  | { readonly kind: "unspecified" }
  | { readonly kind: "selected"; readonly id: string }
  | { readonly kind: "invalid"; readonly values: readonly string[] };

/** 多值地址没有唯一目标，保留每个原值让读取入口失败关闭。 */
export function parseAuctionParameter(values: readonly string[]): AuctionParameter {
  const [id] = values;
  if (id === undefined) return { kind: "unspecified" };
  return values.length === 1 && validUuid(id)
    ? { kind: "selected", id }
    : { kind: "invalid", values };
}

/** 从单值目标构造场次参数，空目标显式清除选择。 */
export function auctionParameter(id: string | null): AuctionParameter {
  return parseAuctionParameter(id === null ? [] : [id]);
}
