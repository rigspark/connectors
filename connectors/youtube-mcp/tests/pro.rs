//! End to end for the research, owner, unofficial and history tools: a real MCP client runs
//! the binary over stdio with every upstream (Data API, Analytics, uploads, OAuth, youtube.com,
//! autocomplete) mocked under one base URL.

use std::time::Duration;

use connector_kit::time::{format_rfc3339, now_unix};
use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, GetPromptRequestParams},
    service::RunningService,
    transport::{ConfigureCommandExt, TokioChildProcess},
};
use serde_json::{Value, json};
use wiremock::matchers::{
    body_string_contains, header, method, path, query_param, query_param_is_missing,
};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

const KEY: &str = "AIzaTEST-not-a-real-key";
const REFRESH: &str = "1//refresh-not-real-0123";
const CLIENT_SECRET: &str = "GOCSPX-not-real-secret";
const A: &str = "UCaaaaaaaaaaaaaaaaaaaaaa";
const B: &str = "UCbbbbbbbbbbbbbbbbbbbbbb";
const C: &str = "UCcccccccccccccccccccccc";
const VID: &str = "dQw4w9WgXcQ";

type Client = RunningService<RoleClient, ()>;

async fn start(server: &MockServer, env: &[(&str, String)]) -> anyhow::Result<Client> {
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    let uri = server.uri();
    let transport = TokioChildProcess::new(
        tokio::process::Command::new(env!("CARGO_BIN_EXE_youtube-mcp")).configure(move |cmd| {
            cmd.env("YOUTUBE_API_KEY", KEY)
                .env("YOUTUBE_API_BASE", &uri)
                .env("RUST_LOG", "warn")
                .env("YOUTUBE_MCP_DB", "off");
            for (k, v) in &env {
                cmd.env(k, v);
            }
        }),
    )?;
    Ok(().serve(transport).await?)
}

fn oauth_env() -> Vec<(&'static str, String)> {
    vec![
        (
            "YOUTUBE_OAUTH_CLIENT_ID",
            "client.apps.googleusercontent.com".into(),
        ),
        ("YOUTUBE_OAUTH_CLIENT_SECRET", CLIENT_SECRET.into()),
        ("YOUTUBE_OAUTH_REFRESH_TOKEN", REFRESH.into()),
    ]
}

async fn call(client: &Client, name: &'static str, args: Value) -> CallToolResult {
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

fn ok(r: &CallToolResult) -> Value {
    assert_ne!(r.is_error, Some(true), "{}", text(r));
    serde_json::from_str(&text(r)).expect("json result")
}

fn err(r: &CallToolResult) -> String {
    assert_eq!(r.is_error, Some(true), "expected an error, got {}", text(r));
    text(r)
}

fn vid_id(i: usize) -> String {
    format!("vid{i:08}")
}

fn video(id: &str, views: u64, days_old: f64, channel: &str, title: &str) -> Value {
    json!({
        "id": id,
        "snippet": {
            "publishedAt": format_rfc3339(now_unix() - (days_old * 86_400.0) as i64),
            "channelId": channel, "channelTitle": format!("chan {}", &channel[2..5]),
            "title": title, "description": "desc", "tags": ["sourdough", "bread"], "categoryId": "26",
            "thumbnails": {"high": {"url": format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg")}}
        },
        "contentDetails": {"duration": "PT10M", "caption": "false"},
        "statistics": {"viewCount": views.to_string(), "likeCount": (views / 25).to_string(), "commentCount": (views / 400).to_string()}
    })
}

fn channel(id: &str, subs: u64, views: u64, videos: u64, desc: &str) -> Value {
    json!({
        "id": id,
        "snippet": {"title": format!("Channel {}", &id[2..6]), "description": desc, "country": "US", "customUrl": "@chan"},
        "statistics": {"subscriberCount": subs.to_string(), "viewCount": views.to_string(), "videoCount": videos.to_string()},
        "contentDetails": {"relatedPlaylists": {"uploads": format!("UU{}", &id[2..])}}
    })
}

fn items(v: Vec<Value>) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(json!({ "items": v }))
}

fn search_items(ids: &[String]) -> ResponseTemplate {
    items(ids.iter().map(|i| json!({"id": {"kind": "youtube#video", "videoId": i}, "snippet": {"title": "t"}})).collect())
}

fn playlist(ids: &[String]) -> ResponseTemplate {
    items(ids.iter().enumerate().map(|(n, i)| json!({"snippet": {"position": n, "title": "t"}, "contentDetails": {"videoId": i}})).collect())
}

/// Autocomplete: answers `[q, [q + " easy", q + " recipe", …]]`, like the real endpoint.
struct Suggest;
impl Respond for Suggest {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let q = req
            .url
            .query_pairs()
            .find(|(k, _)| k == "q")
            .map(|(_, v)| v.into_owned())
            .unwrap_or_default();
        let q = q.trim();
        ResponseTemplate::new(200).set_body_json(json!([
            q,
            [
                format!("{q} easy"),
                format!("{q} recipe"),
                format!("{q} for beginners")
            ]
        ]))
    }
}

// ---------- research ----------

#[tokio::test]
async fn keyword_research_scores_and_related_keywords() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    let ids: Vec<String> = (0..10).map(vid_id).collect();
    Mock::given(path("/search"))
        .and(query_param("q", "sourdough discard"))
        .and(query_param("type", "video"))
        .respond_with(search_items(&ids))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(path("/videos"))
        .respond_with(items(
            ids.iter()
                .map(|i| video(i, 30_000, 60.0, A, "Sourdough discard crackers"))
                .collect(),
        ))
        .mount(&server)
        .await;
    Mock::given(path("/channels"))
        .respond_with(items(vec![channel(A, 8_000, 400_000, 40, "baking")]))
        .mount(&server)
        .await;
    Mock::given(path("/suggest"))
        .respond_with(Suggest)
        .mount(&server)
        .await;
    let client = start(&server, &[]).await?;

    let v = ok(&call(
        &client,
        "keyword_research",
        json!({"keyword": "sourdough discard"}),
    )
    .await);
    let s = &v["scores"];
    assert!(s["competition"].as_u64().unwrap() < 50, "{s}");
    assert_eq!(s["small_channel_wins"], 1.0);
    assert!(s["opportunity"].as_u64().unwrap() > 30, "{s}");
    assert!(
        v["related_keywords"]
            .as_array()
            .unwrap()
            .iter()
            .any(|k| k["keyword"] == "sourdough discard recipe")
    );
    assert_eq!(v["top_results"].as_array().unwrap().len(), 10);

    for r in server.received_requests().await.unwrap() {
        if r.url.path() == "/suggest" {
            assert!(
                r.headers.get("x-goog-api-key").is_none(),
                "API key leaked to the unofficial endpoint"
            );
        }
    }
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn keyword_ideas_long_tail_and_questions() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    Mock::given(path("/suggest"))
        .respond_with(Suggest)
        .expect(27 + 13)
        .mount(&server)
        .await;
    let client = start(&server, &[]).await?;
    let v = ok(&call(
        &client,
        "keyword_ideas",
        json!({"keyword": "air fryer", "mode": "long_tail", "limit": 100}),
    )
    .await);
    let ideas = v["ideas"].as_array().unwrap();
    assert!(ideas.len() > 50, "{}", ideas.len());
    assert_eq!(ideas[0]["popularity"], 100);
    let v = ok(&call(
        &client,
        "keyword_ideas",
        json!({"keyword": "air fryer", "mode": "questions"}),
    )
    .await);
    assert!(v["ideas"].as_array().unwrap().iter().any(|i| {
        i["keyword"]
            .as_str()
            .unwrap()
            .starts_with("how to air fryer")
    }));
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn outliers_in_a_channel_and_by_keyword() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    let ids: Vec<String> = (0..5).map(vid_id).collect();
    Mock::given(path("/channels"))
        .and(query_param("forHandle", "@baker"))
        .respond_with(items(vec![channel(A, 50_000, 5_000_000, 100, "")]))
        .mount(&server)
        .await;
    Mock::given(path("/playlistItems"))
        .respond_with(playlist(&ids))
        .mount(&server)
        .await;
    let views = [1_000, 1_200, 900, 15_000, 1_100];
    Mock::given(path("/videos"))
        .and(query_param("id", ids.join(",")))
        .respond_with(items(
            ids.iter()
                .zip(views)
                .map(|(i, v)| video(i, v, 20.0, A, "t"))
                .collect(),
        ))
        .mount(&server)
        .await;
    // keyword mode: one breakout from a small channel, one big-channel video at its usual level
    let k: Vec<String> = vec![vid_id(10), vid_id(11)];
    Mock::given(path("/search"))
        .respond_with(search_items(&k))
        .mount(&server)
        .await;
    Mock::given(path("/videos"))
        .and(query_param("id", k.join(",")))
        .respond_with(items(vec![
            video(&k[0], 80_000, 30.0, B, "small breakout"),
            video(&k[1], 500_000, 30.0, C, "big usual"),
        ]))
        .mount(&server)
        .await;
    Mock::given(path("/channels"))
        .and(query_param("id", format!("{B},{C}")))
        .respond_with(items(vec![
            channel(B, 5_000, 1_000_000, 100, ""),
            channel(C, 3_000_000, 1_000_000_000, 1_000, ""),
        ]))
        .mount(&server)
        .await;
    let client = start(&server, &[]).await?;

    let v = ok(&call(&client, "outliers", json!({"channel": "@baker"})).await);
    let out = v["outliers"].as_array().unwrap();
    assert_eq!(out.len(), 1);
    assert_eq!(out[0]["id"], ids[3]);
    assert!(out[0]["breakout"].as_f64().unwrap() > 10.0);

    let v = ok(&call(
        &client,
        "outliers",
        json!({"keyword": "bread", "max_subscribers": 100000}),
    )
    .await);
    let out = v["outliers"].as_array().unwrap();
    assert_eq!(out.len(), 1, "{v}");
    assert_eq!(out[0]["title"], "small breakout");
    assert_eq!(out[0]["breakout"], 8.0);

    assert!(
        err(&call(
            &client,
            "outliers",
            json!({"keyword": "x", "channel": "@baker"})
        )
        .await)
        .contains("exactly one")
    );
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn hot_videos_sorted_by_views_per_hour() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    let ids: Vec<String> = (0..3).map(vid_id).collect();
    Mock::given(path("/search"))
        .and(query_param("order", "viewCount"))
        .respond_with(search_items(&ids))
        .mount(&server)
        .await;
    Mock::given(path("/videos"))
        .respond_with(items(vec![
            video(&ids[0], 100_000, 6.0, A, "slow"),
            video(&ids[1], 50_000, 0.5, A, "fast"),
            video(&ids[2], 10_000, 3.0, A, "meh"),
        ]))
        .mount(&server)
        .await;
    Mock::given(path("/channels"))
        .respond_with(items(vec![channel(A, 1_000, 1, 1, "")]))
        .mount(&server)
        .await;
    let client = start(&server, &[]).await?;
    let v = ok(&call(
        &client,
        "hot_videos",
        json!({"query": "bread", "within_days": 7}),
    )
    .await);
    let titles: Vec<&str> = v["videos"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["fast", "slow", "meh"]);
    assert!(
        server.received_requests().await.unwrap().iter().any(|r| r
            .url
            .query()
            .unwrap_or("")
            .contains("publishedAfter="))
    );
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn channel_performance_and_similar_channels() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    let ids: Vec<String> = (0..20).map(vid_id).collect();
    Mock::given(path("/channels"))
        .and(query_param("forHandle", "@baker"))
        .respond_with(items(vec![channel(A, 50_000, 5_000_000, 100, "")]))
        .mount(&server)
        .await;
    Mock::given(path("/playlistItems"))
        .respond_with(playlist(&ids))
        .mount(&server)
        .await;
    Mock::given(path("/videos"))
        .respond_with(items(
            ids.iter()
                .enumerate()
                .map(|(n, i)| {
                    video(
                        i,
                        1_000 * (n as u64 + 1),
                        n as f64 * 4.0 + 0.5,
                        A,
                        "Sourdough bread for beginners",
                    )
                })
                .collect(),
        ))
        .mount(&server)
        .await;
    Mock::given(path("/search"))
        .and(query_param("type", "channel"))
        .respond_with(items(vec![
            json!({"id": {"kind": "youtube#channel", "channelId": B}, "snippet": {}}),
            json!({"id": {"kind": "youtube#channel", "channelId": C}, "snippet": {}}),
        ]))
        .mount(&server)
        .await;
    Mock::given(path("/channels"))
        .and(query_param("id", format!("{B},{C}")))
        .respond_with(items(vec![
            channel(C, 40_000, 1, 1, "gaming streams and esports"),
            channel(B, 60_000, 1, 1, "sourdough bread baking for beginners"),
        ]))
        .mount(&server)
        .await;
    let client = start(&server, &[]).await?;

    let v = ok(&call(
        &client,
        "channel_performance",
        json!({"channel": "@baker", "videos": 20}),
    )
    .await);
    let p = &v["performance"];
    assert_eq!(p["videos_analyzed"], 20);
    assert!(p["uploads_per_week"].as_f64().unwrap() > 1.0);
    assert!(!p["views_by_age"].as_array().unwrap().is_empty());

    let v = ok(&call(&client, "similar_channels", json!({"channel": "@baker"})).await);
    let sim = v["similar"].as_array().unwrap();
    assert_eq!(sim[0]["id"], B, "{v}");
    assert!(sim.iter().all(|c| c["id"] != A));
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn comment_insights_pages_and_validates_an_idea() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    let thread = |n: usize, t: &str| json!({"id": format!("c{n}"), "snippet": {"totalReplyCount": 0, "topLevelComment": {"snippet": {"textOriginal": t, "authorDisplayName": "viewer", "likeCount": n, "publishedAt": "2026-01-01T00:00:00Z"}}}});
    let page1: Vec<Value> = (0..100)
        .map(|n| {
            thread(
                n,
                if n % 4 == 0 {
                    "please make a video on air fryer chips"
                } else {
                    "great video, thanks"
                },
            )
        })
        .collect();
    let page2: Vec<Value> = (100..150)
        .map(|n| thread(n, "how do you clean the basket?"))
        .collect();
    Mock::given(path("/commentThreads"))
        .and(query_param_is_missing("pageToken"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"items": page1, "nextPageToken": "p2"})),
        )
        .mount(&server)
        .await;
    Mock::given(path("/commentThreads"))
        .and(query_param("pageToken", "p2"))
        .respond_with(items(page2))
        .mount(&server)
        .await;
    let client = start(&server, &[]).await?;
    let v = ok(&call(
        &client,
        "comment_insights",
        json!({"video": VID, "idea": "air fryer chips", "max_comments": 500}),
    )
    .await);
    assert_eq!(v["comments_analyzed"], 150);
    let intents: Vec<&str> = v["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["intent"].as_str().unwrap())
        .collect();
    assert!(
        intents.contains(&"content_request")
            && intents.contains(&"question")
            && intents.contains(&"praise"),
        "{intents:?}"
    );
    assert!(v["idea"]["verdict"].as_str().unwrap().starts_with("Strong"));
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn seo_titles_and_earnings() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    let mut v = video(
        VID,
        2_000_000,
        365.0,
        A,
        "7 Budget Phone Mistakes Nobody Talks About",
    );
    v["snippet"]["description"] = json!(format!(
        "Budget phone buying guide. {}\n00:00 Intro\n02:00 Mistakes\n08:00 Picks\nhttps://x.y #phones",
        "word ".repeat(60)
    ));
    v["snippet"]["tags"] = json!(["budget phone", "phones"]);
    v["snippet"]["categoryId"] = json!("28");
    v["snippet"]["defaultLanguage"] = json!("en");
    v["contentDetails"]["caption"] = json!("true");
    v["status"] = json!({"madeForKids": false});
    Mock::given(path("/videos"))
        .respond_with(items(vec![v]))
        .mount(&server)
        .await;
    Mock::given(path("/channels"))
        .respond_with(items(vec![channel(A, 100_000, 1, 1, "")]))
        .mount(&server)
        .await;
    let client = start(&server, &[]).await?;

    let s = ok(&call(
        &client,
        "seo_audit",
        json!({"video": VID, "keyword": "budget phone"}),
    )
    .await);
    assert!(s["seo"]["score"].as_u64().unwrap() >= 85, "{s}");

    let t = ok(&call(&client, "title_score", json!({"titles": ["MY VIDEO!!!!", "7 Budget Phone Mistakes Nobody Talks About"], "keyword": "budget phone"})).await);
    assert_eq!(
        t["ranked"][0]["title"],
        "7 Budget Phone Mistakes Nobody Talks About"
    );

    let e = ok(&call(
        &client,
        "earnings_calculate",
        json!({"monthly_views": 1_000_000, "category": "Technology", "country": "us"}),
    )
    .await);
    assert_eq!(e["estimate"]["monthly_usd_mid"], 7000.0);

    let ve = ok(&call(&client, "video_earnings", json!({"video": VID})).await);
    assert_eq!(ve["category"], "Technology");
    assert!(ve["lifetime_usd"]["mid"].as_f64().unwrap() > 1000.0);
    client.cancel().await?;
    Ok(())
}

// ---------- unofficial ----------

#[tokio::test]
async fn transcripts_via_player_api() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    let track = format!("{}/web/api/timedtext?v={VID}&lang=en", server.uri());
    Mock::given(method("POST")).and(path("/web/youtubei/v1/player")).and(body_string_contains(VID)).respond_with(ResponseTemplate::new(200).set_body_json(json!({
        "playabilityStatus": {"status": "OK"},
        "videoDetails": {"videoId": VID},
        "captions": {"playerCaptionsTracklistRenderer": {"captionTracks": [
            {"baseUrl": track, "languageCode": "en", "kind": "asr", "name": {"simpleText": "English (auto)"}}
        ]}}
    }))).mount(&server).await;
    Mock::given(path("/web/api/timedtext"))
        .and(query_param("fmt", "json3"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"events": [
            {"tStartMs": 1000, "segs": [{"utf8": "hello"}, {"utf8": " there"}]},
            {"tStartMs": 65_000, "segs": [{"utf8": "second line"}]}
        ]})))
        .mount(&server)
        .await;
    let client = start(&server, &[]).await?;
    let v = ok(&call(
        &client,
        "video_transcript",
        json!({"video": format!("https://youtu.be/{VID}")}),
    )
    .await);
    assert_eq!(v["text"], "[00:01] hello there\n[01:05] second line");
    assert_eq!(v["kind"], "auto");
    assert!(
        err(&call(
            &client,
            "video_transcript",
            json!({"video": VID, "language": "fr"})
        )
        .await)
        .contains("available: en (auto)")
    );
    for r in server.received_requests().await.unwrap() {
        assert!(
            r.headers.get("x-goog-api-key").is_none(),
            "API key sent to {}",
            r.url.path()
        );
    }
    client.cancel().await?;

    let client = start(&server, &[("YOUTUBE_MCP_UNOFFICIAL", "off".into())]).await?;
    assert!(
        err(&call(&client, "video_transcript", json!({"video": VID})).await)
            .contains("switched off")
    );
    assert!(
        err(&call(&client, "keyword_ideas", json!({"keyword": "x"})).await)
            .contains("switched off")
    );
    client.cancel().await?;
    Ok(())
}

// ---------- your channel (OAuth) ----------

async fn mock_token(server: &MockServer, times: u64) {
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains("grant_type=refresh_token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token": "ya29.test-access", "expires_in": 3600})),
        )
        .expect(times)
        .mount(server)
        .await;
}

#[tokio::test]
async fn owner_tools_need_oauth() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    let client = start(&server, &[]).await?;
    assert!(err(&call(&client, "my_channels", json!({})).await).contains("youtube-mcp auth"));
    assert!(
        err(&call(&client, "update_video", json!({"video": VID, "title": "x"})).await)
            .contains("aren't set up")
    );
    client.cancel().await?;

    // a revoked refresh token: clear message, no secret in it
    Mock::given(path("/token")).respond_with(ResponseTemplate::new(400).set_body_json(json!({"error": "invalid_grant", "error_description": format!("Token {REFRESH} has been expired or revoked.")}))).mount(&server).await;
    let client = start(&server, &oauth_env()).await?;
    let e = err(&call(&client, "my_channels", json!({})).await);
    assert!(e.contains("refresh token"), "{e}");
    assert!(!e.contains(REFRESH) && !e.contains(CLIENT_SECRET), "{e}");
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn analytics_reports_with_cached_token() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    mock_token(&server, 1).await;
    Mock::given(path("/analytics/reports"))
        .and(header("authorization", "Bearer ya29.test-access"))
        .and(query_param("ids", "channel==MINE"))
        .and(query_param("dimensions", "video"))
        .and(query_param("sort", "-estimatedRevenue"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "columnHeaders": [{"name": "video"}, {"name": "estimatedRevenue"}, {"name": "views"}, {"name": "cpm"}, {"name": "playbackBasedCpm"}, {"name": "monetizedPlaybacks"}],
            "rows": [[VID, 120.5, 50_000, 9.1, 10.2, 20_000]]
        })))
        .mount(&server)
        .await;
    Mock::given(path("/analytics/reports"))
        .and(query_param("filters", format!("video=={VID}")))
        .and(query_param("dimensions", "elapsedVideoTimeRatio"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"columnHeaders": [{"name": "elapsedVideoTimeRatio"}, {"name": "audienceWatchRatio"}, {"name": "relativeRetentionPerformance"}], "rows": [[0.01, 1.0, 0.6]]})))
        .mount(&server)
        .await;
    let client = start(&server, &oauth_env()).await?;
    let v = ok(&call(&client, "channel_analytics", json!({"report": "revenue_per_video", "start_date": "2026-01-01", "end_date": "2026-01-31"})).await);
    assert_eq!(v["rows"][0]["rpm"], 2.41);
    let v = ok(&call(
        &client,
        "channel_analytics",
        json!({"report": "audience_retention", "video": VID}),
    )
    .await);
    assert_eq!(v["rows"][0]["audienceWatchRatio"], 1.0);
    assert!(
        err(&call(
            &client,
            "channel_analytics",
            json!({"report": "audience_retention"})
        )
        .await)
        .contains("needs `video`")
    );
    assert!(
        err(&call(
            &client,
            "channel_analytics",
            json!({"report": "overview", "start_date": "last month"})
        )
        .await)
        .contains("date like")
    );
    client.cancel().await?;
    Ok(())
}

fn report(cols: &[&str], rows: Value) -> ResponseTemplate {
    let headers: Vec<Value> = cols.iter().map(|c| json!({ "name": c })).collect();
    ResponseTemplate::new(200).set_body_json(json!({ "columnHeaders": headers, "rows": rows }))
}

#[tokio::test]
async fn channel_insights_joins_reports_and_survives_a_failed_section() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    mock_token(&server, 1).await;
    let an = || {
        Mock::given(path("/analytics/reports"))
            .and(header("authorization", "Bearer ya29.test-access"))
    };
    an().and(query_param(
        "metrics",
        "videoThumbnailImpressions,videoThumbnailImpressionsClickRate,views",
    ))
    .respond_with(report(
        &[
            "video",
            "videoThumbnailImpressions",
            "videoThumbnailImpressionsClickRate",
            "views",
        ],
        json!([["good", 10000, 8.0, 900], ["weak", 10000, 2.0, 250]]),
    ))
    .mount(&server)
    .await;
    an().and(query_param(
        "metrics",
        "views,averageViewDuration,averageViewPercentage",
    ))
    .respond_with(report(
        &[
            "video",
            "views",
            "averageViewDuration",
            "averageViewPercentage",
        ],
        json!([["good", 1000, 300, 60.0], ["weak", 300, 280, 55.0]]),
    ))
    .mount(&server)
    .await;
    an().and(query_param("dimensions", "insightTrafficSourceType"))
        .respond_with(report(
            &[
                "insightTrafficSourceType",
                "views",
                "engagedViews",
                "estimatedMinutesWatched",
            ],
            json!([
                ["YT_SEARCH", 600, 500, 3000],
                ["RELATED_VIDEO", 400, 300, 1500]
            ]),
        ))
        .mount(&server)
        .await;
    an().and(query_param("filters", "insightTrafficSourceType==EXT_URL"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_json(json!({"error": {"code": 403, "message": "Forbidden"}})),
        )
        .mount(&server)
        .await;
    an().and(query_param("dimensions", "ageGroup,gender"))
        .respond_with(report(
            &["ageGroup", "gender", "viewerPercentage"],
            json!([["age18-24", "male", 40.0], ["age25-34", "female", 60.0]]),
        ))
        .mount(&server)
        .await;
    an().and(query_param("dimensions", "country"))
        .respond_with(report(
            &[
                "country",
                "views",
                "estimatedMinutesWatched",
                "averageViewDuration",
            ],
            json!([["IN", 700, 2000, 200], ["US", 300, 900, 210]]),
        ))
        .mount(&server)
        .await;
    an().and(query_param("metrics", "views"))
        .and(query_param("dimensions", "day"))
        .respond_with(report(
            &["day", "views"],
            json!([["2026-11-11", 120], ["2026-11-12", 80]]),
        ))
        .mount(&server)
        .await;
    an().and(query_param("metrics", "views"))
        .and(query_param("dimensions", "video"))
        .respond_with(report(&["video", "views"], json!([["good", 150]])))
        .mount(&server)
        .await;
    Mock::given(path("/videos"))
        .and(header("x-goog-api-key", KEY))
        .respond_with(items(vec![
            video("good", 50_000, 10.0, A, "The one that worked"),
            video("weak", 9_000, 12.0, A, "The one that didn't"),
        ]))
        .mount(&server)
        .await;

    let client = start(&server, &oauth_env()).await?;
    let v = ok(&call(
        &client,
        "channel_insights",
        json!({"start_date": "2026-10-01", "end_date": "2026-10-28"}),
    )
    .await);
    assert_eq!(v["channel_averages"]["ctr_pct"], 5.0);
    assert_eq!(v["videos"][0]["video_id"], "good");
    assert_eq!(v["videos"][0]["title"], "The one that worked");
    assert_eq!(v["videos"][0]["diagnosis"], "on_par");
    assert_eq!(v["videos"][1]["diagnosis"], "packaging");
    assert_eq!(v["traffic_sources"][0]["label"], "YouTube search");
    assert_eq!(v["traffic_sources"][0]["share_pct"], 60.0);
    assert_eq!(v["audience"]["age_groups"][0]["name"], "25-34");
    assert_eq!(v["audience"]["top_countries"][0]["name"], "IN");
    assert_eq!(v["last_48_hours"]["total_views"], 200);
    assert_eq!(
        v["last_48_hours"]["top_videos"][0]["title"],
        "The one that worked"
    );
    assert!(
        v["headlines"][0]
            .as_str()
            .unwrap()
            .contains("below your 5% average CTR"),
        "{}",
        v["headlines"]
    );
    let notes = v["notes"].to_string();
    assert!(notes.contains("external sites:"), "{notes}");
    assert!(!notes.contains(REFRESH) && !notes.contains(CLIENT_SECRET));
    assert_eq!(v["external_sites"], json!([]));

    // second call within the hour comes from the cache
    let again = ok(&call(
        &client,
        "channel_insights",
        json!({"start_date": "2026-10-01", "end_date": "2026-10-28"}),
    )
    .await);
    assert_eq!(again["cached_minutes_ago"], 0);
    assert_eq!(again["videos"], v["videos"]);
    client.cancel().await?;
    Ok(())
}

#[tokio::test]
async fn writes_update_reply_thumbnail_upload() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    mock_token(&server, 1).await;
    let dir = tempfile::tempdir()?;
    std::fs::write(dir.path().join("thumb.jpg"), vec![0xFFu8; 50_000])?;
    std::fs::write(dir.path().join("clip.mp4"), vec![7u8; 300_000])?;

    // update_video
    Mock::given(method("GET")).and(path("/videos")).and(query_param("part", "snippet,status")).respond_with(items(vec![json!({
        "id": VID,
        "snippet": {"title": "Old title", "description": "d", "categoryId": "28", "channelId": A, "thumbnails": {}, "publishedAt": "2026-01-01T00:00:00Z"},
        "status": {"privacyStatus": "public", "embeddable": true}
    })])).mount(&server).await;
    Mock::given(method("PUT"))
        .and(path("/videos"))
        .and(query_param("part", "snippet"))
        .and(body_string_contains("\"title\":\"New title\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": VID})))
        .expect(1)
        .mount(&server)
        .await;
    // reply
    Mock::given(method("POST"))
        .and(path("/comments"))
        .and(body_string_contains("\"parentId\":\"Ugzthread123\""))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"id": "Ugzthread123.reply1", "snippet": {"textOriginal": "Thanks!"}}),
        ))
        .expect(1)
        .mount(&server)
        .await;
    // thumbnail
    Mock::given(method("POST"))
        .and(path("/upload/thumbnails/set"))
        .and(query_param("videoId", VID))
        .and(header("content-type", "image/jpeg"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"items": [{"default": {"url": "https://i.ytimg.com/x.jpg"}}]}),
            ),
        )
        .expect(1)
        .mount(&server)
        .await;
    // resumable upload
    let session = format!("{}/upload-session/abc", server.uri());
    Mock::given(method("POST"))
        .and(path("/upload/videos"))
        .and(query_param("uploadType", "resumable"))
        .and(header("x-upload-content-length", "300000"))
        .respond_with(ResponseTemplate::new(200).insert_header("location", session.as_str()))
        .mount(&server)
        .await;
    Mock::given(method("PUT")).and(path("/upload-session/abc")).and(header("content-range", "bytes 0-299999/300000"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"id": "newvid00001", "status": {"privacyStatus": "private", "uploadStatus": "uploaded"}})))
        .expect(1)
        .mount(&server)
        .await;

    let mut env = oauth_env();
    env.push(("YOUTUBE_MCP_UPLOAD_DIR", dir.path().display().to_string()));
    let client = start(&server, &env).await?;

    let v = ok(&call(
        &client,
        "update_video",
        json!({"video": VID, "title": "New title"}),
    )
    .await);
    assert_eq!(v["changed"][0]["field"], "title");
    assert_eq!(v["changed"][0]["before"], "Old title");
    let put = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.method.as_str() == "PUT")
        .unwrap();
    let body: Value = serde_json::from_slice(&put.body)?;
    assert!(
        body["snippet"].get("thumbnails").is_none() && body["snippet"].get("channelId").is_none(),
        "read-only fields sent back"
    );

    let v = ok(&call(
        &client,
        "reply_to_comment",
        json!({"comment_id": "Ugzthread123", "text": "Thanks!"}),
    )
    .await);
    assert_eq!(v["comment_id"], "Ugzthread123.reply1");

    ok(&call(
        &client,
        "set_thumbnail",
        json!({"video": VID, "file": "thumb.jpg"}),
    )
    .await);
    assert!(
        err(&call(
            &client,
            "set_thumbnail",
            json!({"video": VID, "file": "/etc/hosts"})
        )
        .await)
        .contains("outside the upload folder")
    );
    assert!(
        err(&call(
            &client,
            "set_thumbnail",
            json!({"video": VID, "file": "../../etc/passwd"})
        )
        .await)
        .contains("upload folder")
    );

    let v = ok(&call(
        &client,
        "upload_video",
        json!({"file": "clip.mp4", "title": "My upload"}),
    )
    .await);
    assert_eq!(v["video_id"], "newvid00001");
    assert_eq!(v["privacy"], "private");
    let start_req = server
        .received_requests()
        .await
        .unwrap()
        .into_iter()
        .find(|r| r.url.path() == "/upload/videos")
        .unwrap();
    let meta: Value = serde_json::from_slice(&start_req.body)?;
    assert_eq!(meta["status"]["privacyStatus"], "private");
    client.cancel().await?;

    // uploads are off unless the host names a folder
    let client = start(&server, &oauth_env()).await?;
    assert!(
        err(&call(
            &client,
            "upload_video",
            json!({"file": "clip.mp4", "title": "x"})
        )
        .await)
        .contains("YOUTUBE_MCP_UPLOAD_DIR")
    );
    client.cancel().await?;
    Ok(())
}

// ---------- history ----------

#[tokio::test]
async fn history_records_and_detects_title_changes() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    let dir = tempfile::tempdir()?;
    let db = dir.path().join("h.sqlite");
    Mock::given(path("/videos"))
        .and(query_param("id", VID))
        .respond_with(items(vec![video(VID, 1_000, 2.0, A, "Old title")]))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(path("/videos"))
        .and(query_param("id", VID))
        .respond_with(items(vec![video(VID, 5_000, 2.0, A, "New title")]))
        .mount(&server)
        .await;
    // track_channel
    let ids: Vec<String> = (0..3).map(vid_id).collect();
    Mock::given(path("/channels"))
        .respond_with(items(vec![channel(A, 1_234, 50_000, 3, "")]))
        .mount(&server)
        .await;
    Mock::given(path("/playlistItems"))
        .respond_with(playlist(&ids))
        .mount(&server)
        .await;
    let thumb = format!("{}/thumb/x.jpg", server.uri());
    Mock::given(path("/videos"))
        .and(query_param("id", ids.join(",")))
        .respond_with(items(
            ids.iter()
                .map(|i| {
                    let mut v = video(i, 100, 1.0, A, "t");
                    v["snippet"]["thumbnails"] = json!({"high": {"url": thumb}});
                    v
                })
                .collect(),
        ))
        .mount(&server)
        .await;
    Mock::given(path("/thumb/x.jpg"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![1u8, 2, 3]))
        .mount(&server)
        .await;

    let client = start(&server, &[("YOUTUBE_MCP_DB", db.display().to_string())]).await?;
    ok(&call(&client, "get_videos", json!({"videos": [VID]})).await);
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    ok(&call(&client, "get_videos", json!({"videos": [VID]})).await);

    let h = ok(&call(&client, "video_history", json!({"video": VID})).await);
    let snaps = h["snapshots"].as_array().unwrap();
    assert_eq!(snaps.len(), 2);
    assert_eq!(snaps[1]["views"], 5_000);
    assert!(snaps[1]["vph_since_previous"].as_f64().unwrap() > 1_000_000.0);

    let c = ok(&call(&client, "packaging_changes", json!({})).await);
    assert_eq!(c["changes"][0]["change"], "title");
    assert_eq!(c["changes"][0]["title_after"], "New title");

    let t = ok(&call(&client, "track_channel", json!({"channels": [A]})).await);
    assert_eq!(t["tracked"][0]["videos_snapshotted"], 3);
    assert_eq!(t["tracked"][0]["thumbnails_fingerprinted"], 3);
    let ch = ok(&call(&client, "channel_history", json!({"channel": A})).await);
    assert_eq!(ch["snapshots"][0]["subscribers"], 1_234);
    client.cancel().await?;

    // the database outlives the process
    assert!(db.exists());
    let client = start(&server, &[("YOUTUBE_MCP_DB", db.display().to_string())]).await?;
    let h = ok(&call(&client, "video_history", json!({"video": VID})).await);
    assert_eq!(h["snapshots"].as_array().unwrap().len(), 2);
    client.cancel().await?;

    let client = start(&server, &[]).await?; // DB off
    assert!(
        err(&call(&client, "video_history", json!({"video": VID})).await).contains("switched off")
    );
    client.cancel().await?;
    Ok(())
}

// ---------- prompts ----------

#[tokio::test]
async fn prompts_point_at_the_data_tools() -> anyhow::Result<()> {
    let server = MockServer::start().await;
    let client = start(&server, &[]).await?;
    let args = json!({"topic": "sourdough for beginners", "keyword": "sourdough"})
        .as_object()
        .cloned()
        .unwrap();
    let p = client
        .get_prompt(GetPromptRequestParams::new("title_ideas").with_arguments(args))
        .await?;
    let body = serde_json::to_string(&p.messages)?;
    assert!(
        body.contains("title_score")
            && body.contains("keyword_research")
            && body.contains("sourdough for beginners")
    );
    let args = json!({"video": VID}).as_object().cloned().unwrap();
    let p = client
        .get_prompt(GetPromptRequestParams::new("comment_replies").with_arguments(args))
        .await?;
    assert!(serde_json::to_string(&p.messages)?.contains("untrusted"));
    client.cancel().await?;
    Ok(())
}
