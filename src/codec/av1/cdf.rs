//! AV1 probability state with the exact C MPP hardware ABI.
//!
//! The AV1D_VPU HAL copies `AV1CDFs` as one raw memory object into its
//! probability DMA buffer. Field order, array extents and the otherwise unused
//! dummy fields are therefore part of the hardware interface.

use std::fmt;
use std::mem::{offset_of, size_of};
use std::ptr;

pub const REF_FRAME_COUNT: usize = 8;
pub const CDF_SIZE: usize = 12_256;
pub const MV_CDF_OFFSET: usize = 2_192;
pub const MV_CDF_SIZE: usize = 138;

const Q_TABLES: [&[u8; CDF_SIZE]; 4] = [
    include_bytes!("data/cdf_q000.bin"),
    include_bytes!("data/cdf_q021.bin"),
    include_bytes!("data/cdf_q061.bin"),
    include_bytes!("data/cdf_q121.bin"),
];
const INTRABC_MV: &[u8; MV_CDF_SIZE] = include_bytes!("data/cdf_ndvc.bin");

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Av1MvCdfs {
    pub joint_cdf: [u16; 3],
    pub sign_cdf: [u16; 2],
    pub clsss_cdf: [[u16; 10]; 2],
    pub clsss0_fp_cdf: [[[u16; 3]; 2]; 2],
    pub fp_cdf: [[u16; 3]; 2],
    pub class0_hp_cdf: [u16; 2],
    pub hp_cdf: [u16; 2],
    pub class0_cdf: [u16; 2],
    pub bits_cdf: [[u16; 10]; 2],
}

impl Av1MvCdfs {
    const ZERO: Self = Self {
        joint_cdf: [0; 3],
        sign_cdf: [0; 2],
        clsss_cdf: [[0; 10]; 2],
        clsss0_fp_cdf: [[[0; 3]; 2]; 2],
        fp_cdf: [[0; 3]; 2],
        class0_hp_cdf: [0; 2],
        hp_cdf: [0; 2],
        class0_cdf: [0; 2],
        bits_cdf: [[0; 10]; 2],
    };

    pub fn default_intrabc() -> Self {
        from_bytes(INTRABC_MV)
    }

    pub fn as_bytes(&self) -> &[u8] {
        as_bytes(self)
    }
}

impl Default for Av1MvCdfs {
    fn default() -> Self {
        Self::ZERO
    }
}

impl fmt::Debug for Av1MvCdfs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Av1MvCdfs")
            .field("bytes", &self.as_bytes().len())
            .field("fnv1a64", &fnv1a64(self.as_bytes()))
            .finish()
    }
}

/// Exact layout of MPP's `AV1CDFs`; do not remove or reorder dummy fields.
#[repr(C)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Av1Cdfs {
    pub partition_cdf: [[u16; 16]; 13],
    pub kf_ymode_cdf: [[[u16; 12]; 5]; 5],
    pub segment_pred_cdf: [u16; 3],
    pub spatial_pred_seg_tree_cdf: [[u16; 7]; 3],
    pub mbskip_cdf: [u16; 3],
    pub delta_q_cdf: [u16; 3],
    pub delta_lf_multi_cdf: [[u16; 3]; 4],
    pub delta_lf_cdf: [u16; 3],
    pub skip_mode_cdf: [u16; 3],
    pub vartx_part_cdf: [[u16; 1]; 22],
    pub tx_size_cdf: [[[u16; 2]; 3]; 4],
    pub if_ymode_cdf: [[u16; 12]; 4],
    pub uv_mode_cdf: [[[u16; 13]; 13]; 2],
    pub intra_inter_cdf: [u16; 4],
    pub comp_inter_cdf: [u16; 5],
    pub single_ref_cdf: [[u16; 6]; 3],
    pub comp_ref_type_cdf: [[u16; 1]; 5],
    pub uni_comp_ref_cdf: [[[u16; 1]; 3]; 3],
    pub comp_ref_cdf: [[u16; 3]; 3],
    pub comp_bwdref_cdf: [[u16; 2]; 3],
    pub newmv_cdf: [u16; 6],
    pub zeromv_cdf: [u16; 2],
    pub refmv_cdf: [u16; 9],
    pub drl_cdf: [u16; 3],
    pub interp_filter_cdf: [[u16; 2]; 16],

    pub mv_cdf: Av1MvCdfs,

    pub obmc_cdf: [u16; 22],
    pub motion_mode_cdf: [[u16; 2]; 22],
    pub inter_compound_mode_cdf: [[u16; 7]; 15],
    pub compound_type_cdf: [[u16; 1]; 22],
    pub interintra_cdf: [u16; 4],
    pub interintra_mode_cdf: [[u16; 3]; 4],
    pub wedge_interintra_cdf: [u16; 22],
    pub wedge_idx_cdf: [[u16; 15]; 22],
    pub palette_y_mode_cdf: [[[u16; 1]; 3]; 7],
    pub palette_uv_mode_cdf: [[u16; 1]; 2],
    pub palette_y_size_cdf: [[u16; 6]; 7],
    pub palette_uv_size_cdf: [[u16; 6]; 7],
    pub cfl_sign_cdf: [u16; 7],
    pub cfl_alpha_cdf: [[u16; 15]; 6],
    pub intrabc_cdf: [u16; 1],
    pub angle_delta_cdf: [[u16; 6]; 8],
    pub filter_intra_mode_cdf: [u16; 4],
    pub filter_intra_cdf: [u16; 22],
    pub comp_group_idx_cdf: [[u16; 1]; 7],
    pub compound_idx_cdf: [[u16; 1]; 6],
    pub dummy0: [u16; 14],
    pub palette_y_color_index_cdf: [[u16; 8]; 18],
    pub palette_uv_color_index_cdf: [[u16; 8]; 18],
    pub tx_type_intra0_cdf: [[[u16; 8]; 13]; 4],
    pub tx_type_intra1_cdf: [[[u16; 4]; 13]; 4],
    pub tx_type_inter_cdf: [[[u16; 16]; 4]; 2],
    pub txb_skip_cdf: [[[u16; 1]; 13]; 5],
    pub eob_extra_cdf: [[[[u16; 1]; 9]; 2]; 5],
    pub dummy_: [u16; 5],
    pub eob_flag_cdf16: [[[u16; 4]; 2]; 2],
    pub eob_flag_cdf32: [[[u16; 8]; 2]; 2],
    pub eob_flag_cdf64: [[[u16; 8]; 2]; 2],
    pub eob_flag_cdf128: [[[u16; 8]; 2]; 2],
    pub eob_flag_cdf256: [[[u16; 8]; 2]; 2],
    pub eob_flag_cdf512: [[[u16; 16]; 2]; 2],
    pub eob_flag_cdf1024: [[[u16; 16]; 2]; 2],
    pub coeff_base_eob_cdf: [[[[u16; 2]; 4]; 2]; 5],
    pub coeff_base_cdf: [[[[u16; 4]; 42]; 2]; 5],
    pub dc_sign_cdf: [[[u16; 1]; 3]; 2],
    pub dummy_2: [u16; 2],
    pub coeff_br_cdf: [[[[u16; 4]; 21]; 2]; 5],
    pub dummy2: [u16; 16],
}

impl Av1Cdfs {
    const ZERO: Self = Self {
        partition_cdf: [[0; 16]; 13],
        kf_ymode_cdf: [[[0; 12]; 5]; 5],
        segment_pred_cdf: [0; 3],
        spatial_pred_seg_tree_cdf: [[0; 7]; 3],
        mbskip_cdf: [0; 3],
        delta_q_cdf: [0; 3],
        delta_lf_multi_cdf: [[0; 3]; 4],
        delta_lf_cdf: [0; 3],
        skip_mode_cdf: [0; 3],
        vartx_part_cdf: [[0; 1]; 22],
        tx_size_cdf: [[[0; 2]; 3]; 4],
        if_ymode_cdf: [[0; 12]; 4],
        uv_mode_cdf: [[[0; 13]; 13]; 2],
        intra_inter_cdf: [0; 4],
        comp_inter_cdf: [0; 5],
        single_ref_cdf: [[0; 6]; 3],
        comp_ref_type_cdf: [[0; 1]; 5],
        uni_comp_ref_cdf: [[[0; 1]; 3]; 3],
        comp_ref_cdf: [[0; 3]; 3],
        comp_bwdref_cdf: [[0; 2]; 3],
        newmv_cdf: [0; 6],
        zeromv_cdf: [0; 2],
        refmv_cdf: [0; 9],
        drl_cdf: [0; 3],
        interp_filter_cdf: [[0; 2]; 16],
        mv_cdf: Av1MvCdfs::ZERO,
        obmc_cdf: [0; 22],
        motion_mode_cdf: [[0; 2]; 22],
        inter_compound_mode_cdf: [[0; 7]; 15],
        compound_type_cdf: [[0; 1]; 22],
        interintra_cdf: [0; 4],
        interintra_mode_cdf: [[0; 3]; 4],
        wedge_interintra_cdf: [0; 22],
        wedge_idx_cdf: [[0; 15]; 22],
        palette_y_mode_cdf: [[[0; 1]; 3]; 7],
        palette_uv_mode_cdf: [[0; 1]; 2],
        palette_y_size_cdf: [[0; 6]; 7],
        palette_uv_size_cdf: [[0; 6]; 7],
        cfl_sign_cdf: [0; 7],
        cfl_alpha_cdf: [[0; 15]; 6],
        intrabc_cdf: [0; 1],
        angle_delta_cdf: [[0; 6]; 8],
        filter_intra_mode_cdf: [0; 4],
        filter_intra_cdf: [0; 22],
        comp_group_idx_cdf: [[0; 1]; 7],
        compound_idx_cdf: [[0; 1]; 6],
        dummy0: [0; 14],
        palette_y_color_index_cdf: [[0; 8]; 18],
        palette_uv_color_index_cdf: [[0; 8]; 18],
        tx_type_intra0_cdf: [[[0; 8]; 13]; 4],
        tx_type_intra1_cdf: [[[0; 4]; 13]; 4],
        tx_type_inter_cdf: [[[0; 16]; 4]; 2],
        txb_skip_cdf: [[[0; 1]; 13]; 5],
        eob_extra_cdf: [[[[0; 1]; 9]; 2]; 5],
        dummy_: [0; 5],
        eob_flag_cdf16: [[[0; 4]; 2]; 2],
        eob_flag_cdf32: [[[0; 8]; 2]; 2],
        eob_flag_cdf64: [[[0; 8]; 2]; 2],
        eob_flag_cdf128: [[[0; 8]; 2]; 2],
        eob_flag_cdf256: [[[0; 8]; 2]; 2],
        eob_flag_cdf512: [[[0; 16]; 2]; 2],
        eob_flag_cdf1024: [[[0; 16]; 2]; 2],
        coeff_base_eob_cdf: [[[[0; 2]; 4]; 2]; 5],
        coeff_base_cdf: [[[[0; 4]; 42]; 2]; 5],
        dc_sign_cdf: [[[0; 1]; 3]; 2],
        dummy_2: [0; 2],
        coeff_br_cdf: [[[[0; 4]; 21]; 2]; 5],
        dummy2: [0; 16],
    };

    pub fn default_for_q_index(q_index: u32) -> Self {
        from_bytes(Q_TABLES[coefficient_context(q_index)])
    }

    pub fn as_bytes(&self) -> &[u8] {
        as_bytes(self)
    }
}

impl Default for Av1Cdfs {
    fn default() -> Self {
        Self::ZERO
    }
}

impl fmt::Debug for Av1Cdfs {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Av1Cdfs")
            .field("bytes", &self.as_bytes().len())
            .field("fnv1a64", &fnv1a64(self.as_bytes()))
            .finish()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Av1Cdf {
    pub regular: Av1Cdfs,
    pub intrabc_mv: Av1MvCdfs,
}

impl fmt::Debug for Av1Cdf {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Av1Cdf")
            .field("regular", &self.regular)
            .field("intrabc_mv", &self.intrabc_mv)
            .finish()
    }
}

impl Av1Cdf {
    pub fn new(regular: Av1Cdfs, intrabc_mv: Av1MvCdfs) -> Self {
        Self {
            regular,
            intrabc_mv,
        }
    }

    pub fn empty() -> Self {
        Self::new(Av1Cdfs::ZERO, Av1MvCdfs::ZERO)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Av1CdfStore {
    slots: [Av1Cdf; REF_FRAME_COUNT],
}

impl Av1CdfStore {
    pub fn new() -> Self {
        Self {
            slots: [Av1Cdf::empty(); REF_FRAME_COUNT],
        }
    }

    pub fn load(&self, slot: usize) -> &Av1Cdf {
        &self.slots[slot]
    }

    pub fn store(&mut self, refresh_frame_flags: u8, cdf: &Av1Cdf) {
        let trace = std::env::var_os("RK_MPP_AV1_TRACE").is_some();
        if trace {
            eprintln!(
                "av1-cdf-store refresh={refresh_frame_flags:#04x} slot4_before={:02x} source={:02x}",
                self.slots[4].regular.as_bytes()[0],
                cdf.regular.as_bytes()[0],
            );
        }
        for (slot, destination) in self.slots.iter_mut().enumerate() {
            if refresh_frame_flags & (1 << slot) != 0 {
                *destination = *cdf;
            }
        }
        if trace {
            eprintln!(
                "av1-cdf-store-end refresh={refresh_frame_flags:#04x} slot4_after={:02x}",
                self.slots[4].regular.as_bytes()[0],
            );
        }
    }

    /// Section 7.21 `show_existing_frame` with a key frame refreshes all slots.
    pub fn copy_key_frame_to_all_slots(&mut self, slot: usize) {
        let selected = self.slots[slot];
        self.slots.fill(selected);
    }

    /// Apply the hardware-updated CDF buffer after a frame completes.
    ///
    /// The hardware writes intrabc MV probabilities in the regular MV region.
    /// MPP splits that region back into `cdfs_ndvc` and keeps the frame's
    /// previous intrabc table outside it.
    pub fn apply_hardware_update(
        &mut self,
        refresh_frame_flags: u8,
        frame_is_intra: bool,
        hardware: &[u8],
    ) {
        assert_eq!(
            hardware.len(),
            CDF_SIZE,
            "AV1 hardware CDF buffer has the wrong size"
        );
        let destination = self
            .slots
            .iter_mut()
            .enumerate()
            .find_map(|(slot, cdf)| (refresh_frame_flags & (1 << slot) != 0).then_some(cdf));
        let Some(cdf) = destination else {
            return;
        };

        if frame_is_intra {
            let regular = cdf.regular.as_bytes_mut();
            regular[..MV_CDF_OFFSET].copy_from_slice(&hardware[..MV_CDF_OFFSET]);
            regular[MV_CDF_OFFSET..MV_CDF_OFFSET + MV_CDF_SIZE]
                .copy_from_slice(&hardware[MV_CDF_OFFSET..MV_CDF_OFFSET + MV_CDF_SIZE]);
            cdf.intrabc_mv = from_bytes(&hardware[MV_CDF_OFFSET..MV_CDF_OFFSET + MV_CDF_SIZE]);
            cdf.regular.as_bytes_mut()[MV_CDF_OFFSET + MV_CDF_SIZE..]
                .copy_from_slice(&hardware[MV_CDF_OFFSET + MV_CDF_SIZE..]);
        } else {
            cdf.regular.as_bytes_mut().copy_from_slice(hardware);
        }

        // MPP stores once from the selected refresh slot to every flagged slot.
        let updated = *cdf;
        self.store(refresh_frame_flags, &updated);
    }
}

impl Default for Av1CdfStore {
    fn default() -> Self {
        Self::new()
    }
}

impl Av1Cdfs {
    fn as_bytes_mut(&mut self) -> &mut [u8] {
        as_bytes_mut(self)
    }
}

pub fn coefficient_context(q_index: u32) -> usize {
    match q_index {
        0..=20 => 0,
        21..=60 => 1,
        61..=120 => 2,
        _ => 3,
    }
}

fn from_bytes<T>(bytes: &[u8]) -> T {
    assert_eq!(bytes.len(), size_of::<T>());
    // The C snapshots and AV1 hardware ABI are byte-addressed and the
    // include_bytes! arrays only have byte alignment.
    unsafe { ptr::read_unaligned(bytes.as_ptr().cast::<T>()) }
}

fn as_bytes<T>(value: &T) -> &[u8] {
    unsafe { std::slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>()) }
}

fn as_bytes_mut<T>(value: &mut T) -> &mut [u8] {
    unsafe { std::slice::from_raw_parts_mut((value as *mut T).cast::<u8>(), size_of::<T>()) }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    })
}

const _: () = {
    assert!(CDF_SIZE == size_of::<Av1Cdfs>());
    assert!(MV_CDF_OFFSET == offset_of!(Av1Cdfs, mv_cdf));
    assert!(MV_CDF_SIZE == size_of::<Av1MvCdfs>());
};
