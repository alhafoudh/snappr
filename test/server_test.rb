# frozen_string_literal: true

require "test_helper"
require "json"
require "stringio"
require "tmpdir"

# The NDJSON protocol `snappr serve` speaks to the Tauri shell.
#
# The assertion that matters most here is test_stdout_carries_only_json: the
# pipeline is chatty and Snappr.logger is used freely throughout it, so the
# separation between the protocol channel and the log channel is one careless
# `puts` away from breaking, and it breaks as a parse error in another language.
class ServerTest < Minitest::Test
  include Snappr::TestHelpers

  def setup
    @dir = Dir.mktmpdir("snappr-server-test")
    @previews = File.join(@dir, "previews")
  end

  def teardown
    FileUtils.remove_entry(@dir)
  end

  # Runs the given request frames through a server and returns the parsed
  # responses, plus whatever landed on stdout, so tests can check both.
  def converse(*frames)
    input = StringIO.new(frames.map { |frame| "#{JSON.generate(frame)}\n" }.join)
    output = StringIO.new
    Snappr::Server.new(input: input, output: output).run
    raw = output.string
    [raw.lines.map { |line| JSON.parse(line) }, raw]
  end

  def hello_frame(overrides = {})
    { id: 1, cmd: "hello", preview_dir: @previews,
      config: { "overlay_svg" => { "landscape" => fixture("window-landscape.svg").to_s,
                                   "portrait" => fixture("window-portrait.svg").to_s } }.merge(overrides) }
  end

  def test_handshake_reports_the_runtime
    responses, = converse(hello_frame)

    assert_equal 1, responses.size
    assert_equal true, responses[0]["ok"]
    assert_equal RUBY_VERSION, responses[0]["ruby"]
    assert_equal Vips::LIBRARY_VERSION, responses[0]["vips"]
  end

  def test_overlay_returns_the_result_and_the_previews
    source = write_fixture(@dir, "IMG_1.jpg", 2400, 1600)
    responses, = converse(hello_frame, { id: 2, cmd: "overlay", source: source.to_s })

    started, done = responses[1], responses[2]
    assert_equal "started", started["event"]
    assert_equal true, done["ok"]
    assert_equal "landscape", done["orientation"]
    assert_equal 8, done["depth"]
    assert_equal [], done["warnings"]
    assert_path_exists done["destination"]

    %w[thumb overlay source].each do |kind|
      assert_path_exists done["previews"][kind], "expected a #{kind} preview"
    end
  end

  # The filmstrip thumbnail is made from the original frame, not from the composite.
  # At 132x84 the branding panel is a fifth of the cell and identical on every one
  # of them, so a strip of composites is a strip of the same logo and the operator
  # cannot tell the cells apart; the whole uncropped frame can be.
  #
  # The source is 2:1 because that is nothing like the print's 1.48:1 - the aspect
  # ratio is what tells a thumbnail of the original from one of the composite, and
  # the 3:2 used elsewhere in this file is too close to 1.48 to distinguish.
  def test_the_thumbnail_comes_from_the_original_not_the_composite
    source = write_fixture(@dir, "IMG_3.jpg", 2400, 1200)
    responses, = converse(hello_frame, { id: 2, cmd: "overlay", source: source.to_s })
    previews = responses[2]["previews"]

    thumb = load_image(previews["thumb"])
    overlay = load_image(previews["overlay"])

    assert_in_delta 2.0, thumb.width.fdiv(thumb.height), 0.02, "the thumb is not the source frame"
    assert_in_delta 1.48, overlay.width.fdiv(overlay.height), 0.02, "the overlay preview is not the print"
  end

  # The camera's own facts reach the shell as a nested object with string keys, which
  # is what the Rust side deserialises into its Exif struct. Symbol keys would arrive
  # as something serde could still read, but scalarize is what guarantees it.
  def test_result_carries_the_source_exif
    source = write_fixture(@dir, "IMG_3.jpg", 2400, 1600, exif: APPLE_EXIF, Q: 100)
    responses, = converse(hello_frame, { id: 2, cmd: "overlay", source: source.to_s })
    exif = responses[2]["exif"]

    assert_equal "iPhone 17 Pro", exif["camera"]
    assert_equal "1/120s", exif["shutter"]
    assert_equal 400, exif["iso"]
  end

  # Pathname and Symbol are not JSON scalars. If they leak through as anything but
  # strings the other side gets a shape it cannot use.
  def test_result_values_are_json_scalars
    source = write_fixture(@dir, "IMG_2.jpg", 2400, 1600)
    responses, = converse(hello_frame, { id: 2, cmd: "overlay", source: source.to_s })
    done = responses[2]

    assert_kind_of String, done["destination"]
    assert_kind_of String, done["orientation"]
    assert_kind_of String, done["svg"]
    assert_kind_of Integer, done["layout"]["canvas_width"]
  end

  def test_a_failing_photo_returns_an_error_rather_than_crashing
    responses, = converse(hello_frame,
                          { id: 2, cmd: "overlay", source: File.join(@dir, "nope.jpg") },
                          { id: 3, cmd: "ping" })

    failure = responses.find { |r| r["id"] == 2 && r.key?("ok") }
    assert_equal false, failure["ok"]
    assert_equal "Snappr::Problem", failure["error"]["class"]
    assert_equal "photo_missing", failure["error"]["code"]
    assert_match(/nope\.jpg/, failure["error"]["message"])

    # The point: the server is still serving afterwards.
    assert_equal true, responses.find { |r| r["id"] == 3 }["ok"]
  end

  def test_overlay_before_hello_is_refused
    responses, = converse({ id: 1, cmd: "overlay", source: "/tmp/whatever.jpg" })

    assert_equal false, responses[0]["ok"]
    assert_equal "not_ready", responses[0]["error"]["code"]
  end

  def test_malformed_json_does_not_stop_the_loop
    input = StringIO.new("{not json\n#{JSON.generate(hello_frame)}\n")
    output = StringIO.new
    Snappr::Server.new(input: input, output: output).run
    responses = output.string.lines.map { |line| JSON.parse(line) }

    assert_equal false, responses[0]["ok"]
    assert_equal "JSON::ParserError", responses[0]["error"]["class"]
    assert_equal true, responses[1]["ok"]
  end

  def test_unknown_command_is_reported
    responses, = converse(hello_frame, { id: 2, cmd: "sing" })

    assert_equal false, responses[1]["ok"]
    assert_equal "unknown_request", responses[1]["error"]["code"]
    # "sing" is the developer's half, so it must not be in what the shell shows.
    refute_match(/sing/, responses[1]["error"]["message"])
  end

  # A bad value must fail the handshake, not the tenth photo. Config coerces
  # lazily, so without the eager validation this surfaces deep inside the pipeline.
  def test_a_bad_config_value_fails_the_handshake
    responses, = converse(hello_frame("background" => "not-a-colour"))

    assert_equal false, responses[0]["ok"]
    assert_equal "background_not_a_colour", responses[0]["error"]["code"]
  end

  def test_stdout_carries_only_json
    source = write_fixture(@dir, "IMG_3.jpg", 2400, 1600)
    _, raw = converse(hello_frame, { id: 2, cmd: "overlay", source: source.to_s })

    refute_empty raw
    raw.each_line do |line|
      JSON.parse(line)
    rescue JSON::ParserError
      flunk "non-JSON line on stdout: #{line.inspect}"
    end
  end

  # The mismatch warning is the one an operator can act on, so it has to arrive as
  # data rather than only in the log. window-portrait.svg is a portrait shape, so
  # forcing it onto a landscape photo triggers it.
  def test_warnings_are_reported_as_data
    source = write_fixture(@dir, "IMG_4.jpg", 2400, 1600)
    frame = hello_frame("overlay_svg" => { "landscape" => fixture("window-portrait.svg").to_s,
                                           "portrait" => fixture("window-portrait.svg").to_s })
    responses, = converse(frame, { id: 2, cmd: "overlay", source: source.to_s })
    done = responses[2]

    assert_equal true, done["ok"]
    assert_equal ["aspect_mismatch"], done["warnings"].map { |w| w["kind"] }
  end

  # No branding is the default of a fresh install, so the handshake has to accept
  # empty slots rather than treating them as a misconfiguration.
  def test_a_handshake_with_no_overlays_succeeds_and_prints_the_crop
    source = write_fixture(@dir, "IMG_5.jpg", 2400, 1600)
    frame = hello_frame("overlay_svg" => { "landscape" => nil, "portrait" => nil })
    responses, = converse(frame, { id: 2, cmd: "overlay", source: source.to_s })
    done = responses[2]

    assert_equal true, responses[0]["ok"]
    assert_equal true, done["ok"]
    assert_nil done["svg"]
    # 2400x1600 is wider than 1.48:1, so the crop keeps the height and shortens
    # the width - the largest inscribed rectangle, with nothing resampled.
    assert_equal [2368, 1600], [done["output_width"], done["output_height"]]
  end

  # How the shell changes the overlays without restarting the sidecar. The same
  # handler as hello, because it rebuilds Config, Overlay and Preview from
  # scratch; this asserts that the next photo actually uses the new settings.
  def test_configure_swaps_the_overlay_mid_session
    source = write_fixture(@dir, "IMG_6.jpg", 2400, 1600)
    responses, = converse(
      hello_frame("overlay_svg" => { "landscape" => nil, "portrait" => nil }),
      { id: 2, cmd: "overlay", source: source.to_s },
      { id: 3, cmd: "configure", preview_dir: @previews,
        config: { "overlay_svg" => { "landscape" => fixture("window-landscape.svg").to_s,
                                     "portrait" => fixture("window-portrait.svg").to_s } } },
      { id: 4, cmd: "overlay", source: source.to_s }
    )

    before, configured, after = responses[2], responses[3], responses[5]
    assert_nil before["svg"]
    assert_equal true, configured["ok"]
    assert_equal fixture("window-landscape.svg"), after["svg"]
    refute_equal before["output_width"], after["output_width"],
                 "the new overlay's window should change the canvas the photo lands on"
  end

  # Two composites of one photo must not share preview filenames.
  #
  # The UI draws the previews through `asset://` URLs built out of these paths, and
  # a webview that has already fetched one serves it from its own cache for the life
  # of the session. Keyed on the source alone - which is what Preview#key_for used
  # to do - clearing a photo, picking a different overlay and dropping the same file
  # in again wrote byte-different JPEGs to exactly the URLs the webview was holding,
  # so the operator kept seeing the overlay they had just replaced while the PNG on
  # disk was the new one. Nothing about the source changes between these two runs,
  # which is the point: the key has to move anyway.
  def test_a_second_composite_writes_previews_under_a_new_name
    source = write_fixture(@dir, "IMG_8.jpg", 2400, 1600)
    responses, = converse(
      hello_frame("overlay_svg" => { "landscape" => nil, "portrait" => nil }),
      { id: 2, cmd: "overlay", source: source.to_s },
      { id: 3, cmd: "configure", preview_dir: @previews,
        config: { "overlay_svg" => { "landscape" => fixture("window-landscape.svg").to_s,
                                     "portrait" => nil } } },
      { id: 4, cmd: "overlay", source: source.to_s }
    )

    before, after = responses[2]["previews"], responses[5]["previews"]

    %w[thumb overlay source].each do |kind|
      refute_equal before[kind], after[kind], "the #{kind} preview kept its name across a re-composite"
      assert_path_exists after[kind]
    end
  end

  # A slot whose file has been moved must not fail the handshake: an app that
  # will not launch because an SVG was tidied away is worse than one that prints
  # unbranded photos and says so.
  def test_a_missing_overlay_file_does_not_fail_the_handshake
    source = write_fixture(@dir, "IMG_7.jpg", 2400, 1600)
    frame = hello_frame("overlay_svg" => { "landscape" => fixture("gone.svg").to_s, "portrait" => nil })
    responses, = converse(frame, { id: 2, cmd: "overlay", source: source.to_s })
    done = responses[2]

    assert_equal true, responses[0]["ok"]
    assert_equal true, done["ok"]
    assert_nil done["svg"]
    assert_equal ["overlay_missing"], done["warnings"].map { |w| w["kind"] }
  end
end
