use std::collections::HashMap;
use std::fs::File;
use std::ops::Add;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use memmap::Mmap;
use owning_ref::OwningHandle;

mod parse;

const LON_MAX: f64 = 179.9999;
const LAT_MAX: f64 = 85.0511;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LatLon {
	// All fields in microdegrees
	lat: i32,
	lon: i32,
}

impl LatLon {
	fn new(lat: i32, lon: i32) -> Self {
		Self { lat: lat, lon: lon }
	}

	fn constrain(&self) -> Self {
		Self {
			lat: self.lat.clamp((-LAT_MAX * 1e6) as i32, (LAT_MAX * 1e6) as i32),
			lon: self.lon.clamp((-LON_MAX * 1e6) as i32, (LON_MAX * 1e6) as i32)
		}
	}
}

#[derive(Debug)]
struct BoundingBox {
	// All fields in microdegrees
	lat_min: i32,
	lon_min: i32,
	lat_max: i32,
	lon_max: i32,
}

impl BoundingBox {
	fn minmax(&self) -> (LatLon, LatLon) {
		(LatLon::new(self.lat_max, self.lon_min), LatLon::new(self.lat_min, self.lon_max))
	}
}

#[derive(Debug)]
struct ZoomInterval {
	base: u8,
	min: u8,
	max: u8,
	start: u64,
	len: u64,
}

#[derive(Debug, Clone)]
pub enum TagDesc {
	Literal(String),
	Byte,
	Short,
	Int,
	Float,
	String,
}

impl TagDesc {
	fn parse(s: String) -> (String, Self) {
		let fields = s.splitn(2, '=').collect::<Vec<_>>();
		let chars = fields[1].chars().collect::<Vec<char>>();
		let val = if chars.len() == 2 && chars[0] == '%' {
			match chars[1] {
				'b' => TagDesc::Byte,
				'h' => TagDesc::Short,
				'i' => TagDesc::Int,
				'f' => TagDesc::Float,
				's' => TagDesc::String,
				_ => panic!("Raise an error"), // TODO
			}
		}
		else {
			TagDesc::Literal(fields[1].to_string())
		};
		(fields[0].to_string(), val)
	}
}

#[derive(Debug)]
pub enum TagValue {
	Literal(String),
	Byte(i8),
	Short(i16),
	Int(i32),
	Float(f32),
	String(String),
}

fn tile_origin(level: u8, xtile: u32, ytile: u32) -> LatLon {
	use std::f64::consts::PI;
	let n = (2 as i32).pow(level as u32) as f64;
	let lon = xtile as f64 / n * 360.0 - 180.0;
	let lat = (PI * (1.0 - 2.0 * ytile as f64 / n)).sinh().atan().to_degrees();
	LatLon::new((lat * 1e6) as i32, (lon * 1e6) as i32)
}

// https://wiki.openstreetmap.org/wiki/Slippy_map_tilenames
fn biased_coord2tile(level: u8, coord: LatLon, bias_low: bool) -> (u32, u32) {
	use std::f64::consts::PI;
	let lat_rad = (coord.lat as f64 / 1000000.0).clamp(-LAT_MAX, LAT_MAX).to_radians();
	let n = (2 as i32).pow(level as u32) as f64;
	let mut xtile = (((coord.lon as f64 / 1000000.0).clamp(-LON_MAX, LON_MAX) + 180.0) / 360.0 * n) as u32;
	let mut ytile = ((1.0 - lat_rad.tan().asinh() / PI) / 2.0 * n) as u32;
	if bias_low {
		let origin = tile_origin(level, xtile, ytile);
		if origin.lat == coord.lat && ytile > 0 { ytile -= 1; }
		if origin.lon == coord.lon && xtile > 0 { xtile -= 1; }
	}
	let maxtile = 2_u32.pow(level as u32) - 1;
	(xtile.min(maxtile), ytile.min(maxtile))
}

fn coord2tile(level: u8, coord: LatLon) -> (u32, u32) {
	biased_coord2tile(level, coord, false) // Not biasing low is more efficient when it doesn't matter
}

fn tileidx(level: u8, idx: u32) -> (u32, u32) {
	let n = (2 as u32).pow(level as u32);
	(idx % n, idx / n)
}

fn num_tiles(level: u8, bounds: &BoundingBox) -> (u32, u32) {
	let (min_coord, max_coord) = bounds.minmax();
	let min = biased_coord2tile(level, min_coord, false);
	let max = biased_coord2tile(level, max_coord, true);
	(max.0 - min.0 + 1, max.1 - min.1 + 1)
}

// Given the absolute indices of a tile in the given zoom level, figure out the number that
// tile would get if all tiles covered by the given bounding box were counted off from zero in
// reading order
fn tile_idx_in_box(level: u8, bounds: &BoundingBox, xtile: u32, ytile: u32) -> Option<u32> {
	let (min_coord, max_coord) = bounds.minmax();
	let min = biased_coord2tile(level, min_coord, false);
	let max = biased_coord2tile(level, max_coord, true);
	if xtile < min.0 || xtile > max.0 || ytile < min.1 || ytile > max.1 { None }
	else {
		let rowlen = max.0 - min.0 + 1;
		Some((ytile - min.1) * rowlen + (xtile - min.0))
	}
}

// For a given tile, translate a list of lat/lon offsets from the tile origin to offsets that treat
// the tile as a unit square
fn translate_offsets(level: u8, xtile: u32, ytile: u32, offsets: &[LatLon]) -> Vec<(f64, f64)> {
	// TODO Do actual trig rather than stretching latitude
	let tile_width = 2.0 * 360e6 / ((2 << (level as u32)) as f64); // TODO Cache this value
	let tile_height = (tile_origin(level, xtile, ytile).lat - tile_origin(level, xtile, ytile + 1).lat) as f64;
	let x_factor = 1.0 / tile_width;
	let y_factor = -1.0 / tile_height;
	let mut ret = vec![];
	for offset in offsets {
		let x = offset.lon as f64 * x_factor;
		let y = offset.lat as f64 * y_factor;
		ret.push((x, y));
	}
	ret
}

#[derive(Debug)]
pub struct TileIndex {
	tile_offsets: Vec<u64>,
}

#[derive(Debug)]
pub struct Poi {
	offset: LatLon,
	layer: i8,
	tags: HashMap<String, TagValue>,
	name: Option<String>,
	house_number: Option<String>,
	elevation: Option<i64>,
}

#[derive(Debug)]
pub struct Way {
	size: u64,
	subtile_map: u16,
	pub layer: i8,
	pub tags: HashMap<String, TagValue>,
	pub name: Option<String>,
	pub house_number: Option<String>,
	reference: Option<String>,
	pub label_pos: Option<LatLon>,
	blocks: Vec<Vec<Vec<LatLon>>>,
}

impl Way {
	pub fn project(&self, tile: &Tile) -> Vec<Vec<Vec<(f64, f64)>>> {
		let mut ret = vec![];
		for block in self.blocks.as_slice() {
			let mut blockdata = vec![];
			for path in block.as_slice() {
				blockdata.push(tile.project(&path));
			}
			ret.push(blockdata);
		}
		ret
	}
}

#[derive(Debug)]
pub struct TileHeader {
	zoom_table: Vec<(u64, u64)>,
	poi_start: u64,
	way_start: u64,
}

#[derive(Debug)]
pub struct Tile {
	pub zoom: u8,
	pub index: (u32, u32),
	pub ways: Vec<Way>,
	pub pois: Vec<Poi>,
}

impl Tile {
	fn empty(zoom: u8, xtile: u32, ytile: u32) -> Self {
		Self { zoom, index: (xtile, ytile), ways: vec![], pois: vec![] }
	}

	fn project(&self, offsets: &[LatLon]) -> Vec<(f64, f64)> {
		translate_offsets(self.zoom, self.index.0, self.index.1, offsets)
	}
}

#[derive(Debug)]
pub struct MapHeader {
	version: u32,
	size: u64,
	created: u64,
	bounds: BoundingBox,
	pub tile_size: u16,
	projection: String,
	debug: bool,
	start_pos: Option<LatLon>,
	start_zoom: Option<u8>,
	pref_lang: Option<String>,
	comment: Option<String>,
	creator: Option<String>,
	poi_tags: Vec<(String, TagDesc)>,
	way_tags: Vec<(String, TagDesc)>,
	zoom_intervals: Vec<ZoomInterval>,
}

pub struct MapFile {
	path: PathBuf,
	data: Arc<Mmap>,
	header: MapHeader,
	zoom_interval_map: HashMap<u8, u8>,
	indices: Vec<TileIndex>,
}

impl MapFile {
	pub fn new(path: PathBuf) -> Self {
		let data = unsafe { Mmap::map(&File::open(&path).unwrap()).unwrap() };
		let header = parse::header(&*data).unwrap().1;
		let mut zoom_map = HashMap::new();
		for (idx, zoom) in header.zoom_intervals.iter().enumerate() {
			for level in zoom.min..=zoom.max {
				zoom_map.insert(level, idx as u8);
			}
		}
		let indices = header.zoom_intervals.iter().map(|subfile| {
			let n = num_tiles(subfile.base, &header.bounds);
			let i = &data[subfile.start as usize ..];
			parse::tile_index((n.0 * n.1) as usize, header.debug, subfile.start, i).unwrap().1
		}).collect();
		Self { path, data: Arc::new(data), header: header, zoom_interval_map: zoom_map, indices }
	}

	pub fn header<'a>(&'a self) -> &'a MapHeader {
		&self.header
	}

	pub fn bounds(&self, zoom: u8) -> ((f64, f64), (f64, f64)) {
		let (free_min, free_max) = self.header.bounds.minmax();
		let (min, max) = (free_min.constrain(), free_max.constrain());
		let (min_xtile, min_ytile) = coord2tile(zoom, min);
		let minorig = tile_origin(zoom, min_xtile, min_ytile);
		let minoff = LatLon { lat: min.lat - minorig.lat, lon: min.lon - minorig.lon };
		let translated_min = translate_offsets(zoom, min_xtile, min_ytile, &vec![minoff])[0];
		let (max_xtile, max_ytile) = coord2tile(zoom, max);
		let maxorig = tile_origin(zoom, max_xtile, max_ytile);
		let maxoff = LatLon { lat: max.lat - maxorig.lat, lon: max.lon - maxorig.lon };
		let translated_max = translate_offsets(zoom, max_xtile, max_ytile, &vec![maxoff])[0];
		(
			(min_xtile as f64 + translated_min.0, min_ytile as f64 + translated_min.1),
			(max_xtile as f64 + translated_max.0, max_ytile as f64 + translated_max.1)
		)
	}

	pub fn desired_zoom_level(&self, cur_zoom: u8, tile_size: f32) -> u8 {
		let zoom_adj = (cur_zoom as f32 + (tile_size / self.header.tile_size as f32).log2().round()).clamp(0.0, 22.0) as u8;
		if let Some(base_zoom) = self.zoom_interval_map.get(&zoom_adj) {
			self.header.zoom_intervals[*base_zoom as usize].base
		}
		else { cur_zoom }
	}

	pub fn tile(&self, zoom: u8, x: u32, y: u32) -> Tile {
		let subfile_num = self.zoom_interval_map.get(&zoom).unwrap().clone();
		let zoom_interval = &self.header.zoom_intervals[subfile_num as usize];
		if zoom_interval.base != zoom { unimplemented!("Cannot retrieve tiles for non-base zoom levels"); } // TODO
		match tile_idx_in_box(zoom, &self.header.bounds, x, y) {
			None => Tile::empty(zoom, x, y),
			Some(tile_idx) => {
				let tile_offset = self.indices.get(subfile_num as usize).unwrap().tile_offsets[tile_idx as usize];
				if tile_offset & 0x8000000000 != 0 { Tile::empty(zoom, x, y) }
				else {
					let i = &self.data[tile_offset as usize ..];
					let (mut i, tile_header) = parse::tile_header(self.header.debug, zoom_interval.max - zoom_interval.min + 1, tile_offset, i).unwrap();
					let num_poi = tile_header.zoom_table.iter().map(|x| x.0).sum();
					let num_way: u64 = tile_header.zoom_table.iter().map(|x| x.1).sum();
					//let tile_origin = tile_origin(zoom_interval.base, x, y);
					let mut pois = vec![];
					for _ in  0 .. num_poi {
						let (newi, poi) = parse::poi(self.header.debug, &self.header.poi_tags, i).unwrap();
						i = newi;
						pois.push(poi);
					}
					let mut ways = vec![];
					for _ in  0 .. num_way {
						let (newi, way) = parse::way(self.header.debug, &self.header.way_tags, i).unwrap();
						i = newi;
						ways.push(way);
					}
					Tile { zoom, index: (x, y), ways, pois }
				}
			}
		}
	}

	pub fn test(&self) {
		let zoom = &self.header.zoom_intervals[1];
		
		println!("{:#?}", self.header);
		println!("Tiles: {:?}", num_tiles(zoom.base, &self.header.bounds));
		let tile = self.indices[0].tile_offsets.iter().enumerate().find(|(_, x)| *x & 0x8000000000 == 0).unwrap().0;
		println!("Examining tile {}", tile);
		let tile_start = &self.indices[0].tile_offsets[tile] & 0x7fffffffff;
		let (i, tile_header) = parse::tile_header(self.header.debug, zoom.max - zoom.min + 1, tile_start, &self.data[tile_start as usize ..]).unwrap();
		println!("{:#?}", tile_header);
		let tile_idx = tileidx(zoom.base, tile as u32);
		
		/*println!("Way 1 at offset {}", i.as_ptr() as usize - self.data.as_ptr() as usize);
		let (i, way1) = parse::way(self.header.debug, &self.header.way_tags, i).unwrap();
		println!("Way 1: {:#?}", way1);
		println!("Way 2 at offset {}", i.as_ptr() as usize - self.data.as_ptr() as usize);
		let (_, way2) = parse::way(self.header.debug, &self.header.way_tags, i).unwrap();
		println!("Way 2: {:#?}", way2);*/

		/*let way = parse::way(self.header.debug, zoom.tile_origin(tile_idx.0, tile_idx.1), &self.header.way_tags, &self.data[tile_header.way_start as usize .. ]).unwrap().1;
		let ret =  vec![way.blocks[0][0].iter().map(|x| x.to_point()).collect()];
		println!("{:?}", ret);
		return ret;*/
	}
}

#[test]
fn test_coord2tile() {
	let tests = vec![
		(0, (90, -180), false, (0, 0)),
		(0, (90, -180), true, (0, 0)),
		(0, (-90, 180), false, (0, 0)),
		(0, (-90, 180), true, (0, 0)),
		(1, (90, -180), false, (0, 0)),
		(1, (0, 0), false, (1, 1)),
		(1, (0, 0), true, (0, 0)),
		(1, (1, 0), false, (1, 0)),
		(1, (1, 0), true, (0, 0)),
		(1, (0, -1), false, (0, 1)),
		(1, (0, -1), true, (0, 0)),
		(1, (0, 1), false, (1, 1)),
		(1, (0, 1), true, (1, 0)),
		(1, (-1, 0), false, (1, 1)),
		(1, (-1, 0), true, (0, 1)),
		(1, (-90, 180), false, (1, 1)),
		(1, (-90, 180), true, (1, 1)),
		(2, (80, -100), false, (0, 0)),
		(2, (80, -100), true, (0, 0)),
		(2, (45, -90), false, (1, 1)),
		(2, (10, -10), false, (1, 1)),
	];
	for (zoom, latlon, bias_low, expected) in tests {
		let actual = biased_coord2tile(zoom, LatLon::new(latlon.0 * 1000000, latlon.1 * 1000000), bias_low);
		assert_eq!(actual, expected, "Lat/lon {:?} at zoom {} with bias_low {} is tile {:?} but expected tile {:?}", latlon, zoom, bias_low, actual, expected);
	}
}

#[test]
fn test_tile_idx_in_box() {
	let tests = vec![
		(1, (-90, -180, 90, 180), (1, 1), Some(3)),
		(2, (-50, -90, 50, 90), (1, 1), Some(0)),
		(2, (-50, -90, 50, 90), (1, 2), Some(2)),
		(2, (-50, -90, 50, 90), (2, 2), Some(3)),
		(2, (-50, -90, 50, 90), (0, 0), None),
		(2, (-50, -90, 50, 90), (2, 3), None),
		(2, (-50, -100, 80, 90), (0, 0), Some(0)),
		(2, (-50, -100, 80, 90), (1, 0), Some(1)),
		(2, (-50, -100, 80, 90), (0, 1), Some(3)),
		(2, (-50, -100, 80, 90), (1, 1), Some(4)),
		(2, (-50, -100, 80, 90), (2, 2), Some(8)),
		(2, (-50, -100, 80, 90), (0, 3), None),
		(2, (-50, -100, 80, 90), (3, 1), None),
	];
	for (level, bounds, tile, expected) in tests {
		let bounding_box = BoundingBox { lat_min: bounds.0 * 1000000, lon_min: bounds.1 * 1000000, lat_max: bounds.2 * 1000000, lon_max: bounds.3 * 1000000 };
		let actual = tile_idx_in_box(level, &bounding_box, tile.0, tile.1);
		assert_eq!(actual, expected, "Index of tile {:?} in bounds {:?} at zoom {} is {:?}, but expected {:?}", tile, bounds, level, actual, expected);
	}
}
