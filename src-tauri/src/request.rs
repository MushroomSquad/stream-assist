use url::Url;

#[derive(Debug, PartialEq)]
pub enum RequestInput {
    SpotifyTrack(String),
    YouTube(String),
    Query(String),
    Empty,
}

pub fn parse_request(input: &str) -> RequestInput {
    let input = input.trim();
    if input.is_empty() {
        return RequestInput::Empty;
    }
    if let Some(id) = input
        .strip_prefix("spotify:track:")
        .filter(|id| !id.is_empty())
    {
        return RequestInput::SpotifyTrack(id.into());
    }
    if let Ok(url) = Url::parse(input) {
        let segments: Vec<_> = url.path_segments().into_iter().flatten().collect();
        match url.host_str() {
            Some("open.spotify.com") => {
                let path = if segments.first().is_some_and(|s| s.starts_with("intl-")) {
                    &segments[1..]
                } else {
                    &segments[..]
                };
                if let ["track", id] = path {
                    if !id.is_empty() {
                        return RequestInput::SpotifyTrack((*id).into());
                    }
                }
            }
            Some("youtu.be") => {
                if let [id] = segments.as_slice() {
                    if !id.is_empty() {
                        return RequestInput::YouTube((*id).into());
                    }
                }
            }
            Some("youtube.com" | "www.youtube.com" | "music.youtube.com") => {
                if url.path() == "/watch" {
                    if let Some((_, id)) = url
                        .query_pairs()
                        .find(|(key, value)| key == "v" && !value.is_empty())
                    {
                        return RequestInput::YouTube(id.into_owned());
                    }
                }
                if let ["shorts", id] = segments.as_slice() {
                    if !id.is_empty() {
                        return RequestInput::YouTube((*id).into());
                    }
                }
            }
            _ => {}
        }
    }
    RequestInput::Query(input.chars().take(200).collect())
}

#[cfg(test)]
mod tests {
    use super::{RequestInput::*, parse_request};
    #[test]
    fn request_parsing() {
        assert_eq!(
            parse_request("https://open.spotify.com/intl-ru/track/4uLU6hMCjMI75M1A2tKUQC?si=x"),
            SpotifyTrack("4uLU6hMCjMI75M1A2tKUQC".into())
        );
        assert_eq!(
            parse_request("spotify:track:4uLU6hMCjMI75M1A2tKUQC"),
            SpotifyTrack("4uLU6hMCjMI75M1A2tKUQC".into())
        );
        assert_eq!(
            parse_request("https://youtu.be/dQw4w9WgXcQ?t=10"),
            YouTube("dQw4w9WgXcQ".into())
        );
        assert_eq!(
            parse_request("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=x"),
            YouTube("dQw4w9WgXcQ".into())
        );
        assert_eq!(
            parse_request("https://youtube.com/shorts/dQw4w9WgXcQ"),
            YouTube("dQw4w9WgXcQ".into())
        );
        assert_eq!(
            parse_request("https://music.youtube.com/watch?v=dQw4w9WgXcQ"),
            YouTube("dQw4w9WgXcQ".into())
        );
        assert_eq!(parse_request("  "), Empty);
        assert_eq!(
            parse_request("Daft Punk 🎧 around the world"),
            Query("Daft Punk 🎧 around the world".into())
        );
        assert!(matches!(parse_request(&"я".repeat(500)), Query(s) if s.chars().count() == 200));
        assert_eq!(
            parse_request("https://open.spotify.com/album/xyz"),
            Query("https://open.spotify.com/album/xyz".into())
        );
    }
}
