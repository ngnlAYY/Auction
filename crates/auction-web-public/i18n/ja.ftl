# 公開端の日本語テキスト。

app-title = オークション
language-label = 言語

nav-auctions = オークション
nav-my-bids = 自分の入札
nav-status = サービス状況
nav-label = メインナビゲーション

auction-title = オークション
auction-world-main = メインワールド
auction-status-live = 開催中
auction-ends-at = 終了時刻
auction-round-eyebrow = 現在のオークション回
auction-round-title = 第 24 回 装備オークション
auction-round-note = 入札結果はリアルタイムで更新されます。入札前に品番と現在価格をご確認ください。

stats-lots = 出品数
stats-bids = 現在の入札

filter-label = 出品カテゴリ
filter-all = すべて
filter-one-handed = 片手
filter-two-handed = 両手
filter-other = その他

category-one-handed = 片手
category-code-one = one

catalog-code = 品番
catalog-item = 装備
catalog-status = 状態
catalog-price = 現在価格
catalog-remaining = 残り時間
catalog-actions = 操作
catalog-view = 詳細
catalog-bid = 入札

status-live = 入札中
status-sold = 落札済み
status-ended = 終了

migration-note = ページのスタイルは Auction-BF の公開オークションページに準拠しています。データ接続後にサンプルデータを差し替えます。
data-loading = オークションデータを読み込んでいます……
data-error = オークションデータを一時的に利用できません。
data-empty = 現在開催中のオークションはありません。

not-found-title = ページが見つかりません
not-found-note = リンクが正しいかご確認ください。

page-auctions-title = オークション一覧
page-auctions-note = 開催回の一覧は、オークション照会ユースケースの接続後に表示されます。
page-lot-title = 出品 { $code }
page-lot-note = 出品詳細と入札は、入札コマンド契約の凍結後に接続されます。
page-bidlog-title = 入札履歴
page-bidlog-note = 入札履歴は公開入札ログプロジェクションから読み込みます。
page-history-title = 過去の開催回
page-history-note = 成約記録は公開価格履歴プロジェクションから読み込みます。
page-login-title = ログイン
page-login-note = ログインはセッション機能の移行後に接続されます。
page-me-title = マイアカウント
page-me-note = アカウント情報はユーザー機能の移行後に接続されます。
page-account-title = 自分の入札
page-account-note = ログインと入札履歴は、アカウント機能の移行後に接続されます。
page-rules-title = オークション規約
page-rules-note = 以下に現在公開されている規約文書を表示します。
page-blacklist-title = ブラックリスト
page-blacklist-note = 以下に現在の公開ブラックリストを表示します。
page-compensation-pool-title = 補償プール
page-compensation-pool-note = 公開済みの精算回と現在の補償プール残高を表示します。
page-compensation-title = 補償記録
page-compensation-note = 最新の公開済み補償台帳を表示します。
compensation-balance = プール残高：{ $balance }
compensation-updated = 更新時刻：{ $updated_at }
compensation-auction = オークション
compensation-opening = 開始残高
compensation-tax = 税収
compensation-out = 補償支出
compensation-closing = 終了残高
compensation-movements = 件数
compensation-settled = 精算時刻
compensation-round = { $world } 第 { $sequence } 回
compensation-sequence = 番号
compensation-kind = 種類
compensation-lot = 出品
compensation-amount = 金額
compensation-balance-after = 後の残高
compensation-created = 作成時刻
forum-title = フォーラムミラー
forum-status = 同期状態：{ $status } · ソース版 { $revision }
forum-updated = 更新時刻：{ $updated_at }
forum-link = フォーラムスレッドを開く
page-status-title = サービス状況
page-status-note = サービスの健全性と永続化されたリアルタイムイベントを表示します。
health-service = サービス
health-status = 状態
health-version = バージョン

bidlog-lot = ロット
bidlog-bidder = 入札者
bidlog-amount = 金額
bidlog-mechanism = 入札方式
bidlog-valid = 受付済み
bidlog-invalid = 無効
bidlog-time = 時刻

history-world = ワールド
history-round = 開催回
history-item = 装備
history-buyer = 買い手
history-seller = 売り手
history-price = 価格
history-closed = 成約時刻
history-statistics = 価格統計
history-count = 件数
history-min = 最低
history-max = 最高
history-median = 中央値

login-key-label = ログインキー
login-submit = ログイン
login-working = ログイン中……
login-success = { $username } としてログインしました。
login-error = ログインに失敗しました。キーを確認して再試行してください。
session-loading = 現在のセッションを確認しています……
session-error = セッションサービスを一時的に利用できません。
session-anonymous = ログインしていません。
session-user = ログイン中のユーザー：{ $username }。
session-expires = セッションの有効期限：{ $expires_at }。
logout = ログアウト
logout-success = ログアウトしました。
logout-error = ログアウトに失敗しました。もう一度お試しください。

account-login-required = 入札を表示するにはログインしてください。
account-total = 入札 { $bids } 件 · 代理承認 { $proxies } 件
account-no-bids = 入札履歴はありません。
account-no-proxies = 代理承認はありません。
account-bids-title = 入札履歴
account-proxies-title = 代理承認
account-lot = ロット
account-item = 装備
account-amount = 金額
account-mechanism = 入札方式
account-leading = 最高額
account-leading-yes = はい
account-leading-no = いいえ
account-time = 時刻
account-mode = モード
account-maximum = 上限
account-quota = クォータ
account-status = 状態
account-capability = 操作可能
account-capability-lower = 引き下げ可能
account-capability-locked = ロック中

bid-amount-label = 入札額
bid-submit = 入札する
bid-working = 送信中……
bid-success = 入札を受け付けました。現在価格 { $price }（バージョン { $version }）。
bid-error = 入札を受け付けられませんでした。出品状態と金額をご確認ください。

rules-revision = 言語：{ $locale }、リビジョン：{ $revision }。
rules-published = 公開時刻：{ $published_at }。
blacklist-total = 公開エントリ { $total } 件
blacklist-empty = 公開ブラックリストは空です。
blacklist-player = プレイヤー ID
blacklist-reason = 公開理由
blacklist-time = 登録時刻

realtime-loading = 永続化されたリアルタイムイベントを読み込んでいます……
realtime-connection-checking = ライブ更新用の公開セッションを確認しています……
realtime-connection-connecting = ライブ更新に接続しています……
realtime-connection-connected = ライブ更新に接続しました。
realtime-connection-anonymous = ライブ更新にはログインが必要です。キャッチアップは利用できます。
realtime-connection-stopped = ライブ更新を停止しました。権威スナップショットを再取得してください。
realtime-connection-error = ライブ更新を利用できません。更新してスナップショットを読み込んでください。
realtime-error = 永続化されたリアルタイムイベントを一時的に利用できません。
realtime-empty = 利用できる永続化リアルタイムイベントはありません。
realtime-refresh = イベントを更新
realtime-load-more = 次のイベントを読む
realtime-cursor = シーケンス { $sequence } まで読み込み済みです。
realtime-refetch-required = 適用を続ける前に、権威スナップショットを再取得する必要があります。
realtime-event-sequence = シーケンス
realtime-event-kind = イベント
realtime-event-schema = スキーマ
realtime-event-payload = 公開ペイロード
realtime-event-time = 時刻

proxy-title = 代理認証
proxy-loading = 代理認証を読み込んでいます……
proxy-load-error = 代理認証を利用できません。ログインして再試行してください。
proxy-create-maximum-label = 上限額
proxy-create-mode-label = モード
proxy-mode-fast = 高速
proxy-mode-slow = 低速
proxy-create-submit = 代理認証を作成
proxy-working = 送信中……
proxy-create-success = 代理認証を受け付けました。上限 { $maximum }（バージョン { $version }）。
proxy-command-error = 代理コマンドを受け付けられませんでした。出品状態と金額をご確認ください。
proxy-current = 現在の代理認証
proxy-status = 状態：{ $status }
proxy-maximum = 上限：{ $maximum }
proxy-lower-label = 新しい上限額
proxy-lower-submit = 上限を引き下げる
proxy-lower-success = 代理上限を { $maximum }（バージョン { $version }）まで引き下げました。
