use gio::prelude::*;
use gtk::prelude::*;

use std::rc::Rc;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use crate::network;
use crate::network::ConnectError;

#[derive(Debug)]
#[repr(i32)]
enum Columns {
    Username = 0,
}

fn initialize_user_list(treeview: &gtk::TreeView) -> Rc<gtk::ListStore> {
    let col_types: [glib::Type; 1] = [
        glib::Type::String,
    ];

    let user_list_model = Rc::new(gtk::ListStore::new(&col_types));
    treeview.set_model(Some(&*user_list_model));

    let renderer = gtk::CellRendererText::new();
    let column = gtk::TreeViewColumn::new();
    column.pack_start(&renderer, true);
    column.set_title("Degenerates");
    column.add_attribute(&renderer, "text", Columns::Username as i32);
    treeview.append_column(&column);

    user_list_model
}

async fn connect_to_server(
    username: String,
    server: String,
    handle: network::Handle,
    pane_stack: gtk::Stack,
    connect_progress_stack: gtk::Stack,
    connect_button: gtk::Button,
    connect_failure_status: gtk::Label,
    user_list_model: Rc<gtk::ListStore>,
    connecting: Rc<AtomicBool>,
) {
    let connect_future = network::future::connect(&handle, server, username).await;
    connect_progress_stack.set_visible_child_name("hidden_progress");
    connect_button.set_sensitive(true);
    connecting.store(false, Ordering::Relaxed);

    match connect_future {
        Ok(users) => {
            pane_stack.set_transition_type(gtk::StackTransitionType::SlideUp);
            pane_stack.set_visible_child_name("status");

            let col_indices: [u32; 1] = [
                Columns::Username as u32,
            ];

            for (_, user) in users.iter() {
                let values: [&dyn ToValue; 1] = [&user];
                user_list_model.insert_with_values(None, &col_indices, &values);
            }
        }
        Err(e) => {
            let error_message = match e {
                ConnectError::DestinationUnusable => "Could not find the server provided".to_string(),
                ConnectError::NetworkError => "Could not connect to the server".to_string(),
                // AlreadyConnected shouldn't be possible, but it's useful for end-to-end testing network futures right now.
                ConnectError::AlreadyConnected => "Can't connect because you're already connected to a server".to_string(),
                ConnectError::RequestError(e) => format!("Request failed: {:?}", e),
            };
            connect_failure_status.set_text(&error_message);
        }
    }
}

async fn disconnect_from_server(
    handle: network::Handle,
    pane_stack: gtk::Stack,
    user_list_model: Rc<gtk::ListStore>,
) {
    let _ = network::future::disconnect(&handle).await;
    pane_stack.set_transition_type(gtk::StackTransitionType::SlideDown);
    pane_stack.set_visible_child_name("connect");
    user_list_model.clear();
}

pub fn run(handle: network::Handle) {
    let application = gtk::Application::new(
        Some("ca.mitchhentges.comms"),
        Default::default(),
    ).unwrap();
    application.connect_startup(move |app| {
        let glib_context = glib::MainContext::default();

        let logo_16 = gdk_pixbuf::Pixbuf::new_from_file("resources/logo-16.png").unwrap();
        let logo_32 = gdk_pixbuf::Pixbuf::new_from_file("resources/logo-32.png").unwrap();
        let logo_48 = gdk_pixbuf::Pixbuf::new_from_file("resources/logo-48.png").unwrap();
        let logo_256 = gdk_pixbuf::Pixbuf::new_from_file("resources/logo-256.png").unwrap();

        let window = gtk::ApplicationWindow::new(app);
        window.set_title("Comms");
        window.set_default_size(280, 250);
        window.set_position(gtk::WindowPosition::Center);
        window.set_icon_list(&[logo_16, logo_32, logo_48, logo_256]);

        let connecting = Rc::new(AtomicBool::new(false));
        let builder = gtk::Builder::new_from_file("resources/app.ui");

        let status_treeview: gtk::TreeView = builder.get_object("status_treeview").unwrap();
        let user_list_model = initialize_user_list(&status_treeview);

        let stack = gtk::Stack::new();
        stack.set_transition_duration(300);
        {
            let stack = stack.clone();
            let username_entry: gtk::Entry = builder.get_object("username_entry").unwrap();
            let server_entry: gtk::Entry = builder.get_object("server_entry").unwrap();
            let connect_failure_status: gtk::Label = builder.get_object("connect_failure_status").unwrap();
            let connect_button: gtk::Button = builder.get_object("connect_button").unwrap();
            let connect_progress_stack: gtk::Stack = builder.get_object("connect_progress_stack").unwrap();
            let connect_progress: gtk::ProgressBar = builder.get_object("connect_progress").unwrap();
            let glib_context = glib_context.clone();
            let user_list_model = user_list_model.clone();
            let connecting = connecting.clone();
            let handle = handle.clone();
            let submit_connection_form = Rc::new(RefCell::new(move || {
                if connecting.load(Ordering::Relaxed) {
                    return;
                }

                let username: String = username_entry.get_buffer().get_text();
                let server: String = server_entry.get_buffer().get_text();
                if username == "" || server == "" {
                    connect_failure_status.set_text("Set both username and server");
                    return;
                } else {
                    connect_failure_status.set_text("");
                }

                connect_button.set_sensitive(false);
                connecting.store(true, Ordering::Relaxed);
                connect_progress_stack.set_visible_child_name("progress");

                {
                    let connecting = connecting.clone();
                    let connect_progress = connect_progress.clone();
                    timeout_add(80, move || {
                        if connecting.load(Ordering::Relaxed) {
                            connect_progress.pulse();
                            Continue(true)
                        } else {
                            Continue(false)
                        }
                    });
                }

                glib_context.spawn_local(connect_to_server(
                    username,
                    server,
                    handle.clone(),
                    stack.clone(),
                    connect_progress_stack.clone(),
                    connect_button.clone(),
                    connect_failure_status.clone(),
                    user_list_model.clone(),
                    connecting.clone(),
                ));
            }));

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
            {
                let submit_connection_form = submit_connection_form.clone();
                server_entry.connect_activate(move |_| {
                    let submit_fn = &mut *submit_connection_form.borrow_mut();
                    submit_fn();
                });
            }
        }
        let connect_form: gtk::Box = builder.get_object("connect_form").unwrap();
        stack.add_named(&connect_form, "connect");

        let disconnect_menu_item: gtk::MenuItem = builder.get_object("disconnect_menu_item").unwrap();
        {
            let stack = stack.clone();
            let handle = handle.clone();
            let glib_context = glib_context.clone();
            disconnect_menu_item.connect_activate(move |_| {
                glib_context.spawn_local(disconnect_from_server(
                    handle.clone(),
                    stack.clone(),
                    user_list_model.clone(),
                ));
            });
        }
        let status_box: gtk::Box = builder.get_object("status_box").unwrap();
        stack.add_named(&status_box, "status");

        window.add(&stack);
        window.show_all();
    });
    application.connect_activate(|_| {});
    application.run(&std::env::args().collect::<Vec<_>>());
}
