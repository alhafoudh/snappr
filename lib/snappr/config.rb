# frozen_string_literal: true

require "yaml"

module Snappr
  # config/snappr.yml, with every path resolved against the project root.
  class Config
    DEFAULTS = {
      "watch_dir" => "~/Downloads",
      "extensions" => %w[heic heif jpg jpeg],
      "print" => { "short_mm" => 100, "long_mm" => 148 },
      "overlay_svg" => {
        "landscape" => "share/overlay-landscape.svg",
        "portrait" => "share/overlay-portrait.svg"
      },
      "output_suffix" => ".overlay.png",
      "png_compression" => 6,
      "background" => "#ffffff",
      "open_after" => true,
      "stability" => { "interval" => 0.3, "checks" => 2, "timeout" => 30 }
    }.freeze

    def self.load(path = Snappr.root.join("config/snappr.yml"))
      file = File.exist?(path) ? YAML.safe_load_file(path) || {} : {}
      new(DEFAULTS.merge(file) { |_key, default, given| default.is_a?(Hash) ? default.merge(given) : given })
    end

    def initialize(data)
      @data = data
    end

    def watch_dir     = Pathname.new(File.expand_path(@data["watch_dir"]))
    def extensions    = @data["extensions"].map { |ext| ext.to_s.downcase.delete_prefix(".") }
    def output_suffix = @data["output_suffix"]

    ORIENTATIONS = %i[landscape portrait].freeze

    # The branding may ship only one orientation. A stretched overlay still
    # warns and prints, a missing one prints nothing - so fall back to the other
    # orientation's SVG, and error out only when neither file exists.
    def overlay_svg(orientation)
      wanted = configured_overlay_svg(orientation)
      return wanted if wanted.file?

      other = (ORIENTATIONS - [orientation.to_sym]).first
      fallback = configured_overlay_svg(other)
      raise Error, "no overlay SVG found: neither #{wanted} nor #{fallback} exists" unless fallback.file?

      Snappr.logger.warn "#{wanted} is missing, using the #{other} overlay #{File.basename(fallback)} instead"
      fallback
    end

    # The configured path for an orientation, whether or not the file exists.
    def configured_overlay_svg(orientation)
      orientation = orientation.to_sym
      raise Error, "unknown orientation: #{orientation}" unless ORIENTATIONS.include?(orientation)

      resolve(@data.fetch("overlay_svg").fetch(orientation.to_s))
    end

    def print_short_mm = Float(@data.dig("print", "short_mm"))
    def print_long_mm  = Float(@data.dig("print", "long_mm"))

    # Target width/height ratio of the print, with the long side following the
    # photo's own orientation.
    def print_ratio(orientation)
      case orientation.to_sym
      when :landscape then print_long_mm / print_short_mm
      when :portrait then print_short_mm / print_long_mm
      else raise Error, "unknown orientation: #{orientation}"
      end
    end

    def png_compression = Integer(@data["png_compression"])

    # Fills whatever the overlay does not cover and the photo does not reach.
    def background_rgb
      hex = @data["background"].to_s.delete_prefix("#")
      raise Error, "background must be a #rrggbb colour, got #{@data["background"].inspect}" unless /\A[0-9a-f]{6}\z/i.match?(hex)

      hex.scan(/../).map { |pair| pair.to_i(16) }
    end
    def open_after?   = !!@data["open_after"]

    def stability_interval = Float(@data.dig("stability", "interval"))
    def stability_checks   = Integer(@data.dig("stability", "checks"))
    def stability_timeout  = Float(@data.dig("stability", "timeout"))

    private

    # Relative paths belong to the project; absolute and ~ paths are taken as given.
    def resolve(path)
      expanded = Pathname.new(File.expand_path(path.to_s, Snappr.root.to_s))
      expanded.absolute? ? expanded : Snappr.root.join(path)
    end
  end
end
