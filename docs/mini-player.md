# Mini-Player: PR Activity Widget

Design doc for [issue #74](https://github.com/Besendorfer/marrow/issues/74).

## Goal

A Spotify-style "mini player" that surfaces PR activity quickly and cleanly — new
comments, status changes, new commits, review requests — across the PRs you care
about. It works alongside the full Marrow window, can optionally **float** as a
separate always-on-top window, is **resizable**, and stays beautiful and
**responsive at any size**. Updates are **near real-time**.

Crucially, it can watch PRs in **orgs/repos where you are not a requested
reviewer** — via saved GitHub searches — not just PRs assigned to you.

## Why it's worth building deliberately

The two surfaces (in-app dock + floating window) are built **in parallel** off one
shared component. That only works if the contract between backend and frontend is
pinned down first: the activity event shape, the seen-state schema, and the watch
config. This doc is that contract.

---

## Architecture overview

```
                 ┌──────────────────────────────────────┐
   GitHub APIs   │            Rust background watcher      │
  ┌───────────┐  │  ┌────────────┐  diff vs  ┌──────────┐ │
  │ /notifs   │─▶│  │ Notifs poll│──────────▶│activity. │ │
  │ /search   │─▶│  │ Search poll│  seen-st  │  json    │ │   emit "pr-activity"
  │ check_pr  │─▶│  │ Focus diff │◀──────────│ (per-PR) │ │──────────────┐
  └───────────┘  │  └────────────┘           └──────────┘ │              │
                 └──────────────────────────────────────┘                │
                                                                          ▼
                          ┌───────────────────────────┐   ┌───────────────────────────┐
                          │  Main window               │   │  Floating widget window     │
                          │  <ActivityWidget> (dock)   │   │  <ActivityWidget> (frameless│
                          │                            │   │   transparent, always-top)  │
                          └───────────────────────────┘   └───────────────────────────┘
                                   actions (open / snooze / mark-seen)
                                   ──────────────▶ Tauri commands + relevantreviews:// deep link
```

**Source of truth is Rust.** A single background watcher merges all three GitHub
sources, de-dupes by PR URL, diffs against persisted seen-state, and emits a
`pr-activity` event to **all** windows. Both UI surfaces are pure subscribers. This
sidesteps the fact that the frontend has no shared state store (all state currently
lives in `app/src/App.tsx`), and it keeps activity flowing even when the floating
widget is the only focused window.

---

## The three activity sources (hybrid)

| Source | Covers | Cadence | Cost |
|---|---|---|---|
| **Notifications API** (`/notifications`) | PRs you're *involved in* (requested, mentioned, commented, subscribed) | Driven by the `X-Poll-Interval` response header; `If-Modified-Since` → 304s are free | Very cheap |
| **Search API** (`/search/issues?q=…`) | **Any** repo/org/author/label — the watch-list unlock | ~60–120s, ETag conditional requests | ~30 req/min budget |
| **Focused diff-poll** (`check_pr_updates`, existing) | Precise CI / review-state on the PR you're actively viewing | 30–60s while focused | Targeted, one PR |

The Notifications API is the near-real-time channel: GitHub literally tells us the
minimum poll interval and 304-responses don't count against the rate limit. The
Search API is what lets us watch arbitrary repos/orgs. The focused diff-poll reuses
what already exists for high-fidelity status on the PR in front of you.

### Watch lists

A first-class config concept — a small set of saved GitHub searches:

```jsonc
// part of marrow config
"watches": [
  { "id": "acme-web", "label": "Acme web", "query": "is:pr is:open repo:acme/web -is:draft" },
  { "id": "my-org",   "label": "Acme org", "query": "is:pr is:open org:acme review-requested:@me" }
]
```

Same query language as GitHub's own search bar. Bounded: we cap result counts,
paginate sensibly, and **surface what was truncated** rather than silently hiding
PRs. Each watch maps to a `reason` on the PRs it returns.

---

## Data contract

### Seen-state — `~/.config/marrow/activity.json`

Keyed by PR URL, independent of which source surfaced the PR:

```jsonc
{
  "https://github.com/acme/web/pull/42": {
    "last_seen_at": "2026-06-28T18:00:00Z",   // when the user last opened/acked it
    "last_comment_count": 7,
    "last_head_sha": "abc123",
    "last_review_state": "changes_requested",
    "last_ci_state": "success"
  }
}
```

"New activity" = any field differs from the live fetch. Opening a PR (or an explicit
mark-seen) writes the current values back, clearing its unread state.

### `pr-activity` event payload (Rust → all windows)

```jsonc
{
  "items": [
    {
      "pr_url": "https://github.com/acme/web/pull/42",
      "number": 42,
      "repo": "acme/web",
      "title": "Fix flaky upload test",
      "author": { "login": "octocat", "avatar_url": "…" },
      "updated_at": "2026-06-28T18:42:00Z",
      "reasons": ["review-requested", "watching:acme-web"],  // can be multiple
      "deltas": ["new-comment", "ci-failed"],                 // what changed since seen
      "ci_state": "failure",
      "review_state": "pending",
      "unresolved_threads": 2,
      "draft": false,
      "unread": true
    }
  ],
  "truncated": { "acme-web": 12 },   // counts dropped per source, for honest UI
  "fetched_at": "2026-06-28T18:42:05Z"
}
```

`reasons[]` and `deltas[]` are the two arrays that drive the UI's "why is this here"
and "what's new" affordances.

---

## UI: one component, four layout modes

The three screenshots in the issue are **not three features — they're one component
at three sizes.** Driven by **CSS container queries** (responding to the widget's own
box, not the viewport), with a spring/ease transition on layout change so resizing
feels alive.

| Mode | Footprint | Issue screenshot | Content |
|---|---|---|---|
| **Pill** | tiny | — | Tier 0: attention count + pulse on new activity |
| **Bar** | wide & short | 633×83 | Tier 1 focused PR + activity ticker + transport |
| **Card** | small square | 343×219 | Tier 1 rich + "N others" |
| **List** | tall & narrow | 247×888 | Tier 1 header + full Tier 2 feed |

### Glance tiers (progressive disclosure)

- **Tier 0 — the pulse:** count of PRs needing attention + subtle animation on new
  events. Always visible, even at pill size.
- **Tier 1 — "now playing":** the single highest-priority item (active PR, or freshest
  activity). Title, repo#, author avatar, and *what changed*.
- **Tier 2 — the feed:** PRs sorted by recent activity. Each row: avatar · title ·
  status glyph cluster (CI ●✓✗ · review state · unresolved-thread count · draft) ·
  unread dot · time-ago · source reason chip.
- **Tier 3 — transport + filter:** prev/next through the attention queue, "play" =
  open in main window, snooze/pin, filter chip (*needs my review* / *has updates* /
  *all* / per-watch).

### Looking "super nice"

- Reuse `app/src/styles.css` design tokens for theme cohesion: `--bg-primary #0d1117`,
  `--accent #58a6ff`, the `--risk-*` and `--diff-*` palettes for status colors,
  `--font-sans` / `--font-mono`.
- Motion: equalizer-style activity indicator (the Spotify nod), avatar stacks, soft
  pulse on new events, eased layout transitions on resize.
- Floating window chrome: frameless + transparent + rounded corners + macOS vibrancy
  blur; small min-size so it can shrink to the pill.

---

## Two surfaces, one component

Build `<ActivityWidget>` fully self-contained and event-driven — **no dependence on
`App.tsx` internals.** It takes activity items as props/subscription and emits
intent callbacks (open, snooze, mark-seen).

- **In-app:** mount as a collapsible dock panel inside the main window (currently the
  single `Marrow` 1400×900 window in `tauri.conf.json`).
- **Floating:** a second `WebviewWindow` — frameless, transparent, always-on-top,
  `resizable: true` with a small min-size — loading a `widget.html` entry that mounts
  the same component.
- **State sync:** Rust emits `pr-activity` to both windows; actions return via Tauri
  commands and the existing `relevantreviews://` deep link to open a PR in the main
  window.

---

## Rate limits & "near real-time"

- Notifications: poll-interval-header-driven + `If-Modified-Since`; quiet periods are
  nearly free.
- Search: slower beat, ETag conditional requests; the 30 req/min ceiling means we
  batch/cap watch queries.
- **Adaptive cadence:** faster when the app is focused or a PR is "hot," slower when
  idle.
- **Honest truncation:** if a watch returns more than the cap, the UI shows "+N more"
  rather than pretending it covered everything.

---

## Build order

Each step is independently useful:

1. **Foundation** — seen-state (`activity.json`) + watch config + Rust background
   watcher emitting `pr-activity`. Start with diff-poll + Search; layer Notifications.
2. **Shared component** — container-query `<ActivityWidget>` (pill → bar → card →
   list), proven in-app as a dock panel.
3. **Floating window** — promote the same component into a resizable frameless
   always-on-top `WebviewWindow`.
4. **Polish** — adaptive cadence, watch-management UX, motion/vibrancy.

## Open questions

- Notifications API coverage: does it carry enough signal (CI? review state?) or is it
  mainly a "something changed, go look" trigger that we enrich via diff-poll?
- Watch-list management UX: raw GitHub search string vs. structured builder.
- Snooze semantics: per-PR mute until next change, or time-based?
