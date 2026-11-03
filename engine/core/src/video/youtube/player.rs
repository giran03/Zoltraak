//! YouTube's player script, turned into a challenge solver.
//!
//! A `videoplayback` URL handed out by InnerTube is not fetchable as-is. Two
//! values on it are obfuscated by the player JavaScript that YouTube serves
//! alongside the page: the `n` throttling token (an unsolved one gets a 403,
//! or a stream throttled to a crawl) and, for some clients, the `sig`
//! signature. Both transforms live only in that script, and the script is
//! re-obfuscated every few days — so the only durable way to undo them is to
//! run the script's own code.
//!
//! The approach mirrors yt-dlp's EJS solver (Unlicense): the player is one big
//! IIFE whose body is thousands of top-level definitions interleaved with a
//! handful of calls that start the player. Keeping only the definitions gives a
//! script that a bare JS engine can evaluate with a few DOM stubs, and the
//! function that builds a `videoplayback` URL — recognisable because it sets
//! `alr=yes` on the URL it returns — decodes both challenges for us, so no
//! part of the obfuscated cipher ever has to be pattern-matched.
//!
//! No JS parser is used for the rewrite. A tokenizer-level scanner that
//! understands strings, templates, regex literals, comments and bracket
//! nesting is enough to find statement boundaries in minified Closure output,
//! and it costs tens of milliseconds on a 2.5 MB player where a full parse
//! costs hundreds. The rewritten script is then evaluated in an embedded
//! QuickJS runtime, once per player version, and every solved value is
//! memoised.

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use reqwest::Client;

use crate::error::{Error, Result};

// ─── Fetching ────────────────────────────────────────────────────────────────

/// The player URL a page or ytcfg names, made absolute.
pub fn absolute_player_url(raw: &str) -> String {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        raw.to_string()
    } else if let Some(rest) = raw.strip_prefix("//") {
        format!("https://{rest}")
    } else if raw.starts_with('/') {
        format!("https://www.youtube.com{raw}")
    } else {
        format!("https://www.youtube.com/{raw}")
    }
}

static PLAYER_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/s/player/([0-9a-fA-F]{8,})/").unwrap());
static IFRAME_PLAYER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"player\\?/([0-9a-fA-F]{8})\\?/"#).unwrap());
static STS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:signatureTimestamp|sts)\s*:\s*([0-9]{5})").unwrap());

/// The 8-hex-digit build id in a player URL, which is what identifies a
/// player version.
pub fn player_id(url: &str) -> Option<&str> {
    PLAYER_ID.captures(url).and_then(|c| c.get(1)).map(|m| m.as_str())
}

/// Ask `iframe_api` which player is current — the fallback when the watch
/// page could not be fetched or named no player.
pub async fn current_player_url(client: &Client) -> Result<String> {
    let body = client
        .get("https://www.youtube.com/iframe_api")
        .header("User-Agent", super::WEB_USER_AGENT)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let id = IFRAME_PLAYER
        .captures(&body)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| Error::InvalidInput("YouTube's iframe API did not name a player version".into()))?;
    Ok(format!("https://www.youtube.com/s/player/{id}/player_ias.vflset/en_US/base.js"))
}

/// One player version: its signature timestamp and, lazily, its solver.
pub struct LoadedPlayer {
    pub url: String,
    /// `signatureTimestamp` — InnerTube must be told which player's cipher the
    /// caller can undo, or it answers with formats for a different one.
    pub sts: Option<u32>,
    prepared: std::result::Result<Prepared, PrepareError>,
    memo: Mutex<HashMap<(ChallengeKind, String), String>>,
}

impl fmt::Debug for LoadedPlayer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LoadedPlayer").field("url", &self.url).field("sts", &self.sts).finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChallengeKind {
    N,
    Sig,
}

impl LoadedPlayer {
    fn from_source(url: String, source: &str) -> Self {
        let sts = STS.captures(source).and_then(|c| c.get(1)).and_then(|m| m.as_str().parse().ok());
        let prepared = prepare(source);
        match &prepared {
            Ok(p) => tracing::info!(
                player = %url, sts, kept = p.statements_kept, total = p.statements_total,
                candidates = ?p.candidates, "prepared YouTube player solver"
            ),
            Err(e) => tracing::warn!(player = %url, error = %e, "could not prepare YouTube player solver"),
        }
        Self { url, sts, prepared, memo: Mutex::new(HashMap::new()) }
    }

    /// Whether this player yielded a usable solver at all.
    pub fn can_solve(&self) -> bool {
        self.prepared.is_ok()
    }

    /// Solve a batch of challenges, in order. Cached answers never touch the
    /// JS engine; the rest are solved in one engine session on a blocking
    /// thread, since a QuickJS runtime is neither `Send` nor async.
    pub async fn solve(self: &Arc<Self>, requests: Vec<(ChallengeKind, String)>) -> Result<Vec<String>> {
        let prepared = match &self.prepared {
            Ok(p) => p,
            Err(e) => return Err(Error::InvalidInput(format!("YouTube player solver unavailable: {e}"))),
        };

        let mut answers: Vec<Option<String>> = Vec::with_capacity(requests.len());
        let mut pending: Vec<(usize, ChallengeKind, String)> = Vec::new();
        {
            let memo = self.memo.lock().unwrap();
            for (i, (kind, challenge)) in requests.iter().enumerate() {
                match memo.get(&(*kind, challenge.clone())) {
                    Some(v) => answers.push(Some(v.clone())),
                    None => {
                        answers.push(None);
                        // The same `n` appears on every format of a response;
                        // solve each distinct one once.
                        if !pending.iter().any(|(_, k, c)| k == kind && c == challenge) {
                            pending.push((i, *kind, challenge.clone()));
                        }
                    }
                }
            }
        }

        if !pending.is_empty() {
            let script = prepared.script.clone();
            let batch: Vec<(ChallengeKind, String)> = pending.iter().map(|(_, k, c)| (*k, c.clone())).collect();
            let started = Instant::now();
            let solved = run_on_big_stack(move || run_solver(&script, &batch))
                .await
                .map_err(|e| Error::InvalidInput(format!("YouTube solver task failed: {e}")))?
                .map_err(|e| Error::InvalidInput(format!("YouTube player solver failed: {e}")))?;
            tracing::debug!(player = %self.url, count = solved.len(), elapsed = ?started.elapsed(), "solved YouTube challenges");
            let mut memo = self.memo.lock().unwrap();
            for ((_, kind, challenge), value) in pending.iter().zip(solved) {
                memo.insert((*kind, challenge.clone()), value);
            }
            for (i, (kind, challenge)) in requests.iter().enumerate() {
                if answers[i].is_none() {
                    answers[i] = memo.get(&(*kind, challenge.clone())).cloned();
                }
            }
        }

        answers
            .into_iter()
            .map(|a| a.ok_or_else(|| Error::InvalidInput("YouTube player solver returned no answer".into())))
            .collect()
    }
}

/// Process-wide cache of loaded players. A player version stays current for
/// days, and every analyze of every YouTube URL would otherwise re-download
/// and re-prepare 2.5 MB of JavaScript.
struct PlayerCache {
    players: Mutex<HashMap<String, Arc<LoadedPlayer>>>,
}

static CACHE: LazyLock<PlayerCache> = LazyLock::new(|| PlayerCache { players: Mutex::new(HashMap::new()) });

/// How many player versions to keep prepared at once. Versions rotate slowly;
/// more than a couple in memory means a leak, not a cache.
const CACHE_LIMIT: usize = 3;

/// Fetch and prepare the player at `url`, or return the cached one.
pub async fn load(client: &Client, url: &str) -> Result<Arc<LoadedPlayer>> {
    let url = absolute_player_url(url);
    if let Some(found) = CACHE.players.lock().unwrap().get(&url) {
        return Ok(found.clone());
    }

    let source = client
        .get(&url)
        .header("User-Agent", super::WEB_USER_AGENT)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;

    // Preparing scans a few megabytes; keep it off the async threads.
    let owned_url = url.clone();
    let player = run_on_big_stack(move || LoadedPlayer::from_source(owned_url, &source))
        .await
        .map_err(|e| Error::InvalidInput(format!("YouTube player preparation failed: {e}")))?;
    let player = Arc::new(player);

    let mut players = CACHE.players.lock().unwrap();
    if players.len() >= CACHE_LIMIT {
        // Evict arbitrarily: with a limit this small the distinction between
        // LRU and any other policy is not worth the bookkeeping.
        let victim = players.keys().next().cloned();
        if let Some(victim) = victim {
            players.remove(&victim);
        }
    }
    players.insert(url, player.clone());
    Ok(player)
}

/// Load a player from source without fetching (tests, offline preparation).
#[doc(hidden)]
pub fn load_from_source(url: &str, source: &str) -> Arc<LoadedPlayer> {
    Arc::new(LoadedPlayer::from_source(url.to_string(), source))
}

// ─── Running the solver ──────────────────────────────────────────────────────

/// Stack for threads that parse or evaluate the player script. Tokio's blocking
/// threads only get 2 MiB, which is less than the 8 MiB QuickJS is told it may
/// use, so deep recursion overflowed the real stack and aborted the process.
const BIG_STACK: usize = 64 << 20;

/// Run blocking work on a fresh thread with [`BIG_STACK`], awaiting its result
/// without blocking the async runtime.
async fn run_on_big_stack<T, F>(work: F) -> std::result::Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> T + Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    std::thread::Builder::new()
        .name("youtube-player".into())
        .stack_size(BIG_STACK)
        .spawn(move || {
            let _ = tx.send(work());
        })
        .map_err(|e| e.to_string())?;
    rx.await.map_err(|e| e.to_string())
}

/// Upper bound on one solver session. Preparing plus evaluating a player takes
/// well under a second; anything past this is a runaway script, not a slow one.
const SOLVER_DEADLINE: Duration = Duration::from_secs(20);

/// Evaluate the prepared script in a fresh QuickJS runtime and run each
/// challenge through it. Runs synchronously; call from a blocking thread.
fn run_solver(script: &str, requests: &[(ChallengeKind, String)]) -> std::result::Result<Vec<String>, String> {
    use rquickjs::{Context, Function, Object, Runtime};

    let runtime = Runtime::new().map_err(|e| format!("could not create JS runtime: {e}"))?;
    // The player defines a lot, but a solver session should never need
    // hundreds of megabytes or deep recursion; both caps bound the damage a
    // changed player can do to the engine process.
    runtime.set_memory_limit(512 << 20);
    runtime.set_max_stack_size(8 << 20);
    let deadline = Instant::now() + SOLVER_DEADLINE;
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)));

    let context = Context::full(&runtime).map_err(|e| format!("could not create JS context: {e}"))?;
    context.with(|ctx| {
        let describe = |what: &str, e: rquickjs::Error| -> String {
            if matches!(e, rquickjs::Error::Exception) {
                let thrown = ctx.catch();
                let message = thrown
                    .as_exception()
                    .and_then(|x| x.message())
                    .or_else(|| thrown.as_string().and_then(|s| s.to_string().ok()))
                    .unwrap_or_else(|| "unknown exception".into());
                format!("{what}: {message}")
            } else {
                format!("{what}: {e}")
            }
        };

        // Sloppy mode, as a browser runs it: the player assigns to names it
        // never declared, which strict mode would refuse.
        let mut options = rquickjs::context::EvalOptions::default();
        options.strict = false;
        options.global = true;
        ctx.eval_with_options::<(), _>(script.as_bytes(), options)
            .map_err(|e| describe("evaluating the player", e))?;
        let result: Object = ctx.globals().get("_result").map_err(|e| describe("reading the solver", e))?;
        let n: Function = result.get("n").map_err(|e| describe("reading the n solver", e))?;
        let sig: Function = result.get("sig").map_err(|e| describe("reading the sig solver", e))?;

        let mut out = Vec::with_capacity(requests.len());
        for (kind, challenge) in requests {
            let function = match kind {
                ChallengeKind::N => &n,
                ChallengeKind::Sig => &sig,
            };
            let solved: String = function
                .call((challenge.as_str(),))
                .map_err(|e| describe(&format!("solving {kind:?} challenge"), e))?;
            if solved.is_empty() || (*kind == ChallengeKind::N && solved == *challenge) {
                // The player's transform never returns its input unchanged;
                // this means the wrong function was called.
                return Err(format!("solver produced no transformation for {kind:?} challenge"));
            }
            out.push(solved);
        }
        Ok(out)
    })
}

// ─── Preparing the script ────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrepareError {
    /// The script is not shaped like a player: no IIFE wrapping the definitions.
    UnexpectedStructure(&'static str),
    /// Nothing in the player looked like the URL-signing function.
    NoSolverCandidate,
}

impl fmt::Display for PrepareError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnexpectedStructure(what) => write!(f, "player script has an unexpected structure: {what}"),
            Self::NoSolverCandidate => f.write_str("could not find the player's URL-signing function"),
        }
    }
}

impl std::error::Error for PrepareError {}

/// The assembled solver script and what went into it.
#[derive(Debug, Clone)]
pub struct Prepared {
    /// Evaluate this in a JS engine; afterwards `_result.n(challenge)` and
    /// `_result.sig(challenge)` are callable.
    pub script: String,
    /// Names (expressions) of the functions that were recognised as the
    /// URL-signing routine. Usually exactly one.
    pub candidates: Vec<String>,
    pub statements_total: usize,
    pub statements_kept: usize,
}

/// DOM/global stubs the kept definitions expect to exist, and the result slot
/// the injected solvers write into. Same shape as yt-dlp's EJS setup.
const SETUP: &str = r#"var _result = { n: null, sig: null };
if (typeof globalThis.XMLHttpRequest === "undefined") { globalThis.XMLHttpRequest = { prototype: {} }; }
if (typeof URL === "undefined") {
  globalThis.location = { hash: "", host: "www.youtube.com", hostname: "www.youtube.com", href: "https://www.youtube.com/watch?v=yt-dlp-wins", origin: "https://www.youtube.com", password: "", pathname: "/watch", port: "", protocol: "https:", search: "?v=yt-dlp-wins", username: "" };
} else {
  globalThis.location = new URL("https://www.youtube.com/watch?v=yt-dlp-wins");
}
if (typeof globalThis.document === "undefined") { globalThis.document = Object.create(null); }
if (typeof globalThis.navigator === "undefined") { globalThis.navigator = Object.create(null); }
if (typeof globalThis.self === "undefined") { globalThis.self = globalThis; }
if (typeof globalThis.window === "undefined") { globalThis.window = globalThis; }
"#;

/// Rewrite a player script into a solver script. See the module docs.
pub fn prepare(player: &str) -> std::result::Result<Prepared, PrepareError> {
    let src = player.as_bytes();
    let program: Vec<Range<usize>> = split_statements(src, 0..src.len())
        .into_iter()
        .filter(|r| !is_blank(&src[r.clone()]))
        .collect();

    // The wrapper is one of `var _yt_player={};(function(g){...})(_yt_player);`
    // and `(function(){...}).call(this);`, occasionally with a leading comment
    // statement — hence "one of the first few".
    let iife_index = program
        .iter()
        .take(3)
        .position(|r| {
            let s = trim(&src[r.clone()]);
            s.starts_with(b"(function") || s.starts_with(b"!function")
        })
        .ok_or(PrepareError::UnexpectedStructure("no wrapping function found"))?;
    let iife = program[iife_index].clone();
    let prelude_end = program[..iife_index].last().map(|r| r.end).unwrap_or(0);
    let prelude = &player[..prelude_end];

    let fn_kw = find(src, iife.start, iife.end, b"function")
        .ok_or(PrepareError::UnexpectedStructure("no function keyword"))?;
    let params_open =
        find_byte(src, fn_kw, iife.end, b'(').ok_or(PrepareError::UnexpectedStructure("no parameter list"))?;
    let params_close = matching_close(src, params_open, iife.end)
        .ok_or(PrepareError::UnexpectedStructure("unbalanced parameter list"))?;
    let body_open =
        find_byte(src, params_close + 1, iife.end, b'{').ok_or(PrepareError::UnexpectedStructure("no function body"))?;
    let body_close =
        matching_close(src, body_open, iife.end).ok_or(PrepareError::UnexpectedStructure("unbalanced function body"))?;

    let head = &player[iife.start..=body_open];
    let tail = &player[body_close..iife.end];

    let mut statements = split_statements(src, body_open + 1..body_close);
    statements.retain(|r| !is_blank(&src[r.clone()]));
    let statements_total = statements.len();

    // The two-statement form opens its body with `var window=this;`, which
    // would shadow the stubbed `window` — the same statement EJS drops.
    if iife_index > 0 {
        if let Some(first) = statements.first() {
            if trim(&src[first.clone()]).starts_with(b"var window=") {
                statements.remove(0);
            }
        }
    }

    let mut kept: Vec<Range<usize>> = Vec::with_capacity(statements.len());
    let mut candidates: Vec<String> = Vec::new();
    for range in statements {
        let text = &src[range.clone()];
        let Some(kind) = classify(text) else { continue };
        kept.push(range.clone());
        if let Some(name) = solver_name(text, kind) {
            if !candidates.contains(&name) {
                candidates.push(name);
            }
        }
    }
    if candidates.is_empty() {
        return Err(PrepareError::NoSolverCandidate);
    }

    let mut script = String::with_capacity(player.len() + 4096);
    script.push_str(SETUP);
    script.push_str(prelude);
    script.push('\n');
    script.push_str(head);
    script.push('\n');
    for range in &kept {
        let stmt = &player[range.clone()];
        script.push_str(stmt);
        if !stmt.trim_end().ends_with(';') && !stmt.trim_end().ends_with('}') {
            script.push(';');
        }
        script.push('\n');
    }
    script.push_str(&solver_exports(&candidates));
    script.push_str(tail);
    script.push('\n');

    Ok(Prepared { script, candidates, statements_total, statements_kept: kept.len() })
}

/// The JS that turns each candidate into `_result.n` / `_result.sig`, run for
/// every candidate and requiring them to agree — verbatim from EJS.
fn solver_exports(candidates: &[String]) -> String {
    let solver = |name: &str| {
        format!(
            r#"(({{sig, n}}) => {{
  const url = ({name})("https://youtube.com/watch?v=yt-dlp-wins", "s", sig ? encodeURIComponent(sig) : undefined);
  url.set("n", n);
  const proto = Object.getPrototypeOf(url);
  const keys = Object.keys(proto).concat(Object.getOwnPropertyNames(proto));
  for (const key of keys) {{
    if (!["constructor", "set", "get", "clone"].includes(key)) {{
      url[key]();
      break;
    }}
  }}
  const s = url.get("s");
  return {{ sig: s ? decodeURIComponent(s) : null, n: url.get("n") ?? null }};
}})"#
        )
    };
    let multi = |field: &str| {
        let generators: Vec<String> = candidates
            .iter()
            .map(|name| format!("({field}) => {}({{{field}}}).{field}", solver(name)))
            .collect();
        format!(
            r#"_result.{field} = (_input) => {{
  const _results = new Set();
  const errors = [];
  for (const _generator of [{}]) {{
    try {{ _results.add(_generator(_input)); }} catch (e) {{ errors.push(e); }}
  }}
  if (!_results.size) {{ throw `no solutions: ${{errors.join(", ")}}`; }}
  if (_results.size !== 1) {{ throw `invalid solutions: ${{[..._results].map(x => JSON.stringify(x)).join(", ")}}`; }}
  return _results.values().next().value;
}};
"#,
            generators.join(", ")
        )
    };
    format!("{}{}", multi("n"), multi("sig"))
}

// ─── Statement classification ────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// `var`/`let`/`const` — may declare functions.
    Declaration,
    /// `function name(...) {...}`
    FunctionDeclaration,
    /// `lhs = rhs;` with a plain identifier / member chain on the left.
    Assignment,
    /// Any other non-expression statement (`if`, `class`, `for`, ...).
    Other,
    /// A bare literal such as `"use strict";`.
    Literal,
}

const STATEMENT_KEYWORDS: &[&[u8]] = &[
    b"class", b"if", b"for", b"while", b"do", b"try", b"switch", b"with", b"throw", b"return", b"break",
    b"continue", b"export", b"import",
];

/// Whether a top-level statement is a definition worth keeping, and what it
/// is. `None` means drop — calls, sequences and anything else that *does*
/// something rather than defining something.
fn classify(text: &[u8]) -> Option<Kind> {
    let t = trim(text);
    if t.is_empty() {
        return None;
    }
    if t[0] == b'"' || t[0] == b'\'' || t[0] == b'`' {
        return Some(Kind::Literal);
    }
    let word = leading_word(t);
    match word {
        b"var" | b"let" | b"const" => return Some(Kind::Declaration),
        b"function" => return Some(Kind::FunctionDeclaration),
        b"async" => {
            let rest = trim_start(&t[word.len()..]);
            return Some(if rest.starts_with(b"function") { Kind::FunctionDeclaration } else { Kind::Other });
        }
        b"debugger" => return None,
        _ => {}
    }
    if STATEMENT_KEYWORDS.contains(&word) {
        // A keyword statement, not an identifier being assigned. Kept for
        // parity with EJS, which keeps every non-expression statement.
        return Some(Kind::Other);
    }
    if is_assignment(t) {
        return Some(Kind::Assignment);
    }
    None
}

/// `lhs OP= rhs` at depth 0, where `lhs` is an identifier or member chain, and
/// with no depth-0 comma (that would be a sequence expression).
fn is_assignment(t: &[u8]) -> bool {
    let Some(lhs_end) = member_chain_end(t) else { return false };
    let op_start = lhs_end + leading_space(&t[lhs_end..]);
    let Some(op_len) = assignment_operator(&t[op_start..]) else { return false };
    !has_depth0_comma(t, op_start + op_len)
}

/// Length of the assignment operator at the start of `s`, if one is there.
fn assignment_operator(s: &[u8]) -> Option<usize> {
    const OPS: &[&[u8]] = &[
        b">>>=", b"**=", b"<<=", b">>=", b"&&=", b"||=", b"??=", b"+=", b"-=", b"*=", b"/=", b"%=", b"&=", b"|=",
        b"^=",
    ];
    for op in OPS {
        if s.starts_with(op) {
            return Some(op.len());
        }
    }
    if s.starts_with(b"=") && !s.starts_with(b"==") && !s.starts_with(b"=>") {
        return Some(1);
    }
    None
}

/// End offset of an identifier / `a.b[c].d` chain at the start of `t`.
fn member_chain_end(t: &[u8]) -> Option<usize> {
    let mut i = ident_end(t, 0)?;
    loop {
        let save = i;
        i += leading_space(&t[i..]);
        if i < t.len() && t[i] == b'.' {
            i += 1;
            i += leading_space(&t[i..]);
            i = ident_end(t, i)?;
        } else if i < t.len() && t[i] == b'[' {
            i = matching_close(t, i, t.len())? + 1;
        } else {
            return Some(save);
        }
    }
}

fn ident_end(t: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    if i >= t.len() || !is_ident_start(t[i]) {
        return None;
    }
    while i < t.len() && is_ident_char(t[i]) {
        i += 1;
    }
    Some(i)
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$' || b >= 0x80
}

fn is_ident_char(b: u8) -> bool {
    is_ident_start(b) || b.is_ascii_digit()
}

fn has_depth0_comma(t: &[u8], from: usize) -> bool {
    let mut scanner = Scanner::new(t, from, t.len());
    while let Some(tok) = scanner.next_token() {
        if scanner.depth() == 0 && matches!(tok, Token::Punct(b',')) {
            return true;
        }
    }
    false
}

/// The expression naming the URL-signing function, if this kept statement
/// defines one: a non-async function whose body *directly* contains a
/// statement of the form `x.y("alr", "yes")`.
fn solver_name(text: &[u8], kind: Kind) -> Option<String> {
    let t = trim(text);
    match kind {
        Kind::FunctionDeclaration => {
            if t.starts_with(b"async") {
                return None;
            }
            let after = trim_start(&t[b"function".len()..]);
            let name_end = ident_end(after, 0)?;
            let name = std::str::from_utf8(&after[..name_end]).ok()?.to_string();
            let body = function_body(after)?;
            sets_alr_yes(&after[body]).then_some(name)
        }
        Kind::Assignment => {
            let lhs_end = member_chain_end(t)?;
            let rest = trim_start(&t[lhs_end..]);
            let op_len = assignment_operator(rest)?;
            let rhs = trim_start(&rest[op_len..]);
            if !rhs.starts_with(b"function") {
                return None;
            }
            let body = function_body(rhs)?;
            // The function must be the whole right-hand side.
            let after_body = trim(&rhs[body.end + 1..]);
            if !(after_body.is_empty() || after_body == b";") {
                return None;
            }
            let name = std::str::from_utf8(&t[..lhs_end]).ok()?.trim().to_string();
            sets_alr_yes(&rhs[body]).then_some(name)
        }
        Kind::Declaration => {
            // `var a=function(){...},b=function(){...};` — check each declarator.
            let word = leading_word(t);
            let rest = trim_start(&t[word.len()..]);
            let mut scanner = Scanner::new(rest, 0, rest.len());
            let mut start = 0usize;
            loop {
                let tok = scanner.next_token();
                let at_boundary = match tok {
                    None => true,
                    Some(Token::Punct(b',')) | Some(Token::Punct(b';')) => scanner.depth() == 0,
                    _ => false,
                };
                if at_boundary {
                    let end = match tok {
                        None => rest.len(),
                        _ => scanner.token_start(),
                    };
                    if let Some(name) = declarator_solver(trim(&rest[start..end])) {
                        return Some(name);
                    }
                    start = scanner.pos();
                    if tok.is_none() {
                        return None;
                    }
                }
            }
        }
        _ => None,
    }
}

fn declarator_solver(decl: &[u8]) -> Option<String> {
    let name_end = ident_end(decl, 0)?;
    let rest = trim_start(&decl[name_end..]);
    if !rest.starts_with(b"=") || rest.starts_with(b"==") {
        return None;
    }
    let rhs = trim_start(&rest[1..]);
    if !rhs.starts_with(b"function") {
        return None;
    }
    let body = function_body(rhs)?;
    if !sets_alr_yes(&rhs[body]) {
        return None;
    }
    std::str::from_utf8(&decl[..name_end]).ok().map(str::to_string)
}

/// Byte range of the body (between the braces, exclusive) of a function whose
/// text starts with `function`.
fn function_body(t: &[u8]) -> Option<Range<usize>> {
    let params_open = find_byte(t, 0, t.len(), b'(')?;
    let params_close = matching_close(t, params_open, t.len())?;
    let body_open = find_byte(t, params_close + 1, t.len(), b'{')?;
    let body_close = matching_close(t, body_open, t.len())?;
    Some(body_open + 1..body_close)
}

/// Whether one of the body's own statements is `ident.prop("alr","yes")`.
fn sets_alr_yes(body: &[u8]) -> bool {
    if !contains(body, b"\"alr\"") && !contains(body, b"'alr'") {
        return false;
    }
    for range in split_statements(body, 0..body.len()) {
        let stmt = trim(&body[range]);
        let stmt = stmt.strip_suffix(b";").map(trim).unwrap_or(stmt);
        let Some(mut i) = ident_end(stmt, 0) else { continue };
        // Exactly one property access: `.name` or `[expr]`.
        if i < stmt.len() && stmt[i] == b'.' {
            let Some(end) = ident_end(stmt, i + 1) else { continue };
            i = end;
        } else if i < stmt.len() && stmt[i] == b'[' {
            let Some(end) = matching_close(stmt, i, stmt.len()) else { continue };
            i = end + 1;
        } else {
            continue;
        }
        if is_alr_yes_call(trim(&stmt[i..])) {
            return true;
        }
    }
    false
}

fn is_alr_yes_call(args: &[u8]) -> bool {
    let Some(inner) = args.strip_prefix(b"(").and_then(|a| a.strip_suffix(b")")).map(trim) else { return false };
    let parts: Vec<&[u8]> = inner.split(|&b| b == b',').map(trim).collect();
    parts.len() == 2 && is_str_literal(parts[0], b"alr") && is_str_literal(parts[1], b"yes")
}

fn is_str_literal(t: &[u8], value: &[u8]) -> bool {
    t.len() == value.len() + 2 && (t[0] == b'"' || t[0] == b'\'') && t[t.len() - 1] == t[0] && &t[1..t.len() - 1] == value
}

// ─── Statement splitting ─────────────────────────────────────────────────────

/// Split `src[range]` into its top-level statements (byte ranges into `src`).
fn split_statements(src: &[u8], range: Range<usize>) -> Vec<Range<usize>> {
    let mut out = Vec::new();
    let mut scanner = Scanner::new(src, range.start, range.end);
    let mut stmt_start: Option<usize> = None;
    let mut starts_with_do = false;

    while let Some(tok) = scanner.next_token() {
        let start = match stmt_start {
            Some(s) => s,
            None => {
                let s = scanner.token_start();
                stmt_start = Some(s);
                starts_with_do = matches!(tok, Token::Word(b"do"));
                s
            }
        };
        if scanner.depth() != 0 {
            continue;
        }
        let end_here = match tok {
            Token::Punct(b';') => true,
            Token::CloseBrace { expr: true } => {
                // Object literal / arrow body / function expression: ends at
                // `;`, or by ASI when nothing continues the expression.
                match scanner.peek_token() {
                    Some(Token::Punct(b';')) => {
                        scanner.next_token();
                        true
                    }
                    Some(next) => !continues_expression(&next),
                    None => true,
                }
            }
            Token::CloseBrace { expr: false } => {
                // Block statement: ends unless a clause continues it.
                match scanner.peek_token() {
                    Some(Token::Word(w)) => {
                        !(w == b"else" || w == b"catch" || w == b"finally" || (w == b"while" && starts_with_do))
                    }
                    Some(Token::Punct(b';')) => {
                        scanner.next_token();
                        true
                    }
                    _ => true,
                }
            }
            _ => false,
        };
        if end_here {
            out.push(start..scanner.pos());
            stmt_start = None;
        }
    }
    if let Some(start) = stmt_start {
        if start < range.end {
            out.push(start..range.end);
        }
    }
    out
}

fn continues_expression(tok: &Token<'_>) -> bool {
    match tok {
        Token::Punct(b) => matches!(
            b,
            b'.' | b',' | b'?' | b':' | b'=' | b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^' | b'<' | b'>'
                | b'!' | b'~'
        ),
        Token::Word(w) => *w == b"in" || *w == b"instanceof",
        Token::Template => true,
        // A call or index on the value: `function(){}(args)`, `{...}[key]`.
        Token::Open(b'(') | Token::Open(b'[') => true,
        _ => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token<'a> {
    Word(&'a [u8]),
    Number,
    Str,
    Template,
    Regex,
    Punct(u8),
    Open(u8),
    CloseParen,
    CloseBracket,
    /// `}` — `expr` says whether the matching `{` opened an object literal or
    /// function/arrow body inside an expression, as opposed to a block.
    CloseBrace { expr: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Frame {
    Paren,
    /// A function's parameter list; `expr` says whether the function is an
    /// expression (its body brace continues the statement) or a declaration.
    FnParams { expr: bool },
    Bracket,
    Brace { expr: bool },
    /// Inside `${ ... }` of a template literal.
    TemplateExpr,
}

/// A JS tokenizer that knows just enough to find statement boundaries: it
/// tracks nesting, skips strings, templates, comments and regex literals, and
/// classifies the previous token so `/` can be told apart from a regex.
struct Scanner<'a> {
    src: &'a [u8],
    pos: usize,
    end: usize,
    stack: Vec<Frame>,
    last: Option<Token<'a>>,
    token_start: usize,
    /// Set by a `function` keyword until its parameter list opens: whether
    /// that function is an expression.
    pending_fn: Option<bool>,
    /// Set when a parameter list closes (or after `=>`): whether the body
    /// brace that follows belongs to an expression.
    pending_body: Option<bool>,
    /// Set by a `class` keyword until its body opens: whether it is a class
    /// expression.
    pending_class: Option<bool>,
}

impl<'a> Scanner<'a> {
    fn new(src: &'a [u8], start: usize, end: usize) -> Self {
        Self {
            src,
            pos: start,
            end,
            stack: Vec::new(),
            last: None,
            token_start: start,
            pending_fn: None,
            pending_body: None,
            pending_class: None,
        }
    }

    fn depth(&self) -> usize {
        self.stack.len()
    }

    fn pos(&self) -> usize {
        self.pos
    }

    fn token_start(&self) -> usize {
        self.token_start
    }

    /// Whether a `function`/`class` keyword seen after `last` begins an
    /// expression rather than a declaration.
    fn in_expression_position(&self) -> bool {
        match self.last {
            None => false,
            Some(Token::Word(w)) => matches!(
                w,
                b"return" | b"typeof" | b"case" | b"in" | b"of" | b"instanceof" | b"void" | b"delete" | b"throw"
                    | b"yield" | b"await" | b"new" | b"default" | b"extends"
            ),
            Some(Token::Punct(b';')) => false,
            Some(Token::Punct(_)) => true,
            Some(Token::Open(b'{')) => false,
            Some(Token::Open(_)) => true,
            Some(Token::CloseParen) | Some(Token::CloseBracket) => false,
            Some(Token::CloseBrace { expr }) => expr,
            Some(Token::Number) | Some(Token::Str) | Some(Token::Template) | Some(Token::Regex) => false,
        }
    }

    fn peek_token(&mut self) -> Option<Token<'a>> {
        let saved = (
            self.pos,
            self.stack.clone(),
            self.last,
            self.token_start,
            self.pending_fn,
            self.pending_body,
            self.pending_class,
        );
        let tok = self.next_token();
        self.pos = saved.0;
        self.stack = saved.1;
        self.last = saved.2;
        self.token_start = saved.3;
        self.pending_fn = saved.4;
        self.pending_body = saved.5;
        self.pending_class = saved.6;
        tok
    }

    fn skip_trivia(&mut self) {
        while self.pos < self.end {
            let b = self.src[self.pos];
            if b.is_ascii_whitespace() {
                self.pos += 1;
            } else if b == b'/' && self.pos + 1 < self.end && self.src[self.pos + 1] == b'/' {
                while self.pos < self.end && self.src[self.pos] != b'\n' {
                    self.pos += 1;
                }
            } else if b == b'/' && self.pos + 1 < self.end && self.src[self.pos + 1] == b'*' {
                match find(self.src, self.pos + 2, self.end, b"*/") {
                    Some(close) => self.pos = close + 2,
                    None => self.pos = self.end,
                }
            } else {
                break;
            }
        }
    }

    fn next_token(&mut self) -> Option<Token<'a>> {
        self.skip_trivia();
        if self.pos >= self.end {
            return None;
        }
        self.token_start = self.pos;
        let b = self.src[self.pos];
        // Pending states apply to the next token only, except that a name
        // (`function name(`, `class Name extends Base {`) keeps them alive.
        let pending_body = self.pending_body.take();
        let pending_class = self.pending_class.take();
        let pending_fn = self.pending_fn.take();
        let tok = if is_ident_start(b) {
            let end = ident_end(self.src, self.pos).unwrap_or(self.pos + 1).min(self.end);
            let word = &self.src[self.pos..end];
            self.pos = end;
            match word {
                b"function" => self.pending_fn = Some(self.in_expression_position()),
                b"class" => self.pending_class = Some(self.in_expression_position()),
                _ => {
                    self.pending_fn = pending_fn;
                    self.pending_class = pending_class;
                }
            }
            Token::Word(word)
        } else if b.is_ascii_digit() || (b == b'.' && self.pos + 1 < self.end && self.src[self.pos + 1].is_ascii_digit()) {
            self.pos += 1;
            while self.pos < self.end
                && (self.src[self.pos].is_ascii_alphanumeric() || self.src[self.pos] == b'.' || self.src[self.pos] == b'_')
            {
                self.pos += 1;
            }
            Token::Number
        } else if b == b'"' || b == b'\'' {
            self.skip_string(b);
            Token::Str
        } else if b == b'`' {
            self.pos += 1;
            self.skip_template();
            Token::Template
        } else if b == b'/' && self.regex_allowed() && self.try_skip_regex() {
            Token::Regex
        } else if b == b'(' || b == b'[' || b == b'{' {
            let frame = match b {
                b'(' => match pending_fn {
                    Some(expr) => Frame::FnParams { expr },
                    None => Frame::Paren,
                },
                b'[' => Frame::Bracket,
                _ => {
                    let expr = match (pending_body, pending_class) {
                        (Some(expr), _) | (None, Some(expr)) => expr,
                        (None, None) => self.brace_is_expression(),
                    };
                    Frame::Brace { expr }
                }
            };
            self.stack.push(frame);
            self.pos += 1;
            Token::Open(b)
        } else if b == b')' || b == b']' || b == b'}' {
            self.pos += 1;
            match self.stack.pop() {
                Some(Frame::TemplateExpr) if b == b'}' => {
                    // Back inside the template literal.
                    self.skip_template();
                    Token::Template
                }
                Some(Frame::Brace { expr }) if b == b'}' => Token::CloseBrace { expr },
                Some(Frame::FnParams { expr }) if b == b')' => {
                    self.pending_body = Some(expr);
                    Token::CloseParen
                }
                _ => match b {
                    b')' => Token::CloseParen,
                    b']' => Token::CloseBracket,
                    _ => Token::CloseBrace { expr: false },
                },
            }
        } else if b == b'=' && self.pos + 1 < self.end && self.src[self.pos + 1] == b'>' {
            // Arrow: the body brace that follows continues the expression.
            self.pos += 2;
            self.pending_body = Some(true);
            Token::Punct(b'>')
        } else {
            self.pos += 1;
            Token::Punct(b)
        };
        self.last = Some(tok);
        Some(tok)
    }

    /// Whether a `{` here starts an object literal rather than a block.
    fn brace_is_expression(&self) -> bool {
        match self.last {
            None => false,
            Some(Token::Word(w)) => matches!(
                w,
                b"return" | b"typeof" | b"case" | b"in" | b"of" | b"instanceof" | b"void" | b"delete" | b"throw"
                    | b"yield" | b"await" | b"new" | b"default"
            ),
            Some(Token::Punct(b';')) => false,
            Some(Token::Punct(_)) => true,
            Some(Token::Open(_)) => true,
            Some(Token::CloseParen) | Some(Token::CloseBracket) | Some(Token::CloseBrace { .. }) => false,
            Some(Token::Number) | Some(Token::Str) | Some(Token::Template) | Some(Token::Regex) => false,
        }
    }

    fn regex_allowed(&self) -> bool {
        match self.last {
            None => true,
            Some(Token::Word(w)) => matches!(
                w,
                b"return" | b"typeof" | b"instanceof" | b"in" | b"of" | b"new" | b"delete" | b"void" | b"throw"
                    | b"case" | b"do" | b"else" | b"yield" | b"await"
            ),
            Some(Token::Number) | Some(Token::Str) | Some(Token::Template) | Some(Token::Regex) => false,
            Some(Token::CloseParen) | Some(Token::CloseBracket) => false,
            Some(Token::CloseBrace { expr }) => !expr,
            Some(Token::Punct(_)) | Some(Token::Open(_)) => true,
        }
    }

    /// Skip a regex literal starting at `self.pos` (`/`). Returns `false`, with
    /// the position untouched, when no well-formed literal ends on this line —
    /// in which case the `/` was a division after all.
    fn try_skip_regex(&mut self) -> bool {
        let mut i = self.pos + 1;
        let mut in_class = false;
        while i < self.end {
            let b = self.src[i];
            match b {
                b'\\' => i += 2,
                b'\n' | b'\r' => return false,
                b'[' => {
                    in_class = true;
                    i += 1;
                }
                b']' => {
                    in_class = false;
                    i += 1;
                }
                b'/' if !in_class => {
                    i += 1;
                    while i < self.end && self.src[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    self.pos = i;
                    return true;
                }
                _ => i += 1,
            }
        }
        false
    }

    fn skip_string(&mut self, quote: u8) {
        self.pos += 1;
        while self.pos < self.end {
            let b = self.src[self.pos];
            if b == b'\\' {
                self.pos += 2;
            } else if b == quote {
                self.pos += 1;
                return;
            } else {
                self.pos += 1;
            }
        }
    }

    /// Skip template text until the closing backtick or a `${` (which pushes a
    /// frame and returns so the expression is scanned as code).
    fn skip_template(&mut self) {
        while self.pos < self.end {
            let b = self.src[self.pos];
            if b == b'\\' {
                self.pos += 2;
            } else if b == b'`' {
                self.pos += 1;
                return;
            } else if b == b'$' && self.pos + 1 < self.end && self.src[self.pos + 1] == b'{' {
                self.stack.push(Frame::TemplateExpr);
                self.pos += 2;
                return;
            } else {
                self.pos += 1;
            }
        }
    }
}

// ─── Byte helpers ─────────────────────────────────────────────────────────────

fn leading_word(t: &[u8]) -> &[u8] {
    match ident_end(t, 0) {
        Some(end) => &t[..end],
        None => &t[..0],
    }
}

fn leading_space(t: &[u8]) -> usize {
    t.iter().take_while(|b| b.is_ascii_whitespace()).count()
}

fn trim(t: &[u8]) -> &[u8] {
    trim_end(trim_start(t))
}

fn trim_start(t: &[u8]) -> &[u8] {
    &t[leading_space(t)..]
}

fn trim_end(t: &[u8]) -> &[u8] {
    let mut j = t.len();
    while j > 0 && t[j - 1].is_ascii_whitespace() {
        j -= 1;
    }
    &t[..j]
}

fn is_blank(t: &[u8]) -> bool {
    let t = trim(t);
    t.is_empty() || t == b";"
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    find(hay, 0, hay.len(), needle).is_some()
}

fn find(src: &[u8], start: usize, end: usize, needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || end < start + needle.len() {
        return None;
    }
    src[start..end].windows(needle.len()).position(|w| w == needle).map(|i| i + start)
}

fn find_byte(src: &[u8], start: usize, end: usize, needle: u8) -> Option<usize> {
    src[start..end].iter().position(|&b| b == needle).map(|i| i + start)
}

/// Index of the bracket closing the one at `open`, scanning with full token
/// awareness so brackets inside strings, regexes and comments do not count.
fn matching_close(src: &[u8], open: usize, end: usize) -> Option<usize> {
    let mut scanner = Scanner::new(src, open, end);
    let first = scanner.next_token()?;
    if !matches!(first, Token::Open(_)) {
        return None;
    }
    while scanner.depth() > 0 {
        scanner.next_token()?;
    }
    Some(scanner.pos() - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A player in miniature: the two-statement wrapper, a `var window=this`
    /// to drop, boot calls to drop, definitions in every shape the splitter
    /// has to get right, and a URL class whose serializer applies the "n"
    /// transform the way the real one does.
    const MINI_PLAYER: &str = r#"var _yt_player={};(function(g){var window=this;
"a bare literal statement";
var Q9,e5,mD5;
Q9=function(t){return t.split("").reverse().join("")};
var e5=class{constructor(){this.S=null}},mD5=class extends Object{constructor(t){super();this.t=t}};
q1=1,q2=2;
var f_=function(t,p,Q=1){return t+p}("a","b"),n_=function(t){return t+"!"};
g.re=/[{}(]"\/`/g;
g.tpl=t=>`${t}{${"}"}`;
if(typeof g.extra==="undefined"){g.extra=1}
g.boot=function(){throw new Error("boot code ran")};
g.boot();
setTimeout(function(){document.body.innerHTML="x"},0);
g.MG=class{constructor(t){this.p={};this.raw=t}set(t,p){this.p[t]=p;return this}get(t){return this.p[t]}clone(){return this}tq(){if(this.p.n)this.p.n=Q9(this.p.n)+"_n";return this.raw}};
od=function(t,p="",Q=""){t=new g.MG(t,!0);t.set("alr","yes");Q&&t.set(p,Q9(Q));return t};
zz=function(t){var e=new g.MG(t);e.get("alr")||e.set("alr","yes");return e};
})(_yt_player);
"#;

    #[test]
    fn a_mini_player_is_rewritten_and_solves_both_challenges() {
        let prepared = prepare(MINI_PLAYER).expect("prepare");
        assert_eq!(prepared.candidates, vec!["od".to_string()], "only the direct alr=yes setter qualifies");
        assert!(!prepared.script.contains("g.boot();"), "boot call must be dropped");
        assert!(!prepared.script.contains("setTimeout("), "boot call must be dropped");
        assert!(!prepared.script.contains("var window=this"), "window shadowing must be dropped");
        assert!(prepared.script.contains("mD5=class extends"), "declaration lists stay whole");
        assert!(!prepared.script.contains("q1=1,q2=2"), "a sequence statement is dropped, as EJS does");
        assert!(prepared.script.contains("n_=function"), "an IIFE-initialised declarator does not split the list");

        let out = run_solver(&prepared.script, &[(ChallengeKind::N, "abc".into()), (ChallengeKind::Sig, "xyz".into())])
            .expect("solve");
        assert_eq!(out, vec!["cba_n".to_string(), "zyx".to_string()]);
    }

    #[test]
    fn statements_split_on_semicolons_blocks_and_asi() {
        let src = b"a=1;function f(){}\nif(x){y()}else{z()}\nb={};c=function(){}(1),d=2;\ne=x=>{return 1}\nf=2";
        let parts: Vec<String> = split_statements(src, 0..src.len())
            .into_iter()
            .map(|r| String::from_utf8_lossy(&src[r]).trim().to_string())
            .collect();
        assert_eq!(
            parts,
            vec![
                "a=1;",
                "function f(){}",
                "if(x){y()}else{z()}",
                "b={};",
                "c=function(){}(1),d=2;",
                "e=x=>{return 1}",
                "f=2",
            ]
        );
    }

    #[test]
    fn regex_literals_and_templates_do_not_confuse_nesting() {
        let src = b"a=/[}{)]/.test(b);c=`x${ {d:1}.d }y`;e=b/2/1;";
        let parts: Vec<String> = split_statements(src, 0..src.len())
            .into_iter()
            .map(|r| String::from_utf8_lossy(&src[r]).to_string())
            .collect();
        assert_eq!(parts, vec!["a=/[}{)]/.test(b);", "c=`x${ {d:1}.d }y`;", "e=b/2/1;"]);
    }

    #[test]
    fn only_definitions_are_kept() {
        assert_eq!(classify(b"a.b=function(){}"), Some(Kind::Assignment));
        assert_eq!(classify(b"a[0].b+=1;"), Some(Kind::Assignment));
        assert_eq!(classify(b"var a=1,b=2;"), Some(Kind::Declaration));
        assert_eq!(classify(b"function f(){}"), Some(Kind::FunctionDeclaration));
        assert_eq!(classify(b"class X{}"), Some(Kind::Other));
        assert_eq!(classify(b"\"use strict\";"), Some(Kind::Literal));
        assert_eq!(classify(b"a.b();"), None, "a call is boot code");
        assert_eq!(classify(b"a=1,b=2;"), None, "a sequence is dropped like EJS does");
        assert_eq!(classify(b"a==b;"), None);
        assert_eq!(classify(b"new X();"), None);
    }

    #[test]
    fn a_player_without_the_signing_function_is_rejected() {
        let src = "var _yt_player={};(function(g){var window=this;g.a=function(){return 1};})(_yt_player);";
        assert_eq!(prepare(src).unwrap_err(), PrepareError::NoSolverCandidate);
    }

    #[test]
    fn player_ids_and_urls_are_recognised() {
        assert_eq!(player_id("https://www.youtube.com/s/player/f572e43c/player_es6.vflset/en_US/base.js"), Some("f572e43c"));
        assert_eq!(player_id("https://www.youtube.com/watch?v=x"), None);
        assert_eq!(
            absolute_player_url("/s/player/f572e43c/player_ias.vflset/en_US/base.js"),
            "https://www.youtube.com/s/player/f572e43c/player_ias.vflset/en_US/base.js"
        );
        assert_eq!(absolute_player_url("//www.youtube.com/x.js"), "https://www.youtube.com/x.js");
    }

    #[test]
    fn a_signature_timestamp_is_read_from_player_source() {
        let player = load_from_source("https://x/base.js", "var a={signatureTimestamp:20697};");
        assert_eq!(player.sts, Some(20697));
        assert!(!player.can_solve());
    }
}
