-- 预置规则：把当前线上发布的五种语言规则文档作为系统自带的初始内容写入数据库。
-- owner: auction-storage（public/rules 的读取与发布事务）；内容来源为 2026-11-13 线上公开接口
-- `/api/v1/rules` 返回的 en / zh-CN / zh-TW / ja / ko 各语言当前版本，标题与正文逐字节照录。
--
-- 不变量：
--   1. 只在该语言还没有规则文档时写入（ON CONFLICT DO NOTHING）；已有规则的库一行都不改，
--      版本、正文、发布者与发布时间保持原样。线上库五种语言都已存在，本迁移对它是空操作。
--   2. 预置文档从版本 1 开始，发布者记为 system；之后在 Admin“规则内容”页的编辑照常做
--      版本 CAS，从版本 1 接续。
--   3. 只预置规则；“关于”没有通用的默认内容，仍由运营者在 Admin 编写。
--   4. 实际写入了哪些语言记入迁移审计记录，可据此区分“预置写入”与“原已存在”。
--
-- 等级：compatible。只向既有表插入此前不存在的行，不改结构、约束与既有数据；106 版程序把它们
-- 当作普通的已发布规则读取，读写语义不变。
WITH preset (locale, title, markdown) AS (
    VALUES
        ('en', $preset_en_title$Auction rules$preset_en_title$, $preset_en$# Auction rules

HV-Auction runs a weekly equipment auction in each world (Main and Isekai). Sellers consign equipment by in-game MoogleMail; bidders compete on this site.

## Schedule

- Auction Preview: Every Friday at 08:00 UTC
- Auction Start: Every Saturday at 08:00 UTC
- Auction End: Every Sunday at 12:00 UTC

Each world runs its own independent auction.

Equipment mailed to the auction bot before the opening enters the current auction; anything arriving later is queued for the next one—nothing is returned for being early or late.

## Bidding

Bidding requires logging in with your personal bidder key. Send an in-game MoogleMail titled `register` to the auction bot and it will reply with your key.

The first accepted offer wins at exactly the starting price. After that, the normal next bid is the current price plus 5% (configurable), with the increase rounded up to a whole K and a natural minimum of 1K.

### 15-minute soft close

Any authoritative price increase inside the last 15 minutes moves that lot's end time to 15 minutes after the increase. Each lot extends independently. The proxy final phase still begins from the scheduled end time and never reverts.

## Proxy bidding

Each lot has at most one active proxy authorization. When proxy maxima compete, the higher maximum wins; equal maxima are won by the earlier accepted authorization. The public price is updated immediately to the next valid bid above the losing maximum, capped at the winner's maximum, and the losing authorization becomes exhausted.

- **Fast (default):** The authorization takes effect immediately and automatically answers every accepted ordinary bid at or below its maximum. Price and leader are resolved atomically.
- **Slow:** Its first activation is immediate. After an ordinary bid below its maximum takes the lead, one response is scheduled for 10 minutes later by default. Further ordinary bids do not postpone that fixed due time. At the due time it releases one response; in the final phase it converts to Fast and resolves any pending response immediately. New Slow authorizations are rejected during the final phase; choose Fast instead.

The public leader cannot place an ordinary bid or create another authorization. The owner of an active authorization cannot place an ordinary bid or create another authorization either. They may only lower that authorization strictly, never raise it; lowering does not consume a use and cannot go below the currently binding public price (or the next valid bid while a Slow response is pending).

Every accepted authorization creation consumes one of the default three configurable uses, including one that immediately loses to a stronger authorization. An exhausted owner may re-enter only with a maximum strictly above their previous maximum.

## Settlement: hold-time rewards and penalties

When a lot closes, the final price is adjusted by how long the winner continuously held the lead, as a share of the planned auction duration. Holding longer earns a discount; sniping late pays a tax.

| Lead held | Adjustment |
| --------: | ---------: |
|        0% |        +8% |
|        5% |      +6.4% |
|       10% |      +4.8% |
|       15% |      +3.2% |
|       20% |      +1.6% |
|       25% |         0% |
|       30% |      -0.4% |
|       40% |      -1.2% |
|       50% |        -2% |
|      100% |        -2% |

Negative values are discounts, positive values are taxes. Values between anchors are interpolated linearly.

### Compensation pool

All tax collected above the original price goes into one shared Persistent-world Credits compensation pool covering equipment from both worlds. It is distributed to discounted lots in proportion to their discount amount. A single lot's compensation never exceeds its listed discount, and any surplus carries into the next globally settled auction round.

For Main-world equipment, the winner receives the equipment with COD equal to the final adjusted price. For Isekai equipment, payment is collected first in Persistent using a 1C carrier with displayed COD equal to the final price plus 1C; the carrier makes the winner's net payment exactly the final price. After payment is confirmed, the equipment is sent in Isekai with no COD. Seller payout is always sent in Persistent only after the equipment delivery completes. Unsold lots are returned to their sellers free of charge.
$preset_en$),
        ('zh-CN', $preset_zh_cn_title$拍卖规则$preset_zh_cn_title$, $preset_zh_cn$# 拍卖规则

HV-Auction 在两个世界（主世界与异世界）各举办每周一届的装备拍卖。卖家通过游戏内 MoogleMail 寄售装备，买家在本站竞价。

## 场次日程

- 拍卖预览：每周五 08:00 UTC
- 正式开拍：每周六 08:00 UTC
- 计划截止：每周日 12:00 UTC

两个世界各自独立开拍；Credits 统一在主世界结算。

开拍前寄达拍卖机器人的装备进入本届拍卖；之后寄达的自动排入下一届，不会因寄早或寄晚被退回。

## 出价

出价需使用您的个人出价 Key 登录。在游戏内给拍卖机器人发送一封标题为 `register` 的 MoogleMail，系统会回信发放您的 Key。

第一笔被接受的出价以起拍价取得领先。此后正常下口价按当前价增加 5%（可配置），涨幅向上取整到整数 K，最低涨幅为 1K。

### 15 分钟软关闭

最后 15 分钟内每次真实的公开价格上涨，都会把该拍品结束时间顺延至上涨后 15 分钟。每件拍品独立顺延；代理出价的最终阶段仍按计划结束时间进入，进入后不会恢复 Slow。

## 代理出价

每件拍品最多只有一份生效中的代理授权。代理上限相互竞争时，较高上限获胜；上限相同则较早被接受的授权获胜。公开价会立即更新为高于落败上限的下一口有效价格，但不会超过胜者上限；落败授权随即进入 exhausted 终态。

- **快速（Fast，默认）：** 授权立即生效，并对所有不高于上限的有效普通出价立即响应；公开价和领先者在同一事务中确定。
- **缓慢（Slow）：** 首次生效仍会立即更新公开价。普通出价在上限以下取得领先后，默认固定在 10 分钟后响应；后续普通出价不会推迟该到期时间。到期时只响应一口；进入最终阶段时会切换为 Fast，并立即结清待响应价格。最终阶段会拒绝新建 Slow 授权，必须选择 Fast。

公开领先者不能普通出价，也不能创建代理授权。生效中授权的所有者同样不能普通出价或再建授权；只能严格降低现有上限，不能提高。降低不消耗次数，且不能低于当前绑定公开价（Slow 等待期间为下一口有效价）。

每次被接受的授权创建都会消耗默认三次（可配置）中的一次，包括创建后立即败给更高授权的情况。授权 exhausted 后，只有新上限严格高于本人上一次上限时才能重入。

## 结算：持有时长奖惩

拍品落锤后，最终价格会按胜者连续保持领先的时长占场次计划时长的比例进行调整。持有越久享折扣，最后时刻狙击则需缴税。

| 领先持有占比 | 价格调整 |
| -----------: | -------: |
|           0% |      +8% |
|           5% |    +6.4% |
|          10% |    +4.8% |
|          15% |    +3.2% |
|          20% |    +1.6% |
|          25% |       0% |
|          30% |    -0.4% |
|          40% |    -1.2% |
|          50% |      -2% |
|         100% |      -2% |

负值为折扣，正值为税费；锚点之间按线性插值计算。

### 补偿池

所有超出原价征收的税款进入一份由两个装备世界共用的主世界 Credits 补偿池，按各折扣单的折扣额等比例分配。单件补偿不超过其标定折扣额，盈余按全局结算顺序滚存至下一场。

主世界装备随邮件发送，COD 等于最终调整后价格。异世界装备先在主世界用 1C 载体收款，邮件显示的 COD 为最终价格加 1C；买家领取 1C 后的净付款仍恰好等于最终价格。确认付款后，系统再在异世界寄送无 COD 的装备。只有装备交付完成后，卖家款项才会在主世界寄出。流拍的拍品免费退回卖家。
$preset_zh_cn$),
        ('zh-TW', $preset_zh_tw_title$拍賣規則$preset_zh_tw_title$, $preset_zh_tw$# 拍賣規則

HV-Auction 在兩個世界（主世界與異世界）各舉辦每週一屆的裝備拍賣。賣家透過遊戲內 MoogleMail 寄售裝備，買家在本站競標。

## 場次日程

- 拍賣預覽：每週五 08:00 UTC
- 正式開拍：每週六 08:00 UTC
- 預定截止：每週日 12:00 UTC

兩個世界各自獨立開拍；Credits 統一在主世界結算。

開拍前寄達拍賣機器人的裝備進入本屆拍賣；之後寄達的自動排入下一屆，不會因寄早或寄晚而被退回。

## 出價

出價需使用您的個人出價 Key 登入。在遊戲內寄一封標題為 `register` 的 MoogleMail 給拍賣機器人，系統會回信發放您的 Key。

第一筆被接受的出價以起標價取得領先。此後正常下一口按目前價格增加 5%（可設定），漲幅向上取整到整數 K，最低漲幅為 1K。

### 15 分鐘軟關閉

最後 15 分鐘內每次真實的公開價格上漲，都會將該拍品結束時間順延至上漲後 15 分鐘。每件拍品獨立順延；代理出價的最終階段仍依預定結束時間進入，進入後不會恢復 Slow。

## 代理出價

每件拍品最多只有一份生效中的代理授權。代理上限互相競爭時，較高上限獲勝；上限相同則較早被接受的授權獲勝。公開價會立即更新為高於落敗上限的下一口有效價格，但不會超過勝者上限；落敗授權隨即進入 exhausted 終態。

- **快速（Fast，預設）：** 授權立即生效，並對所有不高於上限的有效普通出價立即回應；公開價和領先者在同一交易中確定。
- **緩慢（Slow）：** 首次生效仍會立即更新公開價。普通出價在上限以下取得領先後，預設固定於 10 分鐘後回應；後續普通出價不會延後該到期時間。到期時只回應一口；進入最終階段時會切換為 Fast，並立即結清待回應價格。最終階段會拒絕新建 Slow 授權，必須選擇 Fast。

公開領先者不能普通出價，也不能建立代理授權。生效中授權的所有者同樣不能普通出價或再建授權；只能嚴格降低現有上限，不能提高。降低不消耗次數，且不能低於目前綁定公開價（Slow 等待期間為下一口有效價）。

每次被接受的授權建立都會消耗預設三次（可設定）中的一次，包括建立後立即敗給更高授權的情況。授權 exhausted 後，只有新上限嚴格高於本人上一次上限時才能重新進入。

## 結算：持有時長獎懲

拍品結標後，最終價格會依勝者連續保持領先的時長佔場次預定時長的比例進行調整。持有越久享折扣，最後一刻狙擊則需繳稅。

| 領先持有佔比 | 價格調整 |
| -----------: | -------: |
|           0% |      +8% |
|           5% |    +6.4% |
|          10% |    +4.8% |
|          15% |    +3.2% |
|          20% |    +1.6% |
|          25% |       0% |
|          30% |    -0.4% |
|          40% |    -1.2% |
|          50% |      -2% |
|         100% |      -2% |

負值為折扣，正值為稅費；錨點之間以線性插值計算。

### 補償池

所有超出原價徵收的稅款進入一份由兩個裝備世界共用的主世界 Credits 補償池，依各折扣單的折扣額等比例分配。單件補償不超過其標定折扣額，盈餘依全域結算順序滾存至下一場。

主世界裝備隨郵件寄送，COD 等於最終調整後價格。異世界裝備先在主世界用 1C 載體收款，郵件顯示的 COD 為最終價格加 1C；買家領取 1C 後的淨付款仍恰好等於最終價格。確認付款後，系統再於異世界寄送無 COD 的裝備。只有裝備交付完成後，賣家款項才會在主世界寄出。流標拍品免費退回賣家。
$preset_zh_tw$),
        ('ja', $preset_ja_title$オークションルール$preset_ja_title$, $preset_ja$# オークションルール

HV-Auction は各ワールド（メインと異世界）で毎週装備オークションを開催しています。出品者はゲーム内 MoogleMail で装備を委託し、入札者は本サイトで競り合います。

## スケジュール

- オークションプレビュー：毎週金曜 08:00 UTC
- 入札開始：毎週土曜 08:00 UTC
- 終了予定：毎週日曜 12:00 UTC

各ワールドはそれぞれ独立したオークションを開催します。

開始前にオークションボットへ届いた装備は今回のオークションに出品されます。それ以降に届いたものは自動的に次回へ回され、早すぎても遅すぎても返送されることはありません。

## 入札

入札には個人の入札用 Key でのログインが必要です。ゲーム内でオークションボットに件名 `register` の MoogleMail を送ると、折り返し Key が届きます。

最初に受理されたオファーは開始価格ちょうどからリードします。それ以降の通常の次回額は現在価格の5%（設定可能）を加え、増分を整数Kへ切り上げます。最小増分は1Kです。

### 15 分ソフトクローズ

最後の15分以内に公開価格が実際に上昇すると、その出品の終了時刻は上昇から15分後まで延長されます。出品ごとに独立して延長されますが、代理入札の最終フェーズは予定終了時刻を基準に開始し、Slowへ戻ることはありません。

## 代理入札

各出品で有効な代理承認は最大1件です。代理上限同士が競合すると高い上限が勝ち、同額なら先に受理された承認が勝ちます。公開価格は敗者上限を上回る次の有効額へ即時更新されますが、勝者上限を超えません。敗れた承認は直ちに exhausted の終端状態になります。

- **高速（Fast、既定）：** 承認は即時に有効となり、上限以下で受理された通常入札へ直ちに応答します。公開価格とリーダーは同一トランザクションで確定します。
- **スロー（Slow）：** 初回の有効化では公開価格を即時更新します。上限未満の通常入札がリードすると、既定で10分後に1回の応答を固定予約します。その後の通常入札で期限は延期されません。期限には1口だけ応答し、最終フェーズではFastへ切り替えて保留中の応答を即時確定します。最終フェーズでは新しいSlow承認は拒否されるため、Fastを選ぶ必要があります。

公開リーダーは通常入札も新規代理承認も行えません。有効な承認の所有者も通常入札や追加承認を行えず、既存上限を厳密に下げることだけができます。引き上げはできません。引き下げは回数を消費せず、現在の拘束公開価格（Slow待機中は次の有効入札額）を下回れません。

受理された承認作成は、より強い承認に即時敗北する場合も含め、既定3回（設定可能）のうち1回を消費します。exhausted 後の再参加には、本人の直前上限を厳密に上回る新上限が必要です。

## 精算：リード保持時間による割引と課税

出品が終了すると、落札者が連続してリードを保持した時間の割合に応じて最終価格が調整されます。長く保持するほど割引され、終了間際の狙い撃ちには税が課されます。

| リード保持割合 |  調整 |
| -------------: | ----: |
|             0% |   +8% |
|             5% | +6.4% |
|            10% | +4.8% |
|            15% | +3.2% |
|            20% | +1.6% |
|            25% |    0% |
|            30% | -0.4% |
|            40% | -1.2% |
|            50% |   -2% |
|           100% |   -2% |

負の値は割引、正の値は課税です。アンカー間は線形補間されます。

### 補償プール

徴収された税は、両ワールドの装備で共有する Persistent Credits 補償プールに入り、割引額に比例して割引対象へ分配されます。補償は表示された割引額を超えず、余剰は全体の精算順で次のオークションへ繰り越されます。

Main 装備は調整後の最終価格と同額の COD 付きで届きます。Isekai 装備は、まず Persistent で 1C のキャリアを使って支払いを回収し、表示 COD は最終価格 + 1C になります。1C を受け取るため、落札者の実質支払額は最終価格と一致します。入金確認後、Isekai で COD なしの装備を発送します。出品者への支払いは装備の受領完了後にのみ Persistent で送られます。落札されなかった装備は無償で返送されます。
$preset_ja$),
        ('ko', $preset_ko_title$경매 규칙$preset_ko_title$, $preset_ko$# 경매 규칙

HV-Auction은 각 월드(메인과 이세계)에서 매주 장비 경매를 열어요. 판매자는 게임 내 MoogleMail로 장비를 위탁하고, 입찰자는 이 사이트에서 경쟁해요.

## 일정

- 경매 미리보기: 매주 금요일 08:00 UTC
- 입찰 시작: 매주 토요일 08:00 UTC
- 종료 예정: 매주 일요일 12:00 UTC

각 월드는 독립적으로 경매를 진행해요.

경매 시작 전에 봇에게 도착한 장비는 이번 경매에 출품되고, 이후 도착한 장비는 자동으로 다음 경매로 넘어가요.

## 입찰

개인 입찰 Key로 로그인해야 해요. 게임 내에서 경매 봇에게 제목이 `register`인 MoogleMail을 보내면 Key를 답장으로 받을 수 있어요.

처음 승인된 제안은 시작가에서 바로 선두가 돼요. 다음 일반 입찰가는 현재가의 5%(설정 가능)를 더하고 인상분을 정수 K로 올림하며 최소 인상분은 1K예요.

### 15분 소프트 마감

마지막 15분 안에 공개 가격이 오르면 해당 출품의 종료 시각이 상승으로부터 15분 뒤로 연장돼요. 출품마다 독립적으로 연장되지만 자동 입찰 최종 단계는 예정 종료 시각을 기준으로 시작해요.

## 자동 입찰

출품마다 활성 대리 승인은 최대 1개예요. 대리 상한끼리 경쟁하면 더 높은 상한이 이기고, 같으면 먼저 승인된 항목이 이겨요. 공개가는 패자 상한을 넘는 다음 유효 입찰가로 즉시 갱신되지만 승자 상한을 넘지 않아요. 패배한 승인은 즉시 exhausted 최종 상태가 돼요.

- **빠르게 (Fast, 기본):** 승인이 즉시 적용되고 상한 이하의 승인된 일반 입찰에 바로 응답해요. 공개가와 선두는 한 트랜잭션에서 확정돼요.
- **천천히 (Slow):** 최초 활성화는 공개가를 즉시 갱신해요. 상한 미만의 일반 입찰이 선두가 되면 기본 10분 뒤 응답 1회를 고정 예약해요. 이후 일반 입찰은 이 시간을 미루지 않아요. 시간이 되면 한 번만 응답하고, 최종 단계에서는 Fast로 전환해 대기 중 응답을 즉시 확정해요. 최종 단계에서는 새 Slow 승인이 거부되므로 Fast를 선택해야 해요.

공개 선두는 일반 입찰이나 새 대리 승인을 할 수 없어요. 활성 승인의 소유자도 일반 입찰이나 추가 승인을 할 수 없고, 기존 상한을 엄격히 낮추는 것만 가능해요. 올릴 수는 없어요. 낮추기는 횟수를 소모하지 않으며 현재 구속 공개가(Slow 대기 중에는 다음 유효 입찰가)보다 낮출 수 없어요.

승인된 대리 승인 생성은 더 강한 승인에 즉시 패배하는 경우도 포함해 기본 3회(설정 가능) 중 1회를 소모해요. exhausted 뒤 다시 참여하려면 본인의 직전 상한보다 엄격히 높은 새 상한이 필요해요.

## 정산: 선두 유지 시간에 따른 보상과 과세

낙찰자가 연속으로 선두를 유지한 비율에 따라 최종 가격이 조정돼요. 오래 유지할수록 할인을 받고 막판 저격은 세금을 내요.

| 선두 유지 |  조정 |
| --------: | ----: |
|        0% |   +8% |
|        5% | +6.4% |
|       10% | +4.8% |
|       15% | +3.2% |
|       20% | +1.6% |
|       25% |    0% |
|       30% | -0.4% |
|       40% | -1.2% |
|       50% |   -2% |
|      100% |   -2% |

음수는 할인, 양수는 세금이며 앵커 사이는 선형 보간돼요.

### 보상 풀

걷힌 세금은 두 월드 장비가 공유하는 Persistent Credits 보상 풀에 들어가고 할인 금액에 비례해 분배돼요. 보상은 표시된 할인액을 넘지 않으며 남는 금액은 전역 정산 순서의 다음 경매로 이월돼요.

Main 장비는 조정된 최종 가격과 같은 COD로 배송돼요. Isekai 장비는 먼저 Persistent에서 1C 운반체로 결제하며, 표시 COD는 최종 가격 + 1C예요. 구매자가 1C를 받으므로 순결제액은 정확히 최종 가격이에요. 결제 확인 후 Isekai에서 COD 없이 장비를 보내요. 판매자 대금은 장비 수령이 완료된 뒤에만 Persistent에서 지급돼요. 유찰 장비는 무료로 반송돼요.
$preset_ko$)
),
inserted AS (
    INSERT INTO public.rules_documents
        (kind, locale, revision, title, markdown, published_by, published_at)
    SELECT 'rules', locale, 1, title, markdown, 'system', transaction_timestamp()
    FROM preset
    ON CONFLICT (kind, locale) DO NOTHING
    RETURNING locale
)
INSERT INTO public.audit_records
    (id, actor_type, actor_id, action, entity_type, entity_id, result, source, metadata)
SELECT
    gen_random_uuid(), 'system', 'migration-000107', 'system.migration', 'migration',
    '000107', 'success', 'system',
    jsonb_build_object(
        'summary', 'preset rules documents for locales without one',
        'preset_locales', COALESCE((SELECT jsonb_agg(locale ORDER BY locale) FROM inserted), '[]'::jsonb),
        'compatibility', 'compatible');

INSERT INTO public.schema_migration_compatibility (version, level)
VALUES (107, 'compatible');
