pub mod client;
pub mod messages;

pub use client::{PtpClient, PtpConfig, PtpState, PtpStats};
pub use messages::ClockIdentity;

pub type Result<T> = anyhow::Result<T>;
