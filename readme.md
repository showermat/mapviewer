# Mapviewer

[Mapsforge](https://github.com/mapsforge/mapsforge) is a cool format for storing OpenStreetMap data offline, but general-purpose desktop viewers seem to be in short supply.  I've been playing around with the format to see how easy it is to make my own viewer.

This is currently at the proof-of-concept stage and may not progress further.  It can load and render some map data and you can zoom and pan, but it's far from being a usable map application.

## Usage

 1. [Download a Mapsforge map](http://download.mapsforge.org/).
 2. `cargo run -- /path/to/file.map`
