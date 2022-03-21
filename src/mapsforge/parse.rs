use std::collections::HashMap;

use nom::bytes::complete::*;
use nom::combinator::*;
use nom::multi::*;
use nom::number::complete::{be_f32, be_i8, be_i16, be_i32, be_u8, be_u16, be_u32, be_u64};
use nom::sequence::*;
use nom::IResult;

use super::{BoundingBox, LatLon, MapHeader, Poi, TagDesc, TagValue, TileHeader, TileIndex, Tile, Way, ZoomInterval};

fn vbe_u(i: &[u8]) -> IResult<&[u8], u64> {
	let (i, (rest, first)) = pair(take_while(|c| c & 0x80 != 0), be_u8)(i)?;
	let mut ret = first as u64;
	for c in rest.into_iter().rev() { ret = (ret << 7) | (c & 0x7f) as u64; }
	Ok((i, ret))
}

fn vbe_s(i: &[u8]) -> IResult<&[u8], i64> {
	let (i, (rest, first)) = pair(take_while(|c| c & 0x80 != 0), be_u8)(i)?;
	let mut ret = (first & 0x3f) as u64;
	for c in rest.into_iter().rev() { ret = (ret << 7) | (c & 0x7f) as u64; }
	let mul = if first & 0x40 != 0 { -1 } else { 1 };
	Ok((i, mul * (ret as i64)))
}

fn latlon(i: &[u8]) -> IResult<&[u8], LatLon> {
	let (i, values) = tuple((vbe_s, vbe_s))(i)?;
	Ok((i, LatLon::new(values.0 as i32, values.1 as i32)))
}

fn string(i: &[u8]) -> IResult<&[u8], String> {
	let (i, len) = vbe_u(i)?;
	let (i, ret) = take(len as usize)(i)?;
	Ok((i, String::from_utf8(ret.to_vec()).unwrap()))
}

fn zoom_interval(i: &[u8]) -> IResult<&[u8], ZoomInterval> {
	let (i, f) = tuple((be_u8, be_u8, be_u8, be_u64, be_u64))(i)?;
	let ret = ZoomInterval { base: f.0, min: f.1, max: f.2, start: f.3, len: f.4 };
	Ok((i, ret))
}

pub fn header(i: &[u8]) -> IResult<&[u8], MapHeader> {
	//println!("File base is {:?}", i.as_ptr());
	let (i, begin) = preceded(
		tag(b"mapsforge binary OSM"),
		tuple((
			be_u32, // Header size
			be_u32, // Version
			be_u64, // File size
			be_u64, // Creation date
			be_i32, be_i32, be_i32, be_i32, // Bounding box
			be_u16, // Tile size
			string, // projection
			be_u8 // Flags
		))
	)(i)?;
	let flags = begin.10;
	let (i, startpos) = cond(flags & 0x40 != 0, tuple((be_i32, be_i32)))(i)?;
	let (i, startzoom) = cond(flags & 0x20 != 0, be_u8)(i)?;
	let (i, lang) = cond(flags & 0x10 != 0, string)(i)?;
	let (i, comment) = cond(flags & 0x08 != 0, string)(i)?;
	let (i, creator) = cond(flags & 0x04 != 0, string)(i)?;
	let (i, npoitags) = be_u16(i)?;
	let (i, poitags) = count(string, npoitags as usize)(i)?;
	let (i, nwaytags) = be_u16(i)?;
	let (i, waytags) = count(string, nwaytags as usize)(i)?;
	let (i, nzoom) = be_u8(i)?;
	let (i, zooms) = count(zoom_interval, nzoom as usize)(i)?;
	let ret = MapHeader {
		version: begin.1,
		size: begin.2,
		created: begin.3,
		bounds: BoundingBox { lat_min: begin.4, lon_min: begin.5, lat_max: begin.6, lon_max: begin.7 },
		tile_size: begin.8,
		projection: begin.9,
		debug: flags & 0x80 != 0,
		start_pos: startpos.map(|x| LatLon::new(x.0, x.1)),
		start_zoom: startzoom,
		pref_lang: lang,
		comment: comment,
		creator: creator,
		poi_tags: poitags.into_iter().map(|s| TagDesc::parse(s)).collect(),
		way_tags: waytags.into_iter().map(|s| TagDesc::parse(s)).collect(),
		zoom_intervals: zooms,
	};
	Ok((i, ret))
}

pub fn tile_index(num: usize, debug: bool, base: u64, i: &[u8]) -> IResult<&[u8], TileIndex> {
	let (i, _) = cond(debug, take(16 as usize))(i)?;
	let (i, offsets) = count(take(5 as usize), num)(i)?;
	Ok((i, TileIndex { tile_offsets: offsets.into_iter().map(|x| {
		((x[0] as u64) << 32 | (x[1] as u64) << 24 | (x[2] as u64) << 16 | (x[3] as u64) << 8 | x[4] as u64) + base
	}).collect() }))
}

pub fn tile_header(debug: bool, nzoom: u8, base: u64, i: &[u8]) -> IResult<&[u8], TileHeader> {
	let start = i.as_ptr() as usize;
	let (i, _) = cond(debug, take(32 as usize))(i)?;
	let (i, table) = count(tuple((vbe_u, vbe_u)), nzoom as usize)(i)?;
	let (i, poisize) = vbe_u(i)?;
	let hdrsize = (i.as_ptr() as usize - start) as u64;
	Ok((i, TileHeader { zoom_table: table, poi_start: base + hdrsize, way_start: base + hdrsize + poisize }))
}

fn tag_value<'a, 'b>(desc: &TagDesc, i: &'b [u8]) -> IResult<&'b [u8], TagValue> {
	Ok(match desc {
		TagDesc::Literal(s) => (i, TagValue::Literal(s.to_string())),
		TagDesc::Byte => { let res = be_i8(i)?; (res.0, TagValue::Byte(res.1)) },
		TagDesc::Short => { let res = be_i16(i)?; (res.0, TagValue::Short(res.1)) },
		TagDesc::Int => { let res = be_i32(i)?; (res.0, TagValue::Int(res.1)) },
		TagDesc::Float => { let res = be_f32(i)?; (res.0, TagValue::Float(res.1)) },
		TagDesc::String => { let res = string(i)?; (res.0, TagValue::String(res.1)) },
	})
}

fn tagmap<'a, 'b>(ntags: u8, tags: &'a [(String, TagDesc)], i: &'b [u8]) -> IResult<&'b [u8], HashMap<String, TagValue>> {
	let (i, tag_ids) = count (|i| vbe_u(i), ntags as usize)(i)?;
	let tag_descs = tag_ids.into_iter().map(|id| tags[id as usize].clone()).collect::<Vec<(String, TagDesc)>>();
	let mut newi = i;
	let mut tag_values = vec![];
	for desc in &tag_descs {
		let (curi, tagval) = tag_value(&desc.1, newi)?;
		tag_values.push(tagval);
		newi = curi;
	}
	let i = newi;
	Ok((i, tag_descs.into_iter().map(|x| x.0).zip(tag_values).collect()))
}

pub fn poi<'a, 'b>(debug: bool, tags: &'a [(String, TagDesc)], i: &'b [u8]) -> IResult<&'b [u8], Poi> {
	let (i, head) = tuple((
		cond(debug, take(32 as usize)),
		latlon,
		be_u8,
	))(i)?;
	let layer = (head.2 >> 4) as i8 - 5;
	let ntags = head.2 & 0x0f;
	let (i, tags) = tagmap(ntags, tags, i)?;
	let (i, flags) = be_u8(i)?;
	let (i, optfields) = tuple((
		cond(flags & 0x80 != 0, string), // Name
		cond(flags & 0x40 != 0, string), // House number
		cond(flags & 0x20 != 0, vbe_s), // Elevation
	))(i)?;
	Ok((i, Poi {
		offset: head.1,
		layer,
		tags,
		name: optfields.0,
		house_number: optfields.1,
		elevation: optfields.2,
	}))
}

fn decode_single_delta(points: &[LatLon]) -> Vec<LatLon> {
	let mut cur = LatLon::new(0, 0);
	let mut ret = vec![];
	for point in points {
		cur = LatLon::new(cur.lat + point.lat, cur.lon + point.lon);
		ret.push(cur.clone());
	}
	ret
}

fn decode_double_delta(points: &[LatLon]) -> Vec<LatLon> {
	let mut cur = LatLon::new(0, 0);
	let mut offset = LatLon::new(0, 0);
	let mut i = 0;
	let mut ret = vec![];
	for point in points {
		let last = cur;
		cur = LatLon::new(cur.lat + offset.lat + point.lat, cur.lon + offset.lon + point.lon);
		if i > 0 {
			offset = LatLon::new(cur.lat - last.lat, cur.lon - last.lon);
		}
		ret.push(cur.clone());
		i += 1;
	}
	ret
}

fn coord_block(i: &[u8]) -> IResult<&[u8], Vec<LatLon>> {
	let (i, num) = vbe_u(i)?;
	Ok(count(latlon, num as usize)(i)?)
}

fn way_block(double_delta: bool, i: &[u8]) -> IResult<&[u8], Vec<Vec<LatLon>>> {
	let (i, num) = vbe_u(i)?;
	let (i, points) = count(coord_block, num as usize)(i)?;
	let decoded = points.into_iter().map(|poly| match double_delta {
		false => decode_single_delta(&poly),
		true => decode_double_delta(&poly),
	}).collect::<Vec<_>>();
	Ok((i, decoded))
}

pub fn way<'a, 'b>(debug: bool, tags: &'a [(String, TagDesc)], i: &'b [u8]) -> IResult<&'b [u8], Way> {
	let start = i.as_ptr();
	let (i, fields) = tuple((
		cond(debug, take(32 as usize)), // Debug
		vbe_u, // Size
		be_u16, // Subtile map
		be_u8, // Misc info
	))(i)?;
	let layer = (fields.3 >> 4) as i8 - 5;
	let ntags = fields.3 & 0x0f;
	let (i, tags) = tagmap(ntags, tags, i)?;
	let (i, flags) = be_u8(i)?;
	let (i, optfields) = tuple((
		cond(flags & 0x80 != 0, string), // Name
		cond(flags & 0x40 != 0, string), // House number
		cond(flags & 0x20 != 0, string), // Reference
		cond(flags & 0x10 != 0, latlon), // Label position
		cond(flags & 0x08 != 0, vbe_u), // Number of blocks
	))(i)?;
	let nblocks = optfields.4.unwrap_or(1);
	let double_delta = flags & 0x04 != 0;
	let (i, blocks) = count(|i| way_block(double_delta, i), nblocks as usize)(i)?;
	Ok((i, Way {
		size: fields.1,
		subtile_map: fields.2,
		layer,
		tags,
		name: optfields.0,
		house_number: optfields.1,
		reference: optfields.2,
		label_pos: optfields.3,
		blocks,
	}))
}

fn do_test<T: std::cmp::PartialEq + std::fmt::Debug>(f: fn(&[u8]) -> IResult<&[u8], T>, tests: Vec<(Vec<u8>, T, Vec<u8>)>) {
	for (input, expected, remain) in tests {
		assert_eq!(f(&input), Ok((remain.as_slice(), expected)));
	}
}

#[test]
fn test_vbe_u() {
	do_test(vbe_u, vec![
		(vec![0x0a], 10, vec![]),
		(vec![0x81, 0x01], 0x81, vec![]),
		(vec![0x80, 0x01, 0x81], 0x80, vec![0x81]),
	]);
}

#[test]
fn test_vbe_s() {
	do_test(vbe_s, vec![
		(vec![0x02], 2, vec![]),
		(vec![0x81, 0x01], 0x81, vec![]),
		(vec![0x81, 0x41], -0x81, vec![]),
	]);
}

#[test]
fn test_string() {
	do_test(string, vec![
		(b"\x05helloworld".to_vec(), "hello".to_string(), b"world".to_vec()),
	]);
}
