use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tauri::State;
use std::process::Command;

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

pub struct CurrentSession {
    pub start_time: Instant,
    pub day_key: String,
    pub current_lap_start: Instant,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            current_session: Arc::new(Mutex::new(None)),
            day_records: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

#[tauri::command]
async fn start_day(state: State<'_, AppState>) -> Result<String, String> {
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
async fn end_day(state: State<'_, AppState>) -> Result<DayRecord, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    let session = session_guard.take().ok_or("No active session")?;
    let day_key = session.day_key.clone();
    
    // Calculate final duration for current lap
    let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let lap_duration = session.current_lap_start.elapsed().as_secs();
    
    if let Some(day_record) = records_guard.get_mut(&day_key) {
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
    }
}

#[tauri::command]
async fn handle_screen_lock(state: State<'_, AppState>) -> Result<String, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_mut() {
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let lap_duration = session.current_lap_start.elapsed().as_secs();
        
        // End current lap
        if let Some(day_record) = records_guard.get_mut(&session.day_key) {
            if let Some(last_lap) = day_record.laps.last_mut() {
                last_lap.end_time = Some(current_time);
                last_lap.duration = Some(lap_duration);
            }
        }
        
        Ok("Screen locked - timer paused".to_string())
    } else {
        Ok("No active session".to_string())
    }
}

#[tauri::command]
async fn handle_screen_unlock(state: State<'_, AppState>) -> Result<String, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_mut() {
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        
        // Start new lap
        if let Some(day_record) = records_guard.get_mut(&session.day_key) {
            day_record.laps.push(Lap {
                start_time: current_time,
                end_time: None,
                duration: None,
            });
        }
        
        session.current_lap_start = Instant::now();
        
        Ok("Screen unlocked - new lap started".to_string())
    } else {
        Ok("No active session".to_string())
    }
}

#[tauri::command]
async fn get_current_status(state: State<'_, AppState>) -> Result<Option<CurrentStatus>, String> {
    let session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_ref() {
        let current_lap_elapsed = session.current_lap_start.elapsed();
        
        // Calculate total duration from completed laps
        let mut total_duration = 0u64;
        if let Some(day_record) = records_guard.get(&session.day_key) {
            total_duration = day_record.laps.iter()
                .filter_map(|lap| lap.duration)
                .sum();
        }
        
        Ok(Some(CurrentStatus {
            day_key: session.day_key.clone(),
            current_lap_duration: current_lap_elapsed.as_secs(),
            total_session_duration: total_duration,
            is_active: true,
        }))
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
async fn get_current_day_laps(state: State<'_, AppState>) -> Result<Vec<Lap>, String> {
    let session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_ref() {
        if let Some(day_record) = records_guard.get(&session.day_key) {
            println!("Returning {} laps for day {}", day_record.laps.len(), session.day_key);
            for (i, lap) in day_record.laps.iter().enumerate() {
                println!("Lap {}: start_time={}, end_time={:?}, duration={:?}", 
                    i, lap.start_time, lap.end_time, lap.duration);
            }
            Ok(day_record.laps.clone())
        } else {
            println!("No day record found for {}", session.day_key);
            Ok(Vec::new())
        }
    } else {
        println!("No active session");
        Ok(Vec::new())
    }
}

#[tauri::command]
async fn add_lap(state: State<'_, AppState>) -> Result<String, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_mut() {
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let lap_duration = session.current_lap_start.elapsed().as_secs();
        
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
        
        // Reset current lap start time
        session.current_lap_start = Instant::now();
        
        Ok("New lap added successfully".to_string())
    } else {
        Err("No active session".to_string())
    }
}

#[tauri::command]
async fn stop_lap(state: State<'_, AppState>) -> Result<String, String> {
    let mut session_guard = state.current_session.lock().map_err(|e| e.to_string())?;
    let mut records_guard = state.day_records.lock().map_err(|e| e.to_string())?;
    
    if let Some(session) = session_guard.as_mut() {
        let current_time = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let lap_duration = session.current_lap_start.elapsed().as_secs();
        
        // End current lap
        if let Some(day_record) = records_guard.get_mut(&session.day_key) {
            if let Some(last_lap) = day_record.laps.last_mut() {
                last_lap.end_time = Some(current_time);
                last_lap.duration = Some(lap_duration);
            }
        }
        
        // Reset current lap start time (for when user resumes)
        session.current_lap_start = Instant::now();
        
        Ok("Lap stopped successfully".to_string())
    } else {
        Err("No active session".to_string())
    }
}



#[tauri::command]
async fn check_screen_lock_state() -> Result<bool, String> {
    // Simple check for screen saver or login window
    let output = Command::new("sh")
        .arg("-c")
        .arg("ps aux | grep -i 'ScreenSaverEngine\\|loginwindow' | grep -v grep")
        .output()
        .map_err(|e| e.to_string())?;
    
    Ok(!output.stdout.is_empty())
}


#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app_state = AppState::new();
    
    tauri::Builder::default()
        .manage(app_state)
        .invoke_handler(tauri::generate_handler![
            start_day,
            end_day,
            handle_screen_lock,
            handle_screen_unlock,
            get_current_status,
            get_current_day_laps,
            add_lap,
            stop_lap,
            check_screen_lock_state
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
