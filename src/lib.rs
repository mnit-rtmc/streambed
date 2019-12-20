mod error;
mod stream;

pub use crate::error::Error;
pub use crate::stream::{
    Acceleration, AspectRatio, Encoding, Feedback, Flow, FlowBuilder,
    MatrixCrop, Sink, Source,
};
