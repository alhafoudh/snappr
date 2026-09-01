# frozen_string_literal: true

# Runs inside the bundled Ruby runtime, driven by bin/verify-bundle. Not meant to
# be run directly: it expects LD_LIBRARY_PATH to point at the bundled libvips and
# SNAPPR_ROOT at the project.
#
# Exercises the whole pipeline on a HEIC, because HEIC is the format that
# distinguishes a working bundle from one that quietly borrowed Homebrew's
# libheif: it needs the runtime-dlopened vips-heif module, which is exactly what a
# dependency walk cannot see.
#
# Prints one line of JSON on stdout. Everything else - logs, and the flood from
# DYLD_PRINT_LIBRARIES - goes to stderr, which is what makes both halves of the
# verification readable.

require "fileutils"
require "json"
require "tmpdir"
require "snappr"

result = {}

Dir.mktmpdir("snappr-verify") do |dir|
  # A landscape HEIC, built outside the bundle by bin/verify-bundle. Landscape
  # because that is the orientation with real committed branding:
  # share/overlay-portrait.svg was deleted in c4bdfaf, so portrait photos currently
  # fall back to the stretched landscape overlay and a portrait fixture here would
  # assert a known product gap rather than anything about the bundle.
  source = File.join(dir, File.basename(ENV.fetch("SNAPPR_VERIFY_HEIC")))
  FileUtils.cp(ENV.fetch("SNAPPR_VERIFY_HEIC"), source)

  # Prove the pixels really decode. new_from_file only touches the header, so a
  # broken HEVC decoder passes a dimensions check - which is exactly how sharp's
  # libvips looked healthy right up until it was asked for actual pixels.
  result[:heic_pixels] = Vips::Image.new_from_file(source).crop(0, 0, 8, 8).avg.round(2)

  overlay = Snappr::Overlay.new.process(source)
  result.merge!(
    source_width: overlay[:source_width], source_height: overlay[:source_height],
    orientation: overlay[:orientation].to_s, svg: File.basename(overlay[:svg]),
    output_width: overlay[:output_width], output_height: overlay[:output_height],
    depth: overlay[:depth], bytes: overlay[:bytes],
    warnings: overlay[:warnings]
  )

  previews = Snappr::Preview.new(dir: File.join(dir, "previews"))
                            .call(source: source, destination: overlay[:destination])

  result[:previews] = previews.except(:key).transform_values { |path| File.size(path) }

  thumb = Vips::Image.new_from_file(previews[:thumb].to_s)
  result[:thumb_long_edge] = [thumb.width, thumb.height].max
  result[:preview_has_icc] = thumb.get_fields.include?("icc-profile-data")
end

puts JSON.generate(result)
