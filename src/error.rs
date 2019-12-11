use glib::error::BoolError;
use std::fmt;
use std::num::ParseIntError;

#[derive(Debug)]
pub enum Error {
    InvalidCrop(),
    MissingElement(&'static str),
    Bool(BoolError),
    ParseInt(ParseIntError),
    Other(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::InvalidCrop() => write!(f, "invalid crop"),
            Error::MissingElement(e) => write!(f, "missing {:?}", e),
            Error::Bool(e) => write!(f, "glib {:?}", e),
            Error::ParseInt(e) => write!(f, "parse {:?}", e),
            Error::Other(e) => write!(f, "{:?}", e),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Bool(e) => Some(e),
            Error::ParseInt(e) => Some(e),
            _ => None,
        }
    }
}

impl From<BoolError> for Error {
    fn from(e: BoolError) -> Self {
        Error::Bool(e)
    }
}

impl From<ParseIntError> for Error {
    fn from(e: ParseIntError) -> Self {
        Error::ParseInt(e)
    }
}
