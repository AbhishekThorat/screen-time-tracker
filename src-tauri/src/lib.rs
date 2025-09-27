use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tauri::{State, Window, Emitter};
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
    let session = CurrentSession {
        start_time: now,
        day_key: today.clone(),
        current_lap_start: now,
    };
    
    *session_guard = Some(session);
    
    // Initialize or update day record
    let day_record = DayRecord {
        date: today.clone(),
        total_duration: 0,
        laps: vec![Lap {
            start_time: SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs(),
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
            Ok(day_record.laps.clone())
        } else {
            Ok(Vec::new())
        }
    } else {
        Ok(Vec::new())
    }
}



#[tauri::command]
async fn check_screen_lock_state() -> Result<bool, String> {
  Ok(check_screen_lock_state_sync())
}

#[tauri::command]
async fn get_display_state() -> Result<bool, String> {
  // Use ioreg to check if display is connected and active
  let output = Command::new("sh")
    .arg("-c")
    .arg("ioreg -n IODisplayWrangler | grep -i 'CurrentPowerState' | head -1")
    .output()
    .map_err(|e| e.to_string())?;
  
  let output_str = String::from_utf8_lossy(&output.stdout);
  // If CurrentPowerState is 4, display is on; if 0, display is off
  Ok(output_str.contains("4"))
}


#[tauri::command]
async fn start_screen_lock_monitoring(window: Window) -> Result<(), String> {
  let window_clone = window.clone();
  
  // Start a background task to monitor screen lock and sleep state
  std::thread::spawn(move || {
    let mut last_lock_state = false;
    
    loop {
      std::thread::sleep(std::time::Duration::from_secs(1)); // Check more frequently
      
      // Use more reliable methods for screen lock detection
      let is_locked = check_screen_lock_state_sync();
      
      if is_locked != last_lock_state {
        println!("Screen lock state changed: locked={}", is_locked);
        last_lock_state = is_locked;
        
        if is_locked {
          // Screen was locked or machine went to sleep
          println!("Emitting screen-locked event");
          let _ = window_clone.emit("screen-locked", ()).ok();
        } else {
          // Screen was unlocked or machine woke up
          println!("Emitting screen-unlocked event");
          let _ = window_clone.emit("screen-unlocked", ()).ok();
        }
      }
    }
  });
  
  // Also start a log stream monitor for more immediate detection
  let window_clone2 = window.clone();
  std::thread::spawn(move || {
    let mut last_log_lock_state = false;
    
    // Monitor log stream for lock/unlock events
    let log_process = Command::new("log")
      .args(&["stream", "--predicate", "eventMessage CONTAINS \"loginwindow\"", "--style", "compact"])
      .stdout(std::process::Stdio::piped())
      .spawn();
    
    if let Ok(mut child) = log_process {
      use std::io::{BufRead, BufReader};
      
      if let Some(stdout) = child.stdout.take() {
        let reader = BufReader::new(stdout);
        
        for line in reader.lines() {
          if let Ok(line) = line {
            let is_lock_event = line.contains("loginwindow") && 
                               (line.contains("lock") || line.contains("unlock") || line.contains("sleep") || line.contains("wake"));
            
            if is_lock_event {
              let is_locked = line.contains("lock") || line.contains("sleep");
              
              if is_locked != last_log_lock_state {
                println!("Log stream detected lock state change: locked={}", is_locked);
                last_log_lock_state = is_locked;
                
                if is_locked {
                  let _ = window_clone2.emit("screen-locked", ()).ok();
                } else {
                  let _ = window_clone2.emit("screen-unlocked", ()).ok();
                }
              }
            }
          }
        }
      }
    }
  });
  
  Ok(())
}

fn check_screen_lock_state_sync() -> bool {
  // Method 1: Check for ScreenSaverEngine (traditional screen saver)
  let screen_saver_output = Command::new("sh")
    .arg("-c")
    .arg("ps aux | grep -i 'ScreenSaverEngine' | grep -v grep")
    .output();
  
  let is_screen_saver_active = screen_saver_output
    .map(|output| !output.stdout.is_empty())
    .unwrap_or(false);
  
  // Method 2: Check for loginwindow (appears when screen is locked via Cmd+Ctrl+Q)
  let login_window_output = Command::new("sh")
    .arg("-c")
    .arg("ps aux | grep -i 'loginwindow' | grep -v grep")
    .output();
  
  let is_login_window_active = login_window_output
    .map(|output| !output.stdout.is_empty())
    .unwrap_or(false);
  
  // Method 3: Check display power state using ioreg
  let display_state_output = Command::new("sh")
    .arg("-c")
    .arg("ioreg -n IODisplayWrangler | grep -i 'CurrentPowerState' | head -1")
    .output();
  
  let is_display_off = display_state_output
    .map(|output| {
      let output_str = String::from_utf8_lossy(&output.stdout);
      !output_str.contains("4") // 4 means display is on
    })
    .unwrap_or(false);
  
  // Method 4: Check for sleep state using pmset
  let sleep_output = Command::new("sh")
    .arg("-c")
    .arg("pmset -g log | grep -E 'Sleep|Wake' | tail -1")
    .output();
  
  let is_sleeping = sleep_output
    .map(|output| {
      let log_line = String::from_utf8_lossy(&output.stdout);
      log_line.contains("Sleep") && !log_line.contains("Wake")
    })
    .unwrap_or(false);
  
  // Method 5: Check for lock state using log stream (more reliable)
  let lock_output = Command::new("sh")
    .arg("-c")
    .arg("log stream --predicate 'eventMessage CONTAINS \"loginwindow\"' --style compact | head -1")
    .output();
  
  let is_locked_via_log = lock_output
    .map(|output| {
      let output_str = String::from_utf8_lossy(&output.stdout);
      output_str.contains("loginwindow") && output_str.contains("lock")
    })
    .unwrap_or(false);
  
  // Method 6: Check for user session state
  let session_output = Command::new("sh")
    .arg("-c")
    .arg("who | grep -v 'console' | wc -l")
    .output();
  
  let is_user_session_active = session_output
    .map(|output| {
      let output_str = String::from_utf8_lossy(&output.stdout);
      output_str.trim() != "0"
    })
    .unwrap_or(true); // Assume active if we can't determine
  
  // Method 7: Check for lock state using system_profiler (more reliable)
  let system_lock_output = Command::new("sh")
    .arg("-c")
    .arg("system_profiler SPSoftwareDataType | grep -i 'loginwindow'")
    .output();
  
  let is_system_locked = system_lock_output
    .map(|output| !output.stdout.is_empty())
    .unwrap_or(false);
  
  // Method 8: Check for lock state using defaults (most reliable)
  let lock_pref_output = Command::new("sh")
    .arg("-c")
    .arg("defaults read com.apple.loginwindow | grep -i 'lock'")
    .output();
  
  let is_locked_via_prefs = lock_pref_output
    .map(|output| !output.stdout.is_empty())
    .unwrap_or(false);
  
  // Combine all methods - if any indicate locked state, consider it locked
  let is_locked = is_screen_saver_active || 
                  is_login_window_active || 
                  is_display_off || 
                  is_sleeping || 
                  is_locked_via_log ||
                  is_system_locked ||
                  is_locked_via_prefs ||
                  !is_user_session_active;
  
  // Debug output
  if is_locked {
    println!("Lock detected - ScreenSaver: {}, LoginWindow: {}, DisplayOff: {}, Sleeping: {}, LogLock: {}, SystemLock: {}, PrefsLock: {}, UserSession: {}", 
      is_screen_saver_active, is_login_window_active, is_display_off, is_sleeping, is_locked_via_log, is_system_locked, is_locked_via_prefs, is_user_session_active);
  }
  
  is_locked
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
            check_screen_lock_state,
            start_screen_lock_monitoring,
            get_display_state
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
