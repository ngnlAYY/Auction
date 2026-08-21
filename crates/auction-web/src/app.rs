use leptos::prelude::*;
use leptos_meta::{Title, provide_meta_context};
use leptos_router::{StaticSegment, components::*};

#[cfg(feature = "ssr")]
use leptos_meta::Stylesheet;

#[cfg(feature = "ssr")]
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="zh-CN">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <AutoReload options=options.clone()/>
                <HydrationScripts options/>
                <Stylesheet id="auction-style" href="/pkg/auction-web.css"/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();

    view! {
        <Title text="Auction"/>
        <Router>
            <main class="auction-shell">
                <nav aria-label="主导航">
                    <a href="/">"拍卖"</a>
                    <a href="/account">"我的竞价"</a>
                    <a href="/status">"服务状态"</a>
                </nav>
                <Routes fallback=|| "页面不存在".into_view()>
                    <Route path=StaticSegment("") view=AuctionPage/>
                    <Route path=StaticSegment("auction") view=AuctionPage/>
                    <Route path=StaticSegment("account") view=AccountPage/>
                    <Route path=StaticSegment("status") view=StatusPage/>
                </Routes>
            </main>
        </Router>
    }
}

#[component]
fn AuctionPage() -> impl IntoView {
    view! {
        <section aria-labelledby="auction-title" class="public-auction-page">
            <header class="public-auction-round">
                <div class="public-auction-round__title">
                    <h1 id="auction-title">"拍卖"</h1>
                    <span class="public-auction-round__world">"主世界"</span>
                    <span class="public-auction-round__status public-auction-round__status--live">
                        "进行中"
                    </span>
                </div>
                <p class="public-auction-round__end">
                    "结束于"
                    <span class="public-auction-countdown">"02:14:36"</span>
                    <time datetime="2026-08-21T22:00:00Z">"2026-08-21 22:00 UTC"</time>
                </p>
            </header>

            <div class="public-auction-summary">
                <div>
                    <p class="public-auction-summary__eyebrow">"当前拍卖轮次"</p>
                    <h2>"第 24 轮装备拍卖"</h2>
                    <p>"竞价结果会实时更新，出价前请确认拍品编号和当前价格。"</p>
                </div>
                <dl class="public-auction-preview-stats">
                    <div>
                        <dt>"拍品"</dt>
                        <dd>"12"</dd>
                    </div>
                    <div>
                        <dt>"当前出价"</dt>
                        <dd>"8"</dd>
                    </div>
                </dl>
            </div>

            <nav aria-label="拍品分类" class="public-auction-filter">
                <button aria-pressed="true" class="public-auction-filter__tab" type="button">
                    "全部"
                    <span class="public-auction-filter__count">"12"</span>
                </button>
                <button aria-pressed="false" class="public-auction-filter__tab" type="button">
                    "单手"
                    <span class="public-auction-filter__count">"4"</span>
                </button>
                <button aria-pressed="false" class="public-auction-filter__tab" type="button">
                    "双手"
                    <span class="public-auction-filter__count">"3"</span>
                </button>
                <button aria-pressed="false" class="public-auction-filter__tab" type="button">
                    "其他"
                    <span class="public-auction-filter__count">"5"</span>
                </button>
            </nav>

            <section class="public-auction-category">
                <h2 class="public-auction-category__title">
                    "单手"
                    <span class="public-auction-category__code">"one"</span>
                </h2>
                <div class="public-auction-catalog">
                    <table>
                        <thead>
                            <tr>
                                <th>"编号"</th>
                                <th>"装备"</th>
                                <th>"状态"</th>
                                <th class="public-auction-catalog__number">"当前价"</th>
                                <th>"剩余时间"</th>
                                <th>"操作"</th>
                            </tr>
                        </thead>
                        <tbody>
                            <AuctionRow
                                code="one-001"
                                name="Legendary Rapier"
                                price="12,000"
                                status="live"
                                time="01:24:16"
                            />
                            <AuctionRow
                                code="one-002"
                                name="Fine Wakizashi"
                                price="8,500"
                                status="live"
                                time="02:04:42"
                            />
                            <AuctionRow
                                code="one-003"
                                name="Peerless Shortsword"
                                price="6,250"
                                status="sold"
                                time="已结束"
                            />
                        </tbody>
                    </table>
                </div>
            </section>

            <p class="public-page-status" role="status">
                "页面样式已按 Auction-BF 公共拍卖页迁移；数据接口接入后将替换示例数据。"
            </p>
        </section>
    }
}

#[component]
fn AuctionRow(
    code: &'static str,
    name: &'static str,
    price: &'static str,
    status: &'static str,
    time: &'static str,
) -> impl IntoView {
    let status_label = if status == "sold" { "已售出" } else { "竞价中" };
    view! {
        <tr class:public-auction-catalog__leading=status == "live">
            <td class="public-auction-catalog__code" data-label="编号">
                <a href="/lots/example">{code}</a>
            </td>
            <td class="public-auction-catalog__name" data-label="装备">{name}</td>
            <td data-label="状态">
                <span class=format!("public-auction-status public-auction-status--{status}")>
                    {status_label}
                </span>
            </td>
            <td class="public-auction-catalog__amount public-auction-catalog__number" data-label="当前价">
                {price}
            </td>
            <td class="public-auction-countdown" data-label="剩余时间">{time}</td>
            <td class="public-auction-catalog__actions" data-label="操作">
                <span class="public-auction-catalog__actions-list">
                    <a href="/lots/example">"查看"</a>
                    <button disabled=status == "sold" type="button">"出价"</button>
                </span>
            </td>
        </tr>
    }
}

#[component]
fn AccountPage() -> impl IntoView {
    view! {
        <section class="public-auction-empty" aria-labelledby="account-title">
            <h1 id="account-title">"我的竞价"</h1>
            <p>"登录和竞价历史将在账户能力迁移后接入。"</p>
        </section>
    }
}

#[component]
fn StatusPage() -> impl IntoView {
    view! {
        <section class="public-auction-empty" aria-labelledby="status-title">
            <h1 id="status-title">"服务状态"</h1>
            <p>"Public API online"</p>
        </section>
    }
}
