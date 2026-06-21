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
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::{Frame, Terminal};
use serde::{Deserialize, Serialize};
use std::env;
use std::fs;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const CONFIG_DIR_ENV: &str = "AES67_MUSIC_PLAYER_CONFIG_DIR";
const SETTINGS_FILE: &str = "music-player.toml";

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

#[derive(Debug, Clone)]
struct MusicPlayerApp {
    settings: MusicPlayerSettings,
    settings_path: PathBuf,
    interface_options: Vec<NetworkInterface>,
    screen: AppScreen,
    settings_required: bool,
    settings_focus: SettingsField,
    edit: Option<SettingEdit>,
    picker: Option<SettingsPicker>,
    status: String,
    should_quit: bool,
}

impl Default for MusicPlayerSettings {
    fn default() -> Self {
        Self {
            stream: StreamSettings {
                address: "239.69.83.1".to_string(),
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
        let settings_required = settings_created || settings.stream.interface.is_none();
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
            edit: None,
            picker: None,
            status: if settings_created {
                "First launch: select an interface, then press s.".to_string()
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
            KeyCode::Char('a') => {
                self.status = "Add file/folder is planned for the playlist slice.".to_string();
            }
            KeyCode::Char(' ') => {
                self.status = "Playback controls will connect to the streamer next.".to_string();
            }
            _ => {}
        }
        Ok(())
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
        if field == SettingsField::Interface {
            return self.describe_interface_setting();
        }

        field.value(&self.settings.stream)
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
}

pub fn run() -> Result<()> {
    let settings_path = settings_file_path()?;
    let (settings, settings_created) = load_or_create_settings_with_state(&settings_path)?;
    let app = MusicPlayerApp::new(settings, settings_path, settings_created);

    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        let mut stdout = io::stdout();
        let snapshot = render_app_to_string(&app, 100, 30)?;
        stdout.write_all(snapshot.as_bytes())?;
        return Ok(());
    }

    run_terminal_app(app)
}

fn run_terminal_app(mut app: MusicPlayerApp) -> Result<()> {
    enable_raw_mode().context("failed to enable terminal raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("failed to create terminal")?;

    let run_result = run_event_loop(&mut terminal, &mut app);
    let restore_result = restore_terminal(&mut terminal);

    match (run_result, restore_result) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

fn run_event_loop<B: Backend>(terminal: &mut Terminal<B>, app: &mut MusicPlayerApp) -> Result<()> {
    loop {
        terminal.draw(|frame| render_app(frame, app))?;
        if app.should_quit {
            return Ok(());
        }

        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                app.handle_key(key)?;
            }
        }
    }
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
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("stopped", Style::default().fg(Color::Yellow)),
        Span::raw("  "),
        Span::styled(
            format!(
                "{}:{}",
                app.settings.stream.address, app.settings.stream.port
            ),
            Style::default().fg(Color::Green),
        ),
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
        Span::styled(" space ", Style::default().fg(Color::Black).bg(Color::Gray)),
        Span::raw(" play/pause   "),
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
            Span::styled("a", Style::default().fg(Color::Cyan)),
            Span::raw(" to add music in the next slice"),
        ]))]
    } else {
        app.settings
            .playlist
            .files
            .iter()
            .map(|path| ListItem::new(path.clone()))
            .collect()
    };

    let list = List::new(items).block(
        Block::default()
            .title(" Playlist Queue ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan)),
    );
    frame.render_widget(list, area);
}

fn render_side_panel(frame: &mut Frame<'_>, app: &MusicPlayerApp, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(area);

    let now_playing = Paragraph::new(vec![
        Line::from(Span::styled(
            "No track loaded",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from("Playback and queue streaming will be connected next."),
    ])
    .block(
        Block::default()
            .title(" Now Playing ")
            .borders(Borders::ALL),
    )
    .wrap(Wrap { trim: true });
    frame.render_widget(now_playing, chunks[0]);

    let stream = &app.settings.stream;
    let stream_lines = vec![
        Line::from(vec![
            Span::styled("Session: ", Style::default().fg(Color::DarkGray)),
            Span::raw(stream.session_name.clone()),
        ]),
        Line::from(vec![
            Span::styled("RTP: ", Style::default().fg(Color::DarkGray)),
            Span::raw(format!("{}:{}", stream.address, stream.port)),
        ]),
        Line::from(vec![
            Span::styled("Interface: ", Style::default().fg(Color::DarkGray)),
            Span::raw(app.describe_interface_setting()),
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
    frame.render_widget(stream_panel, chunks[1]);
}

fn render_settings_modal(frame: &mut Frame<'_>, app: &MusicPlayerApp, area: Rect) {
    let block = Block::default()
        .title(" Stream Settings ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
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
                style = style.fg(Color::Black).bg(Color::Yellow);
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

            ListItem::new(Line::from(vec![
                Span::styled(format!("{marker} {:<18}", field.label()), style),
                Span::raw(" "),
                Span::styled(value, Style::default().fg(Color::Green)),
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
            Style::default().fg(Color::Cyan),
        ))),
        layout[3],
    );
}

fn render_picker_modal(frame: &mut Frame<'_>, picker: &SettingsPicker, area: Rect) {
    let block = Block::default()
        .title(format!(" {} ", picker.title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
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
                Style::default().fg(Color::Black).bg(Color::Cyan)
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
            Style::default().fg(Color::Cyan),
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
    fn existing_settings_without_interface_reopens_required_settings() {
        let path = temp_settings_file("existing-settings-missing-interface");
        save_settings(&path, &MusicPlayerSettings::default()).expect("settings should save");

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
        let mut app = MusicPlayerApp::new_with_interfaces(
            MusicPlayerSettings::default(),
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
    fn ratatui_renderer_shows_player_surface() {
        let path = temp_settings_file("render-player");
        let mut settings = MusicPlayerSettings::default();
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
        assert!(output.contains("239.69.83.1"));

        fs::remove_dir_all(path.parent().expect("settings should have parent")).ok();
    }
}
