// 规则与关于页面跨路由保留在途发布，重读失败时仍显示成功回执。

import { expect, test } from "bun:test";
import { parseJson } from "@auction/web-shared/net/wire";
import type { Reply } from "../../api/api-request-fixture";
import { fixtureText, SESSION_REPLY, success } from "../../api/auction-contract-fixture.test";
import { jsonObject } from "../../api/json-fixture";
import type { AdminDocumentKind, AdminRules } from "../../api/rules";
import { loadRenderModule } from "../../solid-render-fixture.test";
import {
  click,
  okReply,
  type PageRequest,
  submit,
  textParts,
  withPage,
} from "../browser-page-fixture.test";
import { createComponent, required, settle } from "../solid-dom-fixture";

async function checkPublicationRead(kind: AdminDocumentKind, remount: boolean): Promise<void> {
  const { DocumentHarness } = await loadRenderModule<
    Pick<typeof import("./document-fixture"), "DocumentHarness">
  >(new URL("./document-fixture.tsx", import.meta.url).pathname, "DocumentHarness");
  const endpoint = `/api/v1/admin/${kind}`;
  const readPath = `${endpoint}?lang=en`;
  let document: AdminRules = {
    locale: "en",
    title: "原始标题",
    markdown: "原始正文",
    published_by: "operator",
    published_at: "2026-11-18T00:00:00Z",
    revision: 1,
  };
  const publication = Promise.withResolvers<Reply>();
  let readError: Reply | null = null;
  let reads = 0;
  const respond = ({ method, url, body }: PageRequest): Reply | Promise<Reply> => {
    if (url.pathname === "/api/v1/admin/session") {
      expect(`${method} ${url.pathname}${url.search}`).toBe(
        "GET /api/v1/admin/session?include=server_time",
      );
      return SESSION_REPLY;
    }
    expect(url.pathname).toBe(endpoint);
    if (method === "GET") {
      expect(url.pathname + url.search).toBe(readPath);
      reads += 1;
      const failure = readError;
      readError = null;
      return failure ?? okReply(document);
    }
    expect(`${method} ${url.pathname}${url.search}`).toBe(`PUT ${endpoint}`);
    const sent = jsonObject(success(parseJson(body)), "发布正文");
    expect(sent).toEqual({
      command_id: expect.stringMatching(
        /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u,
      ),
      expected_revision: 1,
      locale: "en",
      title: "新标题",
      markdown: "新正文",
    });
    return publication.promise;
  };

  await withPage(
    `/${kind}`,
    () => createComponent(DocumentHarness, { kind }),
    respond,
    async (mounted) => {
      const texts = () => textParts(mounted.document.roots);
      const requests = () =>
        mounted.requests.map(({ method, url }) => `${method} ${url.pathname}${url.search}`);
      const publish = "保存并发布";
      const initialRequests = [`GET ${readPath}`];
      const writeRequests = [
        ...initialRequests,
        "GET /api/v1/admin/session?include=server_time",
        `PUT ${endpoint}`,
      ];
      expect(requests()).toEqual(initialRequests);
      expect(texts()).toContain(publish);
      await click(mounted.document, "准备已校验草稿");
      await click(mounted.document, publish);
      expect(requests()).toEqual(writeRequests);
      if (remount) {
        await click(mounted.document, "切换测试路由");
        expect(texts()).not.toContain(publish);
        await click(mounted.document, "切换测试路由");
        expect(reads).toBe(2);
        expect(requests()).toEqual([...writeRequests, `GET ${readPath}`]);
      } else {
        readError = {
          status: 503,
          body: '{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}',
        };
      }
      expect(texts()).toContain(publish);

      document = { ...document, revision: 2, title: "新标题", markdown: "新正文" };
      publication.resolve(okReply(document));
      await settle();
      if (remount) {
        expect(reads).toBe(3);
      } else {
        expect(texts()).not.toContain(publish);
        expect(texts()).toContain("所需服务暂时不可用。请稍后再试。");
        expect(texts()).toContain("文档已保存。");
        await click(mounted.document, "重试");
        expect(texts()).toContain("文档已保存。");
      }
      expect(requests()).toEqual([...writeRequests, `GET ${readPath}`, `GET ${readPath}`]);
      expect(texts()).toContain(publish);
      expect(texts()).not.toContain("正在加载文档……");
    },
  );
}

test("rules_reload_after_returning_before_publication_settles", () =>
  checkPublicationRead("rules", true));

test("about_reloads_after_returning_before_publication_settles", () =>
  checkPublicationRead("about", true));

test("publication_receipts_survive_a_failed_read_for_both_document_kinds", async () => {
  for (const kind of ["rules", "about"] as const) {
    await checkPublicationRead(kind, false);
  }
});

test("only_a_verified_publication_conflict_settles_rules_and_about", async () => {
  const { DocumentHarness } = await loadRenderModule<
    Pick<typeof import("./document-fixture"), "DocumentHarness">
  >(new URL("./document-fixture.tsx", import.meta.url).pathname, "DocumentHarness");
  const conflict = fixtureText("error-document-state-conflict");
  for (const kind of ["rules", "about"] as const) {
    const endpoint = `/api/v1/admin/${kind}`;
    const writes: string[] = [];
    const previews: string[] = [];
    await withPage(
      `/${kind}`,
      () => createComponent(DocumentHarness, { kind }),
      ({ method, url, body }) => {
        if (url.pathname === "/api/v1/admin/session") return SESSION_REPLY;
        expect(url.pathname).toBe(endpoint);
        if (method === "GET") {
          expect(url.search).toBe("?lang=en");
          return okReply({
            locale: "en",
            title: "原始标题",
            markdown: "原始正文",
            published_by: "operator",
            published_at: "2026-11-18T00:00:00Z",
            revision: 1,
          });
        }
        if (method === "POST") {
          previews.push(body);
          return { status: 409, body: conflict };
        }
        expect(method).toBe("PUT");
        writes.push(body);
        switch (writes.length) {
          case 1:
            return {
              status: 503,
              body: '{"code":"storage_unavailable","message":"暂不可用","data":null,"request_id":null}',
            };
          case 2:
            return { status: 429, body: conflict };
          case 3:
            return { status: 409, body: conflict };
          default:
            throw new Error("额外的文档发布");
        }
      },
      async ({ document }) => {
        const texts = () => textParts(document.roots);
        await click(document, "准备已校验草稿");
        await submit(required(document.querySelector("form"), "文档校验表单"));
        expect(previews.map((body) => success(parseJson(body)))).toEqual([
          { locale: "en", title: "新标题", markdown: "新正文" },
        ]);
        expect(writes).toEqual([]);
        expect(texts()).not.toContain("原样重试");

        await click(document, "准备已校验草稿");
        await click(document, "保存并发布");
        expect(texts()).toContain("原样重试");
        await click(document, "原样重试");
        expect(texts()).toContain("原样重试");
        await click(document, "原样重试");
        expect(texts()).not.toContain("原样重试");
        expect(writes).toHaveLength(3);
        expect(new Set(writes).size).toBe(1);
        expect(JSON.parse(writes[0] ?? "")).toEqual({
          command_id: expect.stringMatching(/^[\da-f-]{36}$/u),
          expected_revision: 1,
          locale: "en",
          title: "新标题",
          markdown: "新正文",
        });
      },
    );
  }
});
