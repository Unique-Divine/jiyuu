use thiserror::Error;

use std::io;

#[derive(Debug, Error)]
pub enum BashError {
    #[error("bash command failed with error: {}", err)]
    BashCmdFailed { err: String },

    #[error("BashError: {msg}")]
    General { msg: String },

    #[error("IO error: {0}")]
    IO(#[from] io::Error),
}

#[derive(Debug, Error)]
pub enum LocalError {
    #[error("config error: {}", err)]
    Std { err: String },

    #[error("failed to find $HOME directory")]
    FailedToFindHomeDir,

    #[error("failed to create ~/.local/nibiru_dev directory: {}", err)]
    FailedToCreateRootDir { err: &'static str },

    #[error("inner local error: {}", err)]
    InnerError { err: anyhow::Error },
}
