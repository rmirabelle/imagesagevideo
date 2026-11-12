/**
 * Joins WebM clips into one WebM file, one after the other, with no fade
 * between them. The pictures are not encoded again: each clip's blocks are
 * copied as they are, and only their times move. So the join is fast and
 * loses no quality, and no clip goes to the disk in another form.
 *
 * The clips must have the same tracks (codec and picture size); the clips
 * of one document do. The result has the first clip's tracks, a seek index
 * (Cues) with one entry for each cluster that starts with a key picture,
 * and the total duration.
 *
 * When each clip continues the clip before it, the first picture of a clip
 * is the last picture of the clip before. The join can leave that repeated
 * picture out, so the video does not stop for a moment where two clips meet.
 */
const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const SEEK_HEAD: u32 = 0x114D_9B74;
const SEEK: u32 = 0x4DBB;
const SEEK_ID: u32 = 0x53AB;
const SEEK_POSITION: u32 = 0x53AC;
const INFO: u32 = 0x1549_A966;
const TIMECODE_SCALE: u32 = 0x2A_D7B1;
const DURATION: u32 = 0x4489;
const MUXING_APP: u32 = 0x4D80;
const WRITING_APP: u32 = 0x5741;
const TRACKS: u32 = 0x1654_AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_NUMBER: u32 = 0xD7;
const CODEC_ID: u32 = 0x86;
const CODEC_PRIVATE: u32 = 0x63A2;
const VIDEO: u32 = 0xE0;
const PIXEL_WIDTH: u32 = 0xB0;
const PIXEL_HEIGHT: u32 = 0xBA;
const CLUSTER: u32 = 0x1F43_B675;
const TIMECODE: u32 = 0xE7;
const SIMPLE_BLOCK: u32 = 0xA3;
const BLOCK_GROUP: u32 = 0xA0;
const BLOCK: u32 = 0xA1;
const REFERENCE_BLOCK: u32 = 0xFB;
const CUES: u32 = 0x1C53_BB6B;
const CUE_POINT: u32 = 0xBB;
const CUE_TIME: u32 = 0xB3;
const CUE_TRACK_POSITIONS: u32 = 0xB7;
const CUE_TRACK: u32 = 0xF7;
const CUE_CLUSTER_POSITION: u32 = 0xF1;
const TAGS: u32 = 0x1254_C367;
const ATTACHMENTS: u32 = 0x1941_A469;
const CHAPTERS: u32 = 0x1043_A770;

const DEFAULT_TIMECODE_SCALE: u64 = 1_000_000;
const APP_NAME: &str = "Image Sage Video";

/// The elements that sit directly in a Segment; one of them ends a cluster of unknown size.
const SEGMENT_CHILDREN: [u32; 8] = [SEEK_HEAD, INFO, TRACKS, CLUSTER, CUES, TAGS, ATTACHMENTS, CHAPTERS];

fn damaged() -> String {
    "A clip is not a valid WebM file, so the clips cannot be joined.".into()
}

/// One element: its id, and where its data starts and ends in the file.
struct Element {
    id: u32,
    start: usize,
    end: usize,
    /// Where the element itself starts (its id).
    head: usize,
}

/// Reads an element id at `at`: 1 to 4 bytes, with the length marker kept.
fn read_id(bytes: &[u8], at: usize) -> Result<(u32, usize), String> {
    let first = *bytes.get(at).ok_or_else(damaged)?;
    let length = first.leading_zeros() as usize + 1;
    if length > 4 || at + length > bytes.len() {
        return Err(damaged());
    }
    let id = bytes[at..at + length].iter().fold(0_u32, |id, &byte| (id << 8) | u32::from(byte));
    Ok((id, length))
}

/// Reads an element size at `at`: 1 to 8 bytes. None is the "unknown" size that a live stream uses.
fn read_size(bytes: &[u8], at: usize) -> Result<(Option<u64>, usize), String> {
    let first = *bytes.get(at).ok_or_else(damaged)?;
    let length = first.leading_zeros() as usize + 1;
    if length > 8 || at + length > bytes.len() {
        return Err(damaged());
    }
    let mut value = u64::from(first) & (0xFF_u64 >> length);
    for &byte in &bytes[at + 1..at + length] {
        value = (value << 8) | u64::from(byte);
    }
    let unknown = value == (1_u64 << (7 * length)) - 1;
    Ok((if unknown { None } else { Some(value) }, length))
}

/**
 * The elements in `bytes[start..end]`, in order. An element of unknown size
 * runs to the next element that `ends_unknown` names, or to `end`.
 */
fn children(bytes: &[u8], start: usize, end: usize, ends_unknown: &[u32]) -> Result<Vec<Element>, String> {
    let mut elements = Vec::new();
    let mut at = start;
    while at < end {
        let (id, id_length) = read_id(bytes, at)?;
        let (size, size_length) = read_size(bytes, at + id_length)?;
        let data = at + id_length + size_length;
        let data_end = match size {
            Some(size) => data.checked_add(usize::try_from(size).map_err(|_| damaged())?).ok_or_else(damaged)?,
            None if ends_unknown.is_empty() => end,
            None => {
                /* Look through the unknown-size element's own children for the first one that belongs to its parent. */
                let mut inner = data;
                loop {
                    if inner >= end {
                        break end;
                    }
                    let (inner_id, inner_id_length) = read_id(bytes, inner)?;
                    if ends_unknown.contains(&inner_id) {
                        break inner;
                    }
                    let (inner_size, inner_size_length) = read_size(bytes, inner + inner_id_length)?;
                    let inner_size = usize::try_from(inner_size.ok_or_else(damaged)?).map_err(|_| damaged())?;
                    inner = (inner + inner_id_length + inner_size_length).checked_add(inner_size).ok_or_else(damaged)?;
                }
            }
        };
        if data_end > end {
            return Err(damaged());
        }
        elements.push(Element { id, start: data, end: data_end, head: at });
        at = data_end;
    }
    Ok(elements)
}

fn read_uint(bytes: &[u8]) -> u64 {
    bytes.iter().take(8).fold(0_u64, |value, &byte| (value << 8) | u64::from(byte))
}

fn read_float(bytes: &[u8]) -> Option<f64> {
    match bytes.len() {
        4 => Some(f64::from(f32::from_be_bytes(bytes.try_into().ok()?))),
        8 => Some(f64::from_be_bytes(bytes.try_into().ok()?)),
        _ => None,
    }
}

/// What must be the same in two clips before their blocks can share one track list.
#[derive(PartialEq, Debug)]
struct TrackShape {
    number: u64,
    codec: Vec<u8>,
    codec_private: Vec<u8>,
    width: u64,
    height: u64,
}

fn track_shapes(bytes: &[u8], tracks: &Element) -> Result<Vec<TrackShape>, String> {
    let mut shapes = Vec::new();
    for entry in children(bytes, tracks.start, tracks.end, &[])?.into_iter().filter(|element| element.id == TRACK_ENTRY) {
        let mut shape = TrackShape { number: 0, codec: Vec::new(), codec_private: Vec::new(), width: 0, height: 0 };
        for field in children(bytes, entry.start, entry.end, &[])? {
            let data = &bytes[field.start..field.end];
            match field.id {
                TRACK_NUMBER => shape.number = read_uint(data),
                CODEC_ID => shape.codec = data.to_vec(),
                CODEC_PRIVATE => shape.codec_private = data.to_vec(),
                VIDEO => {
                    for part in children(bytes, field.start, field.end, &[])? {
                        match part.id {
                            PIXEL_WIDTH => shape.width = read_uint(&bytes[part.start..part.end]),
                            PIXEL_HEIGHT => shape.height = read_uint(&bytes[part.start..part.end]),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        shapes.push(shape);
    }
    Ok(shapes)
}

/// The track number, the time in the cluster, and the key-picture flag of a block (the data of a SimpleBlock or a Block).
fn block_head(data: &[u8]) -> Result<(u64, i16, bool), String> {
    let (track, length) = read_size(data, 0)?;
    let rest = data.get(length..length + 3).ok_or_else(damaged)?;
    Ok((track.ok_or_else(damaged)?, i16::from_be_bytes([rest[0], rest[1]]), rest[2] & 0x80 != 0))
}

/// One picture of a clip, as it is in the file.
struct Block<'a> {
    /// The SimpleBlock or BlockGroup element, whole.
    whole: &'a [u8],
    /// The data of the SimpleBlock or the Block: its head, then the picture.
    data: &'a [u8],
    track: u64,
    /// The time in the cluster.
    time: i16,
    key: bool,
}

/// One cluster of a clip: its time, and its blocks.
struct Cluster<'a> {
    time: u64,
    blocks: Vec<Block<'a>>,
    /// The time of the last block, from the start of the clip.
    last_time: u64,
    block_count: u64,
}

struct Clip<'a> {
    header: &'a [u8],
    timecode_scale: u64,
    duration: Option<f64>,
    tracks: &'a [u8],
    shapes: Vec<TrackShape>,
    clusters: Vec<Cluster<'a>>,
}

fn parse_cluster<'a>(bytes: &'a [u8], cluster: &Element) -> Result<Cluster<'a>, String> {
    let mut parsed = Cluster { time: 0, blocks: Vec::new(), last_time: 0, block_count: 0 };
    for child in children(bytes, cluster.start, cluster.end, &[])? {
        let data = &bytes[child.start..child.end];
        let (data, (track, time, key)) = match child.id {
            TIMECODE => {
                parsed.time = read_uint(data);
                continue;
            }
            SIMPLE_BLOCK => (data, block_head(data)?),
            BLOCK_GROUP => {
                let parts = children(bytes, child.start, child.end, &[])?;
                let block = parts.iter().find(|part| part.id == BLOCK).ok_or_else(damaged)?;
                let data = &bytes[block.start..block.end];
                let (track, time, _) = block_head(data)?;
                (data, (track, time, !parts.iter().any(|part| part.id == REFERENCE_BLOCK)))
            }
            /* The rest (the cluster's place in the old file, checksums, padding) does not fit the new file. */
            _ => continue,
        };
        parsed.blocks.push(Block { whole: &bytes[child.head..child.end], data, track, time, key });
    }
    parsed.block_count = parsed.blocks.len() as u64;
    parsed.last_time = parsed.blocks.iter().map(|block| parsed.block_time(block)).max().unwrap_or(parsed.time);
    Ok(parsed)
}

impl Cluster<'_> {
    /// A block's time from the start of the clip.
    fn block_time(&self, block: &Block<'_>) -> u64 {
        self.time.saturating_add_signed(i64::from(block.time))
    }
}

fn parse_clip(bytes: &[u8]) -> Result<Clip<'_>, String> {
    let top = children(bytes, 0, bytes.len(), &[])?;
    let header = top.iter().find(|element| element.id == EBML).ok_or_else(damaged)?;
    let segment = top.iter().find(|element| element.id == SEGMENT).ok_or_else(damaged)?;
    let mut clip = Clip {
        header: &bytes[header.head..header.end],
        timecode_scale: DEFAULT_TIMECODE_SCALE,
        duration: None,
        tracks: &[],
        shapes: Vec::new(),
        clusters: Vec::new(),
    };
    for element in children(bytes, segment.start, segment.end, &SEGMENT_CHILDREN)? {
        match element.id {
            INFO => {
                for field in children(bytes, element.start, element.end, &[])? {
                    let data = &bytes[field.start..field.end];
                    match field.id {
                        TIMECODE_SCALE => clip.timecode_scale = read_uint(data).max(1),
                        DURATION => clip.duration = read_float(data),
                        _ => {}
                    }
                }
            }
            TRACKS => {
                clip.tracks = &bytes[element.start..element.end];
                clip.shapes = track_shapes(bytes, &element)?;
            }
            CLUSTER => clip.clusters.push(parse_cluster(bytes, &element)?),
            _ => {}
        }
    }
    if clip.tracks.is_empty() || clip.clusters.iter().all(|cluster| cluster.blocks.is_empty()) {
        return Err(damaged());
    }
    Ok(clip)
}

impl Clip<'_> {
    /**
     * How long the clip is, in its own time units: the time of the last
     * picture plus the usual time between two pictures, or the file's
     * Duration when that is longer.
     */
    fn length(&self) -> u64 {
        let last = self.clusters.iter().map(|cluster| cluster.last_time).max().unwrap_or(0);
        let first = self.clusters.iter().filter(|cluster| !cluster.blocks.is_empty()).map(|cluster| cluster.time).min().unwrap_or(0);
        let blocks: u64 = self.clusters.iter().map(|cluster| cluster.block_count).sum();
        let step = if blocks > 1 { ((last - first.min(last)) as f64 / (blocks - 1) as f64).round() as u64 } else { 0 };
        /* Some encoders write the last picture's time as the Duration; the last picture then still needs its own time on screen. */
        let measured = last + step.max(1);
        match self.duration {
            Some(duration) if duration.is_finite() => (duration.round() as u64).max(measured),
            _ => measured,
        }
    }
}

fn write_id(out: &mut Vec<u8>, id: u32) {
    let bytes = id.to_be_bytes();
    let skip = bytes.iter().take_while(|&&byte| byte == 0).count();
    out.extend_from_slice(&bytes[skip..]);
}

/// Writes a size in 8 bytes, so the size of an element can be known before its data is written.
fn write_size(out: &mut Vec<u8>, size: u64) {
    out.push(0x01);
    out.extend_from_slice(&size.to_be_bytes()[1..]);
}

fn write_element(out: &mut Vec<u8>, id: u32, data: &[u8]) {
    write_id(out, id);
    write_size(out, data.len() as u64);
    out.extend_from_slice(data);
}

fn write_uint(out: &mut Vec<u8>, id: u32, value: u64) {
    write_element(out, id, &value.to_be_bytes());
}

fn id_bytes(id: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_id(&mut bytes, id);
    bytes
}

/// A SeekHead that names where the Info, the Tracks and the Cues start in the segment. Its size does not depend on the places.
fn seek_head(info: u64, tracks: u64, cues: u64) -> Vec<u8> {
    let mut entries = Vec::new();
    for (id, position) in [(INFO, info), (TRACKS, tracks), (CUES, cues)] {
        let mut seek = Vec::new();
        write_element(&mut seek, SEEK_ID, &id_bytes(id));
        write_uint(&mut seek, SEEK_POSITION, position);
        write_element(&mut entries, SEEK, &seek);
    }
    let mut head = Vec::new();
    write_element(&mut head, SEEK_HEAD, &entries);
    head
}

impl Clip<'_> {
    /**
     * The time between the first two pictures, when the first picture can be
     * left out: the second picture must be a key picture, so it needs no
     * picture before it.
     */
    fn first_step(&self) -> Option<u64> {
        let mut blocks = self.clusters.iter().flat_map(|cluster| cluster.blocks.iter().map(move |block| (cluster.block_time(block), block.key)));
        let (first, _) = blocks.next()?;
        let (second, key) = blocks.next()?;
        key.then(|| second.saturating_sub(first))
    }
}

/**
 * The last picture of a clip, as the codec's own bytes. Only VP8 is known
 * here, and the picture must be a key picture: one that is complete without
 * the pictures before it. The engine's clips are made of such pictures only.
 */
pub fn last_picture(bytes: &[u8]) -> Result<&[u8], String> {
    let clip = parse_clip(bytes)?;
    if clip.shapes.len() != 1 || clip.shapes[0].codec != b"V_VP8" {
        return Err("The clip's video format is not VP8, so its last picture cannot be read.".into());
    }
    let block = clip
        .clusters
        .iter()
        .flat_map(|cluster| cluster.blocks.iter().map(move |block| (cluster.block_time(block), block)))
        .max_by_key(|(time, _)| *time)
        .map(|(_, block)| block)
        .ok_or_else(damaged)?;
    let (_, track_length) = read_size(block.data, 0)?;
    let flags = *block.data.get(track_length + 2).ok_or_else(damaged)?;
    let picture = block.data.get(track_length + 3..).ok_or_else(damaged)?;
    /* Bits 1 and 2 of the flags: more than one picture in the block. A VP8 key picture has bit 0 clear and a start code. */
    let key_picture = picture.len() > 10 && picture[0] & 1 == 0 && picture[3..6] == [0x9D, 0x01, 0x2A];
    if flags & 0x06 != 0 || !key_picture {
        return Err("The clip's last picture is not complete by itself, so the next clip cannot start from it.".into());
    }
    Ok(picture)
}

/**
 * Joins the clips, in order, into the bytes of one WebM file. With
 * `continued`, each clip continues the clip before it, and its first
 * picture (the same as the last picture of the clip before) is left out.
 */
pub fn join(clips: &[Vec<u8>], continued: bool) -> Result<Vec<u8>, String> {
    let parsed = clips.iter().map(|bytes| parse_clip(bytes)).collect::<Result<Vec<_>, _>>()?;
    let first = parsed.first().ok_or("There are no clips to join.")?;
    if parsed.iter().any(|clip| clip.shapes != first.shapes || clip.timecode_scale != first.timecode_scale) {
        return Err("The clips do not have the same size and format, so they cannot be joined.".into());
    }
    /* For each clip, the time that its left-out first picture had on screen; 0 when the clip keeps its first picture. */
    let skips: Vec<u64> = parsed
        .iter()
        .enumerate()
        .map(|(index, clip)| if continued && index > 0 { clip.first_step().unwrap_or(0) } else { 0 })
        .collect();
    let total: u64 = parsed.iter().zip(&skips).map(|(clip, skip)| clip.length().saturating_sub(*skip)).sum();

    /* The segment's data: the SeekHead, the Info, the Tracks, every cluster, then the Cues. */
    let mut segment = seek_head(0, 0, 0);
    let seek_head_length = segment.len();

    let info_at = segment.len() as u64;
    let mut info = Vec::new();
    write_uint(&mut info, TIMECODE_SCALE, first.timecode_scale);
    write_element(&mut info, DURATION, &(total as f64).to_be_bytes());
    write_element(&mut info, MUXING_APP, APP_NAME.as_bytes());
    write_element(&mut info, WRITING_APP, APP_NAME.as_bytes());
    write_element(&mut segment, INFO, &info);

    let tracks_at = segment.len() as u64;
    write_element(&mut segment, TRACKS, first.tracks);

    let mut cues = Vec::new();
    let mut offset = 0_u64;
    for (clip, &skip) in parsed.iter().zip(&skips) {
        let mut leave_out = skip > 0;
        for cluster in &clip.clusters {
            let mut blocks = &cluster.blocks[..];
            if leave_out && !blocks.is_empty() {
                blocks = &blocks[1..];
                leave_out = false;
            }
            let Some(first_block) = blocks.first() else { continue };
            let time = (offset + cluster.time).saturating_sub(skip);
            if first_block.key {
                let track = first_block.track;
                let mut positions = Vec::new();
                write_uint(&mut positions, CUE_TRACK, track);
                write_uint(&mut positions, CUE_CLUSTER_POSITION, segment.len() as u64);
                let mut point = Vec::new();
                write_uint(&mut point, CUE_TIME, time);
                write_element(&mut point, CUE_TRACK_POSITIONS, &positions);
                write_element(&mut cues, CUE_POINT, &point);
            }
            let mut data = Vec::with_capacity(blocks.iter().map(|block| block.whole.len()).sum::<usize>() + 16);
            write_uint(&mut data, TIMECODE, time);
            for block in blocks {
                data.extend_from_slice(block.whole);
            }
            write_element(&mut segment, CLUSTER, &data);
        }
        offset += clip.length().saturating_sub(skip);
    }

    let cues_at = segment.len() as u64;
    write_element(&mut segment, CUES, &cues);
    segment[..seek_head_length].copy_from_slice(&seek_head(info_at, tracks_at, cues_at));

    let mut file = Vec::with_capacity(first.header.len() + segment.len() + 12);
    file.extend_from_slice(first.header);
    write_element(&mut file, SEGMENT, &segment);
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn short(id: u32, data: &[u8]) -> Vec<u8> {
        let mut out = id_bytes(id);
        out.push(0x80 | data.len() as u8);
        out.extend_from_slice(data);
        out
    }

    fn simple_block(time: i16, key: bool, payload: u8) -> Vec<u8> {
        let mut data = vec![0x81];
        data.extend_from_slice(&time.to_be_bytes());
        data.push(if key { 0x80 } else { 0 });
        data.push(payload);
        short(SIMPLE_BLOCK, &data)
    }

    /// A small WebM file: `frames` pictures, 62 units apart, in clusters of three; `live` uses unknown sizes as a live stream does.
    fn clip(width: u8, frames: i16, payload: u8, duration: bool, live: bool) -> Vec<u8> {
        let mut info = short(TIMECODE_SCALE, &[0x0F, 0x42, 0x40]);
        if duration {
            info.extend(short(DURATION, &(f32::from(frames) * 62.0).to_be_bytes()));
        }
        let video = [short(PIXEL_WIDTH, &[width]), short(PIXEL_HEIGHT, &[48])].concat();
        let entry = [short(TRACK_NUMBER, &[1]), short(CODEC_ID, b"V_VP9"), short(VIDEO, &video)].concat();
        let mut segment = [short(INFO, &info), short(TRACKS, &short(TRACK_ENTRY, &entry))].concat();
        for start in (0..frames).step_by(3) {
            let mut cluster = short(TIMECODE, &[(start * 62) as u8]);
            for frame in start..(start + 3).min(frames) {
                cluster.extend(simple_block((frame - start) * 62, frame == start, payload));
            }
            if live {
                segment.extend(id_bytes(CLUSTER));
                segment.push(0xFF);
                segment.extend(cluster);
            } else {
                segment.extend(short(CLUSTER, &cluster));
            }
        }
        let mut file = short(EBML, &short(0x4282, b"webm"));
        file.extend(id_bytes(SEGMENT));
        if live {
            file.push(0xFF);
        } else {
            write_size(&mut file, segment.len() as u64);
        }
        file.extend(segment);
        file
    }

    /// Every block of a file as (time from the start of the file, payload byte).
    fn block_times(bytes: &[u8]) -> Vec<(u64, u8)> {
        let parsed = parse_clip(bytes).unwrap();
        let mut times = Vec::new();
        for cluster in &parsed.clusters {
            for block in &cluster.blocks {
                times.push((cluster.block_time(block), block.data[4]));
            }
        }
        times
    }

    #[test]
    fn reads_ids_and_sizes() {
        assert_eq!(read_id(&[0x1A, 0x45, 0xDF, 0xA3], 0).unwrap(), (EBML, 4));
        assert_eq!(read_id(&[0xE7], 0).unwrap(), (TIMECODE, 1));
        assert_eq!(read_size(&[0x85], 0).unwrap(), (Some(5), 1));
        assert_eq!(read_size(&[0x40, 0x02], 0).unwrap(), (Some(2), 2));
        assert_eq!(read_size(&[0xFF], 0).unwrap(), (None, 1));
        assert_eq!(read_size(&[0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF], 0).unwrap(), (None, 8));
        assert!(read_id(&[0x00], 0).is_err());
    }

    #[test]
    fn joined_clips_play_one_after_the_other() {
        let joined = join(&[clip(64, 4, 1, true, false), clip(64, 4, 2, true, false), clip(64, 2, 3, true, false)], false).unwrap();
        let times = block_times(&joined);
        assert_eq!(
            times,
            vec![(0, 1), (62, 1), (124, 1), (186, 1), (248, 2), (310, 2), (372, 2), (434, 2), (496, 3), (558, 3)]
        );
        let parsed = parse_clip(&joined).unwrap();
        assert_eq!(parsed.duration, Some(620.0));
        assert_eq!(parsed.shapes[0].width, 64);
    }

    #[test]
    fn a_clip_with_no_duration_and_unknown_sizes_is_measured() {
        let live = clip(64, 4, 1, false, true);
        assert_eq!(parse_clip(&live).unwrap().length(), 248);
        let joined = join(&[live, clip(64, 1, 2, false, false)], false).unwrap();
        assert_eq!(block_times(&joined), vec![(0, 1), (62, 1), (124, 1), (186, 1), (248, 2)]);
    }

    #[test]
    fn the_seek_index_points_at_each_cluster() {
        let joined = join(&[clip(64, 4, 1, true, false), clip(64, 4, 2, true, false)], false).unwrap();
        let top = children(&joined, 0, joined.len(), &[]).unwrap();
        let segment = top.iter().find(|element| element.id == SEGMENT).unwrap();
        let parts = children(&joined, segment.start, segment.end, &SEGMENT_CHILDREN).unwrap();
        assert_eq!(parts.iter().map(|part| part.id).collect::<Vec<_>>(), vec![SEEK_HEAD, INFO, TRACKS, CLUSTER, CLUSTER, CLUSTER, CLUSTER, CUES]);
        let cues = parts.last().unwrap();
        let points = children(&joined, cues.start, cues.end, &[]).unwrap();
        assert_eq!(points.len(), 4);
        for (point, cluster) in points.iter().zip(parts.iter().filter(|part| part.id == CLUSTER)) {
            let fields = children(&joined, point.start, point.end, &[]).unwrap();
            let positions = children(&joined, fields[1].start, fields[1].end, &[]).unwrap();
            let position = read_uint(&joined[positions[1].start..positions[1].end]) as usize;
            assert_eq!(segment.start + position, cluster.head);
        }
        /* The SeekHead names the Cues. */
        let seeks = children(&joined, parts[0].start, parts[0].end, &[]).unwrap();
        let last = children(&joined, seeks[2].start, seeks[2].end, &[]).unwrap();
        assert_eq!(segment.start + read_uint(&joined[last[1].start..last[1].end]) as usize, cues.head);
    }

    /// Joins real clips: set ISV_JOIN_INPUTS (paths, separated by `;`) and ISV_JOIN_OUTPUT, then run with `--ignored`.
    #[test]
    #[ignore]
    fn joins_real_clips() {
        let inputs = std::env::var("ISV_JOIN_INPUTS").unwrap();
        let clips: Vec<Vec<u8>> = inputs.split(';').map(|path| std::fs::read(path).unwrap()).collect();
        for clip in &clips {
            let parsed = parse_clip(clip).unwrap();
            let blocks: u64 = parsed.clusters.iter().map(|cluster| cluster.block_count).sum();
            println!(
                "codec {:?}, {}x{}, scale {}, duration {:?}, length {}, {} clusters, {} blocks",
                String::from_utf8_lossy(&parsed.shapes[0].codec), parsed.shapes[0].width, parsed.shapes[0].height,
                parsed.timecode_scale, parsed.duration, parsed.length(), parsed.clusters.len(), blocks
            );
        }
        let joined = join(&clips, std::env::var("ISV_JOIN_CONTINUED").is_ok()).unwrap();
        let parsed = parse_clip(&joined).unwrap();
        println!("joined: duration {:?}, {} clusters", parsed.duration, parsed.clusters.len());
        std::fs::write(std::env::var("ISV_JOIN_OUTPUT").unwrap(), joined).unwrap();
    }

    #[test]
    fn clips_of_another_size_are_refused() {
        let error = join(&[clip(64, 2, 1, true, false), clip(32, 2, 2, true, false)], false).unwrap_err();
        assert!(error.contains("same size"));
        assert!(join(&[b"not a video".to_vec()], false).is_err());
        assert!(join(&[], false).is_err());
    }

    /// A clip as the engine makes it: one cluster, and each picture is a key picture.
    fn all_key_clip(frames: i16, payload: u8) -> Vec<u8> {
        let info = [short(TIMECODE_SCALE, &[0x0F, 0x42, 0x40]), short(DURATION, &(f32::from(frames) * 62.0).to_be_bytes())].concat();
        let video = [short(PIXEL_WIDTH, &[64]), short(PIXEL_HEIGHT, &[48])].concat();
        let entry = [short(TRACK_NUMBER, &[1]), short(CODEC_ID, b"V_VP9"), short(VIDEO, &video)].concat();
        let mut cluster = short(TIMECODE, &[0]);
        for frame in 0..frames {
            cluster.extend(simple_block(frame * 62, true, payload));
        }
        let segment = [short(INFO, &info), short(TRACKS, &short(TRACK_ENTRY, &entry)), short(CLUSTER, &cluster)].concat();
        let mut file = short(EBML, &short(0x4282, b"webm"));
        file.extend(id_bytes(SEGMENT));
        write_size(&mut file, segment.len() as u64);
        file.extend(segment);
        file
    }

    #[test]
    fn continued_clips_leave_out_the_repeated_first_picture() {
        /* The second and third clips lose their first picture: it is the last picture of the clip before. */
        let joined = join(&[clip(64, 4, 1, true, false), all_key_clip(4, 2), all_key_clip(2, 3)], true).unwrap();
        assert_eq!(block_times(&joined), vec![(0, 1), (62, 1), (124, 1), (186, 1), (248, 2), (310, 2), (372, 2), (434, 3)]);
        assert_eq!(parse_clip(&joined).unwrap().duration, Some(496.0));
        /* A second picture that needs the first one keeps the first one. */
        let kept = join(&[clip(64, 4, 1, true, false), clip(64, 4, 2, true, false)], true).unwrap();
        assert_eq!(block_times(&kept).len(), 8);
    }

    #[test]
    fn the_last_picture_must_be_a_vp8_key_picture() {
        assert!(last_picture(&clip(64, 4, 1, true, false)).unwrap_err().contains("VP8"));
        assert!(last_picture(b"not a video").is_err());
    }
}
