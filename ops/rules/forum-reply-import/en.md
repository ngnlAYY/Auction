# Forum Reply Bidding Rules

Forum replies are an input channel for bids. The auction database remains the authority.

- A forum reply is evaluated when the Worker imports it, not when the forum displays its post time. A reply posted before a deadline can be rejected if it is imported after the deadline.
- The Worker scans the topic every poll interval (for example, 300 seconds). Within one scan, new replies are processed in floor order.
- A soft-close extension starts at import time, not at the forum post time.
- In the final minutes, bid on the website or use a proxy. Do not rely on forum replies for last-second bids.
- If the topic exceeds the configured page limit, forum import for that auction fails closed. Use the website until an operator resolves it.
- Editing a post does not create a new command and does not cancel an accepted bid. Post a new reply and check the result.
- Repeated scans do not place duplicate bids. The command ID is derived from the topic, post, line, and lot code.
