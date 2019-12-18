use std::fmt;
use std::num::{ParseIntError, TryFromIntError};

#[derive(Debug)]
pub enum Error {
    MissingElement(&'static str),
    InvalidProperty(&'static str),
    ConnectSignal(),
    PipelineAdd(),
    LinkElement(),
    InvalidCrop(),
    ParseInt(ParseIntError),
    TryFromInt(TryFromIntError),
    Other(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::MissingElement(e) => write!(f, "missing element: {}", e),
            Error::InvalidProperty(e) => write!(f, "invalid property: {}", e),
            Error::ConnectSignal() => write!(f, "connect signal"),
            Error::PipelineAdd() => write!(f, "pipeline add"),
            Error::LinkElement() => write!(f, "link elements"),
            Error::InvalidCrop() => write!(f, "invalid crop"),
            Error::ParseInt(e) => write!(f, "parse {:?}", e),
            Error::TryFromInt(e) => write!(f, "try_from {:?}", e),
            Error::Other(e) => write!(f, "{:?}", e),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::ParseInt(e) => Some(e),
            Error::TryFromInt(e) => Some(e),
            _ => None,
        }
    }
}

impl From<ParseIntError> for Error {
    fn from(e: ParseIntError) -> Self {
        Error::ParseInt(e)
    }
}

impl From<TryFromIntError> for Error {
    fn from(e: TryFromIntError) -> Self {
        Error::TryFromInt(e)
    }
}
