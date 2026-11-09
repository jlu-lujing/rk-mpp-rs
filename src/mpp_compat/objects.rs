use std::collections::BTreeMap;
use std::ffi::{c_char, c_int, c_void};
use std::mem::size_of;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::sdk;
use crate::service::dma_heap::DmaBuf;

const MPP_OK: c_int = 0;
const MPP_NOK: c_int = -1;
const MPP_ERR_NULL_PTR: c_int = -3;
const MPP_ERR_MALLOC: c_int = -4;
const MPP_ERR_VALUE: c_int = -6;

const MPP_BUFFER_TYPE_DMA_HEAP: c_int = 4;
const MPP_BUFFER_INTERNAL: c_int = 0;
const MPP_BUFFER_EXTERNAL: c_int = 1;

const MPP_FMT_YUV420SP: u32 = 0;
const MPP_FMT_YUV420P: u32 = 4;

const BUFFER_MAGIC: u64 = 0x4d50_5052_4246_4652;
const GROUP_MAGIC: u64 = 0x4d50_5052_4752_4f50;
const META_MAGIC: u64 = 0x4d50_5052_4d45_5441;
const PACKET_MAGIC: u64 = 0x4d50_5052_504b_5458;
const FRAME_MAGIC: u64 = 0x4d50_5052_4652_4d45;

pub(super) type MppBuffer = *mut c_void;
pub(super) type MppBufferGroup = *mut c_void;
pub(super) type MppPacket = *mut c_void;
pub(super) type MppFrame = *mut c_void;
type MppMeta = *mut c_void;

const fn fourcc(a: u8, b: u8, c: u8, d: u8) -> c_int {
    ((a as c_int) << 24) | ((b as c_int) << 16) | ((c as c_int) << 8) | d as c_int
}

const KEY_INPUT_FRAME: c_int = fourcc(b'i', b'f', b'r', b'm');
const KEY_INPUT_PACKET: c_int = fourcc(b'i', b'p', b'k', b't');
const KEY_OUTPUT_FRAME: c_int = fourcc(b'o', b'f', b'r', b'm');
const KEY_OUTPUT_PACKET: c_int = fourcc(b'o', b'p', b'k', b't');
const KEY_MOTION_INFO: c_int = fourcc(b'm', b'v', b'i', b'f');
const KEY_HDR_INFO: c_int = fourcc(b'h', b'd', b'r', b' ');
const KEY_HDR_META_OFFSET: c_int = fourcc(b'h', b'd', b'r', b'o');
const KEY_HDR_META_SIZE: c_int = fourcc(b'h', b'd', b'r', b'l');
const KEY_INPUT_BLOCK: c_int = fourcc(b'i', b'b', b'l', b'k');
const KEY_OUTPUT_BLOCK: c_int = fourcc(b'o', b'b', b'l', b'k');
const KEY_INPUT_IDR_REQ: c_int = fourcc(b'i', b'i', b'd', b'r');
const KEY_OUTPUT_INTRA: c_int = fourcc(b'o', b'i', b'd', b'r');
const KEY_TEMPORAL_ID: c_int = fourcc(b't', b'l', b'i', b'd');
const KEY_LONG_REF_IDX: c_int = fourcc(b'l', b't', b'i', b'd');
const KEY_ENC_AVERAGE_QP: c_int = fourcc(b'a', b'v', b'g', b'q');
const KEY_ENC_START_QP: c_int = fourcc(b's', b't', b'r', b'q');
const KEY_ENC_BPS_RT: c_int = fourcc(b'r', b't', b'b', b'r');
const KEY_ROI_DATA: c_int = fourcc(b'r', b'o', b'i', b' ');
const KEY_JPEG_ROI_DATA: c_int = fourcc(b'j', b'r', b'o', b'i');
const KEY_OSD_DATA: c_int = fourcc(b'o', b's', b'd', b' ');
const KEY_OSD_DATA2: c_int = fourcc(b'o', b's', b'd', b'2');
const KEY_OSD_DATA3: c_int = fourcc(b'o', b's', b'd', b'3');
const KEY_USER_DATA: c_int = fourcc(b'u', b's', b'r', b'd');
const KEY_USER_DATAS: c_int = fourcc(b'u', b'r', b'd', b's');
const KEY_LVL64_INTER_NUM: c_int = fourcc(b'l', b'6', b'4', b'p');
const KEY_LVL32_INTER_NUM: c_int = fourcc(b'l', b'3', b'2', b'p');
const KEY_LVL16_INTER_NUM: c_int = fourcc(b'l', b'1', b'6', b'p');
const KEY_LVL8_INTER_NUM: c_int = fourcc(b'l', b'8', b'p', b' ');
const KEY_LVL32_INTRA_NUM: c_int = fourcc(b'l', b'3', b'2', b'i');
const KEY_LVL16_INTRA_NUM: c_int = fourcc(b'l', b'1', b'6', b'i');
const KEY_LVL8_INTRA_NUM: c_int = fourcc(b'l', b'8', b'i', b' ');
const KEY_LVL4_INTRA_NUM: c_int = fourcc(b'l', b'4', b'i', b' ');
const KEY_ENC_MADI_B16: c_int = fourcc(b'm', b'a', b'd', b'i');
const KEY_ENC_MADP_CTU: c_int = fourcc(b'm', b'a', b'd', b'p');
const KEY_OUTPUT_PSKIP: c_int = fourcc(b'o', b'p', b's', b'p');
const KEY_INPUT_PSKIP: c_int = fourcc(b'i', b'p', b's', b'p');
const KEY_INPUT_PSKIP_NON_REF: c_int = fourcc(b'i', b'p', b'n', b'r');
const KEY_INPUT_PSKIP_NUM: c_int = fourcc(b'i', b'p', b's', b'n');
const KEY_ENC_SSE: c_int = fourcc(b'e', b's', b's', b'e');
const KEY_ROI_DATA2: c_int = fourcc(b'r', b'o', b'i', b'2');
const KEY_QPMAP0: c_int = fourcc(b'e', b'q', b'm', b'0');
const KEY_NPU_SOBJ_FLAG: c_int = fourcc(b'n', b'p', b'u', b's');
const KEY_NPU_UOBJ_FLAG: c_int = fourcc(b'n', b'p', b'u', b'u');
const KEY_BUFFER_UPSCALE: c_int = fourcc(b'u', b'b', b'u', b'f');
const KEY_BUFFER_DOWNSCALE: c_int = fourcc(b'd', b'b', b'u', b'f');
const KEY_ENC_MARK_LTR: c_int = fourcc(b'm', b'l', b't', b'r');
const KEY_ENC_USE_LTR: c_int = fourcc(b'u', b'l', b't', b'r');
const KEY_ENC_FRAME_QP: c_int = fourcc(b'f', b'r', b'm', b'q');
const KEY_ENC_BASE_LAYER_PID: c_int = fourcc(b'b', b'p', b'i', b'd');
const KEY_DEC_TBN_EN: c_int = fourcc(b't', b'b', b'e', b'n');
const KEY_DEC_TBN_Y_OFFSET: c_int = fourcc(b't', b'b', b'y', b'o');
const KEY_DEC_TBN_UV_OFFSET: c_int = fourcc(b't', b'b', b'c', b'o');

pub(super) struct FrameInputView {
    pub data: *const u8,
    pub len: usize,
    pub width: usize,
    pub height: usize,
    pub hor_stride: usize,
    pub ver_stride: usize,
    pub format: u32,
    pub pts: i64,
    pub dts: i64,
    pub eos: bool,
    pub motion_info: MppBuffer,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MetaType {
    S32,
    S64,
    Ptr,
    Object,
}

fn meta_key_type(key: c_int) -> Option<MetaType> {
    match key {
        KEY_INPUT_FRAME | KEY_OUTPUT_FRAME | KEY_INPUT_PACKET | KEY_OUTPUT_PACKET
        | KEY_MOTION_INFO | KEY_HDR_INFO | KEY_QPMAP0 | KEY_NPU_SOBJ_FLAG | KEY_BUFFER_UPSCALE
        | KEY_BUFFER_DOWNSCALE => Some(MetaType::Object),
        KEY_ROI_DATA | KEY_ROI_DATA2 | KEY_JPEG_ROI_DATA | KEY_OSD_DATA | KEY_OSD_DATA2
        | KEY_OSD_DATA3 | KEY_USER_DATA | KEY_USER_DATAS | KEY_NPU_UOBJ_FLAG => Some(MetaType::Ptr),
        KEY_ENC_SSE => Some(MetaType::S64),
        KEY_HDR_META_OFFSET
        | KEY_HDR_META_SIZE
        | KEY_OUTPUT_INTRA
        | KEY_INPUT_BLOCK
        | KEY_OUTPUT_BLOCK
        | KEY_INPUT_IDR_REQ
        | KEY_TEMPORAL_ID
        | KEY_LONG_REF_IDX
        | KEY_ENC_AVERAGE_QP
        | KEY_ENC_START_QP
        | KEY_ENC_BPS_RT
        | KEY_LVL64_INTER_NUM
        | KEY_LVL32_INTER_NUM
        | KEY_LVL16_INTER_NUM
        | KEY_LVL8_INTER_NUM
        | KEY_LVL32_INTRA_NUM
        | KEY_LVL16_INTRA_NUM
        | KEY_LVL8_INTRA_NUM
        | KEY_LVL4_INTRA_NUM
        | KEY_ENC_MADI_B16
        | KEY_ENC_MADP_CTU
        | KEY_OUTPUT_PSKIP
        | KEY_INPUT_PSKIP
        | KEY_INPUT_PSKIP_NON_REF
        | KEY_INPUT_PSKIP_NUM
        | KEY_ENC_MARK_LTR
        | KEY_ENC_USE_LTR
        | KEY_ENC_FRAME_QP
        | KEY_ENC_BASE_LAYER_PID
        | KEY_DEC_TBN_EN
        | KEY_DEC_TBN_Y_OFFSET
        | KEY_DEC_TBN_UV_OFFSET => Some(MetaType::S32),
        _ => None,
    }
}

#[derive(Clone, Copy)]
enum MetaValue {
    S32(i32),
    S64(i64),
    Ptr(*mut c_void),
    Object(*mut c_void),
}

#[repr(C)]
struct CompatEncUserData {
    len: u32,
    pdata: *mut c_void,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct CompatEncUserDataFull {
    len: u32,
    uuid: *mut u8,
    pdata: *mut c_void,
}

#[repr(C)]
struct CompatEncUserDataSet {
    count: u32,
    datas: *mut CompatEncUserDataFull,
}

struct OwnedUserData {
    public: CompatEncUserData,
    data: Box<[u8]>,
}

impl Clone for OwnedUserData {
    fn clone(&self) -> Self {
        let data = self.data.clone();
        Self {
            public: CompatEncUserData {
                len: self.public.len,
                pdata: data.as_ptr().cast_mut().cast(),
            },
            data,
        }
    }
}

struct OwnedUserDataSet {
    public: CompatEncUserDataSet,
    entries: Box<[CompatEncUserDataFull]>,
    _uuids: Vec<Box<[u8]>>,
    _payloads: Vec<Box<[u8]>>,
}

impl Clone for OwnedUserDataSet {
    fn clone(&self) -> Self {
        let count = self.entries.len();
        let mut entries = vec![
            CompatEncUserDataFull {
                len: 0,
                uuid: std::ptr::null_mut(),
                pdata: std::ptr::null_mut(),
            };
            count
        ]
        .into_boxed_slice();
        let mut uuids = Vec::with_capacity(count);
        let mut payloads = Vec::with_capacity(count);
        for (index, source) in self.entries.iter().enumerate() {
            entries[index].len = source.len;
            if !source.uuid.is_null() {
                let uuid = unsafe { std::slice::from_raw_parts(source.uuid, 16) }
                    .to_vec()
                    .into_boxed_slice();
                entries[index].uuid = uuid.as_ptr().cast_mut();
                uuids.push(uuid);
            }
            if !source.pdata.is_null() {
                let payload = unsafe {
                    std::slice::from_raw_parts(source.pdata.cast::<u8>(), source.len as usize)
                }
                .to_vec()
                .into_boxed_slice();
                entries[index].pdata = payload.as_ptr().cast_mut().cast();
                payloads.push(payload);
            }
        }
        let datas = entries.as_mut_ptr();
        Self {
            public: CompatEncUserDataSet {
                count: count as u32,
                datas,
            },
            entries,
            _uuids: uuids,
            _payloads: payloads,
        }
    }
}

#[repr(C)]
struct CompatMeta {
    magic: u64,
    refs: AtomicU32,
    entries: BTreeMap<c_int, MetaValue>,
    user_data: Option<OwnedUserData>,
    user_data_set: Option<OwnedUserDataSet>,
}

#[repr(C)]
pub(super) struct CompatBufferInfo {
    type_: c_int,
    size: usize,
    ptr: *mut c_void,
    hnd: *mut c_void,
    fd: c_int,
    index: c_int,
}

#[repr(C)]
struct CompatBuffer {
    magic: u64,
    size: usize,
    data: Vec<u8>,
    dma: Option<DmaBuf>,
    external_ptr: *mut u8,
    external_size: usize,
    mapped: bool,
    type_: c_int,
    hnd: *mut c_void,
    group: MppBufferGroup,
    fd: c_int,
    index: c_int,
    offset: usize,
    refs: usize,
}

#[repr(C)]
struct CompatBufferGroup {
    magic: u64,
    type_: c_int,
    mode: c_int,
    limit_size: usize,
    limit_count: isize,
    usage: usize,
    buffer_count: usize,
    unused: Vec<MppBuffer>,
}

#[repr(C)]
struct CompatPacket {
    magic: u64,
    data: *mut u8,
    size: usize,
    position: usize,
    length: usize,
    pts: i64,
    dts: i64,
    flag: u32,
    eos: bool,
    extra_data: bool,
    buffer: MppBuffer,
    meta: MppMeta,
    owned: Option<Box<[u8]>>,
}

#[repr(C)]
pub(super) struct CompatFrame {
    magic: u64,
    width: u32,
    height: u32,
    hor_stride: u32,
    ver_stride: u32,
    format: u32,
    offset_x: u32,
    offset_y: u32,
    mode: u32,
    discard: u32,
    viewid: u32,
    poc: u32,
    pts: i64,
    dts: i64,
    errinfo: u32,
    buffer_size: usize,
    eos: u32,
    info_change: u32,
    thumbnail: u32,
    buffer: MppBuffer,
    meta: MppMeta,
}

unsafe fn magic_ref<T>(ptr: *mut c_void, magic: u64) -> Option<&'static T> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the C object contract requires a live object of this type. The
    // first word is a magic value so common null/stale uses fail deterministically.
    let object = unsafe { &*ptr.cast::<u64>() };
    (*object == magic).then(|| unsafe { &*ptr.cast::<T>() })
}

unsafe fn magic_mut<T>(ptr: *mut c_void, magic: u64) -> Option<&'static mut T> {
    if ptr.is_null() {
        return None;
    }
    // SAFETY: the C object contract requires a live object of this type. The
    // magic check mirrors the reference object implementation.
    let object = unsafe { &mut *ptr.cast::<u64>() };
    (*object == magic).then(|| unsafe { &mut *ptr.cast::<T>() })
}

fn buffer_ptr(buffer: &CompatBuffer) -> *mut u8 {
    if let Some(dma) = &buffer.dma {
        dma.as_slice().as_ptr().cast_mut()
    } else if buffer.external_ptr.is_null() {
        buffer.data.as_ptr().cast_mut()
    } else {
        buffer.external_ptr
    }
}

fn buffer_len(buffer: &CompatBuffer) -> usize {
    buffer.size
}

impl Drop for CompatBuffer {
    fn drop(&mut self) {
        if self.mapped && !self.external_ptr.is_null() && self.external_size != 0 {
            unsafe {
                libc::munmap(self.external_ptr.cast::<c_void>(), self.external_size);
            }
        }
    }
}

unsafe fn buffer_ref(buffer: MppBuffer) -> Option<&'static CompatBuffer> {
    unsafe { magic_ref(buffer, BUFFER_MAGIC) }
}

unsafe fn buffer_mut(buffer: MppBuffer) -> Option<&'static mut CompatBuffer> {
    unsafe { magic_mut(buffer, BUFFER_MAGIC) }
}

unsafe fn group_ref(group: MppBufferGroup) -> Option<&'static CompatBufferGroup> {
    unsafe { magic_ref(group, GROUP_MAGIC) }
}

unsafe fn group_mut(group: MppBufferGroup) -> Option<&'static mut CompatBufferGroup> {
    unsafe { magic_mut(group, GROUP_MAGIC) }
}

unsafe fn group_drop_unused(group: &mut CompatBufferGroup) {
    for buffer in group.unused.drain(..) {
        let Some(object) = (unsafe { buffer_mut(buffer) }) else {
            continue;
        };
        let size = buffer_len(object);
        object.magic = 0;
        group.usage = group.usage.saturating_sub(size);
        group.buffer_count = group.buffer_count.saturating_sub(1);
        drop(unsafe { Box::from_raw(buffer.cast::<CompatBuffer>()) });
    }
}

pub(super) unsafe fn buffer_put_raw(buffer: MppBuffer) -> bool {
    if buffer.is_null() {
        return false;
    }
    let Some(object) = (unsafe { buffer_mut(buffer) }) else {
        return false;
    };
    if object.refs == 0 {
        return false;
    }
    object.refs -= 1;
    if object.refs != 0 {
        return true;
    }
    let group = object.group;
    if !group.is_null() {
        if let Some(group_object) = unsafe { group_mut(group) } {
            object.refs = 0;
            group_object.unused.push(buffer);
            return true;
        }
    }
    object.magic = 0;
    // SAFETY: refs reached zero and only mpp_buffer_get/inc_ref created owners.
    drop(unsafe { Box::from_raw(buffer.cast::<CompatBuffer>()) });
    true
}

unsafe fn packet_ref(packet: MppPacket) -> Option<&'static CompatPacket> {
    unsafe { magic_ref(packet, PACKET_MAGIC) }
}

unsafe fn packet_mut(packet: MppPacket) -> Option<&'static mut CompatPacket> {
    unsafe { magic_mut(packet, PACKET_MAGIC) }
}

fn new_meta() -> MppMeta {
    Box::into_raw(Box::new(CompatMeta {
        magic: META_MAGIC,
        refs: AtomicU32::new(1),
        entries: BTreeMap::new(),
        user_data: None,
        user_data_set: None,
    }))
    .cast()
}

unsafe fn meta_ref(meta: MppMeta) -> Option<&'static CompatMeta> {
    unsafe { magic_ref(meta, META_MAGIC) }
}

unsafe fn meta_mut(meta: MppMeta) -> Option<&'static mut CompatMeta> {
    unsafe { magic_mut(meta, META_MAGIC) }
}

unsafe fn meta_put_raw(meta: MppMeta) -> bool {
    let Some(object) = (unsafe { meta_mut(meta) }) else {
        return false;
    };
    if object.refs.load(Ordering::Acquire) == 0 {
        return false;
    }
    if object.refs.fetch_sub(1, Ordering::AcqRel) != 1 {
        return true;
    }
    object.magic = 0;
    // SAFETY: the final metadata reference was released above.
    drop(unsafe { Box::from_raw(meta.cast::<CompatMeta>()) });
    true
}

unsafe fn meta_get_object(meta: MppMeta, key: c_int) -> Option<*mut c_void> {
    let object = unsafe { meta_mut(meta) }?;
    if meta_key_type(key) != Some(MetaType::Object) {
        return None;
    }
    match object.entries.remove(&key) {
        Some(MetaValue::Object(value)) => Some(value),
        Some(value) => {
            object.entries.insert(key, value);
            None
        }
        None => None,
    }
}

unsafe fn meta_set_object(meta: MppMeta, key: c_int, value: *mut c_void) -> c_int {
    let Some(object) = (unsafe { meta_mut(meta) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if meta_key_type(key) != Some(MetaType::Object) {
        return MPP_NOK;
    }
    object.entries.insert(key, MetaValue::Object(value));
    MPP_OK
}

unsafe fn meta_peek_object(meta: MppMeta, key: c_int) -> Option<*mut c_void> {
    let object = unsafe { meta_ref(meta) }?;
    match object.entries.get(&key) {
        Some(MetaValue::Object(value)) => Some(*value),
        _ => None,
    }
}

pub(super) unsafe fn packet_bytes(packet: MppPacket) -> Option<&'static [u8]> {
    let packet = unsafe { packet_ref(packet) }?;
    if packet.length == 0 {
        return Some(&[]);
    }
    if packet.data.is_null() || packet.position > packet.size {
        return None;
    }
    let available = packet.size - packet.position;
    let length = packet.length.min(available);
    Some(unsafe { std::slice::from_raw_parts(packet.data.add(packet.position), length) })
}

pub(super) unsafe fn packet_metadata(packet: MppPacket) -> Option<(i64, i64, bool)> {
    let packet = unsafe { packet_ref(packet) }?;
    Some((packet.pts, packet.dts, packet.eos))
}

pub(super) unsafe fn packet_output_frame(packet: MppPacket) -> Option<MppFrame> {
    let packet = unsafe { packet_ref(packet) }?;
    unsafe { meta_get_object(packet.meta, KEY_OUTPUT_FRAME) }.filter(|frame| !frame.is_null())
}

pub(super) unsafe fn frame_set_input_packet(frame: MppFrame, packet: MppPacket) -> bool {
    let Some(frame) = (unsafe { frame_mut(frame) }) else {
        return false;
    };
    if frame.meta.is_null() {
        frame.meta = new_meta();
    }
    unsafe { meta_set_object(frame.meta, KEY_INPUT_PACKET, packet) == MPP_OK }
}

pub(super) unsafe fn frame_input_view(frame: MppFrame) -> Option<FrameInputView> {
    let frame = unsafe { frame_ref(frame) }?;
    let motion_info =
        unsafe { meta_peek_object(frame.meta, KEY_MOTION_INFO) }.unwrap_or(std::ptr::null_mut());
    let buffer = unsafe { buffer_mut(frame.buffer) }?;
    let data = buffer_ptr(buffer);
    let len = buffer_len(buffer);
    Some(FrameInputView {
        data,
        len,
        width: frame.width as usize,
        height: frame.height as usize,
        hor_stride: frame.hor_stride as usize,
        ver_stride: frame.ver_stride as usize,
        format: frame.format,
        pts: frame.pts,
        dts: frame.dts,
        eos: frame.eos != 0,
        motion_info,
    })
}

pub(super) unsafe fn buffer_replace(buffer: MppBuffer, data: &[u8]) -> bool {
    let Some(buffer) = (unsafe { buffer_mut(buffer) }) else {
        return false;
    };
    if data.len() > buffer_len(buffer) {
        return false;
    }
    if let Some(dma) = &buffer.dma {
        if dma.sync_write_begin().is_err() {
            return false;
        }
    }
    unsafe {
        std::ptr::copy_nonoverlapping(data.as_ptr(), buffer_ptr(buffer), data.len());
    }
    if let Some(dma) = &buffer.dma {
        if dma.sync_write_end().is_err() {
            return false;
        }
    }
    true
}

pub(super) unsafe fn packet_from_bytes(bytes: Vec<u8>, pts: i64, dts: i64) -> MppPacket {
    let owned = bytes.into_boxed_slice();
    let data = owned.as_ptr() as *mut u8;
    let packet = new_packet(data, owned.len());
    // SAFETY: packet was just allocated above.
    let packet_object = unsafe { packet_mut(packet) }.unwrap();
    packet_object.owned = Some(owned);
    packet_object.pts = pts;
    packet_object.dts = dts;
    packet
}

pub(super) unsafe fn packet_consume(packet: MppPacket) -> bool {
    let Some(packet) = (unsafe { packet_mut(packet) }) else {
        return false;
    };
    packet.position = packet
        .position
        .saturating_add(packet.length)
        .min(packet.size);
    packet.length = packet.size - packet.position;
    true
}

unsafe fn frame_ref(frame: MppFrame) -> Option<&'static CompatFrame> {
    unsafe { magic_ref(frame, FRAME_MAGIC) }
}

unsafe fn frame_mut(frame: MppFrame) -> Option<&'static mut CompatFrame> {
    unsafe { magic_mut(frame, FRAME_MAGIC) }
}

fn align(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

fn copy_rows(
    destination: &mut [u8],
    destination_stride: usize,
    source: &[u8],
    source_stride: usize,
    width: usize,
    height: usize,
) -> bool {
    if source_stride < width
        || destination_stride < width
        || source.len() < source_stride.saturating_mul(height)
        || destination.len() < destination_stride.saturating_mul(height)
    {
        return false;
    }
    for row in 0..height {
        let source_offset = row * source_stride;
        let destination_offset = row * destination_stride;
        destination[destination_offset..destination_offset + width]
            .copy_from_slice(&source[source_offset..source_offset + width]);
    }
    true
}

unsafe fn new_packet(data: *mut u8, size: usize) -> MppPacket {
    Box::into_raw(Box::new(CompatPacket {
        magic: PACKET_MAGIC,
        data,
        size,
        position: 0,
        length: size,
        pts: 0,
        dts: 0,
        flag: 0,
        eos: false,
        extra_data: false,
        buffer: std::ptr::null_mut(),
        meta: std::ptr::null_mut(),
        owned: None,
    }))
    .cast()
}

unsafe fn new_internal_buffer(
    size: usize,
    initial: Option<&[u8]>,
    type_: c_int,
    group: MppBufferGroup,
) -> Result<MppBuffer, c_int> {
    if size == 0 || initial.is_some_and(|data| data.len() > size) {
        return Err(MPP_ERR_VALUE);
    }
    if !group.is_null() {
        let Some(group_object) = (unsafe { group_ref(group) }) else {
            return Err(MPP_ERR_NULL_PTR);
        };
        if group_object.limit_size != 0 && size > group_object.limit_size {
            return Err(MPP_NOK);
        }
        if group_object.limit_count > 0
            && group_object.buffer_count >= group_object.limit_count as usize
        {
            return Err(MPP_NOK);
        }
    }
    let (data, dma, fd) = if type_ & 0xffff == 0 {
        let mut data = vec![0; size];
        if let Some(initial) = initial {
            data[..initial.len()].copy_from_slice(initial);
        }
        (data, None, -1)
    } else {
        let mut dma = DmaBuf::alloc("system", size, false).map_err(|_| MPP_ERR_MALLOC)?;
        if let Some(initial) = initial {
            dma.sync_write_begin().map_err(|_| MPP_NOK)?;
            dma.as_mut_slice()[..initial.len()].copy_from_slice(initial);
            dma.sync_write_end().map_err(|_| MPP_NOK)?;
        }
        let fd = dma.fd();
        (Vec::new(), Some(dma), fd)
    };
    if !group.is_null() {
        let Some(group_object) = (unsafe { group_mut(group) }) else {
            return Err(MPP_ERR_NULL_PTR);
        };
        group_object.usage = group_object.usage.saturating_add(size);
        group_object.buffer_count = group_object.buffer_count.saturating_add(1);
    }
    Ok(Box::into_raw(Box::new(CompatBuffer {
        magic: BUFFER_MAGIC,
        size,
        data,
        dma,
        external_ptr: std::ptr::null_mut(),
        external_size: 0,
        mapped: false,
        type_,
        hnd: std::ptr::null_mut(),
        group,
        fd,
        index: 0,
        offset: 0,
        refs: 1,
    }))
    .cast())
}

unsafe fn new_external_buffer(
    info: &CompatBufferInfo,
    group: MppBufferGroup,
) -> Result<MppBuffer, c_int> {
    if info.size == 0 || (info.ptr.is_null() && info.fd < 0) {
        return Err(MPP_ERR_VALUE);
    }
    if !group.is_null() {
        let Some(group_object) = (unsafe { group_ref(group) }) else {
            return Err(MPP_ERR_NULL_PTR);
        };
        if group_object.limit_size != 0 && info.size > group_object.limit_size {
            return Err(MPP_NOK);
        }
        if group_object.limit_count > 0
            && group_object.buffer_count >= group_object.limit_count as usize
        {
            return Err(MPP_NOK);
        }
    }
    let mut mapped = false;
    let pointer = if info.ptr.is_null() {
        let pointer = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                info.size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                info.fd,
                0,
            )
        };
        if pointer == libc::MAP_FAILED {
            return Err(MPP_NOK);
        }
        mapped = true;
        pointer.cast::<u8>()
    } else {
        info.ptr.cast::<u8>()
    };
    if !group.is_null() {
        let Some(group_object) = (unsafe { group_mut(group) }) else {
            return Err(MPP_ERR_NULL_PTR);
        };
        group_object.usage = group_object.usage.saturating_add(info.size);
        group_object.buffer_count = group_object.buffer_count.saturating_add(1);
    }
    Ok(Box::into_raw(Box::new(CompatBuffer {
        magic: BUFFER_MAGIC,
        size: info.size,
        data: Vec::new(),
        dma: None,
        external_ptr: pointer,
        external_size: info.size,
        mapped,
        type_: info.type_,
        hnd: info.hnd,
        group,
        fd: info.fd,
        index: info.index,
        offset: 0,
        refs: 1,
    }))
    .cast())
}

pub(super) unsafe fn frame_from_sdk(decoded: sdk::Frame) -> Result<MppFrame, ()> {
    let width = u32::try_from(decoded.width()).map_err(|_| ())?;
    let height = u32::try_from(decoded.height()).map_err(|_| ())?;
    let hor_stride = align(width, 16);
    let ver_stride = align(height, 16);
    let luma_size = usize::try_from(hor_stride)
        .ok()
        .and_then(|stride| stride.checked_mul(ver_stride as usize))
        .ok_or(())?;
    let format = match decoded.format() {
        sdk::PixelFormat::Nv12 => MPP_FMT_YUV420SP,
        sdk::PixelFormat::I420 => MPP_FMT_YUV420P,
        sdk::PixelFormat::I420P10 => return Err(()),
    };
    let buffer_size = luma_size.checked_mul(3).ok_or(())? / 2;
    let mut packed = vec![0u8; buffer_size];

    let y = decoded.plane(0).ok_or(())?;
    if !copy_rows(
        &mut packed[..luma_size],
        hor_stride as usize,
        y.data,
        y.stride,
        width as usize,
        height as usize,
    ) {
        return Err(());
    }

    if format == MPP_FMT_YUV420SP {
        let uv = decoded.plane(1).ok_or(())?;
        if !copy_rows(
            &mut packed[luma_size..],
            hor_stride as usize,
            uv.data,
            uv.stride,
            width as usize,
            height.div_ceil(2) as usize,
        ) {
            return Err(());
        }
    } else {
        let chroma_height = height.div_ceil(2) as usize;
        let chroma_stride = hor_stride as usize / 2;
        let chroma_size = chroma_stride.checked_mul(chroma_height).ok_or(())?;
        let u = decoded.plane(1).ok_or(())?;
        let v = decoded.plane(2).ok_or(())?;
        let chroma_width = width.div_ceil(2) as usize;
        if !copy_rows(
            &mut packed[luma_size..luma_size + chroma_size],
            chroma_stride,
            u.data,
            u.stride,
            chroma_width,
            chroma_height,
        ) || !copy_rows(
            &mut packed[luma_size + chroma_size..],
            chroma_stride,
            v.data,
            v.stride,
            chroma_width,
            chroma_height,
        ) {
            return Err(());
        }
    }

    let buffer = match unsafe {
        new_internal_buffer(
            packed.len(),
            Some(&packed),
            MPP_BUFFER_TYPE_DMA_HEAP,
            std::ptr::null_mut(),
        )
    } {
        Ok(buffer) => buffer,
        Err(_) => return Err(()),
    };
    let frame = Box::into_raw(Box::new(CompatFrame {
        magic: FRAME_MAGIC,
        width,
        height,
        hor_stride,
        ver_stride,
        format,
        offset_x: 0,
        offset_y: 0,
        mode: 0,
        discard: 0,
        viewid: 0,
        poc: decoded.poc().max(0) as u32,
        pts: decoded.pts(),
        dts: decoded.dts(),
        errinfo: decoded.err_info(),
        buffer_size,
        eos: 0,
        info_change: 0,
        thumbnail: 0,
        buffer,
        meta: std::ptr::null_mut(),
    }))
    .cast();
    Ok(frame)
}

pub(super) unsafe fn frame_fill_from_sdk(
    target_pointer: MppFrame,
    decoded: sdk::Frame,
) -> Result<(), ()> {
    let Some(target) = (unsafe { frame_mut(target_pointer) }) else {
        return Err(());
    };

    let width = u32::try_from(decoded.width()).map_err(|_| ())?;
    let height = u32::try_from(decoded.height()).map_err(|_| ())?;
    let hor_stride = align(width, 16);
    let ver_stride = align(height, 16);
    let luma_size = usize::try_from(hor_stride)
        .ok()
        .and_then(|stride| stride.checked_mul(ver_stride as usize))
        .ok_or(())?;
    let (format, buffer_size) = match decoded.format() {
        sdk::PixelFormat::Nv12 => (MPP_FMT_YUV420SP, luma_size.checked_mul(3).ok_or(())? / 2),
        sdk::PixelFormat::I420 => (MPP_FMT_YUV420P, luma_size.checked_mul(3).ok_or(())? / 2),
        sdk::PixelFormat::I420P10 => return Err(()),
    };

    let mut target_buffer = target.buffer;
    if target_buffer.is_null() {
        target_buffer = unsafe {
            new_internal_buffer(
                buffer_size,
                None,
                MPP_BUFFER_TYPE_DMA_HEAP,
                std::ptr::null_mut(),
            )
        }
        .map_err(|_| ())?;
        target.buffer = target_buffer;
    }
    let Some(buffer) = (unsafe { buffer_mut(target_buffer) }) else {
        return Err(());
    };
    if buffer_len(buffer) < buffer_size {
        return Err(());
    }
    if buffer
        .dma
        .as_ref()
        .is_some_and(|dma| dma.sync_write_begin().is_err())
    {
        return Err(());
    }
    let destination =
        unsafe { std::slice::from_raw_parts_mut(buffer_ptr(buffer), buffer_len(buffer)) };

    let y = decoded.plane(0).ok_or(())?;
    if !copy_rows(
        &mut destination[..luma_size],
        hor_stride as usize,
        y.data,
        y.stride,
        width as usize,
        height as usize,
    ) {
        return Err(());
    }
    if format == MPP_FMT_YUV420SP {
        let uv = decoded.plane(1).ok_or(())?;
        if !copy_rows(
            &mut destination[luma_size..],
            hor_stride as usize,
            uv.data,
            uv.stride,
            width as usize,
            height.div_ceil(2) as usize,
        ) {
            return Err(());
        }
    } else {
        let chroma_height = height.div_ceil(2) as usize;
        let chroma_stride = hor_stride as usize / 2;
        let chroma_size = chroma_stride.checked_mul(chroma_height).ok_or(())?;
        let chroma_width = width.div_ceil(2) as usize;
        let u = decoded.plane(1).ok_or(())?;
        let v = decoded.plane(2).ok_or(())?;
        if !copy_rows(
            &mut destination[luma_size..luma_size + chroma_size],
            chroma_stride,
            u.data,
            u.stride,
            chroma_width,
            chroma_height,
        ) || !copy_rows(
            &mut destination[luma_size + chroma_size..],
            chroma_stride,
            v.data,
            v.stride,
            chroma_width,
            chroma_height,
        ) {
            return Err(());
        }
    }
    if buffer
        .dma
        .as_ref()
        .is_some_and(|dma| dma.sync_write_end().is_err())
    {
        return Err(());
    }

    target.width = width;
    target.height = height;
    target.hor_stride = hor_stride;
    target.ver_stride = ver_stride;
    target.format = format;
    target.poc = decoded.poc().max(0) as u32;
    target.pts = decoded.pts();
    target.dts = decoded.dts();
    target.errinfo = decoded.err_info();
    target.buffer_size = buffer_size;
    target.eos = 0;
    target.info_change = 0;
    Ok(())
}

pub(super) unsafe fn frame_eos() -> MppFrame {
    Box::into_raw(Box::new(CompatFrame {
        magic: FRAME_MAGIC,
        width: 0,
        height: 0,
        hor_stride: 0,
        ver_stride: 0,
        format: 0,
        offset_x: 0,
        offset_y: 0,
        mode: 0,
        discard: 0,
        viewid: 0,
        poc: 0,
        pts: 0,
        dts: 0,
        errinfo: 0,
        buffer_size: 0,
        eos: 1,
        info_change: 0,
        thumbnail: 0,
        buffer: std::ptr::null_mut(),
        meta: std::ptr::null_mut(),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_group_get(
    group: *mut MppBufferGroup,
    type_: c_int,
    mode: c_int,
    _tag: *const c_char,
    _caller: *const c_char,
) -> c_int {
    if group.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    if !matches!(mode, MPP_BUFFER_INTERNAL | MPP_BUFFER_EXTERNAL) {
        return MPP_NOK;
    }
    // SAFETY: the output pointer was checked above.
    unsafe {
        *group = Box::into_raw(Box::new(CompatBufferGroup {
            magic: GROUP_MAGIC,
            type_,
            mode,
            limit_size: 0,
            limit_count: 0,
            usage: 0,
            buffer_count: 0,
            unused: Vec::new(),
        }))
        .cast();
    }
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_group_put(group: MppBufferGroup) -> c_int {
    let Some(object) = (unsafe { group_mut(group) }) else {
        return MPP_ERR_NULL_PTR;
    };
    unsafe { group_drop_unused(object) };
    object.magic = 0;
    // SAFETY: mpp_buffer_group_get allocated this Box and ownership ends here.
    drop(unsafe { Box::from_raw(group.cast::<CompatBufferGroup>()) });
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_group_clear(group: MppBufferGroup) -> c_int {
    let Some(group) = (unsafe { group_mut(group) }) else {
        return MPP_ERR_NULL_PTR;
    };
    unsafe { group_drop_unused(group) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_group_unused(group: MppBufferGroup) -> c_int {
    unsafe { group_ref(group) }
        .map(|group| group.unused.len().min(c_int::MAX as usize) as c_int)
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_group_usage(group: MppBufferGroup) -> usize {
    unsafe { group_ref(group) }
        .map(|group| group.usage)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_group_mode(group: MppBufferGroup) -> c_int {
    unsafe { group_ref(group) }
        .map(|group| group.mode)
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_group_type(group: MppBufferGroup) -> c_int {
    unsafe { group_ref(group) }
        .map(|group| group.type_)
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_group_limit_config(
    group: MppBufferGroup,
    size: usize,
    count: isize,
) -> c_int {
    let Some(group) = (unsafe { group_mut(group) }) else {
        return MPP_ERR_NULL_PTR;
    };
    group.limit_size = size;
    group.limit_count = count;
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_import_with_tag(
    group: MppBufferGroup,
    info: *mut CompatBufferInfo,
    buffer: *mut MppBuffer,
    _tag: *const c_char,
    _caller: *const c_char,
) -> c_int {
    if info.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let info = unsafe { &*info };
    if buffer.is_null() && group.is_null() {
        return MPP_ERR_VALUE;
    }
    if !group.is_null() {
        let Some(group_object) = (unsafe { group_mut(group) }) else {
            return MPP_ERR_NULL_PTR;
        };
        if group_object.mode != MPP_BUFFER_EXTERNAL
            || (group_object.type_ & 0xffff) != (info.type_ & 0xffff)
        {
            return MPP_NOK;
        }
    }
    let buffer_object = match unsafe { new_external_buffer(info, group) } {
        Ok(buffer) => buffer,
        Err(error) => return error,
    };
    if buffer.is_null() {
        let Some(object) = (unsafe { buffer_mut(buffer_object) }) else {
            return MPP_NOK;
        };
        object.refs = 0;
        let Some(group) = (unsafe { group_mut(group) }) else {
            return MPP_ERR_NULL_PTR;
        };
        group.unused.push(buffer_object);
    } else {
        unsafe { *buffer = buffer_object };
    }
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_get_with_tag(
    group: MppBufferGroup,
    buffer: *mut MppBuffer,
    size: usize,
    _tag: *const c_char,
    _caller: *const c_char,
) -> c_int {
    if buffer.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    unsafe { *buffer = std::ptr::null_mut() };
    if size == 0 {
        return MPP_ERR_VALUE;
    }
    if !group.is_null() {
        let Some(group) = (unsafe { group_mut(group) }) else {
            return MPP_ERR_NULL_PTR;
        };
        let found = group.unused.iter().position(|&candidate| {
            unsafe { buffer_ref(candidate) }.is_some_and(|object| buffer_len(object) >= size)
        });
        if let Some(index) = found {
            let candidate = group.unused.remove(index);
            if let Some(object) = unsafe { buffer_mut(candidate) } {
                object.refs = 1;
            }
            unsafe { *buffer = candidate };
            return MPP_OK;
        }
        if group.mode == MPP_BUFFER_EXTERNAL {
            return MPP_NOK;
        }
    }
    let type_ = if group.is_null() {
        MPP_BUFFER_TYPE_DMA_HEAP
    } else {
        let Some(group) = (unsafe { group_ref(group) }) else {
            return MPP_ERR_NULL_PTR;
        };
        group.type_
    };
    match unsafe { new_internal_buffer(size, None, type_, group) } {
        Ok(allocation) => {
            unsafe { *buffer = allocation };
            MPP_OK
        }
        Err(error) => error,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_put_with_caller(
    buffer: MppBuffer,
    _caller: *const c_char,
) -> c_int {
    if unsafe { buffer_put_raw(buffer) } {
        MPP_OK
    } else {
        MPP_ERR_NULL_PTR
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_inc_ref_with_caller(
    buffer: MppBuffer,
    _caller: *const c_char,
) -> c_int {
    let Some(buffer) = (unsafe { buffer_mut(buffer) }) else {
        return MPP_ERR_NULL_PTR;
    };
    buffer.refs = buffer.refs.saturating_add(1);
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_discard_with_caller(
    buffer: MppBuffer,
    caller: *const c_char,
) -> c_int {
    unsafe { mpp_buffer_put_with_caller(buffer, caller) }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_info_get_with_caller(
    buffer: MppBuffer,
    info: *mut CompatBufferInfo,
    _caller: *const c_char,
) -> c_int {
    if info.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let Some(buffer) = (unsafe { buffer_mut(buffer) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let ptr = buffer_ptr(buffer).cast();
    // SAFETY: the caller supplied writable MppBufferInfo storage.
    unsafe {
        info.write(CompatBufferInfo {
            type_: buffer.type_,
            size: buffer_len(buffer),
            ptr,
            hnd: buffer.hnd,
            fd: buffer.fd,
            index: buffer.index,
        });
    }
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_read_with_caller(
    buffer: MppBuffer,
    offset: usize,
    data: *mut c_void,
    size: usize,
    _caller: *const c_char,
) -> c_int {
    if data.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let Some(buffer) = (unsafe { buffer_mut(buffer) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let length = buffer_len(buffer);
    let Some(end) = offset.checked_add(size) else {
        return MPP_ERR_VALUE;
    };
    if end > length {
        return MPP_ERR_VALUE;
    }
    if buffer
        .dma
        .as_ref()
        .is_some_and(|dma| dma.sync_read_begin().is_err())
    {
        return MPP_NOK;
    }
    // SAFETY: bounds were checked and the C destination is writable for size.
    unsafe {
        std::ptr::copy_nonoverlapping(buffer_ptr(buffer).add(offset), data.cast::<u8>(), size);
    }
    if buffer
        .dma
        .as_ref()
        .is_some_and(|dma| dma.sync_read_end().is_err())
    {
        MPP_NOK
    } else {
        MPP_OK
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_write_with_caller(
    buffer: MppBuffer,
    offset: usize,
    data: *mut c_void,
    size: usize,
    _caller: *const c_char,
) -> c_int {
    if data.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let Some(buffer) = (unsafe { buffer_mut(buffer) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let length = buffer_len(buffer);
    let Some(end) = offset.checked_add(size) else {
        return MPP_ERR_VALUE;
    };
    if end > length {
        return MPP_ERR_VALUE;
    }
    if buffer
        .dma
        .as_ref()
        .is_some_and(|dma| dma.sync_write_begin().is_err())
    {
        return MPP_NOK;
    }
    // SAFETY: bounds were checked and the C source is readable for size.
    unsafe {
        std::ptr::copy_nonoverlapping(data.cast::<u8>(), buffer_ptr(buffer).add(offset), size);
    }
    if buffer
        .dma
        .as_ref()
        .is_some_and(|dma| dma.sync_write_end().is_err())
    {
        MPP_NOK
    } else {
        MPP_OK
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_get_ptr_with_caller(
    buffer: MppBuffer,
    _caller: *const c_char,
) -> *mut c_void {
    unsafe { buffer_mut(buffer) }
        .map(|buffer| buffer_ptr(buffer).cast())
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_get_fd_with_caller(
    buffer: MppBuffer,
    _caller: *const c_char,
) -> c_int {
    unsafe { buffer_ref(buffer) }
        .map(|buffer| buffer.fd)
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_get_size_with_caller(
    buffer: MppBuffer,
    _caller: *const c_char,
) -> usize {
    unsafe { buffer_ref(buffer) }.map(buffer_len).unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_get_index_with_caller(
    buffer: MppBuffer,
    _caller: *const c_char,
) -> c_int {
    unsafe { buffer_ref(buffer) }
        .map(|buffer| buffer.index)
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_set_index_with_caller(
    buffer: MppBuffer,
    index: c_int,
    _caller: *const c_char,
) -> c_int {
    let Some(buffer) = (unsafe { buffer_mut(buffer) }) else {
        return MPP_ERR_NULL_PTR;
    };
    buffer.index = index;
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_get_offset_with_caller(
    buffer: MppBuffer,
    _caller: *const c_char,
) -> usize {
    unsafe { buffer_ref(buffer) }
        .map(|buffer| buffer.offset)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_set_offset_with_caller(
    buffer: MppBuffer,
    offset: usize,
    _caller: *const c_char,
) -> c_int {
    let Some(buffer) = (unsafe { buffer_mut(buffer) }) else {
        return MPP_ERR_NULL_PTR;
    };
    buffer.offset = offset;
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_sync_begin_f(
    buffer: MppBuffer,
    read_only: c_int,
    _caller: *const c_char,
) -> c_int {
    let Some(buffer) = (unsafe { buffer_ref(buffer) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let Some(dma) = &buffer.dma else {
        return MPP_OK;
    };
    let result = if read_only != 0 {
        dma.sync_read_begin()
    } else {
        dma.sync_write_begin()
    };
    if result.is_ok() {
        MPP_OK
    } else {
        MPP_NOK
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_sync_end_f(
    buffer: MppBuffer,
    read_only: c_int,
    _caller: *const c_char,
) -> c_int {
    let Some(buffer) = (unsafe { buffer_ref(buffer) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let Some(dma) = &buffer.dma else {
        return MPP_OK;
    };
    let result = if read_only != 0 {
        dma.sync_read_end()
    } else {
        dma.sync_write_end()
    };
    if result.is_ok() {
        MPP_OK
    } else {
        MPP_NOK
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_sync_partial_begin_f(
    buffer: MppBuffer,
    read_only: c_int,
    _offset: u32,
    _length: u32,
    caller: *const c_char,
) -> c_int {
    unsafe { mpp_buffer_sync_begin_f(buffer, read_only, caller) }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_sync_partial_end_f(
    buffer: MppBuffer,
    read_only: c_int,
    _offset: u32,
    _length: u32,
    caller: *const c_char,
) -> c_int {
    unsafe { mpp_buffer_sync_end_f(buffer, read_only, caller) }
}

#[no_mangle]
pub extern "C" fn mpp_buffer_total_now() -> u32 {
    0
}

#[no_mangle]
pub extern "C" fn mpp_buffer_total_max() -> u32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_new(packet: *mut MppPacket) -> c_int {
    if packet.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    // SAFETY: output pointer is writable by contract.
    unsafe { *packet = new_packet(std::ptr::null_mut(), 0) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_init(
    packet: *mut MppPacket,
    data: *mut c_void,
    size: usize,
) -> c_int {
    if packet.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    // SAFETY: output pointer is writable by contract.
    unsafe { *packet = new_packet(data.cast(), size) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_init_with_buffer(
    packet: *mut MppPacket,
    buffer: MppBuffer,
) -> c_int {
    let Some(buffer_object) = (unsafe { buffer_ref(buffer) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let size = buffer_len(buffer_object);
    let data = unsafe { mpp_buffer_get_ptr_with_caller(buffer, std::ptr::null()) };
    let result = unsafe { mpp_packet_init(packet, data, size) };
    if result == MPP_OK {
        unsafe { mpp_packet_set_buffer(*packet, buffer) };
    }
    result
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_copy_init(
    packet: *mut MppPacket,
    source_pointer: MppPacket,
) -> c_int {
    if packet.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let Some(source) = (unsafe { packet_ref(source_pointer) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let Some(bytes) = (unsafe { packet_bytes(source_pointer) }) else {
        return MPP_ERR_VALUE;
    };
    let owned = bytes.to_vec().into_boxed_slice();
    let data = owned.as_ptr() as *mut u8;
    let copied = new_packet(data, owned.len());
    // SAFETY: copied was just allocated and is valid.
    let object = unsafe { packet_mut(copied) }.unwrap();
    object.owned = Some(owned);
    object.pts = source.pts;
    object.dts = source.dts;
    object.flag = source.flag;
    object.eos = source.eos;
    object.extra_data = source.extra_data;
    // SAFETY: output pointer is writable by contract.
    unsafe { *packet = copied };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_deinit(packet: *mut MppPacket) -> c_int {
    if packet.is_null() || unsafe { *packet }.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let object = unsafe { *packet };
    let Some(packet_object) = (unsafe { packet_mut(object) }) else {
        return MPP_ERR_VALUE;
    };
    let buffer = packet_object.buffer;
    let meta = packet_object.meta;
    packet_object.magic = 0;
    // SAFETY: packet_init/new allocated this object and ownership ends here.
    drop(unsafe { Box::from_raw(object.cast::<CompatPacket>()) });
    if !buffer.is_null() {
        let _ = unsafe { buffer_put_raw(buffer) };
    }
    if !meta.is_null() {
        let _ = unsafe { meta_put_raw(meta) };
    }
    unsafe { *packet = std::ptr::null_mut() };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_set_data(packet: MppPacket, data: *mut c_void) {
    if let Some(packet) = unsafe { packet_mut(packet) } {
        packet.data = data.cast();
        packet.position = 0;
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_set_size(packet: MppPacket, size: usize) {
    if let Some(packet) = unsafe { packet_mut(packet) } {
        packet.size = size;
        packet.position = packet.position.min(size);
        packet.length = size - packet.position;
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_set_pos(packet: MppPacket, pos: *mut c_void) {
    let Some(packet) = (unsafe { packet_mut(packet) }) else {
        return;
    };
    if packet.data.is_null() || pos.is_null() {
        return;
    }
    let base = packet.data as usize;
    let address = pos as usize;
    if address < base || address - base > packet.size {
        return;
    }
    let new_position = address - base;
    if new_position >= packet.position {
        let difference = new_position - packet.position;
        if difference <= packet.length {
            packet.length -= difference;
        } else {
            packet.length = packet.size - new_position;
        }
    } else {
        packet.length = packet.size - new_position;
    }
    packet.position = new_position;
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_set_length(packet: MppPacket, length: usize) {
    if let Some(packet) = unsafe { packet_mut(packet) } {
        packet.length = length.min(packet.size - packet.position);
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_get_data(packet: MppPacket) -> *mut c_void {
    unsafe { packet_ref(packet) }
        .map(|packet| packet.data.cast())
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_get_pos(packet: MppPacket) -> *mut c_void {
    let Some(packet) = (unsafe { packet_ref(packet) }) else {
        return std::ptr::null_mut();
    };
    if packet.data.is_null() {
        std::ptr::null_mut()
    } else {
        unsafe { packet.data.add(packet.position) }.cast()
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_get_size(packet: MppPacket) -> usize {
    unsafe { packet_ref(packet) }
        .map(|packet| packet.size)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_get_length(packet: MppPacket) -> usize {
    unsafe { packet_ref(packet) }
        .map(|packet| packet.length)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_set_pts(packet: MppPacket, pts: i64) {
    if let Some(packet) = unsafe { packet_mut(packet) } {
        packet.pts = pts;
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_get_pts(packet: MppPacket) -> i64 {
    unsafe { packet_ref(packet) }
        .map(|packet| packet.pts)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_set_dts(packet: MppPacket, dts: i64) {
    if let Some(packet) = unsafe { packet_mut(packet) } {
        packet.dts = dts;
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_get_dts(packet: MppPacket) -> i64 {
    unsafe { packet_ref(packet) }
        .map(|packet| packet.dts)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_set_flag(packet: MppPacket, flag: u32) {
    if let Some(packet) = unsafe { packet_mut(packet) } {
        packet.flag = flag;
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_get_flag(packet: MppPacket) -> u32 {
    unsafe { packet_ref(packet) }
        .map(|packet| packet.flag)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_set_eos(packet: MppPacket) -> c_int {
    let Some(packet) = (unsafe { packet_mut(packet) }) else {
        return MPP_ERR_NULL_PTR;
    };
    packet.eos = true;
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_clr_eos(packet: MppPacket) -> c_int {
    let Some(packet) = (unsafe { packet_mut(packet) }) else {
        return MPP_ERR_NULL_PTR;
    };
    packet.eos = false;
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_get_eos(packet: MppPacket) -> u32 {
    unsafe { packet_ref(packet) }
        .map(|packet| u32::from(packet.eos))
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_set_extra_data(packet: MppPacket) -> c_int {
    let Some(packet) = (unsafe { packet_mut(packet) }) else {
        return MPP_ERR_NULL_PTR;
    };
    packet.extra_data = true;
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_set_buffer(packet: MppPacket, buffer: MppBuffer) {
    let Some(packet) = (unsafe { packet_mut(packet) }) else {
        return;
    };
    if packet.buffer == buffer {
        return;
    }
    if !buffer.is_null() {
        let _ = unsafe { mpp_buffer_inc_ref_with_caller(buffer, std::ptr::null()) };
    }
    if !packet.buffer.is_null() {
        let _ = unsafe { buffer_put_raw(packet.buffer) };
    }
    packet.buffer = buffer;
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_get_buffer(packet: MppPacket) -> MppBuffer {
    unsafe { packet_ref(packet) }
        .map(|packet| packet.buffer)
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_read(
    packet: MppPacket,
    offset: usize,
    data: *mut c_void,
    size: usize,
) -> c_int {
    if data.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let Some(packet) = (unsafe { packet_ref(packet) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let Some(end) = offset.checked_add(size) else {
        return MPP_ERR_VALUE;
    };
    if end > packet.size || packet.data.is_null() {
        return MPP_ERR_VALUE;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(packet.data.add(offset), data.cast::<u8>(), size);
    }
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_write(
    packet: MppPacket,
    offset: usize,
    data: *mut c_void,
    size: usize,
) -> c_int {
    if data.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let Some(packet) = (unsafe { packet_mut(packet) }) else {
        return MPP_ERR_NULL_PTR;
    };
    let Some(end) = offset.checked_add(size) else {
        return MPP_ERR_VALUE;
    };
    if end > packet.size || packet.data.is_null() {
        return MPP_ERR_VALUE;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(data.cast::<u8>(), packet.data.add(offset), size);
    }
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_has_meta(packet: MppPacket) -> c_int {
    unsafe { packet_ref(packet) }
        .map(|packet| c_int::from(!packet.meta.is_null()))
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_get_meta(packet: MppPacket) -> MppMeta {
    let Some(packet) = (unsafe { packet_mut(packet) }) else {
        return std::ptr::null_mut();
    };
    if packet.meta.is_null() {
        packet.meta = new_meta();
    }
    packet.meta
}

unsafe fn take_meta_value(
    meta: MppMeta,
    key: c_int,
    expected: MetaType,
) -> Result<Option<MetaValue>, c_int> {
    let Some(object) = (unsafe { meta_mut(meta) }) else {
        return Err(MPP_ERR_NULL_PTR);
    };
    if meta_key_type(key) != Some(expected) {
        return Err(MPP_NOK);
    }
    let Some(value) = object.entries.remove(&key) else {
        return Ok(None);
    };
    let matches = matches!(
        (expected, value),
        (MetaType::S32, MetaValue::S32(_))
            | (MetaType::S64, MetaValue::S64(_))
            | (MetaType::Ptr, MetaValue::Ptr(_))
            | (MetaType::Object, MetaValue::Object(_))
    );
    if !matches {
        object.entries.insert(key, value);
        return Err(MPP_NOK);
    }
    Ok(Some(value))
}

unsafe fn replace_user_data(meta: &mut CompatMeta, source: *mut CompatEncUserData) {
    if source.is_null() {
        meta.user_data = None;
        return;
    }
    let source = unsafe { &*source };
    let Ok(len) = usize::try_from(source.len) else {
        meta.user_data = None;
        return;
    };
    if source.pdata.is_null() || len == 0 {
        meta.user_data = None;
        return;
    }
    let data = unsafe { std::slice::from_raw_parts(source.pdata.cast::<u8>(), len) }
        .to_vec()
        .into_boxed_slice();
    meta.user_data = Some(OwnedUserData {
        public: CompatEncUserData {
            len: source.len,
            pdata: data.as_ptr().cast_mut().cast(),
        },
        data,
    });
}

unsafe fn replace_user_data_set(meta: &mut CompatMeta, source: *mut CompatEncUserDataSet) {
    if source.is_null() {
        meta.user_data_set = None;
        return;
    }
    let source_set = unsafe { &*source };
    let Ok(count) = usize::try_from(source_set.count) else {
        meta.user_data_set = None;
        return;
    };
    if count == 0 || source_set.datas.is_null() {
        meta.user_data_set = None;
        return;
    }
    let source_entries = unsafe { std::slice::from_raw_parts(source_set.datas, count) };
    let mut entries = vec![
        CompatEncUserDataFull {
            len: 0,
            uuid: std::ptr::null_mut(),
            pdata: std::ptr::null_mut(),
        };
        count
    ]
    .into_boxed_slice();
    let mut uuids = Vec::with_capacity(count);
    let mut payloads = Vec::with_capacity(count);
    for (index, source) in source_entries.iter().enumerate() {
        entries[index].len = source.len;
        if !source.uuid.is_null() {
            let uuid = unsafe { std::slice::from_raw_parts(source.uuid, 16) }
                .to_vec()
                .into_boxed_slice();
            entries[index].uuid = uuid.as_ptr().cast_mut();
            uuids.push(uuid);
        }
        if !source.pdata.is_null() {
            let payload = unsafe {
                std::slice::from_raw_parts(source.pdata.cast::<u8>(), source.len as usize)
            }
            .to_vec()
            .into_boxed_slice();
            entries[index].pdata = payload.as_ptr().cast_mut().cast();
            payloads.push(payload);
        }
    }
    let datas = entries.as_mut_ptr();
    meta.user_data_set = Some(OwnedUserDataSet {
        public: CompatEncUserDataSet {
            count: count as u32,
            datas,
        },
        entries,
        _uuids: uuids,
        _payloads: payloads,
    });
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_get_with_tag(
    meta: *mut MppMeta,
    _tag: *const c_char,
    _caller: *const c_char,
) -> c_int {
    if meta.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    unsafe { *meta = new_meta() };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_put(meta: MppMeta) -> c_int {
    if meta.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    if unsafe { meta_put_raw(meta) } {
        MPP_OK
    } else {
        MPP_ERR_VALUE
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_inc_ref(meta: MppMeta) -> c_int {
    let Some(meta) = (unsafe { meta_mut(meta) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if meta.refs.load(Ordering::Acquire) == 0 {
        return MPP_ERR_VALUE;
    }
    meta.refs.fetch_add(1, Ordering::AcqRel);
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_size(meta: MppMeta) -> c_int {
    unsafe { meta_ref(meta) }
        .map(|meta| meta.entries.len().min(c_int::MAX as usize) as c_int)
        .unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_dup(meta: MppMeta) -> MppMeta {
    let Some(meta) = (unsafe { meta_ref(meta) }) else {
        return std::ptr::null_mut();
    };
    Box::into_raw(Box::new(CompatMeta {
        magic: META_MAGIC,
        refs: AtomicU32::new(1),
        entries: meta.entries.clone(),
        user_data: meta.user_data.clone(),
        user_data_set: meta.user_data_set.clone(),
    }))
    .cast()
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_dump(meta: MppMeta) -> c_int {
    let Some(meta) = (unsafe { meta_ref(meta) }) else {
        return MPP_ERR_NULL_PTR;
    };
    println!("rk-mpp-rs meta entries {}", meta.entries.len());
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_set_s32(meta: MppMeta, key: c_int, value: i32) -> c_int {
    let Some(meta) = (unsafe { meta_mut(meta) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if meta_key_type(key) != Some(MetaType::S32) {
        return MPP_NOK;
    }
    meta.entries.insert(key, MetaValue::S32(value));
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_get_s32(meta: MppMeta, key: c_int, value: *mut i32) -> c_int {
    if value.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    match unsafe { take_meta_value(meta, key, MetaType::S32) } {
        Ok(Some(MetaValue::S32(stored))) => {
            unsafe { *value = stored };
            MPP_OK
        }
        Ok(_) => MPP_NOK,
        Err(error) => error,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_get_s32_d(
    meta: MppMeta,
    key: c_int,
    value: *mut i32,
    default: i32,
) -> c_int {
    if value.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    match unsafe { take_meta_value(meta, key, MetaType::S32) } {
        Ok(Some(MetaValue::S32(stored))) => {
            unsafe { *value = stored };
            MPP_OK
        }
        Ok(_) => {
            unsafe { *value = default };
            MPP_NOK
        }
        Err(error) => error,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_set_s64(meta: MppMeta, key: c_int, value: i64) -> c_int {
    let Some(meta) = (unsafe { meta_mut(meta) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if meta_key_type(key) != Some(MetaType::S64) {
        return MPP_NOK;
    }
    meta.entries.insert(key, MetaValue::S64(value));
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_get_s64(meta: MppMeta, key: c_int, value: *mut i64) -> c_int {
    if value.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    match unsafe { take_meta_value(meta, key, MetaType::S64) } {
        Ok(Some(MetaValue::S64(stored))) => {
            unsafe { *value = stored };
            MPP_OK
        }
        Ok(_) => MPP_NOK,
        Err(error) => error,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_get_s64_d(
    meta: MppMeta,
    key: c_int,
    value: *mut i64,
    default: i64,
) -> c_int {
    if value.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    match unsafe { take_meta_value(meta, key, MetaType::S64) } {
        Ok(Some(MetaValue::S64(stored))) => {
            unsafe { *value = stored };
            MPP_OK
        }
        Ok(_) => {
            unsafe { *value = default };
            MPP_NOK
        }
        Err(error) => error,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_set_ptr(meta: MppMeta, key: c_int, value: *mut c_void) -> c_int {
    let Some(meta_object) = (unsafe { meta_mut(meta) }) else {
        return MPP_ERR_NULL_PTR;
    };
    if meta_key_type(key) != Some(MetaType::Ptr) {
        return MPP_NOK;
    }
    if key == KEY_USER_DATA {
        unsafe { replace_user_data(meta_object, value.cast()) };
    } else if key == KEY_USER_DATAS {
        unsafe { replace_user_data_set(meta_object, value.cast()) };
    }
    meta_object.entries.insert(key, MetaValue::Ptr(value));
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_get_ptr(
    meta: MppMeta,
    key: c_int,
    value: *mut *mut c_void,
) -> c_int {
    if value.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    match unsafe { take_meta_value(meta, key, MetaType::Ptr) } {
        Ok(Some(MetaValue::Ptr(stored))) => {
            let output = if key == KEY_USER_DATA {
                let Some(meta) = (unsafe { meta_mut(meta) }) else {
                    return MPP_ERR_VALUE;
                };
                meta.user_data
                    .as_mut()
                    .map(|data| (&mut data.public as *mut CompatEncUserData).cast())
                    .unwrap_or(std::ptr::null_mut())
            } else if key == KEY_USER_DATAS {
                let Some(meta) = (unsafe { meta_mut(meta) }) else {
                    return MPP_ERR_VALUE;
                };
                meta.user_data_set
                    .as_mut()
                    .map(|data| (&mut data.public as *mut CompatEncUserDataSet).cast())
                    .unwrap_or(std::ptr::null_mut())
            } else {
                stored
            };
            unsafe { *value = output };
            MPP_OK
        }
        Ok(_) => MPP_NOK,
        Err(error) => error,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_get_ptr_d(
    meta: MppMeta,
    key: c_int,
    value: *mut *mut c_void,
    default: *mut c_void,
) -> c_int {
    if value.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let result = unsafe { mpp_meta_get_ptr(meta, key, value) };
    if result != MPP_OK && meta_key_type(key) == Some(MetaType::Ptr) {
        unsafe { *value = default };
    }
    result
}

fn meta_store_object(meta: MppMeta, key: c_int, value: *mut c_void) -> c_int {
    unsafe { meta_set_object(meta, key, value) }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_set_frame(meta: MppMeta, key: c_int, frame: MppFrame) -> c_int {
    meta_store_object(meta, key, frame)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_get_frame(
    meta: MppMeta,
    key: c_int,
    frame: *mut MppFrame,
) -> c_int {
    if frame.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    match unsafe { take_meta_value(meta, key, MetaType::Object) } {
        Ok(Some(MetaValue::Object(stored))) => {
            unsafe { *frame = stored };
            MPP_OK
        }
        Ok(_) => MPP_NOK,
        Err(error) => error,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_get_frame_d(
    meta: MppMeta,
    key: c_int,
    frame: *mut MppFrame,
    default: MppFrame,
) -> c_int {
    if frame.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let result = unsafe { mpp_meta_get_frame(meta, key, frame) };
    if result != MPP_OK && meta_key_type(key) == Some(MetaType::Object) {
        unsafe { *frame = default };
    }
    result
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_set_packet(
    meta: MppMeta,
    key: c_int,
    packet: MppPacket,
) -> c_int {
    meta_store_object(meta, key, packet)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_get_packet(
    meta: MppMeta,
    key: c_int,
    packet: *mut MppPacket,
) -> c_int {
    if packet.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    match unsafe { take_meta_value(meta, key, MetaType::Object) } {
        Ok(Some(MetaValue::Object(stored))) => {
            unsafe { *packet = stored };
            MPP_OK
        }
        Ok(_) => MPP_NOK,
        Err(error) => error,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_get_packet_d(
    meta: MppMeta,
    key: c_int,
    packet: *mut MppPacket,
    default: MppPacket,
) -> c_int {
    if packet.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let result = unsafe { mpp_meta_get_packet(meta, key, packet) };
    if result != MPP_OK && meta_key_type(key) == Some(MetaType::Object) {
        unsafe { *packet = default };
    }
    result
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_set_buffer(
    meta: MppMeta,
    key: c_int,
    buffer: MppBuffer,
) -> c_int {
    meta_store_object(meta, key, buffer)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_get_buffer(
    meta: MppMeta,
    key: c_int,
    buffer: *mut MppBuffer,
) -> c_int {
    if buffer.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    match unsafe { take_meta_value(meta, key, MetaType::Object) } {
        Ok(Some(MetaValue::Object(stored))) => {
            unsafe { *buffer = stored };
            MPP_OK
        }
        Ok(_) => MPP_NOK,
        Err(error) => error,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_get_buffer_d(
    meta: MppMeta,
    key: c_int,
    buffer: *mut MppBuffer,
    default: MppBuffer,
) -> c_int {
    if buffer.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let result = unsafe { mpp_meta_get_buffer(meta, key, buffer) };
    if result != MPP_OK && meta_key_type(key) == Some(MetaType::Object) {
        unsafe { *buffer = default };
    }
    result
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_is_partition(_packet: MppPacket) -> u32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_is_soi(_packet: MppPacket) -> u32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_is_eoi(_packet: MppPacket) -> u32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_get_segment_nb(_packet: MppPacket) -> u32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_get_segment_info(_packet: MppPacket) -> *const c_void {
    std::ptr::null()
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_init(frame: *mut MppFrame) -> c_int {
    if frame.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    // SAFETY: output pointer is writable by contract.
    unsafe {
        *frame = Box::into_raw(Box::new(CompatFrame {
            magic: FRAME_MAGIC,
            width: 0,
            height: 0,
            hor_stride: 0,
            ver_stride: 0,
            format: 0,
            offset_x: 0,
            offset_y: 0,
            mode: 0,
            discard: 0,
            viewid: 0,
            poc: 0,
            pts: 0,
            dts: 0,
            errinfo: 0,
            buffer_size: 0,
            eos: 0,
            info_change: 0,
            thumbnail: 0,
            buffer: std::ptr::null_mut(),
            meta: std::ptr::null_mut(),
        }))
        .cast();
    }
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_deinit(frame: *mut MppFrame) -> c_int {
    if frame.is_null() || unsafe { *frame }.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let object = unsafe { *frame };
    let Some(frame_object) = (unsafe { frame_mut(object) }) else {
        return MPP_ERR_VALUE;
    };
    let buffer = frame_object.buffer;
    let meta = frame_object.meta;
    frame_object.magic = 0;
    // SAFETY: frame_init or frame_from_sdk allocated this Box.
    drop(unsafe { Box::from_raw(object.cast::<CompatFrame>()) });
    if !buffer.is_null() {
        let _ = unsafe { buffer_put_raw(buffer) };
    }
    if !meta.is_null() {
        let _ = unsafe { meta_put_raw(meta) };
    }
    unsafe { *frame = std::ptr::null_mut() };
    MPP_OK
}

macro_rules! frame_u32_accessor {
    ($get:ident, $set:ident, $field:ident) => {
        #[no_mangle]
        pub unsafe extern "C" fn $get(frame: MppFrame) -> u32 {
            unsafe { frame_ref(frame) }
                .map(|frame| frame.$field)
                .unwrap_or(0)
        }

        #[no_mangle]
        pub unsafe extern "C" fn $set(frame: MppFrame, value: u32) {
            if let Some(frame) = unsafe { frame_mut(frame) } {
                frame.$field = value;
            }
        }
    };
}

frame_u32_accessor!(mpp_frame_get_width, mpp_frame_set_width, width);
frame_u32_accessor!(mpp_frame_get_height, mpp_frame_set_height, height);
frame_u32_accessor!(
    mpp_frame_get_hor_stride,
    mpp_frame_set_hor_stride,
    hor_stride
);
frame_u32_accessor!(
    mpp_frame_get_ver_stride,
    mpp_frame_set_ver_stride,
    ver_stride
);
frame_u32_accessor!(mpp_frame_get_offset_x, mpp_frame_set_offset_x, offset_x);
frame_u32_accessor!(mpp_frame_get_offset_y, mpp_frame_set_offset_y, offset_y);
frame_u32_accessor!(mpp_frame_get_mode, mpp_frame_set_mode, mode);
frame_u32_accessor!(mpp_frame_get_discard, mpp_frame_set_discard, discard);
frame_u32_accessor!(mpp_frame_get_viewid, mpp_frame_set_viewid, viewid);
frame_u32_accessor!(mpp_frame_get_poc, mpp_frame_set_poc, poc);
frame_u32_accessor!(mpp_frame_get_errinfo, mpp_frame_set_errinfo, errinfo);
frame_u32_accessor!(mpp_frame_get_eos, mpp_frame_set_eos, eos);
frame_u32_accessor!(
    mpp_frame_get_info_change,
    mpp_frame_set_info_change,
    info_change
);
frame_u32_accessor!(
    mpp_frame_get_thumbnail_en,
    mpp_frame_set_thumbnail_en,
    thumbnail
);

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_hor_stride_pixel(frame: MppFrame, value: u32) {
    if let Some(frame) = unsafe { frame_mut(frame) } {
        frame.hor_stride = value;
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_hor_stride_pixel(frame: MppFrame) -> u32 {
    unsafe { frame_ref(frame) }
        .map(|frame| frame.hor_stride)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_fbc_hdr_stride(_frame: MppFrame, _value: u32) {}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_fbc_hdr_stride(_frame: MppFrame) -> u32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_pts(frame: MppFrame) -> i64 {
    unsafe { frame_ref(frame) }
        .map(|frame| frame.pts)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_pts(frame: MppFrame, pts: i64) {
    if let Some(frame) = unsafe { frame_mut(frame) } {
        frame.pts = pts;
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_dts(frame: MppFrame) -> i64 {
    unsafe { frame_ref(frame) }
        .map(|frame| frame.dts)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_dts(frame: MppFrame, dts: i64) {
    if let Some(frame) = unsafe { frame_mut(frame) } {
        frame.dts = dts;
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_buf_size(frame: MppFrame) -> usize {
    unsafe { frame_ref(frame) }
        .map(|frame| frame.buffer_size)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_buf_size(frame: MppFrame, size: usize) {
    if let Some(frame) = unsafe { frame_mut(frame) } {
        frame.buffer_size = size;
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_buffer(frame: MppFrame) -> MppBuffer {
    unsafe { frame_ref(frame) }
        .map(|frame| frame.buffer)
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_buffer(frame: MppFrame, buffer: MppBuffer) {
    let Some(frame) = (unsafe { frame_mut(frame) }) else {
        return;
    };
    if frame.buffer == buffer {
        return;
    }
    if !buffer.is_null() {
        let _ = unsafe { mpp_buffer_inc_ref_with_caller(buffer, std::ptr::null()) };
    }
    if !frame.buffer.is_null() {
        let _ = unsafe { buffer_put_raw(frame.buffer) };
    }
    frame.buffer = buffer;
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_has_meta(frame: MppFrame) -> c_int {
    unsafe { frame_ref(frame) }
        .map(|frame| c_int::from(!frame.meta.is_null()))
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_meta(frame: MppFrame) -> MppMeta {
    let Some(frame) = (unsafe { frame_mut(frame) }) else {
        return std::ptr::null_mut();
    };
    if frame.meta.is_null() {
        frame.meta = new_meta();
    }
    frame.meta
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_meta(frame: MppFrame, meta: MppMeta) {
    let Some(frame) = (unsafe { frame_mut(frame) }) else {
        return;
    };
    if frame.meta == meta {
        return;
    }
    let previous = frame.meta;
    frame.meta = meta;
    if !previous.is_null() {
        let _ = unsafe { meta_put_raw(previous) };
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_fmt(frame: MppFrame) -> u32 {
    unsafe { frame_ref(frame) }
        .map(|frame| frame.format)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_fmt(frame: MppFrame, format: u32) {
    if let Some(frame) = unsafe { frame_mut(frame) } {
        frame.format = format;
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_fmt_layout(_frame: MppFrame) -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_fmt_layout(_frame: MppFrame, _layout: c_int) {}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_fbc_uncompress(_frame: MppFrame) -> u32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_fbc_uncompress(_frame: MppFrame, _value: u32) {}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_fbc_sparse(_frame: MppFrame) -> u32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_fbc_sparse(_frame: MppFrame, _value: u32) {}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_color_range(_frame: MppFrame) -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_color_range(_frame: MppFrame, _value: c_int) {}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_color_primaries(_frame: MppFrame) -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_color_primaries(_frame: MppFrame, _value: c_int) {}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_color_trc(_frame: MppFrame) -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_color_trc(_frame: MppFrame, _value: c_int) {}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_colorspace(_frame: MppFrame) -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_colorspace(_frame: MppFrame, _value: c_int) {}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_chroma_location(_frame: MppFrame) -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_chroma_location(_frame: MppFrame, _value: c_int) {}

const _: () = {
    assert!(size_of::<CompatBufferInfo>() == 40);
};
