//! Headless entry point. Deliberately does not depend on Tauri, so
//! `cargo build -p zoltraak-cli` produces a single binary suitable for a
//! container, a NAS, or a homelab box.

use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::{Parser, Subcommand};
use zoltraak_api::{ApiState, ApiToken, DEFAULT_ADDR};
use zoltraak_core::downloader::{Options, Outcome, Request};
use zoltraak_core::model::DownloadState;
use zoltraak_core::runner::RunnerConfig;
use zoltraak_core::{
    DownloadId, Downloader, NewDownload, Queue, Runner, Supervisor, progress_channel,
};
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(name = "zoltraak", version, about = "Zoltraak download engine (headless)")]
struct Cli {
    /// Queue database.
    #[arg(long, global = true, default_value = "zoltraak.db")]
    db: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Download a single URL immediately, bypassing the queue.
    Get {
        url: String,
        /// Destination directory.
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
        /// Connection count. Defaults to the size-based tier.
        #[arg(short, long)]
        connections: Option<u8>,
        /// Override the filename derived from the server or URL.
        #[arg(long)]
        filename: Option<String>,
    },
    /// Add a URL to the queue without starting it.
    Add {
        url: String,
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
        #[arg(short, long)]
        connections: Option<u8>,
        #[arg(long)]
        filename: Option<String>,
        /// Higher runs first.
        #[arg(short, long, default_value_t = 0)]
        priority: i64,
        /// Use --output as-is, even if a category rule would otherwise claim
        /// this download.
        #[arg(long)]
        no_categorize: bool,
    },
    /// Show the queue.
    List,
    /// Manage rule-based destination routing.
    #[command(subcommand)]
    Category(CategoryCommand),
    /// Work the queue until it is empty.
    Run {
        /// Downloads running at once.
        #[arg(short = 'j', long, default_value_t = 3)]
        concurrency: usize,
        #[arg(short, long)]
        connections: Option<u8>,
    },
    /// Move a paused or failed item back into the queue.
    Retry { id: String },
    /// Remove an item from the queue.
    Remove { id: String },
    /// Run the REST API for extensions and automation.
    ///
    /// Unlike `run`, this stays up with an idle queue and works whatever
    /// arrives over the API.
    Serve {
        #[arg(long, default_value_t = DEFAULT_ADDR)]
        addr: SocketAddr,
        /// Where downloads land when a request names no directory.
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
        /// Downloads running at once.
        #[arg(short = 'j', long, default_value_t = 3)]
        concurrency: usize,
    },
    /// Print the API token an extension or script should present.
    Token,
    /// Browser sessions the extension has shared: list them or forget them.
    #[command(subcommand)]
    Sessions(SessionsCommand),
    /// View or change engine-wide settings: bandwidth cap, connection and
    /// folder defaults, the scheduler window, and the on-completion action.
    #[command(subcommand)]
    Settings(SettingsCommand),
    /// Send finished files to a Discord channel, and manage the webhook or
    /// bot that does it.
    #[command(subcommand)]
    Discord(DiscordCommand),
    /// Download music: a Spotify, Apple Music, Deezer, YouTube, SoundCloud or
    /// Bandcamp link (a track, album or playlist), a direct audio link, or a
    /// search by name.
    #[command(subcommand)]
    Music(MusicCommand),
}

#[derive(Subcommand)]
enum MusicCommand {
    /// Resolve a link (or search) and download every track it holds.
    Get {
        /// A link, or words to search YouTube Music for.
        input: String,
        /// Where the files go. An album or playlist gets a folder of its own
        /// inside it unless --no-folder is given.
        #[arg(short, long, default_value = ".")]
        output: PathBuf,
        /// best (keep the stream as it is), mp3, m4a, opus, ogg, flac, wav, alac.
        #[arg(short, long, default_value = "best")]
        format: String,
        /// kbps, for lossy formats. Defaults per format (mp3 320, m4a 256, opus 160).
        #[arg(short, long)]
        bitrate: Option<u32>,
        /// Save into --output directly, without a folder named after the collection.
        #[arg(long)]
        no_folder: bool,
        /// Skip the metadata and cover-art tagging.
        #[arg(long)]
        no_tags: bool,
        /// Save the cover beside the files: cover.jpg and folder.jpg for an
        /// album or playlist, a picture next to a single track.
        #[arg(long)]
        cover_file: bool,
        /// Do not look cover art up on Apple Music or Deezer when the source
        /// has none, or has only a video still.
        #[arg(long)]
        no_artwork: bool,
        /// Fetch lyrics from lrclib.net and write them into the file (and an .lrc when synced).
        #[arg(long)]
        lyrics: bool,
        /// Tracks fetched at once.
        #[arg(short = 'j', long, default_value_t = 3)]
        concurrency: usize,
        /// Only list what the link holds; download nothing.
        #[arg(long)]
        list: bool,
    },
    /// The music jobs the database holds, newest first.
    Jobs,
}

#[derive(Subcommand)]
enum DiscordCommand {
    /// Whether Discord delivery is set up, and where it is pointing.
    Status,
    /// Store a webhook URL. The channel comes with it.
    ConnectWebhook {
        /// Omit to read the URL from stdin, which keeps it out of shell history.
        url: Option<String>,
    },
    /// Store a bot token, then pick a target with `discord target`.
    ConnectBot {
        /// Omit to read the token from stdin, which keeps it out of shell history.
        token: Option<String>,
    },
    /// Forget the stored credential and stop delivering.
    Disconnect,
    /// Servers the bot has been invited to.
    Guilds,
    /// Channels in a server that the bot can post files to.
    Channels { guild_id: String },
    /// Choose where bot-mode deliveries go, and read the server upload limit.
    Target { guild_id: String, channel_id: String },
    /// The link that adds the bot to a server.
    Invite,
    /// Post a short message to prove the setup works.
    Test,
    /// Send one or more files now.
    Send { paths: Vec<PathBuf> },
    /// What has been sent, skipped or failed.
    Deliveries,
    /// Try a finished delivery again.
    Retry { id: String },
}

#[derive(Subcommand)]
enum SessionsCommand {
    /// Which sites have a stored session, and which cookie names — never
    /// values.
    List,
    /// Forget one site's session, or every session with no domain given.
    Forget { domain: Option<String> },
}

#[derive(Subcommand)]
enum SettingsCommand {
    /// Print the current settings as JSON.
    Show,
    /// Update one or more settings. Flags left out are unchanged.
    Set {
        /// Bytes per second. 0 clears the limit (unlimited).
        #[arg(long)]
        bandwidth: Option<u64>,
        #[arg(long)]
        connections: Option<u8>,
        #[arg(long)]
        folder: Option<PathBuf>,
        /// "HH:MM-HH:MM" (e.g. 22:00-06:00), or "off" to disable the window.
        #[arg(long)]
        schedule: Option<String>,
        /// "none", "open-folder", "exit", or a command to run on completion.
        #[arg(long = "on-complete")]
        on_complete: Option<String>,
    },
}

#[derive(Subcommand)]
enum CategoryCommand {
    /// Add a filing rule. `kind` is `extension`, `domain`, or `regex`.
    Add {
        name: String,
        #[arg(long)]
        kind: String,
        pattern: String,
        directory: PathBuf,
        /// Higher runs first when more than one rule could match.
        #[arg(short, long, default_value_t = 0)]
        priority: i64,
    },
    /// List filing rules, highest priority first.
    List,
    Enable { id: i64 },
    Disable { id: i64 },
    Remove { id: i64 },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "zoltraak=info".into()),
        )
        .init();

    let cli = Cli::parse();
    match cli.command {
        Command::Get {
            url,
            output,
            connections,
            filename,
        } => get(url, output, connections, filename).await,
        Command::Add {
            url,
            output,
            connections,
            filename,
            priority,
            no_categorize,
        } => add(&cli.db, url, output, connections, filename, priority, no_categorize).await,
        Command::List => list(&cli.db).await,
        Command::Category(cmd) => category(&cli.db, cmd).await,
        Command::Run {
            concurrency,
            connections,
        } => run_queue(&cli.db, concurrency, connections).await,
        Command::Retry { id } => set_state(&cli.db, &id, DownloadState::Waiting).await,
        Command::Remove { id } => remove(&cli.db, &id).await,
        Command::Serve {
            addr,
            output,
            concurrency,
        } => serve(&cli.db, addr, output, concurrency).await,
        Command::Token => {
            println!("{}", api_token()?.expose());
            Ok(())
        }
        Command::Settings(cmd) => settings(&cli.db, cmd).await,
        Command::Sessions(cmd) => sessions(cmd).await,
        Command::Discord(cmd) => discord(&cli.db, cmd).await,
        Command::Music(cmd) => music(&cli.db, cmd).await,
    }
}

async fn music(db: &Path, cmd: MusicCommand) -> Result<(), Box<dyn std::error::Error>> {
    let queue = Queue::open(db).await?;
    let vault = match session_vault() {
        Ok(vault) => vault,
        Err(_) => std::sync::Arc::new(zoltraak_core::SessionVault::in_memory()),
    };
    let manager = zoltraak_core::MusicManager::new(reqwest::Client::new(), queue.pool()).with_session_vault(vault);
    manager.recover().await?;

    match cmd {
        MusicCommand::Jobs => {
            for job in manager.list().await {
                println!(
                    "{}  {:<11} {} — {}{}",
                    job.id,
                    job.state.as_str(),
                    job.track.artist_line(),
                    job.track.title,
                    job.path.as_ref().map(|p| format!("  → {}", p.display())).unwrap_or_default()
                );
            }
            Ok(())
        }
        MusicCommand::Get { input, output, format, bitrate, no_folder, no_tags, cover_file, no_artwork, lyrics, concurrency, list } => {
            let format = zoltraak_core::AudioFormat::parse(&format).ok_or("format must be one of: best, mp3, m4a, opus, ogg, flac, wav, alac")?;
            eprintln!("resolving {input}…");
            let source = manager.analyze(&input, &Vec::new()).await?;
            eprintln!(
                "{}: {} — {} track{}{}",
                source.service.label(),
                source.title,
                source.tracks.len(),
                if source.tracks.len() == 1 { "" } else { "s" },
                source.owner.as_ref().map(|o| format!(" (by {o})")).unwrap_or_default()
            );
            if let Some(note) = &source.note {
                eprintln!("note: {note}");
            }
            if list {
                for (i, t) in source.tracks.iter().enumerate() {
                    println!("{:>3}. {} — {}{}", i + 1, t.artist_line(), t.title, t.album.as_ref().map(|a| format!("  [{a}]")).unwrap_or_default());
                }
                return Ok(());
            }

            let mut settings = manager.settings().await;
            settings.max_concurrent = concurrency.max(1);
            manager.update_settings(settings).await?;

            let options = zoltraak_core::OutputOptions {
                format,
                bitrate_kbps: bitrate,
                folder_mode: if no_folder { zoltraak_core::FolderMode::None } else { zoltraak_core::FolderMode::Collection },
                embed_metadata: !no_tags,
                embed_cover: !no_tags,
                fetch_artwork: !no_artwork,
                save_cover_file: cover_file,
                fetch_lyrics: lyrics,
                ..zoltraak_core::OutputOptions::default()
            };
            let enqueued = manager
                .enqueue(zoltraak_core::NewMusicDownload { source, options, base_directory: std::path::absolute(&output)? })
                .await?;
            eprintln!("{} queued into {}{}", enqueued.job_ids.len(), enqueued.directory.display(), if enqueued.skipped > 0 { format!(" ({} already on disk)", enqueued.skipped) } else { String::new() });

            // Report each job's state changes until every one has settled.
            let mut last: std::collections::HashMap<String, String> = std::collections::HashMap::new();
            loop {
                tokio::time::sleep(Duration::from_millis(500)).await;
                let jobs = manager.list().await;
                let mine: Vec<_> = jobs.iter().filter(|j| enqueued.job_ids.contains(&j.id)).collect();
                for job in &mine {
                    let line = match job.state {
                        zoltraak_core::MusicJobState::Finished => format!("done: {}", job.path.as_ref().map(|p| p.display().to_string()).unwrap_or_default()),
                        zoltraak_core::MusicJobState::Failed => format!("failed: {}", job.error.as_deref().unwrap_or("unknown error")),
                        zoltraak_core::MusicJobState::Skipped => "skipped: already on disk".into(),
                        _ => format!("{}{}", job.state.as_str(), job.stage.as_ref().map(|s| format!(" — {s}")).unwrap_or_default()),
                    };
                    let key = job.id.to_string();
                    if last.get(&key) != Some(&line) {
                        eprintln!("[{}] {} — {}: {line}", job.position, job.track.artist_line(), job.track.title);
                        last.insert(key, line);
                    }
                }
                if mine.iter().all(|j| j.state.is_terminal()) {
                    let finished = mine.iter().filter(|j| j.state == zoltraak_core::MusicJobState::Finished).count();
                    let failed = mine.iter().filter(|j| j.state == zoltraak_core::MusicJobState::Failed).count();
                    eprintln!("{finished} finished, {failed} failed, {} skipped", mine.len() - finished - failed);
                    return if failed > 0 { Err(format!("{failed} track(s) failed").into()) } else { Ok(()) };
                }
            }
        }
    }
}

/// Queue a finished file for Discord, if the user turned auto-send on.
///
/// Every failure is swallowed: a download that succeeded must not look broken
/// because a side errand could not be started. What went wrong shows up in
/// `zoltraak discord deliveries` instead.
async fn auto_deliver(
    discord: &zoltraak_core::DiscordService,
    source: zoltraak_core::discord::DeliverySource,
    source_id: Option<String>,
    path: PathBuf,
) {
    match discord.settings().await {
        Ok(settings) if settings.auto_send => {}
        _ => return,
    }
    let Some(new) = zoltraak_core::discord::NewDelivery::from_path(source, source_id, path) else {
        return;
    };
    if let Err(e) = discord.enqueue(new).await {
        tracing::warn!(error = %e, "could not queue a Discord delivery");
    }
}

/// Where the session vault lives: next to the API token, so the app and the
/// CLI share one.
fn session_vault() -> Result<std::sync::Arc<zoltraak_core::SessionVault>, Box<dyn std::error::Error>> {
    let token_path = zoltraak_api::store::default_path()
        .ok_or("could not determine a config directory; set ZOLTRAAK_CONFIG_DIR")?;
    let dir = token_path.parent().ok_or("config path has no parent directory")?;
    Ok(std::sync::Arc::new(zoltraak_core::SessionVault::open(dir)?))
}

/// Where standing credentials live: beside the session vault and the API
/// token, so the app and the CLI share one store.
fn secret_vault() -> Result<std::sync::Arc<zoltraak_core::SecretVault>, Box<dyn std::error::Error>> {
    let token_path = zoltraak_api::store::default_path()
        .ok_or("could not determine a config directory; set ZOLTRAAK_CONFIG_DIR")?;
    let dir = token_path.parent().ok_or("config path has no parent directory")?;
    Ok(std::sync::Arc::new(zoltraak_core::SecretVault::open(dir)?))
}

fn read_secret(given: Option<String>, prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
    if let Some(value) = given {
        return Ok(value);
    }
    eprintln!("{prompt}");
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let trimmed = line.trim().to_string();
    if trimmed.is_empty() {
        return Err("nothing was entered".into());
    }
    Ok(trimmed)
}

async fn discord(db: &std::path::Path, cmd: DiscordCommand) -> Result<(), Box<dyn std::error::Error>> {
    let queue = Queue::open(db).await?;
    let service = zoltraak_core::DiscordService::new(reqwest::Client::new(), queue, secret_vault()?);

    match cmd {
        DiscordCommand::Status => {
            println!("{}", serde_json::to_string_pretty(&service.status().await?)?);
        }
        DiscordCommand::ConnectWebhook { url } => {
            let url = read_secret(url, "Paste the webhook URL:")?;
            let connection = service.connect_webhook(&url).await?;
            println!("webhook checked and saved{}", match connection.bot_username {
                Some(name) => format!(" ({name})"),
                None => String::new(),
            });
        }
        DiscordCommand::ConnectBot { token } => {
            let token = read_secret(token, "Paste the bot token:")?;
            let connection = service.connect_bot(&token).await?;
            println!("connected as {}", connection.bot_username.as_deref().unwrap_or("the bot"));
            if let Some(invite) = connection.invite_url {
                println!("invite it to a server: {invite}");
                println!("then choose a channel: zoltraak discord target <GUILD_ID> <CHANNEL_ID>");
            }
        }
        DiscordCommand::Disconnect => {
            service.disconnect().await?;
            println!("disconnected; the stored credential has been forgotten");
        }
        DiscordCommand::Guilds => {
            for guild in service.guilds().await? {
                println!("{}  {}", guild.id, guild.name);
            }
        }
        DiscordCommand::Channels { guild_id } => {
            for channel in service.channels(&guild_id).await? {
                println!("{}  #{}", channel.id, channel.name.unwrap_or_default());
            }
        }
        DiscordCommand::Target { guild_id, channel_id } => {
            let settings = service.set_target(&guild_id, &channel_id).await?;
            println!(
                "delivering to #{} in {} (upload limit {})",
                settings.channel_name.as_deref().unwrap_or("?"),
                settings.guild_name.as_deref().unwrap_or("?"),
                zoltraak_core::discord::limits::human_bytes(settings.upload_limit()),
            );
        }
        DiscordCommand::Invite => println!("{}", service.invite_url().await?),
        DiscordCommand::Test => println!("{}", service.test().await?),
        DiscordCommand::Send { paths } => {
            if paths.is_empty() {
                return Err("name at least one file to send".into());
            }
            let queued = service.send_paths(paths).await?;
            // The worker runs inside this process, so it has to be given time
            // to finish before the command returns.
            tokio::spawn(service.clone().run_worker());
            let ids: Vec<String> = queued.iter().map(|d| d.id.clone()).collect();
            loop {
                let deliveries = service.deliveries(200).await?;
                let mine: Vec<_> = deliveries.iter().filter(|d| ids.contains(&d.id)).collect();
                if mine.iter().all(|d| d.state.is_terminal()) {
                    for delivery in mine {
                        print_delivery(delivery);
                    }
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            }
        }
        DiscordCommand::Deliveries => {
            let deliveries = service.deliveries(50).await?;
            if deliveries.is_empty() {
                println!("nothing has been sent yet");
            }
            for delivery in &deliveries {
                print_delivery(delivery);
            }
        }
        DiscordCommand::Retry { id } => {
            if service.retry(&id).await? {
                println!("{id} is queued again");
            } else {
                return Err(format!("no finished delivery with id {id}").into());
            }
        }
    }
    Ok(())
}

fn print_delivery(delivery: &zoltraak_core::Delivery) {
    let size = match delivery.sent_bytes {
        Some(sent) if sent != delivery.bytes => format!(
            "{} -> {}",
            zoltraak_core::discord::limits::human_bytes(delivery.bytes),
            zoltraak_core::discord::limits::human_bytes(sent)
        ),
        _ => zoltraak_core::discord::limits::human_bytes(delivery.bytes),
    };
    println!("{:<9} {:<30} {}", delivery.state.as_str(), delivery.filename, size);
    if let Some(reason) = &delivery.reason {
        println!("          {reason}");
    }
    if let Some(url) = &delivery.message_url {
        println!("          {url}");
    }
}

async fn sessions(cmd: SessionsCommand) -> Result<(), Box<dyn std::error::Error>> {
    let vault = session_vault()?;
    match cmd {
        SessionsCommand::List => {
            let summaries = vault.summaries().await;
            if summaries.is_empty() {
                println!("no stored sessions");
            }
            for s in summaries {
                println!(
                    "{:<20} {}  {}{}  cookies: {}",
                    s.domain,
                    s.updated_at.format("%Y-%m-%d %H:%M"),
                    if s.signed_in { "signed in" } else { "anonymous" },
                    if s.has_po_token { " +po-token" } else { "" },
                    if s.cookie_names.is_empty() { "-".to_string() } else { s.cookie_names.join(", ") },
                );
            }
        }
        SessionsCommand::Forget { domain: Some(domain) } => {
            if vault.remove(&domain).await? {
                println!("forgot the session for {domain}");
            } else {
                println!("no session stored for {domain}");
            }
        }
        SessionsCommand::Forget { domain: None } => {
            vault.clear().await?;
            println!("forgot every stored session");
        }
    }
    Ok(())
}

fn parse_id(raw: &str) -> Result<DownloadId, Box<dyn std::error::Error>> {
    raw.parse::<DownloadId>().map_err(Into::into)
}

async fn add(
    db: &Path,
    url: String,
    output: PathBuf,
    connections: Option<u8>,
    filename: Option<String>,
    priority: i64,
    no_categorize: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let queue = Queue::open(db).await?;

    // Store an absolute path: the queue outlives the shell that created it, and
    // a later `run` from a different directory must still resolve the target.
    let directory = std::path::absolute(&output)?;

    let mut new = NewDownload::new(url, directory);
    new.connections = connections;
    new.priority = priority;
    new.categorize = !no_categorize;
    if let Some(name) = filename {
        new.filename = name;
    }

    let id = queue.enqueue(new).await?;
    println!("{id}");
    Ok(())
}

async fn category(db: &Path, cmd: CategoryCommand) -> Result<(), Box<dyn std::error::Error>> {
    let queue = Queue::open(db).await?;

    match cmd {
        CategoryCommand::Add {
            name,
            kind,
            pattern,
            directory,
            priority,
        } => {
            let kind = kind.parse::<zoltraak_core::CategoryKind>()?;
            let directory = std::path::absolute(&directory)?;
            let id = queue
                .add_category(zoltraak_core::NewCategory {
                    name,
                    kind,
                    pattern,
                    directory,
                    priority,
                })
                .await?;
            println!("{id}");
        }
        CategoryCommand::List => {
            let categories = queue.list_categories().await?;
            if categories.is_empty() {
                eprintln!("no categories configured");
                return Ok(());
            }
            println!("{:<4} {:<8} {:<7} {:<20} {:>4} DIRECTORY", "ID", "ENABLED", "KIND", "PATTERN", "PRIO");
            for c in categories {
                println!(
                    "{:<4} {:<8} {:<7} {:<20} {:>4} {}",
                    c.id,
                    if c.enabled { "yes" } else { "no" },
                    c.kind,
                    c.pattern,
                    c.priority,
                    c.directory.display(),
                );
            }
        }
        CategoryCommand::Enable { id } => queue.set_category_enabled(id, true).await?,
        CategoryCommand::Disable { id } => queue.set_category_enabled(id, false).await?,
        CategoryCommand::Remove { id } => queue.remove_category(id).await?,
    }
    Ok(())
}

async fn list(db: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let items = Queue::open(db).await?.list().await?;
    if items.is_empty() {
        eprintln!("queue is empty");
        return Ok(());
    }

    println!("{:<36}  {:<12}  {:>9}  {:>6}  FILENAME", "ID", "STATE", "SIZE", "DONE");
    for item in items {
        let pct = match item.total_bytes {
            Some(total) if total > 0 => {
                format!("{:.0}%", item.downloaded_bytes as f64 / total as f64 * 100.0)
            }
            _ => "--".into(),
        };
        println!(
            "{:<36}  {:<12}  {:>9}  {:>6}  {}",
            item.id,
            item.state,
            item.total_bytes.map(human).unwrap_or_else(|| "--".into()),
            pct,
            item.filename
        );
        if let Some(error) = &item.error {
            println!("{:<36}  └─ {error}", "");
        }
    }
    Ok(())
}

async fn run_queue(
    db: &Path,
    concurrency: usize,
    connections: Option<u8>,
) -> Result<(), Box<dyn std::error::Error>> {
    let queue = Queue::open(db).await?;
    let downloader = Downloader::new(
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()?,
        Options {
            connections,
            ..Options::default()
        },
    );
    // `Runner` has no long-lived loop to load this itself the way `Supervisor`
    // does, so it is applied once, up front, for the life of this invocation.
    let settings = queue.load_settings().await?;
    downloader
        .set_bandwidth_limit(settings.bandwidth_limit_bytes_per_sec)
        .await;

    let runner = Runner::new(
        queue,
        downloader,
        RunnerConfig {
            max_concurrent: concurrency.max(1),
            ..RunnerConfig::default()
        },
    );

    let cancel = CancellationToken::new();
    {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("\ninterrupted — pausing in-flight downloads");
                cancel.cancel();
            }
        });
    }

    let summary = runner.run_until_empty(cancel).await?;
    eprintln!(
        "done: {} completed, {} paused, {} failed",
        summary.completed, summary.paused, summary.failed
    );

    // A failed download should be visible in the exit status for scripting.
    if summary.failed > 0 {
        std::process::exit(1);
    }
    Ok(())
}

async fn set_state(
    db: &Path,
    id: &str,
    state: DownloadState,
) -> Result<(), Box<dyn std::error::Error>> {
    let queue = Queue::open(db).await?;
    let id = parse_id(id)?;
    queue.get(id).await?; // surface "not found" before mutating
    queue.set_state(id, state).await?;
    eprintln!("{id} -> {state}");
    Ok(())
}

async fn remove(db: &Path, id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let queue = Queue::open(db).await?;
    let id = parse_id(id)?;
    queue.get(id).await?;
    queue.remove(id).await?;
    eprintln!("removed {id}");
    Ok(())
}

async fn get(
    url: String,
    output: PathBuf,
    connections: Option<u8>,
    filename: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let downloader = Downloader::new(
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()?,
        Options {
            connections,
            ..Options::default()
        },
    );

    let request = Request {
        url,
        directory: output,
        filename,
        headers: Vec::new(),
    };

    let cancel = CancellationToken::new();
    let (tx, rx) = progress_channel(DownloadId::new());

    // Ctrl-C pauses rather than killing: resume state is flushed on the way
    // out, so re-running the same command continues where this left off.
    {
        let cancel = cancel.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("\ninterrupted — saving resume state");
                cancel.cancel();
            }
        });
    }

    let reporter = tokio::spawn(report(rx));
    let outcome = downloader.download(&request, cancel, &tx).await;
    reporter.abort();

    match outcome? {
        Outcome::Completed { path, bytes } => {
            eprintln!("\ndone: {} ({})", path.display(), human(bytes));
            Ok(())
        }
        Outcome::Paused {
            path,
            downloaded,
            total,
        } => {
            let of = total.map(human).unwrap_or_else(|| "unknown".into());
            eprintln!(
                "\npaused: {} ({} of {}) — re-run to resume",
                path.display(),
                human(downloaded),
                of
            );
            Ok(())
        }
    }
}

/// Redraw a single progress line rather than scrolling the terminal.
async fn report(mut rx: tokio::sync::watch::Receiver<zoltraak_core::Progress>) {
    let mut ticker = tokio::time::interval(Duration::from_millis(120));
    loop {
        ticker.tick().await;
        if rx.changed().await.is_err() {
            return;
        }
        let p = *rx.borrow();

        let bar = match p.fraction() {
            Some(f) => {
                let filled = (f * 28.0).round() as usize;
                format!(
                    "[{}{}] {:>5.1}%",
                    "#".repeat(filled),
                    "-".repeat(28 - filled),
                    f * 100.0
                )
            }
            None => "[ streaming ]".to_string(),
        };

        let eta = p
            .eta_secs()
            .map(|s| format!("{s}s"))
            .unwrap_or_else(|| "--".into());

        eprint!(
            "\r{bar}  {}/s  eta {eta}   ",
            human(p.bytes_per_second)
        );
        let _ = std::io::stderr().flush();
    }
}

fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == 0 {
        return "0 B".into();
    }
    let exp = ((bytes as f64).ln() / 1024f64.ln()).floor() as usize;
    let exp = exp.min(UNITS.len() - 1);
    format!("{:.1} {}", bytes as f64 / 1024f64.powi(exp as i32), UNITS[exp])
}

async fn settings(db: &Path, cmd: SettingsCommand) -> Result<(), Box<dyn std::error::Error>> {
    let queue = Queue::open(db).await?;

    match cmd {
        SettingsCommand::Show => {
            let settings = queue.load_settings().await?;
            println!("{}", serde_json::to_string_pretty(&settings)?);
        }
        SettingsCommand::Set {
            bandwidth,
            connections,
            folder,
            schedule,
            on_complete,
        } => {
            let mut settings = queue.load_settings().await?;

            if let Some(b) = bandwidth {
                settings.bandwidth_limit_bytes_per_sec = (b > 0).then_some(b);
            }
            if let Some(c) = connections {
                settings.default_connections = Some(c);
            }
            if let Some(dir) = folder {
                settings.default_directory = Some(std::path::absolute(dir)?);
            }
            if let Some(raw) = schedule {
                settings.schedule = parse_schedule(&raw)?;
            }
            if let Some(raw) = on_complete {
                settings.on_complete = parse_on_complete(&raw);
            }

            queue.save_settings(&settings).await?;
            println!("{}", serde_json::to_string_pretty(&settings)?);
        }
    }
    Ok(())
}

/// "off" disables the window; otherwise "HH:MM-HH:MM".
fn parse_schedule(raw: &str) -> Result<zoltraak_core::TimeWindow, Box<dyn std::error::Error>> {
    if raw.eq_ignore_ascii_case("off") {
        return Ok(zoltraak_core::TimeWindow {
            enabled: false,
            ..Default::default()
        });
    }
    let (start, stop) = raw
        .split_once('-')
        .ok_or("schedule must be \"HH:MM-HH:MM\" or \"off\"")?;
    Ok(zoltraak_core::TimeWindow {
        enabled: true,
        start_minutes: parse_hhmm(start)?,
        stop_minutes: parse_hhmm(stop)?,
    })
}

fn parse_hhmm(raw: &str) -> Result<u16, Box<dyn std::error::Error>> {
    let (h, m) = raw.split_once(':').ok_or("expected \"HH:MM\"")?;
    let h: u16 = h.parse()?;
    let m: u16 = m.parse()?;
    if h >= 24 || m >= 60 {
        return Err("hour must be 0-23 and minute 0-59".into());
    }
    Ok(h * 60 + m)
}

fn parse_on_complete(raw: &str) -> zoltraak_core::CompletionAction {
    match raw {
        "none" => zoltraak_core::CompletionAction::None,
        "open-folder" => zoltraak_core::CompletionAction::OpenFolder,
        "exit" => zoltraak_core::CompletionAction::ExitApp,
        command => zoltraak_core::CompletionAction::RunCommand {
            command: command.to_string(),
        },
    }
}

/// The install's stable token, created on first use.
fn api_token() -> Result<ApiToken, Box<dyn std::error::Error>> {
    let path = zoltraak_api::store::default_path()
        .ok_or("could not determine a config directory; set ZOLTRAAK_CONFIG_DIR")?;
    Ok(zoltraak_api::store::load_or_create(&path)?)
}

async fn serve(
    db: &Path,
    addr: SocketAddr,
    output: PathBuf,
    concurrency: usize,
) -> Result<(), Box<dyn std::error::Error>> {
    if !addr.ip().is_loopback() {
        // Binding beyond loopback exposes the engine to the network. The token
        // still applies, but the user should know.
        tracing::warn!(%addr, "binding to a non-loopback address exposes the API to the network");
    }

    let token = api_token()?;
    // Printed to stderr so an operator can configure a client without hunting
    // for the file. It is never written to the structured log.
    eprintln!("API token: {}", token.expose());

    let supervisor = Supervisor::start(
        Queue::open(db).await?,
        Downloader::new(
            reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::limited(10))
                .build()?,
            Options::default(),
        ),
        RunnerConfig {
            max_concurrent: concurrency.max(1),
            ..RunnerConfig::default()
        },
    );

    // Shared browser sessions outlive the process, encrypted, so a paste-a-link
    // download behind a login keeps working across restarts.
    let vault = match session_vault() {
        Ok(vault) => vault,
        Err(e) => {
            tracing::warn!(error = %e, "session vault unavailable; sessions will not persist");
            std::sync::Arc::new(zoltraak_core::SessionVault::in_memory())
        }
    };
    // Standing credentials (the Discord bot token or webhook) sit beside the
    // session vault. An in-memory fallback keeps `serve` usable on a box with
    // no writable config directory; Discord simply stays unconfigured there.
    let secrets = match secret_vault() {
        Ok(vault) => vault,
        Err(e) => {
            tracing::warn!(error = %e, "secret vault unavailable; Discord credentials will not persist");
            std::sync::Arc::new(zoltraak_core::SecretVault::in_memory())
        }
    };
    let discord = zoltraak_core::DiscordService::new(
        reqwest::Client::new(),
        supervisor.queue().clone(),
        secrets,
    );
    tokio::spawn(discord.clone().run_worker());

    let deliveries = discord.clone();
    supervisor.on_finished(std::sync::Arc::new(move |id, path| {
        let discord = deliveries.clone();
        tokio::spawn(async move {
            auto_deliver(&discord, zoltraak_core::discord::DeliverySource::Download, Some(id.to_string()), path).await;
        });
    }));

    let video_deliveries = discord.clone();
    let video = zoltraak_core::VideoManager::new(reqwest::Client::new(), supervisor.queue().pool())
        .with_rate_limiter(supervisor.rate_limiter())
        .with_session_vault(std::sync::Arc::clone(&vault))
        .with_completion_hook(std::sync::Arc::new(move |id, path| {
            let discord = video_deliveries.clone();
            tokio::spawn(async move {
                auto_deliver(&discord, zoltraak_core::discord::DeliverySource::Video, Some(id.to_string()), path).await;
            });
        }));
    video.recover().await?;

    let music_deliveries = discord.clone();
    let music = zoltraak_core::MusicManager::new(reqwest::Client::new(), supervisor.queue().pool())
        .with_rate_limiter(supervisor.rate_limiter())
        .with_session_vault(vault)
        .with_completion_hook(std::sync::Arc::new(move |id, path| {
            let discord = music_deliveries.clone();
            tokio::spawn(async move {
                auto_deliver(&discord, zoltraak_core::discord::DeliverySource::Music, Some(id.to_string()), path).await;
            });
        }));
    music.recover().await?;

    let state = ApiState {
        extension_theme: Default::default(),
        guide_theme: Default::default(),
        token: Arc::new(token),
        supervisor: supervisor.clone(),
        video,
        music: Some(music),
        music_prefill_tx: None,
        remux: zoltraak_core::RemuxManager::with_history(supervisor.queue().pool()).await?,
        default_directory: Arc::new(std::path::absolute(&output)?),
        // Headless: no window to fill, so prefill routes no-op.
        download_prefill_tx: None,
        image_prefill_tx: None,
        prefill_tx: None,
        folder_picker_tx: None,
        discord: Some(discord),
        auto_send: None,
        // Headless servers pair through `zoltraak token`; there is no window to click in.
        pairing_window: Arc::new(std::sync::RwLock::new(None)),
        reload_version: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        reload_tx: Arc::new(tokio::sync::watch::channel(0u64).0),
        extension_sources: zoltraak_api::store::extension_source_path()
            .map(zoltraak_api::extension_source::ExtensionSourceManager::open)
            .unwrap_or_default(),
        extension_source_switch: Default::default(),
        app_navigate_tx: None,
    };

    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, "listening");

    // Ctrl-C has to reach the supervisor, not just the socket: dropping the
    // listener would leave in-flight downloads running with nothing to stop
    // them, and the process would hang instead of exiting.
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
        eprintln!("\nshutting down — in-flight downloads will resume on the next launch");
    };

    axum::serve(listener, zoltraak_api::router(state))
        .with_graceful_shutdown(shutdown)
        .await?;

    supervisor.shutdown().await;
    Ok(())
}
