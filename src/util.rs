use arboard::Clipboard;
use base64::Engine;

use crate::error::{AppError, AppResult};

pub fn to_base64_str(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn from_base64_str(s: &str) -> Result<Vec<u8>, String> {
    base64::engine::general_purpose::STANDARD
        .decode(s)
        .map_err(|e| e.to_string())
}

pub fn copy_to_clipboard(text: &str) -> AppResult<()> {
    Clipboard::new()
        .and_then(|mut c| c.set_text(text))
        .map_err(|e| AppError::new("failed to copy to clipboard", e))
}
