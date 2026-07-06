# Screen Time Tracker

A lightweight macOS **menu-bar** application for tracking screen time with automatic pause/resume when the screen is locked/unlocked. It lives in the menu bar, launches automatically at login, manages your work day around machine power events, and keeps a full local history of every day you've tracked.

## Features

- **Menu-bar app**: Lives in the macOS menu bar with no Dock icon. Click the icon for a compact popover; expand to the full window only when you want details
- **Low footprint**: No window is open by default, so **no webview process runs while idle** (~30 MB vs ~150 MB for an always-open window). The timer isn't an always-running counter — elapsed time is computed on demand from timestamps only while a popover/window is open
- **Quick-glance popover**: Shows **current lap** and **today's total** side by side, plus Pause/Resume and New Lap; an "Expand full window" button opens the detailed view
- **Real-time Timer**: Track your daily screen time with a live timer
- **Automatic Pause/Resume**: Timer automatically pauses when screen is locked and resumes with a new lap when unlocked
- **Launch at Login**: The app registers itself to start automatically whenever you start your machine
- **Automatic Day Management**: Your day starts in the background on boot; shutdown ends the day and the next boot starts a fresh one
- **Resilient to Restarts**: A mid-day restart (e.g. a power cut) continues the same day instead of losing it — time while the machine was off is never counted
- **Past-midnight aware**: Work that crosses midnight in one continuous session stays credited to the day it started on
- **Per-day History**: Every tracked day is stored locally with its full lap breakdown and daily total, browsable in the History section
- **Reports**: A **Reports** tab (loaded on demand) with Week / Month / Year views — KPI cards (total, daily average, days tracked, longest day, current streak), daily/monthly bar charts, average-by-weekday, an active-time-by-hour heatmap, and a GitHub-style yearly calendar. All charts are hand-rolled inline SVG/CSS — no chart library — computed in the frontend from existing per-day data
- **Lap Tracking**: Each screen unlock creates a new lap, providing a detailed session breakdown
- **Local-first**: All data lives on your machine (`~/Library/Application Support/screen-time/state.json`) — no server, no account

## How It Works

1. **Autostart**: The app launches at login (into the menu bar) and automatically starts your day in the background. If you're not working yet, click the menu-bar icon and press Pause.
2. **Menu bar**: Click the icon for the quick-glance popover (current lap + today's total, Pause/Resume, New Lap). Use **Expand full window** for laps and history. Right-click the icon for Open / Quit.
3. **Automatic Monitoring**: It monitors screen lock/unlock and sleep/wake events, pausing and resuming tracking accordingly.
4. **Lap Creation**: Each time you lock/unlock (or resume), a new lap is created within the current day.
5. **Day Lifecycle**:
   - **Shutdown** ends your day; the **next day's boot** starts a fresh day.
   - A **restart on the same day** (e.g. electricity cut) continues your existing day, adding a new lap.
   - Working **past midnight** in one sitting keeps the credit on the day you started.
6. **History**: Browse all past days and their laps in the History section, or click **End Day** to finalize the current day early.

## Technology Stack

- **Frontend**: TypeScript, HTML5, CSS3
- **Backend**: Rust with Tauri 2
- **UI Framework**: Vanilla TypeScript with modern CSS
- **Data Storage**: Local JSON persistence (`state.json` in the app data directory), retaining full per-day history
- **Autostart**: `tauri-plugin-autostart` (launch on login)
- **Menu bar**: Tauri tray icon + on-demand popover / full windows (created when opened, destroyed when closed to keep idle memory low)

## Development

### Prerequisites

- Node.js (v16 or higher)
- Rust (latest stable)
- Yarn package manager

### Installation

1. Clone the repository:

```bash
git clone <repository-url>
cd screen-time-tracker
```

2. Install dependencies:

```bash
yarn install
```

3. Run the development server:

```bash
yarn tauri dev
```

### Building

To build the application for production:

```bash
yarn tauri build
```

This will create platform-specific executables in the `src-tauri/target/release` directory.

## Usage

### Starting a Session

1. Launch the Screen Time Tracker app
2. Click the "Start Day" button to begin tracking
3. The timer will start counting your active screen time
4. The app will automatically detect when you lock/unlock your screen

### During a Session

- **Current Session Timer**: Shows the time for the current lap (since last unlock)
- **Today's Total Timer**: Shows the cumulative time for the entire day
- **Automatic Pause**: When you lock your screen, the timer pauses automatically
- **New Lap**: When you unlock your screen, a new lap starts

### Ending a Session

1. Click the "End Day" button to finish tracking
2. The session data will be saved
3. You can view the session details in the History tab

### Today's Laps

- **Current Session**: Shows time since last screen unlock (resets to 00:00:00 on each unlock)
- **Today's Total**: Shows cumulative time from all completed laps
- **Lap List**: Displays all laps for the day with their individual durations and times
- **Visual Indicators**: Completed laps have green borders, current lap has blue border

## Technical Details

### Screen Lock Detection

The app uses macOS system commands to detect screen lock/unlock events:

```bash
ps aux | grep -i 'ScreenSaverEngine' | grep -v grep
```

This approach works on macOS and can be extended for other platforms.

### Data Structure

- **Lap**: Represents a single continuous session between lock/unlock events
- **DayRecord**: Contains all laps for a single day with total duration
- **CurrentStatus**: Real-time status showing current lap and total session duration

### Architecture

- **Frontend**: Single-page application with real-time updates
- **Backend**: Rust service with Tauri commands for data management
- **State Management**: Thread-safe in-memory state (`Arc<Mutex<>>`) persisted to a local JSON file
- **Event System**: Background monitoring threads for screen lock/unlock and sleep/wake

## Future Enhancements

- [x] Persistent data storage (local JSON)
- [x] Launch at login (autostart)
- [x] Full per-day history
- [x] Weekly, monthly, and yearly reports (in-app Reports tab)
- [ ] Migrate storage to SQLite for large histories
- [ ] Export functionality (CSV, PDF reports)
- [ ] Goal setting and notifications
- [ ] Cross-platform screen lock detection
- [ ] Settings and preferences
- [ ] Backup and sync functionality

## Contributing

1. Fork the repository
2. Create a feature branch
3. Make your changes
4. Add tests if applicable
5. Submit a pull request

## Credits

This project was designed and built with [**Cursor**](https://cursor.com) — the AI-first
code editor. Cursor's AI pair-programming was used throughout to architect the day/session
lifecycle, the macOS system-event detection, the local persistence layer, and the
TypeScript UI. Project conventions for the AI assistant live in [`.cursor/rules/`](.cursor/rules).

🖱️ Built with Cursor.

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Support

For issues and feature requests, please create an issue in the repository.
