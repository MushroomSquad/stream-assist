pub mod mixer;
pub mod models;
pub mod store;
pub mod style;

#[derive(Debug, thiserror::Error)]
pub enum VlError {
    #[error("{0}")]
    Code(&'static str),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl VlError {
    pub fn code(&self) -> String {
        match self {
            Self::Code(code) => (*code).to_string(),
            Self::Other(_) => "IO_FAILED".to_string(),
        }
    }
}

impl From<std::io::Error> for VlError {
    fn from(error: std::io::Error) -> Self {
        Self::Other(error.into())
    }
}

impl From<rusqlite::Error> for VlError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Other(error.into())
    }
}

impl From<serde_json::Error> for VlError {
    fn from(error: serde_json::Error) -> Self {
        Self::Other(error.into())
    }
}

pub fn err(code: &'static str) -> VlError {
    VlError::Code(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes() {
        assert_eq!(err("STYLE_BAD_SCHEMA").code(), "STYLE_BAD_SCHEMA");
        assert_eq!(
            VlError::from(anyhow::anyhow!("internal detail")).code(),
            "IO_FAILED"
        );
        assert_eq!(
            VlError::from(std::io::Error::other("internal detail")).code(),
            "IO_FAILED"
        );
        assert_eq!(
            VlError::from(rusqlite::Error::InvalidQuery).code(),
            "IO_FAILED"
        );
        let json_error = serde_json::from_str::<serde_json::Value>("!").unwrap_err();
        assert_eq!(VlError::from(json_error).code(), "IO_FAILED");
    }
}
