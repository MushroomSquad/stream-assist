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
