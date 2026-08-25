//! Product workflows shared by the AES67 command-line, terminal, and desktop applications.
//!
//! The engine owns Send, Receive, and stream discovery workflows. Audio processing,
//! RTP/SAP transport, and PTP timing remain focused lower-level dependencies.

pub mod discovery;
pub mod receiver;
pub mod routing;
pub mod routing_runtime;
pub mod sender;
