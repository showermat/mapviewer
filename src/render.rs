use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use super::mapsforge;
use super::mapsforge::TagValue;

#[derive(Hash, PartialEq, Eq)]
pub enum Material {
	Unknown,
	Land,
	Water,
}

impl Material {
	fn from_tags(tags: &HashMap<String, TagValue>) -> Self {
		match tags.get("natural") {
			Some(TagValue::Literal(natural)) => match natural.as_ref() {
				//"sea" => Self::Water,
				//"nosea" => Self::Land,
				"" => Self::Unknown,
				_ => Self::Unknown,
			},
			_ => Self::Unknown,
		}
	}
}

pub enum Geometry {
	Path(Vec<Vec<(f64, f64)>>),
	Point((f64, f64)),
}

pub struct Object {
	pub geo: Geometry,
	pub name: String,
	pub material: Material,
}

pub struct RenderTile {
	pub layers: BTreeMap<i8, Vec<Object>>,
}

impl RenderTile {
	fn new(tile: mapsforge::Tile) -> Self {
		let mut layers = BTreeMap::new();
		for way in &tile.ways {
			for block in way.project(&tile) {
				let geo = Geometry::Path(block);
				layers.entry(way.layer).or_insert(vec![]).push(Object { geo, name: "".to_string(), material: Material::from_tags(&way.tags) });
			}
		}
		Self { layers }
	}

	fn empty() -> Self {
		Self { layers: BTreeMap::new() }
	}
}

pub struct RenderCache {
	pub map: mapsforge::MapFile,
	tiles: HashMap<u8, HashMap<(u32, u32), Arc<RenderTile>>>,
}

impl RenderCache {
	pub fn new(map: mapsforge::MapFile) -> Self {
		Self { map, tiles: HashMap::new() }
	}

	pub fn tile(&mut self, zoom: u8, x: u32, y: u32) -> Arc<RenderTile> {
		let ntiles = 1 << (zoom as u32);
		if x >= ntiles || y >= ntiles { return Arc::new(RenderTile::empty()) }
		let zoom_cache = self.tiles.entry(zoom).or_insert(HashMap::new());
		if !(*zoom_cache).contains_key(&(x, y)) {
			let tile = Arc::new(RenderTile::new(self.map.tile(zoom, x, y)));
			self.tiles.get_mut(&zoom).expect("Just inserted key but now missing").insert((x, y), tile);
		}
		self.tiles[&zoom][&(x, y)].clone()
	}
}
