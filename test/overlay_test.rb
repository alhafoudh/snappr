# frozen_string_literal: true

require "test_helper"

class OverlayTest < Minitest::Test
  include Snappr::TestHelpers

  def setup
    @config = test_config
    @overlay = Snappr::Overlay.new(@config)
  end

  # --- naming ---

  def test_destination_is_the_basename_plus_the_suffix
    assert_equal "/tmp/IMG_1234.overlay.png", @overlay.destination_for("/tmp/IMG_1234.heic").to_s
    assert_equal "/tmp/photo.overlay.png", @overlay.destination_for("/tmp/photo.JPEG").to_s
    assert_equal "/tmp/a.b.c.overlay.png", @overlay.destination_for("/tmp/a.b.c.jpg").to_s
  end

  # The temp file needs a real image extension, because libvips picks its saver
  # from it, and it needs the output suffix, so the watcher's guard skips it.
  def test_temp_path_is_a_png_and_carries_the_output_suffix
    temp = @overlay.temp_path_for("/tmp/IMG_1234.heic").to_s
    assert temp.end_with?(".overlay.png"), temp
    assert_includes temp, "tmp-"
    refute_equal @overlay.destination_for("/tmp/IMG_1234.heic").to_s, temp
  end

  def test_orientation_of
    assert_equal :landscape, @overlay.orientation_of(4032, 3024)
    assert_equal :portrait, @overlay.orientation_of(3024, 4032)
    assert_equal :landscape, @overlay.orientation_of(2000, 2000), "square counts as landscape"
  end

  # --- the layout maths ---

  def test_crop_area_is_centred
    assert_equal({ left: 0, top: 44, width: 1200, height: 811 },
                 @overlay.crop_area(1200, 900, @config.print_ratio(:landscape)))
    assert_equal({ left: 44, top: 0, width: 811, height: 1200 },
                 @overlay.crop_area(900, 1200, @config.print_ratio(:portrait)))
  end

  def test_crop_area_never_upscales_and_hits_the_ratio
    [[4032, 3024], [3024, 4032], [6000, 4000], [1600, 1200], [2000, 2000]].each do |w, h|
      [0.888, 1.48, 0.6757, 2.4].each do |ratio|
        crop = @overlay.crop_area(w, h, ratio)
        assert_operator crop[:width], :<=, w, "#{w}x#{h} @ #{ratio}: never upscale width"
        assert_operator crop[:height], :<=, h, "#{w}x#{h} @ #{ratio}: never upscale height"
        assert_in_delta ratio, crop[:width].fdiv(crop[:height]), 0.005, "#{w}x#{h} @ #{ratio}"
        assert crop[:width] == w || crop[:height] == h, "#{w}x#{h} @ #{ratio}: crop must be maximal"
      end
    end
  end

  # The canvas is derived from the photo so the window lands at the photo's own
  # resolution. That is what makes the fit a pure crop.
  def test_canvas_keeps_the_print_ratio_and_needs_no_upscaling
    [[4032, 3024, :landscape], [3024, 4032, :portrait]].each do |w, h, orientation|
      layout = @overlay.layout_for(stub_image(w, h), @config.overlay_svg(orientation), orientation)

      assert_in_delta @config.print_ratio(orientation),
                      layout[:canvas_width].fdiv(layout[:canvas_height]), 0.005,
                      "#{orientation}: canvas must keep the paper's ratio"
      assert_operator layout[:photo_width], :<=, w, "#{orientation}: photo must never be upscaled"
      assert_operator layout[:photo_height], :<=, h, "#{orientation}: photo must never be upscaled"
    end
  end

  # --- the composition ---

  def test_photo_lands_exactly_in_the_window
    in_tmpdir do |dir|
      source = write_fixture(dir, "land.jpg", 4032, 3024, Q: 100)
      layout = @overlay.layout_for(stub_image(4032, 3024), @config.overlay_svg(:landscape), :landscape)
      image = load_image(@overlay.call(source))

      assert_equal [layout[:canvas_width], layout[:canvas_height]], [image.width, image.height]
      # The fixture photo is flat grey; the window is the only place it may appear.
      found = grey_bounds(image)
      assert_equal layout[:photo_left], found[:left], "photo must start at the window's left edge"
      assert_equal layout[:photo_top], found[:top], "photo must start at the window's top edge"
      assert_equal layout[:photo_width], found[:width], "photo must fill the window's width"
      assert_equal layout[:photo_height], found[:height], "photo must fill the window's height"
    end
  end

  # The whole point: the photo is placed as a pure centre crop, so wherever the
  # overlay is clear its pixels must arrive unchanged. If this fails, something
  # is scaling or re-encoding the photo.
  def test_photo_pixels_are_untouched_where_the_overlay_is_clear
    in_tmpdir do |dir|
      # Noise, so a lossy or resampling step cannot hide.
      noise = Vips::Image.gaussnoise(4032, 3024, mean: 128, sigma: 55).cast(:uchar)
      source = File.join(dir, "noisy.png")
      noise.bandjoin([noise.rot180, noise.fliphor]).copy(interpretation: :srgb).write_to_file(source)

      layout = @overlay.layout_for(stub_image(4032, 3024), @config.overlay_svg(:landscape), :landscape)
      image = load_image(@overlay.call(source))

      assert_equal 0,
                   photo_difference_under_clear_overlay(source, image, layout,
                                                        @config.overlay_svg(:landscape)),
                   "the photo must survive bit for bit wherever the overlay is transparent"
    end
  end

  # Same guarantee against a real, full-sized overlay rather than a fixture -
  # whatever it depicts. share/ is sample material, not shipped branding, so this
  # goes through the CLI's config to find it.
  def test_photo_is_untouched_with_the_committed_overlays
    config = committed_config
    overlay = Snappr::Overlay.new(config)
    in_tmpdir do |dir|
      noise = Vips::Image.gaussnoise(2400, 1800, mean: 128, sigma: 55).cast(:uchar)
      source = File.join(dir, "noisy.png")
      noise.bandjoin([noise.rot180, noise.fliphor]).copy(interpretation: :srgb).write_to_file(source)

      layout = overlay.layout_for(stub_image(2400, 1800), config.overlay_svg(:landscape), :landscape)
      image = load_image(overlay.call(source))

      assert_equal 0,
                   photo_difference_under_clear_overlay(source, image, layout,
                                                        config.overlay_svg(:landscape)),
                   "the committed overlay must not alter the photo it frames"
    end
  end

  # object-fit: cover means the crop is taken from the middle of the photo.
  def test_crop_is_centred_on_the_photo
    in_tmpdir do |dir|
      layout = @overlay.layout_for(stub_image(4032, 3024), @config.overlay_svg(:landscape), :landscape)
      assert_equal (4032 - layout[:photo_width]) / 2, layout[:crop_left]
      assert_equal (3024 - layout[:photo_height]) / 2, layout[:crop_top]
    end
  end

  # The photo is laid down first and the overlay goes over it, so a one-pixel
  # rounding disagreement would show the background through the join.
  def test_no_background_seam_where_the_artwork_meets_the_window
    in_tmpdir do |dir|
      config = test_config(landscape: "window-abut.svg", portrait: "window-abut.svg",
                           "background" => "#00ff00")
      overlay = Snappr::Overlay.new(config)
      # Flat grey photo, so nothing in it can be mistaken for the green background.
      source = write_fixture(dir, "flat.png", 4032, 3024)
      image = load_image(overlay.call(source))

      assert_equal 0, count_colour(image, [0, 255, 0]),
                   "no background may show between the photo and the artwork"
    end
  end

  def test_background_fills_what_neither_photo_nor_artwork_covers
    in_tmpdir do |dir|
      # window-landscape.svg leaves an 80/2960 margin around the window.
      config = test_config("background" => "#00ff00")
      overlay = Snappr::Overlay.new(config)
      source = write_fixture(dir, "flat.png", 4032, 3024)
      image = load_image(overlay.call(source))

      assert_operator count_colour(image, [0, 255, 0]), :>, 0,
                      "the design's margin must be filled with the background colour"
    end
  end

  def test_marker_is_never_rendered
    in_tmpdir do |dir|
      source = write_fixture(dir, "flat.png", 4032, 3024)
      image = load_image(@overlay.call(source))
      assert_equal 0, count_colour(image, Snappr::OverlaySvg::MARKER, tolerance: 60),
                   "the #FF00FF marker must not appear in the output"
    end
  end

  def test_an_overlay_without_a_marker_is_reported_clearly
    config = test_config(landscape: "absolute.svg", portrait: "absolute.svg")
    in_tmpdir do |dir|
      source = write_fixture(dir, "flat.png", 1200, 900)
      error = assert_raises(Snappr::Error) { Snappr::Overlay.new(config).call(source) }
      assert_match(/no #FF00FF photo window found/, error.message)
      assert_match(/magenta rectangle/, error.message)
    end
  end

  # --- what must never degrade ---

  # vips thumbnail silently downcasts 16-bit to 8-bit, which is why the fit is a
  # plain crop. A 10-bit HEIC must not come out 8-bit.
  def test_preserves_sixteen_bit_depth
    in_tmpdir do |dir|
      source = write_fixture(dir, "deep.png", 2000, 1500, deep: true)
      assert_equal :ushort, load_image(source).format, "fixture must really be 16-bit"

      layout = @overlay.layout_for(stub_image(2000, 1500), @config.overlay_svg(:landscape), :landscape)
      image = load_image(@overlay.call(source))
      assert_equal :ushort, image.format, "16-bit input must stay 16-bit"
      assert_equal 0, photo_difference_under_clear_overlay(source, image, layout,
                                                          @config.overlay_svg(:landscape))
    end
  end

  def test_preserves_eight_bit_depth
    in_tmpdir do |dir|
      assert_equal :uchar, load_image(@overlay.call(write_fixture(dir, "flat.jpg", 1200, 900, Q: 100))).format
    end
  end

  def test_preserves_the_colour_profile
    skip "no Display P3 profile on this machine" unless File.exist?(P3_PROFILE)

    in_tmpdir do |dir|
      source = write_fixture(dir, "p3.png", 1200, 900, profile: P3_PROFILE)
      expected = profile_of(load_image(source))
      refute_nil expected, "fixture must carry a profile"
      assert_equal expected, profile_of(load_image(@overlay.call(source))),
                   "the icc profile must survive byte for byte"
    end
  end

  def test_survives_a_broken_colour_profile
    in_tmpdir do |dir|
      profile = File.join(dir, "broken.icc")
      File.binwrite(profile, "not an icc profile at all")
      source = write_fixture(dir, "bad.png", 1200, 900, profile: profile)
      refute_nil load_image(@overlay.call(source)), "a bad profile must not cost us the photo"
    end
  end

  def test_keeps_the_source_resolution
    in_tmpdir do |dir|
      source = write_fixture(dir, "dpi.jpg", 1200, 900, dpi: DPI_300, Q: 100)
      image = load_image(@overlay.call(source))
      assert_in_delta DPI_300, image.xres, 0.01, "dpi must survive"
      assert_in_delta DPI_300, image.yres, 0.01
    end
  end

  def test_alpha_channel_is_dropped
    in_tmpdir do |dir|
      assert_equal 3, load_image(@overlay.call(write_fixture(dir, "a.jpg", 1200, 900, Q: 100))).bands
    end
  end

  # --- sources and housekeeping ---

  # libvips loads HEIC natively, so there must never be a pre-conversion step.
  def test_reads_heic
    in_tmpdir do |dir|
      destination = @overlay.call(write_fixture(dir, "guest.heic", 1200, 900))
      assert_equal "guest.overlay.png", File.basename(destination)
      assert_equal "pngload", load_image(destination).get("vips-loader")
    end
  end

  # Dimensions must be read AFTER autorot, because ImageProcessing autorotates on
  # load too. Read them before and a portrait photo is judged landscape, so it
  # gets the wrong overlay and the wrong crop.
  def test_respects_exif_orientation
    in_tmpdir do |dir|
      # 1200x900 on disk, but orientation 6 means it is displayed as 900x1200.
      source = write_fixture(dir, "rot.jpg", 1200, 900, orientation: 6, Q: 100)
      layout = @overlay.layout_for(stub_image(900, 1200), @config.overlay_svg(:portrait), :portrait)
      image = load_image(@overlay.call(source))

      assert_equal [layout[:canvas_width], layout[:canvas_height]], [image.width, image.height],
                   "must use the rotated dimensions, and so the portrait overlay"
      assert_operator image.height, :>, image.width, "output must be portrait"
    end
  end

  # Display data for the info line under the photo. It rides along on the result
  # because the header it comes from is already loaded - see Snappr::Exif.
  def test_reports_the_camera_and_exposure
    in_tmpdir do |dir|
      source = write_fixture(dir, "land.jpg", 1200, 900, exif: APPLE_EXIF, Q: 100)
      exif = @overlay.process(source)[:exif]

      assert_equal "iPhone 17 Pro", exif[:camera]
      assert_equal "f/1.8", exif[:aperture]
      assert_equal "2026-03-21T12:40:52+01:00", exif[:taken_at]
    end
  end

  # A screenshot or a scan is an ordinary photo here. No key at all rather than an
  # empty object, so the caller gets nothing to render rather than something empty.
  def test_omits_the_exif_key_when_the_photo_carries_none
    in_tmpdir do |dir|
      result = @overlay.process(write_fixture(dir, "land.jpg", 1200, 900, Q: 100))

      refute result.key?(:exif)
    end
  end

  def test_leaves_no_temp_file_behind
    in_tmpdir do |dir|
      @overlay.call(write_fixture(dir, "land.jpg", 1200, 900, Q: 100))
      assert_empty Dir.children(dir).grep(/tmp-/)
    end
  end

  def test_overwrites_a_previous_result
    in_tmpdir do |dir|
      source = write_fixture(dir, "land.png", 1200, 900)
      first = @overlay.call(source)
      File.write(first, "stale")
      second = @overlay.call(source)

      assert_equal first.to_s, second.to_s
      refute_nil load_image(second)
    end
  end

  def test_missing_source_is_reported_clearly
    error = assert_raises(Snappr::Error) { @overlay.call("/nope/absent.jpg") }
    assert_match(/no such file/, error.message)
  end

  # --- no overlay ---
  #
  # A cleared slot is a legitimate setting and the default of a fresh install, so
  # the plain-crop path gets the same scrutiny as the branded one. The window is
  # the whole frame and there is no seam to hide, so the output is exactly the
  # largest inscribed crop at the print ratio - and the "photo is untouched"
  # invariant holds across all of it rather than only where the overlay is clear.

  def test_with_no_overlay_the_output_is_the_crop_at_the_print_ratio
    overlay = Snappr::Overlay.new(bare_config)
    in_tmpdir do |dir|
      landscape = load_image(overlay.call(write_fixture(dir, "wide.png", 4032, 3024)))
      assert_equal [4032, 2724], [landscape.width, landscape.height]

      portrait = load_image(overlay.call(write_fixture(dir, "tall.png", 3024, 4032)))
      assert_equal [2724, 4032], [portrait.width, portrait.height]
    end
  end

  def test_with_no_overlay_the_canvas_is_the_crop_with_no_seam_margin
    config = bare_config
    overlay = Snappr::Overlay.new(config)
    layout = overlay.layout_for(stub_image(4032, 3024), nil, :landscape)

    assert_equal overlay.crop_dimensions(4032, 3024, config.print_ratio(:landscape)),
                 [layout[:canvas_width], layout[:canvas_height]]
    assert_equal [0, 0], [layout[:photo_left], layout[:photo_top]]
    assert_equal [layout[:canvas_width], layout[:canvas_height]],
                 [layout[:photo_width], layout[:photo_height]]
  end

  def test_with_no_overlay_the_photo_is_bit_identical_across_the_whole_frame
    overlay = Snappr::Overlay.new(bare_config)
    in_tmpdir do |dir|
      noise = Vips::Image.gaussnoise(4032, 3024, mean: 128, sigma: 55).cast(:uchar)
      source = File.join(dir, "noisy.png")
      noise.bandjoin([noise.rot180, noise.fliphor]).copy(interpretation: :srgb).write_to_file(source)

      layout = overlay.layout_for(stub_image(4032, 3024), nil, :landscape)
      image = load_image(overlay.call(source))
      want = Vips::Image.new_from_file(source)
                        .extract_area(layout[:crop_left], layout[:crop_top],
                                      layout[:photo_width], layout[:photo_height])

      assert_equal 0, max_difference(image, want),
                   "with no overlay the whole output must be the source, bit for bit"
    end
  end

  def test_with_no_overlay_depth_and_resolution_still_follow_the_source
    overlay = Snappr::Overlay.new(bare_config)
    in_tmpdir do |dir|
      deep = load_image(overlay.call(write_fixture(dir, "deep.png", 1200, 900, deep: true)))
      assert_equal :ushort, deep.format

      dpi = load_image(overlay.call(write_fixture(dir, "dpi.jpg", 1200, 900, dpi: DPI_300, Q: 100)))
      assert_in_delta DPI_300, dpi.xres, 0.01
    end
  end

  # A slot pointing at an SVG that has since been moved must degrade to no
  # overlay with a warning, not take the photo down with it.
  def test_a_missing_overlay_file_warns_and_prints_the_crop
    overlay = Snappr::Overlay.new(test_config(landscape: "gone.svg"))
    in_tmpdir do |dir|
      result = overlay.process(write_fixture(dir, "wide.png", 1200, 900))

      assert_nil result[:svg]
      assert_equal [1200, 811], [result[:output_width], result[:output_height]]
      assert_includes result[:warnings].map { |w| w[:kind] }, "overlay_missing"
    end
  end

  private

  # No branding at all, which is what a fresh install runs with.
  def bare_config = Snappr::Config.new(Snappr::Config::DEFAULTS)

  # layout_for only reads width and height off the image.
  def stub_image(width, height) = Vips::Image.black(width, height)

  # Bounding box of the flat-grey photo pixels.
  def grey_bounds(image)
    band = image.extract_band(0)
    mask = band.relational_const(:moreeq, GREY - 6)
               .boolean(band.relational_const(:lesseq, GREY + 6), :and)
               .ifthenelse(0, 255)
    left, top, width, height = mask.find_trim(background: 255, threshold: 10)
    { left: left, top: top, width: width, height: height }
  end
end
