use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};
use crate::{Result, EmbazelerError};

// Usage:
//  command("echo", ["hello"]) 
//  will run the following command `echo hello` and return results within the channel
pub fn command(program: &str, args: Vec<String>) -> Result<UnboundedReceiver<String>> {
    let (tx, rx) = unbounded_channel();

    let program = program.to_string();

    let mut child = match Command::new(&program)
        .args(&args)
        .stdout(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            return Err(EmbazelerError::CommandExecError(format!("Failed to spawn command: {e}")))
        }
    };

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            return Err(EmbazelerError::CommandExecError(format!("Failed to capture stdout")))
        }
    };

    tokio::spawn(async move {
        let mut reader = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = reader.next_line().await {
            if tx.send(line).is_err() {
                break; // Receiver dropped
            }
        }
    });

    Ok(rx)
}
