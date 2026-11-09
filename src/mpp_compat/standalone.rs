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
const DEC_CFG_MAGIC: u64 = 0x524b_4445_4343_4647;

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

#[no_mangle]
pub unsafe extern "C" fn _mpp_log_l(
    _level: c_int,
    tag: *const c_char,
    format: *const c_char,
    function: *const c_char,
) {
    log_unformatted(tag, format, function);
}

#[no_mangle]
pub unsafe extern "C" fn _mpp_log(
    tag: *const c_char,
    format: *const c_char,
    function: *const c_char,
) {
    log_unformatted(tag, format, function);
}

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
pub unsafe extern "C" fn kmpp_objdef_get(
    def: *mut *mut c_void,
    _private_size: i32,
    name: *const c_char,
    _flags: u32,
) -> i32 {
    if def.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    let name = match obj_def_name(name) {
        Ok(name) => name,
        Err(error) => return error,
    };
    unsafe { def.write(find_or_create_obj_def(name)) };
    MPP_OK
}

#[no_mangle]
pub unsafe extern "C" fn kmpp_objdef_register(
    def: *mut *mut c_void,
    _private_size: i32,
    _size: i32,
    name: *const c_char,
    _flags: u32,
) -> i32 {
    unsafe { kmpp_objdef_get(def, 0, name, 0) }
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
    let name = match obj_def_name(name) {
        Ok(name) => name,
        Err(error) => return error,
    };
    let pointer = Box::into_raw(Box::new(TrieStub {
        name,
        nodes: 0,
        infos: Vec::new(),
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
    context: *mut c_void,
    context_size: u32,
) -> i32 {
    let Some(trie) = trie_mut(trie) else {
        return MPP_NOK;
    };
    if name.is_null() {
        return MPP_OK;
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
    trie.infos
        .push(Box::into_raw(info.into_boxed_slice()).cast::<u8>() as usize);
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
    let name = unsafe { CStr::from_ptr(name) }.to_bytes();
    trie.infos
        .iter()
        .copied()
        .find(|pointer| {
            let info = *pointer as *const u8;
            let header = unsafe { std::ptr::read_unaligned(info.cast::<u32>()) };
            let length = (header >> 24) as usize;
            let stored = unsafe { std::slice::from_raw_parts(info.add(4), length) };
            stored.starts_with(name) && stored.get(name.len()) == Some(&0)
        })
        .map_or(ptr::null_mut(), |pointer| pointer as *mut c_void)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_get_info_first(trie: *mut c_void) -> *mut c_void {
    trie_mut(trie)
        .and_then(|trie| trie.infos.first().copied())
        .map_or(ptr::null_mut(), |pointer| pointer as *mut c_void)
}

#[no_mangle]
pub unsafe extern "C" fn mpp_trie_get_info_next(
    trie: *mut c_void,
    info: *mut c_void,
) -> *mut c_void {
    let Some(trie) = trie_mut(trie) else {
        return ptr::null_mut();
    };
    let Some(index) = trie
        .infos
        .iter()
        .position(|pointer| *pointer == info as usize)
    else {
        return ptr::null_mut();
    };
    trie.infos
        .get(index + 1)
        .copied()
        .map_or(ptr::null_mut(), |pointer| pointer as *mut c_void)
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

static ENV_STRINGS: OnceLock<Mutex<Vec<CString>>> = OnceLock::new();
static SOC_NAME: OnceLock<CString> = OnceLock::new();
static OBJ_DEFS: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();
static TRIES: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();

struct TrieStub {
    name: CString,
    nodes: i32,
    infos: Vec<usize>,
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

fn find_or_create_obj_def(name: CString) -> *mut c_void {
    let mut defs = obj_defs().lock().unwrap();
    for pointer in defs.iter().copied() {
        let def = unsafe { &*(pointer as *const ObjDefStub) };
        if def.name == name {
            return pointer as *mut c_void;
        }
    }
    let pointer = Box::into_raw(Box::new(ObjDefStub { name, next_node: 1 })) as usize;
    defs.push(pointer);
    pointer as *mut c_void
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
