# youtube-mcp

Read-only MCP connector for public YouTube data, using the YouTube Data API v3.

## Tools

| Tool | What it returns | Quota units |
| --- | --- | --- |
| `search` | Videos, channels or playlists matching a query. Filters: kind, order, channel, date range, region, language, length | **100** |
| `get_videos` | 1–50 videos with views, likes, comments, duration, tags, captions | 1 |
| `get_channel` | Channel profile, subscribers, views, video count, uploads playlist | 1 |
| `channel_videos` | A channel's latest uploads with stats, newest first | 3 |
| `channel_playlists` | A channel's public playlists | 1–2 |
| `playlist_items` | Videos in a playlist, in order | 1 |
| `video_comments` | Top-level comments: author, text, likes, reply count | 1 |
| `trending_videos` | Most popular videos now, by region and category | 1 |

Each key has 10,000 units a day by default, which is about 100 searches. The server's
instructions tell the model to prefer the cheap tools. Wherever a video, channel or playlist
is expected you can pass an id or a URL (`youtu.be/…`, `/shorts/…`, `/@handle`, `/channel/UC…`,
`?list=…`). Every list result takes `page_token`.

## Setup

1. In Google Cloud, create a project, enable **YouTube Data API v3**, and create an **API key**.
   Restrict the key to that API.
2. Pass it as `YOUTUBE_API_KEY` in the connector's environment. The key is sent in the
   `x-goog-api-key` header, never in a URL, and is scrubbed from all output.

```bash
cargo build --release -p youtube-mcp
YOUTUBE_API_KEY=… ./target/release/youtube-mcp    # speaks MCP on stdio
./target/release/youtube-mcp --manifest           # prints connector.toml
```

## Errors the model sees

| Upstream | Message |
| --- | --- |
| `quotaExceeded` | Quota for today is used up (resets midnight Pacific) |
| bad or disabled key | The key was rejected; check it and that the API is enabled |
| `commentsDisabled` | Comments are turned off for this video |
| `*NotFound` / 404 | Not found: … |
| 429 / 5xx | Retried twice with backoff, then reported |

## Not yet

- **Transcripts / captions.** `captions.download` needs OAuth as the video's owner, and the
  public timed-text endpoints aren't a supported API. This needs its own design (OAuth via the
  crew's credential gateway).
- **Write tools** (update metadata, reply to comments, upload) need OAuth. They'd be declared
  `access = "write"` so the crew asks before each one.
- **Analytics** (YouTube Analytics API, owner-only) also needs OAuth.
