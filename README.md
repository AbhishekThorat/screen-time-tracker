# Screen Time Tracker

A simplified Tauri-based desktop application for tracking screen time with automatic pause/resume functionality when the screen is locked/unlocked.

## Features

- **Real-time Timer**: Track your daily screen time with a live timer
- **Automatic Pause/Resume**: Timer automatically pauses when screen is locked and resumes with a new lap when unlocked
- **Daily Sessions**: Start and end daily tracking sessions
- **Lap Tracking**: Each screen unlock creates a new lap, providing detailed session breakdown
- **Today's Laps**: View all laps for the current day with individual durations
- **Beautiful UI**: Modern, responsive design with real-time updates

## How It Works

1. **Start a Day**: Click "Start Day" to begin tracking your screen time
2. **Automatic Monitoring**: The app runs in the background and monitors screen lock/unlock events
3. **Lap Creation**: Each time you lock/unlock your screen, a new lap is created
4. **View Laps**: See all your laps for the current day with individual durations
5. **End Day**: Click "End Day" to finish tracking and save the session

## Technology Stack

- **Frontend**: TypeScript, HTML5, CSS3
- **Backend**: Rust with Tauri
- **UI Framework**: Vanilla TypeScript with modern CSS
- **Data Storage**: In-memory storage (can be extended to persistent storage)

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
- **State Management**: In-memory storage with thread-safe access
- **Event System**: Tauri events for screen lock/unlock notifications

## Future Enhancements

- [ ] Persistent data storage (SQLite/JSON files)
- [ ] Weekly and monthly reports
- [ ] Export functionality (CSV, PDF reports)
- [ ] Customizable tracking intervals
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

## License

This project is licensed under the MIT License - see the LICENSE file for details.

## Support

For issues and feature requests, please create an issue in the repository.
