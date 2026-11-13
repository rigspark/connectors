# youtube-mcp

YouTube research, analytics and channel management over MCP: the tools vidIQ charges
credits for, free. It is built on YouTube's own APIs, plus two switchable unofficial
sources and a local history database.

**32 tools and 6 prompts.** Everything is read-only except the five channel-owner writes,
which are declared `access = "write"` so the crew asks before each one.

## vidIQ → youtube-mcp

| vidIQ (5 credits/call) | youtube-mcp (free) | How |
| --- | --- | --- |
| Keyword research (volume, competition) | `keyword_research` | Competition, demand and opportunity scores (0–100) from the top 25 results and their channels. YouTube publishes no search volumes, so there are none here. |
| Matching terms / questions / rising | `keyword_ideas` (`long_tail`, `questions`) | YouTube autocomplete, ranked by how strongly it's suggested *(unofficial)* |
| Outliers / breakout videos | `outliers` | Views ÷ channel's typical views, by keyword, by seed video, or within a channel |
| Trending / fastest-growing | `hot_videos`, `trending_videos` | Views per hour, filtered by topic, format and channel size |
| Channel performance trends | `channel_performance` | View curve by video age, cadence, Shorts vs long-form, best weekday/hour |
| Similar channels / competitors | `similar_channels` | Topic terms from recent titles/tags → channel search → ranked by overlap and size |
| Comment insights (discover/validate) | `comment_insights` | Requests, questions, complaints and praise with themes and examples; `idea` to validate |
| Score title | `title_score` | Transparent checklist across 10 titles at once |
| Earnings calculator / video estimate | `earnings_calculate`, `video_earnings` | Public RPM ranges by category, country tier and format |
| Video transcript | `video_transcript` | Caption tracks, manual preferred, with timestamps *(unofficial)* |
| Channel analytics (retention, traffic, revenue…) | `channel_analytics` | YouTube Analytics API with 16 presets, including YouTube search terms, CTR per video and external sites *(OAuth)* |
| Channel audit / "why is this video stuck?" | `channel_insights` | One call: per-video CTR and retention against your averages, diagnosed as packaging or content, plus traffic, audience and the last 48 hours *(OAuth, from ViewsFix)* |
| Update video / thumbnail, reply to comment, upload | `update_video`, `set_thumbnail`, `reply_to_comment`, `post_comment`, `upload_video` | *(OAuth, write)* |
| Video change history / title-thumbnail wins | `packaging_changes`, `video_history` | From local snapshots, with views per hour before and after each change |
| Generate titles, script, description, chapters, replies | prompts `title_ideas`, `video_script`, `seo_description`, `video_chapters`, `comment_replies`, `content_ideas` | The host's own model writes, guided by the data tools |
| — | `seo_audit`, `channel_history`, `track_channel` | Extras |

**Not included:** AI thumbnail, b-roll, music, voiceover and video generation (these need
generative media models) and the Instagram/TikTok tools. vidIQ's proprietary datasets
(search volumes, cross-channel subscriber overlap, a crawl of title changes across all of
YouTube) can't be rebuilt from public APIs. The closest equivalents above say so in their
output notes.

## Tools

| Group | Tool | Quota units |
| --- | --- | --- |
| Public | `search` 100 · `get_videos` 1 · `get_channel` 1 · `channel_videos` 3 · `channel_playlists` 1 · `playlist_items` 1 · `video_comments` 1 · `trending_videos` 1 | |
| Research | `keyword_research` ~102 · `outliers` ~102 (3 in channel mode) · `hot_videos` ~102 (1 without a query) · `channel_performance` 3–6 · `similar_channels` ~104 · `comment_insights` 1/100 comments · `seo_audit` 1 · `video_earnings` 1–2 · `title_score` 0 · `earnings_calculate` 0 | |
| Your channel | `my_channels` 1 · `channel_analytics` (separate quota) · `channel_insights` ~1 · `update_video` ~51 · `set_thumbnail` 50 · `reply_to_comment` 50 · `post_comment` 50 · `upload_video` ~1,600 | OAuth |
| Unofficial | `keyword_ideas` 0 · `video_transcript` 0 | switchable |
| History | `video_history` 0 · `channel_history` 0 · `packaging_changes` 0 · `track_channel` ~3/channel | local |

The default budget is 10,000 units a day per API key. That covers roughly 90 search-based
research calls, or thousands of the cheap ones.

## Setup

### 1. API key (required)

In Google Cloud, enable **YouTube Data API v3**, create an **API key** and restrict it to
that API. Pass it as `YOUTUBE_API_KEY`. It's sent in a header, never in a URL, never to
youtube.com, and scrubbed from all output.

### 2. Your channel (optional, OAuth)

1. In the same project, enable **YouTube Analytics API**. Then create an OAuth client of type
   **Desktop app**, and add yourself as a test user on the consent screen.
2. Sign in once:
   ```bash
   export YOUTUBE_OAUTH_CLIENT_ID=… YOUTUBE_OAUTH_CLIENT_SECRET=…
   youtube-mcp auth            # analytics only
   youtube-mcp auth --write    # also edits, comments, thumbnails, uploads
   ```
   This opens the browser (loopback redirect with PKCE) and prints `YOUTUBE_OAUTH_REFRESH_TOKEN=…`.
3. Store the three values in the host's credential store and pass them in the connector's
   environment.
4. For uploads and thumbnails, set `YOUTUBE_MCP_UPLOAD_DIR` to the one folder files may be
   read from. Paths outside it, including `..` tricks and symlinks, are refused. Uploads
   are private unless told otherwise.

### 3. History (on by default)

Snapshots of every video and channel fetched go to
`~/.rigspark/youtube-mcp/history.sqlite` (or `YOUTUBE_MCP_DB`; `off` disables it). To
track growth and catch title/thumbnail changes on competitors, schedule `track_channel`
daily with the channels you care about. Thumbnails are fingerprinted, so a thumbnail
swap is detected even though its URL doesn't change.

### 4. Unofficial sources (on by default)

`keyword_ideas`, `video_transcript` and the related keywords in `keyword_research` use
YouTube's own autocomplete and caption endpoints. These aren't a supported API: they can
change without notice and sit in a grey area of YouTube's terms. Set
`YOUTUBE_MCP_UNOFFICIAL=off` to disable them; everything else keeps working.

## Host config

```json
{
  "mcpServers": {
    "youtube": {
      "command": "/path/to/youtube-mcp",
      "env": {
        "YOUTUBE_API_KEY": "…",
        "YOUTUBE_OAUTH_CLIENT_ID": "…",
        "YOUTUBE_OAUTH_CLIENT_SECRET": "…",
        "YOUTUBE_OAUTH_REFRESH_TOKEN": "…",
        "YOUTUBE_MCP_UPLOAD_DIR": "/Users/me/Videos/to-upload"
      }
    }
  }
}
```

## How the scores work

All scores are transparent heuristics, and each result includes the inputs behind it.

- **Competition** combines median views and median channel size of the top results, the
  share of results from 1M+ subscriber channels, and the share with the exact keyword in
  the title. It is lowered when small channels (<10k subscribers) rank.
- **Demand** is median views and views per hour of the top results.
  **Opportunity** = √(demand × (1 − competition)).
- **Breakout** = a video's views ÷ its channel's median recent views (channel mode) or
  lifetime average views per video (keyword mode).
- **Title score** is a 100-point checklist: length, keyword and its position, number,
  hook word, caps, punctuation, clutter, word count.
- **Channel insights diagnosis** compares each video's thumbnail CTR and average percentage
  watched with the channel's (CTR = clicks ÷ impressions over every video; retention weighted
  by views). More than 15% below on CTR only is **packaging** (fix the title and thumbnail),
  on retention only is **content** (fix the hook and pacing), on both is
  **packaging_and_content**; 15% above on both is **outperforming**. Videos with fewer than
  1,000 impressions or 100 views get no diagnosis.
- **Views per minute** (in `channel_performance`) = views ÷ runtime in minutes, to see which
  lengths earn their time.
- **Earnings** use mid RPMs by category for US/UK/CA/AU long-form, × a country-tier factor,
  with Shorts at about $0.06/1k. The range shown is ×0.5 to ×1.8.

## Privacy

Comment text is third-party data. Before it reaches the model, email addresses and phone
numbers in it are replaced with `[email removed]` and `[phone removed]` (a phone is `+` and
8–15 digits, or 10–15 digits; view counts, dates and timestamps are left alone).

## Credits

`channel_insights`, the CTR, per-video retention and external-site presets, views per minute
and comment scrubbing are ported from ViewsFix (yt-reviewer)'s private YouTube MCP tools. Its
CTR tool asked for `impressions,impressionClickThroughRate`; YouTube's metric names are
`videoThumbnailImpressions` and `videoThumbnailImpressionsClickRate`, used here.
