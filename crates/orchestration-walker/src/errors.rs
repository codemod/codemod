//! Node-style error codes and messages for the napi addon, which exposes the
//! walker to the Node host (the QuickJS host formats errors like its own
//! `fs` primitives instead, see its `select.rs`).
//!
//! - [`io_code`]: the POSIX name Node puts in `error.code` (`ENOENT`, ...).
//! - [`node_message`]: the message Node's `fs` would throw for the same
//!   failure (`EACCES: permission denied, scandir '/x'`, libuv's
//!   `uv_strerror` wording), so the napi addon's native selection fails with
//!   exactly the text the TypeScript walker over the Node host does.

use std::io;

use crate::Error;

/// POSIX name of an I/O error, as Node reports it in `error.code`.
pub fn io_code(error: &io::Error) -> &'static str {
    if let Some(errno) = error.raw_os_error() {
        return match errno {
            libc::ENOENT => "ENOENT",
            libc::EEXIST => "EEXIST",
            libc::ENOTDIR => "ENOTDIR",
            libc::EISDIR => "EISDIR",
            libc::EACCES => "EACCES",
            libc::EPERM => "EPERM",
            libc::ENOTEMPTY => "ENOTEMPTY",
            libc::ELOOP => "ELOOP",
            libc::EINVAL => "EINVAL",
            libc::EBUSY => "EBUSY",
            libc::EMFILE => "EMFILE",
            libc::ENFILE => "ENFILE",
            libc::ENAMETOOLONG => "ENAMETOOLONG",
            libc::EROFS => "EROFS",
            libc::EXDEV => "EXDEV",
            libc::EIO => "EIO",
            libc::ENOMEM => "ENOMEM",
            libc::EAGAIN => "EAGAIN",
            libc::EILSEQ => "EILSEQ",
            _ => "EIO",
        };
    }
    match error.kind() {
        io::ErrorKind::NotFound => "ENOENT",
        io::ErrorKind::AlreadyExists => "EEXIST",
        io::ErrorKind::PermissionDenied => "EACCES",
        io::ErrorKind::NotADirectory => "ENOTDIR",
        io::ErrorKind::IsADirectory => "EISDIR",
        io::ErrorKind::DirectoryNotEmpty => "ENOTEMPTY",
        io::ErrorKind::InvalidInput => "EINVAL",
        _ => "EIO",
    }
}

/// libuv's `uv_strerror` text for the codes [`io_code`] produces.
fn uv_strerror(code: &str) -> Option<&'static str> {
    Some(match code {
        "ENOENT" => "no such file or directory",
        "EEXIST" => "file already exists",
        "ENOTDIR" => "not a directory",
        "EISDIR" => "illegal operation on a directory",
        "EACCES" => "permission denied",
        "EPERM" => "operation not permitted",
        "ENOTEMPTY" => "directory not empty",
        "ELOOP" => "too many symbolic links encountered",
        "EINVAL" => "invalid argument",
        "EBUSY" => "resource busy or locked",
        "EMFILE" => "too many open files",
        "ENFILE" => "file table overflow",
        "ENAMETOOLONG" => "name too long",
        "EROFS" => "read-only file system",
        "EXDEV" => "cross-device link not permitted",
        "EIO" => "i/o error",
        "ENOMEM" => "not enough memory",
        "EAGAIN" => "resource temporarily unavailable",
        "EILSEQ" => "illegal byte sequence",
        _ => return None,
    })
}

/// The `code` and message Node's `fs` throws for this failure; non-I/O
/// errors have no code and the walker's own message (which already is the
/// `ignore` package's text where it has one).
pub fn node_message(error: &Error) -> (Option<&'static str>, String) {
    match error {
        Error::Io { op, path, error } => {
            let code = io_code(error);
            let text = uv_strerror(code).map_or_else(|| error.to_string(), str::to_string);
            (Some(code), format!("{code}: {text}, {op} '{path}'"))
        }
        other => (None, other.to_string()),
    }
}
