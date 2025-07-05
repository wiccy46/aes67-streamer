pub mod gain_node;
pub mod node;
pub mod reader;
pub mod utils;

pub use gain_node::{apply_gain_example, GainNode};
pub use node::{AudioNode, AudioNodeChain, BaseAudioNode};
pub use reader::{AudioInfo, AudioReader, AudioSample, ResamplerQuality};
pub use utils::{
    channels_to_flat_noninterleaved, flat_noninterleaved_to_channels,
    interleaved_to_noninterleaved, noninterleaved_to_interleaved,
};

pub type Result<T> = anyhow::Result<T>;
