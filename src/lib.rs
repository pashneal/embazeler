use nvim_oxi::{Dictionary, Function, Object, self, api::notify};
use std::thread;

use nvim_oxi::libuv;
use nvim_oxi::{Result, schedule};
use tokio::sync::mpsc::{self, UnboundedSender};


type Event = String;

fn info(msg: String) { 
    let empty_opts = &Dictionary::new();
    notify(&msg, nvim_oxi::api::types::LogLevel::Info, empty_opts).expect("couldn't notify");
}

fn forward_to(event_forwarder: UnboundedSender<Event>) -> Object {
    let func = move |event: Event| {
        event_forwarder.send(event).unwrap();
    };
    Object::from(Function::from_fn(func))
}

#[nvim_oxi::plugin]
fn embazeler() -> Result<Dictionary> {

    let event_stream = start_event_stream();
    let capture_func = forward_to(event_stream);

    Ok(Dictionary::from_iter([
        ("capture", capture_func)
    ]))
}

fn neovim_event_handler(event : Event) -> ()  {
  info(format!("handling event: {event}"))
}

fn start_event_stream() -> UnboundedSender<Event> {
    let (event_stream, mut event_reciever) = mpsc::unbounded_channel::<Event>();
    let (tx, mut rx) = mpsc::unbounded_channel::<Event>();

    // we need to set up and forward a channel into the libuv async handle 
    // because it tracks state necessary for the neovim event loop
    // to function properly
    //
    // we *could* theoretically spawn our own threads using tokio, 
    // but then we won't have thread-safe access to nvim_oxi::api 
    // and lua functions
    let event_ready = libuv::AsyncHandle::new(move || {
        let i = event_reciever.blocking_recv().unwrap();
        schedule(move |_| neovim_event_handler(i));
    }).unwrap();

    let _ = thread::spawn( move || { 
        loop {
            let event = rx.blocking_recv().unwrap();
            event_stream.send(event).unwrap();
            event_ready.send().unwrap();  // notify libuv handler!
        }
    });

    tx
}
