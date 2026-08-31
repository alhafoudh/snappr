# frozen_string_literal: true

require "listen"
require "set"

module Snappr
  # Watches the drop folder and composites each photo that lands in it.
  class Watcher
    # AirDrop and browsers write to a temp name first and rename on completion.
    PARTIAL_EXTENSIONS = %w[.download .part .crdownload].freeze

    def initialize(config = Snappr.config, overlay: Overlay.new(config))
      @config = config
      @overlay = overlay
      @in_flight = Set.new
      @mutex = Mutex.new
    end

    def run
      dir = @config.watch_dir
      raise Error, "watch dir does not exist: #{dir}" unless dir.directory?

      Snappr.logger.info "watching #{dir} for #{@config.extensions.join(", ")} files"

      # `ignore: %r{/}` keeps listen out of the subdirectories, and it is a
      # performance constraint rather than a matter of taste. listen's macOS
      # adapter turns every fsevent into a *recursive* rescan of the changed
      # directory (`_queue_change(:dir, dir, rel_path, recursive: true)`), and the
      # watched directory is ~/Downloads - a real downloads folder, ~1.9k entries
      # at the top level but ~19k files once its 89 subdirectories are walked. So
      # one unrelated file changing cost 0.5-5s of CPU, and since snappr writes
      # its own 10-25 MB .overlay.png into that same folder, each photo pinned a
      # core for about a minute afterwards. Measured 0.517s -> 0.094s per event.
      #
      # Note this is not the same thing as the `only:` filter or candidate?: both
      # of those run *after* the scan, so they stop the work but not the walk.
      # AirDrop only ever lands files at the top level, so nothing is lost.
      listener = Listen.to(dir.to_s, only: /\.[^.\/]+\z/, ignore: %r{/}) do |modified, added, _removed|
        (added + modified).uniq.each { |path| handle(path) }
      end
      listener.start

      # A trap handler may not take a mutex, and Listen#stop does - so the
      # handler only flips a flag and the actual shutdown happens out here.
      running = true
      trap("INT") { running = false }
      trap("TERM") { running = false }
      sleep 0.2 while running

      listener.stop
      Snappr.logger.info "stopped"
    end

    # A file snappr is willing to touch. Kept as a pure predicate so it can be
    # tested without any filesystem events.
    def candidate?(path)
      name = File.basename(path.to_s)

      # The self-output guard. Output is written next to the input, so without
      # this the watcher could feed on itself. It also covers the in-progress
      # .tmp-<pid> file, which carries the same suffix. Belt and braces now that
      # the output is .png and png is not a watched extension, but it becomes
      # critical again the moment either of those changes.
      return false if name.downcase.end_with?(@config.output_suffix.downcase)
      return false if PARTIAL_EXTENSIONS.any? { |ext| name.downcase.end_with?(ext) }

      @config.extensions.include?(File.extname(name).downcase.delete_prefix("."))
    end

    private

    def handle(path)
      name = File.basename(path)

      unless candidate?(path)
        Snappr.logger.debug "#{name}: ignored, not a file snappr handles"
        return
      end
      unless claim(path)
        Snappr.logger.debug "#{name}: already being processed, event ignored"
        return
      end

      begin
        Snappr.logger.info "#{name}: new photo detected"
        Snappr.logger.info "#{name}: waiting for the transfer to finish"
        wait_until_stable(path)
        Snappr.logger.info "#{name}: transfer complete (#{Snappr.human_size(File.size(path))}), processing"
        destination = @overlay.call(path)
        Snappr.logger.info "#{name}: done"
        open(destination) if @config.open_after?
      rescue UnstableFileError
        Snappr.logger.warn "#{name}: never finished writing, skipped"
      rescue StandardError => e
        # One bad photo must not take the daemon down.
        Snappr.logger.error "#{name}: #{e.class}: #{e.message}"
      ensure
        release(path)
      end
    end

    # listen emits a burst of modify events for a single arriving file; without
    # this the same photo would be composited several times concurrently.
    def claim(path) = @mutex.synchronize { @in_flight.add?(path) ? true : false }
    def release(path) = @mutex.synchronize { @in_flight.delete(path) }

    # AirDrop writes non-atomically, so wait for size and mtime to settle.
    def wait_until_stable(path)
      deadline = Process.clock_gettime(Process::CLOCK_MONOTONIC) + @config.stability_timeout
      last = nil
      stable = 0

      while stable < @config.stability_checks
        raise UnstableFileError, path.to_s if Process.clock_gettime(Process::CLOCK_MONOTONIC) > deadline

        current = fingerprint(path)
        stable = current && current == last ? stable + 1 : 0
        last = current
        sleep @config.stability_interval
      end
    end

    def fingerprint(path)
      stat = File.stat(path)
      [stat.size, stat.mtime.to_f] unless stat.size.zero?
    rescue Errno::ENOENT
      nil
    end

    def open(destination)
      Snappr.logger.info "#{destination.basename}: opening"
      system("open", destination.to_s) or Snappr.logger.warn "open failed for #{destination}"
    end
  end
end
