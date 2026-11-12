//! Plan limits from Anthropic: the same numbers Claude Code's `/usage` shows
//! (session and weekly percentages, reset times, where the week's usage came
//! from). Unlike the transcripts, these count everything on the account:
//! the phone and web apps, other computers, Cowork.
//!
//! A background thread asks `GET https://api.anthropic.com/api/oauth/usage`
//! every few minutes, using the login Claude Code saved on this computer.
//! The token is read fresh each time and sent only to Anthropic. It is never
//! refreshed here: Claude Code rotates it, and refreshing it from a second
//! program could sign Claude Code out. When it has expired we wait for Claude
//! Code to renew it.

use chrono::{DateTime, Utc};
use serde_json::Value;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const URL: &str = "https://api.anthropic.com/api/oauth/usage";
const EVERY: Duration = Duration::from_secs(5 * 60);
/// Never ask more often than this, even when the drawer is opened repeatedly.
const MIN_GAP: Duration = Duration::from_secs(60);

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
    /// The saved login expired; Claude Code renews it next time it runs.
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
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(|h| std::path::PathBuf::from(h).join(".claude"))
        })?;
    std::fs::read_to_string(dir.join(".credentials.json")).ok()
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
    if expires_ms <= Utc::now().timestamp_millis() + 30_000 {
        return Login::Expired;
    }
    Login::Token(token.to_string())
}

// -------------------------------------------------------------------- fetch --
fn fetch(token: &str) -> Result<String, String> {
    let tls = native_tls::TlsConnector::new().map_err(|e| e.to_string())?;
    let agent = ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("claude-usage-widget/", env!("CARGO_PKG_VERSION")))
        .build();
    let res = agent
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
            limits: None,
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
                    let status = match login() {
                        Login::Missing => Err(Status::NoLogin),
                        Login::Expired => Err(Status::Expired),
                        Login::Token(t) => match fetch(&t) {
                            Ok(body) => parse(&body, Utc::now())
                                .ok_or_else(|| Status::Failed("unexpected reply".into())),
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
                limits: None,
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
            "Claude Code's saved login has expired; run Claude Code to renew it".into()
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
}
