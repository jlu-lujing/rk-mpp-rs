use std::ffi::{c_int, c_void};
use std::mem::size_of;

use super::objects;
use super::{MPP_ERR_NULL_PTR, MPP_NOK, MPP_OK};
use crate::hal::osd;

const CHANGE_MODE: u32 = 1;
const CHANGE_PALETTE: u32 = 2;
const TYPE_USER_DEFINED: u32 = 1;

#[repr(C)]
pub(super) struct Palette {
    data: [u32; osd::PALETTE_COUNT],
}

#[repr(C)]
pub(super) struct PaletteCfg {
    change: u32,
    type_: u32,
    palette: *mut Palette,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Region {
    enable: u32,
    inverse: u32,
    start_mb_x: u32,
    start_mb_y: u32,
    num_mb_x: u32,
    num_mb_y: u32,
    buffer_offset: u32,
}

#[repr(C)]
struct Data {
    buffer: *mut c_void,
    num_region: u32,
    regions: [Region; osd::REGION_COUNT],
}

pub(super) struct PaletteState {
    data: Box<Palette>,
    type_: u32,
    change: u32,
}

impl PaletteState {
    pub(super) fn new() -> Self {
        Self {
            data: Box::new(Palette {
                data: [0; osd::PALETTE_COUNT],
            }),
            type_: 0,
            change: 0,
        }
    }

    pub(super) fn set(&mut self, config: *const PaletteCfg) -> c_int {
        if config.is_null() {
            return MPP_ERR_NULL_PTR;
        }
        // SAFETY: the control command uses the official MppEncOSDPltCfg layout.
        let config = unsafe { &*config };
        let change = config.change;
        if change == 0 {
            return MPP_OK;
        }
        if change & CHANGE_MODE != 0 {
            if config.type_ >= 2 {
                return MPP_NOK;
            }
            self.type_ = config.type_;
        }
        if change & CHANGE_PALETTE != 0 {
            if config.palette.is_null() {
                return MPP_NOK;
            }
            // SAFETY: the caller supplied the official 256-entry palette.
            self.data
                .data
                .copy_from_slice(unsafe { &(*config.palette).data });
        }
        self.change = change;
        MPP_OK
    }

    pub(super) fn get(&self, output: *mut PaletteCfg) -> c_int {
        if output.is_null() {
            return MPP_ERR_NULL_PTR;
        }
        // SAFETY: output points to a writable MppEncOSDPltCfg.
        unsafe {
            output.write(PaletteCfg {
                change: self.change,
                type_: self.type_,
                palette: (&*self.data) as *const Palette as *mut Palette,
            });
        }
        MPP_OK
    }

    fn user_palette(&self) -> Option<[u32; osd::PALETTE_COUNT]> {
        (self.type_ == TYPE_USER_DEFINED).then_some(self.data.data)
    }
}

pub(super) unsafe fn frame_config(
    data: *mut c_void,
    palette: &PaletteState,
) -> Result<Option<osd::Config>, String> {
    if data.is_null() {
        return Ok(None);
    }
    // SAFETY: KEY_OSD_DATA points to the official MppEncOSDData structure.
    let data = unsafe { &*(data.cast::<Data>()) };
    if data.num_region == 0 {
        return Ok(None);
    }
    if data.num_region as usize > osd::REGION_COUNT {
        return Err(format!(
            "OSD data has {} regions, maximum is {}",
            data.num_region,
            osd::REGION_COUNT
        ));
    }
    // SAFETY: the caller owns this MppBuffer for the duration of the encode.
    let buffer_fd = unsafe { objects::buffer_fd(data.buffer) }
        .ok_or_else(|| "OSD data has no valid buffer fd".to_string())?;
    let buffer_size = unsafe { objects::buffer_capacity(data.buffer) }
        .ok_or_else(|| "OSD data has no valid buffer".to_string())?;
    if !unsafe { objects::buffer_sync_write_end(data.buffer) } {
        return Err("OSD data cache flush failed".to_string());
    }
    let regions = data.regions[..data.num_region as usize]
        .iter()
        .map(|region| osd::Region {
            enable: region.enable,
            inverse: region.inverse,
            start_mb_x: region.start_mb_x,
            start_mb_y: region.start_mb_y,
            num_mb_x: region.num_mb_x,
            num_mb_y: region.num_mb_y,
            buffer_offset: region.buffer_offset,
        })
        .collect();
    Ok(Some(osd::Config {
        buffer_fd,
        buffer_size,
        regions,
        user_palette: palette.user_palette(),
    }))
}

const _: () = {
    assert!(size_of::<PaletteCfg>() == 16);
    assert!(size_of::<Data>() == 240);
};
