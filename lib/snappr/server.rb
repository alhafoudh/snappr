# frozen_string_literal: true

require "json"

module Snappr
  # The pipeline as a long-lived subprocess, driven over NDJSON on stdin/stdout.
  # This is what `snappr serve` runs, and what the Tauri shell talks to.
  #
  # One request at a time, strictly sequential. That is a deliberate simplification
  # rather than a limitation: it makes the caller's queue the deduplication
  # mechanism, so the in-flight Set and mutex that Watcher needs to cope with
  # listen's event bursts have no equivalent here.
  #
  # **Stdout carries protocol frames and nothing else.** Snappr.logger writes to
  # stderr for exactly this reason, and `serve` must never `puts`. A single stray
  # line of log output on stdout desynchronises the caller's parser.
  #
  # Frames in:
  #   {"id":1,"cmd":"hello","config":{...},"preview_dir":"/..."}
  #   {"id":2,"cmd":"overlay","source":"/Users/.../IMG_1234.heic"}
  #   {"id":3,"cmd":"configure","config":{...},"preview_dir":"/..."}
  #   {"id":4,"cmd":"ping"}
  #   {"id":5,"cmd":"overlay_preview","svg":"/...","dir":"/...","key":"a1b2c3d4"}
  #
  # Frames out:
  #   {"id":1,"ok":true,"ruby":"3.4.5","vips":"8.18.4"}
  #   {"id":2,"event":"started","source":"..."}
  #   {"id":2,"ok":true,"destination":"...","previews":{...},...}
  #   {"id":2,"ok":false,"error":{"class":"Vips::Error","code":"photo_unusable","message":"..."}}
  #   {"id":5,"ok":true,"thumb":"...","width":440,"height":297,"aspect":1.48,
  #    "window":{"x":0.0,"y":0.0,"width":1.0,"height":1.0},"warnings":[]}
  #
  # **`error.message` is written for the operator; `class` and `code` are for the
  # log.** The shell displays the message verbatim, so nothing technical may go in
  # it - Snappr::Problem's catalogue is what guarantees that, and what keeps a raw
  # libvips string out of a red callout in the window. The class and the code
  # travel so the shell can log them, and the shell must never show either:
  # prefixing the class onto the message is exactly the bug this split exists to
  # avoid. `code` is the catalogue key, so it is stable across rewordings and is
  # the same vocabulary a warning's `kind` uses.
  class Server
    # Only the keys the pipeline actually reads. The watcher-side settings
    # (watch_dir, extensions, stability, open_after) belong to the caller now, so
    # accepting them here would just be a second place for them to drift.
    CONFIG_KEYS = %w[print overlay_svg output_suffix background png_compression preview].freeze

    def initialize(input: $stdin, output: $stdout)
      @input = input
      @output = output
      @output.sync = true
    end

    def run
      # Reading to EOF is the shutdown signal, which is what makes the caller
      # closing the pipe enough - no trap handler, and none of the
      # can't-be-called-from-trap-context trouble that Watcher has to work around.
      @input.each_line do |line|
        line = line.strip
        next if line.empty?

        handle(line)
      end
      0
    end

    private

    def handle(line)
      request = begin
        JSON.parse(line)
      rescue JSON::ParserError => e
        return fail_with(nil, Problem.new(:unparseable_request, error: e.message), "JSON::ParserError")
      end

      id = request["id"]
      case request["cmd"]
      # Same handler as hello, on purpose: it rebuilds Config, Overlay and Preview
      # from scratch, so re-running it is how a running sidecar takes new settings
      # without being restarted. A second name only so the logs stay readable.
      when "hello", "configure" then hello(id, request)
      when "overlay" then overlay(id, request)
      when "overlay_preview" then overlay_preview(id, request)
      when "ping" then emit(id: id, ok: true, pong: true)
      else
        fail_with(id, Problem.new(:unknown_request, cmd: request["cmd"].inspect))
      end
    rescue StandardError => e
      # One bad photo must not take the daemon down - the same rule Watcher#handle
      # follows. The caller still has the original file and can ask again.
      Snappr.logger.error Problem.log_line(e)
      emit(id: request&.dig("id"), ok: false,
           error: { class: e.class.name, code: Problem.code(e), message: Problem.plain(e) })
    end

    # Logs both halves of a catalogued problem and puts the operator's half on
    # the wire. The class travels for the shell's log and is never displayed.
    def fail_with(id, problem, klass = problem.class.name)
      Snappr.logger.error problem.log_line
      emit(id: id, ok: false, error: { class: klass, code: problem.code, message: problem.message })
    end

    def hello(id, request)
      data = Config::DEFAULTS.merge(request["config"].to_h.slice(*CONFIG_KEYS)) do |_key, default, given|
        default.is_a?(Hash) ? default.merge(given) : given
      end
      config = Config.new(data)
      validate!(config)

      Snappr.config = config
      @overlay = Overlay.new(config)
      @preview = Preview.new(config, dir: request.fetch("preview_dir"))

      emit(id: id, ok: true, ruby: RUBY_VERSION, vips: Vips::LIBRARY_VERSION, pid: Process.pid)
    end

    def overlay(id, request)
      raise Problem.new(:not_ready) unless @overlay

      source = request.fetch("source")
      emit(id: id, event: "started", source: source)

      result = @overlay.process(source)
      previews = @preview.call(source: source, destination: result[:destination])

      emit(**result.merge(id: id, ok: true, previews: previews))
    end

    # The picture of one overlay that the panel draws, and where the photo window
    # is inside it. Nothing to do with a photo, so it needs no `source` and does
    # not go near Overlay or Preview.
    #
    # It needs no handshake either - OverlayPreview reads the file it is given and
    # the paper's landscape ratio, and nothing else - but it is still gated on
    # @overlay, because Snappr.config is what supplies that ratio and only `hello`
    # sets it.
    #
    # `dir` and `key` are the caller's to choose, and are fetched rather than
    # defaulted for the reason in the header of lib/snappr/preview.rb. The shell
    # keys these on the file's mtime and deletes the ones no overlay points at any
    # more; Ruby has no index and could not tell a stale one from a current one.
    def overlay_preview(id, request)
      raise Problem.new(:not_ready) unless @overlay

      result = OverlayPreview.call(svg: request.fetch("svg"), dir: request.fetch("dir"),
                                   key: request.fetch("key"))
      emit(**result.merge(id: id, ok: true))
    end

    # Config coerces lazily, so a bad background colour or a non-numeric paper
    # dimension would otherwise surface deep inside the pipeline on some later
    # photo. Force every value now and fail the handshake instead.
    def validate!(config)
      config.print_ratio(:landscape)
      config.print_ratio(:portrait)
      config.background_rgb
      config.png_compression
      config.preview_max_px
      config.preview_thumb_px
      config.preview_quality
      # Only that the paths are resolvable, deliberately not that the files exist.
      # An empty slot is a legitimate setting, and a slot whose file has been moved
      # degrades to no overlay with a warning at process time - failing the
      # handshake over either would mean an app that will not launch.
      Config::ORIENTATIONS.each { |orientation| config.configured_overlay_svg(orientation) }
    end

    # Pathnames and Symbols are not JSON scalars, so normalise rather than letting
    # them serialise as whatever #to_json happens to do.
    def emit(**frame)
      @output.puts(JSON.generate(scalarize(frame)))
    end

    def scalarize(value)
      case value
      when Hash then value.to_h { |key, inner| [key.to_s, scalarize(inner)] }
      when Array then value.map { |inner| scalarize(inner) }
      when Pathname then value.to_s
      when Symbol then value.to_s
      when Float then value.finite? ? value : nil
      else value
      end
    end
  end
end
