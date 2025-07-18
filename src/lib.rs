mod command;

use nvim_oxi::{self, Dictionary, Function, Object, api::notify};
use std::collections::HashSet;
use std::thread;

use nvim_oxi::libuv;
use nvim_oxi::schedule;
use std::sync::{Arc, Mutex};
use thiserror::Error;
// TODO: this doesn't have to be tokio, because we always do blocking recv
use tokio::sync::mpsc::{self, UnboundedSender, UnboundedReceiver};

use serde::{Deserialize, Serialize};
use serde_json;

type Result<T> = std::result::Result<T, EmbazelerError>;

#[derive(Error, Debug)]
pub enum EmbazelerError {
    #[error("Could not get a lock on the mutex: `{0}`")]
    MutexFailure(String),
    #[error("Could not parse event: `{0}`")]
    EventParseError(String),
    #[error("Command execution error: `{0}`")] 
    CommandExecError(String)
}

trait PluginEvent {}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
enum StdThreadEvent {
    FileSave { filename: String },
}

impl PluginEvent for StdThreadEvent{}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
enum NeovimEvent {
    FileOpen { filename: String },
    ReportSave {},
    ReportError { error : String }
}

impl PluginEvent for NeovimEvent{}

type EventJSON = String;

// you can design the state to be a concurrent data structure but we
// just mutex for now
type PluginContext<T> = Context<T>;

pub struct Context < T: PluginEvent > {
    state : Arc<Mutex<State>>,
    event_rx : UnboundedReceiver<T>,
    event_tx: UnboundedSender<EventJSON>,
}

struct State {
    num_visits: i64,
    //opened_files: HashSet<String>,
    //saved_files: HashSet<String>,
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

fn file_report_save(state: Arc<Mutex<State>>) -> Result<()> {
    let state = state.lock().map_err(|e| EmbazelerError::MutexFailure(e.to_string()))?;
    let num_visits = state.num_visits;
    info(&format!("num visits: {num_visits}"));
    Ok(())
}

fn file_save_handler(state: Arc<Mutex<State>>, event_tx: UnboundedSender<EventJSON>, filename: &str) -> Result<()> {

    let mut state = state.lock().map_err(|e| EmbazelerError::MutexFailure(e.to_string()))?;
    state.num_visits += 1;
    let string = serde_json::to_string(&NeovimEvent::ReportSave {  }).unwrap();
    // test propogating to the other event stream
    let _ = event_tx.send(string);
    //info(&format!("saved_file: {filename}"));
    Ok(())
}

fn file_open_handler(filename: &str) -> Result<()> {
    //let mut state = context.lock().map_err(|e| EmbazelerError::MutexFailure(e.to_string()))?;
    //state.opened_files.insert(filename.to_owned());
    //let mut channel = command::command("ls", vec![])?;
    //while let Some(line) = channel.blocking_recv() {
        //info(&format!("executing command ls: {line}"))
    //}
    info(&format!("opened file: {filename}"));
    Ok(())
}

// NOTE: use thread::spawn and not nvim_oxi::schedule to kick off extra work for 
// anything downstream of this function or else expect a crash!
// if you need to do something else, populate the event stream again
fn std_thread_event_handler(state: Arc<Mutex<State>>, event_tx: UnboundedSender<EventJSON>, event : StdThreadEvent) -> Result<()> {

    match &event {
        StdThreadEvent::FileSave{ filename } => file_save_handler(state, event_tx, &filename)?,
        _ => {}
    }

    Ok(())

}

// NOTE: use nvim_oxi::schedule and not thread::spawn to kick off extra work for anything
// downstream of this function. Or else you'll just crash
fn neovim_event_handler(state: Arc<Mutex<State>>, event_tx: UnboundedSender<EventJSON>, event: NeovimEvent) -> Result<()> {
    match &event {
        NeovimEvent::FileOpen{ filename } => file_open_handler(&filename)?,
        NeovimEvent::ReportSave {} => file_report_save(state)?,
        _ => {panic!("cannot handle")}
    }

    Ok(())
}

fn start_event_stream() -> UnboundedSender<EventJSON> {
    let event_splitter = splitter();
    let state = State {
        num_visits: 0,
        //opened_files: HashSet::new(),
        //saved_files: HashSet::new(),
        //event_stream: event_splitter.neovim_events_rx, 
    };

    let global_state = Arc::new(Mutex::new(state));

    let mut std_thread_context = Context{
        state: global_state.clone(),
        event_rx: event_splitter.std_thread_events_rx,
        event_tx: event_splitter.event_stream.clone(),
    };

    let mut neovim_context = Context{
        state: global_state.clone(),
        event_rx: event_splitter.neovim_events_rx,
        event_tx: event_splitter.event_stream.clone(),
    };

    let (libuv_event_stream, mut libuv_event_sink) = mpsc::unbounded_channel::<NeovimEvent>();

    // we need to set up and forward a channel into the libuv async handle
    // because the nvim oxi library tracks state necessary for the neovim event loop
    // to function properly
    //
    // we *could* theoretically spawn our own threads,
    // but then we won't have thread-safe access to nvim_oxi::api
    // and lua functions
    //
    // note - do not try to spawn std threads inside a neovim context, 
    // you'll crash neovim!
    let event_ready = libuv::AsyncHandle::new(move || {
        let event = libuv_event_sink.blocking_recv().unwrap();
        let event_tx = neovim_context.event_tx.clone();
        let state = neovim_context.state.clone();
        schedule(move |_| {
            let result = neovim_event_handler(state, event_tx, event);
            match result {
                Ok(_) => {},
                Err(err) => {error(err.to_string().as_str())}
            }
        });
    })
    .unwrap();
    


    // More setup needed:
    // We also spawn a forever thread that just simply checks if we have 
    // neovim events and copies them into the libuv async runtime
    let _ = thread::spawn(move || {
        loop {
            let event = neovim_context.event_rx.blocking_recv().unwrap();
            libuv_event_stream.send(event).unwrap();
            event_ready.send().unwrap(); // notify libuv handler!
        }
    });

    // Setup the event loop similarly for std thread events:
    let _ = thread::spawn(move || {
        loop {
            let std_thread_event = std_thread_context.event_rx.blocking_recv().unwrap();
            let state = std_thread_context.state.clone();
            let event_tx = std_thread_context.event_tx.clone();
            let _ = thread::spawn(move || { 
                let result = std_thread_event_handler(state, event_tx, std_thread_event);
                match result {
                    Ok(_) => {},
                    Err(err) => {} // TODO: send error to neovim
                }
            });
        }
    });

    event_splitter.event_stream
}


struct EventSplitter {
    event_stream: UnboundedSender<EventJSON>,
    std_thread_events_rx: UnboundedReceiver<StdThreadEvent>,
    neovim_events_rx: UnboundedReceiver<NeovimEvent>,
}

fn splitter() -> EventSplitter {
    let (tx, mut rx) = mpsc::unbounded_channel::<EventJSON>();
    let (thread_tx, thread_rx) = mpsc::unbounded_channel::<StdThreadEvent>();
    let (neovim_tx, neovim_rx) = mpsc::unbounded_channel::<NeovimEvent>();
    
    let _ = thread::spawn(move || {
            loop {
                let event_json = rx.blocking_recv().expect("expected channel to be open");
                let std_thread_result = serde_json::from_str::<StdThreadEvent>(&event_json);
                let neovim_result = serde_json::from_str::<NeovimEvent>(&event_json);

                match (std_thread_result, neovim_result) {
                    (Ok(_), Ok(_)) => panic!("did not expect results to be applicable to both"),
                    (Err(_), Err(_)) => panic!("couldn't process either"),
                    (Ok(std_thread_event), Err(_)) => thread_tx.send(std_thread_event).expect("couldn't send"),
                    (Err(_), Ok(neovim_event)) => neovim_tx.send(neovim_event).expect("couldn't send"),
                };
            }
        }
    );

    EventSplitter { 
        event_stream: tx,
        std_thread_events_rx: thread_rx, 
        neovim_events_rx: neovim_rx,
    } 
}

#[nvim_oxi::plugin]
fn embazeler() -> Result<Dictionary> {
    let event_stream = start_event_stream();
    let capture_func = forward_to(event_stream);

    Ok(Dictionary::from_iter([("capture", capture_func)]))
}
