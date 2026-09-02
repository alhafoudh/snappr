# frozen_string_literal: true

require "rexml/document"
require "vips"

module Snappr
  # Renders an overlay SVG at an exact pixel size, and locates the photo window
  # inside it.
  #
  # Two rules govern this file:
  #
  # 1. **The authored viewBox is preserved.** It is what maps the artwork's own
  #    coordinate system onto the output, so setting width and height is enough to
  #    scale a whole design to any photo size. Overwriting it strands the artwork
  #    at its authored coordinates. This is why overlays can be drawn in any tool.
  #
  # 2. **A #FF00FF rectangle marks where the photo goes**, and is never rendered.
  #    It is a guide for the designer, not part of the artwork.
  module OverlaySvg
    # Magenta, because nothing in real branding is this colour by accident.
    MARKER = [255, 0, 255].freeze
    # Generous, to catch a marker that has been through a colour-managed export.
    MARKER_TOLERANCE = 60
    MARKER_PATTERN = /\A\s*(#ff00ff|#f0f|magenta|rgb\(\s*255\s*,\s*0\s*,\s*255\s*\))\s*\z/i

    class << self
      # Returns an RGBA Vips::Image of exactly width x height, with the photo
      # window left transparent.
      #
      # Pass an array as `warnings` to collect anything worth showing an operator
      # as well as logging it. An out parameter rather than a changed return type,
      # so the callers that only want the image are untouched.
      #
      # `slot` names the setting this file came from ("landscape overlay"), and is
      # only ever used in the messages. This module knows nothing about
      # orientations, but the operator's mental model is the Overlays menu they
      # will go and fix it in, not a bare filename - so the caller that does know
      # says which slot, and every message here can name it.
      def render(svg_path, width, height, warnings: nil, slot: nil)
        image = load_svg(stripped_source(svg_path, width, height, slot: slot))
        image = image.has_alpha? ? image : image.add_alpha
        key_out_marker(image, svg_path, warnings, slot)
      end

      # Where the photo goes, as fractions of the frame: { x:, y:, width:, height: }
      # in 0..1. Fractions rather than pixels, because the canvas size is derived
      # from them later.
      #
      # Found by rendering and matching the colour, not by reading the rect's
      # geometry. Exporters wrap shapes in groups, transforms and clip paths, and
      # a rendered pixel is the one thing that accounts for all of them.
      def photo_window(svg_path, slot: nil)
        _, root = parse(svg_path, slot)
        reference = view_box(root, svg_path, slot).split(/[\s,]+/).map(&:to_f)
        width, height = reference[2].round, reference[3].round

        image = load_svg(source_for(svg_path, width, height, slot: slot))
        box = marker_bounds(image)
        raise Problem.new(:overlay_window_missing, **about(svg_path, slot)) unless box

        { x: box[:left].fdiv(width), y: box[:top].fdiv(height),
          width: box[:width].fdiv(width), height: box[:height].fdiv(height) }
      end

      # The aspect ratio the overlay was drawn at.
      def aspect_ratio(svg_path, slot: nil)
        _, root = parse(svg_path, slot)
        _, _, width, height = view_box(root, svg_path, slot).split(/[\s,]+/).map(&:to_f)
        height.zero? ? nil : width / height
      end

      # The document resized but otherwise untouched - marker included.
      def source_for(svg_path, width, height, slot: nil)
        document, root = parse(svg_path, slot)
        resize(root, svg_path, width, height, slot)
        document.to_s
      end

      # The document resized with every marker element removed.
      def stripped_source(svg_path, width, height, slot: nil)
        document, root = parse(svg_path, slot)
        resize(root, svg_path, width, height, slot)
        remove_marker_elements(root)
        document.to_s
      end

      private

      # The arguments every overlay entry in the catalogue takes. `slot` is
      # "landscape overlay" when the caller said which setting this file came
      # from and plain "overlay" when it did not, so the sentences read the same
      # either way; `file` is the basename the operator sees in the Overlays
      # menu, and `path` is the full one, which only the console gets.
      def about(svg_path, slot) = { slot: slot || "overlay", file: File.basename(svg_path), path: svg_path }

      def load_svg(source) = Vips::Image.new_from_buffer(source, "")

      def resize(root, svg_path, width, height, slot)
        root.add_attributes(
          "width" => width.to_s,
          "height" => height.to_s,
          "viewBox" => view_box(root, svg_path, slot),
          # Fill the frame exactly. The canvas is derived from the print ratio, so
          # the only stretch is rounding residue. The default "meet" would
          # letterbox that residue and leave a transparent sliver along one edge.
          "preserveAspectRatio" => "none"
        )
      end

      def remove_marker_elements(element)
        element.elements.to_a.each do |child|
          if marker?(child)
            element.delete_element(child)
          else
            remove_marker_elements(child)
          end
        end
      end

      def marker?(element)
        return true if MARKER_PATTERN.match?(element.attributes["fill"].to_s)

        style = element.attributes["style"].to_s
        style[/fill\s*:\s*([^;]+)/i] ? MARKER_PATTERN.match?(Regexp.last_match(1)) : false
      end

      # Safety net for a marker whose colour arrives through something the
      # element walk cannot see - a CSS class, a gradient, a <use>. Better a
      # transparent hole than a magenta block on the print.
      def key_out_marker(image, svg_path, warnings = nil, slot = nil)
        mask = marker_mask(image)
        return image if mask.avg.zero?

        Problem.warn(warnings, :marker_survived, **about(svg_path, slot))
        alpha = image.extract_band(3).boolean(mask.relational_const(:more, 128).ifthenelse(0, 255), :and)
        image.extract_band(0, n: 3).bandjoin(alpha)
      end

      def marker_bounds(image)
        mask = marker_mask(image)
        return nil if mask.avg.zero?

        left, top, width, height = mask.find_trim(background: 0, threshold: 128)
        width.zero? || height.zero? ? nil : { left: left, top: top, width: width, height: height }
      end

      # 255 where the pixel is marker-coloured and opaque, 0 elsewhere.
      def marker_mask(image)
        red, green, blue = image.extract_band(0), image.extract_band(1), image.extract_band(2)
        scale = image.format == :ushort ? 257 : 1
        # Inclusive bounds: the marker's own channels are 0 and 255, which
        # exclusive comparisons would reject outright.
        near = lambda do |band, target|
          low = (target - MARKER_TOLERANCE).clamp(0, 255) * scale
          high = (target + MARKER_TOLERANCE).clamp(0, 255) * scale
          band.relational_const(:moreeq, low).boolean(band.relational_const(:lesseq, high), :and)
        end

        mask = near.(red, MARKER[0]).boolean(near.(green, MARKER[1]), :and).boolean(near.(blue, MARKER[2]), :and)
        mask = mask.boolean(image.extract_band(3).relational_const(:moreeq, 128 * scale), :and) if image.has_alpha?
        mask.ifthenelse(255, 0)
      end

      def parse(svg_path, slot = nil)
        raise Problem.new(:overlay_file_missing, **about(svg_path, slot)) unless File.exist?(svg_path)

        document = REXML::Document.new(File.read(svg_path))
        root = document.root or raise Problem.new(:overlay_unreadable, **about(svg_path, slot))
        [document, root]
      end

      # Keep the authored viewBox. A file without one is using its width and
      # height as the coordinate system, so synthesise the equivalent.
      def view_box(root, svg_path, slot = nil)
        authored = root.attributes["viewBox"]
        return authored if authored && !authored.strip.empty?

        width = length(root.attributes["width"])
        height = length(root.attributes["height"])
        raise Problem.new(:overlay_no_size, **about(svg_path, slot)) unless width && height

        "0 0 #{width} #{height}"
      end

      # "2000", "2000px", "2000.5pt" -> 2000.0. Percentages are meaningless
      # without a parent viewport, so they are refused.
      def length(value)
        return nil if value.nil? || value.strip.end_with?("%")

        Float(value.strip[/\A[0-9.]+/].to_s, exception: false)
      end
    end
  end
end
