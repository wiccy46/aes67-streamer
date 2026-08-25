//! Pure routing state shared by future CLI, TUI, and desktop adapters.
//!
//! This module models source-to-stream assignment only. It deliberately does
//! not open sockets, start PTP, decode audio, or spawn stream workers. That
//! separation lets user interfaces edit and validate an atomic routing graph
//! before the runtime applies it to live audio.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::net::Ipv4Addr;

pub const MIN_STREAM_GAIN_DB: f32 = -120.0;
pub const MAX_STREAM_GAIN_DB: f32 = 0.0;

fn default_stream_gain_db() -> Option<f32> {
    Some(MAX_STREAM_GAIN_DB)
}

/// Opaque identifier for an audio source in a routing graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SourceId(u64);

impl SourceId {
    pub fn get_value(self) -> u64 {
        self.0
    }
}

/// Opaque identifier for an AES67 stream in a routing graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StreamId(u64);

impl StreamId {
    pub fn get_value(self) -> u64 {
        self.0
    }
}

/// The input an audio source will provide when a runtime is started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceInput {
    File { path: String },
    LiveInput { device: String },
}

/// User-configurable source definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceConfig {
    pub name: String,
    pub input: SourceInput,
}

/// User-configurable AES67 output stream definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StreamConfig {
    pub name: String,
    pub address: Ipv4Addr,
    pub port: u16,
    /// Per-stream output gain. `None` represents muted (`-infinity dB`).
    #[serde(default = "default_stream_gain_db")]
    pub gain_db: Option<f32>,
}

/// A source definition with its engine-owned identifier.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoutingSource {
    pub id: SourceId,
    pub config: SourceConfig,
}

/// A stream definition with its engine-owned identifier.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingStream {
    pub id: StreamId,
    pub config: StreamConfig,
}

/// One selected source for one stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RouteAssignment {
    pub source_id: SourceId,
    pub stream_id: StreamId,
}

/// Copyable, revisioned view of the routing graph.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RoutingSnapshot {
    pub revision: u64,
    pub sources: Vec<RoutingSource>,
    pub streams: Vec<RoutingStream>,
    pub routes: Vec<RouteAssignment>,
}

/// Failure to apply a routing-model change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingError {
    UnknownSource(SourceId),
    UnknownStream(StreamId),
    InvalidSource { message: String },
    InvalidStream { message: String },
}

impl fmt::Display for RoutingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSource(id) => {
                write!(formatter, "unknown routing source {}", id.get_value())
            }
            Self::UnknownStream(id) => {
                write!(formatter, "unknown routing stream {}", id.get_value())
            }
            Self::InvalidSource { message } => {
                write!(formatter, "invalid routing source: {message}")
            }
            Self::InvalidStream { message } => {
                write!(formatter, "invalid routing stream: {message}")
            }
        }
    }
}

impl Error for RoutingError {}

/// An in-memory routing graph with simple source fan-out semantics.
///
/// A stream has at most one selected source. Assigning a source to an already
/// routed stream atomically replaces the prior assignment; a source may be
/// assigned to any number of streams.
#[derive(Debug, Default)]
pub struct RoutingModel {
    revision: u64,
    next_source_id: u64,
    next_stream_id: u64,
    sources: BTreeMap<SourceId, SourceConfig>,
    streams: BTreeMap<StreamId, StreamConfig>,
    routes_by_stream: BTreeMap<StreamId, SourceId>,
}

impl RoutingModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_source(&mut self, config: SourceConfig) -> Result<SourceId, RoutingError> {
        validate_source_config(&config)?;
        let id = SourceId(self.get_next_source_id());
        self.sources.insert(id, config);
        self.revision += 1;
        Ok(id)
    }

    pub fn update_source(
        &mut self,
        id: SourceId,
        config: SourceConfig,
    ) -> Result<(), RoutingError> {
        validate_source_config(&config)?;
        let source = self
            .sources
            .get_mut(&id)
            .ok_or(RoutingError::UnknownSource(id))?;
        *source = config;
        self.revision += 1;
        Ok(())
    }

    pub fn remove_source(&mut self, id: SourceId) -> Result<(), RoutingError> {
        self.sources
            .remove(&id)
            .ok_or(RoutingError::UnknownSource(id))?;
        self.routes_by_stream
            .retain(|_, source_id| *source_id != id);
        self.revision += 1;
        Ok(())
    }

    pub fn create_stream(&mut self, config: StreamConfig) -> Result<StreamId, RoutingError> {
        validate_stream_config(&config)?;
        let id = StreamId(self.get_next_stream_id());
        self.streams.insert(id, config);
        self.revision += 1;
        Ok(id)
    }

    pub fn update_stream(
        &mut self,
        id: StreamId,
        config: StreamConfig,
    ) -> Result<(), RoutingError> {
        validate_stream_config(&config)?;
        let stream = self
            .streams
            .get_mut(&id)
            .ok_or(RoutingError::UnknownStream(id))?;
        *stream = config;
        self.revision += 1;
        Ok(())
    }

    pub fn remove_stream(&mut self, id: StreamId) -> Result<(), RoutingError> {
        self.streams
            .remove(&id)
            .ok_or(RoutingError::UnknownStream(id))?;
        self.routes_by_stream.remove(&id);
        self.revision += 1;
        Ok(())
    }

    /// Assign a source to a stream, replacing the stream's prior source if set.
    pub fn assign_source(
        &mut self,
        source_id: SourceId,
        stream_id: StreamId,
    ) -> Result<(), RoutingError> {
        if !self.sources.contains_key(&source_id) {
            return Err(RoutingError::UnknownSource(source_id));
        }
        if !self.streams.contains_key(&stream_id) {
            return Err(RoutingError::UnknownStream(stream_id));
        }

        self.routes_by_stream.insert(stream_id, source_id);
        self.revision += 1;
        Ok(())
    }

    /// Remove a stream's source assignment. Returns whether a route was present.
    pub fn remove_route(&mut self, stream_id: StreamId) -> Result<bool, RoutingError> {
        if !self.streams.contains_key(&stream_id) {
            return Err(RoutingError::UnknownStream(stream_id));
        }

        let route_removed = self.routes_by_stream.remove(&stream_id).is_some();
        if route_removed {
            self.revision += 1;
        }
        Ok(route_removed)
    }

    pub fn get_snapshot(&self) -> RoutingSnapshot {
        RoutingSnapshot {
            revision: self.revision,
            sources: self
                .sources
                .iter()
                .map(|(id, config)| RoutingSource {
                    id: *id,
                    config: config.clone(),
                })
                .collect(),
            streams: self
                .streams
                .iter()
                .map(|(id, config)| RoutingStream {
                    id: *id,
                    config: config.clone(),
                })
                .collect(),
            routes: self
                .routes_by_stream
                .iter()
                .map(|(stream_id, source_id)| RouteAssignment {
                    source_id: *source_id,
                    stream_id: *stream_id,
                })
                .collect(),
        }
    }

    fn get_next_source_id(&mut self) -> u64 {
        self.next_source_id += 1;
        self.next_source_id
    }

    fn get_next_stream_id(&mut self) -> u64 {
        self.next_stream_id += 1;
        self.next_stream_id
    }
}

fn validate_source_config(config: &SourceConfig) -> Result<(), RoutingError> {
    validate_name(&config.name, "source")?;
    match &config.input {
        SourceInput::File { path } if path.trim().is_empty() => Err(RoutingError::InvalidSource {
            message: "file path must not be empty".to_string(),
        }),
        SourceInput::LiveInput { device } if device.trim().is_empty() => {
            Err(RoutingError::InvalidSource {
                message: "live-input device must not be empty".to_string(),
            })
        }
        SourceInput::File { .. } | SourceInput::LiveInput { .. } => Ok(()),
    }
}

fn validate_stream_config(config: &StreamConfig) -> Result<(), RoutingError> {
    validate_name(&config.name, "stream")?;
    if config.port == 0 {
        return Err(RoutingError::InvalidStream {
            message: "RTP port must not be zero".to_string(),
        });
    }
    if let Some(gain_db) = config.gain_db {
        if !gain_db.is_finite() || !(MIN_STREAM_GAIN_DB..=MAX_STREAM_GAIN_DB).contains(&gain_db) {
            return Err(RoutingError::InvalidStream {
                message: format!(
                    "gain must be muted or between {MIN_STREAM_GAIN_DB} and {MAX_STREAM_GAIN_DB} dB"
                ),
            });
        }
    }
    Ok(())
}

fn validate_name(name: &str, kind: &str) -> Result<(), RoutingError> {
    if name.trim().is_empty() || name.contains(['\r', '\n']) {
        let message = if name.trim().is_empty() {
            "name must not be empty"
        } else {
            "name must not contain line breaks"
        };
        return match kind {
            "source" => Err(RoutingError::InvalidSource {
                message: message.to_string(),
            }),
            _ => Err(RoutingError::InvalidStream {
                message: message.to_string(),
            }),
        };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_source(name: &str) -> SourceConfig {
        SourceConfig {
            name: name.to_string(),
            input: SourceInput::File {
                path: format!("{name}.wav"),
            },
        }
    }

    fn stream(name: &str, last_octet: u8) -> StreamConfig {
        StreamConfig {
            name: name.to_string(),
            address: Ipv4Addr::new(239, 69, 83, last_octet),
            port: 5004,
            gain_db: Some(MAX_STREAM_GAIN_DB),
        }
    }

    #[test]
    fn source_can_fan_out_to_multiple_streams() {
        let mut model = RoutingModel::new();
        let source = model
            .create_source(file_source("Music bed"))
            .expect("source should be valid");
        let lobby = model
            .create_stream(stream("Lobby", 2))
            .expect("stream should be valid");
        let green_room = model
            .create_stream(stream("Green room", 3))
            .expect("stream should be valid");

        model
            .assign_source(source, lobby)
            .expect("lobby route should be valid");
        model
            .assign_source(source, green_room)
            .expect("green-room route should be valid");

        let snapshot = model.get_snapshot();
        assert_eq!(snapshot.routes.len(), 2);
        assert_eq!(
            snapshot.routes,
            vec![
                RouteAssignment {
                    source_id: source,
                    stream_id: lobby,
                },
                RouteAssignment {
                    source_id: source,
                    stream_id: green_room,
                },
            ]
        );
    }

    #[test]
    fn assigning_new_source_replaces_stream_input() {
        let mut model = RoutingModel::new();
        let music = model
            .create_source(file_source("Music bed"))
            .expect("source should be valid");
        let voiceover = model
            .create_source(file_source("Voiceover"))
            .expect("source should be valid");
        let green_room = model
            .create_stream(stream("Green room", 3))
            .expect("stream should be valid");

        model
            .assign_source(music, green_room)
            .expect("initial route should be valid");
        model
            .assign_source(voiceover, green_room)
            .expect("replacement route should be valid");

        let snapshot = model.get_snapshot();
        assert_eq!(snapshot.routes.len(), 1);
        assert_eq!(
            snapshot.routes[0],
            RouteAssignment {
                source_id: voiceover,
                stream_id: green_room,
            }
        );
    }

    #[test]
    fn removing_source_removes_its_dependent_routes() {
        let mut model = RoutingModel::new();
        let source = model
            .create_source(file_source("Music bed"))
            .expect("source should be valid");
        let stream = model
            .create_stream(stream("Lobby", 2))
            .expect("stream should be valid");
        model
            .assign_source(source, stream)
            .expect("route should be valid");

        model.remove_source(source).expect("source should exist");

        let snapshot = model.get_snapshot();
        assert!(snapshot.sources.is_empty());
        assert!(snapshot.routes.is_empty());
        assert_eq!(snapshot.streams.len(), 1);
    }

    #[test]
    fn invalid_changes_do_not_advance_snapshot_revision() {
        let mut model = RoutingModel::new();
        let revision = model.get_snapshot().revision;

        let result = model.create_source(SourceConfig {
            name: "  ".to_string(),
            input: SourceInput::File {
                path: "music.wav".to_string(),
            },
        });

        assert!(matches!(result, Err(RoutingError::InvalidSource { .. })));
        assert_eq!(model.get_snapshot().revision, revision);

        let result = model.create_stream(StreamConfig {
            gain_db: Some(0.1),
            ..stream("Program", 1)
        });

        assert!(matches!(result, Err(RoutingError::InvalidStream { .. })));
        assert_eq!(model.get_snapshot().revision, revision);
    }

    #[test]
    fn unknown_route_endpoints_are_rejected() {
        let mut model = RoutingModel::new();
        let source = model
            .create_source(file_source("Music bed"))
            .expect("source should be valid");

        let result = model.assign_source(source, StreamId(99));

        assert_eq!(result, Err(RoutingError::UnknownStream(StreamId(99))));
        assert!(model.get_snapshot().routes.is_empty());
    }

    #[test]
    fn remove_route_only_advances_revision_when_route_exists() {
        let mut model = RoutingModel::new();
        let stream = model
            .create_stream(stream("Lobby", 2))
            .expect("stream should be valid");
        let revision = model.get_snapshot().revision;

        assert!(!model.remove_route(stream).expect("stream should exist"));
        assert_eq!(model.get_snapshot().revision, revision);
    }
}
