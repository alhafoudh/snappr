# frozen_string_literal: true

require "fileutils"
require "vips"

module Snappr
  # The PNG the overlay panel draws for one overlay, and where the photo window
  # landed inside it.
  #
  # **This exists to be a check, not a decoration.** The panel paints a grey
  # rectangle with a photo icon at the window this returns and then draws this PNG
  # over it, so the operator sees the photo area exactly where snappr found it. An
  # overlay whose magenta marker did not survive the export shows the icon in the
  # wrong place, or says outright that there is no window - before a print is made
  # rather than after.
  #
  # That is why it goes through librsvg rather than being handed to the webview as
  # an SVG. The webview would render the file with its own engine, and would render
  # the marker as a magenta block; what the panel has to show is what *this*
  # renderer, the one the print uses, made of the file.
  #
  # Two things follow from the render having a transparent photo window:
  #
  # 1. **The output is PNG.** JPEG has no alpha and would flatten the window to
  #    black, which is the one part of the image the operator is here to look at.
  # 2. **The panel supplies the ground.** Everything the artwork does not cover is
  #    transparent, exactly as it is at composite time, and the panel paints the
  #    configured background behind it so the card reads the way the print will.
  #
  # The destination directory is a required argument with no default, for the same
  # reason it is one in Preview: a default could quietly point at the drop folder,
  # and the pipeline eating its own output is the failure that rule exists to stop.
  # These are PNGs and `png` is not a watched extension, so this is belt and braces
  # rather than critical - but it is the same rule and it is cheap to keep.
  module OverlayPreview
    # The long edge of the render, in pixels. Twice the panel's card width, so the
    # card is sharp on a retina display and nothing larger is decoded for a
    # thumbnail that is 220px wide.
    BOX_PX = 440

    class << self
      # Renders `svg` into `dir` as `<key>.png` and returns
      #
      #   { thumb:, width:, height:, aspect:, window:, warnings: }
      #
      # `window` is { x:, y:, width:, height: } in 0..1, or **nil when the marker
      # was not found**. That case is deliberately not an error: the artwork is
      # still worth showing, and a card that renders the design and says the photo
      # window is missing tells the operator far more than an empty cell does. The
      # sentence travels in `warnings` like any other degradation.
      def call(svg:, dir:, key:, box: BOX_PX)
        svg = svg.to_s
        warnings = []
        aspect = ratio_of(svg, warnings)
        width, height = dimensions(aspect, box)

        image = OverlaySvg.render(svg, width, height, warnings: warnings, slot: slot)
        thumb = write(image, Pathname.new(dir).expand_path, key)

        { thumb: thumb, width: width, height: height, aspect: aspect,
          window: window_of(svg, warnings), warnings: warnings }
      end

      private

      # The panel is not one of the two orientation settings, so there is no slot
      # to name. "overlay" is what OverlaySvg falls back to, and every sentence in
      # the catalogue reads correctly with it.
      def slot = nil

      # The shape the overlay was drawn at, so the card shows the design as
      # authored. An overlay drawn at the wrong ratio therefore looks wrong on its
      # card, which is a second check the operator gets for nothing.
      #
      # Falls back to the paper's landscape ratio, which is the only sensible
      # guess and keeps the two numbers Ruby owns about layout in one place.
      def ratio_of(svg, warnings)
        aspect = OverlaySvg.aspect_ratio(svg, slot: slot)
        return aspect if aspect&.finite? && aspect.positive?

        Problem.warn(warnings, :overlay_no_shape, file: File.basename(svg), path: svg)
        Snappr.config.print_ratio(:landscape)
      end

      # Fitted inside a box rather than scaled off the width, so a tall portrait
      # overlay cannot ask for a render several thousand pixels high.
      def dimensions(aspect, box)
        return [box, [(box / aspect).round, 1].max] if aspect >= 1

        [[(box * aspect).round, 1].max, box]
      end

      # A missing window is the thing this whole file is here to make visible, so
      # it is caught rather than raised. Anything else - an unreadable file, no
      # size - would already have come out of the render above.
      def window_of(svg, warnings)
        OverlaySvg.photo_window(svg, slot: slot)
      rescue Problem => e
        Snappr.logger.warn e.log_line
        warnings.push({ kind: e.code.to_s, message: e.message })
        nil
      end

      # Written beside the target and renamed, like Preview#write and
      # Overlay#call: the shell builds an asset URL out of this path as soon as
      # the frame lands, so a reader must never see a half-written PNG.
      def write(image, dir, key)
        FileUtils.mkdir_p(dir)
        target = dir.join("#{key}.png")
        temp = dir.join("#{key}.tmp-#{Process.pid}.png")

        image.write_to_file(temp.to_s)
        File.rename(temp, target)
        target
      ensure
        File.unlink(temp) if temp && File.exist?(temp)
      end
    end
  end
end
