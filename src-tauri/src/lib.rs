use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tauri::{State, AppHandle, Manager};
use std::process::Command;
use std::thread;
use std::time::Duration;
use std::fs;
use std::path::PathBuf;
use std::ffi::CStr;

#[cfg(target_os = "macos")]
use cocoa::base::{id, nil};
#[cfg(target_os = "macos")]
use cocoa::foundation::NSString;
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersistedState {
    current_session: Option<PersistedSessionState>,
    day_records: HashMap<String, DayRecord>,
}

// Get the path to the state file
fn get_state_file_path(app_handle: &AppHandle) -> PathBuf {
    let app_data_dir = app_handle.path().app_data_dir().unwrap();
    fs::create_dir_all(&app_data_dir).ok();
    app_data_dir.join("state.json")
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
        }
    });
    
    let persisted_state = PersistedState {
        current_session: persisted_session,
        day_records: records_guard.clone(),
    };
    
    let state_file = get_state_file_path(app_handle);
    if let Ok(json) = serde_json::to_string_pretty(&persisted_state) {
        fs::write(state_file, json).ok();
        println!("✅ State saved successfully");
    }
}

// Load state from disk
fn load_state(app_handle: &AppHandle, state: &AppStateArc) {
    let state_file = get_state_file_path(app_handle);
    
    if let Ok(json) = fs::read_to_string(&state_file) {
        if let Ok(persisted_state) = serde_json::from_str::<PersistedState>(&json) {
            // Restore day records
            let mut records_guard = state.day_records.lock().unwrap();
            *records_guard = persisted_state.day_records;
            
            // Restore session if it exists
            if let Some(persisted_session) = persisted_state.current_session {
                let now = Instant::now();
                let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
                
                // Check if the session is from today
                let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
                if persisted_session.day_key == today {
                    let mut session_guard = state.current_session.lock().unwrap();
                    *session_guard = Some(CurrentSession {
                        start_time: now,
                        day_key: persisted_session.day_key.clone(),
                        current_lap_start: now,
                        current_lap_start_timestamp: current_time,
                        accumulated_seconds: persisted_session.accumulated_seconds,
                        last_activity_time: now,
                        is_paused: true, // Always start as paused after restart
                        user_paused: false, // System paused (restart), not user paused
                    });
                    
                    println!("✅ Session restored from previous state (marked as paused)");
                } else {
                    println!("⚠️ Previous session was from a different day, not restoring");
                }
            }
            
            println!("✅ State loaded successfully");
        }
    }
}

// Helper function to calculate current lap duration excluding sleep/hibernate time
fn get_current_lap_duration(session: &mut CurrentSession) -> u64 {
    if session.is_paused {
        return session.accumulated_seconds;
    }
    
    let now = Instant::now();
    let time_since_last_activity = now.duration_since(session.last_activity_time).as_secs();
    
    // If more than 5 seconds have passed since last activity check, 
    // system might have been asleep/locked - don't count that time
    let gap_threshold = 5;
    
    if time_since_last_activity > gap_threshold {
        // Large gap detected - system was likely asleep/locked
        // Don't add this gap time, just update the reference point
        session.last_activity_time = now;
        return session.accumulated_seconds;
    }
    
    // Normal case: add the time since last activity to accumulated seconds
    session.accumulated_seconds += time_since_last_activity;
    session.last_activity_time = now;
    
    session.accumulated_seconds
}

#[tauri::command]
async fn start_day(state: State<'_, AppStateArc>) -> Result<String, String> {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    
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
    
    // Initialize or update day record with the first lap
    let day_record = DayRecord {
        date: today.clone(),
        total_duration: 0,
        laps: vec![Lap {
            start_time: current_time,
            end_time: None,
            duration: None,
        }],
        is_active: true,
    };
    
    records_guard.insert(today.clone(), day_record);
    
    Ok(format!("Started tracking for {}", today))
}

#[tauri::command]
async fn end_day(state: State<'_, AppStateArc>, app_handle: AppHandle) -> Result<DayRecord, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    let mut session = session_guard.take().ok_or("No active session")?;
    let day_key = session.day_key.clone();
    
    // Calculate final duration for current lap (excluding sleep/hibernate time)
    let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let lap_duration = get_current_lap_duration(&mut session);
    
    let result = if let Some(day_record) = records_guard.get_mut(&day_key) {
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
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let lap_duration = get_current_lap_duration(session);
        
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
                total_session_duration: total_duration, // Only completed laps
                is_active: false, // Not actively tracking
            }))
        } else {
            // Session is active - include current lap time (excluding sleep/hibernate)
            let current_lap_seconds = get_current_lap_duration(session);
            let total_with_current_lap = total_duration + current_lap_seconds;
            
            Ok(Some(CurrentStatus {
                day_key: session.day_key.clone(),
                current_lap_duration: current_lap_seconds,
                total_session_duration: total_with_current_lap,
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

#[tauri::command]
async fn add_lap(state: State<'_, AppStateArc>) -> Result<String, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_mut() {
        let now = Instant::now();
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let lap_duration = get_current_lap_duration(session);
        
        // End current lap only if it has been running for more than 1 second
        if let Some(day_record) = records_guard.get_mut(&session.day_key) {
            if let Some(last_lap) = day_record.laps.last_mut() {
                if lap_duration > 1 {
                    last_lap.end_time = Some(current_time);
                    last_lap.duration = Some(lap_duration);
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
        let lap_duration = get_current_lap_duration(session);
        
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
                        
                        println!("🗑️ Very short lap ({}s) removed - session paused by user", lap_duration);
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
                    println!("⏸️ Lap stopped ({}s) - session paused", lap_duration);
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
        let lap_duration = get_current_lap_duration(session);
        
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
        let lap_duration = get_current_lap_duration(session);
        
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

// System monitoring functions
fn start_system_monitoring(app_handle: AppHandle, state: AppStateArc) {
    let state_clone = state.clone();
    let app_handle_clone = app_handle.clone();
    
    thread::spawn(move || {
        let mut last_screen_lock_state = false;
        let mut last_sleep_state = false;
        let mut lock_detection_count = 0;
        let mut unlock_detection_count = 0;
        
        loop {
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
            
            // Check for system sleep/wake events
            match check_system_sleep_state() {
                Ok(is_sleeping) => {
                    if is_sleeping && !last_sleep_state {
                        // System just went to sleep - handle directly
                        handle_system_sleep_direct(&app_handle_clone, &state_clone);
                    } else if !is_sleeping && last_sleep_state {
                        // System just woke up - handle directly
                        handle_system_wake_direct(&app_handle_clone, &state_clone);
                    }
                    last_sleep_state = is_sleeping;
                }
                Err(e) => eprintln!("Error checking system sleep state: {}", e),
            }
            
            thread::sleep(Duration::from_millis(500)); // Check every 500ms for more responsive detection
        }
    });
}

// Direct handlers that don't need State wrapper
fn handle_screen_lock_direct(app_handle: &AppHandle, state: &AppStateArc) {
    let mut session_guard = state.current_session.lock().unwrap();
    let mut records_guard = state.day_records.lock().unwrap();
    
    if let Some(session) = session_guard.as_mut() {
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let lap_duration = get_current_lap_duration(session);
        
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
        
        println!("🔒 Screen locked - lap paused (duration: {}s)", lap_duration);
    }
    
    // Release locks before saving
    drop(session_guard);
    drop(records_guard);
    
    // Save state
    save_state(app_handle, state);
}

fn handle_screen_unlock_direct(app_handle: &AppHandle, state: &AppStateArc) {
    let mut session_guard = state.current_session.lock().unwrap();
    let mut records_guard = state.day_records.lock().unwrap();
    
    if let Some(session) = session_guard.as_mut() {
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
            
            println!("🔓 Screen unlocked - new lap auto-started");
        } else {
            println!("🔓 Screen unlocked - session remains paused (user paused manually)");
        }
    }
    
    // Release locks before saving
    drop(session_guard);
    drop(records_guard);
    
    // Save state
    save_state(app_handle, state);
}

fn handle_system_sleep_direct(app_handle: &AppHandle, state: &AppStateArc) {
    let mut session_guard = state.current_session.lock().unwrap();
    let mut records_guard = state.day_records.lock().unwrap();
    
    if let Some(session) = session_guard.as_mut() {
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let lap_duration = get_current_lap_duration(session);
        
        // End current lap
        if let Some(day_record) = records_guard.get_mut(&session.day_key) {
            if let Some(last_lap) = day_record.laps.last_mut() {
                last_lap.end_time = Some(current_time);
                last_lap.duration = Some(lap_duration);
            }
        }
        
        // Mark session as paused by system (not user)
        session.is_paused = true;
        session.user_paused = false; // System paused, not user
        
        println!("💤 System sleep detected - lap paused (duration: {}s)", lap_duration);
    }
    
    // Release locks before saving
    drop(session_guard);
    drop(records_guard);
    
    // Save state
    save_state(app_handle, state);
}

fn handle_system_wake_direct(app_handle: &AppHandle, state: &AppStateArc) {
    let mut session_guard = state.current_session.lock().unwrap();
    let mut records_guard = state.day_records.lock().unwrap();
    
    if let Some(session) = session_guard.as_mut() {
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
            
            // Reset lap tracking and resume
            session.current_lap_start = now;
            session.current_lap_start_timestamp = current_time;
            session.accumulated_seconds = 0;
            session.last_activity_time = now;
            session.is_paused = false; // Resume the session
            
            println!("⏰ System wake detected - new lap auto-started");
        } else {
            println!("⏰ System wake - session remains paused (user paused manually)");
        }
    }
    
    // Release locks before saving
    drop(session_guard);
    drop(records_guard);
    
    // Save state
    save_state(app_handle, state);
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
fn start_macos_screen_lock_monitoring(app_handle: AppHandle, state: AppStateArc) {
    thread::spawn(move || {
        let mut last_screen_lock_state = false;
        let mut lock_detection_count = 0;
        let mut unlock_detection_count = 0;
        
        loop {
            // Check for screen lock using a more reliable method
            match check_macos_screen_lock_state() {
                Ok(is_locked) => {
                    // Debounce: require 2 consecutive detections before changing state
                    if is_locked {
                        lock_detection_count += 1;
                        unlock_detection_count = 0;
                    } else {
                        unlock_detection_count += 1;
                        lock_detection_count = 0;
                    }
                    
                    // Only change state after 2 consecutive detections
                    if is_locked && lock_detection_count >= 2 && !last_screen_lock_state {
                        println!("🔒 macOS Screen lock detected!");
                        handle_screen_lock_direct(&app_handle, &state);
                        last_screen_lock_state = true;
                    } else if !is_locked && unlock_detection_count >= 2 && last_screen_lock_state {
                        println!("🔓 macOS Screen unlock detected!");
                        handle_screen_unlock_direct(&app_handle, &state);
                        last_screen_lock_state = false;
                    }
                }
                Err(e) => {
                    eprintln!("❌ Error checking screen lock state: {}", e);
                }
            }
            
            thread::sleep(Duration::from_millis(500));
        }
    });
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
    
    // Method 3: Use AppleScript to check frontmost process (more reliable)
    let script_output = Command::new("osascript")
        .arg("-e")
        .arg("tell application \"System Events\" to get name of first process whose frontmost is true")
        .output()
        .map_err(|e| e.to_string())?;
    
    if script_output.status.success() {
        let frontmost = String::from_utf8_lossy(&script_output.stdout);
        let frontmost_trimmed = frontmost.trim();
        if frontmost_trimmed == "loginwindow" || frontmost_trimmed == "ScreenSaverEngine" {
            return Ok(true);
        }
    }
    
    Ok(false)
}

fn check_system_sleep_state() -> Result<bool, String> {
    // Check if system is in sleep mode by looking at power management
    let output = Command::new("pmset")
        .arg("-g")
        .arg("ps")
        .output()
        .map_err(|e| e.to_string())?;
    
    let output_str = String::from_utf8_lossy(&output.stdout);
    // If we can get power state info, system is awake
    // If command fails or returns empty, system might be sleeping
    Ok(output_str.is_empty() || output_str.contains("sleep"))
}


#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = Arc::new(AppState::new());
    
    tauri::Builder::default()
        .manage(app_state.clone())
        .invoke_handler(tauri::generate_handler![
            start_day,
            end_day,
            handle_screen_lock,
            handle_screen_unlock,
            get_current_status,
            get_current_day_laps,
            add_lap,
            stop_lap,
            check_screen_lock_state,
            test_screen_lock_detection,
            handle_system_sleep,
            handle_system_wake,
            handle_user_logout,
            handle_user_login
        ])
        .setup(move |app| {
            let app_handle = app.handle().clone();
            
            // Load saved state from disk
            load_state(&app_handle, &app_state);
            
            // Start system monitoring when app starts
            start_system_monitoring(app_handle.clone(), app_state.clone());
            
            // Start macOS-specific screen lock monitoring
            #[cfg(target_os = "macos")]
            start_macos_screen_lock_monitoring(app_handle.clone(), app_state.clone());
            
            // Start periodic state saving (every 30 seconds)
            let state_for_autosave = app_state.clone();
            let handle_for_autosave = app_handle.clone();
            thread::spawn(move || {
                loop {
                    thread::sleep(Duration::from_secs(30));
                    save_state(&handle_for_autosave, &state_for_autosave);
                }
            });
            
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
