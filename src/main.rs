use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use skulpin::rafx::api::RafxExtents2D;
use skulpin::skia_safe::*;
use sdl2::event::{Event, EventSender, WindowEvent};
use sdl2::mouse::MouseButton;

mod render;
mod mapsforge;

use mapsforge::Coord;
use render::{BoundingBox, Geometry, Material, RenderCache};

const ZOOM_MULTIPLIER: f64 = 1.2;
const MAX_DETAIL: i64 = 10; // Smallest feature to display in pixels

struct UpdateEvent { }

struct Trigger {
	sender: EventSender,
}

impl Trigger {
	fn trigger(&self) {
		self.sender.push_custom_event(UpdateEvent { }).unwrap();
	}
}

struct Events {
	pump: sdl2::EventPump,
	subsystem: sdl2::EventSubsystem,
	frames: u64,
	should_quit: bool,
	force_redraw: bool,
	mouse_pos: (i32, i32),
	drag_start: Option<(i32, i32)>,
	button_change: i32,
	clicks: u32,
	wheel: i32,
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
			should_quit: false,
			force_redraw: true,
			mouse_pos: mouse_pos,
			drag_start: if mouse_state.left() { Some(mouse_pos) } else { None },
			button_change: 0,
			clicks: 0,
			wheel: 0,
		}
	}

	fn get_trigger(&mut self) -> Trigger {
		Trigger { sender: self.subsystem.event_sender() }
	}

	fn get_events(&mut self, block: bool) -> Vec<Event> {
		if block {
			let mut ret = vec![self.pump.wait_event()];
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
		self.force_redraw = self.frames == 0;
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
				_ => (),
			}
		}
		let mouse_state = self.pump.mouse_state();
		self.mouse_pos = (mouse_state.x(), mouse_state.y());
	}
}

struct Viewer {
	i: u64,
	size: (u32, u32),
	offset: Coord, // Offset of viewport from origin in coord units
	old_offset: Coord,
	scale: u32, // Coord units per pixel -- larger is zooming out
	font: Font,
	text_paint: Paint,
	paints: HashMap<Material, Paint>,
	render: RenderCache,
}

impl Viewer {
	fn paint_styles() -> HashMap<Material, Paint> {
		let mut ret = HashMap::new();
		ret.insert(Material::Unknown, {
			let mut paint = Paint::new(Color4f::new(1.0, 1.0, 1.0, 1.0), None);
			paint.set_anti_alias(true);
			paint.set_style(paint::Style::Stroke);
			paint.set_stroke_width(2.0);
			paint
		});
		ret.insert(Material::Land, {
			let mut paint = Paint::new(Color4f::new(0.8, 0.8, 0.8, 0.5), None);
			paint.set_anti_alias(true);
			paint.set_style(paint::Style::Fill);
			paint.set_stroke(false);
			paint
		});
		ret.insert(Material::Water, {
			let mut paint = Paint::new(Color4f::new(0.5, 0.5, 1.0, 0.5), None);
			paint.set_anti_alias(true);
			paint.set_style(paint::Style::Fill);
			paint.set_stroke(false);
			paint
		});
		ret
	}

	fn new(maps: Vec<mapsforge::MapFile>, init_size: (u32, u32)) -> Self {
		let mut font = Font::default();
		font.set_size(10.0);
		let paints = Self::paint_styles();
		let mut text_paint = Paint::new(Color4f::new(1.0, 1.0, 1.0, 1.0), None);
		text_paint.set_anti_alias(true);
		text_paint.set_style(paint::Style::Fill);
		text_paint.set_stroke(false);
		let render = RenderCache::new(maps);
		let bounds = render.bounds();
		let scale = (bounds.width() as u32 / init_size.0).max(bounds.height() as u32 / init_size.1);
		let viewport_adj = Coord { x: -(scale as i64 * init_size.0 as i64) / 2, y: -(scale as i64 * init_size.1 as i64) / 2 };
		let offset = bounds.midpoint().unwrap().add(&viewport_adj);
		Self { i: 0, size: init_size, offset, old_offset: offset, scale, font, text_paint, paints, render }
	}

	fn viewport(&self) -> BoundingBox {
		let winsize = Coord { x: self.size.0 as i64 * self.scale as i64, y: self.size.1 as i64 * self.scale as i64 };
		BoundingBox::from_corners((self.offset, self.offset.add(&winsize)))
	}

	fn update(&mut self, events: &mut Events, size: (u32, u32)) -> bool {
		self.i = events.frames;
		self.size = size;
		let mut update = false;

		// TODO Try scrolling mouse wheel while dragging
		if events.button_change > 0 { self.old_offset = self.offset; }
		if let Some(start) = events.drag_start {
			let cur_offset = self.offset;
			self.offset = Coord {
				x: self.old_offset.x - (events.mouse_pos.0 - start.0) as i64 * self.scale as i64,
				y: self.old_offset.y - (events.mouse_pos.1 - start.1) as i64 * self.scale as i64,
			};
			if self.offset != cur_offset { update = true; }
		}
		if events.wheel != 0 {
			let scale_mul = ZOOM_MULTIPLIER.powf(events.wheel as f64);
			let center = (events.mouse_pos.0 as i64, events.mouse_pos.1 as i64);
			if scale_mul != 1.0 {
				self.scale = (self.scale as f64 / scale_mul).round() as u32;
				let factor = self.scale as f64 * (1.0 - scale_mul);
				self.offset = Coord {
					x: self.offset.x - (center.0 as f64 * factor) as i64,
					y: self.offset.y - (center.1 as f64 * factor) as i64,
				};
				update = true;
			}
		}
		update
	}

	fn place_tile(&mut self, canvas: &mut Canvas, tile: Arc<render::RenderTile>) {
		let xform = |point: Coord| ((point.x - self.offset.x) / self.scale as i64, (point.y - self.offset.y) / self.scale as i64);
		let downcast = |point: (i64, i64)| (point.0 as f32, point.1 as f32);
		/*let bounds = tile.bounds();
		canvas.draw_str(format!("{:?}", (tile.x, tile.y)), downcast(xform(bounds.midpoint().unwrap())), &self.font, &self.text_paint);
		let (topleft, botright) = bounds.corners().unwrap();
		let topleft = downcast(xform(topleft));
		let botright = downcast(xform(botright));
		canvas.draw_rect(&Rect { left: topleft.0, top: topleft.1, right: botright.0, bottom: botright.1 }, &self.paints[&Material::Unknown]);
		return;*/
		for (_, objs) in &tile.layers {
			for obj in objs {
				match &obj.geo {
					Geometry::Point(point) => {
						canvas.draw_point(downcast(xform(*point)), &self.paints[&obj.material]);
					},
					Geometry::Path(polies) => {
						let mut path = Path::new();
						let mut bounds = BoundingBox::empty();
						for poly in polies {
							let transformed = xform(poly[0]);
							path.move_to(downcast(transformed));
							bounds.include(transformed.into());
							for point in poly[1..].into_iter() {
								let transformed = xform(*point);
								path.line_to(downcast(transformed));
								bounds.include(transformed.into());
							}
						}
						if bounds.max_dimension() > MAX_DETAIL { canvas.draw_path(&path, &self.paints[&obj.material]); }
					},
				}
			}
		}
	}

	fn draw(&mut self, canvas: &mut Canvas) {
		canvas.clear(Color::from_argb(0, 0, 0, 255));
		for tile in self.render.viewport_tiles(&self.viewport(), self.size.0) {
			self.place_tile(canvas, tile);
		}
	}
}

fn main() {
	let maps: Vec<mapsforge::MapFile> = std::env::args().skip(1).map(|path| mapsforge::MapFile::new(PathBuf::from(path))).collect();
	//map.test();

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

	loop {
		events.update(!redraw);
		if events.should_quit { break; }
		let size = window.vulkan_drawable_size();
		redraw = viewer.update(&mut events, (size.0, size.1));
		if redraw || events.force_redraw {
			renderer.draw(RafxExtents2D { width: size.0, height: size.1 }, 1.0, |canvas, _coordinate_helper| {
				viewer.draw(canvas);
				events.frames += 1;
			}).unwrap();
		}
	}
}
