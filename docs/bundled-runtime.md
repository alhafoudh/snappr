# The bundled runtime

`Snappr.app` ships its own Ruby and its own libvips, so it runs on an operator's
Mac that has never heard of Homebrew. `mise run runtime` builds the lot into
`build/`, which is gitignored - **the scripts are the source of truth, not their
output.**

```
mise run runtime          all five stages, in order
mise run runtime-ruby     bin/build-runtime      the relocatable interpreter
mise run runtime-libvips  bin/build-libvips      libvips from source
mise run runtime-vips     bin/build-vips         the relocated dependency closure
mise run runtime-gems     bin/build-gems         the vendored gems
mise run runtime-verify   bin/verify-bundle      the gate
```

**Order matters**: `build-vips` walks the dependency closure of the libvips that
`build-libvips` produces, so it cannot run first. The individual tasks exist for
iterating on one stage.

**Each script's header carries the full argument** - which configure flag exists
for what, which alternative was measured and rejected, and what the failure looks
like. This file is the map and the short version; read the header before changing
a stage.

## What shapes the layout: codesigning

Tauri signs nested code in `MacOS`, `Frameworks`, `Plugins`, `Helpers`,
`XPCServices` and `Libraries` - **notably not `Resources`**. So every Mach-O file
has to land in one of those, and only inert data may go in `Resources`. That single
constraint is why the Ruby stdlib is built with no `.bundle` files at all, why
`Contents/Frameworks` must stay flat, and why the planned bundle layout in
`runtime.rs` splits the pieces the way it does.

The bundle layout is still unwired - `bundle.resources` names `sample/` and
nothing else - so `Runtime::detect` always takes the development branch today. See
`docs/desktop-app.md`.

## `bin/build-runtime` - the interpreter

**mise's Ruby cannot ship.** Its prefix is compiled in, so a copied binary looks
for its stdlib back in `~/.local/share/mise`, and it links out to Homebrew's
libgmp.

The flags that matter:

- `--enable-load-relative` - the whole point. The binary finds its stdlib at
  `<bin>/../lib/ruby/<ver>` instead of the compiled-in prefix.
- `--disable-shared` - links libruby into the binary. One less dylib to relocate
  and one less Mach-O to sign.
- `--with-static-linked-ext` - links every stdlib C extension in, so there are no
  stdlib `.bundle` files at all. This matters more than it looks, and the reason
  is the codesigning rule above: a surviving `.bundle` under `Resources` would
  have to be moved into `Contents/Frameworks` by hand and would fail notarization
  if it were missed.
- `--with-libyaml-dir` at a staging directory holding only `libyaml.a`, so psych
  links it statically. psych is not optional: it reads `config/snappr.yml`.
- `--without-gmp` and `--with-out-ext=...` drop Homebrew dylibs (libssl,
  libcrypto, libreadline, libncurses, libgdbm) that snappr cannot reach anyway.

openssl is excluded deliberately: snappr does no network I/O, the bundled app
never installs gems, `SecureRandom` uses `Random.urandom`, and digest is its own
extension. The header justifies it at length - read it before adding it back.

## `bin/build-libvips` - libvips from source

**Not Homebrew's, because it cannot be relocated.** From 8.16 libvips ships its
format support as modules it dlopens at runtime from
`<libdir>/vips-modules-<major>.<minor>/`, and `libdir` is **not** derived from the
library's own location: `guess_prefix()` tests the compile-time `VIPS_LIBDIR`
first and returns the compile-time prefix if the module directory exists there.

So a copied Homebrew libvips loads its modules back out of the Cellar, dragging a
second copy of libvips and glib into the process - visible as an objc
duplicate-class warning - after which `heifload` resolves against the wrong
instance and reports itself unavailable. On a machine with no Homebrew there are
no modules at all, and no HEIC support.

**`-Dmodules=disabled` removes the whole problem**: heif, rsvg, png and jpeg are
linked directly into `libvips.42.dylib`, so a dependency walk can see everything
and `Contents/Frameworks` stays flat.

**Not sharp's prebuilt libvips either.** `@img/sharp-libvips-darwin-arm64` is a
single 18 MB dylib with no third-party dependencies and it very nearly works - it
loads clean, renders SVG, and reports `heifload` as present. But it cannot decode
HEVC, because sharp excludes it for patent-licensing reasons and supports only
AVIF in HEIF containers. **iPhone HEICs are HEVC.** Note that its header read
succeeded: libvips is lazy, so only a real pixel decode of a real HEVC file
catches this - which is exactly why `verify-bundle` exists.

Turned off: everything snappr cannot reach. magick, poppler, openslide and jpeg-xl
were the module formats; openexr dragged in the whole OpenEXR/IlmThread family
through `@rpath`; pangocairo and fontconfig are only for vips' own text rendering,
which this pipeline never uses - text in an overlay is drawn by librsvg, which
brings its own pango.

## `bin/build-vips` - relocating the closure

Homebrew's dylibs reference each other by absolute path, so a copy of libvips
taken as-is still loads Homebrew's glib, and on a machine with no Homebrew loads
nothing. Every reference is rewritten to `@loader_path`, which is what makes a
flat directory of dylibs work wherever it is put.

Four things are load-bearing, and the header states them as such:

1. **Files are keyed by the basename of the install name, not of the real file.**
   Homebrew's `libpng16.16.dylib` is a symlink to a versioned file, but every
   dependent records the symlink's name. Copy the real bytes under the name the
   dependents ask for, or nothing resolves.
2. **`@rpath` dependencies have to be followed too.** Some formulae refer to
   siblings as `@rpath/libfoo.dylib`, and resolving that needs the dependent's own
   `LC_RPATH` entries, which are themselves often `@loader_path`-relative. A walk
   that follows only absolute paths drops those silently and libvips fails to
   dlopen.
3. **The directory must end up flat, with only real Mach-O files.** Tauri's
   bundler runs codesign over everything in `Contents/Frameworks` and aborts on
   the first thing it cannot sign, so a stray symlink or subdirectory is a build
   failure much later and far from its cause.
4. **`install_name_tool` invalidates the signature**, so each file is re-signed
   ad-hoc afterwards. Tauri re-signs with the Developer ID at bundle time, but
   dyld refuses to load a file whose signature is present and wrong - which makes
   this fail during development rather than at distribution.

## `bin/build-gems` - the vendored gems

**`bundle install --standalone` is what makes this work.** It generates a
`bundler/setup.rb` that only manipulates `$LOAD_PATH` using `__dir__`-relative
paths, so the bundled app needs neither bundler nor rubygems at runtime and the
tree can be copied into the `.app` unchanged. That is also why the sidecar can be
launched with `ruby --disable-gems`, which is faster to boot.

**It runs under mise's Ruby, not the bundled one**, because the bundled
interpreter is built without openssl and cannot fetch over HTTPS. That is safe and
it was checked rather than assumed: the only gem with a compiled artefact is ffi,
and its precompiled arm64-darwin binary links nothing but libSystem, leaving its
`_rb_*` symbols to resolve against the host process at load time.

Two things are left out on purpose: the `:watch` group (listen, rb-fsevent - Rust
watches the folder inside the app, and rb-fsevent ships a compiled helper that
would be one more Mach-O to sign), and ffi's ABI directories for every Ruby but
ours (six copies of `ffi_c.bundle`, five of them dead signing surface). The kept
ABI directory is **derived from `RUBY_VERSION`, never hardcoded** - pinning the
wrong one prunes the only copy that matters.

## `bin/verify-bundle` - the gate

**This is the stage that matters, and it exists because of a specific trap.** FFI
hardcodes `/opt/homebrew/lib` into its library search path on macOS arm64, second
only to `LD_LIBRARY_PATH`. So a bundle that is missing a dylib **does not fail
here** - it silently borrows Homebrew's copy, passes every test on this machine,
and fails on the operator's Mac where Homebrew does not exist.

The structural checks in `build-vips` cannot catch it either: the thing that went
missing the first time round was not reachable from `otool` at all, because
libvips dlopened its format support at runtime, so the closure had no HEIC support
and no libheif and a dependency walk had no way to know.

Hence the two-part test, and **passing the first without the second means
nothing**:

1. Run the real pipeline on a real HEIC inside the bundled runtime.
2. Assert via `DYLD_PRINT_LIBRARIES` that not one library came from
   `/opt/homebrew`.

`LD_LIBRARY_PATH`, not `DYLD_LIBRARY_PATH`, is what points FFI at the bundled
libvips - FFI reads both from Ruby's `ENV` and does the path joining itself in
Ruby, so it works even though it means nothing to dyld on macOS, and unlike
`DYLD_*` it is not stripped under hardened runtime. That removes the need for the
`com.apple.security.cs.allow-dyld-environment-variables` entitlement. The running
app sets the same variable for the same reason - see `Runtime::env` in
`runtime.rs` and `docs/desktop-app.md`.
