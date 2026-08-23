# frozen_string_literal: true

require "image_processing/vips"
require "tempfile"

module Snappr
  # Fits one photo into the overlay's #FF00FF window, composites the overlay over
  # it, and writes <basename>.overlay.png beside the original.
  #
  # The photo is never scaled: the output canvas is sized so that the window lands
  # at the photo's own resolution, which makes the fit a pure centre crop. See the
  # "Nothing is allowed to degrade the photo" section of CLAUDE.md.
  class Overlay
    # The photo is laid down first and the overlay goes over it, but the overlay
    # has a hole where the window is, so a rounding disagreement between the two
    # would show the background through the join. Growing the photo past the
    # window costs a few pixels of resolution and cannot leave a gap.
    #
    # Two pixels rather than one, for headroom: when the window's edge falls on a
    # fractional coordinate librsvg spreads the anti-aliased transition over as
    # much as three pixels. Measured leakage is zero at a margin of 1, 2 or 3, so
    # this is insurance rather than a fix, bought for 0.1% of the photo's width.
    SEAM_MARGIN = 2

    def initialize(config = Snappr.config)
      @config = config
    end

    # Returns the path written.
    def call(source)
      source = Pathname.new(source).expand_path
      raise Error, "no such file: #{source}" unless source.file?

      image = load(source)
      orientation = orientation_of(image.width, image.height)
      svg = @config.overlay_svg(orientation)
      warn_on_aspect_mismatch(svg, orientation)

      layout = layout_for(image, svg, orientation)
      overlay = overlay_for(image, svg, layout[:canvas_width], layout[:canvas_height])
      background = background_for(image)

      destination = destination_for(source)
      temp = temp_path_for(source)
      begin
        ImageProcessing::Vips
          .source(source.to_s)
          # object-fit: cover, with nothing to scale - see layout_for.
          .crop(layout[:crop_left], layout[:crop_top], layout[:photo_width], layout[:photo_height])
          # Drop the photo onto the full canvas at the window's position.
          .embed(layout[:photo_left], layout[:photo_top],
                 layout[:canvas_width], layout[:canvas_height],
                 extend: :background, background: background)
          .composite(overlay, mode: "over", gravity: "north-west")
          # The canvas is opaque, so this only drops a redundant alpha channel.
          .flatten(background: background)
          .convert("png")
          .saver(compression: @config.png_compression)
          .call(destination: temp.to_s)
        File.rename(temp, destination)
      ensure
        File.unlink(temp) if File.exist?(temp)
      end

      destination
    end

    def destination_for(source)
      source = Pathname.new(source)
      source.dirname.join(stem(source) + @config.output_suffix)
    end

    def temp_path_for(source)
      source = Pathname.new(source)
      source.dirname.join("#{stem(source)}.tmp-#{Process.pid}#{@config.output_suffix}")
    end

    # A square photo is treated as landscape. Arbitrary, but it has to be one of
    # the two and landscape is usually the roomier layout.
    def orientation_of(width, height) = width >= height ? :landscape : :portrait

    # Works out the canvas size, the crop taken from the photo, and where that
    # crop is placed.
    #
    # The canvas is derived from the photo rather than fixed, so that the window
    # comes out at exactly the size of the largest crop the photo can supply at
    # the window's shape. That makes the fit a pure centre crop - object-fit:
    # cover with a scale factor of 1 - and keeps the photo bit-for-bit intact.
    def layout_for(image, svg, orientation)
      window = OverlaySvg.photo_window(svg)
      # The window's shape in output pixels depends on the canvas shape, which is
      # the paper's ratio.
      window_ratio = (window[:width] / window[:height]) * @config.print_ratio(orientation)
      crop = crop_area(image.width, image.height, window_ratio)

      # Hold back the seam margin before deriving the canvas, so growing the
      # photo by it later can never ask for more pixels than the photo has.
      canvas_width = ((crop[:width] - 2 * SEAM_MARGIN) / window[:width]).floor
      canvas_height = ((crop[:height] - 2 * SEAM_MARGIN) / window[:height]).floor

      left = (window[:x] * canvas_width).round
      top = (window[:y] * canvas_height).round
      right = left + (window[:width] * canvas_width).round
      bottom = top + (window[:height] * canvas_height).round

      # Grow by the seam margin, clamped to the canvas.
      left, top = [left - SEAM_MARGIN, 0].max, [top - SEAM_MARGIN, 0].max
      right, bottom = [right + SEAM_MARGIN, canvas_width].min, [bottom + SEAM_MARGIN, canvas_height].min
      photo_width, photo_height = right - left, bottom - top

      { canvas_width: canvas_width, canvas_height: canvas_height,
        photo_left: left, photo_top: top,
        photo_width: photo_width, photo_height: photo_height,
        crop_left: (image.width - photo_width) / 2, crop_top: (image.height - photo_height) / 2 }
    end

    # The largest box of the given ratio that fits inside width x height, centred.
    # Taking the largest *inscribed* box is what keeps the fit a pure crop.
    def crop_area(width, height, ratio)
      if width.fdiv(height) > ratio
        crop_width, crop_height = (height * ratio).round, height
      else
        crop_width, crop_height = width, (width / ratio).round
      end

      { left: (width - crop_width) / 2, top: (height - crop_height) / 2,
        width: crop_width, height: crop_height }
    end

    def crop_dimensions(width, height, ratio)
      crop_area(width, height, ratio).values_at(:width, :height)
    end

    private

    def stem(source) = source.basename(source.extname).to_s

    # Dimensions and the colour profile must both be read AFTER autorot:
    # ImageProcessing autorotates on load too, so anything measured from the
    # pre-rotation header is transposed on every portrait phone photo. That
    # decides the orientation, and so which overlay and which crop are used.
    # libvips is lazy, so this touches the header, not the pixels.
    def load(source) = Vips::Image.new_from_file(source.to_s).autorot

    # Whatever the overlay does not cover and the photo does not fill. White by
    # default, which is what unprinted photo paper looks like anyway.
    def background_for(image)
      scale = image.format == :ushort ? 257 : 1
      @config.background_rgb.map { |channel| channel * scale }
    end

    # The overlay is authored in sRGB, because that is what librsvg renders in.
    # If the photo is in another space - iPhones shoot Display P3 - the overlay's
    # colours have to be converted into it, or they are reinterpreted and come out
    # wrong. White and black are identical in both, so this is a no-op for
    # achromatic branding and insurance for anything with colour in it.
    def overlay_for(image, svg, width, height)
      overlay = OverlaySvg.render(svg, width, height)
      profile = image.get("icc-profile-data") if image.get_fields.include?("icc-profile-data")
      return overlay unless profile

      Tempfile.create(["snappr-profile", ".icc"]) do |file|
        file.binmode
        file.write(profile)
        file.flush
        overlay.icc_transform(file.path, input_profile: "srgb")
      end
    rescue Vips::Error => e
      Snappr.logger.warn "colour profile unusable, compositing the overlay as sRGB: #{e.message.lines.first.strip}"
      overlay
    end

    # The overlay is stretched to fill the frame, so one drawn at the wrong shape
    # is silently distorted. Say so rather than letting it look like a rendering
    # bug: an overlay for this pipeline should be drawn at the paper's ratio.
    def warn_on_aspect_mismatch(svg, orientation)
      drawn = OverlaySvg.aspect_ratio(svg)
      wanted = @config.print_ratio(orientation)
      return if drawn.nil? || (drawn - wanted).abs <= wanted * 0.01

      Snappr.logger.warn format(
        "%s is drawn at %.3f:1 but %s prints at %.3f:1 - it will be stretched. " \
        "Redraw it at %d x %d (or any size with that ratio).",
        File.basename(svg), drawn, orientation, wanted,
        *(orientation == :landscape ? [@config.print_long_mm * 20, @config.print_short_mm * 20]
                                    : [@config.print_short_mm * 20, @config.print_long_mm * 20])
      )
    end
  end
end
