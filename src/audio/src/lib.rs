pub mod reader;

pub use reader::{AudioReader, AudioInfo, AudioSample};

pub type Result<T> = anyhow::Result<T>;