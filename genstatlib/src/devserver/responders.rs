use crate::core::page::RenderedPage;
use crate::devserver::MountDebouncer;
use crate::{AsStdError, Result};
use poem::http::StatusCode;
use poem::{
    handler,
    web::{Data, Path},
    Response,
};
use std::path::PathBuf;
use tracing::trace;

use super::EngineBroker;

#[derive(Clone, Debug)]
pub struct OutputRootDir(pub String);

fn path_to_file<S: AsRef<str>>(path: S) -> String {
    let path = path.as_ref();
    path.replace("../", "")
}

fn error_page() -> &'static str {
    include_str!("error.html")
}

pub fn error_page_with_msg<S: AsRef<str>>(msg: S) -> String {
    let html = error_page().replace("{{ERROR}}", msg.as_ref());
    format!(
        r#"{html}
        <script>{}</script>
        <style>{}</style>
        <div class="devserver-notify-container"><div id="devserver-notify-payload"></div></div>"#,
        include_str!("live-reload.js"),
        include_str!("toast.css")
    )
}

pub fn page_not_found() -> String {
    error_page_with_msg("404")
}

pub fn html_with_live_reload_script(html: &str) -> String {
    format!(
        r#"{html}
        <script>{}</script>
        <style>{}</style>
        <div class="devserver-notify-container"><div id="devserver-notify-payload"></div></div>"#,
        include_str!("live-reload.js"),
        include_str!("toast.css")
    )
}

pub fn try_static_file<S: AsRef<str>>(
    path: S,
    mount_point: &Data<&OutputRootDir>,
) -> Option<Response> {
    trace!("try to serve static file");

    let path = path.as_ref();

    let mount_point = mount_point.0;

    let mut system_path = PathBuf::from(mount_point.0.clone());
    system_path.push(path);
    trace!(path = ?system_path, "using path");

    let mime_type = {
        match mime_guess::from_path(&system_path).first() {
            Some(guess) => guess,
            None => mime::APPLICATION_OCTET_STREAM,
        }
    };

    trace!(mime = ?mime_type);

    match std::fs::read(system_path) {
        Ok(file) => {
            trace!("getgot");
            let mime_type = mime_type.essence_str();
            if mime_type == mime::HTML
                || mime_type == mime::TEXT_HTML
                || mime_type == mime::TEXT_HTML_UTF_8
            {
                let page = String::from_utf8_lossy(&file);
                let page = html_with_live_reload_script(&page);
                Some(Response::builder().content_type(mime_type).body(page))
            } else {
                Some(Response::builder().content_type(mime_type).body(file))
            }
        }
        Err(_) => None,
    }
}

pub async fn try_rendered_file<S: AsRef<str>>(
    broker: &EngineBroker,
    path: S,
) -> Result<Option<RenderedPage>> {
    use crate::core::library::SearchKey;
    use crate::devserver::broker::EngineMsg;
    use crate::devserver::broker::EngineRequest;

    trace!("try to serve rendered file");

    let path = path.as_ref();

    let search_key = {
        if path.starts_with('/') {
            SearchKey::from(path)
        } else {
            SearchKey::from(format!("/{}", path))
        }
    };

    let (send, recv) = EngineRequest::new(search_key);

    broker.send_engine_msg(EngineMsg::RenderPage(send)).await?;
    recv.recv().await?
}

pub fn serve_rendered_file<S: AsRef<str>>(html: S) -> Response {
    Response::builder()
        .content_type(mime::TEXT_HTML_UTF_8)
        .body(html_with_live_reload_script(html.as_ref()))
}

pub async fn run_pipelines<S: AsRef<str>>(broker: &EngineBroker, path: S) -> Result<()> {
    use super::broker::{EngineMsg, EngineRequest};
    use typed_uri::Uri;

    let uri = format!("/{}", path.as_ref());
    let uri = Uri::new(&uri, &uri).unwrap();

    let (send, _recv) = EngineRequest::new(uri);
    broker
        .send_engine_msg(EngineMsg::ProcessPipelines(send))
        .await?;
    Ok(())
}

pub async fn try_mount(broker: &EngineBroker, debouncer: MountDebouncer) -> Result<()> {
    use crate::devserver::broker::{EngineMsg, EngineRequest};
    use std::time::{Duration, Instant};

    let mut last_update = debouncer.last_update.lock_arc().await;
    if Instant::now() - *last_update > Duration::from_millis(1000) {
        *last_update = Instant::now();
        let (send, _recv) = EngineRequest::new(());
        broker
            .send_engine_msg(EngineMsg::ProcessMounts(send))
            .await?;
    }
    Ok(())
}

#[handler]
pub async fn handle(
    path: Path<String>,
    mount_point: Data<&OutputRootDir>,
    broker: Data<&EngineBroker>,
    mount_debouncer: Data<&MountDebouncer>,
) -> std::result::Result<Response, poem::error::Error> {
    let path = path_to_file(path.to_string());

    try_mount(&broker, mount_debouncer.clone())
        .await
        .map_err(|e| poem::error::InternalServerError(AsStdError(e)))?;

    match try_rendered_file(*broker, &path).await {
        Ok(Some(page)) => return Ok(serve_rendered_file(&page.html())),
        Err(e) => {
            let report = {
                let msg = format!("{:?}", e);
                let msg = ansi_to_html::convert_escaped(&msg)
                    .unwrap()
                    .replace('━', "=");
                error_page_with_msg(format!("<pre>{}</pre>", &msg))
            };
            return Err(poem::error::Error::from_string(
                report,
                StatusCode::INTERNAL_SERVER_ERROR,
            ));
        }
        _ => (),
    }

    if let Some(res) = try_static_file(path.clone(), &mount_point) {
        run_pipelines(*broker, &path)
            .await
            .map_err(|e| poem::error::InternalServerError(AsStdError(e)))?;
        Ok(res)
    } else {
        Ok(Response::builder()
            .content_type(mime::TEXT_HTML_UTF_8)
            .status(StatusCode::NOT_FOUND)
            .body(page_not_found()))
    }
}
