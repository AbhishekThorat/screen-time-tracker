use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tauri::{State, AppHandle, Manager, RunEvent, WebviewUrl, WebviewWindowBuilder, WindowEvent};
use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use std::process::Command;
use std::thread;
use std::time::Duration;
use std::fs;
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use cocoa::base::{id, nil};
#[cfg(target_os = "macos")]
use objc::{class, msg_send, sel, sel_impl};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lap {
    pub start_time: u64,
    pub end_time: Option<u64>,
    pub duration: Option<u64>, // in seconds
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DayRecord {
    pub date: String, // YYYY-MM-DD format
    pub total_duration: u64, // in seconds
    pub laps: Vec<Lap>,
    pub is_active: bool,
}


pub struct AppState {
    pub current_session: Arc<Mutex<Option<CurrentSession>>>,
    pub day_records: Arc<Mutex<HashMap<String, DayRecord>>>,
}

pub type AppStateArc = Arc<AppState>;

pub struct CurrentSession {
    pub start_time: Instant,
    pub day_key: String,
    pub current_lap_start: Instant,
    pub current_lap_start_timestamp: u64, // SystemTime timestamp for accurate tracking
    pub accumulated_seconds: u64, // Accumulated seconds for current lap
    pub last_activity_time: Instant, // To detect sleep/hibernate gaps
    pub is_paused: bool,
    pub user_paused: bool, // True if user manually paused, false if system paused (lock/sleep)
}

impl AppState {
    pub fn new() -> Self {
        Self {
            current_session: Arc::new(Mutex::new(None)),
            day_records: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

// Serializable version of session state for persistence
#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedSessionState {
    day_key: String,
    current_lap_start_timestamp: u64,
    accumulated_seconds: u64,
    is_paused: bool,
    // Whether the pause was initiated by the user (vs. system: lock/sleep/shutdown).
    // Older state files won't have this field, so default to false on load.
    #[serde(default)]
    user_paused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedState {
    current_session: Option<PersistedSessionState>,
    day_records: HashMap<String, DayRecord>,
    // Unix timestamp of the last time state was written to disk. Used on the next
    // launch to bound any lap that was still open when the machine shut down / crashed,
    // so time while the machine was OFF is never counted. Defaults to 0 for old files.
    #[serde(default)]
    last_heartbeat: u64,
}

// Local calendar date as "YYYY-MM-DD". We use the machine's LOCAL timezone (not UTC)
// so a "day" matches the user's real day. The day_key is fixed when a session starts
// and never changes while that session runs — this is what gives late-night work that
// crosses midnight to the day it started on.
fn local_date() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs()
}

// A day does not end at midnight. Work that runs from Friday evening into Saturday's
// small hours is still Friday's work, which is why day_key is stamped when a session
// starts and never follows the calendar on its own. What ends a day is a real break.
//
// So a new day begins only once the calendar date has changed AND either:
//   * the user has been away for at least IDLE_ROLLOVER_SECS — they stopped for the
//     night and came back; or
//   * it is past DAY_CUTOFF_HOUR — the backstop for a session that ran straight through
//     the night without ever pausing. With no break there is no gap to measure, and
//     without this the day_key would never roll over at all: that is how a lap at 12:52
//     on a Saturday afternoon ended up filed under Friday.
const IDLE_ROLLOVER_SECS: u64 = 6 * 60 * 60;
const DAY_CUTOFF_HOUR: u32 = 6;

// The rollover decision, kept free of clocks and locks so it can be tested directly.
// `gap` is the seconds since the user last stopped, and is 0 while a lap is open (they
// are working right now, so there is nothing to measure).
fn should_roll_over(date_changed: bool, gap: u64, past_cutoff: bool) -> bool {
    date_changed && (gap >= IDLE_ROLLOVER_SECS || past_cutoff)
}

// Where the outgoing day ends and today begins.
//
// A session that worked straight through the night is split at the cutoff, so the
// pre-dawn hours stay with the day they started on and only the morning counts as today.
// Otherwise the user is coming back from a break and the old day simply ended when they
// stopped, which is `now` — the lap was already closed back then.
fn rollover_boundary(working_through: bool, cutoff_ts: u64, now: u64) -> u64 {
    if working_through {
        cutoff_ts.min(now)
    } else {
        now
    }
}

// The record surgery behind merge_day_into_previous, separated from the command so the
// merge can be tested without a running app. Returns the day that absorbed the laps.
fn merge_records_into_previous(
    records: &mut HashMap<String, DayRecord>,
    date: &str,
) -> Result<String, String> {
    // Dates are "YYYY-MM-DD", so the lexicographic max below `date` is the nearest
    // earlier day that actually has a record (days the machine was off simply don't exist).
    let target = records
        .keys()
        .filter(|k| k.as_str() < date)
        .max()
        .cloned()
        .ok_or_else(|| format!("No earlier day to merge {} into", date))?;

    let source = records
        .remove(date)
        .ok_or_else(|| format!("No record for {}", date))?;

    let dest = records
        .get_mut(&target)
        .ok_or_else(|| format!("No record for {}", target))?;

    dest.laps.extend(source.laps);
    dest.laps.sort_by_key(|l| l.start_time);
    dest.total_duration = dest.laps.iter().filter_map(|l| l.duration).sum();
    // The absorbing day inherits whether the merged day was still being tracked.
    dest.is_active = source.is_active;

    Ok(target)
}

// Today's DAY_CUTOFF_HOUR as a unix timestamp, in local time.
fn cutoff_timestamp_today() -> u64 {
    use chrono::Timelike;
    chrono::Local::now()
        .with_hour(DAY_CUTOFF_HOUR)
        .and_then(|t| t.with_minute(0))
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .map(|t| t.timestamp().max(0) as u64)
        .unwrap_or_else(now_unix)
}

// Roll the session onto today if the rules above say the previous day is over.
// Returns (previous_day_key, its final total) when a rollover happened, so the caller
// can tell the user which day was just closed.
//
// Called from the unlock/wake path (the user is back after a break) and from the 30s
// autosave tick (which is what catches a session that never paused at all).
fn maybe_roll_over_day(app_handle: &AppHandle, state: &AppStateArc) -> Option<(String, u64)> {
    let mut session_guard = state.current_session.lock().unwrap();
    let mut records_guard = state.day_records.lock().unwrap();

    let session = session_guard.as_mut()?;
    let today = local_date();
    if session.day_key == today {
        return None;
    }

    let now = now_unix();
    let last_lap = records_guard.get(&session.day_key).and_then(|r| r.laps.last());
    // An open lap means the user is working right now, so there is no gap to measure.
    let working_through = last_lap.map(|l| l.duration.is_none()).unwrap_or(false);
    let gap = if working_through {
        0
    } else {
        last_lap
            .and_then(|l| l.end_time)
            .map(|end| now.saturating_sub(end))
            .unwrap_or(0)
    };

    let past_cutoff = now >= cutoff_timestamp_today();
    if !should_roll_over(true, gap, past_cutoff) {
        // Still the same working day: the date rolled over, but the user is either mid-lap
        // or back from a short break, and it isn't yet the cutoff. Late-night work stays
        // with the day it started on.
        return None;
    }

    let boundary = rollover_boundary(working_through, cutoff_timestamp_today(), now);

    let previous_day = session.day_key.clone();
    if let Some(old_record) = records_guard.get_mut(&previous_day) {
        finalize_dangling_lap(old_record, boundary);
        old_record.is_active = false;
    }
    let previous_total = records_guard
        .get(&previous_day)
        .map(|r| r.total_duration)
        .unwrap_or(0);

    // Open today. If the user worked through the boundary, today's first lap picks up
    // exactly where the old day's last lap ended, so no time falls into the crack. If the
    // session is paused (screen locked, machine asleep, or a manual pause), today starts
    // with no open lap: the unlock/resume path will add one when the user actually
    // returns, and a manual pause is still honoured across the rollover.
    let resume_now = !session.is_paused;
    let laps = if resume_now {
        vec![Lap { start_time: boundary, end_time: None, duration: None }]
    } else {
        Vec::new()
    };

    records_guard.insert(today.clone(), DayRecord {
        date: today.clone(),
        total_duration: 0,
        laps,
        is_active: true,
    });

    session.day_key = today.clone();
    if resume_now {
        session.current_lap_start = Instant::now();
        session.current_lap_start_timestamp = boundary;
        session.accumulated_seconds = 0;
    }

    println!("📅 Rolled over {} -> {} (gap {}s, cutoff {})", previous_day, today, gap, past_cutoff);

    drop(session_guard);
    drop(records_guard);
    save_state(app_handle, state);

    Some((previous_day, previous_total))
}

// Tell the user a day was closed behind their back. Deliberately a notification rather
// than a modal: wake/unlock fires constantly (a lid opened for 30s, a 3am maintenance
// wake), and a dialog that steals focus the moment you sit down is the wrong tax for a
// background tracker. The automatic call is right nearly always; when it isn't, the day
// can be merged back from the Reports view.
fn notify_day_rolled_over(app_handle: &AppHandle, previous_day: &str, previous_total: u64) {
    use tauri_plugin_notification::NotificationExt;

    let hours = previous_total / 3600;
    let minutes = (previous_total % 3600) / 60;
    let body = format!("{} ended with {}h {}m tracked. Now tracking today.", previous_day, hours, minutes);

    match app_handle
        .notification()
        .builder()
        .title("Started a new day")
        .body(&body)
        .show()
    {
        Ok(_) => println!("✅ Rollover notification shown"),
        Err(e) => eprintln!("❌ Failed to show rollover notification: {}", e),
    }
}

// Run the rollover check and notify if it fired.
fn roll_over_day_if_due(app_handle: &AppHandle, state: &AppStateArc) {
    if let Some((previous_day, previous_total)) = maybe_roll_over_day(app_handle, state) {
        notify_day_rolled_over(app_handle, &previous_day, previous_total);
    }
}

// Close any still-open lap (duration == None) in a day record, ending it at `end_ts`
// instead of "now". Used on startup to exclude time the machine spent powered off.
// Recomputes the record's total_duration afterwards.
fn finalize_dangling_lap(day_record: &mut DayRecord, end_ts: u64) {
    if let Some(last_lap) = day_record.laps.last_mut() {
        if last_lap.duration.is_none() {
            // Guard against a heartbeat that is somehow before the lap start.
            let end = end_ts.max(last_lap.start_time);
            last_lap.end_time = Some(end);
            last_lap.duration = Some(end - last_lap.start_time);
        }
    }
    day_record.total_duration = day_record.laps.iter().filter_map(|lap| lap.duration).sum();
}

// Get the path to the state file
fn get_state_file_path(app_handle: &AppHandle) -> PathBuf {
    let app_data_dir = app_handle.path().app_data_dir().unwrap();
    fs::create_dir_all(&app_data_dir).ok();
    app_data_dir.join("state.json")
}

// Last known-good snapshot, rewritten on every successful load. It is the fallback if
// state.json is ever found corrupt (see load_and_initialize).
fn get_backup_file_path(app_handle: &AppHandle) -> PathBuf {
    let app_data_dir = app_handle.path().app_data_dir().unwrap();
    fs::create_dir_all(&app_data_dir).ok();
    app_data_dir.join("state.backup.json")
}

// Read and parse a state file.
//   None            -> the file does not exist (a genuine first run)
//   Some(Err(..))   -> the file exists but is unreadable or unparseable (corruption)
// The distinction matters: a missing file means "start fresh", but a corrupt file must
// never be treated that way, or we would blank the history and save over the only copy.
fn read_state_file(path: &Path) -> Option<Result<PersistedState, String>> {
    if !path.exists() {
        return None;
    }
    match fs::read_to_string(path) {
        Ok(json) => Some(serde_json::from_str::<PersistedState>(&json).map_err(|e| e.to_string())),
        Err(e) => Some(Err(e.to_string())),
    }
}

// Save state to disk
fn save_state(app_handle: &AppHandle, state: &AppStateArc) {
    let session_guard = state.current_session.lock().unwrap();
    let records_guard = state.day_records.lock().unwrap();
    
    let persisted_session = session_guard.as_ref().map(|session| {
        PersistedSessionState {
            day_key: session.day_key.clone(),
            current_lap_start_timestamp: session.current_lap_start_timestamp,
            accumulated_seconds: session.accumulated_seconds,
            is_paused: session.is_paused,
            user_paused: session.user_paused,
        }
    });

    let persisted_state = PersistedState {
        current_session: persisted_session,
        day_records: records_guard.clone(),
        last_heartbeat: now_unix(),
    };
    
    let state_file = get_state_file_path(app_handle);
    if let Ok(json) = serde_json::to_string_pretty(&persisted_state) {
        // Write to a sibling temp file, then rename into place. rename(2) within a
        // filesystem is atomic, so a crash or power cut mid-save can never leave a
        // half-written state.json behind — the reader either sees the whole old file or
        // the whole new one. A plain fs::write truncates first, and dying in that window
        // used to leave a truncated file that the next launch could not parse.
        let tmp_file = state_file.with_extension("json.tmp");
        if fs::write(&tmp_file, &json).is_ok() && fs::rename(&tmp_file, &state_file).is_ok() {
            println!("✅ State saved successfully");
        } else {
            eprintln!("❌ Failed to save state to {}", state_file.display());
            fs::remove_file(&tmp_file).ok();
        }
    }
}

// Build a brand-new active session + day record for `today`, seeded with one open lap.
// Used both for a genuinely fresh day and for a new day after an overnight shutdown.
fn begin_fresh_day(records: &mut HashMap<String, DayRecord>, today: &str) -> CurrentSession {
    let now = Instant::now();
    let current_time = now_unix();

    records.insert(today.to_string(), DayRecord {
        date: today.to_string(),
        total_duration: 0,
        laps: vec![Lap {
            start_time: current_time,
            end_time: None,
            duration: None,
        }],
        is_active: true,
    });

    CurrentSession {
        start_time: now,
        day_key: today.to_string(),
        current_lap_start: now,
        current_lap_start_timestamp: current_time,
        accumulated_seconds: 0,
        last_activity_time: now,
        is_paused: false,
        user_paused: false,
    }
}

// Load persisted state from disk and decide what today's session should be.
//
// Rules (all dates are LOCAL dates):
//   * Fresh boot on a new day, or a day with no record yet -> auto-start a new active
//     day in the background. The user can manually pause if they aren't working.
//   * Machine restarted (e.g. power cut) on the SAME day the ongoing session belongs to
//     -> continue that same day, appending a new lap (unless the user had manually paused,
//     in which case we respect the pause and don't resume).
//   * The ongoing session belongs to an EARLIER day -> apply the same rule the running app
//     uses (should_roll_over). If the user was away long enough, or it is past the cutoff,
//     that day is finalized and a fresh one starts for today. If not — the app just
//     restarted a few minutes after midnight while they were still working — the earlier
//     day CONTINUES, because late-night work belongs to the day it started on.
//   * A day the user explicitly ended -> left alone; we do NOT auto-restart it.
//
// Any lap left open when the app last stopped is closed at `last_heartbeat` so that
// time while the machine was powered off is never counted.
fn load_and_initialize(app_handle: &AppHandle, state: &AppStateArc) {
    let state_file = get_state_file_path(app_handle);
    let backup_file = get_backup_file_path(app_handle);
    let today = local_date();

    let persisted_state = match read_state_file(&state_file) {
        Some(Ok(loaded)) => Some(loaded),
        // The file is genuinely absent -> first ever run.
        None => None,
        Some(Err(err)) => {
            // state.json exists but will not parse. This must NOT fall through to the
            // "first run" path: that blanks day_records and immediately writes the empty
            // state back, destroying every day the user ever tracked. Instead, quarantine
            // the bad file (so it is still there to inspect or hand-recover) and fall back
            // to the snapshot taken on the last successful load.
            eprintln!("❌ state.json is corrupt: {}", err);
            let quarantine = state_file.with_file_name(format!("state.corrupt-{}.json", now_unix()));
            if fs::rename(&state_file, &quarantine).is_ok() {
                eprintln!("   Corrupt file preserved at {}", quarantine.display());
            }
            match read_state_file(&backup_file) {
                Some(Ok(recovered)) => {
                    println!("✅ Recovered history from {}", backup_file.display());
                    Some(recovered)
                }
                _ => {
                    eprintln!("❌ No usable backup either; starting with empty history");
                    None
                }
            }
        }
    };

    let mut records_guard = state.day_records.lock().unwrap();
    let mut session_guard = state.current_session.lock().unwrap();

    let Some(persisted_state) = persisted_state else {
        // No prior state at all -> very first run. Auto-start today in the background.
        *records_guard = HashMap::new();
        *session_guard = Some(begin_fresh_day(&mut records_guard, &today));
        println!("✅ No prior state; auto-started a fresh day for {}", today);
        drop(session_guard);
        drop(records_guard);
        save_state(app_handle, state);
        return;
    };

    // Snapshot the history we just loaded, before this run starts mutating it. If a later
    // write is ever cut short, this is what the recovery path above restores from.
    if let Ok(json) = serde_json::to_string_pretty(&persisted_state) {
        fs::write(&backup_file, json).ok();
    }

    *records_guard = persisted_state.day_records;
    // Bound any lap that was still open at shutdown to the last heartbeat we recorded.
    let heartbeat = if persisted_state.last_heartbeat > 0 {
        persisted_state.last_heartbeat
    } else {
        now_unix()
    };
    for record in records_guard.values_mut() {
        finalize_dangling_lap(record, heartbeat);
    }

    // Whether the ongoing session's day is over, judged by the same rule the running app
    // uses (see should_roll_over). The app being down is not itself evidence of a new day:
    // launchd's KeepAlive can relaunch it seconds after a crash, and a plain "the date
    // changed while we were down" test would split a night owl's session at midnight.
    // What ends a day is a real break — measured here from the last heartbeat, which is
    // also where any lap left open was just closed.
    let downtime = now_unix().saturating_sub(heartbeat);
    let past_cutoff = now_unix() >= cutoff_timestamp_today();

    match persisted_state.current_session {
        Some(ps)
            if ps.day_key == today
                || !should_roll_over(true, downtime, past_cutoff) =>
        {
            // Continue the ongoing session's day. Either it is still that day (a mid-day
            // restart or power cut), or the date has changed but the user was working
            // through it and the app merely restarted — in which case the work still
            // belongs to the day it started on.
            let day = ps.day_key.clone();
            if let Some(record) = records_guard.get_mut(&day) {
                record.is_active = true;
            } else {
                records_guard.insert(day.clone(), DayRecord {
                    date: day.clone(),
                    total_duration: 0,
                    laps: Vec::new(),
                    is_active: true,
                });
            }

            if ps.user_paused {
                // User had manually paused before the restart -> respect it, stay paused.
                let now = Instant::now();
                *session_guard = Some(CurrentSession {
                    start_time: now,
                    day_key: day.clone(),
                    current_lap_start: now,
                    current_lap_start_timestamp: now_unix(),
                    accumulated_seconds: ps.accumulated_seconds,
                    last_activity_time: now,
                    is_paused: true,
                    user_paused: true,
                });
                println!("✅ Restored paused session for {} (user paused; not resuming)", day);
            } else {
                // Continue the existing day by appending a fresh lap.
                let now = Instant::now();
                let current_time = now_unix();
                if let Some(record) = records_guard.get_mut(&day) {
                    record.laps.push(Lap {
                        start_time: current_time,
                        end_time: None,
                        duration: None,
                    });
                }
                *session_guard = Some(CurrentSession {
                    start_time: now,
                    day_key: day.clone(),
                    current_lap_start: now,
                    current_lap_start_timestamp: current_time,
                    accumulated_seconds: 0,
                    last_activity_time: now,
                    is_paused: false,
                    user_paused: false,
                });
                if day == today {
                    println!("✅ Continued ongoing day {} with a new lap (restart detected)", day);
                } else {
                    println!("✅ Continued {} past midnight (down {}s; not a new day yet)", day, downtime);
                }
            }
        }
        Some(ps) => {
            // The session's day is genuinely over: the user was away long enough, or it is
            // past the cutoff. End that day and start a fresh one for today.
            if let Some(record) = records_guard.get_mut(&ps.day_key) {
                record.is_active = false;
            }
            *session_guard = Some(begin_fresh_day(&mut records_guard, &today));
            println!("✅ Ended previous day {} and auto-started a fresh day for {}", ps.day_key, today);
        }
        None => {
            // No ongoing session was persisted (user had ended their day, or clean state).
            match records_guard.get(&today) {
                Some(record) if !record.is_active => {
                    // User already ended today's day -> don't auto-restart it.
                    println!("ℹ️ Today's day ({}) was already ended; not auto-starting", today);
                }
                _ => {
                    *session_guard = Some(begin_fresh_day(&mut records_guard, &today));
                    println!("✅ Auto-started a fresh day for {}", today);
                }
            }
        }
    }

    drop(session_guard);
    drop(records_guard);
    save_state(app_handle, state);
    println!("✅ State loaded and initialized successfully");
}

// Raw wall-clock elapsed since the lap started. Only valid for closing a lap that
// was awake the whole time (lock, user pause, end-day). Laps interrupted by system
// sleep must NOT be closed with this — use handle_system_suspend_direct, which
// bounds the lap to a pre-sleep timestamp instead of "now".
// Pass in the actual lap start time from records to ensure accuracy
fn finalize_lap_duration(lap_start_time: u64) -> u64 {
    let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    current_time - lap_start_time
}

// Get the actual start time of the active lap from records
fn get_active_lap_start_time(day_record: &DayRecord) -> Option<u64> {
    day_record.laps.iter()
        .filter(|lap| lap.duration.is_none())
        .last()
        .map(|lap| lap.start_time)
}

#[tauri::command]
async fn start_day(state: State<'_, AppStateArc>) -> Result<String, String> {
    let today = local_date();

    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    // Check if already tracking today
    if session_guard.is_some() {
        return Err("Already tracking today's session".to_string());
    }
    
    let now = Instant::now();
    let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    
    let session = CurrentSession {
        start_time: now,
        day_key: today.clone(),
        current_lap_start: now,
        current_lap_start_timestamp: current_time,
        accumulated_seconds: 0,
        last_activity_time: now,
        is_paused: false,
        user_paused: false,
    };
    
    *session_guard = Some(session);

    let new_lap = Lap {
        start_time: current_time,
        end_time: None,
        duration: None,
    };

    // If a record already exists for today (e.g. the user ended their day earlier and is
    // starting again), APPEND a new lap to it so previous laps are preserved. Otherwise
    // create a fresh record for the day.
    if let Some(existing) = records_guard.get_mut(&today) {
        existing.laps.push(new_lap);
        existing.is_active = true;
    } else {
        records_guard.insert(today.clone(), DayRecord {
            date: today.clone(),
            total_duration: 0,
            laps: vec![new_lap],
            is_active: true,
        });
    }

    Ok(format!("Started tracking for {}", today))
}

#[tauri::command]
async fn end_day(state: State<'_, AppStateArc>, app_handle: AppHandle) -> Result<DayRecord, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    let session = session_guard.take().ok_or("No active session")?;
    let day_key = session.day_key.clone();
    
    // Calculate final duration for current lap (excluding sleep/hibernate time)
    let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    
    let result = if let Some(day_record) = records_guard.get_mut(&day_key) {
        // Get actual lap start time from records
        let lap_start_time = get_active_lap_start_time(day_record)
            .unwrap_or(session.current_lap_start_timestamp);
        let lap_duration = finalize_lap_duration(lap_start_time);
        
        // Update the last lap
        if let Some(last_lap) = day_record.laps.last_mut() {
            last_lap.end_time = Some(current_time);
            last_lap.duration = Some(lap_duration);
        }
        
        // Calculate total duration
        day_record.total_duration = day_record.laps.iter()
            .filter_map(|lap| lap.duration)
            .sum();
        
        day_record.is_active = false;
        
        Ok(day_record.clone())
    } else {
        Err("Day record not found".to_string())
    };
    
    // Release locks before saving
    drop(session_guard);
    drop(records_guard);
    
    // Save state to disk
    save_state(&app_handle, &state);
    
    result
}

#[tauri::command]
async fn handle_screen_lock(state: State<'_, AppStateArc>) -> Result<String, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_mut() {
        // Skip if already paused (prevent duplicate events)
        if session.is_paused {
            return Ok("Already paused".to_string());
        }
        
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        
        // Get actual lap start time from records
        let lap_start_time = if let Some(day_record) = records_guard.get(&session.day_key) {
            get_active_lap_start_time(day_record)
                .unwrap_or(session.current_lap_start_timestamp)
        } else {
            session.current_lap_start_timestamp
        };
        
        let lap_duration = finalize_lap_duration(lap_start_time);
        
        // End current lap
        if let Some(day_record) = records_guard.get_mut(&session.day_key) {
            if let Some(last_lap) = day_record.laps.last_mut() {
                last_lap.end_time = Some(current_time);
                last_lap.duration = Some(lap_duration);
            }
        }
        
        // Mark as paused by system
        session.is_paused = true;
        session.user_paused = false; // System paused
        
        Ok("Screen locked - timer paused".to_string())
    } else {
        Ok("No active session".to_string())
    }
}

#[tauri::command]
async fn handle_screen_unlock(state: State<'_, AppStateArc>) -> Result<String, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_mut() {
        // Skip if already active (prevent duplicate events)
        if !session.is_paused {
            return Ok("Already active".to_string());
        }
        
        // Only auto-start if user didn't manually pause
        if !session.user_paused {
            let now = Instant::now();
            let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            
            // Start new lap
            if let Some(day_record) = records_guard.get_mut(&session.day_key) {
                day_record.laps.push(Lap {
                    start_time: current_time,
                    end_time: None,
                    duration: None,
                });
                
            }
            
            // Reset lap tracking
            session.current_lap_start = now;
            session.current_lap_start_timestamp = current_time;
            session.accumulated_seconds = 0;
            session.last_activity_time = now;
            session.is_paused = false;
            
            Ok("Screen unlocked - new lap started".to_string())
        } else {
            Ok("Screen unlocked - session remains paused (user paused)".to_string())
        }
    } else {
        Ok("No active session".to_string())
    }
}

#[tauri::command]
async fn get_current_status(state: State<'_, AppStateArc>) -> Result<Option<CurrentStatus>, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_mut() {
        // Calculate total duration from completed laps only
        let mut total_duration = 0u64;
        if let Some(day_record) = records_guard.get(&session.day_key) {
            // Sum all completed laps
            total_duration = day_record.laps.iter()
                .filter_map(|lap| lap.duration)
                .sum();
        }
        
        if session.is_paused {
            // Session is paused - show only completed laps, no current lap time
            Ok(Some(CurrentStatus {
                day_key: session.day_key.clone(),
                current_lap_duration: 0, // No current lap when paused
                current_lap_start_timestamp: session.current_lap_start_timestamp,
                total_session_duration: total_duration, // Only completed laps
                is_active: false, // Not actively tracking
            }))
        } else {
            // Session is active - use session's current_lap_start_timestamp as source of truth
            let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            let current_lap_seconds = current_time - session.current_lap_start_timestamp;
            
            // IMPORTANT: total_session_duration should be ONLY completed laps
            // Frontend will add current_lap_duration for smooth display
            Ok(Some(CurrentStatus {
                day_key: session.day_key.clone(),
                current_lap_duration: current_lap_seconds,
                current_lap_start_timestamp: session.current_lap_start_timestamp,
                total_session_duration: total_duration, // Only completed laps, NOT including current lap
                is_active: true,
            }))
        }
    } else {
        Ok(None)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CurrentStatus {
    pub day_key: String,
    pub current_lap_duration: u64,
    pub current_lap_start_timestamp: u64, // For frontend smooth display
    pub total_session_duration: u64,
    pub is_active: bool,
}


#[tauri::command]
async fn get_current_day_laps(state: State<'_, AppStateArc>) -> Result<Vec<Lap>, String> {
    let session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_ref() {
        if let Some(day_record) = records_guard.get(&session.day_key) {
            Ok(day_record.laps.clone())
        } else {
            Ok(Vec::new())
        }
    } else {
        Ok(Vec::new())
    }
}

// Return every stored day record, most recent day first, so the frontend can render
// the full per-day history (each day with all of its laps and total duration).
#[tauri::command]
async fn get_all_day_records(state: State<'_, AppStateArc>) -> Result<Vec<DayRecord>, String> {
    let records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    let mut records: Vec<DayRecord> = records_guard.values().cloned().collect();
    // Dates are "YYYY-MM-DD" so lexicographic sort == chronological sort.
    records.sort_by(|a, b| b.date.cmp(&a.date));
    Ok(records)
}

// Undo an automatic rollover: fold `date`'s laps back into the day before it. The
// rollover rules get the call right nearly always, but the 00:00-06:00 window is
// genuinely ambiguous — a 5h break before an early start reads exactly like a late night —
// and only the user knows which it was. This is the escape hatch, so the ambiguity costs
// one click rather than a dialog every morning.
#[tauri::command]
async fn merge_day_into_previous(
    app_handle: AppHandle,
    state: State<'_, AppStateArc>,
    date: String,
) -> Result<String, String> {
    let state_arc = state.inner().clone();
    let target = {
        let mut session_guard = state_arc.current_session.lock().map_err(|e| e.to_string())?;
        let mut records_guard = state_arc.day_records.lock().map_err(|e| e.to_string())?;

        let target = merge_records_into_previous(&mut records_guard, &date)?;

        // If the day being merged away is the one currently being tracked, the live session
        // has to follow it, or the next lap would recreate the record we just removed.
        if let Some(session) = session_guard.as_mut() {
            if session.day_key == date {
                session.day_key = target.clone();
            }
        }

        drop(session_guard);
        drop(records_guard);
        println!("↩️ Merged {} into {}", date, target);
        target
    };

    save_state(&app_handle, &state_arc);
    Ok(format!("Merged {} into {}", date, target))
}

#[tauri::command]
async fn add_lap(state: State<'_, AppStateArc>) -> Result<String, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_mut() {
        let now = Instant::now();
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        
        // Only finalize the last lap if it's still active (no duration set)
        if let Some(day_record) = records_guard.get_mut(&session.day_key) {
            if let Some(last_lap) = day_record.laps.last_mut() {
                // Only finalize if this lap is still active (duration is None)
                if last_lap.duration.is_none() {
                    let lap_start_time = last_lap.start_time;
                    let lap_duration = finalize_lap_duration(lap_start_time);
                    
                    if lap_duration > 1 {
                        last_lap.end_time = Some(current_time);
                        last_lap.duration = Some(lap_duration);
                    }
                }
            }
            
            // Start new lap
            day_record.laps.push(Lap {
                start_time: current_time,
                end_time: None,
                duration: None,
            });
            
        }
        
        // Reset current lap tracking and resume session
        session.current_lap_start = now;
        session.current_lap_start_timestamp = current_time;
        session.accumulated_seconds = 0;
        session.last_activity_time = now;
        session.is_paused = false; // Resume the session
        session.user_paused = false; // Clear user pause flag
        Ok("New lap added successfully - session resumed".to_string())
    } else {
        Err("No active session".to_string())
    }
}

#[tauri::command]
async fn stop_lap(state: State<'_, AppStateArc>) -> Result<String, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_mut() {
        if session.is_paused {
            return Err("Session is already paused".to_string());
        }
        
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        
        // Get actual lap start time from records
        let lap_start_time = if let Some(day_record) = records_guard.get(&session.day_key) {
            get_active_lap_start_time(day_record)
                .unwrap_or(session.current_lap_start_timestamp)
        } else {
            session.current_lap_start_timestamp
        };
        
        let lap_duration = finalize_lap_duration(lap_start_time);
        
        // If lap is very short (< 3 seconds), remove it instead of keeping it
        if lap_duration < 3 {
            if let Some(day_record) = records_guard.get_mut(&session.day_key) {
                // Remove the last lap if it's too short
                if let Some(last_lap) = day_record.laps.last() {
                    if last_lap.duration.is_none() {
                        // This is the active lap, remove it
                        day_record.laps.pop();
                        
                        // Mark session as paused by user and reset accumulated time
                        session.is_paused = true;
                        session.user_paused = true; // User manually paused
                        session.accumulated_seconds = 0;
                        
                        return Ok("Very short lap removed - session paused".to_string());
                    }
                }
            }
        }
        
        // End current lap normally
        if let Some(day_record) = records_guard.get_mut(&session.day_key) {
            if let Some(last_lap) = day_record.laps.last_mut() {
                if last_lap.duration.is_none() {
                    last_lap.end_time = Some(current_time);
                    last_lap.duration = Some(lap_duration);
                }
            }
        }
        
        // Mark session as paused by user (not ended)
        session.is_paused = true;
        session.user_paused = true; // User manually paused
        
        Ok("Lap stopped - session paused".to_string())
    } else {
        Err("No active session".to_string())
    }
}



#[tauri::command]
async fn check_screen_lock_state() -> Result<bool, String> {
    // Use the same method as the monitoring function
    check_screen_lock_state_sync()
}

#[tauri::command]
async fn test_screen_lock_detection() -> Result<String, String> {
    // Test all detection methods
    let mut results = Vec::new();
    
    // Method 1: Display sleep check
    let display_output = Command::new("sh")
        .arg("-c")
        .arg("pmset -g ps")
        .output()
        .map_err(|e| e.to_string())?;
    let display_str = String::from_utf8_lossy(&display_output.stdout);
    results.push(format!("Power state: {}", display_str.trim()));
    
    // Method 2: Screen saver check
    let screensaver_output = Command::new("sh")
        .arg("-c")
        .arg("ps aux | grep -E 'ScreenSaverEngine' | grep -v grep")
        .output()
        .map_err(|e| e.to_string())?;
    results.push(format!("Screen saver: {}", if screensaver_output.stdout.is_empty() { "Not running" } else { "Running" }));
    
    // Method 3: Login window check
    let login_output = Command::new("sh")
        .arg("-c")
        .arg("ps aux | grep -E 'loginwindow' | grep -v grep | wc -l")
        .output()
        .map_err(|e| e.to_string())?;
    let login_count = String::from_utf8_lossy(&login_output.stdout).trim().parse::<i32>().unwrap_or(0);
    results.push(format!("Login windows: {}", login_count));
    
    // Method 4: Current detection result
    let current_result = check_screen_lock_state_sync();
    results.push(format!("Detection result: {}", if current_result.unwrap_or(false) { "LOCKED" } else { "UNLOCKED" }));
    
    Ok(results.join("\n"))
}


#[tauri::command]
async fn handle_system_sleep(state: State<'_, AppStateArc>) -> Result<String, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_mut() {
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        
        // Get actual lap start time from records
        let lap_start_time = if let Some(day_record) = records_guard.get(&session.day_key) {
            get_active_lap_start_time(day_record)
                .unwrap_or(session.current_lap_start_timestamp)
        } else {
            session.current_lap_start_timestamp
        };
        
        let lap_duration = finalize_lap_duration(lap_start_time);
        
        // End current lap
        if let Some(day_record) = records_guard.get_mut(&session.day_key) {
            if let Some(last_lap) = day_record.laps.last_mut() {
                last_lap.end_time = Some(current_time);
                last_lap.duration = Some(lap_duration);
            }
        }
        
        // Mark session as paused
        session.is_paused = true;
        
        Ok("System sleep detected - lap paused".to_string())
    } else {
        Ok("No active session".to_string())
    }
}

#[tauri::command]
async fn handle_system_wake(state: State<'_, AppStateArc>) -> Result<String, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_mut() {
        let now = Instant::now();
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        
        // Start new lap
        if let Some(day_record) = records_guard.get_mut(&session.day_key) {
            day_record.laps.push(Lap {
                start_time: current_time,
                end_time: None,
                duration: None,
            });
            
        }
        
        // Reset lap tracking
        session.current_lap_start = now;
        session.current_lap_start_timestamp = current_time;
        session.accumulated_seconds = 0;
        session.last_activity_time = now;
        session.is_paused = false; // Resume the session
        
        Ok("System wake detected - new lap started".to_string())
    } else {
        Ok("No active session".to_string())
    }
}

#[tauri::command]
async fn handle_user_logout(state: State<'_, AppStateArc>) -> Result<String, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_mut() {
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        
        // Get actual lap start time from records
        let lap_start_time = if let Some(day_record) = records_guard.get(&session.day_key) {
            get_active_lap_start_time(day_record)
                .unwrap_or(session.current_lap_start_timestamp)
        } else {
            session.current_lap_start_timestamp
        };
        
        let lap_duration = finalize_lap_duration(lap_start_time);
        
        // End current lap
        if let Some(day_record) = records_guard.get_mut(&session.day_key) {
            if let Some(last_lap) = day_record.laps.last_mut() {
                last_lap.end_time = Some(current_time);
                last_lap.duration = Some(lap_duration);
            }
        }
        
        // Mark session as paused
        session.is_paused = true;
        
        Ok("User logout detected - lap paused".to_string())
    } else {
        Ok("No active session".to_string())
    }
}

#[tauri::command]
async fn handle_user_login(state: State<'_, AppStateArc>) -> Result<String, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_mut() {
        let now = Instant::now();
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        
        // Start new lap
        if let Some(day_record) = records_guard.get_mut(&session.day_key) {
            day_record.laps.push(Lap {
                start_time: current_time,
                end_time: None,
                duration: None,
            });
            
        }
        
        // Reset lap tracking
        session.current_lap_start = now;
        session.current_lap_start_timestamp = current_time;
        session.accumulated_seconds = 0;
        session.last_activity_time = now;
        session.is_paused = false; // Resume the session
        
        Ok("User login detected - new lap started".to_string())
    } else {
        Ok("No active session".to_string())
    }
}

// If two consecutive iterations of the 1s monitoring loop are separated by more
// than this many seconds of wall-clock time, the process was suspended in between
// (system sleep) rather than merely scheduled late.
const SUSPEND_GAP_THRESHOLD_SECS: u64 = 10;

// Wall-clock timestamp of the last NSWorkspace willSleep notification, 0 once
// consumed. Lets the monitoring loop resync its lock-state machine even after a
// sleep too short to trip the gap check above (otherwise a <10s sleep that wakes
// to an unlocked screen would leave the session paused with nothing to resume it).
static SLEEP_NOTIFIED_AT: AtomicU64 = AtomicU64::new(0);

// Register for NSWorkspace's willSleep notification so the open lap is closed at
// the exact moment the machine goes to sleep. The polling thread can't do this:
// it is frozen during sleep and only learns about it after wake (see the gap
// detector in start_system_monitoring, which remains as a safety net).
// No didWake observer is needed — on wake, the monitoring loop's gap detector and
// lock-state check decide whether to resume immediately (woke unlocked) or wait
// for the real unlock (woke locked). Resuming blindly on didWake would create a
// phantom 1-2s lap whenever the Mac wakes to a locked screen.
#[cfg(target_os = "macos")]
fn register_sleep_observer(app_handle: AppHandle, state: AppStateArc) {
    use block::ConcreteBlock;
    use cocoa::foundation::NSString;

    unsafe {
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let center: id = msg_send![workspace, notificationCenter];
        let name = NSString::alloc(nil).init_str("NSWorkspaceWillSleepNotification");

        let block = ConcreteBlock::new(move |_notification: id| {
            println!("💤 NSWorkspaceWillSleepNotification - closing open lap before suspend");
            let now = now_unix();
            handle_system_suspend_direct(&app_handle, &state, now);
            SLEEP_NOTIFIED_AT.store(now, Ordering::Relaxed);
        });
        // The notification center copies the block, but keep our copy alive too:
        // the observer is registered once and never removed.
        let block = block.copy();
        let block_ptr = &*block as *const _ as *const std::ffi::c_void;
        let _observer: id = msg_send![center, addObserverForName: name
                                                          object: nil
                                                           queue: nil
                                                      usingBlock: block_ptr];
        std::mem::forget(block);
    }
}

// System monitoring functions
fn start_system_monitoring(app_handle: AppHandle, state: AppStateArc) {
    let state_clone = state.clone();
    let app_handle_clone = app_handle.clone();
    
    thread::spawn(move || {
        let mut last_screen_lock_state = false;
        let mut lock_detection_count = 0;
        let mut unlock_detection_count = 0;
        let mut last_iteration_ts = now_unix();

        loop {
            // Suspend (system sleep) detection. Polling can never observe the sleep
            // transition itself: macOS freezes this thread along with the process, so
            // the first thing we see is the wake. What we CAN observe is the aftermath —
            // wall-clock time jumping far beyond the 1s we slept between iterations.
            // Treat a large jump as "the process was suspended" and retroactively close
            // the open lap at the last pre-gap timestamp, so none of the asleep interval
            // is counted as active time. The NSWorkspaceWillSleepNotification observer
            // normally closes the lap first (at the exact sleep moment); this is the
            // safety net for a missed notification.
            let iteration_ts = now_unix();
            let gap_detected =
                iteration_ts.saturating_sub(last_iteration_ts) > SUSPEND_GAP_THRESHOLD_SECS;
            if gap_detected {
                println!("💤 Suspend gap detected ({}s) - closing lap at pre-gap timestamp",
                         iteration_ts - last_iteration_ts);
                handle_system_suspend_direct(&app_handle_clone, &state_clone, last_iteration_ts);
            }
            last_iteration_ts = iteration_ts;

            // Sleeps shorter than the gap threshold don't produce a detectable gap, but
            // the willSleep observer still closed the lap. Consume its marker once enough
            // wall clock has passed that the suspend really happened, so short sleeps go
            // through the same resync below.
            let sleep_notified_ts = SLEEP_NOTIFIED_AT.load(Ordering::Relaxed);
            let slept_via_notification =
                sleep_notified_ts != 0 && iteration_ts >= sleep_notified_ts + 2;
            if slept_via_notification {
                SLEEP_NOTIFIED_AT.store(0, Ordering::Relaxed);
            }

            if gap_detected || slept_via_notification {
                // Force the lock-state machine to "locked" so the normal unlock path
                // below starts a fresh lap — whether the Mac woke to a locked screen
                // (lap resumes at the real unlock) or an already-unlocked one (the
                // very next check sees "unlocked" and resumes immediately).
                last_screen_lock_state = true;
                lock_detection_count = 0;
                unlock_detection_count = 0;
            }

            // Check screen lock state
            match check_screen_lock_state_sync() {
                Ok(is_locked) => {
                    // Debounce: require 2 consecutive detections before changing state
                    if is_locked {
                        lock_detection_count += 1;
                        unlock_detection_count = 0;
                    } else {
                        unlock_detection_count += 1;
                        lock_detection_count = 0;
                    }
                    
                    // Only change state after 1 consecutive detection (less strict)
                    if is_locked && lock_detection_count >= 1 && !last_screen_lock_state {
                        // Screen just got locked - handle directly
                        println!("🔒 Screen lock detected!");
                        handle_screen_lock_direct(&app_handle_clone, &state_clone);
                        last_screen_lock_state = true;
                    } else if !is_locked && unlock_detection_count >= 1 && last_screen_lock_state {
                        // Screen just got unlocked - handle directly
                        println!("🔓 Screen unlock detected!");
                        handle_screen_unlock_direct(&app_handle_clone, &state_clone);
                        last_screen_lock_state = false;
                    }
                }
                Err(e) => eprintln!("Error checking screen lock state: {}", e),
            }

            // Poll once per second. Sub-second lock/sleep latency isn't needed for a time
            // tracker (a ~1s error at a lap boundary is negligible), and 1s halves the
            // subprocess spawns vs. the old 500ms.
            thread::sleep(Duration::from_millis(1000));
        }
    });
}

// Direct handlers that don't need State wrapper
fn handle_screen_lock_direct(app_handle: &AppHandle, state: &AppStateArc) {
    let mut session_guard = state.current_session.lock().unwrap();
    let mut records_guard = state.day_records.lock().unwrap();
    
    if let Some(session) = session_guard.as_mut() {
        // Skip if already paused (prevent duplicate events)
        if session.is_paused {
            return;
        }
        
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        
        // Get actual lap start time from records
        let lap_start_time = if let Some(day_record) = records_guard.get(&session.day_key) {
            get_active_lap_start_time(day_record)
                .unwrap_or(session.current_lap_start_timestamp)
        } else {
            session.current_lap_start_timestamp
        };
        
        let lap_duration = finalize_lap_duration(lap_start_time);
        
        // End current lap
        if let Some(day_record) = records_guard.get_mut(&session.day_key) {
            if let Some(last_lap) = day_record.laps.last_mut() {
                last_lap.end_time = Some(current_time);
                last_lap.duration = Some(lap_duration);
            }
        }
        
        // Mark as paused by system (not user)
        session.is_paused = true;
        session.user_paused = false; // System paused, not user
    }
    
    // Release locks before saving
    drop(session_guard);
    drop(records_guard);
    
    // Save state
    save_state(app_handle, state);
}

fn handle_screen_unlock_direct(app_handle: &AppHandle, state: &AppStateArc) {
    // The user is back. If they were away long enough (or it is past the cutoff) and the
    // date has changed, close out the previous day first — otherwise the lap we are about
    // to open would be filed under the day they started, which is how Saturday's work
    // ended up counted as Friday's. This must run before the lap is pushed below.
    roll_over_day_if_due(app_handle, state);

    let mut session_guard = state.current_session.lock().unwrap();
    let mut records_guard = state.day_records.lock().unwrap();

    if let Some(session) = session_guard.as_mut() {
        // Skip if already active (prevent duplicate events)
        if !session.is_paused {
            return;
        }
        
        // Only auto-start a new lap if user didn't manually pause
        // If user manually paused, respect their choice and don't auto-resume
        if !session.user_paused {
            let now = Instant::now();
            let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            
            // Start new lap
            if let Some(day_record) = records_guard.get_mut(&session.day_key) {
                day_record.laps.push(Lap {
                    start_time: current_time,
                    end_time: None,
                    duration: None,
                });
                
            }
            
            // Reset lap tracking and resume
            session.current_lap_start = now;
            session.current_lap_start_timestamp = current_time;
            session.accumulated_seconds = 0;
            session.last_activity_time = now;
            session.is_paused = false; // Resume active tracking
        }
    }
    
    // Release locks before saving
    drop(session_guard);
    drop(records_guard);
    
    // Save state
    save_state(app_handle, state);
}

// Close the currently-open lap because the machine is going (or went) to sleep.
// `end_ts` is when the lap should end: "now" when called from the NSWorkspace
// willSleep observer (delivered just before the process suspends), or the last
// pre-gap poll timestamp when called from the monitoring loop's gap detector
// after wake. It must never be the wake time — ending the lap at wake is exactly
// what counted a whole night's sleep as one giant active lap.
fn handle_system_suspend_direct(app_handle: &AppHandle, state: &AppStateArc, end_ts: u64) {
    let mut session_guard = state.current_session.lock().unwrap();
    let mut records_guard = state.day_records.lock().unwrap();

    let mut changed = false;
    if let Some(session) = session_guard.as_mut() {
        // Skip if already paused (locked before sleep, user pause, or the willSleep
        // observer already closed the lap and this is the gap detector re-firing).
        if !session.is_paused {
            if let Some(day_record) = records_guard.get_mut(&session.day_key) {
                if let Some(last_lap) = day_record.laps.last_mut() {
                    if last_lap.duration.is_none() {
                        // Guard against an end_ts that is somehow before the lap start.
                        let end = end_ts.max(last_lap.start_time);
                        last_lap.end_time = Some(end);
                        last_lap.duration = Some(end - last_lap.start_time);
                    }
                }
                day_record.total_duration = day_record.laps.iter()
                    .filter_map(|lap| lap.duration)
                    .sum();
            }

            // Mark session as paused by system (not user)
            session.is_paused = true;
            session.user_paused = false;
            changed = true;
        }
    }

    // Release locks before saving
    drop(session_guard);
    drop(records_guard);

    if changed {
        save_state(app_handle, state);
    }
}

fn check_screen_lock_state_sync() -> Result<bool, String> {
    // Use the macOS-specific detection method
    #[cfg(target_os = "macos")]
    {
        return check_macos_screen_lock_state();
    }
    
    // For non-macOS systems, return false
    #[cfg(not(target_os = "macos"))]
    Ok(false)
}

#[cfg(target_os = "macos")]
fn check_macos_screen_lock_state() -> Result<bool, String> {
    // Method 1: Use native Cocoa/Objective-C to check session state
    unsafe {
        let ws_class = class!(NSWorkspace);
        let shared_workspace: id = msg_send![ws_class, sharedWorkspace];
        
        // Check if screen is locked by looking at the active space
        // When locked, we won't be able to get active application
        let active_app: id = msg_send![shared_workspace, frontmostApplication];
        if active_app == nil {
            return Ok(true); // No frontmost app usually means locked
        }
        
        // Get the localized name of the frontmost application
        let app_name: id = msg_send![active_app, localizedName];
        if app_name != nil {
            let name_str: *const i8 = msg_send![app_name, UTF8String];
            if !name_str.is_null() {
                let name = std::ffi::CStr::from_ptr(name_str).to_string_lossy();
                // If loginwindow or ScreenSaverEngine is frontmost, screen is locked
                if name == "loginwindow" || name == "ScreenSaverEngine" {
                    return Ok(true);
                }
            }
        }
    }
    
    // Method 2: Check for screen saver process
    let screensaver_output = Command::new("pgrep")
        .arg("-x")
        .arg("ScreenSaverEngine")
        .output()
        .map_err(|e| e.to_string())?;
    
    if !screensaver_output.stdout.is_empty() {
        return Ok(true);
    }

    // Methods 1 (native NSWorkspace) and 2 (pgrep) above are sufficient to detect a
    // locked screen. We intentionally do NOT shell out to `osascript` here: this runs
    // on every poll while UNLOCKED (the common case), and an AppleScript spawn each
    // cycle was the single biggest source of idle CPU.
    Ok(false)
}

// Get system uptime in seconds (macOS)
#[cfg(target_os = "macos")]
fn get_system_uptime() -> Result<u64, String> {
    let output = Command::new("sysctl")
        .arg("-n")
        .arg("kern.boottime")
        .output()
        .map_err(|e| e.to_string())?;
    
    let output_str = String::from_utf8_lossy(&output.stdout);
    // Parse boot time from output like: { sec = 1234567890, usec = 0 }
    if let Some(sec_str) = output_str.split("sec = ").nth(1) {
        if let Some(boot_time_str) = sec_str.split(',').next() {
            if let Ok(boot_time) = boot_time_str.trim().parse::<u64>() {
                let current_time = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs();
                return Ok(current_time - boot_time);
            }
        }
    }
    
    Err("Failed to parse system uptime".to_string())
}

// Check if we should show startup notification
fn should_show_startup_notification(state: &AppStateArc) -> Result<bool, String> {
    // Check if system was recently started (within last 10 minutes)
    #[cfg(target_os = "macos")]
    {
        let uptime = get_system_uptime()?;
        if uptime > 600 {
            // System has been running for more than 10 minutes, not a fresh startup
            return Ok(false);
        }
    }
    
    let session_guard = state.current_session.lock().map_err(|e| e.to_string())?;

    // We now auto-start/continue a day on boot, so notify whenever a session exists
    // to let the user know tracking is running (and that they can pause if idle).
    Ok(session_guard.is_some())
}

// Show startup notification to user
fn show_startup_notification(app_handle: &AppHandle, state: &AppStateArc) {
    use tauri_plugin_notification::NotificationExt;

    println!("🔔 Showing startup notification to user");

    // Reflect whether we auto-started active tracking or restored a paused session.
    let is_paused = state
        .current_session
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|s| s.is_paused))
        .unwrap_or(false);

    let body = if is_paused {
        "Your day is restored but paused. Open the app to resume when you start working."
    } else {
        "Tracking started for today. Open the app to pause if you're not working yet."
    };

    let notification = app_handle
        .notification()
        .builder()
        .title("Screen Time Tracker")
        .body(body)
        .show();
    
    match notification {
        Ok(_) => println!("✅ Startup notification shown successfully"),
        Err(e) => eprintln!("❌ Failed to show notification: {}", e),
    }
}

// Check and notify on startup
fn check_and_notify_on_startup(app_handle: &AppHandle, state: &AppStateArc) {
    // Wait a bit for the system to settle after startup
    thread::sleep(Duration::from_secs(3));
    
    match should_show_startup_notification(state) {
        Ok(should_notify) => {
            if should_notify {
                show_startup_notification(app_handle, state);
            } else {
                println!("ℹ️ No need to show startup notification");
            }
        }
        Err(e) => {
            eprintln!("❌ Error checking startup notification: {}", e);
        }
    }
}

// ----------------------------------------------------------------------------
// Menu-bar (tray) app: the app lives in the macOS menu bar. Left-clicking the
// tray icon toggles a small popover (quick glance + basic controls); the popover
// can expand to the full window. Keeping the full webview closed by default is
// what keeps idle memory/CPU low — no webview process runs until you open one.
// ----------------------------------------------------------------------------

// Convert a tray icon rectangle into physical (x, y, width, height).
fn rect_to_physical(rect: &tauri::Rect) -> (f64, f64, f64, f64) {
    let (x, y) = match rect.position {
        tauri::Position::Physical(p) => (p.x as f64, p.y as f64),
        tauri::Position::Logical(p) => (p.x, p.y),
    };
    let (w, h) = match rect.size {
        tauri::Size::Physical(s) => (s.width as f64, s.height as f64),
        tauri::Size::Logical(s) => (s.width, s.height),
    };
    (x, y, w, h)
}

// Toggle the popover: close it if open, otherwise build a small borderless window
// anchored under the tray icon and show it. It closes itself when it loses focus.
fn toggle_popover(app: &AppHandle, rect: tauri::Rect) {
    if let Some(existing) = app.get_webview_window("popover") {
        let _ = existing.close();
        return;
    }

    let width = 288.0;
    let height = 214.0;

    let win = match WebviewWindowBuilder::new(app, "popover", WebviewUrl::App("popover.html".into()))
        .inner_size(width, height)
        .decorations(false)
        .always_on_top(true)
        .resizable(false)
        .skip_taskbar(true)
        .visible(false)
        .build()
    {
        Ok(w) => w,
        Err(e) => {
            eprintln!("❌ Failed to build popover window: {}", e);
            return;
        }
    };

    // Anchor the popover horizontally centered under the tray icon.
    let scale = win.scale_factor().unwrap_or(1.0);
    let (ix, iy, iw, ih) = rect_to_physical(&rect);
    let px = (ix + iw / 2.0 - (width * scale) / 2.0).max(0.0);
    let py = iy + ih + 2.0;
    let _ = win.set_position(tauri::PhysicalPosition::new(px, py));
    let _ = win.show();
    let _ = win.set_focus();

    // Dismiss when the user clicks away (popover loses focus).
    let win_for_event = win.clone();
    win.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            let _ = win_for_event.close();
        }
    });
}

// Show (or create) the full UI window and close the popover. The full window is
// created on demand so no webview exists while the app sits in the menu bar.
fn open_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    } else {
        match WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
            .title("Screen Time Tracker")
            .inner_size(1000.0, 800.0)
            .min_inner_size(800.0, 600.0)
            .resizable(true)
            .build()
        {
            Ok(_) => println!("✅ Opened full window"),
            Err(e) => eprintln!("❌ Failed to build main window: {}", e),
        }
    }

    if let Some(p) = app.get_webview_window("popover") {
        let _ = p.close();
    }
}

// Expand from the popover to the full window.
#[tauri::command]
async fn show_main_window(app: AppHandle) -> Result<(), String> {
    open_main_window(&app);
    Ok(())
}

// Tauri command to start day from notification
#[tauri::command]
async fn start_day_from_notification(state: State<'_, AppStateArc>) -> Result<String, String> {
    // Check if already has an active session
    let should_add_lap = {
        let session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
        
        if let Some(session) = session_guard.as_ref() {
            if session.is_paused {
                // We have a paused session, just add a new lap to resume
                true
            } else {
                // Already actively tracking
                return Ok("Already tracking".to_string());
            }
        } else {
            // No session, start a new day
            false
        }
    }; // Drop the lock here before awaiting
    
    if should_add_lap {
        add_lap(state).await
    } else {
        start_day(state).await
    }
}


// The autostart plugin writes a LaunchAgent containing only RunAtLoad, which fires solely
// at login. If the tracker then dies mid-session — crashed, force-quit, or killed because
// its bundle was replaced by a rebuild — launchd will not bring it back until the *next*
// login, and a Mac that is only ever slept can go days without one. That is how three days
// of screen time went unrecorded: the app exited on Jul 11 and nothing restarted it while
// the machine stayed up for 3+ days.
//
// KeepAlive/SuccessfulExit=false makes launchd relaunch the job whenever it exits
// abnormally (non-zero status or killed by a signal), while still honouring a clean exit:
// the tray's Quit calls process::exit(0), so quitting on purpose still quits for good.
//
// The plist is patched rather than rewritten so the plugin stays the single source of
// truth for the executable path (which differs between a dev run and the installed .app).
#[cfg(target_os = "macos")]
fn ensure_autostart_keepalive() {
    const KEEP_ALIVE: &str = "\t<key>KeepAlive</key>\n\t<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>\n";

    let Ok(home) = std::env::var("HOME") else { return };
    let plist = PathBuf::from(home).join("Library/LaunchAgents/screen-time-tracker.plist");

    let Ok(contents) = fs::read_to_string(&plist) else { return };
    if contents.contains("KeepAlive") {
        return;
    }
    // Insert into the job dict, i.e. just before the last closing </dict>.
    let Some(insert_at) = contents.rfind("</dict>") else { return };

    let mut patched = String::with_capacity(contents.len() + KEEP_ALIVE.len());
    patched.push_str(&contents[..insert_at]);
    patched.push_str(KEEP_ALIVE);
    patched.push_str(&contents[insert_at..]);

    match fs::write(&plist, patched) {
        Ok(_) => println!("✅ Autostart KeepAlive set (relaunch if the app dies)"),
        Err(e) => eprintln!("❌ Failed to patch autostart plist: {}", e),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = Arc::new(AppState::new());
    
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(app_state.clone())
        .invoke_handler(tauri::generate_handler![
            start_day,
            end_day,
            handle_screen_lock,
            handle_screen_unlock,
            get_current_status,
            get_current_day_laps,
            get_all_day_records,
            merge_day_into_previous,
            add_lap,
            stop_lap,
            check_screen_lock_state,
            test_screen_lock_detection,
            handle_system_sleep,
            handle_system_wake,
            handle_user_logout,
            handle_user_login,
            start_day_from_notification,
            show_main_window
        ])
        .setup(move |app| {
            let app_handle = app.handle().clone();

            // Run as a menu-bar (accessory) app: no Dock icon, lives in the menu bar.
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Build the menu-bar tray icon. Left click toggles the popover; a right-click
            // menu provides Open (full window) and Quit.
            {
                let open_item = MenuItemBuilder::with_id("open", "Open Screen Time Tracker").build(app)?;
                let quit_item = MenuItemBuilder::with_id("quit", "Quit").build(app)?;
                let tray_menu = MenuBuilder::new(app).items(&[&open_item, &quit_item]).build()?;

                let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png"))
                    .expect("failed to load tray icon");

                TrayIconBuilder::with_id("tray")
                    .icon(tray_icon)
                    .tooltip("Screen Time Tracker")
                    .menu(&tray_menu)
                    .show_menu_on_left_click(false)
                    .on_menu_event(|app, event| match event.id().as_ref() {
                        "open" => open_main_window(app),
                        "quit" => {
                            // The `ExitRequested` handler in `run()` calls `prevent_exit()`
                            // to keep the app alive when its windows close — but that also
                            // swallows a normal `app.exit()`, so Quit never actually quit.
                            // Persist state, then force the process down directly.
                            let state = app.state::<AppStateArc>();
                            save_state(app, state.inner());
                            std::process::exit(0);
                        }
                        _ => {}
                    })
                    .on_tray_icon_event(|tray, event| {
                        if let TrayIconEvent::Click { button, button_state, rect, .. } = event {
                            if button == MouseButton::Left && button_state == MouseButtonState::Up {
                                toggle_popover(tray.app_handle(), rect);
                            }
                        }
                    })
                    .build(app)?;
            }

            // Ensure the app launches automatically when the user logs into the machine.
            {
                use tauri_plugin_autostart::ManagerExt;
                let autostart_manager = app.autolaunch();
                match autostart_manager.enable() {
                    Ok(_) => println!("✅ Autostart enabled (launch on login)"),
                    Err(e) => eprintln!("❌ Failed to enable autostart: {}", e),
                }
                // enable() rewrites the plist from scratch every launch, so the KeepAlive
                // patch has to be re-applied after it, not once at install time.
                #[cfg(target_os = "macos")]
                ensure_autostart_keepalive();
            }

            // Load saved state from disk and decide today's session (auto-start / continue / end).
            load_and_initialize(&app_handle, &app_state);
            
            // Check if we should show startup notification (after system restart)
            let state_for_notification = app_state.clone();
            let handle_for_notification = app_handle.clone();
            thread::spawn(move || {
                check_and_notify_on_startup(&handle_for_notification, &state_for_notification);
            });
            
            // Start system monitoring when app starts. This single thread handles
            // screen lock/unlock polling plus suspend-gap detection (previously a second,
            // redundant macOS-only lock-detection thread ran in parallel — removed to cut
            // idle CPU, as was a per-second `pmset` sleep check that could never fire).
            start_system_monitoring(app_handle.clone(), app_state.clone());

            // Close the open lap at the exact moment the machine sleeps; the gap
            // detector inside the monitoring loop is the fallback if this is missed.
            #[cfg(target_os = "macos")]
            register_sleep_observer(app_handle.clone(), app_state.clone());

            // Start periodic state saving (every 30 seconds)
            let state_for_autosave = app_state.clone();
            let handle_for_autosave = app_handle.clone();
            thread::spawn(move || {
                loop {
                    thread::sleep(Duration::from_secs(30));
                    // Catches the day change for a session that never pauses — nobody
                    // locks the screen or sleeps the Mac, so no unlock event ever fires
                    // and the cutoff backstop has to be evaluated on a timer. Rolls over
                    // and saves; otherwise this is just the periodic save.
                    roll_over_day_if_due(&handle_for_autosave, &state_for_autosave);
                    save_state(&handle_for_autosave, &state_for_autosave);
                }
            });
            
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app_handle, event| {
            // The app is a menu-bar app: keep it running even when all windows are
            // closed. It only quits via the tray's Quit item (app.exit).
            if let RunEvent::ExitRequested { api, .. } = event {
                api.prevent_exit();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 3600;

    fn lap(start: u64, end: Option<u64>) -> Lap {
        Lap { start_time: start, end_time: end, duration: end.map(|e| e - start) }
    }

    fn day(date: &str, laps: Vec<Lap>) -> DayRecord {
        let total = laps.iter().filter_map(|l| l.duration).sum();
        DayRecord { date: date.to_string(), total_duration: total, laps, is_active: false }
    }

    // --- rollover decision -------------------------------------------------
    // `gap` is 0 whenever a lap is open, so "working through" is expressed as gap == 0.

    #[test]
    fn night_owl_past_midnight_stays_on_the_same_day() {
        // Friday 21:00 -> Saturday 02:00, still typing. The date changed, but there is no
        // break and it is nowhere near the cutoff. This is the case that must NOT roll.
        assert!(!should_roll_over(true, 0, false));
    }

    #[test]
    fn short_break_after_midnight_stays_on_the_same_day() {
        // Stepped away for 30 minutes at 01:00. Still Friday's session.
        assert!(!should_roll_over(true, 30 * 60, false));
    }

    #[test]
    fn slept_overnight_starts_a_new_day() {
        // Machine asleep from Friday 02:00, back at Saturday 10:00: an 8h gap.
        assert!(should_roll_over(true, 8 * HOUR, true));
        // ...and it would roll on the gap alone, even before the cutoff hour.
        assert!(should_roll_over(true, 8 * HOUR, false));
    }

    #[test]
    fn six_hour_gap_is_the_boundary() {
        assert!(!should_roll_over(true, 6 * HOUR - 1, false));
        assert!(should_roll_over(true, 6 * HOUR, false));
    }

    #[test]
    fn cutoff_rolls_over_a_session_that_never_paused() {
        // Worked straight through the night with no break at all: gap is 0 forever, so
        // only the cutoff can end the day. Without this the day_key never rolls, which is
        // how a Saturday afternoon lap got filed under Friday.
        assert!(should_roll_over(true, 0, true));
    }

    #[test]
    fn same_date_never_rolls_over() {
        // A long idle stretch within one day (a 7h meeting-free afternoon away from the
        // desk) must not start a second record for the same date.
        assert!(!should_roll_over(false, 9 * HOUR, false));
        assert!(!should_roll_over(false, 0, true));
    }

    // --- where the day is cut ------------------------------------------------

    #[test]
    fn worked_through_the_night_is_cut_at_the_cutoff_not_at_wake() {
        // Open lap running since last night; it is now 09:00 and the cutoff was 06:00.
        // The old day must end at 06:00, so the pre-dawn hours stay with it and only
        // 06:00->09:00 counts as today. Cutting at `now` would hand today three hours of
        // last night's work.
        let cutoff = 6 * HOUR;
        let now = 9 * HOUR;
        assert_eq!(rollover_boundary(true, cutoff, now), cutoff);
    }

    #[test]
    fn returning_from_a_break_cuts_at_now() {
        // No open lap: the user stopped last night and just came back. The old day ended
        // when they stopped, and today starts now.
        let cutoff = 6 * HOUR;
        let now = 10 * HOUR;
        assert_eq!(rollover_boundary(false, cutoff, now), now);
    }

    #[test]
    fn boundary_never_runs_ahead_of_the_clock() {
        // Guards the case where the cutoff has not been reached yet: the boundary must
        // never be a future timestamp, or a lap would be given a negative duration.
        let cutoff = 6 * HOUR;
        let now = 2 * HOUR;
        assert_eq!(rollover_boundary(true, cutoff, now), now);
    }

    // --- merge (undo a rollover) -------------------------------------------

    #[test]
    fn merge_folds_a_day_into_the_previous_one() {
        let mut records = HashMap::new();
        records.insert("2026-07-10".into(), day("2026-07-10", vec![lap(100, Some(400))]));
        records.insert("2026-07-11".into(), day("2026-07-11", vec![lap(500, Some(700))]));

        let target = merge_records_into_previous(&mut records, "2026-07-11").unwrap();

        assert_eq!(target, "2026-07-10");
        assert!(!records.contains_key("2026-07-11"), "merged day should stop existing");
        let merged = &records["2026-07-10"];
        assert_eq!(merged.laps.len(), 2);
        assert_eq!(merged.total_duration, 300 + 200);
        // Laps must come back in chronological order, not append order.
        assert!(merged.laps[0].start_time < merged.laps[1].start_time);
    }

    #[test]
    fn merge_skips_over_days_with_no_record() {
        // The machine was off on the 12th, so the 13th merges into the 11th.
        let mut records = HashMap::new();
        records.insert("2026-07-11".into(), day("2026-07-11", vec![lap(100, Some(200))]));
        records.insert("2026-07-13".into(), day("2026-07-13", vec![lap(900, Some(950))]));

        let target = merge_records_into_previous(&mut records, "2026-07-13").unwrap();
        assert_eq!(target, "2026-07-11");
        assert_eq!(records["2026-07-11"].laps.len(), 2);
    }

    #[test]
    fn merge_carries_the_open_lap_and_active_flag() {
        // Merging today (still being tracked) back into yesterday must keep the day alive
        // and leave the open lap open, or the running session would be orphaned.
        let mut records = HashMap::new();
        records.insert("2026-07-13".into(), day("2026-07-13", vec![lap(100, Some(400))]));
        let mut today = day("2026-07-14", vec![lap(500, None)]);
        today.is_active = true;
        records.insert("2026-07-14".into(), today);

        let target = merge_records_into_previous(&mut records, "2026-07-14").unwrap();

        let merged = &records[&target];
        assert!(merged.is_active, "absorbing day must stay active");
        assert!(merged.laps.iter().any(|l| l.duration.is_none()), "open lap must survive");
        // The open lap contributes nothing to the total until it closes.
        assert_eq!(merged.total_duration, 300);
    }

    #[test]
    fn merge_refuses_when_there_is_no_earlier_day() {
        let mut records = HashMap::new();
        records.insert("2026-07-04".into(), day("2026-07-04", vec![lap(100, Some(200))]));
        assert!(merge_records_into_previous(&mut records, "2026-07-04").is_err());
        // The record must survive a refused merge.
        assert!(records.contains_key("2026-07-04"));
    }
}
