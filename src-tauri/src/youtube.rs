use crate::model::{Source, Track};
use anyhow::Result;
use reqwest::Client;
use serde::Deserialize;

pub fn split_title(title: &str, channel: &str) -> (String, String) {
    if let Some((artist, song)) = title.split_once(" - ").or_else(|| title.split_once(" — ")) {
        return (artist.trim().into(), song.trim().into());
    }
    (
        channel.strip_suffix(" - Topic").unwrap_or(channel).into(),
        title.into(),
    )
}

pub async fn oembed(video_id: &str, base: &str) -> Result<Option<Track>> {
    #[derive(Deserialize)]
    struct Oembed {
        title: String,
        author_name: String,
        thumbnail_url: String,
    }
    let response = Client::new()
        .get(format!("{}/oembed", base.trim_end_matches('/')))
        .query(&[
            ("url", format!("https://www.youtube.com/watch?v={video_id}")),
            ("format", "json".into()),
        ])
        .send()
        .await?;
    if matches!(response.status().as_u16(), 401 | 403 | 404) {
        return Ok(None);
    }
    let data: Oembed = response.error_for_status()?.json().await?;
    let (artist, title) = split_title(&data.title, &data.author_name);
    Ok(Some(Track {
        source: Source::YouTube {
            video_id: video_id.into(),
        },
        title,
        artist,
        duration_ms: None,
        explicit: None,
        cover_url: Some(data.thumbnail_url),
    }))
}

pub async fn search(api_key: &str, q: &str, base: &str) -> Result<Option<String>> {
    #[derive(Deserialize)]
    struct Id {
        #[serde(rename = "videoId")]
        video_id: String,
    }
    #[derive(Deserialize)]
    struct Item {
        id: Id,
    }
    #[derive(Deserialize)]
    struct Search {
        items: Vec<Item>,
    }
    let data: Search = Client::new()
        .get(format!("{}/youtube/v3/search", base.trim_end_matches('/')))
        .query(&[
            ("part", "snippet"),
            ("type", "video"),
            ("videoEmbeddable", "true"),
            ("maxResults", "1"),
            ("q", q),
            ("key", api_key),
        ])
        .send()
        .await
        .map_err(|e| e.without_url())?
        .error_for_status()
        .map_err(|e| e.without_url())?
        .json()
        .await
        .map_err(|e| e.without_url())?;
    Ok(data.items.into_iter().next().map(|item| item.id.video_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Source, Track};
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path, query_param},
    };

    #[tokio::test]
    async fn oembed_404_none() {
        let server = MockServer::start().await;
        for status in [401, 403, 404] {
            server.reset().await;
            Mock::given(method("GET"))
                .and(path("/oembed"))
                .respond_with(ResponseTemplate::new(status))
                .mount(&server)
                .await;
            assert_eq!(oembed("abc", &server.uri()).await.unwrap(), None);
        }
    }

    #[tokio::test]
    async fn oembed_maps_fields() {
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/oembed"))
            .and(query_param("url", "https://www.youtube.com/watch?v=abc"))
            .and(query_param("format", "json"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "title": "Artist - Song", "author_name": "Channel", "thumbnail_url": "https://example.com/cover.jpg"
            }))).expect(1).mount(&server).await;
        assert_eq!(
            oembed("abc", &server.uri()).await.unwrap(),
            Some(Track {
                source: Source::YouTube {
                    video_id: "abc".into()
                },
                title: "Song".into(),
                artist: "Artist".into(),
                duration_ms: None,
                explicit: None,
                cover_url: Some("https://example.com/cover.jpg".into()),
            })
        );
    }

    #[test]
    fn split_title_dash() {
        for title in ["Artist - Song", "Artist — Song"] {
            assert_eq!(
                split_title(title, "Channel"),
                ("Artist".into(), "Song".into())
            );
        }
    }

    #[test]
    fn split_title_topic_channel() {
        assert_eq!(
            split_title("Song", "Artist - Topic"),
            ("Artist".into(), "Song".into())
        );
        assert_eq!(
            split_title("Song", "Channel"),
            ("Channel".into(), "Song".into())
        );
    }

    #[tokio::test]
    async fn search_returns_first_id() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/youtube/v3/search"))
            .and(query_param("part", "snippet"))
            .and(query_param("type", "video"))
            .and(query_param("videoEmbeddable", "true"))
            .and(query_param("maxResults", "1"))
            .and(query_param("q", "artist & song"))
            .and(query_param("key", "test-key"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "items": [{"id": {"videoId": "first"}}, {"id": {"videoId": "second"}}]
            })))
            .expect(1)
            .mount(&server)
            .await;
        assert_eq!(
            search("test-key", "artist & song", &server.uri())
                .await
                .unwrap(),
            Some("first".into())
        );
        server.verify().await;
        server.reset().await;
        Mock::given(path("/youtube/v3/search"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"items": []})),
            )
            .mount(&server)
            .await;
        assert_eq!(
            search("test-key", "empty", &server.uri()).await.unwrap(),
            None
        );
    }
}
