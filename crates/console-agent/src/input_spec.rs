//! Parser for oneshot `--input` specs: comma-separated `COUNT:BUTTONS`
//! segments, e.g. `30:,10:R,5:RA,60:` (empty buttons = no input).

use console_core::input;

/// One `COUNT:BUTTONS` segment, already resolved to a frame count and mask.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    pub count: u64,
    pub mask: u8,
}

/// Parse a full input spec. An empty string parses to zero segments (not an
/// error) so `--frames N` with no `--input` just runs `N` frames of no
/// input.
pub fn parse_spec(spec: &str) -> Result<Vec<Segment>, String> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Ok(Vec::new());
    }
    spec.split(',').map(parse_segment).collect()
}

fn parse_segment(segment: &str) -> Result<Segment, String> {
    let (count_str, buttons_str) = segment
        .split_once(':')
        .ok_or_else(|| format!("invalid input segment {segment:?}: expected COUNT:BUTTONS"))?;
    let count: u64 = count_str
        .trim()
        .parse()
        .map_err(|_| format!("invalid frame count {count_str:?} in segment {segment:?}"))?;
    let mask = input::parse(buttons_str.trim())
        .map_err(|c| format!("unknown button {c:?} in segment {segment:?}"))?;
    Ok(Segment { count, mask })
}

/// Total number of frames covered by explicit segments.
pub fn total_frames(segments: &[Segment]) -> u64 {
    segments.iter().map(|s| s.count).sum()
}

/// The button mask that applies at frame index `frame_idx` (0-based).
/// Frames past the end of all segments get no input (mask 0) — this is how
/// `--frames N` longer than the spec's total is handled.
pub fn mask_at(segments: &[Segment], frame_idx: u64) -> u8 {
    let mut acc = 0u64;
    for seg in segments {
        if frame_idx < acc + seg.count {
            return seg.mask;
        }
        acc += seg.count;
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_basic_spec() {
        let segs = parse_spec("30:,10:R,5:RA").unwrap();
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0], Segment { count: 30, mask: 0 });
        assert_eq!(
            segs[1],
            Segment {
                count: 10,
                mask: input::RIGHT
            }
        );
        assert_eq!(
            segs[2],
            Segment {
                count: 5,
                mask: input::RIGHT | input::A
            }
        );
        assert_eq!(total_frames(&segs), 45);
    }

    #[test]
    fn empty_spec_is_zero_segments() {
        assert_eq!(parse_spec("").unwrap(), Vec::new());
    }

    #[test]
    fn mask_at_falls_back_to_no_input_past_end() {
        let segs = parse_spec("2:R").unwrap();
        assert_eq!(mask_at(&segs, 0), input::RIGHT);
        assert_eq!(mask_at(&segs, 1), input::RIGHT);
        assert_eq!(mask_at(&segs, 2), 0);
        assert_eq!(mask_at(&segs, 100), 0);
    }

    #[test]
    fn rejects_bad_segment() {
        assert!(parse_spec("nope").is_err());
        assert!(parse_spec("5:Q").is_err());
    }
}
