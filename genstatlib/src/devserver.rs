pub mod broker;
pub mod fswatcher;
mod livereload;
mod responders;

pub use broker::EngineBroker;
use eyre::WrapErr;
pub use livereload::DevServerMsg;
pub use responders::{error_page_with_msg, html_with_live_reload_script, page_not_found};

use poem::EndpointExt;
use std::net::SocketAddr;
use std::sync::Arc;
use std::thread::JoinHandle;
use tracing::trace;

use crate::Result;

pub type DevServerSender = async_channel::Sender<crate::devserver::DevServerMsg>;
pub type DevServerReceiver = async_channel::Receiver<crate::devserver::DevServerMsg>;

#[derive(Debug, Clone)]
pub struct MountDebouncer {
    pub last_update: Arc<async_lock::Mutex<std::time::Instant>>,
}

impl MountDebouncer {
    pub fn new() -> Self {
        Self {
            last_update: Arc::new(async_lock::Mutex::new(std::time::Instant::now())),
        }
    }
}

impl Default for MountDebouncer {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct DevServer {
    #[allow(dead_code)]
    server_thread: JoinHandle<()>,
    #[allow(dead_code)]
    broker: EngineBroker,
}

impl DevServer {
    pub fn run<
        P: AsRef<std::path::Path> + std::fmt::Debug,
        B: Into<SocketAddr> + std::fmt::Debug,
    >(
        broker: EngineBroker,
        output_root: P,
        bind: B,
    ) -> Self {
        let output_root = output_root.as_ref().to_owned();
        let bind = bind.into();

        let broker_clone = broker.clone();
        trace!("spawning web server thread...");
        let handle = std::thread::spawn(move || {
            broker_clone
                .handle()
                .block_on(async move { run(broker_clone, output_root, bind).await })
                .expect("failed to start dev server");
        });

        Self {
            server_thread: handle,
            broker,
        }
    }
}

async fn run<R: AsRef<std::path::Path> + std::fmt::Debug, B: Into<SocketAddr> + std::fmt::Debug>(
    broker: EngineBroker,
    output_root: R,
    bind: B,
) -> Result<()> {
    use poem::listener::TcpListener;
    use poem::middleware::AddData;
    use poem::{get, Route, Server};

    trace!("starting dev server");

    let output_root = output_root.as_ref().to_string_lossy().to_string();
    let bind = bind.into();

    let connected_clients = livereload::ClientBroker::new(broker.clone());

    let app = Route::new()
        .at(
            "/ws",
            get(livereload::handle.data(tokio::sync::broadcast::channel::<String>(8).0)),
        )
        .at("/*path", get(responders::handle))
        .with(AddData::new(responders::OutputRootDir(output_root)))
        .with(AddData::new(broker))
        .with(AddData::new(MountDebouncer::new()))
        .with(AddData::new(connected_clients));

    Server::new(TcpListener::bind(bind.to_string()))
        .run(app)
        .await
        .wrap_err("Failed to run dev server")
}
