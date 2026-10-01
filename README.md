# Flowly Desktop

A small tray app for macOS, Windows and Linux to track time on your [Flowly](https://flowly.net) tasks.

- The tray shows the running timer (time in the menu bar on macOS, tooltip elsewhere).
- Tray menu: stop the timer, start it on any of today's tasks, open Flowly, sign out.
- Click the icon for a small window: today's tasks with ▶, and "What are you working on?" for an untitled timer.
- Sign-in uses Flowly's OAuth (PKCE, loopback redirect): the browser opens, you allow access, done.
  The session is stored in the app's config folder; tokens never reach the web view.

Built with [Tauri 2](https://tauri.app) (Rust + a plain HTML/JS window).

## Build

CI (`.github/workflows/build.yml`) builds every push for macOS (universal), Windows and Linux and uploads
the installers as artifacts. Pushing a `v*` tag also creates a draft GitHub release with them.
Builds are **unsigned** for now: on macOS right-click → Open the first time; on Windows choose
"More info → Run anyway".

Locally: install the [Tauri prerequisites](https://tauri.app/start/prerequisites/), then

```bash
npx @tauri-apps/cli@2 dev      # run
npx @tauri-apps/cli@2 build    # installers in src-tauri/target/release/bundle
```

The API defaults to `https://api.flowly.net` (see `src-tauri/src/api.rs`).
