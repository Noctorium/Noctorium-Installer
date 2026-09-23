//! Turns the mark into pixels before the program is compiled.
//!
//! The window shows Noctorium's own artwork, which means the binary needs it as raw pixels. Decoding it
//! here rather than at startup keeps the PNG decoder out of the finished program: it would be several
//! hundred kilobytes carried for one image read once, and a build dependency costs the download nothing.

fn main() {
    let source = "assets/noctorium-mark-128.png";
    println!("cargo:rerun-if-changed={source}");

    let file = std::fs::File::open(source).expect("the mark is missing from assets/");
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder.read_info().expect("the mark is not a PNG");

    // Read before the buffer is allocated, because next_frame borrows the reader for itself. The mark is
    // saved as 8-bit RGBA, which is what egui wants, so this asserts rather than converts -- a mark in
    // some other format is a mistake to fix in the file, not to paper over here.
    let (width, height) = {
        let info = reader.info();
        assert!(
            matches!(info.color_type, png::ColorType::Rgba),
            "the mark must be RGBA, not {:?}",
            info.color_type
        );
        assert!(
            matches!(info.bit_depth, png::BitDepth::Eight),
            "the mark must be 8 bits a channel, not {:?}",
            info.bit_depth
        );
        assert_eq!(info.width, info.height, "the mark must be square");
        (info.width as usize, info.height as usize)
    };

    let mut pixels = vec![0u8; width * height * 4];
    reader
        .next_frame(&mut pixels)
        .expect("the mark could not be read");

    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("cargo sets OUT_DIR"));
    std::fs::write(out.join("mark.rgba"), &pixels).expect("could not write the decoded mark");
    std::fs::write(
        out.join("mark.rs"),
        format!("/// The mark is square; this is its side in pixels.\npub const MARK_SIDE: usize = {width};\n"),
    )
    .expect("could not write the mark's size");
}
