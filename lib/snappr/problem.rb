# frozen_string_literal: true

module Snappr
  # Every message snappr can show, in one table, each with both of its halves.
  #
  # A failure has two audiences and they want opposite things. The operator wants
  # a sentence: what happened, and what to do about it. A developer wants the
  # exception class, the libvips text, the errno and the full path. Writing those
  # at the raise site means nothing guarantees both exist, and the version that
  # gets written is whichever audience the author had in mind - which is how
  # `Snappr::Error: no #FF00FF photo window found in brand.svg` ended up in the
  # window.
  #
  # So they are declared together, keyed by a stable code:
  #
  #   ui:     what the operator reads. No class names, no hex literals, no repo
  #           jargon, no full paths. One or two sentences: what happened, then
  #           what to do. This is the exception's #message, and the shell prints
  #           it verbatim.
  #   detail: what the console gets, appended after the ui half in brackets.
  #           Paths, ratios, vips text, byte offsets. Optional - plenty of
  #           problems have nothing to add.
  #
  # Both are `format` templates using `%{named}` references, so a missing
  # argument is a KeyError at the raise site rather than a half-written sentence
  # in front of an operator.
  #
  # **The code is also the warning's `kind` on the wire**, so the four existing
  # kinds keep their exact spelling - see Server's frame documentation.
  class Problem < Error
    CATALOG = {
      # --- The overlays. Everything an operator is most likely to actually hit,
      # because these are the one part of the pipeline they author themselves.
      overlay_window_missing: {
        ui: "The %{slot} %{file} does not show where the photo goes. Open it in your " \
            "design tool and add a magenta rectangle over the area the photo should " \
            "fill, then export it again over the same file.",
        detail: "no #FF00FF marker pixels in %{path}"
      },
      overlay_file_missing: {
        ui: "The %{slot} %{file} is no longer where it was. Add it again in the " \
            "overlay panel, or put the file back.",
        detail: "%{path} does not exist"
      },
      overlay_unreadable: {
        ui: "The %{slot} %{file} is empty, or is not a drawing Snappr can read. " \
            "Export it again from your design tool.",
        detail: "%{path} parsed but has no root element"
      },
      overlay_no_size: {
        ui: "The %{slot} %{file} does not say what size it was drawn at, so Snappr " \
            "cannot scale it. Export it again from your design tool - most tools " \
            "write the size automatically.",
        detail: "%{path} has no viewBox and no absolute width/height"
      },
      # Distinct from overlay_no_size: the file said its size and the size was
      # unusable, so there is nothing for the operator to add - only a redraw
      # fixes it. Only the panel's preview hits this; the pipeline never asks an
      # overlay for its own shape, it stretches it to the paper's.
      overlay_no_shape: {
        ui: "The overlay %{file} does not say what shape it was drawn at, so its " \
            "preview is being shown at the paper's shape. Export it again from " \
            "your design tool.",
        detail: "%{path} has a viewBox with a zero or negative height"
      },
      overlay_missing: {
        ui: "The %{slot} %{file} is missing, so %{orientation} photos are printing " \
            "with no branding. Add it again in the overlay panel.",
        detail: "%{path} is configured but not on disk"
      },
      aspect_mismatch: {
        ui: "The %{slot} %{file} is not the shape of the paper, so the branding is " \
            "being stretched. Redraw it at %{width} x %{height} px, or any size with " \
            "the same shape.",
        detail: "drawn at %{drawn}:1, %{orientation} prints at %{wanted}:1"
      },
      marker_survived: {
        ui: "The photo area in the %{slot} %{file} could not be removed cleanly, so " \
            "Snappr hid it instead. In your design tool, give that magenta shape a " \
            "plain fill colour.",
        detail: "marker pixels survived the element walk in %{path}, keyed out on alpha"
      },

      # --- The photo itself.
      photo_missing: {
        ui: "The photo %{file} is not there any more - it looks like it was moved or " \
            "deleted.",
        detail: "%{path} is not a file"
      },
      transfer_stalled: {
        ui: "%{file} never finished arriving, so Snappr skipped it. AirDrop it again.",
        detail: "size and mtime never settled within %{timeout}s: %{path}"
      },
      photo_unusable: {
        ui: "Snappr could not make a print from this photo. It may be damaged, or in " \
            "a format Snappr cannot read.",
        detail: "%{error}"
      },
      profile_unusable: {
        ui: "Snappr could not match the branding's colours to this photo, so they may " \
            "print slightly off.",
        detail: "icc_transform failed: %{error}"
      },
      open_failed: {
        ui: "Snappr could not open %{file}.",
        detail: "open(1) failed for %{path}"
      },

      # --- Settings and the watch folder.
      watch_dir_missing: {
        # The path *is* the fact here, so it stays on screen. That is the rule:
        # a path appears when the operator has to go and look at it, not as
        # decoration on a sentence that already reads without it.
        ui: "The folder Snappr was told to watch does not exist: %{path}",
        detail: nil
      },
      background_not_a_colour: {
        ui: "Snappr's background setting is not a colour: %{value}. It should look " \
            "like #ffffff.",
        detail: nil
      },

      # --- Internal. An operator cannot reach any of these: they mean the shell
      # and the pipeline disagree about the protocol, or a caller inside snappr
      # passed something impossible. They still carry a `ui` half, because
      # "unreachable" is a claim about today's code and the shell would display
      # whatever it was handed.
      not_ready: {
        ui: "Snappr is not ready yet. Try this photo again in a moment.",
        detail: "overlay requested before the hello handshake"
      },
      unknown_request: {
        ui: "Snappr was asked to do something it does not know how to do.",
        detail: "unknown cmd: %{cmd}"
      },
      unparseable_request: {
        ui: "Snappr could not understand a request.",
        detail: "%{error}"
      },
      unknown_orientation: {
        ui: "Snappr got confused about which way round this photo is.",
        detail: "unknown orientation: %{orientation}"
      },

      # --- The command line. `snappr help` prints these, so they are the one
      # place a shell-style lower-case phrasing would be at home - and they are
      # still sentences, because the person typing the command is the operator.
      unknown_command: {
        ui: "Snappr has no command called \"%{command}\".",
        detail: nil
      },
      no_paths_given: {
        ui: "Tell Snappr which photos to overlay, for example:\n" \
            "  snappr overlay ~/Downloads/IMG_1234.heic",
        detail: nil
      }
    }.freeze

    class << self
      # Reports a problem without raising: logs both halves and appends the
      # operator's half to a `warnings` array, in the shape Server puts on the
      # wire. This is the whole of "a degradation" - the pipeline carries on.
      def warn(warnings, code, **args)
        new(code, **args).tap do |problem|
          Snappr.logger.warn problem.log_line
          warnings&.push({ kind: problem.code.to_s, message: problem.message })
        end
      end

      # The operator's half of any exception, catalogued or not. A Problem
      # already carries one; a Vips::Error or a KeyError does not and must not
      # have its own text shown, so it borrows :photo_unusable.
      def plain(error)
        error.is_a?(Problem) ? error.message : new(:photo_unusable, error: error.message).message
      end

      # The catalogue key for any exception. Stable across rewordings, which is
      # what makes it the thing to assert on and the thing to log.
      def code(error) = error.is_a?(Problem) ? error.code : :photo_unusable

      # The console's line for any exception: the operator's sentence, then the
      # class and the original text.
      def log_line(error)
        return error.log_line if error.is_a?(Problem)

        "#{plain(error)} [#{error.class}: #{error.message}]"
      end
    end

    attr_reader :code, :detail

    def initialize(code, **args)
      entry = CATALOG.fetch(code) { raise ArgumentError, "no such problem: #{code.inspect}" }
      @code = code
      @detail = entry[:detail] && format(entry[:detail], **args)
      super(format(entry[:ui], **args))
    end

    # What goes to stderr. Both halves, always in the same order, so a log is
    # readable by whoever is holding the printer and still diagnosable later.
    def log_line = detail ? "#{message} [#{detail}]" : message
  end
end
