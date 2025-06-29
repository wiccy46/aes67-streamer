pub mod reader;
pub mod node;
pub mod gain;

pub use reader::{AudioReader, AudioInfo, AudioSample};
pub use node::{AudioNode, AudioNodeChain, ChainableNode, BaseAudioNode};
pub use gain::{GainNode, apply_gain_example};

pub type Result<T> = anyhow::Result<T>;