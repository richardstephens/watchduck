use ctrlc;
use lazy_static::lazy_static;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::process::Stdio;
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::sync::RwLock;

#[derive(Default)]
struct SystemState {
    services: HashSet<CommandService>,
    msg_tx: Option<UnboundedSender<ChannelEvent>>,
}
lazy_static! {
    static ref EXPECTED_CONFIG: Arc<RwLock<Vec<CommandServiceConfig>>> =
        Arc::new(RwLock::new(vec![]));
    static ref SYSTEM_STATE: Arc<RwLock<SystemState>> =
        Arc::new(RwLock::new(SystemState::default()));
}

enum ChannelEvent {
    SigInt,
    SyncConfig,
    ProcessExited,
}

#[derive(Eq, PartialEq, Ord, PartialOrd, Clone, Hash)]
struct CommandServiceConfig {
    pub name: String,
    pub cmd: String,
    pub args: Vec<String>,
}

#[derive(Eq, PartialEq)]
enum CsStatus {
    NotStarted,
    Running,
    Stopped,
    Crashed,
    Exiting,
}
struct CsState {
    status: CsStatus,
}

#[derive(Clone)]
struct CommandService {
    pub config: CommandServiceConfig,
    pub state: Arc<RwLock<CsState>>,
}

async fn sync_config() {
    let config_vec = { EXPECTED_CONFIG.read().await.clone() };
    let config: HashSet<CommandServiceConfig> = HashSet::from_iter(config_vec.iter().cloned());
    let mut w = SYSTEM_STATE.write().await;

    for s in config.clone().iter() {
        let mut found = false;
        for cs in w.services.clone().iter() {
            if &cs.config == s {
                if { cs.state.read().await.status == CsStatus::Running } {
                    found = true;
                }
            }
        }
        if !found {
            let cs = CommandService {
                config: s.clone(),
                state: Arc::new(RwLock::new(CsState {
                    status: CsStatus::NotStarted,
                })),
            };
            cs.spawn().await;
        }
    }
}

async fn event_loop(mut rx: UnboundedReceiver<ChannelEvent>) {
    loop {
        match rx.recv().await {
            None => {
                return;
            }
            Some(ChannelEvent::SigInt) => {
                println!("ctrl-c received.");
                return;
            }
            Some(ChannelEvent::SyncConfig) => {
                sync_config().await;
            }
            Some(ChannelEvent::ProcessExited) => {
                sync_config().await;
            }
        }
    }
}

impl CommandService {
    async fn spawn(&self) {
        let config = &self.config;
        println!("spawning {}", &config.name);
        let mut child = Command::new(&config.cmd)
            .args(&config.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to execute child");
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let mut reader_stdout = BufReader::new(stdout).lines();
        let mut reader_stderr = BufReader::new(stderr).lines();
        let cmd_state = self.state.clone();
        {
            let mut w = cmd_state.write().await;
            w.status = CsStatus::Running;
        }
        tokio::spawn(async move {
            let status = child
                .wait()
                .await
                .expect("child process encountered an error");

            println!("child status was: {}", status);
            let mut w = cmd_state.write().await;
            if w.status == CsStatus::Exiting {
                w.status = CsStatus::Stopped;
            } else {
                w.status = CsStatus::Crashed;
            }
            drop(w);
            if let Some(rx) = { SYSTEM_STATE.read().await.msg_tx.clone() } {
                println!("sending process exited");
                rx.send(ChannelEvent::ProcessExited).unwrap();
            } else {
                println!("channel is missing!");
            }
        });
        tokio::spawn(async move {
            while let Some(line) = reader_stdout.next_line().await.unwrap() {
                println!("S=>'{:?}'", line)
            }
            println!("stdout ending reached");
        });
        tokio::spawn(async move {
            while let Some(line) = reader_stderr.next_line().await.unwrap() {
                println!("E=>'{:?}'", line)
            }
            println!("StdErr ending reached")
        });
    }
}
async fn register_ctrlc(tx: UnboundedSender<ChannelEvent>) {
    ctrlc::set_handler(move || tx.send(ChannelEvent::SigInt).unwrap()).unwrap()
}
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    register_ctrlc(tx.clone()).await;

    {
        let mut w = SYSTEM_STATE.write().await;
        w.msg_tx = Some(tx.clone());
    }

    let c = CommandServiceConfig {
        name: "crashy server".to_string(),
        cmd: "python".to_string(),
        args: vec!["crashy_server.py".to_string()],
    };
    {
        let mut w = EXPECTED_CONFIG.write().await;
        w.push(c);
    }
    tx.send(ChannelEvent::SyncConfig).unwrap();

    event_loop(rx).await;
    Ok(())
}
