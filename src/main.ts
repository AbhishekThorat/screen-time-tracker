import { invoke } from "@tauri-apps/api/core";
import { ReportsView } from "./reports";

interface CurrentStatus {
  day_key: string;
  current_lap_duration: number;
  current_lap_start_timestamp: number;
  total_session_duration: number;
  is_active: boolean;
}

interface Lap {
  start_time: number;
  end_time?: number;
  duration?: number;
}

interface DayRecord {
  date: string;
  total_duration: number;
  laps: Lap[];
  is_active: boolean;
}

class ScreenTimeTracker {
  private currentStatus: CurrentStatus | null = null;
  private isTracking = false;
  private reports: ReportsView | null = null;
  private activeView: 'tracker' | 'reports' = 'tracker';

  constructor() {
    this.initializeUI();
    this.setupEventListeners();
    this.startStatusUpdates();
    this.startHistoryUpdates();
    this.startScreenLockMonitoring();
  }

  private initializeUI(): void {
    const app = document.getElementById('app');
    if (!app) return;

    app.innerHTML = `
      <div class="container">
        <header class="header">
          <h1>🕒 Screen Time Tracker</h1>
          <p class="subtitle">Track your daily screen time with automatic lap management</p>
        </header>

        <nav class="view-tabs" role="tablist">
          <button class="view-tab active" data-view="tracker" role="tab">Tracker</button>
          <button class="view-tab" data-view="reports" role="tab">Reports</button>
        </nav>

        <main class="main-content" id="view-tracker">
          <!-- Timer Section -->
          <section class="timer-section">
            <div class="timer-display">
              <div class="timer-card">
                <h2>Current Lap</h2>
                <div class="timer" id="current-timer">00:00:00</div>
                <div class="session-info" id="session-info">No active session</div>
              </div>
              
              <div class="timer-card">
                <h2>Today's Total</h2>
                <div class="timer" id="total-timer">00:00:00</div>
                <div class="session-info" id="total-info">0 laps completed</div>
              </div>
            </div>

            <!-- Laps Section -->
            <div class="laps-section" id="laps-section" style="display: none;">
              <div class="laps-header">
                <h3>Today's Laps</h3>
                <button id="add-lap-btn" class="btn btn-lap btn-small">Start New Lap</button>
              </div>
              <div class="laps-list" id="laps-list">
                <div class="empty-laps">No laps recorded yet</div>
              </div>
            </div>

            <div class="controls">
              <button id="start-day-btn" class="btn btn-primary">Start Day</button>
              <button id="end-day-btn" class="btn btn-secondary" disabled>End Day</button>
            </div>
          </section>

          <!-- History Section: all recorded days -->
          <section class="history-section">
            <div class="history-header">
              <h3>📅 History</h3>
              <span class="history-subtitle" id="history-summary"></span>
            </div>
            <div class="history-list" id="history-list">
              <div class="empty-history">No history yet</div>
            </div>
          </section>
        </main>

        <div class="main-content" id="view-reports" hidden>
          <div id="reports-root"></div>
        </div>
      </div>
    `;
  }

  private setupEventListeners(): void {
    const startDayBtn = document.getElementById('start-day-btn');
    const endDayBtn = document.getElementById('end-day-btn');
    const addLapBtn = document.getElementById('add-lap-btn');
    const testLockBtn = document.getElementById('test-lock-btn');
    const manualLockBtn = document.getElementById('manual-lock-btn');
    const manualUnlockBtn = document.getElementById('manual-unlock-btn');

    startDayBtn?.addEventListener('click', () => this.startDay());
    endDayBtn?.addEventListener('click', () => this.endDay());
    addLapBtn?.addEventListener('click', () => this.addLap());
    testLockBtn?.addEventListener('click', () => this.testLockDetection());
    manualLockBtn?.addEventListener('click', () => this.simulateLock());
    manualUnlockBtn?.addEventListener('click', () => this.simulateUnlock());

    document.querySelectorAll<HTMLButtonElement>('.view-tab').forEach((tab) => {
      tab.addEventListener('click', () => this.switchView(tab.dataset.view as 'tracker' | 'reports'));
    });
  }

  // Toggle between the live Tracker view and the on-demand Reports view. The
  // Reports view is built lazily the first time it's opened, and re-fetches its
  // data on every open so the numbers stay fresh.
  private switchView(view: 'tracker' | 'reports'): void {
    if (view === this.activeView) return;
    this.activeView = view;

    document.querySelectorAll<HTMLButtonElement>('.view-tab').forEach((tab) => {
      tab.classList.toggle('active', tab.dataset.view === view);
    });
    const tracker = document.getElementById('view-tracker');
    const reports = document.getElementById('view-reports');
    if (tracker) tracker.hidden = view !== 'tracker';
    if (reports) reports.hidden = view !== 'reports';

    if (view === 'reports') {
      const root = document.getElementById('reports-root');
      if (root) {
        if (!this.reports) this.reports = new ReportsView(root);
        void this.reports.open();
      }
    }
  }

  private async startDay(): Promise<void> {
    try {
      await invoke<string>('start_day');
      this.isTracking = true;
      this.updateButtonStates();
      this.showNotification('Day started successfully!', 'success');
      await this.loadCurrentStatus();
    } catch (error) {
      this.showNotification(`Failed to start day: ${error}`, 'error');
    }
  }

  private async endDay(): Promise<void> {
    try {
      const dayRecord = await invoke<DayRecord>('end_day');
      this.isTracking = false;
      this.updateButtonStates();
      this.showNotification(`Day ended! Total time: ${this.formatTime(dayRecord.total_duration)}`, 'success');
      this.hideLapsSection();
      await this.loadHistory();
    } catch (error) {
      this.showNotification(`Failed to end day: ${error}`, 'error');
    }
  }

  private async addLap(): Promise<void> {
    try {
      await invoke('add_lap');
      this.showNotification('New lap started!', 'success');
      await this.loadCurrentStatus();
    } catch (error) {
      this.showNotification(`Failed to add lap: ${error}`, 'error');
    }
  }

  private async testLockDetection(): Promise<void> {
    try {
      const result = await invoke<string>('test_screen_lock_detection');
      this.showNotification(`Lock Detection Test:\n${result}`, 'success');
    } catch (error) {
      this.showNotification(`Failed to test lock detection: ${error}`, 'error');
    }
  }

  private async simulateLock(): Promise<void> {
    try {
      const result = await invoke<string>('handle_screen_lock');
      this.showNotification(`Simulated Lock: ${result}`, 'success');
      await this.loadCurrentStatus();
    } catch (error) {
      this.showNotification(`Failed to simulate lock: ${error}`, 'error');
    }
  }

  private async simulateUnlock(): Promise<void> {
    try {
      const result = await invoke<string>('handle_screen_unlock');
      this.showNotification(`Simulated Unlock: ${result}`, 'success');
      await this.loadCurrentStatus();
    } catch (error) {
      this.showNotification(`Failed to simulate unlock: ${error}`, 'error');
    }
  }


  private async stopLap(): Promise<void> {
    try {
      const result = await invoke<string>('stop_lap');
      this.showNotification(result, 'success');

      // Wait a bit for backend to update state
      await new Promise(resolve => setTimeout(resolve, 100));

      await this.loadCurrentStatus();

      // Session is paused, not ended - just update button states
      this.updateButtonStates();
    } catch (error) {
      this.showNotification(`Failed to stop lap: ${error}`, 'error');
    }
  }

  // Global function for stop button onclick
  public async stopCurrentLap(): Promise<void> {
    await this.stopLap();
  }

  private async loadCurrentStatus(): Promise<void> {
    try {
      this.currentStatus = await invoke<CurrentStatus | null>('get_current_status');

      // Update isTracking state based on backend response
      this.isTracking = this.currentStatus?.is_active ?? false;


      // Update button states when session state changes
      this.updateButtonStates();
      this.updateTimerDisplay();

      // Load laps for both active and paused sessions
      if (this.currentStatus) {
        await this.loadCurrentDayLaps();
      } else {
        this.hideLapsSection();
      }
    } catch (error) {
      console.error('Failed to load current status:', error);
    }
  }

  private updateTimerDisplay(): void {
    const currentTimer = document.getElementById('current-timer');
    const totalTimer = document.getElementById('total-timer');
    const sessionInfo = document.getElementById('session-info');
    const totalInfo = document.getElementById('total-info');

    // Only show dynamic timers if there's an active session
    if (this.currentStatus && this.currentStatus.is_active) {
      // Calculate smooth frontend display (continues even when app is in background)
      const currentTimeSeconds = Math.floor(Date.now() / 1000);
      const lapDuration = currentTimeSeconds - this.currentStatus.current_lap_start_timestamp;

      // Backend's total_session_duration contains ONLY completed laps
      // We add the current lap for smooth counting
      const smoothLapDuration = lapDuration;
      const smoothTotal = this.currentStatus.total_session_duration + smoothLapDuration;

      if (currentTimer) {
        currentTimer.textContent = this.formatTime(Math.floor(smoothLapDuration));
      }
      if (totalTimer) {
        totalTimer.textContent = this.formatTime(Math.floor(smoothTotal));
      }
      if (sessionInfo) {
        sessionInfo.textContent = `Active session for ${this.currentStatus.day_key}`;
      }
      if (totalInfo) {
        totalInfo.textContent = `Total from all laps`;
      }
    } else {
      // Show static timers when no active session or paused
      if (currentTimer) currentTimer.textContent = '00:00:00';
      if (totalTimer) {
        // If we have a paused session, show the completed laps total
        if (this.currentStatus && !this.currentStatus.is_active) {
          totalTimer.textContent = this.formatTime(this.currentStatus.total_session_duration);
        } else {
          totalTimer.textContent = '00:00:00';
        }
      }
      if (sessionInfo) {
        if (this.currentStatus && !this.currentStatus.is_active) {
          sessionInfo.textContent = `Paused session for ${this.currentStatus.day_key}`;
        } else {
          sessionInfo.textContent = 'No active session';
        }
      }
      if (totalInfo) {
        if (this.currentStatus && !this.currentStatus.is_active) {
          totalInfo.textContent = `Completed laps only`;
        } else {
          totalInfo.textContent = '0 laps completed';
        }
      }
    }
  }

  private formatTime(seconds: number): string {
    const hours = Math.floor(seconds / 3600);
    const minutes = Math.floor((seconds % 3600) / 60);
    const secs = seconds % 60;
    return `${hours.toString().padStart(2, '0')}:${minutes.toString().padStart(2, '0')}:${secs.toString().padStart(2, '0')}`;
  }

  private updateButtonStates(): void {
    const startBtn = document.getElementById('start-day-btn') as HTMLButtonElement;
    const endBtn = document.getElementById('end-day-btn') as HTMLButtonElement;
    const addLapBtn = document.getElementById('add-lap-btn') as HTMLButtonElement;

    if (this.isTracking) {
      // Active session
      startBtn.disabled = true;
      endBtn.disabled = false;
      addLapBtn.disabled = false;
    } else if (this.currentStatus && !this.currentStatus.is_active) {
      // Paused session - can add new lap or end day
      startBtn.disabled = true;
      endBtn.disabled = false;
      addLapBtn.disabled = false;
    } else {
      // No session
      startBtn.disabled = false;
      endBtn.disabled = true;
      addLapBtn.disabled = true;
    }
  }

  private startStatusUpdates(): void {
    window.setInterval(async () => {
      // Always check the backend status to stay in sync
      await this.loadCurrentStatus();
    }, 200); // Update 5x per second for smooth display
  }


  private async loadCurrentDayLaps(): Promise<void> {
    try {
      const laps = await invoke<Lap[]>('get_current_day_laps');
      const isActive = this.currentStatus?.is_active ?? false;
      const currentLapDuration = this.currentStatus?.current_lap_duration ?? 0;
      this.displayLaps(laps, isActive, currentLapDuration);
      this.showLapsSection();
    } catch (error) {
      console.error('Failed to load current day laps:', error);
    }
  }

  private displayLaps(laps: Lap[], isActive: boolean, currentLapDuration: number): void {
    const lapsList = document.getElementById('laps-list');
    if (!lapsList) return;

    if (laps.length === 0) {
      lapsList.innerHTML = '<div class="empty-laps">No laps recorded yet</div>';
      return;
    }

    const completedLaps = laps.filter(lap => lap.duration !== undefined && lap.duration !== null && lap.duration > 0);
    const currentLap = laps.find(lap => lap.duration === undefined || lap.duration === null);

    let lapsHtml = '';

    // Display completed laps
    completedLaps.forEach((lap, index) => {
      const startTime = new Date(lap.start_time * 1000);
      const endTime = lap.end_time ? new Date(lap.end_time * 1000) : null;
      const duration = lap.duration || 0;

      lapsHtml += `
        <div class="lap-item completed">
          <div class="lap-info">
            <div class="lap-details">
              <div class="lap-number">Lap ${index + 1}</div>
              <div class="lap-time">${this.formatTime(duration)}</div>
              <div class="lap-period">
                ${startTime.toLocaleTimeString()} - ${endTime?.toLocaleTimeString() || 'Ongoing'}
              </div>
            </div>
          </div>
        </div>
      `;
    });

    // Display current lap only if session is actively tracking
    if (currentLap && isActive) {
      const startTime = new Date(currentLap.start_time * 1000);

      // Use backend's current_lap_duration which already excludes sleep/background time
      // Only show stop button if lap has been running for at least 3 seconds
      const canStop = currentLapDuration >= 3;
      const stopButton = canStop
        ? `<button class="btn btn-stop btn-small" onclick="window.stopCurrentLap()">Stop</button>`
        : `<span class="lap-wait-text">⏳ Wait ${Math.ceil(3 - currentLapDuration)}s to stop</span>`;

      lapsHtml += `
        <div class="lap-item current">
          <div class="lap-info">
            <div class="lap-details">
              <div class="lap-number">Active Lap</div>
              <div class="lap-period">
                ${startTime.toLocaleTimeString()} - Ongoing
              </div>
            </div>
            ${stopButton}
          </div>
        </div>
      `;
    }

    lapsList.innerHTML = lapsHtml;
  }

  private showLapsSection(): void {
    const lapsSection = document.getElementById('laps-section');
    if (lapsSection) {
      lapsSection.style.display = 'block';
    }
  }

  private hideLapsSection(): void {
    const lapsSection = document.getElementById('laps-section');
    if (lapsSection) {
      lapsSection.style.display = 'none';
    }
  }

  private startHistoryUpdates(): void {
    // History changes far less often than the live timer, so poll it slowly.
    this.loadHistory();
    window.setInterval(() => this.loadHistory(), 5000);
  }

  private async loadHistory(): Promise<void> {
    try {
      const records = await invoke<DayRecord[]>('get_all_day_records');
      this.displayHistory(records);
    } catch (error) {
      console.error('Failed to load history:', error);
    }
  }

  private displayHistory(records: DayRecord[]): void {
    const historyList = document.getElementById('history-list');
    const summary = document.getElementById('history-summary');
    if (!historyList) return;

    if (!records || records.length === 0) {
      historyList.innerHTML = '<div class="empty-history">No history yet</div>';
      if (summary) summary.textContent = '';
      return;
    }

    const grandTotal = records.reduce((sum, r) => sum + (r.total_duration || 0), 0);
    if (summary) {
      summary.textContent = `${records.length} day${records.length > 1 ? 's' : ''} · ${this.formatTime(grandTotal)} total`;
    }

    historyList.innerHTML = records.map(record => this.renderHistoryDay(record)).join('');

    // Wire up expand/collapse toggles for each day.
    historyList.querySelectorAll('.history-day-header').forEach(header => {
      header.addEventListener('click', () => {
        const day = header.closest('.history-day');
        day?.classList.toggle('expanded');
      });
    });
  }

  private renderHistoryDay(record: DayRecord): string {
    const completedLaps = record.laps.filter(
      lap => lap.duration !== undefined && lap.duration !== null && lap.duration > 0
    );

    const dateLabel = this.formatDateLabel(record.date);
    const statusBadge = record.is_active
      ? '<span class="history-badge active">Active</span>'
      : '';

    const lapsHtml = record.laps.length === 0
      ? '<div class="empty-laps">No laps recorded</div>'
      : record.laps.map((lap, index) => {
          const startTime = new Date(lap.start_time * 1000);
          const endTime = lap.end_time ? new Date(lap.end_time * 1000) : null;
          const isOngoing = lap.duration === undefined || lap.duration === null;
          const durationText = isOngoing ? 'Ongoing' : this.formatTime(lap.duration || 0);
          return `
            <div class="history-lap ${isOngoing ? 'ongoing' : ''}">
              <span class="history-lap-num">Lap ${index + 1}</span>
              <span class="history-lap-period">${startTime.toLocaleTimeString()} - ${endTime ? endTime.toLocaleTimeString() : 'Ongoing'}</span>
              <span class="history-lap-duration">${durationText}</span>
            </div>
          `;
        }).join('');

    return `
      <div class="history-day">
        <div class="history-day-header">
          <div class="history-day-title">
            <span class="history-day-date">${dateLabel}</span>
            ${statusBadge}
          </div>
          <div class="history-day-meta">
            <span class="history-day-total">${this.formatTime(record.total_duration || 0)}</span>
            <span class="history-day-laps">${completedLaps.length} lap${completedLaps.length === 1 ? '' : 's'}</span>
            <span class="history-chevron">▾</span>
          </div>
        </div>
        <div class="history-day-laps-list">
          ${lapsHtml}
        </div>
      </div>
    `;
  }

  private formatDateLabel(date: string): string {
    // date is "YYYY-MM-DD" in local time; add labels for today/yesterday.
    const today = new Date();
    const todayKey = this.toDateKey(today);
    const yesterday = new Date(today);
    yesterday.setDate(today.getDate() - 1);
    const yesterdayKey = this.toDateKey(yesterday);

    let suffix = '';
    if (date === todayKey) suffix = ' · Today';
    else if (date === yesterdayKey) suffix = ' · Yesterday';

    // Parse as local date (avoid UTC shift from `new Date("YYYY-MM-DD")`).
    const [y, m, d] = date.split('-').map(Number);
    const parsed = new Date(y, m - 1, d);
    const pretty = parsed.toLocaleDateString(undefined, {
      weekday: 'short',
      month: 'short',
      day: 'numeric',
    });
    return `${pretty}${suffix}`;
  }

  private toDateKey(d: Date): string {
    const y = d.getFullYear();
    const m = (d.getMonth() + 1).toString().padStart(2, '0');
    const day = d.getDate().toString().padStart(2, '0');
    return `${y}-${m}-${day}`;
  }


  private async startScreenLockMonitoring(): Promise<void> {
    // System monitoring is now handled by the backend automatically
    // This includes screen lock, sleep/wake, and logout/login events
    console.log('System monitoring is handled automatically by the backend');

    // Add event listeners for system events that might be detected by the frontend
    this.setupSystemEventListeners();

    // Setup notification click handler for startup notifications
    this.setupNotificationHandler();
  }

  private setupNotificationHandler(): void {
    // Listen for notification clicks (when user clicks "Start tracking" notification)
    // The notification plugin emits events we can listen to
    window.addEventListener('notification-clicked', async () => {
      console.log('Notification clicked - starting day from notification');
      try {
        await invoke('start_day_from_notification');
        this.showNotification('Tracking started!', 'success');
        await this.loadCurrentStatus();
      } catch (error) {
        console.error('Failed to start day from notification:', error);
        this.showNotification(`Failed to start tracking: ${error}`, 'error');
      }
    });
  }

  private setupSystemEventListeners(): void {
    // Listen for visibility changes (when user switches tabs or minimizes)
    document.addEventListener('visibilitychange', () => {
      if (document.hidden) {
        console.log('App became hidden - system may be locking or sleeping');
      } else {
        console.log('App became visible - system may have unlocked or woken');
        // Refresh status when app becomes visible again
        this.loadCurrentStatus();
      }
    });

    // Listen for window focus/blur events
    window.addEventListener('blur', () => {
      console.log('Window lost focus - may indicate system events');
    });

    window.addEventListener('focus', () => {
      console.log('Window gained focus - refreshing status');
      this.loadCurrentStatus();
    });

    // Listen for page unload (user closing app or logging out)
    window.addEventListener('beforeunload', () => {
      console.log('App is being closed - system will handle lap management');
    });
  }


  private showNotification(message: string, type: 'success' | 'error'): void {
    const notification = document.createElement('div');
    notification.className = `notification ${type}`;
    notification.textContent = message;

    document.body.appendChild(notification);

    setTimeout(() => {
      notification.remove();
    }, 3000);
  }
}

// Initialize the app
let tracker: ScreenTimeTracker;

document.addEventListener('DOMContentLoaded', () => {
  tracker = new ScreenTimeTracker();

  // Make stopCurrentLap available globally
  (window as any).stopCurrentLap = () => tracker.stopCurrentLap();
});
