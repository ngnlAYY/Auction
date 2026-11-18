// 同文档历史导航的片段定位与滚动位置恢复。

import { expect, test } from "bun:test";

import { writeQueryPairs } from "../net/query";
import { writeLocationPreservingScroll } from "./index";
import { installScrollDocument, openRouter } from "./test-window";

test.each([
  { url: "/me#login-duration", state: null, id: "login-duration" },
  { url: "/me#login-duration", state: [0, 200, "reloaded"], id: "login-duration" },
  { url: "/rules/?lang=en#%E6%97%A5%E7%A8%8B", state: null, id: "日程" },
])("冷启动 $url 在章节异步挂载后只定位一次", ({ url, state, id }) => {
  const opened = openRouter(url, state);
  const dom = installScrollDocument();
  try {
    opened.router.restoreScrollAfterCommit();
    opened.fake.commitFrame();
    expect(dom.seen).toEqual([]);
    expect(dom.observers()).toBe(1);

    dom.mount(id);
    dom.mount("other-heading");
    opened.fake.commitFrame();
    expect(dom.seen).toEqual([id]);
    expect(dom.observers()).toBe(0);
    dom.mount(id);
    opened.router.restoreScrollAfterCommit();
    opened.fake.commitFrame();
    expect(dom.seen).toEqual([id]);
  } finally {
    opened.disposeRouter();
    opened.restore();
    dom.restore();
  }
});

test("离开页面后，取消尚未挂载的旧片段定位", () => {
  const opened = openRouter("/me#login-duration");
  const dom = installScrollDocument();
  try {
    opened.router.restoreScrollAfterCommit();
    opened.fake.commitFrame();
    expect(dom.observers()).toBe(1);
    opened.router.push("/rules#schedule");
    expect(opened.router.currentLocationString()).toBe("/rules#schedule");
    expect(dom.observers()).toBe(0);
    dom.mount("login-duration");
    dom.mount("schedule");
    opened.router.restoreScrollAfterCommit();
    opened.fake.commitFrame();
    expect(dom.seen).toEqual(["schedule"]);
  } finally {
    opened.disposeRouter();
    opened.restore();
    dom.restore();
  }
});

test("同页片段导航仅在目标尚未挂载时等待并定位", () => {
  const opened = openRouter("/me");
  const { router, fake } = opened;
  const dom = installScrollDocument();
  try {
    dom.mount("login-duration");
    fake.followFragment("#login-duration");
    router.restoreScrollAfterCommit();
    fake.commitFrame();
    expect(dom.observers()).toBe(0);
    expect(dom.seen).toEqual([]);

    fake.followFragment("#account-bids-title");
    router.restoreScrollAfterCommit();
    fake.commitFrame();
    expect(dom.observers()).toBe(1);
    dom.mount("account-bids-title");
    fake.commitFrame();
    expect(dom.seen).toEqual(["account-bids-title"]);
    expect(dom.observers()).toBe(0);
    dom.mount("account-bids-title");
    fake.commitFrame();
    expect(dom.seen).toEqual(["account-bids-title"]);
  } finally {
    opened.disposeRouter();
    opened.restore();
    dom.restore();
  }
});

test("仅收到 hashchange 时，去往已挂载章节也会取消旧章节的等待", () => {
  const opened = openRouter("/me");
  const { fake } = opened;
  const dom = installScrollDocument();
  try {
    fake.followFragment("#account-bids-title", false);
    fake.commitFrame();
    expect(dom.observers()).toBe(1);
    dom.mount("login-duration");
    fake.followFragment("#login-duration", false);
    expect(dom.observers()).toBe(0);
    dom.mount("account-bids-title");
    fake.commitFrame();
    expect(dom.seen).toEqual([]);
  } finally {
    opened.disposeRouter();
    opened.restore();
    dom.restore();
  }
});

test("章节在 popstate 与 hashchange 之间挂载时仍定位一次", () => {
  const opened = openRouter("/me");
  const { fake } = opened;
  const dom = installScrollDocument();
  try {
    fake.followFragment("#account-bids-title", true, () => dom.mount("account-bids-title"));
    fake.commitFrame();
    expect(dom.seen).toEqual(["account-bids-title"]);
    expect(dom.observers()).toBe(0);
  } finally {
    opened.disposeRouter();
    opened.restore();
    dom.restore();
  }
});

test("未定位的同页章节后退再前进，挂载后仍完成定位", () => {
  const { router, fake, disposeRouter, restore } = openRouter("/me");
  const dom = installScrollDocument();
  try {
    fake.followFragment("#account-bids-title");
    fake.commitFrame();
    expect(dom.observers()).toBe(1);
    const pending = fake.history.state;
    expect(writeLocationPreservingScroll("/me?bid_page=2#account-bids-title", "replace")).toBe(
      true,
    );
    expect(router.currentLocationString()).toBe("/me?bid_page=2#account-bids-title");
    expect(fake.history.state).toEqual(pending);
    fake.history.back();
    router.restoreScrollAfterCommit();
    fake.commitFrame();
    expect(dom.observers()).toBe(0);

    fake.history.forward();
    router.restoreScrollAfterCommit();
    fake.commitFrame();
    expect(dom.seen).toEqual([]);
    dom.mount("account-bids-title");
    fake.commitFrame();
    expect(dom.seen).toEqual(["account-bids-title"]);
    expect(fake.history.state).toMatchObject({ 0: 0, 1: 0 });
  } finally {
    disposeRouter();
    restore();
    dom.restore();
  }
});

test("同页查询 push 保留未完成的片段，新条目等到标题挂载再定位", () => {
  const { router, fake, disposeRouter, restore } = openRouter("/me");
  const dom = installScrollDocument();
  try {
    fake.scrollByUser(0, 120);
    fake.followFragment("#account-bids-title");
    fake.commitFrame();
    expect(dom.observers()).toBe(1);
    const first = fake.history.state;

    writeQueryPairs(["bid_page"], [["bid_page", "2"]], "push");
    expect(fake.history.state).toMatchObject({ 0: "fragment", 1: "account-bids-title" });
    expect(fake.history.state).not.toEqual(first);
    expect(router.currentLocationString()).toBe("/me?bid_page=2#account-bids-title");
    fake.commitFrame();
    expect(dom.observers()).toBe(1);
    expect(dom.seen).toEqual([]);
    expect(fake.scrollY).toBe(120);

    dom.mount("account-bids-title");
    fake.commitFrame();
    expect(dom.seen).toEqual(["account-bids-title"]);
    expect(fake.history.state).toMatchObject({ 0: 0, 1: 120 });
    expect(dom.observers()).toBe(0);

    writeQueryPairs(["bid_page"], [["bid_page", "3"]], "push");
    expect(router.currentLocationString()).toBe("/me?bid_page=3#account-bids-title");
    expect(fake.history.state).toMatchObject({ 0: 0, 1: 120 });
    fake.commitFrame();
    expect(dom.seen).toEqual(["account-bids-title"]);
  } finally {
    disposeRouter();
    restore();
    dom.restore();
  }
});

test("原生片段在 popstate 后才滚动，离开时保存定位后的坐标", () => {
  const { router, fake, disposeRouter, restore } = openRouter("/me");
  const dom = installScrollDocument();
  const commit = () => {
    router.restoreScrollAfterCommit();
    fake.commitFrame();
  };
  try {
    dom.mount("account-bids-title");
    fake.scrollByUser(0, 120);
    fake.followFragment("#account-bids-title", true, () => {
      expect(fake.history.state).toMatchObject({ 0: "fragment", 1: "account-bids-title" });
      fake.scrollTo(0, 480);
    });
    expect(fake.history.state).toMatchObject({ 0: "fragment", 1: "account-bids-title" });
    expect(dom.seen).toEqual([]);

    fake.history.back();
    commit();
    expect(fake.scrollY).toBe(120);
    fake.history.forward();
    commit();
    expect(fake.scrollY).toBe(480);
    expect(fake.history.state).toMatchObject({ 0: 0, 1: 480 });
    expect(dom.seen).toEqual([]);
  } finally {
    disposeRouter();
    restore();
    dom.restore();
  }
});

test("已挂载的原生片段还没滚动就离开，前进时仍按片段定位", () => {
  const { router, fake, disposeRouter, restore } = openRouter("/me");
  const dom = installScrollDocument();
  try {
    dom.mount("account-bids-title");
    fake.followFragment("#account-bids-title");
    fake.history.back();
    fake.history.forward();
    expect(fake.history.state).toMatchObject({ 0: "fragment", 1: "account-bids-title" });
    router.restoreScrollAfterCommit();
    fake.commitFrame();
    expect(dom.seen).toEqual(["account-bids-title"]);
  } finally {
    disposeRouter();
    restore();
    dom.restore();
  }
});

test("后退和前进恢复各自的条目，未滚动的新页从顶部开始", () => {
  const opened = openRouter("/");
  const { router, fake } = opened;
  const commit = () => {
    router.restoreScrollAfterCommit();
    fake.commitFrame();
  };
  try {
    fake.scrollByUser(5, 480);
    router.push("/bidlog");
    expect(router.currentLocationString()).toBe("/bidlog");
    expect(fake.scrollY).toBe(0);

    fake.history.back();
    commit();
    expect([fake.scrollX, fake.scrollY]).toEqual([5, 480]);
    fake.history.forward();
    commit();
    expect([fake.scrollX, fake.scrollY]).toEqual([0, 0]);

    fake.scrollByUser(7, 240);
    fake.history.back();
    commit();
    expect([fake.scrollX, fake.scrollY]).toEqual([5, 480]);
    fake.history.forward();
    commit();
    expect([fake.scrollX, fake.scrollY]).toEqual([7, 240]);
  } finally {
    opened.disposeRouter();
    opened.restore();
  }
});

test("地址相同的两条记录也各自保存滚动位置", () => {
  const opened = openRouter("/");
  const { router, fake } = opened;
  const commit = () => {
    router.restoreScrollAfterCommit();
    fake.commitFrame();
  };
  try {
    const first = fake.history.state;
    fake.scrollByUser(0, 100);
    writeQueryPairs(["page"], [["page", "2"]], "push");
    expect(router.currentLocationString()).toBe("/?page=2");
    const second = fake.history.state;
    expect(fake.location.search).toBe("?page=2");
    expect((second as unknown[])[2]).not.toBe((first as unknown[])[2]);
    expect(fake.scrollY).toBe(100);
    fake.scrollByUser(0, 200);
    writeQueryPairs(["page"], [], "push");
    expect(router.currentLocationString()).toBe("/");
    expect(fake.location.search).toBe("");
    expect((fake.history.state as unknown[])[2]).not.toBe((first as unknown[])[2]);
    fake.scrollByUser(0, 300);
    fake.history.back();
    commit();
    expect(fake.scrollY).toBe(200);
    fake.history.back();
    commit();
    expect(fake.scrollY).toBe(100);
    fake.history.forward();
    commit();
    expect(fake.scrollY).toBe(200);
    fake.history.forward();
    commit();
    expect(fake.scrollY).toBe(300);
  } finally {
    opened.disposeRouter();
    opened.restore();
  }
});

test("路由 replace 保留条目标识并按新地址定位", () => {
  const { router, fake, disposeRouter, restore } = openRouter("/");
  const dom = installScrollDocument();
  try {
    const initialKey = (fake.history.state as unknown[])[2];
    dom.mount("schedule");
    router.replace("/rules#schedule");
    expect(router.currentLocationString()).toBe("/rules#schedule");
    expect(fake.history.state).toEqual(["fragment", "schedule", initialKey]);
    router.restoreScrollAfterCommit();
    fake.commitFrame();
    expect(dom.seen).toEqual(["schedule"]);
  } finally {
    disposeRouter();
    restore();
    dom.restore();
  }
});

test.each([false, true])("后退的保存位置等到页面足够长，滚动手势可取消（%s）", (cancel) => {
  const opened = openRouter("/");
  const { router, fake } = opened;
  const dom = installScrollDocument();
  try {
    fake.scrollByUser(0, 480);
    router.push("/bidlog");
    expect(router.currentLocationString()).toBe("/bidlog");
    dom.resize(90);
    fake.history.back();
    router.restoreScrollAfterCommit();
    fake.commitFrame();
    expect(fake.scrollY).toBe(0);
    expect(dom.observers()).toBe(1);
    if (cancel) {
      fake.dispatchEvent(new Event("wheel"));
      fake.scrollTo(0, 40);
    }
    dom.resize(800);
    fake.commitFrame();
    expect(fake.scrollY).toBe(cancel ? 40 : 480);
    expect(dom.observers()).toBe(0);
  } finally {
    opened.disposeRouter();
    opened.restore();
    dom.restore();
  }
});

test("等待恢复时筛选产生新条目，旧位置只留给原条目", () => {
  const opened = openRouter("/");
  const { router, fake } = opened;
  const dom = installScrollDocument();
  try {
    fake.scrollByUser(0, 480);
    router.push("/bidlog");
    expect(router.currentLocationString()).toBe("/bidlog");
    dom.resize(90);
    fake.history.back();
    router.restoreScrollAfterCommit();
    fake.commitFrame();
    expect(dom.observers()).toBe(1);
    expect(writeLocationPreservingScroll("/?page=2", "push")).toBe(true);
    expect(router.currentLocationString()).toBe("/?page=2");
    expect(dom.observers()).toBe(0);
    dom.resize(800);
    fake.commitFrame();
    expect(fake.scrollY).toBe(0);
    fake.history.back();
    router.restoreScrollAfterCommit();
    fake.commitFrame();
    expect(fake.scrollY).toBe(480);
  } finally {
    opened.disposeRouter();
    opened.restore();
    dom.restore();
  }
});

test("等待后退恢复时查询纠偏，保留目标条目的历史和内存位置", () => {
  const opened = openRouter("/");
  const { router, fake } = opened;
  const dom = installScrollDocument();
  const commit = () => {
    router.restoreScrollAfterCommit();
    fake.commitFrame();
  };
  try {
    fake.scrollByUser(0, 480);
    router.push("/bidlog");
    expect(router.currentLocationString()).toBe("/bidlog");
    fake.scrollByUser(0, 240);
    dom.resize(300);
    fake.history.back();
    commit();
    expect(fake.scrollY).toBe(240);
    const targetState = fake.history.state;
    expect(targetState).toMatchObject({ 0: 0, 1: 480 });

    expect(writeLocationPreservingScroll("/?world=main", "replace")).toBe(true);
    expect(router.currentLocationString()).toBe("/?world=main");
    expect(fake.history.state).toEqual(targetState);
    // 先离开再后退，避免由首次 pending 的副本掩盖内存快照被覆盖的问题。
    fake.history.forward();
    commit();
    expect(fake.scrollY).toBe(240);
    fake.history.back();
    dom.resize(800);
    commit();
    expect(fake.scrollY).toBe(480);
    expect(fake.location.search).toBe("?world=main");

    fake.scrollByUser(0, 500);
    expect(writeLocationPreservingScroll("/?world=isekai", "replace")).toBe(true);
    expect(router.currentLocationString()).toBe("/?world=isekai");
    // 只换地址不提交新落点，用户位置在真正离开条目时入账。
    expect(fake.history.state).toMatchObject({ 0: 0, 1: 480 });
    fake.history.forward();
    commit();
    fake.history.back();
    commit();
    expect(fake.scrollY).toBe(500);
  } finally {
    opened.disposeRouter();
    opened.restore();
    dom.restore();
  }
});

test("等待前进恢复时查询纠偏，将目标的新内存偏移写入历史", () => {
  const opened = openRouter("/");
  const { router, fake } = opened;
  const dom = installScrollDocument();
  const commit = () => {
    router.restoreScrollAfterCommit();
    fake.commitFrame();
  };
  try {
    router.push("/bidlog");
    expect(router.currentLocationString()).toBe("/bidlog");
    fake.scrollByUser(0, 480);
    fake.history.back(); // 浏览器后退未经过 Router.goBack，离开的条目只在内存记录 480。
    commit();
    fake.scrollByUser(0, 240);
    dom.resize(300);
    fake.history.forward();
    commit();
    expect(fake.history.state).toMatchObject({ 0: 0, 1: 0 });
    writeQueryPairs(["page"], [["page", "2"]], "replace");
    expect(router.currentLocationString()).toBe("/bidlog?page=2");
    expect(fake.location.search).toBe("?page=2");
    expect(fake.history.state).toMatchObject({ 0: 0, 1: 480 });
    dom.resize(800);
    fake.commitFrame();
    expect(fake.scrollY).toBe(480);
  } finally {
    opened.disposeRouter();
    opened.restore();
    dom.restore();
  }
});

test.each([
  { committed: false, type: "pointerdown" },
  { committed: true, type: "pointerdown" },
  { committed: true, type: "touchmove" },
  { committed: true, type: "keydown" },
])("等待恢复期间 $type 接管视口（已提交 $committed）", ({ committed, type }) => {
  const { router, fake, disposeRouter, restore } = openRouter("/");
  const dom = installScrollDocument();
  try {
    fake.scrollByUser(0, 480);
    router.push("/bidlog");
    expect(router.currentLocationString()).toBe("/bidlog");
    fake.scrollByUser(0, 240);
    dom.resize(90);
    fake.history.back();
    if (committed) {
      router.restoreScrollAfterCommit();
      fake.commitFrame();
      expect(dom.observers()).toBe(1);
    }
    fake.dispatchEvent(Object.assign(new Event(type), { key: "PageDown" }));
    fake.scrollTo(0, 40);
    expect(writeLocationPreservingScroll("/?world=main", "replace")).toBe(true);
    expect(router.currentLocationString()).toBe("/?world=main");
    expect(fake.history.state).toMatchObject({ 0: 0, 1: 480 });
    dom.resize(800);
    router.restoreScrollAfterCommit();
    fake.commitFrame();
    expect(fake.scrollY).toBe(40);
    expect(dom.observers()).toBe(0);
    fake.history.forward();
    fake.history.back();
    router.restoreScrollAfterCommit();
    fake.commitFrame();
    expect(fake.scrollY).toBe(40);
    expect(fake.history.state).toMatchObject({ 0: 0, 1: 40 });
  } finally {
    disposeRouter();
    restore();
    dom.restore();
  }
});

test("布局截短的 scroll 迟到时取消本次恢复，保留该条目的落点", () => {
  const { router, fake, disposeRouter, restore } = openRouter("/");
  const dom = installScrollDocument();
  try {
    fake.scrollByUser(0, 480);
    router.push("/bidlog");
    expect(router.currentLocationString()).toBe("/bidlog");
    fake.scrollByUser(0, 240);
    fake.history.back();
    dom.resize(90);
    router.restoreScrollAfterCommit();
    fake.commitFrame();
    expect(dom.observers()).toBe(1);
    fake.setViewport(0, 90);
    dom.resize(800);
    fake.dispatchEvent(new Event("scroll"));
    expect(writeLocationPreservingScroll("/?world=main", "replace")).toBe(true);
    expect(router.currentLocationString()).toBe("/?world=main");
    expect(fake.history.state).toMatchObject({ 0: 0, 1: 480 });
    fake.commitFrame();
    expect(fake.scrollY).toBe(90);
    expect(dom.observers()).toBe(0);
    fake.history.forward();
    fake.history.back();
    router.restoreScrollAfterCommit();
    fake.commitFrame();
    expect(fake.scrollY).toBe(480);
  } finally {
    disposeRouter();
    restore();
    dom.restore();
  }
});
