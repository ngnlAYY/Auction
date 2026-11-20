// 实时运维页的重试、书签恢复、筛选上限和错误展示。

import { expect, test } from "bun:test";
import { utf8ByteLength } from "@auction/web-shared/text";
import type { Reply } from "../../api/api-request-fixture";
import { loadRenderModule } from "../../solid-render-fixture.test";
import {
  byId,
  click,
  input,
  inputAndExpectValueUpdate,
  type MountedPage,
  okReply,
  type PageRequest,
  pendingRead,
  STORAGE_UNAVAILABLE,
  selectedOptions,
  submit,
  textParts,
  withPage,
} from "../browser-page-fixture.test";
import { type TestElement, valueUpdates } from "../solid-dom-fixture";

const EVENTS = "/api/v1/admin/realtime/events";
const FAILURES = "/api/v1/admin/realtime/failures";
const LAG = "/api/v1/admin/realtime/lag";
const DEFAULT_LAG_GET = ["GET", `${LAG}?page=1&page_size=10`] as const;

const TEXT_FILTERS = [
  {
    key: "event_kind",
    id: "realtime-event-kind",
    form: "realtime-event-filters",
    endpoint: EVENTS,
    parameter: "kind",
    overviewSize: "1",
  },
  {
    key: "failure_consumer",
    id: "realtime-failure-consumer",
    form: "realtime-failure-filters",
    endpoint: FAILURES,
    parameter: "consumer_key",
    overviewSize: "10",
  },
] as const;
type TextFilter = (typeof TEXT_FILTERS)[number];

const FILTER_CONTROLS = [
  ["event_stream", "realtime-event-stream"],
  ["event_kind", "realtime-event-kind"],
  ["failure_stream", "realtime-failure-stream"],
  ["failure_consumer", "realtime-failure-consumer"],
  ["failure_code", "realtime-failure-code"],
] as const;

function emptyPage({ method, url }: PageRequest): Reply {
  expect(method).toBe("GET");
  const page = Number(url.searchParams.get("page"));
  const page_size = Number(url.searchParams.get("page_size"));
  const pagination = { page, page_size, total: 0 };
  expect(url.pathname).toBe(LAG);
  return okReply({ lags: [], pagination });
}

async function withRealtime(
  query: string,
  check: (page: MountedPage) => Promise<void> | void,
  respond: (request: PageRequest) => Reply | Promise<Reply> = pendingRead,
): Promise<void> {
  const { outboxPage } = await loadRenderModule<Pick<typeof import("./page"), "outboxPage">>(
    new URL("./page.tsx", import.meta.url).pathname,
    "outboxPage",
  );
  await withPage(`/outbox?${query}`, outboxPage, respond, check);
}

function location(label: string, eventPage: number, failurePage: number): URLSearchParams {
  const pairs = new URLSearchParams();
  for (const [key] of FILTER_CONTROLS) {
    pairs.append(key, ` ${label}-${key} `);
  }
  pairs.append("event_page", String(eventPage));
  pairs.append("failure_page", String(failurePage));
  return pairs;
}

function assertListGet(gets: readonly PageRequest[], field: TextFilter, value: string): URL {
  const lists = gets.filter(
    ({ url }) => url.pathname === field.endpoint && url.searchParams.has(field.parameter),
  );
  expect(lists.map(({ method, url }) => [method, url.searchParams.get(field.parameter)])).toEqual([
    ["GET", value],
  ]);
  const [list] = lists;
  if (list === undefined) {
    throw new Error(`缺少 ${field.endpoint} 的筛选读取`);
  }
  return list.url;
}

function assertError(page: MountedPage, control: TestElement): TestElement {
  expect(control.getAttribute("aria-invalid")).toBe("true");
  expect(control.getAttribute("aria-describedby")).toBe(`${control.id}-error`);
  const error = byId(page.document, `${control.id}-error`);
  expect(textParts(error)).toContain("请求参数无效。请检查输入内容后重试。");
  return error;
}

async function withForm(
  field: TextFilter,
  check: (page: MountedPage, form: TestElement, control: TestElement) => Promise<void>,
): Promise<void> {
  await withRealtime("event_kind=initial&failure_consumer=initial", async (page) => {
    const form = byId(page.document, field.form);
    expect(typeof form.attributes.onSubmit).toBe("function");
    const control = byId(page.document, field.id);
    assertListGet(page.takeRequests(), field, "initial");
    await check(page, form, control);
  });
}

test("realtime_retries_failed_read_after_two_page_renders", async () => {
  let lagReads = 0;
  await withRealtime(
    "",
    async (page) => {
      const heading = byId(page.document, "outbox-title");
      const lag = page.document
        .querySelectorAll("section")
        .find((node) => node.getAttribute("aria-labelledby") === "realtime-lag-title");
      if (lag === undefined) {
        throw new Error("缺少消费延迟分区");
      }
      const failed = "所需服务暂时不可用。请稍后再试。";
      const empty = "当前筛选没有匹配的消费延迟。";
      expect(textParts(lag)).toContain(failed);
      expect(textParts(lag)).not.toContain(empty);
      await page.translateIn("en");
      expect(heading.textContent).toBe("Realtime events");
      await page.translateIn("zh-CN");
      expect(byId(page.document, "outbox-title")).toBe(heading);
      expect(heading.textContent).toBe("实时事件");
      const lagRequests = () =>
        page.requests
          .filter(({ url }) => url.pathname === LAG)
          .map(({ method, url }) => [method, url.pathname + url.search] as const);
      expect(lagRequests()).toEqual([DEFAULT_LAG_GET]);
      await click(lag, "重试");
      expect(lagRequests()).toEqual([DEFAULT_LAG_GET, DEFAULT_LAG_GET]);
      expect(textParts(lag)).toContain(empty);
      expect(textParts(lag)).not.toContain(failed);
    },
    (request) => {
      if (request.url.pathname === LAG) {
        lagReads += 1;
        if (lagReads === 1) {
          return STORAGE_UNAVAILABLE;
        }
        expect(lagReads).toBe(2);
        return emptyPage(request);
      }
      return pendingRead(request);
    },
  );
});

test("failure_status_bookmarks_select_the_option_used_by_the_first_list_get", async () => {
  for (const [state, resolved] of [
    ["dead_letter", "false"],
    ["completed", "true"],
  ] as const) {
    await withRealtime(`failure_state=${state}&failure_consumer=consumer`, (page) => {
      const gets = page.takeRequests().filter(({ url }) => url.pathname === FAILURES);
      expect(gets).toHaveLength(2);
      expect(
        gets.map(({ method, url }) => [
          method,
          url.searchParams.get("consumer_key"),
          url.searchParams.get("resolved"),
        ]),
      ).toEqual([
        ["GET", "consumer", resolved],
        ["GET", null, "false"],
      ]);
      expect(selectedOptions(page.document, "outbox-status")).toEqual([state]);
    });
  }
});

test("invalid_bookmarks_skip_both_lists_and_their_overviews_on_mount", async () => {
  await withRealtime("event_kind=a%0Ab&failure_consumer=&failure_consumer=x", (page) => {
    expect(
      page.takeRequests().map(({ method, url }) => [method, url.pathname + url.search] as const),
    ).toEqual([DEFAULT_LAG_GET]);
  });
});

test("realtime_forms_restore_history_and_preserve_drafts_on_pagination", async () => {
  await withRealtime("", async (page) => {
    const forms = page.document
      .querySelectorAll("form")
      .filter((form) => typeof form.attributes.onSubmit === "function");
    expect(forms).toHaveLength(2);
    const initial = FILTER_CONTROLS.map(([key, id]) => {
      const control = byId(page.document, id);
      return { key, id, control, writes: valueUpdates(control).length };
    });
    await page.restore(location("first", 1, 1));
    const controls = initial.map(({ key, id, control, writes }) => {
      expect(byId(page.document, id)).toBe(control);
      expect(control.value).toBe(`first-${key}`);
      expect(valueUpdates(control).slice(writes)).toContain(`first-${key}`);
      return control;
    });
    const eventStream = byId(page.document, "realtime-event-stream");
    const failureConsumer = byId(page.document, "realtime-failure-consumer");
    await inputAndExpectValueUpdate(eventStream, " event draft ");
    expect(eventStream.value).toBe(" event draft ");
    await inputAndExpectValueUpdate(failureConsumer, " consumer draft ");
    expect(failureConsumer.value).toBe(" consumer draft ");
    const beforePaging = controls.map(valueUpdates);
    for (const [eventPage, failurePage] of [
      [2, 1],
      [2, 3],
      [2, 3],
    ] as const) {
      await page.restore(location("first", eventPage, failurePage));
      expect(controls.map((control) => control.value)).toEqual([
        " event draft ",
        "first-event_kind",
        "first-failure_stream",
        " consumer draft ",
        "first-failure_code",
      ]);
      expect(FILTER_CONTROLS.map(([, id]) => byId(page.document, id))).toEqual(controls);
      expect(controls.map(valueUpdates)).toEqual(beforePaging);
    }
    page.takeRequests();
    for (const form of forms) {
      await submit(form);
    }
    const gets = page.takeRequests();
    const firstEventGet = gets.find(
      ({ method, url }) => method === "GET" && url.pathname === EVENTS,
    );
    if (firstEventGet === undefined) {
      throw new Error("提交后缺少事件 GET 请求");
    }
    expect(firstEventGet.url.searchParams.get("stream_key")).toBe("event draft");
    const event = assertListGet(gets, TEXT_FILTERS[0], "first-event_kind");
    expect(event.searchParams.get("stream_key")).toBe("event draft");
    assertListGet(gets, TEXT_FILTERS[1], "consumer draft");

    for (const label of ["second", "first", "second"]) {
      const snapshots = initial.map(({ key, id, control }) => ({
        key,
        id,
        control,
        writes: valueUpdates(control).length,
      }));
      await page.restore(location(label, 1, 1));
      for (const { key, id, control, writes } of snapshots) {
        expect(byId(page.document, id)).toBe(control);
        const expected = `${label}-${key}`;
        expect(control.value).toBe(expected);
        expect(valueUpdates(control).slice(writes)).toContain(expected);
      }
    }
  });
});

test("realtime_forms_round_trip_128_bytes_and_reject_129_on_submit", async () => {
  const valid = `${"汉".repeat(42)}ab`;
  const tooLong = "汉".repeat(43);
  expect([utf8ByteLength(valid), utf8ByteLength(tooLong)]).toEqual([128, 129]);
  for (const field of TEXT_FILTERS) {
    await withForm(field, async (page, form, control) => {
      const initial = page.address();
      await input(control, ` ${valid} `);
      await submit(form);
      const applied = page.address();
      expect(applied).toContainEqual([field.key, valid]);
      assertListGet(page.takeRequests(), field, valid);
      await page.back();
      expect(page.address()).toEqual(initial);
      page.takeRequests();
      const before = valueUpdates(control).length;
      await page.forward();
      expect(page.address()).toEqual(applied);
      expect(control.value).toBe(valid);
      expect(valueUpdates(control).slice(before)).toContain(valid);
      assertListGet(page.takeRequests(), field, valid);

      await input(control, tooLong);
      expect(byId(page.document, field.id)).toBe(control);
      expect(control.getAttribute("aria-invalid")).toBe("false");
      expect(control.getAttribute("aria-describedby")).toBeNull();
      expect(page.document.getElementById(`${field.id}-error`)).toBeNull();
      expect(page.takeRequests()).toEqual([]);

      await submit(form);
      expect(byId(page.document, field.id)).toBe(control);
      expect(page.address()).toEqual(applied);
      assertError(page, control);
      expect(page.takeRequests()).toEqual([]);
    });
  }
});

async function assertInvalidRestore(
  field: TextFilter,
  originals: readonly string[],
  escaped: readonly string[],
  controlCode?: string,
): Promise<void> {
  await withForm(field, async (page, form, control) => {
    const address = new URLSearchParams(page.address());
    address.delete(field.key);
    for (const original of originals) {
      address.append(field.key, original);
    }
    await page.restore(address);
    expect(page.takeRequests()).toEqual([]);
    const error = assertError(page, control);
    expect(error.querySelectorAll("code").map((node) => node.textContent)).toEqual([...escaped]);
    if (controlCode !== undefined) {
      expect(textParts(error).some((part) => part.includes(controlCode))).toBe(true);
    }
    const invalidAddress = [...address];
    expect(page.address()).toEqual(invalidAddress);
    await submit(form);
    expect(page.address()).toEqual(invalidAddress);
    expect(page.takeRequests()).toEqual([]);
    await input(control, "fixed");
    expect(page.address()).toEqual(invalidAddress);
    expect(page.takeRequests()).toEqual([]);
    await submit(form);
    expect(page.address().filter(([key]) => key === field.key)).toEqual([[field.key, "fixed"]]);
    const gets = page.takeRequests();
    expect(gets).toHaveLength(2);
    assertListGet(gets, field, "fixed");
    expect(
      gets
        .filter(
          ({ url }) => url.pathname === field.endpoint && !url.searchParams.has(field.parameter),
        )
        .map(({ method, url }) => [method, url.searchParams.get("page_size")]),
    ).toEqual([["GET", field.overviewSize]]);
  });
}

test("repeated_filters_preserve_empty_and_identical_values_until_corrected", async () => {
  const tooLong = "汉".repeat(43);
  for (const field of TEXT_FILTERS) {
    await assertInvalidRestore(field, ["", tooLong], ['""', `"${tooLong}"`]);
    await assertInvalidRestore(field, ["same", "same"], ['"same"', '"same"']);
  }
});

test("restored_control_characters_are_escaped_and_wait_for_a_correction", async () => {
  for (const field of TEXT_FILTERS) {
    await assertInvalidRestore(field, ["a\nb"], ['"a\\nb"'], "U+000A");
  }
});

test("restored_overlong_values_use_the_same_error_and_wait_for_a_correction", async () => {
  const tooLong = ` ${"汉".repeat(43)} `;
  for (const field of TEXT_FILTERS) {
    await assertInvalidRestore(field, [tooLong], [`"${tooLong}"`]);
  }
});

test("overlong_ascii_value_renders_in_parts_without_losing_text", async () => {
  const raw = "X".repeat(129);
  await withRealtime(`event_kind=${raw}`, (page) => {
    const parts = textParts(page.document.roots).filter(
      (part) => part.includes("X") && /^[X"]+$/u.test(part),
    );
    expect(parts.length).toBeGreaterThan(1);
    expect(parts.join("")).toBe(`"${raw}"`);
    expect(page.document.querySelectorAll("wbr")).toHaveLength(parts.length - 1);
    expect(page.takeRequests().some(({ url }) => url.pathname === EVENTS)).toBe(false);
  });
});

test("a_single_valid_filter_restores_and_queries_without_an_edit", async () => {
  for (const field of TEXT_FILTERS) {
    await withForm(field, async (page, _form, control) => {
      const address = new URLSearchParams(page.address());
      address.set(field.key, " valid ");
      const before = valueUpdates(control).length;
      await page.restore(address);
      expect(control.value).toBe("valid");
      expect(valueUpdates(control).slice(before)).toContain("valid");
      assertListGet(page.takeRequests(), field, "valid");
    });
  }
});
