use crate::{limits::LimitViolation, player::CancelReason};
pub const TEXT_NOT_FOUND: &str = "ничего не найдено";
pub const TEXT_SERVICE_DOWN: &str = "музыкальный сервис недоступен";
pub fn chat_added(user: &str, artist: &str, title: &str, pos: usize) -> String {
    format!("@{user} трек «{artist} — {title}» в очереди, позиция {pos}")
}
pub fn chat_rejected(user: &str, why: &str) -> String {
    format!("@{user} заказ отклонён: {why}. Баллы возвращены.")
}
pub fn violation_text(v: &LimitViolation) -> &'static str {
    match v {
        LimitViolation::TooLong => "слишком длинный трек",
        LimitViolation::Repeat => "этот трек недавно играл или уже в очереди",
        LimitViolation::UserQuota => "у тебя уже максимум заказов в очереди",
        LimitViolation::Explicit => "explicit запрещён",
        LimitViolation::Blocked => "трек в чёрном списке",
    }
}
pub fn cancel_text(v: &CancelReason) -> &'static str {
    match v {
        CancelReason::RemovedByStreamer => "стример удалил заказ",
        CancelReason::TooLong => "трек слишком длинный",
        CancelReason::YtUnavailable => "видео недоступно",
    }
}
