//! AV1 default CDF tables, generated from the published specification.
//!
//! `tools/gen_av1_cdf.py` parses the tables section of
//! <https://aomediacodec.github.io/av1-spec/> and emits every non-coefficient
//! `Default_*_Cdf` array in the specification's row-major order. `shape` keeps the
//! declared dimension expressions so a reader can trace a flat index back.
//!
//! 74 tables, 2697 values, FNV-1a 0xd399ff47157afae2.

/// One specification table: the declared shape and its flat row-major values.
pub struct CdfTable {
    pub name: &'static str,
    pub shape: &'static [&'static str],
    pub values: &'static [u16],
}

pub static DEFAULT_Y_MODE_CDF: CdfTable = CdfTable {
    name: "Default_Y_Mode_Cdf",
    shape: &["BLOCK_SIZE_GROUPS", "INTRA_MODES + 1"],
    values: &[
        22801, 23489, 24293, 24756, 25601, 26123, 26606, 27418, 27945, 29228, 29685, 30349, 32768,
        0, 18673, 19845, 22631, 23318, 23950, 24649, 25527, 27364, 28152, 29701, 29984, 30852,
        32768, 0, 19770, 20979, 23396, 23939, 24241, 24654, 25136, 27073, 27830, 29360, 29730,
        30659, 32768, 0, 20155, 21301, 22838, 23178, 23261, 23533, 23703, 24804, 25352, 26575,
        27016, 28049, 32768, 0,
    ],
};

pub static DEFAULT_UV_MODE_CFL_NOT_ALLOWED_CDF: CdfTable = CdfTable {
    name: "Default_Uv_Mode_Cfl_Not_Allowed_Cdf",
    shape: &["INTRA_MODES", "UV_INTRA_MODES_CFL_NOT_ALLOWED + 1"],
    values: &[
        22631, 24152, 25378, 25661, 25986, 26520, 27055, 27923, 28244, 30059, 30941, 31961, 32768,
        0, 9513, 26881, 26973, 27046, 27118, 27664, 27739, 27824, 28359, 29505, 29800, 31796,
        32768, 0, 9845, 9915, 28663, 28704, 28757, 28780, 29198, 29822, 29854, 30764, 31777, 32029,
        32768, 0, 13639, 13897, 14171, 25331, 25606, 25727, 25953, 27148, 28577, 30612, 31355,
        32493, 32768, 0, 9764, 9835, 9930, 9954, 25386, 27053, 27958, 28148, 28243, 31101, 31744,
        32363, 32768, 0, 11825, 13589, 13677, 13720, 15048, 29213, 29301, 29458, 29711, 31161,
        31441, 32550, 32768, 0, 14175, 14399, 16608, 16821, 17718, 17775, 28551, 30200, 30245,
        31837, 32342, 32667, 32768, 0, 12885, 13038, 14978, 15590, 15673, 15748, 16176, 29128,
        29267, 30643, 31961, 32461, 32768, 0, 12026, 13661, 13874, 15305, 15490, 15726, 15995,
        16273, 28443, 30388, 30767, 32416, 32768, 0, 19052, 19840, 20579, 20916, 21150, 21467,
        21885, 22719, 23174, 28861, 30379, 32175, 32768, 0, 18627, 19649, 20974, 21219, 21492,
        21816, 22199, 23119, 23527, 27053, 31397, 32148, 32768, 0, 17026, 19004, 19997, 20339,
        20586, 21103, 21349, 21907, 22482, 25896, 26541, 31819, 32768, 0, 12124, 13759, 14959,
        14992, 15007, 15051, 15078, 15166, 15255, 15753, 16039, 16606, 32768, 0,
    ],
};

pub static DEFAULT_UV_MODE_CFL_ALLOWED_CDF: CdfTable = CdfTable {
    name: "Default_Uv_Mode_Cfl_Allowed_Cdf",
    shape: &["INTRA_MODES", "UV_INTRA_MODES_CFL_ALLOWED + 1"],
    values: &[
        10407, 11208, 12900, 13181, 13823, 14175, 14899, 15656, 15986, 20086, 20995, 22455, 24212,
        32768, 0, 4532, 19780, 20057, 20215, 20428, 21071, 21199, 21451, 22099, 24228, 24693,
        27032, 29472, 32768, 0, 5273, 5379, 20177, 20270, 20385, 20439, 20949, 21695, 21774, 23138,
        24256, 24703, 26679, 32768, 0, 6740, 7167, 7662, 14152, 14536, 14785, 15034, 16741, 18371,
        21520, 22206, 23389, 24182, 32768, 0, 4987, 5368, 5928, 6068, 19114, 20315, 21857, 22253,
        22411, 24911, 25380, 26027, 26376, 32768, 0, 5370, 6889, 7247, 7393, 9498, 21114, 21402,
        21753, 21981, 24780, 25386, 26517, 27176, 32768, 0, 4816, 4961, 7204, 7326, 8765, 8930,
        20169, 20682, 20803, 23188, 23763, 24455, 24940, 32768, 0, 6608, 6740, 8529, 9049, 9257,
        9356, 9735, 18827, 19059, 22336, 23204, 23964, 24793, 32768, 0, 5998, 7419, 7781, 8933,
        9255, 9549, 9753, 10417, 18898, 22494, 23139, 24764, 25989, 32768, 0, 10660, 11298, 12550,
        12957, 13322, 13624, 14040, 15004, 15534, 20714, 21789, 23443, 24861, 32768, 0, 10522,
        11530, 12552, 12963, 13378, 13779, 14245, 15235, 15902, 20102, 22696, 23774, 25838, 32768,
        0, 10099, 10691, 12639, 13049, 13386, 13665, 14125, 15163, 15636, 19676, 20474, 23519,
        25208, 32768, 0, 3144, 5087, 7382, 7504, 7593, 7690, 7801, 8064, 8232, 9248, 9875, 10521,
        29048, 32768, 0,
    ],
};

pub static DEFAULT_ANGLE_DELTA_CDF: CdfTable = CdfTable {
    name: "Default_Angle_Delta_Cdf",
    shape: &["DIRECTIONAL_MODES", "( 2 * MAX_ANGLE_DELTA + 1 ) + 1"],
    values: &[
        2180, 5032, 7567, 22776, 26989, 30217, 32768, 0, 2301, 5608, 8801, 23487, 26974, 30330,
        32768, 0, 3780, 11018, 13699, 19354, 23083, 31286, 32768, 0, 4581, 11226, 15147, 17138,
        21834, 28397, 32768, 0, 1737, 10927, 14509, 19588, 22745, 28823, 32768, 0, 2664, 10176,
        12485, 17650, 21600, 30495, 32768, 0, 2240, 11096, 15453, 20341, 22561, 28917, 32768, 0,
        3605, 10428, 12459, 17676, 21244, 30655, 32768, 0,
    ],
};

pub static DEFAULT_INTRABC_CDF: CdfTable = CdfTable {
    name: "Default_Intrabc_Cdf",
    shape: &["2 + 1"],
    values: &[30531, 32768, 0],
};

pub static DEFAULT_PARTITION_W8_CDF: CdfTable = CdfTable {
    name: "Default_Partition_W8_Cdf",
    shape: &["PARTITION_CONTEXTS", "5"],
    values: &[
        19132, 25510, 30392, 32768, 0, 13928, 19855, 28540, 32768, 0, 12522, 23679, 28629, 32768,
        0, 9896, 18783, 25853, 32768, 0,
    ],
};

pub static DEFAULT_PARTITION_W16_CDF: CdfTable = CdfTable {
    name: "Default_Partition_W16_Cdf",
    shape: &["PARTITION_CONTEXTS", "11"],
    values: &[
        15597, 20929, 24571, 26706, 27664, 28821, 29601, 30571, 31902, 32768, 0, 7925, 11043,
        16785, 22470, 23971, 25043, 26651, 28701, 29834, 32768, 0, 5414, 13269, 15111, 20488,
        22360, 24500, 25537, 26336, 32117, 32768, 0, 2662, 6362, 8614, 20860, 23053, 24778, 26436,
        27829, 31171, 32768, 0,
    ],
};

pub static DEFAULT_PARTITION_W32_CDF: CdfTable = CdfTable {
    name: "Default_Partition_W32_Cdf",
    shape: &["PARTITION_CONTEXTS", "11"],
    values: &[
        18462, 20920, 23124, 27647, 28227, 29049, 29519, 30178, 31544, 32768, 0, 7689, 9060, 12056,
        24992, 25660, 26182, 26951, 28041, 29052, 32768, 0, 6015, 9009, 10062, 24544, 25409, 26545,
        27071, 27526, 32047, 32768, 0, 1394, 2208, 2796, 28614, 29061, 29466, 29840, 30185, 31899,
        32768, 0,
    ],
};

pub static DEFAULT_PARTITION_W64_CDF: CdfTable = CdfTable {
    name: "Default_Partition_W64_Cdf",
    shape: &["PARTITION_CONTEXTS", "11"],
    values: &[
        20137, 21547, 23078, 29566, 29837, 30261, 30524, 30892, 31724, 32768, 0, 6732, 7490, 9497,
        27944, 28250, 28515, 28969, 29630, 30104, 32768, 0, 5945, 7663, 8348, 28683, 29117, 29749,
        30064, 30298, 32238, 32768, 0, 870, 1212, 1487, 31198, 31394, 31574, 31743, 31881, 32332,
        32768, 0,
    ],
};

pub static DEFAULT_PARTITION_W128_CDF: CdfTable = CdfTable {
    name: "Default_Partition_W128_Cdf",
    shape: &["PARTITION_CONTEXTS", "9"],
    values: &[
        27899, 28219, 28529, 32484, 32539, 32619, 32639, 32768, 0, 6607, 6990, 8268, 32060, 32219,
        32338, 32371, 32768, 0, 5429, 6676, 7122, 32027, 32227, 32531, 32582, 32768, 0, 711, 966,
        1172, 32448, 32538, 32617, 32664, 32768, 0,
    ],
};

pub static DEFAULT_TX_8X8_CDF: CdfTable = CdfTable {
    name: "Default_Tx_8x8_Cdf",
    shape: &["TX_SIZE_CONTEXTS", "MAX_TX_DEPTH + 1"],
    values: &[19968, 32768, 0, 19968, 32768, 0, 24320, 32768, 0],
};

pub static DEFAULT_TX_16X16_CDF: CdfTable = CdfTable {
    name: "Default_Tx_16x16_Cdf",
    shape: &["TX_SIZE_CONTEXTS", "MAX_TX_DEPTH + 2"],
    values: &[
        12272, 30172, 32768, 0, 12272, 30172, 32768, 0, 18677, 30848, 32768, 0,
    ],
};

pub static DEFAULT_TX_32X32_CDF: CdfTable = CdfTable {
    name: "Default_Tx_32x32_Cdf",
    shape: &["TX_SIZE_CONTEXTS", "MAX_TX_DEPTH + 2"],
    values: &[
        12986, 15180, 32768, 0, 12986, 15180, 32768, 0, 24302, 25602, 32768, 0,
    ],
};

pub static DEFAULT_TX_64X64_CDF: CdfTable = CdfTable {
    name: "Default_Tx_64x64_Cdf",
    shape: &["TX_SIZE_CONTEXTS", "MAX_TX_DEPTH + 2"],
    values: &[
        5782, 11475, 32768, 0, 5782, 11475, 32768, 0, 16803, 22759, 32768, 0,
    ],
};

pub static DEFAULT_TXFM_SPLIT_CDF: CdfTable = CdfTable {
    name: "Default_Txfm_Split_Cdf",
    shape: &["TXFM_PARTITION_CONTEXTS", "3"],
    values: &[
        28581, 32768, 0, 23846, 32768, 0, 20847, 32768, 0, 24315, 32768, 0, 18196, 32768, 0, 12133,
        32768, 0, 18791, 32768, 0, 10887, 32768, 0, 11005, 32768, 0, 27179, 32768, 0, 20004, 32768,
        0, 11281, 32768, 0, 26549, 32768, 0, 19308, 32768, 0, 14224, 32768, 0, 28015, 32768, 0,
        21546, 32768, 0, 14400, 32768, 0, 28165, 32768, 0, 22401, 32768, 0, 16088, 32768, 0,
    ],
};

pub static DEFAULT_FILTER_INTRA_MODE_CDF: CdfTable = CdfTable {
    name: "Default_Filter_Intra_Mode_Cdf",
    shape: &["6"],
    values: &[8949, 12776, 17211, 29558, 32768, 0],
};

pub static DEFAULT_FILTER_INTRA_CDF: CdfTable = CdfTable {
    name: "Default_Filter_Intra_Cdf",
    shape: &["BLOCK_SIZES", "3"],
    values: &[
        4621, 32768, 0, 6743, 32768, 0, 5893, 32768, 0, 7866, 32768, 0, 12551, 32768, 0, 9394,
        32768, 0, 12408, 32768, 0, 14301, 32768, 0, 12756, 32768, 0, 22343, 32768, 0, 16384, 32768,
        0, 16384, 32768, 0, 16384, 32768, 0, 16384, 32768, 0, 16384, 32768, 0, 16384, 32768, 0,
        12770, 32768, 0, 10368, 32768, 0, 20229, 32768, 0, 18101, 32768, 0, 16384, 32768, 0, 16384,
        32768, 0,
    ],
};

pub static DEFAULT_SEGMENT_ID_CDF: CdfTable = CdfTable {
    name: "Default_Segment_Id_Cdf",
    shape: &["SEGMENT_ID_CONTEXTS", "MAX_SEGMENTS + 1"],
    values: &[
        5622, 7893, 16093, 18233, 27809, 28373, 32533, 32768, 0, 14274, 18230, 22557, 24935, 29980,
        30851, 32344, 32768, 0, 27527, 28487, 28723, 28890, 32397, 32647, 32679, 32768, 0,
    ],
};

pub static DEFAULT_SEGMENT_ID_PREDICTED_CDF: CdfTable = CdfTable {
    name: "Default_Segment_Id_Predicted_Cdf",
    shape: &["SEGMENT_ID_PREDICTED_CONTEXTS", "3"],
    values: &[128, 128, 32768, 0, 128, 128, 32768, 0, 128, 128, 32768, 0],
};

pub static DEFAULT_MV_CLASS0_HP_CDF: CdfTable = CdfTable {
    name: "Default_Mv_Class0_Hp_Cdf",
    shape: &["3"],
    values: &[160, 128, 32768, 0],
};

pub static DEFAULT_MV_HP_CDF: CdfTable = CdfTable {
    name: "Default_Mv_Hp_Cdf",
    shape: &["3"],
    values: &[128, 128, 32768, 0],
};

pub static DEFAULT_MV_SIGN_CDF: CdfTable = CdfTable {
    name: "Default_Mv_Sign_Cdf",
    shape: &["3"],
    values: &[128, 128, 32768, 0],
};

pub static DEFAULT_MV_BIT_CDF: CdfTable = CdfTable {
    name: "Default_Mv_Bit_Cdf",
    shape: &["MV_OFFSET_BITS", "3"],
    values: &[
        136, 128, 32768, 0, 140, 128, 32768, 0, 148, 128, 32768, 0, 160, 128, 32768, 0, 176, 128,
        32768, 0, 192, 128, 32768, 0, 224, 128, 32768, 0, 234, 128, 32768, 0, 234, 128, 32768, 0,
        240, 128, 32768, 0,
    ],
};

pub static DEFAULT_MV_CLASS0_BIT_CDF: CdfTable = CdfTable {
    name: "Default_Mv_Class0_Bit_Cdf",
    shape: &["3"],
    values: &[216, 128, 32768, 0],
};

pub static DEFAULT_NEW_MV_CDF: CdfTable = CdfTable {
    name: "Default_New_Mv_Cdf",
    shape: &["NEW_MV_CONTEXTS", "3"],
    values: &[
        24035, 32768, 0, 16630, 32768, 0, 15339, 32768, 0, 8386, 32768, 0, 12222, 32768, 0, 4676,
        32768, 0,
    ],
};

pub static DEFAULT_ZERO_MV_CDF: CdfTable = CdfTable {
    name: "Default_Zero_Mv_Cdf",
    shape: &["ZERO_MV_CONTEXTS", "3"],
    values: &[2175, 32768, 0, 1054, 32768, 0],
};

pub static DEFAULT_REF_MV_CDF: CdfTable = CdfTable {
    name: "Default_Ref_Mv_Cdf",
    shape: &["REF_MV_CONTEXTS", "3"],
    values: &[
        23974, 32768, 0, 24188, 32768, 0, 17848, 32768, 0, 28622, 32768, 0, 24312, 32768, 0, 19923,
        32768, 0,
    ],
};

pub static DEFAULT_DRL_MODE_CDF: CdfTable = CdfTable {
    name: "Default_Drl_Mode_Cdf",
    shape: &["DRL_MODE_CONTEXTS", "3"],
    values: &[13104, 32768, 0, 24560, 32768, 0, 18945, 32768, 0],
};

pub static DEFAULT_IS_INTER_CDF: CdfTable = CdfTable {
    name: "Default_Is_Inter_Cdf",
    shape: &["IS_INTER_CONTEXTS", "3"],
    values: &[
        806, 32768, 0, 16662, 32768, 0, 20186, 32768, 0, 26538, 32768, 0,
    ],
};

pub static DEFAULT_COMP_MODE_CDF: CdfTable = CdfTable {
    name: "Default_Comp_Mode_Cdf",
    shape: &["COMP_INTER_CONTEXTS", "3"],
    values: &[
        26828, 32768, 0, 24035, 32768, 0, 12031, 32768, 0, 10640, 32768, 0, 2901, 32768, 0,
    ],
};

pub static DEFAULT_SKIP_MODE_CDF: CdfTable = CdfTable {
    name: "Default_Skip_Mode_Cdf",
    shape: &["SKIP_MODE_CONTEXTS", "3"],
    values: &[32621, 32768, 0, 20708, 32768, 0, 8127, 32768, 0],
};

pub static DEFAULT_SKIP_CDF: CdfTable = CdfTable {
    name: "Default_Skip_Cdf",
    shape: &["SKIP_CONTEXTS", "3"],
    values: &[31671, 32768, 0, 16515, 32768, 0, 4576, 32768, 0],
};

pub static DEFAULT_COMPOUND_MODE_CDF: CdfTable = CdfTable {
    name: "Default_Compound_Mode_Cdf",
    shape: &["COMPOUND_MODE_CONTEXTS", "COMPOUND_MODES + 1"],
    values: &[
        7760, 13823, 15808, 17641, 19156, 20666, 26891, 32768, 0, 10730, 19452, 21145, 22749,
        24039, 25131, 28724, 32768, 0, 10664, 20221, 21588, 22906, 24295, 25387, 28436, 32768, 0,
        13298, 16984, 20471, 24182, 25067, 25736, 26422, 32768, 0, 18904, 23325, 25242, 27432,
        27898, 28258, 30758, 32768, 0, 10725, 17454, 20124, 22820, 24195, 25168, 26046, 32768, 0,
        17125, 24273, 25814, 27492, 28214, 28704, 30592, 32768, 0, 13046, 23214, 24505, 25942,
        27435, 28442, 29330, 32768, 0,
    ],
};

pub static DEFAULT_INTERP_FILTER_CDF: CdfTable = CdfTable {
    name: "Default_Interp_Filter_Cdf",
    shape: &["INTERP_FILTER_CONTEXTS", "INTERP_FILTERS + 1"],
    values: &[
        31935, 32720, 32768, 0, 5568, 32719, 32768, 0, 422, 2938, 32768, 0, 28244, 32608, 32768, 0,
        31206, 31953, 32768, 0, 4862, 32121, 32768, 0, 770, 1152, 32768, 0, 20889, 25637, 32768, 0,
        31910, 32724, 32768, 0, 4120, 32712, 32768, 0, 305, 2247, 32768, 0, 27403, 32636, 32768, 0,
        31022, 32009, 32768, 0, 2963, 32093, 32768, 0, 601, 943, 32768, 0, 14969, 21398, 32768, 0,
    ],
};

pub static DEFAULT_MOTION_MODE_CDF: CdfTable = CdfTable {
    name: "Default_Motion_Mode_Cdf",
    shape: &["BLOCK_SIZES", "MOTION_MODES + 1"],
    values: &[
        10923, 21845, 32768, 0, 10923, 21845, 32768, 0, 10923, 21845, 32768, 0, 7651, 24760, 32768,
        0, 4738, 24765, 32768, 0, 5391, 25528, 32768, 0, 19419, 26810, 32768, 0, 5123, 23606,
        32768, 0, 11606, 24308, 32768, 0, 26260, 29116, 32768, 0, 20360, 28062, 32768, 0, 21679,
        26830, 32768, 0, 29516, 30701, 32768, 0, 28898, 30397, 32768, 0, 30878, 31335, 32768, 0,
        32507, 32558, 32768, 0, 10923, 21845, 32768, 0, 10923, 21845, 32768, 0, 28799, 31390,
        32768, 0, 26431, 30774, 32768, 0, 28973, 31594, 32768, 0, 29742, 31203, 32768, 0,
    ],
};

pub static DEFAULT_MV_JOINT_CDF: CdfTable = CdfTable {
    name: "Default_Mv_Joint_Cdf",
    shape: &["MV_JOINTS + 1"],
    values: &[4096, 11264, 19328, 32768, 0],
};

pub static DEFAULT_MV_CLASS_CDF: CdfTable = CdfTable {
    name: "Default_Mv_Class_Cdf",
    shape: &["2", "MV_CLASSES + 1"],
    values: &[
        28672, 30976, 31858, 32320, 32551, 32656, 32740, 32757, 32762, 32767, 32768, 0, 28672,
        30976, 31858, 32320, 32551, 32656, 32740, 32757, 32762, 32767, 32768, 0,
    ],
};

pub static DEFAULT_MV_FR_CDF: CdfTable = CdfTable {
    name: "Default_Mv_Fr_Cdf",
    shape: &["2", "MV_JOINTS + 1"],
    values: &[8192, 17408, 21248, 32768, 0, 8192, 17408, 21248, 32768, 0],
};

pub static DEFAULT_PALETTE_Y_SIZE_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Y_Size_Cdf",
    shape: &["PALETTE_BLOCK_SIZE_CONTEXTS", "PALETTE_SIZES + 1"],
    values: &[
        7952, 13000, 18149, 21478, 25527, 29241, 32768, 0, 7139, 11421, 16195, 19544, 23666, 28073,
        32768, 0, 7788, 12741, 17325, 20500, 24315, 28530, 32768, 0, 8271, 14064, 18246, 21564,
        25071, 28533, 32768, 0, 12725, 19180, 21863, 24839, 27535, 30120, 32768, 0, 9711, 14888,
        16923, 21052, 25661, 27875, 32768, 0, 14940, 20797, 21678, 24186, 27033, 28999, 32768, 0,
    ],
};

pub static DEFAULT_PALETTE_UV_SIZE_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Uv_Size_Cdf",
    shape: &["PALETTE_BLOCK_SIZE_CONTEXTS", "PALETTE_SIZES + 1"],
    values: &[
        8713, 19979, 27128, 29609, 31331, 32272, 32768, 0, 5839, 15573, 23581, 26947, 29848, 31700,
        32768, 0, 4426, 11260, 17999, 21483, 25863, 29430, 32768, 0, 3228, 9464, 14993, 18089,
        22523, 27420, 32768, 0, 3768, 8886, 13091, 17852, 22495, 27207, 32768, 0, 2464, 8451,
        12861, 21632, 25525, 28555, 32768, 0, 1269, 5435, 10433, 18963, 21700, 25865, 32768, 0,
    ],
};

pub static DEFAULT_PALETTE_SIZE_2_Y_COLOR_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Size_2_Y_Color_Cdf",
    shape: &["PALETTE_COLOR_CONTEXTS", "3"],
    values: &[
        28710, 32768, 0, 16384, 32768, 0, 10553, 32768, 0, 27036, 32768, 0, 31603, 32768, 0,
    ],
};

pub static DEFAULT_PALETTE_SIZE_3_Y_COLOR_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Size_3_Y_Color_Cdf",
    shape: &["PALETTE_COLOR_CONTEXTS", "4"],
    values: &[
        27877, 30490, 32768, 0, 11532, 25697, 32768, 0, 6544, 30234, 32768, 0, 23018, 28072, 32768,
        0, 31915, 32385, 32768, 0,
    ],
};

pub static DEFAULT_PALETTE_SIZE_4_Y_COLOR_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Size_4_Y_Color_Cdf",
    shape: &["PALETTE_COLOR_CONTEXTS", "5"],
    values: &[
        25572, 28046, 30045, 32768, 0, 9478, 21590, 27256, 32768, 0, 7248, 26837, 29824, 32768, 0,
        19167, 24486, 28349, 32768, 0, 31400, 31825, 32250, 32768, 0,
    ],
};

pub static DEFAULT_PALETTE_SIZE_5_Y_COLOR_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Size_5_Y_Color_Cdf",
    shape: &["PALETTE_COLOR_CONTEXTS", "6"],
    values: &[
        24779, 26955, 28576, 30282, 32768, 0, 8669, 20364, 24073, 28093, 32768, 0, 4255, 27565,
        29377, 31067, 32768, 0, 19864, 23674, 26716, 29530, 32768, 0, 31646, 31893, 32147, 32426,
        32768, 0,
    ],
};

pub static DEFAULT_PALETTE_SIZE_6_Y_COLOR_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Size_6_Y_Color_Cdf",
    shape: &["PALETTE_COLOR_CONTEXTS", "7"],
    values: &[
        23132, 25407, 26970, 28435, 30073, 32768, 0, 7443, 17242, 20717, 24762, 27982, 32768, 0,
        6300, 24862, 26944, 28784, 30671, 32768, 0, 18916, 22895, 25267, 27435, 29652, 32768, 0,
        31270, 31550, 31808, 32059, 32353, 32768, 0,
    ],
};

pub static DEFAULT_PALETTE_SIZE_7_Y_COLOR_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Size_7_Y_Color_Cdf",
    shape: &["PALETTE_COLOR_CONTEXTS", "8"],
    values: &[
        23105, 25199, 26464, 27684, 28931, 30318, 32768, 0, 6950, 15447, 18952, 22681, 25567,
        28563, 32768, 0, 7560, 23474, 25490, 27203, 28921, 30708, 32768, 0, 18544, 22373, 24457,
        26195, 28119, 30045, 32768, 0, 31198, 31451, 31670, 31882, 32123, 32391, 32768, 0,
    ],
};

pub static DEFAULT_PALETTE_SIZE_8_Y_COLOR_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Size_8_Y_Color_Cdf",
    shape: &["PALETTE_COLOR_CONTEXTS", "9"],
    values: &[
        21689, 23883, 25163, 26352, 27506, 28827, 30195, 32768, 0, 6892, 15385, 17840, 21606,
        24287, 26753, 29204, 32768, 0, 5651, 23182, 25042, 26518, 27982, 29392, 30900, 32768, 0,
        19349, 22578, 24418, 25994, 27524, 29031, 30448, 32768, 0, 31028, 31270, 31504, 31705,
        31927, 32153, 32392, 32768, 0,
    ],
};

pub static DEFAULT_PALETTE_SIZE_2_UV_COLOR_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Size_2_Uv_Color_Cdf",
    shape: &["PALETTE_COLOR_CONTEXTS", "3"],
    values: &[
        29089, 32768, 0, 16384, 32768, 0, 8713, 32768, 0, 29257, 32768, 0, 31610, 32768, 0,
    ],
};

pub static DEFAULT_PALETTE_SIZE_3_UV_COLOR_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Size_3_Uv_Color_Cdf",
    shape: &["PALETTE_COLOR_CONTEXTS", "4"],
    values: &[
        25257, 29145, 32768, 0, 12287, 27293, 32768, 0, 7033, 27960, 32768, 0, 20145, 25405, 32768,
        0, 30608, 31639, 32768, 0,
    ],
};

pub static DEFAULT_PALETTE_SIZE_4_UV_COLOR_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Size_4_Uv_Color_Cdf",
    shape: &["PALETTE_COLOR_CONTEXTS", "5"],
    values: &[
        24210, 27175, 29903, 32768, 0, 9888, 22386, 27214, 32768, 0, 5901, 26053, 29293, 32768, 0,
        18318, 22152, 28333, 32768, 0, 30459, 31136, 31926, 32768, 0,
    ],
};

pub static DEFAULT_PALETTE_SIZE_5_UV_COLOR_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Size_5_Uv_Color_Cdf",
    shape: &["PALETTE_COLOR_CONTEXTS", "6"],
    values: &[
        22980, 25479, 27781, 29986, 32768, 0, 8413, 21408, 24859, 28874, 32768, 0, 2257, 29449,
        30594, 31598, 32768, 0, 19189, 21202, 25915, 28620, 32768, 0, 31844, 32044, 32281, 32518,
        32768, 0,
    ],
};

pub static DEFAULT_PALETTE_SIZE_6_UV_COLOR_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Size_6_Uv_Color_Cdf",
    shape: &["PALETTE_COLOR_CONTEXTS", "7"],
    values: &[
        22217, 24567, 26637, 28683, 30548, 32768, 0, 7307, 16406, 19636, 24632, 28424, 32768, 0,
        4441, 25064, 26879, 28942, 30919, 32768, 0, 17210, 20528, 23319, 26750, 29582, 32768, 0,
        30674, 30953, 31396, 31735, 32207, 32768, 0,
    ],
};

pub static DEFAULT_PALETTE_SIZE_7_UV_COLOR_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Size_7_Uv_Color_Cdf",
    shape: &["PALETTE_COLOR_CONTEXTS", "8"],
    values: &[
        21239, 23168, 25044, 26962, 28705, 30506, 32768, 0, 6545, 15012, 18004, 21817, 25503,
        28701, 32768, 0, 3448, 26295, 27437, 28704, 30126, 31442, 32768, 0, 15889, 18323, 21704,
        24698, 26976, 29690, 32768, 0, 30988, 31204, 31479, 31734, 31983, 32325, 32768, 0,
    ],
};

pub static DEFAULT_PALETTE_SIZE_8_UV_COLOR_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Size_8_Uv_Color_Cdf",
    shape: &["PALETTE_COLOR_CONTEXTS", "9"],
    values: &[
        21442, 23288, 24758, 26246, 27649, 28980, 30563, 32768, 0, 5863, 14933, 17552, 20668,
        23683, 26411, 29273, 32768, 0, 3415, 25810, 26877, 27990, 29223, 30394, 31618, 32768, 0,
        17965, 20084, 22232, 23974, 26274, 28402, 30390, 32768, 0, 31190, 31329, 31516, 31679,
        31825, 32026, 32322, 32768, 0,
    ],
};

pub static DEFAULT_PALETTE_UV_MODE_CDF: CdfTable = CdfTable {
    name: "Default_Palette_Uv_Mode_Cdf",
    shape: &["PALETTE_UV_MODE_CONTEXTS", "3"],
    values: &[32461, 32768, 0, 21488, 32768, 0],
};

pub static DEFAULT_DELTA_Q_CDF: CdfTable = CdfTable {
    name: "Default_Delta_Q_Cdf",
    shape: &["DELTA_Q_SMALL + 2"],
    values: &[28160, 32120, 32677, 32768, 0],
};

pub static DEFAULT_DELTA_LF_CDF: CdfTable = CdfTable {
    name: "Default_Delta_Lf_Cdf",
    shape: &["DELTA_LF_SMALL + 2"],
    values: &[28160, 32120, 32677, 32768, 0],
};

pub static DEFAULT_INTER_TX_TYPE_SET1_CDF: CdfTable = CdfTable {
    name: "Default_Inter_Tx_Type_Set1_Cdf",
    shape: &["2", "17"],
    values: &[
        4458, 5560, 7695, 9709, 13330, 14789, 17537, 20266, 21504, 22848, 23934, 25474, 27727,
        28915, 30631, 32768, 0, 1645, 2573, 4778, 5711, 7807, 8622, 10522, 15357, 17674, 20408,
        22517, 25010, 27116, 28856, 30749, 32768, 0,
    ],
};

pub static DEFAULT_INTER_TX_TYPE_SET2_CDF: CdfTable = CdfTable {
    name: "Default_Inter_Tx_Type_Set2_Cdf",
    shape: &["13"],
    values: &[
        770, 2421, 5225, 12907, 15819, 18927, 21561, 24089, 26595, 28526, 30529, 32768, 0,
    ],
};

pub static DEFAULT_INTER_TX_TYPE_SET3_CDF: CdfTable = CdfTable {
    name: "Default_Inter_Tx_Type_Set3_Cdf",
    shape: &["4", "3"],
    values: &[
        16384, 32768, 0, 4167, 32768, 0, 1998, 32768, 0, 748, 32768, 0,
    ],
};

pub static DEFAULT_COMPOUND_IDX_CDF: CdfTable = CdfTable {
    name: "Default_Compound_Idx_Cdf",
    shape: &["COMPOUND_IDX_CONTEXTS", "3"],
    values: &[
        18244, 32768, 0, 12865, 32768, 0, 7053, 32768, 0, 13259, 32768, 0, 9334, 32768, 0, 4644,
        32768, 0,
    ],
};

pub static DEFAULT_COMP_GROUP_IDX_CDF: CdfTable = CdfTable {
    name: "Default_Comp_Group_Idx_Cdf",
    shape: &["COMP_GROUP_IDX_CONTEXTS", "3"],
    values: &[
        26607, 32768, 0, 22891, 32768, 0, 18840, 32768, 0, 24594, 32768, 0, 19934, 32768, 0, 22674,
        32768, 0,
    ],
};

pub static DEFAULT_COMPOUND_TYPE_CDF: CdfTable = CdfTable {
    name: "Default_Compound_Type_Cdf",
    shape: &["BLOCK_SIZES", "COMPOUND_TYPES + 1"],
    values: &[
        16384, 32768, 0, 16384, 32768, 0, 16384, 32768, 0, 23431, 32768, 0, 13171, 32768, 0, 11470,
        32768, 0, 9770, 32768, 0, 9100, 32768, 0, 8233, 32768, 0, 6172, 32768, 0, 16384, 32768, 0,
        16384, 32768, 0, 16384, 32768, 0, 16384, 32768, 0, 16384, 32768, 0, 16384, 32768, 0, 16384,
        32768, 0, 16384, 32768, 0, 11820, 32768, 0, 7701, 32768, 0, 16384, 32768, 0, 16384, 32768,
        0,
    ],
};

pub static DEFAULT_INTER_INTRA_CDF: CdfTable = CdfTable {
    name: "Default_Inter_Intra_Cdf",
    shape: &["BLOCK_SIZE_GROUPS - 1", "3"],
    values: &[26887, 32768, 0, 27597, 32768, 0, 30237, 32768, 0],
};

pub static DEFAULT_INTER_INTRA_MODE_CDF: CdfTable = CdfTable {
    name: "Default_Inter_Intra_Mode_Cdf",
    shape: &["BLOCK_SIZE_GROUPS - 1", "INTERINTRA_MODES + 1"],
    values: &[
        1875, 11082, 27332, 32768, 0, 2473, 9996, 26388, 32768, 0, 4238, 11537, 25926, 32768, 0,
    ],
};

pub static DEFAULT_WEDGE_INDEX_CDF: CdfTable = CdfTable {
    name: "Default_Wedge_Index_Cdf",
    shape: &["BLOCK_SIZES", "16 + 1"],
    values: &[
        2048, 4096, 6144, 8192, 10240, 12288, 14336, 16384, 18432, 20480, 22528, 24576, 26624,
        28672, 30720, 32768, 0, 2048, 4096, 6144, 8192, 10240, 12288, 14336, 16384, 18432, 20480,
        22528, 24576, 26624, 28672, 30720, 32768, 0, 2048, 4096, 6144, 8192, 10240, 12288, 14336,
        16384, 18432, 20480, 22528, 24576, 26624, 28672, 30720, 32768, 0, 2438, 4440, 6599, 8663,
        11005, 12874, 15751, 18094, 20359, 22362, 24127, 25702, 27752, 29450, 31171, 32768, 0, 806,
        3266, 6005, 6738, 7218, 7367, 7771, 14588, 16323, 17367, 18452, 19422, 22839, 26127, 29629,
        32768, 0, 2779, 3738, 4683, 7213, 7775, 8017, 8655, 14357, 17939, 21332, 24520, 27470,
        29456, 30529, 31656, 32768, 0, 1684, 3625, 5675, 7108, 9302, 11274, 14429, 17144, 19163,
        20961, 22884, 24471, 26719, 28714, 30877, 32768, 0, 1142, 3491, 6277, 7314, 8089, 8355,
        9023, 13624, 15369, 16730, 18114, 19313, 22521, 26012, 29550, 32768, 0, 2742, 4195, 5727,
        8035, 8980, 9336, 10146, 14124, 17270, 20533, 23434, 25972, 27944, 29570, 31416, 32768, 0,
        1727, 3948, 6101, 7796, 9841, 12344, 15766, 18944, 20638, 22038, 23963, 25311, 26988,
        28766, 31012, 32768, 0, 2048, 4096, 6144, 8192, 10240, 12288, 14336, 16384, 18432, 20480,
        22528, 24576, 26624, 28672, 30720, 32768, 0, 2048, 4096, 6144, 8192, 10240, 12288, 14336,
        16384, 18432, 20480, 22528, 24576, 26624, 28672, 30720, 32768, 0, 2048, 4096, 6144, 8192,
        10240, 12288, 14336, 16384, 18432, 20480, 22528, 24576, 26624, 28672, 30720, 32768, 0,
        2048, 4096, 6144, 8192, 10240, 12288, 14336, 16384, 18432, 20480, 22528, 24576, 26624,
        28672, 30720, 32768, 0, 2048, 4096, 6144, 8192, 10240, 12288, 14336, 16384, 18432, 20480,
        22528, 24576, 26624, 28672, 30720, 32768, 0, 2048, 4096, 6144, 8192, 10240, 12288, 14336,
        16384, 18432, 20480, 22528, 24576, 26624, 28672, 30720, 32768, 0, 2048, 4096, 6144, 8192,
        10240, 12288, 14336, 16384, 18432, 20480, 22528, 24576, 26624, 28672, 30720, 32768, 0,
        2048, 4096, 6144, 8192, 10240, 12288, 14336, 16384, 18432, 20480, 22528, 24576, 26624,
        28672, 30720, 32768, 0, 154, 987, 1925, 2051, 2088, 2111, 2151, 23033, 23703, 24284, 24985,
        25684, 27259, 28883, 30911, 32768, 0, 1135, 1322, 1493, 2635, 2696, 2737, 2770, 21016,
        22935, 25057, 27251, 29173, 30089, 30960, 31933, 32768, 0, 2048, 4096, 6144, 8192, 10240,
        12288, 14336, 16384, 18432, 20480, 22528, 24576, 26624, 28672, 30720, 32768, 0, 2048, 4096,
        6144, 8192, 10240, 12288, 14336, 16384, 18432, 20480, 22528, 24576, 26624, 28672, 30720,
        32768, 0,
    ],
};

pub static DEFAULT_WEDGE_INTER_INTRA_CDF: CdfTable = CdfTable {
    name: "Default_Wedge_Inter_Intra_Cdf",
    shape: &["BLOCK_SIZES", "3"],
    values: &[
        16384, 32768, 0, 16384, 32768, 0, 16384, 32768, 0, 20036, 32768, 0, 24957, 32768, 0, 26704,
        32768, 0, 27530, 32768, 0, 29564, 32768, 0, 29444, 32768, 0, 26872, 32768, 0, 16384, 32768,
        0, 16384, 32768, 0, 16384, 32768, 0, 16384, 32768, 0, 16384, 32768, 0, 16384, 32768, 0,
        16384, 32768, 0, 16384, 32768, 0, 16384, 32768, 0, 16384, 32768, 0, 16384, 32768, 0, 16384,
        32768, 0,
    ],
};

pub static DEFAULT_USE_OBMC_CDF: CdfTable = CdfTable {
    name: "Default_Use_Obmc_Cdf",
    shape: &["BLOCK_SIZES", "3"],
    values: &[
        16384, 32768, 0, 16384, 32768, 0, 16384, 32768, 0, 10437, 32768, 0, 9371, 32768, 0, 9301,
        32768, 0, 17432, 32768, 0, 14423, 32768, 0, 15142, 32768, 0, 25817, 32768, 0, 22823, 32768,
        0, 22083, 32768, 0, 30128, 32768, 0, 31014, 32768, 0, 31560, 32768, 0, 32638, 32768, 0,
        16384, 32768, 0, 16384, 32768, 0, 23664, 32768, 0, 20901, 32768, 0, 24008, 32768, 0, 26879,
        32768, 0,
    ],
};

pub static DEFAULT_COMP_REF_TYPE_CDF: CdfTable = CdfTable {
    name: "Default_Comp_Ref_Type_Cdf",
    shape: &["COMP_REF_TYPE_CONTEXTS", "3"],
    values: &[
        1198, 32768, 0, 2070, 32768, 0, 9166, 32768, 0, 7499, 32768, 0, 22475, 32768, 0,
    ],
};

pub static DEFAULT_CFL_SIGN_CDF: CdfTable = CdfTable {
    name: "Default_Cfl_Sign_Cdf",
    shape: &["CFL_JOINT_SIGNS + 1"],
    values: &[1418, 2123, 13340, 18405, 26972, 28343, 32294, 32768, 0],
};

pub static DEFAULT_CFL_ALPHA_CDF: CdfTable = CdfTable {
    name: "Default_Cfl_Alpha_Cdf",
    shape: &["CFL_ALPHA_CONTEXTS", "CFL_ALPHABET_SIZE + 1"],
    values: &[
        7637, 20719, 31401, 32481, 32657, 32688, 32692, 32696, 32700, 32704, 32708, 32712, 32716,
        32720, 32724, 32768, 0, 14365, 23603, 28135, 31168, 32167, 32395, 32487, 32573, 32620,
        32647, 32668, 32672, 32676, 32680, 32684, 32768, 0, 11532, 22380, 28445, 31360, 32349,
        32523, 32584, 32649, 32673, 32677, 32681, 32685, 32689, 32693, 32697, 32768, 0, 26990,
        31402, 32282, 32571, 32692, 32696, 32700, 32704, 32708, 32712, 32716, 32720, 32724, 32728,
        32732, 32768, 0, 17248, 26058, 28904, 30608, 31305, 31877, 32126, 32321, 32394, 32464,
        32516, 32560, 32576, 32593, 32622, 32768, 0, 14738, 21678, 25779, 27901, 29024, 30302,
        30980, 31843, 32144, 32413, 32520, 32594, 32622, 32656, 32660, 32768, 0,
    ],
};

pub static DEFAULT_USE_WIENER_CDF: CdfTable = CdfTable {
    name: "Default_Use_Wiener_Cdf",
    shape: &["2 + 1"],
    values: &[11570, 32768, 0],
};

pub static DEFAULT_USE_SGRPROJ_CDF: CdfTable = CdfTable {
    name: "Default_Use_Sgrproj_Cdf",
    shape: &["2 + 1"],
    values: &[16855, 32768, 0],
};

pub static DEFAULT_RESTORATION_TYPE_CDF: CdfTable = CdfTable {
    name: "Default_Restoration_Type_Cdf",
    shape: &["RESTORE_SWITCHABLE + 1"],
    values: &[9413, 22581, 32768, 0],
};

/// Every generated table, in specification order.
pub static TABLES: [&CdfTable; 74] = [
    &DEFAULT_Y_MODE_CDF,
    &DEFAULT_UV_MODE_CFL_NOT_ALLOWED_CDF,
    &DEFAULT_UV_MODE_CFL_ALLOWED_CDF,
    &DEFAULT_ANGLE_DELTA_CDF,
    &DEFAULT_INTRABC_CDF,
    &DEFAULT_PARTITION_W8_CDF,
    &DEFAULT_PARTITION_W16_CDF,
    &DEFAULT_PARTITION_W32_CDF,
    &DEFAULT_PARTITION_W64_CDF,
    &DEFAULT_PARTITION_W128_CDF,
    &DEFAULT_TX_8X8_CDF,
    &DEFAULT_TX_16X16_CDF,
    &DEFAULT_TX_32X32_CDF,
    &DEFAULT_TX_64X64_CDF,
    &DEFAULT_TXFM_SPLIT_CDF,
    &DEFAULT_FILTER_INTRA_MODE_CDF,
    &DEFAULT_FILTER_INTRA_CDF,
    &DEFAULT_SEGMENT_ID_CDF,
    &DEFAULT_SEGMENT_ID_PREDICTED_CDF,
    &DEFAULT_MV_CLASS0_HP_CDF,
    &DEFAULT_MV_HP_CDF,
    &DEFAULT_MV_SIGN_CDF,
    &DEFAULT_MV_BIT_CDF,
    &DEFAULT_MV_CLASS0_BIT_CDF,
    &DEFAULT_NEW_MV_CDF,
    &DEFAULT_ZERO_MV_CDF,
    &DEFAULT_REF_MV_CDF,
    &DEFAULT_DRL_MODE_CDF,
    &DEFAULT_IS_INTER_CDF,
    &DEFAULT_COMP_MODE_CDF,
    &DEFAULT_SKIP_MODE_CDF,
    &DEFAULT_SKIP_CDF,
    &DEFAULT_COMPOUND_MODE_CDF,
    &DEFAULT_INTERP_FILTER_CDF,
    &DEFAULT_MOTION_MODE_CDF,
    &DEFAULT_MV_JOINT_CDF,
    &DEFAULT_MV_CLASS_CDF,
    &DEFAULT_MV_FR_CDF,
    &DEFAULT_PALETTE_Y_SIZE_CDF,
    &DEFAULT_PALETTE_UV_SIZE_CDF,
    &DEFAULT_PALETTE_SIZE_2_Y_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_3_Y_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_4_Y_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_5_Y_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_6_Y_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_7_Y_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_8_Y_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_2_UV_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_3_UV_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_4_UV_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_5_UV_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_6_UV_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_7_UV_COLOR_CDF,
    &DEFAULT_PALETTE_SIZE_8_UV_COLOR_CDF,
    &DEFAULT_PALETTE_UV_MODE_CDF,
    &DEFAULT_DELTA_Q_CDF,
    &DEFAULT_DELTA_LF_CDF,
    &DEFAULT_INTER_TX_TYPE_SET1_CDF,
    &DEFAULT_INTER_TX_TYPE_SET2_CDF,
    &DEFAULT_INTER_TX_TYPE_SET3_CDF,
    &DEFAULT_COMPOUND_IDX_CDF,
    &DEFAULT_COMP_GROUP_IDX_CDF,
    &DEFAULT_COMPOUND_TYPE_CDF,
    &DEFAULT_INTER_INTRA_CDF,
    &DEFAULT_INTER_INTRA_MODE_CDF,
    &DEFAULT_WEDGE_INDEX_CDF,
    &DEFAULT_WEDGE_INTER_INTRA_CDF,
    &DEFAULT_USE_OBMC_CDF,
    &DEFAULT_COMP_REF_TYPE_CDF,
    &DEFAULT_CFL_SIGN_CDF,
    &DEFAULT_CFL_ALPHA_CDF,
    &DEFAULT_USE_WIENER_CDF,
    &DEFAULT_USE_SGRPROJ_CDF,
    &DEFAULT_RESTORATION_TYPE_CDF,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn fnv(values: &[u16]) -> u64 {
        values
            .iter()
            .fold(0xcbf29ce484222325u64, |mut hash, value| {
                hash ^= u64::from(*value);
                hash.wrapping_mul(0x100000001b3)
            })
    }

    #[test]
    fn the_generated_set_matches_the_specification_extract() {
        assert_eq!(TABLES.len(), 74);
        let total: usize = TABLES.iter().map(|table| table.values.len()).sum();
        assert_eq!(total, 2697);
        let mut hash = 0xcbf29ce484222325u64;
        for table in TABLES {
            for value in table.values {
                hash ^= u64::from(*value);
                hash = hash.wrapping_mul(0x100000001b3);
            }
        }
        assert_eq!(hash, 0xd399ff47157afae2);
    }

    #[test]
    fn partition_w8_starts_with_the_specification_row() {
        assert_eq!(DEFAULT_PARTITION_W8_CDF.shape, &["PARTITION_CONTEXTS", "5"]);
        assert_eq!(
            &DEFAULT_PARTITION_W8_CDF.values[..5],
            &[19132, 25510, 30392, 32768, 0]
        );
    }
}
