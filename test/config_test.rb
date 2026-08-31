# frozen_string_literal: true

require "test_helper"

class ConfigTest < Minitest::Test
  include Snappr::TestHelpers

  def test_overlay_svg_returns_the_requested_orientation_when_it_exists
    config = test_config
    assert_equal fixture("window-landscape.svg"), config.overlay_svg(:landscape).to_s
    assert_equal fixture("window-portrait.svg"), config.overlay_svg(:portrait).to_s
  end

  # The branding may ship only one orientation - a stretched overlay still
  # prints, a missing one prints nothing.
  def test_overlay_svg_falls_back_to_the_other_orientation
    config = test_config(portrait: "does-not-exist.svg")
    assert_equal fixture("window-landscape.svg"), config.overlay_svg(:portrait).to_s

    config = test_config(landscape: "does-not-exist.svg")
    assert_equal fixture("window-portrait.svg"), config.overlay_svg(:landscape).to_s
  end

  def test_overlay_svg_errors_when_both_orientations_are_missing
    config = test_config(landscape: "does-not-exist.svg", portrait: "also-missing.svg")
    error = assert_raises(Snappr::Error) { config.overlay_svg(:landscape) }
    assert_match(/no overlay SVG found/, error.message)
    assert_match(/does-not-exist\.svg/, error.message)
    assert_match(/also-missing\.svg/, error.message)
  end

  def test_overlay_svg_rejects_unknown_orientations
    assert_raises(Snappr::Error) { test_config.overlay_svg(:square) }
  end
end
