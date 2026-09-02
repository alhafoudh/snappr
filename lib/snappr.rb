# frozen_string_literal: true

require "logger"
require "pathname"
require "zeitwerk"

# Requiring image_processing/vips runs `Vips.block_untrusted(true)`, and libvips
# marks svgload as an untrusted loader - so merely loading the gem disables SVG
# loading process-wide and every overlay render fails with
# "buffer is not in a known format". Unblock the one loader we need, by name,
# rather than lifting the block wholesale: guest photos are semi-trusted input
# and should keep the hardening, while the overlay is a file the operator picked.
# This must run AFTER the require, or the gem re-blocks it.
require "image_processing/vips"
Vips.block("VipsForeignLoadSvg", false)

module Snappr
  # The base class, and the marker for "this message was written for the person
  # holding the print". Almost everything raised is really a Snappr::Problem,
  # which carries an operator sentence and a console detail from one catalogue -
  # see lib/snappr/problem.rb. Error itself stays here, and bare, because it is
  # referenced before Zeitwerk's loader is set up at the bottom of this file.
  class Error < StandardError; end

  class << self
    # SNAPPR_ROOT comes from mise's {{config_root}}, so paths resolve against
    # the project rather than the working directory. The __dir__ fallback keeps
    # bin/snappr working when it is invoked directly.
    def root
      @root ||= Pathname.new(ENV["SNAPPR_ROOT"] || File.expand_path("..", __dir__)).expand_path
    end

    # Normally config/snappr.yml. `snappr serve` replaces it with the config that
    # arrived over the protocol, so this needs a writer - the memoization below
    # has no other way back out.
    attr_writer :config

    def config
      @config ||= Config.load
    end

    # Logs go to stderr, not stdout. That is not tidiness: under `snappr serve`
    # stdout is the NDJSON protocol channel, and a single log line written there
    # corrupts the stream. This is the only place log output is opened, so keeping
    # it on stderr here is the whole guarantee.
    #
    # SNAPPR_LOG_LEVEL=debug also shows the events the watcher ignores.
    def logger
      @logger ||= Logger.new($stderr, level: ENV.fetch("SNAPPR_LOG_LEVEL", "info")).tap do |log|
        log.formatter = ->(severity, time, _progname, msg) do
          "#{time.strftime("%H:%M:%S")} #{severity.ljust(5)} #{msg}\n"
        end
      end
    end

    def human_size(bytes)
      bytes < 1024 * 1024 ? format("%d KB", bytes / 1024.0) : format("%.1f MB", bytes / (1024.0 * 1024))
    end
  end
end

Zeitwerk::Loader.for_gem.tap do |loader|
  loader.inflector.inflect("overlay_svg" => "OverlaySvg", "cli" => "CLI")
  loader.setup
end
