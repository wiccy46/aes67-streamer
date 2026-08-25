use aes67_engine::routing::{
    RoutingError, RoutingModel, RoutingSnapshot, SourceConfig, SourceId, SourceInput, StreamConfig,
    StreamId,
};
use aes67_engine::routing_runtime::{
    preview_stream_sdp, RoutingRuntime, RoutingRuntimeConfig, RoutingRuntimeLifecycle,
    RoutingRuntimeSnapshot,
};
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;

struct DesktopState {
    routing: Mutex<RoutingModel>,
    runtime: RoutingRuntime,
}

impl DesktopState {
    fn new() -> Self {
        Self {
            routing: Mutex::new(initial_routing_model()),
            runtime: RoutingRuntime::new(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopInfo {
    product_name: &'static str,
    version: &'static str,
    live_routing_available: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum SourceInputKind {
    File,
    LiveInput,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SourceRequest {
    name: String,
    input_kind: SourceInputKind,
    location: String,
}

impl SourceRequest {
    fn into_config(self) -> SourceConfig {
        let input = match self.input_kind {
            SourceInputKind::File => SourceInput::File {
                path: self.location,
            },
            SourceInputKind::LiveInput => SourceInput::LiveInput {
                device: self.location,
            },
        };
        SourceConfig {
            name: self.name,
            input,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StreamRequest {
    name: String,
    address: String,
    port: u16,
    #[serde(default = "default_stream_gain_db")]
    gain_db: Option<f32>,
}

fn default_stream_gain_db() -> Option<f32> {
    Some(0.0)
}

impl StreamRequest {
    fn into_config(self) -> Result<StreamConfig, String> {
        let address = self
            .address
            .parse::<Ipv4Addr>()
            .map_err(|error| format!("invalid stream address: {error}"))?;
        Ok(StreamConfig {
            name: self.name,
            address,
            port: self.port,
            gain_db: self.gain_db,
        })
    }
}

#[tauri::command]
fn get_desktop_info() -> DesktopInfo {
    DesktopInfo {
        product_name: "aes67",
        version: env!("AES67_TOOLS_VERSION"),
        live_routing_available: true,
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeRequest {
    interface: String,
    #[serde(default)]
    ptp_domain: u8,
}

impl RuntimeRequest {
    fn into_config(self) -> RoutingRuntimeConfig {
        RoutingRuntimeConfig {
            interface: self.interface,
            ptp_domain: self.ptp_domain,
            sap: true,
        }
    }
}

#[tauri::command]
fn get_routing_snapshot(state: State<'_, DesktopState>) -> Result<RoutingSnapshot, String> {
    let routing = state
        .routing
        .lock()
        .map_err(|_| "routing state lock was poisoned".to_string())?;
    Ok(routing.get_snapshot())
}

#[tauri::command]
fn create_source(
    request: SourceRequest,
    state: State<'_, DesktopState>,
) -> Result<RoutingSnapshot, String> {
    with_routing(&state, |routing| {
        routing.create_source(request.into_config())?;
        Ok(routing.get_snapshot())
    })
}

#[tauri::command]
fn update_source(
    source_id: SourceId,
    request: SourceRequest,
    state: State<'_, DesktopState>,
) -> Result<RoutingSnapshot, String> {
    with_routing(&state, |routing| {
        routing.update_source(source_id, request.into_config())?;
        Ok(routing.get_snapshot())
    })
}

#[tauri::command]
fn remove_source(
    source_id: SourceId,
    state: State<'_, DesktopState>,
) -> Result<RoutingSnapshot, String> {
    with_routing(&state, |routing| {
        routing.remove_source(source_id)?;
        Ok(routing.get_snapshot())
    })
}

#[tauri::command]
fn create_stream(
    request: StreamRequest,
    state: State<'_, DesktopState>,
) -> Result<RoutingSnapshot, String> {
    let config = request.into_config()?;
    with_routing(&state, |routing| {
        routing.create_stream(config)?;
        Ok(routing.get_snapshot())
    })
}

#[tauri::command]
fn update_stream(
    stream_id: StreamId,
    request: StreamRequest,
    state: State<'_, DesktopState>,
) -> Result<RoutingSnapshot, String> {
    let config = request.into_config()?;
    with_routing(&state, |routing| {
        routing.update_stream(stream_id, config)?;
        Ok(routing.get_snapshot())
    })
}

#[tauri::command]
fn remove_stream(
    stream_id: StreamId,
    state: State<'_, DesktopState>,
) -> Result<RoutingSnapshot, String> {
    with_routing(&state, |routing| {
        routing.remove_stream(stream_id)?;
        Ok(routing.get_snapshot())
    })
}

#[tauri::command]
fn assign_source(
    source_id: SourceId,
    stream_id: StreamId,
    state: State<'_, DesktopState>,
) -> Result<RoutingSnapshot, String> {
    with_routing(&state, |routing| {
        routing.assign_source(source_id, stream_id)?;
        Ok(routing.get_snapshot())
    })
}

#[tauri::command]
fn remove_route(
    stream_id: StreamId,
    state: State<'_, DesktopState>,
) -> Result<RoutingSnapshot, String> {
    with_routing(&state, |routing| {
        routing.remove_route(stream_id)?;
        Ok(routing.get_snapshot())
    })
}

#[tauri::command]
fn get_runtime_snapshot(state: State<'_, DesktopState>) -> RoutingRuntimeSnapshot {
    state.runtime.get_snapshot()
}

#[tauri::command]
async fn start_all(
    request: RuntimeRequest,
    state: State<'_, DesktopState>,
) -> Result<RoutingRuntimeSnapshot, String> {
    let routing = {
        let routing = state
            .routing
            .lock()
            .map_err(|_| "routing state lock was poisoned".to_string())?;
        routing.get_snapshot()
    };
    state
        .runtime
        .start(routing, request.into_config())
        .await
        .map_err(|error| format!("{error:#}"))
}

#[tauri::command]
async fn stop_all(state: State<'_, DesktopState>) -> Result<RoutingRuntimeSnapshot, String> {
    Ok(state.runtime.stop().await)
}

#[tauri::command]
fn get_stream_sdp(
    stream_id: StreamId,
    request: RuntimeRequest,
    state: State<'_, DesktopState>,
) -> Result<String, String> {
    if let Some(sdp) = state.runtime.get_stream_sdp(stream_id) {
        return Ok(sdp);
    }
    let routing = state
        .routing
        .lock()
        .map_err(|_| "routing state lock was poisoned".to_string())?
        .get_snapshot();
    preview_stream_sdp(&routing, stream_id, &request.into_config())
        .map_err(|error| format!("{error:#}"))
}

fn with_routing<T>(
    state: &State<'_, DesktopState>,
    operation: impl FnOnce(&mut RoutingModel) -> Result<T, RoutingError>,
) -> Result<T, String> {
    if matches!(
        state.runtime.get_snapshot().lifecycle,
        RoutingRuntimeLifecycle::Starting | RoutingRuntimeLifecycle::Running
    ) {
        return Err("stop all streams before editing the routing graph".to_string());
    }
    let mut routing = state
        .routing
        .lock()
        .map_err(|_| "routing state lock was poisoned".to_string())?;
    operation(&mut routing).map_err(|error| error.to_string())
}

fn initial_routing_model() -> RoutingModel {
    let mut routing = RoutingModel::new();

    #[cfg(debug_assertions)]
    {
        let demo_audio = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../tests/piano_freesound.wav")
            .to_string_lossy()
            .into_owned();
        let studio = routing
            .create_source(SourceConfig {
                name: "Studio A".to_string(),
                input: SourceInput::File {
                    path: demo_audio.clone(),
                },
            })
            .expect("valid demo source");
        let music = routing
            .create_source(SourceConfig {
                name: "Music bed".to_string(),
                input: SourceInput::File {
                    path: demo_audio.clone(),
                },
            })
            .expect("valid demo source");
        routing
            .create_source(SourceConfig {
                name: "Voiceover".to_string(),
                input: SourceInput::File { path: demo_audio },
            })
            .expect("valid demo source");

        let program = routing
            .create_stream(StreamConfig {
                name: "Program".to_string(),
                address: Ipv4Addr::new(239, 69, 83, 1),
                port: 5004,
                gain_db: Some(0.0),
            })
            .expect("valid demo stream");
        let lobby = routing
            .create_stream(StreamConfig {
                name: "Lobby".to_string(),
                address: Ipv4Addr::new(239, 69, 83, 2),
                port: 5004,
                gain_db: Some(-12.0),
            })
            .expect("valid demo stream");
        let green_room = routing
            .create_stream(StreamConfig {
                name: "Green room".to_string(),
                address: Ipv4Addr::new(239, 69, 83, 3),
                port: 5004,
                gain_db: Some(-6.0),
            })
            .expect("valid demo stream");

        routing
            .assign_source(studio, program)
            .expect("valid demo route");
        routing
            .assign_source(music, lobby)
            .expect("valid demo route");
        routing
            .assign_source(music, green_room)
            .expect("valid demo route");
    }

    routing
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let builder = tauri::Builder::default().plugin(tauri_plugin_dialog::init());
    #[cfg(feature = "e2e")]
    let builder = builder
        .plugin(tauri_plugin_wdio::init())
        .plugin(tauri_plugin_wdio_webdriver::init());

    builder
        .manage(DesktopState::new())
        .invoke_handler(tauri::generate_handler![
            get_desktop_info,
            get_routing_snapshot,
            create_source,
            update_source,
            remove_source,
            create_stream,
            update_stream,
            remove_stream,
            assign_source,
            remove_route,
            get_runtime_snapshot,
            start_all,
            stop_all,
            get_stream_sdp,
        ])
        .run(tauri::generate_context!())
        .expect("error while running aes67 desktop application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_model_matches_reviewed_desktop_workspace() {
        let snapshot = initial_routing_model().get_snapshot();

        assert_eq!(snapshot.sources.len(), 3);
        assert_eq!(snapshot.streams.len(), 3);
        assert_eq!(snapshot.routes.len(), 3);
    }

    #[test]
    fn source_request_maps_to_engine_config() {
        let config = SourceRequest {
            name: "Music".to_string(),
            input_kind: SourceInputKind::File,
            location: "music.wav".to_string(),
        }
        .into_config();

        assert_eq!(config.name, "Music");
        assert!(matches!(
            config.input,
            SourceInput::File { path } if path == "music.wav"
        ));
    }

    #[test]
    fn stream_request_rejects_non_ipv4_address() {
        let result = StreamRequest {
            name: "Program".to_string(),
            address: "not-an-address".to_string(),
            port: 5004,
            gain_db: Some(0.0),
        }
        .into_config();

        assert!(result.is_err());
    }
}
