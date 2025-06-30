pub mod client;

pub use client::{PtpClient, PtpConfig, PtpState, PtpStats, PtpDomain};

pub type Result<T> = anyhow::Result<T>;