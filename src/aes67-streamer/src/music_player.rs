use anyhow::{anyhow, Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use network::{list_ipv4_interfaces, NetworkInterface};
use ratatui::backend::{Backend, CrosstermBackend, TestBackend};
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use streamer_core::{Aes67Streamer, StreamConfig};
use tokio::sync::mpsc::{self, UnboundedReceiver};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const CONFIG_DIR_ENV: &str = "AES67_MUSIC_PLAYER_CONFIG_DIR";
const SETTINGS_FILE: &str = "music-player.toml";
const ACCENT_COLOR: Color = Color::Rgb(214, 132, 58);
const COMPLETION_LIST_LIMIT: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct MusicPlayerSettings {
    stream: StreamSettings,
    playlist: PlaylistSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StreamSettings {
    address: String,
    port: u16,
    interface: Option<String>,
    session_name: String,
    sap: bool,
    ptp_domain: u8,
    payload_type: u8,
    packet_time_ms: u32,
    ttl: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
struct PlaylistSettings {
    files: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AppScreen {
    Player,
    Settings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsField {
    SessionName,
    Address,
    Port,
    Interface,
    Sap,
    PtpDomain,
    PayloadType,
    PacketTimeMs,
    Ttl,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SettingEdit {
    field: SettingsField,
    value: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct PathInput {
    value: String,
    completions: Vec<String>,
    completion_index: usize,
    show_completions: bool,
    browse_completions: bool,
}

impl PathInput {
    fn clear_completions(&mut self) {
        self.completions.clear();
        self.completion_index = 0;
        self.show_completions = false;
        self.browse_completions = false;
    }

    fn should_restart_as_browse_completion(&self) -> bool {
        !self.browse_completions
            && self.completions.len() == 1
            && is_browse_completion_input(&self.value)
            && self
                .completions
                .get(self.completion_index)
                .is_some_and(|completion| completion == &self.value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PickerKind {
    Interface,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PickerValue {
    InterfaceName(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PickerOption {
    label: String,
    value: PickerValue,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SettingsPicker {
    kind: PickerKind,
    title: String,
    options: Vec<PickerOption>,
    selected: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PlaybackState {
    Stopped,
    Starting { track_index: usize },
    Streaming { track_index: usize },
    Stopping { track_index: usize },
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlaybackStartRequest {
    track_index: usize,
    path: String,
    stream: StreamSettings,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PlaybackCommand {
    Start(PlaybackStartRequest),
    Stop,
}

#[derive(Debug, Clone, PartialEq)]
struct PlaybackMeter {
    elapsed: Duration,
    playhead: Duration,
    duration: Option<Duration>,
    progress_ratio: f64,
    target_packets: u64,
    target_packet_rate: u64,
}

#[derive(Debug)]
enum PlaybackRuntimeEvent {
    Started {
        track_index: usize,
        duration: Option<Duration>,
    },
}

#[derive(Debug, Clone)]
struct MusicPlayerApp {
    settings: MusicPlayerSettings,
    settings_path: PathBuf,
    interface_options: Vec<NetworkInterface>,
    screen: AppScreen,
    settings_required: bool,
    settings_focus: SettingsField,
    queue_selected: usize,
    path_input: Option<PathInput>,
    edit: Option<SettingEdit>,
    picker: Option<SettingsPicker>,
    playback_state: PlaybackState,
    playback_started_at: Option<Instant>,
    playback_duration: Option<Duration>,
    pending_playback_command: Option<PlaybackCommand>,
    status: String,
    should_quit: bool,
}

struct ActiveMusicStream {
    track_index: usize,
    shutdown: CancellationToken,
    events: UnboundedReceiver<PlaybackRuntimeEvent>,
    handle: JoinHandle<Result<()>>,
}

impl Default for MusicPlayerSettings {
    fn default() -> Self {
        Self {
            stream: StreamSettings {
                address: String::new(),
                port: 5004,
                interface: None,
                session_name: "AES67 Music Player".to_string(),
                sap: true,
                ptp_domain: 0,
                payload_type: 97,
                packet_time_ms: 1,
                ttl: 32,
            },
            playlist: PlaylistSettings::default(),
        }
    }
}

impl SettingsField {
    const ALL: [Self; 9] = [
        Self::SessionName,
        Self::Address,
        Self::Port,
        Self::Interface,
        Self::Sap,
        Self::PtpDomain,
        Self::PayloadType,
        Self::PacketTimeMs,
        Self::Ttl,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::SessionName => "Session name",
            Self::Address => "Stream address",
            Self::Port => "Port",
            Self::Interface => "Interface",
            Self::Sap => "SAP announcements",
            Self::PtpDomain => "PTP domain",
            Self::PayloadType => "Payload type",
            Self::PacketTimeMs => "Packet time",
            Self::Ttl => "TTL",
        }
    }

    fn value(self, stream: &StreamSettings) -> String {
        match self {
            Self::SessionName => stream.session_name.clone(),
            Self::Address => stream.address.clone(),
            Self::Port => stream.port.to_string(),
            Self::Interface => stream.interface.clone().unwrap_or_default(),
            Self::Sap => {
                if stream.sap {
                    "enabled".to_string()
                } else {
                    "disabled".to_string()
                }
            }
            Self::PtpDomain => stream.ptp_domain.to_string(),
            Self::PayloadType => stream.payload_type.to_string(),
            Self::PacketTimeMs => format!("{} ms", stream.packet_time_ms),
            Self::Ttl => stream.ttl.to_string(),
        }
    }

    fn edit_value(self, stream: &StreamSettings) -> String {
        match self {
            Self::PacketTimeMs => stream.packet_time_ms.to_string(),
            _ => self.value(stream),
        }
    }

    fn hint(self) -> &'static str {
        match self {
            Self::SessionName => "Shown in SDP/SAP.",
            Self::Address => "Multicast destination for RTP audio.",
            Self::Port => "RTP UDP port.",
            Self::Interface => "Enter chooses a local interface; e allows manual name/IP entry.",
            Self::Sap => "Toggle SAP stream discovery announcements.",
            Self::PtpDomain => "PTP domain advertised with the stream.",
            Self::PayloadType => "Dynamic RTP payload type, 96-127.",
            Self::PacketTimeMs => "AES67 packet time in milliseconds.",
            Self::Ttl => "Multicast time-to-live.",
        }
    }

    fn next(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|field| *field == self)
            .expect("settings field should be listed");
        Self::ALL[(index + 1) % Self::ALL.len()]
    }

    fn previous(self) -> Self {
        let index = Self::ALL
            .iter()
            .position(|field| *field == self)
            .expect("settings field should be listed");
        Self::ALL[(index + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

impl SettingsPicker {
    fn new_interface(options: Vec<PickerOption>, selected: usize) -> Self {
        let selected = selected.min(options.len().saturating_sub(1));
        Self {
            kind: PickerKind::Interface,
            title: "Select Interface".to_string(),
            options,
            selected,
        }
    }

    fn selected_option(&self) -> Option<&PickerOption> {
        self.options.get(self.selected)
    }

    fn move_next(&mut self) {
        if !self.options.is_empty() {
            self.selected = (self.selected + 1) % self.options.len();
        }
    }

    fn move_previous(&mut self) {
        if !self.options.is_empty() {
            self.selected = (self.selected + self.options.len() - 1) % self.options.len();
        }
    }
}

impl MusicPlayerApp {
    fn new(settings: MusicPlayerSettings, settings_path: PathBuf, settings_created: bool) -> Self {
        let interface_options = list_ipv4_interfaces().unwrap_or_default();
        Self::new_with_interfaces(settings, settings_path, settings_created, interface_options)
    }

    fn new_with_interfaces(
        settings: MusicPlayerSettings,
        settings_path: PathBuf,
        settings_created: bool,
        interface_options: Vec<NetworkInterface>,
    ) -> Self {
        let missing_required_settings = settings.stream.address.trim().is_empty()
            || settings
                .stream
                .interface
                .as_deref()
                .is_none_or(str::is_empty);
        let settings_required = settings_created || missing_required_settings;
        Self {
            settings,
            settings_path,
            interface_options,
            screen: if settings_required {
                AppScreen::Settings
            } else {
                AppScreen::Player
            },
            settings_required,
            settings_focus: SettingsField::SessionName,
            queue_selected: 0,
            path_input: None,
            edit: None,
            picker: None,
            playback_state: PlaybackState::Stopped,
            playback_started_at: None,
            playback_duration: None,
            pending_playback_command: None,
            status: if settings_created {
                "First launch: set stream address and interface, then press s.".to_string()
            } else if missing_required_settings {
                "Complete required stream settings, then press s.".to_string()
            } else {
                "Ready".to_string()
            },
            should_quit: false,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.should_quit = true;
            return Ok(());
        }

        if self.path_input.is_some() {
            return self.handle_path_input_key(key);
        }

        if self.picker.is_some() {
            return self.handle_picker_key(key);
        }

        match self.screen {
            AppScreen::Player => self.handle_player_key(key),
            AppScreen::Settings => self.handle_settings_key(key),
        }
    }

    fn handle_picker_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.picker = None;
                self.status = "Selection canceled".to_string();
            }
            KeyCode::Down | KeyCode::Tab => {
                if let Some(picker) = &mut self.picker {
                    picker.move_next();
                }
            }
            KeyCode::Up | KeyCode::BackTab => {
                if let Some(picker) = &mut self.picker {
                    picker.move_previous();
                }
            }
            KeyCode::Char('r') => self.refresh_picker()?,
            KeyCode::Enter => self.apply_picker_selection()?,
            _ => {}
        }
        Ok(())
    }

    fn handle_player_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Char('s') | KeyCode::Char('c') => self.open_settings(false),
            KeyCode::Char('a') => self.open_path_input(),
            KeyCode::Char('d') | KeyCode::Delete => self.remove_selected_queue_item()?,
            KeyCode::Down | KeyCode::Char('j') => self.move_queue_selection_next(),
            KeyCode::Up | KeyCode::Char('k') => self.move_queue_selection_previous(),
            KeyCode::Char(' ') => self.toggle_playback(),
            _ => {}
        }
        Ok(())
    }

    fn toggle_playback(&mut self) {
        match self.playback_state {
            PlaybackState::Stopped | PlaybackState::Error { .. } => self.request_stream_start(),
            PlaybackState::Starting { track_index } | PlaybackState::Streaming { track_index } => {
                self.request_stream_stop(track_index)
            }
            PlaybackState::Stopping { .. } => {
                self.status = "Stopping current stream...".to_string();
            }
        }
    }

    fn request_stream_start(&mut self) {
        if let Err(error) = self.validate_settings() {
            self.status = error.to_string();
            return;
        }

        if self.settings.playlist.files.is_empty() {
            self.status = "Queue is empty".to_string();
            return;
        }

        let track_index = self
            .queue_selected
            .min(self.settings.playlist.files.len().saturating_sub(1));
        let Some(path) = self.settings.playlist.files.get(track_index).cloned() else {
            self.status = "Queue is empty".to_string();
            return;
        };

        self.queue_selected = track_index;
        self.playback_started_at = None;
        self.playback_duration = None;
        self.playback_state = PlaybackState::Starting { track_index };
        self.pending_playback_command = Some(PlaybackCommand::Start(PlaybackStartRequest {
            track_index,
            path: path.clone(),
            stream: self.settings.stream.clone(),
        }));
        self.status = format!("Starting {}", display_path_name(&path));
    }

    fn request_stream_stop(&mut self, track_index: usize) {
        self.playback_state = PlaybackState::Stopping { track_index };
        self.pending_playback_command = Some(PlaybackCommand::Stop);
        self.status = "Stopping current stream...".to_string();
    }

    fn take_playback_command(&mut self) -> Option<PlaybackCommand> {
        self.pending_playback_command.take()
    }

    #[cfg(test)]
    fn mark_stream_started(&mut self, track_index: usize, started_at: Instant) {
        self.mark_stream_started_with_duration(track_index, started_at, None);
    }

    fn mark_stream_started_with_duration(
        &mut self,
        track_index: usize,
        started_at: Instant,
        duration: Option<Duration>,
    ) {
        self.playback_started_at = Some(started_at);
        self.playback_duration = duration;
        self.playback_state = PlaybackState::Streaming { track_index };
        self.queue_selected = track_index.min(self.settings.playlist.files.len().saturating_sub(1));
        self.status = format!(
            "Streaming {}",
            self.settings
                .playlist
                .files
                .get(track_index)
                .map(|path| display_path_name(path))
                .unwrap_or_else(|| "track".to_string())
        );
    }

    fn mark_stream_finished(&mut self, track_index: usize, result: Result<()>) {
        self.playback_started_at = None;
        self.playback_duration = None;

        if let Err(error) = result {
            self.playback_state = PlaybackState::Error {
                message: error.to_string(),
            };
            self.status = format!("Streaming failed: {error}");
            return;
        }

        if matches!(self.playback_state, PlaybackState::Stopping { .. }) {
            self.playback_state = PlaybackState::Stopped;
            self.status = "Stopped".to_string();
            return;
        }

        let next_index = track_index + 1;
        if next_index < self.settings.playlist.files.len() {
            self.queue_selected = next_index;
            self.request_stream_start();
        } else {
            self.playback_state = PlaybackState::Stopped;
            self.queue_selected =
                track_index.min(self.settings.playlist.files.len().saturating_sub(1));
            self.status = "Queue finished".to_string();
        }
    }

    fn playback_meter_at(&self, now: Instant) -> PlaybackMeter {
        let elapsed = self
            .playback_started_at
            .and_then(|started_at| now.checked_duration_since(started_at))
            .unwrap_or(Duration::ZERO);
        let packet_time_ms = u64::from(self.settings.stream.packet_time_ms.max(1));
        let playhead = if let Some(duration) = self.playback_duration {
            elapsed.min(duration)
        } else {
            elapsed
        };
        let progress_ratio = self
            .playback_duration
            .filter(|duration| !duration.is_zero())
            .map(|duration| playhead.as_secs_f64() / duration.as_secs_f64())
            .unwrap_or(0.0)
            .clamp(0.0, 1.0);
        PlaybackMeter {
            elapsed,
            playhead,
            duration: self.playback_duration,
            progress_ratio,
            target_packets: elapsed.as_millis() as u64 / packet_time_ms,
            target_packet_rate: 1_000 / packet_time_ms,
        }
    }

    fn playback_state_label(&self) -> &'static str {
        match self.playback_state {
            PlaybackState::Stopped => "stopped",
            PlaybackState::Starting { .. } => "starting",
            PlaybackState::Streaming { .. } => "streaming",
            PlaybackState::Stopping { .. } => "stopping",
            PlaybackState::Error { .. } => "error",
        }
    }

    fn playback_state_style(&self) -> Style {
        match self.playback_state {
            PlaybackState::Stopped => Style::default().fg(Color::Yellow),
            PlaybackState::Starting { .. } => Style::default().fg(ACCENT_COLOR),
            PlaybackState::Streaming { .. } => Style::default().fg(Color::Green),
            PlaybackState::Stopping { .. } => Style::default().fg(Color::Yellow),
            PlaybackState::Error { .. } => Style::default().fg(Color::Red),
        }
    }

    fn playback_action_label(&self) -> &'static str {
        match self.playback_state {
            PlaybackState::Stopped | PlaybackState::Error { .. } => "play",
            PlaybackState::Starting { .. }
            | PlaybackState::Streaming { .. }
            | PlaybackState::Stopping { .. } => "stop",
        }
    }

    fn active_track_index(&self) -> Option<usize> {
        match self.playback_state {
            PlaybackState::Starting { track_index }
            | PlaybackState::Streaming { track_index }
            | PlaybackState::Stopping { track_index } => Some(track_index),
            PlaybackState::Stopped | PlaybackState::Error { .. } => None,
        }
    }

    fn now_playing_track_name(&self) -> String {
        let Some(index) = self.active_track_index() else {
            return "No track playing".to_string();
        };

        self.settings
            .playlist
            .files
            .get(index)
            .map(|path| display_path_name(path))
            .unwrap_or_else(|| "Unknown track".to_string())
    }

    fn selected_track_name(&self) -> Option<String> {
        self.settings
            .playlist
            .files
            .get(self.queue_selected)
            .map(|path| display_path_name(path))
    }

    fn handle_path_input_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.path_input = None;
                self.status = "Add canceled".to_string();
            }
            KeyCode::Enter => {
                if self.select_visible_path_completion() {
                    return Ok(());
                }

                let input = self
                    .path_input
                    .take()
                    .expect("path input state should exist");
                self.add_playlist_path(&input.value)?;
            }
            KeyCode::Tab => self.complete_path_input()?,
            KeyCode::Down => self.move_path_completion_next(),
            KeyCode::Up => self.move_path_completion_previous(),
            KeyCode::Backspace => {
                if let Some(input) = &mut self.path_input {
                    input.value.pop();
                    input.clear_completions();
                }
            }
            KeyCode::Char(ch) => {
                if let Some(input) = &mut self.path_input {
                    input.value.push(ch);
                    input.clear_completions();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn select_visible_path_completion(&mut self) -> bool {
        let Some(input) = &mut self.path_input else {
            return false;
        };

        if !input.show_completions || input.completions.is_empty() {
            return false;
        }

        let selected = input.completions[input.completion_index].clone();
        let label = completion_option_label(&selected);
        input.value = selected;
        input.clear_completions();
        self.status = format!("Selected {label}");
        true
    }

    fn move_path_completion_next(&mut self) {
        self.move_visible_path_completion(1);
    }

    fn move_path_completion_previous(&mut self) {
        self.move_visible_path_completion(-1);
    }

    fn move_visible_path_completion(&mut self, delta: isize) {
        let Some(input) = &mut self.path_input else {
            return;
        };

        if !input.show_completions || input.completions.is_empty() {
            return;
        }

        let len = input.completions.len() as isize;
        input.completion_index = (input.completion_index as isize + delta).rem_euclid(len) as usize;
        let label = completion_option_label(&input.completions[input.completion_index]);
        self.status = format!("Selected {label}");
    }

    fn handle_settings_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.edit.is_some() {
            return self.handle_edit_key(key);
        }

        match key.code {
            KeyCode::Esc => self.close_settings_or_warn(),
            KeyCode::Char('q') => {
                if self.settings_required {
                    self.should_quit = true;
                } else {
                    self.screen = AppScreen::Player;
                    self.status = "Settings unchanged".to_string();
                }
            }
            KeyCode::Char('s') => self.save_and_close_settings()?,
            KeyCode::Char('e') => self.start_text_edit(),
            KeyCode::Enter => self.start_edit_or_toggle(),
            KeyCode::Down | KeyCode::Tab => self.settings_focus = self.settings_focus.next(),
            KeyCode::Up | KeyCode::BackTab => {
                self.settings_focus = self.settings_focus.previous();
            }
            KeyCode::Char(' ') if self.settings_focus == SettingsField::Sap => {
                self.toggle_sap();
            }
            _ => {}
        }

        Ok(())
    }

    fn handle_edit_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.edit = None;
                self.status = "Edit canceled".to_string();
            }
            KeyCode::Enter => {
                let edit = self.edit.take().expect("edit state should exist");
                self.apply_field_value(edit.field, &edit.value)?;
                self.status = format!("Updated {}", edit.field.label());
            }
            KeyCode::Backspace => {
                if let Some(edit) = &mut self.edit {
                    edit.value.pop();
                }
            }
            KeyCode::Char(ch) => {
                if let Some(edit) = &mut self.edit {
                    edit.value.push(ch);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn open_settings(&mut self, required: bool) {
        self.screen = AppScreen::Settings;
        self.settings_required = self.settings_required || required;
        self.settings_focus = SettingsField::SessionName;
        self.edit = None;
        self.picker = None;
        self.status = "Review stream settings.".to_string();
    }

    fn open_path_input(&mut self) {
        self.path_input = Some(PathInput::default());
        self.status = "Enter a music file or folder path.".to_string();
    }

    fn complete_path_input(&mut self) -> Result<()> {
        let Some(input) = &mut self.path_input else {
            return Ok(());
        };

        if input.should_restart_as_browse_completion() {
            input.clear_completions();
        }

        if input.completions.is_empty() {
            input.browse_completions = is_browse_completion_input(&input.value);
            input.completions = path_completions(&input.value)?;
            input.completion_index = 0;
            input.show_completions = false;
        } else if input.browse_completions && !input.show_completions {
            input.show_completions = true;
        } else {
            input.completion_index = (input.completion_index + 1) % input.completions.len();
            input.show_completions = input.browse_completions || input.completions.len() > 1;
        }

        if input.completions.is_empty() {
            input.show_completions = false;
            self.status = "No matching files or folders".to_string();
            return Ok(());
        }

        if input.browse_completions {
            let count = input.completions.len();
            self.status = if input.show_completions {
                format!("Showing {count} matches, press Tab to cycle")
            } else {
                format!("{count} matches, press Tab again to show options")
            };
            return Ok(());
        }

        input.value = input.completions[input.completion_index].clone();
        let count = input.completions.len();
        self.status = if count == 1 {
            "Completed path".to_string()
        } else if input.show_completions {
            format!("Showing {count} matches, press Tab to cycle")
        } else {
            format!("{count} matches, press Tab again to show options")
        };
        Ok(())
    }

    fn close_settings_or_warn(&mut self) {
        if self.settings_required {
            self.status = "Save settings to continue, or press q to quit.".to_string();
        } else {
            self.screen = AppScreen::Player;
            self.status = "Settings unchanged".to_string();
        }
    }

    fn save_and_close_settings(&mut self) -> Result<()> {
        if let Err(error) = self.validate_settings() {
            self.status = error.to_string();
            return Ok(());
        }

        save_settings(&self.settings_path, &self.settings)?;
        self.settings_required = false;
        self.screen = AppScreen::Player;
        self.edit = None;
        self.picker = None;
        self.status = "Settings saved".to_string();
        Ok(())
    }

    fn validate_settings(&self) -> Result<()> {
        if self.settings.stream.address.trim().is_empty() {
            return Err(anyhow!("Stream address is required"));
        }

        if self
            .settings
            .stream
            .interface
            .as_deref()
            .is_none_or(str::is_empty)
        {
            return Err(anyhow!("Interface is required"));
        }

        Ok(())
    }

    fn start_edit_or_toggle(&mut self) {
        if self.settings_focus == SettingsField::Sap {
            self.toggle_sap();
            return;
        }

        if self.settings_focus == SettingsField::Interface {
            self.open_interface_picker();
            return;
        }

        self.start_text_edit();
    }

    fn start_text_edit(&mut self) {
        self.edit = Some(SettingEdit {
            field: self.settings_focus,
            value: self.settings_focus.edit_value(&self.settings.stream),
        });
        self.status = format!("Editing {}", self.settings_focus.label());
    }

    fn open_interface_picker(&mut self) {
        let options = self.interface_picker_options();
        let selected = self.selected_interface_option(&options);
        self.picker = Some(SettingsPicker::new_interface(options, selected));
        self.status = "Choose a network interface.".to_string();
    }

    fn interface_picker_options(&self) -> Vec<PickerOption> {
        self.interface_options
            .iter()
            .map(|interface| {
                let loopback = if interface.is_loopback {
                    " loopback"
                } else {
                    ""
                };
                PickerOption {
                    label: format!("{}  {}{}", interface.name, interface.ipv4, loopback),
                    value: PickerValue::InterfaceName(interface.name.clone()),
                }
            })
            .collect()
    }

    fn selected_interface_option(&self, options: &[PickerOption]) -> usize {
        let Some(interface) = self.settings.stream.interface.as_deref() else {
            return 0;
        };

        options
            .iter()
            .position(|option| match &option.value {
                PickerValue::InterfaceName(name) => {
                    name == interface
                        || self.interface_options.iter().any(|candidate| {
                            candidate.name == *name && candidate.ipv4.to_string() == interface
                        })
                }
            })
            .unwrap_or(0)
    }

    fn refresh_picker(&mut self) -> Result<()> {
        let Some(kind) = self.picker.as_ref().map(|picker| picker.kind) else {
            return Ok(());
        };

        match kind {
            PickerKind::Interface => {
                self.interface_options = list_ipv4_interfaces()?;
                self.open_interface_picker();
                self.status = "Interface list refreshed".to_string();
            }
        }

        Ok(())
    }

    fn apply_picker_selection(&mut self) -> Result<()> {
        let Some(picker) = self.picker.take() else {
            return Ok(());
        };

        match picker.kind {
            PickerKind::Interface => {
                let Some(option) = picker.selected_option() else {
                    self.status = "No interface option selected".to_string();
                    return Ok(());
                };

                match &option.value {
                    PickerValue::InterfaceName(name) => {
                        self.settings.stream.interface = Some(name.clone());
                        self.status = format!("Selected interface {name}");
                    }
                }
            }
        }

        Ok(())
    }

    fn toggle_sap(&mut self) {
        self.settings.stream.sap = !self.settings.stream.sap;
        self.status = if self.settings.stream.sap {
            "SAP announcements enabled".to_string()
        } else {
            "SAP announcements disabled".to_string()
        };
    }

    fn add_playlist_path(&mut self, value: &str) -> Result<()> {
        let value = value.trim();
        if value.is_empty() {
            self.status = "Enter a file or folder path".to_string();
            return Ok(());
        }

        let path = PathBuf::from(value);
        let expanded_path = expand_user_path(&path);
        if !expanded_path.exists() {
            self.status = "Path does not exist".to_string();
            return Ok(());
        }

        if expanded_path.is_file() && !is_supported_audio_file(&expanded_path) {
            self.status = "Unsupported audio file type".to_string();
            return Ok(());
        }

        let files = collect_audio_files(&path)?;
        if files.is_empty() {
            self.status = if expanded_path.is_dir() {
                "No supported audio files found in folder".to_string()
            } else {
                "No supported audio files found".to_string()
            };
            return Ok(());
        }

        let was_empty = self.settings.playlist.files.is_empty();
        let count = files.len();
        self.settings.playlist.files.extend(
            files
                .into_iter()
                .map(|path| path.to_string_lossy().to_string()),
        );
        if was_empty {
            self.queue_selected = 0;
        }

        save_settings(&self.settings_path, &self.settings)?;
        self.status = if count == 1 {
            "Added 1 track".to_string()
        } else {
            format!("Added {count} tracks")
        };
        Ok(())
    }

    fn move_queue_selection_next(&mut self) {
        let len = self.settings.playlist.files.len();
        if len > 0 {
            self.queue_selected = (self.queue_selected + 1) % len;
        }
    }

    fn move_queue_selection_previous(&mut self) {
        let len = self.settings.playlist.files.len();
        if len > 0 {
            self.queue_selected = (self.queue_selected + len - 1) % len;
        }
    }

    fn remove_selected_queue_item(&mut self) -> Result<()> {
        if self.settings.playlist.files.is_empty() {
            self.status = "Queue is empty".to_string();
            return Ok(());
        }

        let index = self
            .queue_selected
            .min(self.settings.playlist.files.len().saturating_sub(1));
        let removed = self.settings.playlist.files.remove(index);
        self.queue_selected = self
            .queue_selected
            .min(self.settings.playlist.files.len().saturating_sub(1));
        save_settings(&self.settings_path, &self.settings)?;
        self.status = format!("Removed {}", display_path_name(&removed));
        Ok(())
    }

    fn apply_field_value(&mut self, field: SettingsField, value: &str) -> Result<()> {
        let value = value.trim();
        match field {
            SettingsField::SessionName => {
                if value.is_empty() {
                    return Err(anyhow!("session name cannot be empty"));
                }
                self.settings.stream.session_name = value.to_string();
            }
            SettingsField::Address => {
                if value.is_empty() {
                    return Err(anyhow!("stream address cannot be empty"));
                }
                self.settings.stream.address = value.to_string();
            }
            SettingsField::Port => {
                let port = parse_u16(value, "port")?;
                if port == 0 {
                    return Err(anyhow!("port must be greater than zero"));
                }
                self.settings.stream.port = port;
            }
            SettingsField::Interface => {
                self.settings.stream.interface =
                    if value.is_empty() || matches!(value, "none" | "clear" | "-") {
                        None
                    } else {
                        Some(value.to_string())
                    };
            }
            SettingsField::Sap => self.settings.stream.sap = parse_bool(value, "SAP")?,
            SettingsField::PtpDomain => {
                self.settings.stream.ptp_domain = parse_u8(value, "PTP domain")?;
            }
            SettingsField::PayloadType => {
                let payload_type = parse_u8(value, "payload type")?;
                if !(96..=127).contains(&payload_type) {
                    return Err(anyhow!("payload type must be between 96 and 127"));
                }
                self.settings.stream.payload_type = payload_type;
            }
            SettingsField::PacketTimeMs => {
                self.settings.stream.packet_time_ms = parse_positive_u32(value, "packet time")?;
            }
            SettingsField::Ttl => {
                let ttl = parse_u8(value, "TTL")?;
                if ttl == 0 {
                    return Err(anyhow!("TTL must be greater than zero"));
                }
                self.settings.stream.ttl = ttl;
            }
        }
        Ok(())
    }

    fn setting_display_value(&self, field: SettingsField) -> String {
        if field == SettingsField::Address && self.settings.stream.address.trim().is_empty() {
            return "Set stream address".to_string();
        }

        if field == SettingsField::Interface {
            return self.describe_interface_setting();
        }

        field.value(&self.settings.stream)
    }

    fn setting_value_style(&self, field: SettingsField) -> Style {
        if self.is_required_setting_missing(field) {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        }
    }

    fn is_required_setting_missing(&self, field: SettingsField) -> bool {
        match field {
            SettingsField::Address => self.settings.stream.address.trim().is_empty(),
            SettingsField::Interface => self
                .settings
                .stream
                .interface
                .as_deref()
                .is_none_or(str::is_empty),
            _ => false,
        }
    }

    fn describe_interface_setting(&self) -> String {
        let Some(interface) = self.settings.stream.interface.as_deref() else {
            return "Select interface".to_string();
        };

        if let Some(option) = self.interface_options.iter().find(|candidate| {
            candidate.name == interface || candidate.ipv4.to_string() == interface
        }) {
            return format!("{}  {}", option.name, option.ipv4);
        }

        interface.to_string()
    }

    fn stream_target_label(&self) -> String {
        if self.settings.stream.address.trim().is_empty() {
            "Set stream address".to_string()
        } else {
            format!(
                "{}:{}",
                self.settings.stream.address, self.settings.stream.port
            )
        }
    }

    fn stream_target_style(&self) -> Style {
        if self.settings.stream.address.trim().is_empty() {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Green)
        }
    }
}

pub async fn run() -> Result<()> {
    let settings_path = settings_file_path()?;
    let (settings, settings_created) = load_or_create_settings_with_state(&settings_path)?;
    let app = MusicPlayerApp::new(settings, settings_path, settings_created);

    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        let mut stdout = io::stdout();
        let snapshot = render_app_to_string(&app, 100, 30)?;
        stdout.write_all(snapshot.as_bytes())?;
        return Ok(());
    }

    run_terminal_app(app).await
}

async fn run_terminal_app(mut app: MusicPlayerApp) -> Result<()> {
    enable_raw_mode().context("failed to enable terminal raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("failed to create terminal")?;
    let mut active_stream = None;

    let run_result = run_event_loop(&mut terminal, &mut app, &mut active_stream).await;
    let restore_result = restore_terminal(&mut terminal);
    let shutdown_result = shutdown_active_stream(active_stream).await;

    match (run_result, restore_result, shutdown_result) {
        (Err(error), _, _) => Err(error),
        (Ok(()), Err(error), _) => Err(error),
        (Ok(()), Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(()), Ok(())) => Ok(()),
    }
}

async fn run_event_loop<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut MusicPlayerApp,
    active_stream: &mut Option<ActiveMusicStream>,
) -> Result<()> {
    loop {
        poll_active_stream(app, active_stream).await?;
        terminal.draw(|frame| render_app(frame, app))?;
        if app.should_quit {
            return Ok(());
        }

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                app.handle_key(key)?;
                handle_playback_command(app, active_stream).await?;
            }
        }
    }
}

async fn handle_playback_command(
    app: &mut MusicPlayerApp,
    active_stream: &mut Option<ActiveMusicStream>,
) -> Result<()> {
    while let Some(command) = app.take_playback_command() {
        match command {
            PlaybackCommand::Start(request) => {
                if let Some(active) = active_stream.take() {
                    shutdown_active_stream(Some(active)).await?;
                }
                *active_stream = Some(spawn_music_stream(request));
            }
            PlaybackCommand::Stop => {
                if let Some(active) = active_stream.as_ref() {
                    active.shutdown.cancel();
                } else {
                    app.playback_state = PlaybackState::Stopped;
                    app.playback_started_at = None;
                    app.playback_duration = None;
                    app.status = "Stopped".to_string();
                }
            }
        }
    }
    Ok(())
}

async fn poll_active_stream(
    app: &mut MusicPlayerApp,
    active_stream: &mut Option<ActiveMusicStream>,
) -> Result<()> {
    let Some(active) = active_stream.as_mut() else {
        return Ok(());
    };

    while let Ok(event) = active.events.try_recv() {
        match event {
            PlaybackRuntimeEvent::Started {
                track_index,
                duration,
            } => app.mark_stream_started_with_duration(track_index, Instant::now(), duration),
        }
    }

    if !active.handle.is_finished() {
        return Ok(());
    }

    let active = active_stream
        .take()
        .expect("active stream should exist after finished check");
    let track_index = active.track_index;
    let result = match active.handle.await {
        Ok(result) => result,
        Err(error) => Err(anyhow!("music streamer task failed: {error}")),
    };
    app.mark_stream_finished(track_index, result);
    handle_playback_command(app, active_stream).await
}

fn spawn_music_stream(request: PlaybackStartRequest) -> ActiveMusicStream {
    let shutdown = CancellationToken::new();
    let task_shutdown = shutdown.clone();
    let track_index = request.track_index;
    let (event_tx, events) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        let config = stream_config_from_settings(&request.stream);
        let mut streamer = Aes67Streamer::new(
            &request.path,
            &request.stream.address,
            request.stream.port,
            request.stream.interface.as_deref(),
            config,
        )
        .await?;
        let duration = streamer.get_audio_info().duration;
        let _ = event_tx.send(PlaybackRuntimeEvent::Started {
            track_index,
            duration,
        });
        streamer.run_until_cancelled(task_shutdown).await
    });

    ActiveMusicStream {
        track_index,
        shutdown,
        events,
        handle,
    }
}

async fn shutdown_active_stream(active_stream: Option<ActiveMusicStream>) -> Result<()> {
    let Some(active) = active_stream else {
        return Ok(());
    };

    active.shutdown.cancel();
    active
        .handle
        .await
        .map_err(|error| anyhow!("music streamer task failed: {error}"))??;
    Ok(())
}

fn restore_terminal<W: Write>(terminal: &mut Terminal<CrosstermBackend<W>>) -> Result<()> {
    disable_raw_mode().context("failed to disable terminal raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)
        .context("failed to leave alternate screen")?;
    terminal.show_cursor().context("failed to show cursor")
}

fn render_app(frame: &mut Frame<'_>, app: &MusicPlayerApp) {
    let area = frame.area();
    render_player_surface(frame, app, area);

    if app.screen == AppScreen::Settings {
        let backdrop_area = Rect {
            x: area.x,
            y: area.y.saturating_add(3),
            width: area.width,
            height: area.height.saturating_sub(6),
        };
        frame.render_widget(Clear, backdrop_area);

        let modal_area = centered_rect(80, 78, area);
        frame.render_widget(Clear, modal_area);
        render_settings_modal(frame, app, modal_area);

        if let Some(picker) = &app.picker {
            let picker_area = centered_rect(64, 54, area);
            frame.render_widget(Clear, picker_area);
            render_picker_modal(frame, picker, picker_area);
        }
    }

    if let Some(input) = &app.path_input {
        let input_area = centered_rect(72, if input.show_completions { 44 } else { 28 }, area);
        frame.render_widget(Clear, input_area);
        render_path_input_modal(frame, input, &app.status, input_area);
    }
}

fn render_player_surface(frame: &mut Frame<'_>, app: &MusicPlayerApp, area: Rect) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(3),
        ])
        .split(area);

    let title = Line::from(vec![
        Span::styled(
            " AES67 Music Player ",
            Style::default()
                .fg(ACCENT_COLOR)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(app.playback_state_label(), app.playback_state_style()),
        Span::raw("  "),
        Span::styled(app.stream_target_label(), app.stream_target_style()),
    ]);
    frame.render_widget(
        Paragraph::new(title)
            .block(Block::default().borders(Borders::BOTTOM))
            .alignment(Alignment::Left),
        vertical[0],
    );

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(62), Constraint::Percentage(38)])
        .split(vertical[1]);

    render_playlist(frame, app, body[0]);
    render_side_panel(frame, app, body[1]);

    let footer = Line::from(vec![
        Span::styled(" q ", Style::default().fg(Color::Black).bg(Color::Gray)),
        Span::raw(" quit   "),
        Span::styled(" s ", Style::default().fg(Color::Black).bg(Color::Gray)),
        Span::raw(" settings   "),
        Span::styled(" a ", Style::default().fg(Color::Black).bg(Color::Gray)),
        Span::raw(" add   "),
        Span::styled(" d ", Style::default().fg(Color::Black).bg(Color::Gray)),
        Span::raw(" remove   "),
        Span::styled(
            " up/down ",
            Style::default().fg(Color::Black).bg(Color::Gray),
        ),
        Span::raw(" queue   "),
        Span::styled(" space ", Style::default().fg(Color::Black).bg(Color::Gray)),
        Span::raw(format!(" {}   ", app.playback_action_label())),
        Span::styled(&app.status, Style::default().fg(Color::Yellow)),
    ]);
    frame.render_widget(
        Paragraph::new(footer).block(Block::default().borders(Borders::TOP)),
        vertical[2],
    );
}

fn render_playlist(frame: &mut Frame<'_>, app: &MusicPlayerApp, area: Rect) {
    let items = if app.settings.playlist.files.is_empty() {
        vec![ListItem::new(Line::from(vec![
            Span::styled("Empty queue", Style::default().fg(Color::DarkGray)),
            Span::raw(" - press "),
            Span::styled("a", Style::default().fg(ACCENT_COLOR)),
            Span::raw(" to add music in the next slice"),
        ]))]
    } else {
        app.settings
            .playlist
            .files
            .iter()
            .enumerate()
            .map(|(index, path)| {
                let selected = index == app.queue_selected;
                let marker = if selected { ">" } else { " " };
                let style = if selected {
                    Style::default()
                        .fg(ACCENT_COLOR)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(Span::styled(
                    format!("{marker} {}", display_path_name(path)),
                    style,
                )))
            })
            .collect()
    };

    let list = List::new(items).block(
        Block::default()
            .title(" Playlist Queue ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(ACCENT_COLOR)),
    );
    frame.render_widget(list, area);
}

fn render_side_panel(frame: &mut Frame<'_>, app: &MusicPlayerApp, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(36),
            Constraint::Length(5),
            Constraint::Min(7),
        ])
        .split(area);

    let now_playing = Paragraph::new(vec![
        Line::from(Span::styled(
            app.now_playing_track_name(),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(match app.playback_state {
            PlaybackState::Stopped if app.settings.playlist.files.is_empty() => {
                "Add files or folders to build the queue."
            }
            PlaybackState::Stopped => "No active stream.",
            PlaybackState::Starting { .. } => "Preparing AES67 streamer.",
            PlaybackState::Streaming { .. } => "Streaming RTP audio from the queue.",
            PlaybackState::Stopping { .. } => "Stopping current stream.",
            PlaybackState::Error { .. } => "Streaming failed; check the status line.",
        }),
        Line::from(
            app.selected_track_name()
                .map(|track| format!("Selected: {track}"))
                .unwrap_or_else(|| "Selected: none".to_string()),
        ),
    ])
    .block(
        Block::default()
            .title(" Now Playing ")
            .borders(Borders::ALL),
    )
    .wrap(Wrap { trim: true });
    frame.render_widget(now_playing, chunks[0]);

    render_stream_meter(frame, app, chunks[1]);

    let stream = &app.settings.stream;
    let stream_lines = vec![
        Line::from(vec![
            Span::styled("Session: ", Style::default().fg(Color::DarkGray)),
            Span::raw(stream.session_name.clone()),
        ]),
        Line::from(vec![
            Span::styled("RTP: ", Style::default().fg(Color::DarkGray)),
            Span::styled(app.stream_target_label(), app.stream_target_style()),
        ]),
        Line::from(vec![
            Span::styled("Interface: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                app.describe_interface_setting(),
                app.setting_value_style(SettingsField::Interface),
            ),
        ]),
        Line::from(vec![
            Span::styled("SAP: ", Style::default().fg(Color::DarkGray)),
            Span::raw(if stream.sap { "enabled" } else { "disabled" }),
        ]),
        Line::from(vec![
            Span::styled("PTP domain: ", Style::default().fg(Color::DarkGray)),
            Span::raw(stream.ptp_domain.to_string()),
        ]),
        Line::from(vec![
            Span::styled("Payload: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!(
                "{} / L24 / 48 kHz / {} ms",
                stream.payload_type, stream.packet_time_ms
            )),
        ]),
        Line::from(vec![
            Span::styled("TTL: ", Style::default().fg(Color::DarkGray)),
            Span::raw(stream.ttl.to_string()),
        ]),
    ];
    let stream_panel = Paragraph::new(stream_lines)
        .block(Block::default().title(" Stream ").borders(Borders::ALL))
        .wrap(Wrap { trim: true });
    frame.render_widget(stream_panel, chunks[2]);
}

fn render_stream_meter(frame: &mut Frame<'_>, app: &MusicPlayerApp, area: Rect) {
    let meter = app.playback_meter_at(Instant::now());
    let active = matches!(
        app.playback_state,
        PlaybackState::Starting { .. }
            | PlaybackState::Streaming { .. }
            | PlaybackState::Stopping { .. }
    );
    let ratio = if active { meter.progress_ratio } else { 0.0 };
    let label = if active {
        if let Some(duration) = meter.duration {
            format!(
                "{} / {}  ~{} RTP packets  {} pps",
                format_elapsed(meter.playhead),
                format_elapsed(duration),
                meter.target_packets,
                meter.target_packet_rate
            )
        } else {
            format!(
                "{} / --:--  ~{} RTP packets  {} pps",
                format_elapsed(meter.playhead),
                meter.target_packets,
                meter.target_packet_rate
            )
        }
    } else {
        "idle".to_string()
    };

    let gauge = Gauge::default()
        .block(Block::default().title(" Progress ").borders(Borders::ALL))
        .gauge_style(Style::default().fg(ACCENT_COLOR))
        .ratio(ratio)
        .label(label);
    frame.render_widget(gauge, area);
}

fn render_path_input_modal(frame: &mut Frame<'_>, input: &PathInput, status: &str, area: Rect) {
    let block = Block::default()
        .title(" Add Music ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT_COLOR));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let show_completions = input.show_completions && !input.completions.is_empty();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if show_completions {
            vec![
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Min(3),
                Constraint::Length(1),
                Constraint::Length(1),
            ]
        } else {
            vec![
                Constraint::Length(1),
                Constraint::Length(3),
                Constraint::Length(1),
                Constraint::Length(1),
            ]
        })
        .split(inner);

    frame.render_widget(
        Paragraph::new("File or folder path").style(Style::default().fg(Color::DarkGray)),
        layout[0],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("> "),
            Span::styled(
                format!("{}_", input.value),
                Style::default().fg(ACCENT_COLOR),
            ),
        ]))
        .block(Block::default().borders(Borders::ALL))
        .wrap(Wrap { trim: false }),
        layout[1],
    );

    let status_index = if show_completions {
        let items: Vec<ListItem> = input
            .completions
            .iter()
            .take(COMPLETION_LIST_LIMIT)
            .enumerate()
            .map(|(index, completion)| {
                let selected = index == input.completion_index;
                let marker = if selected { ">" } else { " " };
                let style = if selected {
                    Style::default().fg(Color::Black).bg(ACCENT_COLOR)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(Span::styled(
                    format!("{marker} {}", completion_option_label(completion)),
                    style,
                )))
            })
            .collect();
        frame.render_widget(
            List::new(items).block(Block::default().title(" Options ").borders(Borders::ALL)),
            layout[2],
        );
        3
    } else {
        2
    };

    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(Color::Yellow)),
        layout[status_index],
    );
    frame.render_widget(
        Paragraph::new(if show_completions {
            "up/down choose | enter select | tab cycle | esc cancel"
        } else {
            "tab complete | enter add | esc cancel"
        })
        .style(Style::default().fg(Color::DarkGray)),
        layout[status_index + 1],
    );
}

fn render_settings_modal(frame: &mut Frame<'_>, app: &MusicPlayerApp, area: Rect) {
    let block = Block::default()
        .title(" Stream Settings ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT_COLOR));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),
            Constraint::Length(2),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(inner);

    let rows: Vec<ListItem> = SettingsField::ALL
        .iter()
        .map(|field| {
            let mut style = Style::default();
            let marker = if *field == app.settings_focus {
                style = style.fg(Color::Black).bg(ACCENT_COLOR);
                ">"
            } else {
                " "
            };

            let value = if app.edit.as_ref().map(|edit| edit.field) == Some(*field) {
                app.edit
                    .as_ref()
                    .map(|edit| format!("{}_", edit.value))
                    .unwrap_or_default()
            } else {
                app.setting_display_value(*field)
            };
            let value_style = if app.edit.as_ref().map(|edit| edit.field) == Some(*field) {
                Style::default().fg(ACCENT_COLOR)
            } else {
                app.setting_value_style(*field)
            };

            ListItem::new(Line::from(vec![
                Span::styled(format!("{marker} {:<18}", field.label()), style),
                Span::raw(" "),
                Span::styled(value, value_style),
            ]))
        })
        .collect();

    frame.render_widget(List::new(rows), layout[0]);

    let hint = Paragraph::new(Line::from(vec![
        Span::styled(
            app.settings_focus.hint(),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw("  "),
        Span::styled(
            format!("Settings file: {}", app.settings_path.display()),
            Style::default().fg(Color::DarkGray),
        ),
    ]))
    .wrap(Wrap { trim: true });
    frame.render_widget(hint, layout[1]);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            &app.status,
            Style::default().fg(Color::Yellow),
        ))),
        layout[2],
    );

    let controls = if app.edit.is_some() {
        "enter apply | esc cancel | type to edit"
    } else if app.settings_required {
        "up/down choose | enter select/edit | e manual | space toggle | s save | q quit"
    } else {
        "up/down choose | enter select/edit | e manual | space toggle | s save | esc close"
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            controls,
            Style::default().fg(ACCENT_COLOR),
        ))),
        layout[3],
    );
}

fn render_picker_modal(frame: &mut Frame<'_>, picker: &SettingsPicker, area: Rect) {
    let block = Block::default()
        .title(format!(" {} ", picker.title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT_COLOR));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(inner);

    let items: Vec<ListItem> = picker
        .options
        .iter()
        .enumerate()
        .map(|(index, option)| {
            let selected = index == picker.selected;
            let marker = if selected { ">" } else { " " };
            let style = if selected {
                Style::default().fg(Color::Black).bg(ACCENT_COLOR)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(
                format!("{marker} {}", option.label),
                style,
            )))
        })
        .collect();

    frame.render_widget(List::new(items), layout[0]);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "up/down choose | enter select | r refresh | esc cancel",
            Style::default().fg(ACCENT_COLOR),
        ))),
        layout[1],
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical_margin = (100 - percent_y) / 2;
    let horizontal_margin = (100 - percent_x) / 2;
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(vertical_margin),
            Constraint::Percentage(percent_y),
            Constraint::Percentage(vertical_margin),
        ])
        .split(area);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(horizontal_margin),
            Constraint::Percentage(percent_x),
            Constraint::Percentage(horizontal_margin),
        ])
        .split(vertical[1]);
    horizontal[1]
}

fn render_app_to_string(app: &MusicPlayerApp, width: u16, height: u16) -> Result<String> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend)?;
    terminal.draw(|frame| render_app(frame, app))?;
    Ok(buffer_to_string(terminal.backend().buffer()))
}

fn buffer_to_string(buffer: &Buffer) -> String {
    let area = buffer.area;
    let mut output = String::new();
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            output.push_str(buffer[(x, y)].symbol());
        }
        output.push('\n');
    }
    output
}

fn settings_file_path() -> Result<PathBuf> {
    Ok(settings_dir()?.join(SETTINGS_FILE))
}

fn settings_dir() -> Result<PathBuf> {
    if let Some(path) = env::var_os(CONFIG_DIR_ENV) {
        return Ok(PathBuf::from(path));
    }

    #[cfg(target_os = "macos")]
    {
        let home = env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
        Ok(PathBuf::from(home)
            .join("Library")
            .join("Application Support")
            .join("aes67-tools"))
    }

    #[cfg(not(target_os = "macos"))]
    {
        if let Some(config_home) = env::var_os("XDG_CONFIG_HOME") {
            return Ok(PathBuf::from(config_home).join("aes67-tools"));
        }

        let home = env::var_os("HOME").ok_or_else(|| anyhow!("HOME is not set"))?;
        Ok(PathBuf::from(home).join(".config").join("aes67-tools"))
    }
}

fn load_or_create_settings_with_state(path: &Path) -> Result<(MusicPlayerSettings, bool)> {
    if path.exists() {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read settings file {}", path.display()))?;
        let settings = toml::from_str(&contents)
            .with_context(|| format!("failed to parse settings file {}", path.display()))?;
        return Ok((settings, false));
    }

    let settings = MusicPlayerSettings::default();
    save_settings(path, &settings)?;
    Ok((settings, true))
}

fn save_settings(path: &Path, settings: &MusicPlayerSettings) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create settings directory {}", parent.display()))?;
    }

    let contents = toml::to_string_pretty(settings)?;
    fs::write(path, contents)
        .with_context(|| format!("failed to write settings file {}", path.display()))
}

fn stream_config_from_settings(stream: &StreamSettings) -> StreamConfig {
    StreamConfig {
        target_sample_rate: 48_000,
        packet_time_ms: stream.packet_time_ms,
        gain_db: 0.0,
        ptp_domain: stream.ptp_domain,
        verbose: false,
        duration: None,
        loop_playback: false,
        ttl: stream.ttl,
        sap: stream.sap,
        payload_type: stream.payload_type,
        ssrc: None,
        session_name: stream.session_name.clone(),
    }
}

fn collect_audio_files(path: &Path) -> Result<Vec<PathBuf>> {
    let expanded_path = expand_user_path(path);

    if expanded_path.is_file() {
        return Ok(if is_supported_audio_file(&expanded_path) {
            vec![expanded_path]
        } else {
            Vec::new()
        });
    }

    if expanded_path.is_dir() {
        let mut files = Vec::new();
        for entry in fs::read_dir(&expanded_path)
            .with_context(|| format!("failed to read folder {}", expanded_path.display()))?
        {
            let entry = entry
                .with_context(|| format!("failed to read entry in {}", expanded_path.display()))?;
            let entry_path = entry.path();
            if entry_path.is_file() && is_supported_audio_file(&entry_path) {
                files.push(entry_path);
            }
        }
        files.sort_by(|left, right| left.to_string_lossy().cmp(&right.to_string_lossy()));
        return Ok(files);
    }

    Ok(Vec::new())
}

fn path_completions(input: &str) -> Result<Vec<String>> {
    path_completions_with_home(input, home_dir().as_deref())
}

fn is_browse_completion_input(input: &str) -> bool {
    let input = input.trim();
    input.is_empty() || input.ends_with(std::path::MAIN_SEPARATOR)
}

fn path_completions_with_home(input: &str, home: Option<&Path>) -> Result<Vec<String>> {
    let input = input.trim();
    if input.is_empty() {
        return completion_values_in_parent(Path::new("."), Some(""), "");
    }

    let Some(context) = completion_context(input, home) else {
        return Ok(Vec::new());
    };
    let parent = context.fs_parent;
    if !parent.is_dir() {
        return Ok(Vec::new());
    }

    completion_values_in_parent(&parent, context.display_parent.as_deref(), &context.prefix)
}

fn completion_values_in_parent(
    parent: &Path,
    display_parent: Option<&str>,
    prefix: &str,
) -> Result<Vec<String>> {
    let mut completions = Vec::new();
    for entry in fs::read_dir(parent)
        .with_context(|| format!("failed to read folder {}", parent.display()))?
    {
        let entry =
            entry.with_context(|| format!("failed to read entry in {}", parent.display()))?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if file_name.starts_with('.') {
            continue;
        }
        if !file_name
            .to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
        {
            continue;
        }

        let path = parent.join(file_name);
        if path.is_dir() {
            completions.push(completion_value(&path, true, display_parent));
        } else if path.is_file() && is_supported_audio_file(&path) {
            completions.push(completion_value(&path, false, display_parent));
        }
    }
    completions.sort();
    Ok(completions)
}

#[derive(Debug)]
struct CompletionContext {
    fs_parent: PathBuf,
    display_parent: Option<String>,
    prefix: String,
}

fn completion_context(input: &str, home: Option<&Path>) -> Option<CompletionContext> {
    if input.ends_with(std::path::MAIN_SEPARATOR) {
        let fs_parent = expand_user_path_with_home(input, home)?;
        return Some(CompletionContext {
            fs_parent,
            display_parent: Some(
                input
                    .trim_end_matches(std::path::MAIN_SEPARATOR)
                    .to_string(),
            ),
            prefix: String::new(),
        });
    }

    let path = Path::new(input);
    let input_parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let fs_parent = expand_user_path_with_home(&input_parent, home)?;
    let display_parent = if input_parent == Path::new(".") {
        None
    } else {
        Some(input_parent.to_string_lossy().to_string())
    };
    let prefix = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_string();
    Some(CompletionContext {
        fs_parent,
        display_parent,
        prefix,
    })
}

fn completion_value(path: &Path, is_dir: bool, display_parent: Option<&str>) -> String {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let mut value = if let Some(parent) = display_parent {
        if parent.is_empty() {
            file_name.to_string()
        } else {
            format!("{}{}{}", parent, std::path::MAIN_SEPARATOR, file_name)
        }
    } else {
        path.to_string_lossy().to_string()
    };
    if is_dir && !value.ends_with(std::path::MAIN_SEPARATOR) {
        value.push(std::path::MAIN_SEPARATOR);
    }
    value
}

fn expand_user_path(path: &Path) -> PathBuf {
    expand_user_path_with_home(path, home_dir().as_deref()).unwrap_or_else(|| path.to_path_buf())
}

fn expand_user_path_with_home(path: impl AsRef<Path>, home: Option<&Path>) -> Option<PathBuf> {
    let path = path.as_ref();
    let path_string = path.to_string_lossy();
    if path_string == "~" {
        return home.map(Path::to_path_buf);
    }
    if let Some(rest) = path_string.strip_prefix("~/") {
        return home.map(|home| home.join(rest));
    }
    Some(path.to_path_buf())
}

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

fn is_supported_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "wav" | "flac" | "mp3" | "aiff" | "aif"
            )
        })
        .unwrap_or(false)
}

fn display_path_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(path)
        .to_string()
}

fn format_elapsed(duration: Duration) -> String {
    let total_seconds = duration.as_secs();
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{minutes:02}:{seconds:02}")
}

fn completion_option_label(path: &str) -> String {
    let is_dir = path.ends_with(std::path::MAIN_SEPARATOR);
    let trimmed = path.trim_end_matches(std::path::MAIN_SEPARATOR);
    let mut label = Path::new(trimmed)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(trimmed)
        .to_string();
    if is_dir {
        label.push(std::path::MAIN_SEPARATOR);
    }
    label
}

fn parse_bool(value: &str, name: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "true" | "yes" | "y" | "1" | "on" | "enabled" => Ok(true),
        "false" | "no" | "n" | "0" | "off" | "disabled" => Ok(false),
        _ => Err(anyhow!("{name} must be true or false")),
    }
}

fn parse_u8(value: &str, name: &str) -> Result<u8> {
    value
        .trim()
        .parse::<u8>()
        .with_context(|| format!("{name} must be an integer between 0 and 255"))
}

fn parse_u16(value: &str, name: &str) -> Result<u16> {
    value
        .trim()
        .parse::<u16>()
        .with_context(|| format!("{name} must be an integer between 0 and 65535"))
}

fn parse_positive_u32(value: &str, name: &str) -> Result<u32> {
    let parsed = value
        .trim()
        .parse::<u32>()
        .with_context(|| format!("{name} must be a positive integer"))?;
    if parsed == 0 {
        return Err(anyhow!("{name} must be greater than zero"));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use network::NetworkInterface;
    use std::net::Ipv4Addr;
    use std::time::Instant;

    fn temp_settings_file(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("aes67-music-player-{name}-{}", std::process::id()));
        fs::remove_dir_all(&dir).ok();
        fs::create_dir_all(&dir).expect("temp dir should be created");
        dir.join(SETTINGS_FILE)
    }

    fn key(ch: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE)
    }

    fn key_code(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn interface(name: &str, ipv4: [u8; 4]) -> NetworkInterface {
        NetworkInterface {
            name: name.to_string(),
            ipv4: Ipv4Addr::from(ipv4),
            is_loopback: ipv4[0] == 127,
        }
    }

    fn configured_app(name: &str) -> (MusicPlayerApp, PathBuf) {
        let path = temp_settings_file(name);
        let mut settings = MusicPlayerSettings::default();
        settings.stream.address = "239.69.83.1".to_string();
        settings.stream.interface = Some("en0".to_string());
        (
            MusicPlayerApp::new_with_interfaces(
                settings,
                path.clone(),
                false,
                vec![interface("en0", [192, 168, 1, 42])],
            ),
            path,
        )
    }

    fn type_path(app: &mut MusicPlayerApp, path: &Path) {
        app.handle_key(key('a')).expect("path input should open");
        for ch in path.to_string_lossy().chars() {
            app.handle_key(key(ch))
                .expect("path character should apply");
        }
        app.handle_key(key_code(KeyCode::Enter))
            .expect("path input should apply");
    }

    fn write_test_file(path: &Path) {
        fs::write(path, b"test").expect("test file should be written");
    }

    #[test]
    fn default_settings_do_not_include_stream_address() {
        assert_eq!(MusicPlayerSettings::default().stream.address, "");
    }

    #[test]
    fn first_run_persists_default_settings() {
        let path = temp_settings_file("first-run");

        let (settings, _) =
            load_or_create_settings_with_state(&path).expect("settings should load");

        assert_eq!(settings, MusicPlayerSettings::default());
        assert!(fs::read_to_string(&path)
            .expect("settings should be readable")
            .contains("address"));

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn field_edit_updates_stream_address() {
        let path = temp_settings_file("field-address");
        let mut settings = MusicPlayerSettings::default();
        settings.stream.interface = Some("en0".to_string());
        let mut app = MusicPlayerApp::new(settings, path.clone(), false);

        app.apply_field_value(SettingsField::Address, "239.69.83.9")
            .expect("field edit should apply");
        app.save_and_close_settings()
            .expect("settings should persist");

        assert_eq!(app.settings.stream.address, "239.69.83.9");
        assert!(fs::read_to_string(&path)
            .expect("settings should be readable")
            .contains("239.69.83.9"));

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn missing_settings_launches_with_settings_modal_open() {
        let path = temp_settings_file("missing-settings");
        fs::remove_file(&path).ok();

        let (settings, created) =
            load_or_create_settings_with_state(&path).expect("settings should load");
        let app = MusicPlayerApp::new(settings, path.clone(), created);

        assert!(created);
        assert_eq!(app.screen, AppScreen::Settings);
        assert!(app.settings_required);

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn existing_settings_launches_on_player_screen() {
        let path = temp_settings_file("existing-settings");
        let mut settings = MusicPlayerSettings::default();
        settings.stream.address = "239.69.83.1".to_string();
        settings.stream.interface = Some("en0".to_string());
        save_settings(&path, &settings).expect("settings should save");

        let (settings, created) =
            load_or_create_settings_with_state(&path).expect("settings should load");
        let app = MusicPlayerApp::new(settings, path.clone(), created);

        assert!(!created);
        assert_eq!(app.screen, AppScreen::Player);
        assert!(!app.settings_required);

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn existing_settings_without_address_reopens_required_settings() {
        let path = temp_settings_file("existing-settings-missing-address");
        let mut settings = MusicPlayerSettings::default();
        settings.stream.interface = Some("en0".to_string());
        save_settings(&path, &settings).expect("settings should save");

        let (settings, created) =
            load_or_create_settings_with_state(&path).expect("settings should load");
        let app = MusicPlayerApp::new(settings, path.clone(), created);

        assert!(!created);
        assert_eq!(app.screen, AppScreen::Settings);
        assert!(app.settings_required);

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn existing_settings_without_interface_reopens_required_settings() {
        let path = temp_settings_file("existing-settings-missing-interface");
        let mut settings = MusicPlayerSettings::default();
        settings.stream.address = "239.69.83.1".to_string();
        save_settings(&path, &settings).expect("settings should save");

        let (settings, created) =
            load_or_create_settings_with_state(&path).expect("settings should load");
        let app = MusicPlayerApp::new(settings, path.clone(), created);

        assert!(!created);
        assert_eq!(app.screen, AppScreen::Settings);
        assert!(app.settings_required);

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn settings_key_reopens_same_settings_modal() {
        let path = temp_settings_file("settings-key");
        let mut app = MusicPlayerApp::new(MusicPlayerSettings::default(), path.clone(), false);

        app.handle_key(key('s')).expect("settings key should apply");

        assert_eq!(app.screen, AppScreen::Settings);
        assert_eq!(app.settings_focus, SettingsField::SessionName);

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn interface_field_enter_opens_interface_picker() {
        let path = temp_settings_file("interface-picker-open");
        let mut app = MusicPlayerApp::new_with_interfaces(
            MusicPlayerSettings::default(),
            path.clone(),
            false,
            vec![interface("en0", [192, 168, 1, 42])],
        );
        app.open_settings(false);
        app.settings_focus = SettingsField::Interface;

        app.handle_key(key_code(KeyCode::Enter))
            .expect("interface picker should open");

        let picker = app.picker.as_ref().expect("picker should be open");
        assert_eq!(picker.title, "Select Interface");
        assert!(!picker
            .options
            .iter()
            .any(|option| option.label == "Default route"));
        assert!(picker.options.iter().any(|option| {
            option.label.contains("en0") && option.label.contains("192.168.1.42")
        }));

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn interface_picker_selects_interface_name() {
        let path = temp_settings_file("interface-picker-select");
        let mut app = MusicPlayerApp::new_with_interfaces(
            MusicPlayerSettings::default(),
            path.clone(),
            false,
            vec![interface("en0", [192, 168, 1, 42])],
        );
        app.open_settings(false);
        app.settings_focus = SettingsField::Interface;
        app.handle_key(key_code(KeyCode::Enter))
            .expect("interface picker should open");

        app.handle_key(key_code(KeyCode::Enter))
            .expect("picker selection should apply");

        assert_eq!(app.settings.stream.interface.as_deref(), Some("en0"));
        assert!(app.picker.is_none());
        assert!(app.status.contains("en0"));

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn interface_picker_renders_interface_names_and_addresses() {
        let path = temp_settings_file("interface-picker-render");
        let mut app = MusicPlayerApp::new_with_interfaces(
            MusicPlayerSettings::default(),
            path.clone(),
            false,
            vec![interface("en0", [192, 168, 1, 42])],
        );
        app.open_settings(false);
        app.settings_focus = SettingsField::Interface;
        app.handle_key(key_code(KeyCode::Enter))
            .expect("interface picker should open");

        let output = render_app_to_string(&app, 100, 30).expect("app should render");

        assert!(output.contains("Select Interface"));
        assert!(!output.contains("Default route"));
        assert!(output.contains("en0"));
        assert!(output.contains("192.168.1.42"));

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn settings_cannot_save_without_interface() {
        let path = temp_settings_file("settings-require-interface");
        let mut settings = MusicPlayerSettings::default();
        settings.stream.address = "239.69.83.1".to_string();
        let mut app = MusicPlayerApp::new_with_interfaces(
            settings,
            path.clone(),
            true,
            vec![interface("en0", [192, 168, 1, 42])],
        );

        app.handle_key(key('s'))
            .expect("save without interface should not crash");

        assert_eq!(app.screen, AppScreen::Settings);
        assert!(app.settings_required);
        assert!(app.status.contains("Interface is required"));

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn settings_cannot_save_without_address() {
        let path = temp_settings_file("settings-require-address");
        let mut settings = MusicPlayerSettings::default();
        settings.stream.interface = Some("en0".to_string());
        let mut app = MusicPlayerApp::new_with_interfaces(
            settings,
            path.clone(),
            true,
            vec![interface("en0", [192, 168, 1, 42])],
        );

        app.handle_key(key('s'))
            .expect("save without address should not crash");

        assert_eq!(app.screen, AppScreen::Settings);
        assert!(app.settings_required);
        assert!(app.status.contains("Stream address is required"));

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn add_audio_file_from_path_input_persists_queue() {
        let (mut app, settings_path) = configured_app("queue-add-file");
        let audio_path = settings_path
            .parent()
            .expect("settings should have parent")
            .join("track.wav");
        write_test_file(&audio_path);

        type_path(&mut app, &audio_path);

        assert_eq!(
            app.settings.playlist.files,
            vec![audio_path.to_string_lossy().to_string()]
        );
        assert!(app.status.contains("Added"));
        assert!(fs::read_to_string(&settings_path)
            .expect("settings should be readable")
            .contains("track.wav"));

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn add_folder_adds_supported_audio_files_in_sorted_order() {
        let (mut app, settings_path) = configured_app("queue-add-folder");
        let folder = settings_path
            .parent()
            .expect("settings should have parent")
            .join("music");
        fs::create_dir_all(&folder).expect("music folder should be created");
        let first = folder.join("a.WAV");
        let second = folder.join("b.flac");
        let ignored = folder.join("notes.txt");
        write_test_file(&second);
        write_test_file(&ignored);
        write_test_file(&first);

        type_path(&mut app, &folder);

        assert_eq!(
            app.settings.playlist.files,
            vec![
                first.to_string_lossy().to_string(),
                second.to_string_lossy().to_string()
            ]
        );

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn unsupported_playlist_path_does_not_modify_queue() {
        let (mut app, settings_path) = configured_app("queue-unsupported-path");
        let text_path = settings_path
            .parent()
            .expect("settings should have parent")
            .join("notes.txt");
        write_test_file(&text_path);

        type_path(&mut app, &text_path);

        assert!(app.settings.playlist.files.is_empty());
        assert!(app.status.contains("Unsupported audio file type"));

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn missing_playlist_path_does_not_modify_queue() {
        let (mut app, settings_path) = configured_app("queue-missing-path");
        let missing_path = settings_path
            .parent()
            .expect("settings should have parent")
            .join("missing.wav");

        type_path(&mut app, &missing_path);

        assert!(app.settings.playlist.files.is_empty());
        assert!(app.status.contains("Path does not exist"));

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn tab_completes_matching_folder_with_trailing_separator() {
        let (mut app, settings_path) = configured_app("queue-complete-folder");
        let root = settings_path.parent().expect("settings should have parent");
        let folder = root.join("Music");
        fs::create_dir_all(&folder).expect("music folder should be created");

        app.handle_key(key('a')).expect("path input should open");
        for ch in root.join("mu").to_string_lossy().chars() {
            app.handle_key(key(ch))
                .expect("path character should apply");
        }
        app.handle_key(key_code(KeyCode::Tab))
            .expect("tab should complete path");

        assert_eq!(
            app.path_input
                .as_ref()
                .expect("input should remain open")
                .value,
            format!("{}/", folder.to_string_lossy())
        );

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn tab_completion_expands_home_directory_prefix() {
        let settings_path = temp_settings_file("queue-complete-home");
        let home = settings_path
            .parent()
            .expect("settings should have parent")
            .join("home");
        let music = home.join("Music");
        fs::create_dir_all(&music).expect("music folder should be created");

        let completions = path_completions_with_home("~/mu", Some(&home))
            .expect("home completion should succeed");

        assert_eq!(completions, vec!["~/Music/"]);

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn empty_path_tab_lists_without_selecting_first_completion() {
        let (mut app, settings_path) = configured_app("queue-complete-empty");

        app.handle_key(key('a')).expect("path input should open");
        app.handle_key(key_code(KeyCode::Tab))
            .expect("first tab should prepare completions");

        let input = app.path_input.as_ref().expect("input should remain open");
        assert_eq!(input.value, "");
        assert!(!input.show_completions);

        app.handle_key(key_code(KeyCode::Tab))
            .expect("second tab should show options");

        let input = app.path_input.as_ref().expect("input should remain open");
        assert_eq!(input.value, "");
        assert!(input.show_completions);

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn folder_path_tab_lists_without_selecting_first_completion() {
        let (mut app, settings_path) = configured_app("queue-complete-folder-list");
        let root = settings_path.parent().expect("settings should have parent");
        let folder = root.join("music");
        let album = folder.join("Album");
        let track = folder.join("intro.wav");
        fs::create_dir_all(&album).expect("album folder should be created");
        write_test_file(&track);

        app.handle_key(key('a')).expect("path input should open");
        let folder_input = format!("{}/", folder.to_string_lossy());
        for ch in folder_input.chars() {
            app.handle_key(key(ch))
                .expect("path character should apply");
        }
        app.handle_key(key_code(KeyCode::Tab))
            .expect("first tab should prepare folder completions");

        let input = app.path_input.as_ref().expect("input should remain open");
        assert_eq!(input.value, folder_input);
        assert!(!input.show_completions);

        app.handle_key(key_code(KeyCode::Tab))
            .expect("second tab should show folder options");

        let input = app.path_input.as_ref().expect("input should remain open");
        assert_eq!(input.value, folder_input);
        assert!(input.show_completions);

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn folder_path_single_match_still_shows_option_on_second_tab() {
        let (mut app, settings_path) = configured_app("queue-complete-folder-single");
        let root = settings_path.parent().expect("settings should have parent");
        let folder = root.join("music");
        let album = folder.join("Album");
        fs::create_dir_all(&album).expect("album folder should be created");

        app.handle_key(key('a')).expect("path input should open");
        let folder_input = format!("{}/", folder.to_string_lossy());
        for ch in folder_input.chars() {
            app.handle_key(key(ch))
                .expect("path character should apply");
        }
        app.handle_key(key_code(KeyCode::Tab))
            .expect("first tab should prepare folder completions");
        app.handle_key(key_code(KeyCode::Tab))
            .expect("second tab should show the single folder option");

        let input = app.path_input.as_ref().expect("input should remain open");
        assert_eq!(input.value, folder_input);
        assert!(input.show_completions);
        assert_eq!(input.completions.len(), 1);
        assert!(app.status.contains("Showing 1 matches"));
        assert!(!app.status.contains("Completed path"));

        let output = render_app_to_string(&app, 100, 30).expect("app should render");
        assert!(output.contains("Album/"));

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn folder_path_options_ignore_hidden_entries() {
        let settings_path = temp_settings_file("queue-complete-hidden");
        let root = settings_path.parent().expect("settings should have parent");
        let folder = root.join("music");
        let hidden_folder = folder.join(".Trash");
        let visible_folder = folder.join("Album");
        let hidden_file = folder.join(".draft.wav");
        let visible_file = folder.join("intro.wav");
        fs::create_dir_all(&hidden_folder).expect("hidden folder should be created");
        fs::create_dir_all(&visible_folder).expect("visible folder should be created");
        write_test_file(&hidden_file);
        write_test_file(&visible_file);

        let completions = path_completions(&format!("{}/", folder.to_string_lossy()))
            .expect("folder completions should load");

        assert!(completions.iter().any(|path| path.ends_with("Album/")));
        assert!(completions.iter().any(|path| path.ends_with("intro.wav")));
        assert!(!completions
            .iter()
            .any(|path| completion_option_label(path).starts_with('.')));

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn completed_folder_path_can_be_browsed_with_next_tabs() {
        let (mut app, settings_path) = configured_app("queue-complete-folder-then-browse");
        let root = settings_path.parent().expect("settings should have parent");
        let music = root.join("Music");
        let album = music.join("Album");
        fs::create_dir_all(&album).expect("album folder should be created");

        app.handle_key(key('a')).expect("path input should open");
        for ch in root.join("Mu").to_string_lossy().chars() {
            app.handle_key(key(ch))
                .expect("path character should apply");
        }
        app.handle_key(key_code(KeyCode::Tab))
            .expect("first tab should complete to the folder path");

        let folder_value = format!("{}/", music.to_string_lossy());
        assert_eq!(
            app.path_input
                .as_ref()
                .expect("input should remain open")
                .value,
            folder_value
        );
        assert!(app.status.contains("Completed path"));

        app.handle_key(key_code(KeyCode::Tab))
            .expect("next tab should prepare child completions");
        let input = app.path_input.as_ref().expect("input should remain open");
        assert_eq!(input.value, folder_value);
        assert!(!input.show_completions);
        assert!(input
            .completions
            .iter()
            .any(|path| path.ends_with("Album/")));
        assert!(!app.status.contains("Completed path"));

        app.handle_key(key_code(KeyCode::Tab))
            .expect("next tab should show child options");
        let input = app.path_input.as_ref().expect("input should remain open");
        assert_eq!(input.value, folder_value);
        assert!(input.show_completions);

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn arrow_keys_move_visible_path_completion_selection() {
        let (mut app, settings_path) = configured_app("queue-complete-arrow");
        let root = settings_path.parent().expect("settings should have parent");
        let folder = root.join("music");
        fs::create_dir_all(folder.join("Album")).expect("album folder should be created");
        fs::create_dir_all(folder.join("Mixes")).expect("mixes folder should be created");

        app.handle_key(key('a')).expect("path input should open");
        let folder_input = format!("{}/", folder.to_string_lossy());
        for ch in folder_input.chars() {
            app.handle_key(key(ch))
                .expect("path character should apply");
        }
        app.handle_key(key_code(KeyCode::Tab))
            .expect("first tab should prepare folder completions");
        app.handle_key(key_code(KeyCode::Tab))
            .expect("second tab should show folder options");

        assert_eq!(
            app.path_input
                .as_ref()
                .expect("input should remain open")
                .completion_index,
            0
        );

        app.handle_key(key_code(KeyCode::Down))
            .expect("down should move option selection");
        assert_eq!(
            app.path_input
                .as_ref()
                .expect("input should remain open")
                .completion_index,
            1
        );

        app.handle_key(key_code(KeyCode::Up))
            .expect("up should move option selection");
        assert_eq!(
            app.path_input
                .as_ref()
                .expect("input should remain open")
                .completion_index,
            0
        );

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn enter_selects_visible_path_completion() {
        let (mut app, settings_path) = configured_app("queue-complete-enter");
        let root = settings_path.parent().expect("settings should have parent");
        let folder = root.join("music");
        let album = folder.join("Album");
        let mixes = folder.join("Mixes");
        fs::create_dir_all(&album).expect("album folder should be created");
        fs::create_dir_all(&mixes).expect("mixes folder should be created");

        app.handle_key(key('a')).expect("path input should open");
        let folder_input = format!("{}/", folder.to_string_lossy());
        for ch in folder_input.chars() {
            app.handle_key(key(ch))
                .expect("path character should apply");
        }
        app.handle_key(key_code(KeyCode::Tab))
            .expect("first tab should prepare folder completions");
        app.handle_key(key_code(KeyCode::Tab))
            .expect("second tab should show folder options");
        app.handle_key(key_code(KeyCode::Down))
            .expect("down should move option selection");
        app.handle_key(key_code(KeyCode::Enter))
            .expect("enter should select the highlighted option");

        let input = app.path_input.as_ref().expect("input should remain open");
        assert_eq!(input.value, format!("{}/", mixes.to_string_lossy()));
        assert!(input.completions.is_empty());
        assert!(!input.show_completions);

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn tab_cycles_matching_audio_files_and_folders() {
        let (mut app, settings_path) = configured_app("queue-complete-cycle");
        let root = settings_path.parent().expect("settings should have parent");
        let album = root.join("album");
        let first = root.join("alpha.wav");
        let second = root.join("ambient.flac");
        fs::create_dir_all(&album).expect("album folder should be created");
        write_test_file(&first);
        write_test_file(&second);

        app.handle_key(key('a')).expect("path input should open");
        for ch in root.join("a").to_string_lossy().chars() {
            app.handle_key(key(ch))
                .expect("path character should apply");
        }

        app.handle_key(key_code(KeyCode::Tab))
            .expect("first tab should complete path");
        let first_completion = app
            .path_input
            .as_ref()
            .expect("input should remain open")
            .value
            .clone();
        app.handle_key(key_code(KeyCode::Tab))
            .expect("second tab should cycle path");
        let second_completion = app
            .path_input
            .as_ref()
            .expect("input should remain open")
            .value
            .clone();

        assert_ne!(first_completion, second_completion);
        assert!(
            first_completion.ends_with('/')
                || first_completion.ends_with(".flac")
                || first_completion.ends_with(".wav")
        );
        assert!(
            second_completion.ends_with('/')
                || second_completion.ends_with(".flac")
                || second_completion.ends_with(".wav")
        );

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn second_tab_displays_completion_options() {
        let (mut app, settings_path) = configured_app("queue-complete-display");
        let root = settings_path.parent().expect("settings should have parent");
        let music = root.join("Music");
        let mixes = root.join("Mixes");
        let ignored = root.join("Memos.txt");
        fs::create_dir_all(&music).expect("music folder should be created");
        fs::create_dir_all(&mixes).expect("mixes folder should be created");
        write_test_file(&ignored);

        app.handle_key(key('a')).expect("path input should open");
        for ch in root.join("M").to_string_lossy().chars() {
            app.handle_key(key(ch))
                .expect("path character should apply");
        }
        app.handle_key(key_code(KeyCode::Tab))
            .expect("first tab should complete path");
        app.handle_key(key_code(KeyCode::Tab))
            .expect("second tab should show options");

        let output = render_app_to_string(&app, 100, 30).expect("app should render");

        assert!(output.contains("Music/"));
        assert!(output.contains("Mixes/"));
        assert!(!output.contains("Memos.txt"));

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn tab_completion_ignores_unsupported_files() {
        let (mut app, settings_path) = configured_app("queue-complete-filter");
        let root = settings_path.parent().expect("settings should have parent");
        let ignored = root.join("ambient.txt");
        let audio = root.join("ambient.wav");
        write_test_file(&ignored);
        write_test_file(&audio);

        app.handle_key(key('a')).expect("path input should open");
        for ch in root.join("amb").to_string_lossy().chars() {
            app.handle_key(key(ch))
                .expect("path character should apply");
        }
        app.handle_key(key_code(KeyCode::Tab))
            .expect("tab should complete path");

        assert_eq!(
            app.path_input
                .as_ref()
                .expect("input should remain open")
                .value,
            audio.to_string_lossy()
        );

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn queue_selection_moves_with_player_arrow_keys() {
        let (mut app, settings_path) = configured_app("queue-selection");
        app.settings.playlist.files = vec!["one.wav".to_string(), "two.wav".to_string()];

        app.handle_key(key_code(KeyCode::Down))
            .expect("down should move queue selection");
        assert_eq!(app.queue_selected, 1);

        app.handle_key(key_code(KeyCode::Up))
            .expect("up should move queue selection");
        assert_eq!(app.queue_selected, 0);

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn remove_selected_queue_item_persists_queue() {
        let (mut app, settings_path) = configured_app("queue-remove");
        app.settings.playlist.files = vec!["one.wav".to_string(), "two.wav".to_string()];
        app.save_and_close_settings()
            .expect("initial queue should persist");

        app.handle_key(key_code(KeyCode::Down))
            .expect("down should move queue selection");
        app.handle_key(key('d'))
            .expect("delete should remove selected queue item");

        assert_eq!(app.settings.playlist.files, vec!["one.wav".to_string()]);
        assert_eq!(app.queue_selected, 0);
        let saved = fs::read_to_string(&settings_path).expect("settings should be readable");
        assert!(saved.contains("one.wav"));
        assert!(!saved.contains("two.wav"));

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn space_on_empty_queue_does_not_start_playback() {
        let (mut app, settings_path) = configured_app("playback-empty-queue");

        app.handle_key(key(' '))
            .expect("space should not fail with an empty queue");

        assert_eq!(app.playback_state, PlaybackState::Stopped);
        assert_eq!(app.take_playback_command(), None);
        assert!(app.status.contains("Queue is empty"));

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn space_requires_stream_settings_before_playback() {
        let path = temp_settings_file("playback-missing-settings");
        let mut settings = MusicPlayerSettings::default();
        settings.playlist.files = vec!["track.wav".to_string()];
        let mut app = MusicPlayerApp::new_with_interfaces(
            settings,
            path.clone(),
            false,
            vec![interface("en0", [192, 168, 1, 42])],
        );

        app.handle_player_key(key(' '))
            .expect("space should not fail when settings are incomplete");

        assert_eq!(app.playback_state, PlaybackState::Stopped);
        assert_eq!(app.take_playback_command(), None);
        assert!(app.status.contains("Stream address is required"));

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn space_starts_selected_queue_item() {
        let (mut app, settings_path) = configured_app("playback-start-selected");
        app.settings.playlist.files = vec!["one.wav".to_string(), "two.wav".to_string()];
        app.queue_selected = 1;

        app.handle_key(key(' '))
            .expect("space should request playback start");

        assert_eq!(
            app.playback_state,
            PlaybackState::Starting { track_index: 1 }
        );
        assert_eq!(
            app.take_playback_command(),
            Some(PlaybackCommand::Start(PlaybackStartRequest {
                track_index: 1,
                path: "two.wav".to_string(),
                stream: app.settings.stream.clone(),
            }))
        );
        assert_eq!(app.take_playback_command(), None);

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn space_stops_active_stream() {
        let (mut app, settings_path) = configured_app("playback-stop-active");
        app.settings.playlist.files = vec!["one.wav".to_string()];
        app.mark_stream_started(0, Instant::now());

        app.handle_key(key(' '))
            .expect("space should request playback stop");

        assert_eq!(
            app.playback_state,
            PlaybackState::Stopping { track_index: 0 }
        );
        assert_eq!(app.take_playback_command(), Some(PlaybackCommand::Stop));

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn now_playing_does_not_follow_queue_selection_when_stopped() {
        let (mut app, settings_path) = configured_app("now-playing-stopped-selection");
        app.settings.playlist.files = vec!["one.wav".to_string(), "two.wav".to_string()];

        app.handle_key(key_code(KeyCode::Down))
            .expect("down should move queue selection");

        assert_eq!(app.queue_selected, 1);
        assert_eq!(app.now_playing_track_name(), "No track playing");
        assert_eq!(app.selected_track_name().as_deref(), Some("two.wav"));

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn now_playing_keeps_active_track_when_selection_moves() {
        let (mut app, settings_path) = configured_app("now-playing-active-selection");
        app.settings.playlist.files = vec!["one.wav".to_string(), "two.wav".to_string()];
        app.mark_stream_started(0, Instant::now());

        app.handle_key(key_code(KeyCode::Down))
            .expect("down should move queue selection");

        assert_eq!(app.queue_selected, 1);
        assert_eq!(app.now_playing_track_name(), "one.wav");
        assert_eq!(app.selected_track_name().as_deref(), Some("two.wav"));

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn completed_track_advances_to_next_queue_item() {
        let (mut app, settings_path) = configured_app("playback-advance");
        app.settings.playlist.files = vec!["one.wav".to_string(), "two.wav".to_string()];
        app.mark_stream_started(0, Instant::now());

        app.mark_stream_finished(0, Ok(()));

        assert_eq!(app.queue_selected, 1);
        assert_eq!(
            app.playback_state,
            PlaybackState::Starting { track_index: 1 }
        );
        assert_eq!(
            app.take_playback_command(),
            Some(PlaybackCommand::Start(PlaybackStartRequest {
                track_index: 1,
                path: "two.wav".to_string(),
                stream: app.settings.stream.clone(),
            }))
        );

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn playback_meter_reports_elapsed_target_packets_and_rate() {
        let (mut app, settings_path) = configured_app("playback-meter");
        let started_at = Instant::now();
        app.settings.stream.packet_time_ms = 1;
        app.mark_stream_started(0, started_at);

        let meter = app.playback_meter_at(started_at + Duration::from_millis(2_345));

        assert_eq!(meter.elapsed, Duration::from_millis(2_345));
        assert_eq!(meter.target_packets, 2_345);
        assert_eq!(meter.target_packet_rate, 1_000);

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn playback_meter_reports_song_playhead_progress() {
        let (mut app, settings_path) = configured_app("playback-song-progress");
        let started_at = Instant::now();
        app.mark_stream_started_with_duration(0, started_at, Some(Duration::from_secs(120)));

        let meter = app.playback_meter_at(started_at + Duration::from_secs(30));

        assert_eq!(meter.playhead, Duration::from_secs(30));
        assert_eq!(meter.duration, Some(Duration::from_secs(120)));
        assert_eq!(meter.progress_ratio, 0.25);

        fs::remove_dir_all(settings_path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn stream_settings_build_streamer_config_for_music_mode() {
        let stream = StreamSettings {
            address: "239.69.83.1".to_string(),
            port: 5006,
            interface: Some("en0".to_string()),
            session_name: "Set".to_string(),
            sap: false,
            ptp_domain: 12,
            payload_type: 101,
            packet_time_ms: 2,
            ttl: 8,
        };

        let config = stream_config_from_settings(&stream);

        assert_eq!(config.target_sample_rate, 48_000);
        assert_eq!(config.packet_time_ms, 2);
        assert_eq!(config.ptp_domain, 12);
        assert_eq!(config.duration, None);
        assert!(!config.loop_playback);
        assert_eq!(config.ttl, 8);
        assert!(!config.sap);
        assert_eq!(config.payload_type, 101);
        assert_eq!(config.ssrc, None);
        assert_eq!(config.session_name, "Set");
    }

    #[test]
    fn ratatui_renderer_shows_player_surface() {
        let path = temp_settings_file("render-player");
        let mut settings = MusicPlayerSettings::default();
        settings.stream.address = "239.69.83.1".to_string();
        settings.stream.interface = Some("en0".to_string());
        let app = MusicPlayerApp::new(settings, path.clone(), false);

        let output = render_app_to_string(&app, 100, 30).expect("app should render");

        assert!(output.contains("AES67 Music Player"));
        assert!(output.contains("Playlist Queue"));
        assert!(output.contains("Now Playing"));
        assert!(output.contains("239.69.83.1"));

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }

    #[test]
    fn ratatui_renderer_shows_settings_modal() {
        let path = temp_settings_file("render-settings");
        let app = MusicPlayerApp::new(MusicPlayerSettings::default(), path.clone(), true);

        let output = render_app_to_string(&app, 100, 30).expect("app should render");

        assert!(output.contains("AES67 Music Player"));
        assert!(output.contains("Stream Settings"));

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }
}
