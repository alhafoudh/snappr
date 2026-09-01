# frozen_string_literal: true

require "digest"
require "fileutils"
require "vips"

module Snappr
  # The JPEGs the UI displays. Three per photo: a filmstrip thumbnail, a large
  # preview of the composite, and a large preview of the untouched source frame
  # for the overlayed/original toggle.
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
      key = key_for(source)
      FileUtils.mkdir_p(@dir)

      { key: key,
        thumb: write(destination, key, :thumb, @config.preview_thumb_px),
        overlay: write(destination, key, :overlay, @config.preview_max_px),
        source: write(source, key, :source, @config.preview_max_px) }
    end

    def path_for(key, kind) = @dir.join("#{key}.#{kind}.jpg")

    # Stable across runs so a restart can find the previews the index points at,
    # and keyed on mtime as well as path so replacing a photo with a different one
    # under the same name does not serve the old preview.
    def key_for(source)
      source = Pathname.new(source)
      mtime = source.exist? ? source.mtime.to_i : 0
      Digest::SHA256.hexdigest("#{source.expand_path}\0#{mtime}")[0, 16]
    end

    private

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
