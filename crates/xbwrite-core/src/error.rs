use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    /// Callers that show this to the user prefix it with the file name themselves.
    #[error("{source}")]
    File {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("not an Xbox disc image: {0}")]
    NotXbox(String),

    #[error("unsupported or corrupt image: {0}")]
    Format(String),

    #[error("verification failed: {0}")]
    Verify(String),

    #[error("{0}")]
    Unsupported(String),

    #[error("cancelled")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

/// A file system error from xdvdfs, which only says what is wrong with the image.
pub(crate) fn format(e: impl std::fmt::Display) -> Error {
    Error::Format(e.to_string())
}

/// Attaches the path to an I/O error.
pub(crate) fn at(path: &std::path::Path) -> impl FnOnce(std::io::Error) -> Error + '_ {
    move |source| Error::File { path: path.to_path_buf(), source }
}

/// An error's message followed by the messages of its sources, e.g.
/// "failed to write to block device: There is not enough space on the disk."
pub(crate) fn chain(e: &dyn std::error::Error) -> String {
    let mut msg = e.to_string();
    let mut cur = e.source();
    while let Some(s) = cur {
        let next = s.to_string();
        if !msg.ends_with(&next) {
            msg = format!("{msg}: {next}");
        }
        cur = s.source();
    }
    msg
}
