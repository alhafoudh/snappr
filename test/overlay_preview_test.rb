# frozen_string_literal: true

require "test_helper"

# The overlay panel's card. What matters here is not that a PNG appears - it is
# that the two things the card exists to show are right: the artwork as librsvg
# renders it, and where that render put the photo window.
class OverlayPreviewTest < Minitest::Test
  include Snappr::TestHelpers

  FIXTURES = File.expand_path("fixtures", __dir__)
  SAMPLE = File.expand_path("../sample/overlay-landscape.svg", __dir__)

  def preview(svg, key: "k", **args)
    Dir.mktmpdir do |dir|
      yield Snappr::OverlayPreview.call(svg: svg, dir: dir, key: key, **args), dir
    end
  end

  def test_it_writes_a_png_with_the_photo_window_transparent
    preview(File.join(FIXTURES, "window-landscape.svg")) do |result|
      assert_path_exists result[:thumb]
      assert_equal ".png", File.extname(result[:thumb])

      image = Vips::Image.new_from_file(result[:thumb].to_s)
      assert image.has_alpha?, "the card paints its own ground behind this, so it needs alpha"
      # The marker rect is 1616x1840 of a 2960x2000 frame - a large hole, so the
      # mean alpha has to be well short of opaque.
      assert_operator image.extract_band(3).avg, :<, 200, "the photo window must be transparent"
    end
  end

  # PNG, not JPEG. JPEG has no alpha and would flatten the window to black, which
  # is the one part of the image the operator opens the panel to look at.
  def test_the_render_is_bounded_and_keeps_the_authored_shape
    preview(File.join(FIXTURES, "window-landscape.svg")) do |result|
      assert_equal Snappr::OverlayPreview::BOX_PX, result[:width], "a landscape overlay fills the box's width"
      assert_operator result[:height], :<=, Snappr::OverlayPreview::BOX_PX
      assert_in_delta 2960.0 / 2000, result[:aspect], 0.001
      assert_in_delta result[:aspect], result[:width].fdiv(result[:height]), 0.01
    end

    preview(File.join(FIXTURES, "window-portrait.svg")) do |result|
      assert_equal Snappr::OverlayPreview::BOX_PX, result[:height], "a portrait overlay fills the box's height"
      assert_operator result[:width], :<=, Snappr::OverlayPreview::BOX_PX
    end
  end

  # The fractions the panel positions its grey photo-icon rectangle with. If these
  # drift, the card lies about where the photo goes - which is the whole point of
  # drawing it.
  def test_the_window_comes_back_as_fractions_of_the_frame
    preview(File.join(FIXTURES, "window-landscape.svg")) do |result|
      window = result[:window]
      assert_in_delta 80.0 / 2960, window[:x], 0.005
      assert_in_delta 80.0 / 2000, window[:y], 0.005
      assert_in_delta 1616.0 / 2960, window[:width], 0.005
      assert_in_delta 1840.0 / 2000, window[:height], 0.005
      assert_empty result[:warnings]
    end
  end

  # The sample's marker covers the whole frame, so its card is entirely photo
  # window with the wordmark on top. Pinned here as well as in overlay_svg_test
  # because a redraw that inset it would quietly turn every default card - and
  # every default print - into a bordered one.
  def test_the_shipped_sample_reads_as_a_full_frame_watermark
    preview(SAMPLE) do |result|
      assert_in_delta 0.0, result[:window][:x], 0.01
      assert_in_delta 0.0, result[:window][:y], 0.01
      assert_in_delta 1.0, result[:window][:width], 0.01
      assert_in_delta 1.0, result[:window][:height], 0.01
    end
  end

  # The failure this whole file exists to make visible, and it must not be an
  # error. An empty card tells the operator nothing; a card that draws the design
  # and says the photo window is missing tells them exactly what to fix.
  def test_a_missing_marker_still_renders_and_reports_rather_than_raising
    preview(File.join(FIXTURES, "absolute.svg")) do |result|
      assert_path_exists result[:thumb], "the artwork is still worth showing"
      assert_nil result[:window]
      assert_equal ["overlay_window_missing"], result[:warnings].map { |w| w[:kind] }
      refute_match(/#FF00FF|viewBox|magenta rectangle marker/i, result[:warnings].first[:message].split(".").first,
                   "the operator's half may not carry a hex literal")
    end
  end

  # A marker that survives the element walk is keyed out on alpha instead, and
  # OverlaySvg warns when it does. That warning has to reach the card, or the
  # operator sees a clean-looking preview of a file that needs fixing.
  def test_a_keyed_out_marker_warns_on_the_card
    preview(File.join(FIXTURES, "window-styled.svg")) do |result|
      assert result[:window], "a styled marker is still found"
      # window-styled marks the window with style="fill:#FF00FF", which the
      # element walk removes - so this asserts the clean path, and the warnings
      # array is the channel a keyed-out one would arrive on.
      assert_empty result[:warnings]
    end
  end

  def test_it_renames_into_place_and_leaves_no_temp_file
    preview(File.join(FIXTURES, "window-landscape.svg"), key: "abc123") do |result, dir|
      assert_equal File.join(dir, "abc123.png"), result[:thumb].to_s
      assert_empty Dir.glob(File.join(dir, "*.tmp-*")), "the temp file must be renamed or removed"
    end
  end

  # The destination has no default, for the same reason Preview's has none: one
  # that could fall back to the drop folder is how the pipeline ends up eating
  # its own output.
  def test_the_destination_is_required
    assert_raises(ArgumentError) { Snappr::OverlayPreview.call(svg: SAMPLE, key: "k") }
  end
end
