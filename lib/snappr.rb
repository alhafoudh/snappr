# frozen_string_literal: true

require "logger"
require "pathname"
require "zeitwerk"

# Requiring image_processing/vips runs `Vips.block_untrusted(true)`, and libvips
# marks svgload as an untrusted loader - so merely loading the gem disables SVG
# loading process-wide and every overlay render fails with
# "buffer is not in a known format". Unblock the one loader we need, by name,
# rather than lifting the block wholesale: guest photos are semi-trusted input
# and should keep the hardening, while share/overlay.svg is a committed asset.
# This must run AFTER the require, or the gem re-blocks it.
require "image_processing/vips"
Vips.block("VipsForeignLoadSvg", false)

module Snappr
  class Error < StandardError; end

  # Raised when a file never stops changing on disk. AirDrop writes
  # non-atomically, so this means the transfer stalled rather than finished.
  class UnstableFileError < Error; end

  class << self
    # SNAPPR_ROOT comes from mise's {{config_root}}, so paths resolve against
    # the project rather than the working directory. The __dir__ fallback keeps
    # bin/snappr working when it is invoked directly.
    def root
      @root ||= Pathname.new(ENV["SNAPPR_ROOT"] || File.expand_path("..", __dir__)).expand_path
    end

    def config
      @config ||= Config.load
    end

    def logger
      @logger ||= Logger.new($stdout, level: Logger::INFO).tap do |log|
        log.formatter = ->(severity, time, _progname, msg) do
          "#{time.strftime("%H:%M:%S")} #{severity.ljust(5)} #{msg}\n"
        end
      end
    end
  end
end

Zeitwerk::Loader.for_gem.tap do |loader|
  loader.inflector.inflect("overlay_svg" => "OverlaySvg", "cli" => "CLI")
  loader.setup
end
