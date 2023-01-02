use std::collections::{HashMap, HashSet};

use skulpin::skia_safe::{Color4f, Paint, paint};

use super::mapsforge::{Poi, TagValue, Way};

#[derive(Clone)]
pub struct Material {
	fill: Option<Color4f>,
	stroke: Option<Color4f>,
}

impl Material {
	fn build_paint(color: Color4f, style: paint::Style) -> Paint {
		let mut paint = Paint::new(color, None);
		paint.set_anti_alias(true);
		paint.set_style(style);
		paint.set_stroke_width(1.0);
		paint
	}

	pub fn paints(&self) -> Vec<Paint> {
		let mut ret = vec![];
		if let Some(fill) = self.fill { ret.push(Self::build_paint(fill, paint::Style::Fill)); }
		if let Some(stroke) = self.stroke { ret.push(Self::build_paint(stroke, paint::Style::Stroke)); }
		ret
	}
}

#[derive(PartialEq)]
enum EntityType {
	Any,
	Path, // Open way
	Area, // Closed way
	Point,
}

enum TagMatch {
	Present,
	Literal(HashSet<String>),
	Regex(String),
}

impl TagMatch {
	fn from_values(values: &[&str]) -> Self {
		Self::Literal(values.iter().map(|x| x.to_string()).collect())
	}
}

struct Matcher {
	entity_type: EntityType,
	tags: HashMap<String, TagMatch>,
	material: String,
}

pub struct Theme {
	materials: HashMap<String, Material>,
	matchers: Vec<Matcher>,
}

impl Theme {
	pub fn match_way(&self, way: &Way) -> Option<Material> {
		for matcher in &self.matchers {
			if matcher.entity_type == EntityType::Point { continue; }
			let area = way.tags.get("area").cloned() == Some(TagValue::Literal("yes".to_string()));
			if (matcher.entity_type == EntityType::Area && !area) || (matcher.entity_type == EntityType::Path && area) { continue; }
			for (tag, tagmatch) in &matcher.tags {
				if let Some(tag_value) = way.tags.get(tag) {
					match tagmatch {
						TagMatch::Present => return self.materials.get(&matcher.material).cloned(),
						TagMatch::Literal(values) => {
							if let TagValue::Literal(literal_value) = tag_value {
								if values.contains(literal_value) {
									return self.materials.get(&matcher.material).cloned();
								}
							}
						}
						TagMatch::Regex(regex) => unimplemented!(),
					}
				}
			}
		}
		None
	}
	
	pub fn match_poi(&self, poi: &Poi) -> Option<Material> {
		None // TODO
	}
}

pub fn outline() -> Theme {
	let materials = vec![
		("outline".to_string(), Material { fill: None, stroke: Some(Color4f::new(1.0, 1.0, 1.0, 1.0)) }),
	].into_iter().collect::<HashMap<_, _>>();
	let matchers = vec![Matcher { entity_type: EntityType::Any, tags: HashMap::new(), material: "outline".to_string() }];
	Theme { materials, matchers }
}

pub fn basic() -> Theme {
	let opacity = 0.8;
	let materials = vec![
		("water_path".to_string(), Material { stroke: Some(Color4f::new(0.2, 0.2, 1.0, opacity)), fill: None }),
		("water_area".to_string(), Material { stroke: None, fill: Some(Color4f::new(0.5, 0.5, 1.0, opacity)) }),
		("land".to_string(), Material { stroke: None, fill: Some(Color4f::new(0.8, 0.8, 0.8, opacity)) }),
		("road".to_string(), Material { stroke: Some(Color4f::new(0.2, 0.2, 0.2, opacity)), fill: None }),
		("building".to_string(), Material { stroke: None, fill: Some(Color4f::new(0.6, 0.6, 0.6, opacity)) }),
		("bsrrier".to_string(), Material { stroke: Some(Color4f::new(0.4, 0.2, 0.2, opacity)), fill: None }),
		("greenspace".to_string(), Material { stroke: None, fill: Some(Color4f::new(0.8, 1.0, 0.8, opacity)) }),
		("rail".to_string(), Material { stroke: Some(Color4f::new(0.2, 0.2, 0.8, opacity)), fill: None }),
	].into_iter().collect();
	let matchers = vec![
		Matcher {
			entity_type: EntityType::Area,
			tags: vec![
				("natural".to_string(), TagMatch::from_values(&["sea", "water"])),
				("waterway".to_string(), TagMatch::Present),
			].into_iter().collect(),
			material: "water_area".to_string(),
		},
		Matcher {
			entity_type: EntityType::Area,
			tags: vec![
				("natural".to_string(), TagMatch::from_values(&["nosea"])),
			].into_iter().collect(),
			material: "land".to_string(),
		},
		Matcher {
			entity_type: EntityType::Path,
			tags: vec![
				("natural".to_string(), TagMatch::from_values(&["sea", "water"])),
				("waterway".to_string(), TagMatch::Present),
			].into_iter().collect(),
			material: "water_path".to_string(),
		},
		Matcher {
			entity_type: EntityType::Path,
			tags: vec![
				("highway".to_string(), TagMatch::Present),
				("bridge".to_string(), TagMatch::Present),
				("aeroway".to_string(), TagMatch::from_values(&["apron", "runway", "taxiway"])),
			].into_iter().collect(),
			material: "road".to_string(),
		},
		Matcher {
			entity_type: EntityType::Path,
			tags: vec![
				("barrier".to_string(), TagMatch::Present),
			].into_iter().collect(),
			material: "barrier".to_string(),
		},
		Matcher {
			entity_type: EntityType::Path,
			tags: vec![
				("building".to_string(), TagMatch::Present),
			].into_iter().collect(),
			material: "building".to_string(),
		},
		Matcher {
			entity_type: EntityType::Area,
			tags: vec![
				("landuse".to_string(), TagMatch::from_values(&["brownfield", "cemetery", "farm", "farmland", "farmyard", "forest", "grass", "meadow", "orchard", "recreation_ground", "village_green", "vineyard", "wood"])),
				("leisure".to_string(), TagMatch::from_values(&["dog_park", "garden", "nature_reserve", "park", "pitch", "playground"])),
				("natural".to_string(), TagMatch::from_values(&["grassland", "heath", "land", "marsh", "scrub", "wetland"])),
			].into_iter().collect(),
			material: "greenspace".to_string(),
		},
		Matcher {
			entity_type: EntityType::Path,
			tags: vec![
				("railway".to_string(), TagMatch::from_values(&["rail"])),
			].into_iter().collect(),
			material: "rail".to_string(),
		},
	];
	Theme { materials, matchers }
}
