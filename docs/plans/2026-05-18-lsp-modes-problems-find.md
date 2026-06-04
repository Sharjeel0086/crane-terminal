# LSP Modes, Problems Pane, Find in Files

Status: proposed
Date: 2026-05-18

Covers three related capabilities that all key off "give the user signal across the whole project without melting their laptop when 10+ Workspaces are open":

1. Per-Workspace LSP mode toggle (Off / Check Workspace / Live LSP)
2. Global Problems Pane (mirrors the Changes pattern)
3. Find in Files modal with inline preview (⌘⇧F)

---

## 1. Per-Workspace LSP modes

### Why
Current behavior: opening any file in a Workspace triggers `did_open` against a live LSP server scoped to `rootUri`. Server (rust-analyzer, tsserver, pyright) then indexes the entire crate/project, including transitive imports. Cost scales by Workspace, not by file. At 10 Workspaces × rust-analyzer ≈ unusable.

### Three-way mode, per-Workspace

| Mode | Behavior | Server lifecycle |
|---|---|---|
| **Off** (default for new Workspaces) | No diagnostics. File Tab squiggles inactive. | Never started. |
| **Check Workspace** | One-shot batch run on demand. Streams diagnostics into Problems Pane, exits when done. | Spawned on button press, exits on completion. |
| **Live LSP** | Squiggles on type, hover, goto-def. Joins LRU pool. | Started on first `did_open` in this Workspace; evicted by LRU or on Workspace close. |

### LRU pool
- Cap N live LSP servers globally (default `N = 3`, configurable in `crane.yaml`).
- When the user touches a Workspace whose server isn't live and the pool is full, evict the least-recently-used server (graceful `shutdown` → `exit`).
- LRU is per-language-server, not per-Workspace: two Workspaces both in Live mode running rust-analyzer = two slots.

### Batch checker (Check Workspace mode)
- Detect language from Workspace contents:
  - `Cargo.toml` → `cargo check --message-format=json`
  - `package.json` + `tsconfig.json` → `tsc --noEmit --pretty false`
  - `pyproject.toml` / `requirements*.txt` → `pyright --outputjson`
- Run as `std::process::Command` on a dedicated worker pool (bounded, 2 concurrent across all Workspaces — clicking "Check" on 10 Workspaces queues them, doesn't fork-bomb).
- Stream stdout line-by-line via `crossbeam` channel, parse to `Diagnostic`, push to shared store.
- Single worker pool lives in `src/lsp/batch.rs` (new).

### Threading
- One reader thread per live LSP server (existing pattern in `src/lsp/server.rs`).
- One bounded batch-checker thread pool (2 workers) for Check Workspace runs.
- Shared `Arc<RwLock<DiagnosticsStore>>` keyed by `(workspace_id, PathBuf)`.
- UI reads on `update()`, never blocks on LSP I/O.

### Persistence
Mode stored per-Workspace in `crane.yaml`:
```yaml
workspaces:
  - project: crane
    branch: main
    lsp_mode: live
  - project: OneVibe
    branch: feat/block-foundation
    lsp_mode: off
```

### UI affordance
- Workspace header row in Left Panel gets a small mode indicator after the branch name:
  - `Off` — dim grey, no icon
  - `Live` — accent dot
  - `⏳` — spinner while a Check is running
- Right-click Workspace row → mode submenu.
- First time a file opens in an `Off` Workspace, show a one-frame inline hint: "Run check on this Workspace? [Check] [Enable Live LSP]". Dismissible, doesn't repeat.

---

## 2. Problems Pane

### Placement
Phase 1: third tab in Right Panel next to Changes / Files. Same render pattern as `src/ui_right.rs`.
Phase 2 (if users ask): promote to `PaneContent::Problems` so it can live in any Layout cell.

### Layout
Tree, three levels:
```
Workspace                    E 3  W 12  I 4
  └─ src/foo.rs              E 2  W 1
       ├─ line 42  E  cannot find type `Bar` in scope
       └─ line 87  W  unused variable `x`
  └─ src/bar.rs              E 1  W 11
```

- Header tab title: `Problems N` where N = total errors across all Workspaces.
- Per-Workspace row: collapsible, shows `E / W / I` counts and mode-state (`Off`, `Live`, `⏳ checking…`).
- Workspaces in `Off` mode collapse to a single dim row — calm panel with 10+ projects.
- Click a diagnostic → opens the file in the active Tab's Files Pane and jumps to line.

### Data model
```rust
pub struct DiagnosticsStore {
    by_workspace: HashMap<WorkspaceId, WorkspaceDiagnostics>,
}
pub struct WorkspaceDiagnostics {
    by_file: HashMap<PathBuf, Vec<Diagnostic>>,
    last_updated: Instant,
    state: DiagState, // Off | Live | Checking
}
```

Writers: LSP reader threads + batch-checker workers. Reader holds `Arc<RwLock<…>>`.

### Counts on File Tab stay
The existing per-file squiggle + count in the File Tab is untouched. Problems Pane is purely additive — aggregates the same data.

---

## 3. Find in Files modal

### Shortcut
⌘⇧F (matches JetBrains / VSCode muscle memory; no clash with existing Crane bindings).

### Engine
Shell out to `rg --json`. Bundle the binary in the `.app` Resources dir; check `$PATH` first, fall back to bundled. Pattern matches `lsp/downloader.rs` for shipping external binaries.

Why ripgrep over a pure-Rust crate:
- Respects `.gitignore` out of the box.
- File mask, regex, case, word toggles map 1:1 to flags.
- Streaming JSON output — UI stays responsive on massive matches.
- 5MB binary, acceptable bundle cost vs. ~30s extra incremental compile if we statically link `grep` crate.

### Modal layout
Follows `src/modals/` pattern. Two stacked regions:

```
┌─────────────────────────────────────────────────────────┐
│ Find in Files   100+ matches in 35+ files    File mask:│
│ 🔍 hello                              [Cc] [W] [.*] [×]│
│ [In Project] [Workspace] [Directory] [Scope]            │
├─────────────────────────────────────────────────────────┤
│ results list (one line per match, file+line right)      │
│ tools:text="hello info test" />        fragment.xml 80 │
│ mBinding.tvUserName.text = "Hello,…    ChatGptFrag 193 │
│ …                                                       │
├─────────────────────────────────────────────────────────┤
│ preview of focused match (syntect, ±10 lines, hit hl)   │
└─────────────────────────────────────────────────────────┘
[ ] Open results in new tab        ⌘↵  [Open in Find Window]
```

### Scope buttons
Map to Crane's hierarchy (`Module` doesn't fit — rename to **Workspace**):
- **In Project** — all Workspaces of the active Project
- **Workspace** — just the active Workspace's worktree
- **Directory** — picker
- **Scope** — saved named scopes (later)

### Streaming
- Worker thread spawns `rg --json --max-count 0 …`.
- Reader thread parses JSON, pushes `Match` rows via `crossbeam` channel.
- UI debounces input ~150ms before relaunching rg.
- Cap rendered list at 1000 matches with "more…" sentinel; rg keeps producing into the store but UI stays snappy.

### Preview
- Reuse `views/file_view.rs` syntect logic in read-only mode.
- Render ±10 lines around the focused match, highlight hit span.
- File is read on focus change, not on every keystroke.
- Real File Tab opens only on Enter (or ⌘Enter for "open in new tab").

### Pin → Pane
The 📌 in the JetBrains screenshot detaches the modal into a regular Pane. Maps to `PaneContent::SearchResults` — same trick used for Diff Pane today. Lets the user keep search results visible while editing.

### File mask + toggles
- `*.java` style mask → `--type-add` + `--type` flags or `--glob`.
- `Cc` → case-sensitive (default rg is smart-case; toggle forces sensitive).
- `W` → `--word-regexp`.
- `.*` → `--regex` (default is fixed-string).

---

## File changes

### New
- `src/lsp/batch.rs` — batch checker worker pool, language detection, diagnostic streaming.
- `src/state/diagnostics.rs` — `DiagnosticsStore`, `Diagnostic`, `WorkspaceDiagnostics`.
- `src/ui/problems_pane.rs` — Problems Pane renderer (Right Panel tab).
- `src/modals/find_in_files.rs` — Find in Files modal.
- `src/search/ripgrep.rs` — rg subprocess + JSON parser.

### Modified
- `src/lsp/mod.rs` — LRU pool, mode awareness, gated `did_open`.
- `src/state/state.rs` — `lsp_mode` field on `Worktree` (→ `Workspace`).
- `src/state/settings.rs` — `crane.yaml` schema for `lsp_mode` and LRU cap.
- `src/ui_right.rs` — Problems tab.
- `src/ui_left.rs` — mode indicator on Workspace row, right-click submenu.
- `src/shortcuts.rs` — ⌘⇧F → open Find in Files modal.
- `src/modals/mod.rs` — register Find in Files modal.
- `src/state/layout.rs` — `PaneContent::SearchResults` (for pinned Find).

### Deps
- `crossbeam-channel` — likely already in tree; confirm.
- Bundle ripgrep binary in `Makefile`'s app-bundle step (Resources dir).

---

## Build sequence

1. `DiagnosticsStore` + plumbing (no UI yet) — wire existing per-file LSP diagnostics through it; verify squiggles still work.
2. Problems Pane (Phase 1, Right Panel tab) reading from store.
3. LSP mode enum + persistence + LRU pool. Default new Workspaces to Off; migrate existing config to Live so nothing visibly changes for current users.
4. Batch checker (Cargo first, then tsc, then pyright).
5. Find in Files modal — bundle rg, build modal, wire ⌘⇧F.
6. Pin → `PaneContent::SearchResults`.

Each step ships independently. No flag-guarded half-states.

---

## Out of scope (deferred)

- Saved scopes for Find in Files (the JetBrains "Scope" tab beyond the four built-ins).
- Quick-fix / code-action integration in Problems Pane (jump-to is enough for v1).
- Multi-language batch check in one Workspace (e.g. a repo with both Rust and TS — pick the dominant manifest for v1).
- Cross-Workspace symbol search (different feature; ripgrep handles text already).
