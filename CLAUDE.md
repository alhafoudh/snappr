# snappr

AirDrop-to-print kiosk pipeline. Photos are AirDropped to this Mac and land in
`~/Downloads` as HEIC or JPG/JPEG. snappr crops each one to the shape of the
100x148mm photo paper it will be printed on, fits it into the window marked in a
branded SVG overlay, composites the overlay over it, writes
`<basename>.overlay.png` next to the original, and opens it. The operator prints
from there. Nothing else: no editing, no review queue, no print driver.

**The photo is bit-for-bit lossless.** Cropping throws away pixels, by design,
and the overlay covers some of what is left; nothing else in the pipeline is
allowed to change a single pixel that survives both. See "Nothing is allowed to
degrade the photo" below - most of the non-obvious decisions in this codebase
exist to protect that property.

An earlier design ran the whole thing through darktable, with a private configdir,
a Lua executor inside `darktable-cli`, sqlite bookkeeping, staged film rolls,
dedupe and a manual operator review pass. It was never implemented and has been
dropped; `Plans/20260822-snappr-ingest-pipeline.md` and
`Plans/20260822-snappr-live-ingest.md` remain only as the historical record.
**Do not reintroduce darktable.**

## Stack

- Ruby 3.4.5, pinned by `mise.toml`
- Zeitwerk autoloading for everything under `lib/`
- libvips 8.18.4 via `ruby-vips`, driven through `image_processing`
- librsvg 2.62.3 for the overlay, libheif for HEIC - both already inside libvips

No Rails. Runtime gems are `image_processing`, `listen`, `ruby-vips`, `rexml`,
`zeitwerk` and nothing else.

## The desktop UI

The Tauri app lives at the repo root: `src/` is the React frontend, `src-tauri/`
the Rust shell. React 18 + Vite + TypeScript, and **Radix Themes 3 carries the
whole look**.

**There is no stylesheet, by design.** `src/app.css` was deleted in the Radix
rewrite. Layout, colour, spacing, selection and status all come from Radix
components, their props and their tokens - a dark-only `<Theme>` in
`src/main.tsx`. Do not reintroduce an app stylesheet: the point is that there is
exactly one place the design comes from. Raw CSS survives in three places only,
each of which Radix genuinely cannot express:

- `index.html` sets `margin: 0` on `<body>`. Radix ships no html/body reset, and
  without it the `100vh` grid overflows by the browser's default 8px.
- `objectFit` is inline on the two `<img>` elements, in `Stage.tsx` and
  `Filmstrip.tsx`. There is no Radix prop for it, and inline style on an `<img>`
  is what Radix's own examples do. The comment on the letterbox rule is
  load-bearing: the overlayed and original views have different aspect ratios by
  design, so the box needs a definite size or a portrait source stretches it.
- Pixel literals passed *as Radix props* (`height="84px"`,
  `columns="repeat(n, 132px)"`). Those props are typed `"enum | string"` for
  exactly this.

Two things that look like they could be simplified and cannot:

- **The filmstrip is `RadioCards`.** The selection ring, hover, focus ring,
  roving arrow-key focus and radiogroup semantics all come with it, which is most
  of what the old stylesheet was for. Because it owns the arrow keys while
  focused, the global keydown handler in `App.tsx` skips events originating
  inside a `[role="radiogroup"]` or `[role="group"]` - without that guard one
  arrow press moves the selection twice.
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
| `Left` / `Right` | next / previous thumb |
| `` ` `` | toggle overlayed against original |
| `Cmd+P` | print |

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

**The Overlays button is one button, not two**, and it opens a native menu with a
submenu per orientation ("Choose..." / "Clear"). The bar is 44px and already
carries the watch toggle, the folder, the count and the view control; two more
path buttons in it read as clutter, and the submenu labels can carry the current
filename, which two buttons could not. It goes through the same `popup()` helper
in `App.tsx` as the filmstrip menus, so the keep-one-menu-alive lifetime that
comment describes applies here too. When both slots are empty the button says
"No overlays" in gray - a fresh install has no branding, and that state has to
look deliberate rather than unloaded. See "The overlay SVGs" for what clearing
means and how the setting reaches the pipeline.

**The info line under the photo is one truncating `Text`, and every group in it
drops out when the source has nothing for it.** `Info` in `Actions.tsx` reads
camera, lens and exposure from `photo.exif`, so a screenshot or a scan gives
exactly the line snappr showed before any EXIF was read. Two rules there are not
obvious:

- **The lens is dropped when it only restates the camera.** Apple writes
  `LensModel` as `"<camera> back camera 6.765mm f/1.78"` - the camera name, then
  the focal length and aperture the line already prints. The rule lives in
  `Snappr::Exif`, not in the frontend, so the shell receives values it can print
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

**The titlebar drag region is `data-tauri-drag-region="deep"`.** Tauri's
`drag.js` treats the bare attribute as "only direct clicks on this element", so a
wrapper element between the header and the pointer silently stops the window
dragging. The same script already refuses to drag from a clickable element, which
is why the old `-webkit-app-region: no-drag` rule was never needed and is gone.

Caveat worth knowing: Radix's stylesheet uses `color-mix()` and `:has()`, which
want Safari 16.2 / 15.4 WebKit, while `minimumSystemVersion` is 11.0. Big Sur can
run Safari 16.6, so a stock un-updated Big Sur degrades rather than breaks, and
`panelBackground="solid"` avoids the translucent surfaces that suffer most.

Run it with `npm run tauri dev` (Vite on a fixed port 5183), after
`mise run runtime` has built the bundled Ruby and libvips into `build/`.

## Tooling

`mise.toml` pins Ruby and is the entry point operators type. `bin/snappr` is the
real executable, but reach it through mise so the toolchain activates and nobody
has to remember `bundle exec`:

```
mise run setup            bundle install
mise run watch            watch the drop folder and overlay each photo that lands
mise run overlay PATH     overlay the given photos now, then open them
mise run test             unit tests
```

`SNAPPR_ROOT` comes from mise's `{{config_root}}`, so `Snappr::Config` resolves
every path against the project root rather than the working directory. There is
an `__dir__` fallback so `bin/snappr` still works when invoked directly.

`mise run overlay` is the loop for iterating on the overlays - it takes paths
straight off the command line, so you never have to AirDrop anything to see a
change. Pass it one landscape and one portrait photo to check both variants at
once.

## Architecture

```
~/Downloads/IMG_1234.heic
      | listen gem: extension filter, self-output guard, stability check
      v
  Overlay#call
      |  new_from_file(src).autorot          -> width, height
      |  Exif.read(image)                    -> camera, lens, exposure, taken_at
      |  orientation_of(width, height)       -> :landscape | :portrait
      |  crop_dimensions(w, h, print_ratio)  -> largest inscribed 148:100 or 100:148
      |  config.overlay_svg(orientation)     -> the operator's SVG, or nil
      |  OverlaySvg.render(svg, crop_w, crop_h)
      |
      |  OverlaySvg.photo_window(svg)         -> the #FF00FF window, as fractions
      |                                          (FULL_FRAME when there is no SVG)
      |  layout_for(...)                      -> canvas size, crop, placement
      |  icc_transform the overlay into the photo's colour space
      |
      |  ImageProcessing::Vips
      |    .crop(left, top, photo_w, photo_h)          object-fit: cover, no scaling
      |    .embed(photo_left, photo_top, canvas_w, canvas_h, background)
      |    .composite(overlay, over, north-west)       only when there is an overlay
      |    .flatten                                    only when there is alpha to drop
      |    .convert(png).saver(compression: 6)
      v
~/Downloads/IMG_1234.tmp-<pid>.overlay.png   (renamed into place)
      v
~/Downloads/IMG_1234.overlay.png  ->  system("open", ...)
```

The photo is fitted to the overlay's window rather than the overlay being sized
to the photo, so the branding always sits exactly where the SVG puts it and is
never itself cropped.

There is no state, no queue and no daemon bookkeeping. Nothing persists between
runs; re-running on the same photo just overwrites its `.overlay.png`.

## Everything stays where it is

There is no `var/`. snappr creates no directories and writes no databases, logs
or staging trees. `~/Downloads` is the only path outside the project it touches,
and the only thing it ever writes there is the `.overlay.png` beside each photo.

**snappr never moves or deletes anything.** Files it does not recognise are
ignored, not swept, not quarantined. `~/Downloads` is the user's real downloads
folder with hundreds of unrelated files in it; treat it as read-only apart from
the one file snappr adds.

Logs go to stdout.

## Nothing is allowed to degrade the photo

Cropping is the one place image data is intentionally discarded. Everything after
it is lossless, and each of the following is load-bearing for that. They were all
established by measurement, and each has a test.

**Output is PNG, not JPEG.** JPEG at Q:100 4:4:4 measured 50.4 dB PSNR - visually
lossless, but not lossless, and it cannot carry more than 8 bits. PNG is exactly
lossless, keeps 16-bit, keeps the ICC profile, keeps the resolution tag, and
encodes in about a third of the time of lossless HEIC.

`overlay_test.rb` asserts the photo is **bit-identical to the source everywhere
the overlay is fully transparent**. That is the invariant to test, and note why it
is phrased that way: the overlay is meant to draw *over* the photo, so pixels
under artwork are supposed to change. An inset-border check was tried first and is
wrong - it passes only for designs whose artwork stays out of the window, and the
real branding draws a 3px anti-aliased edge into it (0.09% of the photo region).
Measured: 99.9% of the photo region bit-identical, and zero difference under clear
overlay.

Lossless alternatives that were measured and rejected, on a 12MP frame:
TIFF+deflate is smaller (6.2 MB vs 9.7) but slower and less pleasant to open;
lossless HEIC is 8.7 MB but takes 3.0s against PNG's 0.9s **and silently drops
the resolution tag**. If output size ever becomes the problem, TIFF is the
sensible next stop, not JPEG.

**The canvas is derived from the photo, not fixed.** `layout_for` sizes the output
so the window lands at exactly the largest crop the photo can supply at the
window's shape. That makes the fit a scale factor of 1 - object-fit: cover with
nothing to scale - so the photo is never resampled and never upscaled. The canvas
therefore comes out *larger* than the photo whenever the window is smaller than
the frame, which is the intended trade: bigger file, untouched pixels.

**The crop is `extract_area`, never `resize_to_fill`.** This is the subtle one.
`resize_to_fill` goes through vips `thumbnail`, and **thumbnail downcasts 16-bit
images to 8-bit** - a 10-bit HEIC decoding to a ushort value of 40000 comes out
of thumbnail as uchar 156. Since `crop_area` only ever returns a box that already
fits inside the photo, there is never anything to scale, so a plain centre crop
is both correct and lossless. Do not "tidy" this back into `resize_to_fill`
because it reads more like object-fit: cover - it is object-fit: cover, and it is
the version that keeps the bit depth.

**Bit depth follows the source.** Nothing casts. An 8-bit JPEG stays 8-bit; a
10-bit HEIC decodes to ushort and is written as a 16-bit PNG. Verified both ways.

**The ICC profile is never stripped**, so the print does not colour-shift.
Verified byte-for-byte with Display P3, the space iPhones actually shoot in.

**The overlay is converted into the photo's colour space before compositing.**
librsvg renders in sRGB. If the photo is Display P3 and the overlay is composited
as raw numbers, any colour in the branding is reinterpreted and comes out wrong:
an sRGB brand red of (200, 30, 40) lands as P3 (184, 50, 49). `Overlay#overlay_for`
does an `icc_transform` from sRGB into the photo's embedded profile, falling back
to compositing as-is (with a warning) if the profile is unusable. Today's branding
is pure white and black, which are identical in both spaces, so this is currently
a no-op - it is insurance for the moment someone adds a coloured logo, and it must
not be removed as "dead code".

**The alpha channel is dropped with `flatten`.** The composite is opaque
everywhere, so this discards nothing and avoids shipping a pointless 4-band PNG.

What is *not* preserved, and cannot be while the output is a still image:
Apple HDR gain maps. Printers are SDR, so the print would be made from the SDR
rendition anyway - this is a non-issue for this pipeline, not a compromise.

## Hard constraints

**The self-output guard.** The output is written next to the input, in the folder
being watched. `Watcher#candidate?` skips anything ending in the configured
`output_suffix`, case-insensitively, and the in-progress `*.tmp-<pid>.overlay.png`
ends in that same suffix so one guard covers both.

Since the move to PNG this is belt and braces rather than critical: `png` is not
in the watched extensions, so the output cannot retrigger the watcher anyway.
Keep the guard. It was load-bearing when the output was `.overlay.jpg` - without
it the watcher fed on itself, producing `IMG_1234.overlay.overlay.jpg` and on
forever - and it becomes load-bearing again the moment anyone adds `png` to
`extensions` or changes the output format back.

**Requiring `image_processing/vips` disables SVG loading.** Its `vips.rb` calls
`Vips.block_untrusted(true)` at require time, and libvips marks `svgload` as an
untrusted loader. The symptom is every render failing with
`VipsForeignLoad: buffer is not in a known format`, which points nowhere near the
cause. `lib/snappr.rb` requires the gem explicitly and then calls
`Vips.block("VipsForeignLoadSvg", false)`, unblocking that one loader by name.
Order matters - do this after the require, or the gem re-blocks it. Do not
"simplify" it to `Vips.block_untrusted(false)`: guest photos are semi-trusted
input and should keep the hardening; only our own committed SVG is exempt.

**The crop keeps the source's resolution, so it must be the largest inscribed
rectangle.** "Keep the dpi of the source photo" is the requirement; the way it is
met is to pick the biggest rectangle of the target ratio that fits inside the
photo, which makes `resize_to_fill`'s scale factor exactly 1. The result is a
pure centre crop: no resampling, no upscaling, and libvips carries `xres`/`yres`
through untouched. `Overlay#crop_dimensions` keeps one side whole and shortens
the other, and `overlay_test.rb` asserts both that neither side ever grows and
that one side is always kept whole - the second half is what stops someone
"fixing" it into a fixed 300 dpi target.

**Read the dimensions after `autorot`, never before.** `ImageProcessing` also
autorotates on load, so anything measured from the pre-rotation header is
transposed on every portrait phone photo. That now decides more than the overlay
size: it picks the orientation, and therefore which SVG is used and which way the
photo is cropped. Get it wrong and a portrait photo is judged landscape, cropped
to 148:100 and given the right-hand panel. `Overlay#dimensions` calls `.autorot`
first for exactly this reason. libvips is lazy, so this costs a header read, not
a decode.

**The temp file needs a real image extension.** libvips picks its saver from the
destination extension, so a `.tmp` temp path fails with
`VipsForeignSave: not a known file format` however the pipeline was configured.
The temp name is `<stem>.tmp-<pid>.overlay.png`: `.png` so the saver works,
`.overlay.png` so the watcher's guard skips it, `tmp-<pid>` so it is
distinguishable and concurrent-safe.

**A trap handler may not take a mutex.** `Listen::Listener#stop` synchronizes, so
calling it from `trap("INT")` dies with
`ThreadError: can't be called from trap context` on Ruby 3.4. The handler only
flips a local flag; `Watcher#run` polls it and stops the listener outside the
trap.

**`listen` must not be allowed to recurse.** `Listen.to` is passed
`ignore: %r{/}`, and that is load-bearing for CPU, not tidiness. listen's macOS
adapter turns every fsevent into a *recursive* rescan of the changed directory
(`adapter/darwin.rb`, `_queue_change(:dir, dir, rel_path, recursive: true)`), and
the watched directory is `~/Downloads`: about 1.9k entries at the top level, but
about 19k files once its 89 subdirectories are walked. One unrelated file
changing therefore cost 0.5-5s of CPU. Since snappr writes its own 10-25 MB
`.overlay.png` into that same folder, the write storm from each photo pinned a
core for **63 seconds** after the photo was already done - a kiosk that looks
idle while eating 100% of a core. Measured per event: 0.517s before, 0.094s
after; total CPU over a 5-minute window with one 12MP HEIC processed, 64.3s
before, 12.1s after.

Note what does *not* help, because it reads like it should: neither the `only:`
regex nor `Watcher#candidate?` prevents any of this. Both run *after* the scan,
so they stop the compositing but not the directory walk - the self-output guard
keeps snappr from re-processing its own PNG while the PNG still costs a full
19k-file rescan. AirDrop only ever lands files at the top level of the drop
folder, so ignoring the subdirectories loses nothing.

If this ever needs to go further, the next stop is dropping `listen` for a plain
`Dir.children` scan on a timer: measured 0.0024s per scan of the same folder,
40x cheaper again, and `wait_until_stable` already covers the partial-write
problem that fsevents were buying.

## The overlay SVGs

**snappr ships no branding.** There are two overlay slots, one per orientation,
and both are **operator settings picked from the top bar** - not assets this repo
provides. A fresh install has neither, and that is a working configuration, not a
broken one.

| | frame | branding |
|---|---|---|
| landscape slot | 148x100mm | right **2/5** by convention |
| portrait slot | 100x148mm | bottom **2/5** by convention |

Giving the branding 2/5 of the frame and the photo 3/5 splits along the frame's
**long axis**, which is why portrait wants a bottom band rather than a right-hand
strip: 2/5 of a 100mm width is only about 40mm, and artwork shrinks badly in it.
That is design advice for whoever draws one, not something Ruby knows.

**An empty slot means no overlay at all.** The photo is still cropped to the
print ratio, but nothing is composited over it: `Overlay::FULL_FRAME` stands in
for the window, so the canvas comes out equal to the crop, the photo lands at
(0, 0), and the seam margin is 0. The output is then bit-identical to the source
across the *whole* frame rather than only where the overlay is clear -
`overlay_test.rb` asserts exactly that.

One trap in that path: **`flatten` is not a no-op on an image with no alpha.** It
fails with `linear: vector must have 1 or 2 elements`, which points nowhere near
the cause. Alpha is present only if the overlay brought it or the source carried
it, so `Overlay#process` asks about precisely that before flattening.

**A slot whose file has been moved degrades to no overlay with a warning**, and
does not raise. The warning travels in the result's `warnings` array, so the cell
and the Actions bar both show it. There is deliberately **no fallback to the
other orientation's SVG** - that existed while the overlays were committed assets
that might ship only one orientation, and now that each slot is picked separately
it would mean clearing portrait silently printed stretched landscape branding.
Wrong branding on a print is worse than none.

Where the settings live and how they reach the pipeline:

- `Settings::overlay_svg_landscape` / `_portrait` in `src-tauri/src/settings.rs`,
  `Option<PathBuf>`, persisted to `app_config_dir()/settings.json` next to
  `watch_dir`. **The path is remembered, not the file copied** - re-exporting a
  design over the same path takes effect on the next photo with no re-picking.
- `pipeline_config()` emits them as `overlay_svg: { landscape, portrait }`, with
  an explicit `null` for a cleared slot. It has to be explicit: `Server#hello`
  merges the frame over `Config::DEFAULTS`, so an absent key would let a default
  win, and the default is the thing being overridden.
- `set_overlay_svg` pushes the change to the running child with `Sidecar::
  reconfigure`, so no relaunch is needed. Photos already in the strip keep their
  output; the per-photo retry button is how one gets regenerated.

**`share/` is test material, not branding**, and must never be listed in
`bundle.resources`. `config/snappr.yml` still points at it because that file is
the CLI's config and nothing else reads it - the app never calls `Config.load`.

**Draw them in whatever tool you like.** Figma, Illustrator and Inkscape exports
all work unmodified. There are three rules:

1. **Mark the photo window with a `#FF00FF` shape.** That is where the photo is
   fitted, `object-fit: cover`. It can be any element - the committed overlays use
   a `<path>`, not a `<rect>` - anywhere in the document, at any nesting depth,
   under any transform. It is **never rendered**: it exists to tell snappr the
   shape and position of the window, and is a guide for the designer.
2. **Draw at the paper's aspect ratio** - 1.48:1 for landscape, 1:1.48 for
   portrait. The overlay is stretched to fill the frame, so one drawn at the wrong
   shape is silently distorted. `Overlay#warn_on_aspect_mismatch` logs a warning
   naming the size to redraw at, and `overlay_svg_test.rb` asserts the files in
   `share/` are within 1% of their print ratio.
3. **Keep a `viewBox`**, or a `width` and `height` in absolute units. That is what
   `OverlaySvg` scales. Exporters emit one by default.

Anything the artwork does not cover and the photo does not reach is filled with
`background` from `config/snappr.yml`, white by default. A design with a margin
around its window gets that margin in the background colour.

Artwork may overlap the window freely - the overlay composites *over* the photo,
so a logo or a soft edge across the photo works as drawn.

To rebrand, pick different SVGs from the Overlays menu. Nothing in Ruby knows
what they depict; Ruby owns exactly two numbers about layout, the paper's
dimensions.

### Finding the window by colour, not by geometry

`OverlaySvg.photo_window` renders the SVG and takes the bounding box of the
magenta pixels, returning it as fractions of the frame. It does not read the
element's coordinates.

That is deliberate. Exporters wrap shapes in groups, apply `transform`
attributes, and express rectangles as `<path>` data - the committed overlays mark
the window with `<path d="M0 0H2209V2000H0V0Z"/>` under a transform. A rendered
pixel is the one thing that accounts for all of it, and it cost nothing: the
detection render is at the authored viewBox size.

Removal is by element, not by colour-keying: `stripped_source` deletes any element
whose `fill` attribute or `style` declares the marker colour. A colour-key would
leave a magenta fringe wherever the shape was anti-aliased. There is still an
alpha-keying safety net for a marker that arrives through something the element
walk cannot see - a CSS class, a gradient, a `<use>` - and it warns when it fires,
because a magenta block on a print is much worse than a hole.

Magenta because nothing in real branding is `#FF00FF` by accident. Matching is
tolerant (±60 per channel) so a colour-managed export still registers.

### Why the photo is grown two pixels past the window

The photo is laid down first and the overlay goes over it, but the overlay has a
hole where the window is - so any rounding disagreement between the two would show
the *background* through the join. `Overlay::SEAM_MARGIN` grows the photo past the
window on every side, and the overhang is either hidden under artwork or clipped
by the canvas.

Two pixels is headroom, not a fix: measured leakage is zero at 1, 2 and 3, but a
window edge on a fractional coordinate spreads librsvg's anti-aliasing across as
much as three pixels. The margin is held back from the canvas calculation before
the canvas is derived, so growing the photo later can never ask for more pixels
than the photo has.

### Never overwrite the authored viewBox

`Snappr::OverlaySvg` sets the root `width`, `height` and `preserveAspectRatio`,
and **preserves the `viewBox` exactly as authored**. The viewBox is what maps the
artwork's own coordinate system onto the output, so setting width and height is
all it takes to scale a whole design to any photo size.

This was got wrong once and it is worth knowing what the failure looks like,
because it reads as "the overlay is broken" rather than as a bug in this file. The
original hand-written overlays used percentage geometry inside a nested `<svg>`,
and `render` rewrote the viewBox to the photo's pixel size. That worked *only* for
that one dialect. The first real exported overlay - absolute user-unit
coordinates against `viewBox="0 0 2000 2960"` - had its coordinate system
replaced with `0 0 2724 4032`, so the artwork kept its authored size and floated
at x 0-73%, y 44-73% of the frame instead of spanning the bottom 2/5.

`test/fixtures/` holds the same trivial design in three dialects - absolute
coordinates, no viewBox at all, and percentages - and
`test_artwork_scales_to_fill_the_frame_in_every_dialect` renders all three and
requires identical placement. Reintroducing the viewBox rewrite fails nine tests.

`preserveAspectRatio="none"` is deliberate: the crop guarantees the photo already
has the print's ratio, so the only stretch is the rounding residue from
`crop_area`, a few hundredths of a percent. Without it the default `meet` would
letterbox that residue and leave a transparent sliver along one edge, showing the
photo through what should be a solid panel.

A file with no viewBox has one synthesised from its `width`/`height`. Percentage
width/height are refused, since they mean nothing without a parent viewport.

## Verified mechanics (libvips 8.18.4, macOS, 2026-08-22)

Re-verify these if libvips is upgraded. They were established empirically.

- **HEIC loads natively** via `heifload`; this build has libheif. **No
  pre-conversion step is needed**; do not add one. `heifsave` also exists, which
  is how the tests build a HEIC fixture without committing a binary.
- SVG loads via `svgload`, backed by librsvg 2.62.3. It is flagged `untrusted` in
  the libvips operation table - see the `block_untrusted` constraint above.
- librsvg 2.62 honours `dominant-baseline="middle"`. Measured: the text centres
  within about 0.3% of the panel centre, the residual being x-height versus
  cap-height. If a future librsvg regresses this, put an explicit `dy="0.35em"`
  on the `<text>` element rather than adding code.
- **vips `thumbnail` downcasts 16-bit to 8-bit.** A ushort input of 40000 comes
  out uchar 156. This is why the crop is `extract_area` and not `resize_to_fill`.
  The single most damaging thing anyone could "simplify" here.
- `extract_area` preserves format exactly, and `composite` promotes an 8-bit
  overlay into a 16-bit photo with correct range scaling: white reaches 65535,
  text reaches 0, and the photo's own 40000 is untouched. So the 8-bit SVG render
  needs no manual scaling.
- librsvg scales an SVG by its `viewBox`, so replacing that viewBox strands the
  artwork at its authored coordinates. Measured: a bottom-2/5 band drawn at
  2000x2960 rendered into 2724x4032 landed at x 0-73%, y 44-73%.
- librsvg anti-aliases a shape edge that falls on a fractional coordinate across
  up to three pixels (measured alpha 0, 7, 255 on consecutive rows). This is why
  the photo is grown past the window, and why "the photo is unchanged" has to be
  asserted against the overlay's alpha rather than an inset border.
- The committed overlays' windows: landscape 74.6% of the width by full height,
  portrait full width by 74.2% of the height. Both marked with a `<path>`, so
  geometry parsing would have needed path and transform support.
- `icc_transform` keeps the alpha channel, and maps sRGB white to (255,255,255)
  in Display P3 - so achromatic branding is unaffected while coloured branding is
  corrected.
- Measured output, 12MP frame (4032x2724): PNG 8-bit 9.7 MB / 0.9s, PNG 16-bit
  24.6 MB, TIFF+deflate 6.2 MB / 1.8s, lossless HEIC 8.7 MB / 3.0s, JPEG Q:100
  4.8 MB / 0.9s but lossy.
- For the record, the JPEG that was replaced: Q:100 gave 4:4:4 chroma (libvips
  disables subsampling at Q>=90) and 50.4 dB PSNR, mean error 0.48/255. Genuinely
  invisible in print - it was replaced for the 8-bit ceiling, not for the DCT
  error. Note the Q:90 cliff though: below 90 libvips switches to 4:2:0 and text
  edges go blotchy, so that setting was never as linear as it looked.
- `xres`/`yres` survive the crop and reach the PNG. Verified round-trip at
  11.811 px/mm (300 dpi) from a JPEG source.
- **The output PNG already carries the source's full EXIF.** libvips passes the
  `exif-data` blob through `pngsave` untouched, so `IMG_1234.overlay.png` reads
  back with Make, Model, LensModel, the exposure and the GPS fix. Nothing in the
  pipeline does this on purpose and nothing needs to be added to keep it -
  verified against a real output. Worth knowing before anyone goes looking for
  the code that preserves metadata, and worth knowing that **the print carries
  the guest's GPS coordinates** if that ever matters.
- **EXIF tags come out of libvips as one annotated string per tag**, shaped
  `<raw> (<human>, <Type>, <n> components, <n> bytes)` - e.g. FNumber is
  `"1244236/699009 (f/1.8, Rational, 1 components, 8 bytes)"`. The human half is
  the one worth showing. `Snappr::Exif::ANNOTATED` anchors on the tail rather
  than stripping the last bracket, because ApertureValue's human half is itself
  parenthesised (`"1.66 EV (f/1.8)"`) and a bracket-counting parse gets it wrong.
  Values arrive as ASCII-8BIT and are transcoded and scrubbed: left alone, one
  high byte in a lens name takes down `JSON.generate` in `Server#emit` and fails
  a photo whose PNG is already written.
- libvips writes EXIF ASCII fields to the 7-bit spec, transliterating accents
  away. So a test fixture that goes through `jpegsave` cannot carry high bytes -
  set them on an in-memory image instead, which is what `exif_test.rb` does.
- **libvips `heifsave` does not write resolution metadata**, so a HEIC written by
  vips reads back at the 1 px/mm default. That only affects test fixtures - real
  camera HEICs carry their own - which is why the HEIC tests assert dimensions and
  depth rather than dpi.
- Do not read too much into the dpi tag either way. It is advisory metadata; what
  actually decides print quality is the pixel count. A 4032x2724 crop on
  148x100mm is about 690 dpi of real detail whatever the tag says, and the print
  dialog sets the scale.
- Only about **54% of the original photo is visible** in the print: the crop keeps
  90% of the pixels and the panel then covers 40% of those. Cropping straight to
  the visible region's shape instead would show ~23% more pixels and a gentler
  crop, at the cost of changing the composition. Considered and not done.

## Repo layout

```
bin/snappr                CLI entrypoint
lib/snappr.rb             Zeitwerk setup, the svgload unblock, Snappr.root/.config/.logger
lib/snappr/config.rb      config/snappr.yml, project-relative path resolution
lib/snappr/exif.rb        camera, lens and exposure off the source header, for the UI
lib/snappr/overlay_svg.rb renders an overlay at a pixel size, finds its #FF00FF window
lib/snappr/overlay.rb     orientation, window fit, colour-managed composite, PNG save
lib/snappr/watcher.rb     drop folder via the listen gem, candidate? guard, stability check
lib/snappr/cli.rb         subcommand dispatch (Snappr::CLI)
share/                       sample overlays for the CLI and for testing - NOT
                             shipped branding, and never in bundle.resources
test/fixtures/               the same design in several SVG dialects
config/snappr.yml
mise.toml
test/
```

Keep the decision logic in Ruby and the geometry in the SVGs. `Snappr::OverlaySvg`
should stay a root-attribute rewrite; if you find yourself computing panel widths
or positions in Ruby, the SVG is not being scaled properly. Ruby owns exactly two
numbers about layout - the paper's two dimensions, in `config/snappr.yml`.

## Working notes

- AirDrop writes into `~/Downloads` non-atomically. `Watcher#wait_until_stable`
  waits for size and mtime to stop changing (two matching reads 0.3s apart,
  giving up after 30s) before compositing. `.download`, `.part` and
  `.crdownload` names are ignored outright.
- `listen` emits a burst of modify events for one arriving file. An in-memory
  in-flight `Set`, guarded by a mutex, keeps the same photo from being
  composited several times concurrently.
- One bad photo must not take the daemon down. `Watcher#handle` logs and moves
  on. There is no retry and no reject folder: the original is still sitting in
  `~/Downloads`, so `mise run overlay` on it is the retry.
- Only one `watch` should run at a time, but nothing enforces it, because two
  watchers doing idempotent overwrites of the same output is harmless. What is
  not harmless is an editor-launched watcher running stale code across a fix, so
  check for a stray process before concluding a change did not work.
- Outputs are large: about 10 MB per 12MP photo, and 25 MB if the source was a
  10-bit HEIC. That is the price of lossless and it is the right trade for a
  kiosk, but `~/Downloads` will grow fast over an event. snappr never deletes
  anything, so clearing up is a manual job.
- `rexml` is a bundled gem from Ruby 3.4 on, so it must be declared in the
  `Gemfile`. It resolves fine outside bundler and then vanishes under
  `bundle exec`, which makes the failure look like a Zeitwerk problem.
