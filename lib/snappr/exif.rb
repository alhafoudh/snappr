# frozen_string_literal: true

module Snappr
  # The camera's own account of the photo, read off the header libvips has
  # already loaded.
  #
  # This is display data and nothing else. Nothing here reaches the pixels, the
  # crop or the layout - the one EXIF tag that decides anything is Orientation,
  # and libvips consumes that itself in Overlay#load's `.autorot`.
  #
  # Every field is independently optional. A screenshot, a scan and a web
  # download all decode fine and carry no camera tags at all, so #read returns
  # whatever it found and an empty hash is a normal answer, not a failure.
  module Exif
    # libvips renders every EXIF tag as one string in a fixed shape:
    #
    #   "1244236/699009 (f/1.8, Rational, 1 components, 8 bytes)"
    #    ^ raw           ^ human    ^ the annotation's fixed tail
    #
    # The human half is the one worth showing - the raw half of FNumber is
    # 1244236/699009 - so it is what everything below reads.
    #
    # The tail is what this anchors on, rather than "strip the last bracket".
    # ApertureValue's human half is itself parenthesised ("1.66 EV (f/1.8)"),
    # so a bracket-counting parse gets it wrong. Non-greedy `raw` stops at the
    # first " (", greedy `human` then swallows everything up to the tail.
    #
    # Known limit: a lens model containing "(" splits in the wrong place and
    # comes out slightly mangled. Rare enough not to be worth guarding, and it
    # degrades to an ugly string rather than to a wrong one.
    ANNOTATED = /\A(?<raw>.*?) \((?<human>.*), \w+, \d+ components?, \d+ bytes\)\z/m

    # EXIF writes its timestamps with colons in the date, which no date parser
    # takes: "2026:03:21 12:40:52". The zone arrives separately, if at all.
    TIMESTAMP = /\A(\d{4}):(\d{2}):(\d{2})[ T](\d{2}:\d{2}:\d{2})/

    module_function

    # Returns a symbol-keyed hash of the fields this photo actually carries.
    #
    # Takes the image rather than the path so it can share the header read that
    # Overlay#load has already paid for. libvips is lazy: this touches no pixels.
    def read(image)
      camera = camera_of(image)

      {
        camera: camera,
        lens: lens_of(image, camera),
        focal_length: human(image, "exif-ifd2-FocalLength")&.delete(" "),
        focal_length_35mm: integer(image, "exif-ifd2-FocalLengthIn35mmFilm"),
        aperture: human(image, "exif-ifd2-FNumber"),
        # "1/120 sec." is how libvips spells it and "1/120s" is how a camera
        # does. Also covers the long end, where the tag reads "2.0 sec.".
        shutter: human(image, "exif-ifd2-ExposureTime")&.sub(/\s*sec\.\z/, "s"),
        iso: integer(image, "exif-ifd2-ISOSpeedRatings"),
        taken_at: taken_at_of(image)
      }.compact
    end

    # The model alone, not "make model".
    #
    # Nikon, Canon and Fujifilm all repeat the make inside the model - "NIKON
    # Z 6", "Canon EOS R5" - so prepending it stutters, and Apple's make is the
    # useless "Apple". The model is what a person calls the camera. Make is the
    # fallback only for the odd file that carries one and no model.
    def camera_of(image)
      human(image, "exif-ifd0-Model") || human(image, "exif-ifd0-Make")
    end

    # The lens, unless it is the camera saying its own name again.
    #
    # Apple writes LensModel as "<camera> back camera 6.765mm f/1.78": the
    # camera name, then the focal length and aperture the line already prints.
    # One prefix test drops every phone lens and keeps every interchangeable
    # one, which is exactly where the lens is the interesting fact.
    def lens_of(image, camera)
      lens = human(image, "exif-ifd2-LensModel")
      return nil if lens.nil? || (camera && lens.downcase.start_with?(camera.downcase))

      lens
    end

    # ISO 8601, so the frontend can hand it straight to Date. The zone is its
    # own tag and is often missing, in which case the timestamp stays naive -
    # better than inventing an offset the camera never recorded.
    def taken_at_of(image)
      stamp = human(image, "exif-ifd2-DateTimeOriginal") || human(image, "exif-ifd0-DateTime")
      match = stamp&.match(TIMESTAMP)
      return nil unless match

      offset = human(image, "exif-ifd2-OffsetTimeOriginal") || human(image, "exif-ifd2-OffsetTime")
      "#{match[1]}-#{match[2]}-#{match[3]}T#{match[4]}#{offset}"
    end

    # The human half of one tag, or nil if the photo does not carry it.
    def human(image, name)
      raw = value(image, name)
      return nil if raw.nil?

      text = raw.match(ANNOTATED)&.[](:human) || raw
      # libvips hands these back as ASCII-8BIT. Left alone, a tag with any byte above
      # 0x7f - an accented lens name, a camera with a non-Latin model - takes down
      # JSON.generate inside Server#emit, failing a photo whose PNG is already written.
      # Scrub rather than reject: a mangled character in the info line beats no line.
      text = text.dup.force_encoding(Encoding::UTF_8).scrub.strip
      text.empty? ? nil : text
    end

    def integer(image, name)
      text = human(image, name)
      text&.match?(/\A\d+\z/) ? Integer(text) : nil
    end

    # #get raises on a field the image does not have, so it has to be asked
    # first. Cameras vary wildly in what they write, and a missing tag is the
    # normal case rather than an error.
    def value(image, name)
      return nil unless image.get_fields.include?(name)

      field = image.get(name)
      field.is_a?(String) ? field : nil
    rescue Vips::Error
      nil
    end

    # #human and #value stay public: they are the general accessors, and asking
    # for one tag in readable form is a reasonable thing to want from outside.
    # The three composers above are #read's business alone.
    private_class_method :camera_of, :lens_of, :taken_at_of
  end
end
