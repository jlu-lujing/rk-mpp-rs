use std::collections::HashMap;
use std::ffi::c_uint;
use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::service::soc::{SocId, SocProfile};

const MPP_OK: c_int = 0;
const MPP_NOK: c_int = -1;
const MPP_ERR_NULL_PTR: c_int = -3;
const MPP_ERR_VALUE: c_int = -6;
const DEC_CFG_MAGIC: u64 = 0x524b_4445_4343_4647;
const ENC_CFG_MAGIC: u64 = 0x524b_454e_4343_4647;
const REF_CFG_MAGIC: u64 = 0x524b_5245_4643_4647;

#[no_mangle]
pub static mut mpp_debug: c_uint = 0;
#[no_mangle]
pub static mut mpp_cfg_io_debug: c_uint = 0;
#[no_mangle]
pub static mut mpp_hal_debug: c_uint = 0;
#[no_mangle]
pub static mut mpp_enc_hal_debug: c_uint = 0;

#[repr(C)]
pub struct SpinLock {
    lock: AtomicU32,
    debug: u32,
    count: i64,
    time: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct SglnInfo {
    id: i32,
    name: *const c_char,
    init: Option<unsafe extern "C" fn()>,
    deinit: Option<unsafe extern "C" fn()>,
}

#[repr(C)]
pub struct SglnList {
    count: u32,
    capacity: u32,
    entries: *mut SglnInfo,
}

#[repr(C)]
pub struct SglnBase {
    name: *const c_char,
    arrays: [*mut SglnList; 2],
    array_count: u32,
    max_name_len: u32,
    debug: u32,
    init_done: u32,
}

struct ObjDefStub {
    name: CString,
    next_node: i32,
}

fn spinlock_mut(lock: *mut SpinLock) -> Option<&'static mut SpinLock> {
    (!lock.is_null()).then(|| unsafe { &mut *lock })
}

#[cfg(not(any(
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "x86_64", target_os = "linux")
)))]
unsafe fn log_unformatted(tag: *const c_char, format: *const c_char, function: *const c_char) {
    let tag = if tag.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(tag) }.to_string_lossy())
    };
    let function = if function.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(function) }.to_string_lossy())
    };
    let format = if format.is_null() {
        None
    } else {
        Some(unsafe { CStr::from_ptr(format) }.to_string_lossy())
    };
    if let Some(format) = format {
        if let Some(tag) = tag {
            eprint!("{tag}: ");
        }
        if let Some(function) = function {
            eprint!("{function}: ");
        }
        eprintln!("{format}");
    }
}

#[cfg(any(
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "x86_64", target_os = "linux")
))]
#[no_mangle]
#[unsafe(naked)]
pub unsafe extern "C" fn _mpp_log_l(
    _level: c_int,
    _tag: *const c_char,
    _format: *const c_char,
    _function: *const c_char,
) {
    #[cfg(target_arch = "aarch64")]
    core::arch::naked_asm!("b rk_mpp_vlog_l");
    #[cfg(target_arch = "x86_64")]
    core::arch::naked_asm!("jmp rk_mpp_vlog_l");
}

#[cfg(not(any(
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "x86_64", target_os = "linux")
)))]
#[no_mangle]
pub unsafe extern "C" fn _mpp_log_l(
    _level: c_int,
    tag: *const c_char,
    format: *const c_char,
    function: *const c_char,
) {
    log_unformatted(tag, format, function);
}

#[cfg(any(
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "x86_64", target_os = "linux")
))]
#[no_mangle]
#[unsafe(naked)]
pub unsafe extern "C" fn _mpp_log(
    _tag: *const c_char,
    _format: *const c_char,
    _function: *const c_char,
) {
    #[cfg(target_arch = "aarch64")]
    core::arch::naked_asm!("b rk_mpp_vlog");
    #[cfg(target_arch = "x86_64")]
    core::arch::naked_asm!("jmp rk_mpp_vlog");
}

#[cfg(not(any(
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "x86_64", target_os = "linux")
)))]
#[no_mangle]
pub unsafe extern "C" fn _mpp_log(
    tag: *const c_char,
    format: *const c_char,
    function: *const c_char,
) {
    log_unformatted(tag, format, function);
}

#[cfg(any(
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "x86_64", target_os = "linux")
))]
#[no_mangle]
#[unsafe(naked)]
pub unsafe extern "C" fn mpp_llog(
    _level: c_int,
    _tag: *const c_char,
    _format: *const c_char,
    _function: *const c_char,
) {
    #[cfg(target_arch = "aarch64")]
    core::arch::naked_asm!("b rk_mpp_vllog");
    #[cfg(target_arch = "x86_64")]
    core::arch::naked_asm!("jmp rk_mpp_vllog");
}

#[cfg(not(any(
    all(target_arch = "aarch64", target_os = "linux"),
    all(target_arch = "x86_64", target_os = "linux")
)))]
#[no_mangle]
pub unsafe extern "C" fn mpp_llog(
    _level: c_int,
    tag: *const c_char,
    format: *const c_char,
    function: *const c_char,
) {
    log_unformatted(tag, format, function);
}

#[no_mangle]
pub unsafe extern "C" fn mpp_spinlock_init(lock: *mut SpinLock) {
    if let Some(lock) = spinlock_mut(lock) {
        lock.lock.store(0, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_spinlock_deinit(lock: *mut SpinLock, _name: *const c_char) {
    if let Some(lock) = spinlock_mut(lock) {
        lock.lock.store(0, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_spinlock_lock(lock: *mut SpinLock) {
    let Some(lock) = spinlock_mut(lock) else {
        return;
    };
    while lock
        .lock
        .compare_exchange_weak(0, 1, Ordering::Acquire, Ordering::Relaxed)
        .is_err()
    {
        std::hint::spin_loop();
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_spinlock_unlock(lock: *mut SpinLock) {
    if let Some(lock) = spinlock_mut(lock) {
        lock.lock.store(0, Ordering::Release);
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_spinlock_trylock(lock: *mut SpinLock) -> c_int {
    spinlock_mut(lock)
        .is_some_and(|lock| {
            lock.lock
                .compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
        })
        .into()
}

#[no_mangle]
pub unsafe extern "C" fn mpp_sgln_base_add(
    base: *mut SglnBase,
    info: *mut SglnInfo,
    _caller: *const c_char,
) -> i32 {
    if base.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    if info.is_null() {
        return MPP_NOK;
    }
    let base = unsafe { &mut *base };
    if !base.name.is_null() {
        let _ = unsafe { mpp_env_get_u32(base.name, &mut base.debug, 0) };
    }
    let id = unsafe { (*info).id };
    let array_index = usize::from(id < 0 && base.array_count > 1);
    let Some(list) = base
        .arrays
        .get(array_index)
        .copied()
        .filter(|list| !list.is_null())
    else {
        return MPP_NOK;
    };
    let list = unsafe { &mut *list };
    let assigned_id = if id >= 0 {
        if id as u32 >= list.capacity {
            return MPP_NOK;
        }
        if !unsafe { (*list.entries.add(id as usize)).name }.is_null() {
            return MPP_NOK;
        }
        list.count = list.count.max(id as u32 + 1);
        id
    } else {
        if base.init_done != 0 || list.count >= list.capacity {
            return MPP_NOK;
        }
        let assigned = list.count as i32;
        list.count += 1;
        unsafe { (*info).id = assigned };
        assigned
    };
    unsafe { *list.entries.add(assigned_id as usize) = *info };
    if !unsafe { (*info).name }.is_null() {
        let length = unsafe { CStr::from_ptr((*info).name) }.to_bytes().len() as u32;
        base.max_name_len = base.max_name_len.max(length);
    }
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_sgln_base_run_init(base: *mut SglnBase) {
    if base.is_null() {
        return;
    }
    let base = unsafe { &mut *base };
    for index in 0..base.array_count.min(2) as usize {
        let list = base.arrays[index];
        if list.is_null() {
            continue;
        }
        let list = unsafe { &*list };
        for slot in 0..list.count as usize {
            let info = unsafe { *list.entries.add(slot) };
            if !info.name.is_null() {
                if let Some(init) = info.init {
                    unsafe { init() };
                }
            }
        }
    }
    base.init_done = 1;
}

#[no_mangle]
pub unsafe extern "C" fn mpp_sgln_base_deinit(base: *mut SglnBase) {
    if base.is_null() {
        return;
    }
    let base = unsafe { &mut *base };
    for index in (0..base.array_count.min(2) as usize).rev() {
        let list = base.arrays[index];
        if list.is_null() {
            continue;
        }
        let list = unsafe { &*list };
        for slot in (0..list.count as usize).rev() {
            let info = unsafe { *list.entries.add(slot) };
            if !info.name.is_null() {
                if let Some(deinit) = info.deinit {
                    unsafe { deinit() };
                }
            }
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_get(def: *mut *mut c_void, _name: *const c_char) -> i32 {
    if def.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    unsafe { def.write(ptr::null_mut()) };
    MPP_NOK
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_register(
    def: *mut *mut c_void,
    _size: i32,
    _name: *const c_char,
) -> i32 {
    if def.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    unsafe { def.write(ptr::null_mut()) };
    MPP_NOK
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_find(def: *mut *mut c_void, name: *const c_char) -> i32 {
    if def.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let name = match obj_def_name(name) {
        Ok(name) => name,
        Err(error) => return error,
    };
    let found = obj_defs().lock().unwrap().iter().copied().find(|pointer| {
        unsafe { &*(*pointer as *const ObjDefStub) }.name.as_c_str() == name.as_c_str()
    });
    unsafe {
        def.write(found.map_or(ptr::null_mut(), |pointer| pointer as *mut c_void));
    }
    if found.is_some() {
        MPP_OK
    } else {
        MPP_NOK
    }
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_put(_def: *mut c_void) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_add_cfg_root(_def: *mut c_void, _root: *mut c_void) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_get_cfg_root(_def: *mut c_void) -> *mut c_void {
    ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_add_entry(
    def: *mut c_void,
    _subroot: i32,
    name: *const c_char,
    _table: *mut c_void,
) -> i32 {
    if name.is_null() {
        return MPP_OK;
    }
    let Some(def) = obj_def_mut(def) else {
        return MPP_NOK;
    };
    let node = def.next_node;
    def.next_node += 1;
    node
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_add_init(_def: *mut c_void, _function: *mut c_void) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_add_deinit(_def: *mut c_void, _function: *mut c_void) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_add_preset(_def: *mut c_void, _function: *mut c_void) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_add_dump(_def: *mut c_void, _function: *mut c_void) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_add_resize(_def: *mut c_void, _function: *mut c_void) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_add_cache_init(
    _def: *mut c_void,
    _function: *mut c_void,
) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_add_cache_deinit(
    _def: *mut c_void,
    _function: *mut c_void,
) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_get_entry(
    _def: *mut c_void,
    _name: *const c_char,
    table: *mut *mut c_void,
) -> i32 {
    if table.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let entry = Box::into_raw(Box::new([0u64; 2])) as *mut c_void;
    unsafe { table.write(entry) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_get_offset(_def: *mut c_void, _name: *const c_char) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_get_cmd(_def: *mut c_void, _name: *const c_char) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_dump(_def: *mut c_void) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_get_name(def: *mut c_void) -> *const c_char {
    obj_def_mut(def).map_or(ptr::null(), |def| def.name.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_get_entry_size(_def: *mut c_void) -> i32 {
    512
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_get_buf_size(_def: *mut c_void) -> i32 {
    1024
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_get_trie(_def: *mut c_void) -> *mut c_void {
    ptr::null_mut()
}

fn new_cfg_object() -> *mut c_void {
    Box::into_raw(Box::new(0u64)) as *mut c_void
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_get_object(
    object: *mut *mut c_void,
    _name: *const c_char,
    _cfg_type: i32,
    _value: *mut c_void,
) -> i32 {
    if object.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    unsafe { object.write(new_cfg_object()) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_get_array(object: *mut *mut c_void, _name: *const c_char) -> i32 {
    if object.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    unsafe { object.write(new_cfg_object()) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_put_all(_object: *mut c_void) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_set_entry(_object: *mut c_void, _entry: *mut c_void) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_set_vla(
    _object: *mut c_void,
    _entry: *mut c_void,
    _cfg_type: i32,
) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_add(_root: *mut c_void, _leaf: *mut c_void) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_vla_add_raw(
    _array: *mut c_void,
    _index: i32,
    _value: *mut c_void,
) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_vla_add_elem(
    _array: *mut c_void,
    _index: i32,
    _element: *mut c_void,
) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_add_detail(_root: *mut c_void, _detail: *mut c_void) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_find(
    object: *mut *mut c_void,
    root: *mut c_void,
    _name: *mut c_char,
    _cfg_type: i32,
) -> i32 {
    if object.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    unsafe { object.write(root) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_get_val(
    _object: *mut c_void,
    _cfg_type: i32,
    value: *mut c_void,
) -> i32 {
    if !value.is_null() {
        unsafe { value.cast::<u64>().write(0) };
    }
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_dump(_object: *mut c_void, _function: *const c_char) {}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_from_trie(_trie: *mut c_void) -> *mut c_void {
    new_cfg_object()
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_to_trie(_object: *mut c_void) -> *mut c_void {
    ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_to_string(
    _object: *mut c_void,
    _format: i32,
    buffer: *mut *mut c_char,
) -> i32 {
    if buffer.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let output = unsafe { libc::malloc(1) }.cast::<c_char>();
    if output.is_null() {
        return MPP_NOK;
    }
    unsafe {
        output.write(0);
        buffer.write(output);
    }
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_from_string(
    object: *mut *mut c_void,
    _format: i32,
    _buffer: *const c_char,
) -> i32 {
    if object.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    unsafe { object.write(new_cfg_object()) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_from_file(
    object: *mut *mut c_void,
    _format: i32,
    _path: *const c_char,
) -> i32 {
    if object.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    unsafe { object.write(new_cfg_object()) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_to_struct(
    _object: *mut c_void,
    _type: *mut c_void,
    _structure: *mut c_void,
) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_from_struct(
    object: *mut *mut c_void,
    _type: *mut c_void,
    _structure: *mut c_void,
) -> i32 {
    if object.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    unsafe { object.write(new_cfg_object()) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_print_string(_buffer: *mut c_char) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_cfg_type_from_elem_type(_element_type: i32) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_init(trie: *mut *mut c_void, name: *const c_char) -> i32 {
    if trie.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let legacy_opt = name.is_null() || (name as usize) < 0x10000;
    let name = if legacy_opt {
        CString::new("standalone-trie").unwrap()
    } else {
        match obj_def_name(name) {
            Ok(name) => name,
            Err(error) => return error,
        }
    };
    let pointer = Box::into_raw(Box::new(TrieStub {
        name,
        nodes: 0,
        infos: Vec::new(),
        names: Vec::new(),
        legacy_opt,
    })) as usize;
    tries().lock().unwrap().push(pointer);
    unsafe { trie.write(pointer as *mut c_void) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_init_by_root(trie: *mut *mut c_void, _root: *mut c_void) -> i32 {
    unsafe { mpp_trie_init(trie, c"standalone-trie".as_ptr()) }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_deinit(_trie: *mut c_void) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_add_info(
    trie: *mut c_void,
    name: *const c_char,
    mut context: *mut c_void,
    mut context_size: u32,
) -> i32 {
    let Some(trie) = trie_mut(trie) else {
        return MPP_NOK;
    };
    if name.is_null() {
        return MPP_OK;
    }
    let name = if trie.legacy_opt {
        let stored_name = unsafe { std::ptr::read_unaligned(name.cast::<usize>()) };
        if stored_name <= 0x10000 || stored_name >= (1usize << 56) {
            return MPP_NOK;
        }
        context = name.cast_mut().cast::<c_void>();
        context_size = 32;
        stored_name as *const c_char
    } else {
        name
    };
    if name.is_null() {
        return MPP_NOK;
    }
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    let context_size = usize::try_from(context_size).unwrap_or(0);
    let name_size = name.len() + 1;
    let string_size = (name_size + 4).next_multiple_of(8) - 4;
    let context_size_aligned = context_size.next_multiple_of(8);
    let mut info = vec![0u8; 4 + string_size + context_size_aligned];
    let header = ((trie.infos.len() as u32) & 0xfff)
        | (((context_size_aligned as u32) & 0xfff) << 12)
        | (((string_size as u32) & 0xff) << 24);
    info[..4].copy_from_slice(&header.to_ne_bytes());
    info[4..4 + name.len()].copy_from_slice(name);
    if !context.is_null() && context_size != 0 {
        let context = unsafe { std::slice::from_raw_parts(context.cast::<u8>(), context_size) };
        info[4 + string_size..4 + string_size + context_size].copy_from_slice(context);
    }
    trie.nodes += 1;
    let pointer = Box::into_raw(info.into_boxed_slice()).cast::<u8>() as usize;
    trie.infos.push(pointer);
    trie.names.push((
        unsafe { CStr::from_ptr(name.as_ptr().cast()) }.to_owned(),
        pointer,
        context as usize,
    ));
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_add_entry(
    trie: *mut c_void,
    _status: *mut c_void,
    name: *const c_char,
    _entry: *mut c_void,
) -> i32 {
    let Some(trie) = trie_mut(trie) else {
        return MPP_NOK;
    };
    if name.is_null() {
        return MPP_OK;
    }
    let node = trie.nodes;
    trie.nodes += 1;
    node
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_get_entry(
    _trie: *mut c_void,
    _status: *mut c_void,
    _name: *const c_char,
    _entry: *mut c_void,
) -> i32 {
    MPP_NOK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_get_node_count(trie: *mut c_void) -> i32 {
    trie_mut(trie).map_or(0, |trie| trie.nodes)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_get_info_count(trie: *mut c_void) -> i32 {
    trie_mut(trie).map_or(0, |trie| trie.infos.len() as i32)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_get_buf_size(_trie: *mut c_void) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_get_name_max(trie: *mut c_void) -> i32 {
    trie_mut(trie).map_or(0, |trie| trie.name.as_bytes().len() as i32)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_get_name(trie: *mut c_void) -> *const c_char {
    trie_mut(trie).map_or(ptr::null(), |trie| trie.name.as_ptr())
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_get_node_root(_trie: *mut c_void) -> *mut c_void {
    ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_get_info(trie: *mut c_void, name: *const c_char) -> *mut c_void {
    if trie.is_null() || name.is_null() {
        return ptr::null_mut();
    }
    let Some(trie) = trie_mut(trie) else {
        return ptr::null_mut();
    };
    let name = unsafe { CStr::from_ptr(name) };
    trie.infos
        .iter()
        .copied()
        .find_map(|pointer| {
            trie.names
                .iter()
                .find(|(stored, candidate, _)| candidate == &pointer && stored.as_c_str() == name)
                .map(|(_, _, context)| {
                    if trie.legacy_opt {
                        *context as *mut c_void
                    } else {
                        pointer as *mut c_void
                    }
                })
        })
        .unwrap_or(ptr::null_mut())
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_get_info_first(trie: *mut c_void) -> *mut c_void {
    let Some(trie) = trie_mut(trie) else {
        return ptr::null_mut();
    };
    trie.names
        .first()
        .map_or(ptr::null_mut(), |(_, info, context)| {
            if trie.legacy_opt {
                *context as *mut c_void
            } else {
                *info as *mut c_void
            }
        })
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_get_info_next(
    trie: *mut c_void,
    info: *mut c_void,
) -> *mut c_void {
    let Some(trie) = trie_mut(trie) else {
        return ptr::null_mut();
    };
    let Some(index) = trie.names.iter().position(|(_, pointer, context)| {
        if trie.legacy_opt {
            *context == info as usize
        } else {
            *pointer == info as usize
        }
    }) else {
        return ptr::null_mut();
    };
    trie.names
        .get(index + 1)
        .map_or(ptr::null_mut(), |(_, pointer, context)| {
            if trie.legacy_opt {
                *context as *mut c_void
            } else {
                *pointer as *mut c_void
            }
        })
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_get_info_from_root(
    _root: *mut c_void,
    _name: *const c_char,
) -> *mut c_void {
    ptr::null_mut()
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_for_each_entry(
    _trie: *mut c_void,
    _callback: *mut c_void,
    _context: *mut c_void,
) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_dump(_trie: *mut c_void, _function: *const c_char) {}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_timing_test(_trie: *mut c_void) {}

#[derive(Debug, Clone, Copy)]
enum ConfigValue {
    S32(i32),
    U32(u32),
    S64(i64),
    U64(u64),
    Ptr(usize),
}

struct DecCfg {
    magic: u64,
    values: HashMap<CString, ConfigValue>,
}

struct EncCfg {
    magic: u64,
    values: HashMap<CString, ConfigValue>,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RefLtCfg {
    lt_idx: i32,
    temporal_id: i32,
    ref_mode: i32,
    ref_arg: i32,
    lt_gap: i32,
    lt_delay: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct RefStCfg {
    is_non_ref: i32,
    temporal_id: i32,
    ref_mode: i32,
    ref_arg: i32,
    repeat: i32,
}

struct RefCfg {
    magic: u64,
    lt_capacity: i32,
    st_capacity: i32,
    lt_count: i32,
    st_count: i32,
    keep_cpb: i32,
    max_tlayers: i32,
    cpb_info: MppEncCpbInfo,
    lt: [RefLtCfg; 16],
    st: [RefStCfg; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MppEncCpbInfo {
    pub dpb_size: i32,
    pub max_lt_cnt: i32,
    pub max_st_cnt: i32,
    pub max_lt_idx: i32,
    pub max_st_tid: i32,
    pub lt_gop: i32,
    pub st_gop: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReferenceFrameStep {
    pub non_reference: bool,
    pub temporal_id: u8,
    pub reference_mode: u8,
    pub reference_argument: i32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct LongTermReferenceConfig {
    pub index: u32,
    pub gap: usize,
    pub delay: usize,
    pub reference_mode: u8,
    pub reference_argument: i32,
}

static ENV_STRINGS: OnceLock<Mutex<Vec<CString>>> = OnceLock::new();
static SOC_NAME: OnceLock<CString> = OnceLock::new();
static OBJ_DEFS: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();
static TRIES: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();

struct TrieStub {
    name: CString,
    nodes: i32,
    infos: Vec<usize>,
    names: Vec<(CString, usize, usize)>,
    legacy_opt: bool,
}

fn env_strings() -> &'static Mutex<Vec<CString>> {
    ENV_STRINGS.get_or_init(|| Mutex::new(Vec::new()))
}

fn obj_defs() -> &'static Mutex<Vec<usize>> {
    OBJ_DEFS.get_or_init(|| Mutex::new(Vec::new()))
}

fn tries() -> &'static Mutex<Vec<usize>> {
    TRIES.get_or_init(|| Mutex::new(Vec::new()))
}

fn trie_mut(trie: *mut c_void) -> Option<&'static mut TrieStub> {
    if trie.is_null() {
        return None;
    }
    tries()
        .lock()
        .unwrap()
        .iter()
        .copied()
        .find(|pointer| *pointer == trie as usize)
        .map(|pointer| unsafe { &mut *(pointer as *mut TrieStub) })
}

fn obj_def_name(name: *const c_char) -> Result<CString, c_int> {
    Ok(c_name(name)?.to_owned())
}

fn obj_def_mut(def: *mut c_void) -> Option<&'static mut ObjDefStub> {
    if def.is_null() {
        return None;
    }
    obj_defs()
        .lock()
        .unwrap()
        .iter()
        .copied()
        .find(|pointer| *pointer == def as usize)
        .map(|pointer| unsafe { &mut *(pointer as *mut ObjDefStub) })
}

fn parse_u32(value: &str) -> Option<u32> {
    if let Some(value) = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
    {
        u32::from_str_radix(value, 16).ok()
    } else {
        value.parse().ok()
    }
}

fn c_name(name: *const c_char) -> Result<&'static CStr, c_int> {
    if name.is_null() {
        return Err(MPP_ERR_NULL_PTR);
    }
    unsafe { Ok(CStr::from_ptr(name)) }
}

fn cfg_key_supported(name: &CStr) -> bool {
    matches!(
        name.to_bytes(),
        b"base:type"
            | b"base:coding"
            | b"base:hw_type"
            | b"base:batch_mode"
            | b"base:out_fmt"
            | b"base:fast_out"
            | b"base:fast_parse"
            | b"base:split_parse"
            | b"base:internal_pts"
            | b"base:sort_pts"
            | b"base:disable_error"
            | b"base:enable_vproc"
            | b"base:enable_fast_play"
            | b"base:enable_hdr_meta"
            | b"base:enable_thumbnail"
            | b"base:enable_mvc"
            | b"base:disable_dpb_chk"
            | b"base:disable_thread"
            | b"base:codec_mode"
            | b"base:dis_err_clr_mark"
            | b"base:meta_chk_len"
            | b"status:width"
            | b"status:height"
            | b"status:hor_stride"
            | b"status:ver_stride"
            | b"status:buf_size"
            | b"cb:pkt_rdy_cb"
            | b"cb:pkt_rdy_ctx"
            | b"cb:pkt_rdy_cmd"
            | b"cb:frm_rdy_cb"
            | b"cb:frm_rdy_ctx"
            | b"cb:frm_rdy_cmd"
    )
}

fn dec_cfg_mut(config: *mut c_void) -> Option<&'static mut DecCfg> {
    if config.is_null() {
        return None;
    }
    let config = unsafe { &mut *config.cast::<DecCfg>() };
    (config.magic == DEC_CFG_MAGIC).then_some(config)
}

fn enc_cfg_mut(config: *mut c_void) -> Option<&'static mut EncCfg> {
    if config.is_null() {
        return None;
    }
    let config = unsafe { &mut *config.cast::<EncCfg>() };
    (config.magic == ENC_CFG_MAGIC).then_some(config)
}

fn enc_cfg_ref(config: *mut c_void) -> Option<&'static EncCfg> {
    if config.is_null() {
        return None;
    }
    let config = unsafe { &*config.cast::<EncCfg>() };
    (config.magic == ENC_CFG_MAGIC).then_some(config)
}

fn ref_cfg_mut(config: *mut c_void) -> Option<&'static mut RefCfg> {
    if config.is_null() {
        return None;
    }
    let config = unsafe { &mut *config.cast::<RefCfg>() };
    (config.magic == REF_CFG_MAGIC).then_some(config)
}

fn new_ref_cfg() -> RefCfg {
    RefCfg {
        magic: REF_CFG_MAGIC,
        lt_capacity: 0,
        st_capacity: 0,
        lt_count: 0,
        st_count: 0,
        keep_cpb: 0,
        max_tlayers: 0,
        cpb_info: MppEncCpbInfo::default(),
        lt: [RefLtCfg::default(); 16],
        st: [RefStCfg::default(); 16],
    }
}

fn ref_cfg_ref(config: *mut c_void) -> Option<&'static RefCfg> {
    if config.is_null() {
        return None;
    }
    let config = unsafe { &*config.cast::<RefCfg>() };
    (config.magic == REF_CFG_MAGIC).then_some(config)
}

fn json_i32(text: &str, key: &str) -> Option<i32> {
    let key = format!("\"{key}\"");
    let start = text.find(&key)? + key.len();
    let colon = start + text[start..].find(':')?;
    let value = text[colon + 1..].trim_start();
    let end = value
        .find(|ch: char| !(ch == '-' || ch.is_ascii_digit()))
        .unwrap_or(value.len());
    value[..end].parse().ok()
}

fn json_object_array<'a>(text: &'a str, key: &str) -> Option<Vec<&'a str>> {
    let key = format!("\"{key}\"");
    let start = text.find(&key)? + key.len();
    let open = start + text[start..].find('[')?;
    let mut depth = 0usize;
    let mut object_start = None;
    let mut quoted = false;
    let mut escaped = false;
    let mut objects = Vec::new();
    for (offset, ch) in text[open + 1..].char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                quoted = false;
            }
            continue;
        }
        match ch {
            '"' => quoted = true,
            '{' => {
                if depth == 0 {
                    object_start = Some(open + 1 + offset);
                }
                depth += 1;
            }
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    objects.push(&text[object_start?..=open + offset]);
                    object_start = None;
                }
            }
            ']' if depth == 0 => return Some(objects),
            _ => {}
        }
    }
    None
}

fn apply_ref_json(config: &mut RefCfg, text: &str) -> i32 {
    let Some(st_count) = json_i32(text, "st_cfg_cnt") else {
        return MPP_NOK;
    };
    let Some(lt_count) = json_i32(text, "lt_cfg_cnt") else {
        return MPP_NOK;
    };
    if !(0..=16).contains(&st_count) || !(0..=16).contains(&lt_count) {
        return MPP_NOK;
    }
    let Some(st_objects) = json_object_array(text, "st_cfg") else {
        return MPP_NOK;
    };
    let Some(lt_objects) = json_object_array(text, "lt_cfg") else {
        return MPP_NOK;
    };
    if st_objects.len() < st_count as usize || lt_objects.len() < lt_count as usize {
        return MPP_NOK;
    }

    *config = new_ref_cfg();
    config.st_capacity = st_count;
    config.lt_capacity = lt_count;
    config.st_count = st_count;
    config.lt_count = lt_count;
    config.keep_cpb = json_i32(text, "keep_cpb").unwrap_or(0);
    for (index, object) in st_objects[..st_count as usize].iter().enumerate() {
        config.st[index] = RefStCfg {
            is_non_ref: json_i32(object, "is_non_ref").unwrap_or(0),
            temporal_id: json_i32(object, "temporal_id").unwrap_or(0),
            ref_mode: json_i32(object, "ref_mode").unwrap_or(0),
            ref_arg: json_i32(object, "ref_arg").unwrap_or(0),
            repeat: json_i32(object, "repeat").unwrap_or(0),
        };
    }
    for (index, object) in lt_objects[..lt_count as usize].iter().enumerate() {
        config.lt[index] = RefLtCfg {
            lt_idx: json_i32(object, "lt_idx").unwrap_or(0),
            temporal_id: json_i32(object, "temporal_id").unwrap_or(0),
            ref_mode: json_i32(object, "ref_mode").unwrap_or(0),
            ref_arg: json_i32(object, "ref_arg").unwrap_or(0),
            lt_gap: json_i32(object, "lt_gap").unwrap_or(0),
            lt_delay: json_i32(object, "lt_delay").unwrap_or(0),
        };
    }
    if validate_ref_cfg(config) {
        MPP_OK
    } else {
        config.st_count = 0;
        config.lt_count = 0;
        MPP_NOK
    }
}

fn extract_ref_json(config: &RefCfg) -> String {
    let mut out = format!(
        "{{\n    \"keep_cpb\" : {},\n    \"st_cfg_cnt\" : {},\n    \"lt_cfg_cnt\" : {},\n",
        config.keep_cpb, config.st_count, config.lt_count
    );
    out.push_str("    \"st_cfg\" : [");
    for (index, step) in config.st[..config.st_count as usize].iter().enumerate() {
        if index != 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "\n        {{ \"is_non_ref\": {}, \"temporal_id\": {}, \"ref_mode\": {}, \"ref_arg\": {}, \"repeat\": {} }}",
            step.is_non_ref,
            step.temporal_id,
            step.ref_mode,
            step.ref_arg,
            step.repeat
        ));
    }
    if config.st_count != 0 {
        out.push_str("\n    ");
    }
    out.push_str("],\n    \"lt_cfg\" : [");
    for (index, step) in config.lt[..config.lt_count as usize].iter().enumerate() {
        if index != 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "\n        {{ \"lt_idx\": {}, \"temporal_id\": {}, \"ref_mode\": {}, \"ref_arg\": {}, \"lt_gap\": {}, \"lt_delay\": {} }}",
            step.lt_idx,
            step.temporal_id,
            step.ref_mode,
            step.ref_arg,
            step.lt_gap,
            step.lt_delay
        ));
    }
    if config.lt_count != 0 {
        out.push_str("\n    ");
    }
    out.push_str("]\n}\n");
    out
}

#[derive(Clone, Copy, Default)]
struct CpbFrame {
    valid: bool,
    sequence: i32,
    long_term: bool,
    long_term_index: i32,
}

fn dryrun_st_cpb_count(config: &RefCfg) -> i32 {
    const CPB_SIZE: usize = 16;
    const TEMPORAL_LAYERS: usize = 4;

    let mut mode_refs = [CpbFrame::default(); 8];
    let mut st_tid_refs = [CpbFrame::default(); TEMPORAL_LAYERS];
    let mut lt_idx_refs = [CpbFrame::default(); CPB_SIZE];
    let mut st_refs = [CpbFrame::default(); CPB_SIZE];
    let mut lt_refs = [CpbFrame::default(); CPB_SIZE];
    let mut lt_counters: Vec<(i32, i32, i32, i32)> = config.lt[..config.lt_count as usize]
        .iter()
        .map(|lt| (lt.lt_delay, 0, 0, lt.lt_gap))
        .collect();
    if config.st_count == 0 {
        return 1;
    }
    let walk_len = config.st_count.max(config.lt_count);
    let mut used_size = 0;
    let mut sequence = 0;

    for step_index in 0..walk_len {
        let step = config.st[(step_index % config.st_count) as usize];
        let repeat = if step.repeat == 0 { 1 } else { step.repeat };
        for _ in 0..repeat {
            let mut frame = CpbFrame {
                valid: true,
                sequence,
                long_term: false,
                long_term_index: 0,
            };
            let is_intra = sequence == 0;
            let mut is_non_reference = step.is_non_ref != 0;
            let mut temporal_id = step.temporal_id;
            let mut reference_mode = step.ref_mode;
            let mut reference_argument = step.ref_arg;
            let mut current_lt = None;

            let mut set_long_term = false;
            for (index, (delay, count, _, len)) in lt_counters.iter_mut().enumerate() {
                if *delay > 0 {
                    *delay -= 1;
                    continue;
                }
                if !set_long_term && *count == 0 {
                    set_long_term = true;
                    current_lt = Some(index);
                }
                *count += 1;
                if *count >= *len {
                    if *len > 0 {
                        *count = 0;
                    } else {
                        *count = 1;
                    }
                }
            }
            if let Some(index) = current_lt {
                let lt = config.lt[index];
                is_non_reference = false;
                frame.long_term = true;
                frame.long_term_index = lt.lt_idx;
                temporal_id = lt.temporal_id;
                if lt.ref_mode != 25 {
                    reference_mode = lt.ref_mode;
                    reference_argument = lt.ref_arg;
                }
            }

            if sequence != 0 {
                let reference = match reference_mode {
                    0..=3 => mode_refs[reference_mode as usize],
                    4 => st_tid_refs[reference_argument.clamp(0, 3) as usize],
                    5 => lt_idx_refs[reference_argument.clamp(0, 15) as usize],
                    6 => st_refs[reference_argument.clamp(0, 15) as usize],
                    _ => CpbFrame::default(),
                };
                if reference.valid && !reference.long_term {
                    if let Some(position) = st_refs[..CPB_SIZE]
                        .iter()
                        .position(|item| item.valid && item.sequence == reference.sequence)
                    {
                        used_size = used_size.max(position as i32 + 1);
                    }
                }
            }

            if !is_non_reference {
                if is_intra {
                    mode_refs[3] = frame;
                }
                if frame.long_term {
                    mode_refs[0] = frame;
                    mode_refs[2] = frame;
                    st_tid_refs[temporal_id.clamp(0, 3) as usize] = frame;
                    lt_idx_refs[frame.long_term_index.clamp(0, 15) as usize] = frame;
                    if let Some(slot) = lt_refs[..CPB_SIZE].iter_mut().find(|item| {
                        !item.valid
                            || (item.long_term && item.long_term_index == frame.long_term_index)
                    }) {
                        *slot = frame;
                    }
                } else {
                    mode_refs[0] = frame;
                    mode_refs[1] = frame;
                    st_tid_refs[temporal_id.clamp(0, 3) as usize] = frame;
                    st_refs.copy_within(0..CPB_SIZE - 1, 1);
                    st_refs[0] = frame;
                }
            }
            sequence += 1;
        }
    }

    used_size.max(1)
}

fn validate_ref_cfg(config: &mut RefCfg) -> bool {
    const MAX_REF_CFG: i32 = 16;
    const MAX_TEMPORAL_LAYERS: i32 = 4;
    const MAX_LT_INDEX: i32 = 16;
    const REF_MODE_GLOBAL_BUTT: i32 = 7;
    const REF_TO_ST_REF_SETUP: i32 = 25;
    const REF_TO_LT_REF_SETUP: i32 = 29;

    if !(0..=MAX_REF_CFG).contains(&config.lt_count)
        || !(0..=MAX_REF_CFG).contains(&config.st_count)
    {
        return false;
    }

    let mut lt_used = 0u32;
    let mut max_lt_cnt = 0;
    let mut max_lt_idx = 0;
    let mut lt_gop = 0;
    for step in &config.lt[..config.lt_count as usize] {
        if !(0..MAX_LT_INDEX).contains(&step.lt_idx) {
            return false;
        }
        let bit = 1u32 << step.lt_idx;
        if lt_used & bit != 0 {
            return false;
        }
        lt_used |= bit;
        max_lt_cnt += 1;
        max_lt_idx = max_lt_idx.max(step.lt_idx);
        if step.temporal_id != 0 {
            return false;
        }
        if !(0..REF_MODE_GLOBAL_BUTT).contains(&step.ref_mode)
            && step.ref_mode != REF_TO_ST_REF_SETUP
        {
            return false;
        }
        lt_gop = lt_gop.max(step.lt_gap.saturating_add(step.lt_delay));
    }

    let mut max_st_tid = 0;
    let mut st_dryrun_length = 0;
    for (index, step) in config.st[..config.st_count as usize].iter_mut().enumerate() {
        if !(0..MAX_TEMPORAL_LAYERS).contains(&step.temporal_id) {
            return false;
        }
        if !(0..REF_MODE_GLOBAL_BUTT).contains(&step.ref_mode)
            && step.ref_mode != REF_TO_LT_REF_SETUP
        {
            return false;
        }
        if step.repeat < 0 {
            step.repeat = 0;
        }
        if (index == 0 || index + 1 == config.st_count as usize)
            && (step.is_non_ref != 0 || step.temporal_id != 0)
        {
            return false;
        }
        max_st_tid = max_st_tid.max(step.temporal_id);
        st_dryrun_length += 1 + step.repeat;
    }

    let max_st_cnt = dryrun_st_cpb_count(config);
    config.max_tlayers = if config.st_count == 0 {
        0
    } else {
        max_st_tid + 1
    };
    config.cpb_info = MppEncCpbInfo {
        dpb_size: (max_lt_cnt + max_st_cnt).max(1),
        max_lt_cnt,
        max_st_cnt,
        max_lt_idx,
        max_st_tid,
        lt_gop,
        st_gop: st_dryrun_length.saturating_sub(1),
    };
    true
}

pub(super) fn reference_frame_pattern(config: *mut c_void) -> Option<Vec<ReferenceFrameStep>> {
    let config = ref_cfg_mut(config)?;
    let step_count = match config.st_count {
        3 | 5 | 9 => config.st_count as usize,
        _ => return None,
    };
    if config.st[1..step_count]
        .iter()
        .any(|step| !(0..=7).contains(&step.temporal_id))
    {
        return None;
    }
    Some(
        config.st[1..step_count]
            .iter()
            .copied()
            .map(|step| ReferenceFrameStep {
                non_reference: step.is_non_ref != 0,
                temporal_id: step.temporal_id as u8,
                reference_mode: step.ref_mode as u8,
                reference_argument: step.ref_arg,
            })
            .collect(),
    )
}

pub(super) fn reference_long_term_config(config: *mut c_void) -> Option<LongTermReferenceConfig> {
    let config = ref_cfg_mut(config)?;
    match config.lt_count {
        0 => None,
        1 => {
            let long_term = config.lt[0];
            (long_term.lt_idx >= 0
                && long_term.temporal_id == 0
                && long_term.lt_gap >= 0
                && long_term.lt_delay >= 0)
                .then_some(LongTermReferenceConfig {
                    index: long_term.lt_idx as u32,
                    gap: long_term.lt_gap as usize,
                    delay: long_term.lt_delay as usize,
                    reference_mode: long_term.ref_mode as u8,
                    reference_argument: long_term.ref_arg,
                })
        }
        _ => None,
    }
}

pub(super) fn encoder_cfg_ref_pattern(config: *mut c_void) -> Option<Vec<ReferenceFrameStep>> {
    let reference = encoder_cfg_reference(config)?;
    reference_frame_pattern(reference)
}

pub(super) fn encoder_cfg_has_ref_cfg(config: *mut c_void) -> bool {
    encoder_cfg_reference(config).is_some()
}

pub(super) fn encoder_cfg_long_term_config(config: *mut c_void) -> Option<LongTermReferenceConfig> {
    let reference = encoder_cfg_reference(config)?;
    reference_long_term_config(reference)
}

fn encoder_cfg_reference(config: *mut c_void) -> Option<*mut c_void> {
    let config = enc_cfg_ref(config)?;
    let key = c"rc:ref_cfg";
    match config.values.get(key)? {
        ConfigValue::Ptr(pointer) => Some(*pointer as *mut c_void),
        _ => return None,
    }
}

fn dec_cfg_ref(config: *mut c_void) -> Option<&'static DecCfg> {
    if config.is_null() {
        return None;
    }
    let config = unsafe { &*config.cast::<DecCfg>() };
    (config.magic == DEC_CFG_MAGIC).then_some(config)
}

fn insert_default(values: &mut HashMap<CString, ConfigValue>, name: &str, value: ConfigValue) {
    if let Ok(name) = CString::new(name) {
        values.insert(name, value);
    }
}

fn new_dec_cfg() -> DecCfg {
    let mut values = HashMap::new();
    insert_default(&mut values, "base:type", ConfigValue::U32(3));
    insert_default(&mut values, "base:coding", ConfigValue::U32(0));
    insert_default(&mut values, "base:hw_type", ConfigValue::S32(-1));
    insert_default(&mut values, "base:fast_parse", ConfigValue::U32(1));
    insert_default(&mut values, "base:enable_fast_play", ConfigValue::U32(1));
    insert_default(&mut values, "base:meta_chk_len", ConfigValue::U32(512));
    for key in [
        "base:batch_mode",
        "base:out_fmt",
        "base:fast_out",
        "base:split_parse",
        "base:internal_pts",
        "base:sort_pts",
        "base:disable_error",
        "base:enable_vproc",
        "base:enable_hdr_meta",
        "base:enable_thumbnail",
        "base:enable_mvc",
        "base:disable_dpb_chk",
        "base:disable_thread",
        "base:codec_mode",
        "base:dis_err_clr_mark",
        "status:width",
        "status:height",
        "status:hor_stride",
        "status:ver_stride",
        "status:buf_size",
        "cb:pkt_rdy_cmd",
        "cb:frm_rdy_cmd",
    ] {
        insert_default(&mut values, key, ConfigValue::U32(0));
    }
    for key in [
        "cb:pkt_rdy_cb",
        "cb:pkt_rdy_ctx",
        "cb:frm_rdy_cb",
        "cb:frm_rdy_ctx",
    ] {
        insert_default(&mut values, key, ConfigValue::Ptr(0));
    }
    DecCfg {
        magic: DEC_CFG_MAGIC,
        values,
    }
}

fn set_value(config: *mut c_void, name: *const c_char, value: ConfigValue) -> c_int {
    let Some(config) = dec_cfg_mut(config) else {
        return MPP_ERR_NULL_PTR;
    };
    let name = match c_name(name) {
        Ok(name) if cfg_key_supported(name) => name,
        Ok(_) => return MPP_NOK,
        Err(error) => return error,
    };
    config.values.insert(name.to_owned(), value);
    MPP_OK
}

unsafe fn get_value<T>(
    config: *mut c_void,
    name: *const c_char,
    output: *mut T,
    convert: impl Fn(ConfigValue) -> Option<T>,
) -> c_int {
    if output.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let Some(config) = dec_cfg_ref(config) else {
        return MPP_ERR_NULL_PTR;
    };
    let name = match c_name(name) {
        Ok(name) => name,
        Err(error) => return error,
    };
    let Some(value) = config.values.get(name).copied().and_then(convert) else {
        return MPP_NOK;
    };
    unsafe { output.write(value) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_env_get_u32(
    name: *const c_char,
    value: *mut u32,
    default_value: u32,
) -> c_int {
    if name.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let parsed = unsafe { CStr::from_ptr(name) }
        .to_str()
        .ok()
        .and_then(|name| std::env::var(name).ok())
        .and_then(|value| parse_u32(&value))
        .unwrap_or(default_value);
    if !value.is_null() {
        unsafe { value.write(parsed) };
    }
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_env_get_str(
    name: *const c_char,
    value: *mut *const c_char,
    default_value: *const c_char,
) -> c_int {
    if name.is_null() || value.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let name = unsafe { CStr::from_ptr(name) };
    let Some(found) = name.to_str().ok().and_then(|name| std::env::var_os(name)) else {
        unsafe { value.write(default_value) };
        return MPP_OK;
    };
    let Some(found) = CString::new(found.to_string_lossy().as_bytes()).ok() else {
        return MPP_NOK;
    };
    let pointer = {
        let mut values = env_strings().lock().unwrap();
        values.push(found);
        values.last().unwrap().as_ptr()
    };
    unsafe { value.write(pointer) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_env_set_u32(name: *const c_char, value: u32) -> c_int {
    let name = match c_name(name) {
        Ok(name) => name,
        Err(error) => return error,
    };
    let value = match CString::new(value.to_string()) {
        Ok(value) => value,
        Err(_) => return MPP_NOK,
    };
    if unsafe { libc::setenv(name.as_ptr(), value.as_ptr(), 1) } == 0 {
        MPP_OK
    } else {
        MPP_NOK
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_env_set_str(name: *const c_char, value: *mut c_char) -> c_int {
    if value.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let name = match c_name(name) {
        Ok(name) => name,
        Err(error) => return error,
    };
    if unsafe { libc::setenv(name.as_ptr(), value, 1) } == 0 {
        MPP_OK
    } else {
        MPP_NOK
    }
}

#[no_mangle]
pub extern "C" fn mpp_time() -> i64 {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    if unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut time) } != 0 {
        return 0;
    }
    time.tv_sec as i64 * 1_000_000 + time.tv_nsec as i64 / 1_000
}

#[no_mangle]
pub unsafe extern "C" fn mpp_osal_malloc(_caller: *const c_char, size: usize) -> *mut c_void {
    unsafe { libc::malloc(size) }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_osal_calloc(_caller: *const c_char, size: usize) -> *mut c_void {
    unsafe { libc::calloc(1, size) }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_osal_realloc(
    _caller: *const c_char,
    pointer: *mut c_void,
    size: usize,
) -> *mut c_void {
    unsafe { libc::realloc(pointer, size) }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_osal_free(_caller: *const c_char, pointer: *mut c_void) {
    unsafe { libc::free(pointer) };
}

#[no_mangle]
pub extern "C" fn mpp_show_mem_status() {}

#[no_mangle]
pub extern "C" fn mpp_mem_total_now() -> u32 {
    0
}

#[no_mangle]
pub extern "C" fn mpp_mem_total_max() -> u32 {
    0
}

#[no_mangle]
pub extern "C" fn mpp_get_soc_name() -> *const c_char {
    SOC_NAME
        .get_or_init(|| {
            let name = match SocProfile::detect().map(|profile| profile.id) {
                Ok(SocId::Rk3588) => "rk3588",
                Ok(SocId::Rk3576) => "rk3576",
                Ok(SocId::Rk3568) => "rk3568",
                Ok(SocId::Rk3566) => "rk3566",
                Ok(SocId::Rk3562) => "rk3562",
                Ok(SocId::Rk3528) => "rk3528",
                Ok(SocId::Rk3572) => "rk3572",
                Ok(SocId::Rv1126B) => "rv1126b",
                _ => "unknown",
            };
            CString::new(name).unwrap()
        })
        .as_ptr()
}

#[no_mangle]
pub extern "C" fn mpp_get_soc_type() -> c_int {
    match SocProfile::detect().map(|profile| profile.id) {
        Ok(SocId::Rk3588) => 22,
        Ok(SocId::Rk3576) => 25,
        Ok(SocId::Rk3568) => 21,
        Ok(SocId::Rk3566) => 19,
        Ok(SocId::Rk3562) => 24,
        Ok(SocId::Rk3528) => 23,
        Ok(SocId::Rk3572) => 27,
        Ok(SocId::Rv1126B) => 26,
        _ => 0,
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_dec_cfg_init(config: *mut *mut c_void) -> c_int {
    if config.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let object = Box::into_raw(Box::new(new_dec_cfg())).cast::<c_void>();
    unsafe { config.write(object) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_dec_cfg_deinit(config: *mut c_void) -> c_int {
    let Some(config) = dec_cfg_mut(config) else {
        return MPP_ERR_NULL_PTR;
    };
    config.magic = 0;
    drop(unsafe { Box::from_raw((config as *mut DecCfg).cast::<c_void>()) });
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_dec_cfg_set_s32(
    config: *mut c_void,
    name: *const c_char,
    value: i32,
) -> c_int {
    set_value(config, name, ConfigValue::S32(value))
}

#[no_mangle]
pub unsafe extern "C" fn mpp_dec_cfg_set_u32(
    config: *mut c_void,
    name: *const c_char,
    value: u32,
) -> c_int {
    set_value(config, name, ConfigValue::U32(value))
}

#[no_mangle]
pub unsafe extern "C" fn mpp_dec_cfg_set_s64(
    config: *mut c_void,
    name: *const c_char,
    value: i64,
) -> c_int {
    set_value(config, name, ConfigValue::S64(value))
}

#[no_mangle]
pub unsafe extern "C" fn mpp_dec_cfg_set_u64(
    config: *mut c_void,
    name: *const c_char,
    value: u64,
) -> c_int {
    set_value(config, name, ConfigValue::U64(value))
}

#[no_mangle]
pub unsafe extern "C" fn mpp_dec_cfg_set_ptr(
    config: *mut c_void,
    name: *const c_char,
    value: *mut c_void,
) -> c_int {
    set_value(config, name, ConfigValue::Ptr(value as usize))
}

#[no_mangle]
pub unsafe extern "C" fn mpp_dec_cfg_get_s32(
    config: *mut c_void,
    name: *const c_char,
    value: *mut i32,
) -> c_int {
    unsafe {
        get_value(config, name, value, |value| match value {
            ConfigValue::S32(value) => Some(value),
            ConfigValue::U32(value) => i32::try_from(value).ok(),
            ConfigValue::S64(value) => i32::try_from(value).ok(),
            ConfigValue::U64(value) => i32::try_from(value).ok(),
            ConfigValue::Ptr(_) => None,
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_dec_cfg_get_u32(
    config: *mut c_void,
    name: *const c_char,
    value: *mut u32,
) -> c_int {
    unsafe {
        get_value(config, name, value, |value| match value {
            ConfigValue::S32(value) => u32::try_from(value).ok(),
            ConfigValue::U32(value) => Some(value),
            ConfigValue::S64(value) => u32::try_from(value).ok(),
            ConfigValue::U64(value) => u32::try_from(value).ok(),
            ConfigValue::Ptr(_) => None,
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_dec_cfg_get_s64(
    config: *mut c_void,
    name: *const c_char,
    value: *mut i64,
) -> c_int {
    unsafe {
        get_value(config, name, value, |value| match value {
            ConfigValue::S32(value) => Some(i64::from(value)),
            ConfigValue::U32(value) => Some(i64::from(value)),
            ConfigValue::S64(value) => Some(value),
            ConfigValue::U64(value) => i64::try_from(value).ok(),
            ConfigValue::Ptr(_) => None,
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_dec_cfg_get_u64(
    config: *mut c_void,
    name: *const c_char,
    value: *mut u64,
) -> c_int {
    unsafe {
        get_value(config, name, value, |value| match value {
            ConfigValue::S32(value) => u64::try_from(value).ok(),
            ConfigValue::U32(value) => Some(u64::from(value)),
            ConfigValue::S64(value) => u64::try_from(value).ok(),
            ConfigValue::U64(value) => Some(value),
            ConfigValue::Ptr(_) => None,
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_dec_cfg_get_ptr(
    config: *mut c_void,
    name: *const c_char,
    value: *mut *mut c_void,
) -> c_int {
    unsafe {
        get_value(config, name, value, |value| match value {
            ConfigValue::Ptr(value) => Some(value as *mut c_void),
            _ => None,
        })
    }
}

#[no_mangle]
pub extern "C" fn mpp_dec_cfg_show() {}

fn new_enc_cfg() -> EncCfg {
    EncCfg {
        magic: ENC_CFG_MAGIC,
        values: HashMap::new(),
    }
}

fn enc_set_value(config: *mut c_void, name: *const c_char, value: ConfigValue) -> i32 {
    let Some(config) = enc_cfg_mut(config) else {
        return MPP_ERR_NULL_PTR;
    };
    let name = match c_name(name) {
        Ok(name) => name,
        Err(error) => return error,
    };
    config.values.insert(name.to_owned(), value);
    MPP_OK
}

unsafe fn enc_get_value<T: Default>(
    config: *mut c_void,
    name: *const c_char,
    output: *mut T,
    convert: impl Fn(ConfigValue) -> Option<T>,
) -> i32 {
    if output.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let Some(config) = enc_cfg_ref(config) else {
        return MPP_ERR_NULL_PTR;
    };
    let name = match c_name(name) {
        Ok(name) => name,
        Err(error) => return error,
    };
    let value = config
        .values
        .get(name)
        .copied()
        .and_then(convert)
        .unwrap_or_default();
    unsafe { output.write(value) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_init(config: *mut *mut c_void) -> i32 {
    if config.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    unsafe { config.write(Box::into_raw(Box::new(new_enc_cfg())).cast()) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_init_k(config: *mut *mut c_void) -> i32 {
    unsafe { mpp_enc_cfg_init(config) }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_create(config: *mut *mut c_void, _mode: u32) -> i32 {
    unsafe { mpp_enc_cfg_init(config) }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_deinit(config: *mut c_void) -> i32 {
    let Some(config) = enc_cfg_mut(config) else {
        return MPP_ERR_NULL_PTR;
    };
    config.magic = 0;
    drop(unsafe { Box::from_raw((config as *mut EncCfg).cast::<c_void>()) });
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_set_s8(
    config: *mut c_void,
    name: *const c_char,
    value: i8,
) -> i32 {
    enc_set_value(config, name, ConfigValue::S32(i32::from(value)))
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_set_u8(
    config: *mut c_void,
    name: *const c_char,
    value: u8,
) -> i32 {
    enc_set_value(config, name, ConfigValue::U32(u32::from(value)))
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_set_s16(
    config: *mut c_void,
    name: *const c_char,
    value: i16,
) -> i32 {
    enc_set_value(config, name, ConfigValue::S32(i32::from(value)))
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_set_u16(
    config: *mut c_void,
    name: *const c_char,
    value: u16,
) -> i32 {
    enc_set_value(config, name, ConfigValue::U32(u32::from(value)))
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_set_s32(
    config: *mut c_void,
    name: *const c_char,
    value: i32,
) -> i32 {
    enc_set_value(config, name, ConfigValue::S32(value))
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_set_u32(
    config: *mut c_void,
    name: *const c_char,
    value: u32,
) -> i32 {
    enc_set_value(config, name, ConfigValue::U32(value))
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_set_s64(
    config: *mut c_void,
    name: *const c_char,
    value: i64,
) -> i32 {
    enc_set_value(config, name, ConfigValue::S64(value))
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_set_u64(
    config: *mut c_void,
    name: *const c_char,
    value: u64,
) -> i32 {
    enc_set_value(config, name, ConfigValue::U64(value))
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_set_ptr(
    config: *mut c_void,
    name: *const c_char,
    value: *mut c_void,
) -> i32 {
    enc_set_value(config, name, ConfigValue::Ptr(value as usize))
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_set_st(
    _config: *mut c_void,
    _name: *const c_char,
    _value: *mut c_void,
) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_get_s8(
    config: *mut c_void,
    name: *const c_char,
    value: *mut i8,
) -> i32 {
    unsafe {
        enc_get_value(config, name, value, |value| match value {
            ConfigValue::S32(value) => i8::try_from(value).ok(),
            ConfigValue::U32(value) => i8::try_from(value).ok(),
            _ => None,
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_get_u8(
    config: *mut c_void,
    name: *const c_char,
    value: *mut u8,
) -> i32 {
    unsafe {
        enc_get_value(config, name, value, |value| match value {
            ConfigValue::S32(value) => u8::try_from(value).ok(),
            ConfigValue::U32(value) => u8::try_from(value).ok(),
            _ => None,
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_get_s16(
    config: *mut c_void,
    name: *const c_char,
    value: *mut i16,
) -> i32 {
    unsafe {
        enc_get_value(config, name, value, |value| match value {
            ConfigValue::S32(value) => i16::try_from(value).ok(),
            ConfigValue::U32(value) => i16::try_from(value).ok(),
            _ => None,
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_get_u16(
    config: *mut c_void,
    name: *const c_char,
    value: *mut u16,
) -> i32 {
    unsafe {
        enc_get_value(config, name, value, |value| match value {
            ConfigValue::S32(value) => u16::try_from(value).ok(),
            ConfigValue::U32(value) => u16::try_from(value).ok(),
            _ => None,
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_get_s32(
    config: *mut c_void,
    name: *const c_char,
    value: *mut i32,
) -> i32 {
    unsafe {
        enc_get_value(config, name, value, |value| match value {
            ConfigValue::S32(value) => Some(value),
            ConfigValue::U32(value) => i32::try_from(value).ok(),
            ConfigValue::S64(value) => i32::try_from(value).ok(),
            ConfigValue::U64(value) => i32::try_from(value).ok(),
            ConfigValue::Ptr(_) => None,
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_get_u32(
    config: *mut c_void,
    name: *const c_char,
    value: *mut u32,
) -> i32 {
    unsafe {
        enc_get_value(config, name, value, |value| match value {
            ConfigValue::S32(value) => u32::try_from(value).ok(),
            ConfigValue::U32(value) => Some(value),
            ConfigValue::S64(value) => u32::try_from(value).ok(),
            ConfigValue::U64(value) => u32::try_from(value).ok(),
            ConfigValue::Ptr(_) => None,
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_get_s64(
    config: *mut c_void,
    name: *const c_char,
    value: *mut i64,
) -> i32 {
    unsafe {
        enc_get_value(config, name, value, |value| match value {
            ConfigValue::S32(value) => Some(i64::from(value)),
            ConfigValue::U32(value) => Some(i64::from(value)),
            ConfigValue::S64(value) => Some(value),
            ConfigValue::U64(value) => i64::try_from(value).ok(),
            ConfigValue::Ptr(_) => None,
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_get_u64(
    config: *mut c_void,
    name: *const c_char,
    value: *mut u64,
) -> i32 {
    unsafe {
        enc_get_value(config, name, value, |value| match value {
            ConfigValue::S32(value) => u64::try_from(value).ok(),
            ConfigValue::U32(value) => Some(u64::from(value)),
            ConfigValue::S64(value) => u64::try_from(value).ok(),
            ConfigValue::U64(value) => Some(value),
            ConfigValue::Ptr(_) => None,
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_get_ptr(
    config: *mut c_void,
    name: *const c_char,
    value: *mut *mut c_void,
) -> i32 {
    unsafe {
        enc_get_value(config, name, value, |value| match value {
            ConfigValue::Ptr(value) => Some(value as *mut c_void),
            _ => None,
        })
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_get_st(
    _config: *mut c_void,
    _name: *const c_char,
    _value: *mut c_void,
) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_show() {}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_extract(
    _config: *mut c_void,
    _format: i32,
    buffer: *mut *mut c_char,
) -> i32 {
    if buffer.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let output = unsafe { libc::malloc(1) }.cast::<c_char>();
    if output.is_null() {
        return MPP_NOK;
    }
    unsafe {
        output.write(0);
        buffer.write(output);
    }
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_apply(
    _config: *mut c_void,
    _format: i32,
    _buffer: *mut c_char,
) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_cfg_apply_file(
    _config: *mut c_void,
    _format: i32,
    _path: *const c_char,
) -> i32 {
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_init(ref_cfg: *mut *mut c_void) -> i32 {
    if ref_cfg.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let object = Box::into_raw(Box::new(new_ref_cfg())).cast::<c_void>();
    unsafe { ref_cfg.write(object) };
    unsafe { mpp_enc_ref_cfg_reset(object) }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_create(ref_cfg: *mut *mut c_void, _mode: u32) -> i32 {
    unsafe { mpp_enc_ref_cfg_init(ref_cfg) }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_deinit(ref_cfg: *mut *mut c_void) -> i32 {
    if ref_cfg.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let object = unsafe { ref_cfg.read() };
    let Some(config) = ref_cfg_mut(object) else {
        return MPP_ERR_NULL_PTR;
    };
    config.magic = 0;
    drop(unsafe { Box::from_raw((config as *mut RefCfg).cast::<c_void>()) });
    unsafe { ref_cfg.write(ptr::null_mut()) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_reset(ref_cfg: *mut c_void) -> i32 {
    let Some(config) = ref_cfg_mut(ref_cfg) else {
        return MPP_ERR_VALUE;
    };
    *config = new_ref_cfg();
    config.st_capacity = 1;
    config.st_count = 1;
    config.max_tlayers = 1;
    config.cpb_info.dpb_size = 1;
    config.cpb_info.max_st_cnt = 1;
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_set_cfg_cnt(
    ref_cfg: *mut c_void,
    lt_count: i32,
    st_count: i32,
) -> i32 {
    unsafe { mpp_enc_ref_cfg_setup(ref_cfg, lt_count, st_count) }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_setup(
    ref_cfg: *mut c_void,
    lt_count: i32,
    st_count: i32,
) -> i32 {
    if !(0..=16).contains(&lt_count) || !(0..=16).contains(&st_count) {
        return MPP_NOK;
    }
    let Some(config) = ref_cfg_mut(ref_cfg) else {
        return MPP_ERR_NULL_PTR;
    };
    config.lt_capacity = lt_count;
    config.st_capacity = st_count;
    config.lt_count = 0;
    config.st_count = 0;
    config.max_tlayers = 0;
    config.cpb_info = MppEncCpbInfo::default();
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_add_lt_cfg(
    ref_cfg: *mut c_void,
    count: i32,
    frames: *const RefLtCfg,
) -> i32 {
    if frames.is_null() || count <= 0 {
        return MPP_ERR_VALUE;
    }
    let Some(config) = ref_cfg_mut(ref_cfg) else {
        return MPP_ERR_NULL_PTR;
    };
    if count > config.lt_capacity.saturating_sub(config.lt_count) {
        return MPP_ERR_VALUE;
    }
    let frames = unsafe { std::slice::from_raw_parts(frames, count as usize) };
    let start = config.lt_count as usize;
    config.lt[start..start + frames.len()].copy_from_slice(frames);
    config.lt_count += count;
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_add_st_cfg(
    ref_cfg: *mut c_void,
    count: i32,
    frames: *const RefStCfg,
) -> i32 {
    if frames.is_null() || count <= 0 {
        return MPP_ERR_VALUE;
    }
    let Some(config) = ref_cfg_mut(ref_cfg) else {
        return MPP_ERR_NULL_PTR;
    };
    if count > config.st_capacity.saturating_sub(config.st_count) {
        return MPP_ERR_VALUE;
    }
    let frames = unsafe { std::slice::from_raw_parts(frames, count as usize) };
    let start = config.st_count as usize;
    config.st[start..start + frames.len()].copy_from_slice(frames);
    config.st_count += count;
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_check(ref_cfg: *mut c_void) -> i32 {
    let Some(config) = ref_cfg_mut(ref_cfg) else {
        return MPP_ERR_VALUE;
    };
    if validate_ref_cfg(config) {
        MPP_OK
    } else {
        MPP_NOK
    }
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_copy(dst: *mut c_void, src: *mut c_void) -> i32 {
    if dst == src {
        return if ref_cfg_ref(dst).is_some() {
            MPP_OK
        } else {
            MPP_ERR_VALUE
        };
    }
    let Some(source) = ref_cfg_ref(src) else {
        return MPP_ERR_VALUE;
    };
    let Some(destination) = ref_cfg_mut(dst) else {
        return MPP_ERR_VALUE;
    };
    *destination = unsafe { std::ptr::read(source) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_get_cpb_info(ref_cfg: *mut c_void) -> *mut MppEncCpbInfo {
    let Some(config) = ref_cfg_mut(ref_cfg) else {
        return ptr::null_mut();
    };
    if !validate_ref_cfg(config) {
        return ptr::null_mut();
    }
    std::ptr::addr_of_mut!(config.cpb_info)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_set_keep_cpb(ref_cfg: *mut c_void, keep: i32) -> i32 {
    let Some(config) = ref_cfg_mut(ref_cfg) else {
        return MPP_ERR_NULL_PTR;
    };
    config.keep_cpb = i32::from(keep != 0);
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_get_preset(_preset: *mut c_void) -> i32 {
    MPP_NOK
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_show() {}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_apply(
    ref_cfg: *mut c_void,
    format: i32,
    buffer: *mut c_char,
) -> i32 {
    const MPP_CFG_STR_FMT_JSON: i32 = 1;
    if format != MPP_CFG_STR_FMT_JSON || buffer.is_null() {
        return MPP_NOK;
    }
    let Some(config) = ref_cfg_mut(ref_cfg) else {
        return MPP_NOK;
    };
    let text = unsafe { CStr::from_ptr(buffer) };
    let Ok(text) = text.to_str() else {
        return MPP_NOK;
    };
    apply_ref_json(config, text)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_apply_file(
    ref_cfg: *mut c_void,
    format: i32,
    path: *const c_char,
) -> i32 {
    const MPP_CFG_STR_FMT_JSON: i32 = 1;
    if format != MPP_CFG_STR_FMT_JSON || path.is_null() {
        return MPP_NOK;
    }
    let Some(config) = ref_cfg_mut(ref_cfg) else {
        return MPP_NOK;
    };
    let path = unsafe { CStr::from_ptr(path) };
    let Ok(path) = path.to_str() else {
        return MPP_NOK;
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return MPP_NOK;
    };
    apply_ref_json(config, &text)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_enc_ref_cfg_extract(
    ref_cfg: *mut c_void,
    format: i32,
    buffer: *mut *mut c_char,
) -> i32 {
    const MPP_CFG_STR_FMT_JSON: i32 = 1;
    if buffer.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    unsafe { buffer.write(ptr::null_mut()) };
    if format != MPP_CFG_STR_FMT_JSON {
        return MPP_NOK;
    }
    let Some(config) = ref_cfg_mut(ref_cfg) else {
        return MPP_NOK;
    };
    if !validate_ref_cfg(config) {
        return MPP_NOK;
    }
    let text = extract_ref_json(config);
    let output = unsafe { libc::malloc(text.len() + 1) }.cast::<c_char>();
    if output.is_null() {
        return MPP_NOK;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(text.as_ptr(), output.cast::<u8>(), text.len());
        output.add(text.len()).write(0);
        buffer.write(output);
    }
    MPP_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dec_cfg_round_trips_values() {
        let mut cfg = std::ptr::null_mut();
        assert_eq!(unsafe { mpp_dec_cfg_init(&mut cfg) }, MPP_OK);
        let name = c"base:split_parse";
        assert_eq!(
            unsafe { mpp_dec_cfg_set_u32(cfg, name.as_ptr(), 7) },
            MPP_OK
        );
        let mut value = 0;
        assert_eq!(
            unsafe { mpp_dec_cfg_get_u32(cfg, name.as_ptr(), &mut value) },
            MPP_OK
        );
        assert_eq!(value, 7);
        assert_eq!(unsafe { mpp_dec_cfg_deinit(cfg) }, MPP_OK);
    }

    #[test]
    fn kmpp_objdef_get_and_register_clear_the_current_lf1_abi_output() {
        let mut def = 1usize as *mut c_void;
        assert_eq!(
            unsafe { kmpp_objdef_get(&mut def, c"kmpp_frame".as_ptr()) },
            MPP_NOK
        );
        assert!(def.is_null());

        def = 1usize as *mut c_void;
        assert_eq!(
            unsafe { kmpp_objdef_register(&mut def, 80, c"kmpp_frame".as_ptr()) },
            MPP_NOK
        );
        assert!(def.is_null());
    }

    #[test]
    fn dec_cfg_rejects_unknown_keys() {
        let mut cfg = std::ptr::null_mut();
        assert_eq!(unsafe { mpp_dec_cfg_init(&mut cfg) }, MPP_OK);
        let name = c"base:unknown";
        assert_eq!(
            unsafe { mpp_dec_cfg_set_u32(cfg, name.as_ptr(), 1) },
            MPP_NOK
        );
        assert_eq!(unsafe { mpp_dec_cfg_deinit(cfg) }, MPP_OK);
    }

    #[test]
    fn encoder_cfg_round_trips_types_and_codec() {
        let mut cfg = std::ptr::null_mut();
        assert_eq!(unsafe { mpp_enc_cfg_init(&mut cfg) }, MPP_OK);
        assert_eq!(
            unsafe { mpp_enc_cfg_set_s32(cfg, c"codec:type".as_ptr(), 0x0100_0004) },
            MPP_OK
        );
        assert_eq!(
            unsafe { mpp_enc_cfg_set_u32(cfg, c"prep:width".as_ptr(), 320) },
            MPP_OK
        );
        let mut codec = 0;
        let mut width = 0;
        assert_eq!(
            unsafe { mpp_enc_cfg_get_s32(cfg, c"codec:type".as_ptr(), &mut codec) },
            MPP_OK
        );
        assert_eq!(
            unsafe { mpp_enc_cfg_get_u32(cfg, c"prep:width".as_ptr(), &mut width) },
            MPP_OK
        );
        assert_eq!(codec, 0x0100_0004);
        assert_eq!(width, 320);
        assert_eq!(unsafe { mpp_enc_cfg_deinit(cfg) }, MPP_OK);
    }

    #[test]
    fn encoder_ref_cfg_stores_hierarchy() {
        let mut cfg = std::ptr::null_mut();
        assert_eq!(unsafe { mpp_enc_ref_cfg_create(&mut cfg, 0) }, MPP_OK);
        assert_eq!(unsafe { mpp_enc_ref_cfg_set_cfg_cnt(cfg, 1, 3) }, MPP_OK);
        let lt = [RefLtCfg {
            lt_idx: 0,
            temporal_id: 0,
            ref_mode: 0,
            ref_arg: 0,
            lt_gap: 60,
            lt_delay: 0,
        }];
        let st = [
            RefStCfg {
                is_non_ref: 0,
                temporal_id: 0,
                ref_mode: 4,
                ref_arg: 0,
                repeat: 0,
            },
            RefStCfg {
                is_non_ref: 1,
                temporal_id: 2,
                ref_mode: 0,
                ref_arg: 0,
                repeat: 0,
            },
            RefStCfg {
                is_non_ref: 0,
                temporal_id: 0,
                ref_mode: 0,
                ref_arg: 0,
                repeat: 0,
            },
        ];
        assert_eq!(
            unsafe { mpp_enc_ref_cfg_add_lt_cfg(cfg, 1, lt.as_ptr()) },
            MPP_OK
        );
        assert_eq!(
            unsafe { mpp_enc_ref_cfg_add_st_cfg(cfg, 3, st.as_ptr()) },
            MPP_OK
        );
        assert_eq!(unsafe { mpp_enc_ref_cfg_check(cfg) }, MPP_OK);
        assert_eq!(unsafe { mpp_enc_ref_cfg_deinit(&mut cfg) }, MPP_OK);
        assert!(cfg.is_null());
    }

    #[test]
    fn reference_cfg_check_applies_vendor_hierarchy_rules() {
        let mut config = new_ref_cfg();
        config.st_count = 2;
        config.st[0] = RefStCfg {
            is_non_ref: 0,
            temporal_id: 0,
            ref_mode: 0,
            ref_arg: 0,
            repeat: 0,
        };
        config.st[1] = RefStCfg {
            is_non_ref: 0,
            temporal_id: 0,
            ref_mode: 0,
            ref_arg: 0,
            repeat: -2,
        };
        assert!(validate_ref_cfg(&mut config));
        assert_eq!(config.st[1].repeat, 0);

        config.st[0].is_non_ref = 1;
        assert!(!validate_ref_cfg(&mut config));

        config.st_count = 1;
        config.st[0].is_non_ref = 0;
        config.st[0].ref_mode = 7;
        assert!(!validate_ref_cfg(&mut config));
        config.st[0].ref_mode = 0;
        config.lt_count = 2;
        config.lt[0].lt_idx = 0;
        config.lt[1].lt_idx = 0;
        assert!(!validate_ref_cfg(&mut config));
        assert_eq!(
            unsafe { mpp_enc_ref_cfg_check(ptr::null_mut()) },
            MPP_ERR_VALUE
        );
    }

    #[test]
    fn reference_cfg_capacity_add_and_copy_follow_the_vendor_lifecycle() {
        let mut source = std::ptr::null_mut();
        let mut destination = std::ptr::null_mut();
        assert_eq!(unsafe { mpp_enc_ref_cfg_init(&mut source) }, MPP_OK);
        assert_eq!(unsafe { mpp_enc_ref_cfg_init(&mut destination) }, MPP_OK);
        assert_eq!(unsafe { mpp_enc_ref_cfg_setup(source, 1, 2) }, MPP_OK);

        let lt = [RefLtCfg {
            lt_idx: 0,
            temporal_id: 0,
            ref_mode: 0,
            ref_arg: 0,
            lt_gap: 8,
            lt_delay: 0,
        }];
        let st = [
            RefStCfg {
                is_non_ref: 0,
                temporal_id: 0,
                ref_mode: 0,
                ref_arg: 0,
                repeat: 0,
            },
            RefStCfg {
                is_non_ref: 0,
                temporal_id: 0,
                ref_mode: 0,
                ref_arg: 0,
                repeat: 0,
            },
        ];
        assert_eq!(
            unsafe { mpp_enc_ref_cfg_add_lt_cfg(source, 1, lt.as_ptr()) },
            MPP_OK
        );
        assert_eq!(
            unsafe { mpp_enc_ref_cfg_add_st_cfg(source, 2, st.as_ptr()) },
            MPP_OK
        );
        assert_eq!(
            unsafe { mpp_enc_ref_cfg_add_st_cfg(source, 1, st.as_ptr()) },
            MPP_ERR_VALUE
        );
        assert_eq!(unsafe { mpp_enc_ref_cfg_check(source) }, MPP_OK);
        assert_eq!(unsafe { mpp_enc_ref_cfg_copy(destination, source) }, MPP_OK);
        assert_eq!(
            reference_frame_pattern(destination),
            reference_frame_pattern(source)
        );
        assert_eq!(unsafe { mpp_enc_ref_cfg_setup(destination, 0, 0) }, MPP_OK);
        assert!(reference_frame_pattern(destination).is_none());

        assert_eq!(unsafe { mpp_enc_ref_cfg_deinit(&mut destination) }, MPP_OK);
        assert_eq!(unsafe { mpp_enc_ref_cfg_deinit(&mut source) }, MPP_OK);
    }

    #[test]
    fn reference_cfg_reset_defaults_to_simple_ippp() {
        let mut config = std::ptr::null_mut();
        assert_eq!(unsafe { mpp_enc_ref_cfg_init(&mut config) }, MPP_OK);
        let store = ref_cfg_ref(config).unwrap();
        assert_eq!((store.st_capacity, store.st_count), (1, 1));
        assert_eq!(
            (
                store.st[0].is_non_ref,
                store.st[0].temporal_id,
                store.st[0].ref_mode,
                store.st[0].ref_arg,
                store.st[0].repeat,
            ),
            (0, 0, 0, 0, 0)
        );
        let info = unsafe { mpp_enc_ref_cfg_get_cpb_info(config) };
        assert!(!info.is_null());
        assert_eq!(
            unsafe { *info },
            MppEncCpbInfo {
                dpb_size: 1,
                max_st_cnt: 1,
                ..MppEncCpbInfo::default()
            }
        );
        assert_eq!(unsafe { mpp_enc_ref_cfg_check(config) }, MPP_OK);
        assert_eq!(unsafe { mpp_enc_ref_cfg_deinit(&mut config) }, MPP_OK);
    }

    #[test]
    fn reference_cfg_reports_the_vendor_cpb_metadata() {
        let mut config = new_ref_cfg();
        config.st_count = 9;
        config.lt_count = 1;
        config.st[..9].copy_from_slice(&[
            RefStCfg {
                is_non_ref: 0,
                temporal_id: 0,
                ref_mode: 4,
                ref_arg: 0,
                repeat: 0,
            },
            RefStCfg {
                is_non_ref: 1,
                temporal_id: 3,
                ref_mode: 0,
                ref_arg: 0,
                repeat: 0,
            },
            RefStCfg {
                is_non_ref: 0,
                temporal_id: 2,
                ref_mode: 0,
                ref_arg: 0,
                repeat: 0,
            },
            RefStCfg {
                is_non_ref: 1,
                temporal_id: 3,
                ref_mode: 0,
                ref_arg: 0,
                repeat: 0,
            },
            RefStCfg {
                is_non_ref: 0,
                temporal_id: 1,
                ref_mode: 2,
                ref_arg: 0,
                repeat: 0,
            },
            RefStCfg {
                is_non_ref: 1,
                temporal_id: 3,
                ref_mode: 0,
                ref_arg: 0,
                repeat: 0,
            },
            RefStCfg {
                is_non_ref: 0,
                temporal_id: 2,
                ref_mode: 0,
                ref_arg: 0,
                repeat: 0,
            },
            RefStCfg {
                is_non_ref: 1,
                temporal_id: 3,
                ref_mode: 0,
                ref_arg: 0,
                repeat: 0,
            },
            RefStCfg {
                is_non_ref: 0,
                temporal_id: 0,
                ref_mode: 4,
                ref_arg: 0,
                repeat: 0,
            },
        ]);
        config.lt[0] = RefLtCfg {
            lt_idx: 0,
            temporal_id: 0,
            ref_mode: 2,
            ref_arg: 0,
            lt_gap: 8,
            lt_delay: 0,
        };
        assert!(validate_ref_cfg(&mut config));
        assert_eq!(
            config.cpb_info,
            MppEncCpbInfo {
                dpb_size: 2,
                max_lt_cnt: 1,
                max_st_cnt: 1,
                max_lt_idx: 0,
                max_st_tid: 3,
                lt_gop: 8,
                st_gop: 8,
            }
        );
    }

    #[test]
    fn reference_cfg_json_round_trips_current_vendor_schema() {
        let json = r#"{
            "keep_cpb": 0,
            "st_cfg_cnt": 3,
            "lt_cfg_cnt": 0,
            "st_cfg": [
                {"is_non_ref":0,"temporal_id":0,"ref_mode":4,"ref_arg":0,"repeat":0},
                {"is_non_ref":1,"temporal_id":1,"ref_mode":0,"ref_arg":0,"repeat":0},
                {"is_non_ref":0,"temporal_id":0,"ref_mode":0,"ref_arg":0,"repeat":0}
            ],
            "lt_cfg": []
        }"#;
        let mut config = std::ptr::null_mut();
        assert_eq!(unsafe { mpp_enc_ref_cfg_init(&mut config) }, MPP_OK);
        let text = CString::new(json).unwrap();
        assert_eq!(
            unsafe { mpp_enc_ref_cfg_apply(config, 1, text.as_ptr().cast_mut()) },
            MPP_OK
        );
        let info = unsafe { mpp_enc_ref_cfg_get_cpb_info(config) };
        assert_eq!(
            unsafe { *info },
            MppEncCpbInfo {
                dpb_size: 1,
                max_st_cnt: 1,
                max_st_tid: 1,
                st_gop: 2,
                ..MppEncCpbInfo::default()
            }
        );

        let mut output = std::ptr::null_mut();
        assert_eq!(
            unsafe { mpp_enc_ref_cfg_extract(config, 1, &mut output) },
            MPP_OK
        );
        assert!(!output.is_null());
        let output_text = unsafe { CStr::from_ptr(output) }.to_str().unwrap();
        assert_eq!(json_i32(output_text, "st_cfg_cnt"), Some(3));
        assert_eq!(json_object_array(output_text, "st_cfg").unwrap().len(), 3);
        unsafe { libc::free(output.cast()) };
        assert_eq!(unsafe { mpp_enc_ref_cfg_deinit(&mut config) }, MPP_OK);
    }

    #[test]
    fn encoder_cfg_reads_tsvc2_reference_pattern() {
        let mut encoder_cfg = std::ptr::null_mut();
        let mut reference_cfg = std::ptr::null_mut();
        assert_eq!(unsafe { mpp_enc_cfg_init(&mut encoder_cfg) }, MPP_OK);
        assert_eq!(
            unsafe { mpp_enc_ref_cfg_create(&mut reference_cfg, 0) },
            MPP_OK
        );
        assert_eq!(
            unsafe { mpp_enc_ref_cfg_set_cfg_cnt(reference_cfg, 0, 3) },
            MPP_OK
        );
        let st = [
            RefStCfg {
                is_non_ref: 0,
                temporal_id: 0,
                ref_mode: 3,
                ref_arg: 0,
                repeat: 0,
            },
            RefStCfg {
                is_non_ref: 1,
                temporal_id: 1,
                ref_mode: 0,
                ref_arg: 0,
                repeat: 0,
            },
            RefStCfg {
                is_non_ref: 0,
                temporal_id: 0,
                ref_mode: 0,
                ref_arg: 0,
                repeat: 0,
            },
        ];
        assert_eq!(
            unsafe { mpp_enc_ref_cfg_add_st_cfg(reference_cfg, 3, st.as_ptr()) },
            MPP_OK
        );
        assert_eq!(
            unsafe { mpp_enc_cfg_set_ptr(encoder_cfg, c"rc:ref_cfg".as_ptr(), reference_cfg) },
            MPP_OK
        );
        assert_eq!(
            encoder_cfg_ref_pattern(encoder_cfg),
            Some(vec![
                ReferenceFrameStep {
                    non_reference: true,
                    temporal_id: 1,
                    reference_mode: 0,
                    reference_argument: 0,
                },
                ReferenceFrameStep {
                    non_reference: false,
                    temporal_id: 0,
                    reference_mode: 0,
                    reference_argument: 0,
                },
            ])
        );
        assert_eq!(unsafe { mpp_enc_cfg_deinit(encoder_cfg) }, MPP_OK);
        assert_eq!(
            unsafe { mpp_enc_ref_cfg_deinit(&mut reference_cfg) },
            MPP_OK
        );
    }

    #[test]
    fn reference_cfg_reads_tsvc3_steps() {
        let mut config = new_ref_cfg();
        config.st_count = 5;
        config.st[..5].copy_from_slice(&[
            RefStCfg {
                temporal_id: 0,
                ref_mode: 4,
                ..RefStCfg::default()
            },
            RefStCfg {
                is_non_ref: 1,
                temporal_id: 2,
                ..RefStCfg::default()
            },
            RefStCfg {
                temporal_id: 1,
                ..RefStCfg::default()
            },
            RefStCfg {
                is_non_ref: 1,
                temporal_id: 2,
                ..RefStCfg::default()
            },
            RefStCfg {
                temporal_id: 0,
                ref_mode: 4,
                ..RefStCfg::default()
            },
        ]);
        let pattern = reference_frame_pattern((&mut config as *mut RefCfg).cast()).unwrap();
        assert_eq!(
            pattern,
            [
                ReferenceFrameStep {
                    non_reference: true,
                    temporal_id: 2,
                    reference_mode: 0,
                    reference_argument: 0,
                },
                ReferenceFrameStep {
                    non_reference: false,
                    temporal_id: 1,
                    reference_mode: 0,
                    reference_argument: 0,
                },
                ReferenceFrameStep {
                    non_reference: true,
                    temporal_id: 2,
                    reference_mode: 0,
                    reference_argument: 0,
                },
                ReferenceFrameStep {
                    non_reference: false,
                    temporal_id: 0,
                    reference_mode: 4,
                    reference_argument: 0,
                },
            ]
        );
    }

    #[test]
    fn reference_cfg_reads_tsvc4_and_long_term() {
        let mut config = new_ref_cfg();
        config.lt_count = 1;
        config.lt[0] = RefLtCfg {
            lt_idx: 0,
            temporal_id: 0,
            ref_mode: 2,
            ref_arg: 0,
            lt_gap: 8,
            lt_delay: 0,
        };
        config.st_count = 9;
        config.st[..9].copy_from_slice(&[
            RefStCfg {
                temporal_id: 0,
                ref_mode: 4,
                ..RefStCfg::default()
            },
            RefStCfg {
                is_non_ref: 1,
                temporal_id: 3,
                ..RefStCfg::default()
            },
            RefStCfg {
                temporal_id: 2,
                ..RefStCfg::default()
            },
            RefStCfg {
                is_non_ref: 1,
                temporal_id: 3,
                ..RefStCfg::default()
            },
            RefStCfg {
                temporal_id: 1,
                ref_mode: 2,
                ..RefStCfg::default()
            },
            RefStCfg {
                is_non_ref: 1,
                temporal_id: 3,
                ..RefStCfg::default()
            },
            RefStCfg {
                temporal_id: 2,
                ..RefStCfg::default()
            },
            RefStCfg {
                is_non_ref: 1,
                temporal_id: 3,
                ..RefStCfg::default()
            },
            RefStCfg {
                temporal_id: 0,
                ref_mode: 4,
                ..RefStCfg::default()
            },
        ]);
        let pointer = (&mut config as *mut RefCfg).cast();
        let pattern = reference_frame_pattern(pointer).unwrap();
        assert_eq!(pattern.len(), 8);
        assert_eq!(pattern[0].temporal_id, 3);
        assert!(pattern[0].non_reference);
        assert_eq!(pattern[3].reference_mode, 2);
        assert_eq!(pattern[7].reference_mode, 4);
        assert_eq!(
            reference_long_term_config(pointer),
            Some(LongTermReferenceConfig {
                index: 0,
                gap: 8,
                delay: 0,
                reference_mode: 2,
                reference_argument: 0,
            })
        );
    }

    #[test]
    fn environment_default_and_override_match_c_conventions() {
        let name = c"RK_MPP_STANDALONE_TEST_ENV";
        let mut value = 99;
        assert_eq!(
            unsafe { mpp_env_get_u32(name.as_ptr(), &mut value, 17) },
            MPP_OK
        );
        assert_eq!(value, 17);
    }

    #[test]
    fn time_is_monotonic_microseconds() {
        let start = mpp_time();
        std::thread::sleep(std::time::Duration::from_micros(1));
        assert!(mpp_time() >= start);
    }

    #[test]
    fn trie_info_layout_is_usable_by_the_c_inline_accessors() {
        let mut trie = std::ptr::null_mut();
        assert_eq!(
            unsafe { mpp_trie_init(&mut trie, c"mpp_opt".as_ptr()) },
            MPP_OK
        );
        let name = c"i";
        let mut context = [11u64, 22, 33];
        assert_eq!(
            unsafe {
                mpp_trie_add_info(
                    trie,
                    name.as_ptr(),
                    context.as_mut_ptr().cast(),
                    std::mem::size_of_val(&context) as u32,
                )
            },
            MPP_OK
        );
        let info = unsafe { mpp_trie_get_info(trie, name.as_ptr()) }.cast::<u8>();
        assert!(!info.is_null());
        let header = unsafe { std::ptr::read_unaligned(info.cast::<u32>()) };
        let context_offset = 4 + (header >> 24) as usize;
        let stored_name = unsafe { std::slice::from_raw_parts(info.add(4), 2) };
        let stored_context =
            unsafe { std::slice::from_raw_parts(info.add(context_offset).cast::<u64>(), 3) };
        assert_eq!(stored_name, b"i\0");
        assert_eq!(stored_context, context);
    }

    #[test]
    fn legacy_option_trie_returns_the_mpp_opt_info_context() {
        let mut trie = std::ptr::null_mut();
        assert_eq!(
            unsafe { mpp_trie_init(&mut trie, 0x47 as *const c_char) },
            MPP_OK
        );
        let name = c"codec";
        let mut option = [
            name.as_ptr().cast::<c_void>(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ];
        assert_eq!(
            unsafe { mpp_trie_add_info(trie, option.as_mut_ptr().cast(), std::ptr::null_mut(), 0) },
            MPP_OK
        );
        let info = unsafe { mpp_trie_get_info(trie, name.as_ptr()) };
        assert_eq!(
            info.cast_const().cast::<c_void>(),
            option.as_mut_ptr().cast()
        );
    }

    #[test]
    fn singleton_base_accepts_an_id_entry() {
        let mut entries = [SglnInfo {
            id: -1,
            name: std::ptr::null(),
            init: None,
            deinit: None,
        }; 2];
        let mut list = SglnList {
            count: 0,
            capacity: 2,
            entries: entries.as_mut_ptr(),
        };
        let mut base = SglnBase {
            name: c"test_singleton".as_ptr(),
            arrays: [&mut list, std::ptr::null_mut()],
            array_count: 1,
            max_name_len: 0,
            debug: 0,
            init_done: 0,
        };
        let mut info = SglnInfo {
            id: 1,
            name: c"entry".as_ptr(),
            init: None,
            deinit: None,
        };
        assert_eq!(
            unsafe { mpp_sgln_base_add(&mut base, &mut info, std::ptr::null()) },
            MPP_OK
        );
        assert_eq!(list.count, 2);
        assert_eq!(unsafe { (*list.entries.add(1)).id }, 1);
    }
}

// ---- MPP log level and callback (mpp_log.h) --------------------------------

use std::sync::atomic::{AtomicI32, AtomicPtr};

static LOG_LEVEL: AtomicI32 = AtomicI32::new(0); // 0 = resolve from env/default
static LOG_CB_CTX: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());
static LOG_CB_FN: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

/// `mpp_get_log_level()` default: MPP_LOG_INFO, overridable with the same
/// `mpp_log_level` environment variable the vendor runtime reads.
fn resolved_log_level() -> c_int {
    let stored = LOG_LEVEL.load(Ordering::Relaxed);
    if stored != 0 {
        return stored;
    }
    let from_env = std::env::var("mpp_log_level")
        .ok()
        .and_then(|value| value.parse::<i32>().ok());
    match from_env {
        Some(level) if (1..=7).contains(&level) => level,
        _ => 4, // MPP_LOG_INFO
    }
}

#[no_mangle]
pub extern "C" fn mpp_set_log_level(level: c_int) {
    let level = if (1..=7).contains(&level) { level } else { 4 };
    LOG_LEVEL.store(level, Ordering::Relaxed);
}

#[no_mangle]
pub extern "C" fn mpp_get_log_level() -> c_int {
    resolved_log_level()
}

/// `mpp_set_log_callback(ctx, cb)`. Passing a null callback restores the
/// default stderr logger. Returns MPP_OK.
#[no_mangle]
pub unsafe extern "C" fn mpp_set_log_callback(ctx: *mut c_void, cb: *mut c_void) -> c_int {
    LOG_CB_CTX.store(ctx, Ordering::Relaxed);
    LOG_CB_FN.store(cb, Ordering::Relaxed);
    0
}

/// Accessors used by the C variadic trampoline in `standalone_log.c`.
#[no_mangle]
pub extern "C" fn rk_mpp_log_level_value() -> c_int {
    resolved_log_level()
}

#[no_mangle]
pub extern "C" fn rk_mpp_log_cb_ctx() -> *mut c_void {
    LOG_CB_CTX.load(Ordering::Relaxed)
}

#[no_mangle]
pub extern "C" fn rk_mpp_log_cb_fn() -> *mut c_void {
    LOG_CB_FN.load(Ordering::Relaxed)
}
