use muon_rs::Error as MuonError;
use std::fmt;
use std::num::{ParseIntError, TryFromIntError};

#[derive(Debug)]
pub enum Error {
    MissingElement(&'static str),
    InvalidProperty(&'static str),
    ConnectSignal(&'static str),
    PipelineAdd(),
    LinkElement(),
    InvalidCrop(),
    ParseInt(ParseIntError),
    TryFromInt(TryFromIntError),
    Io(std::io::Error),
    Muon(MuonError),
    Other(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::MissingElement(e) => write!(f, "missing element: {}", e),
            Error::InvalidProperty(e) => write!(f, "invalid property: {}", e),
            Error::ConnectSignal(e) => write!(f, "connect signal: {}", e),
            Error::PipelineAdd() => write!(f, "pipeline add"),
            Error::LinkElement() => write!(f, "link elements"),
            Error::InvalidCrop() => write!(f, "invalid crop"),
            Error::ParseInt(e) => write!(f, "parse {:?}", e),
            Error::TryFromInt(e) => write!(f, "try_from {:?}", e),
            Error::Io(e) => write!(f, "IO {:?}", e),
            Error::Muon(e) => write!(f, "muon {:?}", e),
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

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<MuonError> for Error {
    fn from(e: MuonError) -> Self {
        Error::Muon(e)
    }
}
