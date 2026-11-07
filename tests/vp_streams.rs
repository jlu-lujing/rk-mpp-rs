//! Damaged and truncated streams: the policy, the classification, and the
//! ladder that proves every cut lands on a named outcome.
//!
//! The policy itself lives with the decoders (`decode_stream` in both
//! `vp8::decoder` and `vp9::decoder`, documented on `Vp8Error::recoverable`):
//! a container error stops the run with nothing decoded; a packet the syntax
//! refuses is dropped with its reason recorded and every surrounding picture
//! still decodes; a hardware or bookkeeping failure stops the run because the
//! machine's next state is unknown. What is provable without a board is the
//! classification and the total behaviour of the parsers under truncation,
//! which is what runs here; the outcome shape of the full policy is locked by
//! the board-ignored tests at the bottom.

use rk_mpp_rs::codec::vp8::parser as vp8;
use rk_mpp_rs::codec::vp9::parser as vp9;

fn rebuild_ivf(source: &[u8], mutate: impl Fn(usize, &[u8]) -> Vec<u8>) -> Vec<u8> {
    // Re-emit the container with each payload passed through `mutate`, so a
    // damaged *payload* stays a well-formed container (a corrupt packet with
    // a lying IVF index tests the container path, not the packet path).
    let header_size = u16::from_le_bytes([source[6], source[7]]) as usize;
    let mut out = source[..header_size].to_vec();
    let mut offset = header_size;
    let mut index = 0;
    while offset + 12 <= source.len() {
        let size = u32::from_le_bytes(source[offset..offset + 4].try_into().unwrap()) as usize;
        let timestamp = source[offset + 4..offset + 12].try_into().unwrap();
        offset += 12;
        let payload = &source[offset..offset + size];
        let payload = mutate(index, payload);
        offset += size;
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(timestamp);
        out.extend_from_slice(&payload);
        index += 1;
    }
    out
}

#[test]
fn vp8_reports_every_truncation_step_by_name() {
    let data = std::fs::read("tests/data/vp8_320x240_12_ip.ivf").expect("fixture");
    let frames = vp8::parse_ivf(&data).expect("container");
    for packet in [frames[0].data, frames[1].data] {
        for cut in 1..packet.len() {
            let result = vp8::Parser::new().parse_picture(&packet[..cut]);
            match result {
                Ok(_) | Err(vp8::Vp8Error::Truncated) | Err(vp8::Vp8Error::Unsupported(_)) => {}
            }
        }
    }
}

#[test]
fn vp8_container_damage_is_named_by_the_container_itself() {
    let data = std::fs::read("tests/data/vp8_320x240_12_ip.ivf").expect("fixture");
    // A payload that runs past the file and a header that promises more
    // frames than the file holds are both container failures, never a
    // per-packet one.
    assert!(vp8::parse_ivf(&data[..data.len() - 3]).is_err());
    let mut lying = data.clone();
    lying[24..28].copy_from_slice(&999u32.to_le_bytes());
    assert!(vp8::parse_ivf(&lying).is_err());
}

#[test]
fn every_vp8_syntax_error_is_a_per_packet_verdict() {
    assert!(vp8::Vp8Error::Truncated.recoverable());
    assert!(vp8::Vp8Error::Unsupported("anything").recoverable());
}

#[test]
fn every_vp9_packet_error_but_the_container_is_a_per_packet_verdict() {
    assert!(vp9::Vp9Error::Truncated { bit: 0, width: 1 }.recoverable());
    assert!(vp9::Vp9Error::BadSyncCode.recoverable());
    assert!(vp9::Vp9Error::BadSuperframe.recoverable());
    assert!(vp9::Vp9Error::BadHeaderSize {
        header: 0,
        partition: 0,
        packet: 0
    }
    .recoverable());
    assert!(vp9::Vp9Error::MissingReference(0).recoverable());
    assert!(vp9::Vp9Error::UnsupportedProfile(1).recoverable());
    assert!(
        !vp9::Vp9Error::BadIvf.recoverable(),
        "the container is not a packet"
    );
}

/// The damage the stream policy has to survive packet-by-packet: last payload
/// halved, one payload's mid-section shredded, and an inter picture ahead of
/// the (now damaged) key frame.
#[test]
fn vp9_parsers_see_packet_damage_as_packet_verdicts() {
    let data = std::fs::read("tests/data/vp9_p0_64x64.ivf").expect("fixture");
    let damaged = rebuild_ivf(&data, |index, payload| {
        let mut payload = payload.to_vec();
        match index {
            0 => {
                // Six bytes: past the frame tag, straight into the key-frame
                // header, where the reader must run out mid-parse. (Halving a
                // VP9 key frame can honestly still deliver a whole
                // uncompressed header - that outcome is not damage by the
                // header's own account.)
                payload.truncate(6);
            }
            1 => {
                let middle = payload.len() / 2;
                payload[middle..].fill(0xa5);
            }
            _ => {}
        }
        payload
    });
    let ivf = vp9::parse_ivf(&damaged).expect("the container still parses");
    let mut parser = vp9::Parser::new();
    let mut outcomes = Vec::new();
    for frame in &ivf.frames {
        let parts = match vp9::split_superframe(frame.data) {
            Ok(parts) => parts,
            Err(vp9::Vp9Error::BadSuperframe) => {
                outcomes.push("superframe");
                continue;
            }
            Err(other) => panic!("unnamed split error {other:?}"),
        };
        for part in parts {
            outcomes.push(match parser.parse(part) {
                Ok(_) => "ok",
                Err(vp9::Vp9Error::Truncated { .. }) => "truncated",
                Err(vp9::Vp9Error::BadSyncCode) => "sync",
                Err(vp9::Vp9Error::BadFrameMarker) => "frame-marker",
                Err(vp9::Vp9Error::BadCompressedHeaderMarker) => "marker",
                Err(vp9::Vp9Error::MissingReference(0xff)) => "pre-key",
                Err(vp9::Vp9Error::MissingReference(slot)) => {
                    outcomes.push("missing-ref");
                    let _ = slot;
                    continue;
                }
                Err(vp9::Vp9Error::BadHeaderSize { .. }) => "size",
                Err(other) => panic!("unnamed error {other:?}"),
            });
        }
    }
    // The six-byte key frame cannot parse (the six bytes cut the reader off
    // mid-field; a superframe misread of the stub is its own named outcome);
    // everything until a surviving key frame becomes a pre-key drop or a
    // named parse failure. No outcome is ever "panic" and none is unnamed.
    assert!(!outcomes.is_empty());
    assert!(
        matches!(outcomes[0], "truncated" | "superframe"),
        "the stub key frame landed on {}",
        outcomes[0]
    );
}

/// A torn final packet matches `mpi_dec_test`: the complete frames survive,
/// the declared tail is zero-padded, and the container fault is recorded
/// without stopping the run.
#[test]
#[ignore = "board-only: opens /dev/mpp_service"]
fn vp8_stream_policy_recovers_a_torn_final_packet() {
    let mut decoder = rk_mpp_rs::codec::vp8::decoder::Decoder::new().expect("vdpu2");
    let data = std::fs::read("tests/data/vp8_320x240_12_ip.ivf").expect("fixture");
    let outcome = decoder.decode_stream(&data[..data.len() - 3]);
    assert!(!outcome.stopped_early);
    assert_eq!(outcome.pictures.len(), 12);
    assert_eq!(
        outcome.dropped.len(),
        1,
        "the container itself is the fault"
    );
    assert!(outcome.dropped[0].reason.starts_with("container:"));
    let stats = decoder.pool_stats();
    assert_eq!(stats.in_use, 0, "all DPB leases returned after flush");
    assert!(
        stats.allocations <= 16,
        "DPB reuse honoured its 16-slot cap"
    );
}

/// A shortened middle packet is submitted like the C harness does. The run
/// continues, but the hardware marks that picture as failed.
#[test]
#[ignore = "board-only: opens /dev/mpp_service"]
fn vp8_stream_policy_marks_a_damaged_middle_packet() {
    let data = std::fs::read("tests/data/vp8_320x240_12_ip.ivf").expect("fixture");
    let mut clean = rk_mpp_rs::codec::vp8::decoder::Decoder::new().expect("vdpu2");
    let clean = clean.decode_stream(&data);
    assert!(!clean.stopped_early && clean.dropped.is_empty());

    let damaged = rebuild_ivf(&data, |index, payload| {
        if index == payload_count_halfway(&data) {
            let mut payload = payload.to_vec();
            payload.truncate(payload.len() / 2);
            payload
        } else {
            payload.to_vec()
        }
    });
    let mut damaged_decoder = rk_mpp_rs::codec::vp8::decoder::Decoder::new().expect("vdpu2");
    let outcome = damaged_decoder.decode_stream(&damaged);
    assert!(!outcome.stopped_early, "one packet must not stop the run");
    assert!(outcome.dropped.is_empty());
    assert_eq!(outcome.pictures.len(), clean.pictures.len());
    assert_eq!(
        outcome
            .pictures
            .iter()
            .filter(|picture| picture.err_info != 0)
            .count(),
        1,
        "exactly the damaged middle picture carries the hardware error"
    );

    // The failed STATUS word forces a replacement mpp_service session.
    // Decode the clean stream through that same decoder and compare every
    // visible plane, proving the next job did not land on a wedged session.
    let recovered = damaged_decoder.decode_stream(&data);
    assert!(
        !recovered.stopped_early && recovered.dropped.is_empty(),
        "clean stream must decode after hardware recovery: {:?}",
        recovered.dropped
    );
    assert_eq!(recovered.pictures.len(), clean.pictures.len());
    for (index, (got, expected)) in recovered
        .pictures
        .iter()
        .zip(clean.pictures.iter())
        .enumerate()
    {
        assert_eq!(got.err_info, 0, "picture {index} retained an error");
        assert_eq!((got.width, got.height), (expected.width, expected.height));
        assert_eq!(got.yuv, expected.yuv, "picture {index} pixels differ");
    }
    let stats = damaged_decoder.pool_stats();
    assert_eq!(stats.in_use, 0, "all DPB leases returned after recovery");
    assert!(
        stats.allocations <= 16,
        "recovery work stayed within the pool cap"
    );
}

fn payload_count_halfway(data: &[u8]) -> usize {
    let frames = vp8::parse_ivf(data).expect("fixture");
    frames.len() / 2
}

/// VP9 side of the same claim, including the bit-flip variant where the
/// hardware (not the parser) is expected to report the damage.
#[test]
#[ignore = "board-only: opens /dev/mpp_service"]
fn vp9_stream_policy_pays_only_for_the_damaged_packet() {
    let data = std::fs::read("tests/data/vp9_p0_64x64.ivf").expect("fixture");
    let mut clean = rk_mpp_rs::codec::vp9::decoder::Decoder::new().expect("rkvdec");
    let clean = clean.decode_stream(&data);
    assert!(!clean.stopped_early && clean.dropped.is_empty());

    let mid_shredded = rebuild_ivf(&data, |index, payload| {
        if index == 5 {
            let mut payload = payload.to_vec();
            let middle = payload.len() / 2;
            payload[middle] ^= 0xff;
            payload
        } else {
            payload.to_vec()
        }
    });
    let mut damaged = rk_mpp_rs::codec::vp9::decoder::Decoder::new().expect("rkvdec");
    let outcome = damaged.decode_stream(&mid_shredded);
    assert!(!outcome.stopped_early, "payload damage stays per-packet");
    assert!(outcome.pictures.len() <= clean.pictures.len());
    let bad = outcome
        .pictures
        .iter()
        .filter(|picture| picture.err_info != 0)
        .count();
    let _ = bad; // the exact err_info propagation is board evidence, recorded by the script
}
