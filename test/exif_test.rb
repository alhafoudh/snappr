# frozen_string_literal: true

require "test_helper"
require "json"

class ExifTest < Minitest::Test
  include Snappr::TestHelpers

  # Every assertion here goes through a real libvips write and read back, so the
  # annotated strings under test are the ones libvips actually produces rather than a
  # hand-written imitation of them.
  def with_exif(tags, &block)
    in_tmpdir do |dir|
      path = write_fixture(dir, "photo.jpg", 64, 48, exif: tags)
      block.call(Vips::Image.new_from_file(path).autorot)
    end
  end

  def read(tags) = with_exif(tags) { |image| Snappr::Exif.read(image) }

  def test_reads_the_fields_a_phone_writes
    got = read(APPLE_EXIF)

    assert_equal "iPhone 17 Pro", got[:camera]
    assert_equal "6.8mm", got[:focal_length]
    assert_equal 24, got[:focal_length_35mm]
    assert_equal "f/1.8", got[:aperture]
    assert_equal "1/120s", got[:shutter]
    assert_equal 400, got[:iso]
    assert_equal "2026-03-21T12:40:52+01:00", got[:taken_at]
  end

  # A screenshot, a scan or a web download decodes fine and carries no camera tags.
  # That is an ordinary photo for this pipeline, not a broken one.
  def test_a_photo_without_camera_tags_reads_as_empty
    assert_empty read({})
  end

  # Each field has to stand on its own: cameras vary wildly in what they write, and one
  # missing tag must not take the rest of the line down with it.
  def test_every_field_is_independently_optional
    APPLE_EXIF.each_key do |dropped|
      got = read(APPLE_EXIF.reject { |tag, _| tag == dropped })

      refute_empty got, "dropping #{dropped} should not empty the whole hash"
    end
  end

  # The human half of the annotation is the one worth showing: FNumber's raw half is
  # the unreadable 1244236/699009 that the rational actually holds.
  def test_prefers_the_readable_half_over_the_raw_rational
    got = read("exif-ifd2-FNumber" => "1244236/699009")

    assert_equal "f/1.8", got[:aperture]
  end

  # ApertureValue renders as "1.66 EV (f/1.8)" - the human half is itself
  # parenthesised, so a parse that counts brackets from the end gets it wrong. Asserted
  # against the parser directly, since #read does not use this tag.
  def test_parses_an_annotation_whose_readable_half_is_itself_parenthesised
    with_exif("exif-ifd2-ApertureValue" => "42657/25639") do |image|
      assert_equal "1.66 EV (f/1.8)", Snappr::Exif.human(image, "exif-ifd2-ApertureValue")
    end
  end

  def test_asking_for_a_tag_the_photo_does_not_carry_is_nil
    with_exif({}) do |image|
      assert_nil Snappr::Exif.human(image, "exif-ifd2-LensModel")
    end
  end

  # Apple writes LensModel as the camera's name followed by the focal length and
  # aperture the line already prints, so it says nothing the rest of the line does not.
  def test_drops_a_lens_that_only_restates_the_camera
    assert_nil read(APPLE_EXIF)[:lens]
  end

  # On an interchangeable-lens camera the lens is the interesting fact, and the same
  # prefix rule keeps it.
  def test_keeps_a_lens_that_is_not_the_camera_again
    got = read("exif-ifd0-Make" => "NIKON CORPORATION",
               "exif-ifd0-Model" => "NIKON Z 6",
               "exif-ifd2-LensModel" => "NIKKOR Z 24-70mm f/2.8 S")

    assert_equal "NIKON Z 6", got[:camera]
    assert_equal "NIKKOR Z 24-70mm f/2.8 S", got[:lens]
  end

  # Every maker but Apple repeats itself in the model - "NIKON Z 6", "Canon EOS R5" -
  # so the model alone is what a person calls the camera. Make is the fallback only.
  def test_the_camera_is_the_model_not_the_make_and_model
    assert_equal "Canon EOS R5", read("exif-ifd0-Make" => "Canon", "exif-ifd0-Model" => "Canon EOS R5")[:camera]
    assert_equal "Apple", read("exif-ifd0-Make" => "Apple")[:camera]
  end

  # The zone lives in its own tag, and plenty of cameras never write it. A naive
  # timestamp is the honest answer - better than inventing an offset.
  def test_a_timestamp_without_a_zone_stays_naive
    got = read("exif-ifd2-DateTimeOriginal" => "2026:03:21 12:40:52")

    assert_equal "2026-03-21T12:40:52", got[:taken_at]
  end

  # DateTime is the file's own modification time in EXIF terms, and is all a camera
  # that writes no DateTimeOriginal will give.
  def test_falls_back_to_ifd0_datetime
    got = read("exif-ifd0-DateTime" => "2026:03:21 12:40:52")

    assert_equal "2026-03-21T12:40:52", got[:taken_at]
  end

  # Long exposures spell it "2 sec." rather than as a fraction.
  def test_formats_a_whole_second_exposure
    assert_equal "2s", read("exif-ifd2-ExposureTime" => "2/1")[:shutter]
  end

  # libvips hands every tag back as ASCII-8BIT, so a value with any byte above 0x7f in
  # it - an accented lens name, a camera with a non-Latin model - would blow up
  # JSON.generate deep inside Server#emit and fail a photo whose PNG is already
  # written. Everything is transcoded and scrubbed on the way out.
  #
  # Set on an in-memory image rather than through write_fixture: libvips writes EXIF
  # ASCII fields to the 7-bit spec and transliterates the accent away, so a fixture
  # that goes to disk cannot carry the bytes this is about. That makes it a test of the
  # unannotated path too, which is what a mutated image gives.
  def test_a_tag_with_high_bytes_survives_json
    image = fixture_image(8, 8).mutate do |m|
      m.set_type!(GObject::GSTR_TYPE, "exif-ifd0-Model", "Hasselblad 907X \xC3\xA9dition".b)
    end
    got = Snappr::Exif.read(image)

    assert_equal Encoding::UTF_8, got[:camera].encoding
    assert_equal "Hasselblad 907X édition", got[:camera]
    assert_equal got, JSON.parse(JSON.generate(got), symbolize_names: true)
  end

  # And a value that is not valid UTF-8 in any encoding still has to come out the far
  # side: a mangled character in the info line beats a failed photo.
  def test_a_tag_with_undecodable_bytes_is_scrubbed_rather_than_dropped
    image = fixture_image(8, 8).mutate do |m|
      m.set_type!(GObject::GSTR_TYPE, "exif-ifd0-Model", "Leica \xFF\xFEM11".b)
    end
    got = Snappr::Exif.read(image)

    assert got[:camera].valid_encoding?
    assert_includes got[:camera], "Leica"
    assert_equal got, JSON.parse(JSON.generate(got), symbolize_names: true)
  end
end
