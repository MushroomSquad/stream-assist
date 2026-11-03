use crate::model::Fragment;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tokio::time::Instant;

pub fn extract_marked(text: &str, marker: &str) -> Option<String> {
    if marker.is_empty() {
        return None;
    }
    let (pos, _) = text.char_indices().find(|&(pos, _)| {
        text[pos..].starts_with(marker) && !text[..pos].ends_with(|c: char| c.is_ascii_digit())
    })?;
    let prefix = text[..pos].trim();
    (!prefix.is_empty()).then(|| prefix.to_owned())
}

pub fn strip_emotes(fragments: &[Fragment]) -> String {
    fragments
        .iter()
        .filter(|f| f.kind != "emote")
        .map(|f| f.text.as_str())
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub enum Reject {
    Empty,
    StopWord,
}

pub fn clean(text: &str, strip_links: bool, stop_words: &[String]) -> Result<String, Reject> {
    let lower = text.to_lowercase();
    if stop_words.iter().any(|word| {
        let word = word.trim();
        !word.is_empty() && lower.contains(&word.to_lowercase())
    }) {
        return Err(Reject::StopWord);
    }

    let mut cleaned = String::new();
    let mut previous = None;
    let mut repeats = 0;
    for token in text.split_whitespace() {
        if strip_links && is_link(token) {
            continue;
        }
        if !cleaned.is_empty() {
            cleaned.push(' ');
        }
        for c in token.chars() {
            if previous == Some(c) {
                repeats += 1;
            } else {
                previous = Some(c);
                repeats = 1;
            }
            if repeats <= 3 {
                cleaned.push(c);
            }
        }
        previous = None;
    }
    if cleaned.is_empty() {
        return Err(Reject::Empty);
    }
    if let Some((end, next)) = cleaned.char_indices().nth(300) {
        cleaned.truncate(end);
        if next != ' ' {
            if let Some(space) = cleaned.rfind(' ') {
                cleaned.truncate(space);
            }
        }
    }
    Ok(cleaned)
}

fn is_link(token: &str) -> bool {
    if token.contains("://") || token.to_lowercase().starts_with("www.") {
        return true;
    }
    // ponytail: "привет.как" / "file.txt" also count as links; a TLD allow-list is the upgrade.
    let token = token.trim_matches(|c: char| !c.is_alphanumeric() && c != '/');
    let host = token.split('/').next().unwrap_or(token);
    let labels: Vec<_> = host.split('.').collect();
    labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty() && label.chars().all(|c| c.is_alphanumeric() || c == '-')
        })
        && labels.last().is_some_and(|label| {
            label.chars().count() >= 2 && label.chars().all(char::is_alphabetic)
        })
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Roles {
    pub everyone: bool,
    pub subscribers: bool,
    pub vips: bool,
    pub moderators: bool,
}

impl Default for Roles {
    fn default() -> Self {
        Self {
            everyone: true,
            subscribers: true,
            vips: true,
            moderators: true,
        }
    }
}

pub fn role_allowed(r: &Roles, badges: &[String]) -> bool {
    r.everyone
        || badges.iter().any(|badge| match badge.as_str() {
            "broadcaster" => true,
            "subscriber" | "founder" => r.subscribers,
            "vip" => r.vips,
            "moderator" | "lead_moderator" => r.moderators,
            _ => false,
        })
}

#[derive(Default)]
pub struct Gate {
    last_global: Option<Instant>,
    last_user: HashMap<String, Instant>,
}

impl Gate {
    pub fn allowed(&self, user: &str, global_s: u32, user_s: u32, now: Instant) -> bool {
        let elapsed = |last: Instant, seconds: u32| {
            now.duration_since(last) >= Duration::from_secs(u64::from(seconds))
        };
        (global_s == 0 || self.last_global.is_none_or(|last| elapsed(last, global_s)))
            && (user_s == 0
                || self
                    .last_user
                    .get(&user.to_lowercase())
                    .is_none_or(|&last| elapsed(last, user_s)))
    }

    pub fn mark(&mut self, user: &str, now: Instant, keep_s: u32) {
        let keep = Duration::from_secs(u64::from(keep_s));
        self.last_user
            .retain(|_, last| now.duration_since(*last) <= keep);
        self.last_user.insert(user.to_lowercase(), now);
        self.last_global = Some(now);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn marker_basic() {
        assert_eq!(
            extract_marked("привет всем % лол", "%"),
            Some("привет всем".into())
        );
        for text in ["без метки", "%", "  %x"] {
            assert_eq!(extract_marked(text, "%"), None);
        }
        assert_eq!(extract_marked("текст", ""), None);
    }

    #[test]
    fn marker_after_digit_ignored() {
        assert_eq!(extract_marked("на 100% согласен", "%"), None);
        assert_eq!(
            extract_marked("скидка 50% а потом привет %", "%"),
            Some("скидка 50% а потом привет".into())
        );
        assert_eq!(extract_marked("да １%", "%"), Some("да １".into()));
    }

    #[test]
    fn marker_multichar() {
        assert_eq!(
            extract_marked("привет !! хвост", "!!"),
            Some("привет".into())
        );
        assert_eq!(extract_marked("5!!!", "!!"), Some("5!".into()));
        assert_eq!(
            extract_marked("привет ※※ хвост", "※※"),
            Some("привет".into())
        );
    }

    #[test]
    fn emotes_stripped() {
        let fragments = [
            Fragment {
                kind: "text".into(),
                text: "привет ".into(),
            },
            Fragment {
                kind: "emote".into(),
                text: "Kappa".into(),
            },
            Fragment {
                kind: "mention".into(),
                text: "@user".into(),
            },
        ];
        assert_eq!(strip_emotes(&fragments), "привет @user");
        assert_eq!(strip_emotes(&[]), "");
    }

    #[test]
    fn links_removed() {
        let text = "https://x.y/z www.Site.com twitch.tv/abc пример.рф т.е. 3.14 ok.";
        assert_eq!(clean(text, true, &[]), Ok("т.е. 3.14 ok.".into()));
        assert_eq!(
            clean("смотри site.com! (x.ru)", true, &[]),
            Ok("смотри".into())
        );
        assert_eq!(
            clean("WWW.site.com ПРИМЕР.РФ a-b.example/path", true, &[]),
            Err(Reject::Empty)
        );
        assert_eq!(
            clean("twitch.tv/abc", false, &[]),
            Ok("twitch.tv/abc".into())
        );
        assert_eq!(
            clean(
                " 
	 ",
                true,
                &[]
            ),
            Err(Reject::Empty)
        );
    }

    #[test]
    fn repeats_collapsed() {
        assert_eq!(clean("ааааааа!!!!!", false, &[]), Ok("ааа!!!".into()));
        assert_eq!(
            clean(
                " а  б
в	г ",
                false,
                &[]
            ),
            Ok("а б в г".into())
        );
    }

    #[test]
    fn stop_word_rejects() {
        let words = vec!["  ".into(), " ПЛОХО ".into()];
        assert_eq!(clean("Это неплохо", true, &words), Err(Reject::StopWord));
        assert_eq!(
            clean("https://плохо.рф", true, &words),
            Err(Reject::StopWord)
        );
        assert_eq!(clean("хорошо", true, &words), Ok("хорошо".into()));
    }

    #[test]
    fn truncate_on_word() {
        let prefix = "аб".repeat(145);
        assert_eq!(
            clean(&format!("{prefix} длинноеслово"), false, &[]),
            Ok(prefix)
        );
        let long = "аб".repeat(160);
        assert_eq!(clean(&long, false, &[]), Ok("аб".repeat(150)));
        let exact = format!("{} слово", "аб".repeat(147));
        assert_eq!(exact.chars().count(), 300);
        assert_eq!(clean(&exact, false, &[]), Ok(exact));
        let boundary = format!("а {}", "бв".repeat(149));
        assert_eq!(
            clean(&format!("{boundary} хвост"), false, &[]),
            Ok(boundary)
        );
    }

    #[test]
    fn roles() {
        let none = Roles {
            everyone: false,
            subscribers: false,
            vips: false,
            moderators: false,
        };
        assert!(!role_allowed(&none, &[]));
        assert!(role_allowed(&none, &["broadcaster".into()]));
        assert!(role_allowed(&Roles::default(), &[]));
        assert_eq!(
            Roles::default(),
            Roles {
                everyone: true,
                subscribers: true,
                vips: true,
                moderators: true
            }
        );
        for badge in [
            "subscriber",
            "founder",
            "vip",
            "moderator",
            "lead_moderator",
        ] {
            assert!(!role_allowed(&none, &[badge.into()]));
            let mut roles = none.clone();
            match badge {
                "subscriber" | "founder" => roles.subscribers = true,
                "vip" => roles.vips = true,
                _ => roles.moderators = true,
            }
            assert!(role_allowed(&roles, &[badge.into()]));
            assert!(!role_allowed(&roles, &["unknown".into()]));
        }
    }

    #[test]
    fn gate_cooldowns() {
        let mut gate = Gate::default();
        let now = Instant::now();
        assert!(gate.allowed("Alice", 10, 60, now));
        assert!(gate.allowed("Alice", 10, 60, now));
        gate.mark("Alice", now, 60);
        assert!(!gate.allowed("bob", 10, 60, now + Duration::from_secs(9)));
        assert!(gate.allowed("bob", 10, 60, now + Duration::from_secs(10)));
        assert!(!gate.allowed("ALICE", 10, 60, now + Duration::from_secs(59)));
        assert!(gate.allowed("alice", 10, 60, now + Duration::from_secs(60)));
        assert!(gate.allowed("alice", 0, 0, now));
        assert!(!gate.allowed("alice", 0, 60, now));
        assert!(!gate.allowed("bob", 10, 0, now));
        gate.mark("bob", now + Duration::from_secs(61), 60);
        assert!(!gate.last_user.contains_key("alice"));
        assert!(gate.last_user.contains_key("bob"));
        gate.mark("BOB", now + Duration::from_secs(62), 60);
        assert_eq!(gate.last_user.len(), 1);
    }
}
