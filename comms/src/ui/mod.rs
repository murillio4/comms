//! Tauri is a presentation layer. This actor owns session state; native audio and
//! network workers continue to communicate without going through the webview.
use std::borrow::Cow;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use directories::ProjectDirs;
use futures::channel::{mpsc, oneshot};
use futures::future::{AbortHandle, Abortable, Either};
use futures::{FutureExt, StreamExt};
use futures_timer::Delay;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager, State};
use tauri_plugin_deep_link::DeepLinkExt;
use uuid::Uuid;

use crate::audio::ipc::{AudioCommand, AudioHandle, EventLoopCreationError};
use crate::network;
use crate::network::future::{attempt_p2p, keep_alive, P2pDestination};
use crate::network::ipc::{ConnectError, NetworkCommand, NetworkEvent, NetworkHandle};
use comms_lib::packet::{
    ClientId, ErrorResponse, NetworkUser, Packet, PacketBody, UserId, UserJoined, UserLeft,
};
use ipc::{AudioIPCInitStatus, UICommand};

pub mod ipc;

pub fn comms_version() -> Cow<'static, str> {
    option_env!("CI_COMMIT_SHA")
        .map(|sha| Cow::from(format!("commit-{sha}")))
        .unwrap_or(env!("CARGO_PKG_VERSION").into())
}

// Keep the GTK client's storage path and format so existing identities and
// per-user volume preferences survive the migration.
#[derive(Serialize, Deserialize)]
struct LocalStorage {
    username: String,
    server: String,
    user_id: UserId,
    volumes: HashMap<UserId, u16>,
}

impl LocalStorage {
    fn load() -> Self {
        storage_path()
            .and_then(|p| std::fs::read(p).ok())
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_else(|| Self {
                username: std::env::var(if cfg!(windows) { "USERNAME" } else { "USER" })
                    .unwrap_or_default(),
                server: "getcomms.app".into(),
                user_id: Uuid::new_v4(),
                volumes: HashMap::new(),
            })
    }

    async fn save(&self) -> Result<(), String> {
        let path = storage_path().ok_or("Cannot find the application data directory")?;
        async_std::fs::create_dir_all(path.parent().unwrap())
            .await
            .map_err(|e| e.to_string())?;
        let bytes = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        // Serialize all writes in the actor and replace only after a full write.
        let temporary = path.with_extension("tmp");
        async_std::fs::write(&temporary, bytes)
            .await
            .map_err(|e| e.to_string())?;
        async_std::fs::rename(temporary, path)
            .await
            .map_err(|e| e.to_string())
    }
}

fn storage_path() -> Option<std::path::PathBuf> {
    ProjectDirs::from("", "Comms", "Comms").map(|dirs| dirs.data_dir().join("local_storage"))
}

#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct User {
    client_id: ClientId,
    #[serde(skip)]
    user_id: UserId,
    name: String,
    volume: u16,
    is_current_user: bool,
    latency_ms: f64,
    is_peer_to_peer: bool,
    is_speaking: bool,
    #[serde(skip)]
    speaking_until: Option<Instant>,
}

impl User {
    fn speak_for(&mut self, duration: Duration) -> bool {
        let changed = !self.is_speaking;
        self.is_speaking = true;
        let until = Instant::now() + duration;
        self.speaking_until = Some(self.speaking_until.map_or(until, |old| old.max(until)));
        changed
    }

    fn expire_speaking(&mut self, now: Instant) -> bool {
        if self.speaking_until.is_some_and(|until| until <= now) {
            self.speaking_until = None;
            self.is_speaking = false;
            return true;
        }
        false
    }
}

#[derive(Clone, Serialize)]
struct Device {
    id: String,
    label: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Snapshot {
    revision: u64,
    version: String,
    username: String,
    server: String,
    status: ConnectionStatus,
    audio_ready: bool,
    audio_error: Option<String>,
    error: Option<String>,
    users: Vec<User>,
    input_devices: Vec<Device>,
    output_devices: Vec<Device>,
}

enum Command {
    Snapshot,
    OpenLink(String),
    Connect { username: String, server: String },
    Disconnect,
    Volume { client_id: ClientId, volume: u16 },
    ListDevices,
}

type Response = Result<Snapshot, String>;
struct Request {
    command: Command,
    response: oneshot::Sender<Response>,
}
struct Bridge(async_channel::Sender<Request>);

impl Bridge {
    async fn request(&self, command: Command) -> Response {
        let (response, receiver) = oneshot::channel();
        self.0
            .send(Request { command, response })
            .await
            .map_err(|_| "Client worker stopped")?;
        receiver
            .await
            .map_err(|_| "Client worker stopped".to_string())?
    }
}

#[tauri::command]
async fn get_state(bridge: State<'_, Bridge>) -> Response {
    bridge.request(Command::Snapshot).await
}
#[tauri::command]
async fn connect(bridge: State<'_, Bridge>, username: String, server: String) -> Response {
    bridge.request(Command::Connect { username, server }).await
}
#[tauri::command]
async fn disconnect(bridge: State<'_, Bridge>) -> Response {
    bridge.request(Command::Disconnect).await
}
#[tauri::command]
async fn set_volume(bridge: State<'_, Bridge>, client_id: ClientId, volume: u16) -> Response {
    bridge.request(Command::Volume { client_id, volume }).await
}
#[tauri::command]
async fn list_devices(bridge: State<'_, Bridge>) -> Response {
    bridge.request(Command::ListDevices).await
}

// Keep only one discovery task per live peer and one keepalive per session.
// Dropping or clearing the group makes pending tasks unable to affect a later
// connection, even if a client ID is reused.
#[derive(Default)]
struct SessionTasks(HashMap<Option<ClientId>, AbortHandle>);

impl SessionTasks {
    fn spawn(
        &mut self,
        peer: Option<ClientId>,
        task: impl std::future::Future<Output = ()> + Send + 'static,
    ) {
        let (handle, registration) = AbortHandle::new_pair();
        if let Some(old) = self.0.insert(peer, handle) {
            old.abort();
        }
        async_std::task::spawn(async move {
            let _ = Abortable::new(task, registration).await;
        });
    }

    fn remove(&mut self, peer: Option<ClientId>) {
        if let Some(task) = self.0.remove(&peer) {
            task.abort();
        }
    }

    fn clear(&mut self) {
        for (_, task) in self.0.drain() {
            task.abort();
        }
    }
}

impl Drop for SessionTasks {
    fn drop(&mut self) {
        self.clear();
    }
}

struct Session {
    storage: LocalStorage,
    snapshot: Snapshot,
    client_id: Option<ClientId>,
    round_trip_latency: Duration,
    tasks: SessionTasks,
    audio: AudioHandle,
    network: NetworkHandle,
    events: mpsc::Sender<NetworkEvent>,
    app: tauri::AppHandle,
}

impl Session {
    fn publish(&mut self) {
        self.snapshot.revision += 1;
        if let Err(error) = self.app.emit("comms-state", &self.snapshot) {
            eprintln!("Cannot publish client state: {error}");
        }
    }

    fn send_audio(&self, command: AudioCommand) -> Result<(), String> {
        self.audio
            .send(command)
            .map_err(|_| "Audio worker stopped; restart Comms".into())
    }

    fn add_peer(&mut self, user: NetworkUser, lead: bool) -> Result<(), String> {
        let volume = self
            .storage
            .volumes
            .get(&user.user_id)
            .copied()
            .unwrap_or(100)
            .min(400);
        self.send_audio(AudioCommand::NewClient(user.client_id))?;
        self.send_audio(AudioCommand::ChangeClientVolume(user.client_id, volume))?;
        self.tasks.spawn(
            Some(user.client_id),
            attempt_p2p(
                self.network.clone(),
                self.events.clone(),
                self.client_id.unwrap(),
                P2pDestination {
                    client_id: user.client_id,
                    local_udp_addr: user.local_udp_addr,
                    public_udp_addr: user.public_udp_addr,
                },
                lead,
            ),
        );
        self.snapshot.users.push(User {
            client_id: user.client_id,
            user_id: user.user_id,
            name: user.name,
            volume,
            is_current_user: false,
            latency_ms: (user.round_trip_latency + self.round_trip_latency).as_secs_f64() * 500.0,
            is_peer_to_peer: false,
            is_speaking: false,
            speaking_until: None,
        });
        Ok(())
    }

    async fn disconnect(&mut self, error: Option<String>) -> Result<(), String> {
        self.tasks.clear();
        self.snapshot.status = ConnectionStatus::Disconnecting;
        self.publish();
        let result = network::future::disconnect(&self.network, None)
            .await
            .map_err(|e| format!("{e:?}"));
        let audio_result = self.send_audio(AudioCommand::ServerDisconnect);
        self.client_id = None;
        self.snapshot.users.clear();
        self.snapshot.status = ConnectionStatus::Disconnected;
        self.snapshot.error = error;
        result.and(audio_result)
    }

    async fn command(&mut self, command: Command) -> Result<(), String> {
        match command {
            Command::Snapshot => (),
            Command::OpenLink(link) => {
                let server = server_from_link(&link)?;
                if self.snapshot.status != ConnectionStatus::Disconnected {
                    return Err("Disconnect before opening another server link".into());
                }
                self.snapshot.server = server;
                self.snapshot.error = None;
            }
            Command::Connect { username, server } => {
                let (username, server) = validate_connection(&username, &server)?;
                if self.snapshot.status != ConnectionStatus::Disconnected {
                    return Err("A session is already active".into());
                }
                if !self.snapshot.audio_ready {
                    return Err(
                        "Audio is unavailable; check your microphone and output device".into(),
                    );
                }
                self.storage.username = username.clone();
                self.storage.server = server.clone();
                self.snapshot.username = username.clone();
                self.snapshot.server = server.clone();
                self.snapshot.error = None;
                self.snapshot.status = ConnectionStatus::Connecting;
                self.publish();
                if let Err(error) = self.storage.save().await {
                    self.snapshot.error = Some(format!("Could not save preferences: {error}"));
                }
                match network::future::connect(
                    &self.network,
                    server,
                    username.clone(),
                    self.storage.user_id,
                )
                .await
                {
                    Ok((client_id, users, latency)) => {
                        self.client_id = Some(client_id);
                        self.round_trip_latency = latency;
                        self.snapshot.status = ConnectionStatus::Connected;
                        self.send_audio(AudioCommand::ServerConnect)?;
                        let volume = self
                            .storage
                            .volumes
                            .get(&self.storage.user_id)
                            .copied()
                            .unwrap_or(100)
                            .min(400);
                        self.send_audio(AudioCommand::ChangeMainVolume(volume))?;
                        self.snapshot.users.push(User {
                            client_id,
                            user_id: self.storage.user_id,
                            name: username,
                            volume,
                            is_current_user: true,
                            latency_ms: latency.as_secs_f64() * 500.0,
                            is_peer_to_peer: false,
                            is_speaking: false,
                            speaking_until: None,
                        });
                        self.tasks
                            .spawn(None, keep_alive(self.network.clone(), self.events.clone()));
                        for user in users {
                            self.add_peer(user, true)?;
                        }
                    }
                    Err(error) => {
                        self.snapshot.status = ConnectionStatus::Disconnected;
                        return Err(connect_error(error));
                    }
                }
            }
            Command::Disconnect => {
                if self.snapshot.status == ConnectionStatus::Connected {
                    self.disconnect(None).await?;
                }
            }
            Command::Volume { client_id, volume } => {
                if volume > 400 {
                    return Err("Volume must be between 0 and 400".into());
                }
                let user = self
                    .snapshot
                    .users
                    .iter()
                    .find(|u| u.client_id == client_id)
                    .ok_or("User is no longer connected")?;
                self.send_audio(if user.is_current_user {
                    AudioCommand::ChangeMainVolume(volume)
                } else {
                    AudioCommand::ChangeClientVolume(client_id, volume)
                })?;
                let user = self
                    .snapshot
                    .users
                    .iter_mut()
                    .find(|u| u.client_id == client_id)
                    .unwrap();
                user.volume = volume;
                self.storage.volumes.insert(user.user_id, volume);
                self.storage
                    .save()
                    .await
                    .map_err(|e| format!("Could not save volume: {e}"))?;
            }
            Command::ListDevices => {
                self.send_audio(AudioCommand::ListDevices)?;
            }
        }
        Ok(())
    }

    async fn network_event(&mut self, event: NetworkEvent) -> Result<(), String> {
        if self.snapshot.status != ConnectionStatus::Connected {
            return Ok(());
        }
        match event {
            NetworkEvent::SocketClosed => {
                self.disconnect(Some("The server closed the connection".into()))
                    .await?
            }
            NetworkEvent::TimedOut => {
                self.disconnect(Some("The connection timed out".into()))
                    .await?
            }
            NetworkEvent::NetworkError => {
                self.disconnect(Some("The connection failed".into()))
                    .await?
            }
            NetworkEvent::P2pEstablished {
                client_id,
                round_trip_latency,
                is_from_lead_client,
            } => {
                if let Some(user) = self
                    .snapshot
                    .users
                    .iter_mut()
                    .find(|u| u.client_id == client_id)
                {
                    if !user.is_peer_to_peer || is_from_lead_client {
                        user.is_peer_to_peer = true;
                        user.latency_ms = round_trip_latency.as_secs_f64() * 500.0;
                    }
                }
            }
            NetworkEvent::Packet(packet) => match packet.kind() {
                UserJoined::KIND_ID => {
                    let packet: Packet<UserJoined> = packet
                        .finish_parse()
                        .map_err(|e| format!("Invalid join packet: {e:?}"))?;
                    if !self
                        .snapshot
                        .users
                        .iter()
                        .any(|u| u.client_id == packet.body.user.client_id)
                    {
                        self.add_peer(packet.body.user, false)?;
                        self.send_audio(AudioCommand::UserJoined)?;
                    }
                }
                UserLeft::KIND_ID => {
                    let packet: Packet<UserLeft> = packet
                        .finish_parse()
                        .map_err(|e| format!("Invalid leave packet: {e:?}"))?;
                    let id = packet.body.client_id;
                    self.tasks.remove(Some(id));
                    self.send_audio(AudioCommand::DropClient(id))?;
                    self.network
                        .command(NetworkCommand::RemoveP2pClient { client_id: id });
                    self.snapshot.users.retain(|u| u.client_id != id);
                    self.send_audio(AudioCommand::UserLeft)?;
                }
                ErrorResponse::KIND_ID => {
                    let packet: Packet<ErrorResponse> = packet
                        .finish_parse()
                        .map_err(|e| format!("Invalid error packet: {e:?}"))?;
                    self.disconnect(Some(format!("Server error: {:?}", packet.body)))
                        .await?;
                }
                _ => (),
            },
        }
        Ok(())
    }

    fn audio_event(&mut self, event: UICommand) -> bool {
        match event {
            UICommand::AudioIPCInitStatus(status) => {
                match status {
                    AudioIPCInitStatus::Ready => self.snapshot.audio_ready = true,
                    AudioIPCInitStatus::Error(error) => {
                        self.snapshot.audio_ready = false;
                        let message = match error {
                            EventLoopCreationError::InputDeviceError(message) => {
                                format!("Could not initialize microphone: {message}")
                            }
                            EventLoopCreationError::OutputDeviceError(message) => {
                                format!("Could not initialize output: {message}")
                            }
                        };
                        self.snapshot.audio_error = Some(format!("{message}. Check devices and microphone permission, then restart Comms."));
                    }
                }
            }
            UICommand::DevicesFound {
                input_devices,
                output_devices,
            } => {
                let convert = |d: audio_device::audio_connection::DeviceInfo| Device {
                    id: d.id,
                    label: d.label,
                };
                self.snapshot.input_devices = input_devices.into_iter().map(convert).collect();
                self.snapshot.output_devices = output_devices.into_iter().map(convert).collect();
            }
            UICommand::UserIsSpeaking {
                client_id,
                duration,
            } => {
                return self
                    .snapshot
                    .users
                    .iter_mut()
                    .find(|u| u.client_id == client_id)
                    .is_some_and(|user| user.speak_for(duration));
            }
            UICommand::CurrentUserStartedSpeaking | UICommand::CurrentUserStoppedSpeaking => {
                let speaking = matches!(event, UICommand::CurrentUserStartedSpeaking);
                if let Some(user) = self.snapshot.users.iter_mut().find(|u| u.is_current_user) {
                    let changed = user.is_speaking != speaking;
                    user.is_speaking = speaking;
                    return changed;
                }
                return false;
            }
        }
        true
    }
}

fn server_from_link(link: &str) -> Result<String, String> {
    let url = tauri::Url::parse(link).map_err(|_| "Invalid Comms server link")?;
    if url.scheme() != "comms"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || !matches!(url.path(), "" | "/")
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Expected a comms://hostname link without a custom port".into());
    }
    url.host_str()
        .filter(|host| !host.is_empty())
        .map(str::to_string)
        .ok_or_else(|| "The server link needs a hostname".into())
}

fn validate_connection(username: &str, server: &str) -> Result<(String, String), String> {
    let username = username.trim();
    let server = server.trim().trim_start_matches("comms:").trim_matches('/');
    if username.is_empty() || server.is_empty() {
        return Err("Set both username and server".into());
    }
    Ok((username.into(), server.into()))
}

fn connect_error(error: ConnectError) -> String {
    match error {
        ConnectError::DestinationUnusable => "Could not find the server provided".into(),
        ConnectError::NetworkError => "Could not connect to the server".into(),
        ConnectError::AlreadyConnected => "Already connected to a server".into(),
        ConnectError::RequestError(e) => format!("Connection request failed: {e:?}"),
        ConnectError::IncompatibleServerProtocolVersion(version) => format!(
            "Incompatible server protocol ({version}); this client uses {}. Update the older client or server.",
            comms_lib::packet::PROTOCOL_VERSION),
    }
}

pub fn run(
    network: NetworkHandle,
    mut network_events: mpsc::Receiver<NetworkEvent>,
    events: mpsc::Sender<NetworkEvent>,
    ui_events: async_channel::Receiver<UICommand>,
    audio: AudioHandle,
) {
    let storage = LocalStorage::load();
    let server = std::env::args()
        .nth(1)
        .unwrap_or_else(|| storage.server.clone());
    let server = server
        .trim_start_matches("comms:")
        .trim_matches('/')
        .to_string();
    let snapshot = Snapshot {
        revision: 0,
        version: comms_version().into_owned(),
        username: storage.username.clone(),
        server,
        status: ConnectionStatus::Disconnected,
        audio_ready: false,
        audio_error: None,
        error: None,
        users: Vec::new(),
        input_devices: Vec::new(),
        output_devices: Vec::new(),
    };
    let (sender, requests) = async_channel::bounded::<Request>(32);
    tauri::Builder::default()
        .plugin(tauri_plugin_deep_link::init())
        .manage(Bridge(sender))
        .invoke_handler(tauri::generate_handler![
            get_state,
            connect,
            disconnect,
            set_volume,
            list_devices
        ])
        .setup(move |app| {
            let mut snapshot = snapshot;
            if let Some(urls) = app.deep_link().get_current()? {
                if let Some(server) = urls
                    .iter()
                    .find_map(|url| server_from_link(url.as_str()).ok())
                {
                    snapshot.server = server;
                }
            }
            let link_requests = app.state::<Bridge>().0.clone();
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    let (response, _) = oneshot::channel();
                    if let Err(error) = link_requests.try_send(Request {
                        command: Command::OpenLink(url.to_string()),
                        response,
                    }) {
                        eprintln!("Cannot open server link: {error}");
                    }
                }
            });
            let mut session = Session {
                storage,
                snapshot,
                client_id: None,
                round_trip_latency: Duration::ZERO,
                tasks: SessionTasks::default(),
                audio,
                network,
                events,
                app: app.handle().clone(),
            };
            // The actor uses the existing async-std based network futures. No
            // mutex is held over a connection attempt or a filesystem operation.
            async_std::task::spawn(async move {
                let mut audio_open = true;
                enum Input {
                    Request(Result<Request, async_channel::RecvError>),
                    Network(Option<NetworkEvent>),
                    Audio(Option<UICommand>),
                    Tick,
                }
                loop {
                    // Drop all pending receiver futures before processing the
                    // selected message, so transitions can drain stale events.
                    let input = {
                        // Sleep until a remote speaking indicator expires. An
                        // idle or disconnected client needs no periodic timer.
                        let deadline = session
                            .snapshot
                            .users
                            .iter()
                            .filter_map(|user| user.speaking_until)
                            .min();
                        let tick = match deadline {
                            Some(until) => Either::Left(Delay::new(
                                until.saturating_duration_since(Instant::now()),
                            )),
                            None => Either::Right(futures::future::pending::<()>()),
                        }
                        .fuse();
                        let request = requests.recv().fuse();
                        let network_event = network_events.next().fuse();
                        let audio_event = async {
                            if audio_open {
                                ui_events.recv().await.ok()
                            } else {
                                futures::future::pending().await
                            }
                        }
                        .fuse();
                        futures::pin_mut!(request, network_event, audio_event, tick);
                        futures::select! {
                            request = request => Input::Request(request),
                            event = network_event => Input::Network(event),
                            event = audio_event => Input::Audio(event),
                            _ = tick => Input::Tick,
                        }
                    };
                    match input {
                        Input::Request(Ok(request)) => {
                            if matches!(&request.command, Command::Connect { .. })
                                && session.snapshot.status == ConnectionStatus::Disconnected
                            {
                                // A failed or previous session may have queued
                                // socket-close and discovery notifications.
                                while network_events.try_recv().is_ok() {}
                            }
                            let result = session.command(request.command).await;
                            if let Err(error) = &result {
                                session.snapshot.error = Some(error.clone());
                            }
                            session.publish();
                            let _ = request
                                .response
                                .send(result.map(|_| session.snapshot.clone()));
                        }
                        Input::Network(Some(event)) => {
                            if let Err(error) = session.network_event(event).await {
                                let _ = session.disconnect(Some(error)).await;
                            }
                            session.publish();
                        }
                        Input::Audio(Some(event)) => {
                            if session.audio_event(event) {
                                session.publish();
                            }
                        }
                        Input::Audio(None) => {
                            audio_open = false;
                        }
                        Input::Tick => {
                            let now = Instant::now();
                            let mut changed = false;
                            for user in &mut session.snapshot.users {
                                changed |= user.expire_speaking(now);
                            }
                            if changed {
                                session.publish();
                            }
                        }
                        Input::Request(Err(_)) | Input::Network(None) => break,
                    }
                }
            });
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Failed to initialize Comms")
        .run(|app, event| {
            if let tauri::RunEvent::Exit = event {
                // The process owns the native workers; explicitly close the
                // session before the event loop and process terminate.
                let bridge = app.state::<Bridge>();
                let _ = async_std::task::block_on(bridge.request(Command::Disconnect));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_fields_are_validated_and_links_normalized() {
        assert!(validate_connection("  ", "localhost").is_err());
        assert!(validate_connection("Alice", "comms://").is_err());
        assert_eq!(
            validate_connection(" Alice ", " comms://localhost/ ").unwrap(),
            ("Alice".into(), "localhost".into())
        );
    }

    #[test]
    fn server_links_require_a_hostname_and_reject_other_url_components() {
        assert_eq!(
            server_from_link("comms://voice.example.com/").unwrap(),
            "voice.example.com"
        );
        for link in [
            "https://example.com",
            "comms://",
            "comms://user@host",
            "comms://host:1234",
            "comms://host/path",
            "comms://host?server=other",
        ] {
            assert!(server_from_link(link).is_err(), "{link}");
        }
    }

    #[test]
    fn gtk_settings_preserve_identity_and_volume() {
        let id = Uuid::new_v4();
        let raw = format!(
            r#"{{"username":"Alice","server":"localhost","user_id":"{id}","volumes":{{"{id}":175}}}}"#
        );
        let storage: LocalStorage = serde_json::from_str(&raw).unwrap();
        assert_eq!(storage.user_id, id);
        assert_eq!(storage.volumes[&id], 175);
        assert_eq!(
            serde_json::to_value(&storage).unwrap(),
            serde_json::from_str::<serde_json::Value>(&raw).unwrap()
        );
    }

    #[test]
    fn speaking_expires_and_new_audio_extends_the_indicator() {
        let mut user = User {
            client_id: 1,
            user_id: Uuid::new_v4(),
            name: "Alice".into(),
            volume: 100,
            is_current_user: false,
            latency_ms: 12.0,
            is_peer_to_peer: false,
            is_speaking: false,
            speaking_until: None,
        };
        assert!(user.speak_for(Duration::from_millis(100)));
        assert!(!user.speak_for(Duration::from_millis(200)));
        assert!(!user.expire_speaking(Instant::now() + Duration::from_millis(150)));
        assert!(user.is_speaking);
        assert!(user.expire_speaking(Instant::now() + Duration::from_millis(300)));
        assert!(!user.is_speaking);
    }

    #[test]
    fn session_task_cancellation_stops_pending_work() {
        async_std::task::block_on(async {
            let (release, wait) = oneshot::channel::<()>();
            let (result, received) = oneshot::channel();
            let mut tasks = SessionTasks::default();
            tasks.spawn(Some(1), async move {
                let _ = wait.await;
                let _ = result.send("stale peer registration");
            });
            tasks.clear();
            let _ = release.send(());
            assert!(received.await.is_err());
            assert!(tasks.0.is_empty());
        });
    }

    #[test]
    fn removing_a_peer_cancels_only_its_discovery() {
        async_std::task::block_on(async {
            let (release, wait) = oneshot::channel::<()>();
            let (result, received) = oneshot::channel();
            let mut tasks = SessionTasks::default();
            tasks.spawn(Some(1), async move {
                let _ = wait.await;
                let _ = result.send(());
            });
            tasks.spawn(None, futures::future::pending());
            tasks.remove(Some(1));
            let _ = release.send(());
            assert!(received.await.is_err());
            assert!(tasks.0.contains_key(&None));
        });
    }
}
