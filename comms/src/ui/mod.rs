use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use directories::ProjectDirs;
use futures::channel::mpsc::Sender;
use futures::stream::StreamExt;
use gio::prelude::*;
use gio::ApplicationFlags;
use glib::object::ObjectRef;
use gtk::prelude::*;
use gtk::{CellRendererText, Orientation, PositionType, TreeView, TreeViewColumn};
use serde::{Deserialize, Serialize};

use comms_lib::packet::{ClientId, Packet, PacketBody, UserJoined, UserLeft};

use crate::audio::ipc::{AudioCommand, AudioHandle};
use crate::network;
use crate::network::future::{attempt_p2p, P2pDestination};
use crate::network::ipc::{ConnectError, NetworkCommand, NetworkEvent, NetworkHandle};
use crate::ui::ipc::UICommand;
use comms_lib::network::CommsNetworkError;
use glib::timeout_add_local;

pub mod ipc;

#[derive(Debug)]
#[repr(i32)]
enum Columns {
    Id = 0,
    Username = 1,
    Volume = 2,
    IsCurrentUser = 3,
    NetworkLatency = 4,
    IsPeerToPeer = 5,
}

fn initialize_user_list(treeview: &TreeView) -> gtk::ListStore {
    let col_types = [
        glib::Type::U32,
        glib::Type::STRING,
        glib::Type::U32,
        glib::Type::BOOL,
        glib::Type::STRING,
        glib::Type::BOOL,
    ];

    let user_list_model = gtk::ListStore::new(&col_types);
    treeview.set_model(Some(&user_list_model));
    user_list_model
}

#[derive(Serialize, Deserialize)]
struct LocalStorage {
    username: String,
    server: String,
}

async fn try_connect(
    audio_handle: AudioHandle,
    network_handle: NetworkHandle,
    network_event_sender: Sender<NetworkEvent>,
    ui: Rc<RefCell<UiModel>>,
) {
    let old_state = ui.borrow().state.clone();
    if let UiState::ChooseServer = old_state {
        let builder = ui.borrow().builder.clone();
        let username_entry: gtk::Entry = builder.object("username_entry").unwrap();
        let server_entry: gtk::Entry = builder.object("server_entry").unwrap();
        let connect_button: gtk::Button = builder.object("connect_button").unwrap();
        let connect_progress_stack: gtk::Stack = builder.object("connect_progress_stack").unwrap();
        let connect_progress: gtk::ProgressBar = builder.object("connect_progress").unwrap();
        let connect_failure_status: gtk::Label = builder.object("connect_failure_status").unwrap();

        let username: String = username_entry.buffer().text();
        let server: String = server_entry.buffer().text();
        if username.is_empty() || server.is_empty() {
            connect_failure_status.set_text("Set both username and server");
            return;
        } else {
            connect_failure_status.set_text("");
        }

        ui.borrow_mut().state = UiState::Connecting;
        connect_button.set_sensitive(false);
        connect_progress_stack.set_visible_child_name("progress");

        let finished = Rc::new(Cell::new(false));
        {
            let finished = finished.clone();
            timeout_add_local(Duration::from_millis(80), move || {
                if !finished.get() {
                    connect_progress.pulse();
                    Continue(true)
                } else {
                    Continue(false)
                }
            });
        }

        if let Some((project_dirs, storage)) =
            ProjectDirs::from("", "Comms", "Comms").and_then(|project_dirs| {
                let local_storage = LocalStorage {
                    username: username.clone(),
                    server: server.clone(),
                };
                serde_json::to_string(&local_storage)
                    .ok()
                    .map(|storage| (project_dirs, storage))
            })
        {
            let _ = async_std::fs::create_dir_all(project_dirs.data_dir()).await;
            let _ =
                async_std::fs::write(project_dirs.data_dir().join("local_storage"), storage).await;
        }

        let connect_result =
            network::future::connect(&network_handle, server, username.clone()).await;

        finished.replace(true);
        connect_progress_stack.set_visible_child_name("hidden_progress");
        connect_button.set_sensitive(true);

        match connect_result {
            Ok((client_id, users, round_trip_latency)) => {
                let glib_context = glib::MainContext::default();

                for user in users.iter() {
                    audio_handle
                        .send(AudioCommand::NewClient(user.client_id))
                        .unwrap();

                    glib_context.spawn_local(attempt_p2p(
                        network_handle.clone(),
                        network_event_sender.clone(),
                        client_id,
                        P2pDestination {
                            client_id: user.client_id,
                            local_udp_addr: user.local_udp_addr,
                            public_udp_addr: user.public_udp_addr,
                        },
                        true,
                    ));
                }

                let mut ui = ui.borrow_mut();
                let status_treeview: TreeView = ui.builder.object("status_treeview").unwrap();
                let network_latency_column_ref = ui.network_latency_column_ref.clone();
                let user_list_model = initialize_user_list(&status_treeview);

                status_treeview.set_property("has-tooltip", true);
                {
                    let user_list_model = user_list_model.clone();
                    status_treeview.connect_query_tooltip(
                        move |treeview, x, y, keyboard_mode, tooltip| {
                            let (path, column) = if keyboard_mode {
                                treeview.cursor()
                            } else {
                                let (bin_x, bin_y) =
                                    treeview.convert_widget_to_bin_window_coords(x, y);
                                if let Some(r) = treeview.path_at_pos(bin_x, bin_y) {
                                    let (path, column, _, _) = r;
                                    (path, column)
                                } else {
                                    return false;
                                }
                            };

                            let (path, column) = if let (Some(path), Some(column)) = (path, column)
                            {
                                (path, column)
                            } else {
                                return false;
                            };

                            if let Some(network_latency_column_ref) =
                                network_latency_column_ref.as_ref()
                            {
                                if network_latency_column_ref != column.as_object_ref() {
                                    return false;
                                }
                            } else {
                                return false;
                            }

                            let iter = if let Some(iter) = user_list_model.iter(&path) {
                                iter
                            } else {
                                return false;
                            };

                            if let (
                                Ok(is_peer_to_peer),
                                Ok(is_current_user),
                                Ok(one_way_latency),
                            ) = (
                                user_list_model
                                    .value(&iter, Columns::IsPeerToPeer as i32)
                                    .get::<bool>(),
                                user_list_model
                                    .value(&iter, Columns::IsCurrentUser as i32)
                                    .get::<bool>(),
                                user_list_model
                                    .value(&iter, Columns::NetworkLatency as i32)
                                    .get::<String>(),
                            ) {
                                let text = match (is_peer_to_peer, is_current_user) {
                                    (_, true) => {
                                        format!("Latency to server is {}", one_way_latency)
                                    }
                                    (true, _) => format!(
                                        "Latency to other client (direct) is {}",
                                        one_way_latency
                                    ),
                                    (false, _) => format!(
                                        "Latency to other client (via server) is {}",
                                        one_way_latency
                                    ),
                                };
                                tooltip.set_text(Some(&text));
                                treeview.set_tooltip_cell(
                                    tooltip,
                                    Some(&path),
                                    Some(&column),
                                    None as Option<&CellRendererText>,
                                );
                                true
                            } else {
                                false
                            }
                        },
                    );
                }

                let values: [(u32, &dyn ToValue); 6] = [
                    (Columns::Id as u32, &(client_id as u32)),
                    (Columns::Username as u32, &username),
                    (Columns::Volume as u32, &100),
                    (Columns::IsCurrentUser as u32, &true),
                    (
                        Columns::NetworkLatency as u32,
                        &format!(
                            "{:.1}ms",
                            (round_trip_latency.as_micros() as f64) / 1000.0 / 2.0
                        ),
                    ),
                    (Columns::IsPeerToPeer as u32, &false),
                ];
                user_list_model.insert_with_values(None, &values);
                for user in users.iter() {
                    let one_way_latency =
                        user.round_trip_latency.saturating_add(round_trip_latency) / 2;

                    let values: [(u32, &dyn ToValue); 5] = [
                        (Columns::Id as u32, &(user.client_id as u32)),
                        (Columns::Username as u32, &user.name),
                        (Columns::Volume as u32, &100),
                        (
                            Columns::NetworkLatency as u32,
                            &format!("{:.1}ms", (one_way_latency.as_micros() as f64) / 1000.0),
                        ),
                        (Columns::IsPeerToPeer as u32, &false),
                    ];
                    user_list_model.insert_with_values(None, &values);
                }

                ui.pane_stack
                    .set_transition_type(gtk::StackTransitionType::SlideUp);
                ui.pane_stack.set_visible_child_name("status");
                ui.state = UiState::Main {
                    user_list_model,
                    client_id,
                    round_trip_latency,
                };
            }
            Err(e) => {
                let error_message = match e {
                    ConnectError::DestinationUnusable => {
                        "Could not find the server provided".to_string()
                    }
                    ConnectError::NetworkError => "Could not connect to the server".to_string(),
                    // AlreadyConnected shouldn't be possible, but it's useful for end-to-end testing network futures right now.
                    ConnectError::AlreadyConnected => {
                        "Can't connect because you're already connected to a server".to_string()
                    }
                    ConnectError::RequestError(e) => format!("Request failed: {:?}", e),
                };
                connect_failure_status.set_text(&error_message);
                ui.borrow_mut().state = UiState::ChooseServer;
            }
        }
    }
}

async fn disconnect_from_server(
    ui: Rc<RefCell<UiModel>>,
    handle: NetworkHandle,
    audio: AudioHandle,
) {
    let state = ui.borrow().state.clone();
    if let UiState::Main {
        user_list_model, ..
    } = state
    {
        network::future::disconnect(&handle).await;
        {
            let pane_stack = &ui.borrow().pane_stack;
            pane_stack.set_transition_type(gtk::StackTransitionType::SlideDown);
            pane_stack.set_visible_child_name("connect");
        }
        user_list_model.clear();
        audio.send(AudioCommand::ServerDisconnect).unwrap();
        ui.borrow_mut().state = UiState::ChooseServer;
    }
}

#[derive(Clone, Debug)]
enum UiState {
    ChooseServer,
    Main {
        user_list_model: gtk::ListStore,
        client_id: ClientId,
        round_trip_latency: Duration,
    },
    Connecting,
}

struct UiModel {
    state: UiState,
    pane_stack: gtk::Stack,
    builder: gtk::Builder,
    network_latency_column_ref: Option<ObjectRef>,
}

async fn handle_network_events(
    mut receiver: futures::channel::mpsc::Receiver<NetworkEvent>,
    network_event_sender: Sender<NetworkEvent>,
    audio: AudioHandle,
    network: NetworkHandle,
    ui: Rc<RefCell<UiModel>>,
) {
    while let Some(event) = receiver.next().await {
        let state = ui.borrow().state.clone();
        match event {
            NetworkEvent::SocketClosed => {
                if let UiState::Main { .. } = state {
                    disconnect_from_server(ui.clone(), network.clone(), audio.clone()).await;

                    let builder = &ui.borrow().builder;
                    let connect_failure_status: gtk::Label =
                        builder.object("connect_failure_status").unwrap();
                    connect_failure_status
                        .set_text("Disconnecting from the server because the socket closed");
                }
            }
            NetworkEvent::NetworkError => {
                if let UiState::Main { .. } = state {
                    disconnect_from_server(ui.clone(), network.clone(), audio.clone()).await;

                    let builder = &ui.borrow().builder;
                    let connect_failure_status: gtk::Label =
                        builder.object("connect_failure_status").unwrap();
                    connect_failure_status
                        .set_text("Disconnecting from the server because of a network issue");
                }
            }
            NetworkEvent::P2pEstablished {
                round_trip_latency,
                client_id,
                is_from_lead_client,
            } => {
                if let UiState::Main {
                    user_list_model, ..
                } = ui.borrow().state.clone()
                {
                    let iter = user_list_model.iter_first().unwrap();
                    let iter = loop {
                        if let Ok(row_client_id) = user_list_model
                            .value(&iter, Columns::Id as i32)
                            .get::<u32>()
                        {
                            if row_client_id == client_id as u32 {
                                break Some(iter);
                            }
                        }

                        if !user_list_model.iter_next(&iter) {
                            break None;
                        }
                    };

                    if let Some(iter) = iter {
                        if let Ok(is_already_peer_to_peer) = user_list_model
                            .value(&iter, Columns::IsPeerToPeer as i32)
                            .get::<bool>()
                        {
                            if !is_already_peer_to_peer || is_from_lead_client {
                                user_list_model.set(
                                    &iter,
                                    &[
                                        (
                                            Columns::NetworkLatency as u32,
                                            &format!(
                                                "{:.1}ms 🔗",
                                                (round_trip_latency.as_micros() as f64)
                                                    / 1000.0
                                                    / 2.0
                                            ),
                                        ),
                                        (Columns::IsPeerToPeer as u32, &true),
                                    ],
                                );
                            }
                        }
                    }
                }
            }
            NetworkEvent::Packet(packet) => {
                if let UiState::Main {
                    user_list_model,
                    client_id,
                    round_trip_latency,
                } = ui.borrow().state.clone()
                {
                    let result: Result<(), CommsNetworkError> = (|| {
                        match packet.kind() {
                            UserJoined::KIND_ID => {
                                let packet: Packet<UserJoined> = packet.finish_parse()?;
                                audio
                                    .send(AudioCommand::NewClient(packet.body.user.client_id))
                                    .unwrap();
                                let glib_context = glib::MainContext::default();
                                glib_context.spawn_local(attempt_p2p(
                                    network.clone(),
                                    network_event_sender.clone(),
                                    client_id,
                                    P2pDestination {
                                        client_id: packet.body.user.client_id,
                                        local_udp_addr: packet.body.user.local_udp_addr,
                                        public_udp_addr: packet.body.user.public_udp_addr,
                                    },
                                    false,
                                ));

                                let one_way_latency = packet
                                    .body
                                    .user
                                    .round_trip_latency
                                    .saturating_add(round_trip_latency)
                                    / 2;

                                user_list_model.set(
                                    &user_list_model.append(),
                                    &[
                                        (Columns::Id as u32, &(packet.body.user.client_id as u32)),
                                        (Columns::Username as u32, &packet.body.user.name),
                                        (Columns::Volume as u32, &100),
                                        (
                                            Columns::NetworkLatency as u32,
                                            &format!(
                                                "{:.1}ms",
                                                (one_way_latency.as_micros() as f64) / 1000.0
                                            ),
                                        ),
                                        (Columns::IsPeerToPeer as u32, &false),
                                    ],
                                );
                            }
                            UserLeft::KIND_ID => {
                                let packet: Packet<UserLeft> = packet.finish_parse()?;
                                audio
                                    .send(AudioCommand::DropClient(packet.body.client_id))
                                    .unwrap();

                                network.command(NetworkCommand::RemoveP2pClient {
                                    client_id: packet.body.client_id,
                                });

                                let id = packet.body.client_id as u32;
                                let iter = user_list_model.iter_first().unwrap();
                                loop {
                                    let value = user_list_model
                                        .value(&iter, Columns::Id as i32)
                                        .get::<u32>()
                                        .unwrap();
                                    if value == id {
                                        user_list_model.remove(&iter);
                                        break;
                                    }
                                    if !user_list_model.iter_next(&iter) {
                                        println!("Couldn't find user to remove");
                                        break;
                                    }
                                }
                            }
                            _ => (),
                        }

                        Ok(())
                    })();
                    if result.is_err() {
                        let builder = &ui.borrow().builder;
                        let connect_failure_status: gtk::Label =
                            builder.object("connect_failure_status").unwrap();
                        connect_failure_status
                            .set_text("Disconnecting from the server because of a network issue");
                    }
                }
            }
        }
    }
}

fn gtk_start(f: impl FnOnce(&gtk::Application) + 'static) {
    let application =
        gtk::Application::new(Some("ca.mitchhentges.comms"), ApplicationFlags::NON_UNIQUE);

    let o = RefCell::new(Some(f));
    application.connect_startup(move |a| {
        let mut o = o.borrow_mut();
        if let Some(f) = o.take() {
            f(a);
        }
    });
    application.connect_activate(|_| {});
    application.run();
}

static RESOURCES_DIR: include_dir::Dir = include_dir::include_dir!("$CARGO_MANIFEST_DIR/resources");

pub fn run(
    network: NetworkHandle,
    network_event_receiver: futures::channel::mpsc::Receiver<NetworkEvent>,
    network_event_sender: Sender<NetworkEvent>,
    ui_receiver: std::sync::mpsc::Receiver<UICommand>,
    audio: AudioHandle,
) {
    #[cfg(not(debug_assertions))]
    {
        let self_path = std::env::current_exe().unwrap();
        let pixbuf_module_path = self_path.parent().unwrap().join("loaders.cache");
        std::env::set_var("GDK_PIXBUF_MODULE_FILE", pixbuf_module_path.as_os_str());
    }

    let command = ui_receiver.recv().unwrap();
    match command {
        UICommand::AudioIPCReady => (),
        _ => panic!("Expected first UICommand to be 'AudioIPCReady'"),
    };
    audio.send(AudioCommand::ListDevices).unwrap();
    let ui_cmd_from_audio: UICommand = ui_receiver.recv().unwrap();
    let (input_devices, output_devices) = match ui_cmd_from_audio {
        UICommand::DevicesFound {
            input_devices,
            output_devices,
        } => (input_devices, output_devices),
        _ => panic!("Expected second UICommand to be 'DevicesFound'"),
    };

    let initial_local_storage: Option<LocalStorage> = ProjectDirs::from("", "Comms", "Comms")
        .and_then(|project_dirs| {
            std::fs::read_to_string(project_dirs.data_dir().join("local_storage")).ok()
        })
        .and_then(|storage_string| serde_json::from_str(&storage_string).ok());

    gtk_start(move |app| {
        let stack = gtk::Stack::new();
        stack.set_visible(true);
        let raw_app_ui = RESOURCES_DIR
            .get_file("app.ui")
            .unwrap()
            .contents_utf8()
            .unwrap();
        let builder = gtk::Builder::from_string(raw_app_ui);
        let ui = Rc::new(RefCell::new(UiModel {
            state: UiState::ChooseServer,
            pane_stack: stack.clone(),
            builder: builder.clone(),
            network_latency_column_ref: None,
        }));

        let glib_context = glib::MainContext::default();
        glib_context.spawn_local(handle_network_events(
            network_event_receiver,
            network_event_sender.clone(),
            audio.clone(),
            network.clone(),
            ui.clone(),
        ));

        let create_icon = |name| {
            let stream =
                gio::ReadInputStream::new(RESOURCES_DIR.get_file(name).unwrap().contents());
            gdk_pixbuf::Pixbuf::from_stream(&stream, None as Option<&gio::Cancellable>).unwrap()
        };

        let window = gtk::ApplicationWindow::new(app);
        window.set_title("Comms");
        window.set_default_size(280, 250);
        window.set_position(gtk::WindowPosition::Center);
        window.set_icon_list(&[
            create_icon("logo-16.png"),
            create_icon("logo-32.png"),
            create_icon("logo-48.png"),
            create_icon("logo-256.png"),
        ]);

        stack.set_transition_duration(300);

        // --- Setup for connect-to-server state

        let username_entry: gtk::Entry = builder.object("username_entry").unwrap();

        if let Some(initial_storage) = initial_local_storage {
            let server_entry: gtk::Entry = builder.object("server_entry").unwrap();
            server_entry.set_text(&initial_storage.server);
            username_entry.set_text(&initial_storage.username);
        } else if let Some(val) = std::env::var_os(if cfg!(windows) { "USERNAME" } else { "USER" })
        {
            username_entry.set_text(&val.to_string_lossy().to_string());
        }

        let submit_connection_form = {
            let glib_context = glib_context.clone();
            let network = network.clone();
            let audio = audio.clone();
            let ui = ui.clone();
            Rc::new(RefCell::new(move || {
                glib_context.spawn_local(try_connect(
                    audio.clone(),
                    network.clone(),
                    network_event_sender.clone(),
                    ui.clone(),
                ));
            }))
        };

        let connect_button: gtk::Button = builder.object("connect_button").unwrap();
        {
            let submit_connection_form = submit_connection_form.clone();
            connect_button.connect_clicked(move |_| {
                let submit_fn = &mut *submit_connection_form.borrow_mut();
                submit_fn();
            });
        }

        let username_entry: gtk::Entry = builder.object("username_entry").unwrap();
        {
            let submit_connection_form = submit_connection_form.clone();
            username_entry.connect_activate(move |_| {
                let submit_fn = &mut *submit_connection_form.borrow_mut();
                submit_fn();
            });
        }

        let server_entry: gtk::Entry = builder.object("server_entry").unwrap();
        server_entry.connect_activate(move |_| {
            let submit_fn = &mut *submit_connection_form.borrow_mut();
            submit_fn();
        });
        let connect_form: gtk::Box = builder.object("connect_form").unwrap();
        stack.add_named(&connect_form, "connect");

        // --- Setup for main state

        let treeview: gtk::TreeView = builder.object("status_treeview").unwrap();
        let username_column = TreeViewColumn::with_attributes(
            "Users",
            &gtk::CellRendererText::new(),
            &[("text", Columns::Username as i32)],
        );
        username_column.set_property("expand", true);
        treeview.append_column(&username_column);

        let network_latency_column = TreeViewColumn::with_attributes(
            "Network Latency",
            &gtk::CellRendererText::builder().xalign(1.0).build(),
            &[("text", Columns::NetworkLatency as i32)],
        );
        treeview.append_column(&network_latency_column);
        {
            ui.borrow_mut().network_latency_column_ref =
                Some(network_latency_column.as_object_ref().clone());
        }

        {
            let audio = audio.clone();
            let ui = ui.clone();

            treeview.connect_button_press_event(move |treeview, event| {
                if event.event_type() == gtk::gdk::EventType::ButtonPress && event.button() == 3 {
                    let position = (event.position().0 as i32, event.position().1 as i32);
                    if let Some(user) = treeview.path_at_pos(position.0, position.1) {
                        let model = treeview.model().unwrap();
                        let iter = model.iter(user.0.as_ref().unwrap()).unwrap();

                        let id = model.value(&iter, Columns::Id as i32).get::<u32>().unwrap();

                        let volume = model
                            .value(&iter, Columns::Volume as i32)
                            .get::<u32>()
                            .unwrap();
                        let is_current_user = model
                            .value(&iter, Columns::IsCurrentUser as i32)
                            .get::<bool>()
                            .unwrap();

                        let popover = gtk::Popover::new(Some(treeview));
                        let popover_box = gtk::Box::new(gtk::Orientation::Vertical, 0);

                        let adjustment =
                            gtk::Adjustment::new(volume as f64, 0.0, 400.0, 2.0, 10.0, 0.0);
                        let scale = gtk::builders::ScaleBuilder::new()
                            .adjustment(&adjustment)
                            .value_pos(PositionType::Left)
                            .orientation(Orientation::Horizontal)
                            .build();

                        scale.add_mark(100.0, gtk::PositionType::Bottom, None);
                        scale.add_mark(200.0, gtk::PositionType::Bottom, None);
                        scale.add_mark(300.0, gtk::PositionType::Bottom, None);

                        scale.set_width_request(200);
                        popover_box.add(&scale);

                        let audio = audio.clone();
                        let ui = ui.clone();
                        scale.connect_value_changed(move |scale| {
                            let new_volume = scale.adjustment().value() as u32;
                            audio
                                .send(if is_current_user {
                                    AudioCommand::ChangeMainVolume(new_volume)
                                } else {
                                    AudioCommand::ChangeClientVolume(id as ClientId, new_volume)
                                })
                                .unwrap();
                            let state = &ui.borrow().state;
                            if let UiState::Main {
                                user_list_model, ..
                            } = state
                            {
                                let values: [(u32, &dyn ToValue); 1] =
                                    [(Columns::Volume as u32, &new_volume)];
                                user_list_model.set(&iter, &values);
                            }
                        });

                        let rectangle = gtk::Rectangle::new(position.0, position.1, 0, 0);
                        popover.set_pointing_to(&rectangle);
                        popover.add(&popover_box);
                        popover.popup();
                        popover.show_all();
                    }
                }
                Inhibit(false)
            });
        }

        let disconnect_menu_item: gtk::MenuItem = builder.object("disconnect_menu_item").unwrap();
        disconnect_menu_item.connect_activate(move |_| {
            glib_context.spawn_local(disconnect_from_server(
                ui.clone(),
                network.clone(),
                audio.clone(),
            ));
        });

        let inputs_submenu: gtk::Menu = builder.object("inputs_submenu").unwrap();
        let outputs_submenu: gtk::Menu = builder.object("outputs_submenu").unwrap();

        for (device_count, device) in input_devices.into_iter().enumerate() {
            let input_menu_item: gtk::MenuItem = gtk::builders::MenuItemBuilder::new()
                .label(&device.label)
                .build();
            input_menu_item.connect_activate(|_| {
                //send command switch device
            });
            let device_count = device_count as u32;
            inputs_submenu.attach(&input_menu_item, 0, 1, device_count, device_count + 1);
        }

        for (device_count, device) in output_devices.into_iter().enumerate() {
            let output_menu_item: gtk::MenuItem = gtk::builders::MenuItemBuilder::new()
                .label(&device.label)
                .build();
            output_menu_item.connect_activate(|_| {
                println!("Clicked output menu item button.");
            });
            let device_count = device_count as u32;
            outputs_submenu.attach(&output_menu_item, 0, 1, device_count, device_count + 1);
        }

        let status_box: gtk::Box = builder.object("status_box").unwrap();
        stack.add_named(&status_box, "status");

        window.add(&stack);
        window.show();
    });
}
