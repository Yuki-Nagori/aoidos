mod events;
mod tui;

use aoidos_app::{GameServices, profiles::Profiles};
use aoidos_engine::{fault::Fault, storage::Storage};
use aoidos_llm::{providers::ProviderId, proxy::SystemProxySnapshot};
use clap::{Parser, Subcommand};
use events::{EVENT_CAPACITY, EventBridge, RuntimeEvent};
use secrecy::SecretString;
use std::{error::Error, io, path::PathBuf, sync::Arc};
use tokio::sync::mpsc;

#[derive(Debug, Parser)]
#[command(name = "aoidos", about = "Aoidos 终端游戏入口")]
struct Arguments {
    #[arg(long, global = true, help = "覆盖应用数据目录")]
    data_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// 打印已保存的模型配置（不显示密钥）。
    Profiles,
    /// 通过隐藏输入写入供应商密钥库。
    Key {
        #[command(subcommand)]
        action: KeyAction,
    },
    /// 仅从本地账本恢复费用结算，不会重新发送供应商请求。
    Billing {
        #[command(subcommand)]
        action: BillingAction,
    },
    /// 打开可点击的终端游戏界面。
    Play,
}

#[derive(Debug, Subcommand)]
enum KeyAction {
    Set {
        #[arg(long)]
        provider: String,
    },
    Clear {
        #[arg(long)]
        provider: String,
    },
}

#[derive(Debug, Subcommand)]
enum BillingAction {
    RetrySettlement {
        #[arg(long)]
        run_id: String,
        #[arg(long)]
        request_id: String,
    },
}

struct CliApp {
    _instance_lock: aoidos_store::lock::InstanceLock,
    storage: Arc<Storage>,
    profiles: Arc<Profiles>,
    services: GameServices,
    events: mpsc::Receiver<RuntimeEvent>,
    event_bridge: Arc<EventBridge>,
}

impl CliApp {
    fn open(root: PathBuf) -> Result<Self, Box<dyn Error>> {
        std::fs::create_dir_all(&root)?;
        let instance_lock = aoidos_store::lock::acquire(&root)?;
        let (sender, events) = mpsc::channel(EVENT_CAPACITY);
        let bridge = Arc::new(EventBridge::new(sender));
        let storage = Arc::new(Storage::new(&root, bridge.clone(), bridge.clone()));
        let profiles = Arc::new(Profiles::new(&root.join("llm")));
        let services = GameServices::new(
            &root,
            storage.clone(),
            bridge.clone(),
            bridge.clone(),
            profiles.clone(),
            SystemProxySnapshot::capture(),
        )
        .map_err(fault_error)?;
        Ok(Self {
            _instance_lock: instance_lock,
            storage,
            profiles,
            services,
            events,
            event_bridge: bridge,
        })
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("错误：{error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse();
    let root = match arguments.data_dir {
        Some(root) => root,
        None => app_data_dir()?,
    };
    match arguments.command.unwrap_or(Command::Play) {
        Command::Profiles => {
            let profiles = Profiles::new(&root.join("llm"));
            for profile in profiles.list().map_err(fault_error)? {
                println!(
                    "{}  {} / {}",
                    profile.profile_id, profile.provider_id, profile.model
                );
            }
        }
        Command::Key { action } => {
            std::fs::create_dir_all(root.join("llm"))?;
            let profiles = Profiles::new(&root.join("llm"));
            match action {
                KeyAction::Set { provider } => {
                    let provider_id = provider_id(&provider)?;
                    let secret = rpassword::prompt_password(format!(
                        "输入 {} API 密钥（输入不回显）：",
                        provider_id.as_str()
                    ))?;
                    if secret.trim().is_empty() {
                        return Err("密钥不能为空".into());
                    }
                    profiles
                        .set_key(provider_id.as_str(), SecretString::from(secret))
                        .map_err(fault_error)?;
                    println!("已保存 {} 凭据。", provider_id.as_str());
                }
                KeyAction::Clear { provider } => {
                    let provider_id = provider_id(&provider)?;
                    profiles
                        .clear_key(provider_id.as_str())
                        .map_err(fault_error)?;
                    println!("已清除 {} 凭据。", provider_id.as_str());
                }
            }
        }
        Command::Billing { action } => {
            let mut app = CliApp::open(root)?;
            let result = app
                .storage
                .ready()
                .map_err(fault_error)
                .and_then(|()| match action {
                    BillingAction::RetrySettlement { run_id, request_id } => {
                        app.services
                            .billing
                            .retry_pending_settlement(&run_id, &request_id)
                            .map_err(fault_error)?;
                        Ok(())
                    }
                });
            app.shutdown().await;
            result?;
            println!("已从本地用量恢复费用结算；没有重新发送模型请求。");
        }
        Command::Play => {
            let mut app = CliApp::open(root)?;
            let result = match app.storage.ready().map_err(fault_error) {
                Ok(()) => tui::run(&mut app).await,
                Err(error) => Err(Box::new(error) as Box<dyn Error>),
            };
            app.shutdown().await;
            result?;
        }
    }
    Ok(())
}

impl CliApp {
    async fn shutdown(&mut self) {
        self.services.runtime.shutdown().await;
        self.services.turns.coordinator.shutdown().await;
        let _ = self.storage.close();
    }
}

fn fault_error(fault: Fault) -> io::Error {
    io::Error::other(format!("{}：{}", fault.code, fault.message))
}

fn provider_id(value: &str) -> Result<ProviderId, Box<dyn Error>> {
    ProviderId::parse(value).ok_or_else(|| format!("未知供应商：{value}").into())
}

fn app_data_dir() -> Result<PathBuf, Box<dyn Error>> {
    #[cfg(target_os = "macos")]
    let path = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or("无法解析 HOME")?
        .join("Library/Application Support/com.yuki.aoidos");

    #[cfg(target_os = "windows")]
    let path = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .ok_or("无法解析 APPDATA")?
        .join("com.yuki.aoidos");

    #[cfg(target_os = "linux")]
    let path = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
        .ok_or("无法解析 XDG_DATA_HOME 或 HOME")?
        .join("com.yuki.aoidos");

    Ok(path)
}
