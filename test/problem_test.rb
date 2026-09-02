# frozen_string_literal: true

require "test_helper"

# The catalogue is the whole guarantee that a message has both an operator half
# and a console half, so these are properties of the table rather than of any one
# sentence in it. Reword freely; these should keep passing.
class ProblemTest < Minitest::Test
  # Words that belong to whoever wrote the code, not to whoever is holding the
  # print. Each one of these has actually been shown to an operator at some
  # point, which is why this test exists.
  JARGON = [
    "#FF00FF",     # the marker colour. "magenta" is the operator's word for it.
    "viewBox",     # SVG's word for the size, and an exporter's problem
    "sidecar",     # our word for the Ruby subprocess
    "handshake",   # our word for the hello frame
    "NDJSON",      # the protocol
    "stdin",
    "stdout",
    "icc",         # the colour profile
    "::"           # any Ruby class name at all
  ].freeze

  def test_every_entry_has_an_operator_sentence
    Snappr::Problem::CATALOG.each do |code, entry|
      refute_nil entry[:ui], "#{code} has no ui half"
      refute_empty entry[:ui].strip, "#{code}'s ui half is blank"
    end
  end

  # The operator's half is what the shell prints verbatim into the window, so
  # this is the test that stops "Snappr::Error: no #FF00FF photo window found"
  # coming back.
  def test_no_operator_sentence_contains_jargon
    Snappr::Problem::CATALOG.each do |code, entry|
      JARGON.each do |word|
        refute_includes entry[:ui].downcase, word.downcase, "#{code}'s ui half says #{word.inspect}"
      end
    end
  end

  # Sentences, not log lines - so they start with a capital. An entry whose
  # sentence opens on a filename is exempt: "IMG_1234.heic never finished
  # arriving" is correct English and the file is not ours to capitalise.
  def test_every_operator_sentence_reads_as_a_sentence
    Snappr::Problem::CATALOG.each do |code, entry|
      next if entry[:ui].start_with?("%{")

      first = entry[:ui][0]
      assert_equal first.upcase, first, "#{code}'s ui half does not start with a capital"
    end
  end

  def test_an_unknown_code_is_refused_loudly
    error = assert_raises(ArgumentError) { Snappr::Problem.new(:no_such_thing) }
    assert_match(/no_such_thing/, error.message)
  end

  # A missing argument must fail here, at the raise site, rather than leaving a
  # literal "%{file}" in front of an operator. Ruby's format raises ArgumentError
  # when no arguments are given at all and KeyError when some are but not this
  # one, so both count.
  def test_a_missing_argument_is_refused_loudly
    assert_raises(ArgumentError, KeyError) { Snappr::Problem.new(:photo_missing) }
    assert_raises(KeyError) { Snappr::Problem.new(:photo_missing, file: "IMG_1.heic") }
  end

  def test_log_line_carries_both_halves
    problem = Snappr::Problem.new(:photo_missing, file: "IMG_1.heic", path: "/tmp/IMG_1.heic")

    assert_equal "IMG_1.heic", problem.message[/IMG_1\.heic/]
    refute_includes problem.message, "/tmp/"
    assert_includes problem.log_line, problem.message
    assert_includes problem.log_line, "/tmp/IMG_1.heic"
  end

  def test_an_entry_with_no_detail_logs_just_the_sentence
    problem = Snappr::Problem.new(:watch_dir_missing, path: "/nope")

    assert_nil problem.detail
    assert_equal problem.message, problem.log_line
  end

  # The rule for anything snappr did not raise itself: its own text is for the
  # console, and the operator gets the catalogue's sentence instead.
  def test_a_foreign_exception_never_shows_its_own_message
    error = Vips::Error.new("VipsForeignLoad: buffer is not in a known format")

    refute_includes Snappr::Problem.plain(error), "VipsForeignLoad"
    assert_equal :photo_unusable, Snappr::Problem.code(error)
    assert_includes Snappr::Problem.log_line(error), "VipsForeignLoad"
    assert_includes Snappr::Problem.log_line(error), "Vips::Error"
  end

  def test_a_catalogued_problem_keeps_its_own_sentence
    problem = Snappr::Problem.new(:not_ready)

    assert_equal problem.message, Snappr::Problem.plain(problem)
    assert_equal :not_ready, Snappr::Problem.code(problem)
  end

  # Warnings and errors share the catalogue, and a warning's `kind` on the wire
  # is its code - which is what keeps the two vocabularies from drifting apart.
  def test_warn_reports_without_raising
    warnings = []
    Snappr::Problem.warn(warnings, :profile_unusable, error: "icc_transform: no such profile")

    assert_equal 1, warnings.size
    assert_equal "profile_unusable", warnings.first[:kind]
    refute_includes warnings.first[:message], "icc_transform"
  end

  def test_warn_tolerates_no_collector
    assert_silent_of_exceptions { Snappr::Problem.warn(nil, :not_ready) }
  end

  private

  def assert_silent_of_exceptions
    yield
    pass
  end
end
