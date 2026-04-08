# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

`autohour` is a Rust CLI tool that automates submitting daily work logs and work hours to the Linker platform (`lh.i.linker.cc` / `weeksystem.linker.cc`). It also supports a macOS tray application with reminders.

## Build & Run Commands

```bash
cargo check          # Check compilation
cargo build          # Debug build
cargo build --release
cargo test           # Run tests
cargo run --          # Submit today's log (default behavior)
cargo run -- --date 2026-04-02        # Submit for specific date
cargo run -- check-missing            # Check missing reports this month
cargo run -- check-missing --year 2026 --month 4
cargo run -- login                    # Refresh session cookies
cargo run -- daemon --at 18:30        # Scheduled foreground runner
cargo run -- tray                     # macOS tray app (macOS only)
cargo run -- install-launch-agent     # macOS launch-at-login setup
./scripts/build-macos-app.sh          # Build distributable Autohour.app → dist/
```

## Architecture

The entire codebase is a single file: `src/main.rs`. Key structural layers:

- **CLI** (`Cli` / `Commands` via `clap`): Entry point dispatching to subcommands — `Login`, `Submit`, `AddManHour`, `CheckMissing`, `Daemon`, `Tray`, `InstallLaunchAgent`, `UninstallLaunchAgent`.

- **`LinkerClient`**: The HTTP client wrapping `reqwest` (blocking). Handles RSA-encrypted login against `login.linker.cc`, session cookie persistence to `.linker_session.cookies.json`, and all API calls (work hours, daily reports).

- **Log parsing** (`ParsedLog`): Reads `YYYY-MM-DD.md` files from `AUTOHOUR_LOG_DIR`. Expects `## 工作记录` and `## 明日计划` sections; extracts work hours from a line like `工时：8` or `工时: 7.5h`.

- **Holiday logic** (`HolidayConfig`): Loads `holidays/<year>.json` (or `AUTOHOUR_HOLIDAY_DIR`). Contains `holidays` (days off) and `makeup_workdays` (weekend days that are working days). Used by `check-missing` and tray reminders to skip non-working days.

- **Notifications** (`NotificationConfig`): Optional Telegram bot and/or SMTP email. Sent on submit success/failure and missing-report detection.

- **macOS tray** (feature-gated `#[cfg(target_os = "macos")]`): Uses `tao` event loop + `tray-icon`. Runs reminder logic in a background thread via `ReminderState` / `ReminderSlotState` — checks morning (08:00–10:00) whether yesterday was filed, and evening (18:00–20:00) whether today was filed, with 30-minute retry intervals.

- **Env loading**: On startup, `.env` is loaded from multiple locations in priority order: `AUTOHOUR_ENV_FILE` → CWD `.env` → executable-adjacent `.env` → `.app` bundle `Contents/Resources/.env` → `~/Library/Application Support/autohour/.env`.

## Key Environment Variables

Required: `LINKER_USERNAME`, `LINKER_PASSWORD`, `LINKER_PROJECT_ID`, `AUTOHOUR_LOG_DIR`

Optional: `AUTOHOUR_SCHEDULE_AT` (default `18:00`), `AUTOHOUR_HOLIDAY_DIR`, `TELEGRAM_BOT_TOKEN`, `TELEGRAM_CHAT_ID`, `SMTP_*`

Copy `.env.example` to `.env` to get started.

## Holiday Configuration

`holidays/<year>.json` must exist for `check-missing` and tray reminders to work. Format:

```json
{
  "holidays": ["2026-01-01"],
  "makeup_workdays": ["2026-02-08"]
}
```
