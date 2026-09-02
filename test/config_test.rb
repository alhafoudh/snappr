# frozen_string_literal: true

require "test_helper"

class ConfigTest < Minitest::Test
  include Snappr::TestHelpers

  def test_overlay_svg_returns_the_requested_orientation_when_it_exists
    config = test_config
    assert_equal fixture("window-landscape.svg"), config.overlay_svg(:landscape).to_s
    assert_equal fixture("window-portrait.svg"), config.overlay_svg(:portrait).to_s
  end

  # An empty slot is a legitimate setting, not a misconfiguration: the operator
  # clears an orientation to print the crop with no branding over it.
  def test_overlay_svg_is_nil_when_the_slot_is_empty
    config = Snappr::Config.new(Snappr::Config::DEFAULTS)
    assert_nil config.overlay_svg(:landscape)
    assert_nil config.overlay_svg(:portrait)
  end

  def test_no_overlay_is_the_default
    assert_nil Snappr::Config::DEFAULTS.dig("overlay_svg", "landscape")
    assert_nil Snappr::Config::DEFAULTS.dig("overlay_svg", "portrait")
  end

  # Someone moves a branding folder mid-event. Printing plain photos with a
  # warning is recoverable; erroring on every photo is not.
  def test_overlay_svg_warns_and_degrades_to_none_when_the_file_is_missing
    config = test_config(landscape: "does-not-exist.svg")
    warnings = []
    assert_nil config.overlay_svg(:landscape, warnings)
    assert_equal 1, warnings.size
    assert_equal "overlay_missing", warnings.first[:kind]
    assert_match(/does-not-exist\.svg/, warnings.first[:message])
  end

  # There used to be one, from when the overlays were committed assets that might
  # ship only one orientation. Now that each slot is picked separately, clearing
  # portrait has to mean no portrait overlay rather than a stretched landscape one.
  def test_overlay_svg_never_falls_back_to_the_other_orientation
    assert_nil test_config(portrait: "does-not-exist.svg").overlay_svg(:portrait)
    assert_nil test_config(landscape: "does-not-exist.svg").overlay_svg(:landscape)
  end

  # Absolute paths are what the app sends: the operator picks an SVG anywhere on
  # disk and it is taken as given rather than resolved against the project.
  def test_overlay_svg_takes_absolute_paths_as_given
    config = test_config
    assert_equal fixture("window-landscape.svg"), config.overlay_svg(:landscape).to_s
  end

  def test_overlay_svg_rejects_unknown_orientations
    assert_raises(Snappr::Error) { test_config.overlay_svg(:square) }
  end
end
