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
    assert_equal "Snappr::Error", failure["error"]["class"]
    assert_match(/no such file/, failure["error"]["message"])

    # The point: the server is still serving afterwards.
    assert_equal true, responses.find { |r| r["id"] == 3 }["ok"]
  end

  def test_overlay_before_hello_is_refused
    responses, = converse({ id: 1, cmd: "overlay", source: "/tmp/whatever.jpg" })

    assert_equal false, responses[0]["ok"]
    assert_match(/hello/, responses[0]["error"]["message"])
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
    assert_match(/unknown cmd/, responses[1]["error"]["message"])
  end

  # A bad value must fail the handshake, not the tenth photo. Config coerces
  # lazily, so without the eager validation this surfaces deep inside the pipeline.
  def test_a_bad_config_value_fails_the_handshake
    responses, = converse(hello_frame("background" => "not-a-colour"))

    assert_equal false, responses[0]["ok"]
    assert_match(/background/, responses[0]["error"]["message"])
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
end
