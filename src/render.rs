use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use super::mapsforge;
use super::mapsforge::{Coord, TagValue};
use super::UpdateEvent;

#[derive(Debug, Clone, Copy)]
pub struct BoundingBox {
	empty: bool,
	min: Coord,
	max: Coord,
}

impl BoundingBox {
	pub fn empty() -> Self {
		Self { empty: true, min: Coord { x: 0, y: 0 }, max: Coord { x: 0, y: 0 } }
	}

	pub fn from_corners(corners: (Coord, Coord)) -> Self {
		Self {
			empty: false,
			min: Coord { x: corners.0.x.min(corners.1.x), y: corners.0.y.min(corners.1.y) },
			max: Coord { x: corners.0.x.max(corners.1.x), y: corners.0.y.max(corners.1.y) },
		}
	}

	pub fn corners(&self) -> Option<(Coord, Coord)> {
		if self.empty { None }
		else { Some((self.min, self.max)) }
	}

	pub fn include(&mut self, point: Coord) {
		if self.empty {
			self.empty = false;
			self.min = point;
			self.max = point;
		}
		else {
			if point.x < self.min.x { self.min.x = point.x; }
			if point.y < self.min.y { self.min.y = point.y; }
			if point.x > self.max.x { self.max.x = point.x; }
			if point.y > self.max.y { self.max.y = point.y; }
		}
	}

	pub fn union(&self, other: &Self) -> Self {
		let mut ret = self.clone();
		if !other.empty {
			ret.include(other.min);
			ret.include(other.max);
		}
		ret
	}

	pub fn intersection(&self, other: &Self) -> Self {
		if self.empty || other.empty { Self::empty() }
		else {
			let xmin = self.min.x.max(other.min.x);
			let xmax = self.max.x.min(other.max.x);
			let ymin = self.min.y.max(other.min.y);
			let ymax = self.max.y.min(other.max.y);
			if xmin > xmax || ymin > ymax { Self::empty() }
			else {
				Self { empty: false, min: Coord { x: xmin, y: ymin }, max: Coord { x: xmax, y: ymax } }
			}
		}
	}

	pub fn width(&self) -> i64 {
		if self.empty { 0 }
		else { self.max.x - self.min.x }
	}

	pub fn height(&self) -> i64 {
		if self.empty { 0 }
		else { self.max.y - self.min.y }
	}

	pub fn midpoint(&self) -> Option<Coord> {
		if self.empty { None }
		else { Some(Coord { x: (self.min.x + self.max.x) / 2, y: (self.min.y + self.max.y) / 2 }) }
	}

	pub fn max_dimension(&self) -> i64 {
		self.width().max(self.height())
	}

	pub fn is_empty(&self) -> bool {
		self.max_dimension() == 0
	}
}

#[derive(Clone, Copy, Hash, PartialEq, Eq)]
pub enum Material {
	Unknown,
	Land,
	Water,
	Road,
	Building,
	Barrier,
	Greenspace,
}

impl Material {
	fn from_tags(tags: &HashMap<String, TagValue>) -> Option<Self> {
		if let Some(TagValue::Literal(natural)) = tags.get("natural") {
			match natural.as_ref() {
				"sea" | "water" => Some(Self::Water),
				"nosea" => Some(Self::Land),
				"grassland" | "heath" | "land" | "marsh" | "scrub" | "wetland" => Some(Self::Greenspace),
				"" => None,
				_ => None,
			}
		}
		else if let Some(TagValue::Literal(leisure)) = tags.get("leisure") {
			match leisure.as_ref() {
				"dog_park" | "garden" | "nature_reserve" | "park" | "pitch" | "playground" => Some(Self::Greenspace),
				"" => None,
				_ => None,
			}
		}
		else if let Some(TagValue::Literal(landuse)) = tags.get("landuse") {
			match landuse.as_ref() {
				"brownfield" | "cemetery" | "farm" | "farmland" | "farmyard" | "forest" | "grass" | "meadow" | "orchard" | "recreation_ground" | "village_green" | "vineyard" | "wood" => Some(Self::Greenspace),
				"" => None,
				_ => None,
			}
		}
		else if tags.contains_key("building") { Some(Self::Building) }
		else if tags.contains_key("highway") { Some(Self::Road) }
		else if tags.contains_key("barrier") { Some(Self::Barrier) }
		else { None }
	}
}

pub enum Geometry {
	Path(Vec<Vec<Coord>>),
	Point(Coord),
}

pub struct Object {
	pub geo: Geometry,
	pub name: Option<String>,
	pub material: Material,
}

pub struct RenderTile {
	pub zoom: u8,
	pub x: i64,
	pub y: i64,
	pub layers: BTreeMap<i8, Vec<Object>>,
}

impl RenderTile {
	fn new(tile: mapsforge::Tile, zoom: u8, x: i64, y: i64) -> Self {
		let mut layers = BTreeMap::new();
		for way in &tile.ways {
			if let Some(material) = Material::from_tags(&way.tags) {
				for block in way.project(&tile) {
					let geo = Geometry::Path(block);
					layers.entry(way.layer).or_insert(vec![]).push(Object { geo, name: way.name.clone(), material });
				}
			}
		}
		for poi in &tile.pois {
			if let Some(material) = Material::from_tags(&poi.tags) {
				let geo = Geometry::Point(poi.project(&tile));
				layers.entry(poi.layer).or_insert(vec![]).push(Object { geo, name: poi.name.clone(), material });
			}
		}
		Self { zoom, x, y, layers }
	}

	fn empty(zoom: u8, x: i64, y: i64) -> Self {
		Self { zoom, x, y, layers: BTreeMap::new() }
	}

	pub fn bounds(&self) -> BoundingBox {
		let tile_size = mapsforge::COORD_MAX >> self.zoom;
		BoundingBox::from_corners((
			Coord { x: tile_size * self.x, y: tile_size * self.y },
			Coord { x: tile_size * (self.x + 1), y: tile_size * (self.y + 1) },
		))
	}
}

fn visible_tiles(viewport: &BoundingBox, zoom: u8) -> ((i64, i64), (i64, i64)) {
	let tileidx = |coord: i64| coord.div_floor(mapsforge::COORD_MAX >> zoom);
	let (min, max) = viewport.corners().unwrap();
	((tileidx(min.x), tileidx(max.x)), (tileidx(min.y), tileidx(max.y)))
}

pub struct RenderManager {
	pub maps: Vec<Arc<mapsforge::MapFile>>,
	tiles: HashMap<(PathBuf, u8), Arc<Mutex<HashMap<(u32, u32), Arc<RenderTile>>>>>,
	cur_generation: Arc<AtomicU64>,
	render_threads: rayon::ThreadPool,
}

impl RenderManager {
	pub fn new(maps: Vec<Arc<mapsforge::MapFile>>) -> Self {
		Self { maps, tiles: HashMap::new(), cur_generation: Arc::new(AtomicU64::new(0)), render_threads: rayon::ThreadPoolBuilder::new().build().unwrap() }
	}

	pub fn bounds(&self) -> BoundingBox {
		self.maps.iter()
			.map(|map| BoundingBox::from_corners(map.bounds()))
			.fold(BoundingBox::empty(), |accum, cur| accum.union(&cur))
	}

	pub fn async_viewport_tiles(&mut self, viewport: &BoundingBox, winwidth: u32, generation: u64, updater: super::Updater) {
		self.cur_generation.store(generation, Ordering::Relaxed);
		let deg_lon_per_px = viewport.width() as f64 * 360.0 / (winwidth as f64 * mapsforge::COORD_MAX as f64);
		for map in &self.maps {
			if BoundingBox::from_corners(map.bounds()).intersection(viewport).is_empty() { continue; }
			let maybe_zoom = map.desired_zoom_level(deg_lon_per_px);
			if let Some(zoom) = maybe_zoom {
				let (xrange, yrange) = visible_tiles(&viewport, zoom);
				let zoom_cache = self.tiles.entry((map.path().to_path_buf(), zoom)).or_insert(Arc::new(Mutex::new(HashMap::new())));
				let ntile = 1 << zoom;
				for y in yrange.0..=yrange.1 {
					for x in xrange.0..=xrange.1 {
						if y <= 0 || x <= 0 || y > ntile || x > ntile {
							updater.send(UpdateEvent::Tile { generation, tile: Arc::new(RenderTile::empty(zoom, x, y)) });
						}
						else {
							let (x, y) = (x as u32, y as u32);
							let thread_updater = updater.clone();
							let thread_map = map.clone();
							let thread_cache = zoom_cache.clone();
							let thread_generation = self.cur_generation.clone();
							self.render_threads.spawn(move || {
								if generation < thread_generation.load(Ordering::Relaxed) { return; }
								let cached_tile = thread_cache.lock().expect("Poisoned lock").get(&(x, y)).cloned();
								let tile = if let Some(existing_tile) = cached_tile {
									existing_tile.clone()
								}
								else {
									let new_tile = Arc::new(RenderTile::new(thread_map.tile(zoom, x, y), zoom, x as i64, y as i64));
									thread_cache.lock().expect("Poisoned lock").insert((x, y), new_tile.clone());
									new_tile
								};
								thread_updater.send(UpdateEvent::Tile { generation, tile });
							});
						}
					}
				}
			}
		}
	}
}
