# frozen_string_literal: true

require "test_helper"

class OverlaySvgTest < Minitest::Test
  include Snappr::TestHelpers

  FIXTURES = File.expand_path("fixtures", __dir__)

  # Each fixture draws the same thing - a full-width white band across the bottom
  # 40% - in a different SVG dialect. All three must render identically, because
  # overlays are hand-drawn in whatever tool the designer uses.
  DIALECTS = {
    "absolute.svg" => "absolute user units, as exported by Figma",
    "no-viewbox.svg" => "no viewBox at all",
    "percent.svg" => "percentage geometry"
  }.freeze

  # THE regression test. Overwriting the authored viewBox strands artwork at its
  # authored coordinates: a band meant for the bottom 40% of a 2724x4032 frame
  # ended up floating at x 0-73%, y 44-73%. Any dialect must scale to fill.
  def test_artwork_scales_to_fill_the_frame_in_every_dialect
    DIALECTS.each do |name, description|
      image = Snappr::OverlaySvg.render(File.join(FIXTURES, name), 2724, 4032)
      box = opaque_bounds(image)

      assert_equal 0, box[:left], "#{description}: band must start at the left edge"
      assert_equal 2724, box[:width], "#{description}: band must span the full width"
      assert_in_delta 4032 * 0.6, box[:top], 3, "#{description}: band must start at 60% height"
      assert_in_delta 4032 * 0.4, box[:height], 3, "#{description}: band must be 40% tall"
    end
  end

  def test_scales_to_any_size
    [[600, 888], [2724, 4032], [4000, 5920]].each do |width, height|
      image = Snappr::OverlaySvg.render(File.join(FIXTURES, "absolute.svg"), width, height)
      assert_equal [width, height], [image.width, image.height]
      assert image.has_alpha?
      assert_in_delta height * 0.6, opaque_bounds(image)[:top], 3, "band placement must be proportional"
    end
  end

  def test_the_authored_view_box_is_preserved
    # REXML serialises attributes with single quotes, so match on value not syntax.
    source = Snappr::OverlaySvg.source_for(File.join(FIXTURES, "absolute.svg"), 2724, 4032)
    assert_match(/viewBox=.0 0 1000 1480./, source, "the authored viewBox must survive")
    assert_match(/width=.2724./, source)
    assert_match(/height=.4032./, source)
    assert_match(/preserveAspectRatio=.none./, source)
  end

  # Without a viewBox the width/height are the coordinate system, so one has to
  # be synthesised or the artwork stays at its authored size.
  def test_a_missing_view_box_is_synthesised_from_width_and_height
    source = Snappr::OverlaySvg.source_for(File.join(FIXTURES, "no-viewbox.svg"), 2724, 4032)
    assert_match(/viewBox=.0 0 1000\.0 1480\.0./, source)
  end

  def test_aspect_ratio_comes_from_the_view_box
    assert_in_delta 1000.0 / 1480, Snappr::OverlaySvg.aspect_ratio(File.join(FIXTURES, "absolute.svg")), 0.0001
    assert_in_delta 1000.0 / 1480, Snappr::OverlaySvg.aspect_ratio(File.join(FIXTURES, "no-viewbox.svg")), 0.0001
  end

  def test_missing_svg_is_reported_clearly
    error = assert_raises(Snappr::Problem) { Snappr::OverlaySvg.render("/nope/absent.svg", 100, 100) }
    assert_equal :overlay_file_missing, error.code
    assert_match(/absent\.svg/, error.message)
    assert_match(%r{/nope/absent\.svg}, error.detail)
  end

  def test_an_unsizeable_svg_is_reported_clearly
    in_tmpdir do |dir|
      path = File.join(dir, "vague.svg")
      File.write(path, '<svg width="100%" height="100%" xmlns="http://www.w3.org/2000/svg"></svg>')
      error = assert_raises(Snappr::Problem) { Snappr::OverlaySvg.source_for(path, 100, 100) }
      assert_equal :overlay_no_size, error.code
      # "viewBox" is the word for whoever wrote the exporter, not for the operator.
      refute_match(/viewBox/, error.message)
      assert_match(/viewBox/, error.detail)
    end
  end

  # --- the real, committed branding: only properties that must hold whatever it depicts ---

  def test_committed_overlays_are_drawn_at_the_print_ratio
    refute_empty committed_overlays, "at least one overlay SVG must be committed"
    committed_overlays.each do |orientation, svg|
      assert_in_delta committed_config.print_ratio(orientation), Snappr::OverlaySvg.aspect_ratio(svg), 0.01,
                      "#{File.basename(svg)} must be drawn at the paper's ratio or it gets stretched"
    end
  end

  # Catches the stranded-artwork bug against the real branding, without assuming
  # anything about what that branding depicts.
  def test_committed_overlays_have_a_photo_window_and_strip_it
    committed_overlays.each do |orientation, svg|
      window = Snappr::OverlaySvg.photo_window(svg)

      assert_operator window[:width], :>, 0.1, "#{orientation}: window looks too small to be real"
      assert_operator window[:height], :>, 0.1, "#{orientation}: window looks too small to be real"
      assert_operator window[:x] + window[:width], :<=, 1.001, "#{orientation}: window runs off the frame"
      assert_operator window[:y] + window[:height], :<=, 1.001, "#{orientation}: window runs off the frame"

      image = Snappr::OverlaySvg.render(svg, 1480, 1000)
      assert_equal 0, marker_pixels(image), "#{orientation}: the marker must never be rendered"
      assert_operator image.extract_band(3).min, :<, 128,
                      "#{orientation}: the window must be left transparent for the photo"
    end
  end

  # If the viewBox were being overwritten, the artwork would shrink away from the
  # edges instead of scaling with the frame.
  def test_committed_overlay_artwork_scales_with_the_frame
    committed_overlays.each do |orientation, svg|
      small = opaque_bounds(Snappr::OverlaySvg.render(svg, 740, 500))
      large = opaque_bounds(Snappr::OverlaySvg.render(svg, 2960, 2000))

      %i[left top width height].each do |edge|
        assert_in_delta small[edge] * 4, large[edge], 8,
                        "#{orientation}: artwork #{edge} must scale with the frame"
      end
    end
  end

  private

  # The sample overlays config/snappr.yml names, which is the only place any are
  # named now - the app ships none and picks them at runtime. An orientation may
  # legitimately have no file, so only the ones that exist are held to these
  # properties.
  def committed_overlays
    config = committed_config
    Snappr::Config::ORIENTATIONS.filter_map do |orientation|
      svg = config.configured_overlay_svg(orientation)
      [orientation, svg] if svg&.file?
    end
  end

  def marker_pixels(image)
    red, green, blue = image.extract_band(0), image.extract_band(1), image.extract_band(2)
    mask = red.relational_const(:moreeq, 195)
              .boolean(green.relational_const(:lesseq, 60), :and)
              .boolean(blue.relational_const(:moreeq, 195), :and)
    mask = mask.boolean(image.extract_band(3).relational_const(:moreeq, 128), :and) if image.has_alpha?
    (mask.avg / 255.0 * image.width * image.height).round
  end

  # Bounding box of the opaque (non-transparent) artwork.
  def opaque_bounds(image)
    mask = image.extract_band(3).relational_const(:more, 128).ifthenelse(0, 255)
    left, top, width, height = mask.find_trim(background: 255, threshold: 10)
    { left: left, top: top, width: width, height: height }
  end
end
