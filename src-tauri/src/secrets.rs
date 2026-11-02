use anyhow::Result;
const SERVICE: &str = "stream-assist";
pub fn store(key: &str, value: &str) -> Result<()> {
    keyring::Entry::new(SERVICE, key)?.set_password(value)?;
    Ok(())
}
pub fn load(key: &str) -> Result<Option<String>> {
    match keyring::Entry::new(SERVICE, key)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.into()),
    }
}
pub fn delete(key: &str) -> Result<()> {
    match keyring::Entry::new(SERVICE, key)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

// FIFO submission is synchronous; slow blocking writes cannot overtake logout.
type Write = (
    String,
    Option<String>,
    tokio::sync::oneshot::Sender<Result<()>>,
);
fn writer() -> &'static std::sync::mpsc::Sender<Write> {
    static WRITER: std::sync::OnceLock<std::sync::mpsc::Sender<Write>> = std::sync::OnceLock::new();
    WRITER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Write>();
        std::thread::spawn(move || {
            for (key, value, done) in rx {
                let result = match value {
                    Some(value) => store(&key, &value),
                    None => delete(&key),
                };
                let _ = done.send(result);
            }
        });
        tx
    })
}
fn generations() -> &'static std::sync::Mutex<std::collections::HashMap<String, u64>> {
    static GENERATIONS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, u64>>,
    > = std::sync::OnceLock::new();
    GENERATIONS.get_or_init(Default::default)
}
pub fn generation(key: &str) -> u64 {
    *generations()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .entry(key.into())
        .or_default()
}
fn submit(key: &str, value: Option<String>) -> tokio::sync::oneshot::Receiver<Result<()>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let _ = writer().send((key.to_owned(), value, tx));
    rx
}
pub fn write(key: &str, value: Option<String>) -> tokio::sync::oneshot::Receiver<Result<()>> {
    let mut epochs = generations().lock().unwrap_or_else(|e| e.into_inner());
    if value.is_none() {
        *epochs.entry(key.into()).or_default() += 1;
    }
    submit(key, value)
}
pub fn write_if_current(
    key: &str,
    epoch: u64,
    value: String,
) -> Option<tokio::sync::oneshot::Receiver<Result<()>>> {
    let mut epochs = generations().lock().unwrap_or_else(|e| e.into_inner());
    if *epochs.entry(key.into()).or_default() != epoch {
        return None;
    }
    Some(submit(key, Some(value)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn logout_rejects_late_refresh_save() {
        let key = "test-logout-generation";
        let epoch = generation(key);
        // Invalidate the session without accessing the machine's keyring.
        *generations().lock().unwrap().entry(key.into()).or_default() += 1;
        assert!(write_if_current(key, epoch, "late-token".into()).is_none());
    }
}
