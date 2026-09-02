# frozen_string_literal: true

module Snappr
  # Raised when a file never stops changing on disk. AirDrop writes
  # non-atomically, so this means the transfer stalled rather than finished.
  #
  # A Problem subclass rather than just a catalogue code, because Watcher rescues
  # it by class to tell "the transfer stalled" apart from "the photo is broken" -
  # the first is worth one calm line, the second is a failure.
  #
  # It lives in its own file rather than beside Error in snappr.rb because it
  # needs Problem, and snappr.rb runs before Zeitwerk's loader is set up.
  class UnstableFileError < Problem; end
end
