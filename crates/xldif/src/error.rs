use serde::Serialize;

/// Stable failure categories. The CLI exposes them as `error.code`; the
/// message is for people and may change between versions. The names follow
/// `dsd` so agents handle both tools alike; `UnsupportedFormat` is new here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// An input path does not exist or is not a regular file.
    FileNotFound,
    /// The file is not a ZIP-based workbook, or its container is unsafe,
    /// corrupt, or beyond the reader's bounds.
    InvalidArchive,
    /// The file is a workbook format the reader does not handle (`.xlsb`, `.xls`).
    UnsupportedFormat,
    /// A required workbook part is not well-formed XML or lacks required content.
    InvalidXml,
    /// An option value is out of range or malformed.
    InvalidArgument,
    /// The output path exists, or is an input, and replacing it was not authorized.
    OutputExists,
    /// Reading or writing the filesystem failed.
    IoError,
}

impl ErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::FileNotFound => "file_not_found",
            Self::InvalidArchive => "invalid_archive",
            Self::UnsupportedFormat => "unsupported_format",
            Self::InvalidXml => "invalid_xml",
            Self::InvalidArgument => "invalid_argument",
            Self::OutputExists => "output_exists",
            Self::IoError => "io_error",
        }
    }
}

/// A failure the caller can act on.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    pub code: ErrorCode,
    pub message: String,
    /// What to do next, when there is a specific recovery.
    pub hint: Option<String>,
}

impl Error {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), hint: None }
    }

    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::ErrorCode;

    #[test]
    fn codes_serialize_as_their_contract_names() {
        for code in [
            ErrorCode::FileNotFound,
            ErrorCode::InvalidArchive,
            ErrorCode::UnsupportedFormat,
            ErrorCode::InvalidXml,
            ErrorCode::InvalidArgument,
            ErrorCode::OutputExists,
            ErrorCode::IoError,
        ] {
            assert_eq!(serde_json::to_value(code).unwrap(), code.as_str());
        }
    }
}
