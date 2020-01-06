// error.rs
//
// Copyright (C) 2019-2020  Minnesota Department of Transportation
//
use muon_rs::Error as MuonError;
use std::fmt;
use std::num::{ParseIntError, TryFromIntError};

/// Streambed errors
#[derive(Debug)]
pub enum Error {
    /// Missing gstreamer element
    MissingElement(&'static str),
    /// Invalid gstreamer property
    InvalidProperty(&'static str),
    /// Error while connecting a glib signal
    ConnectSignal(&'static str),
    /// Error while adding an element to a pipeline
    PipelineAdd(),
    /// Invalid MatrixCrop definition
    InvalidCrop(),
    /// Error parsing integer
    ParseInt(ParseIntError),
    /// Error converting from integer
    TryFromInt(TryFromIntError),
    /// I/O error
    Io(std::io::Error),
    /// Muon error
    Muon(MuonError),
    /// Other error
    Other(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::MissingElement(e) => write!(f, "missing element: {}", e),
            Error::InvalidProperty(e) => write!(f, "invalid property: {}", e),
            Error::ConnectSignal(e) => write!(f, "connect signal: {}", e),
            Error::PipelineAdd() => write!(f, "pipeline add"),
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
