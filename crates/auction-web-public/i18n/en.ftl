# 公开端英文文案（默认语言）。
# Fluent 的消息 ID 不允许包含点号，因此旧系统的 `nav.home` 一类键统一改为 `nav-home`。

app-title = Auction
language-label = Language

nav-auctions = Auctions
nav-my-bids = My bids
nav-status = Service status
nav-label = Main navigation

auction-title = Auction
auction-world-main = Main world
auction-status-live = Live
auction-ends-at = Ends at
auction-round-eyebrow = Current auction round
auction-round-title = Round 24 equipment auction
auction-round-note = Bid results update in real time. Confirm the lot code and current price before bidding.

stats-lots = Lots
stats-bids = Bids

filter-label = Lot categories
filter-all = All
filter-one-handed = One-handed
filter-two-handed = Two-handed
filter-other = Other

category-one-handed = One-handed
category-code-one = one

catalog-code = Code
catalog-item = Equipment
catalog-status = Status
catalog-price = Current price
catalog-remaining = Remaining
catalog-actions = Actions
catalog-view = View
catalog-bid = Bid

status-live = Bidding
status-sold = Sold
status-ended = Ended

migration-note = Page styling follows the Auction-BF public auction page. Sample data will be replaced once the data interfaces are connected.
data-loading = Loading auction data…
data-error = Auction data is temporarily unavailable.
data-empty = There is no current auction.

not-found-title = Page not found
not-found-note = Please check that the link is correct.

page-auctions-title = Auction list
page-auctions-note = The round list will appear once the auction query use case is connected.
page-lot-title = Lot { $code }
page-lot-note = Lot details and bidding will be connected once the bid command contract is frozen.
page-bidlog-title = Bid log
page-bidlog-note = The bid ledger is read from the public bid-log projection.
page-history-title = Past rounds
page-history-note = Completed sales are read from the public price-history projection.
page-login-title = Log in
page-login-note = Login will be connected once the session capability is migrated.
page-me-title = My account
page-me-note = Account details will be connected once the user capability is migrated.
page-account-title = My bids
page-account-note = Login and bid history will be connected once the account capability is migrated.
page-rules-title = Auction rules
page-rules-note = The current published rules document is shown below.
page-blacklist-title = Blacklist
page-blacklist-note = The current public blacklist is shown below.
page-compensation-pool-title = Compensation pool
page-compensation-pool-note = Published settlement rounds and the current pool balance.
page-compensation-title = Compensation records
page-compensation-note = The latest published compensation ledger.
compensation-balance = Pool balance: { $balance }
compensation-updated = Updated at { $updated_at }
compensation-auction = Auction
compensation-opening = Opening
compensation-tax = Tax income
compensation-out = Compensation out
compensation-closing = Closing
compensation-movements = Movements
compensation-settled = Settled at
compensation-round = { $world } round { $sequence }
compensation-sequence = Sequence
compensation-kind = Kind
compensation-lot = Lot
compensation-amount = Amount
compensation-balance-after = Balance after
compensation-created = Created at
forum-title = Forum mirror
forum-status = Sync status: { $status } · source revision { $revision }
forum-updated = Updated at { $updated_at }
forum-link = Open forum thread
page-status-title = Service status
page-status-note = Service health and persisted realtime events are shown below.
health-service = Service
health-status = Status
health-version = Version

bidlog-lot = Lot
bidlog-bidder = Bidder
bidlog-amount = Amount
bidlog-mechanism = Mechanism
bidlog-valid = Accepted
bidlog-invalid = Invalid
bidlog-time = Time

history-world = World
history-round = Round
history-item = Equipment
history-buyer = Buyer
history-seller = Seller
history-price = Price
history-closed = Closed at
history-statistics = Price statistics
history-count = Count
history-min = Minimum
history-max = Maximum
history-median = Median

login-key-label = Login key
login-submit = Log in
login-working = Signing in…
login-success = Signed in as { $username }.
login-error = Login failed. Check the key and try again.
session-loading = Checking the current session…
session-error = The session service is temporarily unavailable.
session-anonymous = You are not signed in.
session-user = Signed in as { $username }.
session-expires = Session expires at { $expires_at }.
logout = Log out
logout-success = You have been signed out.
logout-error = Sign-out failed. Please try again.

account-login-required = Sign in to view your bids.
account-total = { $bids } bids · { $proxies } proxy authorizations
account-no-bids = No bids yet.
account-no-proxies = No proxy authorizations yet.
account-bids-title = Bid history
account-proxies-title = Proxy authorizations
account-lot = Lot
account-item = Equipment
account-amount = Amount
account-mechanism = Mechanism
account-leading = Leading
account-leading-yes = Yes
account-leading-no = No
account-time = Time
account-mode = Mode
account-maximum = Maximum
account-quota = Quota
account-status = Status
account-capability = Capability
account-capability-lower = Can lower
account-capability-locked = Locked

bid-amount-label = Bid amount
bid-submit = Place bid
bid-working = Submitting…
bid-success = Bid accepted at { $price } (version { $version }).
bid-error = Bid was not accepted. Check the lot status and amount.

rules-revision = Language { $locale }, revision { $revision }.
rules-published = Published at { $published_at }.
blacklist-total = { $total } public entries
blacklist-empty = The public blacklist is empty.
blacklist-player = Player ID
blacklist-reason = Public reason
blacklist-time = Blacklisted at

realtime-loading = Loading persisted realtime events…
realtime-connection-checking = Checking the public session for live updates…
realtime-connection-connecting = Connecting to live updates…
realtime-connection-connected = Live updates connected.
realtime-connection-anonymous = Sign in to receive live updates; catch-up remains available.
realtime-connection-stopped = Live updates stopped; reload the authoritative snapshot.
realtime-connection-error = Live updates are unavailable; use refresh to read the snapshot.
realtime-error = Persisted realtime events are temporarily unavailable.
realtime-empty = No persisted realtime events are available.
realtime-refresh = Refresh events
realtime-load-more = Load next events
realtime-cursor = Cursor after sequence { $sequence }.
realtime-refetch-required = The stream requires an authoritative snapshot refetch before it can be applied.
realtime-event-sequence = Sequence
realtime-event-kind = Event
realtime-event-schema = Schema
realtime-event-payload = Public payload
realtime-event-time = Time

proxy-title = Proxy authorization
proxy-loading = Loading proxy authorization…
proxy-load-error = Proxy authorization is unavailable. Sign in and try again.
proxy-create-maximum-label = Maximum amount
proxy-create-mode-label = Mode
proxy-mode-fast = Fast
proxy-mode-slow = Slow
proxy-create-submit = Create proxy authorization
proxy-working = Submitting…
proxy-create-success = Proxy authorization accepted at { $maximum } (version { $version }).
proxy-command-error = Proxy command was not accepted. Check the lot status and amount.
proxy-current = Current proxy authorization
proxy-status = Status: { $status }
proxy-maximum = Maximum: { $maximum }
proxy-lower-label = New maximum amount
proxy-lower-submit = Lower maximum
proxy-lower-success = Proxy maximum lowered to { $maximum } (version { $version }).
