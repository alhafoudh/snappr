# frozen_string_literal: true

require "yaml"

module Snappr
  # config/snappr.yml, with every path resolved against the project root.
  class Config
    DEFAULTS = {
      "watch_dir" => "~/Downloads",
      "extensions" => %w[heic heif jpg jpeg],
      "print" => { "short_mm" => 100, "long_mm" => 148 },
      # No branding by default. The overlays are an operator setting picked in the
      # app, not an asset this project ships - see #overlay_svg. `share/` holds
      # test material only, which is why config/snappr.yml (the CLI's config, not
      # the app's) is the one place that still names those files.
      "overlay_svg" => { "landscape" => nil, "portrait" => nil },
      "output_suffix" => ".overlay.png",
      "png_compression" => 6,
      "background" => "#ffffff",
      "open_after" => true,
      "stability" => { "interval" => 0.3, "checks" => 2, "timeout" => 30 },
      "preview" => { "max_px" => 2000, "thumb_px" => 400, "quality" => 80 }
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

    # The overlay to composite for an orientation, or nil for none.
    #
    # nil is a legitimate configuration, not a failure: the operator can clear
    # either slot, and a cleared slot prints the crop with no branding over it.
    #
    # A slot that is set but whose file has gone warns and degrades to nil for the
    # same reason. Someone moves a branding folder mid-event; a kiosk that keeps
    # printing plain photos is recoverable, one that errors on every photo is not.
    # The warning reaches the filmstrip, so it is not silent.
    #
    # There is deliberately no fall back to the other orientation's SVG. That
    # existed while the overlays were committed assets that "may ship only one
    # orientation"; now that each slot is picked explicitly, clearing portrait has
    # to mean no portrait overlay rather than a stretched landscape one.
    def overlay_svg(orientation, warnings = nil)
      wanted = configured_overlay_svg(orientation)
      return nil if wanted.nil?
      return wanted if wanted.file?

      message = "#{wanted} is missing - printing #{orientation} photos with no overlay until it is picked again"
      Snappr.logger.warn message
      warnings&.push({ kind: "overlay_missing", message: message })
      nil
    end

    # The configured path for an orientation, whether or not the file exists, or
    # nil when the slot is empty.
    def configured_overlay_svg(orientation)
      orientation = orientation.to_sym
      raise Error, "unknown orientation: #{orientation}" unless ORIENTATIONS.include?(orientation)

      path = @data.fetch("overlay_svg", {})[orientation.to_s]
      path.to_s.empty? ? nil : resolve(path)
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

    # Long edge of the previews the UI displays, in pixels, and their JPEG
    # quality. These govern display only - the printed PNG is unaffected, so
    # quality here is a size-against-appearance trade with nothing at stake.
    def preview_max_px   = Integer(@data.dig("preview", "max_px"))
    def preview_thumb_px = Integer(@data.dig("preview", "thumb_px"))
    def preview_quality  = Integer(@data.dig("preview", "quality"))

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
