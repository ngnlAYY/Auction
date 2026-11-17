// 原样重试被拒时的文案不再把已提交金额当作最低价。

import { expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { createRoot } from "solid-js";

import type { CommandError } from "../../api/commands";

const { buildBundle, createLocaleSession, DEFAULT_PUBLIC_LOCALE } = await import(
  "@auction/web-shared/ui/i18n"
);
const { formatCreditAmount } = await import("@auction/web-shared/ui/format");
const { bidErrorMessage, proxyErrorMessage } = await import("./pending");
const { retryErrorMessage } = await import("./retry");

/** 用户在结果未知前提交的金额。 */
const SUBMITTED = 50_000;

const ENGLISH = readFileSync(
  new URL("../../../../../crates/auction-web-public/i18n/en.ftl", import.meta.url),
  "utf8",
);

function error(code: string): CommandError {
  return { code, status: 409 };
}

test("price_change_on_retry_does_not_quote_the_submitted_amount_as_minimum：重试价格变化时不把已提交金额当最低价", () => {
  createRoot((dispose) => {
    const bundle = buildBundle(DEFAULT_PUBLIC_LOCALE, ENGLISH, DEFAULT_PUBLIC_LOCALE, null);
    if (!bundle.ok) {
      throw new Error("译文未能初始化");
    }
    createLocaleSession(bundle.value, DEFAULT_PUBLIC_LOCALE);
    const tooLow = error("bid_too_low");
    const leading = error("already_leading");
    const messages = [
      retryErrorMessage(tooLow, (value) => bidErrorMessage(value, SUBMITTED)),
      retryErrorMessage(tooLow, (value) => proxyErrorMessage(value, SUBMITTED)),
      retryErrorMessage(leading, (value) => bidErrorMessage(value, SUBMITTED)),
      bidErrorMessage(leading, SUBMITTED),
    ];
    expect(messages).toHaveLength(4);
    const submitted = formatCreditAmount(SUBMITTED);
    for (const message of messages.slice(0, 2)) {
      expect(message).not.toContain(submitted);
      expect(message).toContain("price has changed");
    }
    // 其他错误码与新命令的映射一致。
    expect(messages[2]).toBe(messages[3]);
    dispose();
  });
});
