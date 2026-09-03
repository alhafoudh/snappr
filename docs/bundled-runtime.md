# The bundled libvips

`Snappr.app` ships its own libvips, so it runs on an operator's Mac that has never
heard of Homebrew. `mise run runtime` builds it into `build/`, which is gitignored
- **the tasks are the source of truth, not their output.**

```
mise run runtime           all of it, in order
mise run runtime-libvips   cargo xtask libvips   libvips from source
mise run runtime-closure   cargo xtask closure   the relocated dependency closure
mise run runtime-verify    cargo xtask verify    the gate
```

**Order matters**: `closure` walks the dependency closure of the libvips that
`libvips` produces, and `verify` runs a binary linked against what `closure`
relocated. The individual tasks exist for iterating on one stage.

**Each module's header carries the full argument** - which configure flag exists
for what, which alternative was measured and rejected, and what the failure looks
like. This file is the map and the short version; read the header in
`crates/xtask/src/` before changing a stage.

This document used to describe five stages. Two of them built a relocatable CRuby
3.4.5 from source and vendored gems standalone, because the pipeline was Ruby and
a child interpreter had to ship inside the app. **That half never shipped**:
`bundle.resources` named only `sample/`, so the bundle layout below was never
assembled and the app spawned the repo's development Ruby instead. The pipeline is
Rust now, and what is left is one dependency.

## Which libvips gets linked, and when

Two profiles, two answers, and the reason is that a debug build must work on a
fresh clone while a release build must not depend on the machine that made it.

| profile | links | resolved at runtime by |
|---|---|---|
| debug | `build/libvips-prefix`, or Homebrew if that has not been built | the absolute install name meson wrote |
| release | `build/link`, whose symlinks point into `build/vips` | `LC_RPATH`, giving `Contents/Frameworks` or `build/vips` |

`crates/snappr-core/build.rs` decides that from `PROFILE`, and it is worth knowing
why the debug case avoids `@rpath` entirely: `cargo:rustc-link-arg` and its
`-tests` variant do not reach a library crate's own unit-test harness, so a debug
build that needed an rpath would fail `cargo test` with "Library not loaded:
@rpath/libvips.42.dylib" while `cargo run` worked perfectly.

**Homebrew is a development fallback and never a bundle.** Homebrew's libvips
dlopens its format support - including HEIC - from a module directory baked in at
compile time, so a copied Homebrew libvips silently cannot decode an iPhone photo.
That is the whole reason libvips is built here at all, and `verify` is what
catches a bundle that borrowed it.

## What shapes the layout: install names

Three libraries are named from *outside* `Contents/Frameworks`, because the
binaries that link them live in `Contents/MacOS`:

```
Snappr.app/Contents/
  MacOS/Snappr              LC_RPATH: @executable_path/../Frameworks, and build/vips
  Frameworks/*.dylib        45 dylibs, copied and codesigned by Tauri
    libvips.42.dylib          id @rpath/...        linked directly
    libglib-2.0.0.dylib       id @rpath/...        linked directly
    libgobject-2.0.0.dylib    id @rpath/...        linked directly
    everything else           id @loader_path/...  only ever loaded by a sibling
  Resources/sample/         the one overlay that ships
```

`@loader_path` for an executable means *the executable's own directory*, so an
`@loader_path` id on libvips would have the app looking for it in
`Contents/MacOS`. Hence the split - and the sibling references to those three are
rewritten to `@loader_path` anyway, so both routes work at once.

**glib and gobject are on that list because `rs-vips` links them directly**: it
calls GObject to manage image lifetimes, and emits `-lglib-2.0 -lgobject-2.0`
alongside `-lvips`. Miss them and a binary links Homebrew's glib by absolute path
while libvips resolves to the bundled one - two copies of glib in one process,
which segfaults inside `vips_init` with nothing in the message about glib. That
was a real afternoon.

`build/link` exists for the linker only: `-lglib-2.0` resolves to
`libglib-2.0.dylib`, an unversioned name the flat closure does not have. Its
symlinks live outside `build/vips` so that directory stays exactly what ships -
`closure` asserts it holds no links at all.

## `cargo xtask libvips` - libvips from source

libvips 8.18.4, tarball SHA-256 pinned, built with meson into
`build/libvips-prefix`.

**`-Dmodules=disabled` is the point of the exercise.** It links the format loaders
*into* the library instead of leaving them in a module directory beside it, which
is what makes the closure relocatable. The `report` step asserts no
`vips-modules-*` directory exists, because that is the symptom of getting it wrong.

`heif rsvg jpeg png lcms exif zlib tiff` are `enabled`, not `auto`: a missing
dependency must fail the build loudly here rather than silently produce a libvips
that cannot read what gets AirDropped. Thirty other features are `disabled`, and
meson's split between "feature" options and plain booleans is why there are two
lists - passing `disabled` to a boolean fails setup with `Option "nsgif" value
disabled is not boolean`.

Rejected: Homebrew (the module directory), and sharp's prebuilt libvips - it
cannot decode HEVC, and iPhone HEICs are HEVC.

## `cargo xtask closure` - relocating the closure

Breadth-first over `otool -L` from libvips, copying every vendored library into
`build/vips` and rewriting its install names. 45 dylibs, about 36 MB.

Three things here were each learned the hard way and are asserted at the end:

- **The walk follows `@rpath` references, not only absolute ones.** Homebrew's
  OpenEXR names libIlmThread as `@rpath/libIlmThread-3_4.33.dylib`, so a walk that
  followed absolute paths alone silently dropped the whole OpenEXR family - and
  the resulting libvips then failed to load with nothing saying why on a machine
  that had Homebrew to fall back on.
- **Libraries are keyed by the basename of the install *name*, not of the real
  file.** Homebrew's `libpng16.16.dylib` is a symlink and dependents record the
  symlink's name.
- **Every rewritten file is re-signed.** `install_name_tool` invalidates the
  signature, and dyld refuses a present-but-wrong one - so an unsigned rewrite
  fails at launch rather than here.

It also writes `src-tauri/tauri.frameworks.json`, the `bundle.macOS.frameworks`
list Tauri needs. Generated rather than maintained: the task that decides which
dylibs exist is the one that should write the list, and 45 paths are not something
to keep by hand. `mise run app` merges it with `--config`.

## `cargo xtask verify` - the gate

Runs `target/release/snappr-cli inspect` on a HEIC, in a stripped environment,
with `DYLD_PRINT_LIBRARIES=1`.

Six checks, and the last is the only one that cannot be faked by the machine it
runs on:

1. the HEIC header reads back at 2400x1600
2. **the HEVC pixels actually decode** - the mean of a flat patch is the value
   that went in. A header read is lazy; this is the check sharp's libvips fails
3. the HEIC loader was the one used, not some fallback
4. bit depth is preserved as 8
5. libvips reports its version
6. **nothing loaded from Homebrew** - every path dyld reports is checked, and one
   line from `/opt/homebrew` fails the build

Passing the first five without the sixth means nothing: dyld will quietly satisfy
a missing library from `/opt/homebrew/lib` and say so nowhere. Last run: 784
libraries loaded, 0 of them from Homebrew.

The HEIC fixture is written by `cargo test -p snappr-core --test fixtures`, using
the *system* libvips - it has to come from somewhere other than the thing under
test, since the claim is that the bundled libvips decodes a file it did not
create. It is not committed: a binary in the repo is a binary nobody can review.

**What this proves, precisely**: that a binary linked the way the app is linked
runs with nothing from Homebrew. It does not open the assembled `.app`. The app
binary carries the same two rpaths and links the same libvips, and `mise run app`
puts the same closure into `Contents/Frameworks`, so the linkage under test is the
shipped one - but if that ever stops being true, this is the sentence that was
wrong.

## Entitlements

`hardenedRuntime` is on, and `entitlements.plist` carries
`com.apple.security.cs.disable-library-validation` because the closure is ad-hoc
signed while the app is not signed with the same identity.

It deliberately does **not** ask for
`com.apple.security.cs.allow-dyld-environment-variables`, and it does not need to:
libvips is found through `LC_RPATH`, which hardened runtime does not strip. The
Ruby child needed `LD_LIBRARY_PATH` only because its FFI layer read that variable
itself and joined the paths by hand.
