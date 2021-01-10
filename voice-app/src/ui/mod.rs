use std::cell::{Cell, RefCell};
use std::rc::Rc;

use directories::ProjectDirs;
use futures::stream::StreamExt;
use gio::prelude::*;
use gtk::prelude::*;
use serde::{Deserialize, Serialize};

use voice_lib::packet::{Packet, PacketBody, UserJoined, UserLeft};

use crate::audio::ipc::{AudioCommand, AudioHandle};
use crate::network;
use crate::network::ipc::{ConnectError, NetworkEvent, NetworkHandle};
use crate::ui::ipc::UICommand;
use glib::timeout_add_local;
use voice_lib::network::CommsNetworkError;

pub mod ipc;

#[derive(Debug)]
#[repr(i32)]
enum Columns {
    Id = 0,
    Username = 1,
}

fn initialize_user_list(treeview: &gtk::TreeView) -> gtk::ListStore {
    let col_types: [glib::Type; 2] = [glib::Type::U32, glib::Type::String];

    let user_list_model = gtk::ListStore::new(&col_types);
    treeview.set_model(Some(&user_list_model));
    user_list_model
}

#[derive(Serialize, Deserialize)]
struct LocalStorage {
    username: String,
    server: String,
}

async fn try_connect(audio_handle: AudioHandle, network: NetworkHandle, ui: Rc<RefCell<UiModel>>) {
    let old_state = ui.borrow().state.clone();
    if let UiState::ChooseServer = old_state {
        let builder = ui.borrow().builder.clone();
        let username_entry: gtk::Entry = builder.get_object("username_entry").unwrap();
        let server_entry: gtk::Entry = builder.get_object("server_entry").unwrap();
        let connect_button: gtk::Button = builder.get_object("connect_button").unwrap();
        let connect_progress_stack: gtk::Stack =
            builder.get_object("connect_progress_stack").unwrap();
        let connect_progress: gtk::ProgressBar = builder.get_object("connect_progress").unwrap();
        let connect_failure_status: gtk::Label =
            builder.get_object("connect_failure_status").unwrap();

        let username: String = username_entry.get_buffer().get_text();
        let server: String = server_entry.get_buffer().get_text();
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
            timeout_add_local(80, move || {
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
            let _ = async_std::fs::create_dir(project_dirs.data_dir()).await;
            let _ =
                async_std::fs::write(project_dirs.data_dir().join("local_storage"), storage).await;
        }

        let connect_result = network::future::connect(&network, server, username).await;

        finished.replace(true);
        connect_progress_stack.set_visible_child_name("hidden_progress");
        connect_button.set_sensitive(true);

        match connect_result {
            Ok(users) => {
                for (client_id, _) in users.iter() {
                    audio_handle
                        .send(AudioCommand::NewClient(*client_id))
                        .unwrap()
                }

                let col_indices: [u32; 2] = [Columns::Id as u32, Columns::Username as u32];

                let mut ui = ui.borrow_mut();
                let status_treeview = ui.builder.get_object("status_treeview").unwrap();
                let user_list_model = initialize_user_list(&status_treeview);
                for (id, user) in users.iter() {
                    let values: [&dyn ToValue; 2] = [&(*id as u32), &user];
                    user_list_model.insert_with_values(None, &col_indices, &values);
                }

                ui.pane_stack
                    .set_transition_type(gtk::StackTransitionType::SlideUp);
                ui.pane_stack.set_visible_child_name("status");
                ui.state = UiState::Main { user_list_model };
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

async fn disconnect_from_server(ui: Rc<RefCell<UiModel>>, handle: NetworkHandle) {
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
        ui.borrow_mut().state = UiState::ChooseServer;
    }
}

#[derive(Clone, Debug)]
enum UiState {
    ChooseServer,
    Main { user_list_model: gtk::ListStore },
    Connecting,
}

struct UiModel {
    state: UiState,
    pane_stack: gtk::Stack,
    builder: gtk::Builder,
}

async fn handle_network_events(
    mut receiver: futures::channel::mpsc::Receiver<NetworkEvent>,
    audio_handle: AudioHandle,
    network: NetworkHandle,
    ui: Rc<RefCell<UiModel>>,
) {
    while let Some(event) = receiver.next().await {
        let state = ui.borrow().state.clone();
        match event {
            NetworkEvent::SocketClosed => {
                if let UiState::Main { .. } = state {
                    disconnect_from_server(ui.clone(), network.clone()).await;

                    let builder = &ui.borrow().builder;
                    let connect_failure_status: gtk::Label =
                        builder.get_object("connect_failure_status").unwrap();
                    connect_failure_status
                        .set_text("Disconnecting from the server because the socket closed");
                }
            }
            NetworkEvent::NetworkError => {
                if let UiState::Main { .. } = state {
                    disconnect_from_server(ui.clone(), network.clone()).await;

                    let builder = &ui.borrow().builder;
                    let connect_failure_status: gtk::Label =
                        builder.get_object("connect_failure_status").unwrap();
                    connect_failure_status
                        .set_text("Disconnecting from the server because of a network issue");
                }
            }
            NetworkEvent::Packet(packet) => {
                if let UiState::Main { user_list_model } = ui.borrow().state.clone() {
                    let result: Result<(), CommsNetworkError> = (|| {
                        match packet.kind() {
                            UserJoined::KIND_ID => {
                                let packet: Packet<UserJoined> = packet.finish_parse()?;
                                audio_handle
                                    .send(AudioCommand::NewClient(packet.body.client_id))
                                    .unwrap();
                                let col_indices: [u32; 2] =
                                    [Columns::Id as u32, Columns::Username as u32];
                                user_list_model.set(
                                    &user_list_model.append(),
                                    &col_indices,
                                    &[&(packet.body.client_id as u32), &packet.body.name],
                                );
                            }
                            UserLeft::KIND_ID => {
                                let packet: Packet<UserLeft> = packet.finish_parse()?;
                                let id = packet.body.client_id as u32;
                                let iter = user_list_model.get_iter_first().unwrap();
                                loop {
                                    let value = user_list_model
                                        .get_value(&iter, Columns::Id as i32)
                                        .get_some::<u32>()
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
                            builder.get_object("connect_failure_status").unwrap();
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
        gtk::Application::new(Some("ca.mitchhentges.comms"), Default::default()).unwrap();

    let o = RefCell::new(Some(f));
    application.connect_startup(move |a| {
        let mut o = o.borrow_mut();
        if let Some(f) = o.take() {
            f(a);
        }
    });
    application.connect_activate(|_| {});
    application.run(&std::env::args().collect::<Vec<_>>());
}

pub fn run(
    network: NetworkHandle,
    network_event_receiver: futures::channel::mpsc::Receiver<NetworkEvent>,
    ui_receiver: std::sync::mpsc::Receiver<UICommand>,
    audio_handle: AudioHandle,
) {
    let command = ui_receiver.recv().unwrap();
    match command {
        UICommand::AudioIPCReady => (),
        _ => panic!("Expected first UICommand to be 'AudioIPCReady'"),
    };
    audio_handle.send(AudioCommand::ListDevices).unwrap();
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
        let builder = gtk::Builder::from_file("resources/app.ui");
        let ui = Rc::new(RefCell::new(UiModel {
            state: UiState::ChooseServer,
            pane_stack: stack.clone(),
            builder: builder.clone(),
        }));

        let glib_context = glib::MainContext::default();
        glib_context.spawn_local(handle_network_events(
            network_event_receiver,
            audio_handle.clone(),
            network.clone(),
            ui.clone(),
        ));

        let logo_16 = gdk_pixbuf::Pixbuf::from_file("resources/logo-16.png").unwrap();
        let logo_32 = gdk_pixbuf::Pixbuf::from_file("resources/logo-32.png").unwrap();
        let logo_48 = gdk_pixbuf::Pixbuf::from_file("resources/logo-48.png").unwrap();
        let logo_256 = gdk_pixbuf::Pixbuf::from_file("resources/logo-256.png").unwrap();

        let window = gtk::ApplicationWindow::new(app);
        window.set_title("Comms");
        window.set_default_size(280, 250);
        window.set_position(gtk::WindowPosition::Center);
        window.set_icon_list(&[logo_16, logo_32, logo_48, logo_256]);

        stack.set_transition_duration(300);

        // --- Setup for connect-to-server state
        if let Some(initial_storage) = initial_local_storage {
            let username_entry: gtk::Entry = builder.get_object("username_entry").unwrap();
            username_entry.set_text(&initial_storage.username);
            let server_entry: gtk::Entry = builder.get_object("server_entry").unwrap();
            server_entry.set_text(&initial_storage.server);
        }

        let submit_connection_form = {
            // TODO use clone! macro?
            let glib_context = glib_context.clone();
            let network = network.clone();
            let ui = ui.clone();
            Rc::new(RefCell::new(move || {
                glib_context.spawn_local(try_connect(
                    audio_handle.clone(),
                    network.clone(),
                    ui.clone(),
                ));
            }))
        };

        let connect_button: gtk::Button = builder.get_object("connect_button").unwrap();
        {
            let submit_connection_form = submit_connection_form.clone();
            connect_button.connect_clicked(move |_| {
                let submit_fn = &mut *submit_connection_form.borrow_mut();
                submit_fn();
            });
        }

        let username_entry: gtk::Entry = builder.get_object("username_entry").unwrap();
        {
            let submit_connection_form = submit_connection_form.clone();
            username_entry.connect_activate(move |_| {
                let submit_fn = &mut *submit_connection_form.borrow_mut();
                submit_fn();
            });
        }

        let server_entry: gtk::Entry = builder.get_object("server_entry").unwrap();
        server_entry.connect_activate(move |_| {
            let submit_fn = &mut *submit_connection_form.borrow_mut();
            submit_fn();
        });
        let connect_form: gtk::Box = builder.get_object("connect_form").unwrap();
        stack.add_named(&connect_form, "connect");

        // --- Setup for main state

        let treeview: gtk::TreeView = builder.get_object("status_treeview").unwrap();
        let renderer = gtk::CellRendererText::new();
        let column = gtk::TreeViewColumn::new();
        column.pack_start(&renderer, true);
        column.set_title("Users");
        column.add_attribute(&renderer, "text", Columns::Username as i32);
        treeview.append_column(&column);

        let disconnect_menu_item: gtk::MenuItem =
            builder.get_object("disconnect_menu_item").unwrap();
        disconnect_menu_item.connect_activate(move |_| {
            glib_context.spawn_local(disconnect_from_server(ui.clone(), network.clone()));
        });

        let inputs_submenu: gtk::Menu = builder.get_object("inputs_submenu").unwrap();
        let outputs_submenu: gtk::Menu = builder.get_object("outputs_submenu").unwrap();

        for (device_count, device) in input_devices.into_iter().enumerate() {
            let input_menu_item: gtk::MenuItem =
                gtk::MenuItemBuilder::new().label(&device.label).build();
            input_menu_item.connect_activate(|_| {
                //send command switch device
            });
            let device_count = device_count as u32;
            inputs_submenu.attach(&input_menu_item, 0, 1, device_count, device_count + 1);
        }

        for (device_count, device) in output_devices.into_iter().enumerate() {
            let output_menu_item: gtk::MenuItem =
                gtk::MenuItemBuilder::new().label(&device.label).build();
            output_menu_item.connect_activate(|_| {
                println!("Clicked output menu item button.");
            });
            let device_count = device_count as u32;
            outputs_submenu.attach(&output_menu_item, 0, 1, device_count, device_count + 1);
        }

        let status_box: gtk::Box = builder.get_object("status_box").unwrap();
        stack.add_named(&status_box, "status");

        window.add(&stack);
        window.show_all();
    });
}
