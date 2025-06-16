use nvim_oxi::{self, Dictionary, Function, Object, api::notify};
use std::collections::HashSet;
use std::thread;

use nvim_oxi::libuv;
use nvim_oxi::schedule;
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tokio::sync::mpsc::{self, UnboundedSender};

use serde::{Deserialize, Serialize};
use serde_json;

type Result<T> = std::result::Result<T, EmbazelerError>;

#[derive(Error, Debug)]
pub enum EmbazelerError {
    #[error("Could not get a lock on the mutex: `{0}`")]
    MutexFailure(String),
    #[error("Could not parse event: `{0}`")]
    EventParseError(String),
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
enum Event {
    FileOpen { filename: String },
    FileSave { filename: String },
}

type EventJSON = String;


// you can design the state to be a concurrent data structure but we
// just mutex for now
type PluginContext = Arc<Mutex<State>>;
struct State {
    num_visits: i64,
    opened_files: HashSet<String>,
    saved_files: HashSet<String>,
}

fn error(msg: &str) {
    let empty_opts = &Dictionary::new();
    let error_string = format!("embazeler: {msg}");
    notify(&error_string, nvim_oxi::api::types::LogLevel::Error, empty_opts).expect("couldn't notify");
}

fn info(msg: &str) {
    let empty_opts = &Dictionary::new();
    let info_string = format!("embazeler: {msg}");
    notify(&info_string, nvim_oxi::api::types::LogLevel::Info, empty_opts).expect("couldn't notify");
}

fn forward_to(event_forwarder: UnboundedSender<EventJSON>) -> Object {
    let func = move |event: EventJSON| {
        event_forwarder.send(event).unwrap();
    };
    Object::from(Function::from_fn(func))
}

fn file_save_handler(context: PluginContext, filename: &str) -> Result<()> {
    let mut state = context.lock().map_err(|e| EmbazelerError::MutexFailure(e.to_string()))?;
    state.saved_files.insert(filename.to_owned());
    info(&format!("saved_file: {filename}"));
    Ok(())
}

fn file_open_handler(context: PluginContext, filename: &str) -> Result<()> {
    let mut state = context.lock().map_err(|e| EmbazelerError::MutexFailure(e.to_string()))?;
    state.opened_files.insert(filename.to_owned());
    info(&format!("opened_file: {filename}"));
    Ok(())
}

fn neovim_event_handler(context: PluginContext, event: EventJSON) -> Result<()> {
    let result = serde_json::from_str(&event);
    let event : Event = result.map_err(|_| EmbazelerError::EventParseError(event))?;

    match &event {
        Event::FileOpen{ filename } => file_open_handler(context.clone(), &filename)?,
        Event::FileSave{ filename } => file_save_handler(context.clone(), &filename)?,
        _ => error("don't know how to process that yet")
    }

    Ok(())
}

fn start_event_stream() -> UnboundedSender<EventJSON> {
    let (event_stream, mut event_reciever) = mpsc::unbounded_channel::<EventJSON>();
    let (tx, mut rx) = mpsc::unbounded_channel::<EventJSON>();
    let state = State {
        num_visits: 0,
        opened_files: HashSet::new(),
        saved_files: HashSet::new(),
    };
    let context = Arc::new(Mutex::new(state));

    // we need to set up and forward a channel into the libuv async handle
    // because it tracks state necessary for the neovim event loop
    // to function properly
    //
    // we *could* theoretically spawn our own threads using tokio,
    // but then we won't have thread-safe access to nvim_oxi::api
    // and lua functions
    let event_ready = libuv::AsyncHandle::new(move || {
        let neovim_context = context.clone();
        let event = event_reciever.blocking_recv().unwrap();
        schedule(move |_| {
            let result = neovim_event_handler(neovim_context, event);
            match result {
                Ok(_) => {},
                Err(err) => {error(err.to_string().as_str())}
            }
        });
    })
    .unwrap();

    let _ = thread::spawn(move || {
        loop {
            let event = rx.blocking_recv().unwrap();
            event_stream.send(event).unwrap();
            event_ready.send().unwrap(); // notify libuv handler!
        }
    });

    tx
}

#[nvim_oxi::plugin]
fn embazeler() -> Result<Dictionary> {
    let event_stream = start_event_stream();
    let capture_func = forward_to(event_stream);

    Ok(Dictionary::from_iter([("capture", capture_func)]))
}
