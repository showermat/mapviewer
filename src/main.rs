use std::collections::HashMap;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use skulpin::rafx::api::RafxExtents2D;
use skulpin::skia_safe::*;
use sdl2::event::{Event, EventSender, WindowEvent};
use sdl2::mouse::MouseButton;

mod render;
mod mapsforge;

use render::{Geometry, Material, RenderCache};

const ZOOM_MULTIPLIER: f32 = 1.2;

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
	offset: (i32, i32),
	old_offset: (i32, i32),
	scale: f32,
	zoom: u8,
	font: Font,
	text_paint: Paint,
	paints: HashMap<Material, Paint>,
	render: RenderCache,
}

impl Viewer {
	fn fit_to_screen(&mut self) {
		let (w, h) = self.size;
		let ((minx, miny), (maxx, maxy)) = self.render.map.bounds(self.zoom);
		self.scale = (w as f64 / (maxx - minx)).min(h as f64 / (maxy - miny)) as f32;
		//self.offset = ((-minx * self.scale as f64) as i32, (-miny * self.scale as f64) as i32);
		self.offset = (((w as f64 - (minx + maxx) * self.scale as f64) / 2.0) as i32, ((h as f64 - (miny + maxy) * self.scale as f64) / 2.0) as i32);
	}

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

	fn new(map: mapsforge::MapFile, init_size: (u32, u32)) -> Self {
		let mut font = Font::default();
		font.set_size(10.0);
		let paints = Self::paint_styles();
		let mut text_paint = Paint::new(Color4f::new(1.0, 1.0, 1.0, 1.0), None);
		text_paint.set_anti_alias(true);
		text_paint.set_style(paint::Style::Fill);
		text_paint.set_stroke(false);
		let mut ret = Self { i: 0, size: init_size, offset: (0, 0), old_offset: (0, 0), scale: 1.0, zoom: 5, font, text_paint, paints, render: RenderCache::new(map) };
		ret.fit_to_screen();
		ret
	}

	fn scale(&mut self, factor: f32, center: (i32, i32)) {
	}

	fn update(&mut self, events: &mut Events, size: (u32, u32)) -> bool {
		self.i = events.frames;
		self.size = size;
		let mut update = false;

		if events.button_change > 0 { self.old_offset = self.offset; }
		if let Some(start) = events.drag_start {
			let cur_offset = self.offset;
			self.offset = (self.old_offset.0 + events.mouse_pos.0 - start.0, self.old_offset.1 + events.mouse_pos.1 - start.1);
			if self.offset != cur_offset { update = true; }
		}
		if events.wheel != 0 {
			let scale_mul = ZOOM_MULTIPLIER.powf(events.wheel as f32);
			let center = events.mouse_pos;
			if scale_mul != 1.0 {
				self.scale *= scale_mul;
				let center_offset = (center.0 - self.offset.0, center.1 - self.offset.1);
				let newxoff = center.0 - (center_offset.0 as f32 * scale_mul) as i32;
				let newyoff = center.1 - (center_offset.1 as f32 * scale_mul) as i32;
				self.offset = (newxoff, newyoff);
				let new_zoom = self.render.map.desired_zoom_level(self.zoom, self.scale);
				if new_zoom != self.zoom {
					let factor = 2.0_f32.powf(self.zoom as f32 - new_zoom as f32);
					self.scale *= factor;
					self.zoom = new_zoom;
				}
				update = true;
			}
		}
		update
	}

	fn place_tile(&mut self, canvas: &mut Canvas, tile: (u32, u32), loc: (f32, f32), scale: f32) {
		let layers = &self.render.tile(self.zoom, tile.0, tile.1).layers;
		let xform = |point: (f64, f64)| (point.0 as f32 * scale + loc.0, point.1 as f32 * scale + loc.1);
		//canvas.draw_str(format!("{:?}", tile), xform((0.5, 0.5)), &self.font, &self.text_paint);
		//let ((left, top), (right, bottom)) = (xform((0.0, 0.0)), xform((1.0, 1.0)));
		//canvas.draw_rect(&Rect { top, left, bottom, right }, &self.paints[&Material::Unknown]);
		for (_, objs) in layers {
			for obj in objs {
				match &obj.geo {
					Geometry::Point(point) => {
						canvas.draw_point(xform(*point), &self.paints[&obj.material]);
					},
					Geometry::Path(polies) => {
						let mut path = Path::new();
						for poly in polies {
							path.move_to(xform(poly[0]));
							for point in poly[1..].into_iter() { path.line_to(xform(*point)); }
						}
						canvas.draw_path(&path, &self.paints[&obj.material]);
					},
				}
			}
		}
	}

	fn visible_tiles(&self, zoom: u8, w: u32, h: u32) -> ((u32, u32), (u32, u32)) {
		let ntiles = (1 << (zoom as u32)) as f32;
		let xmin = (-self.offset.0 as f32 / self.scale).floor();
		let xmax = ((w as i32 - self.offset.0) as f32 / self.scale).ceil();
		let ymin = (-self.offset.1 as f32 / self.scale).floor();
		let ymax = ((h as i32 - self.offset.1) as f32 / self.scale).ceil();
		((xmin.clamp(0.0, ntiles) as u32, xmax.clamp(0.0, ntiles) as u32), (ymin.clamp(0.0, ntiles) as u32, ymax.clamp(0.0, ntiles) as u32))
	}

	fn draw(&mut self, canvas: &mut Canvas) {
		let ISize { width: w, height: h } = canvas.base_layer_size();
		canvas.clear(Color::from_argb(0, 0, 0, 255));
		let (xrange, yrange) = self.visible_tiles(self.zoom, w as u32, h as u32);
		for xtile in xrange.0 .. xrange.1 {
			for ytile in yrange.0 .. yrange.1 {
				let origin = (self.scale * xtile as f32 + self.offset.0 as f32, self.scale * ytile as f32 + self.offset.1 as f32);
				self.place_tile(canvas, (xtile, ytile), origin, self.scale);
			}
		}
	}
}

fn main() {
	let path = std::env::args().skip(1).next().unwrap();
	let map = mapsforge::MapFile::new(PathBuf::from(path));
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

	let mut viewer = Viewer::new(map, (size.0, size.1));
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
