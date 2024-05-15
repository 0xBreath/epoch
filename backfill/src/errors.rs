use std::{io, string::FromUtf8Error};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BackfillError {
    #[error("Io: {0}")]
    Io(#[from] io::Error),

    #[error("SerdeYaml: {0}")]
    SerdeYaml(#[from] serde_yaml::Error),

    #[error("Utf8: {0}")]
    Utf8(#[from] FromUtf8Error),

    #[allow(dead_code)]
    #[error("BackfillStopped: {0}")]
    BackfillStopped(String),
}
