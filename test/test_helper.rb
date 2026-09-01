# frozen_string_literal: true

require "minitest/autorun"
require "tmpdir"
require "tempfile"

# Keep the pipeline's progress logging out of the test output; warnings and
# errors still show. Set SNAPPR_LOG_LEVEL=info to see it while debugging.
ENV["SNAPPR_LOG_LEVEL"] ||= "warn"

require "snappr"

module Snappr
  module TestHelpers
    # A flat mid-grey photo, so any pixel that is not grey in the output came
    # from the overlay.
    GREY = 120

    def fixture_image(width, height, grey: GREY)
      base = Vips::Image.black(width, height).add(grey).cast(:uchar)
      base.bandjoin([base, base]).copy(interpretation: :srgb)
    end

    # 300 dpi, expressed the way libvips holds it: pixels per mm.
    DPI_300 = 300 / 25.4

    # Display P3, the space iPhones actually shoot in.
    P3_PROFILE = "/System/Library/ColorSync/Profiles/Display P3.icc"

    # A stand-in for what an iPhone writes, in the raw spellings EXIF uses - rationals
    # as fractions, the date with colons. Passed to write_fixture as `exif:`.
    APPLE_EXIF = {
      "exif-ifd0-Make" => "Apple",
      "exif-ifd0-Model" => "iPhone 17 Pro",
      "exif-ifd2-LensModel" => "iPhone 17 Pro back camera 6.765mm f/1.78",
      "exif-ifd2-FNumber" => "89/50",
      "exif-ifd2-ExposureTime" => "1/120",
      "exif-ifd2-ISOSpeedRatings" => "400",
      "exif-ifd2-FocalLength" => "677/100",
      "exif-ifd2-FocalLengthIn35mmFilm" => "24",
      "exif-ifd2-DateTimeOriginal" => "2026:03:21 12:40:52",
      "exif-ifd2-OffsetTimeOriginal" => "+01:00"
    }.freeze

    def write_fixture(dir, name, width, height,
                      orientation: nil, dpi: nil, marker_at: nil, profile: nil, deep: false,
                      exif: nil, **save)
      image = fixture_image(width, height)
      image = image.insert(Vips::Image.black(40, 40).bandjoin([0, 0]), *marker_at) if marker_at
      image = image.cast(:ushort).linear(257, 0).cast(:ushort) if deep
      image = image.copy(xres: dpi, yres: dpi) if dpi
      image = image.mutate { |m| m.set_type!(GObject::GINT_TYPE, "orientation", orientation) } if orientation
      # Set as plain strings and libvips writes real EXIF, which reads back in its own
      # annotated form ("89/50 (f/1.8, Rational, 1 components, 8 bytes)"). So a fixture
      # exercises the format Snappr::Exif actually has to parse, not a mock of it.
      if exif
        image = image.mutate do |m|
          exif.each { |tag, value| m.set_type!(GObject::GSTR_TYPE, tag, value) }
        end
      end
      if profile
        blob = File.binread(profile)
        image = image.mutate { |m| m.set_type!(Vips::BLOB_TYPE, "icc-profile-data", blob) }
      end

      path = File.join(dir, name)
      image.write_to_file(path, **save)
      path
    end

    def profile_of(image)
      image.get_fields.include?("icc-profile-data") ? image.get("icc-profile-data") : nil
    end

    # The photo pixels the pipeline should have placed, taken straight from the
    # source. Used to assert the photo survives bit-for-bit.
    #
    # `inset` trims the border where the overlay's anti-aliased window edge
    # legitimately blends artwork into the photo. Anything inside that is the
    # photo alone and must be untouched.
    def expected_photo(path, layout, inset: Snappr::Overlay::SEAM_MARGIN)
      Vips::Image.new_from_file(path).autorot
                 .extract_area(layout[:crop_left] + inset, layout[:crop_top] + inset,
                               layout[:photo_width] - 2 * inset, layout[:photo_height] - 2 * inset)
    end

    # The photo region as it actually came out, trimmed the same way.
    def placed_photo(image, layout, inset: Snappr::Overlay::SEAM_MARGIN)
      image.extract_area(layout[:photo_left] + inset, layout[:photo_top] + inset,
                         layout[:photo_width] - 2 * inset, layout[:photo_height] - 2 * inset)
    end

    # The largest difference between the placed photo and its source, counting
    # only where the overlay is fully transparent.
    #
    # This is the invariant that matters, and it holds whatever the design does:
    # the overlay is meant to draw *over* the photo, so pixels under artwork are
    # supposed to change. Everywhere the overlay is clear, the photo must arrive
    # untouched. An inset border check cannot express this - a design that draws
    # into the middle of the window would defeat it.
    def photo_difference_under_clear_overlay(source, image, layout, svg)
      overlay = Snappr::OverlaySvg.render(svg, layout[:canvas_width], layout[:canvas_height])
      region = ->(img, left, top) do
        img.extract_area(left, top, layout[:photo_width], layout[:photo_height])
      end

      got = region.(image, layout[:photo_left], layout[:photo_top])
      want = region.(Vips::Image.new_from_file(source.to_s).autorot, layout[:crop_left], layout[:crop_top])
      clear = region.(overlay, layout[:photo_left], layout[:photo_top])
                .extract_band(3).relational_const(:equal, 0)

      difference = (got.cast(:float) - want.cast(:float)).abs.bandor
      clear.ifthenelse(difference, 0).max
    end

    # Count of pixels matching an [r,g,b] within a tolerance.
    def count_colour(image, rgb, tolerance: 40)
      mask = (0..2).map do |band|
        channel = image.extract_band(band)
        channel.relational_const(:moreeq, rgb[band] - tolerance)
               .boolean(channel.relational_const(:lesseq, rgb[band] + tolerance), :and)
      end.reduce { |a, b| a.boolean(b, :and) }
      (mask.avg / 255.0 * image.width * image.height).round
    end

    def max_difference(a, b)
      (a.cast(:float) - b.cast(:float)).abs.max
    end

    def in_tmpdir(&block) = Dir.mktmpdir("snappr-test", &block)

    FIXTURES = File.expand_path("fixtures", __dir__)

    def fixture(name) = File.join(FIXTURES, name)

    # A config pointing at fixture overlays, so tests do not depend on whatever
    # branding happens to be committed.
    def test_config(landscape: "window-landscape.svg", portrait: "window-portrait.svg", **overrides)
      Snappr::Config.new(Snappr::Config::DEFAULTS.merge(
        "overlay_svg" => { "landscape" => fixture(landscape), "portrait" => fixture(portrait) }
      ).merge(overrides))
    end

    def load_image(path) = Vips::Image.new_from_file(path.to_s)

    # x of the first fully white pixel along the horizontal centre line, or nil.
    def white_starts_x(image)
      row = image.extract_area(0, image.height / 2, image.width, 1).extract_band(0)
      (0...image.width).find { |x| row.getpoint(x, 0).first > 250 }
    end

    # y of the first fully white pixel down the vertical centre line, or nil.
    def white_starts_y(image)
      col = image.extract_area(image.width / 2, 0, 1, image.height).extract_band(0)
      (0...image.height).find { |y| col.getpoint(0, y).first > 250 }
    end

    def three_fifths(size) = (size * 3.0 / 5).round
  end
end
