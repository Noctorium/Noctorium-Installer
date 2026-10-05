//! The window drawn without a window: every stage of it laid out by egui, painted into memory by a few
//! dozen lines of rasteriser, and written out as a PNG -- so what it looks like can be checked on a
//! machine with no display, or without anybody's desktop being touched to find out.
//!
//! The painting is the same arithmetic egui's own OpenGL painter does: vertex colour times texture in
//! gamma space, premultiplied, blended one-minus-source-alpha. It does not antialias -- egui's shapes are
//! feathered at their edges already, which is what the GPU is relied on for anyway.
//!
//! `cargo test --bin noctorium-installer -- --ignored the_window_drawn` writes the pictures into the
//! folder NOCTORIUM_RENDER_TO names, or into noctorium-installer-render in the temporary folder. The
//! tests that are not ignored lay every stage out and read back what a screen reader would be told,
//! which is quick, and needs no pictures.

use super::*;
use eframe::egui::accesskit::{Role, Toggled};
use noctorium_installer::github::{Arch, Asset, Release};
use noctorium_installer::install::Places;
use noctorium_installer::system::{Os, System};
use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// A texture as egui has described it so far.
struct Texture {
    size: [usize; 2],
    pixels: Vec<egui::Color32>,
}

impl Texture {
    /// Bilinear, clamped at the edges, as the painter's sampler is.
    fn sample(&self, uv: egui::Pos2) -> [f32; 4] {
        let [w, h] = self.size;
        let x = (uv.x * w as f32 - 0.5).clamp(0.0, (w - 1) as f32);
        let y = (uv.y * h as f32 - 0.5).clamp(0.0, (h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let at = |x: usize, y: usize| {
            let c = self.pixels[y * w + x];
            [c.r(), c.g(), c.b(), c.a()].map(|v| v as f32 / 255.0)
        };
        let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * fx;
            let bottom = c[i] + (d[i] - c[i]) * fx;
            top + (bottom - top) * fy
        })
    }
}

/// One frame, as pixels, and what AccessKit was told about it.
pub struct Picture {
    pub width: usize,
    pub height: usize,
    rgba: Vec<[f32; 4]>,
    pub accessible: Vec<egui::accesskit::Node>,
}

/// Lays out [draw] in a window of [size] points, a few frames running so everything that measures itself
/// has, and paints the last of them at [scale] pixels a point when [paint] says to.
pub fn render(
    size: egui::Vec2,
    scale: f32,
    paint: bool,
    mut draw: impl FnMut(&egui::Context),
) -> Picture {
    let ctx = egui::Context::default();
    ctx.enable_accesskit();
    let mut textures: HashMap<egui::TextureId, Texture> = HashMap::new();
    let mut primitives = Vec::new();
    let mut accessible = Vec::new();
    for frame in 0..4 {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            time: Some(1.0 + frame as f64 / 60.0),
            ..Default::default()
        };
        input
            .viewports
            .entry(egui::ViewportId::ROOT)
            .or_default()
            .native_pixels_per_point = Some(scale);
        let output = ctx.run(input, |ctx| draw(ctx));
        for (id, delta) in &output.textures_delta.set {
            let egui::ImageData::Color(image) = &delta.image;
            match delta.pos {
                None => {
                    textures.insert(
                        *id,
                        Texture {
                            size: image.size,
                            pixels: image.pixels.clone(),
                        },
                    );
                }
                Some([left, top]) => {
                    let texture = textures
                        .get_mut(id)
                        .expect("a patch of a texture never set");
                    for y in 0..image.size[1] {
                        for x in 0..image.size[0] {
                            texture.pixels[(top + y) * texture.size[0] + left + x] =
                                image.pixels[y * image.size[0] + x];
                        }
                    }
                }
            }
        }
        if let Some(update) = output.platform_output.accesskit_update {
            accessible = update.nodes.into_iter().map(|(_, node)| node).collect();
        }
        primitives = ctx.tessellate(output.shapes, output.pixels_per_point);
    }

    let width = (size.x * scale).round() as usize;
    let height = (size.y * scale).round() as usize;
    let mut picture = Picture {
        width,
        height,
        rgba: vec![[0.0; 4]; width * height],
        accessible,
    };
    if paint {
        for clipped in &primitives {
            if let egui::epaint::Primitive::Mesh(mesh) = &clipped.primitive {
                if let Some(texture) = textures.get(&mesh.texture_id) {
                    picture.mesh(mesh, texture, clipped.clip_rect, scale);
                }
            }
        }
    }
    picture
}

impl Picture {
    fn mesh(&mut self, mesh: &egui::Mesh, texture: &Texture, clip: egui::Rect, scale: f32) {
        let left = ((clip.min.x * scale).floor().max(0.0)) as usize;
        let top = ((clip.min.y * scale).floor().max(0.0)) as usize;
        let right = ((clip.max.x * scale).ceil() as usize).min(self.width);
        let bottom = ((clip.max.y * scale).ceil() as usize).min(self.height);
        for triangle in mesh.indices.as_chunks::<3>().0 {
            let [a, b, c] = [0, 1, 2].map(|i| &mesh.vertices[triangle[i] as usize]);
            let [pa, pb, pc] = [a, b, c].map(|v| v.pos * scale);
            let edge = |p: egui::Pos2, q: egui::Pos2, r: egui::Pos2| {
                (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x)
            };
            let area = edge(pa, pb, pc);
            if area.abs() < 1e-9 {
                continue;
            }
            let x0 = (pa.x.min(pb.x).min(pc.x).floor().max(0.0) as usize).max(left);
            let y0 = (pa.y.min(pb.y).min(pc.y).floor().max(0.0) as usize).max(top);
            let x1 = (pa.x.max(pb.x).max(pc.x).ceil() as usize + 1).min(right);
            let y1 = (pa.y.max(pb.y).max(pc.y).ceil() as usize + 1).min(bottom);
            let colour = |v: &egui::epaint::Vertex| {
                [v.color.r(), v.color.g(), v.color.b(), v.color.a()].map(|c| c as f32 / 255.0)
            };
            let (ca, cb, cc) = (colour(a), colour(b), colour(c));
            for y in y0..y1 {
                for x in x0..x1 {
                    // A hair off the pixel's centre, so a point never lies exactly on the edge two
                    // triangles share, and is painted by one of them rather than by both or neither.
                    let p = egui::pos2(x as f32 + 0.500_37, y as f32 + 0.500_61);
                    let wa = edge(pb, pc, p) / area;
                    let wb = edge(pc, pa, p) / area;
                    let wc = 1.0 - wa - wb;
                    if wa < 0.0 || wb < 0.0 || wc < 0.0 {
                        continue;
                    }
                    let uv = egui::pos2(
                        a.uv.x * wa + b.uv.x * wb + c.uv.x * wc,
                        a.uv.y * wa + b.uv.y * wb + c.uv.y * wc,
                    );
                    let texel = texture.sample(uv);
                    let source: [f32; 4] =
                        std::array::from_fn(|i| (ca[i] * wa + cb[i] * wb + cc[i] * wc) * texel[i]);
                    let pixel = &mut self.rgba[y * self.width + x];
                    for i in 0..4 {
                        pixel[i] = source[i] + pixel[i] * (1.0 - source[3]);
                    }
                }
            }
        }
    }

    /// Written as an 8-bit RGBA PNG, compressed by the deflate this program already carries.
    pub fn save(&self, path: &Path) {
        let mut raw = Vec::with_capacity((self.width * 4 + 1) * self.height);
        for row in self.rgba.chunks(self.width) {
            raw.push(0);
            for pixel in row {
                raw.extend(pixel.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8));
            }
        }
        let mut deflated =
            flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        deflated.write_all(&raw).unwrap();
        let mut header = Vec::new();
        header.extend((self.width as u32).to_be_bytes());
        header.extend((self.height as u32).to_be_bytes());
        header.extend([8, 6, 0, 0, 0]);
        let mut file = b"\x89PNG\r\n\x1a\n".to_vec();
        for (kind, data) in [
            (b"IHDR", header),
            (b"IDAT", deflated.finish().unwrap()),
            (b"IEND", Vec::new()),
        ] {
            file.extend((data.len() as u32).to_be_bytes());
            let mut crc = flate2::Crc::new();
            crc.update(kind);
            crc.update(&data);
            file.extend(kind);
            file.extend(&data);
            file.extend(crc.sum().to_be_bytes());
        }
        std::fs::write(path, file).unwrap();
    }

    /// The checkboxes, as a screen reader is told of them: the label, whether it is ticked, and whether
    /// it can be.
    fn checkboxes(&self) -> Vec<(String, bool, bool)> {
        self.accessible
            .iter()
            .filter(|node| node.role() == Role::CheckBox)
            .map(|node| {
                (
                    node.label().unwrap_or_default().to_string(),
                    node.toggled() == Some(Toggled::True),
                    !node.is_disabled(),
                )
            })
            .collect()
    }

    fn says(&self, text: &str) -> bool {
        self.accessible.iter().any(|node| {
            node.label().is_some_and(|l| l.contains(text))
                || node.value().is_some_and(|v| v.contains(text))
        })
    }
}

// ---------------------------------------------------------------- stages to draw

const SUMS: &str = "\
1111111111111111111111111111111111111111111111111111111111111111  Noctorium-0.9.1-windows-x64.msi
2222222222222222222222222222222222222222222222222222222222222222  Noctorium-0.9.1-windows-x64-setup.exe
3333333333333333333333333333333333333333333333333333333333333333  noctorium-cli-0.9.1-windows-x64.zip
4444444444444444444444444444444444444444444444444444444444444444  noctorium_0.9.1_amd64.deb
5555555555555555555555555555555555555555555555555555555555555555  Noctorium-0.9.1-x86_64.AppImage
6666666666666666666666666666666666666666666666666666666666666666  noctorium-cli-0.9.1-linux-x64.tar.gz
7777777777777777777777777777777777777777777777777777777777777777  Noctorium-0.9.1-x86_64.flatpak
";

/// v0.9.1 as it was published, sizes and all, but at addresses that go nowhere.
fn release() -> Release {
    let asset = |name: &str, size: u64| Asset {
        name: name.into(),
        url: format!("https://example.test/{name}"),
        size,
    };
    Release {
        tag: "v0.9.1".into(),
        assets: vec![
            asset("Noctorium-0.9.1-windows-x64.msi", 343_048_105),
            asset("Noctorium-0.9.1-windows-x64-setup.exe", 343_778_304),
            asset("noctorium-cli-0.9.1-windows-x64.zip", 62_723_902),
            asset("noctorium_0.9.1_amd64.deb", 254_526_766),
            asset("Noctorium-0.9.1-x86_64.AppImage", 264_321_528),
            asset("noctorium-cli-0.9.1-linux-x64.tar.gz", 66_452_296),
            asset("Noctorium-0.9.1-x86_64.flatpak", 251_243_192),
            asset("SHA256SUMS.txt", 2078),
        ],
    }
}

fn system(os: Os) -> System {
    System {
        os,
        arch: Some(Arch::X86_64),
        arch_name: "x86_64",
        release: None,
        mac_version: None,
        family: None,
        package_manager: (os == Os::Linux)
            .then_some(noctorium_installer::system::PackageManager::Apt),
        flatpak: os == Os::Linux,
        mpv: true,
        // So that a Linux plan drawn on any machine needs nobody's password, which on Windows it
        // could not find a way to ask for.
        root: os == Os::Linux,
        sudo_user: None,
    }
}

fn found(os: Os) -> Found {
    Found {
        release: release(),
        checksums: Some(SUMS.into()),
        latest: true,
        system: system(os),
        places: Places {
            home: Some(PathBuf::from(if os == Os::Windows {
                r"C:\Users\Sam"
            } else {
                "/home/sam"
            })),
            data: Some("/home/sam/.local/share".into()),
            programs: Some(r"C:\Users\Sam\AppData\Local\Programs".into()),
            installed: (os == Os::Windows).then(|| PathBuf::from(r"C:\Program Files\Noctorium\")),
            ..Places::default()
        },
    }
}

/// The window, at a stage made up for it, with nothing running behind it.
fn gui_at(ctx: &egui::Context, stage: impl FnOnce(&Gui) -> Stage) -> Gui {
    let mut gui = Gui::new(ctx, Arc::new(AtomicBool::new(false)));
    gui.stage = stage(&gui);
    gui
}

/// Choosing, as it would be on a machine that has downloaded nothing yet -- whatever this one's temporary
/// folder happens to hold.
fn choosing(os: Os, wanted: Wanted) -> Stage {
    let mut choosing = Choosing::new(found(os), wanted);
    if let Ok(plan) = &mut choosing.plan {
        for item in &mut plan.items {
            item.on_disk = OnDisk::Nothing;
        }
    }
    Stage::Ready(Box::new(choosing))
}

fn working(rows: Vec<Row>, finished: bool, wizard: bool) -> Stage {
    let options = Options {
        products: if rows.len() > 1 {
            Products::Both
        } else {
            Products::Desktop
        },
        wizard,
        ..Options::default()
    };
    let plan = flow::plan(&found(Os::Windows), options).expect("plans");
    let mut working = Working::new(plan);
    working.rows = rows;
    working.finished = finished;
    Stage::Working(Box::new(working))
}

/// A stage, made when it is drawn, so each drawing starts from nothing.
type MakeStage = Box<dyn Fn() -> Stage>;

/// Every stage worth looking at, by the name its picture is saved under.
fn stages() -> Vec<(&'static str, MakeStage)> {
    let mb = |n: u64| n * 1_048_576;
    vec![
        ("01-looking", Box::new(|| Stage::Looking)),
        (
            "02-ready-windows",
            Box::new(|| choosing(Os::Windows, Wanted::default())),
        ),
        (
            "03-ready-windows-both-wizard",
            Box::new(|| {
                choosing(
                    Os::Windows,
                    Wanted {
                        desktop: true,
                        cli: true,
                        wizard: true,
                    },
                )
            }),
        ),
        (
            "04-ready-cli-only",
            Box::new(|| {
                choosing(
                    Os::Windows,
                    Wanted {
                        desktop: false,
                        cli: true,
                        wizard: false,
                    },
                )
            }),
        ),
        (
            "05-ready-nothing",
            Box::new(|| {
                choosing(
                    Os::Windows,
                    Wanted {
                        desktop: false,
                        cli: false,
                        wizard: false,
                    },
                )
            }),
        ),
        (
            "06-ready-linux-both",
            Box::new(|| {
                choosing(
                    Os::Linux,
                    Wanted {
                        desktop: true,
                        cli: true,
                        wizard: false,
                    },
                )
            }),
        ),
        (
            "07-downloading-both",
            Box::new(move || {
                working(
                    vec![
                        Row::Downloading {
                            done: mb(131),
                            total: 343_048_105,
                        },
                        Row::Downloading {
                            done: mb(37),
                            total: 62_723_902,
                        },
                    ],
                    false,
                    false,
                )
            }),
        ),
        (
            "08-cli-installing-while-noctorium-downloads",
            Box::new(move || {
                working(
                    vec![
                        Row::Downloading {
                            done: mb(170),
                            total: 343_048_105,
                        },
                        Row::Installing,
                    ],
                    false,
                    false,
                )
            }),
        ),
        (
            "09-installing-noctorium",
            Box::new(|| working(vec![Row::Installing, Row::Installed(None)], false, false)),
        ),
        (
            "10-both-installed",
            Box::new(|| {
                working(
                    vec![Row::Installed(None), Row::Installed(None)],
                    true,
                    false,
                )
            }),
        ),
        (
            "11-one-failed",
            Box::new(|| {
                working(
                    vec![
                        Row::Failed("The install was cancelled, and nothing was changed.".into()),
                        Row::Installed(None),
                    ],
                    true,
                    false,
                )
            }),
        ),
        (
            "12-noctorium-installed",
            Box::new(|| {
                working(
                    vec![Row::Installed(Some(
                        "Windows says it needs restarting to finish installing it. Noctorium can be \
                         started before then."
                            .into(),
                    ))],
                    true,
                    false,
                )
            }),
        ),
        (
            "13-failed",
            Box::new(|| {
                Stage::Failed(
                    "Noctorium is open. Quit it -- from its menu, or from its icon by the clock if it \
                     is still there -- and run this again: Windows cannot replace a program while it \
                     is running. Nothing has been changed."
                        .into(),
                )
            }),
        ),
    ]
}

fn draw(name: &str, stage: &dyn Fn() -> Stage, paint: bool) -> Picture {
    let mut gui: Option<Gui> = None;
    let size = if name.contains("linux") {
        egui::vec2(470.0, 520.0)
    } else {
        egui::vec2(SIZE[0], 470.0)
    };
    render(size, 1.5, paint, |ctx| {
        let gui = gui.get_or_insert_with(|| gui_at(ctx, |_| stage()));
        gui.frame(ctx);
    })
}

#[test]
#[ignore = "writes pictures; run with --ignored to look at the window"]
fn the_window_drawn_at_every_stage() {
    let folder = std::env::var_os("NOCTORIUM_RENDER_TO")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("noctorium-installer-render"));
    std::fs::create_dir_all(&folder).unwrap();
    for (name, stage) in stages() {
        let picture = draw(name, stage.as_ref(), true);
        let path = folder.join(format!("{name}.png"));
        picture.save(&path);
        println!("{}", path.display());
    }
}

#[test]
fn every_stage_lays_out() {
    for (name, stage) in stages() {
        let picture = draw(name, stage.as_ref(), false);
        assert!(
            !picture.accessible.is_empty(),
            "{name} told a screen reader nothing"
        );
    }
}

/// The choice, as somebody who cannot see it is told of it: two boxes, each saying what it is and how
/// big, Noctorium ticked to begin with -- and on Windows a third, for the folder, not ticked.
#[test]
fn the_choice_of_products_is_told_to_a_screen_reader_whole() {
    let picture = draw("ready", &|| choosing(Os::Windows, Wanted::default()), false);
    let boxes = picture.checkboxes();
    assert_eq!(boxes.len(), 3, "{boxes:?}");
    // Found by what they say, since the tree is not handed over in the order it is drawn.
    let find = |test: &dyn Fn(&str) -> bool| {
        boxes
            .iter()
            .find(|(label, _, _)| test(label))
            .unwrap_or_else(|| panic!("{boxes:?}"))
            .clone()
    };
    let (desktop, ticked, enabled) =
        find(&|label| label.starts_with("Noctorium ") && !label.starts_with("Noctorium CLI"));
    assert!(
        desktop.contains("the music player") && desktop.contains("327 MB"),
        "{desktop}"
    );
    assert!(ticked && enabled);
    let (cli, ticked, enabled) = find(&|label| label.starts_with("Noctorium CLI"));
    assert!(cli.contains("60 MB"), "{cli}");
    assert!(!ticked && enabled);
    let (_, ticked, _) = find(&|label| label.contains("Choose the folder"));
    assert!(!ticked);
    assert!(picture.says("Install Noctorium"));
    assert!(picture.says("327 MB to download"));
}

#[test]
fn both_chosen_says_both_and_their_size_together() {
    let both = Wanted {
        desktop: true,
        cli: true,
        wizard: false,
    };
    let picture = draw("both", &|| choosing(Os::Windows, both), false);
    assert!(picture.says("Install both"));
    assert!(picture.says("387 MB to download, both at once"));

    let none = Wanted {
        desktop: false,
        cli: false,
        wizard: false,
    };
    let picture = draw("none", &|| choosing(Os::Windows, none), false);
    assert!(picture.says("Choose Noctorium, the Noctorium CLI, or both."));
    // Nothing to choose the folder of.
    assert_eq!(picture.checkboxes().len(), 2);
}

#[test]
fn each_product_says_how_it_ended_and_how_to_start_it() {
    let picture = draw(
        "done",
        &|| {
            working(
                vec![Row::Installed(None), Row::Installed(None)],
                true,
                false,
            )
        },
        false,
    );
    assert!(picture.says("Both are installed."));
    assert!(picture.says("Start it from the Start menu"));
    assert!(picture.says("Open a new terminal"));

    let picture = draw(
        "partly",
        &|| {
            working(
                vec![
                    Row::Failed("The install was cancelled, and nothing was changed.".into()),
                    Row::Installed(None),
                ],
                true,
                false,
            )
        },
        false,
    );
    assert!(picture.says("That did not all work."));
    assert!(picture.says("Noctorium was not installed."));
    assert!(picture.says("The install was cancelled"));
    assert!(picture.says("Noctorium CLI is installed."));
    assert!(picture.says("Try again"));
}
