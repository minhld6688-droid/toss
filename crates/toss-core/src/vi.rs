//! Vietnamese texts for errors that reach the UI (nearby-v0.8.md §1.3:
//! "every error string that can reach the UI is Vietnamese").
//!
//! Low-level code keeps English `anyhow` contexts for the logs; the places
//! where an error becomes a `TransferEvent::Error` message or a `Node` API
//! error turn it into one short Vietnamese sentence here. A message that is
//! already meant for the user is wrapped in [`UiMsg`] (see [`ui`]) and passes
//! through unchanged.

use std::fmt;

use crate::xfer::SessErr;

/// An error message written for the user (Vietnamese).
#[derive(Debug, Clone)]
pub(crate) struct UiMsg(pub String);

impl fmt::Display for UiMsg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UiMsg {}

/// An `anyhow::Error` carrying a user-facing message.
pub(crate) fn ui(msg: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(UiMsg(msg.into()))
}

/// The user-facing message somewhere in `e`'s chain, if any.
pub(crate) fn ui_text(e: &anyhow::Error) -> Option<String> {
    e.chain()
        .find_map(|c| c.downcast_ref::<UiMsg>())
        .map(|m| m.0.clone())
}

/// A file system error as one Vietnamese sentence (with the path when the
/// outermost context names one).
pub(crate) fn io_msg(e: &anyhow::Error) -> String {
    if let Some(m) = ui_text(e) {
        return m;
    }
    let kind = e
        .chain()
        .find_map(|c| c.downcast_ref::<std::io::Error>())
        .map(|io| io.kind());
    use std::io::ErrorKind as K;
    let what = match kind {
        Some(K::StorageFull) => "Ổ đĩa đã đầy",
        Some(K::PermissionDenied) => "Không có quyền đọc/ghi file",
        Some(K::NotFound) => "Không tìm thấy file hoặc thư mục",
        Some(K::ReadOnlyFilesystem) => "Ổ đĩa chỉ cho đọc, không ghi được",
        Some(K::AlreadyExists) => "File đã tồn tại",
        _ => "Lỗi đọc/ghi file",
    };
    let top = e.to_string();
    let path = top.split_once(' ').and_then(|(verb, rest)| {
        matches!(
            verb,
            "open"
                | "create"
                | "read"
                | "write"
                | "seek"
                | "rename"
                | "remove"
                | "truncate"
                | "sync"
                | "stat"
        )
        .then(|| rest.to_string())
    });
    match path {
        Some(p) => format!("{what} ({p})"),
        None => what.to_string(),
    }
}

/// Message of a failed transfer session.
pub(crate) fn sess_msg(e: &SessErr) -> String {
    match e {
        SessErr::Denied(r) => r.to_string(),
        SessErr::Offline { connected, err } => ui_text(err).unwrap_or_else(|| {
            if *connected {
                "Mất kết nối với máy kia".into()
            } else {
                "Không kết nối được tới máy kia".into()
            }
        }),
        SessErr::Io(e) => io_msg(e),
        SessErr::Other(e) => ui_text(e).unwrap_or_else(|| PROTOCOL.into()),
    }
}

/// Generic protocol failure.
pub(crate) const PROTOCOL: &str =
    "Máy kia trả lời không đúng — hãy cập nhật Toss ở cả hai máy rồi thử lại";
/// A Toss < 0.8 got a v0.8 request it cannot decode.
pub(crate) const OLD_PEER: &str =
    "Máy kia đang dùng Toss bản cũ — cập nhật Toss trên máy đó (bản 0.8 trở lên) rồi thử lại";
/// The receiver could not verify some files (Done.ok = false).
pub(crate) const DONE_FAILED: &str =
    "Một số file bị lỗi khi truyền — bấm Tiếp tục hoặc gửi lại để thử lại";
/// User cancelled.
pub(crate) const CANCELLED: &str = "Đã huỷ";

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    #[test]
    fn messages() {
        let e = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::StorageFull))
            .context("write /x/a.part");
        assert_eq!(io_msg(&e), "Ổ đĩa đã đầy (/x/a.part)");
        let e = anyhow::Error::from(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert_eq!(io_msg(&e), "Không có quyền đọc/ghi file");
        let e = ui("Máy kia bận").context("connect");
        assert_eq!(ui_text(&e).as_deref(), Some("Máy kia bận"));
        assert_eq!(
            sess_msg(&SessErr::Offline {
                connected: false,
                err: anyhow::anyhow!("timed out")
            }),
            "Không kết nối được tới máy kia"
        );
        assert_eq!(sess_msg(&SessErr::Other(anyhow::anyhow!("x"))), PROTOCOL);
        let r: Result<(), _> = Err(std::io::Error::from(std::io::ErrorKind::NotFound));
        let e = r.context("open /a").unwrap_err();
        assert_eq!(io_msg(&e), "Không tìm thấy file hoặc thư mục (/a)");
    }
}
