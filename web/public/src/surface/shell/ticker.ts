//! 外壳 ticker 的标题、状态、世界与倒计时投影；只把当前展示场次翻译成文案。

import type { CurrentAuctionItemResponse } from "@auction/contracts/dto/public/catalog/auction";
import { createValueMemo } from "@auction/web-shared/reactive/memo";
import { t } from "@auction/web-shared/ui/i18n";

import { tickerCountdown } from "../board/realtime";
import { worldLabel } from "../board/world";
import { auctionStatusLabel } from "../catalog/presentation";

export interface PublicTickerCountdownDisplay {
  readonly label: string;
  readonly targetAt: string;
  readonly expiredLabel: string;
}

export interface PublicTickerLabels {
  title(): string;
  status(): string;
  world(): string;
  countdown(): PublicTickerCountdownDisplay | null;
}

/** 场次缺失时沿用外壳的默认标题；倒计时只由竞价期与预览期产生。 */
export function createPublicTickerLabels(
  auction: () => CurrentAuctionItemResponse | null,
  fallbackWorld: () => string,
): PublicTickerLabels {
  const title = (): string => {
    const current = auction();
    return current === null
      ? t("nav-auctions")
      : t("ticker-auction-title", { number: current.number });
  };
  const status = (): string => {
    const current = auction();
    return current === null ? "" : auctionStatusLabel(current.status);
  };
  const world = (): string => {
    const current = auction();
    return worldLabel(current === null ? fallbackWorld() : current.world);
  };
  const countdown = createValueMemo((): PublicTickerCountdownDisplay | null => {
    const current = auction();
    if (current === null) {
      return null;
    }
    const value = tickerCountdown(current);
    if (value === null) {
      return null;
    }
    if (value.kind === "closing") {
      return {
        label: t("ticker-remaining"),
        targetAt: value.endsAt,
        expiredLabel: t("auction-countdown-expired"),
      };
    }
    return {
      label: t("ticker-opens-in"),
      targetAt: value.opensAt,
      expiredLabel: t("ticker-opening"),
    };
  });
  return { title, status, world, countdown };
}
