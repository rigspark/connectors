//! End to end: a real MCP client launches the `youtube-mcp` binary over stdio, pointed at a
//! mock YouTube API, and calls its tools the way the crew would.

use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult},
    service::RunningService,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use serde_json::{Value, json};
use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

const KEY: &str = "AIzaTEST-not-a-real-key";
const VID: &str = "dQw4w9WgXcQ";
const CH: &str = "UC_x5XG1OV2P6uZZ5FSM9Ttw";

async fn start(server: &MockServer) -> anyhow::Result<RunningService<RoleClient, ()>> {
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(env!("CARGO_BIN_EXE_youtube-mcp")).configure(|cmd| {
            cmd.env("YOUTUBE_API_KEY", KEY)
                .env("YOUTUBE_API_BASE", server.uri())
                .env("RUST_LOG", "warn");
        }),
    )?;
    Ok(().serve(transport).await?)
}

async fn call(
    client: &RunningService<RoleClient, ()>,
    name: &'static str,
    args: Value,
) -> CallToolResult {
    let Value::Object(map) = args else {
        panic!("args must be an object")
    };
    client
        .call_tool(CallToolRequestParams::new(name).with_arguments(map))
        .await
        .expect("tools/call")
}

fn text(r: &CallToolResult) -> String {
    r.content
        .iter()
        .filter_map(|c| c.as_text().map(|t| t.text.clone()))
        .collect::<Vec<_>>()
        .join("\n")
}

fn video_json() -> Value {
    json!({
        "kind": "youtube#video",
        "id": VID,
        "snippet": {
            "publishedAt": "2009-10-25T06:57:33Z",
            "channelId": CH,
            "channelTitle": "Rick Astley",
            "title": "Never Gonna Give You Up",
            "description": "The official video.",
            "tags": ["rick astley", "80s"],
            "categoryId": "10",
            "liveBroadcastContent": "none",
            "thumbnails": {"high": {"url": "https://i.ytimg.com/vi/x/hqdefault.jpg"}}
        },
        "contentDetails": {"duration": "PT3M33S", "caption": "true"},
        "statistics": {"viewCount": "1700000000", "likeCount": "18000000", "commentCount": "2400000"}
    })
}

#[tokio::test]
async fn lists_tools_and_reports_identity() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    let client = start(&server).await?;

    let info = client.peer_info().expect("server info");
    assert_eq!(
        info.server_info.as_ref().map(|s| s.name.as_str()),
        Some("youtube-mcp")
    );
    assert!(
        info.instructions
            .as_deref()
            .unwrap_or_default()
            .contains("Quota")
    );

    let tools = client.list_all_tools().await?;
    let mut names: Vec<_> = tools.iter().map(|t| t.name.to_string()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "channel_playlists",
            "channel_videos",
            "get_channel",
            "get_videos",
            "playlist_items",
            "search",
            "trending_videos",
            "video_comments"
        ]
    );
    assert!(
        tools
            .iter()
            .all(|t| t.annotations.as_ref().and_then(|a| a.read_only_hint) == Some(true))
    );
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn get_videos_accepts_urls_and_sends_key_in_header() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/videos"))
        .and(header("x-goog-api-key", KEY))
        .and(query_param("id", VID))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": [video_json()]})))
        .expect(1)
        .mount(&server)
        .await;
    let client = start(&server).await?;

    let r = call(
        &client,
        "get_videos",
        json!({"videos": [format!("https://youtu.be/{VID}"), VID]}),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{}", text(&r));
    let v: Value = serde_json::from_str(&text(&r))?;
    assert_eq!(v[0]["id"], VID);
    assert_eq!(v[0]["duration_secs"], 213);
    assert_eq!(v[0]["views"], 1_700_000_000u64);
    assert_eq!(v[0]["has_captions"], true);

    // the key must never be in a URL
    for req in server.received_requests().await.unwrap_or_default() {
        assert!(
            !req.url.as_str().contains(KEY),
            "key leaked into {}",
            req.url
        );
    }
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn search_with_handle_resolves_channel_first() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(path("/channels"))
        .and(query_param("forHandle", "@RickAstleyYT"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"items": [{"id": CH, "snippet": {"title": "Rick Astley"}}]})),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/search"))
        .and(query_param("channelId", CH))
        .and(query_param("type", "video"))
        .and(query_param("videoDuration", "short"))
        .and(query_param("order", "viewCount"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "nextPageToken": "CAUQAA",
            "pageInfo": {"totalResults": 1234},
            "items": [{
                "id": {"kind": "youtube#video", "videoId": VID},
                "snippet": {"title": "Never Gonna Give You Up", "channelId": CH, "channelTitle": "Rick Astley", "publishedAt": "2009-10-25T06:57:33Z", "description": "x"}
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;
    let client = start(&server).await?;

    let r = call(
        &client,
        "search",
        json!({"query": "never gonna", "channel": "@RickAstleyYT", "length": "short", "order": "viewCount"}),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{}", text(&r));
    let v: Value = serde_json::from_str(&text(&r))?;
    assert_eq!(v["items"][0]["kind"], "video");
    assert_eq!(
        v["items"][0]["url"],
        format!("https://www.youtube.com/watch?v={VID}")
    );
    assert_eq!(v["next_page_token"], "CAUQAA");
    assert_eq!(v["total_results"], 1234);
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn channel_videos_keeps_upload_order() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    let a = "aaaaaaaaaaa";
    let b = "bbbbbbbbbbb";
    Mock::given(path("/channels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": [{
            "id": CH,
            "snippet": {"title": "Rick Astley", "customUrl": "@rickastleyyt"},
            "statistics": {"subscriberCount": "4000000", "hiddenSubscriberCount": false},
            "contentDetails": {"relatedPlaylists": {"uploads": "UU_x5XG1OV2P6uZZ5FSM9Ttw"}}
        }]})))
        .mount(&server)
        .await;
    Mock::given(path("/playlistItems"))
        .and(query_param("playlistId", "UU_x5XG1OV2P6uZZ5FSM9Ttw"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": [
            {"snippet": {"position": 0, "title": "A"}, "contentDetails": {"videoId": a}},
            {"snippet": {"position": 1, "title": "B"}, "contentDetails": {"videoId": b}}
        ]})))
        .mount(&server)
        .await;
    // API returns them in the other order
    let mut vb = video_json();
    vb["id"] = json!(b);
    let mut va = video_json();
    va["id"] = json!(a);
    Mock::given(path("/videos"))
        .and(query_param("id", format!("{a},{b}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"items": [vb, va]})))
        .mount(&server)
        .await;
    let client = start(&server).await?;

    let r = call(
        &client,
        "channel_videos",
        json!({"channel": format!("https://www.youtube.com/channel/{CH}"), "max_results": 2}),
    )
    .await;
    assert_ne!(r.is_error, Some(true), "{}", text(&r));
    let v: Value = serde_json::from_str(&text(&r))?;
    assert_eq!(v["items"][0]["id"], a);
    assert_eq!(v["items"][1]["id"], b);
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn upstream_errors_are_tool_errors_with_no_secret() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(path("/commentThreads"))
        .respond_with(ResponseTemplate::new(403).set_body_json(json!({"error": {
            "code": 403, "message": format!("quota exceeded for key {KEY}"),
            "errors": [{"reason": "quotaExceeded"}]
        }})))
        .mount(&server)
        .await;
    Mock::given(path("/videos"))
        .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": {
            "code": 400, "message": format!("API key not valid: {KEY}"), "status": "INVALID_ARGUMENT"
        }})))
        .mount(&server)
        .await;
    let client = start(&server).await?;

    let r = call(&client, "video_comments", json!({"video": VID})).await;
    assert_eq!(r.is_error, Some(true));
    assert!(text(&r).contains("quota"), "{}", text(&r));

    let r = call(&client, "trending_videos", json!({"region": "in"})).await;
    assert_eq!(r.is_error, Some(true));
    assert!(text(&r).contains("rejected the API key"), "{}", text(&r));
    assert!(!text(&r).contains(KEY));
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn bad_arguments_never_reach_youtube() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;
    let client = start(&server).await?;

    for (tool, args, needle) in [
        (
            "get_videos",
            json!({"videos": ["https://evil.example/watch?v=dQw4w9WgXcQ"]}),
            "not a YouTube video",
        ),
        ("get_videos", json!({"videos": []}), "between 1 and 50"),
        (
            "get_channel",
            json!({"channel": "no spaces allowed"}),
            "not a channel",
        ),
        ("search", json!({"query": "  "}), "empty"),
        (
            "search",
            json!({"query": "x", "published_after": "last week"}),
            "RFC 3339",
        ),
        (
            "search",
            json!({"query": "x", "kind": "channel", "length": "long"}),
            "only applies",
        ),
        ("trending_videos", json!({"region": "INDIA"}), "region code"),
        ("trending_videos", json!({"category_id": "music"}), "number"),
        ("playlist_items", json!({"playlist": "x"}), "playlist"),
    ] {
        let r = call(&client, tool, args).await;
        assert_eq!(r.is_error, Some(true), "{tool}");
        assert!(text(&r).contains(needle), "{tool}: {}", text(&r));
    }
    client.cancel().await?;
    Ok(())
}
