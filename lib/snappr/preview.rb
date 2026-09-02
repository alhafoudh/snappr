# frozen_string_literal: true

require "digest"
require "fileutils"
require "vips"

module Snappr
  # The JPEGs the UI displays. Three per photo: a filmstrip thumbnail, a large
  # preview of the composite, and a large preview of the untouched source frame
  # for the overlayed/original toggle.
  #
  # **The thumbnail is made from the original photo, not from the composite.** At
  # 132x84 the branding panel is a fifth of the cell and identical on every one of
  # them, so a strip of composites is a strip of near-identical logos - the whole
  # frame is what tells the operator which photo a cell is. The composite is what
  # the stage shows, and the toggle is there for comparing them.
  #
  # These exist because the webview cannot be shown the real output. Tauri's asset
  # protocol reads the whole file into memory for every request, and the overlay
  # PNGs are 10-25 MB and 16-bit when the source was HEIC - a filmstrip of those
  # would exhaust memory long before it finished drawing.
  #
  # Three decisions here are load-bearing:
  #
  # 1. **Previews are derived from the finished files on disk, not spliced into
  #    the compositing pipeline.** Re-reading the written PNG costs a fraction of a
  #    second. Fusing this into the ImageProcessing chain in Overlay#process would
  #    save that and put the invariant those tests guard - the photo bit-identical
  #    to the source wherever the overlay is clear - at risk for nothing.
  #
  # 2. **Previews are never written into the watched folder.** `jpg` is a watched
  #    extension, so a preview sitting beside the original would pass the watcher's
  #    candidate? filter and the pipeline would eat its own output, forever. The
  #    overlay PNG gets away with living there only because `png` is not watched.
  #    This is why the destination directory is a required argument with no default
  #    that could quietly point at the drop folder.
  #
  # 3. **Previews are converted to sRGB.** Photos from an iPhone are Display P3.
  #    A webview interprets an untagged JPEG as sRGB, so handing it P3 numbers
  #    unconverted shows the operator colours that are not what will print.
  #    `export_profile` does the conversion and tags the result, and it copes with
  #    a source that has no profile at all by assuming sRGB.
  class Preview
    KINDS = %i[thumb overlay source].freeze

    def initialize(config = Snappr.config, dir:)
      @config = config
      @dir = Pathname.new(dir).expand_path
    end

    # Writes the three previews and returns
    # { key:, thumb:, overlay:, source: } with Pathnames.
    #
    # `destination` is the overlay PNG that Overlay#process just wrote; `source` is
    # the original photo.
    def call(source:, destination:)
      source = Pathname.new(source)
      destination = Pathname.new(destination)
      key = key_for(source, destination)
      FileUtils.mkdir_p(@dir)

      { key: key,
        thumb: write(source, key, :thumb, @config.preview_thumb_px),
        overlay: write(destination, key, :overlay, @config.preview_max_px),
        source: write(source, key, :source, @config.preview_max_px) }
    end

    def path_for(key, kind) = @dir.join("#{key}.#{kind}.jpg")

    # Stable across runs so a restart can find the previews the index points at,
    # and keyed on mtime as well as path so replacing a photo with a different one
    # under the same name does not serve the old preview.
    #
    # **The composite is in the key too, and that is what stops the window showing
    # yesterday's branding.** The UI draws these files through `asset://` URLs built
    # straight out of these filenames, the responses carry no cache validators, and
    # the page never reloads - so a URL the webview has already fetched comes back
    # out of its own memory cache for the life of the session. Keyed on the source
    # alone, clearing a photo from the strip, picking a different overlay and
    # dropping the same file in again reproduced the identical key: byte-different
    # JPEGs written to exactly the URLs the webview was already holding, so the
    # operator kept seeing the overlay they had just replaced while the PNG on disk
    # was the new one. Every composite now names its own files instead.
    #
    # The superseded set is deleted by the shell, which knows both the record that
    # was there before and the one that replaced it - see `discard_superseded` in
    # `src-tauri/src/lib.rs`. Ruby has no index and cannot tell an old preview of
    # this photo from a current preview of another.
    def key_for(source, destination)
      Digest::SHA256.hexdigest("#{stamp(source)}\0#{stamp(destination)}")[0, 16]
    end

    private

    # Path and mtime, to the nanosecond. Seconds are not enough: a retry straight
    # after a failed composite can land inside the same second, and a key that
    # cannot tell two composites apart is the whole thing this is here to avoid.
    def stamp(path)
      path = Pathname.new(path)
      mtime = path.exist? ? path.mtime : nil
      format("%s\0%d.%09d", path.expand_path, mtime&.to_i || 0, mtime&.nsec || 0)
    end

    def write(from, key, kind, max_px)
      target = path_for(key, kind)
      # Same reasoning as Overlay#temp_path_for: write beside the target and
      # rename, so a reader never sees a half-written JPEG. The suffix keeps it
      # distinguishable and safe against a second process.
      temp = @dir.join("#{key}.#{kind}.tmp-#{Process.pid}.jpg")

      # size: :down never enlarges, so a photo smaller than max_px is copied at
      # its own size rather than upscaled into blur.
      image = Vips::Image.thumbnail(from.to_s, max_px, height: max_px, size: :down,
                                                       export_profile: "srgb")
      image.write_to_file(temp.to_s, Q: @config.preview_quality, optimize_coding: true)
      File.rename(temp, target)
      target
    ensure
      File.unlink(temp) if temp && File.exist?(temp)
    end
  end
end
