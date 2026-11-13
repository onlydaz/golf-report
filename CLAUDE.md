# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

Windows desktop app (Tauri 2 + Rust backend + React 19/TypeScript/Ant Design frontend) that collects measurement results from an AS200 PLC over Modbus TCP and stores them in PostgreSQL (local or Neon). UI text and many code comments are in Vietnamese. See [README.md](README.md) for the connection workflow and `docs/` for the register map (xlsx).

## Commands

Frontend (run from repo root):
- `npm run tauri dev` — run the full app (starts Vite on fixed port 1420 via `beforeDevCommand`)
- `npm run build` — `tsc` + `vite build` (output `dist/`, used as Tauri `frontendDist`)
- `npm test` — Vitest; single file: `npx vitest run src/utils/as200Report.test.ts`

Backend (run from `src-tauri/`):
- `cargo test` — unit tests; single test: `cargo test <name>` (e.g. `cargo test --lib plc::as200::tests::`)
- Postgres integration tests are `#[ignore]`d and require explicit opt-in via `AS200_TEST_DATABASE=golf_report_test` (see [test_support.rs](src-tauri/src/database/test_support.rs)); run with `cargo test -- --ignored`. They only accept a localhost DB and never reset data.
- Helper bins: `cargo run --bin as200-db [-- --database NAME [--migrate]]` (audit by default; `--migrate` requires explicit `--database`), `cargo run --bin as200-reader`.
- `npm run tauri build` produces the NSIS installer (`perMachine`, hooks in `installer-hooks.nsh`).

## Architecture

**Frontend** (`src/`): pages under `src/pages/*` routed via react-router in `layouts/MainLayout.tsx`; all backend access goes through `src/services/*` wrappers around Tauri `invoke`. Report/Excel logic lives in `src/utils/as200Report*.ts` / `as200Export.ts` (exceljs/xlsx).

**Backend** (`src-tauri/src/`), command handlers registered in [lib.rs](src-tauri/src/lib.rs):
- `plc/as200/` — pure Modbus protocol layer: `reader` (FC03 polling of a 240-WORD double buffer, stability checks → `Unstable`), `decode` (DINT32/REAL32 with configurable `WordOrder`), `map` (register layout), `ack` / `heartbeat` (opt-in FC06/FC16 writes; `As200Config` deliberately has no `Default`).
- `plc/worker.rs` (+ `worker/retry.rs`) — long-running background task owned by `WorkerState`: polls the PLC, writes captures to a local **journal** file, then sinks them into the PostgreSQL **inbox**, then ACKs the PLC. Recovery guards (journal / inbox / DB reconciliation) intentionally block progress on counter decrease, pending data or unknown ACK outcome rather than guessing.
- `plc/connection_settings.rs` — Tauri commands for the QC screen flow (stop → set endpoint → test → connect); `simulator_config.rs` parses `as200_config.toml`.
- `database/` (state, config loading, embedded sqlx migrations in `src-tauri/migrations/`), then `repositories/` → `services/` → `commands/` layering for measurements, inbox, and PLC sources.

Key concepts: data flows PLC → journal file → `inbox` table → measurements; each physical endpoint has an internal *source identity* and *session id* derived by the backend (users only enter IP/port/unit ID). Switching PLC endpoints is deliberately guarded — do not bypass guards by deleting the journal or changing session IDs.

## Configuration & runtime files

- `as200_config.toml` (git-ignored; template: `as200_config.example.toml`) — PLC endpoint and safety flags (`ack_enabled`, `allow_plc_heartbeat_write`, ... all default off). Do not enable PLC writes casually.
- `database_config.toml` — template `database_config.example.toml`; real file holds credentials and must not be committed. Lookup: `%APPDATA%\com.golf.report\` overrides an embedded `src-tauri/resources/database_config.toml`.
- Debug builds keep PLC state in the Tauri app config dir; **release builds use `%ProgramData%\GolfReport`** and refuse to start if stray PLC history exists in the other location ([lib.rs](src-tauri/src/lib.rs)).
- `local-runtime/`, `*.lock` files, and `as200_capture_journal.json` are runtime artifacts.
- Rust edition 2024; release profile uses LTO + `panic = "abort"`.
