import { invoke } from "@tauri-apps/api/core";

interface CurrentStatus {
  day_key: string;
  current_lap_duration: number;
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

  constructor() {
    this.initializeUI();
    this.setupEventListeners();
    this.startStatusUpdates();
    this.startScreenLockMonitoring();
  }

  private initializeUI(): void {
    const app = document.getElementById('app');
    if (!app) return;

    app.innerHTML = `
      <div class="container">
        <header class="header">
          <h1>🕒 Screen Time Tracker</h1>
          <p class="subtitle">Track your daily screen time with automatic pause on screen lock</p>
        </header>

        <main class="main-content">
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
        </main>
      </div>
    `;
  }

  private setupEventListeners(): void {
    const startDayBtn = document.getElementById('start-day-btn');
    const endDayBtn = document.getElementById('end-day-btn');
    const addLapBtn = document.getElementById('add-lap-btn');

    startDayBtn?.addEventListener('click', () => this.startDay());
    endDayBtn?.addEventListener('click', () => this.endDay());
    addLapBtn?.addEventListener('click', () => this.addLap());
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

  private async stopLap(): Promise<void> {
    try {
      await invoke('stop_lap');
      this.showNotification('Lap stopped!', 'success');
      await this.loadCurrentStatus();
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
      console.log('Current status:', this.currentStatus);
      this.updateTimerDisplay();

      // If there's an active session, also load the laps
      if (this.currentStatus && this.currentStatus.is_active) {
        console.log('Active session detected, loading laps...');
        await this.loadCurrentDayLaps();
      } else {
        console.log('No active session, hiding laps section');
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

    if (this.currentStatus) {
      if (currentTimer) {
        currentTimer.textContent = this.formatTime(this.currentStatus.current_lap_duration);
      }
      if (totalTimer) {
        totalTimer.textContent = this.formatTime(this.currentStatus.total_session_duration);
      }
      if (sessionInfo) {
        sessionInfo.textContent = `Active session for ${this.currentStatus.day_key}`;
      }
      if (totalInfo) {
        totalInfo.textContent = `Total from all laps`;
      }
    } else {
      if (currentTimer) currentTimer.textContent = '00:00:00';
      if (totalTimer) totalTimer.textContent = '00:00:00';
      if (sessionInfo) sessionInfo.textContent = 'No active session';
      if (totalInfo) totalInfo.textContent = '0 laps completed';
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
      startBtn.disabled = true;
      endBtn.disabled = false;
      addLapBtn.disabled = false;
    } else {
      startBtn.disabled = false;
      endBtn.disabled = true;
      addLapBtn.disabled = true;
    }
  }

  private startStatusUpdates(): void {
    window.setInterval(async () => {
      if (this.isTracking) {
        await this.loadCurrentStatus();
      }
    }, 1000);
  }


  private async loadCurrentDayLaps(): Promise<void> {
    try {
      const laps = await invoke<Lap[]>('get_current_day_laps');
      console.log('Loaded laps from backend:', laps);
      this.displayLaps(laps);
      this.showLapsSection();
    } catch (error) {
      console.error('Failed to load current day laps:', error);
    }
  }

  private displayLaps(laps: Lap[]): void {
    const lapsList = document.getElementById('laps-list');
    if (!lapsList) return;

    console.log('Displaying laps:', laps);

    if (laps.length === 0) {
      lapsList.innerHTML = '<div class="empty-laps">No laps recorded yet</div>';
      return;
    }

    const completedLaps = laps.filter(lap => lap.duration !== undefined && lap.duration !== null && lap.duration > 0);
    const currentLap = laps.find(lap => lap.duration === undefined || lap.duration === null);

    console.log('Completed laps:', completedLaps);
    console.log('Current lap:', currentLap);

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

    // Display current lap if exists
    if (currentLap) {
      const startTime = new Date(currentLap.start_time * 1000);
      lapsHtml += `
        <div class="lap-item current">
          <div class="lap-info">
            <div class="lap-details">
              <div class="lap-number">Active Lap</div>
              <div class="lap-period">
                ${startTime.toLocaleTimeString()} - Ongoing
              </div>
            </div>
            <button class="btn btn-stop btn-small" onclick="window.stopCurrentLap()">Stop</button>
          </div>
        </div>
      `;
    }

    lapsList.innerHTML = lapsHtml;
  }

  private showLapsSection(): void {
    const lapsSection = document.getElementById('laps-section');
    console.log('Showing laps section, element found:', !!lapsSection);
    if (lapsSection) {
      lapsSection.style.display = 'block';
      console.log('Laps section display set to block');
    }
  }

  private hideLapsSection(): void {
    const lapsSection = document.getElementById('laps-section');
    if (lapsSection) {
      lapsSection.style.display = 'none';
    }
  }


  private async startScreenLockMonitoring(): Promise<void> {
    // Screen lock monitoring is now handled by the backend automatically
    // No need to start it explicitly since we removed the complex monitoring
    console.log('Screen lock monitoring is handled automatically by the backend');
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
