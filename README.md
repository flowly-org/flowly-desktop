# Flowly Desktop

A small tray app for macOS, Windows and Linux to track time on your [Flowly](https://flowly.net) tasks, on
flowly.net or on any self-hosted Flowly server.

- The tray shows the running timer (time in the menu bar on macOS, tooltip elsewhere).
- Tray menu: stop the timer, start it on any of today's tasks, open Flowly, sign out.
- Click the icon for a small window: today's tasks with ▶, and "What are you working on?" for an untitled timer.
- Sign-in uses Flowly's OAuth (PKCE, loopback redirect): the browser opens, you allow access, done.
  The session is stored in the app's config folder; tokens never reach the web view.

## Servers

The app can be signed in to several Flowly servers at once: flowly.net and any self-hosted instance. Switch
between them from the tray (**Server** submenu) or the selector at the top of the window; today's tasks and the
timer always belong to the active server.

**Add server…** takes the address of the server's web app (e.g. `https://flowly.example.com`) or of its API.
The app finds the API through the web app's `config.json`, then reads the server's OAuth metadata
(`/.well-known/oauth-authorization-server`) for the sign-in, token and revocation endpoints. Servers must use
https; `http://localhost` and `http://127.0.0.1` are accepted for development.

Servers and their sessions are kept in `servers.json` in the app's config folder. A `session.json` from
version 0.2 (one server) is moved into the flowly.net entry on first start, so you stay signed in.

Built with [Tauri 2](https://tauri.app) (Rust + a plain HTML/JS window).

## Build

CI (`.github/workflows/build.yml`) builds every push for macOS (universal), Windows and Linux and uploads
the installers as artifacts. Pushing a `v*` tag also publishes a GitHub release with them. It needs no secrets,
so it works the same in forks.
Builds are **unsigned** for now: on macOS right-click → Open the first time; on Windows choose
"More info → Run anyway".

Locally: install the [Tauri prerequisites](https://tauri.app/start/prerequisites/), then

```bash
npx @tauri-apps/cli@2 dev      # run
npx @tauri-apps/cli@2 build    # installers in src-tauri/target/release/bundle
```

The default server is flowly.net (see `default_server` in `src-tauri/src/api.rs`).

## Licence

Flowly Desktop is free software under the [GNU Affero General Public License v3.0](LICENSE).
