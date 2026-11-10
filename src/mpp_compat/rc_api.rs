use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::mem::size_of;
use std::ptr;
use std::sync::{Mutex, MutexGuard, OnceLock};

use super::{
    MPP_ERR_NULL_PTR, MPP_NOK, MPP_OK, MPP_VIDEO_CODING_AVC, MPP_VIDEO_CODING_HEVC,
    MPP_VIDEO_CODING_MJPEG, MPP_VIDEO_CODING_VP8,
};

type RcCallback = unsafe extern "C" fn(*mut c_void, *mut c_void) -> c_int;

#[repr(C)]
#[derive(Clone, Copy)]
pub(super) struct RcImplApi {
    pub name: *const c_char,
    pub coding: c_int,
    pub ctx_size: u32,
    pub init: Option<RcCallback>,
    pub deinit: Option<RcCallback>,
    pub check_drop: Option<RcCallback>,
    pub check_reenc: Option<RcCallback>,
    pub frame_start: Option<RcCallback>,
    pub frame_end: Option<RcCallback>,
    pub hal_start: Option<RcCallback>,
    pub hal_end: Option<RcCallback>,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(super) struct RcApiBrief {
    pub name: *const c_char,
    pub coding: c_int,
}

#[repr(C)]
pub(super) struct RcApiQueryAll {
    pub brief: *mut RcApiBrief,
    pub max_count: c_int,
    pub count: c_int,
}

#[repr(C)]
pub(super) struct RcApiQueryType {
    pub brief: *mut RcApiBrief,
    pub max_count: c_int,
    pub coding: c_int,
    pub count: c_int,
}

struct Registration {
    name: CString,
    coding: c_int,
    #[allow(dead_code)] // Retained for the per-frame callback bridge.
    callbacks: RcCallbacks,
}

#[derive(Clone, Copy)]
#[allow(dead_code)] // Retained until custom strategy execution is wired.
struct RcCallbacks {
    init: Option<RcCallback>,
    deinit: Option<RcCallback>,
    check_drop: Option<RcCallback>,
    check_reenc: Option<RcCallback>,
    frame_start: Option<RcCallback>,
    frame_end: Option<RcCallback>,
    hal_start: Option<RcCallback>,
    hal_end: Option<RcCallback>,
}

static REGISTRY: OnceLock<Mutex<Vec<Registration>>> = OnceLock::new();

fn lock_registry() -> MutexGuard<'static, Vec<Registration>> {
    REGISTRY
        .get_or_init(|| Mutex::new(builtins()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn builtins() -> Vec<Registration> {
    [
        ("default", MPP_VIDEO_CODING_AVC),
        ("default", MPP_VIDEO_CODING_HEVC),
        ("default", MPP_VIDEO_CODING_MJPEG),
        ("default", MPP_VIDEO_CODING_VP8),
        ("smart", MPP_VIDEO_CODING_AVC),
        ("smart", MPP_VIDEO_CODING_HEVC),
    ]
    .into_iter()
    .map(|(name, coding)| {
        new_registration(
            CString::new(name).expect("built-in RC API names contain no NUL"),
            coding,
            None,
        )
    })
    .collect()
}

fn new_registration(name: CString, coding: c_int, api: Option<&RcImplApi>) -> Registration {
    let callbacks = api.map_or(
        RcCallbacks {
            init: None,
            deinit: None,
            check_drop: None,
            check_reenc: None,
            frame_start: None,
            frame_end: None,
            hal_start: None,
            hal_end: None,
        },
        |api| RcCallbacks {
            init: api.init,
            deinit: api.deinit,
            check_drop: api.check_drop,
            check_reenc: api.check_reenc,
            frame_start: api.frame_start,
            frame_end: api.frame_end,
            hal_start: api.hal_start,
            hal_end: api.hal_end,
        },
    );
    Registration {
        name,
        coding,
        callbacks,
    }
}

fn same_name(left: &CStr, right: &CStr) -> bool {
    let left = left.to_bytes();
    let right = right.to_bytes();
    let len = left.len().min(right.len()).min(31);
    left[..len] == right[..len] && (left.len().min(31) == right.len().min(31))
}

fn add_unlocked(registry: &mut Vec<Registration>, api: &RcImplApi) -> c_int {
    if api.name.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    // SAFETY: the public ABI requires a NUL-terminated strategy name.
    let requested = unsafe { CStr::from_ptr(api.name) };
    let registration = new_registration(requested.to_owned(), api.coding, Some(api));
    if let Some(existing) = registry
        .iter_mut()
        .find(|existing| existing.coding == api.coding && same_name(&existing.name, requested))
    {
        *existing = registration;
    } else {
        registry.push(registration);
    }
    MPP_OK
}

pub(super) unsafe fn add(api: *const RcImplApi) -> c_int {
    if api.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    // SAFETY: the caller passed the official RcImplApi pointer for this
    // command. The registry copies the struct before the control returns.
    let api = unsafe { &*api };
    add_unlocked(&mut lock_registry(), api)
}

unsafe fn write_briefs(
    output: *mut RcApiBrief,
    max_count: c_int,
    count: *mut c_int,
    registry: &[Registration],
    coding: Option<c_int>,
) -> c_int {
    if output.is_null() || count.is_null() || max_count <= 0 {
        return MPP_NOK;
    }
    let mut written = 0usize;
    for registration in registry {
        if coding.is_some_and(|coding| coding != registration.coding) {
            continue;
        }
        if written >= max_count as usize {
            break;
        }
        // SAFETY: output points to max_count writable RcApiBrief entries.
        unsafe {
            output.add(written).write(RcApiBrief {
                name: registration.name.as_ptr(),
                coding: registration.coding,
            });
            *count = written as c_int + 1;
        }
        written += 1;
    }
    if written == 0 {
        // SAFETY: count was checked non-null above.
        unsafe { *count = 0 };
    }
    MPP_OK
}

pub(super) unsafe fn query_all(query: *mut RcApiQueryAll) -> c_int {
    if query.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    // SAFETY: command arguments use the official RcApiQueryAll layout.
    let query = unsafe { &mut *query };
    let registry = lock_registry();
    // SAFETY: all pointers originate from the caller's query structure.
    unsafe {
        write_briefs(
            query.brief,
            query.max_count,
            &mut query.count,
            &registry,
            None,
        )
    }
}

pub(super) unsafe fn query_by_type(query: *mut RcApiQueryType) -> c_int {
    if query.is_null() {
        return MPP_ERR_NULL_PTR;
    }
    // SAFETY: command arguments use the official RcApiQueryType layout.
    let query = unsafe { &mut *query };
    let registry = lock_registry();
    // SAFETY: all pointers originate from the caller's query structure.
    unsafe {
        write_briefs(
            query.brief,
            query.max_count,
            &mut query.count,
            &registry,
            Some(query.coding),
        )
    }
}

pub(super) struct Selection {
    name: Option<CString>,
    coding: c_int,
}

impl Selection {
    pub(super) fn new() -> Self {
        Self {
            name: Some(CString::new("default").expect("literal")),
            coding: 0,
        }
    }

    pub(super) fn get(&self, output: *mut RcApiBrief) -> c_int {
        if output.is_null() {
            return MPP_ERR_NULL_PTR;
        }
        // SAFETY: output points to the official writable RcApiBrief.
        unsafe {
            *output = RcApiBrief {
                name: self.name.as_ref().map_or(ptr::null(), |name| name.as_ptr()),
                coding: self.coding,
            };
        }
        MPP_OK
    }

    pub(super) fn set(&mut self, input: *const RcApiBrief, coding: c_int) -> c_int {
        if input.is_null() {
            return MPP_ERR_NULL_PTR;
        }
        // SAFETY: input points to the official RcApiBrief for this command.
        let input = unsafe { &*input };
        if input.coding != coding {
            return MPP_NOK;
        }
        self.name = if input.name.is_null() {
            None
        } else {
            // SAFETY: the public ABI requires a NUL-terminated strategy name.
            Some(unsafe { CStr::from_ptr(input.name) }.to_owned())
        };
        self.coding = input.coding;
        MPP_OK
    }

    pub(super) fn is_native_default(&self) -> bool {
        self.name
            .as_deref()
            .is_none_or(|name| name.to_bytes() == b"default")
    }

    pub(super) fn name(&self) -> Option<&CStr> {
        self.name.as_deref()
    }
}

const _: () = {
    assert!(size_of::<RcApiBrief>() == 16);
    assert!(size_of::<RcApiQueryAll>() == 16);
    assert!(size_of::<RcApiQueryType>() == 24);
    assert!(size_of::<RcImplApi>() == 80);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_matches_the_vendor_query_and_replacement_rules() {
        let mut registry = builtins();
        assert_eq!(registry.len(), 6);
        assert_eq!(registry[0].name.to_str().unwrap(), "default");
        assert_eq!(registry[0].coding, MPP_VIDEO_CODING_AVC);
        assert_eq!(registry[4].name.to_str().unwrap(), "smart");

        let api = RcImplApi {
            name: c"codex_rc_test".as_ptr(),
            coding: MPP_VIDEO_CODING_AVC,
            ctx_size: 0,
            init: None,
            deinit: None,
            check_drop: None,
            check_reenc: None,
            frame_start: None,
            frame_end: None,
            hal_start: None,
            hal_end: None,
        };
        assert_eq!(add_unlocked(&mut registry, &api), MPP_OK);
        assert_eq!(registry.len(), 7);
        assert_eq!(add_unlocked(&mut registry, &api), MPP_OK);
        assert_eq!(registry.len(), 7);

        let mut briefs = [RcApiBrief {
            name: ptr::null(),
            coding: 0,
        }; 16];
        let mut count = -1;
        let result = unsafe {
            write_briefs(
                briefs.as_mut_ptr(),
                briefs.len() as c_int,
                &mut count,
                &registry,
                Some(MPP_VIDEO_CODING_AVC),
            )
        };
        assert_eq!(result, MPP_OK);
        assert_eq!(count, 3);
        assert_eq!(
            unsafe { CStr::from_ptr(briefs[2].name) }.to_bytes(),
            b"codex_rc_test"
        );
    }

    #[test]
    fn selection_tracks_the_default_and_rejects_a_mismatched_coding() {
        let mut selection = Selection::new();
        assert!(selection.is_native_default());
        let input = RcApiBrief {
            name: c"smart".as_ptr(),
            coding: MPP_VIDEO_CODING_HEVC,
        };
        assert_eq!(selection.set(&input, MPP_VIDEO_CODING_HEVC), MPP_OK);
        assert!(!selection.is_native_default());
        assert_eq!(selection.name().unwrap().to_bytes(), b"smart");
        assert_eq!(selection.set(&input, MPP_VIDEO_CODING_AVC), MPP_NOK);
    }
}
