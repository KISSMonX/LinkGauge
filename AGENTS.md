# AGENTS.md

Guidance for AI coding agents working in this repository. Human-facing docs live in
[`README.md`](README.md) / [`README.zh-CN.md`](README.zh-CN.md) — read the README first for
what the product does; this file covers how to change it without breaking its conventions.

## What this project is

LinkGauge is a desktop network performance tester: Rust + Tauri 2 backend, Vue 3 + TypeScript
frontend. Ping uses the system command; all TCP/UDP testing runs on
[riperf3](https://github.com/therealevanhenry/riperf3), a pure-Rust iperf3 implementation
vendored at `vendor/riperf3` and compiled **into** the application.

**The zero-external-binary property is a product guarantee, not an implementation detail.**
The README and the installer notes both promise that no iperf3 (or ssh, or any other network
tool) executable is bundled, resolved, or spawned. Do not introduce a dependency that shells
out to one. When a feature needs a protocol, add a pure-Rust library — that is why the SSH
console uses `russh` rather than `Command::new("ssh")`.

## Commands

```bash
npm ci                       # install frontend deps
npm run tauri dev            # run the desktop app
npm run dev                  # browser-only preview (simulated data, see "Preview mode")
npm run build                # vue-tsc --noEmit && vite build  — run before every commit
cargo test   --manifest-path src-tauri/Cargo.toml
cargo check  --manifest-path src-tauri/Cargo.toml
cargo fmt    --manifest-path src-tauri/Cargo.toml -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
npm run tauri build          # NSIS installer (Windows) / add -- --bundles appimage,deb on Linux
```

All four checks — `npm run build`, `cargo fmt --check`, `cargo clippy -- -D warnings` and
`cargo test` — must pass before you commit; CI runs exactly these on Windows and Linux
(`.github/workflows/ci.yml`), so a local failure is a red build. The tree is currently clean
on all four, and `clippy` is gated at `-D warnings`, so a new warning breaks the build rather
than accumulating.

## Layout

```text
src/                    Vue 3 frontend
  App.vue               all app state, task queue, cross-window sync, event handling
  components/           panels (config / dashboards / status / console), chart, icons
  types.ts              frontend half of the IPC contract
  i18n.ts               every user-visible string
  terminal.ts           SSH console line buffer
  styles.css            entire visual system (no scoped styles in components)
src-tauri/src/
  lib.rs                Tauri builder, window creation, command registration
  runner.rs             riperf3 client/server tasks, Ping, per-test log files, cancellation
  ssh.rs                SSH remote console: session, PTY shell, output decoding
  report.rs             HTML/PDF report generation
  settings.rs           settings.json read/write, config export
  system.rs             local network interface information
  models.rs             Rust half of the IPC contract
vendor/riperf3/         vendored engine + local patch — see "Vendored engine"
```

## Conventions that are easy to get wrong

### Comments and strings are in different languages, deliberately

- **Source comments are Chinese.** Match that. Comments explain *why*, not *what* — most
  existing comments document a non-obvious constraint (a race, a protocol quirk, a layout
  trap). Write that kind, or none.
- **User-visible strings are never hard-coded.** They go through `src/i18n.ts` on the
  frontend and `tr()` / `tr_format!` in Rust.

### Adding a UI string

`src/i18n.ts` defines `const zh = {...}`, derives `MessageKey` from it, then declares
`const en: Record<MessageKey, string>`. Adding a key to `zh` makes the compiler demand the
English one — so add to `zh` first and let `vue-tsc` find the gap. Keys are dotted by area
(`cfg.`, `srv.`, `ssh.`, `dash.`, `sdash.`, `st.`, `rep.`, `err.`, `log.`, `preview.`).

### Adding a backend message

Rust user-facing text uses `tr(locale, "中文", "English")` or the `tr_format!` macro. The
locale is read **live** from `AppState.locale` via `current_locale()` on each emission, not
captured at session start — switching language mid-run must change subsequent engine logs.
`runner.rs` and `ssh.rs` each define their own local copies of these helpers.

### Adding a Tauri command

1. Write it in the owning module with `#[tauri::command]`.
2. Register it in the `invoke_handler![...]` list in `lib.rs` — forgetting this compiles fine
   and fails at runtime.
3. Request/response structs get `#[serde(rename_all = "camelCase")]`, and the matching
   TypeScript interface goes in `src/types.ts`. These two files are one contract; change them
   together.
4. New long-lived state goes in a `struct XxxState` registered with `.manage()`.

### Adding shared state (the multi-window rule)

Client and server are tabs that can be dragged out into separate OS windows (`main`,
`client`, `server`). Every window runs the same `App.vue` and they stay in lockstep by
broadcasting a full state bundle. **Any state that must survive a tab being detached has to
be wired in four places:**

1. the `SyncState` interface in `types.ts`
2. `syncBundle()` in `App.vue`
3. `applySync()` in `App.vue`
4. a `watch(..., () => emitSync())` so local changes propagate

The `syncing` flag suppresses echo loops; `stateReady` stops a freshly detached window from
broadcasting its empty state over everyone else's live data. Respect both.

**Do not put high-volume data in the sync bundle.** It is serialized on every change. The SSH
console scrollback is the worked example of the alternative: each window listens to the
broadcast event itself, and a newly opened window calls `ssh_scrollback` once to prime its
buffer, using per-chunk stream offsets to discard what the snapshot already contained.

A new window label must also be added to `windows` in `src-tauri/capabilities/default.json`,
or the window loads with no IPC permissions.

### Backend → frontend events

Two broadcast channels, both `app.emit` to every window:

- `test-event` — Ping/riperf3 metrics, logs, server status heartbeats, completion, errors
- `ssh-event` — SSH console status, logs, output chunks, session end

Frontend handlers filter by session id. Note the ordering hazard both channels share: on
loopback the backend can emit before `invoke()` has returned the session id to JavaScript, so
those events are dropped. `runner.rs` works around it for the client local port (handled
before the session check); `ssh.rs` works around it by having the snapshot carry the live
connection state. If you add a "fires immediately" event, plan for this.

### Logs

`log(level, message, module)` in `App.vue`. The `module` value routes the entry:
`'server'` and `'ssh'` show in the server view, everything else in the client view, `'UI'`
shows in both and stays window-local (it is excluded from the sync bundle).

### Secrets

The iperf3 auth password, the SSH login password and the SSH key passphrase are held **in
memory only**. They are stripped before `localStorage` writes and before config export
(`withoutSecret` / `withoutSshSecret` in `App.vue`), and must be re-entered after a restart.
Non-secret fields — usernames, host names, key file paths — are persisted normally. Keep any
new credential on the same footing.

### Preview mode

`npm run dev` runs the frontend in a plain browser with no Tauri backend. Guard every
`invoke()` behind `isTauri()` and provide a fallback: simulated data, or an `infoDialog` with
a `preview.*` message. The app must not throw when opened this way.

### Styling

All CSS lives in `src/styles.css` — components carry no `<style>` blocks. Colors come from
the CSS custom properties defined in `:root` and `[data-theme='dark']`; never hard-code a hex
value, or dark mode breaks. Templates in this codebase are intentionally dense (multiple
elements per line); match the surrounding file instead of reformatting it.

## Testing

- Rust unit tests live beside the code in `#[cfg(test)] mod tests`.
- `src-tauri/tests/engine_smoke.rs` runs a real riperf3 client against a real riperf3 server
  on loopback. The SSH tests do the same with an embedded `russh` server. **Prefer this
  style**: the interesting bugs here are in protocol and timing, and both harnesses run
  offline in well under a second.
- Long-running async paths are decoupled from `AppHandle` so they can be tested — `ssh.rs`
  takes a `Sink` callback instead of emitting directly. Follow that pattern for new ones.
- There is no frontend test runner; `vue-tsc` is the frontend's safety net.

## Vendored engine

`vendor/riperf3` is upstream riperf3 plus a local patch that adds live `on_interval`
callbacks (upstream only reports intervals after a run finishes). The patch sites are marked
with `local LinkGauge patch` comments. If you upgrade the vendored source, re-apply the patch
and update the engine version notes in both READMEs.

## Releases

`release-please` (`.github/workflows/release-please.yml`) owns version numbers and tags. After
a merge to `master` it opens a release PR when there are `feat:`/`fix:`/breaking commits; on
merge of that PR it bumps the version in `Cargo.toml`, `Cargo.lock`, `package.json`,
`package-lock.json` and `tauri.conf.json`, rewrites `CHANGELOG.md`, and creates the `v*` tag
plus the GitHub Release that `release.yml` then populates with installers. **Never push a
`v*` tag or bump versions by hand** — a manual tag will conflict with the next release PR, and
a version bump commit in a normal PR is a merge-conflict mine for the automation. If a release
must be cut immediately (no `feat:`/`fix:` commits), use `workflow_dispatch` on `release.yml`
with an existing tag or a `release-as` in a release PR instead.

### Never make the Release a draft

`release-please-config.json` must not set `"draft": true`, and `release.yml` must not set
`releaseDraft: true`. It reads like a safety measure — don't publish until the installers are
attached — but it breaks the pipeline in two ways at once:

1. **A draft Release has no git tag.** GitHub only creates the `v*` ref when the release is
   published, so `release.yml` (`on: push: tags`) never fires. Installers only ever got built
   because someone published the draft by hand.
2. **release-please cannot see its own draft releases.** Its "what did I release last?" lookup
   skips drafts, so every subsequent run finds no previous release, walks the entire history
   from the first commit, and opens a release PR with a version derived from scratch.

The repository ran this way for a week and the symptom was unmistakable: merging one feature
branch produced a chain of release PRs proposing 0.2.1, then 0.3.0, then 0.4.0, each merge
rewriting `CHANGELOG.md` with the whole history again. The run logs show it directly —
`Expected 1 releases, only found 0` followed by `Considering: 452 commits` while the release
was a draft, versus `Considering: 6 commits` on the next run once it had been published.

The accepted trade-off is that a Release is public from the moment its release PR merges and
stays empty until the matrix build finishes (roughly fifteen minutes). If that window ever
needs closing, close it by gating the *tag*, never by drafting the Release.

### Updater signing (release builds depend on it)

`bundle.createUpdaterArtifacts` is on, so every release build also emits the update package
and a detached minisign `.sig`, and `tauri-action` merges them into a `latest.json` on the
Release. The app verifies that signature against `plugins.updater.pubkey` in
`tauri.conf.json`.

**The bundler fails the build when a pubkey is configured but no private key is available**,
so `release.yml` needs two repository secrets:

| Secret | Value |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | full contents of the minisign private key file |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | that key's password (empty string if it has none) |

The keypair is generated once with `npm run tauri signer generate -- -w <path outside the
repo>`. The private key never enters the repository, and it must not be rotated casually:
clients that already trust the old public key will reject packages signed by a new one, so a
rotation requires shipping a build with the new pubkey *before* any release is signed with it.

`Cargo.lock` contains an array of packages with many `version` fields, so release-please's
generic TOML updater cannot reliably target only LinkGauge's root package. After
release-please runs, `release-please.yml` resolves any open bot release PR, checks out its
branch, runs `cargo metadata` to synchronize the lockfile from `Cargo.toml`, and commits the
result back to the same PR. Keep this post-processing step when changing release automation.

`latest.json` is fetched from `releases/latest/download/`, which only resolves for a
**published** release. release-please publishes the Release when its release PR merges, so
the endpoint resolves while `release.yml` builds and uploads the updater artifacts.

**This also breaks local packaging.** `npm run tauri build` on a developer machine hits the
same "public key but no private key" failure, because the signing key is a CI secret. To
produce a local installer, either point the bundler at your own throwaway key —

```bash
export TAURI_SIGNING_PRIVATE_KEY="$(cat /path/to/throwaway.key)"   # PowerShell: $env:TAURI_SIGNING_PRIVATE_KEY = ...
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
npm run tauri build
```

— or, when you only need the compiled binary and not an installer, skip bundling entirely with
`npm run tauri build -- --no-bundle`, which never reaches the signing step. Do not commit a
change to `createUpdaterArtifacts` or `pubkey` to work around a local build; releases depend on
both.

### Updater behavior on the client side

`src/composables/useUpdater.ts` owns the flow, and it is deliberately split into three
user-visible steps — **check → download → install** — rather than one:

- The startup check (5 s after mount, `main` window only) **only checks**. Finding a version
  lights a red dot on the About button; it never downloads tens of megabytes on its own, and
  it never throws a modal at a user who just launched the app.
- Downloading is an explicit click in the About dialog; installing is a second click in the
  "restart now" modal. `beforeInstall` stops a running client queue and server first, because
  the installer terminates the process.
- `updater_supported` (in `lib.rs`) reports whether *this* install can replace itself: on
  Linux that means the `APPIMAGE` env var is present, so `deb` / `rpm` users are pointed at
  the Releases page instead of downloading a package that `install()` would reject.
- Every window listens to the `update-state` broadcast (stage + version, never progress —
  progress changes per chunk, see the sync-bundle rule above), so a check or download in one
  window suppresses a duplicate in another.

## Housekeeping

- Both READMEs are maintained in parallel — a feature that changes behavior updates the
  English and the Chinese one, and the feature list, command table and project tree stay in
  sync with reality.
- New third-party dependencies get an entry in `THIRD-PARTY-NOTICES.md`.
- Work on a feature branch (`feat/…`, `fix/…`, `chore/…`), not `master`.
- Never commit `node_modules/`, `dist/`, `src-tauri/target/`, or `.zcode/`.
