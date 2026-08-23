# frozen_string_literal: true

require "test_helper"

class WatcherTest < Minitest::Test
  include Snappr::TestHelpers

  def setup
    @watcher = Snappr::Watcher.new
  end

  def test_accepts_recognised_photos_in_any_case
    %w[IMG_1234.heic photo.HEIC scan.heif a.jpg b.JPG c.jpeg d.JPEG].each do |name|
      assert @watcher.candidate?("/tmp/#{name}"), "#{name} should be a candidate"
    end
  end

  # The self-output guard. Without it the watcher composites its own output and
  # could composite its own output. png is not a watched extension either.
  def test_rejects_its_own_output
    refute @watcher.candidate?("/tmp/IMG_1234.overlay.png")
    refute @watcher.candidate?("/tmp/IMG_1234.OVERLAY.PNG")
    # and png is not a watched extension either, so it could never retrigger
    refute @watcher.candidate?("/tmp/anything.png")
  end

  # The in-progress file is a sibling with the same suffix, so the same guard
  # has to cover it.
  def test_rejects_its_own_in_progress_temp_file
    temp = Snappr::Overlay.new.temp_path_for("/tmp/IMG_1234.heic")
    refute @watcher.candidate?(temp), temp.to_s
  end

  def test_rejects_partial_downloads
    refute @watcher.candidate?("/tmp/IMG_1234.heic.download")
    refute @watcher.candidate?("/tmp/IMG_1234.jpg.part")
    refute @watcher.candidate?("/tmp/IMG_1234.jpg.crdownload")
  end

  # ~/Downloads is the user's real downloads folder; anything unrecognised is
  # ignored and never touched.
  def test_ignores_everything_else
    %w[notes.txt report.pdf archive.zip movie.mov overlay.svg IMG_1234].each do |name|
      refute @watcher.candidate?("/tmp/#{name}"), "#{name} should be ignored"
    end
  end
end
