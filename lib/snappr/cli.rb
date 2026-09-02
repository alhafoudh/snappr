# frozen_string_literal: true

module Snappr
  class CLI
    USAGE = <<~TEXT
      usage: snappr <command>

        serve              read NDJSON requests on stdin and write results to stdout
        watch              watch the drop folder and overlay each photo that lands in it
        overlay PATH...    overlay the given photos now, then open them
        help               this message
    TEXT

    def self.call(argv) = new.call(argv)

    def call(argv)
      command, *args = argv
      case command
      # serve, not watch, is the default: inside snappr.app this is spawned as a
      # sidecar with no arguments, and the app owns the watching.
      when nil, "serve" then serve
      when "watch" then watch
      when "overlay" then overlay(args)
      when "help", "-h", "--help" then puts USAGE
      else
        warn "#{Problem.new(:unknown_command, command: command).message}\n\n#{USAGE}"
        1
      end
    # A Problem's message is already a sentence for whoever typed the command, so
    # it is printed on its own. Ruby's default handler would put a backtrace on
    # top of it, which reads as a crash rather than as "that folder does not
    # exist". Anything else keeps its backtrace on purpose: that one is a bug in
    # snappr and the trace is the useful part.
    rescue Problem => e
      warn e.message
      Snappr.logger.debug e.detail if e.detail
      1
    end

    private

    def serve
      Server.new.run
      0
    end

    def watch
      Watcher.new.run
      0
    end

    def overlay(paths)
      if paths.empty?
        warn "#{Problem.new(:no_paths_given).message}\n\n#{USAGE}"
        return 1
      end

      overlay = Overlay.new
      failed = 0
      Snappr.logger.info "overlaying #{paths.size} photo#{"s" unless paths.size == 1}"
      paths.each do |path|
        destination = overlay.call(path)
        if Snappr.config.open_after?
          Snappr.logger.info "#{destination.basename}: opening"
          system("open", destination.to_s)
        end
      rescue StandardError => e
        failed += 1
        Snappr.logger.error Problem.log_line(e)
      end
      failed.zero? ? 0 : 1
    end
  end
end
