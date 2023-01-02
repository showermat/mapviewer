#![feature(int_roundings)]

extern crate rayon;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use skulpin::rafx::api::RafxExtents2D;
use skulpin::skia_safe::*;
use sdl2::event::{Event, EventSender, WindowEvent};
use sdl2::keyboard::{Keycode, Mod};
use sdl2::mouse::MouseButton;

mod mapsforge;
mod render;
mod theme;

use mapsforge::Coord;
use render::{BoundingBox, Geometry, RenderManager, RenderTile};

const ZOOM_MULTIPLIER: f64 = 1.2;
const PAN_INCREMENT: i32 = 100;
const MAX_DETAIL: i64 = 4; // Smallest feature to display in pixels

enum UpdateEvent {
	Tile { generation: u64, tile: Arc<RenderTile> },
}

#[derive(Clone)]
pub struct Updater {
	sender: Arc<EventSender>,
}

impl Updater {
	fn send(&self, event: UpdateEvent) {
		self.sender.push_custom_event(event).unwrap();
	}
}

unsafe impl Send for Updater { }
unsafe impl Sync for Updater { }

struct Events {
	pump: sdl2::EventPump,
	subsystem: sdl2::EventSubsystem,
	frames: u64,
	force_redraw: bool,
	should_quit: bool,
	tiles_ready: Vec<(u64, Arc<RenderTile>)>,
	mouse_pos: (i32, i32),
	prev_mouse_pos: (i32, i32),
	drag_start: Option<(i32, i32)>,
	button_change: i32,
	clicks: u32,
	wheel: i32,
	keys: Vec<(Keycode, Mod)>,
}

impl Events {
	fn new(context: &sdl2::Sdl) -> Self {
		let subsys = context.event().unwrap();
		let pump = context.event_pump().unwrap();
		subsys.register_custom_event::<UpdateEvent>().unwrap();
		let mouse_state = pump.mouse_state();
		let mouse_pos = (mouse_state.x(), mouse_state.y());
		Self {
			pump: pump,
			subsystem: subsys,
			frames: 0,
			force_redraw: false,
			should_quit: false,
			tiles_ready: vec![],
			mouse_pos: mouse_pos,
			prev_mouse_pos: mouse_pos,
			drag_start: if mouse_state.left() { Some(mouse_pos) } else { None },
			button_change: 0,
			clicks: 0,
			wheel: 0,
			keys: vec![],
		}
	}

	fn get_updater(&mut self) -> Updater {
		Updater { sender: Arc::new(self.subsystem.event_sender()) }
	}

	fn get_events(&mut self, block: bool) -> Vec<Event> {
		if block {
			let mut ret = vec![];
			//let mut ret = vec![self.pump.wait_event()];
			// TODO This loop is nasty and we should be able to replace it with the single line
			// above, but for some reason the presence of user events added by another thread does
			// not always cause wait_event to return.  In the loop below, we see the timeout being
			// reached and returning no events, and then many events being immediately found when
			// it is executed on the next run through the loop.  I assume this is something
			// threading-related.  Until I can figure it out, this is a hack that gets us close
			// enough.
			loop {
				if let Some(event) = self.pump.wait_event_timeout(500) {
					ret.push(event);
					break;
				}
			}
			ret.extend(self.pump.poll_iter());
			ret
		}
		else {
			self.pump.poll_iter().collect()
		}
	}

	fn update(&mut self, block: bool) {
		self.button_change = 0;
		self.clicks = 0;
		self.wheel = 0;
		self.force_redraw = false;
		//self.tiles_ready.clear();
		self.keys = vec![];
		for event in self.get_events(block) {
			match event {
				Event::Quit { .. } => self.should_quit = true,
				Event::MouseButtonDown { mouse_btn, x, y, .. } if mouse_btn == MouseButton::Left => {
					self.button_change += 1;
					self.drag_start = Some((x, y))
				},
				Event::MouseButtonUp { mouse_btn, x, y, .. } if mouse_btn == MouseButton::Left => {
					self.button_change -= 1;
					if self.drag_start == Some((x, y)) { self.clicks += 1; }
					self.drag_start = None;
				},
				Event::MouseWheel { y, .. } => self.wheel += y,
				Event::Window { win_event, .. } => {
					match win_event {
						WindowEvent::Resized(_, _) | WindowEvent::SizeChanged(_, _) => self.force_redraw = true,
						_ => (),
					}
				},
				Event::KeyDown { keycode, keymod, .. } => {
					if let Some(code) = keycode {
						self.keys.push((code, keymod));
						if (code, keymod) == (Keycode::Q, Mod::empty()) { self.should_quit = true; }
					}
				}
				Event::User { .. } => {
					match event.as_user_event_type::<UpdateEvent>().unwrap() {
						UpdateEvent::Tile { generation, tile } => self.tiles_ready.push((generation, tile)),
					}
				}
				_ => (),
			}
		}
		let mouse_state = self.pump.mouse_state();
		self.prev_mouse_pos = self.mouse_pos;
		self.mouse_pos = (mouse_state.x(), mouse_state.y());
	}
}

struct Viewer {
	size: (u32, u32),
	offset: Coord, // Offset of viewport from origin in coord units
	scale: u32, // Coord units per pixel -- larger is zooming out
	font: Font,
	text_paint: Paint,
	render: RenderManager,
	generation: u64,
}

impl Viewer {
	fn zoom_to_fit(&mut self) {
		let bounds = self.render.bounds();
		self.scale = (bounds.width() as u32 / self.size.0).max(bounds.height() as u32 / self.size.1);
		let viewport_adj = Coord { x: -(self.scale as i64 * self.size.0 as i64) / 2, y: -(self.scale as i64 * self.size.1 as i64) / 2 };
		self.offset = bounds.midpoint().unwrap().add(&viewport_adj);
	}

	fn new(maps: Vec<Arc<mapsforge::MapFile>>, init_size: (u32, u32)) -> Self {
		let mut font = Font::default();
		font.set_size(10.0);
		let mut text_paint = Paint::new(Color4f::new(1.0, 1.0, 1.0, 1.0), None);
		text_paint.set_anti_alias(true);
		text_paint.set_style(paint::Style::Fill);
		text_paint.set_stroke(false);
		let render = RenderManager::new(maps);
		let mut ret = Self { size: init_size, offset: Coord { x: 0, y: 0 }, scale: 0, font, text_paint, render, generation: 0 };
		ret.zoom_to_fit();
		ret
	}

	fn viewport(&self) -> BoundingBox {
		let winsize = Coord { x: self.size.0 as i64 * self.scale as i64, y: self.size.1 as i64 * self.scale as i64 };
		BoundingBox::from_corners((self.offset, self.offset.add(&winsize)))
	}

	fn zoom(&mut self, factor: i32, center: (u32, u32)) {
		let scale_mul = ZOOM_MULTIPLIER.powf(factor as f64);
		self.scale = (self.scale as f64 / scale_mul).round() as u32;
		let offset_mul = self.scale as f64 * (1.0 - scale_mul);
		self.offset = Coord {
			x: self.offset.x - (center.0 as f64 * offset_mul) as i64,
			y: self.offset.y - (center.1 as f64 * offset_mul) as i64,
		};
	}

	fn pan(&mut self, delta: (i32, i32)) {
		self.offset = Coord {
			x: self.offset.x - delta.0 as i64 * self.scale as i64,
			y: self.offset.y - delta.1 as i64 * self.scale as i64,
		};
	}

	fn update(&mut self, events: &Events, size: (u32, u32)) -> bool {
		let mut update = events.force_redraw;
		if size != self.size || events.frames == 0 { update = true; }
		self.size = size;

		if events.drag_start.is_some() {
			let delta = (events.mouse_pos.0 - events.prev_mouse_pos.0, events.mouse_pos.1 - events.prev_mouse_pos.1);
			if delta != (0, 0) {
				self.pan(delta);
				update = true;
			}
		}
		if events.wheel != 0 {
			self.zoom(events.wheel, (events.mouse_pos.0.max(0) as u32, events.mouse_pos.1.max(0) as u32));
			update = true;
		}
		let mut key_zoom = 0;
		let mut key_pan = (0, 0);
		let mut reset = false;
		for key in &events.keys {
			if !key.1.is_empty() { continue; }
			match key.0 {
				Keycode::Equals | Keycode::KpPlus => { key_zoom += 1; },
				Keycode::Minus | Keycode::KpMinus => { key_zoom -= 1; },
				Keycode::Left | Keycode::H => { key_pan.0 += PAN_INCREMENT; },
				Keycode::Right | Keycode::L => { key_pan.0 -= PAN_INCREMENT; },
				Keycode::Up | Keycode::K => { key_pan.1 += PAN_INCREMENT; },
				Keycode::Down | Keycode::J => { key_pan.1 -= PAN_INCREMENT; },
				Keycode::Num0 => { reset = true; },
				_ => {}
			}
		}
		if reset {
			self.zoom_to_fit();
			update = true;
		}
		else {
			if key_pan != (0, 0) {
				self.pan(key_pan);
				update = true;
			}
			if key_zoom != 0 {
				self.zoom(key_zoom, (self.size.0 / 2, self.size.1 / 2));
				update = true;
			}
		}

		if update { self.generation = events.frames; }
		update
	}

	fn place_tile(&mut self, canvas: &mut Canvas, tile: Arc<render::RenderTile>) {
		let xform = |point: Coord| Coord { x: (point.x - self.offset.x) / self.scale as i64, y: (point.y - self.offset.y) / self.scale as i64 };
		let downcast = |point: Coord| (point.x as f32, point.y as f32);
		let bounds = tile.bounds();
		let (topleft, botright) = bounds.corners().unwrap();
		let topleft = downcast(xform(topleft));
		let botright = downcast(xform(botright));
		canvas.draw_rect(Rect::new(topleft.0, topleft.1, botright.0, botright.1), &Paint::new(Color4f::new(0.0, 0.0, 0.0, 1.0), None));
		/*canvas.draw_rect(Rect::new(topleft.0, topleft.1, botright.0, botright.1), &self.paints[&Material::Unknown]);
		canvas.draw_str(format!("{:?} {}", (tile.x, tile.y), self.generation), downcast(xform(bounds.midpoint().unwrap())), &self.font, &self.text_paint);
		return;*/
		for (_, objs) in &tile.layers {
			for obj in objs {
				match &obj.geo {
					Geometry::Point(point) => {
						let loc = downcast(xform(*point));
						for paint in obj.material.paints() {
							canvas.draw_point(loc, &paint);
						}
						if let Some(name) = &obj.name {
							canvas.draw_str(name, loc, &self.font, &self.text_paint);
						}
					},
					Geometry::Path(polies) => {
						let mut path = Path::new();
						let mut bounds = BoundingBox::empty();
						for poly in polies {
							let point = xform(poly[0]);
							path.move_to(downcast(point));
							bounds.include(point);
							for point in poly[1..].into_iter() {
								let point = xform(*point);
								path.line_to(downcast(point));
								bounds.include(point);
							}
						}
						if bounds.max_dimension() > MAX_DETAIL {
							for paint in obj.material.paints() {
								canvas.draw_path(&path, &paint);
							}
							/*if let Some(name) = &obj.name {
								let loc = downcast(bounds.midpoint().expect("No midpoint of non-mepty bounding box"));
								canvas.draw_str(name, loc, &self.font, &self.text_paint);
							}*/
						}
					},
				}
			}
		}
	}
	
	fn clear(&mut self, canvas: &mut Canvas) {
		canvas.clear(Color4f::new(0.0, 0.0, 0.0, 1.0));
	}

	fn draw(&mut self, canvas: &mut Canvas, tiles: &mut Vec<(u64, Arc<RenderTile>)>) {
		// These two lines do the transformation for us, but it's not faster and also scales fonts
		// and line widths, which we don't want.
		//canvas.scale(((1.0 / self.scale as f64) as f32, (1.0 / self.scale as f64) as f32));
		//canvas.translate((-self.offset.x as f32, -self.offset.y as f32));
		for tile in tiles.drain(..) {
			if tile.0 == self.generation {
				self.place_tile(canvas, tile.1);
			}
		}
	}
}

fn main() {
	let maps: Vec<Arc<mapsforge::MapFile>> = std::env::args().skip(1).map(|path| Arc::new(mapsforge::MapFile::new(PathBuf::from(path)))).collect();
	if maps.is_empty() {
		println!("Nothing to display");
		return;
	}

	let sdl_context = sdl2::init().unwrap();
	let video = sdl_context.video().unwrap();
	let window = video
		.window("Map Viewer", 800, 600)
		.position_centered()
		.allow_highdpi()
		.resizable()
		.build().unwrap();
	let size = window.vulkan_drawable_size();
	let mut renderer = skulpin::RendererBuilder::new()
		.coordinate_system(skulpin::CoordinateSystem::Logical)
		.build(&window, RafxExtents2D { width: size.0, height: size.1 }).unwrap();
	let mut events = Events::new(&sdl_context);

	let mut viewer = Viewer::new(maps, (size.0, size.1));
	let mut redraw = true;
	renderer.draw(RafxExtents2D { width: size.0, height: size.1 }, 1.0, |canvas, _| {
		canvas.clear(Color::from_argb(0, 0, 0, 255));
	}).unwrap();

	loop {
		events.update(!redraw);
		if events.should_quit { break; }
		let size = window.vulkan_drawable_size();
		let extents = RafxExtents2D { width: size.0, height: size.1 };
		redraw = viewer.update(&mut events, (size.0, size.1));
		if redraw {
			viewer.render.async_viewport_tiles(&viewer.viewport(), viewer.size.0, events.frames, events.get_updater());
			// Without this call, junk on the canvas is not cleared when the window is resized.  Race condition?
			renderer.draw(extents, 1.0, |_canvas, _| {
				//viewer.clear(canvas);
			}).unwrap();
		}
		else if !events.tiles_ready.is_empty() {
			renderer.draw(extents, 1.0, |canvas, _| {
				viewer.draw(canvas, &mut events.tiles_ready);
			}).unwrap();
		}
		events.frames += 1;
	}
}
