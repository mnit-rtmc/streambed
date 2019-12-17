mod error;
mod stream;

pub use crate::error::Error;
pub use crate::stream::{
    Acceleration, AspectRatio, Encoding, Feedback, MatrixCrop, Sink, Source,
    Stream, StreamBuilder,
};
