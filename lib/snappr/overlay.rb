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

    # The window used when an orientation has no overlay: the whole frame, in the
    # same fractions-of-the-frame form OverlaySvg.photo_window returns.
    FULL_FRAME = { x: 0.0, y: 0.0, width: 1.0, height: 1.0 }.freeze

    def initialize(config = Snappr.config)
      @config = config
    end

    # Returns the path written. See #process when you want the details too.
    def call(source) = process(source).fetch(:destination)

    # Composites one photo and returns everything known about the operation.
    #
    # #call used to be the whole of this and returned a bare Pathname, which meant
    # the dimensions, orientation, chosen overlay, layout, bit depth and timings
    # were computed and then thrown into a log line. The daemon needs them as data
    # to hand to the UI, so they are collected and returned here instead. #call
    # stays as a delegate because a dozen tests and both CLI commands pass its
    # return value straight to Pathname methods.
    def process(source)
      source = Pathname.new(source).expand_path
      unless source.file?
        raise Problem.new(:photo_missing, file: source.basename, path: source)
      end

      started = Process.clock_gettime(Process::CLOCK_MONOTONIC)
      name = source.basename
      warnings = []

      image = load(source)
      exif = Exif.read(image)
      orientation = orientation_of(image.width, image.height)
      # nil is a legitimate answer: the operator can clear either orientation's
      # slot, and a cleared slot prints the crop with nothing composited over it.
      svg = @config.overlay_svg(orientation, warnings)
      Snappr.logger.info format(
        "%s: %dx%d, %s - %s", name, image.width, image.height, orientation,
        svg ? "using #{File.basename(svg)}" : "no overlay, printing the crop"
      )
      warn_on_aspect_mismatch(svg, orientation, warnings) if svg

      layout = layout_for(image, svg, orientation)
      Snappr.logger.info format(
        "%s: cropping %dx%d at (%d, %d), placing on a %dx%d canvas",
        name, layout[:photo_width], layout[:photo_height], layout[:crop_left], layout[:crop_top],
        layout[:canvas_width], layout[:canvas_height]
      )
      overlay = overlay_for(image, svg, layout[:canvas_width], layout[:canvas_height], orientation, warnings) if svg
      background = background_for(image)

      destination = destination_for(source)
      temp = temp_path_for(source)
      Snappr.logger.info "#{name}: writing #{destination.basename}"
      begin
        pipeline = ImageProcessing::Vips
          .source(source.to_s)
          # object-fit: cover, with nothing to scale - see layout_for.
          .crop(layout[:crop_left], layout[:crop_top], layout[:photo_width], layout[:photo_height])
          # Drop the photo onto the full canvas at the window's position. With no
          # overlay the canvas is the crop and the position is (0, 0), so this is
          # a no-op - left unconditional because it costs nothing and one chain
          # shape is easier to follow than two.
          .embed(layout[:photo_left], layout[:photo_top],
                 layout[:canvas_width], layout[:canvas_height],
                 extend: :background, background: background)
        pipeline = pipeline.composite(overlay, mode: "over", gravity: "north-west") if overlay
        # The canvas is opaque, so this only drops a redundant alpha channel.
        # Conditional because vips flatten is NOT a no-op on an image that has no
        # alpha to drop: it fails with "linear: vector must have 1 or 2 elements",
        # which points nowhere near the cause. Alpha is here only if the overlay
        # brought it or the source carried it, so ask about exactly that.
        pipeline = pipeline.flatten(background: background) if overlay || image.has_alpha?
        pipeline
          .convert("png")
          .saver(compression: @config.png_compression)
          .call(destination: temp.to_s)
        File.rename(temp, destination)
      ensure
        File.unlink(temp) if File.exist?(temp)
      end

      elapsed = Process.clock_gettime(Process::CLOCK_MONOTONIC) - started
      bytes = File.size(destination)
      Snappr.logger.info format(
        "%s: wrote %s (%s, %.1fs)",
        name, destination.basename, Snappr.human_size(bytes), elapsed
      )

      { source: source, destination: destination,
        source_width: image.width, source_height: image.height,
        orientation: orientation, svg: svg, layout: layout,
        output_width: layout[:canvas_width], output_height: layout[:canvas_height],
        # Follows the source and is never cast, so it is worth reporting: an
        # 8-bit JPEG stays 8-bit and a 10-bit HEIC comes out as a 16-bit PNG.
        depth: image.format == :ushort ? 16 : 8,
        dpi: [image.xres, image.yres],
        icc: image.get_fields.include?("icc-profile-data"),
        bytes: bytes, elapsed: elapsed, warnings: warnings,
        # Camera, lens and exposure, for the UI to put under the photo. Read off
        # the header #load already pulled in, so it costs nothing measurable.
        # Left out entirely when the photo carries none - a screenshot or a scan
        # should give the caller no key rather than an empty object.
        **(exif.empty? ? {} : { exif: exif }) }
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
    # A nil svg means no overlay: the window is the whole frame and there is no
    # seam to hide, so this comes out as a plain centre crop to the print ratio
    # with the canvas equal to the crop. Nothing special-cases it further down.
    def layout_for(image, svg, orientation)
      window = svg ? OverlaySvg.photo_window(svg, slot: slot_for(orientation)) : FULL_FRAME
      margin = svg ? SEAM_MARGIN : 0
      # The window's shape in output pixels depends on the canvas shape, which is
      # the paper's ratio.
      window_ratio = (window[:width] / window[:height]) * @config.print_ratio(orientation)
      crop = crop_area(image.width, image.height, window_ratio)

      # Hold back the seam margin before deriving the canvas, so growing the
      # photo by it later can never ask for more pixels than the photo has.
      canvas_width = ((crop[:width] - 2 * margin) / window[:width]).floor
      canvas_height = ((crop[:height] - 2 * margin) / window[:height]).floor

      left = (window[:x] * canvas_width).round
      top = (window[:y] * canvas_height).round
      right = left + (window[:width] * canvas_width).round
      bottom = top + (window[:height] * canvas_height).round

      # Grow by the seam margin, clamped to the canvas.
      left, top = [left - margin, 0].max, [top - margin, 0].max
      right, bottom = [right + margin, canvas_width].min, [bottom + margin, canvas_height].min
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
    def overlay_for(image, svg, width, height, orientation, warnings = nil)
      overlay = OverlaySvg.render(svg, width, height, warnings: warnings, slot: slot_for(orientation))
      profile = image.get("icc-profile-data") if image.get_fields.include?("icc-profile-data")
      return overlay unless profile

      Snappr.logger.debug "converting the overlay into the photo's colour space"
      Tempfile.create(["snappr-profile", ".icc"]) do |file|
        file.binmode
        file.write(profile)
        file.flush
        overlay.icc_transform(file.path, input_profile: "srgb")
      end
    rescue Vips::Error => e
      Problem.warn(warnings, :profile_unusable, error: e.message.lines.first.strip)
      overlay
    end

    # How a message names an overlay slot: the same words the Overlays menu uses,
    # so the operator knows which of the two to go and fix.
    def slot_for(orientation) = "#{orientation} overlay"

    # The overlay is stretched to fill the frame, so one drawn at the wrong shape
    # is silently distorted. Say so rather than letting it look like a rendering
    # bug: an overlay for this pipeline should be drawn at the paper's ratio.
    def warn_on_aspect_mismatch(svg, orientation, warnings = nil)
      drawn = OverlaySvg.aspect_ratio(svg, slot: slot_for(orientation))
      wanted = @config.print_ratio(orientation)
      return if drawn.nil? || (drawn - wanted).abs <= wanted * 0.01

      width, height = if orientation == :landscape
        [@config.print_long_mm * 20, @config.print_short_mm * 20]
      else
        [@config.print_short_mm * 20, @config.print_long_mm * 20]
      end

      Problem.warn(
        warnings, :aspect_mismatch,
        slot: slot_for(orientation), file: File.basename(svg), orientation: orientation,
        width: width.round, height: height.round,
        # The two ratios are the developer's half: they say how far off it is,
        # which is no help at all to someone about to redraw it at the size the
        # sentence already names.
        drawn: format("%.3f", drawn), wanted: format("%.3f", wanted)
      )
    end
  end
end
