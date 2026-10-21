// SPDX-License-Identifier: GPL-3.0-only

//! What travels in a drag of a tray icon: its key, in a MIME type of our own.

use std::borrow::Cow;

use cosmic::iced::clipboard::mime::{AllowedMimeTypes, AsMimeTypes};

/// Only our own icons are accepted: drags from other programs carry other types.
pub const MIME: &str = "application/x-io.github.aenes0.cosmictray-item";

/// The key (see `identity`) of the dragged icon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayDrag(pub String);

impl AllowedMimeTypes for TrayDrag {
    fn allowed() -> Cow<'static, [String]> {
        Cow::Owned(vec![MIME.to_string()])
    }
}

impl AsMimeTypes for TrayDrag {
    fn available(&self) -> Cow<'static, [String]> {
        Cow::Owned(vec![MIME.to_string()])
    }

    fn as_bytes(&self, mime: &str) -> Option<Cow<'static, [u8]>> {
        (mime == MIME).then(|| Cow::Owned(self.0.as_bytes().to_vec()))
    }
}

impl TryFrom<(Vec<u8>, String)> for TrayDrag {
    type Error = &'static str;

    fn try_from((data, mime): (Vec<u8>, String)) -> Result<Self, Self::Error> {
        if mime != MIME {
            return Err("unexpected MIME type");
        }
        match String::from_utf8(data) {
            Ok(key) if !key.is_empty() => Ok(TrayDrag(key)),
            Ok(_) => Err("empty key"),
            Err(_) => Err("key is not UTF-8"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tray_drag_round_trip() {
        let d = TrayDrag("Σ".into());
        let bytes = d.as_bytes(MIME).unwrap().into_owned();
        assert_eq!(TrayDrag::try_from((bytes, MIME.to_string())), Ok(d));
    }

    #[test]
    fn tray_drag_rejects_other_mime() {
        assert!(TrayDrag::try_from((b"a".to_vec(), "text/uri-list".to_string())).is_err());
    }

    #[test]
    fn tray_drag_rejects_invalid_utf8_and_empty() {
        assert!(TrayDrag::try_from((vec![0xff], MIME.to_string())).is_err());
        assert!(TrayDrag::try_from((vec![], MIME.to_string())).is_err());
    }
}
