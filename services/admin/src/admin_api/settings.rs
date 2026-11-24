use super::*;

// ----------------------------------------------------------------- settings

#[derive(Serialize, Deserialize)]
pub struct AdminSettings {
    system_prompt: String,
    display: DisplaySettings,
    /// Seconds a reply may run (30 to 3600). Always present when read; when a save leaves it out,
    /// the stored value is kept.
    #[serde(default)]
    generation_timeout_secs: Option<u32>,
    /// How many earlier questions, each with its reply, go with a new one (0 to 1000; 0 sends the
    /// whole conversation). Always present when read; when a save leaves it out, it is kept.
    #[serde(default)]
    history_turns: Option<u32>,
    /// The kill switch: while on, the server starts no replies. Always present when read; when a
    /// save leaves it out, the stored value is kept.
    #[serde(default)]
    kill_switch: Option<bool>,
    /// When the switch goes on: stop the replies already running (the default), or let them finish.
    #[serde(default)]
    kill_switch_aborts_running: Option<bool>,
    /// Days audit records are kept (7 to 3650). Always present when read; left out on a save keeps it.
    #[serde(default)]
    audit_retention_days: Option<u32>,
    /// Minutes without use before sealed content seals itself (1 to 720). Always present when read; a save that leaves it
    /// out keeps it.
    #[serde(default)]
    unseal_idle_minutes: Option<u32>,
    /// Minutes in all before it seals itself (1 to 720), however it is used.
    #[serde(default)]
    unseal_total_minutes: Option<u32>,
    /// The id of the key the server seals with now. Read-only: changing it is the rotate action.
    #[serde(default, skip_deserializing)]
    audit_key_id: Option<String>,
    /// The privacy notice clients show after pairing and whenever it changes (ADR 0012): on unless switched off. Always present
    /// when read; a save that leaves it out keeps it.
    #[serde(default)]
    privacy_notice_enabled: Option<bool>,
    /// Its wording. Reads as the default wording until the administrator writes their own; a save of the default wording, or of
    /// nothing, goes back to the default.
    #[serde(default)]
    privacy_notice_text: Option<String>,
    /// The warning shown above the input box: on unless switched off.
    #[serde(default)]
    ai_warning_enabled: Option<bool>,
    #[serde(default)]
    ai_warning_text: Option<String>,
}

/// The most characters the wording of the notice and of the warning may have.
use crate::constants::{MAX_NOTICE_CHARS, MAX_WARNING_CHARS};

/// The value to keep for a wording: what the administrator wrote, trimmed, or nothing when it is empty or is the default.
fn wording_to_keep(text: &str, default: &str) -> String {
    let t = text.trim();
    if t == default {
        String::new()
    } else {
        t.to_string()
    }
}

use crate::constants::{
    MAX_AUDIT_RETENTION_DAYS, MAX_GENERATION_TIMEOUT_SECS, MAX_HISTORY_TURNS, MIN_AUDIT_RETENTION_DAYS, MIN_GENERATION_TIMEOUT_SECS,
};

fn check_display(d: &DisplaySettings) -> Result<(), Failure> {
    if !matches!(d.theme.as_str(), "auto" | "light" | "dark") {
        return Err(bad("Theme must be auto, light or dark."));
    }
    let in_range = |v: u32, lo: u32, hi: u32| (lo..=hi).contains(&v);
    if !in_range(d.width, 300, 4000) || !in_range(d.max_height, 200, 4000) || !in_range(d.history_width, 120, 1000) {
        return Err(bad("Width must be 300-4000, max height 200-4000 and history width 120-1000."));
    }
    if !(0.0..=0.8).contains(&d.top_percent) {
        return Err(bad("Top percent must be between 0 and 0.8."));
    }
    Ok(())
}

/// The wording to show: the one stored, or the default when none (or an empty one) is.
fn wording_or_default(stored: Option<String>, default: &str) -> String {
    stored.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| default.to_string())
}

pub(crate) fn setting(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0)).ok()
}

pub async fn get_settings(State(state): St) -> Json<AdminSettings> {
    let db = state.db.lock().await;
    let c = db.conn();
    Json(AdminSettings {
        system_prompt: setting(c, "system_prompt").unwrap_or_else(|| askbar_proto::DEFAULT_SYSTEM_PROMPT.to_string()),
        display: setting(c, "display").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default(),
        generation_timeout_secs: Some(
            setting(c, "generation_timeout_secs")
                .and_then(|s| s.trim().parse().ok())
                .unwrap_or(askbar_proto::DEFAULT_GENERATION_TIMEOUT_SECS),
        ),
        history_turns: Some(setting(c, "history_turns").and_then(|s| s.trim().parse().ok()).unwrap_or(0)),
        kill_switch: Some(
            c.query_row("SELECT kill_switch FROM budgets WHERE id = 1", [], |r| r.get::<_, i64>(0)).map(|v| v != 0).unwrap_or(false),
        ),
        kill_switch_aborts_running: Some(
            setting(c, "kill_switch_aborts_running").map(|v| !matches!(v.trim(), "0" | "false")).unwrap_or(true),
        ),
        audit_retention_days: Some(setting(c, "audit_retention_days").and_then(|s| s.trim().parse().ok()).unwrap_or(30)),
        unseal_idle_minutes: Some((crate::sealed::limits(c).0.as_secs() / 60) as u32),
        unseal_total_minutes: Some((crate::sealed::limits(c).1.as_secs() / 60) as u32),
        audit_key_id: crate::db::active_audit_key(c).ok().flatten().map(|k| k.key_id),
        privacy_notice_enabled: Some(setting(c, "privacy_notice_enabled").is_none_or(|v| v.trim() != "0")),
        privacy_notice_text: Some(wording_or_default(setting(c, "privacy_notice_text"), askbar_proto::DEFAULT_PRIVACY_NOTICE)),
        ai_warning_enabled: Some(setting(c, "ai_warning_enabled").is_none_or(|v| v.trim() != "0")),
        ai_warning_text: Some(wording_or_default(setting(c, "ai_warning_text"), askbar_proto::DEFAULT_AI_WARNING)),
    })
}

pub async fn put_settings(
    State(state): St,
    headers: axum::http::HeaderMap,
    notes: Option<axum::Extension<crate::actions::ActionNotes>>,
    ApiJson(s): ApiJson<AdminSettings>,
) -> Result<StatusCode, Failure> {
    // What is worth keeping in the record of this change: the two settings that change what the stack does at once.
    // The prompt and the display are not copied into it.
    if let Some(axum::Extension(n)) = &notes {
        if let Some(days) = s.audit_retention_days {
            n.add("audit_retention_days", serde_json::json!(days));
        }
        if let Some(on) = s.kill_switch {
            n.add("kill_switch", serde_json::json!(on));
        }
        if let Some(m) = s.unseal_idle_minutes {
            n.add("unseal_idle_minutes", serde_json::json!(m));
        }
        if let Some(m) = s.unseal_total_minutes {
            n.add("unseal_total_minutes", serde_json::json!(m));
        }
        // Whether each is on is worth keeping; the wording is not copied into the record (it is in the settings).
        if let Some(on) = s.privacy_notice_enabled {
            n.add("privacy_notice_enabled", serde_json::json!(on));
        }
        if let Some(on) = s.ai_warning_enabled {
            n.add("ai_warning_enabled", serde_json::json!(on));
        }
    }
    if s.privacy_notice_text.as_deref().is_some_and(|t| t.chars().count() > MAX_NOTICE_CHARS) {
        return Err(bad(&format!("The privacy notice is at most {MAX_NOTICE_CHARS} characters.")));
    }
    if s.ai_warning_text.as_deref().is_some_and(|t| t.chars().count() > MAX_WARNING_CHARS) {
        return Err(bad(&format!("The AI warning is at most {MAX_WARNING_CHARS} characters.")));
    }
    let notice_text = s.privacy_notice_text.as_deref().map(|t| wording_to_keep(t, askbar_proto::DEFAULT_PRIVACY_NOTICE));
    let warning_text = s.ai_warning_text.as_deref().map(|t| wording_to_keep(t, askbar_proto::DEFAULT_AI_WARNING));
    let (notice_on, warning_on) =
        (s.privacy_notice_enabled.map(|b| if b { "1" } else { "0" }), s.ai_warning_enabled.map(|b| if b { "1" } else { "0" }));
    if s.system_prompt.chars().count() > MAX_PROMPT_CHARS {
        return Err(bad("The system prompt is too long."));
    }
    check_display(&s.display)?;
    if let Some(secs) = s.generation_timeout_secs {
        if !(MIN_GENERATION_TIMEOUT_SECS..=MAX_GENERATION_TIMEOUT_SECS).contains(&secs) {
            return Err(bad(&format!(
                "The generation timeout must be between {MIN_GENERATION_TIMEOUT_SECS} and {MAX_GENERATION_TIMEOUT_SECS} seconds."
            )));
        }
    }
    let display = serde_json::to_string(&s.display).map_err(|_| bad("Invalid display settings."))?;
    let timeout = s.generation_timeout_secs.map(|n| n.to_string());
    if s.history_turns.is_some_and(|n| n > MAX_HISTORY_TURNS) {
        return Err(bad(&format!("History sent with each question must be between 0 and {MAX_HISTORY_TURNS} questions.")));
    }
    let turns = s.history_turns.map(|n| n.to_string());
    if let Some(days) = s.audit_retention_days {
        if days < MIN_AUDIT_RETENTION_DAYS {
            return Err(bad(&format!("Audit records are kept for at least {MIN_AUDIT_RETENTION_DAYS} days.")));
        }
        if days > MAX_AUDIT_RETENTION_DAYS {
            return Err(bad(&format!("Audit records are kept for at most {MAX_AUDIT_RETENTION_DAYS} days.")));
        }
    }
    // Keeping records for less time destroys evidence, so it takes a fresh sign-in and no API key (ADR 0040).
    let current_days = audit_retention_days(state.db.lock().await.conn());
    if s.audit_retention_days.is_some_and(|days| days < current_days) {
        crate::web::require_fresh_signin(&state, &headers)?;
    }
    let retention = s.audit_retention_days.map(|n| n.to_string());
    // How long sealed content stays open (ADR 0011 rule 11): up to 12 hours each, and the idle time may not exceed the total.
    let (old_idle, old_total) = crate::sealed::limits(state.db.lock().await.conn());
    let (old_idle_m, old_total_m) = ((old_idle.as_secs() / 60) as u32, (old_total.as_secs() / 60) as u32);
    let (idle_m, total_m) = (s.unseal_idle_minutes.unwrap_or(old_idle_m), s.unseal_total_minutes.unwrap_or(old_total_m));
    for (n, what) in [(idle_m, "The idle time"), (total_m, "The total time")] {
        if !(1..=crate::sealed::MAX_MINUTES).contains(&n) {
            return Err(bad(&format!("{what} for sealed content must be between 1 and {} minutes.", crate::sealed::MAX_MINUTES)));
        }
    }
    if idle_m > total_m {
        return Err(bad("Sealed content cannot stay open longer without use than in all."));
    }
    let (idle_text, total_text) = (idle_m.to_string(), total_m.to_string());
    let aborts = s.kill_switch_aborts_running.map(|b| if b { "1" } else { "0" });
    let kill = s.kill_switch;
    mutate(&state, vec![], move |c| {
        let mut pairs = vec![("system_prompt", s.system_prompt.as_str()), ("display", display.as_str())];
        if let Some(t) = timeout.as_deref() {
            pairs.push(("generation_timeout_secs", t));
        }
        if let Some(n) = turns.as_deref() {
            pairs.push(("history_turns", n));
        }
        if let Some(a) = aborts {
            pairs.push(("kill_switch_aborts_running", a));
        }
        if let Some(r) = retention.as_deref() {
            pairs.push(("audit_retention_days", r));
        }
        for (key, value) in [
            ("privacy_notice_enabled", notice_on),
            ("privacy_notice_text", notice_text.as_deref()),
            ("ai_warning_enabled", warning_on),
            ("ai_warning_text", warning_text.as_deref()),
        ] {
            if let Some(v) = value {
                pairs.push((key, v));
            }
        }
        pairs.push(("unseal_idle_minutes", idle_text.as_str()));
        pairs.push(("unseal_total_minutes", total_text.as_str()));
        if let Some(k) = kill {
            c.execute("UPDATE budgets SET kill_switch = ?1 WHERE id = 1", [k as i64]).map_err(internal)?;
        }
        for (k, v) in pairs {
            c.execute(
                "INSERT INTO settings (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![k, v],
            )
            .map_err(internal)?;
        }
        Ok(())
    })
    .await?;
    // The auditor keeps the records for as long as this says: give it the setting now, not at its next round.
    if let Some(a) = state.auditor.get() {
        a.mark_dirty();
    }
    // A change of how long content stays unsealed is an alert, and applies to a session that is open (ADR 0011 rule 12).
    if (idle_m, total_m) != (old_idle_m, old_total_m) {
        state
            .sealed
            .set_limits(std::time::Duration::from_secs(u64::from(idle_m) * 60), std::time::Duration::from_secs(u64::from(total_m) * 60));
        state.sealed.alert(
            "timeouts_changed",
            format!("The time sealed content stays open was changed: {idle_m} minutes without use, {total_m} in all (it was {old_idle_m} and {old_total_m})."),
        );
    }
    Ok(StatusCode::NO_CONTENT)
}

/// One user's display defaults, instead of the server-wide ones.
pub async fn put_user_display(
    State(state): St,
    Path(user_id): Path<String>,
    ApiJson(d): ApiJson<DisplaySettings>,
) -> Result<StatusCode, Failure> {
    check_display(&d)?;
    let value = serde_json::to_string(&d).map_err(|_| bad("Invalid display settings."))?;
    let (status, body) = link(&state)?
        .request(reqwest::Method::GET, &askbar_proto::paths::join(askbar_proto::paths::SERVER_V1, "/users"), None)
        .await
        .map_err(from_push)?;
    let known = status == 200
        && serde_json::from_slice::<serde_json::Value>(&body)
            .ok()
            .is_some_and(|v| v.as_array().is_some_and(|a| a.iter().any(|u| u["id"].as_str() == Some(user_id.as_str()))));
    if !known {
        return Err(err(StatusCode::NOT_FOUND, "No such user."));
    }
    mutate(&state, vec![], move |c| {
        c.execute(
            "INSERT INTO display_defaults (user_id, value) VALUES (?1, ?2) ON CONFLICT(user_id) DO UPDATE SET value = excluded.value",
            params![user_id, value],
        )
        .map(|_| ())
        .map_err(internal)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}
