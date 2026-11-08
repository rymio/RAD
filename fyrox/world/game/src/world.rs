//! Building the world's scene: the ground, the river, the first things in it, and the two
//! characters. A world is made once (`fresh`), then saved and loaded as an ordinary Fyrox scene,
//! so the editor can change it too. Things in it are found again by their tags:
//!   `thing:<kind>:<who built it>`  a thing (its node's name is the thing's name)
//!   `water`, `bridge`             the river and where it can be crossed
//!   `character:gilgamesh`, `character:you`

#[allow(unused_imports)]
use fyrox::graph::prelude::*;
use fyrox::{
    asset::untyped::ResourceKind,
    core::{algebra::Vector3, color::Color, math::Rect, pool::Handle, uuid::Uuid},
    material::{Material, MaterialResource},
    resource::texture::{Texture, TextureKind, TextureMagnificationFilter, TextureMinificationFilter, TexturePixelKind, TextureResource, TextureWrapMode},
    scene::{
        base::BaseBuilder,
        camera::{CameraBuilder, OrthographicProjection, Projection},
        dim2::rectangle::{Rectangle, RectangleBuilder},
        graph::Graph,
        light::{point::PointLightBuilder, BaseLightBuilder},
        node::Node,
        pivot::PivotBuilder,
        transform::TransformBuilder,
        Scene,
    },
};

use crate::art::{self, Img};
use crate::catalog::{self, Kind};

pub const W: f32 = 48.0;
pub const H: f32 = 32.0;
/// How many tiles show from top to bottom of the window.
pub const VIEW: f32 = 15.0;

/// The river runs north–south here, with a bridge for the path.
pub const RIVER: (f32, f32) = (30.0, 33.0);
pub const BRIDGE: (f32, f32) = (14.0, 16.5);

pub fn texture(img: &Img, repeat: bool) -> TextureResource {
    let mut t = Texture::from_bytes(TextureKind::Rectangle { width: img.w, height: img.h }, TexturePixelKind::RGBA8, img.px.clone()).expect("texture size");
    t.set_magnification_filter(TextureMagnificationFilter::Nearest);
    t.set_minification_filter(TextureMinificationFilter::Nearest);
    let wrap = if repeat { TextureWrapMode::Repeat } else { TextureWrapMode::ClampToEdge };
    t.set_s_wrap_mode(wrap);
    t.set_t_wrap_mode(wrap);
    TextureResource::new_ok(Uuid::new_v4(), ResourceKind::Embedded, t)
}

pub fn material(tex: TextureResource) -> MaterialResource {
    let mut m = Material::standard_2d();
    m.bind("diffuseTexture", Some(tex));
    MaterialResource::new_ok(Uuid::new_v4(), ResourceKind::Embedded, m)
}

/// Depth for something standing at `y` (lower on screen is nearer, so it covers what's behind).
/// Everything stands between z 10 and 10.4; the ground lies just behind (11–12), so the night
/// lights (z 10.5) reach both.
pub fn depth(y: f32) -> f32 {
    10.0 + y * 0.01
}

/// A picture `w`×`h` tiles whose bottom centre is at (x, y).
#[allow(clippy::too_many_arguments)]
pub fn sprite(graph: &mut Graph, name: &str, tag: &str, mat: MaterialResource, x: f32, y: f32, w: f32, h: f32, z: f32) -> Handle<Rectangle> {
    RectangleBuilder::new(
        BaseBuilder::new()
            .with_name(name)
            .with_tag(tag.to_string())
            .with_local_transform(TransformBuilder::new().with_local_position(Vector3::new(x, y + h / 2.0, z)).with_local_scale(Vector3::new(w, h, 1.0)).build()),
    )
    .with_material(mat)
    .build(graph)
}

/// Ground that repeats its tile over the area (x0, y0)–(x1, y1).
fn ground(graph: &mut Graph, name: &str, tag: &str, img: &Img, x0: f32, y0: f32, x1: f32, y1: f32, z: f32) -> Handle<Rectangle> {
    let (w, h) = (x1 - x0, y1 - y0);
    let r = sprite(graph, name, tag, material(texture(img, true)), x0 + w / 2.0, y0, w, h, z);
    if let Ok(rect) = graph.try_get_mut(r) {
        rect.set_uv_rect(Rect::new(0.0, 0.0, w, h));
    }
    r
}

/// A light that comes on at night (fires, lamps, windows).
pub fn night_light(graph: &mut Graph, x: f32, y: f32, radius: f32, color: Color) -> Handle<Node> {
    let l = PointLightBuilder::new(BaseLightBuilder::new(BaseBuilder::new().with_name("Night light").with_tag("night-light".into()).with_visibility(false).with_local_transform(TransformBuilder::new().with_local_position(Vector3::new(x, y, 10.5)).build())).with_color(color).with_scatter_enabled(false))
        .with_radius(radius)
        .build(graph);
    l.transmute()
}

/// Puts a thing in the world: its picture, tagged with its kind and who built it.
pub fn place(graph: &mut Graph, parent: Handle<Node>, kind: &Kind, name: &str, by: &str, x: f32, y: f32) -> Handle<Rectangle> {
    let z = if kind.flat { 11.5 - y * 0.001 } else { depth(y) };
    let r = sprite(graph, name, &format!("thing:{}:{}", kind.id, by), material(texture(&(kind.draw)(), false)), x, y, kind.w, kind.h, z);
    graph.link_nodes(r, parent);
    // Lights are children of the picture, so their place is in its (scaled) space: `up` tiles
    // above its foot, in front of it.
    let light = |g: &mut Graph, up: f32, radius: f32, color: Color| {
        let l = night_light(g, 0.0, (up - kind.h / 2.0) / kind.h, radius, color);
        g[l].local_transform_mut().set_position(Vector3::new(0.0, (up - kind.h / 2.0) / kind.h, 10.5 - z));
        g.link_nodes(l, r);
    };
    match kind.id {
        "fire" => light(graph, 0.6, 3.0, Color::opaque(200, 120, 60)),
        "lamp" => light(graph, 1.5, 2.6, Color::opaque(170, 140, 90)),
        "house" | "workshop" => light(graph, 1.2, 2.2, Color::opaque(150, 110, 60)),
        _ => {}
    }
    r
}

/// The things a new world starts with: (kind, x, y, name).
pub const START: &[(&str, f32, f32, &str)] = &[
    ("house", 8.0, 17.0, "Gilgamesh's house"),
    ("tablet", 12.5, 16.2, "The clay tablet"),
    ("garden", 13.0, 20.0, "Kitchen garden"),
    ("well", 15.5, 12.0, "Well"),
    ("fire", 5.0, 13.5, "Fire pit"),
    ("bench", 7.0, 13.2, "Bench"),
    ("palm", 27.0, 22.0, "Palm"),
    ("palm", 28.0, 8.0, "Palm"),
    ("palm", 22.0, 26.0, "Palm"),
    ("palm", 36.0, 25.0, "Palm"),
    ("palm", 37.0, 9.0, "Palm"),
    ("cedar", 3.0, 27.0, "Cedar"),
    ("cedar", 6.0, 29.0, "Cedar"),
    ("cedar", 2.5, 23.0, "Cedar"),
    ("cedar", 9.0, 28.5, "Cedar"),
    ("ziggurat", 42.0, 19.0, "The ziggurat"),
    ("field", 39.0, 11.0, "Barley field"),
    ("field", 42.0, 11.0, "Barley field"),
    ("reeds", 29.4, 25.0, "Reeds"),
    ("reeds", 29.5, 5.0, "Reeds"),
    ("reeds", 33.6, 19.0, "Reeds"),
    ("boat", 31.5, 21.0, "Reed boat"),
    ("flowers", 11.0, 22.5, "Flowers"),
    ("flowers", 17.0, 18.0, "Flowers"),
    ("rock", 19.0, 6.0, "Rock"),
    ("lamp", 11.0, 15.5, "Lamp"),
];

pub const GILGAMESH_START: (f32, f32) = (10.5, 15.2);
pub const YOU_START: (f32, f32) = (18.0, 13.0);

/// A new world.
pub fn fresh() -> Scene {
    let mut scene = Scene::new();
    scene.rendering_options.clear_color = Some(Color::opaque(108, 158, 74));
    scene.rendering_options.ambient_lighting_color = Color::WHITE;
    let g = &mut scene.graph;

    let cam = CameraBuilder::new(BaseBuilder::new().with_name("Camera").with_local_transform(TransformBuilder::new().with_local_position(Vector3::new(YOU_START.0, YOU_START.1, -5.0)).build()))
        .with_projection(Projection::Orthographic(OrthographicProjection { z_near: 0.0, z_far: 100.0, vertical_size: VIEW / 2.0 }))
        .build(g);
    let _ = cam;

    let ground_root = PivotBuilder::new(BaseBuilder::new().with_name("Ground")).build(g);
    let layers = [
        ground(g, "Grass", "ground", &art::grass(), 0.0, 0.0, W, H, 12.0),
        ground(g, "Sand (east bank)", "ground", &art::sand(), RIVER.0 - 1.0, 0.0, RIVER.0, H, 11.95),
        ground(g, "Sand (west bank)", "ground", &art::sand(), RIVER.1, 0.0, RIVER.1 + 1.0, H, 11.95),
        ground(g, "River", "water", &art::water(), RIVER.0, 0.0, RIVER.1, H, 11.9),
        ground(g, "Bridge", "bridge", &art::bridge(), RIVER.0 - 0.5, BRIDGE.0, RIVER.1 + 0.5, BRIDGE.1, 11.85),
        ground(g, "Path to the bridge", "path", &art::path(), 9.0, 14.6, RIVER.0 - 0.5, 15.9, 11.88),
        ground(g, "Path to the ziggurat", "path", &art::path(), RIVER.1 + 0.5, 14.6, 42.0, 15.9, 11.88),
        ground(g, "Path to the temple", "path", &art::path(), 41.4, 15.9, 42.6, 19.0, 11.88),
    ];
    for l in layers {
        g.link_nodes(l, ground_root);
    }

    let things = PivotBuilder::new(BaseBuilder::new().with_name("Things")).build(g);
    for (id, x, y, name) in START {
        let k = catalog::by_id(id).expect("kind");
        place(g, things.transmute(), k, name, "world", *x, *y);
    }

    character(g, "Gilgamesh", "character:gilgamesh", &art::gilgamesh(0), GILGAMESH_START);
    character(g, "You", "character:you", &art::traveller(0), YOU_START);
    scene
}

/// A character: a pivot at their feet, with a shadow and a body (and for Gilgamesh, his face).
pub fn character(g: &mut Graph, name: &str, tag: &str, img: &Img, at: (f32, f32)) -> Handle<Node> {
    let root = PivotBuilder::new(BaseBuilder::new().with_name(name).with_tag(tag.into()).with_local_transform(TransformBuilder::new().with_local_position(Vector3::new(at.0, at.1, depth(at.1) - 0.005)).build())).build(g);
    let shadow = sprite(g, "Shadow", "", material(texture(&art::shadow(), false)), 0.0, -0.15, 1.0, 0.375, 0.002);
    let body = sprite(g, "Body", "", material(texture(img, false)), 0.0, 0.0, 1.0, 1.5, 0.0);
    g.link_nodes(shadow, root);
    g.link_nodes(body, root);
    root.transmute()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_world() {
        let s = fresh();
        let tagged = |t: &str| s.graph.pair_iter().filter(|(_, n)| n.tag().starts_with(t)).count();
        assert_eq!(tagged("thing:"), START.len());
        assert_eq!(tagged("water"), 1);
        assert_eq!(tagged("character:"), 2);
    }

    /// Fyrox's 2D camera looks along +z, so +x is on the LEFT of the screen (and Rectangle maps
    /// the left of a texture to +x). The arrow keys and what Gilgamesh is told depend on it.
    #[test]
    fn x_grows_to_the_left_of_the_screen() {
        use fyrox::scene::camera::Camera;
        let mut s = fresh();
        s.graph.update_hierarchical_data();
        let (h, _) = s.graph.pair_iter().find(|(_, n)| n.cast::<Camera>().is_some()).unwrap();
        s.graph[h].cast_mut::<Camera>().unwrap().calculate_matrices(fyrox::core::algebra::Vector2::new(800.0, 600.0));
        let p = s.graph[h].global_position();
        let cam = s.graph[h].cast::<Camera>().unwrap();
        let x_on_screen = |x: f32| {
            let v = cam.view_projection_matrix() * fyrox::core::algebra::Vector4::new(x, p.y, 10.0, 1.0);
            v.x / v.w
        };
        assert!(x_on_screen(p.x + 1.0) < x_on_screen(p.x));
    }
}
