//! Plan limits from Anthropic: the same numbers Claude Code's `/usage` shows
//! (session and weekly percentages, reset times, where the week's usage came
//! from). Unlike the transcripts, these count everything on the account:
//! the phone and web apps, other computers, Cowork.
//!
//! A background thread asks `GET https://api.anthropic.com/api/oauth/usage`
//! every few minutes, using the login Claude Code saved on this computer.
//! The token is read fresh each time and sent only to Anthropic.
//!
//! When the saved login has expired (Claude Desktop runs its own copy of
//! Claude Code, so the terminal login can go stale for days), the widget
//! renews it the same way Claude Code does, and writes the new tokens back
//! for Claude Code to use. Renewing rotates the refresh token, so it is done
//! only while holding Claude Code's own refresh lock (`.oauth_refresh.lock`
//! in the Claude folder), after re-reading the file under that lock. Claude
//! Code also re-reads the file under the lock before it renews, so it simply
//! picks up the tokens the widget saved. On macOS the login lives in the
//! Keychain and is never renewed here.

use chrono::{DateTime, Utc};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const URL: &str = "https://api.anthropic.com/api/oauth/usage";
const EVERY: Duration = Duration::from_secs(5 * 60);
/// Never ask more often than this, even when the drawer is opened repeatedly.
const MIN_GAP: Duration = Duration::from_secs(60);
/// Renew this long before the access token runs out.
const RENEW_BEFORE_MS: i64 = 5 * 60 * 1000;

#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    pub percent: f64,
    pub resets_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Limits {
    pub session: Option<Window>,
    pub weekly: Option<Window>,
    /// Per-model weekly limits, e.g. ("Fable", 0%).
    pub scoped: Vec<(String, Window)>,
    /// Where this week's usage came from, e.g. ("Claude Code", 50).
    pub breakdown: Vec<(String, u32)>,
    pub fetched_at: DateTime<Utc>,
}

/// Why there are no fresh numbers (shown in the drawer).
#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Off,
    Loading,
    Ok,
    /// No Claude Code login on this computer (or an API key is used).
    NoLogin,
    /// The saved login expired and couldn't be renewed here.
    Expired,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub status: Status,
    /// The last good numbers, kept while later fetches fail.
    pub limits: Option<Limits>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Snapshot {
            status: Status::Off,
            limits: None,
        }
    }
}

// ------------------------------------------------------------------ parsing --
fn window(v: &Value) -> Option<Window> {
    let percent = v.get("utilization")?.as_f64()?;
    let resets_at = DateTime::parse_from_rfc3339(v.get("resets_at")?.as_str()?)
        .ok()?
        .with_timezone(&Utc);
    Some(Window { percent, resets_at })
}

pub fn parse(body: &str, now: DateTime<Utc>) -> Option<Limits> {
    let v: Value = serde_json::from_str(body).ok()?;
    let session = window(&v["five_hour"]);
    let weekly = window(&v["seven_day"]);
    let mut scoped = Vec::new();
    for l in v["limits"].as_array().into_iter().flatten() {
        if l["kind"].as_str() != Some("weekly_scoped") {
            continue;
        }
        let name = l["scope"]["model"]["display_name"].as_str().unwrap_or("");
        let pct = l["percent"].as_f64();
        let at = l["resets_at"]
            .as_str()
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok());
        if let (false, Some(percent), Some(at)) = (name.is_empty(), pct, at) {
            scoped.push((
                name.to_string(),
                Window {
                    percent,
                    resets_at: at.with_timezone(&Utc),
                },
            ));
        }
    }
    let breakdown = v["seven_day_breakdown"]["rows"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            let name = r["display_name"].as_str()?;
            let pct = r["percent"].as_u64()? as u32;
            Some((name.to_string(), pct))
        })
        .filter(|(_, p)| *p > 0)
        .collect();
    if session.is_none() && weekly.is_none() {
        return None;
    }
    Some(Limits {
        session,
        weekly,
        scoped,
        breakdown,
        fetched_at: now,
    })
}

// -------------------------------------------------------------------- login --
enum Login {
    Token(String),
    Expired,
    Missing,
}

/// Claude Code's folder: `CLAUDE_CONFIG_DIR`, else `~/.claude`.
fn claude_dir() -> Option<PathBuf> {
    std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(|h| PathBuf::from(h).join(".claude"))
        })
}

fn credentials_json() -> Option<String> {
    // macOS keeps Claude Code's login in the Keychain (macOS asks once
    // whether this app may read it); other systems use a file.
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("security")
            .args([
                "find-generic-password",
                "-s",
                "Claude Code-credentials",
                "-w",
            ])
            .output()
            .ok()?;
        if out.status.success() {
            return Some(String::from_utf8_lossy(&out.stdout).trim().to_string());
        }
    }
    std::fs::read_to_string(claude_dir()?.join(".credentials.json")).ok()
}

fn login() -> Login {
    let Some(text) = credentials_json() else {
        return Login::Missing;
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return Login::Missing;
    };
    let o = &v["claudeAiOauth"];
    let Some(token) = o["accessToken"].as_str().filter(|t| !t.is_empty()) else {
        return Login::Missing;
    };
    let expires_ms = o["expiresAt"].as_i64().unwrap_or(i64::MAX);
    let now = Utc::now().timestamp_millis();
    if expires_ms > now + RENEW_BEFORE_MS {
        return Login::Token(token.to_string());
    }
    #[cfg(not(target_os = "macos"))]
    if let Some(t) = renew::renew() {
        return Login::Token(t);
    }
    if expires_ms > now + 30_000 {
        // Renewing didn't work out this time, but the token still works.
        return Login::Token(token.to_string());
    }
    Login::Expired
}

pub(crate) fn agent() -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("claude-usage-widget/", env!("CARGO_PKG_VERSION")))
        .build())
}

// -------------------------------------------------------------------- renew --
#[cfg(not(target_os = "macos"))]
mod renew {
    use super::{agent, claude_dir, RENEW_BEFORE_MS};
    use chrono::Utc;
    use serde_json::{json, Value};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Duration;

    /// Claude Code's token endpoint and public client id (from Claude Code).
    const TOKEN_URL: &str = "https://platform.claude.com/v1/oauth/token";
    const CLIENT_ID: &str = "9d1c250a-e61b-44d9-88ed-5944d1962f5e";
    /// Claude Code treats a lock older than this as left behind by a crash.
    const STALE: Duration = Duration::from_secs(60);

    /// A refresh token the server refused: don't send it again.
    static DEAD: Mutex<Option<String>> = Mutex::new(None);

    /// A lock folder, as Claude Code's lock library makes them (mkdir is
    /// atomic). Removed again when dropped.
    struct Lock(PathBuf);

    impl Drop for Lock {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir(&self.0);
        }
    }

    fn lock(path: PathBuf) -> Option<Lock> {
        for _ in 0..2 {
            match std::fs::create_dir(&path) {
                Ok(()) => return Some(Lock(path)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let stale = std::fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.elapsed().ok())
                        .is_some_and(|age| age > STALE);
                    if !stale {
                        return None; // Claude Code is renewing right now
                    }
                    let _ = std::fs::remove_dir(&path);
                }
                Err(_) => return None,
            }
        }
        None
    }

    fn read(file: &Path) -> Option<Value> {
        serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()
    }

    /// Write via a temporary file and a rename, so the file is never half
    /// written.
    fn write(file: &Path, v: &Value) -> std::io::Result<()> {
        let text = serde_json::to_string(v)?;
        let tmp = file.with_extension("json.widget-tmp");
        {
            use std::io::Write;
            let mut o = std::fs::OpenOptions::new();
            o.write(true).create(true).truncate(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                o.mode(0o600);
            }
            let mut f = o.open(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, file)
    }

    /// Put the server's reply into the saved login, keeping every other
    /// field. Returns the new access token.
    pub(super) fn apply(creds: &mut Value, reply: &Value, now_ms: i64) -> Option<String> {
        let access = reply["access_token"].as_str().filter(|t| !t.is_empty())?;
        let expires_in = reply["expires_in"].as_i64()?;
        let o = creds.get_mut("claudeAiOauth")?.as_object_mut()?;
        o.insert("accessToken".into(), access.into());
        if let Some(r) = reply["refresh_token"].as_str().filter(|t| !t.is_empty()) {
            o.insert("refreshToken".into(), r.into());
        }
        o.insert("expiresAt".into(), (now_ms + expires_in * 1000).into());
        if let Some(s) = reply["refresh_token_expires_in"].as_i64() {
            o.insert("refreshTokenExpiresAt".into(), (now_ms + s * 1000).into());
        }
        if let Some(sc) = reply["scope"].as_str().filter(|s| !s.trim().is_empty()) {
            let scopes: Vec<Value> = sc.split_whitespace().map(Value::from).collect();
            o.insert("scopes".into(), scopes.into());
        }
        Some(access.to_string())
    }

    fn post(body: &Value) -> Result<Value, (u16, String)> {
        let res = agent()
            .map_err(|e| (0, e))?
            .post(TOKEN_URL)
            .set("Content-Type", "application/json")
            .send_string(&body.to_string());
        match res {
            Ok(r) => {
                let text = r.into_string().map_err(|e| (0, e.to_string()))?;
                serde_json::from_str(&text).map_err(|e| (0, e.to_string()))
            }
            Err(ureq::Error::Status(code, r)) => Err((code, r.into_string().unwrap_or_default())),
            Err(e) => Err((0, e.to_string())),
        }
    }

    /// Renew the saved login. Returns a working access token, or None if
    /// there is nothing to renew, Claude Code is renewing it right now, or
    /// the server said no.
    pub(super) fn renew() -> Option<String> {
        let dir = claude_dir()?;
        let file = dir.join(".credentials.json");
        // Claude Code holds both of these while it renews.
        let _lock = lock(dir.join(".oauth_refresh.lock"))?;
        let mut legacy = dir.clone().into_os_string();
        legacy.push(".lock");
        let _legacy = lock(PathBuf::from(legacy))?;

        // Re-read under the lock: Claude Code may have just renewed it.
        let creds = read(&file)?;
        let o = &creds["claudeAiOauth"];
        let now = Utc::now().timestamp_millis();
        if o["expiresAt"].as_i64().unwrap_or(i64::MAX) > now + RENEW_BEFORE_MS {
            return o["accessToken"].as_str().map(str::to_string);
        }
        let refresh = o["refreshToken"]
            .as_str()
            .filter(|t| !t.is_empty())?
            .to_string();
        if DEAD.lock().ok()?.as_deref() == Some(refresh.as_str()) {
            return None;
        }
        let scopes: Vec<&str> = o["scopes"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let body = json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh,
            "client_id": o["clientId"].as_str().unwrap_or(CLIENT_ID),
            "scope": scopes.join(" "),
        });

        let reply = match post(&body) {
            Ok(r) => r,
            Err((400 | 401, text)) if text.contains("invalid_grant") => {
                if let Ok(mut d) = DEAD.lock() {
                    *d = Some(refresh);
                }
                log(&format!("login renewal refused: {}", short(&text)));
                return None;
            }
            Err((code, text)) => {
                log(&format!("login renewal failed ({code}): {}", short(&text)));
                return None;
            }
        };

        // The old refresh token is now used up: the new one must be saved.
        // Re-read once more and only write if nobody replaced the login.
        let now = Utc::now().timestamp_millis();
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(250 * attempt));
            }
            let Some(mut latest) = read(&file) else {
                continue;
            };
            if latest["claudeAiOauth"]["refreshToken"].as_str() != Some(refresh.as_str()) {
                // Someone signed in again meanwhile: keep theirs.
                return latest["claudeAiOauth"]["accessToken"]
                    .as_str()
                    .map(str::to_string);
            }
            let access = apply(&mut latest, &reply, now)?;
            match write(&file, &latest) {
                Ok(()) => return Some(access),
                Err(e) => log(&format!("saving renewed login failed: {e}")),
            }
        }
        None
    }

    /// Append to error.log (never the tokens themselves).
    fn log(msg: &str) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(crate::config::log_path())
        {
            let _ = writeln!(
                f,
                "[{}] {msg}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
            );
        }
    }

    fn short(s: &str) -> String {
        s.chars().take(200).collect()
    }
}

// -------------------------------------------------------------------- cache --
/// The last good numbers are kept on disk (percentages and reset times only,
/// never the login), so the drawer and taskbar show the last known limits
/// when Anthropic can't be reached, even right after start.
fn cache_path() -> PathBuf {
    crate::config::data_dir().join("limits-cache.json")
}

fn save_cache(body: &str, at: DateTime<Utc>) {
    let v = serde_json::json!({ "fetched_at": at.to_rfc3339(), "body": body });
    let _ = std::fs::write(cache_path(), v.to_string());
}

fn load_cache() -> Option<Limits> {
    let v: Value = serde_json::from_str(&std::fs::read_to_string(cache_path()).ok()?).ok()?;
    let at = DateTime::parse_from_rfc3339(v["fetched_at"].as_str()?).ok()?;
    parse(v["body"].as_str()?, at.with_timezone(&Utc))
}

// -------------------------------------------------------------------- fetch --
fn fetch(token: &str) -> Result<String, String> {
    let res = agent()?
        .get(URL)
        .set("Authorization", &format!("Bearer {token}"))
        .set("anthropic-beta", "oauth-2025-04-20")
        .set("Content-Type", "application/json")
        .call();
    match res {
        Ok(r) => r.into_string().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(401 | 403, _)) => Err("auth".into()),
        Err(ureq::Error::Status(code, _)) => Err(format!("HTTP {code}")),
        Err(e) => Err(match e.kind() {
            ureq::ErrorKind::Dns | ureq::ErrorKind::ConnectionFailed => "offline".into(),
            _ => "network error".into(),
        }),
    }
}

/// Runs the background fetcher. Cheap to query from the UI thread.
pub struct Fetcher {
    shared: Arc<Mutex<(bool, Snapshot)>>,
    wake: Sender<()>,
}

impl Fetcher {
    pub fn start(enabled: bool) -> Fetcher {
        let first = Snapshot {
            status: if enabled {
                Status::Loading
            } else {
                Status::Off
            },
            limits: if enabled { load_cache() } else { None },
        };
        let shared = Arc::new(Mutex::new((enabled, first)));
        let (wake, rx) = mpsc::channel::<()>();
        let s = shared.clone();
        std::thread::spawn(move || {
            let mut last: Option<Instant> = None;
            loop {
                let enabled = s.lock().map(|g| g.0).unwrap_or(false);
                if enabled && last.is_none_or(|t| t.elapsed() >= MIN_GAP) {
                    last = Some(Instant::now());
                    let now = Utc::now();
                    let status = match login() {
                        Login::Missing => Err(Status::NoLogin),
                        Login::Expired => Err(Status::Expired),
                        Login::Token(t) => match fetch(&t) {
                            Ok(body) => match parse(&body, now) {
                                Some(l) => {
                                    save_cache(&body, now);
                                    Ok(l)
                                }
                                None => Err(Status::Failed("unexpected reply".into())),
                            },
                            Err(e) if e == "auth" => Err(Status::Expired),
                            Err(e) => Err(Status::Failed(e)),
                        },
                    };
                    if let Ok(mut g) = s.lock() {
                        if g.0 {
                            match status {
                                Ok(l) => {
                                    g.1 = Snapshot {
                                        status: Status::Ok,
                                        limits: Some(l),
                                    }
                                }
                                Err(st) => g.1.status = st,
                            }
                        }
                    }
                }
                match rx.recv_timeout(EVERY) {
                    Ok(()) | Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
        });
        Fetcher { shared, wake }
    }

    pub fn snapshot(&self) -> Snapshot {
        self.shared.lock().map(|g| g.1.clone()).unwrap_or_default()
    }

    /// Ask again soon (e.g. the drawer was opened). Rate-limited.
    pub fn poke(&self) {
        let _ = self.wake.send(());
    }

    pub fn set_enabled(&self, on: bool) {
        if let Ok(mut g) = self.shared.lock() {
            g.0 = on;
            g.1 = Snapshot {
                status: if on { Status::Loading } else { Status::Off },
                limits: if on { load_cache() } else { None },
            };
        }
        self.poke();
    }
}

/// One fetch, described in words: for `--check-limits` troubleshooting.
pub fn check_once() -> String {
    match login() {
        Login::Missing => "no Claude Code login found on this computer".into(),
        Login::Expired => {
            "Claude Code's saved login has expired and couldn't be renewed; run Claude Code to sign in again".into()
        }
        Login::Token(t) => match fetch(&t).map(|b| parse(&b, Utc::now())) {
            Ok(Some(l)) => {
                let w = |n: &str, w: &Option<Window>| match w {
                    Some(w) => format!(
                        "{n}: {:.0}% (resets {})",
                        w.percent,
                        w.resets_at.to_rfc3339()
                    ),
                    None => format!("{n}: n/a"),
                };
                format!(
                    "ok
{}
{}",
                    w("session", &l.session),
                    w("weekly", &l.weekly)
                )
            }
            Ok(None) => "unexpected reply from Anthropic".into(),
            Err(e) => format!("failed: {e}"),
        },
    }
}

/// Made-up numbers for `--demo` and the README screenshots.
pub fn sample(now: DateTime<Utc>) -> Snapshot {
    let w = |percent, h: i64| Window {
        percent,
        resets_at: now + chrono::Duration::minutes(h),
    };
    Snapshot {
        status: Status::Ok,
        limits: Some(Limits {
            session: Some(w(34.0, 162)),
            weekly: Some(w(24.0, 60 * 33 + 28)),
            scoped: vec![("Fable".into(), w(6.0, 60 * 33 + 28))],
            breakdown: vec![
                ("Claude Code".into(), 50),
                ("Cowork".into(), 44),
                ("Chats".into(), 6),
            ],
            fetched_at: now - chrono::Duration::minutes(2),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPLY: &str = r#"{"five_hour":{"utilization":34.0,"resets_at":"2026-11-12T18:39:59.756337+00:00"},
      "seven_day":{"utilization":24.0,"resets_at":"2026-11-13T23:59:59.756357+00:00"},
      "seven_day_opus":null,
      "limits":[{"kind":"session","percent":34,"resets_at":"2026-11-12T18:39:59+00:00","scope":null},
        {"kind":"weekly_scoped","percent":0,"resets_at":"2026-11-14T00:00:00+00:00",
         "scope":{"model":{"id":null,"display_name":"Fable"},"surface":null}}],
      "seven_day_breakdown":{"rows":[{"key":"claude_code","display_name":"Claude Code","percent":50},
        {"key":"chat","display_name":"Chats","percent":6},{"key":"other","display_name":"Other","percent":0}]}}"#;

    #[test]
    fn parses_usage_reply() {
        let l = parse(REPLY, Utc::now()).expect("limits");
        assert_eq!(l.session.as_ref().unwrap().percent, 34.0);
        assert_eq!(l.weekly.as_ref().unwrap().percent, 24.0);
        assert_eq!(l.scoped.len(), 1);
        assert_eq!(l.scoped[0].0, "Fable");
        // zero-percent sources are dropped
        assert_eq!(
            l.breakdown,
            vec![("Claude Code".to_string(), 50), ("Chats".to_string(), 6)]
        );
    }

    #[test]
    fn rejects_replies_without_limits() {
        assert!(parse("{}", Utc::now()).is_none());
        assert!(parse("not json", Utc::now()).is_none());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn renewal_keeps_other_fields() {
        let mut creds: Value = serde_json::from_str(
            r#"{"claudeAiOauth":{"accessToken":"old","refreshToken":"r1","expiresAt":1,
                "scopes":["user:inference"],"subscriptionType":"pro","rateLimitTier":"x"},
                "other":{"keep":true}}"#,
        )
        .unwrap();
        let reply: Value = serde_json::from_str(
            r#"{"access_token":"new","refresh_token":"r2","expires_in":28800,
                "refresh_token_expires_in":100,"scope":"user:inference user:profile"}"#,
        )
        .unwrap();
        assert_eq!(
            renew::apply(&mut creds, &reply, 1_000).as_deref(),
            Some("new")
        );
        let o = &creds["claudeAiOauth"];
        assert_eq!(o["accessToken"], "new");
        assert_eq!(o["refreshToken"], "r2");
        assert_eq!(o["expiresAt"], 1_000 + 28_800_000);
        assert_eq!(o["refreshTokenExpiresAt"], 1_000 + 100_000);
        assert_eq!(o["scopes"][1], "user:profile");
        assert_eq!(o["subscriptionType"], "pro");
        assert_eq!(creds["other"]["keep"], true);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn renewal_without_new_refresh_token_keeps_old_one() {
        let mut creds: Value =
            serde_json::from_str(r#"{"claudeAiOauth":{"accessToken":"a","refreshToken":"r1"}}"#)
                .unwrap();
        let reply: Value = serde_json::from_str(r#"{"access_token":"b","expires_in":10}"#).unwrap();
        renew::apply(&mut creds, &reply, 0).unwrap();
        assert_eq!(creds["claudeAiOauth"]["refreshToken"], "r1");
    }
}
