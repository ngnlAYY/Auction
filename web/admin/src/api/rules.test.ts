// 文档读取根据各自接口与语言解析 Rust 的规则、旧语言规则及关于样本。

import { expect, test } from "bun:test";

import { fixtureText, requestFixture, success } from "./auction-contract-fixture.test";
import { fetchAdminRules } from "./rules";

test("site_documents_decode_backend_contract_fixtures", async () => {
  for (const [sample, kind, locale] of [
    ["rules", "rules", "en"],
    ["rules-legacy-locale", "rules", "fr"],
    ["about", "about", "zh-CN"],
  ] as const) {
    const document = success(
      await requestFixture(
        [{ body: fixtureText(sample) }],
        [["GET", `/api/v1/admin/${kind}?lang=${locale}`]],
        (host, signal) => fetchAdminRules(host, kind, locale, signal),
      ),
    );
    expect(document.locale).toBe(locale);
    expect(document.revision).toBe(6);
  }
});
