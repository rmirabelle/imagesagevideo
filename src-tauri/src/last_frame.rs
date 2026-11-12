use base64::{engine::general_purpose::STANDARD, Engine as _};
use image::imageops;

use crate::webm;

/**
 * The picture that the next clip of a layer starts from: the last picture
 * of the clip before it, as a PNG data URL at the clip size.
 *
 * The engine makes each picture of a clip with its WebP encoder and puts
 * them in a WebM file as VP8 key pictures. A VP8 key picture in a small
 * WebP wrapper is a normal WebP file, so the WebP decoder reads it. The
 * picture stays in memory; it is never written to the disk.
 */
pub fn start_picture(clip: &[u8], width: u32, height: u32) -> Result<String, String> {
    let webp = as_webp(webm::last_picture(clip)?);
    let fail = |error: image::ImageError| format!("Could not read the last picture of the clip before this one: {error}");
    let mut picture = image::load_from_memory_with_format(&webp, image::ImageFormat::WebP).map_err(fail)?.into_rgb8();
    if picture.dimensions() != (width, height) {
        picture = imageops::resize(&picture, width, height, imageops::FilterType::Lanczos3);
    }
    let mut png = Vec::new();
    picture.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).map_err(fail)?;
    Ok(format!("data:image/png;base64,{}", STANDARD.encode(png)))
}

/// A WebP file that holds one VP8 key picture. A part of odd length gets one byte of padding.
fn as_webp(picture: &[u8]) -> Vec<u8> {
    let padding = picture.len() % 2;
    let mut file = Vec::with_capacity(picture.len() + padding + 20);
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&((4 + 8 + picture.len() + padding) as u32).to_le_bytes());
    file.extend_from_slice(b"WEBPVP8 ");
    file.extend_from_slice(&(picture.len() as u32).to_le_bytes());
    file.extend_from_slice(picture);
    file.resize(file.len() + padding, 0);
    file
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wrapper_is_a_webp_file() {
        let file = as_webp(&[1, 2, 3]);
        assert_eq!(&file[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(file[4..8].try_into().unwrap()) as usize, file.len() - 8);
        assert_eq!(&file[8..16], b"WEBPVP8 ");
        assert_eq!(u32::from_le_bytes(file[16..20].try_into().unwrap()), 3);
        assert_eq!(file.len() % 2, 0);
    }

    /// A real VP8 key picture, 32 x 16: the left half is red and the right half is blue.
    const PICTURE: [u8; 70] = [
        0xD0, 0x03, 0x00, 0x9D, 0x01, 0x2A, 0x20, 0x00, 0x10, 0x00, 0x3E, 0xB5, 0x4C, 0xA1, 0x4A, 0xA7, 0x24, 0x23, 0x21, 0xB0, 0x08, 0x00, 0xE0, 0x16,
        0x89, 0x68, 0x00, 0x9D, 0x32, 0x84, 0x70, 0x37, 0xA0, 0x7E, 0x2A, 0x80, 0x86, 0x75, 0x00, 0x00, 0xFE, 0xEA, 0x65, 0xFD, 0xD9, 0x8D, 0x0C, 0xFF,
        0xF9, 0x25, 0xFD, 0x3F, 0x9D, 0x9F, 0xF6, 0x55, 0xDB, 0x07, 0x16, 0x70, 0x3F, 0x8E, 0xBF, 0xE6, 0x77, 0x1C, 0x62, 0x04, 0x80, 0x00,
    ];

    fn element(id: &[u8], data: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        out.push(0x01);
        out.extend_from_slice(&(data.len() as u64).to_be_bytes()[1..]);
        out.extend_from_slice(data);
        out
    }

    /// A VP8 WebM clip of two pictures; the last one is `PICTURE`.
    fn clip() -> Vec<u8> {
        let block = |time: u8, picture: &[u8]| element(&[0xA3], &[&[0x81, 0x00, time, 0x80], picture].concat());
        let video = [element(&[0xB0], &[32]), element(&[0xBA], &[16])].concat();
        let entry = [element(&[0xD7], &[1]), element(&[0x86], b"V_VP8"), element(&[0xE0], &video)].concat();
        let cluster = [element(&[0xE7], &[0]), block(0, &[0u8; 12]), block(62, &PICTURE)].concat();
        let segment = [element(&[0x16, 0x54, 0xAE, 0x6B], &element(&[0xAE], &entry)), element(&[0x1F, 0x43, 0xB6, 0x75], &cluster)].concat();
        [element(&[0x1A, 0x45, 0xDF, 0xA3], &element(&[0x42, 0x82], b"webm")), element(&[0x18, 0x53, 0x80, 0x67], &segment)].concat()
    }

    #[test]
    fn the_next_clip_starts_from_the_last_picture() {
        let data_url = start_picture(&clip(), 64, 32).unwrap();
        let png = STANDARD.decode(data_url.strip_prefix("data:image/png;base64,").unwrap()).unwrap();
        let picture = image::load_from_memory(&png).unwrap().into_rgb8();
        assert_eq!(picture.dimensions(), (64, 32));
        let (left, right) = (picture.get_pixel(8, 16), picture.get_pixel(56, 16));
        assert!(left[0] > 150 && left[2] < 90, "the left half is red: {left:?}");
        assert!(right[2] > 150 && right[0] < 90, "the right half is blue: {right:?}");
    }

    /// Reads the last picture of a real clip: set ISV_LAST_FRAME_INPUT (a clip) and ISV_LAST_FRAME_OUTPUT (a PNG), then run with `--ignored`.
    #[test]
    #[ignore]
    fn reads_the_last_picture_of_a_real_clip() {
        let clip = std::fs::read(std::env::var("ISV_LAST_FRAME_INPUT").unwrap()).unwrap();
        let picture = image::load_from_memory_with_format(&as_webp(webm::last_picture(&clip).unwrap()), image::ImageFormat::WebP).unwrap();
        println!("the last picture is {} x {}", picture.width(), picture.height());
        picture.save(std::env::var("ISV_LAST_FRAME_OUTPUT").unwrap()).unwrap();
    }
}
