use std::ffi::{c_char, c_int, c_void};
use std::mem::size_of;

use crate::sdk;

const MPP_OK: c_int = 0;
const MPP_NOK: c_int = -1;
const MPP_ERR_NULL_PTR: c_int = -3;
const MPP_ERR_VALUE: c_int = -6;

const MPP_BUFFER_TYPE_DMA_HEAP: c_int = 4;
const MPP_BUFFER_INTERNAL: c_int = 0;

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

#[repr(C)]
struct CompatMeta {
    magic: u64,
    output_frame: MppFrame,
    input_packet: MppPacket,
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
    data: Vec<u8>,
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
    meta: Option<Box<CompatMeta>>,
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

fn buffer_ptr(buffer: &mut CompatBuffer) -> *mut u8 {
    buffer.data.as_mut_ptr()
}

fn buffer_len(buffer: &CompatBuffer) -> usize {
    buffer.data.len()
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

unsafe fn meta_mut(meta: MppMeta) -> Option<&'static mut CompatMeta> {
    unsafe { magic_mut(meta, META_MAGIC) }
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
    let meta = packet.meta.as_ref()?;
    (!meta.output_frame.is_null()).then_some(meta.output_frame)
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
        meta: None,
        owned: None,
    }))
    .cast()
}

unsafe fn new_buffer(bytes: Vec<u8>) -> MppBuffer {
    Box::into_raw(Box::new(CompatBuffer {
        magic: BUFFER_MAGIC,
        data: bytes,
        fd: -1,
        index: 0,
        offset: 0,
        refs: 1,
    }))
    .cast()
}

unsafe fn discard_frame_object(frame_pointer: MppFrame) {
    let Some(frame) = (unsafe { frame_mut(frame_pointer) }) else {
        return;
    };
    let buffer = frame.buffer;
    frame.magic = 0;
    // SAFETY: frame_from_sdk/frame_init allocated this Box.
    drop(unsafe { Box::from_raw(frame_pointer.cast::<CompatFrame>()) });
    if !buffer.is_null() {
        let _ = unsafe { buffer_put_raw(buffer) };
    }
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

    let buffer = unsafe { new_buffer(packed) };
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
    let source_pointer = unsafe { frame_from_sdk(decoded) }?;
    let Some(source) = (unsafe { frame_mut(source_pointer) }) else {
        return Err(());
    };
    let Some(target) = (unsafe { frame_mut(target_pointer) }) else {
        unsafe { discard_frame_object(source_pointer) };
        return Err(());
    };
    let source_buffer_pointer = source.buffer;
    let target_buffer_pointer = target.buffer;

    if !target_buffer_pointer.is_null() {
        if target_buffer_pointer == source_buffer_pointer {
            unsafe { discard_frame_object(source_pointer) };
            return Err(());
        }
        let Some(source_buffer) = (unsafe { buffer_ref(source_buffer_pointer) }) else {
            unsafe { discard_frame_object(source_pointer) };
            return Err(());
        };
        let Some(target_buffer) = (unsafe { buffer_mut(target_buffer_pointer) }) else {
            unsafe { discard_frame_object(source_pointer) };
            return Err(());
        };
        if target_buffer.data.len() < source_buffer.data.len() {
            unsafe { discard_frame_object(source_pointer) };
            return Err(());
        }
        target_buffer.data[..source_buffer.data.len()].copy_from_slice(&source_buffer.data);
    } else {
        target.buffer = source_buffer_pointer;
        source.buffer = std::ptr::null_mut();
    }

    target.width = source.width;
    target.height = source.height;
    target.hor_stride = source.hor_stride;
    target.ver_stride = source.ver_stride;
    target.format = source.format;
    target.poc = source.poc;
    target.pts = source.pts;
    target.dts = source.dts;
    target.errinfo = source.errinfo;
    target.buffer_size = source.buffer_size;
    target.eos = source.eos;
    target.info_change = source.info_change;

    unsafe { discard_frame_object(source_pointer) };
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
    if mode != MPP_BUFFER_INTERNAL {
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
    object.magic = 0;
    // SAFETY: mpp_buffer_group_get allocated this Box and ownership ends here.
    drop(unsafe { Box::from_raw(group.cast::<CompatBufferGroup>()) });
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_group_clear(group: MppBufferGroup) -> c_int {
    if unsafe { group_ref(group) }.is_some() {
        MPP_OK
    } else {
        MPP_ERR_NULL_PTR
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_group_unused(_group: MppBufferGroup) -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_group_usage(_group: MppBufferGroup) -> usize {
    0
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
pub unsafe extern "C" fn mpp_buffer_get_with_tag(
    _group: MppBufferGroup,
    buffer: *mut MppBuffer,
    size: usize,
    _tag: *const c_char,
    _caller: *const c_char,
) -> c_int {
    if buffer.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    // SAFETY: the out pointer is writable by contract.
    unsafe {
        *buffer = if size == 0 {
            std::ptr::null_mut()
        } else {
            new_buffer(vec![0; size])
        };
    }
    MPP_OK
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
            type_: MPP_BUFFER_TYPE_DMA_HEAP,
            size: buffer_len(buffer),
            ptr,
            hnd: std::ptr::null_mut(),
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
    // SAFETY: bounds were checked and the C destination is writable for size.
    unsafe {
        std::ptr::copy_nonoverlapping(buffer_ptr(buffer).add(offset), data.cast::<u8>(), size);
    }
    MPP_OK
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
    // SAFETY: bounds were checked and the C source is readable for size.
    unsafe {
        std::ptr::copy_nonoverlapping(data.cast::<u8>(), buffer_ptr(buffer).add(offset), size);
    }
    MPP_OK
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
    _read_only: c_int,
    _caller: *const c_char,
) -> c_int {
    if unsafe { buffer_ref(buffer) }.is_some() {
        MPP_OK
    } else {
        MPP_ERR_NULL_PTR
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_buffer_sync_end_f(
    buffer: MppBuffer,
    _read_only: c_int,
    _caller: *const c_char,
) -> c_int {
    if unsafe { buffer_ref(buffer) }.is_some() {
        MPP_OK
    } else {
        MPP_ERR_NULL_PTR
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
    packet_object.magic = 0;
    // SAFETY: packet_init/new allocated this object and ownership ends here.
    drop(unsafe { Box::from_raw(object.cast::<CompatPacket>()) });
    if !buffer.is_null() {
        let _ = unsafe { buffer_put_raw(buffer) };
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
        .and_then(|packet| packet.meta.as_ref())
        .map(|_| 1)
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_packet_get_meta(packet: MppPacket) -> MppMeta {
    let Some(packet) = (unsafe { packet_mut(packet) }) else {
        return std::ptr::null_mut();
    };
    if packet.meta.is_none() {
        let meta = Box::new(CompatMeta {
            magic: META_MAGIC,
            output_frame: std::ptr::null_mut(),
            input_packet: std::ptr::null_mut(),
        });
        packet.meta = Some(meta);
    }
    packet
        .meta
        .as_mut()
        .map(|meta| (&mut **meta as *mut CompatMeta).cast())
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_set_frame(meta: MppMeta, _key: c_int, frame: MppFrame) -> c_int {
    let Some(meta) = (unsafe { meta_mut(meta) }) else {
        return MPP_ERR_NULL_PTR;
    };
    meta.output_frame = frame;
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_get_frame(
    meta: MppMeta,
    _key: c_int,
    frame: *mut MppFrame,
) -> c_int {
    if frame.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let Some(meta) = (unsafe { meta_mut(meta) }) else {
        return MPP_ERR_NULL_PTR;
    };
    unsafe { *frame = meta.output_frame };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_set_packet(
    meta: MppMeta,
    _key: c_int,
    packet: MppPacket,
) -> c_int {
    let Some(meta) = (unsafe { meta_mut(meta) }) else {
        return MPP_ERR_NULL_PTR;
    };
    meta.input_packet = packet;
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_meta_get_packet(
    meta: MppMeta,
    _key: c_int,
    packet: *mut MppPacket,
) -> c_int {
    if packet.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let Some(meta) = (unsafe { meta_mut(meta) }) else {
        return MPP_ERR_NULL_PTR;
    };
    unsafe { *packet = meta.input_packet };
    MPP_OK
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
    frame_object.magic = 0;
    // SAFETY: frame_init or frame_from_sdk allocated this Box.
    drop(unsafe { Box::from_raw(object.cast::<CompatFrame>()) });
    if !buffer.is_null() {
        let _ = unsafe { buffer_put_raw(buffer) };
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
pub unsafe extern "C" fn mpp_frame_has_meta(_frame: MppFrame) -> c_int {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_get_meta(_frame: MppFrame) -> MppMeta {
    std::ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn mpp_frame_set_meta(_frame: MppFrame, _meta: MppMeta) {}

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
