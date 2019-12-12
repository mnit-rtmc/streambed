mod error;
mod stream;

pub use crate::error::Error;
pub use crate::stream::{
    AspectRatio, Encoding, MatrixCrop, Sink, Stream, StreamBuilder,
    StreamControl,
};
