//! Incremental drop-in compatibility surface for the official Rockchip MPI.

use std::collections::VecDeque;
use std::ffi::{c_int, c_void};
use std::mem::size_of;

use crate::{encoder, sdk};

mod cfg;
mod objects;
mod osd;
mod rc_api;
mod standalone;

type MppCtx = *mut c_void;
type MppFrame = *mut c_void;
type MppPacket = *mut c_void;
type MppBuffer = *mut c_void;
type MppTask = *mut c_void;
type MppParam = *mut c_void;

const MPP_OK: c_int = 0;
const MPP_NOK: c_int = -1;
const MPP_ERR_NULL_PTR: c_int = -3;
const MPP_ERR_VALUE: c_int = -6;

const MPP_CTX_DEC: c_int = 0;
const MPP_CTX_ENC: c_int = 1;
const MPP_CTX_ISP: c_int = 2;
const MPP_CTX_BUTT: c_int = 3;

const MPP_VIDEO_CODING_MPEG2: c_int = 2;
const MPP_VIDEO_CODING_H263: c_int = 3;
const MPP_VIDEO_CODING_MPEG4: c_int = 4;
const MPP_VIDEO_CODING_AVC: c_int = 7;
const MPP_VIDEO_CODING_MJPEG: c_int = 8;
const MPP_VIDEO_CODING_VP8: c_int = 9;
const MPP_VIDEO_CODING_VP9: c_int = 10;
const MPP_VIDEO_CODING_HEVC: c_int = 0x0100_0004;
const MPP_VIDEO_CODING_AV1: c_int = 0x0100_0008;

const MPP_DEC_SET_OUTPUT_FORMAT: c_int = 3_211_274;
const MPP_DEC_SET_DISABLE_ERROR: c_int = 3_211_275;
const MPP_DEC_SET_IMMEDIATE_OUT: c_int = 3_211_276;
const MPP_DEC_SET_CFG: c_int = 3_211_777;
const MPP_DEC_GET_CFG: c_int = 3_211_778;

const MPP_ENC_SET_CFG: c_int = 3_276_801;
const MPP_ENC_GET_CFG: c_int = 3_276_802;
const MPP_ENC_SET_IDR_FRAME: c_int = 3_276_809;
const MPP_ENC_GET_HDR_SYNC: c_int = 3_276_813;
const MPP_ENC_SET_SEI_CFG: c_int = 3_276_815;
const MPP_ENC_SET_HEADER_MODE: c_int = 3_309_569;
const MPP_ENC_SET_OSD_PLT_CFG: c_int = 3_310_593;
const MPP_ENC_GET_OSD_PLT_CFG: c_int = 3_310_594;
const MPP_ENC_SET_OSD_DATA_CFG: c_int = 3_310_595;
const MPP_ENC_GET_RC_API_ALL: c_int = 3_277_313;
const MPP_ENC_GET_RC_API_BY_TYPE: c_int = 3_277_314;
const MPP_ENC_SET_RC_API_CFG: c_int = 3_277_315;
const MPP_ENC_GET_RC_API_CURRENT: c_int = 3_277_316;
const MPP_ENC_SET_RC_API_CURRENT: c_int = 3_277_317;
const MPP_ENC_SET_REF_CFG: c_int = 3_310_081;
const MPP_SET_OUTPUT_TIMEOUT: c_int = 2_097_159;

const CONTEXT_MAGIC: u64 = 0x4d50_5052_5343_5458;
const REF_TO_PREV_REF_FRM: u8 = 0;
const REF_TO_PREV_LT_REF: u8 = 2;
const REF_TO_PREV_INTRA: u8 = 3;
const REF_TO_TEMPORAL_LAYER: u8 = 4;
const RECONSTRUCTION_SLOTS: usize = 3;
const REFERENCE_DPB_SIZE: usize = 2;

type DecodeFn = unsafe extern "C" fn(MppCtx, MppPacket, *mut MppFrame) -> c_int;
type DecodePutFn = unsafe extern "C" fn(MppCtx, MppPacket) -> c_int;
type DecodeGetFn = unsafe extern "C" fn(MppCtx, *mut MppFrame) -> c_int;
type EncodeFn = unsafe extern "C" fn(MppCtx, MppFrame, *mut MppPacket) -> c_int;
type EncodePutFn = unsafe extern "C" fn(MppCtx, MppFrame) -> c_int;
type EncodeGetFn = unsafe extern "C" fn(MppCtx, *mut MppPacket) -> c_int;
type IspFn = unsafe extern "C" fn(MppCtx, MppFrame, MppFrame) -> c_int;
type IspPutFn = unsafe extern "C" fn(MppCtx, MppFrame) -> c_int;
type IspGetFn = unsafe extern "C" fn(MppCtx, *mut MppFrame) -> c_int;
type PollFn = unsafe extern "C" fn(MppCtx, c_int, c_int) -> c_int;
type TaskFn = unsafe extern "C" fn(MppCtx, c_int, *mut MppTask) -> c_int;
type EnqueueFn = unsafe extern "C" fn(MppCtx, c_int, MppTask) -> c_int;
type ResetFn = unsafe extern "C" fn(MppCtx) -> c_int;
type ControlFn = unsafe extern "C" fn(MppCtx, c_int, MppParam) -> c_int;

#[repr(C)]
pub struct MppApi {
    size: u32,
    version: u32,
    decode: Option<DecodeFn>,
    decode_put_packet: Option<DecodePutFn>,
    decode_get_frame: Option<DecodeGetFn>,
    encode: Option<EncodeFn>,
    encode_put_frame: Option<EncodePutFn>,
    encode_get_packet: Option<EncodeGetFn>,
    isp: Option<IspFn>,
    isp_put_frame: Option<IspPutFn>,
    isp_get_frame: Option<IspGetFn>,
    poll: Option<PollFn>,
    dequeue: Option<TaskFn>,
    enqueue: Option<EnqueueFn>,
    reset: Option<ResetFn>,
    control: Option<ControlFn>,
    reserv: [u32; 16],
}

static MPP_API: MppApi = MppApi {
    size: size_of::<MppApi>() as u32,
    version: 0,
    decode: Some(compat_decode),
    decode_put_packet: Some(compat_decode_put_packet),
    decode_get_frame: Some(compat_decode_get_frame),
    encode: Some(compat_encode),
    encode_put_frame: Some(compat_encode_put_frame),
    encode_get_packet: Some(compat_encode_get_packet),
    isp: Some(compat_isp),
    isp_put_frame: Some(compat_isp_put_frame),
    isp_get_frame: Some(compat_isp_get_frame),
    poll: Some(compat_poll),
    dequeue: Some(compat_dequeue),
    enqueue: Some(compat_enqueue),
    reset: Some(compat_reset),
    control: Some(compat_control),
    reserv: [0; 16],
};

#[repr(C)]
struct CompatContext {
    magic: u64,
    ctx_type: c_int,
    coding: c_int,
    decoder: Option<sdk::Decoder>,
    encoder: Option<encoder::Encoder>,
    eos_submitted: bool,
    eos_delivered: bool,
    pending_output_frame: MppFrame,
    pending_input_packet: MppPacket,
    pending_packet_motion_info: VecDeque<MppBuffer>,
    encoder_ref_pattern: Vec<standalone::ReferenceFrameStep>,
    encoder_ref_schedule: Option<ReferenceSchedule>,
    rc_api: rc_api::Selection,
    header_sync_requested: bool,
    custom_rc: Option<rc_api::Session>,
    custom_rc_pending: Option<rc_api::EncRcTask>,
    custom_rc_max_reencodes: i32,
    custom_rc_sequence: u32,
    custom_force_idr: bool,
    osd_palette: osd::PaletteState,
}

#[derive(Clone, Copy, Debug)]
struct ActiveReference {
    sequence: usize,
    slot: usize,
    temporal_id: u8,
    frame_num: u32,
    intra: bool,
    long_term: Option<u32>,
}

struct ReferenceSchedule {
    steps: Vec<standalone::ReferenceFrameStep>,
    next_sequence: usize,
    frame_num: u32,
    last_non_reference: bool,
    active: Vec<ActiveReference>,
    long_term: Option<standalone::LongTermReferenceConfig>,
    igop: usize,
}

struct PendingReferenceFrame {
    current_slot: usize,
    reference_slot: usize,
    non_reference: bool,
    temporal_id: u8,
    frame_num: u32,
    poc: u32,
    reference_reorder: Option<encoder::ReferenceReorder>,
    current_long_term_index: Option<u32>,
    commit: Option<ActiveReference>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CustomRcAction {
    Encode,
    Skip,
}

impl CompatContext {
    fn prepare_custom_rc(&mut self) -> Result<CustomRcAction, c_int> {
        let Some(session) = self.custom_rc.as_mut() else {
            return Ok(CustomRcAction::Encode);
        };
        let sequence = self.custom_rc_sequence;
        let forced_idr = self.custom_force_idr;
        let intra = sequence == 0 || forced_idr;
        let mut task = rc_api::EncRcTask::default();
        if let Err(error) = session.check_drop(&mut task) {
            eprintln!("rk-mpp-rs mpp: RC drop decision failed: {error}");
            return Err(MPP_NOK);
        }
        if task.frm.is_drop() {
            self.custom_rc_sequence = sequence.saturating_add(1);
            self.custom_force_idr = false;
            return Ok(CustomRcAction::Skip);
        }
        task.set_frame(sequence, intra, intra);
        if let Err(error) = session.frame_start(&mut task) {
            eprintln!("rk-mpp-rs mpp: RC frame start failed: {error}");
            return Err(MPP_NOK);
        }
        let (bit_target, qp_target, qp_min, qp_max) = task.rc_target();
        let qp_max = qp_max.clamp(0, 51);
        let qp_min = qp_min.clamp(0, qp_max);
        let qp_target = qp_target.clamp(qp_min, qp_max);
        let Some(encoder) = self.encoder.as_mut() else {
            return Err(MPP_NOK);
        };
        encoder.force_next_rate_control(bit_target, qp_target as u32, qp_min as u32, qp_max as u32);
        if task.frm.is_intra() || task.frm.is_idr() {
            if let Err(error) = encoder.force_idr() {
                eprintln!("rk-mpp-rs mpp: user RC forced-IDR request failed: {error}");
                return Err(MPP_NOK);
            }
        }
        self.custom_rc_pending = Some(task);
        self.custom_force_idr = false;
        Ok(CustomRcAction::Encode)
    }

    fn submit_custom_rc_frame(
        &mut self,
        input: encoder::InputFrame<'_>,
    ) -> Result<usize, sdk::Error> {
        let Some(mut session) = self.custom_rc.take() else {
            return Err(sdk::Error::State("custom rate-control session is missing"));
        };
        let Some(mut task) = self.custom_rc_pending.take() else {
            self.custom_rc = Some(session);
            return Err(sdk::Error::State("custom rate-control task is missing"));
        };
        let Some(mut encoder) = self.encoder.take() else {
            self.custom_rc = Some(session);
            self.custom_rc_pending = Some(task);
            return Err(sdk::Error::State("custom rate-control encoder is missing"));
        };
        let reencode_limit = self.custom_rc_max_reencodes.max(0);
        let mut reencodes = 0;
        let result = encoder.submit_frame_with_rate_control_reencode(input, |feedback| {
            let reencode = session.finish_attempt(&mut task, |task| {
                task.info.bit_real = feedback.bit_real;
                task.info.quality_real = feedback.quality_real;
            })?;
            if !reencode || reencodes >= reencode_limit {
                return Ok(None);
            }
            reencodes += 1;
            task.info.clear_measurements();
            session.hal_start(&mut task)?;
            let (bit_target, qp_target, qp_min, qp_max) = task.rc_target();
            Ok(Some(encoder::ExternalRateControl {
                bit_target,
                qp_target: qp_target as u32,
                qp_min: qp_min as u32,
                qp_max: qp_max as u32,
            }))
        });
        self.encoder = Some(encoder);
        self.custom_rc = Some(session);
        self.custom_rc_pending = Some(task);
        result
    }

    fn finish_custom_rc(&mut self) -> Result<(), c_int> {
        let Some(mut task) = self.custom_rc_pending.take() else {
            return Ok(());
        };
        let Some(session) = self.custom_rc.as_mut() else {
            return Err(MPP_NOK);
        };
        if let Err(error) = session.frame_end(&mut task) {
            eprintln!("rk-mpp-rs mpp: RC frame completion failed: {error}");
            return Err(MPP_NOK);
        }
        task.frm.set_reencode(false);
        self.custom_rc_sequence = self.custom_rc_sequence.saturating_add(1);
        Ok(())
    }
}

impl ReferenceSchedule {
    fn new(
        steps: Vec<standalone::ReferenceFrameStep>,
        long_term: Option<standalone::LongTermReferenceConfig>,
        igop: usize,
    ) -> Self {
        Self {
            steps,
            next_sequence: 0,
            frame_num: 0,
            last_non_reference: false,
            active: Vec::with_capacity(REFERENCE_DPB_SIZE),
            long_term,
            igop: igop.max(1),
        }
    }

    fn force_idr(&mut self) {
        self.next_sequence = 0;
        self.frame_num = 0;
        self.last_non_reference = false;
        self.active.clear();
    }

    fn prepare(&self) -> Result<PendingReferenceFrame, String> {
        let sequence = self.next_sequence;
        if sequence == 0 {
            let current_long_term_index = self.long_term.map(|long_term| long_term.index);
            return Ok(PendingReferenceFrame {
                current_slot: 0,
                reference_slot: 0,
                non_reference: false,
                temporal_id: 0,
                frame_num: 0,
                poc: 0,
                reference_reorder: None,
                current_long_term_index,
                commit: Some(ActiveReference {
                    sequence,
                    slot: 0,
                    temporal_id: 0,
                    frame_num: 0,
                    intra: true,
                    long_term: current_long_term_index,
                }),
            });
        }
        if self.steps.is_empty() {
            return Err("H.264 reference schedule has no short-term steps".into());
        }
        let mut step = self
            .steps
            .get((sequence - 1) % self.steps.len())
            .copied()
            .ok_or_else(|| "H.264 reference schedule has no short-term steps".to_string())?;
        let current_long_term_index = self
            .is_long_term_frame(sequence)
            .then(|| self.long_term.map(|long_term| long_term.index))
            .flatten();
        if let (Some(long_term), Some(_)) = (self.long_term, current_long_term_index) {
            step.non_reference = false;
            step.temporal_id = 0;
            step.reference_mode = long_term.reference_mode;
            step.reference_argument = long_term.reference_argument;
        }
        let frame_num = if !step.non_reference && self.last_non_reference {
            self.frame_num
        } else {
            self.frame_num
                .checked_add(1)
                .ok_or_else(|| "H.264 TSVC frame_num overflowed".to_string())?
        };
        let reference = self.resolve_reference(step)?;
        let current_slot = (0..RECONSTRUCTION_SLOTS)
            .find(|slot| self.active.iter().all(|reference| reference.slot != *slot))
            .ok_or_else(|| {
                "H.264 reference schedule has no free reconstruction slot".to_string()
            })?;
        let reference_reorder = if let Some(long_term_pic_num) = reference.long_term {
            self.active
                .iter()
                .any(|active| active.long_term.is_none())
                .then_some(encoder::ReferenceReorder::LongTerm { long_term_pic_num })
        } else {
            let latest_short_term = self
                .active
                .iter()
                .filter(|active| active.long_term.is_none())
                .max_by_key(|active| active.sequence)
                .ok_or_else(|| {
                    "H.264 reference schedule has no active short-term reference".to_string()
                })?;
            (reference.sequence != latest_short_term.sequence).then(|| {
                encoder::ReferenceReorder::ShortTerm(encoder::ShortTermReorder::new(
                    frame_num,
                    reference.frame_num,
                ))
            })
        };
        let commit = (!step.non_reference).then_some(ActiveReference {
            sequence,
            slot: current_slot,
            temporal_id: step.temporal_id,
            frame_num,
            intra: false,
            long_term: current_long_term_index,
        });
        Ok(PendingReferenceFrame {
            current_slot,
            reference_slot: reference.slot,
            non_reference: step.non_reference,
            temporal_id: step.temporal_id,
            frame_num,
            poc: u32::try_from(sequence)
                .map_err(|_| "H.264 POC sequence overflowed".to_string())?
                .saturating_mul(2),
            reference_reorder,
            current_long_term_index,
            commit,
        })
    }

    fn is_long_term_frame(&self, sequence: usize) -> bool {
        self.long_term.is_some_and(|long_term| {
            long_term.gap != 0
                && sequence >= long_term.delay
                && (sequence - long_term.delay) % long_term.gap == 0
        })
    }

    fn resolve_reference(
        &self,
        step: standalone::ReferenceFrameStep,
    ) -> Result<ActiveReference, String> {
        let reference = match step.reference_mode {
            REF_TO_PREV_REF_FRM => self
                .active
                .iter()
                .max_by_key(|reference| reference.sequence),
            REF_TO_PREV_LT_REF => self
                .active
                .iter()
                .filter(|reference| reference.long_term.is_some())
                .max_by_key(|reference| reference.sequence),
            REF_TO_PREV_INTRA => self.active.iter().find(|reference| reference.intra),
            REF_TO_TEMPORAL_LAYER => {
                let temporal_id = u8::try_from(step.reference_argument)
                    .map_err(|_| "H.264 temporal-layer reference is out of range".to_string())?;
                self.active
                    .iter()
                    .filter(|reference| reference.temporal_id == temporal_id)
                    .max_by_key(|reference| reference.sequence)
            }
            mode => return Err(format!("H.264 reference mode {mode} is not implemented")),
        };
        reference.copied().ok_or_else(|| {
            format!(
                "H.264 reference mode {} has no valid target",
                step.reference_mode
            )
        })
    }

    fn commit(&mut self, frame: PendingReferenceFrame) {
        self.next_sequence += 1;
        if let Some(reference) = frame.commit {
            self.frame_num = reference.frame_num;
            if let Some(long_term_index) = reference.long_term {
                self.active
                    .retain(|active| active.long_term != Some(long_term_index));
            }
            self.active.push(reference);
            while self.active.len() > REFERENCE_DPB_SIZE {
                let oldest = self
                    .active
                    .iter()
                    .enumerate()
                    .filter(|(_, reference)| reference.long_term.is_none())
                    .min_by_key(|(_, reference)| reference.sequence)
                    .map(|(index, _)| index)
                    .or_else(|| {
                        self.active
                            .iter()
                            .enumerate()
                            .min_by_key(|(_, reference)| reference.sequence)
                            .map(|(index, _)| index)
                    })
                    .expect("non-empty reference DPB");
                self.active.remove(oldest);
            }
        } else {
            self.frame_num = self.frame_num.saturating_add(1);
        }
        self.last_non_reference = frame.non_reference;
        if self.next_sequence >= self.igop {
            self.next_sequence = 0;
            self.frame_num = 0;
            self.last_non_reference = false;
            self.active.clear();
        }
    }
}

unsafe fn context_mut<'a>(ctx: MppCtx) -> Option<&'a mut CompatContext> {
    if ctx.is_null() {
        return None;
    }
    // SAFETY: the public C contract requires a live MppCtx returned by
    // mpp_create(). The magic check catches stale or foreign pointers that are
    // still readable, without pretending arbitrary pointers are valid.
    let context = unsafe { &mut *(ctx.cast::<CompatContext>()) };
    (context.magic == CONTEXT_MAGIC).then_some(context)
}

fn nv12_input(frame: &objects::FrameInputView) -> Result<Vec<u8>, c_int> {
    let luma_size = frame
        .hor_stride
        .checked_mul(frame.ver_stride)
        .ok_or(MPP_ERR_VALUE)?;
    let output_size = frame
        .width
        .checked_mul(frame.height)
        .and_then(|luma| luma.checked_mul(3).map(|bytes| bytes / 2))
        .ok_or(MPP_ERR_VALUE)?;
    if frame.data.is_null()
        || frame.len < luma_size
        || frame.hor_stride < frame.width
        || frame.height % 2 != 0
    {
        return Err(MPP_ERR_VALUE);
    }
    let bytes = unsafe { std::slice::from_raw_parts(frame.data, frame.len) };
    let mut output = vec![0u8; output_size];
    for row in 0..frame.height {
        let source = row * frame.hor_stride;
        let destination = row * frame.width;
        output[destination..destination + frame.width]
            .copy_from_slice(&bytes[source..source + frame.width]);
    }
    let uv_source = luma_size;
    let uv_destination = frame.width * frame.height;
    for row in 0..frame.height / 2 {
        let source = uv_source + row * frame.hor_stride;
        let destination = uv_destination + row * frame.width;
        output[destination..destination + frame.width]
            .copy_from_slice(&bytes[source..source + frame.width]);
    }
    Ok(output)
}

unsafe extern "C" fn compat_decode(ctx: MppCtx, packet: MppPacket, frame: *mut MppFrame) -> c_int {
    let put = unsafe { compat_decode_put_packet(ctx, packet) };
    if put != MPP_OK {
        return put;
    }
    unsafe { compat_decode_get_frame(ctx, frame) }
}

unsafe extern "C" fn compat_decode_put_packet(ctx: MppCtx, packet: MppPacket) -> c_int {
    let Some(context) = (unsafe { context_mut(ctx) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if context.ctx_type != MPP_CTX_DEC {
        return MPP_ERR_VALUE;
    }
    let Some(decoder) = context.decoder.as_mut() else {
        return MPP_ERR_VALUE;
    };
    let Some(bytes) = (unsafe { objects::packet_bytes(packet) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let Some((pts, dts, eos)) = (unsafe { objects::packet_metadata(packet) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let output_frame =
        unsafe { objects::packet_output_frame(packet) }.unwrap_or(std::ptr::null_mut());
    if !output_frame.is_null() {
        if let Some(buffer) = unsafe { objects::frame_output_dmabuf(output_frame) } {
            if matches!(
                context.coding,
                MPP_VIDEO_CODING_AVC
                    | MPP_VIDEO_CODING_MJPEG
                    | MPP_VIDEO_CODING_VP9
                    | MPP_VIDEO_CODING_AV1
                    | MPP_VIDEO_CODING_HEVC
            ) {
                if let Err(error) = decoder.import_output_dmabuf(buffer) {
                    eprintln!("rk-mpp-rs mpp: output buffer import failed: {error}");
                    return MPP_NOK;
                }
            }
        }
    }
    let flags = if eos {
        sdk::PacketFlags::empty().with_eos()
    } else {
        sdk::PacketFlags::empty()
    };
    let sdk_packet = sdk::Packet::new(bytes)
        .with_pts(pts)
        .with_dts(dts)
        .with_flags(flags);
    if decoder.submit_packet(sdk_packet).is_err() {
        return MPP_NOK;
    }
    if !unsafe { objects::packet_consume(packet) } {
        return MPP_ERR_VALUE;
    }
    context.eos_submitted |= eos;
    context.pending_output_frame = output_frame;
    context.pending_input_packet = packet;
    MPP_OK
}

unsafe extern "C" fn compat_decode_get_frame(ctx: MppCtx, frame: *mut MppFrame) -> c_int {
    if frame.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let Some(context) = (unsafe { context_mut(ctx) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if context.ctx_type != MPP_CTX_DEC {
        return MPP_ERR_VALUE;
    }
    let Some(decoder) = context.decoder.as_mut() else {
        return MPP_ERR_VALUE;
    };
    unsafe { *frame = std::ptr::null_mut() };
    if let Some(decoded) = decoder.receive_frame() {
        let frame_object = if context.pending_output_frame.is_null() {
            let Ok(frame) = (unsafe { objects::frame_from_sdk(decoded) }) else {
                return MPP_NOK;
            };
            frame
        } else {
            let frame = context.pending_output_frame;
            if unsafe { objects::frame_fill_from_sdk(frame, decoded) }.is_err() {
                return MPP_NOK;
            }
            context.pending_output_frame = std::ptr::null_mut();
            frame
        };
        if !context.pending_input_packet.is_null() {
            let _ = unsafe {
                objects::frame_set_input_packet(frame_object, context.pending_input_packet)
            };
        }
        unsafe { *frame = frame_object };
        return MPP_OK;
    }
    if context.eos_submitted && decoder.is_finished() && !context.eos_delivered {
        context.eos_delivered = true;
        unsafe { *frame = objects::frame_eos() };
    }
    MPP_OK
}

unsafe extern "C" fn compat_encode(ctx: MppCtx, frame: MppFrame, packet: *mut MppPacket) -> c_int {
    let put = unsafe { compat_encode_put_frame(ctx, frame) };
    if put != MPP_OK {
        return put;
    }
    unsafe { compat_encode_get_packet(ctx, packet) }
}

unsafe extern "C" fn compat_encode_put_frame(ctx: MppCtx, frame: MppFrame) -> c_int {
    let Some(context) = (unsafe { context_mut(ctx) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if context.ctx_type != MPP_CTX_ENC {
        return MPP_ERR_VALUE;
    }
    let picture_layout = match context.encoder_ref_schedule.as_ref() {
        Some(schedule) => match schedule.prepare() {
            Ok(frame) => Some(frame),
            Err(error) => {
                eprintln!("rk-mpp-rs mpp: reference schedule rejected: {error}");
                return MPP_NOK;
            }
        },
        None => None,
    };
    let Some(frame) = (unsafe { objects::frame_input_view(frame) }) else {
        eprintln!("rk-mpp-rs mpp: encode frame has no readable buffer");
        return MPP_ERR_NULL_PTR;
    };
    let motion_info = frame.motion_info;
    if frame.eos {
        return MPP_OK;
    }
    if frame.width == 0 || frame.height == 0 || frame.format != 0 {
        eprintln!(
            "rk-mpp-rs mpp: encode frame has invalid geometry/format {}x{} fmt {}",
            frame.width, frame.height, frame.format
        );
        return MPP_ERR_VALUE;
    }
    let input = match nv12_input(&frame) {
        Ok(input) => input,
        Err(ret) => {
            eprintln!("rk-mpp-rs mpp: encode frame is not a complete NV12 picture");
            return ret;
        }
    };
    let osd = match unsafe { osd::frame_config(frame.osd_data, &context.osd_palette) } {
        Ok(osd) => osd,
        Err(error) => {
            eprintln!("rk-mpp-rs mpp: OSD rejected: {error}");
            return MPP_NOK;
        }
    };
    let input = encoder::InputFrame::new(&input)
        .with_pts(frame.pts)
        .with_dts(frame.dts)
        .with_osd(osd);
    if let Some(frame) = picture_layout.as_ref() {
        let Some(encoder) = context.encoder.as_mut() else {
            return MPP_ERR_VALUE;
        };
        if let Err(error) = encoder.force_next_picture_layout(
            frame.current_slot,
            frame.reference_slot,
            frame.non_reference,
            Some(frame.temporal_id),
            frame.frame_num,
            frame.poc,
            frame.reference_reorder,
            frame.current_long_term_index,
        ) {
            eprintln!("rk-mpp-rs mpp: reference layout rejected: {error}");
            return MPP_NOK;
        }
    }
    match context.prepare_custom_rc() {
        Ok(CustomRcAction::Encode) => {}
        Ok(CustomRcAction::Skip) => return MPP_OK,
        Err(error) => return error,
    }
    let submitted = if context.custom_rc_pending.is_some() {
        context.submit_custom_rc_frame(input)
    } else {
        match context.encoder.as_mut() {
            Some(encoder) => encoder.submit_frame(input),
            None => {
                return MPP_ERR_VALUE;
            }
        }
    };
    match submitted {
        Ok(count) => {
            if let Err(error) = context.finish_custom_rc() {
                return error;
            }
            if let (Some(schedule), Some(frame)) =
                (context.encoder_ref_schedule.as_mut(), picture_layout)
            {
                schedule.commit(frame);
            }
            if !motion_info.is_null() {
                let Some(encoder) = context.encoder.as_mut() else {
                    return MPP_NOK;
                };
                if let Some(mut data) = encoder.take_motion_info() {
                    if let Some(capacity) = unsafe { objects::buffer_capacity(motion_info) } {
                        data.truncate(capacity);
                    }
                    if !unsafe { objects::buffer_replace(motion_info, &data) } {
                        eprintln!(
                            "rk-mpp-rs mpp: failed to write the caller's KEY_MOTION_INFO buffer"
                        );
                        return MPP_NOK;
                    }
                }
            }
            for _ in 0..count {
                context.pending_packet_motion_info.push_back(motion_info);
            }
            MPP_OK
        }
        Err(error) => {
            context.custom_rc_pending = None;
            eprintln!("rk-mpp-rs mpp: encode frame failed: {error}");
            MPP_NOK
        }
    }
}

unsafe extern "C" fn compat_encode_get_packet(ctx: MppCtx, packet: *mut MppPacket) -> c_int {
    if packet.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let Some(context) = (unsafe { context_mut(ctx) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if context.ctx_type != MPP_CTX_ENC {
        return MPP_ERR_VALUE;
    }
    let Some(encoder) = context.encoder.as_mut() else {
        return MPP_ERR_VALUE;
    };
    unsafe { *packet = std::ptr::null_mut() };
    if let Some(encoded) = encoder.receive_packet() {
        let pts = encoded.pts();
        let dts = encoded.dts();
        let meta = encoded.meta().clone();
        let data = encoded.into_data();
        let motion_info = context
            .pending_packet_motion_info
            .pop_front()
            .unwrap_or(std::ptr::null_mut());
        let output = unsafe { objects::packet_from_bytes(data, pts, dts) };
        if !unsafe { objects::packet_attach_encode_meta(output, &meta, motion_info) } {
            let mut packet = output;
            unsafe {
                let _ = objects::mpp_packet_deinit(&mut packet);
            }
            return MPP_NOK;
        }
        unsafe { *packet = output };
    }
    MPP_OK
}

unsafe extern "C" fn compat_isp(_ctx: MppCtx, _dst: MppFrame, _src: MppFrame) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_isp_put_frame(_ctx: MppCtx, _frame: MppFrame) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_isp_get_frame(_ctx: MppCtx, _frame: *mut MppFrame) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_poll(_ctx: MppCtx, _port: c_int, _timeout: c_int) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_dequeue(_ctx: MppCtx, _port: c_int, _task: *mut MppTask) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_enqueue(_ctx: MppCtx, _port: c_int, _task: MppTask) -> c_int {
    MPP_NOK
}

unsafe extern "C" fn compat_reset(ctx: MppCtx) -> c_int {
    let Some(context) = (unsafe { context_mut(ctx) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if let Some(decoder) = context.decoder.as_mut() {
        if decoder.reset().is_err() {
            return MPP_NOK;
        }
    }
    context.encoder = None;
    context.eos_submitted = false;
    context.eos_delivered = false;
    context.pending_output_frame = std::ptr::null_mut();
    context.pending_input_packet = std::ptr::null_mut();
    context.pending_packet_motion_info.clear();
    context.encoder_ref_pattern.clear();
    context.encoder_ref_schedule = None;
    context.custom_rc = None;
    context.custom_rc_pending = None;
    context.custom_rc_max_reencodes = 0;
    context.custom_rc_sequence = 0;
    context.custom_force_idr = false;
    MPP_OK
}

unsafe extern "C" fn compat_control(ctx: MppCtx, command: c_int, param: MppParam) -> c_int {
    let Some(context) = (unsafe { context_mut(ctx) }) else {
        return MPP_ERR_NULL_PTR;
    };
    match command {
        // Configuration is consumed by the surrounding SDK session at
        // mpp_init. Keep the vendor MppDecCfg object valid and accept the
        // values the official simple decode loop sets.
        MPP_DEC_SET_CFG
        | MPP_DEC_GET_CFG
        | MPP_DEC_SET_DISABLE_ERROR
        | MPP_DEC_SET_IMMEDIATE_OUT => MPP_OK,
        MPP_DEC_SET_OUTPUT_FORMAT => {
            if param.is_null() {
                return MPP_ERR_NULL_PTR;
            }
            // The compatible session currently exposes NV12.
            let format = unsafe { *param.cast::<u32>() };
            if format == 0 {
                MPP_OK
            } else {
                MPP_NOK
            }
        }
        MPP_ENC_SET_CFG => {
            if context.ctx_type != MPP_CTX_ENC || param.is_null() {
                return MPP_ERR_VALUE;
            }
            context.encoder_ref_pattern =
                standalone::encoder_cfg_ref_pattern(param).unwrap_or_default();
            let encoder_ref_long_term = standalone::encoder_cfg_long_term_config(param);
            let has_ref_cfg = standalone::encoder_cfg_has_ref_cfg(param);
            context.encoder_ref_schedule = None;
            let mut config = match unsafe { cfg::encoder_config(param) } {
                Ok(config) => config,
                Err(error) => {
                    eprintln!("rk-mpp-rs mpp: encoder config rejected: {error}");
                    return MPP_NOK;
                }
            };
            match context.encoder_ref_pattern.len() {
                // mpp_enc_ref_cfg_check publishes the short-term CPB depth
                // into h264e_sps_init.
                0 if !has_ref_cfg => config = config.with_num_ref_frames(1),
                0 => {}
                2 => config = config.with_num_ref_frames(1),
                _ => config = config.with_num_ref_frames(2),
            }
            match encoder::Encoder::new_mpi_compat(config) {
                Ok(mut encoder) => {
                    if context.coding == MPP_VIDEO_CODING_HEVC && context.header_sync_requested {
                        if let Err(error) = encoder.set_h265_mpi_compat() {
                            eprintln!("rk-mpp-rs mpp: H.265 header mode rejected: {error}");
                            return MPP_NOK;
                        }
                    }
                    if context.coding == MPP_VIDEO_CODING_HEVC && !has_ref_cfg {
                        if let Err(error) = encoder.set_h265_long_term_reference(false) {
                            eprintln!("rk-mpp-rs mpp: H.265 reference mode rejected: {error}");
                            return MPP_NOK;
                        }
                    }
                    if !encoder.supports_h264_reference_layout()
                        && !context.encoder_ref_pattern.is_empty()
                    {
                        context.encoder_ref_pattern.clear();
                        context.encoder_ref_schedule = None;
                    }
                    if context.coding == MPP_VIDEO_CODING_AVC {
                        if !context.encoder_ref_pattern.is_empty() {
                            if !matches!(config.profile(), 0 | 66 | 100) {
                                eprintln!(
                                    "rk-mpp-rs mpp: H.264 profile {} is not supported",
                                    config.profile()
                                );
                                return MPP_NOK;
                            }
                            if encoder.force_reference_slot(None).is_err() {
                                return MPP_NOK;
                            }
                        } else if has_ref_cfg && matches!(config.profile(), 0 | 66) {
                            if encoder.force_reference_slot(Some(0)).is_err() {
                                eprintln!(
                                    "rk-mpp-rs mpp: default MPP H.264 reference schedule is unavailable"
                                );
                                return MPP_NOK;
                            }
                        } else if !matches!(config.profile(), 0 | 66 | 100) {
                            eprintln!(
                                "rk-mpp-rs mpp: H.264 profile {} is not supported",
                                config.profile()
                            );
                            return MPP_NOK;
                        } else if encoder.force_reference_slot(None).is_err() {
                            return MPP_NOK;
                        }
                    }
                    if !context.encoder_ref_pattern.is_empty() {
                        context.encoder_ref_schedule = Some(ReferenceSchedule::new(
                            context.encoder_ref_pattern.clone(),
                            encoder_ref_long_term,
                            config.gop() as usize,
                        ));
                    }
                    if !context.rc_api.is_native_default() {
                        let Some(name) = context.rc_api.name() else {
                            return MPP_NOK;
                        };
                        if !encoder.supports_external_rate_control() {
                            eprintln!(
                                "rk-mpp-rs mpp: selected RC API {name:?} is unavailable on this encoder"
                            );
                            return MPP_NOK;
                        }
                        if let Err(error) =
                            encoder.set_rate_control_name(name.to_string_lossy().into_owned())
                        {
                            eprintln!("rk-mpp-rs mpp: RC strategy name rejected: {error}");
                            return MPP_NOK;
                        }
                        let mut rc_cfg = rc_api::RcCfg::from_encoder(config);
                        let session = match rc_api::Session::new(name, context.coding, &mut rc_cfg)
                        {
                            Ok(session) => session,
                            Err(error) => {
                                eprintln!("rk-mpp-rs mpp: RC strategy rejected: {error}");
                                return MPP_NOK;
                            }
                        };
                        context.custom_rc = Some(session);
                        context.custom_rc_pending = None;
                        context.custom_rc_max_reencodes = rc_cfg.max_reencode_times.max(0);
                        context.custom_rc_sequence = 0;
                        context.custom_force_idr = false;
                    } else {
                        context.custom_rc = None;
                        context.custom_rc_pending = None;
                        context.custom_rc_max_reencodes = 0;
                        context.custom_rc_sequence = 0;
                        context.custom_force_idr = false;
                    }
                    context.encoder = Some(encoder);
                    MPP_OK
                }
                Err(error) => {
                    eprintln!("rk-mpp-rs mpp: encoder open failed: {error}");
                    MPP_NOK
                }
            }
        }
        MPP_ENC_GET_CFG | MPP_SET_OUTPUT_TIMEOUT => MPP_OK,
        MPP_ENC_SET_OSD_PLT_CFG => {
            if context.ctx_type != MPP_CTX_ENC {
                return MPP_ERR_VALUE;
            }
            context.osd_palette.set(param.cast())
        }
        MPP_ENC_GET_OSD_PLT_CFG => {
            if context.ctx_type != MPP_CTX_ENC {
                return MPP_ERR_VALUE;
            }
            context.osd_palette.get(param.cast())
        }
        MPP_ENC_SET_OSD_DATA_CFG => {
            if context.ctx_type != MPP_CTX_ENC || param.is_null() {
                return MPP_ERR_VALUE;
            }
            MPP_OK
        }
        MPP_ENC_GET_RC_API_ALL => {
            if context.ctx_type != MPP_CTX_ENC {
                return MPP_ERR_VALUE;
            }
            unsafe { rc_api::query_all(param.cast()) }
        }
        MPP_ENC_GET_RC_API_BY_TYPE => {
            if context.ctx_type != MPP_CTX_ENC {
                return MPP_ERR_VALUE;
            }
            unsafe { rc_api::query_by_type(param.cast()) }
        }
        MPP_ENC_SET_RC_API_CFG => {
            if context.ctx_type != MPP_CTX_ENC {
                return MPP_ERR_VALUE;
            }
            unsafe { rc_api::add(param.cast()) }
        }
        MPP_ENC_GET_RC_API_CURRENT => {
            if context.ctx_type != MPP_CTX_ENC {
                return MPP_ERR_VALUE;
            }
            context.rc_api.get(param.cast())
        }
        MPP_ENC_SET_RC_API_CURRENT => {
            if context.ctx_type != MPP_CTX_ENC {
                return MPP_ERR_VALUE;
            }
            context.rc_api.set(param.cast(), context.coding)
        }
        MPP_ENC_SET_IDR_FRAME => {
            if context.ctx_type != MPP_CTX_ENC {
                return MPP_ERR_VALUE;
            }
            let Some(encoder) = context.encoder.as_mut() else {
                return MPP_ERR_VALUE;
            };
            if encoder.force_idr().is_err() {
                return MPP_NOK;
            }
            context.custom_force_idr = true;
            if let Some(schedule) = context.encoder_ref_schedule.as_mut() {
                schedule.force_idr();
            }
            MPP_OK
        }
        MPP_ENC_SET_SEI_CFG => {
            if context.ctx_type != MPP_CTX_ENC || param.is_null() {
                return MPP_ERR_VALUE;
            }
            let mode = unsafe { *param.cast::<u32>() };
            if mode > 2 {
                return MPP_NOK;
            }
            let Some(encoder) = context.encoder.as_mut() else {
                return MPP_ERR_VALUE;
            };
            if encoder.set_sei_enabled(mode != 0).is_err() && context.coding == MPP_VIDEO_CODING_AVC
            {
                return MPP_NOK;
            }
            MPP_OK
        }
        MPP_ENC_SET_HEADER_MODE => {
            if context.ctx_type != MPP_CTX_ENC || param.is_null() {
                return MPP_ERR_VALUE;
            }
            let mode = unsafe { *param.cast::<u32>() };
            if mode > 1 {
                return MPP_NOK;
            }
            let Some(encoder) = context.encoder.as_mut() else {
                return MPP_ERR_VALUE;
            };
            if encoder.set_headers_each_idr(mode == 1).is_err()
                && context.coding == MPP_VIDEO_CODING_AVC
            {
                return MPP_NOK;
            }
            MPP_OK
        }
        MPP_ENC_SET_REF_CFG => {
            if context.ctx_type != MPP_CTX_ENC || param.is_null() {
                return MPP_ERR_VALUE;
            }
            context.encoder_ref_pattern =
                standalone::reference_frame_pattern(param).unwrap_or_default();
            let encoder_ref_long_term = standalone::reference_long_term_config(param);
            context.encoder_ref_schedule = (!context.encoder_ref_pattern.is_empty()).then(|| {
                let igop = context
                    .encoder
                    .as_ref()
                    .map_or(60, |encoder| encoder.config().gop() as usize);
                ReferenceSchedule::new(
                    context.encoder_ref_pattern.clone(),
                    encoder_ref_long_term,
                    igop,
                )
            });
            MPP_OK
        }
        MPP_ENC_GET_HDR_SYNC => {
            if context.ctx_type != MPP_CTX_ENC {
                return MPP_ERR_VALUE;
            }
            context.header_sync_requested = true;
            if context.coding == MPP_VIDEO_CODING_HEVC {
                if let Some(encoder) = context.encoder.as_mut() {
                    if let Err(error) = encoder.set_h265_mpi_compat() {
                        eprintln!("rk-mpp-rs mpp: H.265 header mode rejected: {error}");
                        return MPP_NOK;
                    }
                }
            }
            MPP_OK
        }
        _ => {
            eprintln!("rk-mpp-rs mpp: unsupported control command {command}");
            MPP_NOK
        }
    }
}

fn coding_supported(ctx_type: c_int, coding: c_int) -> bool {
    match ctx_type {
        MPP_CTX_DEC => matches!(
            coding,
            MPP_VIDEO_CODING_MPEG2
                | MPP_VIDEO_CODING_H263
                | MPP_VIDEO_CODING_MPEG4
                | MPP_VIDEO_CODING_AVC
                | MPP_VIDEO_CODING_MJPEG
                | MPP_VIDEO_CODING_VP8
                | MPP_VIDEO_CODING_VP9
                | MPP_VIDEO_CODING_HEVC
                | MPP_VIDEO_CODING_AV1
        ),
        MPP_CTX_ENC => matches!(
            coding,
            MPP_VIDEO_CODING_AVC | MPP_VIDEO_CODING_MJPEG | MPP_VIDEO_CODING_HEVC
        ),
        _ => false,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_create(ctx: *mut MppCtx, mpi: *mut *mut MppApi) -> c_int {
    if ctx.is_null() || mpi.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    // SAFETY: the two out pointers were checked non-null.
    unsafe {
        *ctx = std::ptr::null_mut();
        *mpi = std::ptr::null_mut();
        let context = Box::new(CompatContext {
            magic: CONTEXT_MAGIC,
            ctx_type: MPP_CTX_BUTT,
            coding: 0,
            decoder: None,
            encoder: None,
            eos_submitted: false,
            eos_delivered: false,
            pending_output_frame: std::ptr::null_mut(),
            pending_input_packet: std::ptr::null_mut(),
            pending_packet_motion_info: VecDeque::new(),
            encoder_ref_pattern: Vec::new(),
            encoder_ref_schedule: None,
            rc_api: rc_api::Selection::new(),
            header_sync_requested: false,
            custom_rc: None,
            custom_rc_pending: None,
            custom_rc_max_reencodes: 0,
            custom_rc_sequence: 0,
            custom_force_idr: false,
            osd_palette: osd::PaletteState::new(),
        });
        *ctx = Box::into_raw(context).cast();
        *mpi = std::ptr::addr_of!(MPP_API).cast_mut();
    }
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_init(ctx: MppCtx, ctx_type: c_int, coding: c_int) -> c_int {
    let Some(context) = (unsafe { context_mut(ctx) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if ctx_type >= MPP_CTX_BUTT || !coding_supported(ctx_type, coding) {
        return MPP_ERR_VALUE;
    }
    if ctx_type == MPP_CTX_ISP {
        return MPP_NOK;
    }
    let decoder = if ctx_type == MPP_CTX_DEC {
        let Some(codec) = sdk_codec(coding) else {
            return MPP_NOK;
        };
        let config = sdk::DecoderConfig::new(codec)
            .with_output_format(sdk::PixelFormat::Nv12)
            .with_dmabuf_output(matches!(
                coding,
                MPP_VIDEO_CODING_AVC
                    | MPP_VIDEO_CODING_MJPEG
                    | MPP_VIDEO_CODING_VP9
                    | MPP_VIDEO_CODING_HEVC
                    | MPP_VIDEO_CODING_AV1
            ));
        match sdk::Decoder::new(config) {
            Ok(decoder) => Some(decoder),
            Err(_) => return MPP_NOK,
        }
    } else {
        None
    };
    context.ctx_type = ctx_type;
    context.coding = coding;
    context.decoder = decoder;
    context.encoder = None;
    context.eos_submitted = false;
    context.eos_delivered = false;
    context.pending_output_frame = std::ptr::null_mut();
    context.pending_input_packet = std::ptr::null_mut();
    context.pending_packet_motion_info.clear();
    context.encoder_ref_pattern.clear();
    context.encoder_ref_schedule = None;
    MPP_OK
}

fn sdk_codec(coding: c_int) -> Option<sdk::Codec> {
    match coding {
        MPP_VIDEO_CODING_MPEG2 => Some(sdk::Codec::Mpeg2),
        MPP_VIDEO_CODING_H263 => Some(sdk::Codec::H263),
        MPP_VIDEO_CODING_MPEG4 => Some(sdk::Codec::Mpeg4),
        MPP_VIDEO_CODING_AVC => Some(sdk::Codec::H264),
        MPP_VIDEO_CODING_MJPEG => Some(sdk::Codec::Mjpeg),
        MPP_VIDEO_CODING_VP8 => Some(sdk::Codec::Vp8),
        MPP_VIDEO_CODING_VP9 => Some(sdk::Codec::Vp9),
        MPP_VIDEO_CODING_HEVC => Some(sdk::Codec::H265),
        MPP_VIDEO_CODING_AV1 => Some(sdk::Codec::Av1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn step(
        non_reference: bool,
        temporal_id: u8,
        reference_mode: u8,
    ) -> standalone::ReferenceFrameStep {
        standalone::ReferenceFrameStep {
            non_reference,
            temporal_id,
            reference_mode,
            reference_argument: 0,
        }
    }

    #[test]
    fn tsvc3_schedule_tracks_references_and_reorders_temporal_zero() {
        let mut schedule = ReferenceSchedule::new(
            vec![
                step(true, 2, REF_TO_PREV_REF_FRM),
                step(false, 1, REF_TO_PREV_REF_FRM),
                step(true, 2, REF_TO_PREV_REF_FRM),
                step(false, 0, REF_TO_TEMPORAL_LAYER),
            ],
            None,
            60,
        );
        let mut frames = Vec::new();
        for _ in 0..9 {
            let frame = schedule.prepare().unwrap();
            frames.push((
                frame.current_slot,
                frame.reference_slot,
                frame.non_reference,
                frame.temporal_id,
                frame.reference_reorder,
            ));
            schedule.commit(frame);
        }
        assert_eq!(
            frames,
            [
                (0, 0, false, 0, None),
                (1, 0, true, 2, None),
                (1, 0, false, 1, None),
                (2, 1, true, 2, None),
                (
                    2,
                    0,
                    false,
                    0,
                    Some(encoder::ReferenceReorder::ShortTerm(
                        encoder::ShortTermReorder::new(2, 0),
                    )),
                ),
                (0, 2, true, 2, None),
                (0, 2, false, 1, None),
                (1, 0, true, 2, None),
                (
                    1,
                    2,
                    false,
                    0,
                    Some(encoder::ReferenceReorder::ShortTerm(
                        encoder::ShortTermReorder::new(4, 2),
                    )),
                ),
            ]
        );
        assert_eq!(schedule.frame_num, 4);
        assert_eq!(schedule.next_sequence, 9);
    }

    #[test]
    fn tsvc4_schedule_replaces_the_long_term_reference_every_eight_frames() {
        let mut schedule = ReferenceSchedule::new(
            vec![
                step(true, 3, REF_TO_PREV_REF_FRM),
                step(false, 2, REF_TO_PREV_REF_FRM),
                step(true, 3, REF_TO_PREV_REF_FRM),
                step(false, 1, REF_TO_PREV_LT_REF),
                step(true, 3, REF_TO_PREV_REF_FRM),
                step(false, 2, REF_TO_PREV_REF_FRM),
                step(true, 3, REF_TO_PREV_REF_FRM),
                step(false, 0, REF_TO_TEMPORAL_LAYER),
            ],
            Some(standalone::LongTermReferenceConfig {
                index: 0,
                gap: 8,
                delay: 0,
                reference_mode: REF_TO_PREV_LT_REF,
                reference_argument: 0,
            }),
            60,
        );
        let mut frames = Vec::new();
        for _ in 0..12 {
            let frame = schedule.prepare().unwrap();
            frames.push((
                frame.current_slot,
                frame.reference_slot,
                frame.non_reference,
                frame.temporal_id,
                frame.reference_reorder,
                frame.current_long_term_index,
            ));
            schedule.commit(frame);
        }
        assert_eq!(
            frames,
            [
                (0, 0, false, 0, None, Some(0)),
                (1, 0, true, 3, None, None),
                (1, 0, false, 2, None, None),
                (2, 1, true, 3, None, None),
                (
                    2,
                    0,
                    false,
                    1,
                    Some(encoder::ReferenceReorder::LongTerm {
                        long_term_pic_num: 0
                    }),
                    None
                ),
                (1, 2, true, 3, None, None),
                (1, 2, false, 2, None, None),
                (2, 1, true, 3, None, None),
                (
                    2,
                    0,
                    false,
                    0,
                    Some(encoder::ReferenceReorder::LongTerm {
                        long_term_pic_num: 0
                    }),
                    Some(0)
                ),
                (
                    0,
                    2,
                    true,
                    3,
                    Some(encoder::ReferenceReorder::LongTerm {
                        long_term_pic_num: 0
                    }),
                    None
                ),
                (
                    0,
                    2,
                    false,
                    2,
                    Some(encoder::ReferenceReorder::LongTerm {
                        long_term_pic_num: 0
                    }),
                    None
                ),
                (1, 0, true, 3, None, None),
            ]
        );
        assert_eq!(schedule.frame_num, 6);
        assert_eq!(schedule.active[0].sequence, 8);
        assert_eq!(schedule.active[1].sequence, 10);
    }

    #[test]
    fn reference_schedule_restarts_at_the_igop_boundary() {
        let mut schedule = ReferenceSchedule::new(
            vec![
                step(true, 1, REF_TO_PREV_REF_FRM),
                step(false, 0, REF_TO_PREV_REF_FRM),
            ],
            None,
            3,
        );
        for _ in 0..3 {
            let frame = schedule.prepare().unwrap();
            schedule.commit(frame);
        }
        assert_eq!(schedule.next_sequence, 0);
        assert!(schedule.active.is_empty());
        let idr = schedule.prepare().unwrap();
        assert_eq!(idr.current_slot, 0);
        assert!(!idr.non_reference);
        assert_eq!(idr.current_long_term_index, None);
    }

    #[test]
    fn forced_idr_resets_the_reference_schedule_mid_gop() {
        let mut schedule = ReferenceSchedule::new(
            vec![
                step(true, 1, REF_TO_PREV_REF_FRM),
                step(false, 0, REF_TO_PREV_REF_FRM),
            ],
            None,
            60,
        );
        for _ in 0..4 {
            let frame = schedule.prepare().unwrap();
            schedule.commit(frame);
        }
        schedule.force_idr();
        let idr = schedule.prepare().unwrap();
        assert_eq!(idr.current_slot, 0);
        assert_eq!(idr.reference_slot, 0);
        assert_eq!(idr.frame_num, 0);
        assert_eq!(idr.poc, 0);
        assert!(!idr.non_reference);
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_destroy(ctx: MppCtx) -> c_int {
    if ctx.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    // SAFETY: the magic check validates a context allocated by mpp_create().
    let context = unsafe { &mut *(ctx.cast::<CompatContext>()) };
    if context.magic != CONTEXT_MAGIC {
        return MPP_ERR_VALUE;
    }
    context.magic = 0;
    // SAFETY: ownership is returned to Box exactly once after the magic was
    // consumed by this destroy call.
    drop(unsafe { Box::from_raw(ctx.cast::<CompatContext>()) });
    MPP_OK
}

#[no_mangle]
pub extern "C" fn mpp_check_support_format(ctx_type: c_int, coding: c_int) -> c_int {
    if coding_supported(ctx_type, coding) {
        MPP_OK
    } else {
        MPP_NOK
    }
}

#[no_mangle]
pub extern "C" fn mpp_show_support_format() {
    println!("rk-mpp-rs compatibility formats:");
    println!("  dec: MPEG2 H263 MPEG4 AVC MJPEG VP8 VP9 HEVC AV1");
    println!("  enc: AVC MJPEG HEVC");
}

#[no_mangle]
pub extern "C" fn mpp_show_color_format() {
    println!("rk-mpp-rs color formats: NV12 I420 I420P10");
}

const _: () = {
    assert!(size_of::<MppApi>() == 184);
};
