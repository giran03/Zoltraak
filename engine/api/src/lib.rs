//! Localhost REST API for external clients: the browser extension, the CLI,
//! and any automation the user wires up.
//!
//! The Tauri desktop shell does *not* consume this — it talks to the engine
//! in-process over Tauri IPC. It does, however, *host* it, so that closing a
//! terminal is not a prerequisite for the extension to work.

pub mod auth;
pub mod extension_source;
pub mod store;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{Path, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use zoltraak_core::error::Error as EngineError;
use zoltraak_core::model::DownloadId;
use zoltraak_core::queue::NewDownload;
use zoltraak_core::remux::manager::{NewRemux, RemuxJob, RemuxManager};
use zoltraak_core::remux::{Container, MediaInfo};
use zoltraak_core::{
    Category, CategoryKind, DownloadSnapshot, EngineSettings, ImageAlbum, MusicJob, MusicManager, MusicSettings, MusicSource,
    NewCategory, NewMusicDownload, NewVideoDownload, OutputOptions, SessionSummary, SiteSession, Supervisor, VideoFormat, VideoJob,
    VideoManager, headers as engine_headers,
};
use serde::{Deserialize, Serialize};

pub use auth::ApiToken;

/// Loopback-only by default. Binding to `0.0.0.0` would expose the engine to
/// the whole LAN; that is opt-in and belongs to the headless/NAS mode, which
/// must ship its own auth story before it is enabled.
pub const DEFAULT_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 1450);

#[derive(Clone)]
pub struct ApiState {
    /// Resolved desktop palette; null restores the extension's own defaults.
    pub extension_theme: Arc<std::sync::RwLock<serde_json::Value>>,
    /// Resolved desktop palette for first-party pages, independent of extension sync.
    pub guide_theme: Arc<std::sync::RwLock<serde_json::Value>>,
    pub token: Arc<ApiToken>,
    pub supervisor: Supervisor,
    pub video: VideoManager,
    /// Music capture, when the host wired one up. `None` leaves the
    /// `/music/*` routes answering 503, like Discord.
    pub music: Option<MusicManager>,
    /// Repackaging of files already on disk. Unlike music and Discord this is
    /// never `None`: it needs no credentials, no database and no host wiring —
    /// only a local `ffmpeg`, whose absence is reported per job rather than by
    /// taking the routes away.
    pub remux: RemuxManager,
    /// Where `POST /music/prefill` hands a link for the Music view. `None`
    /// in the headless server, which has no window to fill.
    pub music_prefill_tx: Option<tokio::sync::mpsc::UnboundedSender<(String, engine_headers::Headers)>>,
    /// Where a request that names no directory lands.
    pub default_directory: Arc<PathBuf>,
    /// Where `POST /download/prefill` hands the URL to open/paste in the downloads input.
    pub download_prefill_tx: Option<tokio::sync::mpsc::UnboundedSender<(String, engine_headers::Headers)>>,
    /// Where `POST /image/prefill` hands a post link for the Images view.
    pub image_prefill_tx: Option<tokio::sync::mpsc::UnboundedSender<(String, engine_headers::Headers)>>,
    /// Where `POST /video/prefill` hands the URL (and its sanitized headers —
    /// the caller's session cookie, most often, for a login-gated page): the
    /// desktop shell listens on the receiver to focus its window, fill the
    /// Video view, and carry the headers into the analyze/download the user
    /// triggers there. `None` in the headless CLI/server, which has no window
    /// — the route then no-ops.
    pub prefill_tx: Option<
        tokio::sync::mpsc::UnboundedSender<(String, engine_headers::Headers, bool, Option<String>)>,
    >,
    /// Discord delivery, when the host wired one up. `None` leaves the
    /// `/discord/*` routes answering 503 rather than being absent, so a client
    /// can tell "this build has no Discord" from "you typed the path wrong".
    pub discord: Option<zoltraak_core::discord::DiscordService>,
    /// Thread-safe set of download/video job IDs flagged for Discord delivery upon completion.
    pub auto_send: Option<Arc<tokio::sync::RwLock<std::collections::HashSet<String>>>>,
    /// Temporary, single-use window during which the `/pair` route answers requests from extension
    /// origins with the API token. It is closed unless the user has just asked the app to pair.
    pub pairing_window: Arc<std::sync::RwLock<Option<std::time::Instant>>>,
    /// Where `POST /folder/choose` requests a native folder picker. `None`
    /// in the headless server.
    pub folder_picker_tx: Option<tokio::sync::mpsc::UnboundedSender<FolderPickerRequest>>,
    /// Monotonically increasing version bumped whenever extensions should reload their code/tokens.
    pub reload_version: Arc<std::sync::atomic::AtomicU64>,
    /// Watch sender for instant reload notification broadcasts.
    pub reload_tx: Arc<tokio::sync::watch::Sender<u64>>,
    pub extension_sources: extension_source::ExtensionSourceManager,
    pub extension_source_switch: Arc<tokio::sync::Mutex<()>>,
    /// Channel to command the desktop shell to focus window and navigate to a section/tab.
    pub app_navigate_tx: Option<tokio::sync::mpsc::UnboundedSender<(String, Option<String>)>>,
}

pub type FolderPickerRequest = (
    Option<PathBuf>,
    tokio::sync::oneshot::Sender<Option<PathBuf>>,
);

/// Creates a new reload broadcast channel and atomic version counter.
pub fn new_reload_channel() -> (Arc<std::sync::atomic::AtomicU64>, Arc<tokio::sync::watch::Sender<u64>>) {
    let (tx, _) = tokio::sync::watch::channel(0u64);
    (Arc::new(std::sync::atomic::AtomicU64::new(0)), Arc::new(tx))
}

impl ApiState {
    async fn finish_extension_source_mutation(
        &self,
        mutation: extension_source::Mutation,
    ) -> Result<extension_source::ExtensionSourceState, EngineError> {
        if mutation.source_changed {
            self.video.clear_shared_sessions().await?;
            self.extension_sources.commit_source_change();
        }
        Ok(self.extension_sources.state())
    }

    pub async fn extension_source_state(&self) -> Result<extension_source::ExtensionSourceState, EngineError> {
        let _transition = self.extension_source_switch.lock().await;
        let mutation = self.extension_sources.refresh();
        self.finish_extension_source_mutation(mutation).await
    }

    pub async fn select_extension_source(
        &self,
        client_id: &str,
    ) -> Result<extension_source::ExtensionSourceState, String> {
        let _transition = self.extension_source_switch.lock().await;
        let mutation = self.extension_sources.select(client_id).map_err(|e| e.to_string())?;
        self.finish_extension_source_mutation(mutation)
            .await
            .map_err(|e| e.to_string())
    }

    async fn heartbeat_extension_source(
        &self,
        heartbeat: extension_source::ExtensionHeartbeat,
    ) -> Result<extension_source::ExtensionSourceState, String> {
        let _transition = self.extension_source_switch.lock().await;
        let mutation = self.extension_sources.touch(heartbeat).map_err(|e| e.to_string())?;
        self.finish_extension_source_mutation(mutation)
            .await
            .map_err(|e| e.to_string())
    }

    pub fn enable_pairing_window(&self, duration: std::time::Duration) {
        if let Ok(mut held) = self.pairing_window.write() {
            *held = Some(std::time::Instant::now() + duration);
        }
    }

    /// Pairing hands out the token once per explicit request, so it closes after a success.
    pub fn close_pairing_window(&self) {
        if let Ok(mut held) = self.pairing_window.write() {
            *held = None;
        }
    }

    pub fn is_pairing_window_active(&self) -> bool {
        match self.pairing_window.read() {
            Ok(held) => match *held {
                Some(deadline) => std::time::Instant::now() < deadline,
                None => false,
            },
            Err(_) => false,
        }
    }

    /// Signals all connected extensions (and any future pollers) to reload themselves.
    pub fn trigger_extension_reload(&self) {
        let next = self.reload_version.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let _ = self.reload_tx.send(next);
    }

    pub fn current_reload_version(&self) -> u64 {
        self.reload_version.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Resets the extension reload version to 0 and notifies subscribers.
    pub fn reset_reload_version(&self) {
        self.reload_version.store(0, std::sync::atomic::Ordering::SeqCst);
        let _ = self.reload_tx.send(0);
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadPrefillRequest {
    pub url: String,
    pub referrer: Option<String>,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRequest {
    #[serde(default)]
    pub paused: bool,
    pub url: String,
    pub filename: Option<String>,
    /// Absolute path. Ignored when relative — an external client should not be
    /// able to steer writes relative to whatever directory the engine happens
    /// to have been launched from.
    pub directory: Option<PathBuf>,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    pub referrer: Option<String>,
    pub connections: Option<u8>,
    #[serde(default)]
    pub priority: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadAccepted {
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryRequest {
    pub name: String,
    pub kind: CategoryKind,
    pub pattern: String,
    /// Absolute path, same reasoning as `DownloadRequest::directory`.
    pub directory: PathBuf,
    #[serde(default)]
    pub priority: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CategoryCreated {
    pub id: i64,
}

#[derive(Debug, Serialize)]
pub struct ApiError {
    pub error: String,
}

/// A browser session the extension shares for one site. Stored in the
/// engine's session vault — minimised to the cookie names the site's
/// extractor needs, encrypted at rest, never written to a job row.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRequest {
    /// A canonical platform key (`"youtube.com"`, `"x.com"`, ...) — the
    /// extension already knows which one it's on, matching
    /// `zoltraak_core::video::site::cookie_domain_for`. An unrecognized value
    /// is stored but never looked up until the engine also recognizes it.
    pub domain: String,
    /// The single `Cookie` header value for the site.
    pub cookie: Option<String>,
    /// YouTube's browser-minted proof-of-origin token and the visitor id it
    /// is bound to.
    pub po_token: Option<String>,
    pub visitor_data: Option<String>,
    pub po_token_video_id: Option<String>,
    /// Older extensions send the session as a header list instead; `Cookie`
    /// and the `X-Youtube-*` pseudo-headers are read out of it.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoAnalyzeRequest {
    #[serde(default)]
    pub use_fallback: bool,
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    pub referrer: Option<String>,
    pub source_kind: Option<String>,
    pub media_type: Option<String>,
}

/// Analyze a post URL into the pictures it holds.
///
/// Shaped exactly like [`VideoAnalyzeRequest`] because the caller's problem is
/// the same one: a URL, and whatever session it takes to see what is behind it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageAnalyzeRequest {
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    pub referrer: Option<String>,
    #[serde(default)]
    pub browser_documents: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoPrefillRequest {
    pub url: String,
    /// The caller's session cookie for `url`, most often — carried through so
    /// a login-gated page can still be analyzed/downloaded once it lands in
    /// the Video view, the same way `DownloadRequest`/`VideoAnalyzeRequest` do.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    pub referrer: Option<String>,
    /// When set, the Video view should immediately download the best-quality
    /// result instead of waiting for the user to choose a format — used by the
    /// floating "Download this video" button.
    #[serde(default)]
    pub auto_download: bool,
    /// The source page's title (`document.title`), used only as a fallback
    /// filename for raw CDN links whose extractor has no real title to offer
    /// (`direct::single_format`'s title is otherwise just the URL's filename).
    pub title: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoDownloadRequest {
    pub title: String,
    pub webpage_url: String,
    pub format: VideoFormat,
    /// Absolute path, same reasoning as `DownloadRequest::directory`.
    pub directory: Option<PathBuf>,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    pub referrer: Option<String>,
    #[serde(default)]
    pub auto_discord: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoJobAccepted {
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevealDownloadRequest {
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenDownloadRequest {
    pub id: Option<String>,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevealFolderRequest {
    /// Absolute path, same reasoning as `DownloadRequest::directory`.
    pub path: Option<PathBuf>,
}

pub fn router(state: ApiState) -> Router {
    let protected = Router::new()
        .route("/download", post(create_download))
        .route("/download/prefill", post(prefill_download))
        .route("/downloads", get(list_downloads))
        .route("/downloads/{id}/pause", post(pause_download))
        .route("/downloads/{id}/resume", post(resume_download))
        .route("/downloads/{id}", delete(remove_download))
        .route("/download/reveal", post(reveal_download))
        .route("/download/open", post(open_download))
        .route("/folder/reveal", post(reveal_folder))
        .route("/folder/choose", post(choose_folder_dialog))
        .route("/folder/list", post(list_directory_folders))
        .route("/categories", get(list_categories).post(create_category))
        .route("/categories/{id}/enable", post(enable_category))
        .route("/categories/{id}/disable", post(disable_category))
        .route("/categories/{id}", delete(remove_category))
        .route("/settings", get(get_settings).put(put_settings))
        .route("/extension/theme", get(get_extension_theme))
        .route("/extension/status", get(extension_status))
        .route("/video/analyze", post(analyze_video))
        .route("/image/analyze", post(analyze_images))
        .route("/image/prefill", post(prefill_images))
        .route("/video/prefill", post(prefill_video))
        .route("/site-cookies", post(set_session))
        .route("/sessions", get(list_sessions).post(set_session).delete(forget_all_sessions))
        .route("/sessions/{domain}", delete(forget_session))
        .route("/video/download", post(download_video))
        .route("/video/jobs", get(list_video_jobs))
        .route("/video/jobs/{id}", delete(remove_video_job))
        .route("/video/jobs/{id}/pause", post(pause_video_job))
        .route("/video/jobs/{id}/resume", post(resume_video_job))
        .route("/music/analyze", post(analyze_music))
        .route("/music/download", post(download_music))
        .route("/music/prefill", post(prefill_music))
        .route("/music/jobs", get(list_music_jobs).delete(clear_music_done))
        .route("/music/jobs/{id}", delete(remove_music_job))
        .route("/music/jobs/{id}/pause", post(pause_music_job))
        .route("/music/jobs/{id}/resume", post(resume_music_job))
        .route("/music/collections/{id}", delete(remove_music_collection))
        .route("/music/collections/{id}/retry", post(retry_music_collection))
        .route("/music/settings", get(get_music_settings).put(put_music_settings))
        .route("/music/tools", get(music_tools))
        .route("/remux/probe", post(probe_media))
        .route("/convert/capabilities", get(conversion_capabilities))
        .route("/convert/plan", post(plan_conversions))
        .route("/convert/jobs", get(list_remux_jobs).post(create_conversion_jobs).delete(clear_remux_done))
        .route("/convert/jobs/{id}", delete(remove_remux_job))
        .route("/convert/jobs/{id}/cancel", post(cancel_remux_job))
        .route("/convert/jobs/{id}/retry", post(retry_remux_job))
        .route("/remux/jobs", get(list_remux_jobs).post(create_remux_jobs).delete(clear_remux_done))
        .route("/remux/jobs/{id}", delete(remove_remux_job))
        .route("/remux/jobs/{id}/cancel", post(cancel_remux_job))
        .route("/remux/jobs/{id}/retry", post(retry_remux_job))
        .route("/discord/status", get(discord_status))
        .route("/discord/config", get(discord_config).put(put_discord_config))
        .route("/discord/credentials", post(discord_connect).delete(discord_disconnect))
        .route("/discord/guilds", get(discord_guilds))
        .route("/discord/channels/{guild_id}", get(discord_channels))
        .route("/discord/target", post(discord_set_target))
        .route("/discord/test", post(discord_test))
        .route("/discord/reconnect", post(discord_reconnect))
        .route("/discord/bot/profile", get(discord_bot_profile_get).patch(discord_bot_profile_patch).layer(axum::extract::DefaultBodyLimit::max(15 * 1024 * 1024)))
        .route("/discord/bot/diagnostics", get(discord_bot_diagnostics))
        .route("/discord/send", post(discord_send))
        .route("/discord/deliveries", get(discord_deliveries).delete(discord_clear_deliveries))
        .route("/discord/deliveries/{id}/retry", post(discord_retry_delivery))
        .route("/discord/deliveries/{id}/message", delete(discord_delete_message))
        .layer(middleware::from_fn_with_state(state.clone(), guard));

    Router::new()
        // Unauthenticated on purpose: lets the extension discover whether the
        // engine is running. It returns no user data.
        .route("/health", get(health))
        .route("/guide", get(guide))
        .route("/guide/theme", get(get_guide_theme))
        .route("/legal", get(legal))
        .route("/pair", post(pair_extension).options(pair_options))
        .route("/extension/check-reload", get(check_extension_reload).options(check_reload_options))
        .route("/app/open", get(app_open_get).post(app_open_post).options(app_open_options))
        .route("/app/focus", get(app_open_get).post(app_open_post).options(app_open_options))
        .merge(protected)
        .with_state(state)
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppNavigateParams {
    pub section: Option<String>,
    pub tab: Option<String>,
}

async fn handle_app_navigate(state: ApiState, params: AppNavigateParams) -> Response {
    let section = params.section.unwrap_or_else(|| "Settings".to_string());
    let tab = params.tab.or_else(|| Some("Appearance".to_string()));

    if let Some(tx) = &state.app_navigate_tx {
        let _ = tx.send((section.clone(), tab.clone()));
    }

    let mut response = (
        StatusCode::OK,
        Json(serde_json::json!({
            "success": true,
            "section": section,
            "tab": tab,
        })),
    )
        .into_response();
    let headers = response.headers_mut();
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, header::HeaderValue::from_static("*"));
    headers.insert(header::ACCESS_CONTROL_ALLOW_METHODS, header::HeaderValue::from_static("GET, POST, OPTIONS"));
    headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, header::HeaderValue::from_static("Content-Type, Authorization"));
    response
}

async fn app_open_get(
    State(state): State<ApiState>,
    axum::extract::Query(params): axum::extract::Query<AppNavigateParams>,
) -> Response {
    handle_app_navigate(state, params).await
}

async fn app_open_post(
    State(state): State<ApiState>,
    axum::extract::Query(q_params): axum::extract::Query<AppNavigateParams>,
    body: Result<Json<AppNavigateParams>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let params = match body {
        Ok(Json(p)) => AppNavigateParams {
            section: p.section.or(q_params.section),
            tab: p.tab.or(q_params.tab),
        },
        Err(_) => q_params,
    };
    handle_app_navigate(state, params).await
}

async fn app_open_options() -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    let headers = response.headers_mut();
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, header::HeaderValue::from_static("*"));
    headers.insert(header::ACCESS_CONTROL_ALLOW_METHODS, header::HeaderValue::from_static("GET, POST, OPTIONS"));
    headers.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, header::HeaderValue::from_static("Content-Type, Authorization"));
    response
}

/// Serve the API on an already-bound listener.
///
/// Owning this here keeps `axum` an implementation detail of this crate —
/// the CLI and the Tauri shell never depend on the web framework directly.
pub async fn serve(listener: tokio::net::TcpListener, state: ApiState) -> std::io::Result<()> {
    axum::serve(listener, router(state)).await
}


// ---- Discord delivery ----------------------------------------------------
//
// Credentials go in and never come back out: `/discord/status` reports only
// whether one is stored and its last four characters.

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordConnectRequest {
    /// Which credential `value` is.
    pub mode: zoltraak_core::DiscordMode,
    pub value: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordTargetRequest {
    pub guild_id: String,
    pub channel_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscordSendRequest {
    /// Queue item or video job ids. Paths are deliberately not accepted: the
    /// engine already knows where a download landed.
    pub ids: Vec<String>,
    pub options: Option<zoltraak_core::discord::DiscordSendOptions>,
}

/// The service, or a 503 explaining that this build has none.
fn discord_of(state: &ApiState) -> Result<&zoltraak_core::discord::DiscordService, Response> {
    state
        .discord
        .as_ref()
        .ok_or_else(|| reject(StatusCode::SERVICE_UNAVAILABLE, "Discord delivery is not enabled in this build"))
}

async fn discord_status(State(state): State<ApiState>) -> Response {
    let discord = match discord_of(&state) {
        Ok(discord) => discord,
        Err(response) => return response,
    };
    match discord.status().await {
        Ok(status) => Json(status).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn discord_config(State(state): State<ApiState>) -> Response {
    let discord = match discord_of(&state) {
        Ok(discord) => discord,
        Err(response) => return response,
    };
    match discord.settings().await {
        Ok(settings) => Json(settings).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn put_discord_config(
    State(state): State<ApiState>,
    Json(settings): Json<zoltraak_core::DiscordSettings>,
) -> Response {
    let discord = match discord_of(&state) {
        Ok(discord) => discord,
        Err(response) => return response,
    };
    match discord.update_settings(settings).await {
        Ok(stored) => Json(stored).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn discord_connect(
    State(state): State<ApiState>,
    Json(request): Json<DiscordConnectRequest>,
) -> Response {
    let discord = match discord_of(&state) {
        Ok(discord) => discord,
        Err(response) => return response,
    };
    let result = match request.mode {
        zoltraak_core::DiscordMode::Webhook => discord.connect_webhook(&request.value).await,
        zoltraak_core::DiscordMode::Bot => discord.connect_bot(&request.value).await,
    };
    match result {
        Ok(connection) => Json(connection).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn discord_reconnect(State(state): State<ApiState>) -> Response {
    let discord = match discord_of(&state) {
        Ok(discord) => discord,
        Err(response) => return response,
    };
    match discord.reconnect().await {
        Ok(connection) => Json(connection).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn discord_bot_profile_get(
    State(state): State<ApiState>,
    axum::extract::Query(mut request): axum::extract::Query<zoltraak_core::discord::profile::ProfileRequest>,
) -> Response {
    request.fields = None;
    discord_bot_profile_patch(State(state), Json(request)).await
}

async fn discord_bot_profile_patch(
    State(state): State<ApiState>,
    Json(request): Json<zoltraak_core::discord::profile::ProfileRequest>,
) -> Response {
    let discord = match discord_of(&state) { Ok(discord) => discord, Err(response) => return response };
    match discord.bot_profile(request).await { Ok(profile) => Json(profile).into_response(), Err(e) => engine_error(e) }
}

async fn discord_bot_diagnostics(State(state): State<ApiState>) -> Response {
    let discord = match discord_of(&state) { Ok(discord) => discord, Err(response) => return response };
    match discord.bot_diagnostics().await { Ok(result) => Json(result).into_response(), Err(e) => engine_error(e) }
}

async fn discord_disconnect(State(state): State<ApiState>) -> Response {
    let discord = match discord_of(&state) {
        Ok(discord) => discord,
        Err(response) => return response,
    };
    match discord.disconnect().await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => engine_error(e),
    }
}

async fn discord_guilds(State(state): State<ApiState>) -> Response {
    let discord = match discord_of(&state) {
        Ok(discord) => discord,
        Err(response) => return response,
    };
    match discord.guilds().await {
        Ok(guilds) => Json(guilds).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn discord_channels(State(state): State<ApiState>, Path(guild_id): Path<String>) -> Response {
    let discord = match discord_of(&state) {
        Ok(discord) => discord,
        Err(response) => return response,
    };
    match discord.channels(&guild_id).await {
        Ok(channels) => Json(channels).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn discord_set_target(
    State(state): State<ApiState>,
    Json(request): Json<DiscordTargetRequest>,
) -> Response {
    let discord = match discord_of(&state) {
        Ok(discord) => discord,
        Err(response) => return response,
    };
    match discord.set_target(&request.guild_id, &request.channel_id).await {
        Ok(settings) => Json(settings).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn discord_test(State(state): State<ApiState>) -> Response {
    let discord = match discord_of(&state) {
        Ok(discord) => discord,
        Err(response) => return response,
    };
    match discord.test().await {
        Ok(url) => Json(serde_json::json!({ "messageUrl": url })).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn discord_send(State(state): State<ApiState>, Json(request): Json<DiscordSendRequest>) -> Response {
    let discord = match discord_of(&state) {
        Ok(discord) => discord,
        Err(response) => return response,
    };
    let paths = match zoltraak_core::discord::resolve_entries(&state.supervisor, &state.video, state.music.as_ref(), &request.ids).await {
        Ok(paths) => paths,
        Err(e) => return engine_error(e),
    };
    match discord.send_entries(paths, request.options.unwrap_or_default()).await {
        Ok(created) => Json(created).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn discord_deliveries(State(state): State<ApiState>) -> Response {
    let discord = match discord_of(&state) {
        Ok(discord) => discord,
        Err(response) => return response,
    };
    match discord.deliveries(50).await {
        Ok(deliveries) => Json(deliveries).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn discord_retry_delivery(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let discord = match discord_of(&state) {
        Ok(discord) => discord,
        Err(response) => return response,
    };
    match discord.retry(&id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => reject(StatusCode::NOT_FOUND, "no finished delivery with that id"),
        Err(e) => engine_error(e),
    }
}

async fn discord_delete_message(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let discord = match discord_of(&state) { Ok(discord) => discord, Err(response) => return response };
    match discord.delete_message(&id).await {
        Ok(deleted) => Json(deleted).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn discord_clear_deliveries(State(state): State<ApiState>) -> Response {
    let discord = match discord_of(&state) {
        Ok(discord) => discord,
        Err(response) => return response,
    };
    match discord.clear_history().await {
        Ok(removed) => Json(serde_json::json!({ "removed": removed })).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

const GUIDE_THEME_VARS: [(&str, &str); 16] = [
    ("--panel", "--c-panel"),
    ("--face", "--c-panel-face"),
    ("--recess", "--c-panel-recess"),
    ("--panel-sunk", "--c-panel-sunk"),
    ("--edge", "--c-edge"),
    ("--edge-strong", "--c-edge-strong"),
    ("--ink", "--c-ink"),
    ("--ink-dim", "--c-ink-dim"),
    ("--ink-faint", "--c-ink-faint"),
    ("--route", "--c-route"),
    ("--clear", "--c-clear"),
    ("--caution", "--c-caution"),
    ("--danger", "--c-danger"),
    ("--on-signal", "--c-on-signal"),
    ("--lamp-off", "--c-lamp-off"),
    ("--brass", "--c-brass"),
];

/// The corner radius and density the app published, restricted to the values
/// the guide's stylesheet understands.
fn guide_geometry(theme: &serde_json::Value) -> (&'static str, &'static str) {
    let pick = |key: &str, allowed: &[&'static str], default: &'static str| -> &'static str {
        theme
            .get("geometry")
            .and_then(|geometry| geometry.get(key))
            .and_then(serde_json::Value::as_str)
            .and_then(|value| allowed.iter().copied().find(|candidate| *candidate == value))
            .unwrap_or(default)
    };
    (
        pick("radius", &["sharp", "bevel", "modern", "rounded"], "bevel"),
        pick("density", &["compact", "default", "comfortable"], "default"),
    )
}

fn themed_guide(theme: &serde_json::Value) -> String {
    let template = include_str!("../../../scripts/templates/guide.html");
    let (radius, density) = guide_geometry(theme);
    let declarations = theme
        .get("variables")
        .and_then(serde_json::Value::as_object)
        .map(|variables| {
            GUIDE_THEME_VARS
                .iter()
                .filter_map(|(source, target)| {
                    let color = variables.get(*source)?.as_str()?;
                    (color.len() == 7
                        && color.starts_with('#')
                        && color[1..].bytes().all(|b| b.is_ascii_hexdigit()))
                    .then(|| format!("{target}:{color}"))
                })
                .collect::<Vec<_>>()
                .join(";")
        })
        .unwrap_or_default();
    let scheme = match theme.get("colorScheme").and_then(serde_json::Value::as_str) {
        Some("light") => "light",
        _ => "dark",
    };
    let scheme = if declarations.is_empty() { "dark" } else { scheme };
    let html = template.replacen(
        r#"<html lang="en" data-theme="dark""#,
        &format!(r#"<html lang="en" data-radius="{radius}" data-density="{density}" data-theme="{scheme}""#),
        1,
    );
    if declarations.is_empty() {
        return html;
    }
    html.replacen(
        "</head>",
        &format!(
            r#"<style id="app-theme" data-color-scheme="{scheme}">:root{{{declarations}}}</style>
</head>"#,
        ),
        1,
    )
}

async fn guide(State(state): State<ApiState>) -> impl IntoResponse {
    let theme = state
        .guide_theme
        .read()
        .expect("theme lock poisoned")
        .clone();
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        themed_guide(&theme),
    )
}

async fn get_guide_theme(State(state): State<ApiState>) -> Json<serde_json::Value> {
    let theme = state.guide_theme.read().expect("theme lock poisoned").clone();
    let Some(variables) = theme.get("variables").and_then(serde_json::Value::as_object) else {
        return Json(serde_json::Value::Null);
    };
    let safe_variables: serde_json::Map<String, serde_json::Value> = GUIDE_THEME_VARS.iter()
        .filter_map(|(source, _)| {
            let color = variables.get(*source)?.as_str()?;
            (color.len() == 7 && color.starts_with('#') && color[1..].bytes().all(|b| b.is_ascii_hexdigit()))
                .then(|| ((*source).to_owned(), serde_json::Value::String(color.to_owned())))
        }).collect();
    Json(serde_json::json!({
        "colorScheme": if theme.get("colorScheme").and_then(serde_json::Value::as_str) == Some("light") { "light" } else { "dark" },
        "variables": safe_variables,
        "geometry": { "radius": guide_geometry(&theme).0, "density": guide_geometry(&theme).1 },
    }))
}

async fn legal() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        include_str!("../../../scripts/templates/legal.html"),
    )
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairResponse {
    pub token: String,
    pub port: u16,
}

async fn pair_extension(
    State(state): State<ApiState>,
    request: Request,
) -> Response {
    let headers = request.headers();
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());

    // Only browser extensions are ever permitted to pair. Web pages are strictly forbidden.
    if !auth::is_extension_origin(origin) {
        return reject(StatusCode::FORBIDDEN, "origin not permitted; only browser extensions may pair");
    }

    // An origin header proves nothing to a non-browser client, so even the official extension's
    // public ID cannot skip the window: the token is issued only while the user has asked the
    // app to pair (Reload / Install / Repair extension), and only once per request.
    let origin_str = origin.unwrap_or("*");

    if !state.is_pairing_window_active() {
        let mut resp = reject(
            StatusCode::FORBIDDEN,
            "pairing is not open; choose Reload or Repair extension in Zoltraak settings, or use native messaging",
        );
        resp.headers_mut().insert(
            header::ACCESS_CONTROL_ALLOW_ORIGIN,
            header::HeaderValue::from_str(origin_str).unwrap_or(header::HeaderValue::from_static("*")),
        );
        return resp;
    }
    state.close_pairing_window();
    let mut response = (
        StatusCode::OK,
        Json(PairResponse {
            token: state.token.expose().to_string(),
            port: DEFAULT_ADDR.port(),
        }),
    )
        .into_response();

    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        header::HeaderValue::from_str(origin_str).unwrap_or(header::HeaderValue::from_static("*")),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        header::HeaderValue::from_static("POST, OPTIONS"),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        header::HeaderValue::from_static("Content-Type, Authorization"),
    );
    response
}

async fn pair_options(
    request: Request,
) -> Response {
    let headers = request.headers();
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if !auth::is_extension_origin(origin) {
        return reject(StatusCode::FORBIDDEN, "origin not permitted");
    }
    let origin_str = origin.unwrap_or("*");
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        header::HeaderValue::from_str(origin_str).unwrap_or(header::HeaderValue::from_static("*")),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        header::HeaderValue::from_static("POST, OPTIONS"),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        header::HeaderValue::from_static("Content-Type, Authorization"),
    );
    response
}

#[derive(Debug, Deserialize)]
pub struct CheckReloadQuery {
    #[serde(default)]
    pub v: u64,
}

#[derive(Debug, Serialize)]
pub struct CheckReloadResponse {
    pub reload: bool,
    pub version: u64,
}

const EXTENSION_CLIENT_ID_HEADER: &str = "x-zoltraak-client-id";
const EXTENSION_BROWSER_ID_HEADER: &str = "x-zoltraak-browser-id";
const EXTENSION_VERSION_HEADER: &str = "x-zoltraak-extension-version";

#[derive(Debug, Deserialize)]
#[serde(default, rename_all = "camelCase")]
struct ExtensionStatusQuery {
    reload_version: u64,
    source_revision: u64,
}

impl Default for ExtensionStatusQuery {
    fn default() -> Self {
        Self { reload_version: 0, source_revision: 0 }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionStatusResponse {
    reload: bool,
    reload_version: u64,
    source_changed: bool,
    source_revision: u64,
    active_cookie_source: bool,
    /// Always true: kept so extensions built for licensed releases keep capturing.
    licensed: bool,
}

fn extension_heartbeat(headers: &HeaderMap) -> Result<extension_source::ExtensionHeartbeat, Response> {
    let text = |name: &'static str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
    };
    let client_id = text(EXTENSION_CLIENT_ID_HEADER)
        .ok_or_else(|| reject(StatusCode::BAD_REQUEST, "missing extension client id"))?;
    Ok(extension_source::ExtensionHeartbeat {
        client_id,
        browser_id: text(EXTENSION_BROWSER_ID_HEADER).unwrap_or_else(|| "browser".into()),
        extension_version: text(EXTENSION_VERSION_HEADER).unwrap_or_default(),
    })
}

fn is_gecko_client(heartbeat: &extension_source::ExtensionHeartbeat, headers: &HeaderMap) -> bool {
    let b_id = heartbeat.browser_id.to_ascii_lowercase();
    if b_id == "zen" || b_id == "firefox" || b_id.contains("gecko") {
        return true;
    }
    if let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) {
        if origin.starts_with("moz-extension://") {
            return true;
        }
    }
    if let Some(ua) = headers.get(header::USER_AGENT).and_then(|v| v.to_str().ok()) {
        if ua.contains("Firefox") || ua.contains("Gecko") {
            return true;
        }
    }
    false
}

async fn extension_status(
    State(state): State<ApiState>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<ExtensionStatusQuery>,
) -> Response {
    let heartbeat = match extension_heartbeat(&headers) {
        Ok(heartbeat) => heartbeat,
        Err(response) => return response,
    };
    let client_id = heartbeat.client_id.clone();
    let is_gecko = is_gecko_client(&heartbeat, &headers);
    let mut source_rx = state.extension_sources.subscribe();
    let mut reload_rx = state.reload_tx.subscribe();
    let mut source = match state.heartbeat_extension_source(heartbeat.clone()).await {
        Ok(source) => source,
        Err(error) => return reject(StatusCode::BAD_REQUEST, &error),
    };

    let reload_version = state.current_reload_version();
    // Gecko extensions are immutable signed packages; they never hot-reload.
    // Treat reload version as matched so Gecko clients don't abort long-polling.
    let reload_matches = is_gecko || reload_version == query.reload_version;
    if reload_matches && source.revision == query.source_revision {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(25), async {
            if is_gecko {
                let _ = source_rx.changed().await;
            } else {
                tokio::select! {
                    _ = reload_rx.changed() => {},
                    _ = source_rx.changed() => {},
                }
            }
        }).await;
        source = match state.heartbeat_extension_source(heartbeat).await {
            Ok(source) => source,
            Err(error) => return reject(StatusCode::BAD_REQUEST, &error),
        };
    }

    let current_reload = state.current_reload_version();
    let should_reload = !is_gecko && current_reload > query.reload_version;
    Json(ExtensionStatusResponse {
        reload: should_reload,
        reload_version: current_reload,
        source_changed: source.revision > query.source_revision,
        source_revision: source.revision,
        active_cookie_source: source.active_client_id.as_deref() == Some(client_id.as_str()),
        licensed: true,
    })
    .into_response()
}

async fn check_reload_options(request: Request) -> Response {
    let headers = request.headers();
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if !auth::is_extension_origin(origin) {
        return reject(StatusCode::FORBIDDEN, "origin not permitted");
    }
    let origin_str = origin.unwrap_or("*");
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        header::HeaderValue::from_str(origin_str).unwrap_or(header::HeaderValue::from_static("*")),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        header::HeaderValue::from_static("GET, OPTIONS"),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        header::HeaderValue::from_static("Content-Type, Authorization"),
    );
    response
}

async fn check_extension_reload(
    State(state): State<ApiState>,
    request: Request,
) -> Response {
    let headers = request.headers();
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if !auth::is_extension_origin(origin) {
        return reject(StatusCode::FORBIDDEN, "origin not permitted; only browser extensions may check reload status");
    }
    let origin_str = origin.unwrap_or("*");

    let is_gecko = origin.is_some_and(|o| o.starts_with("moz-extension://"))
        || headers.get(header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .map(|ua| ua.contains("Firefox") || ua.contains("Gecko"))
            .unwrap_or(false);

    let query: CheckReloadQuery = axum::extract::Query::try_from_uri(request.uri())
        .map(|axum::extract::Query(q)| q)
        .unwrap_or(CheckReloadQuery { v: 0 });

    let current = state.current_reload_version();
    let (should_reload, final_version) = if is_gecko {
        (false, current)
    } else if query.v < current {
        (true, current)
    } else {
        let mut rx = state.reload_tx.subscribe();
        match tokio::time::timeout(std::time::Duration::from_secs(25), rx.changed()).await {
            Ok(Ok(_)) => {
                let v = *rx.borrow_and_update();
                (v > query.v, v)
            }
            _ => (false, current),
        }
    };

    let mut response = (
        StatusCode::OK,
        Json(CheckReloadResponse {
            reload: should_reload,
            version: final_version,
        }),
    )
        .into_response();

    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        header::HeaderValue::from_str(origin_str).unwrap_or(header::HeaderValue::from_static("*")),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        header::HeaderValue::from_static("GET, OPTIONS"),
    );
    response.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        header::HeaderValue::from_static("Content-Type, Authorization"),
    );
    response
}

/// Rejects anything that is not an authenticated, non-web-page caller.
///
/// Note what is deliberately *absent*: no CORS layer, so no route ever answers
/// with `Access-Control-Allow-Origin`. A page therefore cannot pass a preflight,
/// which means it cannot send an `Authorization` header at all — the origin
/// check below is the second of two independent barriers, not the only one.
async fn guard(State(state): State<ApiState>, request: Request, next: Next) -> Response {
    let headers = request.headers();

    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    if !auth::host_allowed(host) {
        return reject(StatusCode::FORBIDDEN, "host not permitted");
    }

    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if !auth::origin_allowed(origin) {
        return reject(StatusCode::FORBIDDEN, "origin not permitted");
    }

    let presented = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(auth::bearer);

    match presented {
        Some(token) if state.token.verify(token) => next.run(request).await,
        _ => reject(StatusCode::UNAUTHORIZED, "invalid or missing bearer token"),
    }
}

fn reject(status: StatusCode, message: &str) -> Response {
    (
        status,
        Json(ApiError {
            error: message.to_string(),
        }),
    )
        .into_response()
}

/// Map an engine error onto a status a client can act on.
///
/// The message is the engine's own, which can name a local path or a driver
/// detail. That is acceptable here and not in the desktop UI: reaching this
/// route already requires the install's token, so the caller is not a stranger.
fn engine_error(e: EngineError) -> Response {
    let status = match e {
        EngineError::NotFound(_) => StatusCode::NOT_FOUND,
        EngineError::Duplicate { .. } => StatusCode::CONFLICT,
        EngineError::InvalidInput(_) => StatusCode::BAD_REQUEST,
        // An upstream service said no. The request that reached us was fine,
        // so this is a gateway failure rather than our own fault.
        EngineError::ApiRejected { .. } => StatusCode::BAD_GATEWAY,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    reject(status, &e.to_string())
}

fn parse_id(raw: &str) -> Result<DownloadId, StatusCode> {
    raw.parse::<DownloadId>().map_err(|_| StatusCode::BAD_REQUEST)
}

fn parse_category_id(raw: &str) -> Result<i64, StatusCode> {
    raw.parse::<i64>().map_err(|_| StatusCode::BAD_REQUEST)
}

async fn create_download(
    State(state): State<ApiState>,
    request_headers: HeaderMap,
    Json(body): Json<DownloadRequest>,
) -> Response {
    // Only the two schemes the engine can actually fetch. Without this a
    // `file://` URL would turn the API into a local-file reader for anything
    // holding the token.
    let scheme_ok = body.url.starts_with("http://") || body.url.starts_with("https://");
    if !scheme_ok {
        return reject(StatusCode::BAD_REQUEST, "url must be http or https");
    }

    let explicit_directory = body.directory.is_some();
    let directory = match body.directory {
        // A relative path would resolve against the engine's working
        // directory, which the caller cannot see and did not choose.
        Some(dir) if dir.is_absolute() => dir,
        Some(_) => return reject(StatusCode::BAD_REQUEST, "directory must be absolute"),
        None => state.default_directory.as_ref().clone(),
    };

    let mut new = NewDownload::new(body.url, directory);
    new.paused = body.paused;
    new.categorize = !explicit_directory;
    new.connections = body.connections;
    new.priority = body.priority;
    if let Some(name) = body.filename {
        // The browser sends the absolute path it was going to save to, so the
        // name is its last segment — and taking only that is also what keeps a
        // caller from steering the write out of the destination directory.
        if let Some(name) = zoltraak_core::probe::requested_filename(&name) {
            new.filename = name;
        }
    }

    let mut headers = filter_inactive_extension_credentials(&state, &request_headers, body.headers).await;
    // `referrer` is a convenience for clients that think in terms of the page
    // rather than the header; an explicit `Referer` the caller sent wins.
    if let Some(referrer) = body.referrer {
        let already_set = headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("referer"));
        if !already_set {
            headers.push(("Referer".into(), referrer));
        }
    }
    new.headers = engine_headers::sanitize(headers);

    match state.supervisor.enqueue(new).await {
        Ok(id) => (
            StatusCode::CREATED,
            Json(DownloadAccepted { id: id.to_string() }),
        )
            .into_response(),
        Err(e) => engine_error(e),
    }
}

async fn list_downloads(State(state): State<ApiState>) -> Response {
    match state.supervisor.snapshot().await {
        Ok(items) => Json::<Vec<DownloadSnapshot>>(items).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn pause_download(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(status) => return reject(status, "malformed download id"),
    };
    match state.supervisor.pause(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => engine_error(e),
    }
}

async fn resume_download(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(status) => return reject(status, "malformed download id"),
    };
    match state.supervisor.resume(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => engine_error(e),
    }
}

async fn remove_download(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(status) => return reject(status, "malformed download id"),
    };
    match state.supervisor.remove(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => engine_error(e),
    }
}

/// Reveal a finished download (or its in-progress destination) in the OS
/// file manager. Checks the segmented-download queue first, then video jobs.
async fn reveal_download(State(state): State<ApiState>, Json(body): Json<RevealDownloadRequest>) -> Response {
    let id = match parse_id(&body.id) {
        Ok(id) => id,
        Err(status) => return reject(status, "malformed download id"),
    };

    // `reveal` selects the file when it is there and otherwise falls back to
    // the nearest folder that exists, so an in-progress or since-deleted
    // download still lands the user in its destination.
    if let Ok(item) = state.supervisor.queue().get(id).await {
        let path = item.path();
        let result = if path.exists() {
            zoltraak_core::shell::reveal(&path)
        } else {
            zoltraak_core::shell::reveal_dir(&item.directory)
        };
        return match result {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => reject(StatusCode::INTERNAL_SERVER_ERROR, &format!("could not open: {e}")),
        };
    }

    if let Some(job) = state.video.get(id).await {
        if let Some(path) = job.path {
            return match zoltraak_core::shell::reveal(&path) {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(e) => reject(StatusCode::INTERNAL_SERVER_ERROR, &format!("could not open: {e}")),
            };
        }
    }

    reject(StatusCode::NOT_FOUND, "download not found")
}

/// Open a finished download directly with the OS default application.
/// Checks the segmented-download queue first, then video jobs, or directly opens `body.path`.
/// If the target is not a file or does not exist yet, falls back to revealing it.
async fn open_download(State(state): State<ApiState>, Json(body): Json<OpenDownloadRequest>) -> Response {
    if let Some(ref id_str) = body.id {
        if let Ok(id) = parse_id(id_str) {
            if let Ok(item) = state.supervisor.queue().get(id).await {
                let path = item.path();
                if path.is_file() {
                    return match zoltraak_core::shell::open_file(&path) {
                        Ok(()) => StatusCode::NO_CONTENT.into_response(),
                        Err(e) => reject(StatusCode::INTERNAL_SERVER_ERROR, &format!("could not open: {e}")),
                    };
                } else {
                    let result = if path.exists() {
                        zoltraak_core::shell::reveal(&path)
                    } else {
                        zoltraak_core::shell::reveal_dir(&item.directory)
                    };
                    return match result {
                        Ok(()) => StatusCode::NO_CONTENT.into_response(),
                        Err(e) => reject(StatusCode::INTERNAL_SERVER_ERROR, &format!("could not open: {e}")),
                    };
                }
            }

            if let Some(job) = state.video.get(id).await {
                if let Some(path) = job.path {
                    if path.is_file() {
                        return match zoltraak_core::shell::open_file(&path) {
                            Ok(()) => StatusCode::NO_CONTENT.into_response(),
                            Err(e) => reject(StatusCode::INTERNAL_SERVER_ERROR, &format!("could not open: {e}")),
                        };
                    } else {
                        return match zoltraak_core::shell::reveal(&path) {
                            Ok(()) => StatusCode::NO_CONTENT.into_response(),
                            Err(e) => reject(StatusCode::INTERNAL_SERVER_ERROR, &format!("could not open: {e}")),
                        };
                    }
                }
            }
        }
    }

    if let Some(ref path) = body.path {
        if path.is_file() {
            return match zoltraak_core::shell::open_file(path) {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(e) => reject(StatusCode::INTERNAL_SERVER_ERROR, &format!("could not open: {e}")),
            };
        } else {
            // Not a file (yet, or any more) — show where it lives instead.
            return match zoltraak_core::shell::reveal(path) {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(e) => reject(StatusCode::INTERNAL_SERVER_ERROR, &format!("could not open: {e}")),
            };
        }
    }

    reject(StatusCode::NOT_FOUND, "download not found")
}

/// Reveal an arbitrary absolute folder, or the default download directory
/// when no path is given.
async fn reveal_folder(State(state): State<ApiState>, Json(body): Json<RevealFolderRequest>) -> Response {
    // A caller-supplied path is revealed as-is (it may well be a file — the
    // extension passes a download's own path here); only the app's own default
    // directory is ours to create if a fresh install has not made it yet.
    let result = match body.path {
        Some(path) if path.is_absolute() => zoltraak_core::shell::reveal(&path),
        Some(_) => return reject(StatusCode::BAD_REQUEST, "path must be absolute"),
        None => zoltraak_core::shell::reveal_dir(state.default_directory.as_ref()),
    };
    match result {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => reject(StatusCode::INTERNAL_SERVER_ERROR, &format!("could not open: {e}")),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChooseFolderRequest {
    pub start: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChooseFolderResponse {
    pub path: Option<String>,
}

async fn choose_folder_dialog(
    State(state): State<ApiState>,
    Json(body): Json<ChooseFolderRequest>,
) -> Response {
    let Some(picker_tx) = &state.folder_picker_tx else {
        return reject(StatusCode::NOT_IMPLEMENTED, "folder picker not available on this host");
    };

    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    if picker_tx.send((body.start, resp_tx)).is_err() {
        return reject(StatusCode::INTERNAL_SERVER_ERROR, "folder picker channel closed");
    }

    match resp_rx.await {
        Ok(Some(path)) => Json(ChooseFolderResponse {
            path: Some(path.to_string_lossy().into_owned()),
        })
        .into_response(),
        Ok(None) => Json(ChooseFolderResponse { path: None }).into_response(),
        Err(_) => reject(StatusCode::INTERNAL_SERVER_ERROR, "folder picker closed unexpectedly"),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListFoldersRequest {
    pub path: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListFoldersResponse {
    pub current: String,
    pub parent: Option<String>,
    pub folders: Vec<FolderEntry>,
    pub drives: Vec<String>,
}

async fn list_directory_folders(
    State(state): State<ApiState>,
    Json(body): Json<ListFoldersRequest>,
) -> Response {
    #[cfg(windows)]
    let drives = {
        let mut list = Vec::new();
        for letter in b'A'..=b'Z' {
            let drive = format!("{}:\\", letter as char);
            if std::path::Path::new(&drive).exists() {
                list.push(drive);
            }
        }
        list
    };
    #[cfg(not(windows))]
    let drives = vec!["/".to_string()];

    let current_dir = match body.path {
        Some(p) if p.is_absolute() && p.exists() && p.is_dir() => p,
        _ => state.default_directory.as_ref().clone(),
    };

    let parent = current_dir.parent().map(|p| p.to_string_lossy().into_owned());

    let mut folders = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&current_dir) {
        for entry in entries.flatten() {
            if let Ok(file_type) = entry.file_type() {
                if file_type.is_dir() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if !name.starts_with('.') && !name.starts_with('$') {
                        folders.push(FolderEntry {
                            name,
                            path: entry.path().to_string_lossy().into_owned(),
                        });
                    }
                }
            }
        }
    }
    folders.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

    Json(ListFoldersResponse {
        current: current_dir.to_string_lossy().into_owned(),
        parent,
        folders,
        drives,
    })
    .into_response()
}

async fn create_category(State(state): State<ApiState>, Json(body): Json<CategoryRequest>) -> Response {
    if !body.directory.is_absolute() {
        return reject(StatusCode::BAD_REQUEST, "directory must be absolute");
    }

    let result = state
        .supervisor
        .queue()
        .add_category(NewCategory {
            name: body.name,
            kind: body.kind,
            pattern: body.pattern,
            directory: body.directory,
            priority: body.priority,
        })
        .await;

    match result {
        Ok(id) => (StatusCode::CREATED, Json(CategoryCreated { id })).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn list_categories(State(state): State<ApiState>) -> Response {
    match state.supervisor.queue().list_categories().await {
        Ok(categories) => Json::<Vec<Category>>(categories).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn enable_category(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    set_category_enabled(state, id, true).await
}

async fn disable_category(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    set_category_enabled(state, id, false).await
}

async fn set_category_enabled(state: ApiState, id: String, enabled: bool) -> Response {
    let id = match parse_category_id(&id) {
        Ok(id) => id,
        Err(status) => return reject(status, "malformed category id"),
    };
    match state.supervisor.queue().set_category_enabled(id, enabled).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => engine_error(e),
    }
}

async fn remove_category(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let id = match parse_category_id(&id) {
        Ok(id) => id,
        Err(status) => return reject(status, "malformed category id"),
    };
    match state.supervisor.queue().remove_category(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => engine_error(e),
    }
}

async fn get_settings(State(state): State<ApiState>) -> Response {
    let settings = state.supervisor.settings();
    let resolved_dir = settings
        .default_directory
        .clone()
        .unwrap_or_else(|| state.default_directory.as_ref().clone());
    let mut val = match serde_json::to_value(&settings) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    val.insert(
        "resolvedDefaultDirectory".to_string(),
        serde_json::Value::String(resolved_dir.to_string_lossy().into_owned()),
    );
    Json(val).into_response()
}

async fn get_extension_theme(State(state): State<ApiState>) -> Json<serde_json::Value> {
    Json(state.extension_theme.read().expect("theme lock poisoned").clone())
}

async fn put_settings(State(state): State<ApiState>, Json(body): Json<EngineSettings>) -> Response {
    if let Some(dir) = &body.default_directory {
        if !dir.is_absolute() {
            return reject(StatusCode::BAD_REQUEST, "default_directory must be absolute");
        }
    }
    match state.supervisor.update_settings(body).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => engine_error(e),
    }
}

/// Same convenience as `create_download`'s `referrer` field: a client that
/// thinks in terms of the page rather than the header gets one added, unless
/// it already sent its own `Referer`.
fn with_referrer(mut headers: Vec<(String, String)>, referrer: Option<String>) -> Vec<(String, String)> {
    if let Some(referrer) = referrer {
        let already_set = headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("referer"));
        if !already_set {
            headers.push(("Referer".into(), referrer));
        }
    }
    headers
}

fn is_browser_credential(name: &str) -> bool {
    name.eq_ignore_ascii_case("cookie")
        || name.eq_ignore_ascii_case("authorization")
        || name.eq_ignore_ascii_case(zoltraak_core::video::youtube::POTOKEN_HEADER)
        || name.eq_ignore_ascii_case(zoltraak_core::video::youtube::VISITOR_HEADER)
        || name.eq_ignore_ascii_case(zoltraak_core::video::youtube::POTOKEN_VIDEO_HEADER)
}

async fn filter_inactive_extension_credentials(
    state: &ApiState,
    request_headers: &HeaderMap,
    mut supplied: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let origin = request_headers.get(header::ORIGIN).and_then(|value| value.to_str().ok());
    let client_id = request_headers
        .get(EXTENSION_CLIENT_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let is_extension = auth::is_extension_origin(origin) || client_id.is_some();
    if !is_extension {
        return supplied;
    }
    let active = match client_id {
        Some(client_id) => state
            .extension_source_state()
            .await
            .is_ok_and(|source| source.active_client_id.as_deref() == Some(client_id)),
        None => false,
    };
    if !active {
        supplied.retain(|(name, _)| !is_browser_credential(name));
    }
    supplied
}

/// Hand a URL to the desktop app's downloads text input and bring its window forward.
async fn prefill_download(State(state): State<ApiState>, request_headers: HeaderMap, Json(body): Json<DownloadPrefillRequest>) -> Response {
    let scheme_ok = body.url.starts_with("http://") || body.url.starts_with("https://");
    if !scheme_ok {
        return reject(StatusCode::BAD_REQUEST, "url must be http or https");
    }
    if let Some(tx) = &state.download_prefill_tx {
        let supplied = filter_inactive_extension_credentials(&state, &request_headers, body.headers).await;
        let headers = engine_headers::sanitize(with_referrer(supplied, body.referrer));
        let _ = tx.send((body.url, headers));
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Hand a URL to the desktop app's Video view and bring its window forward, so
/// the user lands on the Video page with the address already filled in rather
/// than downloading headlessly. Returns `204` even when no shell is listening
/// (the headless CLI/server has no window), so the caller need not special-case
/// that — the request is a UI hint, not a download command.
async fn prefill_video(State(state): State<ApiState>, request_headers: HeaderMap, Json(body): Json<VideoPrefillRequest>) -> Response {
    let scheme_ok = body.url.starts_with("http://") || body.url.starts_with("https://");
    if !scheme_ok {
        return reject(StatusCode::BAD_REQUEST, "url must be http or https");
    }
    if let Some(tx) = &state.prefill_tx {
        let supplied = filter_inactive_extension_credentials(&state, &request_headers, body.headers).await;
        let headers = engine_headers::sanitize(with_referrer(supplied, body.referrer));
        // The receiver being gone (window closed) is not an error worth
        // surfacing to the extension — the app simply isn't there to fill.
        let _ = tx.send((body.url, headers, body.auto_download, body.title));
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Hand a post URL to the desktop app's Images view and bring its window forward.
async fn prefill_images(State(state): State<ApiState>, request_headers: HeaderMap, Json(body): Json<ImageAnalyzeRequest>) -> Response {
    let scheme_ok = body.url.starts_with("http://") || body.url.starts_with("https://");
    if !scheme_ok {
        return reject(StatusCode::BAD_REQUEST, "url must be http or https");
    }
    if !body.browser_documents.is_empty() {
        // A browser snapshot carries account-visible media, so it follows the
        // same profile ownership rules as the shared session vault.
        let _transition = state.extension_source_switch.lock().await;
        let client_id = request_headers.get(EXTENSION_CLIENT_ID_HEADER).and_then(|v| v.to_str().ok());
        let origin = request_headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
        if auth::is_extension_origin(origin) || client_id.is_some() {
            let mutation = state.extension_sources.refresh();
            let active = state.finish_extension_source_mutation(mutation).await
                .is_ok_and(|s| s.active_client_id.as_deref() == client_id && client_id.is_some());
            if !active { return reject(StatusCode::LOCKED, "this browser is not the active cookie source"); }
        }
        if let Err(error) = state.video.store_facebook_post(&body.url, &body.browser_documents).await { return engine_error(error); }
    }
    if let Some(tx) = &state.image_prefill_tx {
        let supplied = filter_inactive_extension_credentials(&state, &request_headers, body.headers).await;
        let headers = engine_headers::sanitize(with_referrer(supplied, body.referrer));
        let _ = tx.send((body.url, headers));
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Let the extension keep the engine's session vault fresh for a login-gated
/// platform, just from ordinary browsing — not only from an explicit
/// "Download with Zoltraak" click. This is what lets a URL typed or pasted
/// straight into the Video view (no extension action involved at all) still
/// resolve without hitting a login wall, as long as the browser has a session
/// for that site. A request with nothing usable in it (the browser signed
/// out) removes the stored session rather than leaving a stale one behind.
async fn set_session(
    State(state): State<ApiState>,
    headers: HeaderMap,
    Json(body): Json<SessionRequest>,
) -> Response {
    if body.domain.trim().is_empty() {
        return reject(StatusCode::BAD_REQUEST, "domain must not be empty");
    }
    let origin = headers.get(header::ORIGIN).and_then(|value| value.to_str().ok());
    let client_id = headers
        .get(EXTENSION_CLIENT_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if auth::is_extension_origin(origin) && client_id.is_none() {
        return reject(StatusCode::PRECONDITION_REQUIRED, "update the extension before sharing browser sessions");
    }
    if let Some(client_id) = client_id {
        let source = match state.extension_source_state().await {
            Ok(source) => source,
            Err(error) => return engine_error(error),
        };
        if source.active_client_id.as_deref() != Some(client_id) {
            return reject(StatusCode::LOCKED, "this browser is not the active cookie source");
        }
    }
    // Explicit fields win; the header list is the older extension's shape.
    let mut session = SiteSession::from_headers(&engine_headers::sanitize(body.headers));
    if let Some(cookie) = body.cookie.filter(|c| !c.trim().is_empty()) {
        session.cookie = Some(cookie);
    }
    if let Some(token) = body.po_token.filter(|t| !t.trim().is_empty()) {
        session.po_token = Some(token);
    }
    if let Some(visitor) = body.visitor_data.filter(|v| !v.trim().is_empty()) {
        session.visitor_data = Some(visitor);
    }
    if let Some(video_id) = body.po_token_video_id.filter(|v| !v.trim().is_empty()) {
        session.po_token_video_id = Some(video_id);
    }
    match state.video.store_session(&body.domain, session).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => engine_error(e),
    }
}

/// The stored sessions: which sites, when, and which cookie *names* — never
/// a value. The extension popup and the app's settings both render this.
async fn list_sessions(State(state): State<ApiState>) -> Response {
    Json::<Vec<SessionSummary>>(state.video.sessions().await).into_response()
}

async fn forget_session(State(state): State<ApiState>, Path(domain): Path<String>) -> Response {
    match state.video.forget_session(&domain).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => reject(StatusCode::NOT_FOUND, "no session stored for that site"),
        Err(e) => engine_error(e),
    }
}

async fn forget_all_sessions(State(state): State<ApiState>) -> Response {
    match state.video.forget_all_sessions().await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => engine_error(e),
    }
}

async fn analyze_video(State(state): State<ApiState>, request_headers: HeaderMap, Json(body): Json<VideoAnalyzeRequest>) -> Response {
    let scheme_ok = body.url.starts_with("http://") || body.url.starts_with("https://");
    if !scheme_ok {
        return reject(StatusCode::BAD_REQUEST, "url must be http or https");
    }
    if body.source_kind.as_deref().is_some_and(|kind| !["file", "hls", "dash", "page"].contains(&kind))
        || body.media_type.as_deref().is_some_and(|kind| !["audio", "video"].contains(&kind)) {
        return reject(StatusCode::BAD_REQUEST, "unsupported media source kind or type");
    }

    let supplied = filter_inactive_extension_credentials(&state, &request_headers, body.headers).await;
    let headers = engine_headers::sanitize(with_referrer(supplied, body.referrer));
    match state.video.analyze_source(&body.url, &headers, body.use_fallback, body.source_kind.as_deref(), body.media_type.as_deref()).await {
        Ok(metadata) => Json(metadata).into_response(),
        Err(e) => engine_error(e),
    }
}

/// The pictures in a post. There is no matching `/image/download`: each item
/// is a plain file, so the client queues it through `POST /download` like any
/// other link — carrying the item's own `headers`, which some CDNs require.
async fn analyze_images(State(state): State<ApiState>, request_headers: HeaderMap, Json(body): Json<ImageAnalyzeRequest>) -> Response {
    let scheme_ok = body.url.starts_with("http://") || body.url.starts_with("https://");
    if !scheme_ok {
        return reject(StatusCode::BAD_REQUEST, "url must be http or https");
    }

    let supplied = filter_inactive_extension_credentials(&state, &request_headers, body.headers).await;
    let headers = engine_headers::sanitize(with_referrer(supplied, body.referrer));
    match state.video.analyze_images(&body.url, &headers).await {
        Ok(album) => Json::<ImageAlbum>(album).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn download_video(State(state): State<ApiState>, request_headers: HeaderMap, Json(body): Json<VideoDownloadRequest>) -> Response {
    let directory = match body.directory {
        Some(dir) if dir.is_absolute() => dir,
        Some(_) => return reject(StatusCode::BAD_REQUEST, "directory must be absolute"),
        None => state.default_directory.as_ref().clone(),
    };

    let supplied = filter_inactive_extension_credentials(&state, &request_headers, body.headers).await;
    let headers = engine_headers::sanitize(with_referrer(supplied, body.referrer));
    let new = NewVideoDownload {
        title: body.title,
        webpage_url: body.webpage_url,
        format: body.format,
        directory,
        headers,
    };

    match state.video.start(new).await {
        Ok(id) => {
            if body.auto_discord {
                if let Some(ref auto_send) = state.auto_send {
                    auto_send.write().await.insert(id.to_string());
                }
            }
            (StatusCode::CREATED, Json(VideoJobAccepted { id: id.to_string() })).into_response()
        }
        Err(e) => engine_error(e),
    }
}

async fn list_video_jobs(State(state): State<ApiState>) -> Response {
    Json::<Vec<VideoJob>>(state.video.list().await).into_response()
}

/// Stop a running job at its next resumable boundary.
///
/// `200` whether or not anything was actually running: pausing an
/// already-paused job is the state the caller asked for, not an error.
async fn pause_video_job(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(status) => return reject(status, "malformed video job id"),
    };
    state.video.cancel(id).await;
    StatusCode::NO_CONTENT.into_response()
}

async fn resume_video_job(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(status) => return reject(status, "malformed video job id"),
    };
    match state.video.resume(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => engine_error(e),
    }
}

async fn remove_video_job(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(status) => return reject(status, "malformed video job id"),
    };
    match state.video.remove(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => reject(StatusCode::NOT_FOUND, "video job not found"),
        Err(e) => engine_error(e),
    }
}

// ---- Music ---------------------------------------------------------------
//
// A link (or a search) resolves to a set of tracks; the client hands back the
// ones it wants with the output options, and each becomes a job. Analysis is
// stateless, the same as video.

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicAnalyzeRequest {
    /// A Spotify/Apple Music/Deezer/YouTube/SoundCloud/Bandcamp link, a direct
    /// audio link, or — when it is not a link — words to search for.
    pub input: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    pub referrer: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicDownloadRequest {
    /// The analyzed source, holding only the tracks to download.
    pub source: MusicSource,
    #[serde(default)]
    pub options: OutputOptions,
    /// Absolute path, same reasoning as `DownloadRequest::directory`. Also
    /// honoured inside `options.directory`; this is the fallback.
    pub directory: Option<PathBuf>,
    #[serde(default)]
    pub auto_discord: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicPrefillRequest {
    pub url: String,
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    pub referrer: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicResumeQuery {
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicTools {
    pub ffmpeg: Option<String>,
    pub ffprobe: bool,
}

fn music_of(state: &ApiState) -> Result<&MusicManager, Response> {
    state
        .music
        .as_ref()
        .ok_or_else(|| reject(StatusCode::SERVICE_UNAVAILABLE, "music capture is not enabled in this build"))
}

async fn analyze_music(State(state): State<ApiState>, request_headers: HeaderMap, Json(body): Json<MusicAnalyzeRequest>) -> Response {
    let music = match music_of(&state) {
        Ok(m) => m,
        Err(r) => return r,
    };
    let input = body.input.trim().to_string();
    if input.is_empty() {
        return reject(StatusCode::BAD_REQUEST, "input must be a link or a search");
    }
    let supplied = filter_inactive_extension_credentials(&state, &request_headers, body.headers).await;
    let headers = engine_headers::sanitize(with_referrer(supplied, body.referrer));
    match music.analyze(&input, &headers).await {
        Ok(source) => Json(source).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn download_music(State(state): State<ApiState>, Json(body): Json<MusicDownloadRequest>) -> Response {
    let music = match music_of(&state) {
        Ok(m) => m,
        Err(r) => return r,
    };
    let base_directory = match body.directory {
        Some(dir) if dir.is_absolute() => dir,
        Some(_) => return reject(StatusCode::BAD_REQUEST, "directory must be absolute"),
        None => state.default_directory.join("Music"),
    };
    let mut options = body.options;
    if let Some(dir) = &options.directory {
        if !dir.is_absolute() {
            return reject(StatusCode::BAD_REQUEST, "options.directory must be absolute");
        }
    }
    if options.filename_template.trim().is_empty() {
        options.filename_template = zoltraak_core::music::name::DEFAULT_TEMPLATE.into();
    }
    match music.enqueue(NewMusicDownload { source: body.source, options, base_directory }).await {
        Ok(enqueued) => {
            if body.auto_discord {
                if let Some(ref auto_send) = state.auto_send {
                    let mut set = auto_send.write().await;
                    for id in &enqueued.job_ids {
                        set.insert(id.to_string());
                    }
                }
            }
            (StatusCode::CREATED, Json(enqueued)).into_response()
        }
        Err(e) => engine_error(e),
    }
}

/// Hand a link to the desktop app's Music view. A no-op in the headless
/// server, which has no view to fill.
async fn prefill_music(State(state): State<ApiState>, request_headers: HeaderMap, Json(body): Json<MusicPrefillRequest>) -> Response {
    let scheme_ok = body.url.starts_with("http://") || body.url.starts_with("https://") || body.url.starts_with("spotify:");
    if !scheme_ok {
        return reject(StatusCode::BAD_REQUEST, "url must be http, https or a spotify: URI");
    }
    let supplied = filter_inactive_extension_credentials(&state, &request_headers, body.headers).await;
    let headers = engine_headers::sanitize(with_referrer(supplied, body.referrer));
    if let Some(tx) = &state.music_prefill_tx {
        let _ = tx.send((body.url, headers));
    }
    StatusCode::ACCEPTED.into_response()
}

async fn list_music_jobs(State(state): State<ApiState>) -> Response {
    match music_of(&state) {
        Ok(m) => Json::<Vec<MusicJob>>(m.list().await).into_response(),
        Err(r) => r,
    }
}

async fn clear_music_done(State(state): State<ApiState>) -> Response {
    let music = match music_of(&state) {
        Ok(m) => m,
        Err(r) => return r,
    };
    match music.clear_done().await {
        Ok(n) => Json(serde_json::json!({ "removed": n })).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn remove_music_job(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let music = match music_of(&state) {
        Ok(m) => m,
        Err(r) => return r,
    };
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(status) => return reject(status, "malformed music job id"),
    };
    match music.remove(id).await {
        Ok(true) => StatusCode::NO_CONTENT.into_response(),
        Ok(false) => reject(StatusCode::NOT_FOUND, "music job not found"),
        Err(e) => engine_error(e),
    }
}

async fn pause_music_job(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let music = match music_of(&state) {
        Ok(m) => m,
        Err(r) => return r,
    };
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(status) => return reject(status, "malformed music job id"),
    };
    music.pause(id).await;
    StatusCode::NO_CONTENT.into_response()
}

async fn resume_music_job(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let music = match music_of(&state) {
        Ok(m) => m,
        Err(r) => return r,
    };
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(status) => return reject(status, "malformed music job id"),
    };
    match music.resume(id, false).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => engine_error(e),
    }
}

async fn remove_music_collection(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let music = match music_of(&state) {
        Ok(m) => m,
        Err(r) => return r,
    };
    match music.remove_collection(&id).await {
        Ok(n) => Json(serde_json::json!({ "removed": n })).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn retry_music_collection(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let music = match music_of(&state) {
        Ok(m) => m,
        Err(r) => return r,
    };
    match music.retry_collection(&id).await {
        Ok(n) => Json(serde_json::json!({ "requeued": n })).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn get_music_settings(State(state): State<ApiState>) -> Response {
    match music_of(&state) {
        Ok(m) => Json::<MusicSettings>(m.settings().await).into_response(),
        Err(r) => r,
    }
}

async fn put_music_settings(State(state): State<ApiState>, Json(body): Json<MusicSettings>) -> Response {
    let music = match music_of(&state) {
        Ok(m) => m,
        Err(r) => return r,
    };
    if let Some(dir) = &body.output.directory {
        if !dir.is_absolute() {
            return reject(StatusCode::BAD_REQUEST, "output.directory must be absolute");
        }
    }
    match music.update_settings(body).await {
        Ok(settings) => Json(settings).into_response(),
        Err(e) => engine_error(e),
    }
}

async fn music_tools() -> Response {
    Json(MusicTools {
        ffmpeg: zoltraak_core::music::convert::ffmpeg_version().await,
        ffprobe: zoltraak_core::music::convert::ffprobe_available().await,
    })
    .into_response()
}

// ---- Remux ---------------------------------------------------------------
//
// The one part of the engine whose input is a local path rather than a URL.
// Paths are taken as given: this API already requires the install's token, and
// a caller that holds it can read the disk by other means anyway.

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaProbeRequest {
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemuxRequest {
    pub paths: Vec<PathBuf>,
    /// `mp4`, `mkv`, `mov` or `webm`.
    pub container: String,
    /// Absent writes into Converted under the current configured working folder.
    pub directory: Option<PathBuf>,
    #[serde(default)]
    pub replace_source: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RemuxAccepted {
    pub ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConversionRequest {
    paths: Vec<PathBuf>,
    settings: zoltraak_core::conversion::Settings,
    directory: Option<PathBuf>,
    #[serde(default)]
    replace_source: bool,
}

async fn conversion_capabilities() -> Response {
    match zoltraak_core::conversion::capabilities().await {
        Ok(value) => Json(value).into_response(),
        Err(error) => engine_error(error),
    }
}

async fn plan_conversions(Json(body): Json<ConversionRequest>) -> Response {
    if body.paths.is_empty() || body.paths.len() > 500 {
        return reject(StatusCode::BAD_REQUEST, "Choose between 1 and 500 files");
    }
    let mut plans = Vec::new();
    for path in body.paths {
        let result = match zoltraak_core::conversion::inspect_for_plan(&path).await {
            Ok(info) => body.settings.plan(&info),
            Err(error) => Err(error),
        };
        plans.push(match result {
            Ok(plan) => serde_json::json!({"path":path,"plan":plan,"error":null}),
            Err(error) => serde_json::json!({"path":path,"plan":null,"error":error.to_string()}),
        });
    }
    Json(plans).into_response()
}

async fn create_conversion_jobs(
    State(state): State<ApiState>,
    Json(body): Json<ConversionRequest>,
) -> Response {
    if body.paths.is_empty() || body.paths.len() > 500 {
        return reject(StatusCode::BAD_REQUEST, "Choose between 1 and 500 files");
    }
    if body
        .directory
        .as_ref()
        .is_some_and(|path| !path.is_absolute())
    {
        return reject(StatusCode::BAD_REQUEST, "directory must be absolute");
    }
    let request = NewRemux {
        inputs: body.paths,
        container: body.settings.format.container().unwrap_or(Container::Mp4),
        output_directory: body.directory.or_else(|| Some(conversion_directory(&state))),
        replace_source: body.replace_source,
    };
    match state.remux.enqueue_conversion(request, body.settings).await {
        Ok(ids) => (
            StatusCode::CREATED,
            Json(RemuxAccepted {
                ids: ids.into_iter().map(|id| id.to_string()).collect(),
            }),
        )
            .into_response(),
        Err(error) => engine_error(error),
    }
}

fn conversion_directory(state: &ApiState) -> PathBuf {
    zoltraak_core::conversion::default_output_directory(
        &state.supervisor.settings().default_directory.unwrap_or_else(||state.default_directory.as_ref().clone()),
    )
}

/// Describe local files without starting anything, so a client can show what
/// a conversion would do before the user commits to it.
async fn probe_media(State(state): State<ApiState>, Json(body): Json<MediaProbeRequest>) -> Response {
    let _ = &state;
    let mut out: Vec<MediaInfo> = Vec::with_capacity(body.paths.len());
    for path in body.paths {
        match zoltraak_core::remux::probe(&path).await {
            Ok(info) => out.push(info),
            Err(e) => return engine_error(e),
        }
    }
    Json(out).into_response()
}

async fn create_remux_jobs(State(state): State<ApiState>, Json(body): Json<RemuxRequest>) -> Response {
    let Some(container) = Container::parse(&body.container) else {
        return reject(StatusCode::BAD_REQUEST, "container must be one of mp4, mkv, mov, webm");
    };
    if let Some(dir) = &body.directory {
        if !dir.is_absolute() {
            return reject(StatusCode::BAD_REQUEST, "directory must be absolute");
        }
    }

    let new = NewRemux {
        inputs: body.paths,
        container,
        output_directory: body.directory.or_else(|| Some(conversion_directory(&state))),
        replace_source: body.replace_source,
    };
    match state.remux.enqueue(new).await {
        Ok(ids) => (
            StatusCode::CREATED,
            Json(RemuxAccepted { ids: ids.iter().map(ToString::to_string).collect() }),
        )
            .into_response(),
        Err(e) => engine_error(e),
    }
}

async fn list_remux_jobs(State(state): State<ApiState>) -> Response {
    Json::<Vec<RemuxJob>>(state.remux.list().await).into_response()
}

async fn clear_remux_done(State(state): State<ApiState>) -> Response {
    Json(serde_json::json!({ "removed": state.remux.clear_done().await })).into_response()
}

async fn remove_remux_job(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(status) => return reject(status, "malformed remux job id"),
    };
    match state.remux.remove(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => engine_error(e),
    }
}

async fn cancel_remux_job(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(status) => return reject(status, "malformed remux job id"),
    };
    match state.remux.cancel(id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => engine_error(e),
    }
}

async fn retry_remux_job(State(state): State<ApiState>, Path(id): Path<String>) -> Response {
    let id = match parse_id(&id) {
        Ok(id) => id,
        Err(status) => return reject(status, "malformed remux job id"),
    };
    match state.remux.retry(id, None).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => engine_error(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_bind_is_loopback() {
        assert!(DEFAULT_ADDR.ip().is_loopback());
        assert_eq!(DEFAULT_ADDR.port(), 1450);
    }

    #[test]
    fn guide_uses_only_valid_colors_from_the_current_app_theme() {
        let html = themed_guide(&serde_json::json!({
            "colorScheme": "light",
            "variables": {
                "--panel": "#abcdef",
                "--route": "url(https://example.test/not-css)",
            }
        }));
        assert!(html.contains(r#"data-theme="light""#));
        assert!(html.contains("--c-panel:#abcdef"));
        assert!(!html.contains("not-css"));
    }

    #[test]
    fn guide_geometry_accepts_only_known_settings() {
        let html = themed_guide(&serde_json::json!({
            "geometry": {"radius": "modern", "density": "compact"}
        }));
        assert!(html.contains(r#"data-radius="modern""#));
        assert!(html.contains(r#"data-density="compact""#));

        let html = themed_guide(&serde_json::json!({
            "geometry": {"radius": "\"><script>", "density": 7}
        }));
        assert!(html.contains(r#"data-radius="bevel""#));
        assert!(html.contains(r#"data-density="default""#));
        assert!(!html.contains("<script>alert"));
    }
}
