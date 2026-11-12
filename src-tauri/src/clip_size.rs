/**
 * Size and length rules for the clips, the same as Image Sage's Animate
 * Layer. A clip has the shape of the document. Its sides are multiples of
 * 16, and its area stays within 832 x 480, the area Wan 2.2 learned from
 * for a small clip.
 */
const SIZE_MULTIPLE: u32 = 16;
const MAX_ASPECT: f64 = 3.0;
/// The largest area of a small clip, in pixels.
pub const SMALL_PIXELS: u32 = 832 * 480;
/// Wan 2.2 makes 16 pictures for each second.
pub const FPS: u32 = 16;
/// Each clip is 5 seconds, the longest that Wan 2.2 learned from.
pub const CLIP_SECONDS: u32 = 5;
/// How many pictures one clip has: Wan 2.2 needs a multiple of 4, plus 1.
pub const CLIP_FRAMES: u32 = CLIP_SECONDS * FPS + 1;

/**
 * The largest clip size of this shape within `pixels`. None when the shape
 * is wider or taller than 3:1. Each height that is a multiple of 16 gets the
 * nearest width that is one, so the shape changes by one percent or two at most.
 */
pub fn clip_size(width: u32, height: u32, pixels: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    let aspect = f64::from(width) / f64::from(height);
    if aspect.max(1.0 / aspect) > MAX_ASPECT {
        return None;
    }
    let multiple = f64::from(SIZE_MULTIPLE);
    let round = |side: f64| ((side / multiple).round() as u32 * SIZE_MULTIPLE).max(SIZE_MULTIPLE);
    let tallest = (((f64::from(pixels) / aspect).sqrt() / multiple + 1.0).floor() as u32) * SIZE_MULTIPLE;
    let mut clip_height = tallest;
    while clip_height >= SIZE_MULTIPLE {
        let clip_width = round(f64::from(clip_height) * aspect);
        if clip_width * clip_height <= pixels {
            return Some((clip_width, clip_height));
        }
        clip_height -= SIZE_MULTIPLE;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_clip_keeps_the_shape_within_the_area() {
        assert_eq!(clip_size(1920, 1080, SMALL_PIXELS), Some((832, 464)));
        assert_eq!(clip_size(1024, 1024, SMALL_PIXELS), Some((624, 624)));
        assert_eq!(clip_size(1080, 1920, SMALL_PIXELS), Some((464, 832)));
        for (width, height) in [(1536, 1024), (1024, 1536), (3000, 1000), (640, 480)] {
            let (clip_width, clip_height) = clip_size(width, height, SMALL_PIXELS).unwrap();
            assert_eq!((clip_width % 16, clip_height % 16), (0, 0));
            assert!(clip_width * clip_height <= SMALL_PIXELS);
        }
    }

    #[test]
    fn a_very_wide_or_empty_picture_has_no_clip_size() {
        assert_eq!(clip_size(4000, 1000, SMALL_PIXELS), None);
        assert_eq!(clip_size(1000, 4000, SMALL_PIXELS), None);
        assert_eq!(clip_size(0, 100, SMALL_PIXELS), None);
    }

    #[test]
    fn a_clip_has_81_pictures() {
        assert_eq!(CLIP_FRAMES, 81);
        assert_eq!(CLIP_FRAMES % 4, 1);
    }
}
