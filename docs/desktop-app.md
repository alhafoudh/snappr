# The desktop app

The Tauri app: `src-tauri/` is the shell, `src/` the React frontend, and
`snappr-core` does the image work. `CLAUDE.md` owns the pipeline and the
image-quality invariants; this file owns everything around it.

**Read the source for the reasoning.** Nearly every decision here is already
argued at the place it lives, with the bug it prevents and the measurement
attached - `lib.rs`, `settings.rs`, `photo.rs`, `overlays.rs`, `mac.rs`, and
core's `watcher.rs`, `worker.rs` and `preview.rs` all carry their own comments, as
do `usePhotos.ts`, `ipc.ts` and `App.tsx`. This document deliberately does not
restate them. What it records is the material that spans files and therefore sits
in no file's comments, plus the map of which file to open.

## The division of labour

`lib.rs` states it at the top and it is the line to hold:

> This side owns the window, the settings, the persisted index, printing and
> sharing, and the drops. **`snappr-core` owns the pipeline and the watch**: one
> photo in, an overlay PNG and three previews out. Nothing about image processing
> lives on this side.

The corollary is that `snappr-core` knows nothing about the shell. The CLI and the
app drive the same `Overlay`, and anything that would make the pipeline aware of a
window is going the wrong way.

**There was a process boundary here until 2026-09-03**, and the shape of this file
still shows where it was. The pipeline was a Ruby child on the far end of an
NDJSON pipe; `sidecar.rs` and `runtime.rs` are gone, along with the frame grammar,
the reader and stderr-pump threads, the 120-second request timeout, the `pending`
queue for drops that arrived before the child was up, and the seven `Pipeline*`
problems that described a child in one state or another.

## The worker

Image work goes through `core::worker`: **one thread, one queue.** Callers submit
a closure and wait on a channel for their own result.

**That queue used to be free, and losing it would have been a silent regression.**
The Ruby child accepted one request at a time, so the shell could spawn an
unbounded thread per photo and they all queued on the pipe - `server.rb` called
that the deduplication mechanism. Delete the child without replacing the queue and
a busy AirDrop session composites four 12MP frames at once, each holding a 10-25 MB
output plus its source; memory is what gives out first, on the machine least able
to afford it. `jobs_never_run_concurrently` in `worker.rs` is the assertion that
keeps it.

Two things about the shape:

- **The queue knows nothing about what the work is.** It takes closures, so a
  composite, a preview and an overlay card all go through one lane without the
  module learning about any of them.
- **The config is read when a photo is *queued*, not inside the job.** So a photo
  composites with the overlays that were live when it arrived, rather than
  whatever the operator switched to while it waited. That is also why
  `set_overlay` only saves: there is no second copy of the settings to push to,
  which is what `configure` existed for.

Threads, now: the event thread (Tauri's, where every command body and every drop
runs), one `process` thread per photo, the watcher's poll thread, one boot thread,
one `overlays::refresh` thread, and the worker. The three sidecar-adjacent threads
are gone. **No command does image work on the event thread** - the commands are
synchronous `#[tauri::command]`s, which Tauri runs on the main thread, and they
are safe only because they all delegate.

**Startup is no longer a wait.** `RuntimeStatus.ready` used to become true about a
second after launch, once a Ruby interpreter had started and answered a handshake;
it is now one `Vips::init` and the only way it fails is a broken install. The
`pending` queue that held drops arriving during that second - with its own lock
discipline against the boot thread - is gone, because the window it protected no
longer exists.

**If the worker thread dies, every later photo fails with
`ImageToolsUnavailable`** and the recovery story is the same as it was: one message
saying to restart snappr. That is better than it was - a dead Ruby child left
`ready` claiming `true` and blocked each caller for the full 120-second timeout -
but it is still not supervision.

## Invariants that span the whole app

**Events are not replayed, so every channel is pull-then-subscribe.** The pipeline
is ready within a millisecond of launch, long before the webview has attached its
listeners, so a UI that only subscribed would wait forever for a message already
sent. Hence `RuntimeStatus` is *stored as well as emitted* (`lib.rs`), and
`usePhotos` subscribes **before** it reads, awaiting the subscriptions because
`listen()` resolves on a microtask. The margin used to be a second and is now
vanishing, which makes this *more* necessary rather than less.

**Nothing on the wire is written twice.** `src/bindings/` is generated from the
Rust structs by `cargo test`, and `src/ipc.ts` is the only file that names a
command or an event. `src/types.ts` used to hand-mirror nineteen `Settings`
fields, `Photo`, `Exif` and `OverlayCard` against structs nothing checked it
against, and three had already drifted: `Photo.orientation` was narrowed in
TypeScript to a union Rust did not guarantee, `mac::Anchor` had no type at all,
and a `Settings` field added in Rust was invisible. Adding a field is now a
compile error until the frontend is regenerated.

**Rust is the sole owner of what the filmstrip contains.** The frontend never adds
or removes a photo on its own. Even clearing goes out as a command and comes back
as a `cleared` event, which is a separate channel from `photo` for a concrete
reason: a removal has no record left to travel on.

**Processing failures ride the photo record; command failures throw.** Two classes,
two renderers, and they are not interchangeable:

| failure | travels as | rendered by |
|---|---|---|
| compositing a photo | `photo.error` / `photo.warnings` | Stage callout + Try again, filmstrip dot |
| a command the operator invoked | a rejected `invoke` | `problem` state in `App.tsx` |
| discovered after the command returned | the `notice` event | the same info line |

This is why `reprocess` is the one call in `usePhotos` that does not throw - its
failure is already coming back on the record.

**Two ordering conventions coexist, on purpose.** `usePhotos` keeps arrival order
and its "newest" fallbacks read the *last* element; `App.tsx` derives `ordered` as
newest-first for display, so an arriving photo lands at the left-hand end where
the operator's eye already is. Reordering inside `usePhotos` - the obvious
tidy-up - silently inverts those fallbacks.

**`ordered` is that arrival order reversed, and deliberately not a sort on
`added_at`.** The sort was there first and was wrong: `added_at` is whole seconds,
so photos landing in the same second tie, and a stable sort leaves a tie in
`photos` order - oldest first, inverted against the rest of the strip. A batch of
arrivals therefore read backwards *inside themselves*, and since the arrow keys
follow `ordered`, Right from the newest cell jumped to the oldest photo of the
batch. Arrival order is the ground truth on both sides - `Index::upsert` pushes,
`list_photos` sorts nothing, `merge` appends and replaces in place - and `added_at`
is derived from it, so reversing is the same ordering at full resolution with no
ties and no dependence on clock granularity. A reprocessed photo keeps its slot
either way, which is why `process` preserves its `added_at`.

**An arrival takes the selection only from the head of the strip.** Following is
the kiosk's point - a photo AirDropped while nobody is at the keyboard has to show
itself - but it must not yank the operator out of a photo they navigated to, and it
used to: the selection moved under them and, because the strip's arrow keys carry
on from wherever focus is, their place was gone. `usePhotos` keeps an `atHead` ref
and only follows when the selection is still the newest record.

The test is "at the head" rather than "not touching the keyboard" because it
re-arms itself by arrowing back to the newest cell, reads the same for the mouse and
the keyboard, and leaves the operator at the head after each followed arrival, so an
unattended run behaves exactly as it always did. **Focus was the other candidate and
is wrong**: clicking a cell leaves focus inside the strip for the rest of the
session, so a focus test would switch following off for good on the first click.

## The preview tier

`core::preview` writes three JPEGs per photo: a filmstrip thumb, a preview of the
composite, and a preview of the untouched source for the before/after toggle.

- **They exist because the webview cannot be shown the real output.** Tauri's
  asset protocol reads a whole file into memory per request, and the prints are
  10-25 MB and 16-bit when the source was HEIC.
- **The thumb is made from the original, not the composite.** At 132x84 the
  branding is a fifth of the cell and identical on every one, so a strip of
  composites is a strip of near-identical logos.
- **They are converted to sRGB**, because a webview reads an untagged JPEG as sRGB
  and iPhone photos are Display P3.
- **They are derived from the finished files on disk**, deliberately not spliced
  into the compositing chain - that would put the bit-identical invariant at risk
  to save a fraction of a second.
- **The cache key includes the composite, not just the source.** This one was a
  real bug: keyed on the source alone, clearing a photo, picking a different
  overlay and dropping the same file back in reproduced the identical key, so the
  webview served the overlay the operator had just replaced out of its own memory
  cache while the PNG on disk was correct. The key is SHA256 over path and
  nanosecond mtime of both files.
- **Superseded sets are deleted by the shell**, in `discard_superseded`, because
  the pipeline has no index and cannot tell an old preview of this photo from a
  current preview of another. It runs *after* `publish`, so a crash in between
  leaks three JPEGs rather than blanking a live cell.

Where they go and why it is not free: see "Previews never go in the drop folder"
in `CLAUDE.md`.

## Finding libvips

`core::vips::init` is the whole of it now, and there is no layout to detect: the
binary links libvips and dyld resolves it through the two `LC_RPATH` entries
`src-tauri/build.rs` bakes in - `Contents/Frameworks` in a bundle, `build/vips` in
development. Which libvips gets *linked* is a per-profile decision, in
`docs/bundled-runtime.md`.

There used to be a `Runtime::detect` here that chose between a development layout
and a bundle layout, spawned a child with four environment variables, and probed
with `is_dir` rather than `exists` because `target/debug` already contained a file
called `snappr`. Its bundle branch never once executed. What survives from it:

- **`LIBHEIF_PLUGIN_PATH` still points at a directory that deliberately does not
  exist**, set before `Vips::init` rather than passed to a child. libheif
  opportunistically dlopens everything in its plugin directory, dragging in a
  second libheif plus ffmpeg, opus, vpx and x264. libde265 is linked in, so
  decoding needs none of it; point the scan somewhere empty and it finds nothing.
- **`LD_LIBRARY_PATH` is gone and should not come back.** It only ever worked
  because Ruby's FFI read that variable itself and joined the paths by hand;
  `DYLD_*` means nothing here, hardened runtime strips it, and `LC_RPATH` is the
  mechanism that does not need an exemption in `entitlements.plist`.

## The watcher

`crates/snappr-core/src/watcher.rs`, used by the app and by `mise run watch`.
There were two - the app's, and a `listen`-based one in Ruby for the CLI - because
the CLI predated the app. One is the point of the rewrite; the CLI inherited the
better implementation in the process, gaining `already_there`, `Presence` and
stall recovery.

Four behaviours, each easy to get wrong in a way that only shows at an event:
**non-recursive** (the 63-seconds-of-pinned-core fix), **arrivals only**, **filter
before touching the disk**, and **debounce on size and mtime** because AirDrop
writes non-atomically and a file that exists is not a file that is finished. The
first two are argued in full in `CLAUDE.md` under Hard constraints.

The second is the one to read before touching this file. **Event kinds cannot
substitute for it** - FSEvents accumulates flags per path, so `Create(File)` is
reported for a chmod, an append and a deletion alike, even on a file that predates
the watch. So `watcher.rs` ignores kinds and keeps the state instead:
`already_there` for what was in the folder when the watch began, and `admit` for
the rule. `listen` gave the Ruby side the same answer for free by diffing
directory snapshots, which is why that half never had to solve this. The live
tests in that file, `#[ignore]`d and run with `cargo test -- --ignored`, exist
because this is not catchable any other way.

`watcher::refusal` is the single implementation of the candidate rule and
`is_candidate` delegates to it, so the filter and the explanation it gives the
operator cannot drift. Two constraints on it, both from the hot path - it runs on
every filesystem event in a folder with ~1.9k top-level entries, where refusal is
the *common* answer:

1. **No payload.** A `String` there would allocate on every rejected event.
2. **No I/O.** A `stat` per event is exactly what the non-recursive watch exists
   to avoid. So an extensionless path comes back as `Unknown`, and the drop path -
   which has one file, not 1.9k - upgrades it to `IsAFolder` with a single
   `is_dir()`.

Both constraints are on `refusal` specifically, not on everything downstream of an
event. `presence` does stat, and is allowed to: by the time it runs the event has
already been accepted as an arrival naming a candidate file, which is the rare
case, and the poll loop has to stat that path anyway.

`Arrival::Abandoned` is reported rather than only logged because no record has
been published for that file yet, so a stalled AirDrop had nothing on screen and
used to disappear in silence.

**It is not the last word on that path, though.** `Arriving::stalled` keeps a file
under observation after the message has gone out, so a very slow transfer that
finishes long after the 30-second timeout still arrives as a `Settled` and
`process` replaces the failed record in place - which is what the comment beside
the failed cell in `lib.rs` promises. That has to be polling rather than events:
the writes that would finish the transfer are not arrivals. Two rules come with
it. A stalled entry is dropped once its path is gone, so a cancelled AirDrop the
operator deletes does not sit in the map for the length of an event; and a stalled
entry must never be what holds the poll thread open, or every change of folder and
every toggle of the watch leaks one.

## Settings on disk

Plain serde into `app_config_dir()/settings.json` rather than
`tauri-plugin-store`: the shell owns settings, so this way the file is
human-readable JSON with no IPC round-trip and **no capability to grant**. The
capability file stays correspondingly tiny.

**`load` never refuses to start.** A kiosk that will not open because of a stray
character is worse than one that comes up on defaults. The `.broken` rename and
why its ordering matters are in `CLAUDE.md` under "Everything stays where it is".

**The overlay slot is the only field whose on-disk shape has ever changed**, and
it has a hand-written `Deserialize` for that reason. Since `load` answers any
parse failure by discarding the whole file, a field that cannot read its own
history costs the operator their watch folder, their printer and their paper name
too - not just their overlay. It reads the legacy `null` (as Sample) and the
legacy path string (as Custom) alongside the tagged form, `#[serde(alias)]` reads
the old key names, and the field names are what get written back, so the file
migrates itself on the first save. Going through `Value` first is what makes
try-this-then-that possible at all: a `Deserializer` cannot be rewound once an
inner call has consumed a token.

It is **adjacently tagged and deliberately not `untagged`** - `untagged` turns
every parse error into "data did not match any variant", which is the one message
you cannot act on from a log line.

**The overlay *library* is a new key beside the slots, not a third shape for
them.** `Settings::overlays` is a `Vec<Overlay>`, so it reuses the element
`Deserialize` that already reads every historic shape, and `#[serde(default)]` on
the struct covers the key being absent. Re-spelling `overlay_landscape` as a list
would have put the watch folder and the printer at risk to gain a panel; a new key
cannot.

Nothing had to learn any history for it either. `Settings::normalise`, run after
every successful parse, **derives the library an old file implies**: the sample
forced to the front, plus whatever either slot already pointed at. So a
`settings.json` written before the panel existed comes up with the sample and the
operator's own overlay already in the panel, with no version stamp and no
migration code.

The tests in `settings.rs` are the only Rust tests in the project, and their
thesis is stated in the module: the thing worth testing is not that the new shape
round-trips, it is that none of the old ones can take the watch folder down.

What the overlay states mean, and how they reach the pipeline, is in `CLAUDE.md`
under "The overlay SVGs"; what the panel does with them is under "The overlay
panel" below.

## Printing and sharing

Both are AppKit, both must run on the main thread, and both act on **the finished
overlay PNG - the exact bytes that go to paper** - never on the source or a
preview. `mac.rs` explains why neither alternative was taken: `Webview::print()`
exists but prints the DOM through WKWebView, so the photo is rasterised by the web
engine, which is the wrong pipeline for a codebase whose whole purpose is not
resampling the photo; and a Swift helper would be a second signed binary on the
notarization surface that still could not anchor a popover to our own view.

**Paper is selected by PPD name, never by size, and that was learned the hard
way.** The Canon SELPHY driver defines two papers with identical dimensions -
`Postcard` with ~3mm margins and `Postcard.Fullbleed` edge to edge, which macOS
shows as "Postcard Borderless". Asking for a custom size of those dimensions
resolves to the *first*, because it is the driver's default, and the print comes
back with a white border. The size is only a fallback for a printer that has no
paper of that name at all, and the driver is probed by setting the paper and
reading it back.

The print panel is forced to show Paper Size and Orientation, which the default
panel hides - that is exactly what the operator needs to see to confirm the
borderless paper before spending any.

Anything discovered *after* the command returned - an unreadable PNG, a printer
with no paper of the right size - goes out as a `notice`, because the closure runs
after the `Ok`. That is the only place notices come from.

## Drops

**Window drops**: Tauri owns them (`dragDropEnabled` is on), so the webview never
sees the drop and cannot navigate to the file, which is what it did while the flag
was off. The frontend only paints the target. The whole drop is one unit, so a
drop carrying several unusable files explains itself once rather than once per
file.

**A drop is routed by kind, never by where it landed.** `ingest_drop` partitions
the paths: `.svg` goes to the overlay panel, everything else down the photo path,
and a mixed drop is split. It is worth knowing why this is not a drop target on
the panel, because that is the obvious design and it cannot be built:

- Only Rust ever sees a drop's position, and **a Dock drop has no position at
  all** - it arrives through `RunEvent::Opened`. A panel-shaped target would be a
  promise the Dock could not keep.
- Routing by kind is strictly more forgiving anyway. An SVG dropped an inch to the
  left of the panel is added rather than refused, and there is no coordinate space
  to get wrong.

The frontend classifies the drag the same way, from the paths on the `enter`
event, and lights the panel or the Stage accordingly - so it still *reads* as a
drop target. `enter` is the only event carrying paths, so `over` deliberately
leaves the classification alone rather than recomputing it.

`watcher::refusal` is untouched by any of this and keeps its "no payload, no I/O"
contract: the split happens above it, so an SVG never reaches the filter that
would call it the wrong kind of file.

**Dock drops**: macOS delivers these to the running instance directly, so the
single-instance plugin is not involved, and a drop on the Dock icon **can be what
launches the app** - which is why the refusal alert is the OS's own and is raised
from Rust. A frontend modal cannot explain a drop from before the webview existed.
`show`, never `blocking_show`: this runs on the event thread.

The Dock only accepts a drop because `src-tauri/Info.plist` declares the photo
types under `CFBundleDocumentTypes`, by UTI, with the extensions as a fallback for
files with no UTI binding. `Viewer` + `Alternate` keeps snappr out of the
default-opener slot while still appearing in "Open With". It is kept in
`Info.plist` rather than `bundle.fileAssociations` so the UTIs can be named.

**Dock drops cannot be tested under `tauri dev`.** Launch Services refuses to
register a bare executable as an application (`lsregister` fails with -10811), so
the Dock has nothing to check a drop against however complete the embedded plist
is. Test with `npm run tauri build -- --debug --bundles app` and the `.app` it
produces.

A Dock drop can be what *launches* the app, and that used to need a pending queue
with a lock discipline on both sides - `ingest_one` held `pending` while it looked
at the sidecar, and the boot thread took the same lock before installing the child
and draining, so a path could not fall between the two. Both are gone: the
pipeline is up before the window is, so a drop is simply handled where it lands.

## The window

From `tauri.conf.json`, the settings that are decisions rather than taste:

- `assetProtocol.scope` is `$APPDATA/previews/**` and `$APPDATA/overlays/**` - the
  only reason previews and the overlay cards render. The directory choice and the
  scope entry are one decision in both cases, so moving `preview_dir` or
  `overlays::dir` breaks image loading silently.
- `titleBarStyle: "Overlay"` + `hiddenTitle` pairs with the
  `core:window:allow-start-dragging` capability: the frontend draws its own
  titlebar, which is what makes the drag region matter.
- `zoomHotkeysEnabled: false` - kiosk hardening; Cmd+/Cmd- would resize the UI.
- `backgroundColor: "#111111"` - no white flash before the webview paints.
- A locked-down CSP, with `style-src 'unsafe-inline'` as the one concession to
  Radix Themes.

The window-state plugin restores the frame the operator set up **and nothing
else** - a window quit while hidden comes back visible, and decorations stay
whatever the config says. So the flags are `SIZE | POSITION | MAXIMIZED |
FULLSCREEN`, never `StateFlags::all()`.

**`MAXIMIZED` and `FULLSCREEN` are in that list for the *size*, not because a
kiosk needs to reopen zoomed**, and the reason is a macOS quirk worth knowing:
`isZoomed` is derived from the frame rather than from a gesture, so a window
merely dragged out to fill the screen - the obvious kiosk setup - reports as
maximized. The plugin then skips saving the size while still tracking every move,
and the file ends up with a new position beside a stale size, so the window
reopens small at the right place. Recording the two states is what makes the saved
frame coherent again. `lib.rs` carries the full argument at the registration.

The single-instance plugin must be registered first, per its own documentation.

The **selection** is restored too, but it lives in the index rather than in
settings, because it is only meaningful against those records and is dropped with
them. A selection that did not survive the intactness filter is no selection, and
the fallback to the newest photo is frontend policy.

---

## The desktop UI

React 18 + Vite + TypeScript, and **Radix Themes 3 carries the whole look**.

**There is no stylesheet, by design.** `src/app.css` was deleted in the Radix
rewrite. Layout, colour, spacing, selection and status all come from Radix
components, their props and their tokens - a dark-only `<Theme>` in
`src/main.tsx`. Do not reintroduce an app stylesheet: the point is that there is
exactly one place the design comes from. Raw CSS survives only where Radix
genuinely cannot express the thing, and every one of those is a named constant or
carries a comment saying why:

- `index.html` sets `margin: 0` on `<body>`. Radix ships no html/body reset, and
  without it the `100vh` grid overflows by the browser's default 8px.
- `objectFit` is inline on the `<img>` elements, in `Stage.tsx`, `Filmstrip.tsx`
  and `OverlayPanel.tsx`. There is no Radix prop for it, and inline style on an
  `<img>` is what Radix's own examples do. The comment on the letterbox rule is
  load-bearing: the overlayed and original views have different aspect ratios by
  design, so the box needs a definite size or a portrait source stretches it. The
  panel's is `fill` rather than `contain`, because its box is already the render's
  own aspect ratio and the artwork has to land on the placeholder exactly.
- Pixel literals passed *as Radix props* (`height="84px"`,
  `columns="repeat(n, 132px)"`). Those props are typed `"enum | string"` for
  exactly this.
- `OverlayPanel.tsx` positions the photo-window placeholder with percentages from
  the fractions the pipeline reports, and sizes the card with `aspectRatio` from the
  overlay's own shape. Neither is a design choice: they are measurements of the
  file being drawn.
- `writing-mode: vertical-rl` on the collapsed rail's label. A `transform:
  rotate()` would leave the layout box unrotated and overflow the rail.

Two things that look like they could be simplified and cannot:

- **The filmstrip is `RadioCards`.** The selection ring, hover, focus ring,
  roving arrow-key focus and radiogroup semantics all come with it, which is most
  of what the old stylesheet was for. Because it owns the arrow keys while
  focused, the global keydown handler in `App.tsx` skips events originating
  inside a `[role="radiogroup"]` or `[role="group"]` - without that guard one
  arrow press moves the selection twice.

  Two consequences of it owning them - the first cost a bug, the second a
  disagreement between the two arrow paths:

  **It navigates from the focused item, never from the checked one.**
  `react-roving-focus` picks the next cell with
  `candidateNodes.indexOf(event.currentTarget)`, so a selection changed by anything
  but an arrow press - an arriving photo, a right-click - leaves focus behind on the
  old cell and the next arrow press steps from *there*, which reads as the selection
  teleporting into the middle of the strip. `App.tsx`'s `[selected]` effect closes
  that by moving focus to the selected cell, but only when focus is already inside
  that cell's own radiogroup: otherwise an arrival would snatch focus off the button
  the operator was reaching for.

  **`loop={false}`, because Radix loops and `step` clamps.** The strip has two arrow
  paths and they have to feel the same, so both stop at the ends. Wrapping from the
  oldest cell onto the newest is a jump across the whole session that nobody asked
  for.
- **`disabled` goes on `SegmentedControl.Root`, never on its Items.** The Item
  type removes the prop.
- **Never wrap a `RadioCards.Item` in a `Tooltip`.** Tooltip's trigger is
  `asChild` and puts its own `data-state` on whatever it wraps, and RadioGroup's
  item spreads incoming props *after* setting `data-state`
  (`react-radio-group/dist/index.mjs:94-98`), so the wrapper's `closed` wins and
  the checked styling never renders. It fails silently: the selected cell simply
  looks idle, which reads as "Radix selection is too subtle" rather than as a
  bug. The Tooltip goes inside the Item.

**The keyboard map**, all of it in one `window` keydown handler in `App.tsx`:

| Key | Does |
|---|---|
| `Left` / `Right` | next / previous thumb, stopping at both ends |
| `` ` `` | toggle overlayed against original |
| `Cmd+P` | print |

The arrows stop rather than wrap on **both** paths - `step` clamps, and the strip is
`loop={false}` to match it. See the `RadioCards` note above.

**Tab does nothing, anywhere in the app.** The handler calls `preventDefault` on
it unconditionally and steps nothing, so the browser's focus traversal is
suppressed outright rather than left to walk the toolbar. This is deliberate:
there is no form here to tab through, and every control is a single button
reached with the mouse. The arrows are the strip's navigation - Tab used to be a
second next/previous and no longer is, which is why the branch looks empty apart
from the `preventDefault`. Do not delete it as dead code, and do not "restore
accessibility" by deleting it either - the filmstrip is a radiogroup and is still
fully operable from the keyboard.

`` ` `` rather than Lightroom's `\` for the before/after toggle: it is under the
left little finger without a reach, and the gesture gets used dozens of times an
hour.

**The context menus are native** (`@tauri-apps/api/menu`), not Radix
`ContextMenu`. This is a macOS-only app and the OS gets placement, keyboard
handling and dismissal right for nothing.

A trap worth knowing before touching those menus: Tauri's `MenuItemPayloadKind` is
`#[serde(untagged)]` and tries `Check` **before** `Submenu`
(`tauri/src/menu/plugin.rs`), and `CheckMenuItemPayload.checked` is a
non-optional `bool`. So a plain `{text, checked, action}` object becomes a real
macOS check item with no ceremony - and **`checked` on a submenu object would
parse as a check item and make its `items` vanish**. Never put it there. Nothing
in the app uses a check item today; the panel replaced the menu that did.

### The titlebar carries nothing

`Toolbar.tsx` is the window's identity and the traffic lights' clearance, and
that is all. Everything it used to hold moved to where its subject is: the watch
toggle, the folder and the photo count into the zeroth filmstrip cell, the
Overlays button into the panel, and the before/after switch down into the Actions
row beside the buttons that act on the same photo. The bar had become the place
everything that was not a photo ended up.

Three numbers have to agree and there is one of each: `TOOLBAR_HEIGHT` is exported
from `Toolbar.tsx` because `App.tsx`'s grid row applies it and
`trafficLightPosition` in `tauri.conf.json` is set to centre the buttons against
it. Change the height and the `y` moves with it - roughly `(height - 16) / 2`.
The `x` there is the close button's own origin, so the clearance `Box` in the
header has to clear `x + 2 * 20 + 14`.

### The zeroth filmstrip cell

The watch toggle, the watched folder and the photo count live at the **head of the
filmstrip**, not in the titlebar. That is where the photos come from: `ordered` is
newest-first, so every arrival lands directly beside the control that decides
whether arrivals happen at all, and beside the count of what has arrived. In the
bar they were app chrome, which is not what a drop folder is.

The cell is **outside `RadioCards.Root` and outside the `ScrollArea`**, and both
halves matter. Inside the radiogroup it would have to be a radio item - selectable,
and in the arrow keys' roving focus. Inside the scroll area it would slide away
from the newest photo the moment the strip filled up.

It keeps the pill's **three states, which are the reason it exists**:
`watch_enabled` is what the operator *asked* for and is not the same as what is
happening. A watch can fail to start and leave the setting on, and saying
"Watching" then is a lie the bar told for a while. `RuntimeStatus.watch_error` is
the third state, and the click handler retries rather than toggling when it is set
- otherwise the retry the pill offers would *pause*.

### The overlay panel

Overlays are a **library**, not a picker: a collapsible column of cards down the
left of the photo, one per overlay, each with its own landscape and portrait
toggles. One overlay can be live for both, or for neither. Adding is a pick or a
drop; removing is on the card's context menu.

**The picture on each card is a check, not a decoration.** The card paints the
print's background, then a grey rectangle with a photo icon at exactly the window
the pipeline reported, then the render on top - which has that window punched out
of it, so the placeholder shows through. An overlay whose `#FF00FF` marker did not survive
its export puts the icon somewhere obviously wrong, or reports no window at all and
says so. Before this, that failure was invisible until a print came out wrong.

It only means anything because the render comes back **through librsvg, the same
renderer the print uses**. Handing the SVG to the webview would show what WebKit
makes of the file, magenta marker and all, and would prove nothing about what will
be composited. That is the whole reason the card goes through
`core::overlay_preview` rather than being a `data:` URL.

Four things that are decisions rather than taste:

- **`set_overlay` does all the assignment work, unchanged.** Lighting a card's
  landscape icon sends that card's overlay; turning it off sends `None`.
  Exclusivity is free because a slot holds one value, so there is no second command
  and no way for two cards to show lit for one orientation.
- **The cards are plain `Card`s, not `RadioCards.Item`s.** There is no "selected
  overlay" - the toggles carry all the state - and this keeps the panel clear of
  the Tooltip-around-an-Item trap above.
- **The orientation pair is two `IconButton`s**, not a `SegmentedControl`.
  SegmentedControl is single-select and so cannot express "both on", which is the
  case the panel exists to make possible, and Radix Themes 3 ships no
  `ToggleGroup`. `gap="1"` and matching sizes are what make them read as a pair.
- **Adding does not assign.** A card arrives with both toggles dark. An add that
  quietly took a slot would overwrite a choice the operator had already made.

The collapsed state is **persisted in settings** (`overlays_collapsed`). Overlays
are set up once and then left alone, so a kiosk that gave the photo back 180px on
every launch would have to be collapsed again every morning. Collapsed, the rail
still carries the "Overlays" label - turned on its side with `writing-mode:
vertical-rl`, not a `transform: rotate()`, which would rotate the glyphs and leave
a wide layout box overflowing a 36px rail - and the two orientation indicators, lit
when something is being composited.

`PANEL_WIDTH` and `RAIL_WIDTH` are **exported and applied by `App.tsx`'s grid
column**, and the panel's own content fills that column with `flexGrow` rather
than repeating the number. Pinning the inner `Flex` to the same width is how the
1px `Separator` on the right edge ends up with no room and silently shrinks to
nothing - which reads as "the border is the wrong colour" rather than as a missing
element. Both separators are `--accent-a6`; the only question is ever whether they
have space to draw.

The card's PNG lives in `app_data_dir()/overlays`, keyed on the SVG's path *and*
mtime. That is the same key discipline as the previews and for the same reason:
the webview serves an `asset://` URL it has already fetched out of its own memory
cache for the life of the session, so re-exporting a design under an unchanged key
would leave the operator looking at the previous artwork. Files no card points at
are collected on the next refresh - that directory is snappr's, and holds nothing
else.

See "The overlay SVGs" in `CLAUDE.md` for what each overlay means and how it
reaches the pipeline.

**The info line under the photo is one truncating `Text`, and every group in it
drops out when the source has nothing for it.** `Info` in `Actions.tsx` reads
camera, lens and exposure from `photo.exif`, so a screenshot or a scan gives
exactly the line snappr showed before any EXIF was read. Two rules there are not
obvious:

- **The lens is dropped when it only restates the camera.** Apple writes
  `LensModel` as `"<camera> back camera 6.765mm f/1.78"` - the camera name, then
  the focal length and aperture the line already prints. The rule lives in
  `core::exif`, not in the frontend, so the shell receives values it can print
  unmodified. It keeps every interchangeable lens, which is where the lens is the
  interesting fact.
- **The shutter time is shown only when it is more than `TAKEN_AT_DRIFT` from the
  arrival time.** `added_at` is when snappr saw the file, not when the shutter
  fired. For a photo AirDropped straight off the phone those are seconds apart
  and "added 2 minutes ago" says everything; for one out of the camera roll the
  arrival time is actively misleading, and only then does the line grow a
  `taken 21 Mar, 12:40` group.

The line hover is a Radix `Tooltip` carrying the same facts in full, including
the two deliberately kept off the line - the 35mm equivalent and the exact clock
times. The `<time>` element inside it carries no `title` of its own: a native
tooltip underneath a Radix one races it and wins.

**The line names the orientation beside the pixel count**, as `▭ Landscape 4032 x
2724 px`, and the filmstrip cell carries the same glyph in the corner the status
dot does not use. It is `Photo.orientation`, which is geometric and measured after
`autorot` rather than the EXIF `Orientation` tag - so it is the answer that
actually picked the crop and the overlay, which is why it is printed rather than
derived from the two numbers next to it. The cell gets the glyph without the word:
"Landscape" at `size="1"` is most of a 132px cell and the filename would have
nothing left, so the word lives in the cell's `aria-label` and on this line.
`OrientationIcon` is shared with the overlay panel's toggles, so an orientation
looks like one thing wherever the app says it.

**The titlebar drag region is `data-tauri-drag-region="deep"`.** Tauri's
`drag.js` treats the bare attribute as "only direct clicks on this element", so a
wrapper element between the header and the pointer silently stops the window
dragging. The same script already refuses to drag from a clickable element, which
is why the old `-webkit-app-region: no-drag` rule was never needed and is gone.

Caveat worth knowing: Radix's stylesheet uses `color-mix()` and `:has()`, which
want Safari 16.2 / 15.4 WebKit, while `minimumSystemVersion` is 11.0. Big Sur can
run Safari 16.6, so a stock un-updated Big Sur degrades rather than breaks, and
`panelBackground="solid"` avoids the translucent surfaces that suffer most.

Run it with `npm run tauri dev` (Vite on a fixed port 5183). A debug build links
Homebrew's libvips if `mise run runtime` has never run, so it comes up on a fresh
clone; `mise run app` is what builds the bundle with libvips inside it.

---

## What the app is allowed to say

**Every message exists in exactly two versions, declared together.** There is one
catalogue, `crates/snappr-core/src/problem.rs` - an enum with one arm per problem
in `Problem::parts`. Nothing writes a user-facing string anywhere else.
`src-tauri/src/problem.rs` is a shim over it that adds the one thing core cannot
do: delivering a message to the window, which needs an `AppHandle`.

| half | goes to | may contain |
|---|---|---|
| `ui` | the window, and the first part of every log line | one or two plain sentences: what happened, then what to do |
| `detail` | the console only | the exception class, the errno, the libvips text, full paths, ratios |

`ui` may not contain a type name, a hex literal, a backticked shell command, or
any of this repo's own vocabulary - "viewBox", "keying out", "index", "pipeline",
"libvips". ("sidecar" and "handshake" were on that list too, and are now
unsayable for a better reason: there is no child process to name.) A path appears in `ui` only when the operator has to go
and look at it (the watched folder), never as decoration on a sentence that reads
without it. `tests/problem_rules.rs` asserts all of this against the whole table,
so reword freely - the tests assert on **codes**, not wording.

This exists because it was got wrong. The app used to show
`Snappr::Error: no #FF00FF photo window found in brand.svg`: the shell built its
display string as `format!("{}: {}", class, message)`, and the message itself had
been written at the raise site for whoever wrote the code. **Do not put a type name
in front of a message.** The code travels beside it - every command returns
`Result<T, Payload>`, which is `{ code, message }` - so the frontend can branch on
the kind of failure without ever displaying the code.

`Problem::parts` being a single match is what stops a new variant being added with
only one of its halves. That guarantee is why merging the two catalogues into one
was safe.

Anything snappr did not raise itself - `Vips::Error`, `KeyError`, a coercion
failure - never shows its own text. `Problem.plain` substitutes the catalogue's
`photo_unusable` sentence and keeps the original for the log. There is no
message-matching anywhere: the mapping is by exception class, and an uncatalogued
class falls through to one generic sentence rather than leaking.

**Four channels, and each kind of problem has exactly one home.**

| kind | channel | where it shows |
|---|---|---|
| about one photo | `photo.error` / `photo.warnings` | Stage callout + Try again, filmstrip dot, info line |
| about the last thing the operator did | `problem` in `App.tsx`, a `{level, text}` | the info line, red for a failure and amber for a caveat |
| discovered after the command returned | the `notice` Tauri event | the same info line |
| watching asked for and not happening | `RuntimeStatus.watch_error` | the watch pill's third state |

`RuntimeStatus.error` is **only ever set when `ready` is false**, because that is
the only case `Stage` renders it. A watch failure put there was invisible, since at
an event the Stage always has a photo in it. The pill is the home for that instead,
and it needs its own field rather than sharing `notice`: the pill offers a retry,
and must only offer it when a failed watch is the reason. Note the pill's failed
state keeps `watch_enabled` true - the operator *asked* for watching - so its click
handler retries rather than toggling, or it would pause.

The watch pill has **three states, not two**, for the same reason: `watch_enabled`
is what the operator asked for, which is not the same as what is happening. Saying
"Watching" when a watch failed to start was a real lie the bar told for a while,
and `start_watching_noted` exists so that every path which turns watching on - boot,
the pill, a change of folder - cannot tell it again.

**A modal is only for a gesture that otherwise produced nothing at all.** Today
that is exactly one thing: a drag & drop snappr refused. There is no cell, no
spinner, no photo, and no line either - the info line belongs to the selected
photo and the drop never made one. Everything else has somewhere on screen it
belongs, and a modal per print would be intolerable at an event. It is the OS's
alert (`app.dialog().message(...)` from Rust, `show` and never `blocking_show`),
raised on the Rust side because a drop on the Dock icon can be what *launches* the
app - a frontend modal cannot explain a drop from before the webview existed.

`watcher::refusal` is the single implementation of the candidate rule and
`is_candidate` delegates to it, so the filter and the explanation cannot drift.
`Refusal` carries **no payload and does no I/O**: it runs on every fsevent in a
folder with ~1.9k entries and refusal is the common answer there. `ingest_one`
upgrades `Unknown` to `IsAFolder` with one `is_dir()`, on the single path the
operator actually dropped.

---

## Things a simplification would break

Each is argued at its own site; this is an index, not the argument.

| change | breaks |
|---|---|
| prefixing a type name onto a message | the exact regression the two-version catalogue exists to prevent |
| any `stat` in `watcher::refusal` / `is_candidate` | undoes the pinned-core fix |
| `RecursiveMode::Recursive` | the same, harder |
| deriving `Deserialize` for `Overlay` (`settings.rs`) | a legacy `null` takes down the whole settings file |
| `#[serde(untagged)]` on `Overlay` | every parse error becomes "did not match any variant" |
| removing the `.broken` rename, or moving it after the message | announces a loss that is already unrecoverable |
| `StateFlags::all()` on the window-state plugin | restores hidden windows and decorations too |
| dropping `MAXIMIZED`/`FULLSCREEN` from those flags | a dragged-to-fill window reopens small, at the right position |
| making `Photo::exif` non-`Option` | drops the whole session on the first launch after an update |
| dropping `Stored::Legacy` | the same, for indexes already on disk |
| `blocking_show` for the refusal alert | freezes the window - it runs on the event thread |
| doing image work in a command body | they are synchronous, so that is the event thread |
| `Webview::print()` instead of `mac::print` | rasterises the photo through WKWebView |
| setting the paper by size rather than PPD name | prints bordered on a SELPHY |
| dropping the `PICKER` thread-local | the share popover is torn down as it appears |
| `bundle.fileAssociations` instead of `Info.plist` | loses the named UTIs |
| narrowing `assetProtocol.scope`, or moving `preview_dir` / `overlays::dir` | previews or overlay cards stop rendering, silently |
| re-spelling `overlay_landscape` as a list instead of adding `overlays` | puts the watch folder and printer behind a fourth on-disk shape |
| dropping `Settings::normalise` | an existing install opens with an empty panel and its own overlay nowhere in it |
| keying overlay PNGs on the path alone | a re-exported design keeps showing the old artwork, out of WebKit's cache |
| rendering the card's SVG in the webview | the card stops being a check - it would show WebKit's render, marker included |
| making the card render fail when no window is found | an unfixable-looking empty card instead of the artwork and the reason |
| routing drops by pointer position rather than by extension | Dock drops have no position at all |
| emitting `runtime` without storing it | a late listener waits forever |
| routing boot's watch failure through `start_watching_noted` | emits the event twice |
| moving `discard_superseded` before `publish` | a crash in between blanks a live cell |
| sorting in `usePhotos` instead of `App.ordered` | inverts the newest-photo fallbacks |
| sorting `ordered` on `added_at` instead of reversing | same-second arrivals read backwards, and the arrows walk that |
| dropping the focus move in `App.tsx`'s `[selected]` effect | arrows step from the cell focus was left on, not the selected one |
| following every arrival regardless of `atHead` | an arrival yanks the operator out of the photo they navigated to |
| `loop` left at Radix's default on the strip | one arrow path wraps and the other clamps |
| more than one worker thread | four 12MP composites at once, each holding a 25 MB output |
| hand-editing `src/bindings/` | it is generated; `cargo test` overwrites it |
| naming a command or an event outside `src/ipc.ts` | the drift the generated bindings exist to end |
