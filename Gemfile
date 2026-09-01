# frozen_string_literal: true

source "https://rubygems.org"

ruby "3.4.5"

gem "image_processing", "~> 2.0"
# rexml is a bundled gem from Ruby 3.4 on, so it has to be declared explicitly.
gem "rexml", "~> 3.3"
gem "ruby-vips", "~> 2.2"
gem "zeitwerk", "~> 2.6"

# Only `bin/snappr watch` uses listen. Inside snappr.app the drop folder is watched
# by the Rust side instead, so bin/build-gems leaves this group out of the vendored
# tree - which also keeps rb-fsevent's compiled fsevent_watch helper out of the
# bundle, and with it one more Mach-O file that would need signing.
group :watch do
  gem "listen", "~> 3.9"
end

group :test do
  gem "minitest", "~> 5.0"
end
