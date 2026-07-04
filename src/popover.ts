import { invoke } from "@tauri-apps/api/core";

interface CurrentStatus {
  day_key: string;
  current_lap_duration: number;
  current_lap_start_timestamp: number;
  total_session_duration: number;
  is_active: boolean;
}

// Compact menu-bar popover: a quick glance at today's timer plus the essential
// controls (pause/resume, new lap) and a button to expand the full window.
class Popover {
  private status: CurrentStatus | null = null;

  constructor() {
    this.render();
    this.startPolling();
  }

  private render(): void {
    const root = document.getElementById("popover");
    if (!root) return;
    root.innerHTML = `
      <div class="pop-card">
        <div class="pop-stats">
          <div class="pop-stat">
            <div class="pop-stat-label">Current lap</div>
            <div class="pop-stat-value" id="pop-lap-time">00:00:00</div>
          </div>
          <div class="pop-stat">
            <div class="pop-stat-label">Today's total</div>
            <div class="pop-stat-value" id="pop-total-time">00:00:00</div>
          </div>
        </div>
        <div class="pop-status" id="pop-status">No active session</div>
        <div class="pop-actions">
          <button class="pop-btn pop-btn-primary" id="pop-toggle">Start</button>
          <button class="pop-btn" id="pop-lap">New Lap</button>
        </div>
        <button class="pop-expand" id="pop-expand">⤢ Expand full window</button>
      </div>
    `;

    document.getElementById("pop-toggle")?.addEventListener("click", () => this.toggle());
    document.getElementById("pop-lap")?.addEventListener("click", () => this.newLap());
    document.getElementById("pop-expand")?.addEventListener("click", () => this.expand());
  }

  private startPolling(): void {
    this.refresh();
    // Only runs while the popover window is open, so 1s is plenty and cheap.
    window.setInterval(() => this.refresh(), 1000);
  }

  private async refresh(): Promise<void> {
    try {
      this.status = await invoke<CurrentStatus | null>("get_current_status");
      this.update();
    } catch (e) {
      console.error("popover refresh failed", e);
    }
  }

  private update(): void {
    const lapTime = document.getElementById("pop-lap-time");
    const totalTime = document.getElementById("pop-total-time");
    const status = document.getElementById("pop-status");
    const toggle = document.getElementById("pop-toggle") as HTMLButtonElement | null;
    const lapBtn = document.getElementById("pop-lap") as HTMLButtonElement | null;

    if (!this.status) {
      // No session at all (day ended or not started).
      if (lapTime) lapTime.textContent = "00:00:00";
      if (totalTime) totalTime.textContent = "00:00:00";
      if (status) status.textContent = "No active session";
      if (toggle) toggle.textContent = "Start Day";
      if (lapBtn) lapBtn.disabled = true;
      return;
    }

    const active = this.status.is_active;
    // Live current-lap time is derived from the lap's start timestamp; when paused
    // there is no running lap, so it shows 00:00:00.
    const live = active
      ? Math.max(0, Math.floor(Date.now() / 1000) - this.status.current_lap_start_timestamp)
      : 0;
    const total = this.status.total_session_duration + live;

    if (lapTime) lapTime.textContent = this.formatTime(live);
    if (totalTime) totalTime.textContent = this.formatTime(total);
    if (status) status.textContent = active ? "Tracking · today" : "Paused · today";
    if (toggle) toggle.textContent = active ? "Pause" : "Resume";
    if (lapBtn) lapBtn.disabled = false;
  }

  private async toggle(): Promise<void> {
    try {
      if (this.status?.is_active) {
        await invoke("stop_lap");
      } else if (this.status) {
        // Paused session -> resume with a new lap.
        await invoke("add_lap");
      } else {
        // No session -> start a fresh day.
        await invoke("start_day");
      }
      await this.refresh();
    } catch (e) {
      console.error("toggle failed", e);
    }
  }

  private async newLap(): Promise<void> {
    try {
      await invoke("add_lap");
      await this.refresh();
    } catch (e) {
      console.error("new lap failed", e);
    }
  }

  private async expand(): Promise<void> {
    try {
      await invoke("show_main_window");
    } catch (e) {
      console.error("expand failed", e);
    }
  }

  private formatTime(seconds: number): string {
    const h = Math.floor(seconds / 3600);
    const m = Math.floor((seconds % 3600) / 60);
    const s = seconds % 60;
    return `${h.toString().padStart(2, "0")}:${m.toString().padStart(2, "0")}:${s
      .toString()
      .padStart(2, "0")}`;
  }
}

document.addEventListener("DOMContentLoaded", () => new Popover());
